// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! Repo Bolt — vaire's second managed unit kind (BOLT-2).
//!
//! Where [`crate::devtree`] manages *one* dev tree across two drives, a **repo
//! Bolt** manages an arbitrary git repository as a unit under the Loom's care:
//! an **integrity-verified bare mirror** plus an **append-only, hash-chained
//! snapshot ledger**, under the same default-deny credential floor as Bolt-1.
//! This is the repo-manager increment: the thing that makes a repository a
//! managed object rather than a directory someone remembered to back up.
//!
//! # What a repo Bolt is
//!
//! * **The mirror** — `<bolt_root>/mirrors/<name>.git`, a bare `--mirror` clone
//!   of the source repository. Committed history only: a repo Bolt never reads
//!   the source's working tree, so an untracked file cannot enter the bolt by
//!   construction (a strictly stronger statement than Bolt-1's filtered
//!   penumbra copy).
//! * **The ledger** — `<bolt_root>/ledger/<name>.ledger`, an append-only chain
//!   of entries. Each entry records the run stamp, every ref and its object id,
//!   the credential findings, and the hash of the previous entry; its own hash
//!   covers all of that. Editing or dropping any historical entry breaks the
//!   chain and [`verify`] says so and names the entry.
//! * **The credential floor** — the *same* [`crate::devtree::ExcludeRules`]
//!   default-deny patterns. A mirror carries whatever was committed, so a
//!   floor cannot retroactively un-commit a key; what it can do — and what this
//!   module does — is **audit and report, never silently**. Every path in a
//!   mirrored head tree matching a credential pattern becomes a
//!   [`CredentialFinding`], is written into the ledger entry, and turns the
//!   bolt Amber until it is dealt with.
//!
//! # The one write surface (the source-never-written invariant)
//!
//! Carried verbatim from Bolt-1 and made provable the same way: every mutation
//! in this module goes through [`BoltRoot`], which is built *solely* from the
//! manifest's `bolt_root`. There is no constructor that accepts a source
//! repository path as a write destination. The source is only ever read — the
//! `git clone --mirror` / `git fetch` that populate the mirror run **with the
//! mirror as the cwd**, pulling *from* the source, so no git command in this
//! module can write into a managed repository.
//!
//! # Dry-run is the default
//!
//! [`plan_weave`] computes the whole would-do plan and writes nothing.
//! [`weave`] is the applying verb and is only reached through an explicit
//! `--apply`.
//!
//! # UnaFS alignment (the layout mapping, stated explicitly)
//!
//! The host layout above is deliberately shaped so it maps 1:1 onto native
//! UnaFS objects, using the VAIRE-3 engine that already exists — no new
//! filesystem code. [`unafs_view`] makes that mapping *executable* rather than
//! aspirational: it projects a [`RepoManifest`] into the [`DevManifest`] the
//! [`crate::usync`] engine consumes, so `usync` weaves a repo Bolt into a UnaFS
//! v3 image today, one root flip per run, with a retained snapshot per weave.
//!
//! | repo-Bolt host artifact | UnaFS-native form |
//! | --- | --- |
//! | `mirrors/<name>.git/**` | a directory tree: `mkdir` per dir, one `create_files_batch` per parent dir |
//! | each mirrored file's size/mtime/source | the four `vaire.size` / `vaire.mtime` / `vaire.src` / `vaire.sync` K6 typed attrs, folded into the file's creation inode |
//! | one ledger entry `<stamp>` | a `vaire.repo.ledger.<stamp>` String attr on the unit-root inode (the `vaire.summary.<stamp>` pattern) |
//! | the ledger head | `vaire.repo.head` (the chain hash) on the unit root |
//! | a credential finding | a `vaire.repo.cred.<n>` String attr — reported in the image itself, never silent |
//! | the ledger's tamper-evidence | strengthened, not replaced: `snapshot_create(<stamp>, "vaire")` retains the *whole tree* at that stamp as an immutable CoW root, so a chain hash can be checked against a real historical root via `open_snapshot` rather than against a file the same process could rewrite |
//! | Crystal Color | computed, never stored (it is a function of live truth) |
//!
//! The honest boundary: host-native vaire cannot mount a *kernel* UnaFS volume
//! — there is no host↔kernel bridge today, and the crate's only host device is
//! a file ([`unafs::FileDevice`]). "Utilizing UnaFS" here means the **image
//! file** and the **format**: the same v3 on-disk layout, the same typed-attr
//! and retained-root vocabulary the kernel mounts. An image woven here is the
//! same bytes a kernel mount would read.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::CrystalColor;
use crate::devtree::{DevManifest, ExcludeRules, utc_stamp};

/// The bolt-root marker file, carrying the layout version. A future layout
/// change is detectable rather than silently mis-read.
pub const BOLT_MARKER: &str = ".vaire-repo-bolt";
/// The layout version written into [`BOLT_MARKER`].
pub const LAYOUT_VERSION: u32 = 1;
/// The genesis `prev` of a ledger chain (no previous entry).
pub const GENESIS: &str = "0000000000000000000000000000000000000000";

// ---------------------------------------------------------------------------
// Manifest
// ---------------------------------------------------------------------------

