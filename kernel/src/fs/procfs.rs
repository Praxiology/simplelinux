//! Phase 6: `procfs` — synthetic files over live kernel state.
//!
//! Phase 6：`procfs`——叠在实时内核状态上的“合成文件”。挂载在 `/proc`，不存储
//! 任何数据：每次读文件时都从运行中的内核现场重新生成文本（如 Phase 3 的堆/帧
//! 统计、Phase 2 的 tick 数）。这展示了 VFS 如何让“信息”看起来与“文件”完全一样。
//!
//! Mounted at `/proc`. Nothing is stored: reading a file regenerates its text
//! from the running kernel (Phase 3 heap/frame stats, Phase 2 tick count). This
//! shows how the VFS lets "information" look exactly like "files".

use alloc::{
    format,
    string::{String, ToString},
    sync::Arc,
    vec,
    vec::Vec,
};

use super::vfs::{FileKind, FileSystem, FsError, FsResult, Inode};

/// The `procfs` filesystem object (stateless).
pub struct ProcFs;

impl FileSystem for ProcFs {
    fn name(&self) -> &'static str {
        "procfs"
    }
    fn root(&self) -> Arc<dyn Inode> {
        Arc::new(ProcDir)
    }
}

/// The `/proc` directory.
struct ProcDir;

impl Inode for ProcDir {
    fn kind(&self) -> FileKind {
        FileKind::Dir
    }
    fn lookup(&self, name: &str) -> FsResult<Arc<dyn Inode>> {
        match name {
            "meminfo" => Ok(Arc::new(ProcFile { gen: meminfo })),
            "uptime" => Ok(Arc::new(ProcFile { gen: uptime })),
            _ => Err(FsError::NotFound),
        }
    }
    fn list_dir(&self) -> FsResult<Vec<String>> {
        Ok(vec!["meminfo".to_string(), "uptime".to_string()])
    }
}

/// 只读合成文件：内容不存储，每次读取时由函数指针 `gen` 按需生成。
/// A read-only synthetic file whose bytes are produced on demand by `gen`.
struct ProcFile {
    gen: fn() -> String,
}

impl Inode for ProcFile {
    fn kind(&self) -> FileKind {
        FileKind::File
    }
    fn read(&self, offset: u64, buf: &mut [u8]) -> FsResult<usize> {
        let content = (self.gen)();
        let bytes = content.as_bytes();
        let start = offset as usize;
        if start >= bytes.len() {
            return Ok(0);
        }
        let n = core::cmp::min(buf.len(), bytes.len() - start);
        buf[..n].copy_from_slice(&bytes[start..start + n]);
        Ok(n)
    }
}

fn meminfo() -> String {
    use crate::memory::{heap, pmm};
    let free_frames = pmm::FRAME_ALLOCATOR.lock().free_count();
    format!(
        "MemTotal: {} KiB\nMemUsed: {} KiB\nMemFree: {} KiB\nFreeFrames: {}\n",
        heap::total_bytes() / 1024,
        heap::used_bytes() / 1024,
        heap::free_bytes() / 1024,
        free_frames,
    )
}

fn uptime() -> String {
    let secs = crate::timer::ticks() / crate::timer::HZ;
    format!("{} seconds since boot\n", secs)
}
