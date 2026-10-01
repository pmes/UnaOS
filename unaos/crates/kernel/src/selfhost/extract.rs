// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// SRCEXTRACT — SELFHOST rung 3: the verified source tree is MATERIALISED on the system volume under
// `/SRC/`, so a future in-tree build tool has files to read (ROADMAP §1c SH-5's precondition).
//
// Three verbs (`src extract [--dry-run]`, `src status`, `src verify`), one streaming tar parser.
//
// MECHANISM
//   * `SRC.TGZ` is pulled off the volume by the SELFHOST-2 `FatSource` (sha256 in the same pass),
//     inflated by `inflate::gunzip`, and the decompressed bytes are pushed into [`Stream`] — a tar
//     parser that turns the byte stream into `begin / data* / end` member events for a [`Handler`].
//     Nothing is buffered whole: the only payload buffer is `FILE_BUF` (64 KiB) per member.
//   * [`Extractor`] is the writing handler: `create_dir` / `create_in_dir` (8.3 or VFAT LFN, the
//     writer picks) and `write_grow` per 64 KiB flush. `write_grow` is called, not modified (SDHCMULTI2
//     is changing it). KNOWN COST: it re-collects the cluster chain per call, so a member of M
//     clusters costs O(M * chunks) FAT reads; the tree's largest members are small enough for that to be
//     noise, and claiming a whole run up front needs a FAT-writer entry this arc does not own.
//   * `target/` members and unsafe paths (`..`) are skipped; links are skipped; PAX `path=` and GNU
//     `L` long names are honoured (the member's name, not the truncated one).
//
// NAMES. FAT refuses control bytes and `" * / : < > ? \ |`, trailing dots/spaces, and names over its
// long-name slot budget. [`fat_name`] renames such a component DETERMINISTICALLY — illegal or
// non-ASCII chars become `_`, over-long names are truncated, and a `~hhhh` tag (16-bit FNV-1a of the
// ORIGINAL component) goes before the extension — so `src verify` recomputes the same name. A case-
// insensitive collision inside one directory (README vs readme) takes the SECOND tag ([`fat_name_alt`]).
// Every rename is logged: `[src] renamed a/b/c.rs -> SRC/a/b/c~1f2e.rs`.
//
// VERIFY. The extract keeps a 32-byte sha256 per member in a bounded ring of the LAST 64 files, then
// re-reads up to 8 random ring entries from the volume and compares sha + size (`verify=ok/n`).
// No re-inflate: the digest is taken from the same stream the writer consumed. The ring is the TAIL
// of the archive, so this is a spot check of the write path, not of the whole tree — `src verify`
// is the whole-tree pass (sizes of every file, hash of a 1-in-8 sample, a disk-side file count).

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::inflate::{self, Sink};
use super::{fat_reason, hex32, parse_stamp, tar, FatSource, SrcStamp, SHA_FILE_MAX};
use crate::fs::fat::{self, DirEntry, FatError, FatFs};
use crate::hash::Sha256;

/// Where the tree lives: the volume root, root-owned (the kernel principal writes it; DIRNS rule —
/// a system tree is a top-level directory, never under a home).
pub const ROOT_DIR: &str = "SRC";

const BLOCK: usize = 512;
const FILE_BUF: usize = 64 * 1024;
const META_MAX: usize = 8192;
const RING: usize = 64;
const SAMPLE: usize = 8;
const PROGRESS_EVERY: u32 = 256;
const RENAME_LOG_MAX: u32 = 64;
/// FAT long-name budget is 255 UTF-16 units; stay well inside it so the `~hhhh` tag always fits.
const NAME_MAX: usize = 200;

// ── the tar stream → member events ──────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    File,
    Dir,
}

pub struct Hdr {
    /// Normalised: no leading `./`, no trailing `/`.
    pub path: String,
    pub size: u64,
    pub kind: Kind,
}

