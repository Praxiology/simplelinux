//! Phase 5: user mode entry + `int 0x80` system calls.
//!
//! Phase 5：进入用户态（ring 3）与 `int 0x80` 系统调用。系统调用 ABI（自定，刻意
//! 仿 Linux）：调用方执行 `int 0x80`，`rax` 传调用号，参数在 `rdi/rsi/rdx`，返回值
//! 放 `rax`（负数即 errno）。进入 ring 3 用 `iretq`；从系统调用返回则借助 TSS 中
//! 配好的 RSP0 内核栈。通过 `exit` 退出的用户程序会直接跳回 `enter_user_asm` 记录的
//! 内核栈，unwind 回调用处。
//!
//! Syscall ABI (our own, deliberately Linux-like):
//!   * call:   `int 0x80` with `rax` = number, args in `rdi, rsi, rdx`
//!   * return: `rax` = result (negative = error)
//!
//! Entering ring 3 uses `iretq` (push SS/RSP/RFLAGS/CS/RIP, then `iretq`).
//! Returning from a syscall uses the CPU-loaded `RSP0` kernel stack (set up in
//! the TSS); a task exiting via the `exit` syscall jumps straight back onto the
//! kernel stack that `enter_user_asm` recorded, unwinding to the caller.

use core::arch::global_asm;

use crate::gdt;

/// System call numbers (kept in one place like a syscall table).
/// `write`/`exit` match `tools/gen_hello_elf.py`; the rest back the Phase-6 VFS.
#[repr(u64)]
#[derive(Debug, Clone, Copy)]
pub enum SyscallNr {
    Write = 1,
    Exit = 2,
    Open = 3,
    Read = 4,
    Close = 5,
    Socket = 6,
    Bind = 7,
    Connect = 8,
}

/// The general-purpose registers saved by the naked `int 0x80` entry stub.
/// Field order matches ascending stack address (see the push order below).
#[repr(C)]
#[derive(Debug, Default)]
pub struct SyscallFrame {
    pub rax: u64,
    pub rbx: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rbp: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
}

/// Kernel RSP captured by `enter_user_asm`, restored when a task exits.
#[no_mangle]
pub static mut USER_KERNEL_RSP: u64 = 0;

global_asm!(
    "
    # void enter_user_asm(u64 entry /*rdi*/, u64 stack /*rsi*/, u64 cs /*rdx*/, u64 ss /*rcx*/)
    .global enter_user_asm
    enter_user_asm:
        lea r11, [rip + USER_KERNEL_RSP]
        mov [r11], rsp                 # remember the kernel stack for exit
        push rcx                       # SS  (RPL 3)
        push rsi                       # user RSP
        push qword ptr 0x202           # RFLAGS: reserved bit 1 + IF
        push rdx                       # CS  (RPL 3)
        push rdi                       # user RIP (entry)
        iretq

    # int 0x80 entry: save GPRs (r15 first so rax ends lowest == *mut SyscallFrame),
    # call syscall_dispatch, restore, iretq back to ring 3.
    .global __syscall_entry
    __syscall_entry:
        push r15
        push r14
        push r13
        push r12
        push r11
        push r10
        push r9
        push r8
        push rbp
        push rdi
        push rsi
        push rdx
        push rcx
        push rbx
        push rax
        mov rdi, rsp
        call syscall_dispatch
        pop rax
        pop rbx
        pop rcx
        pop rdx
        pop rsi
        pop rdi
        pop rbp
        pop r8
        pop r9
        pop r10
        pop r11
        pop r12
        pop r13
        pop r14
        pop r15
        iretq

    # Restore the recorded kernel stack and `ret` from enter_user_asm.
    .global syscall_return_to_kernel
    syscall_return_to_kernel:
        mov rsp, [rip + USER_KERNEL_RSP]
        ret
    "
);

extern "C" {
    fn enter_user_asm(entry: u64, stack: u64, cs: u64, ss: u64);
    fn __syscall_entry();
    fn syscall_return_to_kernel() -> !;
}

/// Address of the `int 0x80` handler, for installing into the IDT.
pub fn entry_addr() -> u64 {
    __syscall_entry as *const () as usize as u64
}

/// 从 ring-3 处理程序分发一次系统调用：按 `rax` 调用号路由到对应 `sys_*`，把结果写回 `rax`。
/// Dispatch one syscall from the ring-3 handler. `frame` points at saved GPRs.
///
/// # Safety
/// Called from assembly with a valid pointer into the current kernel stack.
#[no_mangle]
pub unsafe extern "sysv64" fn syscall_dispatch(frame: *mut SyscallFrame) {
    let f = &mut *frame;
    match f.rax {
        n if n == SyscallNr::Write as u64 => f.rax = sys_write(f.rdi, f.rsi, f.rdx),
        n if n == SyscallNr::Read as u64 => f.rax = sys_read(f.rdi, f.rsi, f.rdx),
        n if n == SyscallNr::Open as u64 => f.rax = sys_open(f.rdi),
        n if n == SyscallNr::Close as u64 => f.rax = sys_close(f.rdi),
        n if n == SyscallNr::Socket as u64 => f.rax = sys_socket(f.rdi),
        n if n == SyscallNr::Bind as u64 => f.rax = sys_bind(f.rdi, f.rsi),
        n if n == SyscallNr::Connect as u64 => f.rax = sys_connect(f.rdi, f.rsi, f.rdx),
        n if n == SyscallNr::Exit as u64 => {
            crate::println!("[kernel] user program exited (code={})", f.rdi as i64);
            syscall_return_to_kernel();
        }
        _ => f.rax = (-3i64) as u64, // -ENOSYS
    }
}

