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
//   SYS_PATH_WRITE + PATH_W_RENAME RING3RENAME (rmbp-ledger B608): `rename` — the path to the data's path, a move
//                                  anywhere on the native UnaFS root as this program's principal (the object, the
//                                  folder it leaves and the folder it enters must be writable); FAT refused by name
//   SYS_ATTR_SET / SYS_ATTR_GET    RING3RENAME: `PathTrash`'s `una:trash-*` record on the object (trash_core's sidecar
//                                  on an attribute volume); ring 3 has no attribute-remove verb, so `remove_attr` is a
//                                  no-op (a stamped object left outside the Trash is ignored by trash_core's scoped query)

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
        una_abi::EEXIST => "EEXIST (that name is taken)",
        una_abi::ENOTSUP => "ENOTSUP (an end is on FAT: the path rename runs on the UnaFS root only)",
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

/// RING3RENAME (rmbp-ledger B608): rename or move `from` to `to` (both absolute) — one SYS_PATH_WRITE with
/// PATH_W_RENAME, `to` as the data. A folder moves with everything under it; the object keeps its id, so its owner,
/// grants and attributes go with it. 0 or -errno (`-ENOTSUP` = an end on FAT, `-EEXIST` = `to` is taken).
pub fn rename(from: &str, to: &str) -> Result<(), i64> {
    if to.is_empty() || to.len() > una_abi::PATH_IO_PATH_MAX {
        return Err(una_abi::EINVAL);
    }
    let req = path_req(from, una_abi::PATH_W_RENAME, 0, to.as_bytes()).ok_or(una_abi::EINVAL)?;
    let r = sys(una_abi::SYS_PATH_WRITE, req.as_ptr() as u64, req.len() as u64, 0, 0);
    if r < 0 {
        Err(r)
    } else {
        Ok(())
    }
}

/// RING3RENAME: the session user's name (SYS_WHOAMI), else `anon` — trash_core's `una:trash-by`.
pub fn user_name() -> String {
    let mut rec = [0u8; una_abi::WHOAMI_MAX];
    let r = sys(una_abi::SYS_WHOAMI, rec.as_mut_ptr() as u64, rec.len() as u64, 0, 0);
    if r > 0 {
        if let Some(w) = una_abi::whoami_parse(&rec[..(r as usize).min(rec.len())]) {
            if let Ok(n) = core::str::from_utf8(w.name) {
                if !n.is_empty() {
                    return String::from(n);
                }
            }
        }
    }
    String::from("anon")
}

/// RING3RENAME: unix seconds (SYS_TIME), 0 while the clock is unanchored.
pub fn now() -> u64 {
    let r = sys(una_abi::SYS_TIME, 0, 0, 0, 0);
    if r > 0 {
        r as u64
    } else {
        0
    }
}

/// RING3RENAME: one typed attribute set (SYS_ATTR_SET, una-abi's ATTRSURF request: path, key, value wire).
fn attr_set(path: &str, key: &str, tag: u8, payload: &[u8]) -> Result<(), i64> {
    let hdr = una_abi::AttrWireHdr { tag, _rsv: [0; 3], len: payload.len() as u32 }.to_bytes();
    let mut val = Vec::with_capacity(hdr.len() + payload.len());
    val.extend_from_slice(&hdr);
    val.extend_from_slice(payload);
    let mut req = alloc::vec![0u8; una_abi::ATTR_REQ_HDR_LEN + path.len() + key.len() + val.len()];
    let n = una_abi::attr_req_build(path.as_bytes(), key.as_bytes(), &val, &mut req).ok_or(una_abi::EINVAL)?;
    let r = sys(una_abi::SYS_ATTR_SET, req.as_ptr() as u64, n as u64, 0, 0);
    if r < 0 {
        Err(r)
    } else {
        Ok(())
    }
}

/// RING3RENAME: one typed attribute read (SYS_ATTR_GET): `(tag, payload)`, or `None` when absent / unreadable.
fn attr_get(path: &str, key: &str) -> Option<(u8, Vec<u8>)> {
    let mut req = alloc::vec![0u8; una_abi::ATTR_REQ_HDR_LEN + path.len() + key.len()];
    let n = una_abi::attr_req_build(path.as_bytes(), key.as_bytes(), &[], &mut req)?;
    let mut out = alloc::vec![0u8; una_abi::ATTR_WIRE_HDR_LEN + una_abi::ATTR_VALUE_MAX];
    let r = sys(una_abi::SYS_ATTR_GET, req.as_ptr() as u64, n as u64, out.as_mut_ptr() as u64, out.len() as u64);
    if r < 0 {
        return None;
    }
    let (tag, p, _) = una_abi::attr_wire_parse(&out[..(r as usize).min(out.len())]).ok()?;
    Some((tag, p.to_vec()))
}