pub trait Handler {
    /// A directory or regular-file member starts. `Err` aborts the whole walk.
    fn begin(&mut self, h: &Hdr) -> Result<(), ()>;
    /// A slice of the current file's payload (never empty, at most `FILE_BUF`).
    fn data(&mut self, d: &[u8]) -> Result<(), ()>;
    /// The current file's payload is complete (also called for a zero-length file).
    fn end(&mut self) -> Result<(), ()>;
    /// A member the stream refused to hand over (target/, unsafe path, link, unknown type).
    fn skipped(&mut self, path: &str, why: &'static str);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    File,
    Meta(u8),
    Skip,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StreamErr {
    Tar(tar::TarError),
    Handler,
}

pub struct Stream<H: Handler> {
    pub h: H,
    block: [u8; BLOCK],
    filled: usize,
    /// Payload bytes still to come for the current member (0 = in a header or in padding).
    left: u64,
    /// Zero padding bytes still to swallow after the payload.
    pad: u64,
    mode: Mode,
    zero_run: u32,
    pub saw_end: bool,
    pub err: Option<StreamErr>,
    long_path: Option<String>,
    meta: Vec<u8>,
    buf: Vec<u8>,
}

impl<H: Handler> Stream<H> {
    pub fn new(h: H) -> Self {
        Self {
            h,
            block: [0u8; BLOCK],
            filled: 0,
            left: 0,
            pad: 0,
            mode: Mode::Skip,
            zero_run: 0,
            saw_end: false,
            err: None,
            long_path: None,
            meta: Vec::new(),
            buf: Vec::new(),
        }
    }

    fn flush(&mut self) -> Result<(), StreamErr> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let r = self.h.data(&self.buf);
        self.buf.clear();
        r.map_err(|_| StreamErr::Handler)
    }

    fn finish_body(&mut self) -> Result<(), StreamErr> {
        match self.mode {
            Mode::File => {
                self.flush()?;
                self.h.end().map_err(|_| StreamErr::Handler)?;
            }
            Mode::Meta(t) => {
                let m = core::mem::take(&mut self.meta);
                match t {
                    b'L' => {
                        let end = m.iter().position(|&b| b == 0).unwrap_or(m.len());
                        if let Ok(s) = core::str::from_utf8(&m[..end]) {
                            self.long_path = Some(String::from(s));
                        }
                    }
                    b'x' => {
                        if let Some(p) = pax_path(&m) {
                            self.long_path = Some(p);
                        }
                    }
                    _ => {}
                }
            }
            Mode::Skip => {}
        }
        Ok(())
    }

    fn on_header(&mut self) -> Result<(), StreamErr> {
        let te = StreamErr::Tar;
        if self.block.iter().all(|&b| b == 0) {
            self.zero_run += 1;
            if self.zero_run >= 2 {
                self.saw_end = true;
            }
            return Ok(());
        }
        if self.saw_end {
            return Err(te(tar::TarError::TrailingData));
        }
        self.zero_run = 0;
        if &self.block[257..262] != b"ustar" {
            return Err(te(tar::TarError::BadHeader));
        }
        tar::verify_checksum(&self.block).map_err(te)?;
        let size = tar::octal(&self.block[124..136]).ok_or(te(tar::TarError::BadSize))?;
        let typeflag = self.block[156];
        self.pad = (BLOCK as u64 - size % BLOCK as u64) % BLOCK as u64;

        let set_body = |s: &mut Self, mode: Mode| {
            s.left = size;
            s.mode = mode;
            if mode == Mode::File {
                s.buf.clear();
            }
        };
        match typeflag {
            b'x' | b'L' | b'g' => {
                self.meta.clear();
                set_body(self, Mode::Meta(typeflag));
                return Ok(());
            }
            _ => {}
        }
        let raw = self.long_path.take().unwrap_or_else(|| tar::full_path(&self.block));
        let path = normalise(&raw);
        let kind = match typeflag {
            b'0' | 0 => Kind::File,
            b'5' => Kind::Dir,
            _ => {
                self.h.skipped(&path, "link or special member");
                set_body(self, Mode::Skip);
                return Ok(());
            }
        };
        if path.is_empty() {
            set_body(self, Mode::Skip);
            return Ok(());
        }
        if path.starts_with("target/") || path == "target" {
            self.h.skipped(&path, "target/");
            set_body(self, Mode::Skip);
            return Ok(());
        }
        if path.split('/').any(|c| c == ".." || c == "." || c.is_empty()) {
            self.h.skipped(&path, "unsafe path component");
            set_body(self, Mode::Skip);
            return Ok(());
        }
        let hdr = Hdr { path, size: if kind == Kind::File { size } else { 0 }, kind };
        self.h.begin(&hdr).map_err(|_| StreamErr::Handler)?;
        if kind == Kind::File {
            set_body(self, Mode::File);
            if size == 0 {
                self.finish_body()?;
                self.pad = 0;
            }
        } else {
            // A directory carries no payload.
            self.left = size;
            self.mode = Mode::Skip;
        }
        Ok(())
    }
}

impl<H: Handler> Sink for Stream<H> {
    fn push(&mut self, byte: u8) -> Result<(), ()> {
        if self.err.is_some() {
            return Err(());
        }
        let r = (|| -> Result<(), StreamErr> {
            if self.left > 0 {
                self.left -= 1;
                match self.mode {
                    Mode::File => {
                        if self.buf.capacity() == 0 {
                            self.buf.reserve_exact(FILE_BUF);
                        }
                        self.buf.push(byte);
                        if self.buf.len() >= FILE_BUF {
                            self.flush()?;
                        }
                    }
                    Mode::Meta(_) => {
                        if self.meta.len() < META_MAX {
                            self.meta.push(byte);
                        }
                    }
                    Mode::Skip => {}
                }
                if self.left == 0 {
                    self.finish_body()?;
                }
                return Ok(());
            }
            if self.pad > 0 {
                self.pad -= 1;
                return Ok(());
            }
            self.block[self.filled] = byte;
            self.filled += 1;
            if self.filled == BLOCK {
                self.filled = 0;
                self.on_header()?;
            }
            Ok(())
        })();
        match r {
            Ok(()) => Ok(()),
            Err(e) => {
                self.err = Some(e);
                Err(())
            }
        }
    }
}

/// Strip a leading `./` (repeatedly) and trailing `/`.
fn normalise(raw: &str) -> String {
    let mut s = raw;
    while let Some(r) = s.strip_prefix("./") {
        s = r;
    }
    String::from(s.trim_end_matches('/'))
}

/// PAX extended header: records are `<len> <key>=<value>\n`; return the last `path` value.
fn pax_path(m: &[u8]) -> Option<String> {
    let mut at = 0usize;
    let mut out = None;
    while at < m.len() {
        let sp = m[at..].iter().position(|&b| b == b' ')?;
        let len: usize = core::str::from_utf8(&m[at..at + sp]).ok()?.parse().ok()?;
        if len == 0 || at + len > m.len() {
            break;
        }
        let rec = &m[at + sp + 1..at + len];
        let rec = rec.strip_suffix(b"\n").unwrap_or(rec);
        if let Some(v) = rec.strip_prefix(b"path=") {
            if let Ok(s) = core::str::from_utf8(v) {
                out = Some(String::from(s));
            }
        }
        at += len;
    }
    out
}

// ── names ───────────────────────────────────────────────────────────────────────────────────────

fn fnv16(s: &str, salt: u32) -> u16 {
    let mut h: u32 = 0x811c_9dc5 ^ salt;
    for b in s.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    ((h >> 16) ^ (h & 0xFFFF)) as u16
}

fn tagged(clean: &str, tag: u16) -> String {
    let (base, ext) = match clean.rfind('.') {
        Some(i) if i > 0 => (&clean[..i], &clean[i..]),
        _ => (clean, ""),
    };
    format!("{}~{:04x}{}", base, tag, ext)
}

/// The deterministic FAT-safe spelling of one path component. Unchanged components pass through
/// untouched (and are NOT tagged); any component that had to change carries the `~hhhh` tag.
pub fn fat_name(comp: &str) -> String {
    name_with(comp, 0)
}

/// The spelling a component takes when [`fat_name`]'s own spelling collides (case-insensitively)
/// with an entry already in the directory.
pub fn fat_name_alt(comp: &str) -> String {
    name_with(comp, 0x5a5a_5a5a)
}

fn name_with(comp: &str, salt: u32) -> String {
    let mut s = String::new();
    for c in comp.chars() {
        let ok = (' '..='~').contains(&c) && !matches!(c, '"' | '*' | '/' | ':' | '<' | '>' | '?' | '\\' | '|');
        s.push(if ok { c } else { '_' });
    }
    // Leading space and trailing dots/spaces are refused by the long-name rules.
    if s.starts_with(' ') {
        s.replace_range(0..1, "_");
    }
    let keep = s.trim_end_matches(|c| c == ' ' || c == '.').len();
    if keep < s.len() {
        let n = s.len() - keep;
        s.truncate(keep);
        for _ in 0..n {
            s.push('_');
        }
    }
    let mut changed = s != comp;
    if s.len() > NAME_MAX {
        let ext_at = s.rfind('.').filter(|&i| i > 0 && s.len() - i <= 16);
        let ext = ext_at.map(|i| String::from(&s[i..])).unwrap_or_default();
        let keep = NAME_MAX - ext.len();
        s.truncate(keep);
        s.push_str(&ext);
        changed = true;
    }
    if changed || salt != 0 {
        tagged(&s, fnv16(comp, salt))
    } else {
        s
    }
}

// ── the extracting handler ──────────────────────────────────────────────────────────────────────

struct Member {
    path: String,
    size: u64,
    digest: [u8; 32],
}

struct Cur {
    path: String,
    expect: u64,
    size: u32,
    first: u32,
    lba: u64,
    off: usize,
    write: bool,
    sha: Sha256,
}

pub struct Extractor<'a> {
    fs: Option<&'a FatFs>,
    /// Original directory path ("" = the tree root) -> (first cluster, on-disk path).
    dirs: BTreeMap<String, (u32, String)>,
    cur: Option<Cur>,
    pub files: u32,
    pub dir_count: u32,
    pub bytes: u64,
    pub renamed: u32,
    pub skipped: u32,
    members: u32,
    ring: Vec<Member>,
    ring_at: usize,
    pub fail: Option<String>,
    say: &'a mut dyn FnMut(&str),
}

