//! Phase 3: memory management.
//!
//! Phase 3：内存管理。子模块自底向上为：
//!   * `pmm`  —— 基于位图的物理帧分配器；
//!   * `vmm`  —— 在当前 4 级页表上做映射的薄封装；
//!   * `heap` —— 架在 linked_list_allocator 上的内核堆。
//!
//! Initialization order matters and is fixed here:
//!   1. record the physical-memory offset (from BootInfo),
//!   2. build the physical frame allocator from the memory map,
//!   3. map and turn on the kernel heap.
//!
//! Nothing that uses `Vec`/`Box`/`String` may run before `init` returns.

pub mod heap;
pub mod pmm;
pub mod vmm;

use bootloader_api::{
    info::{MemoryRegion, MemoryRegionKind},
    BootInfo,
};
use x86_64::VirtAddr;

/// 初始化内存子系统：记录物理内存偏移→构建帧分配器→映射并启用内核堆。
/// Set up physical frame tracking and the kernel heap.
pub fn init(boot_info: &'static mut BootInfo) {
    // 1. Where did the bootloader map all of physical memory?
    let phys_offset = boot_info
        .physical_memory_offset
        .into_option()
        .expect("bootloader did not map physical memory (enable config.mappings.physical_memory)");
    let phys_offset = VirtAddr::new(phys_offset);
    // SAFETY: `phys_offset` comes straight from the bootloader's own BootInfo.
    unsafe { vmm::set_phys_offset(phys_offset) };

    // 2. Seed the frame allocator from the firmware memory map.
    let kernel_start = boot_info.kernel_addr;
    let kernel_end = kernel_start + boot_info.kernel_len;
    let usable_regions = count_usable(&boot_info.memory_regions);
    {
        let mut alloc = pmm::FRAME_ALLOCATOR.lock();
        for region in boot_info.memory_regions.iter() {
            let range = region.start..region.end;
            match region.kind {
                MemoryRegionKind::Usable => alloc.mark_free_range(range),
                _ => alloc.mark_used_range(range),
            }
        }
        // The kernel image may live in a "usable" region per the firmware map,
        // so protect it explicitly, along with the very first page (null guard).
        alloc.mark_used_range(kernel_start..kernel_end);
        alloc.mark_used_range(0..pmm::PAGE_SIZE);
    }

    let free_frames = pmm::FRAME_ALLOCATOR.lock().free_count();
    crate::println!(
        "[memory] phys mem mapped at {:#x}; {} usable regions, {} free frames",
        phys_offset.as_u64(),
        usable_regions,
        free_frames
    );

    // 3. Turn on the heap (this consumes frames from the allocator above).
    // SAFETY: called once, after the frame allocator is ready and before any
    // `alloc` usage elsewhere in the kernel.
    unsafe { heap::init() };
}

fn count_usable(regions: &[MemoryRegion]) -> usize {
    regions
        .iter()
        .filter(|r| matches!(r.kind, MemoryRegionKind::Usable))
        .count()
}
