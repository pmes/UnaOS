// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — fulfiller
//!
//! SELFDIAG (rmbp-ledger B324, R82) — the kernel half of the smart installer's diagnosis program
//! (`APPS/DIAG.ELF`, crates/user-diag). Two things live here, neither a second implementation of the
//! diagnosis: that is `diag_core`, which this file and the program both link.
//!
//! * [`path_fulfil`] — `SYS_PATH_READ` (59) / `SYS_PATH_WRITE` (60): whole-path file I/O for ring 3, the
//!   kernel FULFILLING the volume's verbs over the VFS (the ATTRSURF pattern: one request buffer, the path
//!   resolved in the live namespace, the caller's principal). Layout in una-abi's SELFDIAG block. The
//!   diagnosis program reads `/var/log/boot.<n>.witness` and the selfhost tree and writes its record and
//!   the patched tree through these two numbers.
//! * [`principal_for`] — the installer's grant, PATH-scoped: the selfhost tree (`/SRC/`, `/boot/SRC/`), the
//!   system table (`/system/`, `/boot/system/`) and the diagnosis records (`/var/log/diag.*`) are read and
//!   written with the kernel's authority (the tree is root-owned by DIRNS rule; FAT's volume principal is
//!   the kernel). Every other path runs under the caller's principal. Program-scoped identity (only the
//!   installer may write the tree) is OWED (Holocron).
//! * [`selftest`] — `tests selfdiag`: the WHOLE pipe in the kernel over the VFS with the Echo fixture and
//!   no network — witness text → FAIL select → owner → prompt → Echo answer → parse → locate (fuzz) →
//!   stream-apply to `<file>~dgn` → copy back → verify bytes → record → the next boot's verdict.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::fs::vfs::{MountTable, NodeKind, VfsError, KERNEL_PRINCIPAL};
use diag_core::apply::{self, ReadAt, Span};
use diag_core::{diff, fixture, owners, prompt, record, witness, Out};

/// Lines of an owner's section on each side of the site.
pub const SECTION_RADIUS: u32 = 40;

fn errno(e: &VfsError) -> i64 {
    crate::fs::attrsys::errno_of(e)
}

/// Whether `p` is `root` or lies under it.
fn under(p: &str, root: &str) -> bool {
    p == root || (p.len() > root.len() && p.starts_with(root) && p.as_bytes()[root.len()] == b'/')
}

/// The principal a path-I/O request runs under (the installer's path-scoped grant, see the module note).
pub fn principal_for<'a>(path: &str, caller: &'a str) -> &'a str {
    let granted = under(path, "/SRC")
        || under(path, "/boot/SRC")
        || under(path, "/system")
        || under(path, "/boot/system")
        || (path.starts_with("/var/log/diag.") && !path[1..].contains("/.."));
    if granted { KERNEL_PRINCIPAL } else { caller }
}

/// A path the surface accepts: absolute, 1..=PATH_IO_PATH_MAX bytes, no empty/`.`/`..` component.
pub fn path_ok(p: &str) -> bool {
    if !p.starts_with('/') || p.len() > una_abi::PATH_IO_PATH_MAX || p.len() < 2 {
        return false;
    }
    p[1..].split('/').all(|c| !c.is_empty() && c != "." && c != "..")
}

/// Create every missing parent directory of `p`.
pub(crate) fn mkdirs(mt: &MountTable, p: &str, who: &str) -> Result<(), VfsError> {
    let mut i = 1;
    while let Some(k) = p[i..].find('/') {
        let d = &p[..i + k];
        match mt.stat(d) {
            Ok(st) if st.kind == NodeKind::Dir => {}
            Ok(_) => return Err(VfsError::NotADirectory),
            Err(_) => {
                mt.create(d, NodeKind::Dir, who)?;
            }
        }
        i += k + 1;
    }
    Ok(())
}

/// Create-or-truncate `p` and write `data` from offset 0 (unlink + create: FAT has no shrink).
pub(crate) fn put_file(mt: &MountTable, p: &str, data: &[u8], who: &str) -> Result<usize, VfsError> {
    match mt.stat(p) {
        Ok(st) if st.kind == NodeKind::Dir => return Err(VfsError::IsADirectory),
        Ok(_) => mt.unlink(p, who)?,
        Err(_) => {}
    }
    mt.create(p, NodeKind::File, who)?;
    write_at(mt, p, 0, data, who)
}

