// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `amber` — The Block's layout CLI (AMBER1, SR34). Every command is a bus verb run locally
//! (`amber_bytes::Amber::handle`), so the CLI and the bus cannot drift:
//!
//! ```text
//! amber list                                        DiskList (read-only)
//! amber plan    <target> --layout L | --part ...    PlanLayout: dry run + sha256, writes nothing
//! amber apply   <target> --layout L --sha256 HEX    Apply (refused unless HEX is the plan's)
//! amber verify  <target>                            Verify (exit 1 on FAIL)
//! amber recover <target> [--write]                  Recover (dry run unless --write)
//! amber bus                                         JSON requests on stdin, responses on stdout
//! ```
//!
//! A real disk is written only with `--yes-i-mean-it` AND `--allow <dev>` (or `AMBER_ALLOW`).

use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use amber_bytes::bus::{Amber, Request, Response};
use amber_bytes::layout::{LayoutSpec, PartSpec};
use amber_bytes::policy::Policy;
use clap::{Args, Parser, Subcommand};

#[derive(Parser)]
#[command(name = "amber", version, about = "Amber Bytes — partitioning, formatting, recovery")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Args, Clone)]
struct Keys {
    /// First key for a real disk: the operator means it.
    #[arg(long)]
    yes_i_mean_it: bool,
    /// Second key: the device on the allowlist (repeatable; also AMBER_ALLOW=a:b).
    #[arg(long)]
    allow: Vec<PathBuf>,
}

#[derive(Args, Clone)]
struct LayoutArgs {
    /// A layout file (`amber-layout v1`).
    #[arg(long, conflicts_with = "part")]
    layout: Option<PathBuf>,
    /// A partition `kind:size:name[:format]` (repeatable), e.g. `esp:128M:UNAOS-ESP:fat32`.
    #[arg(long)]
    part: Vec<String>,
    /// The disk seed for `--part` layouts (GUIDs derive from it).
    #[arg(long, default_value = "AMBER")]
    seed: String,
}

impl LayoutArgs {
    fn spec(&self) -> Result<LayoutSpec, String> {
        if let Some(p) = &self.layout {
            return LayoutSpec::parse(&std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?);
        }
        if self.part.is_empty() {
            return Err("give --layout FILE or at least one --part".into());
        }
        let parts = self.part.iter().map(|s| PartSpec::parse_compact(s)).collect::<Result<Vec<_>, _>>()?;
        let spec = LayoutSpec { disk_seed: self.seed.clone(), parts };
        LayoutSpec::parse(&spec.emit()) // the same validation a file gets
    }
}

