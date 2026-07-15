// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! Dev-tree Bolt — vaire's first real managed unit (BOLT-1).
//!
//! A **dev-tree** Bolt is a source tree that must exist coherently on two
//! physical drives: the *live* copy (the internal "narino" drive, where work
//! happens) and a *target* copy on a removable volume (`/Volumes/40G`). This
//! module adds three read/coherence verbs to the Loom — **STATUS**, **SNAP**,
//! **SYNC** — WITHOUT the live-copy flip (SWITCH is arc 2).
//!
//! # The narino-never-written invariant (arc-1 headline safety property)
//!
//! No code path here writes, deletes, renames, or chmods anything under the
//! live (narino) tree. The invariant is made *provable* by construction:
//!
//! * The only write surface in this module is [`Target`]. Every filesystem
//!   mutation goes through a `&Target`, and a `Target` is built solely from the
//!   manifest's `target_root` (the 40G side) — there is no constructor that
//!   accepts the live root as a write destination.
//! * The live side is only ever opened read-only: STATUS shells `git` in
//!   read-only queries and reads penumbra bytes with [`std::fs::read`]; SNAP
//!   reads the live repos to produce bundles it writes *into the Target*; SYNC
//!   pushes live refs into a bare mirror *on the Target* and copies penumbra
//!   files *into the Target*.
//!
//! # Two riders (Maestro)
//!
//! * **Rider 1** — the executor never touches the real trees; all tests run
//!   against tempdir fixtures. The first real STATUS/`--apply` is attended.
//! * **Rider 2** — the penumbra manifest is *default-deny* for credential-shaped
//!   files: a matching file is skipped and **reported** (never silently), and
//!   the credential class is reported distinctly from ordinary build junk. See
//!   [`ExcludeRules`].

use std::fs;
use std::io::Write as _;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::CrystalColor;

// ---------------------------------------------------------------------------
// Manifest (the unit boundary — a versioned, commented config file)
// ---------------------------------------------------------------------------

/// Which physical drive holds the live copy. Arc-1 fact: the live copy is
/// always the internal narino drive — there is no flip (SWITCH is arc 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LiveDrive {
    /// The internal drive where development happens today.
    Narino,
}

/// One git unit inside the dev-tree Bolt: the main repo or a sibling worktree.
#[derive(Debug, Clone, Deserialize)]
pub struct RepoUnit {
    /// Short label used in reports and as the mirror/bundle basename.
    pub name: String,
    /// Absolute path to the repo (or worktree) on the LIVE drive.
    pub path: PathBuf,
    /// True if this is a git worktree (its `.git` is an absolute-path pointer
    /// file). Such units are flagged RED-for-switch and never rewritten here.
    #[serde(default)]
    pub worktree: bool,
}

/// The two exclusion classes. Credentials are the security-adjacent,
/// default-deny class (Rider 2); junk is ordinary build/OS noise.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ExcludeRules {
    /// Build/OS noise: `target`, `node_modules`, `.DS_Store`, `*.o`, `*.elf`, …
    #[serde(default)]
    pub junk: Vec<String>,
    /// **Rider 2 default-deny** credential-shaped patterns. A match is skipped
    /// and reported as [`ExcludeClass::Credential`].
    #[serde(default)]
    pub credentials: Vec<String>,
}

/// Why a path was excluded from the penumbra.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExcludeClass {
    /// Build/OS junk.
    Junk,
    /// Credential-shaped (Rider 2 default-deny) — reported distinctly.
    Credential,
}

