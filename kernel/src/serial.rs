use uart_16550::SerialPort;
use spin::Mutex;
use lazy_static::lazy_static;

lazy_static! {
    pub static ref SERIAL1: Mutex<SerialPort> = {
        let mut serial_port = unsafe { SerialPort::new(0x3F8) };
        serial_port.init();
        Mutex::new(serial_port)
    };
}

pub fn init() {
    // 触发 lazy_static 初始化
    let _ = &*SERIAL1;
}

#[doc(hidden)]
pub fn _print(args: ::core::fmt::Arguments) {
    use core::fmt::Write;
    let mut serial = SERIAL1.lock();
    serial.write_fmt(args).expect("serial write failed");
}

/// Write raw bytes to the serial console (used by the `write` syscall, where the
/// buffer is arbitrary bytes and not guaranteed to be valid UTF-8).
pub fn write_bytes(bytes: &[u8]) {
    let mut serial = SERIAL1.lock();
    for &b in bytes {
        serial.send(b);
    }
}
