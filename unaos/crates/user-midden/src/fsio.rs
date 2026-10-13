// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// MIDDEN2 (LEDGER SR120): the shell's file and launch doors, moved out of main.rs so the app reads as the shell.
// SYS_STAT (a Volume probe, `cd`, `stat`), SYS_PATH_READ (a whole file, a tail window, a PATH_R_LIST listing),
// SYS_PATH_WRITE (the history file: create-or-truncate, append at the end, chunked at PATH_IO_MAX) and
// SYS_SPAWN_PATH (plain, and with RING3PIPE's SPAWN_PIPE). Every function answers the kernel's errno unchanged.

use alloc::string::String;
use alloc::vec::Vec;
use una_abi::SYS_STAT;
use vein_ring3::files;
use vein_ring3::sys::sys;

/// The most a `cat` or a listing reads (the scrollback keeps its own budget).
pub const CAT_MAX: usize = 64 * 1024;

pub struct Stat {
    pub kind: u32,
    pub size: u64,
    pub id: u64,
    pub flags: u32,
}

pub fn stat(path: &str) -> Result<Stat, i64> {
    let mut st = [0u8; una_abi::USER_STAT_LEN];
    let r = sys(SYS_STAT, path.as_ptr() as u64, path.len() as u64, st.as_mut_ptr() as u64, 0);
    if r < 0 {
        return Err(r);
    }
    let u32_at = |o: usize| u32::from_le_bytes([st[o], st[o + 1], st[o + 2], st[o + 3]]);
    let u64_at = |o: usize| {
        let mut b = [0u8; 8];
        b.copy_from_slice(&st[o..o + 8]);
        u64::from_le_bytes(b)
    };
    Ok(Stat { kind: u32_at(0), flags: u32_at(4), size: u64_at(8), id: u64_at(16) })
}

/// A regular file right now?
pub fn is_file(path: &str) -> bool {
    matches!(stat(path), Ok(s) if s.kind == 0)
}

/// `p` resolved against `cwd`: absolute, `.`/`..`/empty components folded (`..` stops at `/`).
pub fn resolve(cwd: &str, p: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    let joined;
    let full = if p.starts_with('/') {
        p
    } else {
        joined = alloc::format!("{}/{}", cwd, p);
        &joined
    };
    for c in full.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            c => parts.push(c),
        }
    }
    let mut out = String::new();
    for c in parts {
        out.push('/');
        out.push_str(c);
    }
    if out.is_empty() {
        out.push('/');
    }
    out
}

fn path_req(path: &str, flags: u16, offset: u64, data: &[u8]) -> Option<Vec<u8>> {
    let p = path.as_bytes();
    if p.is_empty() || p.len() > una_abi::PATH_IO_PATH_MAX || data.len() > una_abi::PATH_IO_MAX {
        return None;
    }
    let mut r = alloc::vec![0u8; una_abi::PATH_IO_HDR_LEN + p.len() + data.len()];
    r[0..2].copy_from_slice(&(p.len() as u16).to_le_bytes());
    r[2..4].copy_from_slice(&flags.to_le_bytes());
    r[8..16].copy_from_slice(&offset.to_le_bytes());
    r[16..16 + p.len()].copy_from_slice(p);
    r[16 + p.len()..].copy_from_slice(data);
    Some(r)
}

/// A directory's entry names (PATH_R_LIST: `\n`-joined, a directory with a trailing `/`).
pub fn list(path: &str) -> Result<Vec<String>, i64> {
    let mut all = Vec::new();
    let mut buf = alloc::vec![0u8; 4096];
    loop {
        let req = path_req(path, una_abi::PATH_R_LIST, all.len() as u64, &[]).ok_or(una_abi::EINVAL)?;
        let r = sys(una_abi::SYS_PATH_READ, req.as_ptr() as u64, req.len() as u64, buf.as_mut_ptr() as u64, buf.len() as u64);
        if r < 0 {
            return Err(r);
        }
        if r == 0 {
            break;
        }
        all.extend_from_slice(&buf[..r as usize]);
        if all.len() > CAT_MAX {
            break;
        }
    }
    let text = String::from_utf8_lossy(&all).into_owned();
    Ok(text.split('\n').filter(|l| !l.is_empty()).map(String::from).collect())
}

/// At most `max` bytes of `path` from `offset` (EOF ends it early).
pub fn read_from(path: &str, offset: u64, max: usize) -> Result<Vec<u8>, i64> {
    let mut out = Vec::new();
    let mut buf = alloc::vec![0u8; 4096];
    loop {
        let r = files::path_read(path.as_bytes(), offset + out.len() as u64, &mut buf);
        if r < 0 {
            return Err(r);
        }
        if r == 0 {
            break;
        }
        out.extend_from_slice(&buf[..r as usize]);
        if out.len() >= max {
            out.truncate(max);
            break;
        }
    }
    Ok(out)
}

/// A whole file, up to [`CAT_MAX`].
pub fn read_file(path: &str) -> Result<Vec<u8>, i64> {
    read_from(path, 0, CAT_MAX)
}

/// `data` written at `offset` of `path` (the file must exist and `offset <= size`), or — with `create` — the file
/// created-or-truncated first (offset 0, missing parents made). Chunked at PATH_IO_MAX. Bytes written or -errno.
pub fn write_at(path: &str, offset: u64, create: bool, data: &[u8]) -> i64 {
    let mut done = 0usize;
    loop {
        let n = (data.len() - done).min(una_abi::PATH_IO_MAX);
        let first = done == 0;
        let flags = if first && create { una_abi::PATH_W_TRUNC | una_abi::PATH_W_MKDIRS } else { 0 };
        let off = if first && create { 0 } else { offset + done as u64 };
        let Some(req) = path_req(path, flags, off, &data[done..done + n]) else {
            return una_abi::EINVAL;
        };
        let r = sys(una_abi::SYS_PATH_WRITE, req.as_ptr() as u64, req.len() as u64, 0, 0);
        if r < 0 {
            return r;
        }
        done += n;
        if done >= data.len() {
            return done as i64;
        }
    }
}

/// RING3EXEC (B521): `SYS_SPAWN_PATH(path, len, argv, argc)` — the argv descriptors point at the words in place.
pub fn spawn_path(path: &str, argv: &[&str]) -> i64 {
    let descs: Vec<una_abi::SpawnArg> = argv.iter().map(|a| una_abi::SpawnArg { ptr: a.as_ptr() as u64, len: a.len() as u64 }).collect();
    sys(una_abi::SYS_SPAWN_PATH, path.as_ptr() as u64, path.len() as u64, descs.as_ptr() as u64, descs.len() as u64)
}

/// RING3PIPE (B562): `SYS_SPAWN_PATH` with `SPAWN_PIPE` on argc's high half — the pid names the child's output pipe.
pub fn spawn_piped(path: &str, argv: &[&str]) -> i64 {
    let descs: Vec<una_abi::SpawnArg> = argv.iter().map(|a| una_abi::SpawnArg { ptr: a.as_ptr() as u64, len: a.len() as u64 }).collect();
    sys(una_abi::SYS_SPAWN_PATH, path.as_ptr() as u64, path.len() as u64, descs.as_ptr() as u64, descs.len() as u64 | una_abi::SPAWN_PIPE)
}
