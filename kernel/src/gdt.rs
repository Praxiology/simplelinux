//! GDT + TSS setup.
//!
//! The bootloader already runs with `CS = 0x8`, `DS/SS = 0x10`. Our own GDT must
//! therefore place a kernel code segment at selector `0x8` and a kernel data
//! segment at `0x10` (otherwise interrupt gates, which reference selector `0x8`,
//! would fault with #GP). We then add ring-3 code/data segments (used by Phase 5
//! user mode) and finally the TSS, which provides:
//!   * `privilege_stack_table[0]` (RSP0): the kernel stack the CPU switches to
//!     when a ring-3 task traps in via `int 0x80`.
//!   * `interrupt_stack_table[0]` (IST): a dedicated stack for double faults.

use lazy_static::lazy_static;
use x86_64::instructions::segmentation::{Segment, CS, SS};
use x86_64::instructions::tables::load_tss;
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::VirtAddr;

/// IST slot reserved for the double-fault handler.
pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

/// Size of the dedicated double-fault stack (5 pages).
const STACK_SIZE: usize = 4096 * 5;

/// Size of the ring3 -> ring0 (syscall) kernel stack.
const PRIV_STACK_SIZE: usize = 4096 * 4;

/// A static stack region used only when a double fault hits.
static mut DOUBLE_FAULT_STACK: [u8; STACK_SIZE] = [0; STACK_SIZE];
/// The stack loaded into RSP when a user task traps into the kernel.
static mut SYSCALL_STACK: [u8; PRIV_STACK_SIZE] = [0; PRIV_STACK_SIZE];

lazy_static! {
    static ref TSS: TaskStateSegment = {
        let mut tss = TaskStateSegment::new();
        // The IST stores the *top* of the stack (x86 grows downward).
        let df = VirtAddr::from_ptr(&raw const DOUBLE_FAULT_STACK);
        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] =
            df + STACK_SIZE as u64;
        // RSP0: top of the kernel stack used on privilege-level transitions.
        let sc = VirtAddr::from_ptr(&raw const SYSCALL_STACK);
        tss.privilege_stack_table[0] = sc + PRIV_STACK_SIZE as u64;
        tss
    };
}

struct Selectors {
    code: SegmentSelector,
    data: SegmentSelector,
    user_data: SegmentSelector,
    user_code: SegmentSelector,
    tss: SegmentSelector,
}

lazy_static! {
    static ref GDT: (GlobalDescriptorTable, Selectors) = {
        let mut gdt = GlobalDescriptorTable::new();
        let code = gdt.append(Descriptor::kernel_code_segment()); // -> 0x08
        let data = gdt.append(Descriptor::kernel_data_segment()); // -> 0x10
        let user_data = gdt.append(Descriptor::user_data_segment()); // -> 0x18
        let user_code = gdt.append(Descriptor::user_code_segment()); // -> 0x20
        let tss = gdt.append(Descriptor::tss_segment(&TSS)); // -> 0x28 (2 entries)
        (
            gdt,
            Selectors {
                code,
                data,
                user_data,
                user_code,
                tss,
            },
        )
    };
}

/// User-mode code selector (RPL 3) to load via `iretq`.
pub fn user_code_selector() -> u16 {
    GDT.1.user_code.0 | 0x3
}

/// User-mode data selector (RPL 3) to load via `iretq`.
pub fn user_data_selector() -> u16 {
    GDT.1.user_data.0 | 0x3
}

/// Kernel code selector (RPL 0), used to fake a return to ring 0.
pub fn kernel_code_selector() -> u16 {
    GDT.1.code.0
}

/// Load the GDT, reload the code/data segment registers, and load the TSS.
pub fn init() {
    GDT.0.load();
    unsafe {
        CS::set_reg(GDT.1.code);
        SS::set_reg(GDT.1.data);
        load_tss(GDT.1.tss);
    }
    crate::println!(
        "[gdt] GDT + TSS loaded (user CS={:#x} SS={:#x}, RSP0 ready)",
        user_code_selector(),
        user_data_selector()
    );
}