fn emit(say: &mut dyn FnMut(&str), line: &str) {
    serial_println!("{}", line);
    say(line);
}

impl<'a> Extractor<'a> {
    fn new(fs: Option<&'a FatFs>, say: &'a mut dyn FnMut(&str)) -> Result<Self, String> {
        let mut dirs = BTreeMap::new();
        let mut cl = 0u32;
        if let Some(f) = fs {
            cl = match f.locate_in_dir(0, ROOT_DIR) {
                Ok((de, _, _)) if de.is_dir => de.first_cluster(),
                Ok(_) => return Err(format!("/{} exists and is a FILE", ROOT_DIR)),
                Err(FatError::NotFound) => f
                    .create_dir(0, ROOT_DIR)
                    .map(|(de, _, _)| de.first_cluster())
                    .map_err(|e| format!("cannot create /{} ({})", ROOT_DIR, fat_reason(e)))?,
                Err(e) => return Err(format!("cannot look up /{} ({})", ROOT_DIR, fat_reason(e))),
            };
        }
        dirs.insert(String::new(), (cl, String::from(ROOT_DIR)));
        Ok(Self {
            fs,
            dirs,
            cur: None,
            files: 0,
            dir_count: 0,
            bytes: 0,
            renamed: 0,
            skipped: 0,
            members: 0,
            ring: Vec::new(),
            ring_at: 0,
            fail: None,
            say,
        })
    }

