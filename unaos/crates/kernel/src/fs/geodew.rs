//! CHARTER: Geode — shared-core (the `.geode` v1 codec is `unaos/libs/fs/geode_core`, the crate `handlers/geode` links, a normal kernel dependency: ONE encoder, ONE decoder; GEODEMOD SR157, GEODEDEP SR160)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//!
//! GEODEBACKUP (rmbp-ledger B740, BP-bar-1/14, BP-storage-28). The user's files outlive any one disk: the home goes
//! to a removable volume as ONE `.geode` v1 archive (GEODEMETA SR148), and comes back from one, every record whole —
//! kind, bytes, every attribute (small and spilled) and the three §R7 times.
//!
//! GEODEMOD (LEDGER SR157): the format — header, entry heads, trailer, the bounded streaming decoder and the
//! archive's structural rules — is [`geode_core`], the very file the host handler (`handlers/geode`) links
//! (`unaos/libs/fs/geode_core`, a normal kernel dependency since GEODEDEP SR160; GEODEMOD's `#[path]` is
//! retired). This module keeps only what is the kernel's: the volume walk, the mount holds and the
//! Settings face's worker.
//!
//! * **The writer streams.** [`plan`] walks the directory in the crate's order (pre-order, siblings in `ls` order)
//!   keeping names and inode ids only, and measures the exact archive length (`geode_core::entry_len`); [`write`]
//!   then emits each entry head from its own hold through `geode_core::Encoder` and the file's bytes in
//!   [`CHUNK`]-sized holds, so the home is never in RAM and the IRQ-masked mount lock is never held for the volume
//!   (BP-storage-8).
//! * **Versions are NOT carried in v1.** A file's previous versions (`una:versions`, §R17, BP-storage-24) are not
//!   attributes and the format has no field for them: a backup carries each file's current bytes only.
//! * **The reader restores** by the crate's `unpack` rules: the archive is checked whole first (`geode_core::check`,
//!   streamed: names only in RAM), every top-level name is checked free in the target (a taken name writes
//!   nothing), then each entry is created, written and given its attributes in order (`geode_core::Decoder`), and
//!   the times are set last, deepest first.
//!
//! The volume is a [`Vol`]: any `UnaFS<D>` (the fixture's RAM volumes) or [`Bound`] (THE mount, one `with_unafs`
//! per hold). The Settings face and its worker task are [`job`] (x86). Fixture: `tests geodebackup`.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use ::unafs::fs::{DirEntry, FileSystemError};
use ::unafs::{BlockDevice, FileKind, UnaFS};

// GEODEMOD (SR157): the shared `.geode` v1 codec — the host handler's own crate. GEODEDEP (SR160): a normal
// dependency (crates/kernel/Cargo.toml, `geode_core` under `unafs` and the aarch64 deps), no longer `#[path]`;
// re-exported under its old name so `fs::geodew::geode_core` and every `geode_core::…` below hold unchanged.
pub use ::geode_core;

pub use self::geode_core::{Bytes, Sink, Src, Why, CHUNK};
use self::geode_core::{leaf, parent_of, Decoder, Encoder, HEADER_LEN, TRAILER_LEN};

/// Deepest nesting walked before the volume is called corrupt (the crate's bound).
const MAX_DEPTH: usize = ::unafs::fs::MAX_PATH_DEPTH;