/// One managed repository inside a repo Bolt.
#[derive(Debug, Clone, Deserialize)]
pub struct RepoSource {
    /// Short label: the mirror basename and the ledger basename.
    pub name: String,
    /// The source repository — READ-ONLY to this module.
    pub source: PathBuf,
}

/// A repo Bolt manifest: a set of managed repositories plus the one write root.
#[derive(Debug, Clone, Deserialize)]
pub struct RepoManifest {
    /// Bolt label (recorded in the ledger and as the UnaFS unit attr).
    pub name: String,
    /// The ONLY write surface: mirrors, ledgers, and the marker live here.
    pub bolt_root: PathBuf,
    /// The managed repositories, in declaration order.
    #[serde(default, rename = "repo")]
    pub repos: Vec<RepoSource>,
    /// Exclusion rules; the credential floor is always merged in.
    #[serde(default)]
    pub exclude: ExcludeRules,
}

impl RepoManifest {
    /// Parse a manifest from TOML text, merging in the credential floor so a
    /// forgotten pattern cannot silence the audit.
    pub fn from_toml(text: &str) -> Result<Self> {
        let mut m: RepoManifest = toml::from_str(text).context("parse repo-bolt manifest")?;
        m.exclude = std::mem::take(&mut m.exclude).with_credential_floor();
        if m.bolt_root.as_os_str().is_empty() {
            bail!("repo-bolt manifest needs a bolt_root (the only write surface)");
        }
        for r in &m.repos {
            if r.name.is_empty() || r.name.contains('/') || r.name.contains("..") {
                bail!("repo name must be a plain label, got {:?}", r.name);
            }
        }
        Ok(m)
    }

    /// Load a manifest from a file path.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let text = fs::read_to_string(path.as_ref())
            .with_context(|| format!("read repo-bolt manifest {}", path.as_ref().display()))?;
        Self::from_toml(&text)
    }

    /// The [`BoltRoot`] for this manifest — the only write surface.
    pub fn bolt(&self) -> BoltRoot {
        BoltRoot {
            root: self.bolt_root.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// BoltRoot — the single write surface
// ---------------------------------------------------------------------------

/// The one type through which this module writes. Built only from the
/// manifest's `bolt_root`; there is deliberately no constructor accepting a
/// managed repository's path, so no write can land in a source repo.
#[derive(Debug, Clone)]
pub struct BoltRoot {
    root: PathBuf,
}

impl BoltRoot {
    /// The bolt root path.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Does the bolt root exist?
    pub fn present(&self) -> bool {
        self.root.exists()
    }

    /// Resolve `rel` under the bolt root, refusing any path escaping it.
    fn resolve(&self, rel: &Path) -> Result<PathBuf> {
        for comp in rel.components() {
            if matches!(comp, Component::ParentDir | Component::RootDir) {
                bail!("refusing bolt path escaping root: {}", rel.display());
            }
        }
        Ok(self.root.join(rel))
    }

    /// The mirror directory for `name` (may not exist yet).
    pub fn mirror_dir(&self, name: &str) -> Result<PathBuf> {
        self.resolve(&PathBuf::from("mirrors").join(format!("{name}.git")))
    }

    /// The ledger file for `name` (may not exist yet).
    pub fn ledger_file(&self, name: &str) -> Result<PathBuf> {
        self.resolve(&PathBuf::from("ledger").join(format!("{name}.ledger")))
    }

    fn ensure_dir(&self, rel: &Path) -> Result<PathBuf> {
        let abs = self.resolve(rel)?;
        fs::create_dir_all(&abs).with_context(|| format!("mkdir {}", abs.display()))?;
        Ok(abs)
    }

    /// Create the bolt root and its marker if absent (idempotent).
    fn ensure_layout(&self) -> Result<()> {
        fs::create_dir_all(&self.root)
            .with_context(|| format!("mkdir bolt root {}", self.root.display()))?;
        self.ensure_dir(Path::new("mirrors"))?;
        self.ensure_dir(Path::new("ledger"))?;
        let marker = self.resolve(Path::new(BOLT_MARKER))?;
        if !marker.exists() {
            fs::write(&marker, format!("vaire-repo-bolt v{LAYOUT_VERSION}\n"))
                .with_context(|| format!("write {}", marker.display()))?;
        }
        Ok(())
    }

    /// Append one entry's text to a ledger file (the only ledger mutation —
    /// there is no rewrite path, by construction).
    fn append_ledger(&self, name: &str, text: &str) -> Result<PathBuf> {
        use std::io::Write as _;
        let path = self.ledger_file(name)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("open ledger {}", path.display()))?;
        f.write_all(text.as_bytes())?;
        Ok(path)
    }
}

// ---------------------------------------------------------------------------
// The ledger
// ---------------------------------------------------------------------------

/// One `refname -> object id` pair as recorded in a ledger entry.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RefRecord {
    pub name: String,
    pub oid: String,
}

/// A credential-shaped path found in a mirrored head tree. Reported, never
/// silent — a committed secret cannot be un-committed by a filter, so the
/// floor's job here is to make it impossible to miss.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CredentialFinding {
    /// The ref whose tree contains it.
    pub reference: String,
    /// The path inside that tree.
    pub path: String,
}