    fn note_rename(&mut self, from: &str, to: &str) {
        self.renamed += 1;
        if self.renamed <= RENAME_LOG_MAX {
            let l = format!("[src] renamed {} -> {}", from, to);
            emit(self.say, &l);
        } else if self.renamed == RENAME_LOG_MAX + 1 {
            emit(self.say, "[src] further renames are counted, not listed");
        }
    }

    /// Pick the on-disk name for `comp` inside directory `parent` (cluster), or `None` when no
    /// spelling is free. Returns `(name, existing)` where `existing` is the entry already there with
    /// exactly that spelling (a resumable file or a reusable directory).
    fn pick_name(&self, parent: u32, comp: &str, want_dir: bool) -> Result<(String, Option<(DirEntry, u64, usize)>), String> {
        let first = fat_name(comp);
        let Some(fs) = self.fs else { return Ok((first, None)) };
        for cand in [first, fat_name_alt(comp)] {
            match fs.locate_in_dir(parent, &cand) {
                Err(FatError::NotFound) => return Ok((cand, None)),
                Ok((de, l, o)) => {
                    if de.is_dir == want_dir {
                        return Ok((cand, Some((de, l, o))));
                    }
                    // A file where a directory is wanted (or the reverse): try the next spelling.
                }
                Err(e) => return Err(format!("lookup {} failed ({})", cand, fat_reason(e))),
            }
        }
        Err(format!("no free spelling for {}", comp))
    }

    fn ensure_dir(&mut self, orig: &str) -> Result<(u32, String), String> {
        if let Some(v) = self.dirs.get(orig) {
            return Ok(v.clone());
        }
        let (parent_orig, comp) = match orig.rsplit_once('/') {
            Some((p, c)) => (p, c),
            None => ("", orig),
        };
        let (pc, ppath) = self.ensure_dir(parent_orig)?;
        let (name, existing) = self.pick_name(pc, comp, true)?;
        let cl = match (existing, self.fs) {
            (Some((de, _, _)), _) => de.first_cluster(),
            (None, Some(fs)) => fs
                .create_dir(pc, &name)
                .map(|(de, _, _)| de.first_cluster())
                .map_err(|e| format!("mkdir {}/{} failed ({})", ppath, name, fat_reason(e)))?,
            (None, None) => 0,
        };
        let disk = format!("{}/{}", ppath, name);
        if name != comp {
            let from = String::from(orig);
            self.note_rename(&from, &disk);
        }
        self.dirs.insert(String::from(orig), (cl, disk.clone()));
        Ok((cl, disk))
    }

    fn progress(&mut self) {
        let l = format!(
            "[src] files={} dirs={} bytes={} renamed={} skipped={}",
            self.files, self.dir_count, self.bytes, self.renamed, self.skipped
        );
        emit(self.say, &l);
    }

    fn remember(&mut self, m: Member) {
        if self.ring.len() < RING {
            self.ring.push(m);
        } else {
            self.ring[self.ring_at] = m;
            self.ring_at = (self.ring_at + 1) % RING;
        }
    }
}

