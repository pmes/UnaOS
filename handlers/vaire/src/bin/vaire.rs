// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `vaire` — the Loom CLI. BOLT-1 exposes the dev-tree Bolt's three coherence
//! verbs: STATUS, SNAP, and SYNC (dry-run default; `--apply` to write).
//!
//! Usage:
//!   vaire status [--manifest <path>]
//!   vaire snap   [--manifest <path>]
//!   vaire sync   [--manifest <path>] [--apply]
//!
//! ⚠ RIDER 1: the FIRST real run against the live narino tree / `/Volumes/40G`
//! is Peter-attended. `--apply` always SNAPs first.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use vaire::CrystalColor;
use vaire::devtree::{self, DevManifest, ExcludeClass, PenumbraDelta};

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
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--manifest" => {
                i += 1;
                manifest_path = PathBuf::from(args.get(i).context("--manifest needs a path")?);
            }
            "--apply" => apply = true,
            other => bail!("unknown argument: {other}"),
        }
        i += 1;
    }

    match cmd {
        Some("status") => cmd_status(&manifest_path),
        Some("snap") => cmd_snap(&manifest_path),
        Some("sync") => cmd_sync(&manifest_path, apply),
        Some("-h") | Some("--help") | None => {
            print_usage();
            Ok(())
        }
        Some(other) => {
            bail!("unknown command: {other} (try: status | snap | sync)")
        }
    }
}

fn print_usage() {
    println!(
        "vaire — the Loom (BOLT-1 dev-tree Bolt)\n\n\
         USAGE:\n\
         \x20 vaire status [--manifest <path>]\n\
         \x20 vaire snap   [--manifest <path>]\n\
         \x20 vaire sync   [--manifest <path>] [--apply]\n\n\
         SYNC is DRY-RUN by default; --apply writes (narino->40G) and SNAPs first.\n\
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
