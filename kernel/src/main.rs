#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

extern crate alloc;

mod fs;
mod drivers;
mod gdt;
mod interrupts;
mod memory;
mod net;
mod process;
mod serial;
mod shell;
mod syscall;
mod timer;
mod vga;

use bootloader_api::{
    config::{BootloaderConfig, Mapping},
    entry_point, BootInfo,
};
use core::panic::PanicInfo;

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {{
        $crate::serial::_print(format_args!($($arg)*));
    }};
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}

/// Ask the bootloader to map all of physical memory so the kernel can access
/// frames and page tables directly (required by Phase 3's memory manager).
pub static BOOTLOADER_CONFIG: BootloaderConfig = {
    let mut config = BootloaderConfig::new_default();
    config.mappings.physical_memory = Some(Mapping::Dynamic);
    // Debug builds emit large stack frames; give the kernel some breathing room.
    config.kernel_stack_size = 512 * 1024;
    config
};

entry_point!(kernel_main, config = &BOOTLOADER_CONFIG);

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    serial::init();

    println!("Hello SimpleLinux!");
    println!("Boot info at {:p}", boot_info as *const BootInfo);

    // Phase 2 (part 1): GDT + IDT first, so any fault during memory setup is
    // reported by our handlers instead of triple-faulting.
    gdt::init();
    interrupts::init();

    // Phase 3: physical frame allocator + kernel heap (must run before any
    // `Vec`/`Box`/`String` is used).
    memory::init(boot_info);
    memory_self_test();

    // Phase 2 (part 2): breakpoint demo before we hand control to the scheduler.
    println!("[main] triggering a breakpoint exception on purpose...");
    unsafe { core::arch::asm!("int3") };
    println!("[main] returned from int3");

    // Phase 4: arm the scheduler, then start the PIT (its IRQ drives ticks).
    process::init();
    timer::init();

    // Phase 5: enter ring 3, run the embedded user program to completion, and
    // come back to ring 0. No scheduler tasks exist yet, so the PIT tick that
    // fires during user mode is a no-op (no preemption of ring 3).
    syscall::init();

    // Phase 6: mount the VFS (devfs/procfs/initrd) and seed fds 0/1/2 before
    // any user program runs, since `write` now travels through the fd table.
    fs::init();
    phase6_self_test();

    static USER_ELF: &[u8] = include_bytes!("../../user/hello.elf");
    static VFS_ELF: &[u8] = include_bytes!("../../user/vfs.elf");
    println!("[main] Phase 5: running hello.elf in ring3");
    unsafe { syscall::run_user(USER_ELF) };
    println!("[main] Phase 6: running vfs.elf (open + write /dev/console) in ring3");
    unsafe { syscall::run_user(VFS_ELF) };

    // Phase 7: bring up device drivers (keyboard + e1000) and smoke-test them.
    phase7_setup();
    phase7_demo();

    // Phase 8: bring up the network stack, self-test it, and queue a scripted
    // shell session (the shell task consumes it once the scheduler starts).
    phase8_setup();

    println!("[main] spawning Phase 4 demo tasks and starting scheduler");
    phase4_demo()
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("\n!!! KERNEL PANIC !!!");
    println!("{}", info);
    loop {
        x86_64::instructions::hlt();
    }
}

/// Phase 3 smoke tests: exercise the frame allocator and the heap.
fn memory_self_test() {
    use alloc::{vec, vec::Vec};
    use x86_64::structures::paging::{FrameAllocator, FrameDeallocator};

    // Frame allocator: reserve 1000 frames, then release them all.
    let before = {
        let alloc = memory::pmm::FRAME_ALLOCATOR.lock();
        alloc.free_count()
    };
    let frames: Vec<_> = {
        let mut alloc = memory::pmm::FRAME_ALLOCATOR.lock();
        (0..1000)
            .map(|_| alloc.allocate_frame())
            .collect::<Option<Vec<_>>>()
            .expect("failed to allocate 1000 test frames")
    };
    let during = memory::pmm::FRAME_ALLOCATOR.lock().free_count();
    {
        let mut alloc = memory::pmm::FRAME_ALLOCATOR.lock();
        for f in frames {
            unsafe { alloc.deallocate_frame(f) };
        }
    }
    let after = memory::pmm::FRAME_ALLOCATOR.lock().free_count();
    println!(
        "[mem-test] frames free: {before} -> {during} -> {after} (expected -1000 then restored)"
    );

    // Heap: allocate a few Vecs and confirm the used-byte counter moves.
    let mut data = vec![0u8; 4096];
    data[4095] = 0xAB;
    let mut v: Vec<u32> = Vec::new();
    for i in 0..100 {
        v.push(i * i);
    }
    println!(
        "[mem-test] heap used={} KiB free={} KiB; data[4095]={:#x} v[99]={}",
        memory::heap::used_bytes() / 1024,
        memory::heap::free_bytes() / 1024,
        data[4095],
        v[99]
    );
}

/// A worker task: print a few iterations, sleeping between them so the timer
/// IRQ wakes it and the other tasks get to interleave.
fn worker(name: &'static str) -> impl FnOnce() {
    move || {
        for i in 1..=5 {
            println!("[{name}] iter {i} (tick={})", timer::ticks());
            process::sleep(3);
        }
        println!("[{name}] finished");
    }
}

/// Spawn three workers, join them, then run a leak-focused stress test.
fn manager_task() {
    let a = process::spawn(worker("t1"));
    let b = process::spawn(worker("t2"));
    let c = process::spawn(worker("t3"));
    process::join(a);
    process::join(b);
    process::join(c);
    println!("[manager] joined all workers; alive={}", process::task_count());
    stress_test();
    println!("[manager] Phase 4 demo complete");
}

