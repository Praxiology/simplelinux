//! Phase 8: a small, hand-written, layered TCP/IP stack.
//!
//! It sits directly on the polled e1000 byte pipes ([`crate::drivers::e1000::send`]
//! / [`crate::drivers::e1000::poll_receive`]) so every layer is visible:
//!
//! ```text
//!   application (shell / socket)      <- socket.rs
//!   transport   UDP | TCP (skeleton)   <- udp.rs / tcp.rs
//!   network     IPv4 + ICMP            <- ip.rs / icmp.rs
//!   link        Ethernet + ARP         <- ethernet.rs / arp.rs
//!   driver      e1000 frame send/recv
//! ```
//!
//! Incoming frames are decoded bottom-up and dispatched by EtherType / IP
//! protocol; outgoing packets are built top-down. The RX direction is fully
//! exercised by the Phase-8 self-test; the TX direction is byte-correct but the
//! wire cannot be observed on this QEMU build (see the e1000 TX-completion note).

pub mod arp;
pub mod ethernet;
pub mod icmp;
pub mod ip;
pub mod socket;
pub mod tcp;
pub mod udp;

pub use ethernet::MacAddr;

use crate::drivers::e1000;

/// Our hard-coded IPv4 identity (matches the e1000 driver's QEMU user-mode setup).
pub const IP: [u8; 4] = e1000::IP;
pub const GATEWAY: [u8; 4] = e1000::GATEWAY;

/// Bring up the mutable pieces of the stack (ARP cache, socket table).
pub fn init() {
    arp::init();
    socket::init();
    crate::println!("[net] stack up: ip={}.{}.{}.{} gw={}.{}.{}.{}",
        IP[0], IP[1], IP[2], IP[3], GATEWAY[0], GATEWAY[1], GATEWAY[2], GATEWAY[3]);
}

/// The RFC 1071 ones-complement internet checksum (used by IPv4, ICMP, UDP, TCP).
pub fn checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut i = 0;
    while i + 1 < data.len() {
        sum += u16::from_be_bytes([data[i], data[i + 1]]) as u32;
        i += 2;
    }
    if i < data.len() {
        sum += (data[i] as u32) << 8; // odd trailing byte, high-order aligned
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

/// Pull every frame the NIC has buffered and feed them up the stack.
/// Returns how many frames were processed. Safe to call from any task.
pub fn pump() -> usize {
    let mut n = 0;
    while let Some(frame) = e1000::poll_receive() {
        ethernet::handle(&frame);
        n += 1;
        if n >= 16 {
            break; // bound work per call so we yield the CPU promptly
        }
    }
    n
}

/// Our own MAC, or all-zero if the NIC never came up.
pub fn our_mac() -> MacAddr {
    MacAddr(e1000::mac_address().unwrap_or([0; 6]))
}

/// Wrap an L3 payload in an Ethernet frame addressed to `dst_mac` and send it.
pub fn transmit_ethernet(dst_mac: MacAddr, ethertype: u16, payload: &[u8]) -> bool {
    let frame = ethernet::frame(dst_mac, our_mac(), ethertype, payload);
    e1000::send(&frame)
}

/// Resolve the next-hop MAC for an IPv4 destination: a directly-connected host
/// from the ARP cache, otherwise the gateway (falling back to broadcast so the
/// teaching demo always produces a well-formed frame).
pub fn next_hop_mac(dst_ip: [u8; 4]) -> MacAddr {
    if let Some(m) = arp::lookup(dst_ip) {
        return m;
    }
    if let Some(m) = arp::lookup(GATEWAY) {
        return m;
    }
    MacAddr::BROADCAST
}

/// Round the stack once for a fixed set of crafted inbound frames and print
/// what each layer produced. This exercises decode + response construction
/// deterministically, independent of the (unobservable) wire TX.
pub fn demo() {
    crate::println!("[net] ---- Phase 8 layered-stack self test ----");
    let mac = our_mac();
    crate::println!(
        "[net] our MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac.0[0], mac.0[1], mac.0[2], mac.0[3], mac.0[4], mac.0[5]
    );
    arp::demo();
    ip::demo();
    icmp::demo();
    udp::demo();
    tcp::demo();
    socket::demo();
    crate::println!("[net] ---- self test complete ----");
}