fn write_at(mt: &MountTable, p: &str, off: u64, data: &[u8], who: &str) -> Result<usize, VfsError> {
    let mut done = 0usize;
    while done < data.len() {
        match mt.write(p, off + done as u64, &data[done..], who)? {
            0 => return Err(VfsError::Backend("short-write")),
            w => done += w,
        }
    }
    Ok(done)
}

/// `SYS_PATH_READ` / `SYS_PATH_WRITE` over the VFS. `inb` is the whole request; `cap` the read ceiling.
/// Returns `(bytes to copy out, return value)` or `-errno`.
pub fn path_fulfil(nr: u64, inb: &[u8], caller: &str, cap: usize) -> Result<(Vec<u8>, i64), i64> {
    let h = una_abi::PATH_IO_HDR_LEN;
    if inb.len() < h {
        return Err(una_abi::EINVAL);
    }
    let plen = u16::from_le_bytes([inb[0], inb[1]]) as usize;
    let flags = u16::from_le_bytes([inb[2], inb[3]]);
    let rsv = u32::from_le_bytes([inb[4], inb[5], inb[6], inb[7]]);
    let mut o8 = [0u8; 8];
    o8.copy_from_slice(&inb[8..16]);
    let off = u64::from_le_bytes(o8);
    if rsv != 0 || plen == 0 || inb.len() < h + plen {
        return Err(una_abi::EINVAL);
    }
    let path = core::str::from_utf8(&inb[h..h + plen]).map_err(|_| una_abi::EINVAL)?;
    if !path_ok(path) {
        return Err(una_abi::EINVAL);
    }
    let data = &inb[h + plen..];
    let who = principal_for(path, caller);
    if nr == una_abi::SYS_PATH_READ && flags == 0 && data.is_empty() && who == KERNEL_PRINCIPAL {
        if let Some(b) = crate::video::text::resident_read(path, off, cap) {
            let k = b.len() as i64;
            return Ok((b, k)); // LUMENFAST (B507): a face the kernel holds, from that copy — no mount table, no volume walk
        }
    }
    if nr == una_abi::SYS_PATH_WRITE {
        crate::video::text::resident_forget(path); // LUMENFAST (B507): the file changes; the resident copy no longer stands for it
    }
    let mt = crate::shell::vfs_mount_table();
    if nr == una_abi::SYS_PATH_READ {
        if !data.is_empty() {
            return Err(una_abi::EINVAL);
        }
        if flags & una_abi::PATH_R_LIST != 0 {
            return list_dir(&mt, path, caller, off, cap); // HOLOCRON2 (B355): a directory's names, inside the caller's home
        }
        let st = if who == KERNEL_PRINCIPAL { mt.stat(path) } else { mt.open_read(path, who) }.map_err(|e| errno(&e))?;
        if st.kind == NodeKind::Dir {
            return Err(una_abi::EISDIR);
        }
        if off >= st.size || cap == 0 {
            return Ok((Vec::new(), 0));
        }
        let n = ((st.size - off) as usize).min(cap);
        let b = mt.read(path, off, n).map_err(|e| errno(&e))?;
        let k = b.len() as i64;
        return Ok((b, k));
    }
    if nr != una_abi::SYS_PATH_WRITE || data.len() > una_abi::PATH_IO_MAX {
        return Err(una_abi::EINVAL);
    }
    if flags & !(una_abi::PATH_W_TRUNC | una_abi::PATH_W_MKDIRS | una_abi::PATH_W_UNLINK) != 0 {
        return Err(una_abi::EINVAL);
    }
    if flags & una_abi::PATH_W_UNLINK != 0 {
        if !data.is_empty() || off != 0 {
            return Err(una_abi::EINVAL);
        }
        mt.unlink(path, who).map_err(|e| errno(&e))?;
        return Ok((Vec::new(), 0));
    }
    if flags & una_abi::PATH_W_MKDIRS != 0 {
        mkdirs(&mt, path, who).map_err(|e| errno(&e))?;
    }
    if flags & una_abi::PATH_W_TRUNC != 0 {
        if off != 0 {
            return Err(una_abi::EINVAL);
        }
        let w = put_file(&mt, path, data, who).map_err(|e| errno(&e))?;
        return Ok((Vec::new(), w as i64));
    }
    let st = mt.stat(path).map_err(|e| errno(&e))?;
    if st.kind == NodeKind::Dir {
        return Err(una_abi::EISDIR);
    }
    if off > st.size {
        return Err(una_abi::EINVAL);
    }
    let w = write_at(&mt, path, off, data, who).map_err(|e| errno(&e))?;
    Ok((Vec::new(), w as i64))
}

