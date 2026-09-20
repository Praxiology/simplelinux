//! 串口（COM1）控制台输出。
//!
//! 16550 UART 映射在 I/O 端口 `0x3F8`。相比 VGA 显存，串口不依赖显示器，
//! 是 QEMU 无头（`-display none`）环境下最可靠的观测通道——内核所有
//! `println!` 最终都汇聚到这里。

use uart_16550::SerialPort;
use spin::Mutex;
use lazy_static::lazy_static;

lazy_static! {
    /// 全局唯一的 COM1 端口，用 `Mutex` 保护以保证多任务下输出不交错。
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

/// `print!`/`println!` 宏的底层实现：把格式化后的文本写入串口。
/// 标记 `#[doc(hidden)]` 因为它只应通过宏间接调用。
#[doc(hidden)]
pub fn _print(args: ::core::fmt::Arguments) {
    use core::fmt::Write;
    let mut serial = SERIAL1.lock();
    serial.write_fmt(args).expect("serial write failed");
}

/// 向串口控制台写入原始字节（`write` 系统调用使用：此处缓冲区是任意字节，
/// 不保证合法 UTF-8，故逐字节发送而非走格式化路径）。
/// Write raw bytes to the serial console (used by the `write` syscall, where the
/// buffer is arbitrary bytes and not guaranteed to be valid UTF-8).
pub fn write_bytes(bytes: &[u8]) {
    let mut serial = SERIAL1.lock();
    for &b in bytes {
        serial.send(b);
    }
}
