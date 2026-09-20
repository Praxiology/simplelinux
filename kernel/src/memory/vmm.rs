//! Virtual memory manager: thin helpers over the active 4-level page table.
//!
//! 虚拟内存管理：在当前生效的 4 级页表（CR3）之上的一组薄封装。借助
//! `OffsetPageTable`（物理内存已在高半区恒等映射）即可逐级遍历页表。我们
//! 不长期持有 `&mut PageTable`（借用检查难以表达），而是每次需要时临时构造
//! 一个映射器——单核内核下这样最简单且足够安全。
//!
//! The bootloader leaves us a valid PML4 in CR3 and maps all of physical memory
//! at `PHYS_OFFSET`. `OffsetPageTable` uses exactly those two facts: it walks the
//! page-table hierarchy by treating every table's *physical* frame as reachable
//! at `PHYS_OFFSET + frame`.
//!
//! We deliberately do *not* keep a long-lived `&mut PageTable` in a global: the
//! borrow checker makes that painful, and on a single-core kernel it is enough
//! to reconstruct a short-lived mapper each time we need one.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use x86_64::registers::control::Cr3;
use x86_64::structures::paging::page_table::PageTableFlags;
use x86_64::structures::paging::{
    FrameAllocator, Mapper, OffsetPageTable, Page, PhysFrame, Size4KiB, Translate,
};
use x86_64::{PhysAddr, VirtAddr};

use super::pmm;

/// Whether `PHYS_OFFSET` has been stored (set once during `memory::init`).
static READY: AtomicBool = AtomicBool::new(false);
/// Virtual address at which physical memory starts (from BootInfo).
static PHYS_OFFSET: AtomicU64 = AtomicU64::new(0);

/// Record the physical-memory offset so the mapper can be built later.
///
/// # Safety
/// `offset` must be the value the bootloader chose for the physical mapping.
pub unsafe fn set_phys_offset(offset: VirtAddr) {
    PHYS_OFFSET.store(offset.as_u64(), Ordering::Relaxed);
    READY.store(true, Ordering::Release);
}

/// The physical-memory offset, or `None` before `memory::init` ran.
pub fn phys_offset() -> Option<VirtAddr> {
    if READY.load(Ordering::Acquire) {
        Some(VirtAddr::new(PHYS_OFFSET.load(Ordering::Relaxed)))
    } else {
        None
    }
}

/// 构造一个绑定到当前生效 PML4（CR3）的 `OffsetPageTable`。
/// Build an `OffsetPageTable` bound to the *currently active* PML4 (CR3).
///
/// # Safety
/// Caller must not create a second overlapping mutable view of the page tables
/// (we only ever call this from non-reentrant init code / single-threaded paths).
pub unsafe fn active_mapper<'a>() -> OffsetPageTable<'a> {
    let offset = phys_offset().expect("physical memory offset not set");
    let (frame, _flags) = Cr3::read();
    let pml4_virt = offset + frame.start_address().as_u64();
    let table = &mut *(pml4_virt.as_mut_ptr());
    OffsetPageTable::new(table, offset)
}

/// 将单个 4 KiB 页 `page` 映射到已选定的物理帧 `frame`（返回中间页表额外消耗的帧数供参考）。
/// Map a single 4 KiB `page` to an already-chosen physical `frame`.
///
/// Returns the number of extra frames consumed for intermediate page tables
/// (0..=3), so callers can reason about allocator pressure.
///
/// # Safety
/// Standard `Mapper::map_to` safety: the page must be unmapped and the frame
/// must not be aliased elsewhere.
pub unsafe fn map_page(
    page: Page<Size4KiB>,
    frame: PhysFrame<Size4KiB>,
    flags: PageTableFlags,
) -> Result<(), MapError> {
    let mut mapper = active_mapper();
    let mut alloc = pmm::FRAME_ALLOCATOR.lock();
    mapper
        .map_to(page, frame, flags, &mut *alloc)
        .map_err(|_| MapError::Map)?
        .flush();
    Ok(())
}

/// Allocate one free frame and map `page` to it with `flags`.
///
/// # Safety
/// See [`map_page`]; additionally the returned frame must be used only through
/// this mapping.
pub unsafe fn allocate_and_map(
    page: Page<Size4KiB>,
    flags: PageTableFlags,
) -> Result<PhysAddr, MapError> {
    let frame = pmm::FRAME_ALLOCATOR.lock().allocate_frame();
    let frame = match frame {
        Some(f) => f,
        None => return Err(MapError::OutOfFrames),
    };
    map_page(page, frame, flags)?;
    Ok(frame.start_address())
}

/// Convert a virtual address to its physical address, if mapped.
pub fn translate(vaddr: VirtAddr) -> Option<PhysAddr> {
    let mapper = unsafe { active_mapper() };
    mapper.translate_addr(vaddr)
}

/// Errors from the mapping helpers.
#[derive(Debug, Clone, Copy)]
pub enum MapError {
    Map,
    OutOfFrames,
}

/// Convenience: default flags for a kernel-writable, non-executable page.
pub fn kernel_data_flags() -> PageTableFlags {
    PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE
}

/// Allocate one free physical frame (4 KiB) without mapping it.
pub fn alloc_frame() -> Option<PhysFrame<Size4KiB>> {
    pmm::FRAME_ALLOCATOR.lock().allocate_frame()
}