// ── tests selfdiag ──────────────────────────────────────────────────────────────────────────────────

/// A VFS file as a `diag_core::apply::ReadAt` source (the program's twin reads through SYS_PATH_READ).
struct VfsSrc<'a> {
    mt: &'a MountTable,
    p: &'a str,
    size: u64,
}

impl ReadAt for VfsSrc<'_> {
    fn read_at(&mut self, off: u64, buf: &mut [u8]) -> Result<usize, i64> {
        if off >= self.size || buf.is_empty() {
            return Ok(0);
        }
        let n = ((self.size - off) as usize).min(buf.len());
        let b = self.mt.read(self.p, off, n).map_err(|e| errno(&e))?;
        buf[..b.len()].copy_from_slice(&b);
        Ok(b.len())
    }
}

fn src_of<'a>(mt: &'a MountTable, p: &'a str) -> Result<VfsSrc<'a>, i64> {
    let st = mt.stat(p).map_err(|e| errno(&e))?;
    Ok(VfsSrc { mt, p, size: st.size })
}

/// Register `tests selfdiag` once.
pub fn ensure() {
    use core::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, Ordering::AcqRel) {
        crate::tests::register("selfdiag", selftest);
    }
}

/// `tests selfdiag`: the whole pipe with the Echo fixture (see the module note).
pub fn selftest() {
    let (bootlog, n, kept, lost, _) = crate::bootwit::state();
    let mt = crate::shell::vfs_mount_table();
    serial_println!("[selfdiag] bootlog state={} boot={} kept={} lost={} tree={}", bootlog, n, kept, lost, tree_root(&mt).as_deref().unwrap_or("none"));
    let unafs = crate::bootwit::root_is_unafs(&mt);
    let dir: &str = if unafs { "/var/tmp/selfdiag" } else { "/boot/SDFIX" };
    match run_fixture(&mt, dir) {
        Ok(f) => {
            serial_println!("[selfdiag] fx root={} dir={} prompt={} answer={} hunks={} next={}", if unafs { "unafs" } else { "fat" }, dir, f.prompt, f.answer, f.hunks, f.next);
            let pass = f.fails == 1 && f.patched == 1 && f.refused == 0 && f.next == "fixed";
            serial_println!(":: SELFDIAG: provider=echo fails={} patched={} asked={} refused={} next={} bootlog={} -> {} ::",
                f.fails, f.patched, f.asked, f.refused, f.next, bootlog, if pass { "PASS" } else { "FAIL" });
        }
        Err((stage, e)) => {
            serial_println!(":: SELFDIAG: provider=echo stage={} err={} dir={} bootlog={} -> FAIL ::", stage, e, dir, bootlog);
        }
    }
}

struct Fx {
    fails: usize,
    asked: usize,
    patched: usize,
    refused: usize,
    prompt: usize,
    answer: usize,
    hunks: usize,
    next: &'static str,
}

