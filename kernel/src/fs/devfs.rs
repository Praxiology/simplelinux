//! Phase 6: `devfs` — device files behind the VFS interface.
//!
//! Phase 6：`devfs`——把设备伪装成文件接入 VFS。挂载在 `/dev`：写 `/dev/console`
//! 直达串口；`/dev/null` 吞掉一切；`/dev/zero` 读出无限 `\0`；键盘输入 `/dev/kbd`
//! 在 Phase 7 接入。它们都是无状态结构体，靠 `impl Inode` 提供各自的读写行为。
//!
//! Mounted at `/dev`. Writing to `/dev/console` goes to the serial port;
//! `/dev/null` swallows everything; `/dev/zero` yields infinite `\0` on read.
//! Keyboard input (`/dev/kbd`) arrives in Phase 7.

use alloc::{string::String, sync::Arc, vec, vec::Vec};

use super::vfs::{FileKind, FileSystem, FsResult, Inode};

/// The `devfs` filesystem object (stateless).
pub struct DevFs;

impl FileSystem for DevFs {
    fn name(&self) -> &'static str {
        "devfs"
    }
    fn root(&self) -> Arc<dyn Inode> {
        Arc::new(DevDir)
    }
}

/// The `/dev` directory: knows its three device children by name.
struct DevDir;

impl Inode for DevDir {
    fn kind(&self) -> FileKind {
        FileKind::Dir
    }
    fn lookup(&self, name: &str) -> FsResult<Arc<dyn Inode>> {
        match name {
            "console" => Ok(Arc::new(Console)),
            "null" => Ok(Arc::new(Null)),
            "zero" => Ok(Arc::new(Zero)),
            "kbd" => Ok(Arc::new(Kbd)),
            _ => Err(super::vfs::FsError::NotFound),
        }
    }
    fn list_dir(&self) -> FsResult<Vec<String>> {
        Ok(vec![
            String::from("console"),
            String::from("null"),
            String::from("zero"),
            String::from("kbd"),
        ])
    }
}

/// `/dev/console`: writes go straight to the serial port.
struct Console;

impl Inode for Console {
    fn kind(&self) -> FileKind {
        FileKind::CharDev
    }
    fn read(&self, _offset: u64, _buf: &mut [u8]) -> FsResult<usize> {
        // No input device wired up yet (Phase 7 adds the keyboard).
        Ok(0)
    }
    fn write(&self, _offset: u64, data: &[u8]) -> FsResult<usize> {
        crate::serial::write_bytes(data);
        Ok(data.len())
    }
}

/// `/dev/null`: reads nothing, discards writes.
struct Null;

impl Inode for Null {
    fn kind(&self) -> FileKind {
        FileKind::CharDev
    }
    fn read(&self, _offset: u64, _buf: &mut [u8]) -> FsResult<usize> {
        Ok(0)
    }
    fn write(&self, _offset: u64, data: &[u8]) -> FsResult<usize> {
        Ok(data.len())
    }
}

/// `/dev/zero`: reads return zero bytes, discards writes.
struct Zero;

impl Inode for Zero {
    fn kind(&self) -> FileKind {
        FileKind::CharDev
    }
    fn read(&self, _offset: u64, buf: &mut [u8]) -> FsResult<usize> {
        for b in buf.iter_mut() {
            *b = 0;
        }
        Ok(buf.len())
    }
    fn write(&self, _offset: u64, data: &[u8]) -> FsResult<usize> {
        Ok(data.len())
    }
}

/// `/dev/kbd`: reads decoded keypress characters from the keyboard driver.
struct Kbd;

impl Inode for Kbd {
    fn kind(&self) -> FileKind {
        FileKind::CharDev
    }
    fn read(&self, _offset: u64, buf: &mut [u8]) -> FsResult<usize> {
        // Non-blocking: fill with however many keys are buffered (0 if none).
        let mut n = 0;
        while n < buf.len() {
            match crate::drivers::keyboard::read_key() {
                Some(b) => {
                    buf[n] = b;
                    n += 1;
                }
                None => break,
            }
        }
        Ok(n)
    }
}
