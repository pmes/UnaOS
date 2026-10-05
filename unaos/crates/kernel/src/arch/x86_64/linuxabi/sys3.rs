// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! CHARTER: Kernel — driver
//! SELFBUILD2 M2 (B349) — the file and IPC syscalls the host rustc made that the shim did not answer (SELFBUILD1's measured
//! table): `statx` (152 calls in the rustc trace), `ftruncate`/`truncate`/`fallocate`, `flock`, `eventfd2` + `epoll_*` and
//! `socketpair(AF_UNIX)` + `sendto`/`recvfrom` (the jobserver / helper thread), and `sendfile` (busybox `cat`, measured on the
//! SELFBUILD2 probe). `sys2::handle`'s fall-through asks [`handle`] before `-ENOSYS`.
//!
//! The three new descriptor kinds ride one `fd::Kind::Ext` variant, so `sys.rs` only routes: an eventfd counter, an epoll
//! interest list (keyed by fd number; level-triggered; EPOLLONESHOT honoured, EPOLLET treated as level), and a stream socket
//! end (two 64 KiB pipes crossed — the pipe ring and its reader/writer counts). Sizes come from the VFS through the same
//! helpers `stat`/`fstat` use; truncation goes through the mount table (`MountTable::truncate`), with a rewrite fallback where a
//! backend has no in-place shrink. `flock` is advisory, per open file description, keyed by the VFS path.

use super::fd::{self, Desc, Kind, Pipe, PIPE_CAP};
use super::proc::ProcInfo;
use super::{sys, LinuxProc};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};

const ENOENT: i64 = 2;
const EIO: i64 = 5;
const EBADF: i64 = 9;
const EAGAIN: i64 = 11;
const EFAULT: i64 = 14;
const EEXIST: i64 = 17;
const EINVAL: i64 = 22;
const EACCES: i64 = 13;
const EFBIG: i64 = 27;
const EPIPE: i64 = 32;
const ENOTSOCK: i64 = 88;
const EOPNOTSUPP: i64 = 95;
const EAFNOSUPPORT: i64 = 97;
const O_CLOEXEC: u64 = 0o2000000;
const MAX_FILE: u64 = 128 << 20;

/// The SELFBUILD2 descriptor kinds (inside `fd::Kind::Ext`).
pub enum Ext {
    EventFd { count: u64, semaphore: bool },
    Epoll { items: Vec<(i32, u32, u64)> },
    Sock { rx: Arc<Pipe>, tx: Arc<Pipe> },
}

