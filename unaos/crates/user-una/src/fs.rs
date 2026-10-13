// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// UNAMETAL (LEDGER SR90): the file surface all three panes share — MIDDEN.ELF's (crates/user-midden/src/main.rs,
// SR75) `stat` / `resolve` / `list` / `read_file` / `spawn_path` / `home` / `errno_text`, copied (the crate is not
// linked), plus `write_file` for the editor's Ctrl+S over SYS_PATH_WRITE.
//
//   SYS_STAT                       kind (0 file, 1 dir), size, id, flags
//   SYS_PATH_READ                  a file from `offset`, PATH_IO_MAX per call (0 = end of file)
//   SYS_PATH_READ + PATH_R_LIST    a directory's names, `\n`-joined, a directory with a trailing `/`; any directory
//                                  the ownership ACL lets this program read (RING3EXEC B521), else -EACCES
//   SYS_PATH_WRITE                 the first chunk with PATH_W_TRUNC at offset 0 (create-or-truncate), then each
//                                  chunk at its offset; UNAMETAL2 (LEDGER SR98): on the default x86 kernel as this
//                                  program's principal under the volume's write ACL (-EACCES by the ACL's word);
//                                  UNAIDE2 (LEDGER SR111): PATH_W_UNLINK (`unlink`) and PATH_W_MKDIRS (`mkdir`) too

use alloc::string::String;
use alloc::vec::Vec;
use vein_ring3::files;
use vein_ring3::sys::sys;

/// The most a `cat` or a listing reads (MIDDEN.ELF's bound).
pub const CAT_MAX: usize = 64 * 1024;
/// The largest document the editor opens: the whole file sits in edit_core's original buffer.
pub const EDIT_MAX: usize = 1024 * 1024;

pub struct Stat {
    pub kind: u32,
    pub size: u64,
    pub id: u64,
    pub flags: u32,
}

pub fn stat(path: &str) -> Result<Stat, i64> {
    let mut st = [0u8; una_abi::USER_STAT_LEN];
    let r = sys(una_abi::SYS_STAT, path.as_ptr() as u64, path.len() as u64, st.as_mut_ptr() as u64, 0);
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

/// An errno's name and the ring-3 reason it most often carries here.
pub fn errno_text(e: i64) -> &'static str {
    match e {
        una_abi::ENOSYS => "ENOSYS (this kernel does not carry that syscall for ring 3)",
        una_abi::EACCES => "EACCES (refused by the ownership ACL for this program's principal, or APPTRUST: answer the Open question first)",
        una_abi::E2BIG => "E2BIG (the image exceeds the user window, or the words exceed the args page)",
        una_abi::ENOEXEC => "ENOEXEC (not an ELF64 program for this machine)",
        una_abi::EAGAIN => "EAGAIN (the loader refused: process table or memory's limit)",
        una_abi::EIO => "EIO (short read)",
        una_abi::ENOENT => "ENOENT (no such file or directory)",
        una_abi::EISDIR => "EISDIR (a directory: use ls)",
        una_abi::ENOTDIR => "ENOTDIR (not a directory)",
        una_abi::EINVAL => "EINVAL (the path is not one the surface accepts)",
        _ => "error",
    }
}

/// The short errno word the status line has room for.
pub fn errno_word(e: i64) -> &'static str {
    let t = errno_text(e);
    match t.find(' ') {
        Some(i) => &t[..i],
        None => t,
    }
}

/// The SYS_PATH_* request header + path (+ `data` for a write).
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

/// A directory's entry names (a directory keeps its trailing `/`).
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

/// The whole file, up to `max` bytes. `Err(E2BIG)` when the file is longer than `max` and `strict`.
pub fn read_file(path: &str, max: usize, strict: bool) -> Result<Vec<u8>, i64> {
    let mut out = Vec::new();
    let mut buf = alloc::vec![0u8; 4096];
    loop {
        let r = files::path_read(path.as_bytes(), out.len() as u64, &mut buf);
        if r < 0 {
            return Err(r);
        }
        if r == 0 {
            break;
        }
        out.extend_from_slice(&buf[..r as usize]);
        if out.len() >= max {
            if strict {
                return Err(una_abi::E2BIG);
            }
            break;
        }
    }
    Ok(out)
}

