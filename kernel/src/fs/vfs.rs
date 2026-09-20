//! Phase 6: the "everything is a file" abstraction.
//!
//! Two traits drive the whole VFS:
//!   * [`FileSystem`] - a mountable tree exposing a single root [`Inode`].
//!   * [`Inode`]      - a node that can be read/written/looked-up/listed.
//!
//! Concrete filesystems (devfs, procfs, initrd) `impl` these; the mount table +
//! [`resolve`] turn a path like `/dev/console` into an `Arc<dyn Inode>`. This
//! trait-object design is exactly how a real kernel erases the many underlying
//! file implementations behind one interface.

use alloc::{string::String, sync::Arc, vec::Vec};
use spin::Mutex;

/// Errors a filesystem operation can report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsError {
    NotFound,
    PermissionDenied,
    NotDir,
    IsDir,
    InvalidArg,
    NoSpace,
    Io,
}

impl FsError {
    /// Map to a negative errno-style code for the syscall return value.
    pub fn errno(self) -> i64 {
        match self {
            FsError::Io => -5,
            FsError::NotFound => -2,
            FsError::PermissionDenied => -13,
            FsError::NotDir => -20,
            FsError::IsDir => -21,
            FsError::InvalidArg => -22,
            FsError::NoSpace => -28,
        }
    }
}

pub type FsResult<T> = Result<T, FsError>;

/// What kind of node an inode is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    File,
    Dir,
    CharDev,
}

/// An open file: an inode plus a per-open read/write cursor.
pub struct File {
    pub inode: Arc<dyn Inode>,
    pub offset: u64,
}

/// A single node in a filesystem tree.
pub trait Inode: Send + Sync {
    fn kind(&self) -> FileKind;

    fn read(&self, _offset: u64, _buf: &mut [u8]) -> FsResult<usize> {
        Err(FsError::PermissionDenied)
    }
    fn write(&self, _offset: u64, _data: &[u8]) -> FsResult<usize> {
        Err(FsError::PermissionDenied)
    }
    /// Find a child by name (directories only).
    fn lookup(&self, _name: &str) -> FsResult<Arc<dyn Inode>> {
        Err(FsError::NotFound)
    }
    /// List child names (directories only); default reports "not a directory".
    fn list_dir(&self) -> FsResult<Vec<String>> {
        Err(FsError::NotDir)
    }
    /// Network sockets override this to expose their socket-table index so the
    /// `bind`/`connect` syscalls can find them through the fd (trait polymorphism
    /// instead of downcasting). Regular files return `None`.
    fn sock_index(&self) -> Option<usize> {
        None
    }
}

/// A mountable filesystem that exposes a root inode.
pub trait FileSystem: Send + Sync {
    fn name(&self) -> &'static str;
    fn root(&self) -> Arc<dyn Inode>;
}

/// The global mount table: `(prefix, filesystem)`, e.g. `("/dev", devfs)`.
static MOUNTS: Mutex<Vec<(&'static str, Arc<dyn FileSystem>)>> = Mutex::new(Vec::new());

/// Register `fs` at absolute mount `prefix` (no trailing slash, e.g. `/dev`).
pub fn mount(prefix: &'static str, fs: Arc<dyn FileSystem>) {
    crate::println!("[vfs] mounted {} at {}", fs.name(), prefix);
    MOUNTS.lock().push((prefix, fs));
}

/// Does mount `prefix` cover `path` as a whole path component?
fn covers(prefix: &str, path: &str) -> bool {
    path == prefix
        || (path.starts_with(prefix)
            && path.as_bytes().get(prefix.len()) == Some(&b'/'))
}

/// Longest matching mount prefix (so `/dev` beats `/` if both were mounted).
fn longest_mount(path: &str) -> Option<(usize, Arc<dyn FileSystem>)> {
    let mut best: Option<(usize, Arc<dyn FileSystem>)> = None;
    for (prefix, fs) in MOUNTS.lock().iter() {
        if covers(prefix, path) && best.as_ref().map_or(true, |&(l, _)| prefix.len() >= l) {
            best = Some((prefix.len(), fs.clone()));
        }
    }
    best
}

/// Resolve an absolute path to an inode by matching a mount then walking
/// each `/`-separated component through [`Inode::lookup`].
pub fn resolve(path: &str) -> FsResult<Arc<dyn Inode>> {
    if !path.starts_with('/') {
        return Err(FsError::InvalidArg);
    }
    let (plen, fs) = longest_mount(path).ok_or(FsError::NotFound)?;
    let mut node = fs.root();
    for comp in path[plen..].split('/').filter(|c| !c.is_empty()) {
        node = node.lookup(comp)?;
    }
    Ok(node)
}