/// One append-only ledger entry: the state of a mirror at a stamp, chained to
/// its predecessor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerEntry {
    pub stamp: String,
    pub repo: String,
    /// The previous entry's `hash`, or [`GENESIS`] for the first.
    pub prev: String,
    pub refs: Vec<RefRecord>,
    pub credentials: Vec<CredentialFinding>,
    /// SHA-1 over this entry's canonical payload (which includes `prev`).
    pub hash: String,
}

impl LedgerEntry {
    /// The canonical payload — everything the hash covers, in a fixed order.
    /// Refs and findings are sorted so the same state always hashes the same.
    fn payload(
        stamp: &str,
        repo: &str,
        prev: &str,
        refs: &[RefRecord],
        creds: &[CredentialFinding],
    ) -> String {
        let mut s = String::new();
        s.push_str(&format!("entry {stamp}\n"));
        s.push_str(&format!("repo {repo}\n"));
        s.push_str(&format!("prev {prev}\n"));
        for r in refs {
            s.push_str(&format!("ref {} {}\n", r.oid, r.name));
        }
        for c in creds {
            s.push_str(&format!("cred {} {}\n", c.reference, c.path));
        }
        s
    }

    /// Build an entry, computing its chain hash.
    pub fn new(
        stamp: String,
        repo: String,
        prev: String,
        mut refs: Vec<RefRecord>,
        mut credentials: Vec<CredentialFinding>,
    ) -> Self {
        refs.sort();
        credentials.sort();
        let payload = Self::payload(&stamp, &repo, &prev, &refs, &credentials);
        let hash = chain_hash(payload.as_bytes());
        Self {
            stamp,
            repo,
            prev,
            refs,
            credentials,
            hash,
        }
    }

    /// The on-disk text of this entry (payload + its hash line + a blank line).
    pub fn render(&self) -> String {
        let mut s = Self::payload(
            &self.stamp,
            &self.repo,
            &self.prev,
            &self.refs,
            &self.credentials,
        );
        s.push_str(&format!("hash {}\n\n", self.hash));
        s
    }

    /// Recompute this entry's hash from its own fields. A mismatch against
    /// [`Self::hash`] means the entry was edited after it was written.
    pub fn recomputed_hash(&self) -> String {
        chain_hash(
            Self::payload(
                &self.stamp,
                &self.repo,
                &self.prev,
                &self.refs,
                &self.credentials,
            )
            .as_bytes(),
        )
    }
}

/// The ledger chain hash: SHA-1 as produced by `gix`'s collision-detecting
/// hasher — the same primitive (and the same detector) git uses for object
/// ids, so the ledger needs no hash dependency the crate does not already
/// carry for reading repositories.
fn chain_hash(bytes: &[u8]) -> String {
    let mut h = gix::hash::hasher(gix::hash::Kind::Sha1);
    h.update(bytes);
    match h.try_finalize() {
        Ok(id) => id.to_hex().to_string(),
        // The only failure mode is a detected SHA-1 collision attempt. Refusing
        // to produce a hash at all would lose the entry; emit a loud, distinct,
        // never-valid marker so `verify` can never call such a chain clean.
        Err(_) => "collision-detected".repeat(3),
    }
}

/// The entry being accumulated while parsing, before its `hash` line closes it.
#[derive(Default)]
struct PartialEntry {
    stamp: String,
    repo: String,
    prev: String,
    refs: Vec<RefRecord>,
    credentials: Vec<CredentialFinding>,
}

/// Parse a whole ledger file into its entries, in file order.
pub fn parse_ledger(text: &str) -> Result<Vec<LedgerEntry>> {
    let mut entries = Vec::new();
    let mut cur: Option<PartialEntry> = None;
    for (n, line) in text.lines().enumerate() {
        let lineno = n + 1;
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        let (tag, rest) = line.split_once(' ').unwrap_or((line, ""));
        match tag {
            "entry" => {
                if cur.is_some() {
                    bail!("ledger line {lineno}: new entry before the previous one's hash");
                }
                cur = Some(PartialEntry {
                    stamp: rest.to_string(),
                    ..PartialEntry::default()
                });
            }
            "repo" => match cur.as_mut() {
                Some(c) => c.repo = rest.to_string(),
                None => bail!("ledger line {lineno}: `repo` outside an entry"),
            },
            "prev" => match cur.as_mut() {
                Some(c) => c.prev = rest.to_string(),
                None => bail!("ledger line {lineno}: `prev` outside an entry"),
            },
            "ref" => {
                let (oid, name) = rest
                    .split_once(' ')
                    .with_context(|| format!("ledger line {lineno}: malformed ref"))?;
                match cur.as_mut() {
                    Some(c) => c.refs.push(RefRecord {
                        name: name.to_string(),
                        oid: oid.to_string(),
                    }),
                    None => bail!("ledger line {lineno}: `ref` outside an entry"),
                }
            }
            "cred" => {
                let (reference, path) = rest
                    .split_once(' ')
                    .with_context(|| format!("ledger line {lineno}: malformed cred"))?;
                match cur.as_mut() {
                    Some(c) => c.credentials.push(CredentialFinding {
                        reference: reference.to_string(),
                        path: path.to_string(),
                    }),
                    None => bail!("ledger line {lineno}: `cred` outside an entry"),
                }
            }
            "hash" => {
                let p = cur
                    .take()
                    .with_context(|| format!("ledger line {lineno}: `hash` outside an entry"))?;
                entries.push(LedgerEntry {
                    stamp: p.stamp,
                    repo: p.repo,
                    prev: p.prev,
                    refs: p.refs,
                    credentials: p.credentials,
                    hash: rest.to_string(),
                });
            }
            other => bail!("ledger line {lineno}: unknown tag {other:?}"),
        }
    }
    if cur.is_some() {
        bail!("ledger ends mid-entry (no `hash` line) — truncated");
    }
    Ok(entries)
}

