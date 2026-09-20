//! Phase 7: hardware device drivers.
//!
//! Phase 7：硬件设备驱动。
//!   * `keyboard` —— PS/2 键盘（IRQ1），解码后喂给 `/dev/kbd`；
//!   * `e1000`    —— Intel e1000 网卡（PCI），轮询式收发（供 Phase 8 网络栈使用）。
//!
//! * [`keyboard`] - PS/2 keyboard (IRQ1) feeding `/dev/kbd`.
//! * [`e1000`]    - Intel e1000 NIC (PCI), polled transmit/receive (Phase 8 stack).

pub mod e1000;
pub mod keyboard;