impl Handler for Extractor<'_> {
    fn begin(&mut self, h: &Hdr) -> Result<(), ()> {
        self.members += 1;
        if self.members % PROGRESS_EVERY == 0 {
            self.progress();
        }
        macro_rules! bail {
            ($e:expr) => {{
                self.fail = Some($e);
                return Err(());
            }};
        }
        if h.kind == Kind::Dir {
            if let Err(e) = self.ensure_dir(&h.path) {
                bail!(e);
            }
            self.dir_count += 1;
            return Ok(());
        }
        if h.size > i32::MAX as u64 {
            bail!(format!("{} is {} bytes — beyond a FAT32 file", h.path, h.size));
        }
        let (parent_orig, leaf) = match h.path.rsplit_once('/') {
            Some((p, c)) => (p, c),
            None => ("", h.path.as_str()),
        };
        let (pc, ppath) = match self.ensure_dir(parent_orig) {
            Ok(v) => v,
            Err(e) => bail!(e),
        };
        let (name, existing) = match self.pick_name(pc, leaf, false) {
            Ok(v) => v,
            Err(e) => bail!(e),
        };
        let disk = format!("{}/{}", ppath, name);
        if name != leaf {
            let from = h.path.clone();
            self.note_rename(&from, &disk);
        }
        let mut cur = Cur {
            path: disk.clone(),
            expect: h.size,
            size: 0,
            first: 0,
            lba: 0,
            off: 0,
            write: self.fs.is_some(),
            sha: Sha256::new(),
        };
        if let Some(fs) = self.fs {
            match existing {
                // Same size as the member ⇒ a finished earlier run (size is published LAST per
                // chunk, so a short file is never mistaken for a whole one): leave it, re-hash it.
                Some((de, _, _)) if de.size as u64 == h.size => {
                    cur.write = false;
                    self.skipped += 1;
                }
                other => {
                    if let Some((de, l, o)) = other {
                        if fs.delete_located(l, o, de.first_cluster()).is_err() {
                            bail!(format!("cannot replace stale {}", disk));
                        }
                    }
                    match fs.create_in_dir(pc, &name, 0x20) {
                        Ok((_, l, o)) => {
                            cur.lba = l;
                            cur.off = o;
                        }
                        Err(e) => bail!(format!("create {} failed ({})", disk, fat_reason(e))),
                    }
                }
            }
        }
        self.cur = Some(cur);
        Ok(())
    }

    fn data(&mut self, d: &[u8]) -> Result<(), ()> {
        let Some(cur) = self.cur.as_mut() else { return Err(()) };
        cur.sha.update(d);
        self.bytes += d.len() as u64;
        if cur.write {
            if let Some(fs) = self.fs {
                match fs.write_grow(cur.first, cur.size, cur.lba, cur.off, cur.size, d) {
                    Ok((n, new_size, new_first)) if n == d.len() => {
                        cur.size = new_size;
                        cur.first = new_first;
                    }
                    Ok(_) => {
                        self.fail = Some(format!("short write to {}", cur.path));
                        return Err(());
                    }
                    Err(e) => {
                        self.fail = Some(format!("write {} failed ({})", cur.path, fat_reason(e)));
                        return Err(());
                    }
                }
            }
        } else {
            cur.size = cur.size.saturating_add(d.len() as u32);
        }
        Ok(())
    }

    fn end(&mut self) -> Result<(), ()> {
        let Some(cur) = self.cur.take() else { return Err(()) };
        if cur.size as u64 != cur.expect {
            self.fail = Some(format!("{}: wrote {} of {} bytes", cur.path, cur.size, cur.expect));
            return Err(());
        }
        self.files += 1;
        let digest = cur.sha.finalize();
        self.remember(Member { path: cur.path, size: cur.expect, digest });
        Ok(())
    }

    fn skipped(&mut self, path: &str, why: &'static str) {
        self.skipped += 1;
        // `target/` is the expected skip and is counted quietly; anything else is named once.
        if why != "target/" && self.skipped <= RENAME_LOG_MAX {
            let l = format!("[src] skipped {} ({})", path, why);
            emit(self.say, &l);
        }
    }
}

// ── shared plumbing ─────────────────────────────────────────────────────────────────────────────

struct Payload {
    fs: FatFs,
    tgz: DirEntry,
    stamp: SrcStamp,
}

fn open_payload(tag: &str, say: &mut dyn FnMut(&str)) -> Option<Payload> {
    let fs = match fat::mount_program_source() {
        Ok(f) => f,
        Err(e) => {
            emit(say, &format!(":: {}: no program-source volume ({}) -> FAIL ::", tag, fat_reason(e)));
            return None;
        }
    };
    let (tgz, shafile) = match (fs.find_in_root("SRC.TGZ"), fs.find_in_root("SRC.SHA")) {
        (Ok(t), Ok(s)) => (t, s),
        _ => {
            emit(say, &format!(":: {}: no SRC.TGZ/SRC.SHA on {} — nothing to extract from -> FAIL ::", tag, fs.source_name()));
            return None;
        }
    };
    let mut raw = Vec::new();
    if let Err(e) = fs.read_file(&shafile, &mut raw, SHA_FILE_MAX) {
        emit(say, &format!(":: {}: SRC.SHA unreadable ({}) -> FAIL ::", tag, fat_reason(e)));
        return None;
    }
    let stamp = match core::str::from_utf8(&raw).ok().and_then(parse_stamp) {
        Some(s) => s,
        None => {
            emit(say, &format!(":: {}: SRC.SHA carries no 64-hex sha256 on line 1 -> FAIL ::", tag));
            return None;
        }
    };
    Some(Payload { fs, tgz, stamp })
}