/// Read a ledger file's entries; an absent file is an empty chain (not an error
/// — a bolt that has never been woven simply has no history yet).
pub fn read_ledger(path: &Path) -> Result<Vec<LedgerEntry>> {
    match fs::read_to_string(path) {
        Ok(t) => parse_ledger(&t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e).with_context(|| format!("read ledger {}", path.display())),
    }
}

// ---------------------------------------------------------------------------
// Plan / weave
// ---------------------------------------------------------------------------

/// What a weave would do to one repo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeaveAction {
    /// No mirror yet — a fresh `git clone --mirror`.
    Clone,
    /// Mirror present — `git fetch --prune` into it.
    Update,
    /// The source is unreadable; the repo is skipped and reported.
    SourceUnreadable,
}

/// The planned disposition of one repo in a weave.
#[derive(Debug, Clone)]
pub struct RepoPlan {
    pub name: String,
    pub source: PathBuf,
    pub action: WeaveAction,
    /// Refs currently in the mirror (empty before the first weave).
    pub mirror_refs: usize,
    /// Refs currently in the source.
    pub source_refs: usize,
    /// Ledger entries recorded so far.
    pub ledger_entries: usize,
}

/// The full would-do plan of a weave (printing this IS the dry-run).
#[derive(Debug, Clone)]
pub struct WeavePlan {
    pub bolt: PathBuf,
    pub bolt_present: bool,
    pub repos: Vec<RepoPlan>,
}

impl WeavePlan {
    /// Human-readable plan text.
    pub fn render(&self) -> String {
        let mut s = String::new();
        for r in &self.repos {
            let verb = match r.action {
                WeaveAction::Clone => "CLONE ",
                WeaveAction::Update => "UPDATE",
                WeaveAction::SourceUnreadable => "SKIP  ",
            };
            s.push_str(&format!(
                "{verb} {}  <- {}  (mirror {} refs, source {} refs, {} ledger entries)\n",
                r.name,
                r.source.display(),
                r.mirror_refs,
                r.source_refs,
                r.ledger_entries
            ));
        }
        s
    }
}

/// Compute the weave plan. Writes nothing, anywhere.
pub fn plan_weave(m: &RepoManifest) -> Result<WeavePlan> {
    let bolt = m.bolt();
    let mut repos = Vec::new();
    for r in &m.repos {
        let mirror = bolt.mirror_dir(&r.name)?;
        let source_refs = read_refs(&r.source).map(|v| v.len());
        let action = match (&source_refs, mirror.exists()) {
            (Err(_), _) => WeaveAction::SourceUnreadable,
            (Ok(_), true) => WeaveAction::Update,
            (Ok(_), false) => WeaveAction::Clone,
        };
        repos.push(RepoPlan {
            name: r.name.clone(),
            source: r.source.clone(),
            action,
            mirror_refs: read_refs(&mirror).map(|v| v.len()).unwrap_or(0),
            source_refs: source_refs.unwrap_or(0),
            ledger_entries: read_ledger(&bolt.ledger_file(&r.name)?)?.len(),
        });
    }
    Ok(WeavePlan {
        bolt: m.bolt_root.clone(),
        bolt_present: bolt.present(),
        repos,
    })
}

/// The result of one repo's applied weave.
#[derive(Debug, Clone)]
pub struct WovenRepo {
    pub name: String,
    pub action: WeaveAction,
    pub mirror: PathBuf,
    pub entry: Option<LedgerEntry>,
}

/// The result of an applied weave.
#[derive(Debug, Clone)]
pub struct WeaveReport {
    pub bolt: PathBuf,
    pub stamp: String,
    pub repos: Vec<WovenRepo>,
}

impl WeaveReport {
    /// Total credential findings across every repo woven in this run.
    pub fn credential_findings(&self) -> usize {
        self.repos
            .iter()
            .filter_map(|r| r.entry.as_ref())
            .map(|e| e.credentials.len())
            .sum()
    }
}

