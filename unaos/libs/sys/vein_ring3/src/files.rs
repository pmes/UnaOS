// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! LUMENUX M4 (rmbp-ledger B348): whole files for a ring-3 caller — read a file, create it, append to it —
//! over whichever surface the running kernel offers, probed once:
//!
//! * **PATH** — SELFDIAG's `SYS_PATH_READ`/`SYS_PATH_WRITE` (B324) with RING3ABI2's `SYS_WHOAMI` (B333) for
//!   the home: absolute paths through the kernel VFS (UnaFS or FAT), parents created by `PATH_W_MKDIRS`.
//!   Those numbers are NOT in this branch's una-abi (they are on `exec-rmbp-merge12`); [`next`] carries
//!   them verbatim until the fold, where they become `una_abi::` names (LUMENUX.md §Notes for the fold).
//! * **OPEN** — the classic handle verbs (`SYS_OPEN` O_CREAT, `SYS_SEEK`, `SYS_WRITE`, `SYS_READ`) on a path
//!   RELATIVE to the session home (the EL0 resolver starts a relative path at `/home/<user>`), inside the
//!   40-byte `SYS_OPEN` cap. Needs the x86 storage service (`irqstorage`) and an existing directory: this
//!   surface has no mkdir.
//!
//! Neither present ⇒ [`Files::None`] and the caller says "history off" with the reason.

use crate::sys::sys;
use una_abi::{ENOENT, ENOSYS, O_CREAT, SYS_CLOSE, SYS_OPEN, SYS_READ, SYS_SEEK, SYS_WRITE};

/// The merge12 numbers (SELFDIAG B324, RING3ABI2 B333), verbatim from that branch's una-abi tail.
pub mod next {
    pub const SYS_PATH_READ: u64 = 59;
    pub const SYS_PATH_WRITE: u64 = 60;
    pub const SYS_WHOAMI: u64 = 61;
    pub const PATH_W_TRUNC: u16 = 1 << 0;
    pub const PATH_W_MKDIRS: u16 = 1 << 1;
    pub const PATH_IO_HDR_LEN: usize = 16;
    pub const PATH_IO_PATH_MAX: usize = 255;
    pub const PATH_IO_MAX: usize = 32 * 1024;
    pub const WHOAMI_HDR_LEN: usize = 16;
    pub const WHOAMI_MAX: usize = WHOAMI_HDR_LEN + 255 + 255;
}

/// Which surface [`probe`] found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Files {
    /// `SYS_PATH_*` with the home from `SYS_WHOAMI`: `home[..home_n]` is the home path.
    Path,
    /// `SYS_OPEN` on home-relative paths.
    Open,
    /// No file surface (the code is why: ENOSYS / ENOENT from the probes).
    None(i64),
}

impl Files {
    pub fn as_str(self) -> &'static str {
        match self {
            Files::Path => "path",
            Files::Open => "open",
            Files::None(_) => "off",
        }
    }
}

/// The caller's home (`SYS_WHOAMI`), at most 255 bytes.
pub struct Home {
    pub b: [u8; 256],
    pub n: usize,
}

/// Probe the file surfaces: PATH when `SYS_WHOAMI` names a home and `SYS_PATH_READ` is not ENOSYS; else
/// OPEN when a home-relative `SYS_OPEN` of `.config` answers anything but ENOENT (without the storage
/// service every non-staged name is ENOENT, so a missing `.config` reads as no surface — and OPEN could
/// not make the directories anyway).
pub fn probe(home: &mut Home) -> Files {
    let mut w = [0u8; next::WHOAMI_MAX];
    let r = sys(next::SYS_WHOAMI, w.as_mut_ptr() as u64, w.len() as u64, 0, 0);
    if r >= next::WHOAMI_HDR_LEN as i64 {
        let nl = u16::from_le_bytes([w[8], w[9]]) as usize;
        let hl = u16::from_le_bytes([w[10], w[11]]) as usize;
        let a = next::WHOAMI_HDR_LEN + nl;
        if hl > 0 && hl <= 255 && a + hl <= r as usize {
            home.b[..hl].copy_from_slice(&w[a..a + hl]);
            home.n = hl;
            // the read verb itself: a missing file is ENOENT, an absent verb ENOSYS
            let mut one = [0u8; 1];
            let pr = path_read(b"/", 0, &mut one);
            if pr != ENOSYS {
                return Files::Path;
            }
        }
    }
    let d = b".config";
    let h = sys(SYS_OPEN, d.as_ptr() as u64, d.len() as u64, 0, 0);
    if h >= 0 {
        sys(SYS_CLOSE, h as u64, 0, 0, 0);
    }
    if h != ENOENT {
        return Files::Open;
    }
    Files::None(if r < 0 { r } else { h })
}

fn path_req(path: &[u8], flags: u16, offset: u64, data: &[u8], req: &mut [u8]) -> Option<usize> {
    if path.is_empty() || path.len() > next::PATH_IO_PATH_MAX {
        return None;
    }
    let n = next::PATH_IO_HDR_LEN + path.len() + data.len();
    if n > req.len() {
        return None;
    }
    req[0..2].copy_from_slice(&(path.len() as u16).to_le_bytes());
    req[2..4].copy_from_slice(&flags.to_le_bytes());
    req[4..8].fill(0);
    req[8..16].copy_from_slice(&offset.to_le_bytes());
    req[16..16 + path.len()].copy_from_slice(path);
    req[16 + path.len()..n].copy_from_slice(data);
    Some(n)
}

