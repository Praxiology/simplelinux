//! Phase 7: Intel e1000 (82540EM) NIC driver, programmed via PCI + MMIO.
//!
//! Scope for this teaching kernel: enumerate the device on PCI bus 0, enable
//! memory + bus-mastering, map BAR0 (uncached), reset it, read its MAC, set up
//! legacy descriptor rings, then send/receive Ethernet frames by *polling* (no
//! device IRQ). Phase 8 builds the TCP/IP stack on top of [`send`] / [`poll_receive`].

use alloc::vec::Vec;
use spin::Mutex;

use x86_64::structures::paging::{Page, PhysFrame, Size4KiB};
use x86_64::structures::paging::page_table::PageTableFlags;
use x86_64::instructions::port::Port;
use x86_64::{PhysAddr, VirtAddr};

use crate::memory::vmm;

const VENDOR_INTEL: u16 = 0x8086;
const DEVICE_E1000: u16 = 0x100e;

// --- MMIO register offsets ---
const REG_CTRL: u32 = 0x0000;
const REG_STATUS: u32 = 0x0008;
const REG_IMC: u32 = 0x0068;
const REG_ICR: u32 = 0x00c0;
const REG_RCTL: u32 = 0x0100;
const REG_TCTL: u32 = 0x0400;
const REG_MTA: u32 = 0x2c00;
const REG_RAL0: u32 = 0x5400;
const REG_RAH0: u32 = 0x5404;
const REG_RDBAL: u32 = 0x2800;
const REG_RDBAH: u32 = 0x2804;
const REG_RDLEN: u32 = 0x2808;
const REG_RDH: u32 = 0x2810;
const REG_RDT: u32 = 0x2818;
const REG_TDBAL: u32 = 0x3800;
const REG_TDBAH: u32 = 0x3804;
const REG_TDLEN: u32 = 0x3808;
const REG_TDH: u32 = 0x3810;
const REG_TDT: u32 = 0x3818;

// --- register bits ---
const CTRL_SLU: u32 = 1 << 6; // set link up
const CTRL_RST: u32 = 1 << 26; // software reset
const STATUS_LU: u32 = 1 << 1; // link up
const RCTL_EN: u32 = 1 << 1;
const RCTL_UPE: u32 = 1 << 2;
const TCTL_EN: u32 = 1 << 0;

const NUM_RX: usize = 8;
const NUM_TX: usize = 8;
const DESC_DD: u8 = 0x01; // descriptor done
const TX_CMD: u8 = 0x19; // IFCS(0x01) | RS(0x08) | EOP(0x10)

/// Our hard-coded IPv4 identity for QEMU user-mode networking (Phase 8).
pub const IP: [u8; 4] = [10, 0, 2, 15];
pub const GATEWAY: [u8; 4] = [10, 0, 2, 2];

/// One legacy (16-byte) transmit/receive descriptor.
/// Intel legacy layout: addr[0..8] len[8..10] tos[10] cmd[11] status[12] css[13] special[14..16].
/// `cmd` (byte 11) drives transmit (EOP/IFCS/RS); `status` (byte 12) holds DD.
#[repr(C)]
#[derive(Clone, Copy)]
struct Desc {
    addr: u64,
    length: u16,
    tos: u8,
    cmd: u8,
    status: u8,
    css: u8,
    offset: u16,
}
const _: () = assert!(core::mem::size_of::<Desc>() == 16);

/// Everything the driver needs after init.
struct Nic {
    base: VirtAddr,
    mac: [u8; 6],
    rx_ring: *mut Desc,
    rx_bufs: Vec<(VirtAddr, u64)>, // (kernel virt, phys)
    rx_cursor: usize,
    tx_ring: *mut Desc,
    tx_buf: VirtAddr,
    tx_buf_phys: u64,
    tx_cursor: usize,
    rx_ring_phys: u64,
    tx_ring_phys: u64,
}

static NIC: Mutex<Option<Nic>> = Mutex::new(None);

// The descriptor pointers are raw only to keep the layout explicit; all access
// is serialized through `NIC` (a `Mutex`) on this single-core kernel.
unsafe impl Send for Nic {}

