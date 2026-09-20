//! BSD-style sockets layered on the stack, exposed through the VFS.
//!
//! The teaching point is "a network endpoint is a file": a [`Socket`] is wrapped
//! in a [`SocketInode`] that `impl`s the Phase-6 [`Inode`] trait, so the ordinary
//! `read`/`write`/`close` syscalls move datagrams too. `bind`/`connect` reach the
//! socket through a new [`Inode::sock_index`] hook (trait polymorphism, no casts).

use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

use super::ip::PROTO_UDP;
use super::udp;
use crate::fs::vfs::{FileKind, FsResult, Inode};

/// One endpoint: a protocol, an optional bound port, an optional peer, and a
/// queue of datagrams the RX path has delivered for us to `recvfrom`.
struct Socket {
    proto: u8,
    local_port: u16,
    peer: Option<([u8; 4], u16)>,
    rx: VecDeque<([u8; 4], u16, Vec<u8>)>,
}

static SOCKETS: Mutex<Vec<Option<Socket>>> = Mutex::new(Vec::new());

/// Reset the table (called from `net::init`).
pub fn init() {
    SOCKETS.lock().clear();
}

/// Allocate an unbound socket; returns its index (our "file handle" number).
pub fn socket(proto: u8) -> usize {
    let mut t = SOCKETS.lock();
    let s = Socket { proto, local_port: 0, peer: None, rx: VecDeque::new() };
    if let Some(i) = t.iter().position(|s| s.is_none()) {
        t[i] = Some(s);
        return i;
    }
    t.push(Some(s));
    t.len() - 1
}

/// Bind a socket to a local UDP port so the RX path can route to it.
pub fn bind(idx: usize, port: u16) -> bool {
    let mut t = SOCKETS.lock();
    match t.get_mut(idx).and_then(|o| o.as_mut()) {
        Some(s) => { s.local_port = port; true }
        None => false,
    }
}

/// Remember the default peer a socket talks to (used by `write`/`send`).
pub fn connect(idx: usize, ip: [u8; 4], port: u16) -> bool {
    let mut t = SOCKETS.lock();
    match t.get_mut(idx).and_then(|o| o.as_mut()) {
        Some(s) => { s.peer = Some((ip, port)); true }
        None => false,
    }
}

/// Free a socket slot.
pub fn close(idx: usize) {
    let mut t = SOCKETS.lock();
    if idx < t.len() {
        t[idx] = None;
    }
}

/// `sendto`: dispatch by protocol. Returns bytes written or -errno.
fn sendto(idx: usize, dst: [u8; 4], port: u16, data: &[u8]) -> FsResult<usize> {
    let proto = SOCKETS.lock().get(idx).and_then(|o| o.as_ref()).map(|s| s.proto).unwrap_or(0);
    if proto != PROTO_UDP {
        return Err(crate::fs::vfs::FsError::InvalidArg); // TCP send is out of scope here
    }
    let sport = SOCKETS.lock().get(idx).and_then(|o| o.as_ref()).map(|s| s.local_port).unwrap_or(0);
    if udp::send(sport, dst, port, data) { Ok(data.len()) } else { Err(crate::fs::vfs::FsError::Io) }
}

/// `recvfrom`: pop one queued datagram (non-blocking).
pub fn recvfrom(idx: usize, buf: &mut [u8]) -> Option<(usize, [u8; 4], u16)> {
    let mut t = SOCKETS.lock();
    let s = t.get_mut(idx)?.as_mut()?;
    let (src, sport, data) = s.rx.pop_front()?;
    let n = core::cmp::min(buf.len(), data.len());
    buf[..n].copy_from_slice(&data[..n]);
    Some((n, src, sport))
}

/// RX hook (called by `udp::handle`): append to every UDP socket bound to `dport`.
pub fn deliver(dport: u16, src: [u8; 4], sport: u16, data: &[u8]) {
    let mut t = SOCKETS.lock();
    let mut delivered = 0usize;
    for slot in t.iter_mut() {
        if let Some(s) = slot {
            if s.proto == PROTO_UDP && s.local_port == dport {
                s.rx.push_back((src, sport, data.to_vec()));
                delivered += 1;
            }
        }
    }
    if delivered > 0 {
        crate::println!("[sock] delivered {} bytes to {} socket(s) on :{}", data.len(), delivered, dport);
    }
}

/// A snapshot for `netstat`: (idx, proto, bound port, peer, queued datagrams).
pub fn info() -> Vec<(usize, u8, u16, Option<([u8; 4], u16)>, usize)> {
    SOCKETS.lock().iter().enumerate().filter_map(|(i, s)| {
        s.as_ref().map(|x| (i, x.proto, x.local_port, x.peer, x.rx.len()))
    }).collect()
}

/// An [`Inode`] view over a socket slot: this is what makes "network a file".
pub struct SocketInode {
    idx: usize,
}

impl Inode for SocketInode {
    fn kind(&self) -> FileKind {
        FileKind::CharDev
    }
    // Reuse the VFS trait hook so bind/connect syscalls can find our slot.
    fn sock_index(&self) -> Option<usize> {
        Some(self.idx)
    }
    fn read(&self, _offset: u64, buf: &mut [u8]) -> FsResult<usize> {
        match recvfrom(self.idx, buf) {
            Some((n, _, _)) => Ok(n),
            None => Ok(0), // nothing queued yet (non-blocking)
        }
    }
    fn write(&self, _offset: u64, data: &[u8]) -> FsResult<usize> {
        let peer = SOCKETS.lock().get(self.idx).and_then(|o| o.as_ref()).and_then(|s| s.peer);
        match peer {
            Some((ip, port)) => sendto(self.idx, ip, port, data),
            None => Err(crate::fs::vfs::FsError::InvalidArg), // not connected
        }
    }
}

/// Wrap a socket slot as an inode (to be installed into an fd by the syscall).
pub fn inode(idx: usize) -> Arc<dyn Inode> {
    Arc::new(SocketInode { idx })
}

/// Self-test: create + bind a socket, deliver a datagram, and read it back
/// through the VFS inode (the "network is a file" path end to end).
pub fn demo() {
    crate::println!("[sock] -- demo --");
    let idx = socket(PROTO_UDP);
    bind(idx, 5555);
    let fd = crate::fs::install_inode(inode(idx)).map(|f| f as usize);
    crate::println!("[sock] socket {} bound :5555 -> fd {:?}", idx, fd);
    deliver(5555, super::GATEWAY, 4242, b"ping-pong");
    if let Ok(fd) = fd {
        let mut buf = [0u8; 32];
        match crate::fs::read(fd, &mut buf) {
            Ok(n) => crate::println!("[sock] read(fd) -> {} bytes: '{}'", n,
                core::str::from_utf8(&buf[..n]).unwrap_or("?")),
            Err(e) => crate::println!("[sock] read(fd) err {:?}", e),
        }
        let _ = crate::fs::close(fd);
    }
}
