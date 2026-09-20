//! Minimal ELF64 loader for Phase 5.
//!
//! 极简 ELF64 装载器：只需理解 `tools/gen_hello_elf.py` 产出的格式（静态、非
//! 可重定位、带 `PT_LOAD` 段的 ELF64）——遍历程序头表，把每个可装载段以
//! `USER_ACCESSIBLE` 位映射进当前（内核）页表，再把文件字节拷贝进去。
//! 用户程序与内核共享同一地址空间（低半区为用户、高半区为内核），以此保持简单。
//!
//! We only understand what our `tools/gen_hello_elf.py` produces (and, more
//! generally, static non-relocatable ELF64 with `PT_LOAD` segments): walk the
//! program headers, map each loadable segment page into the current (kernel)
//! page table with the `USER_ACCESSIBLE` bit set, and copy the file bytes in.
//!
//! The user program shares the kernel's address space (low half is the user
//! region, high half the kernel), which keeps Phase 5 simple; per-process page
//! tables would be the next refinement.

use core::slice;

use x86_64::structures::paging::page_table::PageTableFlags;
use x86_64::structures::paging::{Page, Size4KiB};
use x86_64::VirtAddr;

use crate::memory::vmm;

const ELF_MAGIC: [u8; 4] = *b"\x7fELF";
const PT_LOAD: u32 = 1;
const PF_W: u32 = 0x2;
const PF_X: u32 = 0x1;

/// Virtual top of the user stack (kept well above the 0x400000 text region).
pub const USER_STACK_TOP: u64 = 0x0050_0000;
/// Number of pages backing the user stack.
const USER_STACK_PAGES: u64 = 4;

/// Where to jump and what to load into RSP when entering ring 3.
pub struct LoadedImage {
    pub entry: u64,
    pub stack_top: u64,
}

fn u16_at(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}
fn u32_at(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}
fn u64_at(b: &[u8], off: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[off..off + 8]);
    u64::from_le_bytes(a)
}

/// Map `page` (if not already mapped) with `flags`, zeroing the backing frame.
unsafe fn ensure_mapped(page: Page<Size4KiB>, flags: PageTableFlags) {
    if vmm::translate(page.start_address()).is_some() {
        return;
    }
    let frame = vmm::alloc_frame().expect("out of physical frames");
    vmm::map_page(page, frame, flags).expect("failed to map user page");
    // Clear the page through its (now valid) user virtual address.
    core::ptr::write_bytes(page.start_address().as_mut_ptr::<u8>(), 0, 4096);
}

/// Load `elf` into memory and return the entry point + user stack top.
///
/// # Safety
/// Must run after `memory::init` (frame allocator + mapping available) and with
/// SMAP disabled so the kernel can write into the user pages it just created.
pub unsafe fn load(elf: &[u8]) -> Result<LoadedImage, &'static str> {
    if elf.len() < 64 || elf[0..4] != ELF_MAGIC {
        return Err("not an ELF64 file");
    }
    if elf[4] != 2 {
        return Err("not ELFCLASS64");
    }
    let entry = u64_at(elf, 0x18);
    let phoff = u64_at(elf, 0x20) as usize;
    let phentsize = u16_at(elf, 0x36) as usize;
    let phnum = u16_at(elf, 0x38) as usize;

    for i in 0..phnum {
        let p = phoff + i * phentsize;
        if u32_at(elf, p) != PT_LOAD {
            continue;
        }
        let flags_dw = u32_at(elf, p + 4);
        let off = u64_at(elf, p + 8) as usize;
        let vaddr = u64_at(elf, p + 16);
        let filesz = u64_at(elf, p + 32) as usize;
        let memsz = u64_at(elf, p + 40) as usize;

        let mut pt_flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
        if flags_dw & PF_W != 0 {
            pt_flags |= PageTableFlags::WRITABLE;
        }
        if flags_dw & PF_X == 0 {
            pt_flags |= PageTableFlags::NO_EXECUTE;
        }

        let start_page = vaddr / 4096;
        let end_page = (vaddr + memsz as u64 + 4095) / 4096;
        for pg in start_page..end_page {
            ensure_mapped(Page::containing_address(VirtAddr::new(pg * 4096)), pt_flags);
        }

        // Copy the segment's file bytes into the freshly-mapped user memory.
        let dst = slice::from_raw_parts_mut(vaddr as *mut u8, filesz);
        dst.copy_from_slice(&elf[off..off + filesz]);
        // Zero any .bss tail (memsz > filesz).
        if memsz > filesz {
            core::ptr::write_bytes((vaddr as usize + filesz) as *mut u8, 0, memsz - filesz);
        }
    }

    // Map the user stack pages.
    let stack_flags = PageTableFlags::PRESENT
        | PageTableFlags::USER_ACCESSIBLE
        | PageTableFlags::WRITABLE
        | PageTableFlags::NO_EXECUTE;
    for i in 0..USER_STACK_PAGES {
        let addr = USER_STACK_TOP - (i + 1) * 4096;
        ensure_mapped(Page::containing_address(VirtAddr::new(addr)), stack_flags);
    }

    Ok(LoadedImage {
        entry,
        stack_top: USER_STACK_TOP,
    })
}