fn run_fixture(mt: &MountTable, dir: &str) -> Result<Fx, (&'static str, i64)> {
    let k = KERNEL_PRINCIPAL;
    let io = |s: &'static str| move |e: VfsError| (s, errno(&e));
    // 1. A boot's witness text with one canned FAIL among other lines.
    let wit = format!("# boot 0 witness fixture\n:: AHCI: port=0 -> PASS ::\n{}\n:: BOOT: total=1 ms ::\n", fixture::LINE);
    let fl: Vec<&str> = witness::fails(&wit).collect();
    // 2. Lay the fixture source and its owners table under the scratch tree.
    let src_path = format!("{}/{}", dir, fixture::PATH);
    mkdirs(mt, &src_path, k).map_err(io("mkdirs"))?;
    put_file(mt, &src_path, fixture::SOURCE.as_bytes(), k).map_err(io("lay"))?;
    let own_path = format!("{}/witness-owners.txt", dir);
    put_file(mt, &own_path, fixture::OWNERS.as_bytes(), k).map_err(io("lay-owners"))?;
    let table = mt.read(&own_path, 0, fixture::OWNERS.len()).map_err(io("read-owners"))?;
    let table = core::str::from_utf8(&table).map_err(|_| ("owners-utf8", 0))?;
    // 3. Prompt: each FAIL with its owner and section, read back off the volume.
    let mut pbuf = alloc::vec![0u8; 16 * 1024];
    let mut sec = alloc::vec![0u8; 8 * 1024];
    let mut scratch = alloc::vec![0u8; 4096];
    let mut o = Out::new(&mut pbuf);
    prompt::begin(&mut o, 0, fl.len(), Some(dir));
    let mut asked = 0usize;
    let mut answers: Vec<&'static str> = Vec::new();
    for (i, line) in fl.iter().enumerate() {
        let tag = witness::tag(line).unwrap_or("?");
        let owner = owners::lookup(table, tag);
        let mut section = None;
        let mut sn = 0usize;
        let mut first = 0u32;
        if let Some((p, site)) = owner {
            let fp = format!("{}/{}", dir, p);
            let mut s = src_of(mt, &fp).map_err(|e| ("section", e))?;
            first = site.saturating_sub(SECTION_RADIUS).max(1);
            let (b, _) = apply::read_lines(&mut s, first, 2 * SECTION_RADIUS + 1, &mut sec, &mut scratch).map_err(|r| ("section", r_code(r)))?;
            sn = b;
        }
        if sn > 0 {
            section = core::str::from_utf8(&sec[..sn]).ok().map(|t| (t, first));
        }
        prompt::item(&mut o, i, &prompt::Item { line, tag, owner, section }, 12 * 1024);
        // 4. The Echo provider answers the canned line.
        asked += 1;
        if let Some(a) = fixture::answer(line) {
            answers.push(a);
        }
    }
    prompt::end(&mut o);
    let plen = o.done().ok_or(("prompt-overflow", 0))?;
    let answer = *answers.first().ok_or(("echo-silent", 0))?;
    // 5. Parse, resolve every hunk of every file, then apply (stream to ~dgn, copy back, unlink).
    let patch = diff::parse(diff::extract(answer).ok_or(("no-diff", 0))?).map_err(|r| ("parse", r_code(r)))?;
    let mut spans: Vec<Vec<Span>> = Vec::new();
    for (fi, f) in patch.files().iter().enumerate() {
        let hs = patch.hunks_of(f);
        let mut sp = alloc::vec![Span::default(); hs.len()];
        let fp = format!("{}/{}", dir, f.path);
        let mut s = src_of(mt, &fp).map_err(|e| ("open", e))?;
        apply::resolve(&mut s, hs, &mut sp, &mut scratch, fi).map_err(|r| ("resolve", r_code(r)))?;
        spans.push(sp);
    }
    let mut patched = 0usize;
    for (fi, f) in patch.files().iter().enumerate() {
        let fp = format!("{}/{}", dir, f.path);
        let tmp = format!("{}~dgn", fp);
        let mut out: Vec<u8> = Vec::new();
        {
            let mut s = src_of(mt, &fp).map_err(|e| ("open", e))?;
            apply::emit(&mut s, patch.hunks_of(f), &spans[fi], &mut scratch, &mut |b| {
                out.extend_from_slice(b);
                Ok(())
            })
            .map_err(|r| ("emit", r_code(r)))?;
        }
        put_file(mt, &tmp, &out, k).map_err(io("write-dgn"))?;
        let back = mt.read(&tmp, 0, out.len()).map_err(io("read-dgn"))?;
        put_file(mt, &fp, &back, k).map_err(io("copy-back"))?;
        let _ = mt.unlink(&tmp, k);
        patched += 1;
    }
    // 6. Verify the bytes on the volume.
    let got = mt.read(&src_path, 0, fixture::PATCHED.len() + 64).map_err(io("verify"))?;
    if got != fixture::PATCHED.as_bytes() {
        return Err(("verify-bytes", got.len() as i64));
    }
    // 7. The record, then the NEXT boot's verdict on it (the bootwit half of the loop).
    let tags: Vec<&str> = fl.iter().filter_map(|l| witness::tag(l)).collect();
    let mut rbuf = alloc::vec![0u8; 24 * 1024];
    let mut r = Out::new(&mut rbuf);
    record::head(&mut r, 0, "echo", &tags, asked, patched, 0, "patched");
    record::section(&mut r, "prompt", &pbuf[..plen]);
    record::section(&mut r, "answer", answer.as_bytes());
    record::rebuild(&mut r);
    let rn = r.done().ok_or(("record-overflow", 0))?;
    let md_path = format!("{}/diag.0.md", dir);
    put_file(mt, &md_path, &rbuf[..rn], k).map_err(io("record"))?;
    let md = mt.read(&md_path, 0, rn).map_err(io("record-read"))?;
    let mds = core::str::from_utf8(&md).map_err(|_| ("record-utf8", 0))?;
    let mut nb = [0u8; 512];
    let mut no = Out::new(&mut nb);
    let next = if record::next_boot(mds, 1, ":: SDFIX: probe=1 -> PASS ::\n", &mut no) {
        let t = core::str::from_utf8(no.bytes()).unwrap_or("");
        if t.contains("verdict: fixed") { "fixed" } else if t.contains("still-failing") { "still-failing" } else { "unseen" }
    } else {
        "none"
    };
    Ok(Fx { fails: fl.len(), asked, patched, refused: 0, prompt: plen, answer: answer.len(), hunks: patch.total_hunks(), next })
}