impl Drop for Ext {
    fn drop(&mut self) {
        if let Ext::Sock { rx, tx } = self {
            rx.readers.fetch_sub(1, Ordering::AcqRel);
            tx.writers.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

/// `read` on an Ext descriptor. `Err(sys::RETRY)` = would block, nothing consumed.
pub fn read_ext(x: &mut Ext, cnt: usize, nb: bool) -> Result<Vec<u8>, i64> {
    match x {
        Ext::EventFd { count, semaphore } => {
            if cnt < 8 {
                return Err(-EINVAL);
            }
            if *count == 0 {
                return Err(if nb { -EAGAIN } else { sys::RETRY });
            }
            let v = if *semaphore { 1 } else { *count };
            *count -= v;
            Ok(v.to_le_bytes().to_vec())
        }
        Ext::Epoll { .. } => Err(-EINVAL),
        Ext::Sock { rx, .. } => {
            let mut b = fd::lk(&rx.buf);
            let n = cnt.min(b.len());
            if n == 0 {
                if rx.writers.load(Ordering::Acquire) == 0 {
                    return Ok(Vec::new()); // the peer end is closed: EOF
                }
                return Err(if nb { -EAGAIN } else { sys::RETRY });
            }
            Ok(b.drain(..n).collect())
        }
    }
}

/// `write` on an Ext descriptor.
pub fn write_ext(x: &mut Ext, v: &[u8], nb: bool) -> i64 {
    match x {
        Ext::EventFd { count, .. } => {
            if v.len() < 8 {
                return -EINVAL;
            }
            let add = u64::from_le_bytes(v[0..8].try_into().unwrap_or([0; 8]));
            if add == u64::MAX {
                return -EINVAL;
            }
            if *count > u64::MAX - 1 - add {
                return if nb { -EAGAIN } else { sys::RETRY };
            }
            *count += add;
            8
        }
        Ext::Epoll { .. } => -EINVAL,
        Ext::Sock { tx, .. } => {
            if tx.readers.load(Ordering::Acquire) == 0 {
                return -EPIPE;
            }
            let mut b = fd::lk(&tx.buf);
            let space = PIPE_CAP - b.len();
            if space == 0 {
                return if nb { -EAGAIN } else { sys::RETRY };
            }
            let n = space.min(v.len());
            b.extend(v[..n].iter().copied());
            n as i64
        }
    }
}

/// poll/epoll readiness bits (POLLIN 1, POLLOUT 4, POLLERR 8, POLLHUP 16 — the same values epoll uses).
pub fn ready(x: &Ext) -> i16 {
    match x {
        Ext::EventFd { count, .. } => (if *count > 0 { 1 } else { 0 }) | (if *count < u64::MAX - 1 { 4 } else { 0 }),
        Ext::Epoll { .. } => 0,
        Ext::Sock { rx, tx } => {
            let mut r = 0i16;
            if !fd::lk(&rx.buf).is_empty() {
                r |= 1;
            } else if rx.writers.load(Ordering::Acquire) == 0 {
                r |= 16;
            }
            if tx.readers.load(Ordering::Acquire) == 0 {
                r |= 8;
            } else if fd::lk(&tx.buf).len() < PIPE_CAP {
                r |= 4;
            }
            r
        }
    }
}

/// `st_mode` of an Ext descriptor: anonymous-inode files read 0600 (no type bits), a socket S_IFSOCK.
pub fn mode(x: &Ext) -> u32 {
    match x {
        Ext::Sock { .. } => 0o140777,
        _ => 0o600,
    }
}

// ---------------------------------------------------------------------------------------------
// statx
// ---------------------------------------------------------------------------------------------

/// `(mode, size, ino)` of an open descriptor — `fstat`'s view.
fn fd_meta(p: &LinuxProc, fdn: u64) -> Result<(u32, u64, u64), i64> {
    let d = sys::get(p, fdn).ok_or(-EBADF)?;
    let k = fd::lk(&d.k);
    Ok(match &*k {
        Kind::Console => (0o020620, 0, 1),
        Kind::File { path, .. } => (0o100644, sys::fsize(path), sys::ino_of(path)),
        Kind::Dir { path, .. } => (0o040755, 0, sys::ino_of(path)),
        Kind::PipeR(pp) | Kind::PipeW(pp) => (0o010600, fd::lk(&pp.buf).len() as u64, 2),
        Kind::Ext(x) => (mode(x), 0, 3),
    })
}

/// `statx(dirfd, path, flags, mask, buf)` (332) — the `struct statx` std's `fs::metadata` prefers. Answers STATX_BASIC_STATS
/// (`0x7ff`): type+mode, nlink 1, uid/gid 0, ino, size, blocks, and atime/mtime/ctime from the VFS stamp where the medium keeps one.
fn statx(p: &mut LinuxProc, a: [u64; 6]) -> i64 {
    const AT_EMPTY_PATH: u64 = 0x1000;
    let (dirfd, path_va, flags, buf) = (a[0] as i64, a[1], a[2], a[4]);
    let Some(path) = p.asp.read_cstr(path_va, 4096) else { return -EFAULT };
    let (mode, size, ino, mtime) = if path.is_empty() {
        if flags & AT_EMPTY_PATH == 0 {
            return -ENOENT;
        }
        if dirfd == sys::AT_FDCWD {
            (0o040755, 0, sys::ino_of(&p.cwd), 0)
        } else {
            match fd_meta(p, dirfd as u64) {
                Ok((m, s, i)) => (m, s, i, 0),
                Err(e) => return e,
            }
        }
    } else {
        let full = match sys::resolve(p, dirfd, path_va) {
            Ok(f) => f,
            Err(e) => return e,
        };
        let (dir, size) = match sys::stat_full(&full) {
            Ok(v) => v,
            Err(e) => return e,
        };
        let mt = crate::shell::vfs_mount_table().stat(&full).ok().and_then(|s| s.mtime).unwrap_or(0);
        (if dir { 0o040755 } else { 0o100644 }, size, sys::ino_of(&full), mt)
    };
    let mut s = [0u8; 256];
    s[0..4].copy_from_slice(&0x7ffu32.to_le_bytes()); // stx_mask = STATX_BASIC_STATS
    s[4..8].copy_from_slice(&4096u32.to_le_bytes()); // stx_blksize
    s[16..20].copy_from_slice(&1u32.to_le_bytes()); // stx_nlink
    s[28..30].copy_from_slice(&(mode as u16).to_le_bytes()); // stx_mode
    s[32..40].copy_from_slice(&ino.to_le_bytes());
    s[40..48].copy_from_slice(&size.to_le_bytes());
    s[48..56].copy_from_slice(&size.div_ceil(512).to_le_bytes()); // stx_blocks
    for off in [64usize, 96, 112] {
        s[off..off + 8].copy_from_slice(&mtime.to_le_bytes()); // atime / ctime / mtime (tv_sec)
    }
    if p.asp.copy_out(buf, &s, false) { 0 } else { -EFAULT }
}

// ---------------------------------------------------------------------------------------------
// ftruncate / truncate / fallocate
// ---------------------------------------------------------------------------------------------

/// Set `path` to `len` bytes through the mount table; `cur` is the caller's view of the contents (kept in step).
fn set_len(path: &str, len: u64, cur: &mut Vec<u8>) -> i64 {
    use crate::fs::vfs::{VfsError, KERNEL_PRINCIPAL};
    if len > MAX_FILE {
        return -EFBIG;
    }
    if !sys::may_write(path) {
        return -EACCES;
    }
    let mt = crate::shell::vfs_mount_table();
    match mt.truncate(path, len, KERNEL_PRINCIPAL) {
        Ok(()) => {}
        Err(VfsError::Unsupported) => {
            // No in-place shrink/grow on this backend: rewrite. Shrink = empty the file, write the kept prefix back;
            // grow = write zeros from the old end.
            let old = cur.len() as u64;
            let (from, bytes): (u64, Vec<u8>) = if len < old {
                if mt.truncate(path, 0, KERNEL_PRINCIPAL).is_err() {
                    return -EIO;
                }
                (0, cur[..len as usize].to_vec())
            } else {
                (old, alloc::vec![0u8; (len - old) as usize])
            };
            let mut done = 0usize;
            while done < bytes.len() {
                let n = (bytes.len() - done).min(4096);
                match mt.write(path, from + done as u64, &bytes[done..done + n], KERNEL_PRINCIPAL) {
                    Ok(w) if w > 0 => done += w,
                    _ => return -EIO,
                }
            }
        }
        Err(_) => return -EIO,
    }
    cur.resize(len as usize, 0);
    0
}

fn ftruncate(p: &mut LinuxProc, fdn: u64, len: i64) -> i64 {
    if len < 0 {
        return -EINVAL;
    }
    let Some(d) = sys::get(p, fdn) else { return -EBADF };
    let mut k = fd::lk(&d.k);
    match &mut *k {
        Kind::File { path, write: true, .. } => {
            let path = path.clone();
            let mut cur = prefix(&path, len as u64); // SELFBUILD3: no slurped copy — the kept prefix from the VFS
            set_len(&path, len as u64, &mut cur)
        }
        Kind::File { .. } => -EINVAL, // not open for writing
        _ => -EINVAL,
    }
}

fn truncate_path(p: &mut LinuxProc, path_va: u64, len: i64) -> i64 {
    if len < 0 {
        return -EINVAL;
    }
    let full = match sys::resolve(p, sys::AT_FDCWD, path_va) {
        Ok(f) => f,
        Err(e) => return e,
    };
    let mt = crate::shell::vfs_mount_table();
    let Ok(st) = mt.stat(&full) else { return -ENOENT };
    let _ = (mt, st);
    let mut cur = prefix(&full, len as u64);
    set_len(&full, len as u64, &mut cur)
}

fn fallocate(p: &mut LinuxProc, fdn: u64, mode: u64, off: i64, len: i64) -> i64 {
    if off < 0 || len <= 0 {
        return -EINVAL;
    }
    if mode == 1 {
        return 0; // FALLOC_FL_KEEP_SIZE: nothing to reserve on these media
    }
    if mode != 0 {
        return -EOPNOTSUPP; // punch-hole / collapse / zero-range
    }
    let Some(end) = (off as u64).checked_add(len as u64) else { return -EFBIG };
    let Some(d) = sys::get(p, fdn) else { return -EBADF };
    let mut k = fd::lk(&d.k);
    match &mut *k {
        Kind::File { path, write: true, .. } => {
            let path = path.clone();
            let size = sys::fsize(&path);
            if end <= size {
                return 0;
            }
            let mut cur = prefix(&path, end);
            set_len(&path, end, &mut cur)
        }
        Kind::File { .. } => -EBADF,
        _ => -EINVAL,
    }
}

// ---------------------------------------------------------------------------------------------
// flock (advisory, per open file description)
// ---------------------------------------------------------------------------------------------

static FLOCKS: spin::Mutex<Vec<(String, usize, bool)>> = spin::Mutex::new(Vec::new());
static NFLOCK: AtomicUsize = AtomicUsize::new(0);

fn locks<R>(f: impl FnOnce(&mut Vec<(String, usize, bool)>) -> R) -> R {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut g = FLOCKS.lock();
        let r = f(&mut g);
        NFLOCK.store(g.len(), Ordering::Release);
        r
    })
}

/// The open file description `owner` is gone (`fd::Desc::drop`): its locks go with it.
pub fn flock_drop(owner: usize) {
    if NFLOCK.load(Ordering::Acquire) == 0 {
        return;
    }
    locks(|v| v.retain(|e| e.1 != owner));
}

fn flock(p: &mut LinuxProc, fdn: u64, op: u64) -> i64 {
    let Some(d) = sys::get(p, fdn) else { return -EBADF };
    let path = match &*fd::lk(&d.k) {
        Kind::File { path, .. } | Kind::Dir { path, .. } => path.clone(),
        _ => return 0, // a pipe/console/socket: nobody else can name it — the lock is trivially granted
    };
    let owner = Arc::as_ptr(&d) as usize;
    let nb = op & 4 != 0;
    match op & !4 {
        8 => {
            locks(|v| v.retain(|e| !(e.1 == owner && e.0 == path)));
            0
        }
        o @ (1 | 2) => {
            let excl = o == 2;
            locks(|v| {
                if v.iter().any(|e| e.0 == path && e.1 != owner && (excl || e.2)) {
                    return if nb { -EAGAIN } else { sys::RETRY }; // EWOULDBLOCK == EAGAIN
                }
                v.retain(|e| !(e.1 == owner && e.0 == path));
                v.push((path.clone(), owner, excl));
                0
            })
        }
        _ => -EINVAL,
    }
}

// ---------------------------------------------------------------------------------------------
// eventfd / epoll / socketpair / sendfile
// ---------------------------------------------------------------------------------------------

fn eventfd(p: &mut LinuxProc, init: u64, flags: u64) -> i64 {
    if flags & !(1 | fd::O_NONBLOCK as u64 | O_CLOEXEC) != 0 {
        return -EINVAL;
    }
    let d = Desc::new(Kind::Ext(Ext::EventFd { count: init as u32 as u64, semaphore: flags & 1 != 0 }), (flags as u32) & fd::O_NONBLOCK);
    sys::install(p, d, 0, flags & O_CLOEXEC != 0)
}

fn epoll_create(p: &mut LinuxProc, flags: u64) -> i64 {
    if flags & !O_CLOEXEC != 0 {
        return -EINVAL;
    }
    sys::install(p, Desc::new(Kind::Ext(Ext::Epoll { items: Vec::new() }), 0), 0, flags & O_CLOEXEC != 0)
}

fn epoll_ctl(p: &mut LinuxProc, ep: u64, op: u64, fdn: u64, ev_va: u64) -> i64 {
    let Some(e) = sys::get(p, ep) else { return -EBADF };
    if sys::get(p, fdn).is_none() {
        return -EBADF;
    }
    if ep == fdn {
        return -EINVAL;
    }
    let mut ev = [0u8; 12];
    if op != 2 && !p.asp.copy_in(ev_va, &mut ev) {
        return -EFAULT;
    }
    let events = u32::from_le_bytes(ev[0..4].try_into().unwrap_or([0; 4]));
    let data = u64::from_le_bytes(ev[4..12].try_into().unwrap_or([0; 8]));
    let mut k = fd::lk(&e.k);
    let Kind::Ext(Ext::Epoll { items }) = &mut *k else { return -EINVAL };
    let at = items.iter().position(|it| it.0 == fdn as i32);
    match (op, at) {
        (1, Some(_)) => -EEXIST,
        (1, None) => {
            items.push((fdn as i32, events, data));
            0
        }
        (2, Some(i)) => {
            items.remove(i);
            0
        }
        (3, Some(i)) => {
            items[i] = (fdn as i32, events, data);
            0
        }
        (1..=3, None) => -ENOENT,
        _ => -EINVAL,
    }
}

fn epoll_wait(p: &mut LinuxProc, ep: u64, evs_va: u64, max: i64, timeout_ms: i64) -> i64 {
    const ONESHOT: u32 = 1 << 30;
    if !(1..=1024).contains(&max) {
        return -EINVAL;
    }
    let Some(e) = sys::get(p, ep) else { return -EBADF };
    let items = match &*fd::lk(&e.k) {
        Kind::Ext(Ext::Epoll { items }) => items.clone(),
        _ => return -EINVAL,
    };
    let mut out: Vec<u8> = Vec::new();
    let mut fired: Vec<i32> = Vec::new();
    for (fdn, events, data) in items.iter() {
        if out.len() / 12 >= max as usize {
            break;
        }
        let Some(d) = sys::get(p, *fdn as u64) else { continue }; // closed since: Linux drops it with the description
        if Arc::ptr_eq(&d, &e) {
            continue;
        }
        let rev = sys::poll_ready(&d, (*events & 0xffff) as i16) as u16 as u32;
        if rev != 0 {
            out.extend_from_slice(&rev.to_le_bytes());
            out.extend_from_slice(&data.to_le_bytes());
            if events & ONESHOT != 0 {
                fired.push(*fdn);
            }
        }
    }
    let now = crate::arch::ms();
    if !out.is_empty() || timeout_ms == 0 || p.sleep_until.is_some_and(|t| now >= t) {
        p.sleep_until = None;
        if !fired.is_empty() {
            if let Kind::Ext(Ext::Epoll { items }) = &mut *fd::lk(&e.k) {
                for it in items.iter_mut().filter(|it| fired.contains(&it.0)) {
                    it.1 &= ONESHOT; // disarmed until EPOLL_CTL_MOD
                }
            }
        }
        if !out.is_empty() && !p.asp.copy_out(evs_va, &out, false) {
            return -EFAULT;
        }
        return (out.len() / 12) as i64;
    }
    if timeout_ms > 0 && p.sleep_until.is_none() {
        p.sleep_until = Some(now + timeout_ms as u64);
    }
    sys::RETRY
}

fn socketpair(p: &mut LinuxProc, domain: u64, ty: u64, sv: u64) -> i64 {
    const SOCK_NONBLOCK: u64 = 0o4000;
    if domain != 1 {
        return -EAFNOSUPPORT; // AF_UNIX only
    }
    if !matches!(ty & 0xf, 1 | 2 | 5) {
        return -EINVAL; // SOCK_STREAM / SOCK_DGRAM / SOCK_SEQPACKET (all served as a byte stream)
    }
    if !p.asp.writable_range(sv, 8) {
        return -EFAULT;
    }
    let mk = || Arc::new(Pipe { buf: spin::Mutex::new(alloc::collections::VecDeque::new()), readers: AtomicUsize::new(1), writers: AtomicUsize::new(1) });
    let (ab, ba) = (mk(), mk());
    let fl = if ty & SOCK_NONBLOCK != 0 { fd::O_NONBLOCK } else { 0 };
    let cx = ty & O_CLOEXEC != 0;
    let a = Desc::new(Kind::Ext(Ext::Sock { rx: ba.clone(), tx: ab.clone() }), fl);
    let b = Desc::new(Kind::Ext(Ext::Sock { rx: ab, tx: ba }), fl);
    let fa = sys::install(p, a, 0, cx);
    if fa < 0 {
        return fa;
    }
    let fb = sys::install(p, b, 0, cx);
    if fb < 0 {
        p.fds[fa as usize] = None;
        return fb;
    }
    let mut o = [0u8; 8];
    o[0..4].copy_from_slice(&(fa as i32).to_le_bytes());
    o[4..8].copy_from_slice(&(fb as i32).to_le_bytes());
    p.asp.copy_out(sv, &o, false);
    0
}

fn is_sock(p: &LinuxProc, fdn: u64) -> Result<(), i64> {
    let d = sys::get(p, fdn).ok_or(-EBADF)?;
    let k = fd::lk(&d.k);
    if matches!(&*k, Kind::Ext(Ext::Sock { .. })) { Ok(()) } else { Err(-ENOTSOCK) }
}

/// `sendfile(out, in, offset*, count)` (40): from a regular file's bytes into any writable descriptor.
fn sendfile(p: &mut LinuxProc, out: u64, inp: u64, offp: u64, count: u64) -> i64 {
    let (Some(din), Some(dout)) = (sys::get(p, inp), sys::get(p, out)) else { return -EBADF };
    let cnt = count.min(1 << 20) as usize;
    let mut off = [0u8; 8];
    if offp != 0 && !p.asp.copy_in(offp, &mut off) {
        return -EFAULT;
    }
    let (chunk, start) = match &*fd::lk(&din.k) {
        Kind::File { path, pos, read: true, .. } => {
            let start = if offp != 0 { u64::from_le_bytes(off) } else { *pos };
            match sys::file_read(path, start, cnt) {
                Ok(v) => (v, start), // SELFBUILD3: from the VFS, not a slurped copy
                Err(e) => return e,
            }
        }
        Kind::File { .. } => return -EBADF,
        _ => return -EINVAL,
    };
    if chunk.is_empty() {
        return 0;
    }
    let rc = sys::write_desc(&dout, &chunk, None);
    if rc > 0 {
        let next = start + rc as u64;
        if offp != 0 {
            let _ = p.asp.copy_out(offp, &next.to_le_bytes(), false);
        } else if let Kind::File { pos, .. } = &mut *fd::lk(&din.k) {
            *pos = next;
        }
    }
    rc
}

/// The SELFBUILD2 M2 table. `None` = not answered here either (the caller notes `-ENOSYS`).
pub fn handle(p: &mut LinuxProc, info: &Arc<ProcInfo>, nr: u64, a: [u64; 6]) -> Option<i64> {
    let _ = info;
    Some(match nr {
        332 => statx(p, a),
        77 => ftruncate(p, a[0], a[1] as i64),
        76 => truncate_path(p, a[0], a[1] as i64),
        285 => fallocate(p, a[0], a[1], a[2] as i64, a[3] as i64),
        73 => flock(p, a[0], a[1]),
        290 => eventfd(p, a[0], a[1]),
        284 => eventfd(p, a[0], 0),
        291 => epoll_create(p, a[0]),
        213 => {
            if (a[0] as i32) <= 0 { -EINVAL } else { epoll_create(p, 0) }
        }
        233 => epoll_ctl(p, a[0], a[1], a[2], a[3]),
        232 | 281 => epoll_wait(p, a[0], a[1], a[2] as i32 as i64, a[3] as i32 as i64),
        53 => socketpair(p, a[0], a[1], a[3]),
        44 => match is_sock(p, a[0]) {
            Ok(()) => sys::do_write(p, a[0], a[1], a[2], None),
            Err(e) => e,
        },
        45 => match is_sock(p, a[0]) {
            Ok(()) => {
                let r = sys::do_read(p, a[0], a[1], a[2], None);
                if r >= 0 && a[5] != 0 {
                    let _ = p.asp.copy_out(a[5], &0u32.to_le_bytes(), false); // *addrlen = 0: an unnamed peer
                }
                r
            }
            Err(e) => e,
        },
        40 => sendfile(p, a[0], a[1], a[2], a[3]),
        _ => return None,
    })
}

/// SELFBUILD3: what `set_len` needs to know of the file — its bytes up to `len` when shrinking (the prefix a rewrite keeps), else a
/// zero buffer of the current size (only its LENGTH is read on the grow path).
fn prefix(path: &str, len: u64) -> Vec<u8> {
    let size = sys::fsize(path);
    if len < size {
        sys::file_read(path, 0, size as usize).unwrap_or_default()
    } else {
        alloc::vec![0u8; size as usize]
    }
}
