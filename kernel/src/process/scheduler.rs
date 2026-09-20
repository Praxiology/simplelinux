//! Round-robin scheduler for kernel tasks.
//!
//! Design notes (single-core / UP, so "concurrency" is really interleaving):
//!
//! * Tasks live in a `Vec<Option<Task>>` of *stable slots*. A slot index never
//!   changes while a task is alive, so a `TaskContext` pointer taken before a
//!   context switch stays valid even if the vector later reallocs (the stack
//!   buffer is heap-allocated independently and never moves).
//! * The scheduler `Mutex` is **never held across `__switch`** (that would
//!   deadlock when the resumed task re-enters `schedule`). We lock briefly to
//!   choose the next task and grab raw context pointers, then drop the guard.
//! * Interrupts are disabled across the whole switch (and the timer ISR already
//!   runs with interrupts masked), so the vector cannot mutate mid-switch.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;
use x86_64::instructions::interrupts;

use super::task::{BlockReason, Task, TaskFn, TaskId, TaskState};
use super::context::TaskContext;
use crate::timer;

/// Default time slice in ticks before a running task is preempted.
pub const DEFAULT_SLICE: u64 = 2;
/// Sentinel for "no task slot" (the boot context).
const NO_TASK: usize = usize::MAX;

extern "C" {
    fn __switch(old: *mut TaskContext, new: *const TaskContext);
}

struct Scheduler {
    slots: Vec<Option<Task>>,
    current: usize,
    next_id: u32,
    cursor: usize,
}

impl Scheduler {
    const fn new() -> Self {
        Self {
            slots: Vec::new(),
            current: NO_TASK,
            next_id: 1,
            cursor: 0,
        }
    }

    fn add(&mut self, func: TaskFn) -> TaskId {
        let id = TaskId(self.next_id);
        self.next_id += 1;
        let task = unsafe { Task::new(id, func, DEFAULT_SLICE) };
        match self.slots.iter_mut().find(|s| s.is_none()) {
            Some(hole) => *hole = Some(task),
            None => self.slots.push(Some(task)),
        }
        id
    }

    fn find_slot(&self, id: TaskId) -> Option<usize> {
        self.slots
            .iter()
            .position(|s| matches!(s, Some(t) if t.id == id))
    }

    /// Next runnable slot (round robin), or `None` if there is none.
    fn next_ready(&self) -> Option<usize> {
        let n = self.slots.len();
        for i in 0..n {
            let idx = (self.cursor + i) % n;
            if matches!(&self.slots[idx], Some(t) if t.state == TaskState::Ready) {
                return Some(idx);
            }
        }
        None
    }

    /// Mark `idx` Running and return (old_ctx, new_ctx) pointers for a switch.
    fn prep_switch(&mut self, idx: usize) -> (*mut TaskContext, *const TaskContext) {
        let base = self.slots.as_mut_ptr();
        let old = Self::ctx_ptr(base, self.current);
        if let Some(t) = &mut self.slots[idx] {
            t.state = TaskState::Running;
        }
        self.current = idx;
        self.cursor = idx + 1;
        (old, Self::ctx_ptr(base, idx))
    }

    fn ctx_ptr(base: *mut Option<Task>, i: usize) -> *mut TaskContext {
        if i == NO_TASK {
            return raw_boot();
        }
        unsafe { &mut (*(*base.add(i)).as_mut().unwrap_unchecked()).ctx }
    }
}

static SCHEDULER: Mutex<Scheduler> = Mutex::new(Scheduler::new());
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// Throwaway context used when leaving the boot context / an exiting task.
static mut BOOT_CTX: TaskContext = TaskContext { rsp: 0 };

fn raw_boot() -> *mut TaskContext {
    &raw mut BOOT_CTX
}

fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// Arm the scheduler (must run before the timer starts preempting).
pub fn init() {
    ACTIVE.store(true, Ordering::Relaxed);
}

/// Add a kernel task to the run queue.
pub fn spawn(func: impl FnOnce() + Send + 'static) -> TaskId {
    let mut g = SCHEDULER.lock();
    g.add(Box::new(func))
}

/// Enter task context: switch to the first Ready task and never return.
pub fn start() -> ! {
    let idx = { SCHEDULER.lock().next_ready() };
    let Some(idx) = idx else {
        crate::println!("[sched] no tasks to run");
        idle();
    };
    interrupts::disable();
    let (old, new) = {
        let mut g = SCHEDULER.lock();
        g.prep_switch(idx)
    };
    unsafe { __switch(old, new) };
    idle();
}