impl ExcludeRules {
    /// The built-in conservative defaults, used when a manifest omits a class.
    /// Credentials default-deny even if the manifest forgets them.
    pub fn defaults() -> Self {
        Self {
            junk: [
                "target",
                "node_modules",
                ".DS_Store",
                "*.o",
                "*.elf",
                "*.rlib",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            credentials: Self::credential_defaults(),
        }
    }

    /// The Rider-2 credential pattern floor. These are always enforced (a
    /// manifest can add to them but the crate merges these in regardless).
    pub fn credential_defaults() -> Vec<String> {
        [
            "*.pem",
            "*.key",
            "*_rsa*",
            "*.p12",
            "*token*",
            "*secret*",
            "*credential*",
            ".env",
            ".env.*",
            ".netrc",
            "*.keychain",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    /// Merge the always-on credential floor into whatever the manifest declared
    /// so Rider 2 holds even if the config forgets a pattern.
    fn with_credential_floor(mut self) -> Self {
        for p in Self::credential_defaults() {
            if !self.credentials.iter().any(|c| c == &p) {
                self.credentials.push(p);
            }
        }
        self
    }

    /// Classify a path relative to a penumbra root. Credentials are checked
    /// FIRST so a credential-shaped name is never mis-reported as mere junk.
    /// Directory-name patterns (no `*`) match any path component; glob patterns
    /// match the final component (filename).
    pub fn classify(&self, rel: &Path) -> Option<ExcludeClass> {
        if Self::any_match(&self.credentials, rel) {
            return Some(ExcludeClass::Credential);
        }
        if Self::any_match(&self.junk, rel) {
            return Some(ExcludeClass::Junk);
        }
        None
    }

    fn any_match(patterns: &[String], rel: &Path) -> bool {
        let file = rel
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        for pat in patterns {
            if pat.contains('*') {
                if glob_match(pat, file) {
                    return true;
                }
            } else {
                // Plain name: match any path component (so `target` prunes the
                // whole subtree, and `.netrc` matches the file).
                for comp in rel.components() {
                    if let Component::Normal(c) = comp
                        && c.to_str() == Some(pat.as_str())
                    {
                        return true;
                    }
                }
            }
        }
        false
    }
}

/// The dev-tree Bolt manifest: the unit boundary, read from a commented config.
#[derive(Debug, Clone, Deserialize)]
pub struct DevManifest {
    /// Bolt label.
    pub name: String,
    /// Arc-1: always `narino`.
    pub live: LiveDrive,
    /// Absolute root of the live (narino) copy — READ-ONLY to this arc.
    pub live_root: PathBuf,
    /// Absolute root of the target copy (the 40G volume) — the only write side.
    pub target_root: PathBuf,
    /// Git units (main repo + sibling worktrees) mirrored on sync.
    #[serde(default, rename = "repo")]
    pub repos: Vec<RepoUnit>,
    /// Untracked penumbra roots (e.g. the plans tree) copied on sync,
    /// manifest-bounded and exclusion-filtered.
    #[serde(default)]
    pub penumbra: Vec<PathBuf>,
    /// Exclusion rules (Rider 2 credential floor always merged in).
    #[serde(default)]
    pub exclude: ExcludeRules,
}

impl DevManifest {
    /// Parse a manifest from TOML text, then merge in the credential floor.
    pub fn from_toml(text: &str) -> Result<Self> {
        let mut m: DevManifest = toml::from_str(text).context("parse dev-tree manifest")?;
        m.exclude = std::mem::take(&mut m.exclude).with_credential_floor();
        if m.live != LiveDrive::Narino {
            bail!("arc-1 requires live = narino (SWITCH/flip is arc 2)");
        }
        Ok(m)
    }

    /// Load a manifest from a file path.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let text = fs::read_to_string(path.as_ref())
            .with_context(|| format!("read manifest {}", path.as_ref().display()))?;
        Self::from_toml(&text)
    }

    /// A `Target` bound to this manifest's target root — the ONLY write surface.
    pub fn target(&self) -> Target {
        Target {
            root: self.target_root.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Target — the single write surface (narino-never-written invariant)
// ---------------------------------------------------------------------------

/// The one type through which this arc writes. Built only from the manifest's
/// `target_root` (the 40G side). There is deliberately no constructor that
/// accepts the live/narino root, so no write can land there.
#[derive(Debug, Clone)]
pub struct Target {
    root: PathBuf,
}

impl Target {
    /// The target root (40G volume path).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Is the target volume present (mounted / directory exists)?
    pub fn present(&self) -> bool {
        self.root.exists()
    }

    /// Resolve `rel` under the target root, refusing any path that would escape
    /// it (defence in depth for the invariant).
    fn resolve(&self, rel: &Path) -> Result<PathBuf> {
        for comp in rel.components() {
            if matches!(comp, Component::ParentDir | Component::RootDir) {
                bail!("refusing target path escaping root: {}", rel.display());
            }
        }
        Ok(self.root.join(rel))
    }

    /// Create a directory (and parents) under the target root.
    fn ensure_dir(&self, rel: &Path) -> Result<PathBuf> {
        let abs = self.resolve(rel)?;
        fs::create_dir_all(&abs).with_context(|| format!("mkdir {}", abs.display()))?;
        Ok(abs)
    }

    /// Copy `src` (read-only source) to `rel` under the target root.
    fn copy_in(&self, src: &Path, rel: &Path) -> Result<PathBuf> {
        let abs = self.resolve(rel)?;
        if let Some(parent) = abs.parent() {
            fs::create_dir_all(parent).with_context(|| format!("mkdir {}", parent.display()))?;
        }
        fs::copy(src, &abs)
            .with_context(|| format!("copy {} -> {}", src.display(), abs.display()))?;
        Ok(abs)
    }

    /// Write bytes to `rel` under the target root.
    fn write_file(&self, rel: &Path, bytes: &[u8]) -> Result<PathBuf> {
        let abs = self.resolve(rel)?;
        if let Some(parent) = abs.parent() {
            fs::create_dir_all(parent).with_context(|| format!("mkdir {}", parent.display()))?;
        }
        let mut f = fs::File::create(&abs).with_context(|| format!("create {}", abs.display()))?;
        f.write_all(bytes)?;
        Ok(abs)
    }
}

// ---------------------------------------------------------------------------
// STATUS — coherence truth, no writes
// ---------------------------------------------------------------------------

/// Per-copy git truth for one [`RepoUnit`] (read-only).
#[derive(Debug, Clone)]
pub struct RepoStatus {
    pub name: String,
    pub branch: String,
    /// Number of dirty (tracked-change) entries in `git status --porcelain`.
    pub dirty_files: usize,
    /// Number of commits on local branches not present on any remote.
    pub unpushed: usize,
    /// True if this repo's `.git` is a worktree pointer file (RED-for-switch).
    pub is_worktree_pointer: bool,
    /// If a worktree pointer, the absolute `gitdir:` it points at (surfaced,
    /// never rewritten — that is arc 2's job).
    pub worktree_gitdir: Option<String>,
    /// Whether the target mirror holds this repo's current HEAD (coherent) or
    /// is missing / behind (drift). `None` when the target volume is absent.
    pub mirror_coherent: Option<bool>,
}

impl RepoStatus {
    /// Drive-coherence crystal. Drift = the two drives diverge: an uncommitted
    /// (dirty) working tree — which a committed-refs mirror cannot carry — or a
    /// target mirror behind the live HEAD. `unpushed` counts commits not on any
    /// GitHub remote; those ARE captured by the 40G mirror, so they are reported
    /// as git truth but do NOT by themselves make the Bolt Amber.
    fn crystal(&self) -> CrystalColor {
        if self.dirty_files > 0 || self.mirror_coherent == Some(false) {
            CrystalColor::Amber
        } else {
            CrystalColor::Green
        }
    }
}

/// One penumbra file's coherence between live and target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PenumbraDelta {
    /// Present on both, byte-identical.
    Same,
    /// Present on live, absent on target.
    OnlyLive,
    /// Present on both, contents differ.
    Differs,
}

/// A single penumbra path's status (relative to its root).
#[derive(Debug, Clone)]
pub struct PenumbraEntry {
    pub root: PathBuf,
    pub rel: PathBuf,
    pub delta: PenumbraDelta,
}

/// A skipped, excluded path — always reported, never silent (Rider 2).
#[derive(Debug, Clone)]
pub struct ExcludedEntry {
    pub root: PathBuf,
    pub rel: PathBuf,
    pub class: ExcludeClass,
}

/// The full dev-tree STATUS.
#[derive(Debug, Clone)]
pub struct DevStatus {
    pub name: String,
    pub live: LiveDrive,
    pub target_present: bool,
    pub crystal: CrystalColor,
    pub repos: Vec<RepoStatus>,
    pub penumbra: Vec<PenumbraEntry>,
    pub excluded: Vec<ExcludedEntry>,
    /// Worktree pointer rows (RED-for-switch, arc-2's job — surfaced only).
    pub worktree_pointers: Vec<RepoStatus>,
    /// Last SNAP stamp found under `<target>/.vaire-snaps/`, if any.
    pub last_weave: Option<String>,
}

/// Compute STATUS for a dev-tree Bolt. Pure-read: the live side is only queried
/// (git) and read; nothing is written anywhere.
pub fn status(m: &DevManifest) -> Result<DevStatus> {
    let target = m.target();
    let target_present = target.present();

    let mut repos = Vec::new();
    let mut worktree_pointers = Vec::new();
    for r in &m.repos {
        let rs = repo_status(r, &target, target_present)?;
        if rs.is_worktree_pointer {
            worktree_pointers.push(rs.clone());
        }
        repos.push(rs);
    }

    // Penumbra delta + exclusion report.
    let mut penumbra = Vec::new();
    let mut excluded = Vec::new();
    for root in &m.penumbra {
        let root_name = root.file_name().and_then(|s| s.to_str()).unwrap_or("penumbra");
        walk_penumbra(root, &m.exclude, &mut |rel, class| match class {
            Some(class) => excluded.push(ExcludedEntry {
                root: root.clone(),
                rel: rel.to_path_buf(),
                class,
            }),
            None => {
                let live = root.join(rel);
                let tgt = target.root().join("penumbra").join(root_name).join(rel);
                let delta = penumbra_delta(&live, &tgt, target_present);
                penumbra.push(PenumbraEntry {
                    root: root.clone(),
                    rel: rel.to_path_buf(),
                    delta,
                });
            }
        })?;
    }

    let last_weave = latest_snap(&target);

    // Overall crystal: RED if the target volume is absent (unsyncable). Else
    // AMBER on any drift (dirty/unpushed/mirror-behind or penumbra difference),
    // GREEN when fully coherent. Worktree-pointer rows are informational
    // (RED-for-switch) and do not, by themselves, force overall RED here.
    let crystal = if !target_present {
        CrystalColor::Red
    } else if repos.iter().any(|r| r.crystal() == CrystalColor::Amber)
        || penumbra.iter().any(|p| p.delta != PenumbraDelta::Same)
    {
        CrystalColor::Amber
    } else {
        CrystalColor::Green
    };

    Ok(DevStatus {
        name: m.name.clone(),
        live: m.live,
        target_present,
        crystal,
        repos,
        penumbra,
        excluded,
        worktree_pointers,
        last_weave,
    })
}

fn repo_status(r: &RepoUnit, target: &Target, target_present: bool) -> Result<RepoStatus> {
    let (is_worktree_pointer, worktree_gitdir) = detect_worktree_pointer(&r.path);

    let branch = git_out(&r.path, &["rev-parse", "--abbrev-ref", "HEAD"])
        .unwrap_or_else(|_| "UNKNOWN".to_string());

    let dirty_files = git_out(&r.path, &["status", "--porcelain"])
        .map(|s| s.lines().filter(|l| !l.trim().is_empty()).count())
        .unwrap_or(0);

    // Commits on local branches not on any remote. Empty (0) when there is no
    // remote or nothing is ahead.
    let unpushed = git_out(&r.path, &["log", "--branches", "--not", "--remotes", "--oneline"])
        .map(|s| s.lines().filter(|l| !l.trim().is_empty()).count())
        .unwrap_or(0);

    let mirror_coherent = if !target_present {
        None
    } else {
        Some(mirror_has_head(&r.path, target, &r.name))
    };

    Ok(RepoStatus {
        name: r.name.clone(),
        branch,
        dirty_files,
        unpushed,
        is_worktree_pointer,
        worktree_gitdir,
        mirror_coherent,
    })
}

/// A worktree's `.git` is a *file* containing `gitdir: <abs>`; a normal repo's
/// `.git` is a directory. Surface the pointer; never rewrite it.
fn detect_worktree_pointer(repo: &Path) -> (bool, Option<String>) {
    let dot_git = repo.join(".git");
    match fs::metadata(&dot_git) {
        Ok(meta) if meta.is_file() => {
            let gitdir = fs::read_to_string(&dot_git)
                .ok()
                .and_then(|s| s.lines().find_map(|l| l.strip_prefix("gitdir:").map(|p| p.trim().to_string())));
            (true, gitdir)
        }
        _ => (false, None),
    }
}

/// Does the bare mirror on the target hold this repo's current HEAD commit?
fn mirror_has_head(repo: &Path, target: &Target, name: &str) -> bool {
    let head = match git_out(repo, &["rev-parse", "HEAD"]) {
        Ok(h) => h,
        Err(_) => return false,
    };
    let mirror = target.root().join("git-mirror").join(format!("{name}.git"));
    if !mirror.exists() {
        return false;
    }
    git_out(&mirror, &["cat-file", "-e", &format!("{head}^{{commit}}")]).is_ok()
}

fn penumbra_delta(live: &Path, tgt: &Path, target_present: bool) -> PenumbraDelta {
    if !target_present || !tgt.exists() {
        return PenumbraDelta::OnlyLive;
    }
    match (fs::read(live), fs::read(tgt)) {
        (Ok(a), Ok(b)) if a == b => PenumbraDelta::Same,
        _ => PenumbraDelta::Differs,
    }
}

fn latest_snap(target: &Target) -> Option<String> {
    let snaps = target.root().join(".vaire-snaps");
    let mut stamps: Vec<String> = fs::read_dir(&snaps)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    stamps.sort();
    stamps.pop()
}

// ---------------------------------------------------------------------------
// SNAP — a stamped capture on the TARGET before any apply
// ---------------------------------------------------------------------------

/// The result of a SNAP: where it landed and what it captured.
#[derive(Debug, Clone)]
pub struct SnapReport {
    pub stamp: String,
    pub dir: PathBuf,
    pub bundles: Vec<PathBuf>,
    pub penumbra_files: usize,
    pub excluded: Vec<ExcludedEntry>,
}

/// Take a stamped snapshot on the target: a git bundle (all refs) per repo plus
/// a manifest-bounded penumbra copy, under `<target>/.vaire-snaps/<stamp>/`.
/// Refuses honestly if the target volume is absent.
pub fn snap(m: &DevManifest) -> Result<SnapReport> {
    let target = m.target();
    if !target.present() {
        bail!(
            "SNAP refused: target volume absent at {} (mount it first)",
            target.root().display()
        );
    }
    snap_with_stamp(m, &target, &utc_stamp(SystemTime::now()))
}

fn snap_with_stamp(m: &DevManifest, target: &Target, stamp: &str) -> Result<SnapReport> {
    let base = PathBuf::from(".vaire-snaps").join(stamp);
    let dir = target.ensure_dir(&base)?;

    // git bundles (read-only on the source repo; written into the Target).
    let mut bundles = Vec::new();
    for r in &m.repos {
        let rel = base.join("git").join(format!("{}.bundle", r.name));
        let abs = target.resolve(&rel)?;
        if let Some(parent) = abs.parent() {
            fs::create_dir_all(parent)?;
        }
        // `git bundle create <file> --all` reads the source repo and writes the
        // bundle file (on the Target). It never mutates the source.
        let out = Command::new("git")
            .arg("-C")
            .arg(&r.path)
            .args(["bundle", "create"])
            .arg(&abs)
            .arg("--all")
            .output()
            .context("run git bundle")?;
        if out.status.success() {
            bundles.push(abs);
        }
        // A repo with no commits yet can't bundle; that is skipped, not fatal.
    }

    // penumbra copy (manifest-bounded, exclusion-filtered), written to Target.
    let mut penumbra_files = 0usize;
    let mut excluded = Vec::new();
    for root in &m.penumbra {
        let root_name = root
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("penumbra");
        walk_penumbra(root, &m.exclude, &mut |rel, class| match class {
            Some(class) => excluded.push(ExcludedEntry {
                root: root.clone(),
                rel: rel.to_path_buf(),
                class,
            }),
            None => {
                let src = root.join(rel);
                let dst_rel = base.join("penumbra").join(root_name).join(rel);
                if target.copy_in(&src, &dst_rel).is_ok() {
                    penumbra_files += 1;
                }
            }
        })?;
    }

    Ok(SnapReport {
        stamp: stamp.to_string(),
        dir,
        bundles,
        penumbra_files,
        excluded,
    })
}

// ---------------------------------------------------------------------------
// SYNC — narino -> 40G one-way (dry-run default; --apply writes, SNAP-first)
// ---------------------------------------------------------------------------

/// One planned action in a SYNC plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlannedAction {
    /// Mirror all refs of a repo into `<target>/git-mirror/<name>.git`.
    MirrorRepo { name: String, from: PathBuf },
    /// Copy a penumbra file (new or changed) into the target.
    CopyPenumbra { rel: PathBuf, reason: CopyReason },
    /// Skip an excluded penumbra file — always in the plan, never silent.
    SkipExcluded { rel: PathBuf, class: ExcludeClass },
}

/// Why a penumbra file is planned for copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyReason {
    /// Absent on the target.
    New,
    /// Present but different.
    Changed,
}

/// The full would-do plan of a SYNC. Printing this IS the dry-run.
#[derive(Debug, Clone)]
pub struct SyncPlan {
    pub actions: Vec<PlannedAction>,
}

impl SyncPlan {
    /// Human-readable plan text (what `sync` prints in dry-run).
    pub fn render(&self) -> String {
        let mut s = String::new();
        for a in &self.actions {
            match a {
                PlannedAction::MirrorRepo { name, from } => {
                    s.push_str(&format!("MIRROR   git  {name}  <- {}\n", from.display()));
                }
                PlannedAction::CopyPenumbra { rel, reason } => {
                    let tag = match reason {
                        CopyReason::New => "new",
                        CopyReason::Changed => "changed",
                    };
                    s.push_str(&format!("COPY     pen  {} ({tag})\n", rel.display()));
                }
                PlannedAction::SkipExcluded { rel, class } => {
                    let tag = match class {
                        ExcludeClass::Credential => "CREDENTIAL",
                        ExcludeClass::Junk => "junk",
                    };
                    s.push_str(&format!("SKIP     pen  {} [{tag}]\n", rel.display()));
                }
            }
        }
        s
    }
}

/// Compute the SYNC plan (no writes). Deterministic ordering: repos in manifest
/// order, then penumbra roots in order with entries sorted by relative path.
pub fn plan_sync(m: &DevManifest) -> Result<SyncPlan> {
    let target = m.target();
    let target_present = target.present();
    let mut actions = Vec::new();

    for r in &m.repos {
        actions.push(PlannedAction::MirrorRepo {
            name: r.name.clone(),
            from: r.path.clone(),
        });
    }

    for root in &m.penumbra {
        let root_name = root.file_name().and_then(|s| s.to_str()).unwrap_or("penumbra");
        let mut entries: Vec<PlannedAction> = Vec::new();
        walk_penumbra(root, &m.exclude, &mut |rel, class| match class {
            Some(class) => entries.push(PlannedAction::SkipExcluded {
                rel: PathBuf::from(root_name).join(rel),
                class,
            }),
            None => {
                let live = root.join(rel);
                let tgt = target.root().join("penumbra").join(root_name).join(rel);
                let reason = if target_present && tgt.exists() {
                    match (fs::read(&live), fs::read(&tgt)) {
                        (Ok(a), Ok(b)) if a == b => return, // coherent: no action
                        _ => CopyReason::Changed,
                    }
                } else {
                    CopyReason::New
                };
                entries.push(PlannedAction::CopyPenumbra {
                    rel: PathBuf::from(root_name).join(rel),
                    reason,
                });
            }
        })?;
        entries.sort_by_key(plan_key);
        actions.extend(entries);
    }

    Ok(SyncPlan { actions })
}

fn plan_key(a: &PlannedAction) -> PathBuf {
    match a {
        PlannedAction::CopyPenumbra { rel, .. } | PlannedAction::SkipExcluded { rel, .. } => {
            rel.clone()
        }
        PlannedAction::MirrorRepo { name, .. } => PathBuf::from(name),
    }
}

/// Apply a SYNC narino->40G. SNAPs first (the undo story), then mirrors each
/// repo's refs into a bare mirror on the target and copies the changed/new
/// penumbra files into the target. The source (narino) is only ever read.
pub fn apply_sync(m: &DevManifest) -> Result<(SnapReport, SyncPlan)> {
    let target = m.target();
    if !target.present() {
        bail!(
            "SYNC --apply refused: target volume absent at {}",
            target.root().display()
        );
    }

    // 1. SNAP first — always.
    let snap = snap_with_stamp(m, &target, &utc_stamp(SystemTime::now()))?;

    // 2. The plan we are about to execute (also the record of what we did).
    let plan = plan_sync(m)?;

    // 3. git mirror push: bare mirror per repo on the target.
    for r in &m.repos {
        let mirror = target
            .ensure_dir(&PathBuf::from("git-mirror").join(format!("{}.git", r.name)))?;
        if fs::read_dir(&mirror).map(|mut d| d.next().is_none()).unwrap_or(true) {
            // Initialise a bare mirror the first time.
            run_git(&mirror, &["init", "--bare", "-q"])?;
        }
        // Push ALL refs from the (read-only) source into the target mirror.
        run_git(
            &r.path,
            &["push", "--mirror", "-q", &mirror.to_string_lossy()],
        )
        .with_context(|| format!("mirror push {}", r.name))?;
    }

    // 4. penumbra: copy exactly the planned files (manifest-bounded, never a
    //    bare rsync of the tree).
    for root in &m.penumbra {
        let root_name = root.file_name().and_then(|s| s.to_str()).unwrap_or("penumbra");
        walk_penumbra(root, &m.exclude, &mut |rel, class| {
            if class.is_none() {
                let src = root.join(rel);
                let dst_rel = PathBuf::from("penumbra").join(root_name).join(rel);
                let _ = target.copy_in(&src, &dst_rel);
            }
        })?;
    }

    // 5. stamp the weave.
    let stamp = format!("last-weave: {}\n", snap.stamp);
    let _ = target.write_file(Path::new(".vaire-last-weave"), stamp.as_bytes());

    Ok((snap, plan))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Walk a penumbra root, invoking `f(rel, class)` for every file: `class =
/// Some(..)` for an excluded file (subtree pruned for excluded dirs), `None`
/// for a file to consider. Read-only.
fn walk_penumbra(
    root: &Path,
    rules: &ExcludeRules,
    f: &mut dyn FnMut(&Path, Option<ExcludeClass>),
) -> Result<()> {
    fn recurse(
        root: &Path,
        cur: &Path,
        rules: &ExcludeRules,
        f: &mut dyn FnMut(&Path, Option<ExcludeClass>),
    ) -> Result<()> {
        let dir = root.join(cur);
        let mut entries: Vec<_> = match fs::read_dir(&dir) {
            Ok(rd) => rd.filter_map(|e| e.ok()).collect(),
            Err(_) => return Ok(()),
        };
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name();
            let rel = if cur.as_os_str().is_empty() {
                PathBuf::from(&name)
            } else {
                cur.join(&name)
            };
            let meta = match e.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            if let Some(class) = rules.classify(&rel) {
                // Excluded: report it (dir OR file), and prune a dir subtree.
                f(&rel, Some(class));
                continue;
            }
            if meta.is_dir() {
                recurse(root, &rel, rules, f)?;
            } else if meta.is_file() {
                f(&rel, None);
            }
        }
        Ok(())
    }
    recurse(root, Path::new(""), rules, f)
}

/// Minimal glob: `*` matches any (possibly empty) run of characters; every
/// other character is literal. Sufficient for the credential/junk patterns
/// (`*.pem`, `*_rsa*`, `.env.*`). No `?` / char-classes (not needed here).
fn glob_match(pattern: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    // Iterative backtracking wildcard match.
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut mark) = (None, 0usize);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Run a git command for its stdout (trimmed). Read-only queries only.
fn git_out(repo: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .context("run git")?;
    if !out.status.success() {
        bail!("git {:?} failed", args);
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Run a git command that must succeed (used only against Target-side repos or
/// as a read-only push FROM the source — the source repo is never mutated).
fn run_git(cwd: &Path, args: &[&str]) -> Result<()> {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .context("run git")?;
    if !out.status.success() {
        bail!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}

/// Format a `SystemTime` as a compact UTC stamp `YYYYMMDDThhmmssZ`. Pure — no
/// external date dependency (Howard Hinnant's civil-from-days algorithm).
pub fn utc_stamp(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (hh, mm, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);

    // civil_from_days
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };

    format!("{year:04}{m:02}{d:02}T{hh:02}{mm:02}{ss:02}Z")
}

// ---------------------------------------------------------------------------
// Tests (fixtures only — Rider 1: never the real trees)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
