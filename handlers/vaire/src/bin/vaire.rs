// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `vaire` — the Loom CLI. BOLT-1 exposes the dev-tree Bolt's three coherence
//! verbs: STATUS, SNAP, and SYNC (dry-run default; `--apply` to write).
//!
//! Usage:
//!   vaire status  [--manifest <path>]
//!   vaire snap    [--manifest <path>]
//!   vaire sync    [--manifest <path>] [--apply]
//!   vaire usync   [<image>] [--manifest <path>] [--apply] [--size-mb <n>]
//!   vaire ustatus [<image>] [--manifest <path>]
//!
//! ⚠ RIDER 1: the FIRST real run against the live narino tree / `/Volumes/40G`
//! is Peter-attended. `--apply` always SNAPs first.
//!
//! `usync`/`ustatus` are the UnaFS-native Loom (VAIRE-2): the dev-tree woven
//! into a UnaFS v3 image as native objects. `usync` is DRY-RUN by default;
//! `--apply` writes and retains a snapshot. The image is an explicit argument
//! (never a shared/fixed path); it defaults under `~/unaos-bench/vaire/`.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use vaire::CrystalColor;
use vaire::devtree::{self, DevManifest, ExcludeClass, PenumbraDelta};
use vaire::repo::{self, RepoManifest, WeaveAction};
use vaire::usync::{self, DEFAULT_IMAGE_MB, FileDisposition};

const DEFAULT_MANIFEST: &str = "handlers/vaire/bolt.manifest.toml";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("vaire: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str);

    let mut manifest_path = PathBuf::from(DEFAULT_MANIFEST);
    let mut apply = false;
    let mut deep = false;
    let mut size_mb = DEFAULT_IMAGE_MB;
    let mut image: Option<PathBuf> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--manifest" => {
                i += 1;
                manifest_path = PathBuf::from(args.get(i).context("--manifest needs a path")?);
            }
            "--apply" => apply = true,
            "--deep" => deep = true,
            "--size-mb" => {
                i += 1;
                size_mb = args
                    .get(i)
                    .context("--size-mb needs a number")?
                    .parse()
                    .context("--size-mb must be an integer")?;
            }
            other if other.starts_with("--") => bail!("unknown argument: {other}"),
            // A bare positional: the image path (usync/ustatus only).
            _ if image.is_none() => image = Some(PathBuf::from(&args[i])),
            other => bail!("unexpected argument: {other}"),
        }
        i += 1;
    }

    match cmd {
        Some("status") => cmd_status(&manifest_path),
        Some("snap") => cmd_snap(&manifest_path),
        Some("sync") => cmd_sync(&manifest_path, apply),
        Some("usync") => cmd_usync(&manifest_path, image, apply, size_mb),
        Some("ustatus") => cmd_ustatus(&manifest_path, image),
        Some("repo-status") => cmd_repo_status(&manifest_path),
        Some("repo-plan") => cmd_repo_plan(&manifest_path),
        Some("repo-weave") => cmd_repo_weave(&manifest_path, apply),
        Some("repo-verify") => cmd_repo_verify(&manifest_path, image, deep),
        Some("repo-layout") => cmd_repo_layout(&manifest_path),
        Some("repo-ufit") => cmd_repo_ufit(&manifest_path, size_mb),
        Some("repo-uweave") => cmd_repo_uweave(&manifest_path, image, apply, size_mb),
        Some("-h") | Some("--help") | None => {
            print_usage();
            Ok(())
        }
        Some(other) => bail!(
            "unknown command: {other} (try: status | snap | sync | usync | ustatus | \
             repo-status | repo-plan | repo-weave | repo-verify | repo-layout | repo-ufit | \
             repo-uweave)"
        ),
    }
}

/// The default image path when none is given: `~/unaos-bench/vaire/devtree.img`
/// (NEVER a shared/fixed bench path).
fn default_image() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME must be set for the default image path")?;
    Ok(PathBuf::from(home).join("unaos-bench/vaire/devtree.img"))
}