/// Spawn `N` short-lived tasks, join them, and report how much heap / how many
/// physical frames are still held afterwards. Running the batch twice and
/// getting the same numbers proves stacks/frames are reclaimed (no per-task leak).
fn stress_test() {
    use alloc::vec::Vec;

    let one_round = || {
        let ids: Vec<_> = (0..32)
            .map(|_| {
                process::spawn(|| {
                    let mut v = alloc::vec![7u8; 2048];
                    v[0] = v[0].wrapping_add(1);
                    process::yield_now();
                })
            })
            .collect();
        for id in ids {
            process::join(id);
        }
        process::sleep(2); // let the scheduler reap the last stragglers
        let frames = memory::pmm::FRAME_ALLOCATOR.lock().free_count();
        (memory::heap::used_bytes(), frames)
    };

    let (heap1, frames1) = one_round();
    let (heap2, frames2) = one_round();
    let ok = heap2 <= heap1 + 1024 && frames2 == frames1;
    println!(
        "[stress] round1 heap={} frames={} | round2 heap={} frames={} ({})",
        heap1,
        frames1,
        heap2,
        frames2,
        if ok { "stable" } else { "LEAK?" }
    );
}

/// Launch the scheduler with a clock task + manager and hand over control.
fn phase4_demo() -> ! {
    // Bounded clock task: report the tick count once per second, three times.
    process::spawn(|| {
        let mut last = 0u64;
        let mut reported = 0;
        while reported < 3 {
            let t = timer::ticks();
            if t - last >= timer::HZ {
                println!("tick={t}");
                last = t;
                reported += 1;
            }
            process::yield_now();
        }
        println!("[clock] done");
    });
    process::spawn(manager_task);
    // e1000 ARP demo runs as a task so it can `sleep` while waiting for the
    // reply to come back over the (emulated) wire.
    process::spawn(|| drivers::e1000::demo_arp());
    // Phase 8: an interactive shell task reading the (scripted) keyboard input.
    process::spawn(shell::run);
    process::start()
}

/// Phase 6 smoke tests: exercise the VFS straight from the kernel (the same
/// code paths the open/read/write/close syscalls use).
fn phase6_self_test() {
    use crate::fs;

    match fs::list_dir("/initrd") {
        Ok(names) => println!("[fs-test] ls /initrd -> {:?}", names.as_slice()),
        Err(e) => println!("[fs-test] ls /initrd err {:?}", e),
    }
    match fs::list_dir("/dev") {
        Ok(names) => println!("[fs-test] ls /dev -> {:?}", names.as_slice()),
        Err(e) => println!("[fs-test] ls /dev err {:?}", e),
    }

    let mut scratch = [0u8; 256];
    fs_cat("/initrd/motd.txt", &mut scratch);
    fs_cat("/proc/uptime", &mut scratch);
    fs_cat("/proc/meminfo", &mut scratch);
    fs_cat("/nope", &mut scratch); // expect NotFound
}

/// open + read-to-EOF + close, printing the result; used by the VFS self-test.
fn fs_cat(path: &str, scratch: &mut [u8]) {
    use crate::fs;
    let fd = match fs::open(path) {
        Ok(fd) => fd,
        Err(e) => {
            println!("[fs-test] open {} -> {:?}", path, e);
            return;
        }
    };
    let mut total = 0usize;
    while total < scratch.len() {
        match fs::read(fd, &mut scratch[total..]) {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(_) => break,
        }
    }
    let _ = fs::close(fd);
    let text = core::str::from_utf8(&scratch[..total]).unwrap_or("<non-utf8>");
    println!("[fs-test] cat {} ({} bytes):\n{}", path, total, text);
}

/// Phase 7: initialize the e1000 NIC (keyboard IRQ1 is already live).
fn phase7_setup() {
    println!("[main] Phase 7: initializing e1000 NIC");
    // SAFETY: memory/GDT/IDT are up; the NIC uses polled MMIO + the frame allocator.
    unsafe { drivers::e1000::init() };
}

/// Phase 7 smoke tests: keyboard decode -> /dev/kbd, and an e1000 ARP round-trip.
fn phase7_demo() {
    // Inject the scancodes for "hello" (scan code set 1 make codes) and read
    // them back through the VFS, proving IRQ decode -> buffer -> /dev/kbd.
    use crate::fs;
    drivers::keyboard::inject(&[0x23, 0x12, 0x26, 0x26, 0x18]);
    match fs::open("/dev/kbd") {
        Ok(fd) => {
            let mut buf = [0u8; 16];
            let n = fs::read(fd, &mut buf).unwrap_or(0);
            let text = core::str::from_utf8(&buf[..n]).unwrap_or("<non-utf8>");
            println!("[kbd-test] /dev/kbd read {} bytes: '{}'", n, text);
            let _ = fs::close(fd);
        }
        Err(e) => println!("[kbd-test] open /dev/kbd -> {:?}", e),
    }
}

/// Phase 8: bring up the network stack, run the layered self-test, and queue a
/// scripted shell session for the shell task to replay once we start scheduling.
fn phase8_setup() {
    println!("[main] Phase 8: bringing up the network stack");
    net::init();
    net::demo();
    println!("[main] Phase 8: queuing a scripted shell session");
    drivers::keyboard::inject_str(
        "help\nifconfig\narp -a\nnetstat\nping 10.0.2.2\nfree\nps\nls /initrd\ncat /initrd/motd.txt\nexit\n",
    );
}