#[derive(Subcommand)]
enum Cmd {
    /// List the host's block devices (read-only; nothing is opened).
    List {
        #[arg(long)]
        json: bool,
    },
    /// Plan a layout on a target: print the exact writes and the plan's sha256. Writes nothing.
    Plan {
        target: PathBuf,
        #[command(flatten)]
        layout: LayoutArgs,
        /// Save the canonical layout here (what `apply --layout` takes).
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    /// Apply a planned layout: lay the table, format the partitions, read everything back.
    Apply {
        target: PathBuf,
        #[command(flatten)]
        layout: LayoutArgs,
        /// The sha256 `amber plan` showed for this target.
        #[arg(long)]
        sha256: String,
        /// Check everything, write nothing.
        #[arg(long)]
        dry_run: bool,
        /// Create the target image (sparse, the layout's size) if it does not exist.
        #[arg(long)]
        create: bool,
        #[command(flatten)]
        keys: Keys,
    },
    /// Verify the partition table (primary, backup, MBR, agreement) and probe every partition.
    Verify { target: PathBuf },
    /// Restore a lost GPT copy from the good one. Dry run unless --write.
    Recover {
        target: PathBuf,
        #[arg(long)]
        write: bool,
        #[command(flatten)]
        keys: Keys,
    },
    /// Serve the bus wire on stdio: one JSON request per line in, one JSON response per line out.
    Bus {
        #[command(flatten)]
        keys: Keys,
    },
}

fn amber(keys: Option<&Keys>) -> Amber {
    let k = keys.cloned().unwrap_or(Keys { yes_i_mean_it: false, allow: vec![] });
    Amber::new(Policy::from_env(k.yes_i_mean_it, &k.allow))
}

fn print_lines(lines: &[String]) {
    for l in lines {
        println!("{l}");
    }
}

fn run(cli: Cli) -> Result<bool, String> {
    match cli.cmd {
        Cmd::List { json } => match amber(None).handle(Request::DiskList) {
            Response::Disks { disks } if json => {
                println!("{}", serde_json::to_string_pretty(&disks).map_err(|e| e.to_string())?);
                Ok(true)
            }
            Response::Disks { disks } => {
                for d in &disks {
                    println!("{}", d.line());
                }
                Ok(true)
            }
            other => Err(format!("{other:?}")),
        },
        Cmd::Plan { target, layout, out } => {
            let spec = layout.spec()?;
            let exists = target.exists();
            let req = Request::PlanLayout { layout: spec.emit(), target: exists.then(|| target.clone()), disk_sectors: None };
            match amber(None).handle(req) {
                Response::Planned { disk_sectors, layout, dry_run, scheme, signature } => {
                    if !exists {
                        println!("# {} does not exist: planned as a new {disk_sectors}-sector image (`apply --create` makes it)", target.display());
                    }
                    print!("{dry_run}");
                    println!("{scheme} {signature}");
                    if let Some(o) = out {
                        std::fs::write(&o, layout).map_err(|e| format!("{}: {e}", o.display()))?;
                        println!("# layout saved to {}", o.display());
                    }
                    Ok(true)
                }
                Response::Error { message } => Err(message),
                other => Err(format!("{other:?}")),
            }
        }
        Cmd::Apply { target, layout, sha256, dry_run, create, keys } => {
            let spec = layout.spec()?;
            if !target.exists() {
                if !create {
                    return Err(format!("{} does not exist (pass --create to make the image)", target.display()));
                }
                if dry_run {
                    return Err("--dry-run with --create: nothing exists to check yet; run `amber plan`".into());
                }
                create_image(&target, spec.image_sectors()?)?;
            }
            let req = Request::Apply { target, layout: spec.emit(), scheme: "sha256".into(), signature: sha256, dry_run };
            match amber(Some(&keys)).handle(req) {
                Response::Applied { lines, .. } => {
                    print_lines(&lines);
                    Ok(true)
                }
                Response::Error { message } => Err(message),
                other => Err(format!("{other:?}")),
            }
        }
        Cmd::Verify { target } => match amber(None).handle(Request::Verify { target }) {
            Response::Verified { ok, lines, .. } => {
                print_lines(&lines);
                Ok(ok)
            }
            Response::Error { message } => Err(message),
            other => Err(format!("{other:?}")),
        },
        Cmd::Recover { target, write, keys } => match amber(Some(&keys)).handle(Request::Recover { target, dry_run: !write }) {
            Response::Recovered { written, lines, .. } => {
                print_lines(&lines);
                if !written && write {
                    println!("nothing to write");
                } else if !write {
                    println!("dry run: nothing written (pass --write to restore)");
                }
                Ok(true)
            }
            Response::Error { message } => Err(message),
            other => Err(format!("{other:?}")),
        },
        Cmd::Bus { keys } => {
            let a = amber(Some(&keys));
            for line in std::io::stdin().lock().lines() {
                let line = line.map_err(|e| e.to_string())?;
                if line.trim().is_empty() {
                    continue;
                }
                println!("{}", a.handle_json(&line));
            }
            Ok(true)
        }
    }
}

fn create_image(path: &Path, sectors: u64) -> Result<(), String> {
    let f = std::fs::OpenOptions::new().write(true).create_new(true).open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    f.set_len(sectors * 512).map_err(|e| format!("{}: {e}", path.display()))?;
    println!("created {} ({} sectors, sparse)", path.display(), sectors);
    Ok(())
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            eprintln!("amber: {e}");
            ExitCode::from(2)
        }
    }
}