/// Write `data` as the whole file at `path`: the first request truncates (or creates) it, each later one
/// writes its chunk at its offset. Bytes written, or the first -errno.
pub fn write_file(path: &str, data: &[u8]) -> Result<usize, i64> {
    let mut done = 0usize;
    loop {
        let step = (data.len() - done).min(una_abi::PATH_IO_MAX);
        let flags = if done == 0 { una_abi::PATH_W_TRUNC } else { 0 };
        let req = path_req(path, flags, done as u64, &data[done..done + step]).ok_or(una_abi::EINVAL)?;
        let r = sys(una_abi::SYS_PATH_WRITE, req.as_ptr() as u64, req.len() as u64, 0, 0);
        if r < 0 {
            return Err(r);
        }
        if r == 0 && step > 0 {
            return Err(una_abi::EIO);
        }
        done += r as usize;
        if done >= data.len() {
            return Ok(done);
        }
    }
}

/// UNAIDE2 (LEDGER SR111): remove the file at `path` — one SYS_PATH_WRITE with PATH_W_UNLINK (empty data, offset 0).
pub fn unlink(path: &str) -> Result<(), i64> {
    let req = path_req(path, una_abi::PATH_W_UNLINK, 0, &[]).ok_or(una_abi::EINVAL)?;
    let r = sys(una_abi::SYS_PATH_WRITE, req.as_ptr() as u64, req.len() as u64, 0, 0);
    if r < 0 {
        Err(r)
    } else {
        Ok(())
    }
}

/// UNAIDE2: make the directory `dir` (absolute). SYS_PATH_WRITE has no mkdir verb, but PATH_W_MKDIRS creates every
/// missing PARENT of the path it is handed: `dir/.keep` is written empty with TRUNC|MKDIRS (which makes `dir`), then
/// unlinked again, leaving the folder empty.
pub fn mkdir(dir: &str) -> Result<(), i64> {
    let keep = alloc::format!("{}/.keep", dir.trim_end_matches('/'));
    let req = path_req(&keep, una_abi::PATH_W_TRUNC | una_abi::PATH_W_MKDIRS, 0, &[]).ok_or(una_abi::EINVAL)?;
    let r = sys(una_abi::SYS_PATH_WRITE, req.as_ptr() as u64, req.len() as u64, 0, 0);
    if r < 0 {
        return Err(r);
    }
    unlink(&keep)
}

/// RING3EXEC (B521): `SYS_SPAWN_PATH(path, len, argv, argc)` — the argv descriptors point at the words in place.
pub fn spawn_path(path: &str, argv: &[&str]) -> i64 {
    let descs: Vec<una_abi::SpawnArg> = argv.iter().map(|a| una_abi::SpawnArg { ptr: a.as_ptr() as u64, len: a.len() as u64 }).collect();
    sys(una_abi::SYS_SPAWN_PATH, path.as_ptr() as u64, path.len() as u64, descs.as_ptr() as u64, descs.len() as u64)
}

/// The session home (SYS_WHOAMI) when a user is logged in, else the root.
pub fn home() -> String {
    let mut rec = [0u8; una_abi::WHOAMI_MAX];
    let r = sys(una_abi::SYS_WHOAMI, rec.as_mut_ptr() as u64, rec.len() as u64, 0, 0);
    if r > 0 {
        if let Some(w) = una_abi::whoami_parse(&rec[..(r as usize).min(rec.len())]) {
            if let Ok(h) = core::str::from_utf8(w.home) {
                let h = h.trim_end_matches('/');
                if h.starts_with('/') {
                    return String::from(h);
                }
            }
        }
    }
    String::from("/")
}
