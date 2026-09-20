//! Phase 4: processes / kernel threads / cooperative + preemptive scheduling.
//!
//! Public API used by the rest of the kernel:
//!   * [`spawn`]    - add a new kernel task running the given closure
//!   * [`start`]    - hand control to the scheduler (call once, never returns)
//!   * [`yield_now`] / [`sleep`] / [`join`] - the classic task operations

pub mod context;
pub mod elf;
pub mod scheduler;
pub mod task;

pub use scheduler::{
    init, join, sleep, snapshot, spawn, start, task_count, tick, yield_now,
};
