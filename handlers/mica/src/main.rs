// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `mica jobs <verb>` — the jobs on the UnaOS volume (UNAOSVOLUME, rmbp-ledger B427). Mica is the only writer.
//!
//!   mica jobs build   --repo R --img I [--size-mb N] [--into]   repo records → a fresh volume (or INTO an existing one)
//!   mica jobs add     --img I --set-by S [--refs B1,B2] <claim sentence>
//!   mica jobs cite    --repo R --img I --set-by S <ST<n>> <status> [<f<n>> [<the wire line, quoted whole>]]
//!   mica jobs verify  --repo R --img I
//!   mica jobs list    --img I [claim|ledger|queue]
//!   mica jobs query   --img I <status=confirmed flight=f24 | UnaFS query text>
//!   mica jobs export  --repo R --img I [--out DIR]               STATUS.tsv + the ledgers, written back (DIR defaults to R)
//!   mica jobs witness --repo R [--img I]                         repo → volume → export: the witness line
use anyhow::{Context, Result, anyhow, bail};
use jobs_core as jc;
use std::path::{Path, PathBuf};
use unafs::{FileDevice, FileSystem};

struct Args {
    pos: Vec<String>,
    repo: PathBuf,
    img: Option<String>,
    out: Option<PathBuf>,
    set_by: String,
    refs: String,
    size_mb: Option<u64>,
    into: bool,
}

fn parse(v: &[String]) -> Result<Args> {
    let mut a = Args { pos: vec![], repo: PathBuf::from("."), img: None, out: None, set_by: String::new(), refs: String::new(), size_mb: None, into: false };
    let mut i = 0;
    while i < v.len() {
        let next = |i: usize| v.get(i + 1).cloned().ok_or_else(|| anyhow!("{} needs a value", v[i]));
        match v[i].as_str() {
            "--repo" => { a.repo = PathBuf::from(next(i)?); i += 1 }
            "--img" | "-i" => { a.img = Some(next(i)?); i += 1 }
            "--out" => { a.out = Some(PathBuf::from(next(i)?)); i += 1 }
            "--set-by" => { a.set_by = next(i)?; i += 1 }
            "--refs" => { a.refs = next(i)?; i += 1 }
            "--size-mb" => { a.size_mb = Some(next(i)?.parse()?); i += 1 }
            "--into" => a.into = true,
            _ => a.pos.push(v[i].clone()),
        }
        i += 1;
    }
    Ok(a)
}

fn mount(img: &Option<String>) -> Result<FileSystem> {
    let img = img.as_ref().ok_or_else(|| anyhow!("--img <volume image> is required"))?;
    let dev = FileDevice::open(img).with_context(|| format!("open {img}"))?;
    FileSystem::mount(dev).map_err(|e| anyhow!("mount {img}: {e:?}"))
}