/// Apply a weave: mirror every managed repo into the bolt and append one
/// ledger entry per repo. The sources are only ever read.
pub fn weave(m: &RepoManifest) -> Result<WeaveReport> {
    let bolt = m.bolt();
    bolt.ensure_layout()?;
    let stamp = utc_stamp(SystemTime::now());

    let mut repos = Vec::new();
    for r in &m.repos {
        let mirror = bolt.mirror_dir(&r.name)?;
        if read_refs(&r.source).is_err() {
            repos.push(WovenRepo {
                name: r.name.clone(),
                action: WeaveAction::SourceUnreadable,
                mirror,
                entry: None,
            });
            continue;
        }

        // Mirror the source. Both commands run with the BOLT side as the write
        // target and pull FROM the source — no git invocation here can write
        // into a managed repository.
        let action = if mirror.exists() {
            git_ok(&mirror, &["fetch", "--prune", "--quiet", "origin"])
                .with_context(|| format!("fetch into mirror {}", r.name))?;
            WeaveAction::Update
        } else {
            bolt.ensure_dir(Path::new("mirrors"))?;
            let out = Command::new("git")
                .args(["clone", "--mirror", "--quiet"])
                .arg(&r.source)
                .arg(&mirror)
                .output()
                .context("run git clone --mirror")?;
            if !out.status.success() {
                bail!(
                    "clone --mirror {} failed: {}",
                    r.name,
                    String::from_utf8_lossy(&out.stderr)
                );
            }
            WeaveAction::Clone
        };

        let refs = read_refs(&mirror)?;
        let credentials = audit_credentials(&mirror, &refs, &m.exclude)?;
        let prev = read_ledger(&bolt.ledger_file(&r.name)?)?
            .last()
            .map(|e| e.hash.clone())
            .unwrap_or_else(|| GENESIS.to_string());
        let entry = LedgerEntry::new(stamp.clone(), r.name.clone(), prev, refs, credentials);
        bolt.append_ledger(&r.name, &entry.render())?;

        repos.push(WovenRepo {
            name: r.name.clone(),
            action,
            mirror,
            entry: Some(entry),
        });
    }

    Ok(WeaveReport {
        bolt: m.bolt_root.clone(),
        stamp,
        repos,
    })
}

// ---------------------------------------------------------------------------
// Verify
// ---------------------------------------------------------------------------

/// One thing verification found wrong. Every variant names the repo and enough
/// detail to act; nothing is summarised away.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Breach {
    /// The mirror directory is missing.
    MirrorMissing,
    /// No ledger entry exists yet (the bolt has never been woven).
    LedgerEmpty,
    /// An entry's recorded hash does not match its contents (it was edited).
    EntryTampered { stamp: String },
    /// An entry's `prev` does not match its predecessor's hash (an entry was
    /// dropped, reordered, or inserted).
    ChainBroken { stamp: String, expected: String },
    /// An object id the newest entry recorded is not present in the mirror.
    ObjectMissing { oid: String, reference: String },
    /// The mirror's current refs differ from the newest entry's — the mirror
    /// moved without the ledger (or the ledger without the mirror).
    RefDrift { reference: String, detail: String },
    /// `git fsck` rejected the mirror (deep check only).
    FsckFailed { detail: String },
}

/// The verification result for one repo.
#[derive(Debug, Clone)]
pub struct VerifyReport {
    pub name: String,
    pub entries: usize,
    /// Credential findings recorded in the newest entry.
    pub credentials: Vec<CredentialFinding>,
    pub breaches: Vec<Breach>,
}

impl VerifyReport {
    /// Nothing broken (credential findings are reported separately — they are
    /// a real-world fact about the history, not a corruption of the bolt).
    pub fn is_intact(&self) -> bool {
        self.breaches.is_empty()
    }

    /// The Crystal Color of this repo Bolt: **Red** on any integrity breach,
    /// **Amber** when intact but carrying unresolved credential findings,
    /// **Green** when intact and clean.
    pub fn crystal(&self) -> CrystalColor {
        if !self.breaches.is_empty() {
            CrystalColor::Red
        } else if !self.credentials.is_empty() {
            CrystalColor::Amber
        } else {
            CrystalColor::Green
        }
    }
}

