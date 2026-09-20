//! Ethernet II: the 14-byte link-layer header and EtherType demultiplexing.
//!
//! Frame layout: `dst[6] src[6] type[2] payload...`. We hand our own buffer to
//! the driver, so no Ethernet FCS is appended (the e1000 computes it on TX and
//! strips it on RX).

use alloc::vec::Vec;

use super::{arp, ip};

/// A 48-bit MAC address, kept as a newtype so it never confuses with an IPv4
/// `[u8; 4]` at the type level.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct MacAddr(pub [u8; 6]);

impl MacAddr {
    pub const BROADCAST: MacAddr = MacAddr([0xff; 6]);

    /// `aa:bb:cc:dd:ee:ff` rendering helper for the built-in commands.
    pub fn fmt(&self) -> Vec<u8> {
        let hex = b"0123456789abcdef";
        let mut out = Vec::with_capacity(17);
        for (i, b) in self.0.iter().enumerate() {
            if i > 0 {
                out.push(b':');
            }
            out.push(hex[(b >> 4) as usize]);
            out.push(hex[(b & 0xf) as usize]);
        }
        out
    }
}

// EtherType values we understand.
pub const ETHERTYPE_ARP: u16 = 0x0806;
pub const ETHERTYPE_IP: u16 = 0x0800;

const HEADER_LEN: usize = 14;

fn be16(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}

/// Build a complete Ethernet frame (header + payload copy).
pub fn frame(dst: MacAddr, src: MacAddr, ethertype: u16, payload: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(HEADER_LEN + payload.len());
    f.extend_from_slice(&dst.0);
    f.extend_from_slice(&src.0);
    f.extend_from_slice(&ethertype.to_be_bytes());
    f.extend_from_slice(payload);
    f
}

/// Decode one inbound frame and hand the payload to the right L3 protocol.
pub fn handle(frame: &[u8]) {
    if frame.len() < HEADER_LEN {
        return;
    }
    let ethertype = be16(&frame[12..14]);
    let payload = &frame[HEADER_LEN..];
    match ethertype {
        ETHERTYPE_ARP => arp::handle(payload),
        ETHERTYPE_IP => ip::handle(payload),
        other => crate::println!("[eth] unknown EtherType {:#06x} ({} bytes)", other, payload.len()),
    }
}