fn print_usage() {
    println!(
        "vaire — the Loom (BOLT-1 dev-tree Bolt)\n\n\
         USAGE:\n\
         \x20 vaire status [--manifest <path>]\n\
         \x20 vaire snap   [--manifest <path>]\n\
         \x20 vaire sync    [--manifest <path>] [--apply]\n\
         \x20 vaire usync   [<image>] [--manifest <path>] [--apply] [--size-mb <n>]\n\
         \x20 vaire ustatus [<image>] [--manifest <path>]\n\n\
         REPO BOLTS (BOLT-2) — a git repository managed as a unit. These verbs take\n\
         a REPO-bolt manifest, so --manifest <path> is required:\n\
         \x20 vaire repo-status  --manifest <path>\n\
         \x20 vaire repo-plan    --manifest <path>\n\
         \x20 vaire repo-weave   --manifest <path> [--apply]\n\
         \x20 vaire repo-verify  [<image>] --manifest <path> [--deep]\n\
         \x20 vaire repo-layout  --manifest <path>\n\
         \x20 vaire repo-ufit    --manifest <path> [--size-mb <n>]\n\
         \x20 vaire repo-uweave  [<image>] --manifest <path> [--apply] [--size-mb <n>]\n\n\
         SYNC is DRY-RUN by default; --apply writes (narino->40G) and SNAPs first.\n\
         USYNC weaves the penumbra into a UnaFS v3 image as native objects (DRY-RUN\n\
         by default; --apply writes + retains a snapshot). Image defaults under\n\
         ~/unaos-bench/vaire/.\n\
         REPO-WEAVE is DRY-RUN by default too; --apply mirrors and appends one\n\
         hash-chained ledger entry per repo. REPO-UWEAVE weaves the bolt's mirrors\n\
         into a UnaFS v3 image through the usync engine and, on --apply, anchors\n\
         each ledger head in the image's retained root. REPO-VERIFY with an image\n\
         checks the ledger head against that anchor (the tamper-evident path).\n\
         Default manifest (dev-tree verbs only): {DEFAULT_MANIFEST}"
    );
}

fn crystal_tag(c: CrystalColor) -> &'static str {
    match c {
        CrystalColor::Green => "GREEN",
        CrystalColor::Amber => "AMBER",
        CrystalColor::Red => "RED",
    }
}

fn cmd_status(manifest: &std::path::Path) -> Result<()> {
    let m = DevManifest::load(manifest)?;
    let s = devtree::status(&m)?;
    println!("Bolt: {}  [{}]", s.name, crystal_tag(s.crystal));
    println!(
        "  live: narino ({})   target: {} ({})",
        m.live_root.display(),
        m.target_root.display(),
        if s.target_present { "present" } else { "ABSENT" }
    );
    if let Some(w) = &s.last_weave {
        println!("  last weave: {w}");
    }
    println!("  repos:");
    for r in &s.repos {
        let mirror = match r.mirror_coherent {
            Some(true) => "mirror:coherent",
            Some(false) => "mirror:BEHIND",
            None => "mirror:n/a",
        };
        let wt = if r.is_worktree_pointer { "  [worktree-pointer RED-for-switch]" } else { "" };
        println!(
            "    - {}: {} dirty={} unpushed={} {mirror}{wt}",
            r.name, r.branch, r.dirty_files, r.unpushed
        );
    }
    if !s.penumbra.is_empty() {
        let (mut same, mut only, mut diff) = (0, 0, 0);
        for p in &s.penumbra {
            match p.delta {
                PenumbraDelta::Same => same += 1,
                PenumbraDelta::OnlyLive => only += 1,
                PenumbraDelta::Differs => diff += 1,
            }
        }
        println!("  penumbra: {same} same, {only} only-live, {diff} differ");
    }
    if !s.excluded.is_empty() {
        let creds = s.excluded.iter().filter(|e| e.class == ExcludeClass::Credential).count();
        let links = s.excluded.iter().filter(|e| e.class == ExcludeClass::Symlink).count();
        println!(
            "  excluded: {} ({} CREDENTIAL-shaped default-deny, {} symlinks not followed)",
            s.excluded.len(),
            creds,
            links
        );
        for e in s.excluded.iter().filter(|e| e.class == ExcludeClass::Credential) {
            println!("    - CREDENTIAL skip: {}/{}", e.root.display(), e.rel.display());
        }
        for e in s.excluded.iter().filter(|e| e.class == ExcludeClass::Symlink) {
            println!("    - symlink skip (not followed): {}/{}", e.root.display(), e.rel.display());
        }
    }
    Ok(())
}

