// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! UnaFS-native sync — vaire's *real* Loom (VAIRE-2).
//!
//! Bolt-1's [`crate::devtree`] `sync` is the host-side quick solution: git
//! mirrors + penumbra file copies onto another host volume. This module is the
//! destination the direction ruling names (RECONCILIATION-2026-07, the vaire
//! ruling): the managed dev-tree is woven into a **UnaFS v3 image as NATIVE
//! objects** — dirs are `mkdir`, files are `create_file` + `write_data`, host
//! metadata rides as K6 typed attributes, and every completed sync ends in a
//! retained-root **snapshot** (the SNAP concept, native). The local source of
//! truth becomes the UnaFS database on the K-line CoW machinery, not a pile of
//! host-fs copies.
//!
//! # Two invariants carried verbatim from Bolt-1
//!
//! * **The live tree is READ-ONLY.** The only write surface is the UnaFS image
//!   handle; no constructor here accepts a live-tree write path. The penumbra
//!   is read with [`std::fs::read`] and its metadata queried — never mutated.
//! * **The exclusion floor is the same default-deny.** `usync` walks the penumbra
//!   through [`crate::devtree::walk_penumbra`], so the manifest's credential
//!   default-deny floor, junk pruning, and reported-never-followed symlinks all
//!   apply identically; every skip is counted and reported, never silent.
//!
//! # Measured from birth (the benchmark corollary)
//!
//! The corollary to the vaire ruling: a real dev-tree sync — hundreds of
//! mixed-size files + typed metadata + journaled (CoW) writes in ONE measured
//! run — is the first-class UnaFS baseline benchmark, and the per-phase costs
//! are the evidence base for the crate-side batched-sync work. So `usync` is
//! instrumented with the NSSPAN pattern (host-ported): per-phase `AtomicU64`
//! counters folded by an RAII [`PhaseProbe`] over [`std::time::Instant`], plus
//! UnaFS's own [`CommitStats`]. Its summary block IS the benchmark ledger line,
//! and each run's summary is persisted as a typed attr on the unit root so runs
//! accrue in the image itself.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use unafs::{AttributeValue, BatchFile, CommitStats, FileDevice, FileSystem, SNAPSHOT_CAP};

use crate::devtree::{self, DevManifest, ExcludedEntry};

/// Default image size when a fresh volume must be formatted. The dev-tree
/// penumbra is a few MB of source/markdown; 256 MB leaves generous headroom
/// for many accreting snapshots (each retained root shares blocks with the
/// live tree until they diverge) without inviting a `NoSpace` mid-benchmark.
pub const DEFAULT_IMAGE_MB: u64 = 256;

/// The creator principal recorded on every `usync` snapshot (K8b typed attr).
const CREATOR: &str = "vaire";

// ---------------------------------------------------------------------------
// Phase instrumentation (NSSPAN pattern, host-ported)
// ---------------------------------------------------------------------------

/// Per-phase elapsed-nanosecond accumulators. The kernel's `NsSpanProbe`
/// (syscall.rs) folds CNTPCT ticks into per-site `AtomicU64`s with `fetch_max`;
/// the host twin folds `Instant` deltas with `fetch_add`, because a benchmark
/// wants the TOTAL time each phase spent, not its single worst call.
#[derive(Debug, Default)]
pub struct PhaseLedger {
    /// Walking the live penumbra tree (dir reads, metadata, classification).
    pub scan: AtomicU64,
    /// IMAGE-side lookups: the per-file incremental check (`ls`/attr reads
    /// against the stored object) plus directory-chain resolution
    /// (`ensure_dir` — which on a cold run also folds in the fresh `mkdir`s
    /// it performs on first touch of each directory). On a warm run this is
    /// almost pure read-path lookup cost — exactly the batched-sync evidence.
    pub lookup: AtomicU64,
    /// Host file reads ([`std::fs::read`] of each woven file).
    pub read: AtomicU64,
    /// `write_data` into the image (the CoW data path).
    pub write: AtomicU64,
    /// `set_attribute` (K6 typed attrs on files + the unit root).
    pub attrs: AtomicU64,
    /// Explicit `commit` (the root flip — the transaction's atomic point).
    pub commit: AtomicU64,
    /// `snapshot_create` (retain the current root; incref its whole tree).
    pub snapshot: AtomicU64,
}

