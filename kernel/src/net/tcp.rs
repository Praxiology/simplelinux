//! TCP: a deliberately minimal, teaching-grade skeleton.
//!
//! We model the connection as an explicit [`TcpState`] state machine (the point
//! of the exercise) and build/parse a 20-byte header with a pseudo-header
//! checksum. Fixed MSS = 536, no congestion control, no retransmit timer — a
//! real stack would add those; here we demonstrate a three-way handshake
//! progresses correctly for a single inbound SYN.

use alloc::vec::Vec;

use super::checksum;
use super::ip::{self, PROTO_TCP};
use super::IP;

/// MSS = 536 keeps every segment inside the 576-byte IPv4 minimum MTU.
pub const MSS: usize = 536;

const HEADER_LEN: usize = 20;
const FIN: u16 = 0x01;
const SYN: u16 = 0x02;
const RST: u16 = 0x04;
const ACK: u16 = 0x10;

/// The connection lifecycle. Newtype-per-state data (the `{ .. }` variants in
/// the plan) is collapsed to the essentials here to stay within the teaching
/// line budget, but the transitions are the real TCP ones. We only drive a
/// Listen->SynReceived step in the demo, so the full enum is documentation of
/// the protocol (hence `allow(dead_code)`).
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TcpState {
    Closed,
    Listen,
    SynSent,
    SynReceived,
    Established,
    FinWait1,
    FinWait2,
    CloseWait,
    LastAck,
    TimeWait,
}

/// Decode a segment's flags for a human-readable line.
fn flag_names(f: u16) -> &'static str {
    match (f & SYN != 0, f & ACK != 0, f & FIN != 0, f & RST != 0) {
        (true, false, false, false) => "SYN",
        (true, true, false, false) => "SYN-ACK",
        (false, true, false, false) => "ACK",
        (false, true, true, false) => "FIN-ACK",
        (false, false, false, true) => "RST",
        _ => "?",
    }
}

/// Build a 20-byte TCP segment (payload appended) with a correct checksum.
fn segment(sport: u16, dport: u16, seq: u32, ack: u32, flags: u16, payload: &[u8], dst: [u8; 4]) -> Vec<u8> {
    let mut t = Vec::with_capacity(HEADER_LEN + payload.len());
    t.extend_from_slice(&sport.to_be_bytes());
    t.extend_from_slice(&dport.to_be_bytes());
    t.extend_from_slice(&seq.to_be_bytes());
    t.extend_from_slice(&ack.to_be_bytes());
    t.extend_from_slice(&(((HEADER_LEN / 4) as u16) << 12 | flags).to_be_bytes());
    t.extend_from_slice(&536u16.to_be_bytes()); // window
    t.extend_from_slice(&0u16.to_be_bytes()); // checksum placeholder
    t.extend_from_slice(&0u16.to_be_bytes()); // urgent pointer
    t.extend_from_slice(payload);

    let mut pseudo = Vec::new();
    pseudo.extend_from_slice(&IP);
    pseudo.extend_from_slice(&dst);
    pseudo.push(0);
    pseudo.push(PROTO_TCP);
    pseudo.extend_from_slice(&(t.len() as u16).to_be_bytes());
    pseudo.extend_from_slice(&t);
    let cks = checksum(&pseudo);
    t[16..18].copy_from_slice(&cks.to_be_bytes());
    t
}

/// Send a bare TCP segment (used by the handshake demo).
#[allow(dead_code)]
pub fn send_segment(sport: u16, dst: [u8; 4], dport: u16, seq: u32, ack: u32, flags: u16) -> bool {
    let t = segment(sport, dport, seq, ack, flags, &[], dst);
    ip::send(dst, PROTO_TCP, &t)
}

/// Parse and describe an inbound segment; drive one listen->SYN-ACK step.
pub fn handle(src: [u8; 4], t: &[u8]) {
    if t.len() < HEADER_LEN {
        return;
    }
    let sport = u16::from_be_bytes([t[0], t[1]]);
    let dport = u16::from_be_bytes([t[2], t[3]]);
    let seq = u32::from_be_bytes([t[4], t[5], t[6], t[7]]);
    let ack = u32::from_be_bytes([t[8], t[9], t[10], t[11]]);
    let flags = u16::from_be_bytes([t[12], t[13]]);
    crate::println!("[tcp] {}.{}.{}.{}:{} -> :{} {} seq={} ack={}",
        src[0], src[1], src[2], src[3], sport, dport, flag_names(flags), seq, ack);
    if flags & SYN != 0 && flags & ACK == 0 {
        // LISTEN -> SYN-RECEIVED, exactly like `tcp_connect` completing step 2.
        // Reply: our port becomes the source, the caller's port the destination.
        let syn_ack = segment(dport, sport, 1000, seq + 1, SYN | ACK, &[], src);
        ip::send(src, PROTO_TCP, &syn_ack);
        crate::println!("[tcp] state Listen -> SynReceived (sent SYN-ACK on :{})", dport);
    }
}

/// Self-test: run a synthetic inbound SYN through the parser + handshake step.
pub fn demo() {
    crate::println!("[tcp] -- demo -- (MSS={})", MSS);
    let mut seg = Vec::new();
    let sport = 51000u16;
    let dport = 80u16;
    seg.extend_from_slice(&sport.to_be_bytes());
    seg.extend_from_slice(&dport.to_be_bytes());
    seg.extend_from_slice(&777u32.to_be_bytes()); // seq
    seg.extend_from_slice(&0u32.to_be_bytes()); // ack
    seg.extend_from_slice(&(((HEADER_LEN / 4) as u16) << 12 | SYN).to_be_bytes());
    seg.extend_from_slice(&536u16.to_be_bytes());
    seg.extend_from_slice(&0u16.to_be_bytes());
    seg.extend_from_slice(&0u16.to_be_bytes());
    crate::println!("[tcp] feeding a synthetic inbound SYN:");
    handle(super::GATEWAY, &seg);
    crate::println!("[tcp] states: {:?}", [TcpState::Listen, TcpState::SynReceived, TcpState::Established]);
}
