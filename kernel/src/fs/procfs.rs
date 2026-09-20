//! Phase 6: `procfs` — synthetic files over live kernel state.
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
