//! Phase 4: processes / kernel threads / cooperative + preemptive scheduling.
//!
//! Phase 4：进程/内核线程与协作式+抢占式调度。子模块：
//!   * `task`      —— 任务体、状态机、每任务内核栈；
//!   * `context`   —— 手写的上下文切换汇编（保存/恢复被调用者保存寄存器 + rsp）；
//!   * `scheduler` —— 轮转（round-robin）调度器；
//!   * `elf`       —— 极简 ELF64 装载器。
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