// --- PCI configuration-space access (ports 0xCF8 / 0xCFC) ---
unsafe fn pci_read(bus: u8, dev: u8, func: u8, offset: u8) -> u32 {
    let addr = 0x8000_0000u32
        | ((bus as u32) << 16)
        | ((dev as u32) << 11)
        | ((func as u32) << 8)
        | ((offset as u32) & 0xfc);
    Port::<u32>::new(0xcf8).write(addr);
    Port::<u32>::new(0xcfc).read()
}

unsafe fn pci_write(bus: u8, dev: u8, func: u8, offset: u8, value: u32) {
    let addr = 0x8000_0000u32
        | ((bus as u32) << 16)
        | ((dev as u32) << 11)
        | ((func as u32) << 8)
        | ((offset as u32) & 0xfc);
    Port::<u32>::new(0xcf8).write(addr);
    Port::<u32>::new(0xcfc).write(value);
}

/// Find an Intel e1000 on bus 0, returning (dev, func, BAR0 physical base).
unsafe fn find_device() -> Option<(u8, u8, u64)> {
    for dev in 0..32u8 {
        let vid = (pci_read(0, dev, 0, 0x00) & 0xffff) as u16;
        if vid == 0xffff {
            continue;
        }
        let did = (pci_read(0, dev, 0, 0x00) >> 16) as u16;
        if vid == VENDOR_INTEL && did == DEVICE_E1000 {
            let bar = pci_read(0, dev, 0, 0x10) as u64;
            // bit0=0 => memory BAR; mask off type bits.
            let base = bar & 0xffff_fff0;
            // Enable memory space (bit1) + bus mastering (bit2) in the command reg.
            let cmd = pci_read(0, dev, 0, 0x04);
            pci_write(0, dev, 0, 0x04, cmd | 0x06);
            return Some((dev, 0, base));
        }
    }
    None
}

/// Ensure `phys..phys+pages*4KiB` is reachable through the physical map, marked
/// uncached (device MMIO must not be write-back cached). The bootloader already
/// backs the whole physical range, so we only fill in pages still missing; the
/// DMA descriptor/buffer pages live in the normal identity map.
unsafe fn map_mmio(phys: u64, pages: u64) -> VirtAddr {
    let offset = vmm::phys_offset().expect("physical offset");
    let flags = PageTableFlags::PRESENT
        | PageTableFlags::WRITABLE
        | PageTableFlags::NO_CACHE
        | PageTableFlags::NO_EXECUTE;
    let virt = offset + phys;
    for i in 0..pages {
        let page = Page::<Size4KiB>::containing_address(virt + i * 4096);
        if vmm::translate(page.start_address()).is_none() {
            let frame = PhysFrame::<Size4KiB>::containing_address(PhysAddr::new(phys + i * 4096));
            vmm::map_page(page, frame, flags).expect("failed to map MMIO page");
        }
    }
    virt
}

/// Allocate one zeroed 4 KiB page; return (kernel virt, physical).
unsafe fn alloc_page(zero: bool) -> (VirtAddr, u64) {
    let frame = vmm::alloc_frame().expect("out of frames");
    let phys = frame.start_address().as_u64();
    let virt = vmm::phys_offset().expect("physical offset") + phys;
    if zero {
        core::ptr::write_bytes(virt.as_mut_ptr::<u8>(), 0, 4096);
    }
    (virt, phys)
}

impl Nic {
    unsafe fn write_reg(&self, off: u32, val: u32) {
        core::ptr::write_volatile((self.base.as_u64() + off as u64) as *mut u32, val);
    }
    unsafe fn read_reg(&self, off: u32) -> u32 {
        core::ptr::read_volatile((self.base.as_u64() + off as u64) as *mut u32)
    }
}