/// Resolve an on-disk `SRC/a/b/c` path to its directory entry, component by component.
fn walk_disk(fs: &FatFs, disk: &str) -> Result<DirEntry, FatError> {
    let mut cl = 0u32;
    let mut last = None;
    for comp in disk.split('/') {
        let (de, _, _) = fs.locate_in_dir(cl, comp)?;
        cl = de.first_cluster();
        last = Some(de);
    }
    last.ok_or(FatError::NotFound)
}

fn hash_disk_file(fs: &FatFs, de: &DirEntry) -> Option<[u8; 32]> {
    let mut sha = Sha256::new();
    let mut pos = 0u32;
    let mut out = Vec::new();
    while pos < de.size {
        let want = core::cmp::min(FILE_BUF as u32, de.size - pos) as usize;
        fs.read_at(de.first_cluster(), de.size, pos, &mut out, want).ok()?;
        if out.is_empty() {
            return None;
        }
        sha.update(&out);
        pos += out.len() as u32;
    }
    Some(sha.finalize())
}

/// Run `gunzip` over `SRC.TGZ` into `stream`; returns (gzip ok, sha256 matches SRC.SHA, reason).
fn drive<H: Handler>(p: &Payload, stream: &mut Stream<H>) -> (bool, bool, Option<&'static str>) {
    let mut src = FatSource::new(&p.fs, &p.tgz);
    let gz = inflate::gunzip(&mut src, stream);
    src.drain();
    let sha_ok = src.sha.finalize() == p.stamp.sha;
    match gz {
        Ok(_) => {
            if !stream.saw_end {
                (false, sha_ok, Some("archive ended before the end-of-archive marker"))
            } else {
                (true, sha_ok, None)
            }
        }
        Err(e) => {
            let why = match stream.err {
                Some(StreamErr::Tar(te)) => tar::tar_reason(te),
                Some(StreamErr::Handler) => "extraction handler refused",
                None => inflate::inflate_reason(e),
            };
            (false, sha_ok, Some(why))
        }
    }
}

// ── M1: src extract ─────────────────────────────────────────────────────────────────────────────

/// `src extract [--dry-run]`. Returns true on PASS.
pub fn extract(dry: bool, say: &mut dyn FnMut(&str)) -> bool {
    let t0 = crate::clock::uptime_ms().unwrap_or(0);
    let Some(p) = open_payload("SRCEXTRACT", say) else { return false };
    if !dry {
        if let Some(why) = p.fs.write_veto() {
            emit(say, &format!(":: SRCEXTRACT: {} is write-protected ({}) -> FAIL ::", p.fs.source_name(), why));
            return false;
        }
    }
    emit(say, &format!("[src] {} /{} from SRC.TGZ on {}", if dry { "dry-run of" } else { "extracting to" }, ROOT_DIR, p.fs.source_name()));

    let ex = match Extractor::new(if dry { None } else { Some(&p.fs) }, say) {
        Ok(e) => e,
        Err(m) => {
            // `say` was moved into the failed constructor's scope only on success; report via serial.
            serial_println!(":: SRCEXTRACT: {} -> FAIL ::", m);
            return false;
        }
    };
    let mut stream = Stream::new(ex);
    let (gz_ok, sha_ok, why) = drive(&p, &mut stream);
    let fail = stream.h.fail.take();
    let (files, dirs, bytes, renamed, skipped) =
        (stream.h.files, stream.h.dir_count, stream.h.bytes, stream.h.renamed, stream.h.skipped);

    // Spot-check: up to SAMPLE random ring entries re-read from the volume.
    let (mut ok, mut n) = (0u32, 0u32);
    if !dry && gz_ok && fail.is_none() {
        let ring = core::mem::take(&mut stream.h.ring);
        let mut seed = [0u8; 32];
        let _ = crate::rand::fill(&mut seed);
        let take = core::cmp::min(SAMPLE, ring.len());
        for i in 0..take {
            let idx = (u32::from_le_bytes([seed[i * 2], seed[i * 2 + 1], seed[i * 2 + 2], 0]) as usize) % ring.len();
            let m = &ring[idx];
            n += 1;
            let good = match walk_disk(&p.fs, &m.path) {
                Ok(de) => de.size as u64 == m.size && hash_disk_file(&p.fs, &de) == Some(m.digest),
                Err(_) => false,
            };
            if good {
                ok += 1;
            } else {
                emit(stream.h.say, &format!("[src] verify MISMATCH {}", m.path));
            }
        }
    }
    let say = stream.h.say;
    let ms = crate::clock::uptime_ms().unwrap_or(0).saturating_sub(t0);

    if let Some(f) = fail {
        emit(say, &format!(":: SRCEXTRACT: {} after files={} dirs={} bytes={} -> FAIL ::", f, files, dirs, bytes));
        return false;
    }
    if let Some(w) = why {
        emit(say, &format!(":: SRCEXTRACT: walk aborted ({}) after files={} sha_match={} -> FAIL ::", w, files, sha_ok as u8));
        return false;
    }
    if !sha_ok {
        emit(
            say,
            &format!(
                ":: SRCEXTRACT: src sha256 does not match SRC.SHA (want {}){} -> FAIL ::",
                hex32(&p.stamp.sha),
                if dry { "" } else { " — /SRC was written from an UNVERIFIED payload, remove it" }
            ),
        );
        return false;
    }
    let verify_ok = dry || (n > 0 && ok == n);
    let verify_s = if dry { String::from("dry") } else { format!("{}/{}", ok, n) };
    let pass = files > 0 && verify_ok;
    emit(
        say,
        &format!(
            ":: SRCEXTRACT: files={} dirs={} bytes={} renamed={} ms={} verify={} -> {} ::",
            files,
            dirs,
            bytes,
            renamed,
            ms,
            verify_s,
            if pass { "PASS" } else { "FAIL" }
        ),
    );
    pass
}