fn cmd_snap(manifest: &std::path::Path) -> Result<()> {
    let m = DevManifest::load(manifest)?;
    let r = devtree::snap(&m)?;
    println!("SNAP {} -> {}", r.stamp, r.dir.display());
    println!("  bundles: {}  penumbra files: {}", r.bundles.len(), r.penumbra_files);
    let creds = r.excluded.iter().filter(|e| e.class == ExcludeClass::Credential).count();
    let links = r.excluded.iter().filter(|e| e.class == ExcludeClass::Symlink).count();
    println!(
        "  excluded: {} ({creds} credential-shaped, {links} symlinks not followed)",
        r.excluded.len()
    );
    Ok(())
}

fn cmd_sync(manifest: &std::path::Path, apply: bool) -> Result<()> {
    let m = DevManifest::load(manifest)?;
    if apply {
        let (snap, plan) = devtree::apply_sync(&m)?;
        println!("SYNC --apply (SNAP {} taken first)", snap.stamp);
        print!("{}", plan.render());
        println!("apply complete.");
    } else {
        let plan = devtree::plan_sync(&m)?;
        println!("SYNC dry-run (narino->40G). Pass --apply to write.\n");
        print!("{}", plan.render());
    }
    Ok(())
}

fn cmd_usync(
    manifest: &std::path::Path,
    image: Option<PathBuf>,
    apply: bool,
    size_mb: u64,
) -> Result<()> {
    let m = DevManifest::load(manifest)?;
    let image = match image {
        Some(p) => p,
        None => default_image()?,
    };
    let r = usync::usync(&m, &image, apply, size_mb)?;

    if r.applied {
        println!(
            "USYNC --apply -> {}{}",
            r.image.display(),
            if r.formatted { "  (freshly formatted)" } else { "" }
        );
    } else {
        println!("USYNC dry-run -> {}. Pass --apply to write.", r.image.display());
    }

    // Per-file rows (Bolt-1 style: WOVE / skip / SKIP-excluded), deterministic.
    for row in &r.rows {
        match row.disposition {
            FileDisposition::Written => {
                println!("  WOVE  {} ({} B)", row.rel.display(), row.bytes)
            }
            FileDisposition::Skipped => {
                println!("  skip  {} (unchanged)", row.rel.display())
            }
        }
    }
    let creds = r.excluded.iter().filter(|e| e.class == ExcludeClass::Credential).count();
    let links = r.excluded.iter().filter(|e| e.class == ExcludeClass::Symlink).count();
    let junk = r.excluded.iter().filter(|e| e.class == ExcludeClass::Junk).count();
    for e in &r.excluded {
        let tag = match e.class {
            ExcludeClass::Credential => "CREDENTIAL",
            ExcludeClass::Junk => "junk",
            ExcludeClass::Symlink => "symlink (not followed)",
        };
        println!("  SKIP  {}/{} [{tag}]", e.root.display(), e.rel.display());
    }

    if let Some((name, generation)) = &r.snapshot {
        println!("  snapshot retained: '{name}' (generation {generation})");
    }
    println!(
        "\nsummary: {} written, {} skipped, {} excluded ({creds} credential, {junk} junk, {links} symlink), {} dirs, {} bytes",
        r.files_written, r.files_skipped, r.excluded.len(), r.dirs_created, r.bytes_written
    );
    println!("BENCHMARK: {}", r.ledger_line());
    Ok(())
}

