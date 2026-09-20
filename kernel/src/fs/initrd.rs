//! Phase 6: `initrd` — a read-only filesystem parsed from a ustar tarball.
//!
//! We embed a small `initrd.tar` (see `tools/make_initrd.py`) and, at init,
//! walk its 512-byte headers to build an in-memory tree of [`Inode`]s. This is
//! the classic "initial ramdisk": a real on-disk format, parsed into the same
//! VFS traits as devfs/procfs, so files packed at build time look no different
//! from synthetic ones.

use alloc::{string::String, sync::Arc, vec::Vec};
use spin::Mutex;

use super::vfs::{FileKind, FileSystem, FsError, FsResult, Inode};

const BLOCK: usize = 512;

/// The initrd filesystem: holds the parsed root directory inode.
pub struct Initrd {
    root: Arc<InodeNode>,
}

impl Initrd {
    /// Parse a ustar byte stream into a filesystem.
    pub fn from_tar(tar: &[u8]) -> Self {
        Self {
            root: Arc::new(InodeNode::dir("/")),
        }
        .with_entries(parse(tar))
    }

    fn with_entries(self, entries: Vec<Entry>) -> Self {
        for e in entries {
            self.insert(e);
        }
        self
    }

    /// Place one parsed entry into the tree, creating parent dirs as needed.
    fn insert(&self, e: Entry) {
        let comps: Vec<&str> = e.path.split('/').filter(|c| !c.is_empty()).collect();
        if comps.is_empty() {
            return;
        }
        let mut cur = self.root.clone();
        for (i, comp) in comps.iter().enumerate() {
            let last = i == comps.len() - 1;
            if last {
                let node = if e.is_dir {
                    Arc::new(InodeNode::dir(comp))
                } else {
                    Arc::new(InodeNode::file(comp, e.data.clone()))
                };
                cur.children.lock().push(node);
            } else {
                cur = match find_child(&cur, comp) {
                    Some(c) => c,
                    None => {
                        let d = Arc::new(InodeNode::dir(comp));
                        cur.children.lock().push(d.clone());
                        d
                    }
                };
            }
        }
    }
}

impl FileSystem for Initrd {
    fn name(&self) -> &'static str {
        "initrd"
    }
    fn root(&self) -> Arc<dyn Inode> {
        self.root.clone()
    }
}

fn find_child(dir: &InodeNode, name: &str) -> Option<Arc<InodeNode>> {
    dir.children.lock().iter().find(|c| c.name == name).cloned()
}

/// One parsed tar entry: its full path, contents, and whether it's a dir.
struct Entry {
    path: String,
    data: Vec<u8>,
    is_dir: bool,
}

/// Walk a ustar archive, returning regular-file and directory entries.
fn parse(tar: &[u8]) -> Vec<Entry> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos + BLOCK <= tar.len() {
        let hdr = &tar[pos..pos + BLOCK];
        if hdr[0] == 0 {
            break; // start of the terminating zero block
        }
        let name = read_cstr(&hdr[0..100]);
        let size = octal(&hdr[124..136]) as usize;
        let typeflag = hdr[156];
        let body = pos + BLOCK;
        let blocks = (size + BLOCK - 1) / BLOCK;
        if !name.is_empty() {
            let is_dir = typeflag == b'5';
            let data = if is_dir {
                Vec::new()
            } else {
                tar[body..core::cmp::min(body + size, tar.len())].to_vec()
            };
            out.push(Entry {
                path: name,
                data,
                is_dir,
            });
        }
        pos = body + blocks * BLOCK;
    }
    out
}

/// A NUL-terminated C string within a fixed field.
fn read_cstr(field: &[u8]) -> String {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).into_owned()
}

/// Parse an ASCII-octal tar numeric field (ignoring trailing spaces/NULs).
fn octal(field: &[u8]) -> u64 {
    let mut value = 0u64;
    for &b in field {
        match b {
            b'0'..=b'7' => value = value * 8 + (b - b'0') as u64,
            b' ' | 0 => {}
            _ => break,
        }
    }
    value
}

/// A node in the parsed initrd tree (file or directory).
struct InodeNode {
    name: String,
    kind: FileKind,
    data: Vec<u8>,
    children: Mutex<Vec<Arc<InodeNode>>>,
}

impl InodeNode {
    fn dir(name: &str) -> Self {
        Self {
            name: String::from(name),
            kind: FileKind::Dir,
            data: Vec::new(),
            children: Mutex::new(Vec::new()),
        }
    }
    fn file(name: &str, data: Vec<u8>) -> Self {
        Self {
            name: String::from(name),
            kind: FileKind::File,
            data,
            children: Mutex::new(Vec::new()),
        }
    }
}

impl Inode for InodeNode {
    fn kind(&self) -> FileKind {
        self.kind
    }
    fn read(&self, offset: u64, buf: &mut [u8]) -> FsResult<usize> {
        let start = offset as usize;
        if start >= self.data.len() {
            return Ok(0);
        }
        let n = core::cmp::min(buf.len(), self.data.len() - start);
        buf[..n].copy_from_slice(&self.data[start..start + n]);
        Ok(n)
    }
    fn lookup(&self, name: &str) -> FsResult<Arc<dyn Inode>> {
        find_child(self, name)
            .map(|c| c as Arc<dyn Inode>)
            .ok_or(FsError::NotFound)
    }
    fn list_dir(&self) -> FsResult<Vec<String>> {
        Ok(self.children.lock().iter().map(|c| c.name.clone()).collect())
    }
}
