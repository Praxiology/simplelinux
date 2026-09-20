//! ARP: map IPv4 -> MAC, with a small learning cache.
//!
//! The 28-byte ARP payload (after the Ethernet header) is:
//! `htype[2] ptype[2] hlen plen oper[2] sha[6] spa[4] tha[6] tpa[4]`.

use alloc::{collections::BTreeMap, vec::Vec};
use spin::Mutex;

use super::ethernet::{MacAddr, ETHERTYPE_ARP};
use super::{our_mac, transmit_ethernet, GATEWAY, IP};

const HTYPE_ETH: u16 = 1;
const PTYPE_IP: u16 = 0x0800;
const OP_REQUEST: u16 = 1;
const OP_REPLY: u16 = 2;

/// Known IPv4 -> MAC bindings, learned from traffic we receive.
static CACHE: Mutex<BTreeMap<[u8; 4], MacAddr>> = Mutex::new(BTreeMap::new());

fn be16(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}

/// Reset the cache (called from `net::init`).
pub fn init() {
    CACHE.lock().clear();
}

fn remember(ip: [u8; 4], mac: MacAddr) {
    CACHE.lock().insert(ip, mac);
}

/// Look up a cached MAC for `ip`.
pub fn lookup(ip: [u8; 4]) -> Option<MacAddr> {
    CACHE.lock().get(&ip).copied()
}

/// A snapshot of the cache for `arp -a`.
pub fn table() -> Vec<([u8; 4], MacAddr)> {
    CACHE.lock().iter().map(|(k, m)| (*k, *m)).collect()
}

/// Build a 28-byte ARP payload with the given opcode and addresses.
fn payload(oper: u16, sha: MacAddr, spa: [u8; 4], tha: MacAddr, tpa: [u8; 4]) -> Vec<u8> {
    let mut p = Vec::with_capacity(28);
    p.extend_from_slice(&HTYPE_ETH.to_be_bytes());
    p.extend_from_slice(&PTYPE_IP.to_be_bytes());
    p.push(6);
    p.push(4);
    p.extend_from_slice(&oper.to_be_bytes());
    p.extend_from_slice(&sha.0);
    p.extend_from_slice(&spa);
    p.extend_from_slice(&tha.0);
    p.extend_from_slice(&tpa);
    p
}

/// Send an ARP "who-has <target>" broadcast from our own address.
pub fn request(target: [u8; 4]) {
    let me = our_mac();
    let pkt = payload(OP_REQUEST, me, IP, MacAddr([0; 6]), target);
    transmit_ethernet(MacAddr::BROADCAST, ETHERTYPE_ARP, &pkt);
    crate::println!("[arp] who-has {}.{}.{}.{} tell {}.{}.{}.{}",
        target[0], target[1], target[2], target[3], IP[0], IP[1], IP[2], IP[3]);
}

/// Decode an inbound ARP payload: learn the sender, and answer requests aimed
/// at our IP with a unicast reply.
pub fn handle(arp: &[u8]) {
    if arp.len() < 28 {
        return;
    }
    let oper = be16(&arp[6..8]);
    let sha = MacAddr([arp[8], arp[9], arp[10], arp[11], arp[12], arp[13]]);
    let spa = [arp[14], arp[15], arp[16], arp[17]];
    let tpa = [arp[24], arp[25], arp[26], arp[27]];
    remember(spa, sha); // every ARP frame teaches us the sender's binding
    crate::println!("[arp] learned {}.{}.{}.{} -> {}", spa[0], spa[1], spa[2], spa[3],
        core::str::from_utf8(&sha.fmt()).unwrap_or("?"));
    if oper == OP_REQUEST && tpa == IP {
        let pkt = payload(OP_REPLY, our_mac(), IP, sha, spa);
        transmit_ethernet(sha, ETHERTYPE_ARP, &pkt);
        crate::println!("[arp] reply {}.{}.{}.{} is at {}", IP[0], IP[1], IP[2], IP[3],
            core::str::from_utf8(&our_mac().fmt()).unwrap_or("?"));
    }
}

/// Deterministic self-test: fabricate a gateway ARP reply, push it through the
/// decoder, and confirm the cache learned the binding.
pub fn demo() {
    crate::println!("[arp] -- demo --");
    // Pretend the gateway answers our Phase-7 request.
    let reply = payload(OP_REPLY, MacAddr([0x52, 0x54, 0x00, 0x12, 0x34, 0x02]), GATEWAY, our_mac(), IP);
    handle(&reply);
    match lookup(GATEWAY) {
        Some(m) => crate::println!("[arp] gateway resolved to {} (cache size {})",
            core::str::from_utf8(&m.fmt()).unwrap_or("?"), table().len()),
        None => crate::println!("[arp] gateway unresolved"),
    }
}
