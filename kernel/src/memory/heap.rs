//! Kernel heap on top of `linked_list_allocator`.
//!
//! We carve out a fixed virtual range (`HEAP_START..HEAP_START + HEAP_SIZE`),
//! back each page with a freshly allocated physical frame, then hand that
//! contiguous region to the allocator. Once `init` returns, the `alloc` crate
//! (`Vec`, `Box`, `String`, ...) is usable kernel-wide.

use linked_list_allocator::LockedHeap;
use x86_64::structures::paging::{Page, Size4KiB};
use x86_64::VirtAddr;

use super::vmm;

/// Virtual address at which the kernel heap starts.
///
/// Chosen well above the physically-mapped region and the kernel image so it
/// will not collide with bootloader mappings.
pub const HEAP_START: u64 = 0xffff_a000_0000_0000;

/// Heap size in bytes (1 MiB == 256 frames).
pub const HEAP_SIZE: usize = 1024 * 1024;

/// The global allocator. `empty()` means "not usable until `init` runs", which
/// is exactly the guarantee Phase 3 relies on (no heap before `memory::init`).
#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

/// Map `HEAP_SIZE` worth of frames and register them as the heap.
///
/// # Safety
/// Must be called exactly once, before any use of the `alloc` types, and after
/// the frame allocator (`pmm`) is initialized.
pub unsafe fn init() {
    let page_count = HEAP_SIZE / 4096;
    let base = VirtAddr::new(HEAP_START);
    let flags = vmm::kernel_data_flags();

    for i in 0..page_count {
        let page = Page::<Size4KiB>::containing_address(base + (i as u64) * 4096);
        vmm::allocate_and_map(page, flags).expect("failed to map heap page");
    }

    ALLOCATOR
        .lock()
        .init(HEAP_START as *mut u8, HEAP_SIZE);

    crate::println!(
        "[heap] {} KiB kernel heap mapped at {:#x}",
        HEAP_SIZE / 1024,
        HEAP_START
    );
}

/// Allocated (in use) heap bytes, for diagnostics.
pub fn used_bytes() -> usize {
    ALLOCATOR.lock().used()
}

/// Free heap bytes, for diagnostics.
pub fn free_bytes() -> usize {
    ALLOCATOR.lock().free()
}

/// Total usable heap size in bytes.
pub fn total_bytes() -> usize {
    ALLOCATOR.lock().size()
}