/// The `tests srcextract` fixture: the dry run (walks, hashes and plans names; writes nothing).
pub fn dry_run_fixture() {
    let _ = extract(true, &mut |_| {});
}

// ── M2: src status ──────────────────────────────────────────────────────────────────────────────

/// Count regular files under the directory at `cl` (iterative; bounded by `limit` directories).
fn count_files(fs: &FatFs, cl: u32, limit: usize) -> Result<u32, FatError> {
    let mut stack = alloc::vec![cl];
    let (mut files, mut seen) = (0u32, 0usize);
    while let Some(d) = stack.pop() {
        seen += 1;
        if seen > limit {
            return Err(FatError::BadChain);
        }
        for de in fs.read_dir(d)? {
            let n = de.name();
            if n == "." || n == ".." {
                continue;
            }
            if de.is_dir {
                stack.push(de.first_cluster());
            } else {
                files += 1;
            }
        }
    }
    Ok(files)
}

/// `src status`: is `/SRC/` present, which commit SRC.SHA names, how many files it holds.
pub fn status(say: &mut dyn FnMut(&str)) {
    let fs = match fat::mount_program_source() {
        Ok(f) => f,
        Err(e) => {
            emit(say, &format!("[src] no program-source volume ({})", fat_reason(e)));
            return;
        }
    };
    let commit = fs
        .find_in_root("SRC.SHA")
        .ok()
        .and_then(|de| {
            let mut raw = Vec::new();
            fs.read_file(&de, &mut raw, SHA_FILE_MAX).ok()?;
            let st = core::str::from_utf8(&raw).ok().and_then(parse_stamp)?;
            Some(format!("{} ({})", st.commit.get(..12).unwrap_or("?"), if st.describe.is_empty() { "?" } else { st.describe.as_str() }))
        })
        .unwrap_or_else(|| String::from("unknown (no readable SRC.SHA)"));
    match fs.locate_in_dir(0, ROOT_DIR) {
        Ok((de, _, _)) if de.is_dir => match count_files(&fs, de.first_cluster(), 100_000) {
            Ok(n) => emit(say, &format!("[src] /{} present on {} files={} stamp-commit={}", ROOT_DIR, fs.source_name(), n, commit)),
            Err(e) => emit(say, &format!("[src] /{} present but unreadable ({})", ROOT_DIR, fat_reason(e))),
        },
        Ok(_) => emit(say, &format!("[src] /{} is a FILE, not a tree", ROOT_DIR)),
        Err(FatError::NotFound) => emit(say, &format!("[src] /{} absent on {} — run `src extract` (SRC.SHA commit {})", ROOT_DIR, fs.source_name(), commit)),
        Err(e) => emit(say, &format!("[src] /{} lookup failed ({})", ROOT_DIR, fat_reason(e))),
    }
    emit(say, "[src] the commit is SRC.SHA's provenance, not proof /SRC matches it — `src verify` compares");
}

// ── M3: src verify ──────────────────────────────────────────────────────────────────────────────

/// Handler that checks every tar file against the tree on disk: present, same size, and (1 in 8) the
/// same sha256.
struct Verifier<'a> {
    fs: &'a FatFs,
    dirs: BTreeMap<String, u32>,
    cur: Option<(DirEntry, Sha256, bool, String)>,
    seen: u32,
    missing: u32,
    size_bad: u32,
    hashed: u32,
    hash_bad: u32,
    skipped: u32,
    first_bad: Option<String>,
}

