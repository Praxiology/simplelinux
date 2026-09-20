//! IPv4: build/parse the fixed 20-byte header (no options), checksum the
//! header, and demultiplex the payload by protocol number.

use alloc::vec::Vec;

use super::checksum;
use super::ethernet::ETHERTYPE_IP;
use super::{icmp, next_hop_mac, tcp, udp};
use super::{transmit_ethernet, IP};

pub const PROTO_ICMP: u8 = 1;
pub const PROTO_TCP: u8 = 6;
pub const PROTO_UDP: u8 = 17;

const IHL: u8 = 5; // 5 * 4 = 20-byte header
const HEADER_LEN: usize = 20;

/// Assemble a 20-byte IPv4 header with a correct checksum.
fn header(proto: u8, src: [u8; 4], dst: [u8; 4], total: u16) -> [u8; HEADER_LEN] {
    let mut h = [0u8; HEADER_LEN];
    h[0] = (4 << 4) | IHL;
    h[1] = 0; // DSCP/ECN
    h[2..4].copy_from_slice(&total.to_be_bytes());
    h[4..6].copy_from_slice(&1u16.to_be_bytes()); // identification
    h[6..8].copy_from_slice(&0x4000u16.to_be_bytes()); // flags: Don't Fragment
    h[8] = 64; // TTL
    h[9] = proto;
    // h[10..12] checksum starts zeroed for the computation
    h[12..16].copy_from_slice(&src);
    h[16..20].copy_from_slice(&dst);
    let cks = checksum(&h);
    h[10..12].copy_from_slice(&cks.to_be_bytes());
    h
}

/// Send an IPv4 packet carrying `payload` for `proto` to `dst`, resolving the
/// next-hop MAC via the ARP cache.
pub fn send(dst: [u8; 4], proto: u8, payload: &[u8]) -> bool {
    let total = (HEADER_LEN + payload.len()) as u16;
    let mut pkt = Vec::with_capacity(total as usize);
    pkt.extend_from_slice(&header(proto, IP, dst, total));
    pkt.extend_from_slice(payload);
    let mac = next_hop_mac(dst);
    transmit_ethernet(mac, ETHERTYPE_IP, &pkt)
}

fn be16(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}

/// Decode an inbound IPv4 packet and hand its payload to the L4 protocol.
pub fn handle(pkt: &[u8]) {
    if pkt.len() < HEADER_LEN || pkt[0] >> 4 != 4 {
        crate::println!("[ip] malformed packet ({} bytes)", pkt.len());
        return;
    }
    let ihl = (pkt[0] & 0x0f) as usize * 4;
    let total = be16(&pkt[2..4]) as usize;
    let proto = pkt[9];
    let src = [pkt[12], pkt[13], pkt[14], pkt[15]];
    let body = &pkt[ihl.max(HEADER_LEN)..total.min(pkt.len())];
    crate::println!("[ip] {} -> {} proto={} len={}",
        fmt(src), fmt(IP), proto, total);
    match proto {
        PROTO_ICMP => icmp::handle(src, body),
        PROTO_UDP => udp::handle(src, body),
        PROTO_TCP => tcp::handle(src, body),
        other => crate::println!("[ip] unsupported protocol {}", other),
    }
}

fn fmt(ip: [u8; 4]) -> alloc::string::String {
    use alloc::fmt::Write as _;
    let mut s = alloc::string::String::new();
    let _ = write!(s, "{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
    s
}

/// Self-test: report a route decision and round-trip a made-up inbound packet.
pub fn demo() {
    crate::println!("[ip] -- demo --");
    let mac = next_hop_mac(super::GATEWAY);
    crate::println!("[ip] next-hop MAC for gateway: {}",
        core::str::from_utf8(&mac.fmt()).unwrap_or("?"));
    // Build a well-formed ICMP echo-request-bearing datagram and decode it.
    let icmp_req = icmp::echo_request(0x1234, 1, b"simplelinux");
    let total = (HEADER_LEN + icmp_req.len()) as u16;
    let mut pkt = Vec::with_capacity(total as usize);
    pkt.extend_from_slice(&header(PROTO_ICMP, [10, 0, 2, 2], IP, total));
    pkt.extend_from_slice(&icmp_req);
    crate::println!("[ip] feeding a crafted inbound echo-request through the decoder:");
    handle(&pkt);
}