fn cmd_ustatus(manifest: &std::path::Path, image: Option<PathBuf>) -> Result<()> {
    let m = DevManifest::load(manifest)?;
    let image = match image {
        Some(p) => p,
        None => default_image()?,
    };
    let st = usync::ustatus(&m, &image)?;
    println!("USTATUS {}", st.image.display());
    println!("  units:");
    for u in &st.units {
        println!(
            "    - {} : {} files, {} bytes  [manifest={}  git={}  last-run={}]",
            u.name,
            u.files,
            u.bytes,
            u.manifest.as_deref().unwrap_or("?"),
            u.git_head.as_deref().unwrap_or("?"),
            u.last_run.as_deref().unwrap_or("never"),
        );
    }
    println!(
        "  image objects: {}   live penumbra files: {}",
        st.image_objects, st.live_files
    );
    if st.snapshots.is_empty() {
        println!("  snapshots: none");
    } else {
        println!("  snapshots ({} retained):", st.snapshots.len());
        for s in &st.snapshots {
            println!(
                "    - gen {:>4}  {:<18}  creator={}  ts={}",
                s.generation, s.name, s.creator, s.timestamp
            );
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Repo Bolts (BOLT-2)
// ---------------------------------------------------------------------------

/// Load a REPO-bolt manifest, refusing the dev-tree default: the two manifest
/// kinds are different files, and silently reading the wrong one would point a
/// weave at the wrong tree.
fn load_repo_manifest(path: &std::path::Path) -> Result<RepoManifest> {
    if path == std::path::Path::new(DEFAULT_MANIFEST) {
        bail!(
            "repo verbs need a REPO-bolt manifest: pass --manifest <path> \
             (the default {DEFAULT_MANIFEST} is the dev-tree Bolt's)"
        );
    }
    RepoManifest::load(path)
}

/// The image size a repo verb will actually create, in bytes, clamped to the
/// v3 format cap (a larger request cannot be formatted anyway).
fn volume_bytes(size_mb: u64) -> u64 {
    size_mb
        .saturating_mul(1024 * 1024)
        .min(repo::UNAFS_VOLUME_CAP_BYTES)
}

fn cmd_repo_status(manifest: &std::path::Path) -> Result<()> {
    let m = load_repo_manifest(manifest)?;
    println!("Repo Bolt: {}  (root {})", m.name, m.bolt_root.display());
    for row in repo::status(&m)? {
        println!(
            "  - {} [{}]  entries={} last={} credentials={}{}",
            row.name,
            crystal_tag(row.crystal),
            row.entries,
            row.last_stamp.as_deref().unwrap_or("never"),
            row.credentials,
            if row.source_ahead { "  source:AHEAD" } else { "" }
        );
        for b in &row.breaches {
            println!("      BREACH {b:?}");
        }
    }
    Ok(())
}

fn cmd_repo_plan(manifest: &std::path::Path) -> Result<()> {
    let m = load_repo_manifest(manifest)?;
    let plan = repo::plan_weave(&m)?;
    println!(
        "REPO-WEAVE dry-run -> {} ({}). Pass --apply to write.\n",
        plan.bolt.display(),
        if plan.bolt_present {
            "present"
        } else {
            "will be created"
        }
    );
    print!("{}", plan.render());
    Ok(())
}

fn cmd_repo_weave(manifest: &std::path::Path, apply: bool) -> Result<()> {
    if !apply {
        return cmd_repo_plan(manifest);
    }
    let m = load_repo_manifest(manifest)?;
    let r = repo::weave(&m)?;
    println!("REPO-WEAVE --apply {} -> {}", r.stamp, r.bolt.display());
    for w in &r.repos {
        match (&w.entry, w.action) {
            (Some(e), action) => println!(
                "  {:?}  {}: {} refs, {} credential findings, entry {}",
                action,
                w.name,
                e.refs.len(),
                e.credentials.len(),
                &e.hash[..12.min(e.hash.len())]
            ),
            (None, WeaveAction::SourceUnreadable) => {
                println!("  SKIP  {}: source unreadable (nothing recorded)", w.name)
            }
            (None, action) => println!("  {:?}  {}: no ledger entry", action, w.name),
        }
        if let Some(e) = &w.entry {
            for c in &e.credentials {
                println!("      CREDENTIAL in history: {} @ {}", c.path, c.reference);
            }
        }
    }
    println!("total credential findings: {}", r.credential_findings());
    Ok(())
}

fn cmd_repo_verify(
    manifest: &std::path::Path,
    image: Option<PathBuf>,
    deep: bool,
) -> Result<()> {
    let m = load_repo_manifest(manifest)?;
    // With an image, verify through the ANCHORED path: the ledger head is
    // checked against the CoW-retained anchor, which catches a whole-tail
    // rewrite or a truncation the bare chain cannot. Without one, verify is
    // chain-internal only — tamper-evident against an editor, not against a
    // writer who owns the ledger file.
    let anchored = image.is_some();
    let reports = match &image {
        Some(img) => repo::verify_anchored(&m, img, deep)?,
        None => repo::verify(&m, deep)?,
    };
    let mut intact = true;
    for v in reports {
        println!(
            "  - {} [{}]  entries={}  credentials={}",
            v.name,
            crystal_tag(v.crystal()),
            v.entries,
            v.credentials.len()
        );
        for b in &v.breaches {
            intact = false;
            println!("      BREACH {b:?}");
        }
    }
    if !intact {
        bail!("verification found integrity breaches (see above)");
    }
    if anchored {
        println!(
            "verify: chain, objects, refs AND image anchor intact{}",
            if deep { " (deep: git fsck clean)" } else { "" }
        );
    } else {
        println!(
            "verify: chain, objects and refs intact{} \
             (chain-internal only — pass an image to check the head anchor)",
            if deep { " (deep: git fsck clean)" } else { "" }
        );
    }
    Ok(())
}

fn cmd_repo_layout(manifest: &std::path::Path) -> Result<()> {
    let m = load_repo_manifest(manifest)?;
    println!("UnaFS layout mapping for repo Bolt '{}':", m.name);
    for row in repo::unafs_layout(&m) {
        println!("  {:<40} -> {}", row.host, row.native);
    }
    Ok(())
}

fn cmd_repo_ufit(manifest: &std::path::Path, size_mb: u64) -> Result<()> {
    let m = load_repo_manifest(manifest)?;
    let volume_bytes = volume_bytes(size_mb);
    let r = repo::unafs_readiness_for(&m, volume_bytes)?;
    println!(
        "UnaFS v3 fit for '{}': {} files, {} MiB payload (target volume {} MiB of a \
         {} MiB cap, single-file ceiling {} MiB)",
        m.name,
        r.files,
        r.total_bytes / 1024 / 1024,
        volume_bytes / 1024 / 1024,
        repo::UNAFS_VOLUME_CAP_BYTES / 1024 / 1024,
        repo::UNAFS_MAX_FILE_BYTES / 1024 / 1024
    );
    if let Some((path, n)) = &r.widest_dir {
        println!("  widest directory: {} ({n} entries)", path.display());
    }
    for w in &r.warnings {
        println!("  note:    {w}");
    }
    for b in &r.blockers {
        println!("  BLOCKER: {b}");
    }
    println!(
        "  verdict: {}",
        if r.fits() {
            "FITS — repo-uweave can proceed"
        } else {
            "DOES NOT FIT — repo-uweave would be refused"
        }
    );
    Ok(())
}

fn cmd_repo_uweave(
    manifest: &std::path::Path,
    image: Option<PathBuf>,
    apply: bool,
    size_mb: u64,
) -> Result<()> {
    let m = load_repo_manifest(manifest)?;
    // Check the measured v3 limits BEFORE touching an image, so a run that
    // cannot fit is a true no-op with a reason, not an InodeTooLarge from
    // three layers down halfway through a weave. The check is against the
    // volume THIS run will create (`--size-mb`), not merely the format cap —
    // otherwise a bolt bigger than the image would still be waved through.
    let fit = repo::unafs_readiness_for(&m, volume_bytes(size_mb))?;
    for w in &fit.warnings {
        eprintln!("vaire: note: {w}");
    }
    if !fit.fits() {
        for b in &fit.blockers {
            eprintln!("vaire: BLOCKER: {b}");
        }
        bail!("repo-uweave refused: the bolt does not fit a UnaFS v3 volume (nothing written)");
    }
    let image = match image {
        Some(p) => p,
        None => default_image()?,
    };
    // On --apply, weave through the anchoring path so the ledger head is pinned
    // in the image's retained root (checkable by `repo-verify <image>`). The
    // dry-run stays a pure projection preview — it writes nothing.
    let r = if apply {
        repo::weave_into_image(&m, &image, size_mb)?
    } else {
        let dm = repo::unafs_view(&m)?;
        usync::usync(&dm, &image, false, size_mb)?
    };
    if r.applied {
        println!(
            "REPO-UWEAVE --apply -> {}{}",
            r.image.display(),
            if r.formatted {
                "  (freshly formatted)"
            } else {
                ""
            }
        );
    } else {
        println!(
            "REPO-UWEAVE dry-run -> {}. Pass --apply to write.",
            r.image.display()
        );
    }
    if let Some((name, generation)) = &r.snapshot {
        println!("  snapshot retained: '{name}' (generation {generation})");
    }
    println!(
        "summary: {} written, {} skipped, {} excluded, {} dirs, {} bytes",
        r.files_written,
        r.files_skipped,
        r.excluded.len(),
        r.dirs_created,
        r.bytes_written
    );
    println!("BENCHMARK: {}", r.ledger_line());
    Ok(())
}