impl Verifier<'_> {
    /// Cluster of the on-disk directory for `orig`, trying both spellings per component.
    fn dir_cluster(&mut self, orig: &str) -> Option<u32> {
        if orig.is_empty() {
            if let Some(&c) = self.dirs.get("") {
                return Some(c);
            }
            let (de, _, _) = self.fs.locate_in_dir(0, ROOT_DIR).ok()?;
            self.dirs.insert(String::new(), de.first_cluster());
            return Some(de.first_cluster());
        }
        if let Some(&c) = self.dirs.get(orig) {
            return Some(c);
        }
        let (p, comp) = orig.rsplit_once('/').unwrap_or(("", orig));
        let pc = self.dir_cluster(p)?;
        let de = self.find(pc, comp, true)?;
        self.dirs.insert(String::from(orig), de.first_cluster());
        Some(de.first_cluster())
    }

    fn find(&self, parent: u32, comp: &str, want_dir: bool) -> Option<DirEntry> {
        for cand in [fat_name(comp), fat_name_alt(comp)] {
            if let Ok((de, _, _)) = self.fs.locate_in_dir(parent, &cand) {
                if de.is_dir == want_dir {
                    return Some(de);
                }
            }
        }
        None
    }

    fn bad(&mut self, path: &str) {
        if self.first_bad.is_none() {
            self.first_bad = Some(String::from(path));
        }
    }
}

impl Handler for Verifier<'_> {
    fn begin(&mut self, h: &Hdr) -> Result<(), ()> {
        if h.kind == Kind::Dir {
            if self.dir_cluster(&h.path).is_none() {
                self.missing += 1;
                let p = h.path.clone();
                self.bad(&p);
            }
            return Ok(());
        }
        self.seen += 1;
        let (p, leaf) = h.path.rsplit_once('/').unwrap_or(("", h.path.as_str()));
        let found = self.dir_cluster(p).and_then(|pc| self.find(pc, leaf, false));
        match found {
            None => {
                self.missing += 1;
                let hp = h.path.clone();
                self.bad(&hp);
                self.cur = None;
            }
            Some(de) => {
                if de.size as u64 != h.size {
                    self.size_bad += 1;
                    let hp = h.path.clone();
                    self.bad(&hp);
                }
                let sample = fnv16(&h.path, 0) % 8 == 0 && de.size as u64 == h.size;
                self.cur = Some((de, Sha256::new(), sample, h.path.clone()));
            }
        }
        Ok(())
    }

    fn data(&mut self, d: &[u8]) -> Result<(), ()> {
        if let Some((_, sha, sample, _)) = self.cur.as_mut() {
            if *sample {
                sha.update(d);
            }
        }
        Ok(())
    }

    fn end(&mut self) -> Result<(), ()> {
        if let Some((de, sha, sample, path)) = self.cur.take() {
            if sample {
                self.hashed += 1;
                // Zero-length files hash the empty stream; `hash_disk_file` agrees (loop never runs).
                if hash_disk_file(self.fs, &de) != Some(sha.finalize()) {
                    self.hash_bad += 1;
                    self.bad(&path);
                }
            }
        }
        Ok(())
    }

    fn skipped(&mut self, _path: &str, _why: &'static str) {
        self.skipped += 1;
    }
}

/// `src verify`: walk the tar and compare `/SRC/` against it. Returns true on PASS.
pub fn verify(say: &mut dyn FnMut(&str)) -> bool {
    let t0 = crate::clock::uptime_ms().unwrap_or(0);
    let Some(p) = open_payload("SRCVERIFY", say) else { return false };
    let root_cl = match p.fs.locate_in_dir(0, ROOT_DIR) {
        Ok((de, _, _)) if de.is_dir => de.first_cluster(),
        _ => {
            emit(say, &format!(":: SRCVERIFY: /{} is absent -> FAIL ::", ROOT_DIR));
            return false;
        }
    };
    let v = Verifier {
        fs: &p.fs,
        dirs: BTreeMap::new(),
        cur: None,
        seen: 0,
        missing: 0,
        size_bad: 0,
        hashed: 0,
        hash_bad: 0,
        skipped: 0,
        first_bad: None,
    };
    let mut stream = Stream::new(v);
    let (gz_ok, sha_ok, why) = drive(&p, &mut stream);
    let v = &stream.h;
    // Disk-side count: files on disk the tar does not account for (extras) or the reverse.
    let disk_files = count_files(&p.fs, root_cl, 100_000).unwrap_or(u32::MAX);
    let ms = crate::clock::uptime_ms().unwrap_or(0).saturating_sub(t0);
    let (seen, missing, size_bad, hashed, hash_bad) = (v.seen, v.missing, v.size_bad, v.hashed, v.hash_bad);
    if let Some(b) = &v.first_bad {
        emit(say, &format!("[src] first mismatch: {}", b));
    }
    let pass = gz_ok && sha_ok && why.is_none() && missing == 0 && size_bad == 0 && hash_bad == 0 && disk_files == seen && seen > 0;
    emit(
        say,
        &format!(
            ":: SRCVERIFY: tar_files={} disk_files={} missing={} size_bad={} hashed={} hash_bad={} sha_match={} ms={} -> {} ::",
            seen,
            disk_files,
            missing,
            size_bad,
            hashed,
            hash_bad,
            sha_ok as u8,
            ms,
            if pass { "PASS" } else { "FAIL" }
        ),
    );
    pass
}
