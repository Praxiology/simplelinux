//! VGA 文本模式（80×25）屏幕驱动。
//!
//! 文本模式下显存映射在物理地址 `0xB8000`：每个字符占 2 字节——低字节是
//! ASCII 码，高字节是颜色属性（低 4 位前景色、高 4 位背景色）。本驱动把
//! 字符写入最后一行，行满时整体上移一行实现“滚屏”。它是继串口之后的
//! 第二条输出通道，主要用于早期（Phase 0.5）引导阶段的人机可见反馈。

use core::fmt;
use lazy_static::lazy_static;
use spin::Mutex;

/// 文本屏幕的行数。
const BUFFER_HEIGHT: usize = 25;
/// 文本屏幕的列数。
const BUFFER_WIDTH: usize = 80;
/// VGA 文本显存的线性起始地址（每字符 2 字节）。
const VGA_BUFFER: *mut u8 = 0xb8000 as *mut u8;

/// 16 种标准 VGA 颜色，枚举值即颜色编码（0..15）。
#[allow(dead_code)]
#[repr(u8)]
#[derive(Clone, Copy)]
pub enum Color {
    Black = 0,
    Blue = 1,
    Green = 2,
    Cyan = 3,
    Red = 4,
    Magenta = 5,
    Brown = 6,
    LightGray = 7,
    DarkGray = 8,
    LightBlue = 9,
    LightGreen = 10,
    LightCyan = 11,
    LightRed = 12,
    Pink = 13,
    Yellow = 14,
    White = 15,
}

/// 显存中的一个字符单元：1 字节 ASCII + 1 字节颜色，`#[repr(C)]` 保证布局。
#[derive(Clone, Copy)]
#[repr(C)]
struct ScreenChar {
    ascii_character: u8,
    color_code: u8,
}

/// 屏幕写入游标。我们只把文本固定输出在最后一行，故仅需记录列位置。
pub struct Writer {
    column_position: usize,
    color_code: u8,
}

impl Writer {
    /// 写入一个字符到 VGA 缓冲区
    fn write_char(&mut self, row: usize, col: usize, ch: ScreenChar) {
        let offset = (row * BUFFER_WIDTH + col) * 2;
        unsafe {
            core::ptr::write_volatile(VGA_BUFFER.add(offset) as *mut u8, ch.ascii_character);
            core::ptr::write_volatile(VGA_BUFFER.add(offset + 1) as *mut u8, ch.color_code);
        }
    }

    /// 从 VGA 缓冲区读取一个字符
    fn read_char(&self, row: usize, col: usize) -> ScreenChar {
        let offset = (row * BUFFER_WIDTH + col) * 2;
        unsafe {
            ScreenChar {
                ascii_character: core::ptr::read_volatile(VGA_BUFFER.add(offset) as *const u8),
                color_code: core::ptr::read_volatile(VGA_BUFFER.add(offset + 1) as *const u8),
            }
        }
    }

    /// 写入一个字节：遇换行则滚屏，否则写到当前列并右移（超宽自动换行）。
    pub fn write_byte(&mut self, byte: u8) {
        match byte {
            b'\n' => self.new_line(),
            byte => {
                if self.column_position >= BUFFER_WIDTH {
                    self.new_line();
                }
                let row = BUFFER_HEIGHT - 1;
                let col = self.column_position;
                self.write_char(row, col, ScreenChar {
                    ascii_character: byte,
                    color_code: self.color_code,
                });
                self.column_position += 1;
            }
        }
    }

    fn new_line(&mut self) {
        // 滚屏：把每行上移一行
        for row in 1..BUFFER_HEIGHT {
            for col in 0..BUFFER_WIDTH {
                let character = self.read_char(row, col);
                self.write_char(row - 1, col, character);
            }
        }
        // 清空最后一行
        self.clear_row(BUFFER_HEIGHT - 1);
        self.column_position = 0;
    }

    fn clear_row(&mut self, row: usize) {
        let blank = ScreenChar {
            ascii_character: b' ',
            color_code: self.color_code,
        };
        for col in 0..BUFFER_WIDTH {
            self.write_char(row, col, blank);
        }
    }

    /// 设置前景/背景色，拼成显存颜色字节（背景在高 4 位）。
    pub fn set_color(&mut self, foreground: Color, background: Color) {
        self.color_code = (background as u8) << 4 | (foreground as u8);
    }
}

/// 接入 `core::fmt::Write`，使 `write!`/`println!` 能直接输出到 VGA。
impl fmt::Write for Writer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            self.write_byte(byte);
        }
        Ok(())
    }
}

lazy_static! {
    /// 全局唯一的屏幕写入器；`Mutex` 保证并发写不撕裂。默认黑底浅绿字。
    pub static ref WRITER: Mutex<Writer> = Mutex::new(Writer {
        column_position: 0,
        color_code: (Color::Black as u8) << 4 | (Color::LightGreen as u8),
    });
}

/// 初始化屏幕：清空所有行并把游标归零。
pub fn init() {
    // 清屏
    let mut writer = WRITER.lock();
    for row in 0..BUFFER_HEIGHT {
        writer.clear_row(row);
    }
    writer.column_position = 0;
}