/// `SYS_PATH_READ` one call: bytes read (0 = EOF) or -errno.
pub fn path_read(path: &[u8], offset: u64, out: &mut [u8]) -> i64 {
    let mut req = [0u8; next::PATH_IO_HDR_LEN + next::PATH_IO_PATH_MAX];
    let Some(n) = path_req(path, 0, offset, &[], &mut req) else { return una_abi::EINVAL };
    let cap = out.len().min(next::PATH_IO_MAX);
    sys(next::SYS_PATH_READ, req.as_ptr() as u64, n as u64, out.as_mut_ptr() as u64, cap as u64)
}

/// Read a whole file (or its first `out.len()` bytes from `offset`) through `f`: bytes read or -errno.
/// `path` is absolute for [`Files::Path`], home-relative for [`Files::Open`].
pub fn read(f: Files, path: &[u8], offset: u64, out: &mut [u8]) -> i64 {
    match f {
        Files::Path => {
            let mut got = 0usize;
            while got < out.len() {
                let r = path_read(path, offset + got as u64, &mut out[got..]);
                if r < 0 {
                    return if got == 0 { r } else { got as i64 };
                }
                if r == 0 {
                    break;
                }
                got += r as usize;
            }
            got as i64
        }
        Files::Open => {
            let h = sys(SYS_OPEN, path.as_ptr() as u64, path.len() as u64, 0, 0);
            if h < 0 {
                return h;
            }
            if offset > 0 {
                let s = sys(SYS_SEEK, h as u64, offset, 0, 0);
                if s < 0 {
                    sys(SYS_CLOSE, h as u64, 0, 0, 0);
                    return s;
                }
            }
            let mut got = 0usize;
            while got < out.len() {
                let k = sys(SYS_READ, h as u64, out[got..].as_mut_ptr() as u64, (out.len() - got) as u64, 0);
                if k <= 0 {
                    break;
                }
                got += k as usize;
            }
            sys(SYS_CLOSE, h as u64, 0, 0, 0);
            got as i64
        }
        Files::None(e) => e,
    }
}

/// The size of a file: read it through to the end in `scratch`-sized steps (neither surface has a
/// stat for a relative path). -errno if absent.
pub fn size(f: Files, path: &[u8], scratch: &mut [u8]) -> i64 {
    let mut off = 0u64;
    loop {
        let r = read(f, path, off, scratch);
        if r < 0 {
            return if off == 0 { r } else { off as i64 };
        }
        off += r as u64;
        if (r as usize) < scratch.len() {
            return off as i64;
        }
    }
}

/// Write `data` at `offset` (= the current size: an append), creating the file (and, on PATH, its parent
/// directories) when `create`. Bytes written or -errno.
pub fn append(f: Files, path: &[u8], offset: u64, create: bool, data: &[u8], req: &mut [u8]) -> i64 {
    match f {
        Files::Path => {
            let mut done = 0usize;
            while done < data.len() || (create && done == 0 && data.is_empty()) {
                let step = (data.len() - done).min(next::PATH_IO_MAX).min(req.len().saturating_sub(next::PATH_IO_HDR_LEN + path.len()));
                let first = done == 0 && create;
                let flags = if first { next::PATH_W_TRUNC | next::PATH_W_MKDIRS } else { 0 };
                let at = if first { 0 } else { offset + done as u64 };
                let Some(n) = path_req(path, flags, at, &data[done..done + step], req) else { return una_abi::EINVAL };
                let r = sys(next::SYS_PATH_WRITE, req.as_ptr() as u64, n as u64, 0, 0);
                if r < 0 {
                    return r;
                }
                if r == 0 && step > 0 {
                    return una_abi::EIO;
                }
                done += r as usize;
                if data.is_empty() {
                    break;
                }
            }
            done as i64
        }
        Files::Open => {
            let mode = 1 | if create { O_CREAT } else { 0 };
            let h = sys(SYS_OPEN, path.as_ptr() as u64, path.len() as u64, mode, 0);
            if h < 0 {
                return h;
            }
            let s = sys(SYS_SEEK, h as u64, offset, 0, 0);
            if s < 0 {
                sys(SYS_CLOSE, h as u64, 0, 0, 0);
                return s;
            }
            let mut done = 0usize;
            while done < data.len() {
                let step = (data.len() - done).min(4096);
                let k = sys(SYS_WRITE, h as u64, data[done..].as_ptr() as u64, step as u64, 0);
                if k <= 0 {
                    sys(SYS_CLOSE, h as u64, 0, 0, 0);
                    return if k < 0 { k } else { una_abi::EIO };
                }
                done += k as usize;
            }
            sys(SYS_CLOSE, h as u64, 0, 0, 0);
            done as i64
        }
        Files::None(e) => e,
    }
}

/// Does the file exist (a read of one byte answers anything but ENOENT)?
pub fn exists(f: Files, path: &[u8]) -> bool {
    let mut one = [0u8; 1];
    let r = read(f, path, 0, &mut one);
    r >= 0
}
