//! Phase 7/8: ARP frame construction + the NIC smoke test, layered on the raw
//! e1000 [`super::send`] / [`super::poll_receive`] byte pipes.

use alloc::vec::Vec;

use super::{mac_address, poll_receive, rx_debug, send, GATEWAY, IP};

/// Build an ARP request: "who-has <target>, tell <IP>" from our MAC.
pub fn arp_request(mac: &[u8; 6], target: [u8; 4]) -> Vec<u8> {
    let mut f = Vec::new();
    f.extend_from_slice(&[0xff; 6]); // dst: broadcast
    f.extend_from_slice(mac); // src
    f.extend_from_slice(&[0x08, 0x06]); // EtherType = ARP
    f.extend_from_slice(&[0x00, 0x01]); // hw type: Ethernet
    f.extend_from_slice(&[0x08, 0x00]); // proto type: IPv4
    f.push(6);
    f.push(4); // hlen, plen
    f.extend_from_slice(&[0x00, 0x01]); // oper: request
    f.extend_from_slice(mac); // sender MAC
    f.extend_from_slice(&IP); // sender IP
    f.extend_from_slice(&[0; 6]); // target MAC (unknown)
    f.extend_from_slice(&target); // target IP
    f
}

/// Is `frame` an ARP reply (EtherType 0x0806, oper 2)?
pub fn is_arp_reply(frame: &[u8]) -> bool {
    frame.len() >= 42
        && frame[12] == 0x08
        && frame[13] == 0x06
        && frame[21] == 0x02
}

/// Phase-7 smoke test: ARP the gateway, then poll the RX ring for the reply.
/// Must be called from a scheduler task so `sleep` can hand the CPU back while
/// we wait real milliseconds for the packet to come back.
pub fn demo_arp() {
    let mac = match mac_address() {
        Some(m) => m,
        None => {
            crate::println!("[e1000] no NIC; skipping ARP demo");
            return;
        }
    };
    let req = arp_request(&mac, GATEWAY);
    crate::println!(
        "[e1000] ARP request who-has {}.{}.{}.{} ({} bytes)",
        GATEWAY[0], GATEWAY[1], GATEWAY[2], GATEWAY[3], req.len()
    );
    if !send(&req) {
        crate::println!("[e1000] send failed");
        return;
    }
    // Give the network a few hundred ms, sleeping between poll bursts so other
    // tasks keep running and the PIT keeps ticking.
    for _ in 0..40 {
        crate::process::sleep(5); // 50 ms
        while let Some(frame) = poll_receive() {
            if is_arp_reply(&frame) {
                crate::println!(
                    "[e1000] ARP reply: {}.{}.{}.{} is at {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                    frame[28], frame[29], frame[30], frame[31],
                    frame[22], frame[23], frame[24], frame[25], frame[26], frame[27]
                );
                return;
            }
        }
    }
    crate::println!("[e1000] no ARP reply seen on this QEMU build (TX-completion limitation)");
    rx_debug();
}
