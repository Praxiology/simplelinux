//! UDP: a connectionless datagram service. The 8-byte header is
//! `sport[2] dport[2] length[2] checksum[2]`; the checksum additionally covers
//! a 12-byte IPv4 pseudo-header so a misrouted datagram is detected.

use alloc::vec::Vec;

use super::checksum;
use super::ip::{self, PROTO_UDP};
use super::socket;
use super::IP;

const HEADER_LEN: usize = 8;

/// Send a UDP datagram from `src_port` to `dst:dst_port`.
pub fn send(src_port: u16, dst: [u8; 4], dst_port: u16, data: &[u8]) -> bool {
    let total = (HEADER_LEN + data.len()) as u16;
    let mut d = Vec::with_capacity(total as usize);
    d.extend_from_slice(&src_port.to_be_bytes());
    d.extend_from_slice(&dst_port.to_be_bytes());
    d.extend_from_slice(&total.to_be_bytes());
    d.extend_from_slice(&0u16.to_be_bytes()); // checksum placeholder
    d.extend_from_slice(data);

    // Checksum = internet checksum over (pseudo-header | datagram).
    let mut pseudo = Vec::with_capacity(12 + total as usize);
    pseudo.extend_from_slice(&IP);
    pseudo.extend_from_slice(&dst);
    pseudo.push(0);
    pseudo.push(PROTO_UDP);
    pseudo.extend_from_slice(&total.to_be_bytes());
    pseudo.extend_from_slice(&d);
    let cks = checksum(&pseudo);
    d[6..8].copy_from_slice(&cks.to_be_bytes());

    ip::send(dst, PROTO_UDP, &d)
}

/// Decode an inbound UDP datagram and deliver its payload to any socket bound
/// to the destination port.
pub fn handle(src: [u8; 4], d: &[u8]) {
    if d.len() < HEADER_LEN {
        return;
    }
    let sport = u16::from_be_bytes([d[0], d[1]]);
    let dport = u16::from_be_bytes([d[2], d[3]]);
    let length = u16::from_be_bytes([d[4], d[5]]) as usize;
    let end = length.max(HEADER_LEN).min(d.len());
    let payload = &d[HEADER_LEN..end];
    crate::println!("[udp] {}.{}.{}.{}:{} -> :{} ({} bytes)",
        src[0], src[1], src[2], src[3], sport, dport, payload.len());
    socket::deliver(dport, src, sport, payload);
}

/// Self-test: craft a datagram to a bound port and read it back through the
/// socket layer (the RX half of an `nc -u` session).
pub fn demo() {
    crate::println!("[udp] -- demo --");
    let sock = socket::socket(PROTO_UDP);
    socket::bind(sock, 9999);
    // Simulate an inbound segment (as the wire would deliver it) and drain it.
    let mut seg = Vec::new();
    seg.extend_from_slice(&7u16.to_be_bytes()); // sport
    seg.extend_from_slice(&9999u16.to_be_bytes()); // dport
    let msg = b"hello over udp";
    seg.extend_from_slice(&((HEADER_LEN + msg.len()) as u16).to_be_bytes());
    seg.extend_from_slice(&0u16.to_be_bytes());
    seg.extend_from_slice(msg);
    handle(super::GATEWAY, &seg);
    let mut buf = [0u8; 64];
    match socket::recvfrom(sock, &mut buf) {
        Some((n, from, fport)) => crate::println!(
            "[udp] socket {} recv {} bytes from {}.{}.{}.{}:{}: '{}'",
            sock, n, from[0], from[1], from[2], from[3], fport,
            core::str::from_utf8(&buf[..n]).unwrap_or("?")),
        None => crate::println!("[udp] socket {} got nothing", sock),
    }
    socket::close(sock);
}
