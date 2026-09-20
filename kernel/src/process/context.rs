//! Task context switching.
//!
//! We switch contexts with the classic "save callee-saved registers + stack
//! pointer, restore the next task's" scheme. Only `rsp` needs to be remembered
//! per task: the six callee-saved registers (`rbx`, `rbp`, `r12`..`r15`) are
//! pushed/popped on the task's own kernel stack, and the instruction pointer is
//! carried implicitly by the return address sitting on that stack.
//!
//! This phase switches only the kernel stack (no CR3 change): every task shares
//! the kernel address space, which is exactly the "pure kernel task" milestone.

use core::arch::global_asm;

/// Per-task switch state. Just the saved stack pointer; everything else lives on
/// the stack pointed to by `rsp`.
#[derive(Debug, Default, Clone, Copy)]
#[repr(C)]
pub struct TaskContext {
    pub rsp: u64,
}

global_asm!(
    "
    .global __switch
    .type __switch, @function
    __switch:
        # rdi = *mut TaskContext (old), rsi = *const TaskContext (new)
        push rbx
        push rbp
        push r12
        push r13
        push r14
        push r15
        mov [rdi], rsp        # remember where the old stack now is
        mov rsp, [rsi]        # jump onto the new task's stack
        pop r15
        pop r14
        pop r13
        pop r12
        pop rbp
        pop rbx
        ret                   # resume new task at its saved return address
    "
);

extern "C" {
    /// Save the current context into `old` and resume the context in `new`.
    ///
    /// After the function returns (much later), execution continues in whichever
    /// context was loaded from `new`.
    pub fn __switch(old: *mut TaskContext, new: *const TaskContext);
}

/// Return address every fresh task starts executing at (see `task.rs`).
///
/// Declared here so `initial_context` and the trampoline share one symbol.
extern "C" fn task_trampoline() -> ! {
    crate::process::scheduler::current_task_body()
}

/// Build the [`TaskContext`] for a brand-new task whose stack spans
/// `[stack_base, stack_base + stack_size)` and whose entry point is `entry`.
///
/// The synthetic frame matches what `__switch` pops: six zeroed callee-saved
/// slots followed by a return address. On first resume the task "returns" into
/// [`task_trampoline`], which runs the task body.
///
/// # Safety
/// `entry` must be a valid function pointer and the stack range must be
/// writable, 16-byte aligned, and reserved exclusively for this task.
pub unsafe fn initial_context(stack_base: u64, stack_size: usize) -> TaskContext {
    let top = (stack_base + stack_size as u64) & !15u64; // 16-byte aligned top
    let ret_slot = top - 16; // 16-aligned slot holding the "return" address
    (ret_slot as *mut u64).write(task_trampoline as *const () as u64);
    // Zero the six callee-saved slots beneath the return address.
    for i in 0..6 {
        ((ret_slot - 8 - i * 8) as *mut u64).write(0);
    }
    TaskContext { rsp: ret_slot - 48 }
}