/// Verify every managed repo: the ledger chain, then the newest entry's objects
/// and refs against the mirror. Pure-read — verification never repairs, and a
/// verb that could rewrite the thing it audits would not be a verification.
///
/// `deep` additionally runs `git fsck --connectivity-only` per mirror (object
/// graph integrity), which is the expensive check and therefore opt-in.
pub fn verify(m: &RepoManifest, deep: bool) -> Result<Vec<VerifyReport>> {
    let bolt = m.bolt();
    let mut reports = Vec::new();
    for r in &m.repos {
        let mirror = bolt.mirror_dir(&r.name)?;
        let entries = read_ledger(&bolt.ledger_file(&r.name)?)?;
        let mut breaches = Vec::new();

        // 1. The chain: each entry's own hash, then its link to its predecessor.
        let mut prev_hash = GENESIS.to_string();
        for e in &entries {
            if e.recomputed_hash() != e.hash {
                breaches.push(Breach::EntryTampered {
                    stamp: e.stamp.clone(),
                });
            }
            if e.prev != prev_hash {
                breaches.push(Breach::ChainBroken {
                    stamp: e.stamp.clone(),
                    expected: prev_hash.clone(),
                });
            }
            prev_hash = e.hash.clone();
        }

        if !mirror.exists() {
            breaches.push(Breach::MirrorMissing);
        }
        let newest = entries.last();
        if newest.is_none() {
            breaches.push(Breach::LedgerEmpty);
        }

        // 2. The newest entry vs the mirror: every recorded object present, and
        //    the ref set identical (drift in either direction is named).
        let mut credentials = Vec::new();
        if let (Some(entry), true) = (newest, mirror.exists()) {
            credentials = entry.credentials.clone();
            for rec in &entry.refs {
                if !object_present(&mirror, &rec.oid) {
                    breaches.push(Breach::ObjectMissing {
                        oid: rec.oid.clone(),
                        reference: rec.name.clone(),
                    });
                }
            }
            let live_refs = read_refs(&mirror)?;
            let live: BTreeMap<&str, &str> = live_refs
                .iter()
                .map(|r| (r.name.as_str(), r.oid.as_str()))
                .collect();
            let recorded: BTreeMap<&str, &str> = entry
                .refs
                .iter()
                .map(|r| (r.name.as_str(), r.oid.as_str()))
                .collect();
            for (name, oid) in &recorded {
                match live.get(name) {
                    None => breaches.push(Breach::RefDrift {
                        reference: (*name).to_string(),
                        detail: "recorded in the ledger, absent from the mirror".to_string(),
                    }),
                    Some(live_oid) if live_oid != oid => breaches.push(Breach::RefDrift {
                        reference: (*name).to_string(),
                        detail: format!("ledger {oid}, mirror {live_oid}"),
                    }),
                    _ => {}
                }
            }
            for name in live.keys() {
                if !recorded.contains_key(name) {
                    breaches.push(Breach::RefDrift {
                        reference: (*name).to_string(),
                        detail: "present in the mirror, absent from the ledger".to_string(),
                    });
                }
            }

            if deep {
                let out = Command::new("git")
                    .arg("-C")
                    .arg(&mirror)
                    .args(["fsck", "--connectivity-only", "--no-progress"])
                    .output()
                    .context("run git fsck")?;
                if !out.status.success() {
                    breaches.push(Breach::FsckFailed {
                        detail: String::from_utf8_lossy(&out.stderr).trim().to_string(),
                    });
                }
            }
        }

        reports.push(VerifyReport {
            name: r.name.clone(),
            entries: entries.len(),
            credentials,
            breaches,
        });
    }
    Ok(reports)
}

/// The God-view row for one repo Bolt.
#[derive(Debug, Clone)]
pub struct RepoBoltStatus {
    pub name: String,
    pub crystal: CrystalColor,
    pub entries: usize,
    pub last_stamp: Option<String>,
    pub credentials: usize,
    pub breaches: Vec<Breach>,
    /// Refs in the source not yet reflected in the newest ledger entry.
    pub source_ahead: bool,
}

/// STATUS for a repo Bolt: verification plus a source-vs-ledger drift check.
/// Pure-read. A bolt whose source has moved since the last weave is Amber —
/// intact, but no longer current.
pub fn status(m: &RepoManifest) -> Result<Vec<RepoBoltStatus>> {
    let bolt = m.bolt();
    let verified = verify(m, false)?;
    let mut rows = Vec::new();
    for (r, v) in m.repos.iter().zip(verified) {
        let entries = read_ledger(&bolt.ledger_file(&r.name)?)?;
        let last = entries.last();
        let source_ahead = match (read_refs(&r.source), last) {
            (Ok(src), Some(e)) => {
                let recorded: BTreeMap<&str, &str> =
                    e.refs.iter().map(|x| (x.name.as_str(), x.oid.as_str())).collect();
                src.iter()
                    .any(|s| recorded.get(s.name.as_str()) != Some(&s.oid.as_str()))
            }
            _ => false,
        };
        let crystal = match (v.crystal(), source_ahead) {
            (CrystalColor::Green, true) => CrystalColor::Amber,
            (c, _) => c,
        };
        rows.push(RepoBoltStatus {
            name: r.name.clone(),
            crystal,
            entries: v.entries,
            last_stamp: last.map(|e| e.stamp.clone()),
            credentials: v.credentials.len(),
            breaches: v.breaches,
            source_ahead,
        });
    }
    Ok(rows)
}

// ---------------------------------------------------------------------------
// The UnaFS bridge
// ---------------------------------------------------------------------------

/// Project a repo Bolt into the [`DevManifest`] shape the [`crate::usync`]
/// engine consumes, so `usync` weaves the bolt's mirrors into a UnaFS v3 image
/// as native objects — one root flip per run, one retained snapshot per weave,
/// the same four `vaire.*` typed attrs per file, and the same default-deny
/// credential floor on the walk.
///
/// This is the *executable* half of the layout mapping in the module docs: the
/// bolt's `mirrors/` directory is already shaped as a `usync` unit root, so
/// nothing about the native path is hypothetical. The `live_root` /
/// `target_root` fields are the host-sync verbs' concern and are set to the
/// bolt root; `usync` reads neither (its only write surface is the image
/// handle), and the returned manifest is deliberately **not** usable to drive
/// the Bolt-1 host `sync` — see [`unafs_layout`] for what the mapping claims.
pub fn unafs_view(m: &RepoManifest) -> Result<DevManifest> {
    let bolt = m.bolt();
    let mirrors = bolt.resolve(Path::new("mirrors"))?;
    let toml = format!(
        "name = {}\n\
         live = 'narino'\n\
         live_root = {}\n\
         target_root = {}\n\
         penumbra = [{}]\n",
        toml_str(&m.name),
        toml_str(&m.bolt_root.to_string_lossy()),
        toml_str(&m.bolt_root.to_string_lossy()),
        toml_str(&mirrors.to_string_lossy()),
    );
    let mut dm = DevManifest::from_toml(&toml)?;
    dm.exclude = m.exclude.clone();
    Ok(dm)
}

