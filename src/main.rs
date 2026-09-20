//! Runner entry point (host side): launches QEMU with the BIOS disk image that
//! `build.rs` produced from the kernel. This is what `make run` invokes.
use std::process::exit;

fn main() {
    let bios_path = env!("BIOS_PATH");

    let mut cmd = std::process::Command::new("qemu-system-x86_64");
    cmd.args(["-drive", &format!("format=raw,file={bios_path}")]);
    cmd.args(["-m", "128M", "-serial", "stdio", "-display", "none", "-no-reboot"]);
    // isa-debug-exit lets the kernel request a clean QEMU shutdown (Phase 2+ tests).
    cmd.args(["-device", "isa-debug-exit,iobase=0xf4,iosize=0x04"]);
    cmd.args([
        "-netdev",
        "user,id=n0,hostfwd=tcp::1234-:1234,hostfwd=udp::9999-:9999",
    ]);
    cmd.args(["-device", "e1000,netdev=n0"]);

    // Forward any extra CLI args (e.g. `-s -S` for gdb) to QEMU.
    cmd.args(std::env::args().skip(1));

    let status = cmd.status().expect("failed to start qemu-system-x86_64");
    // isa-debug-exit: the guest writes (code << 1) | 1 to port 0xf4.
    match status.code().unwrap_or(1) {
        0x10 => exit(0), // QemuExitCode::Success
        0x11 => exit(1), // QemuExitCode::Failed
        code => exit(code),
    }
}
