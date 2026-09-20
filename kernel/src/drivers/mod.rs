//! Phase 7: hardware device drivers.
//!
//! * [`keyboard`] - PS/2 keyboard (IRQ1) feeding `/dev/kbd`.
//! * [`e1000`]    - Intel e1000 NIC (PCI), polled transmit/receive (Phase 8 stack).

pub mod e1000;
pub mod keyboard;