fn main() -> Result<()> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.first().map(String::as_str) != Some("jobs") || argv.len() < 2 {
        bail!("usage: mica jobs <build|add|cite|verify|list|query|export|witness> … (see src/main.rs)");
    }
    let verb = argv[1].clone();
    let a = parse(&argv[2..])?;
    match verb.as_str() {
        "build" => {
            let src = mica::Sources::read(&a.repo)?;
            let recs = src.records()?;
            let img = a.img.clone().ok_or_else(|| anyhow!("--img is required"))?;
            if !a.into {
                let mb = a.size_mb.unwrap_or_else(|| mica::size_mb_for(&recs));
                let f = std::fs::File::create(&img).with_context(|| format!("create {img}"))?;
                f.set_len(mb * 1024 * 1024)?;
                drop(f);
                let mut dev = FileDevice::open(&img)?;
                unafs::format(&mut dev, &unafs::FormatParams::sized_mb(mb)).map_err(|e| anyhow!("format: {e:?}"))?;
            } else if mount(&a.img)?.resolve_path(jc::ROOT).is_ok() {
                bail!("{img}: {} already exists — the volume is the store; `mica jobs` edits it, a build never overwrites it", jc::ROOT);
            }
            let mut fs = mount(&a.img)?;
            mica::populate(&mut fs, &recs)?;
            let w = mica::witness_on(&a.repo, &src, &mut fs)?;
            for id in &w.failed {
                eprintln!("[jobs] verify FAILED {id}");
            }
            println!("[jobs] built {img} volume={} records={}", jc::ROOT, recs.len());
            println!("{}", w.line());
            if !w.identical {
                bail!("the volume's export differs from the git text");
            }
        }
        "add" => {
            let mut fs = mount(&a.img)?;
            let r = mica::add_claim(&mut fs, &a.pos.join(" "), &a.set_by, &a.refs)?;
            println!("[jobs] add {} status=open {}", r.id, r.path());
        }
        "cite" => {
            if a.pos.len() < 2 {
                bail!("usage: mica jobs cite --repo R --img I --set-by S ST<n> <status> [f<n> [line]]");
            }
            let mut fs = mount(&a.img)?;
            let fl = a.pos.get(2).cloned().unwrap_or_default();
            let line = if a.pos.len() > 3 { a.pos[3..].join(" ") } else { String::new() };
            let r = mica::cite_claim(&a.repo, &mut fs, &a.pos[0], &a.pos[1], &fl, &line, &a.set_by)?;
            println!("[jobs] cite {} status={} flight={} line={}", r.id, r.get(jc::K_STATUS), if fl.is_empty() { "-" } else { &fl }, if line.is_empty() { "-" } else { "verified" });
        }
        "verify" => {
            let mut fs = mount(&a.img)?;
            let recs = mica::load(&mut fs)?;
            let (ok, n, bad) = mica::verify_all(&a.repo, &recs);
            for id in &bad {
                println!("[jobs] verify FAILED {id}");
            }
            println!("[jobs] verify={ok}/{n}");
            if !bad.is_empty() {
                std::process::exit(1);
            }
        }
        "list" => {
            let mut fs = mount(&a.img)?;
            let want = a.pos.first().and_then(|w| jc::Kind::from_word(w));
            for r in mica::load(&mut fs)?.iter().filter(|r| want.is_none_or(|k| r.kind == k)) {
                println!("{}\t{}\t{}\t{}", r.path(), r.get(jc::K_STATUS), r.get(jc::K_FLIGHT), r.get(jc::K_ARC));
            }
        }
        "query" => {
            let mut fs = mount(&a.img)?;
            let hits = mica::query(&mut fs, &a.pos.join(" "))?;
            for p in &hits {
                println!("{p}");
            }
            println!("[jobs] query hits={}", hits.len());
        }
        "export" => {
            let mut fs = mount(&a.img)?;
            let recs = mica::load(&mut fs)?;
            let src = mica::Sources::read(&a.repo)?;
            let out = a.out.clone().unwrap_or_else(|| a.repo.clone());
            let mut changed = 0;
            for (rel, body) in src.export(&recs) {
                let p = out.join(&rel);
                if std::fs::read_to_string(&p).ok().as_deref() != Some(body.as_str()) {
                    changed += 1;
                }
                if let Some(d) = p.parent() {
                    std::fs::create_dir_all(d)?;
                }
                std::fs::write(&p, body).with_context(|| format!("write {}", p.display()))?;
            }
            println!("[jobs] export files={} changed={} records={}", 1 + jc::LEDGERS.len(), changed, recs.len());
        }
        "witness" => {
            let w = match &a.img {
                Some(_) => {
                    let src = mica::Sources::read(&a.repo)?;
                    mica::witness_on(&a.repo, &src, &mut mount(&a.img)?)?
                }
                None => mica::witness(Path::new(&a.repo))?,
            };
            for id in &w.failed {
                eprintln!("[jobs] verify FAILED {id}");
            }
            println!("{}", w.line());
            if !w.identical || !w.failed.is_empty() {
                std::process::exit(1);
            }
        }
        v => bail!("unknown verb {v}"),
    }
    Ok(())
}