/// Discover, reset and configure the NIC; stores it for later send/receive.
///
/// # Safety
/// Requires memory (frames + physical map), GDT and IDT to be initialized.
pub unsafe fn init() {
    let (dev, func, bar) = match find_device() {
        Some(x) => x,
        None => {
            crate::println!("[e1000] no Intel e1000 found on PCI bus 0");
            return;
        }
    };
    crate::println!("[e1000] device found, BAR0 MMIO base = {:#x}", bar);
    // Verify bus-mastering + memory-space actually stuck in the PCI command reg
    // (without bus master the device cannot DMA its descriptor rings).
    let cmd = pci_read(0, dev, func, 0x04);
    crate::println!("[e1000] PCI cmd={:#x} (mem={}, busmaster={})", cmd, cmd & 0x02 != 0, cmd & 0x04 != 0);
    configure(bar);
}

/// The heavy bring-up, kept separate so `init` stays short.
unsafe fn configure(bar: u64) {
    let mut nic = build_nic(bar);
    // 1. All interrupts off, clear any pending.
    nic.write_reg(REG_IMC, 0x7fff_ffff);
    let _ = nic.read_reg(REG_ICR);
    // 2. Software reset first (this wipes all other CTRL bits), then link up.
    nic.write_reg(REG_CTRL, nic.read_reg(REG_CTRL) | CTRL_RST);
    for _ in 0..100_000 {
        if nic.read_reg(REG_CTRL) & CTRL_RST == 0 {
            break;
        }
    }
    // 2b. Re-assert "set link up" after the reset cleared it.
    nic.write_reg(REG_CTRL, nic.read_reg(REG_CTRL) | CTRL_SLU);
    // 3. Read the MAC programmed by the firmware/QEMU.
    let ral = nic.read_reg(REG_RAL0);
    let rah = nic.read_reg(REG_RAH0);
    nic.mac = [
        ral as u8,
        (ral >> 8) as u8,
        (ral >> 16) as u8,
        (ral >> 24) as u8,
        rah as u8,
        (rah >> 8) as u8,
    ];
    // 4. Clear the multicast filter table.
    for i in 0..128 {
        nic.write_reg(REG_MTA + i * 4, 0);
    }
    // 5. Receive ring: give the hardware every buffer.
    let rx_ring_phys = nic.rx_ring_phys;
    nic.write_reg(REG_RDBAL, rx_ring_phys as u32);
    nic.write_reg(REG_RDBAH, 0);
    nic.write_reg(REG_RDLEN, (NUM_RX * 16) as u32);
    nic.write_reg(REG_RDH, 0);
    nic.write_reg(REG_RDT, NUM_RX as u32 - 1); // last buffer hw may use
    nic.write_reg(REG_RCTL, RCTL_EN | RCTL_UPE);
    // 6. Transmit ring. Enable TX before the first tail write.
    let tx_ring_phys = nic.tx_ring_phys;
    nic.write_reg(REG_TDBAL, tx_ring_phys as u32);
    nic.write_reg(REG_TDBAH, 0);
    nic.write_reg(REG_TDLEN, (NUM_TX * 16) as u32);
    nic.write_reg(REG_TCTL, TCTL_EN);
    nic.write_reg(REG_TDH, 0);
    nic.write_reg(REG_TDT, 0);

    let mac = nic.mac;
    let link = nic.read_reg(REG_STATUS) & STATUS_LU != 0;
    *NIC.lock() = Some(nic);
    crate::println!(
        "[e1000] MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}, link {}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5],
        if link { "up" } else { "down" }
    );
}

/// Allocate rings + buffers and describe the descriptor layout for the device.
unsafe fn build_nic(bar: u64) -> Nic {
    let base = map_mmio(bar, 16);
    let (rx_ring_v, rx_ring_p) = alloc_page(true);
    let (tx_ring_v, tx_ring_p) = alloc_page(true);
    let mut rx_bufs = Vec::new();
    for _ in 0..NUM_RX {
        rx_bufs.push(alloc_page(false));
    }
    let (tx_buf, tx_buf_phys) = alloc_page(true);
    // Initialize RX descriptors to point at the buffers.
    let rx_ring = rx_ring_v.as_mut_ptr::<Desc>();
    for i in 0..NUM_RX {
        core::ptr::write_volatile(
            rx_ring.add(i),
            Desc {
                addr: rx_bufs[i].1,
                length: 0,
                tos: 0,
                cmd: 0,
                status: 0,
                css: 0,
                offset: 0,
            },
        );
    }
    Nic {
        base,
        mac: [0; 6],
        rx_ring,
        rx_bufs,
        rx_cursor: 0,
        tx_ring: tx_ring_v.as_mut_ptr::<Desc>(),
        tx_buf,
        tx_buf_phys,
        tx_cursor: 0,
        rx_ring_phys: rx_ring_p,
        tx_ring_phys: tx_ring_p,
    }
}

