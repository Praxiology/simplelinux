//! Task abstraction: identity, state machine, and per-task kernel stack.

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;

use super::context::{self, TaskContext};

/// Unique task identifier (newtype so it can't be confused with a plain u32).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TaskId(pub u32);

/// A task body: a `'static` closure run once on the task's kernel stack.
pub type TaskFn = Box<dyn FnOnce() + Send>;

/// Cooperative + preemptive lifecycle of a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    /// Runnable, waiting for its turn.
    Ready,
    /// Currently executing on the CPU.
    Running,
    /// Waiting for an event (see [`BlockReason`]).
    Blocked,
    /// Finished; awaiting `join`. Its stack is freed on reaping.
    Zombie,
}

/// Why a task is [`TaskState::Blocked`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockReason {
    /// Not blocked.
    None,
    /// Blocked until the timer reaches `wake_at` ticks.
    Sleep { wake_at: u64 },
    /// Blocked until task `id` becomes a [`TaskState::Zombie`].
    Join { id: TaskId },
}

/// Size of each task's kernel stack (16 KiB).
pub const KERNEL_STACK_SIZE: usize = 16 * 1024;

/// A heap-allocated, per-task kernel stack.
///
/// The backing buffer is a `Vec<u8>` that is never resized, so `as_mut_ptr()`
/// stays valid for the task's whole life; `Drop` returns the memory to the heap.
pub struct KernelStack {
    buffer: Vec<u8>,
}

impl KernelStack {
    pub fn new() -> Self {
        Self {
            buffer: vec![0u8; KERNEL_STACK_SIZE],
        }
    }

    /// 16-byte-aligned base address of the usable region.
    fn aligned_base(&mut self) -> u64 {
        let raw = self.buffer.as_mut_ptr() as u64;
        (raw + 15) & !15u64
    }

    /// Usable (aligned) size in bytes.
    fn usable_size(&mut self) -> usize {
        let base = self.aligned_base();
        let end = (self.buffer.as_mut_ptr() as u64 + self.buffer.len() as u64) & !15u64;
        (end - base) as usize
    }

    /// Produce the initial [`TaskContext`] that will start running at `entry`.
    ///
    /// # Safety
    /// The returned context assumes `entry` (the trampoline) and the callee-saved
    /// slots live entirely inside this stack's region.
    pub unsafe fn initial_context(&mut self) -> TaskContext {
        let base = self.aligned_base();
        let size = self.usable_size();
        context::initial_context(base, size)
    }

    /// Debug helper: the stack's address range.
    pub fn range(&mut self) -> (u64, u64) {
        (self.aligned_base(), self.usable_size() as u64)
    }
}

/// A single schedulable kernel task.
pub struct Task {
    pub id: TaskId,
    pub state: TaskState,
    pub block: BlockReason,
    pub ctx: TaskContext,
    /// Remaining ticks of the current time slice before the timer preempts it.
    pub slice: u64,
    stack: KernelStack,
    func: Option<TaskFn>,
}

impl Task {
    /// Create a ready task with a fresh stack wired to the trampoline.
    ///
    /// # Safety
    /// Sets up the synthetic initial context on the new stack.
    pub unsafe fn new(id: TaskId, func: TaskFn, slice: u64) -> Self {
        let mut stack = KernelStack::new();
        let ctx = stack.initial_context();
        Self {
            id,
            state: TaskState::Ready,
            block: BlockReason::None,
            ctx,
            slice,
            stack,
            func: Some(func),
        }
    }

    /// Take the task body (only succeeds once).
    pub fn take_func(&mut self) -> Option<TaskFn> {
        self.func.take()
    }

    /// Mutable view of the stack range (for diagnostics).
    pub fn stack_range(&mut self) -> (u64, u64) {
        self.stack.range()
    }
}