/// RING3RENAME: every `(id, path)` SYS_QUERY answers for `q` (una-abi's QUERY reply: `[id u64][path_len u16][path]`).
fn query(q: &str) -> Vec<(u64, String)> {
    let mut out = alloc::vec![0u8; una_abi::ATTR_OUT_MAX];
    let r = sys(una_abi::SYS_QUERY, q.as_ptr() as u64, q.len() as u64, out.as_mut_ptr() as u64, out.len() as u64);
    let mut v = Vec::new();
    if r <= 0 {
        return v;
    }
    let b = &out[..(r as usize).min(out.len())];
    let mut o = 0usize;
    while o + 10 <= b.len() {
        let mut w = [0u8; 8];
        w.copy_from_slice(&b[o..o + 8]);
        let id = u64::from_le_bytes(w);
        let pl = u16::from_le_bytes([b[o + 8], b[o + 9]]) as usize;
        o += 10;
        if o + pl > b.len() {
            break;
        }
        if let Ok(p) = core::str::from_utf8(&b[o..o + pl]) {
            v.push((id, String::from(p)));
        }
        o += pl;
    }
    v
}

/// RING3RENAME: trash_core's I/O seam over this program's doors — the Trash rule itself (where, the collision name,
/// the record, the order stamp-then-rename) is trash_core's alone (TRASHCORE B449, the one Matrix and the kernel link).
pub struct PathTrash;

impl trash_core::TrashFs for PathTrash {
    fn stat(&mut self, path: &str) -> Option<trash_core::Node> {
        stat(path).ok().map(|s| trash_core::Node { dir: s.kind == 1, id: if s.flags & una_abi::STAT_HAS_ID != 0 { Some(s.id) } else { None } })
    }
    fn mkdir(&mut self, path: &str) -> Result<(), String> {
        mkdir(path).map_err(|e| alloc::format!("mkdir {} {}", errno_word(e), e))
    }
    fn rename(&mut self, src: &str, dst: &str) -> Result<(), String> {
        rename(src, dst).map_err(|e| alloc::format!("rename {} {}", errno_word(e), e))
    }
    fn set_attr(&mut self, path: &str, key: &str, v: trash_core::Attr) -> Result<(), String> {
        let r = match v {
            trash_core::Attr::Str(s) => attr_set(path, key, una_abi::ATTR_TAG_STR, s.as_bytes()),
            trash_core::Attr::Int(i) => attr_set(path, key, una_abi::ATTR_TAG_INT, &i.to_le_bytes()),
        };
        r.map_err(|e| alloc::format!("attr {} {}", errno_word(e), e))
    }
    fn get_attr(&mut self, path: &str, key: &str) -> Option<trash_core::Attr> {
        match attr_get(path, key)? {
            (una_abi::ATTR_TAG_STR, p) => String::from_utf8(p).ok().map(trash_core::Attr::Str),
            (una_abi::ATTR_TAG_INT, p) if p.len() == 8 => {
                let mut w = [0u8; 8];
                w.copy_from_slice(&p);
                Some(trash_core::Attr::Int(i64::from_le_bytes(w)))
            }
            _ => None,
        }
    }
    fn remove_attr(&mut self, _path: &str, _key: &str) {}
    fn query(&mut self, q: &str) -> Vec<(u64, String)> {
        query(q)
    }
    fn list(&mut self, dir: &str) -> Result<Vec<String>, String> {
        list(dir).map(|v| v.into_iter().map(|n| String::from(n.trim_end_matches('/'))).collect()).map_err(|e| alloc::format!("list {} {}", errno_word(e), e))
    }
    fn unlink(&mut self, path: &str) -> Result<(), String> {
        unlink(path).map_err(|e| alloc::format!("unlink {} {}", errno_word(e), e))
    }
    fn rmdir(&mut self, path: &str) -> Result<(), String> {
        Err(alloc::format!("rmdir {}: ring 3 has no directory-removal door", path))
    }
    fn read_file(&mut self, path: &str) -> Option<Vec<u8>> {
        read_file(path, CAT_MAX, true).ok()
    }
    fn write_file(&mut self, path: &str, bytes: &[u8]) -> Result<(), String> {
        write_file(path, bytes).map(|_| ()).map_err(|e| alloc::format!("write {} {}", errno_word(e), e))
    }
    fn append_file(&mut self, path: &str, bytes: &[u8]) -> Result<(), String> {
        let mut all = read_file(path, CAT_MAX, true).unwrap_or_default();
        all.extend_from_slice(bytes);
        self.write_file(path, &all)
    }
}
