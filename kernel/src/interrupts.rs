//! IDT (interrupt descriptor table): CPU exceptions + hardware IRQ handling.
//!
//! We remap the 8259 PIC so IRQ0..15 map to interrupt vectors 32..47 (leaving
//! 0..31 for CPU exceptions), then register handlers for a few exceptions and
//! the timer IRQ.

use lazy_static::lazy_static;
use pic8259::ChainedPics;
use spin::Mutex;
use x86_64::instructions::interrupts;
use x86_64::registers::control::Cr2;
use x86_64::structures::idt::{
    InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode,
};
use x86_64::{PrivilegeLevel, VirtAddr};

use crate::gdt;
use crate::timer;

/// Primary PIC offset (IRQ0 -> vector 32).
pub const PIC_1_OFFSET: u8 = 32;
/// Secondary PIC offset (IRQ8 -> vector 40).
pub const PIC_2_OFFSET: u8 = 40;

/// Vector for the PIT timer (IRQ0).
pub const TIMER_INTERRUPT_INDEX: u8 = PIC_1_OFFSET;
/// Vector for the PS/2 keyboard (IRQ1).
pub const KEYBOARD_INTERRUPT_INDEX: u8 = PIC_1_OFFSET + 1;

static PIC: Mutex<ChainedPics> =
    Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });

lazy_static! {
    static ref IDT: InterruptDescriptorTable = {
        let mut idt = InterruptDescriptorTable::new();
        idt.breakpoint.set_handler_fn(breakpoint_handler);
        unsafe {
            idt.double_fault
                .set_handler_fn(double_fault_handler)
                .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
        }
        idt.page_fault.set_handler_fn(page_fault_handler);
        idt.general_protection_fault
            .set_handler_fn(general_protection_handler);
        idt[TIMER_INTERRUPT_INDEX].set_handler_fn(timer_interrupt_handler);
        idt[KEYBOARD_INTERRUPT_INDEX].set_handler_fn(keyboard_interrupt_handler);
        // Phase 5: `int 0x80` syscall gate, callable from ring 3 (DPL 3). We
        // install a raw handler address because the stub is hand-written
        // assembly, not an `extern "x86-interrupt"` fn.
        unsafe {
            idt[0x80u8]
                .set_handler_addr(VirtAddr::new(crate::syscall::entry_addr()))
                .set_privilege_level(PrivilegeLevel::Ring3);
        }
        idt
    };
}

/// Load the IDT, remap + initialize the PIC, and enable hardware interrupts.
pub fn init() {
    IDT.load();
    unsafe {
        let mut pics = PIC.lock();
        pics.initialize();
        // Unmask IRQ0 (PIT timer) and IRQ1 (keyboard): BIOS/bootloader may
        // leave them masked.
        let masks = pics.read_masks();
        pics.write_masks(masks[0] & !0x03, masks[1]);
    }
    interrupts::enable();
    crate::println!("[int] IDT loaded, PIC remapped, interrupts enabled");
}

extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    crate::println!("[int] BREAKPOINT (int3) hit: {:#?}", &*stack_frame);
    // Returning resumes normal execution right after the `int3`.
}

extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) -> ! {
    panic!(
        "[int] DOUBLE FAULT: {:#?} error_code={error_code:#x}",
        &*stack_frame
    );
}

extern "x86-interrupt" fn general_protection_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    crate::println!(
        "[int] GENERAL PROTECTION FAULT err={error_code:#x}: {:#?}",
        &*stack_frame
    );
    panic!("general protection fault");
}

extern "x86-interrupt" fn page_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: PageFaultErrorCode,
) {
    // Note: `format!`/String needs a heap (Phase 3), so print the raw CR2 value.
    crate::println!(
        "[int] PAGE FAULT at {:#x} error={:?}\n{:#?}",
        Cr2::read_raw(),
        error_code,
        &*stack_frame
    );
    panic!("page fault");
}

extern "x86-interrupt" fn timer_interrupt_handler(_stack_frame: InterruptStackFrame) {
    // Advance the clock and give the scheduler a chance to preempt. Handlers
    // must stay silent (no println!) to avoid deadlocking on the serial lock.
    timer::tick();
    crate::process::tick();
    unsafe {
        PIC.lock()
            .notify_end_of_interrupt(TIMER_INTERRUPT_INDEX);
    }
}

extern "x86-interrupt" fn keyboard_interrupt_handler(_stack_frame: InterruptStackFrame) {
    // Decode the scancode into the /dev/kbd buffer, then EOI IRQ1.
    crate::drivers::keyboard::interrupt_handler();
    unsafe {
        PIC.lock()
            .notify_end_of_interrupt(KEYBOARD_INTERRUPT_INDEX);
    }
}
