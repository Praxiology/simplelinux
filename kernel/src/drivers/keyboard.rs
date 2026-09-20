//! Phase 7: PS/2 keyboard driver (IRQ1).
//!
//! On every IRQ1 we read one scancode byte from port `0x60`, run it through the
//! `pc-keyboard` state machine (scancode set 1, US 104-key layout), and push the
//! decoded ASCII character onto a ring buffer. `devfs` exposes that buffer as
//! `/dev/kbd` so a `read` syscall picks characters up.

use alloc::collections::VecDeque;

use lazy_static::lazy_static;
use pc_keyboard::{
    layouts, DecodedKey, HandleControl, Keyboard, ScancodeSet1,
};
use spin::Mutex;
use x86_64::instructions::port::Port;

type Kbd = Keyboard<layouts::Us104Key, ScancodeSet1>;

struct KeyboardState {
    decoder: Kbd,
    buffer: VecDeque<u8>,
}

impl KeyboardState {
    fn new() -> Self {
        let decoder = Keyboard::new(
            ScancodeSet1::new(),
            layouts::Us104Key,
            HandleControl::Ignore,
        );
        Self {
            decoder,
            buffer: VecDeque::new(),
        }
    }
}

lazy_static! {
    static ref KEYBOARD: Mutex<KeyboardState> = Mutex::new(KeyboardState::new());
}

/// IRQ1 handler: translate the scancode and queue any printable character.
pub fn interrupt_handler() {
    let scancode: u8 = unsafe { Port::new(0x60).read() };
    handle_scancode(scancode);
}

/// Decode one raw scancode and enqueue any resulting ASCII character.
fn handle_scancode(scancode: u8) {
    let mut kb = KEYBOARD.lock();
    // `add_byte` may return a complete key event once enough bytes arrived.
    if let Ok(Some(event)) = kb.decoder.add_byte(scancode) {
        if let Some(decoded) = kb.decoder.process_keyevent(event) {
            if let DecodedKey::Unicode(ch) = decoded {
                push(&mut kb.buffer, ch as u32);
            }
        }
    }
}

/// Test hook: feed raw scancodes as if the user typed them. Headless QEMU has
/// no PS/2 host input, so this lets us exercise the full decode + buffer +
/// `/dev/kbd` read path deterministically.
pub fn inject(scancodes: &[u8]) {
    for &s in scancodes {
        handle_scancode(s);
    }
}

/// Test hook: queue a whole ASCII string as if it were typed. Bypasses the
/// scancode decoder so a scripted command line (e.g. driving the Phase-8 shell)
/// is fully deterministic in headless QEMU.
pub fn inject_str(s: &str) {
    let mut kb = KEYBOARD.lock();
    for ch in s.chars() {
        push(&mut kb.buffer, ch as u32);
    }
}

/// Queue a decoded code point (ASCII range only for this teaching kernel).
fn push(buf: &mut VecDeque<u8>, cp: u32) {
    if cp < 0x80 {
        let _ = buf.push_back(cp as u8);
    }
}

/// Pop one buffered byte, or `None` if there is no keypress waiting.
pub fn read_key() -> Option<u8> {
    KEYBOARD.lock().buffer.pop_front()
}
