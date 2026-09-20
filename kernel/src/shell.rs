//! Phase 8: a tiny interactive shell running as a kernel task.
//!
//! The plan sketches a ring-3 `user/bin/shell.rs`, but our user programs are
//! hand-assembled fixed ELFs (see `tools/*.py`) — an interactive read-eval loop
//! is impractical there. So the shell lives in the kernel and reads keystrokes
//! from `/dev/kbd` through the VFS, which is the honest, demonstrable equivalent
//! for a teaching kernel. Built-ins reuse the subsystems built in Phases 3–8
//! (`free`->PMM/heap, `ps`->scheduler, `ls`/`cat`->VFS, `ping`/`netstat`->net).

use alloc::string::String;
use alloc::vec::Vec;

use crate::{fs, memory, net, process};

/// Entry point run as a task. Reads lines from `/dev/kbd` until `exit` or the
/// input goes idle (so a scripted key injection terminates cleanly).
pub fn run() {
    crate::println!("SimpleLinux shell (Phase 8) — type 'help' for commands.");
    let fd = match fs::open("/dev/kbd") {
        Ok(fd) => fd,
        Err(e) => {
            crate::println!("[shell] cannot open /dev/kbd: {:?}", e);
            return;
        }
    };
    let mut line: Vec<u8> = Vec::new();
    let mut idle = 0usize;
    loop {
        let mut buf = [0u8; 32];
        let n = fs::read(fd, &mut buf).unwrap_or(0);
        if n == 0 {
            idle += 1;
            process::sleep(2);
            if idle > 200 {
                crate::println!("[shell] input idle; exiting");
                break;
            }
            continue;
        }
        idle = 0;
        for &b in &buf[..n] {
            match b {
                b'\n' | b'\r' => {
                    crate::print!("# ");
                    let done = eval(&line);
                    line.clear();
                    if done {
                        fs::close(fd).ok();
                        return;
                    }
                }
                b'\x08' => {
                    line.pop(); // backspace
                }
                c if c >= 0x20 && c < 0x7f => line.push(c),
                _ => {}
            }
        }
    }
    fs::close(fd).ok();
}

/// Run one command line; returns true if the shell should quit.
fn eval(line: &[u8]) -> bool {
    let text = core::str::from_utf8(line).unwrap_or("");
    let mut it = text.split_whitespace();
    let cmd = match it.next() {
        Some(c) => c,
        None => return false,
    };
    let args: Vec<&str> = it.collect();
    match cmd {
        "help" => cmd_help(),
        "echo" => crate::println!("{}", args.join(" ")),
        "ls" => cmd_ls(&args),
        "cat" => cmd_cat(&args),
        "ps" => cmd_ps(),
        "free" => cmd_free(),
        "ifconfig" => cmd_ifconfig(),
        "arp" => cmd_arp(),
        "netstat" => cmd_netstat(),
        "ping" => cmd_ping(&args),
        "net" => net::demo(),
        "exit" | "quit" => return true,
        other => crate::println!("{}: command not found (try 'help')", other),
    }
    false
}

fn cmd_help() {
    crate::println!(
        "commands: help echo ls cat ps free ifconfig arp netstat ping net exit"
    );
}

fn cmd_ls(args: &[&str]) {
    let path = args.first().copied().unwrap_or("/");
    match fs::list_dir(path) {
        Ok(names) => crate::println!("{}: {:?}", path, names.as_slice()),
        Err(e) => crate::println!("ls: {} -> {:?}", path, e),
    }
}

fn cmd_cat(args: &[&str]) {
    let path = match args.first() {
        Some(p) => *p,
        None => {
            crate::println!("cat: missing operand");
            return;
        }
    };
    let fd = match fs::open(path) {
        Ok(fd) => fd,
        Err(e) => {
            crate::println!("cat: {} -> {:?}", path, e);
            return;
        }
    };
    let mut buf = [0u8; 256];
    let mut out = String::new();
    while let Ok(n) = fs::read(fd, &mut buf) {
        if n == 0 {
            break;
        }
        out.push_str(core::str::from_utf8(&buf[..n]).unwrap_or("<binary>"));
    }
    fs::close(fd).ok();
    crate::print!("{}", out);
}

fn cmd_ps() {
    crate::println!("[pid] state");
    for (id, st) in process::snapshot() {
        crate::println!("  {} {}", id, st);
    }
}

fn cmd_free() {
    let frames = memory::pmm::FRAME_ALLOCATOR.lock().free_count();
    crate::println!(
        "heap used={} KiB free={} KiB | physical free frames={} ({} MiB)",
        memory::heap::used_bytes() / 1024,
        memory::heap::free_bytes() / 1024,
        frames,
        (frames * 4) / 1024
    );
}

fn cmd_ifconfig() {
    let mac = net::our_mac();
    crate::println!(
        "e1000: ip={}.{}.{}.{} gw={}.{}.{}.{} mac={}",
        net::IP[0], net::IP[1], net::IP[2], net::IP[3],
        net::GATEWAY[0], net::GATEWAY[1], net::GATEWAY[2], net::GATEWAY[3],
        core::str::from_utf8(&mac.fmt()).unwrap_or("?")
    );
}

fn cmd_arp() {
    let t = net::arp::table();
    if t.is_empty() {
        crate::println!("arp cache empty");
        return;
    }
    for (ip, mac) in t {
        crate::println!(
            "  {}.{}.{}.{} -> {}",
            ip[0], ip[1], ip[2], ip[3],
            core::str::from_utf8(&mac.fmt()).unwrap_or("?")
        );
    }
}

fn cmd_netstat() {
    crate::println!("  proto  local  peer            queue");
    for (idx, proto, port, peer, q) in net::socket::info() {
        let name = if proto == net::ip::PROTO_UDP { "udp" } else { "tcp" };
        let peer_s = match peer {
            Some((ip, p)) => alloc::format!("{}.{}.{}.{}:{}", ip[0], ip[1], ip[2], ip[3], p),
            None => String::from("-"),
        };
        crate::println!("  {} {:>4} {:>5} {:<15} {}", name, idx, port, peer_s, q);
    }
}

fn cmd_ping(args: &[&str]) {
    let dst = match args.first().copied().and_then(parse_ip) {
        Some(ip) => ip,
        None => {
            crate::println!("usage: ping a.b.c.d");
            return;
        }
    };
    // Pre-resolve the gateway so the stack has a next-hop MAC to frame with.
    if net::arp::lookup(dst).is_none() {
        net::arp::request(net::GATEWAY);
    }
    for seq in 1..=3 {
        net::icmp::ping(dst, seq);
        process::sleep(5);
        net::pump();
    }
}

/// Parse "a.b.c.d" into four octets.
fn parse_ip(s: &str) -> Option<[u8; 4]> {
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() != 4 {
        return None;
    }
    let mut out = [0u8; 4];
    for i in 0..4 {
        out[i] = parts[i].parse::<u8>().ok()?;
    }
    Some(out)
}