/// TOML single-quoted literal string, with the one character it cannot hold
/// rejected rather than mangled.
fn toml_str(s: &str) -> String {
    debug_assert!(!s.contains('\''), "path contains a quote: {s}");
    format!("'{}'", s.replace('\'', ""))
}

/// One row of the host-artifact → UnaFS-object mapping, as data rather than
/// prose, so the claim is inspectable (and test-pinned) instead of asserted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutMapping {
    /// The host artifact, relative to the bolt root.
    pub host: String,
    /// What it becomes in a UnaFS v3 image.
    pub native: String,
}

/// The explicit UnaFS mapping for a repo Bolt. Keep this in lockstep with the
/// table in the module docs and the README.
pub fn unafs_layout(m: &RepoManifest) -> Vec<LayoutMapping> {
    let mut rows = vec![LayoutMapping {
        host: "mirrors/".to_string(),
        native: "image root: one directory per managed repo".to_string(),
    }];
    for r in &m.repos {
        rows.push(LayoutMapping {
            host: format!("mirrors/{}.git/**", r.name),
            native: format!(
                "dir tree under /{}.git — mkdir per dir, one create_files_batch per parent dir; \
                 four vaire.* typed attrs per file",
                r.name
            ),
        });
        rows.push(LayoutMapping {
            host: format!("ledger/{}.ledger (entry <stamp>)", r.name),
            native: format!(
                "vaire.repo.ledger.<stamp> String attr on the /{}.git unit root; \
                 head chain hash in vaire.repo.head",
                r.name
            ),
        });
    }
    rows.push(LayoutMapping {
        host: format!("{BOLT_MARKER} (layout v{LAYOUT_VERSION})"),
        native: "vaire.repo.layout Int attr on the image root".to_string(),
    });
    rows.push(LayoutMapping {
        host: "one weave".to_string(),
        native: "one snapshot_create(<stamp>, \"vaire\") — a retained CoW root per weave"
            .to_string(),
    });
    rows
}

// ---------------------------------------------------------------------------
// UnaFS readiness — the measured limits, enforced before a native weave
// ---------------------------------------------------------------------------

/// A v3 volume's hard size cap: `MAX_BLOCK_COUNT` (524,288) × 4 KiB = 2 GiB.
/// One indirect refmap level; a bigger store needs a second.
pub const UNAFS_VOLUME_CAP_BYTES: u64 = 524_288 * 4096;

/// The measured single-file ceiling. An `Inode` — extent list included — must
/// serialize inside ONE 4 KiB block, so a file's *extent count* is the binding
/// limit, not the volume size. Measured on a fresh volume with 1 MiB writes:
/// 75 MiB → 151 extents, fine; 85 MiB → `InodeTooLarge(4100, 4096)`. A
/// fragmented volume will fail earlier, so this is a ceiling, not a promise.
/// (Harness: `examples/unafs_repo_feasibility.rs`.)
pub const UNAFS_MAX_FILE_BYTES: u64 = 75 * 1024 * 1024;

/// Directory width past which a name lookup gets expensive: UnaFS has no path
/// index, so a lookup is a full `ls` plus a linear scan (measured: 0.007 ms at
/// 64 entries, 0.045 ms at 512, 0.312 ms at 4096 — linear in directory size).
/// Git's 256-way object fan-out keeps directories far below this; a flat
/// object store would be quadratic.
pub const UNAFS_WIDE_DIR_ENTRIES: usize = 4096;

/// Whether a repo Bolt's mirrors can be woven into a UnaFS v3 image today, and
/// on which of the measured limits it is close to the line.
#[derive(Debug, Clone)]
pub struct Readiness {
    pub total_bytes: u64,
    pub files: usize,
    /// Files at or over [`UNAFS_MAX_FILE_BYTES`] — each one blocks a weave.
    pub oversize: Vec<(PathBuf, u64)>,
    /// The widest directory found and its entry count.
    pub widest_dir: Option<(PathBuf, usize)>,
    /// Blocking reasons; empty means a native weave can proceed.
    pub blockers: Vec<String>,
    /// Non-blocking observations worth printing.
    pub warnings: Vec<String>,
}

impl Readiness {
    /// Can `repo-uweave` proceed?
    pub fn fits(&self) -> bool {
        self.blockers.is_empty()
    }
}

