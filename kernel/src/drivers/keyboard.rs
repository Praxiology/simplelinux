//! Phase 7: PS/2 keyboard driver (IRQ1).
//!
//! Phase 7：PS/2 键盘驱动（IRQ1）。每次 IRQ1 从端口 `0x60` 读一个 scancode 字节，
//! 送入 `pc-keyboard` 状态机（扫描码集 1、US 104 键布局）解码，把得到的 ASCII
//! 字符压入环形缓冲。`devfs` 把该缓冲暴露为 `/dev/kbd`，于是 `read` 系统调用就能取到按键。
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

/// 键盘驱动状态：解码器 + 已解码字符的环形缓冲。
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
    /// 全局键盘状态，`Mutex` 保护（中断与读操作共享）。
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
