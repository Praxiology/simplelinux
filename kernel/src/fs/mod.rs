//! Phase 6: virtual filesystem entry point.
//!
//! Wires the concrete filesystems into the global mount table and provides the
//! small file-descriptor layer the syscalls sit on top of:
//!   * [`open`] / [`close`]  - allocate a descriptor over a resolved inode
//!   * [`read`] / [`write`]  - move bytes and advance the descriptor offset
//!   * [`list_dir`]          - directory enumeration
//!
//! Permissions are expressed through the [`Inode`] trait itself: `read`/`write`
//! default to `PermissionDenied`, so a read-only file (procfs, initrd) simply
//! never overrides `write` — no separate permission bits needed.

pub mod devfs;
pub mod initrd;
pub mod procfs;
pub mod vfs;

use alloc::{string::String, sync::Arc, vec::Vec};
use spin::Mutex;

use devfs::DevFs;
use initrd::Initrd;
use procfs::ProcFs;
use vfs::{File, FsError, FsResult};

/// Embedded initial ramdisk (ustar), produced by `tools/make_initrd.py`.
pub static INITRD: &[u8] = include_bytes!("../../../user/initrd.tar");

/// Per-"process" descriptor table. We have a single active user program at a
/// time in this teaching kernel, so one global table stands in for the planned
/// per-task table.
const MAX_FDS: usize = 64;
static FD_TABLE: Mutex<Vec<Option<File>>> = Mutex::new(Vec::new());

/// Mount the filesystems and seed stdin/stdout/stderr onto fds 0/1/2.
pub fn init() {
    vfs::mount("/dev", Arc::new(DevFs));
    vfs::mount("/proc", Arc::new(ProcFs));
    vfs::mount("/initrd", Arc::new(Initrd::from_tar(INITRD)));

    let console = vfs::resolve("/dev/console").expect("devfs must provide /dev/console");
    for _ in 0..3 {
        install(File {
            inode: console.clone(),
            offset: 0,
        })
        .expect("no free fd for std stream");
    }
    crate::println!("[vfs] /dev, /proc, /initrd mounted; fds 0-2 -> /dev/console");
}

fn install(f: File) -> Option<usize> {
    let mut t = FD_TABLE.lock();
    if let Some(i) = t.iter().position(|slot| slot.is_none()) {
        t[i] = Some(f);
        return Some(i);
    }
    if t.len() < MAX_FDS {
        t.push(Some(f));
        return Some(t.len() - 1);
    }
    None
}

/// Open `path` and return a fresh descriptor.
pub fn open(path: &str) -> FsResult<usize> {
    let inode = vfs::resolve(path)?;
    install(File { inode, offset: 0 }).ok_or(FsError::NoSpace)
}

/// Read up to `buf.len()` bytes at the descriptor's offset into `buf`.
pub fn read(fd: usize, buf: &mut [u8]) -> FsResult<usize> {
    let mut t = FD_TABLE.lock();
    let f = t.get_mut(fd).and_then(|o| o.as_mut()).ok_or(FsError::InvalidArg)?;
    let n = f.inode.read(f.offset, buf)?;
    f.offset += n as u64;
    Ok(n)
}

/// Write `data` at the descriptor's offset, advancing it.
pub fn write(fd: usize, data: &[u8]) -> FsResult<usize> {
    let mut t = FD_TABLE.lock();
    let f = t.get_mut(fd).and_then(|o| o.as_mut()).ok_or(FsError::InvalidArg)?;
    let n = f.inode.write(f.offset, data)?;
    f.offset += n as u64;
    Ok(n)
}

/// Release descriptor `fd`.
pub fn close(fd: usize) -> FsResult<()> {
    let mut t = FD_TABLE.lock();
    match t.get_mut(fd) {
        Some(slot) if slot.is_some() => {
            *slot = None;
            Ok(())
        }
        _ => Err(FsError::InvalidArg),
    }
}

/// List the entries of directory `path`.
pub fn list_dir(path: &str) -> FsResult<Vec<String>> {
    vfs::resolve(path)?.list_dir()
}
