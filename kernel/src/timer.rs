//! PIT (8253/8254) timer, programmed to fire IRQ0 at ~100 Hz.

use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::instructions::port::Port;

/// PIT channel 0 data port.
const PIT_CHANNEL0: u16 = 0x40;
/// PIT command port.
const PIT_COMMAND: u16 = 0x43;
/// PIT base oscillator frequency (Hz).
const PIT_BASE_FREQUENCY: u64 = 1_193_182;
/// Desired timer frequency (Hz) -> a tick every 10 ms.
pub const HZ: u64 = 100;

/// Monotonic tick counter, incremented by the IRQ0 handler.
static TICKS: AtomicU64 = AtomicU64::new(0);

/// Number of timer interrupts since boot.
pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

/// Called from the IRQ0 handler to advance the clock.
pub(crate) fn tick() {
    TICKS.fetch_add(1, Ordering::Relaxed);
}

/// Program PIT channel 0 to generate `HZ` interrupts per second.
pub fn init() {
    let divisor = (PIT_BASE_FREQUENCY / HZ) as u16; // 11931 -> ~100 Hz
    unsafe {
        let mut command = Port::<u8>::new(PIT_COMMAND);
        // Channel 0 data port must receive the low byte then the high byte as
        // two separate 8-bit writes (a 16-bit outw would spill into port 0x41).
        let mut channel0 = Port::<u8>::new(PIT_CHANNEL0);
        // 0x36 = channel 0 | lobyte/hibyte | mode 3 (square wave) | binary.
        command.write(0x36);
        channel0.write(divisor as u8);
        channel0.write((divisor >> 8) as u8);
    }
    crate::println!("[timer] PIT channel 0 programmed to {} Hz (divisor={})", HZ, divisor);
}
