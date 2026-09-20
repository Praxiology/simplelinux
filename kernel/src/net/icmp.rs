//! ICMP: just enough of echo request/reply to power `ping`.
//!
//! Echo message layout: `type[1] code[1] checksum[2] id[2] seq[2] data...`.
//! The checksum covers the whole ICMP message (checksum field zeroed first).

use alloc::vec::Vec;

use super::checksum;
use super::ip::{self, PROTO_ICMP};

const ECHO_REPLY: u8 = 0;
const ECHO_REQUEST: u8 = 8;

/// Build an ICMP echo *request* (what `ping` sends).
pub fn echo_request(id: u16, seq: u16, payload: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(8 + payload.len());
    m.push(ECHO_REQUEST);
    m.push(0); // code
    m.extend_from_slice(&0u16.to_be_bytes()); // checksum placeholder
    m.extend_from_slice(&id.to_be_bytes());
    m.extend_from_slice(&seq.to_be_bytes());
    m.extend_from_slice(payload);
    let cks = checksum(&m);
    m[2..4].copy_from_slice(&cks.to_be_bytes());
    m
}

/// Turn an inbound echo *request* into the matching *reply* (swap id/seq, keep data).
fn echo_reply(req: &[u8]) -> Vec<u8> {
    let mut m = req.to_vec();
    m[0] = ECHO_REPLY;
    m[1] = 0;
    m[2] = 0;
    m[3] = 0;
    let cks = checksum(&m);
    m[2..4].copy_from_slice(&cks.to_be_bytes());
    m
}

/// Handle an ICMP message: answer echo requests, report echo replies.
pub fn handle(src: [u8; 4], msg: &[u8]) {
    if msg.len() < 8 {
        return;
    }
    match msg[0] {
        ECHO_REQUEST => {
            crate::println!("[icmp] echo request from {}.{}.{}.{} ({} data bytes) -> reply",
                src[0], src[1], src[2], src[3], msg.len() - 8);
            let reply = echo_reply(msg);
            ip::send(src, PROTO_ICMP, &reply);
        }
        ECHO_REPLY => {
            let id = u16::from_be_bytes([msg[4], msg[5]]);
            let seq = u16::from_be_bytes([msg[6], msg[7]]);
            crate::println!("[icmp] echo reply from {}.{}.{}.{} id={:#x} seq={}",
                src[0], src[1], src[2], src[3], id, seq);
        }
        t => crate::println!("[icmp] unhandled type {}", t),
    }
}

/// Send one ICMP echo request to `dst` (the outbound half of `ping`).
pub fn ping(dst: [u8; 4], seq: u16) {
    let req = echo_request(0x5f, seq, b"simplelinux-ping");
    crate::println!("[icmp] ping {}.{}.{}.{} seq={} ({} bytes)",
        dst[0], dst[1], dst[2], dst[3], seq, req.len());
    ip::send(dst, PROTO_ICMP, &req);
}

/// Self-test: build a request, decode it back as a reply, verify checksums.
pub fn demo() {
    crate::println!("[icmp] -- demo --");
    let req = echo_request(7, 1, b"hi");
    crate::println!("[icmp] echo request checksum ok = {}", checksum(&req) == 0);
    let reply = echo_reply(&req);
    crate::println!("[icmp] built reply type={} checksum ok = {}",
        reply[0], checksum(&reply) == 0);
    crate::println!("[icmp] handling a fabricated echo request from the gateway:");
    handle(super::GATEWAY, &req);
}