/// `open(path)`: copy the NUL-terminated path from user memory, then resolve.
unsafe fn sys_open(path_ptr: u64) -> u64 {
    use crate::fs::vfs::FsError;
    let path = match read_user_cstr(path_ptr) {
        Some(s) => s,
        None => return FsError::InvalidArg.errno() as u64,
    };
    match crate::fs::open(&path) {
        Ok(fd) => fd as u64,
        Err(e) => e.errno() as u64,
    }
}

/// `read(fd, buf, len)`: fill the user buffer, return count or -errno.
unsafe fn sys_read(fd: u64, buf: u64, len: u64) -> u64 {
    let n = core::cmp::min(len, 4096) as usize;
    let dst = core::slice::from_raw_parts_mut(buf as *mut u8, n);
    match crate::fs::read(fd as usize, dst) {
        Ok(k) => k as u64,
        Err(e) => e.errno() as u64,
    }
}

/// `write(fd, buf, len)`: hand the (shared) user buffer to the VFS. fd 1/2 are
/// pre-wired to `/dev/console`, so a plain console write and an opened-file
/// write travel the exact same path.
unsafe fn sys_write(fd: u64, buf: u64, len: u64) -> u64 {
    let n = core::cmp::min(len, 4096) as usize;
    let src = core::slice::from_raw_parts(buf as *const u8, n);
    match crate::fs::write(fd as usize, src) {
        Ok(k) => k as u64,
        Err(e) => e.errno() as u64,
    }
}

/// `close(fd)`.
unsafe fn sys_close(fd: u64) -> u64 {
    match crate::fs::close(fd as usize) {
        Ok(()) => 0,
        Err(e) => e.errno() as u64,
    }
}

/// `socket(proto)`: allocate a socket, wrap it as a VFS inode, return its fd.
/// "A network endpoint is a file": `read`/`write` on the fd move datagrams.
unsafe fn sys_socket(proto: u64) -> u64 {
    let idx = crate::net::socket::socket(proto as u8);
    match crate::fs::install_inode(crate::net::socket::inode(idx)) {
        Ok(fd) => fd as u64,
        Err(e) => e.errno() as u64,
    }
}

/// `bind(fd, port)`: attach a local UDP port to the socket behind `fd`.
unsafe fn sys_bind(fd: u64, port: u64) -> u64 {
    use crate::fs::vfs::FsError;
    match crate::fs::sock_index(fd as usize) {
        Some(idx) if crate::net::socket::bind(idx, port as u16) => 0,
        Some(_) => FsError::InvalidArg.errno() as u64,
        None => FsError::InvalidArg.errno() as u64,
    }
}

/// `connect(fd, dst_ip, port)`: set the default peer (dst ip in host byte order).
unsafe fn sys_connect(fd: u64, ip: u64, port: u64) -> u64 {
    use crate::fs::vfs::FsError;
    let [a, b, c, d] = (ip as u32).to_be_bytes();
    match crate::fs::sock_index(fd as usize) {
        Some(idx) if crate::net::socket::connect(idx, [a, b, c, d], port as u16) => 0,
        _ => FsError::InvalidArg.errno() as u64,
    }
}

/// Read a NUL-terminated, valid-UTF-8 string from shared user memory.
unsafe fn read_user_cstr(ptr: u64) -> Option<alloc::string::String> {
    let mut bytes = alloc::vec::Vec::new();
    let mut p = ptr as *const u8;
    for _ in 0..256 {
        match *p {
            0 => return alloc::string::String::from_utf8(bytes).ok(),
            c => bytes.push(c),
        }
        p = p.add(1);
    }
    None // not terminated within the limit
}

/// Disable SMAP so the kernel may touch user (U/S=1) pages during syscalls.
pub fn init() {
    use x86_64::registers::control::{Cr4, Cr4Flags};
    unsafe {
        Cr4::write(Cr4::read() - Cr4Flags::SUPERVISOR_MODE_ACCESS_PREVENTION);
    }
    crate::println!("[syscall] int 0x80 gate installed, SMAP disabled");
}

/// 装载并运行一个用户 ELF 直到其 `exit`，然后返回调用处。
/// Load + run a user ELF to completion, then return to the caller.
///
/// # Safety
/// Requires memory + GDT + IDT initialized and `elf` to be a valid ELF64.
pub unsafe fn run_user(elf: &[u8]) {
    let img = match crate::process::elf::load(elf) {
        Ok(img) => img,
        Err(e) => {
            crate::println!("[syscall] ELF load failed: {}", e);
            return;
        }
    };
    crate::println!(
        "[syscall] entering ring3: entry={:#x} stack={:#x}",
        img.entry, img.stack_top
    );
    enter_user_asm(
        img.entry,
        img.stack_top,
        gdt::user_code_selector() as u64,
        gdt::user_data_selector() as u64,
    );
    crate::println!("[syscall] back in ring0 from user mode");
}