/// Choose the next runnable task and switch to it; returns when rescheduled.
fn schedule() {
    // Remember the caller's interrupt state: when preempted from the timer ISR
    // IF is already 0, and we must not enable it mid-handler on resume.
    let was_enabled = interrupts::are_enabled();
    interrupts::disable();
    let switch = {
        let mut g = SCHEDULER.lock();
        // A Running task reaching here (yield / preemption) is demoted.
        let cur = g.current;
        if cur != NO_TASK {
            if let Some(t) = &mut g.slots[cur] {
                if t.state == TaskState::Running {
                    t.state = TaskState::Ready;
                }
            }
        }
        let next = g.next_ready();
        next.map(|idx| g.prep_switch(idx))
    };
    match switch {
        Some((old, new)) => unsafe { __switch(old, new) },
        None => {
            if was_enabled {
                interrupts::enable();
            }
            return;
        }
    }
    // Resumed: restore whatever interrupt state this task's caller had.
    if was_enabled {
        interrupts::enable();
    }
}

/// Voluntarily give up the CPU.
pub fn yield_now() {
    if active() {
        schedule();
    }
}

/// Block the current task for at least `ticks` timer ticks.
pub fn sleep(ticks: u64) {
    if !active() {
        return;
    }
    let wake = timer::ticks() + ticks;
    {
        let mut g = SCHEDULER.lock();
        let cur = g.current;
        if let Some(t) = &mut g.slots[cur] {
            t.state = TaskState::Blocked;
            t.block = BlockReason::Sleep { wake_at: wake };
        }
    }
    schedule();
}

/// Block until task `id` finishes.
pub fn join(id: TaskId) {
    if !active() {
        return;
    }
    let done = {
        let g = SCHEDULER.lock();
        match g.find_slot(id) {
            None => true,
            Some(s) => g.slots[s].as_ref().unwrap().state == TaskState::Zombie,
        }
    };
    if done {
        return;
    }
    {
        let mut g = SCHEDULER.lock();
        let cur = g.current;
        if let Some(t) = &mut g.slots[cur] {
            t.state = TaskState::Blocked;
            t.block = BlockReason::Join { id };
        }
    }
    schedule();
}

/// Terminate the current task (called from the trampoline after the body).
fn exit_current() {
    interrupts::disable();
    let switch = {
        let mut g = SCHEDULER.lock();
        let cur = g.current;
        let dead_id = {
            let t = g.slots[cur].as_mut().unwrap();
            t.state = TaskState::Zombie;
            t.id
        };
        // Wake joiners waiting on us.
        for slot in g.slots.iter_mut() {
            if let Some(t) = slot {
                if t.state == TaskState::Blocked {
                    if let BlockReason::Join { id } = t.block {
                        if id == dead_id {
                            t.state = TaskState::Ready;
                            t.block = BlockReason::None;
                        }
                    }
                }
            }
        }
        // Reap our own slot (frees the stack) now that we are a finished zombie.
        let next = g.next_ready();
        g.slots[cur] = None;
        g.current = NO_TASK;
        next.map(|idx| g.prep_switch(idx))
    };
    match switch {
        Some((_old, new)) => unsafe { __switch(raw_boot(), new) },
        None => {
            interrupts::enable();
            idle();
        }
    }
    // Unreachable: an exited task never resumes here.
    idle();
}

/// Entry run by a freshly-switched task: call its body, then exit.
pub fn current_task_body() -> ! {
    let func = {
        let mut g = SCHEDULER.lock();
        let cur = g.current;
        g.slots[cur].as_mut().unwrap().take_func()
    };
    if let Some(f) = func {
        interrupts::enable();
        f();
    }
    exit_current();
    idle();
}

/// Called from the timer ISR: wake sleepers and account for time slices.
pub fn tick() {
    if !active() {
        return;
    }
    let now = timer::ticks();
    let preempt = {
        let mut g = SCHEDULER.lock();
        for slot in g.slots.iter_mut() {
            if let Some(t) = slot {
                if t.state == TaskState::Blocked {
                    if let BlockReason::Sleep { wake_at } = t.block {
                        if now >= wake_at {
                            t.state = TaskState::Ready;
                            t.block = BlockReason::None;
                        }
                    }
                }
            }
        }
        let cur = g.current;
        if cur == NO_TASK {
            false
        } else if let Some(t) = &mut g.slots[cur] {
            if t.slice == 0 {
                t.slice = DEFAULT_SLICE;
                true
            } else {
                t.slice -= 1;
                false
            }
        } else {
            false
        }
    };
    if preempt {
        schedule();
    }
}

/// Number of tasks currently alive (any state), for tests.
pub fn task_count() -> usize {
    SCHEDULER.lock().slots.iter().filter(|s| s.is_some()).count()
}

/// A `ps`-style snapshot: `(id, state)` for every live task.
pub fn snapshot() -> Vec<(u32, &'static str)> {
    SCHEDULER
        .lock()
        .slots
        .iter()
        .filter_map(|s| s.as_ref())
        .map(|t| (t.id.0, state_name(t.state)))
        .collect()
}

fn state_name(s: TaskState) -> &'static str {
    match s {
        TaskState::Ready => "ready",
        TaskState::Running => "running",
        TaskState::Blocked => "blocked",
        TaskState::Zombie => "zombie",
    }
}

/// Park the CPU forever (all tasks done / boot with none).
fn idle() -> ! {
    loop {
        interrupts::enable();
        x86_64::instructions::hlt();
    }
}