/// Send one Ethernet frame. Returns false if no NIC is present.
pub fn send(frame: &[u8]) -> bool {
    let mut guard = NIC.lock();
    let nic = match guard.as_mut() {
        Some(n) => n,
        None => return false,
    };
    unsafe {
        // Copy the frame into the shared TX buffer.
        core::ptr::copy_nonoverlapping(frame.as_ptr(), nic.tx_buf.as_mut_ptr::<u8>(), frame.len());
        let idx = nic.tx_cursor % NUM_TX;
        core::ptr::write_volatile(
            nic.tx_ring.add(idx),
            Desc {
                addr: nic.tx_buf_phys,
                length: frame.len() as u16,
                tos: 0,
                cmd: TX_CMD,
                status: 0,
                css: 0,
                offset: 0,
            },
        );
        nic.tx_cursor += 1;
        // Order the descriptor + buffer writes before the doorbell reaches the
        // device (which DMAs them as soon as TDT advances).
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
        let tail = (nic.tx_cursor % NUM_TX) as u32;
        nic.write_reg(REG_TDT, tail);
    }
    true
}

/// Poll the receive ring once; return the next complete frame if available.
pub fn poll_receive() -> Option<Vec<u8>> {
    let mut guard = NIC.lock();
    let nic = guard.as_mut()?;
    unsafe {
        let idx = nic.rx_cursor % NUM_RX;
        let d = core::ptr::read_volatile(nic.rx_ring.add(idx));
        if d.status & DESC_DD == 0 || d.length == 0 {
            return None;
        }
        let buf = nic.rx_bufs[idx].0;
        let out = Vec::from(core::slice::from_raw_parts(buf.as_ptr::<u8>(), d.length as usize));
        // Recycle: clear DD/length and hand the buffer back by advancing RDT.
        core::ptr::write_volatile(
            nic.rx_ring.add(idx),
            Desc {
                addr: nic.rx_bufs[idx].1,
                length: 0,
                tos: 0,
                cmd: 0,
                status: 0,
                css: 0,
                offset: 0,
            },
        );
        nic.write_reg(REG_RDT, idx as u32);
        nic.rx_cursor += 1;
        Some(out)
    }
}

/// The NIC's MAC address, if a device was found and initialized.
pub fn mac_address() -> Option<[u8; 6]> {
    NIC.lock().as_ref().map(|n| n.mac)
}

/// Diagnostics: summarize RX/TX ring ownership and the descriptor DD bits.
pub fn rx_debug() {
    let guard = NIC.lock();
    let nic = match guard.as_ref() {
        Some(n) => n,
        None => return,
    };
    unsafe {
        let mut dd = [0u8; NUM_RX];
        for i in 0..NUM_RX {
            dd[i] = core::ptr::read_volatile(&(*nic.rx_ring.add(i)).status);
        }
        let tx0 = core::ptr::read_volatile(&(*nic.tx_ring.add(0)));
        crate::println!(
            "[e1000] RX rdh={} rdt={} icr={:#x} desc_dd={:?}",
            nic.read_reg(REG_RDH),
            nic.read_reg(REG_RDT),
            nic.read_reg(REG_ICR),
            dd,
        );
        crate::println!(
            "[e1000] TX tdh={} tdt={} tctl={:#x} desc0(status={:#x} cmd={:#x} len={})",
            nic.read_reg(REG_TDH),
            nic.read_reg(REG_TDT),
            nic.read_reg(REG_TCTL),
            tx0.status,
            tx0.cmd,
            tx0.length,
        );
    }
}

// ARP frame helpers + the Phase-7 smoke test live in the `arp` submodule.
mod arp;
pub use arp::demo_arp;