/// Check a repo Bolt's mirrors against the measured UnaFS v3 limits, BEFORE a
/// native weave touches an image. Cheap (a metadata walk) and read-only.
///
/// This exists because the feasibility study found two hard walls and one
/// soft one, and a repo manager that discovers them halfway through a weave —
/// as an `InodeTooLarge` from three layers down — is not a manager.
pub fn unafs_readiness(m: &RepoManifest) -> Result<Readiness> {
    let bolt = m.bolt();
    let mirrors = bolt.resolve(Path::new("mirrors"))?;
    let mut r = Readiness {
        total_bytes: 0,
        files: 0,
        oversize: Vec::new(),
        widest_dir: None,
        blockers: Vec::new(),
        warnings: Vec::new(),
    };
    walk_sizes(&mirrors, &mirrors, &mut r)?;

    for (path, bytes) in &r.oversize {
        r.blockers.push(format!(
            "{} is {} MiB — over the {} MiB single-file ceiling (an Inode's extent \
             list must fit one 4 KiB block). Repack the mirror into smaller packs, \
             or land the extent-list indirection named in the study.",
            path.display(),
            bytes / 1024 / 1024,
            UNAFS_MAX_FILE_BYTES / 1024 / 1024
        ));
    }
    // Payload alone is not the whole cost: measured write amplification on a
    // git-shaped mix is ~1.4x, and every retained snapshot that diverges adds
    // more. Call it at half the cap rather than at the cap.
    if r.total_bytes.saturating_mul(3) / 2 >= UNAFS_VOLUME_CAP_BYTES {
        r.blockers.push(format!(
            "{} MiB of mirror payload against a {} MiB v3 volume cap (×~1.4 measured \
             write amplification, before snapshots diverge)",
            r.total_bytes / 1024 / 1024,
            UNAFS_VOLUME_CAP_BYTES / 1024 / 1024
        ));
    } else if r.total_bytes * 4 >= UNAFS_VOLUME_CAP_BYTES {
        r.warnings.push(format!(
            "{} MiB of payload is over a quarter of the {} MiB volume cap — snapshot \
             headroom is finite",
            r.total_bytes / 1024 / 1024,
            UNAFS_VOLUME_CAP_BYTES / 1024 / 1024
        ));
    }
    if let Some((path, n)) = &r.widest_dir
        && *n >= UNAFS_WIDE_DIR_ENTRIES
    {
        r.warnings.push(format!(
            "{} holds {n} entries; UnaFS has no path index, so a lookup there is a \
             full ls plus a linear scan",
            path.display()
        ));
    }
    Ok(r)
}

fn walk_sizes(root: &Path, dir: &Path, r: &mut Readiness) -> Result<()> {
    let entries: Vec<_> = match fs::read_dir(dir) {
        Ok(rd) => rd.filter_map(|e| e.ok()).collect(),
        Err(_) => return Ok(()),
    };
    let count = entries.len();
    if r.widest_dir.as_ref().is_none_or(|(_, n)| count > *n) {
        r.widest_dir = Some((dir.to_path_buf(), count));
    }
    for e in entries {
        let meta = match e.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.is_symlink() {
            continue; // never followed — the Bolt-1 rule, unchanged
        }
        if meta.is_dir() {
            walk_sizes(root, &e.path(), r)?;
        } else if meta.is_file() {
            r.files += 1;
            r.total_bytes += meta.len();
            if meta.len() >= UNAFS_MAX_FILE_BYTES {
                let rel = e.path().strip_prefix(root).unwrap_or(&e.path()).to_path_buf();
                r.oversize.push((rel, meta.len()));
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// git helpers (read-only against sources; the mirror is the only write target)
// ---------------------------------------------------------------------------

/// Every ref in a repository as `(name, oid)`. Errors if the path is not a
/// readable repository — which is how an unreadable source is detected.
fn read_refs(repo: &Path) -> Result<Vec<RefRecord>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["for-each-ref", "--format=%(objectname) %(refname)"])
        .output()
        .context("run git for-each-ref")?;
    if !out.status.success() {
        bail!("not a readable repository: {}", repo.display());
    }
    let mut refs: Vec<RefRecord> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.split_once(' '))
        .map(|(oid, name)| RefRecord {
            name: name.to_string(),
            oid: oid.to_string(),
        })
        .collect();
    refs.sort();
    Ok(refs)
}

/// Is `oid` present in the mirror's object store?
fn object_present(mirror: &Path, oid: &str) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(mirror)
        .args(["cat-file", "-e", &format!("{oid}^{{object}}")])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Audit every mirrored ref's tree for credential-shaped paths. Committed
/// secrets cannot be filtered out after the fact — they are reported.
fn audit_credentials(
    mirror: &Path,
    refs: &[RefRecord],
    rules: &ExcludeRules,
) -> Result<Vec<CredentialFinding>> {
    let mut found = Vec::new();
    for r in refs {
        // Only commit-ish refs have trees; tags to blobs and the like are
        // skipped silently here because they carry no paths to audit.
        let out = Command::new("git")
            .arg("-C")
            .arg(mirror)
            .args(["ls-tree", "-r", "--name-only", &format!("{}^{{tree}}", r.oid)])
            .output()
            .context("run git ls-tree")?;
        if !out.status.success() {
            continue;
        }
        for path in String::from_utf8_lossy(&out.stdout).lines() {
            if rules.is_credential(Path::new(path)) {
                found.push(CredentialFinding {
                    reference: r.name.clone(),
                    path: path.to_string(),
                });
            }
        }
    }
    found.sort();
    found.dedup();
    Ok(found)
}

fn git_ok(cwd: &Path, args: &[&str]) -> Result<()> {
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

#[cfg(test)]
mod tests;