impl PhaseLedger {
    /// A running phase probe: it folds its lifetime's elapsed nanos into `site`
    /// on drop. Bracket exactly the call being measured with a `{ let _p = … }`.
    fn probe<'a>(&'a self, site: &'a AtomicU64) -> PhaseProbe<'a> {
        PhaseProbe {
            start: Instant::now(),
            site,
        }
    }

    fn ns(site: &AtomicU64) -> u64 {
        site.load(Ordering::Relaxed)
    }
}

/// RAII probe (the host twin of the kernel `NsSpanProbe`). No heap, no lock.
pub struct PhaseProbe<'a> {
    start: Instant,
    site: &'a AtomicU64,
}

impl Drop for PhaseProbe<'_> {
    fn drop(&mut self) {
        let delta = self.start.elapsed().as_nanos() as u64;
        self.site.fetch_add(delta, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------------
// Report types
// ---------------------------------------------------------------------------

/// One file's sync disposition (the row vocabulary, like Bolt-1's plan rows).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileDisposition {
    /// Absent in the image — created + written.
    Written,
    /// Present with matching size+mtime attrs — skipped (incremental).
    Skipped,
}

/// A per-file row in the sync report.
#[derive(Debug, Clone)]
pub struct FileRow {
    /// Path relative to the image root (includes the unit-root dir name).
    pub rel: PathBuf,
    pub bytes: u64,
    pub disposition: FileDisposition,
}

/// The full result of a `usync` run (dry-run or apply).
#[derive(Debug)]
pub struct SyncReport {
    pub image: PathBuf,
    pub applied: bool,
    /// True when the image did not exist and was freshly formatted.
    pub formatted: bool,
    pub rows: Vec<FileRow>,
    pub excluded: Vec<ExcludedEntry>,
    pub dirs_created: usize,
    pub files_written: usize,
    pub files_skipped: usize,
    pub bytes_written: u64,
    /// `(name, generation)` of the snapshot retained at the end of an apply.
    pub snapshot: Option<(String, u64)>,
    pub run_stamp: String,
    pub ledger: PhaseLedger,
    pub commit_stats: CommitStats,
}

impl SyncReport {
    /// The one-line benchmark ledger persisted per run (and printed by the CLI).
    pub fn ledger_line(&self) -> String {
        let l = &self.ledger;
        format!(
            "written={} skipped={} excluded={} dirs={} bytes={} | \
             scan={:.3}ms lookup={:.3}ms read={:.3}ms write={:.3}ms attrs={:.3}ms commit={:.3}ms snapshot={:.3}ms | \
             commits={} blocks_written={} snapshots_created={}",
            self.files_written,
            self.files_skipped,
            self.excluded.len(),
            self.dirs_created,
            self.bytes_written,
            ms(PhaseLedger::ns(&l.scan)),
            ms(PhaseLedger::ns(&l.lookup)),
            ms(PhaseLedger::ns(&l.read)),
            ms(PhaseLedger::ns(&l.write)),
            ms(PhaseLedger::ns(&l.attrs)),
            ms(PhaseLedger::ns(&l.commit)),
            ms(PhaseLedger::ns(&l.snapshot)),
            self.commit_stats.commits,
            self.commit_stats.blocks_written,
            self.commit_stats.snapshots_created,
        )
    }
}

fn ms(nanos: u64) -> f64 {
    nanos as f64 / 1_000_000.0
}

/// Read-only status of a synced image (the `ustatus` verb).
#[derive(Debug)]
pub struct UStatus {
    pub image: PathBuf,
    /// Unit-root dir name -> its K6 typed attrs (manifest name, git head, run).
    pub units: Vec<UnitInfo>,
    /// The on-disk snapshot index (retained roots).
    pub snapshots: Vec<unafs::SnapshotEntry>,
    /// Objects (files+dirs) counted live in the image vs the live penumbra.
    pub image_objects: usize,
    pub live_files: usize,
}

/// One synced unit root's headline attrs + object/byte tallies.
#[derive(Debug, Clone)]
pub struct UnitInfo {
    pub name: String,
    pub manifest: Option<String>,
    pub git_head: Option<String>,
    pub last_run: Option<String>,
    pub files: usize,
    pub bytes: u64,
}

// ---------------------------------------------------------------------------
// Scan — collect the penumbra work set (read-only, live tree never written)
// ---------------------------------------------------------------------------

/// One file to consider, captured during the scan. `rel` is relative to the
/// image root and carries the unit-root dir name as its first component.
struct FileItem {
    abs: PathBuf,
    rel: PathBuf,
    size: u64,
    mtime: u64,
}

/// Walk every penumbra root, collecting files, their ancestor dirs, and the
/// excluded skips — all under the `scan` phase probe. Read-only.
fn scan(m: &DevManifest, ledger: &PhaseLedger) -> Result<(Vec<FileItem>, Vec<ExcludedEntry>)> {
    let _p = ledger.probe(&ledger.scan);
    let mut files = Vec::new();
    let mut excluded = Vec::new();
    for root in &m.penumbra {
        let root_name = root
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("penumbra")
            .to_string();
        devtree::walk_penumbra(root, &m.exclude, &mut |rel, class| match class {
            Some(class) => excluded.push(ExcludedEntry {
                root: root.clone(),
                rel: rel.to_path_buf(),
                class,
            }),
            None => {
                let abs = root.join(rel);
                let (size, mtime) = match std::fs::metadata(&abs) {
                    Ok(meta) => (meta.len(), mtime_secs(&meta)),
                    Err(_) => (0, 0),
                };
                files.push(FileItem {
                    abs,
                    rel: Path::new(&root_name).join(rel),
                    size,
                    mtime,
                });
            }
        })?;
    }
    // Deterministic order: by image-relative path.
    files.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok((files, excluded))
}

fn mtime_secs(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// usync — the native sync verb (dry-run default; apply writes + snapshots)
// ---------------------------------------------------------------------------

/// Sync the manifest-bounded penumbra into `image` as native UnaFS objects.
///
/// `apply == false` (the DEFAULT) is a dry-run: it reports exactly what a real
/// run would write vs skip (mounting the image read-only to read prior attrs
/// when it exists) and touches nothing. `apply == true` formats the image if
/// absent, weaves the tree, stamps typed attrs, and retains a snapshot.
pub fn usync(m: &DevManifest, image: &Path, apply: bool, size_mb: u64) -> Result<SyncReport> {
    usync_with_root_attrs(m, image, apply, size_mb, &BTreeMap::new())
}

/// [`usync`], plus a set of String attrs stamped on the IMAGE ROOT inode before
/// the run's single flip, so they ride the SAME retained snapshot the sync
/// takes. This is the write half of the repo-Bolt head anchor
/// ([`crate::repo::weave_into_image`]): the ledger head `(count, hash)` lands in
/// a CoW-retained root, where the ledger-file writer cannot silently rewrite it.
/// On a dry-run (`apply == false`) the attrs are ignored — a dry-run writes
/// nothing, by construction.
pub fn usync_with_root_attrs(
    m: &DevManifest,
    image: &Path,
    apply: bool,
    size_mb: u64,
    root_attrs: &BTreeMap<String, String>,
) -> Result<SyncReport> {
    let ledger = PhaseLedger::default();
    let run_stamp = devtree::utc_stamp(SystemTime::now());
    let (items, excluded) = scan(m, &ledger)?;

    if !apply {
        return dry_run(image, items, excluded, ledger, run_stamp);
    }

    // --- APPLY: format-if-absent, then mount read-write. ---
    let formatted = !image.exists();
    if formatted {
        if let Some(parent) = image.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create image dir {}", parent.display()))?;
        }
        let file = std::fs::File::create(image)
            .with_context(|| format!("create image {}", image.display()))?;
        file.set_len(size_mb * 1024 * 1024)
            .context("size image file")?;
        let device = FileDevice::open(image).context("open fresh image device")?;
        FileSystem::format(device, size_mb).map_err(|e| anyhow::anyhow!("format image: {e:?}"))?;
    }

    let device = FileDevice::open(image).context("open image device")?;
    let mut fs = FileSystem::mount(device).map_err(|e| anyhow::anyhow!("mount image: {e:?}"))?;

    // Honor the snapshot cap UP FRONT, before any write: a completed sync must
    // end in a retained root, so if the index is already full the whole run is
    // refused as a true no-op on the image (nothing written, nothing committed,
    // no half-synced tree stamped without its snapshot). Never auto-drops.
    let index = fs
        .snapshot_index()
        .map_err(|e| anyhow::anyhow!("read snapshot index: {e:?}"))?;
    if index.len() >= SNAPSHOT_CAP {
        bail!(
            "snapshot index full ({}/{} retained roots): drop one first with \
             `vaire snapdrop`/`unafs snapdrop` — usync never auto-drops a retained root \
             (nothing was written)",
            index.len(),
            SNAPSHOT_CAP
        );
    }

    // The one-flip regime (VAIRE-3 — UNAFS-BATCH adoption). Every op is CoW,
    // but autocommit is turned OFF once for the whole run: the ancestor `mkdir`s
    // and each directory's WRITE set stage into ONE transaction, landed by a
    // single `commit()` below (the unit-root-attr commit IS that one flip).
    // The per-directory write set goes through `create_files_batch` — one
    // folded inode write per file (its four `vaire.*` attrs ride the creation
    // inode) and ONE parent-directory + catalog rewrite for the whole group,
    // instead of the former per-file create_file/write_data/4×set_attribute/
    // commit. This collapses the commit phase (formerly 97 % of the cold wall
    // across one root flip per file) to the single outer flip; the `write`
    // phase now covers stale-object unlinks plus the batch staging, and file
    // attrs no longer appear under the `attrs` phase (they fold into the batch).
    // Skipped/written/excluded row meanings are unchanged; the incremental skip
    // and stale-object unlink paths are untouched. snapshot_create still commits
    // unconditionally regardless of this flag.
    fs.set_autocommit(false);

    let root_id = fs.root_inode();
    let mut dir_cache: BTreeMap<PathBuf, u64> = BTreeMap::new();
    dir_cache.insert(PathBuf::new(), root_id);
    let mut dirs_created = 0usize;
    let mut rows = Vec::new();
    let mut files_written = 0usize;
    let mut files_skipped = 0usize;
    let mut bytes_written = 0u64;

    // The WRITE set, grouped by parent-dir inode: one `create_files_batch` per
    // group. A BTreeMap keeps the staging order deterministic.
    let mut batches: BTreeMap<u64, Vec<BatchFile>> = BTreeMap::new();

    for item in &items {
        let parent_rel = item.rel.parent().unwrap_or(Path::new(""));
        let parent_id = {
            let _p = ledger.probe(&ledger.lookup);
            ensure_dir(&mut fs, &mut dir_cache, parent_rel, &mut dirs_created)?
        };
        let name = file_name_str(&item.rel)?;

        // Incremental: an existing object with matching size+mtime attrs is
        // skipped and counted (never silently). The ls/attr reads against the
        // stored object are the `lookup` phase — the warm run's dominant cost.
        let existing = {
            let _p = ledger.probe(&ledger.lookup);
            match find_child(&mut fs, parent_id, name)? {
                Some(id) if attrs_match(&mut fs, id, item)? => {
                    files_skipped += 1;
                    rows.push(FileRow {
                        rel: item.rel.clone(),
                        bytes: item.size,
                        disposition: FileDisposition::Skipped,
                    });
                    continue;
                }
                other => other,
            }
        };
        if existing.is_some() {
            // Changed: unlink the stale object so the batch name is free and the
            // rewrite is exact (the grow-only write_data cannot shrink a file in
            // place). The unlink stages into the same outer transaction.
            let _p = ledger.probe(&ledger.write);
            fs.unlink(parent_id, name)
                .map_err(|e| anyhow::anyhow!("unlink stale {}: {e:?}", item.rel.display()))?;
        }

        let data = {
            let _p = ledger.probe(&ledger.read);
            std::fs::read(&item.abs)
                .with_context(|| format!("read live file {}", item.abs.display()))?
        };
        // The four `vaire.*` K6 attrs ride the BatchFile map — exactly what the
        // per-op path set with four `set_attribute` calls, folded into the
        // file's single creation inode write.
        let mut attributes = BTreeMap::new();
        attributes.insert(
            "vaire.size".to_string(),
            AttributeValue::Int(item.size as i64),
        );
        attributes.insert(
            "vaire.mtime".to_string(),
            AttributeValue::Int(item.mtime as i64),
        );
        attributes.insert(
            "vaire.src".to_string(),
            AttributeValue::String(item.abs.to_string_lossy().to_string()),
        );
        attributes.insert(
            "vaire.sync".to_string(),
            AttributeValue::String(run_stamp.clone()),
        );
        batches.entry(parent_id).or_default().push(BatchFile {
            name: name.to_string(),
            data,
            attributes,
        });
        files_written += 1;
        bytes_written += item.size;
        rows.push(FileRow {
            rel: item.rel.clone(),
            bytes: item.size,
            disposition: FileDisposition::Written,
        });
    }

    // Land each directory's write set with one batch call — all staged into the
    // single outer transaction (autocommit off), committed by the flip below.
    {
        let _p = ledger.probe(&ledger.write);
        for (parent_id, files) in batches {
            fs.create_files_batch(parent_id, files).map_err(|e| {
                anyhow::anyhow!("create_files_batch under dir {parent_id}: {e:?}")
            })?;
        }
    }

    // Unit-root attrs + this run's summary (accrues per run in the image).
    // The ledger line itself is built AFTER the snapshot probe folds, so both
    // the persisted attr and the CLI block include the snapshot phase; the one
    // thing the persisted line cannot include is the commit that persists it
    // (inherent — the report's commit_stats ARE refreshed after that commit,
    // so the CLI figures cover every root flip of the run).
    let summary_key = format!("vaire.summary.{run_stamp}");
    for root in &m.penumbra {
        let root_name = root
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("penumbra");
        if let Some(&unit_id) = dir_cache.get(Path::new(root_name)) {
            let _p = ledger.probe(&ledger.attrs);
            set_str(&mut fs, unit_id, "vaire.unit", &m.name)?;
            set_str(&mut fs, unit_id, "vaire.githead", &git_head(root))?;
            set_str(&mut fs, unit_id, "vaire.run", &run_stamp)?;
        }
    }
    // Anchor attrs on the IMAGE ROOT — set BEFORE the flip and the snapshot
    // below, so they are captured by this run's retained CoW root (the
    // tamper-anchor the repo Bolt reads back via `read_retained_root_attr`).
    if !root_attrs.is_empty() {
        let _p = ledger.probe(&ledger.attrs);
        for (key, value) in root_attrs {
            set_str(&mut fs, root_id, key, value)?;
        }
    }
    // THE one flip: this single commit lands the whole staged tree — every
    // `mkdir`, every batched file+attrs, and these unit-root attrs — as one
    // atomic root. (A failed sync therefore never leaves a partial tree: any
    // error above unwinds the whole outer transaction to the last committed
    // root; nothing is committed and no snapshot is taken.)
    {
        let _p = ledger.probe(&ledger.commit);
        fs.commit()
            .map_err(|e| anyhow::anyhow!("commit sync transaction: {e:?}"))?;
    }

    // The retained root — one snapshot per completed sync (cap already
    // checked, up front, before any write).
    let generation = {
        let _p = ledger.probe(&ledger.snapshot);
        fs.snapshot_create(run_stamp.clone(), CREATOR.to_string(), now_secs())
            .map_err(|e| anyhow::anyhow!("snapshot_create: {e:?}"))?
    };

    // Persist this run's ledger line as a typed attr on each unit root.
    let commit_stats = fs.commit_stats();
    let mut report = SyncReport {
        image: image.to_path_buf(),
        applied: true,
        formatted,
        rows,
        excluded,
        dirs_created,
        files_written,
        files_skipped,
        bytes_written,
        snapshot: Some((run_stamp.clone(), generation)),
        run_stamp: run_stamp.clone(),
        ledger,
        commit_stats,
    };
    let line = report.ledger_line();
    for root in &m.penumbra {
        let root_name = root
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("penumbra");
        if let Some(&unit_id) = dir_cache.get(Path::new(root_name)) {
            let _p = report.ledger.probe(&report.ledger.attrs);
            set_str(&mut fs, unit_id, &summary_key, &line)?;
        }
    }
    {
        let _p = report.ledger.probe(&report.ledger.commit);
        fs.commit()
            .map_err(|e| anyhow::anyhow!("commit run summary: {e:?}"))?;
    }
    // Refresh so the CLI's figures cover the summary commit too (the persisted
    // attr line inherently predates it — see the comment above).
    report.commit_stats = fs.commit_stats();

    Ok(report)
}

/// Dry-run: compute written/skipped/excluded without any write. Mounts the
/// image read-only (when present) to read prior size+mtime attrs; a missing
/// image means every file is a would-write.
fn dry_run(
    image: &Path,
    items: Vec<FileItem>,
    excluded: Vec<ExcludedEntry>,
    ledger: PhaseLedger,
    run_stamp: String,
) -> Result<SyncReport> {
    // Ancestor dirs that a real run would create (deduped).
    let mut dirs: std::collections::BTreeSet<PathBuf> = std::collections::BTreeSet::new();
    for item in &items {
        let mut p = item.rel.parent();
        while let Some(dir) = p {
            if dir.as_os_str().is_empty() {
                break;
            }
            dirs.insert(dir.to_path_buf());
            p = dir.parent();
        }
    }

    let mut existing: Option<FileSystem> = if image.exists() {
        let device = FileDevice::open_read_only(image).context("open image read-only")?;
        Some(FileSystem::mount(device).map_err(|e| anyhow::anyhow!("mount image (ro): {e:?}"))?)
    } else {
        None
    };

    let mut rows = Vec::new();
    let mut files_written = 0usize;
    let mut files_skipped = 0usize;
    let mut bytes_written = 0u64;
    for item in &items {
        let skipped = match existing.as_mut() {
            Some(fs) => object_attrs_match(fs, &item.rel, item)?,
            None => false,
        };
        let disposition = if skipped {
            FileDisposition::Skipped
        } else {
            FileDisposition::Written
        };
        match disposition {
            FileDisposition::Skipped => files_skipped += 1,
            FileDisposition::Written => {
                files_written += 1;
                bytes_written += item.size;
            }
        }
        rows.push(FileRow {
            rel: item.rel.clone(),
            bytes: item.size,
            disposition,
        });
    }

    Ok(SyncReport {
        image: image.to_path_buf(),
        applied: false,
        formatted: false,
        rows,
        excluded,
        dirs_created: dirs.len(),
        files_written,
        files_skipped,
        bytes_written,
        snapshot: None,
        run_stamp,
        ledger,
        commit_stats: CommitStats::default(),
    })
}

// ---------------------------------------------------------------------------
// ustatus — read-only report of a synced image
// ---------------------------------------------------------------------------

/// Read-only status of a synced image: unit attrs, snapshot index, and object
/// counts vs the live tree. Opens the image read-only (bytes untouched).
pub fn ustatus(m: &DevManifest, image: &Path) -> Result<UStatus> {
    if !image.exists() {
        bail!("image absent: {}", image.display());
    }
    let device = FileDevice::open_read_only(image).context("open image read-only")?;
    let mut fs = FileSystem::mount(device).map_err(|e| anyhow::anyhow!("mount image (ro): {e:?}"))?;

    let snapshots = fs
        .snapshot_index()
        .map_err(|e| anyhow::anyhow!("read snapshot index: {e:?}"))?;

    let root_id = fs.root_inode();
    let mut units = Vec::new();
    let mut image_objects = 0usize;
    for root in &m.penumbra {
        let root_name = root
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("penumbra")
            .to_string();
        let unit_id = match child_id(&mut fs, root_id, &root_name)? {
            Some(id) => id,
            None => continue,
        };
        let (files, bytes, objects) = count_tree(&mut fs, unit_id)?;
        image_objects += objects;
        units.push(UnitInfo {
            name: root_name,
            manifest: get_str(&mut fs, unit_id, "vaire.unit"),
            git_head: get_str(&mut fs, unit_id, "vaire.githead"),
            last_run: get_str(&mut fs, unit_id, "vaire.run"),
            files,
            bytes,
        });
    }

    // Live-tree file count (for the vs-live comparison), excluding skips.
    let ledger = PhaseLedger::default();
    let (live_items, _) = scan(m, &ledger)?;

    Ok(UStatus {
        image: image.to_path_buf(),
        units,
        snapshots,
        image_objects,
        live_files: live_items.len(),
    })
}

/// Read one String attr of the IMAGE ROOT **as of the newest retained
/// snapshot** — the tamper-anchor read path.
///
/// The value is returned from a [`unafs::SnapshotView`], i.e. the root attr the
/// CoW machinery retained at snapshot time, not the mutable live root. Editing
/// it in place is not possible without rewriting the retained tree and breaking
/// its refcounts (which `git fsck`/`unafs fsck` would flag); this is what lets a
/// repo Bolt pin its ledger head where the ledger-file writer cannot reach.
///
/// Returns `Ok(None)` when the image is absent, holds no retained root, or the
/// newest root carries no such attr.
pub fn read_retained_root_attr(image: &Path, key: &str) -> Result<Option<String>> {
    if !image.exists() {
        return Ok(None);
    }
    let device = FileDevice::open_read_only(image).context("open image read-only")?;
    let mut fs =
        FileSystem::mount(device).map_err(|e| anyhow::anyhow!("mount image (ro): {e:?}"))?;
    let index = fs
        .snapshot_index()
        .map_err(|e| anyhow::anyhow!("read snapshot index: {e:?}"))?;
    let newest = match index.iter().map(|s| s.generation).max() {
        Some(g) => g,
        None => return Ok(None),
    };
    let root_id = fs.root_inode();
    let mut view = fs
        .open_snapshot(newest)
        .map_err(|e| anyhow::anyhow!("open snapshot {newest}: {e:?}"))?;
    match view.get_attribute(root_id, key) {
        Ok(Some(AttributeValue::String(v))) => Ok(Some(v)),
        Ok(_) => Ok(None),
        Err(e) => Err(anyhow::anyhow!("read snapshot root attr {key}: {e:?}")),
    }
}

/// Recursively count (files, total file bytes, all objects) under a dir inode.
fn count_tree(fs: &mut FileSystem, dir_id: u64) -> Result<(usize, u64, usize)> {
    let (mut files, mut bytes, mut objects) = (0usize, 0u64, 0usize);
    for e in fs
        .ls(dir_id)
        .map_err(|err| anyhow::anyhow!("ls {dir_id}: {err:?}"))?
    {
        objects += 1;
        match e.kind {
            unafs::FileKind::Directory => {
                let (f, b, o) = count_tree(fs, e.inode_id)?;
                files += f;
                bytes += b;
                objects += o;
            }
            _ => {
                files += 1;
                let inode = fs
                    .read_inode(e.inode_id)
                    .map_err(|err| anyhow::anyhow!("read_inode {}: {err:?}", e.inode_id))?;
                bytes += inode.size;
            }
        }
    }
    Ok((files, bytes, objects))
}

// ---------------------------------------------------------------------------
// Helpers over the UnaFS handle
// ---------------------------------------------------------------------------

/// Ensure the directory chain `rel` (image-root-relative) exists, caching inode
/// ids. Reuses an existing dir; `mkdir`s the rest, counting fresh creations.
fn ensure_dir(
    fs: &mut FileSystem,
    cache: &mut BTreeMap<PathBuf, u64>,
    rel: &Path,
    created: &mut usize,
) -> Result<u64> {
    if let Some(&id) = cache.get(rel) {
        return Ok(id);
    }
    let parent = rel.parent().unwrap_or(Path::new(""));
    let parent_id = ensure_dir(fs, cache, parent, created)?;
    let name = file_name_str(rel)?;
    let id = match find_child(fs, parent_id, name)? {
        Some(id) => id,
        None => {
            *created += 1;
            fs.mkdir(parent_id, name.to_string())
                .map_err(|e| anyhow::anyhow!("mkdir {}: {e:?}", rel.display()))?
        }
    };
    cache.insert(rel.to_path_buf(), id);
    Ok(id)
}

/// The inode id of a named child of `parent_id`, if present.
fn find_child(fs: &mut FileSystem, parent_id: u64, name: &str) -> Result<Option<u64>> {
    Ok(fs
        .ls(parent_id)
        .map_err(|e| anyhow::anyhow!("ls {parent_id}: {e:?}"))?
        .into_iter()
        .find(|e| e.name == name)
        .map(|e| e.inode_id))
}

fn child_id(fs: &mut FileSystem, parent_id: u64, name: &str) -> Result<Option<u64>> {
    find_child(fs, parent_id, name)
}

/// Does the object already stored at `inode_id` carry size+mtime attrs matching
/// the live file? (The incremental-skip predicate.)
fn attrs_match(fs: &mut FileSystem, inode_id: u64, item: &FileItem) -> Result<bool> {
    let size = get_int(fs, inode_id, "vaire.size");
    let mtime = get_int(fs, inode_id, "vaire.mtime");
    Ok(size == Some(item.size as i64) && mtime == Some(item.mtime as i64))
}

/// Resolve `rel` (image-root-relative) and test its stored attrs — used by the
/// read-only dry-run, which has no dir-id cache.
fn object_attrs_match(fs: &mut FileSystem, rel: &Path, item: &FileItem) -> Result<bool> {
    let mut id = fs.root_inode();
    for comp in rel.components() {
        let name = match comp {
            std::path::Component::Normal(s) => s.to_string_lossy().to_string(),
            _ => return Ok(false),
        };
        match find_child(fs, id, &name)? {
            Some(child) => id = child,
            None => return Ok(false),
        }
    }
    attrs_match(fs, id, item)
}

fn set_str(fs: &mut FileSystem, id: u64, key: &str, v: &str) -> Result<()> {
    fs.set_attribute(id, key.to_string(), AttributeValue::String(v.to_string()))
        .map_err(|e| anyhow::anyhow!("set_attribute {key}: {e:?}"))
}

fn get_int(fs: &mut FileSystem, id: u64, key: &str) -> Option<i64> {
    match fs.get_attribute(id, key) {
        Ok(Some(AttributeValue::Int(v))) => Some(v),
        _ => None,
    }
}

fn get_str(fs: &mut FileSystem, id: u64, key: &str) -> Option<String> {
    match fs.get_attribute(id, key) {
        Ok(Some(AttributeValue::String(v))) => Some(v),
        _ => None,
    }
}

fn file_name_str(rel: &Path) -> Result<&str> {
    rel.file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow::anyhow!("path has no final component: {}", rel.display()))
}

/// The git HEAD of the repo at (or above) `dir`, short-form, or `"none"`. Read
/// only (the live tree is never mutated).
fn git_head(dir: &Path) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--short", "HEAD"])
        .output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => "none".to_string(),
    }
}

/// The image-root inode id, via the mounted superblock. A small shim so the
/// engine never hardcodes the reserved root id.
trait RootInode {
    fn root_inode(&self) -> u64;
}

impl RootInode for FileSystem {
    fn root_inode(&self) -> u64 {
        self.superblock.root_inode
    }
}

#[cfg(test)]
mod tests;