fn r_code(r: diff::Refuse) -> i64 {
    match r {
        diff::Refuse::Io(e) => e,
        diff::Refuse::Mismatch(f, h) => -(1000 + f as i64 * 100 + h as i64),
        _ => -1,
    }
}

/// The selfhost tree's root on this boot (`/SRC` on a FAT root, `/boot/SRC` beside a UnaFS root), if
/// `src extract` has materialised it.
pub fn tree_root(mt: &MountTable) -> Option<String> {
    for r in ["/boot/SRC", "/SRC"] {
        if matches!(mt.stat(r), Ok(st) if st.kind == NodeKind::Dir) {
            return Some(String::from(r));
        }
    }
    None
}

// ── HOLOCRON2 (rmbp-ledger B355): `PATH_R_LIST` ─────────────────────────────────────────────────────
/// A directory's entry names for ring 3 (`\n`-joined; a directory gets a trailing `/`), from byte `off` of
/// that listing, at most `cap` bytes. Only inside the CALLER's own home: `caller` is the ATTRSURF principal
/// `user:<name>#<uid>`, the home is the users store's record for `<name>` (never a `/home/<name>` literal),
/// and an anonymous caller lists nothing. Names are what the directory holds — the medium's authority, so
/// Holocron's `SecretList` has no index file to drift from it.
fn list_dir(mt: &MountTable, path: &str, caller: &str, off: u64, cap: usize) -> Result<(Vec<u8>, i64), i64> {
    if !in_own_home(path, caller) {
        return Err(una_abi::EACCES);
    }
    let st = mt.stat(path).map_err(|e| errno(&e))?;
    if st.kind != NodeKind::Dir {
        return Err(una_abi::ENOTDIR);
    }
    let ents = mt.read_dir(path).map_err(|e| errno(&e))?;
    let mut all = Vec::new();
    for e in ents {
        if e.name == "." || e.name == ".." {
            continue;
        }
        all.extend_from_slice(e.name.as_bytes());
        if e.kind == NodeKind::Dir {
            all.push(b'/');
        }
        all.push(b'\n');
    }
    let o = (off as usize).min(all.len());
    let n = (all.len() - o).min(cap);
    let out = all[o..o + n].to_vec();
    Ok((out, n as i64))
}

/// Is `path` the caller's home or below it?
fn in_own_home(path: &str, caller: &str) -> bool {
    #[cfg(feature = "login")]
    {
        let Some(rest) = caller.strip_prefix("user:") else { return false };
        let name = rest.split('#').next().unwrap_or("");
        let mut h = [0u8; crate::fs::users::HOME_MAX];
        let Some(n) = crate::fs::users::home_of(name.as_bytes(), &mut h) else { return false };
        let Ok(home) = core::str::from_utf8(&h[..n]) else { return false };
        let home = home.trim_end_matches('/');
        return !home.is_empty() && (path == home || (path.starts_with(home) && path.as_bytes().get(home.len()) == Some(&b'/')));
    }
    #[cfg(not(feature = "login"))]
    {
        let _ = (path, caller);
        false
    }
}