fn fse(e: FileSystemError) -> Why {
    match e {
        FileSystemError::NoSpace => "nospace",
        FileSystemError::NotFound => "changed",
        FileSystemError::NotADirectory => "not-a-directory",
        _ => "unafs",
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The volume seam
// ---------------------------------------------------------------------------------------------------------------

/// A UnaFS volume the writer reads and the reader writes, one hold at a time.
pub trait Vol {
    type D: BlockDevice;
    /// Run `f` in one hold (THE mount: one `with_unafs`).
    fn run<R>(&mut self, f: &mut dyn FnMut(&mut UnaFS<Self::D>) -> R) -> Result<R, Why>;
    /// Write one chunk of a file's bytes (BP-storage-21's one mapping of a full volume).
    fn put(fs: &mut UnaFS<Self::D>, id: u64, off: u64, data: &[u8]) -> Result<(), Why>;
}

/// Any volume held directly (the fixture's RAM volumes).
impl<D: BlockDevice> Vol for UnaFS<D> {
    type D = D;
    fn run<R>(&mut self, f: &mut dyn FnMut(&mut UnaFS<D>) -> R) -> Result<R, Why> {
        Ok(f(self))
    }
    fn put(fs: &mut UnaFS<D>, id: u64, off: u64, data: &[u8]) -> Result<(), Why> {
        super::unafs::diskfull::write_or_full(fs, "ram", id, off, data).map_err(|_| "nospace")
    }
}

/// THE mount (the root volume), one `with_unafs` per hold.
pub struct Bound;

impl Vol for Bound {
    type D = ::unafs::adapter::BlockAdapter<super::unafs::SdSectorDevice>;
    fn run<R>(&mut self, f: &mut dyn FnMut(&mut UnaFS<Self::D>) -> R) -> Result<R, Why> {
        super::unafs::with_unafs(|fs| f(fs)).map_err(|_| "busy")
    }
    fn put(fs: &mut UnaFS<Self::D>, id: u64, off: u64, data: &[u8]) -> Result<(), Why> {
        super::unafs::diskfull::write_root(fs, "native", id, off, data).map_err(|_| "nospace")
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Backup: plan (names + the measured length), then the streamed write
// ---------------------------------------------------------------------------------------------------------------

/// One entry to archive: its relative path, inode and kind (no bytes, no attributes).
pub struct Item {
    pub path: String,
    pub id: u64,
    pub kind: FileKind,
}

/// The walk: the entries in archive order, how many are files, and the archive's exact length.
pub struct Plan {
    pub items: Vec<Item>,
    pub files: u32,
    pub archive_bytes: u64,
    pub data_bytes: u64,
}

/// Walk directory `dir` (what it holds, not itself) in the crate's archive order: one hold per node.
pub fn plan<V: Vol>(v: &mut V, dir: &str) -> Result<Plan, Why> {
    let top = v.run(&mut |fs| fs.resolve_path(dir))?.map_err(fse)?;
    let mut p = Plan { items: Vec::new(), files: 0, archive_bytes: HEADER_LEN + TRAILER_LEN, data_bytes: 0 };
    let mut stack: Vec<(u64, String, usize)> = alloc::vec![(top, String::new(), 0)];
    while let Some((id, rel, depth)) = stack.pop() {
        let (kind, size, alen, kids) = v
            .run(&mut |fs| -> Result<(FileKind, u64, u64, Vec<DirEntry>), FileSystemError> {
                let ino = fs.read_inode(id)?;
                let alen = if rel.is_empty() { 0 } else { geode_core::attrs_len(&super::unafs::record_attrs_on(fs, id, &ino)?) };
                let kids = if ino.kind == FileKind::Directory { fs.ls(id)? } else { Vec::new() };
                Ok((ino.kind, ino.size, alen, kids))
            })?
            .map_err(fse)?;
        if rel.is_empty() && kind != FileKind::Directory {
            return Err("not-a-directory");
        }
        if !rel.is_empty() {
            if !matches!(kind, FileKind::File | FileKind::Directory) {
                return Err("kind"); // the crate's Unsupported: a symlink or system record is not archived
            }
            let data = if kind == FileKind::File { size } else { 0 };
            p.archive_bytes += geode_core::entry_len(rel.len(), alen, data);
            p.data_bytes += data;
            p.files += (kind == FileKind::File) as u32;
            p.items.push(Item { path: rel.clone(), id, kind });
        }
        if kind != FileKind::Directory {
            continue;
        }
        if depth >= MAX_DEPTH {
            return Err("depth");
        }
        for c in kids.into_iter().rev() {
            let child = if rel.is_empty() { c.name.clone() } else { format!("{}/{}", rel, c.name) };
            stack.push((c.inode_id, child, depth + 1));
        }
    }
    Ok(p)
}

/// Stream `plan`'s entries from `v` into `sink` as a `.geode` v1 archive (`geode_core::Encoder`);
/// `tick(done_files, total_files)` after each file. The archive's length on success.
pub fn write<V: Vol, S: Sink>(v: &mut V, plan: &Plan, sink: &mut S, tick: &mut dyn FnMut(u32, u32)) -> Result<u64, Why> {
    let mut enc = Encoder::new(sink, plan.items.len() as u64)?;
    let mut done = 0u32;
    for it in &plan.items {
        let id = it.id;
        let (head, size) = v
            .run(&mut |fs| -> Result<(Vec<u8>, u64), FileSystemError> {
                let ino = fs.read_inode(id)?;
                if ino.kind != it.kind {
                    return Err(FileSystemError::NotFound); // re-identified: refused as `changed`
                }
                let attrs = super::unafs::record_attrs_on(fs, id, &ino)?;
                let size = if ino.kind == FileKind::File { ino.size } else { 0 };
                Ok((geode_core::entry_head(&it.path, ino.kind, ino.ctime, ino.mtime, ino.atime, &attrs, size), size))
            })?
            .map_err(fse)?;
        enc.put(&head)?;
        let mut off = 0u64;
        while off < size {
            let n = (size - off).min(CHUNK as u64);
            let b = v.run(&mut |fs| fs.read_data(id, off, n))?.map_err(fse)?;
            if b.len() as u64 != n {
                return Err("changed"); // the file shrank between its head and its bytes
            }
            enc.put(&b)?;
            off += n;
        }
        if it.kind == FileKind::File {
            done += 1;
            tick(done, plan.files);
        }
    }
    enc.finish()
}

/// [`plan`] then [`write`]: the whole backup of `dir` into `sink`. `(files, archive bytes)`.
pub fn backup_to<V: Vol, S: Sink>(v: &mut V, dir: &str, sink: &mut S) -> Result<(u32, u64), Why> {
    let p = plan(v, dir)?;
    let n = write(v, &p, sink, &mut |_, _| {})?;
    Ok((p.files, n))
}

// ---------------------------------------------------------------------------------------------------------------
// Restore: check the archive whole (streamed), then unpack (streamed)
// ---------------------------------------------------------------------------------------------------------------

/// What a restore did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Restored {
    pub files: u32,
    pub dirs: u32,
    pub attrs: u32,
    pub bytes: u64,
}

/// Restore an archive under directory `dir` of `v`. `make`: `dir` must not exist and is created (its name taken =
/// `taken`); else it must exist and hold none of the archive's top-level names (`taken`). The archive is checked
/// whole first (`geode_core::check`) and nothing is written before every check passes. `tick(done_files,
/// total_files)` after each file.
pub fn restore<V: Vol, S: Src>(v: &mut V, src: &mut S, dir: &str, make: bool, tick: &mut dyn FnMut(u32, u32)) -> Result<Restored, Why> {
    let sum = geode_core::check(src)?;
    let top = if make {
        let (pdir, name) = match dir.rsplit_once('/') {
            Some(("", n)) => ("/", n),
            Some((p, n)) => (p, n),
            None => return Err("path"),
        };
        if name.is_empty() {
            return Err("path");
        }
        v.run(&mut |fs| -> Result<Result<u64, Why>, FileSystemError> {
            let p = fs.resolve_path(pdir)?;
            if fs.ls(p)?.iter().any(|e| e.name == name) {
                return Ok(Err("taken"));
            }
            Ok(Ok(fs.mkdir(p, String::from(name))?))
        })?
        .map_err(fse)??
    } else {
        v.run(&mut |fs| -> Result<Result<u64, Why>, FileSystemError> {
            let t = fs.resolve_path(dir)?;
            if fs.read_inode(t)?.kind != FileKind::Directory {
                return Err(FileSystemError::NotADirectory);
            }
            let present: BTreeSet<String> = fs.ls(t)?.into_iter().map(|e| e.name).collect();
            if sum.tops.iter().any(|n| present.contains(n)) {
                return Ok(Err("taken"));
            }
            Ok(Ok(t))
        })?
        .map_err(fse)??
    };
    let mut d = Decoder::open(src)?;
    let count = d.count();
    let mut ids: BTreeMap<String, u64> = BTreeMap::new();
    let mut made: Vec<(u64, u64, u64, u64)> = Vec::with_capacity(count as usize);
    let mut r = Restored { files: 0, dirs: 0, attrs: 0, bytes: 0 };
    for _ in 0..count {
        let h = d.head()?;
        let parent = match parent_of(&h.path) {
            "" => top,
            p => *ids.get(p).ok_or("parent")?,
        };
        let name = leaf(&h.path);
        let id = v
            .run(&mut |fs| -> Result<u64, FileSystemError> {
                let id = if h.kind == FileKind::Directory { fs.mkdir(parent, String::from(name))? } else { fs.create_file(parent, String::from(name))? };
                for (k, val) in &h.attrs {
                    fs.set_attribute(id, k.clone(), val.clone())?;
                }
                Ok(id)
            })?
            .map_err(fse)?;
        r.attrs += h.attrs.len() as u32;
        let mut off = 0u64;
        while off < h.data_len {
            let b = d.chunk(h.data_len - off, CHUNK)?;
            v.run(&mut |fs| V::put(fs, id, off, &b))??;
            off += b.len() as u64;
        }
        if h.kind == FileKind::Directory {
            r.dirs += 1;
            ids.insert(h.path, id);
        } else {
            r.files += 1;
            r.bytes += h.data_len;
            tick(r.files, sum.files);
        }
        made.push((id, h.ctime, h.mtime, h.atime));
    }
    // Times last, deepest first: a child's create edits its parent directory, which the clock would otherwise stamp.
    for group in made.rchunks(64) {
        v.run(&mut |fs| -> Result<(), FileSystemError> {
            for &(id, ct, mt, at) in group.iter().rev() {
                fs.set_times(id, ct, mt, at)?;
            }
            Ok(())
        })?
        .map_err(fse)?;
    }
    Ok(r)
}

// ---------------------------------------------------------------------------------------------------------------
// The Settings face's worker (x86): one job at a time, on a worker core, never on the render task
// ---------------------------------------------------------------------------------------------------------------

/// GEODEBACKUP M2: what Settings > Storage asks for, run on a one-shot `geode` task on a worker core (the dock's
/// HIDSTALL2 shape: `smp::xhci_worker_cpu`, not the hid pump's core). The press latches, the settings service pass
/// [`submit`]s, the painter reads [`progress`] (atomics) and [`model`] (`try_lock`). No worker core = refused.
#[cfg(target_arch = "x86_64")]
pub mod job {
    use super::{Bound, Src, Sink, Why};
    use crate::fs::vfs::{MountTable, NodeKind, KERNEL_PRINCIPAL as P};
    use alloc::format;
    use alloc::string::String;
    use alloc::vec::Vec;
    use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};

    /// What the task is asked to do. `home` is the session's home as the mount table names it.
    pub enum Req {
        /// Measure the home and list the volumes with room and the archives on the sticks.
        Survey { home: String },
        /// Back the home up to `/volumes/<vol>`.
        Backup { home: String, user: String, vol: String },
        /// Restore `/volumes/<vol>/<file>` into `<home>/Restored <stem>`.
        Restore { home: String, vol: String, file: String },
    }

    /// The phases the note row shows.
    pub const IDLE: u8 = 0;
    pub const MEASURING: u8 = 1;
    pub const BACKING_UP: u8 = 2;
    pub const RESTORING: u8 = 3;

    /// What the last survey and the last job left, for the pane.
    pub struct Model {
        /// `(volume, free bytes)`: the writable removable volumes with room for the home's archive.
        pub vols: Vec<(String, u64)>,
        /// `(volume, file)`: the `*.geode` files at the sticks' roots.
        pub archives: Vec<(String, String)>,
        /// The home's archive length, measured (0 = not measured).
        pub need: u64,
        /// The survey has run since the last ask.
        pub surveyed: bool,
        /// The last job's outcome, as the note row says it.
        pub said: String,
    }

    static BUSY: AtomicBool = AtomicBool::new(false);
    static PHASE: AtomicU8 = AtomicU8::new(IDLE);
    static DONE: AtomicU32 = AtomicU32::new(0);
    static TOTAL: AtomicU32 = AtomicU32::new(0);
    /// Moves when the model changes (the pane repaints on a move).
    static GEN: AtomicU32 = AtomicU32::new(0);
    static REQ: crate::sync::Mutex<Option<Req>> = crate::sync::Mutex::new(None);
    static MODEL: crate::sync::Mutex<Model> = crate::sync::Mutex::new(Model { vols: Vec::new(), archives: Vec::new(), need: 0, surveyed: false, said: String::new() });

    /// `(phase, done files, total files, generation)`.
    pub fn progress() -> (u8, u32, u32, u32) {
        (PHASE.load(Ordering::Acquire), DONE.load(Ordering::Relaxed), TOTAL.load(Ordering::Relaxed), GEN.load(Ordering::Acquire))
    }

    /// The model, without waiting (`None` while the task holds it).
    pub fn model() -> Option<crate::sync::MutexGuard<'static, Model>> {
        MODEL.try_lock()
    }

    /// Forget the survey (the pane's Cancel): the next ask measures again.
    pub fn stale() {
        if let Some(mut m) = MODEL.try_lock() {
            m.surveyed = false;
        }
    }

    /// Start `req` on a worker core. `Err`: a job is running, or no worker core apart from the hid pump's.
    pub fn submit(req: Req) -> Result<(), Why> {
        let Some(cpu) = crate::arch::smp::xhci_worker_cpu(0).filter(|&c| c != crate::hidstall::pump_cpu()) else { return Err("no-worker-core") };
        if BUSY.swap(true, Ordering::AcqRel) {
            return Err("busy");
        }
        *REQ.lock() = Some(req);
        crate::arch::sched::spawn("geode", run, 0, cpu, crate::arch::sched::PRIO_NORMAL);
        Ok(())
    }

    fn run(_: usize) {
        let req = REQ.lock().take();
        match req {
            Some(Req::Survey { home }) => survey(&home),
            Some(Req::Backup { home, user, vol }) => backup(&home, &user, &vol),
            Some(Req::Restore { home, vol, file }) => restore(&home, &vol, &file),
            None => {}
        }
        PHASE.store(IDLE, Ordering::Release);
        GEN.fetch_add(1, Ordering::AcqRel);
        BUSY.store(false, Ordering::Release);
    }

    fn said(s: String) {
        MODEL.lock().said = s;
    }

    /// The native volume's path for the mount-table `path`; `None` when the root volume does not claim it.
    fn native_of(mt: &MountTable, path: &str) -> Option<String> {
        let (b, rel) = mt.resolve(path).ok()?;
        if b.volume_name() != "native" {
            return None;
        }
        let v = b.on_volume(rel);
        Some(if v.is_empty() { String::from("/") } else if v.starts_with('/') { v } else { format!("/{}", v) })
    }

    /// The bytes of every file under `dir` (the `df` verb's used measure), bounded in depth.
    fn used(mt: &MountTable, dir: &str, depth: u32) -> Result<u64, Why> {
        if depth > 32 {
            return Err("depth");
        }
        let mut t = 0u64;
        for e in mt.read_dir(dir).map_err(|_| "unreadable")? {
            let p = format!("{}/{}", dir.trim_end_matches('/'), e.name);
            t += if e.kind == NodeKind::Dir { used(mt, &p, depth + 1)? } else { e.size };
        }
        Ok(t)
    }

    /// A removable volume's free bytes (capacity minus its files' bytes); `Err` for a read-only one.
    fn free_of(mt: &MountTable, vol: &str) -> Result<u64, Why> {
        let root = format!("/volumes/{}", vol);
        if mt.write_veto(&root).map_err(|_| "gone")?.is_some() {
            return Err("read-only");
        }
        let total = mt.resolve(&root).ok().and_then(|(b, _)| b.volume_bytes()).ok_or("no-size")?;
        Ok(total.saturating_sub(used(mt, &root, 0)?))
    }

    /// The archive's file extension (a visible file on the stick, not a dotfile).
    const EXT: &str = "geode";

    fn is_geode(name: &str) -> bool {
        name.len() > 6 && name.as_bytes()[name.len() - 6..].eq_ignore_ascii_case(b".geode")
    }

    fn survey(home: &str) {
        PHASE.store(MEASURING, Ordering::Release);
        let mt = crate::shell::vfs_mount_table();
        let need = native_of(&mt, home).and_then(|n| super::plan(&mut Bound, &n).ok()).map(|p| p.archive_bytes).unwrap_or(0);
        let mut vols = Vec::new();
        let mut archives = Vec::new();
        for v in crate::fs::removable::mounted_names() {
            if let Ok(free) = free_of(&mt, &v) {
                if need != 0 && free >= need {
                    vols.push((v.clone(), free));
                }
            }
            if let Ok(rows) = mt.read_dir(&format!("/volumes/{}", v)) {
                for e in rows {
                    if e.kind == NodeKind::File && is_geode(&e.name) {
                        archives.push((v.clone(), e.name));
                    }
                }
            }
        }
        serial_println!("[backup] survey need={} vols={} archives={}", need, vols.len(), archives.len());
        let mut m = MODEL.lock();
        m.need = need;
        m.vols = vols;
        m.archives = archives;
        m.surveyed = true;
    }

    /// `<user>-home-YYYYMMDD-HHMMSS.geode` from the wall clock (uptime ms when it was never set).
    fn archive_name(user: &str) -> String {
        let Some(t) = crate::clock::unix_now() else { return format!("{}-home-{}.{}", user, crate::arch::ms(), EXT) };
        let (days, s) = ((t / 86_400) as i64, t % 86_400);
        // Civil date from days since 1970-01-01 (H. Hinnant's algorithm).
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = yoe + era * 400 + (m <= 2) as i64;
        format!("{}-home-{:04}{:02}{:02}-{:02}{:02}{:02}.{}", user, y, m, d, s / 3600, (s / 60) % 60, s % 60, EXT)
    }

    /// A file on a mounted volume, written in [`super::CHUNK`]-sized appends.
    struct VfsSink<'a> {
        mt: &'a MountTable,
        path: String,
        off: u64,
        buf: Vec<u8>,
    }

    impl VfsSink<'_> {
        fn flush(&mut self) -> Result<(), Why> {
            if self.buf.is_empty() {
                return Ok(());
            }
            let n = self.mt.write(&self.path, self.off, &self.buf, P).map_err(|_| "stick-write")?;
            if n != self.buf.len() {
                return Err("stick-full");
            }
            self.off += n as u64;
            self.buf.clear();
            Ok(())
        }
    }

    impl Sink for VfsSink<'_> {
        fn put(&mut self, b: &[u8]) -> Result<(), Why> {
            self.buf.extend_from_slice(b);
            if self.buf.len() >= super::CHUNK {
                self.flush()?;
            }
            Ok(())
        }
    }

    /// A file on a mounted volume, read at offsets.
    struct VfsSrc<'a> {
        mt: &'a MountTable,
        path: String,
        size: u64,
    }

    impl Src for VfsSrc<'_> {
        fn len(&self) -> u64 {
            self.size
        }
        fn read_at(&mut self, off: u64, n: usize) -> Result<Vec<u8>, Why> {
            self.mt.read(&self.path, off, n).map_err(|_| "stick-read")
        }
    }

    fn backup(home: &str, user: &str, vol: &str) {
        let t0 = crate::arch::ms();
        let mt = crate::shell::vfs_mount_table();
        let mut files = 0u32;
        let mut bytes = 0u64;
        let r = (|| -> Result<(), Why> {
            let native = native_of(&mt, home).ok_or("not-unafs")?;
            PHASE.store(MEASURING, Ordering::Release);
            let plan = super::plan(&mut Bound, &native)?;
            files = plan.files;
            if free_of(&mt, vol)? < plan.archive_bytes {
                return Err("no-room");
            }
            let path = format!("/volumes/{}/{}", vol, archive_name(user));
            mt.create(&path, NodeKind::File, P).map_err(|_| "create")?;
            DONE.store(0, Ordering::Relaxed);
            TOTAL.store(plan.files, Ordering::Relaxed);
            PHASE.store(BACKING_UP, Ordering::Release);
            let mut sink = VfsSink { mt: &mt, path: path.clone(), off: 0, buf: Vec::new() };
            let w = super::write(&mut Bound, &plan, &mut sink, &mut |d, _| DONE.store(d, Ordering::Relaxed)).and_then(|n| sink.flush().map(|_| n));
            match w {
                Ok(n) => {
                    bytes = n;
                    Ok(())
                }
                Err(e) => {
                    let _ = mt.unlink(&path, P); // a partial archive is never left behind
                    Err(e)
                }
            }
        })();
        let ms = crate::arch::ms().saturating_sub(t0);
        match r {
            Ok(()) => {
                serial_println!("[backup] to={} files={} bytes={} ms={} -> ok", vol, files, bytes, ms);
                said(format!("Backed up {} files to {} ({} KB). Previous versions stay on this disk.", files, vol, bytes / 1024));
            }
            Err(w) => {
                serial_println!("[backup] to={} files={} bytes={} ms={} -> refused:{}", vol, files, bytes, ms, w);
                said(format!("The backup to {} did not complete: {}", vol, w));
            }
        }
    }

    fn restore(home: &str, vol: &str, file: &str) {
        let t0 = crate::arch::ms();
        let mt = crate::shell::vfs_mount_table();
        let stem = &file[..file.len() - 6];
        let into = format!("{}/Restored {}", home.trim_end_matches('/'), stem);
        let mut done = super::Restored { files: 0, dirs: 0, attrs: 0, bytes: 0 };
        let r = (|| -> Result<(), Why> {
            let native = native_of(&mt, &into).ok_or("not-unafs")?;
            let path = format!("/volumes/{}/{}", vol, file);
            let size = mt.stat(&path).map_err(|_| "gone")?.size;
            let mut src = VfsSrc { mt: &mt, path, size };
            DONE.store(0, Ordering::Relaxed);
            TOTAL.store(0, Ordering::Relaxed);
            PHASE.store(RESTORING, Ordering::Release);
            done = super::restore(&mut Bound, &mut src, &native, true, &mut |d, t| {
                DONE.store(d, Ordering::Relaxed);
                TOTAL.store(t, Ordering::Relaxed);
            })?;
            Ok(())
        })();
        let ms = crate::arch::ms().saturating_sub(t0);
        match r {
            Ok(()) => {
                serial_println!("[backup] restore from={}/{} into={} files={} bytes={} ms={} -> ok", vol, file, into, done.files, done.bytes, ms);
                said(format!("Restored {} files into {}", done.files, into));
            }
            Err(w) => {
                let _ = crate::fs::unafs::diskfull::after_hold();
                serial_println!("[backup] restore from={}/{} into={} files={} bytes={} ms={} -> refused:{}", vol, file, into, done.files, done.bytes, ms, w);
                said(format!("{} was not restored: {}", file, w));
            }
        }
    }
}
