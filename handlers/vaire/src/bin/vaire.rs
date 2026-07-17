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
        Some("-h") | Some("--help") | None => {
            print_usage();
            Ok(())
        }
        Some(other) => {
            bail!("unknown command: {other} (try: status | snap | sync | usync | ustatus)")
        }
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
         SYNC is DRY-RUN by default; --apply writes (narino->40G) and SNAPs first.\n\
         USYNC weaves the penumbra into a UnaFS v3 image as native objects (DRY-RUN\n\
         by default; --apply writes + retains a snapshot). Image defaults under\n\
         ~/unaos-bench/vaire/.\n\
         Default manifest: {DEFAULT_MANIFEST}"
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
