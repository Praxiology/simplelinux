//! Physical memory manager: a bitmap frame allocator.
//!
//! We track every 4 KiB physical frame with a single bit (1 = used, 0 = free).
//! The whole physical RAM is mapped in the upper half of the address space by
//! the bootloader (see `BootInfo::physical_memory_offset`), so from now on a
//! physical frame can always be reached at `offset + phys_addr`.
//!
//! Keeping the bitmap as plain bits (rather than a free-list) is the simplest
//! choice for teaching: allocation is "find the first cleared bit".

use core::ops::Range;
use spin::Mutex;
use x86_64::structures::paging::{
    FrameAllocator, FrameDeallocator, PhysFrame, Size4KiB,
};
use x86_64::PhysAddr;

/// Bytes in one page / frame.
pub const PAGE_SIZE: u64 = 4096;

/// Highest physical byte the bitmap can describe. With `-m 128M` we only need
/// 128 MiB, but 1 GiB keeps headroom for larger QEMU memory settings while the
/// bitmap itself stays a modest 128 KiB static array.
pub const MAX_PHYS: usize = 1 << 30; // 1 GiB

/// Total number of 4 KiB frames covered by the bitmap.
pub const NUM_FRAMES: usize = MAX_PHYS / PAGE_SIZE as usize;

/// Number of `u64` words needed to store `NUM_FRAMES` bits.
const WORDS: usize = NUM_FRAMES / 64;

/// A conservative bitmap allocator. All bits start set (`used`), so a frame is
/// only ever handed out after `mark_free` explicitly releases it.
pub struct BitmapFrameAllocator {
    used: [u64; WORDS],
    /// Round-robin hint so repeated allocations don't rescan from frame 0.
    next_word: usize,
}

impl BitmapFrameAllocator {
    /// All frames start out marked used; `init` frees the usable regions.
    pub const fn new() -> Self {
        Self {
            used: [u64::MAX; WORDS],
            next_word: 0,
        }
    }

    fn bit_index(frame: usize) -> (usize, u32) {
        (frame / 64, (frame % 64) as u32)
    }

    fn frame_used(&self, frame: usize) -> bool {
        let (w, b) = Self::bit_index(frame);
        (self.used[w] >> b) & 1 == 1
    }

    fn set_used(&mut self, frame: usize, value: bool) {
        let (w, b) = Self::bit_index(frame);
        if value {
            self.used[w] |= 1 << b;
        } else {
            self.used[w] &= !(1 << b);
        }
    }

    /// Convert a physical byte range to a clamped frame index range.
    fn frames_for(&self, range: Range<u64>) -> Option<Range<usize>> {
        let start = (range.start / PAGE_SIZE) as usize;
        // Round the end up so partial trailing pages are still protected.
        let end = (range.end.div_ceil(PAGE_SIZE)) as usize;
        let end = end.min(NUM_FRAMES);
        if start < end {
            Some(start..end)
        } else {
            None
        }
    }

    pub fn mark_used_range(&mut self, range: Range<u64>) {
        if let Some(frames) = self.frames_for(range) {
            for f in frames {
                self.set_used(f, true);
            }
        }
    }

    pub fn mark_free_range(&mut self, range: Range<u64>) {
        if let Some(frames) = self.frames_for(range) {
            for f in frames {
                self.set_used(f, false);
            }
        }
    }

    /// Find the first free frame, mark it used, and return it.
    fn alloc_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        for wi in 0..WORDS {
            let idx = (self.next_word + wi) % WORDS;
            if self.used[idx] == u64::MAX {
                continue;
            }
            for b in 0..64 {
                let frame = idx * 64 + b;
                if frame >= NUM_FRAMES {
                    return None;
                }
                if !self.frame_used(frame) {
                    self.set_used(frame, true);
                    self.next_word = idx;
                    let addr = PhysAddr::new((frame as u64) * PAGE_SIZE);
                    return Some(PhysFrame::containing_address(addr));
                }
            }
        }
        None
    }

    fn free_frame(&mut self, frame: PhysFrame<Size4KiB>) {
        let idx = (frame.start_address().as_u64() / PAGE_SIZE) as usize;
        if idx < NUM_FRAMES {
            self.set_used(idx, false);
        }
    }

    /// Number of frames currently available.
    pub fn free_count(&self) -> usize {
        let mut used = 0usize;
        for &w in self.used.iter() {
            used += w.count_ones() as usize;
        }
        NUM_FRAMES - used
    }
}

// SAFETY: `alloc_frame` returns each frame at most once because it flips the
// frame's bit to `used` before handing it out.
unsafe impl FrameAllocator<Size4KiB> for BitmapFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        self.alloc_frame()
    }
}

impl FrameDeallocator<Size4KiB> for BitmapFrameAllocator {
    /// # Safety
    /// The caller must guarantee the frame is no longer in use.
    unsafe fn deallocate_frame(&mut self, frame: PhysFrame<Size4KiB>) {
        self.free_frame(frame);
    }
}

/// The single global frame allocator, guarded by a spin lock (UP kernel).
pub static FRAME_ALLOCATOR: Mutex<BitmapFrameAllocator> =
    Mutex::new(BitmapFrameAllocator::new());
