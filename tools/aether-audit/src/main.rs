//! AETHERSEE — the loop that lets an executor SEE Aether's output.
//!
//! `aether-audit run`  renders every page of `corpus/` with Aether's headless
//! oracle and with Chromium (Playwright, `ref.mjs`), writes
//! `out/<id>.{aether,ref,diff}.png`, `out/scores.json` and `out/SCORE.md`.
//! `aether-audit gate` compares `out/scores.json` against `baseline.json`
//! and exits 1 when any page regresses past the recorded thresholds.
//!
//! Scores: `ssim` is a mean structural similarity over 8x8 windows of the
//! 2x-downsampled greyscale frames (1.0 = identical); `mismatch` is the
//! percentage of pixels whose largest channel differs by more than 40.

use image::{Rgba, RgbaImage};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

const MOBILE: (u32, u32) = (375, 812);
const DESKTOP: (u32, u32) = (800, 600);
const PIXEL_TOL: i32 = 40;
const AETHER_TIMEOUT: Duration = Duration::from_secs(90);

struct Job {
    id: String,
    url: String,
    html: Option<PathBuf>,
    w: u32,
    h: u32,
}

struct Score {
    ssim: f64,
    mismatch: f64,
    missing: usize,
    top: Vec<(String, u64)>,
    note: String,
}

fn root() -> PathBuf {
    std::env::var_os("AETHER_AUDIT_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")))
}

fn arg_val(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1).cloned())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("run") => run(&args[1..]),
        Some("gate") => gate(&args[1..]),
        _ => {
            eprintln!(
                "usage: aether-audit run [--aether BIN] [--live] [--only SUBSTR] [--refresh-ref]\n       \
                 aether-audit gate [--write]"
            );
            ExitCode::from(2)
        }
    }
}

fn jobs(live: bool, only: Option<&str>) -> Vec<Job> {
    let corpus = root().join("corpus");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&corpus)
        .expect("corpus dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "html"))
        .collect();
    files.sort();
    let mut out = Vec::new();
    for f in files {
        let stem = f.file_stem().unwrap().to_string_lossy().to_string();
        let abs = f.canonicalize().unwrap();
        let header = std::fs::read_to_string(&abs).unwrap_or_default();
        let header = header.lines().next().unwrap_or("");
        let url = format!("file://{}", abs.display());
        out.push(Job { id: stem.clone(), url: url.clone(), html: Some(abs.clone()), w: DESKTOP.0, h: DESKTOP.1 });
        // A page whose header names the 375 width is responsive: render it twice.
        if header.contains("375") {
            out.push(Job { id: format!("{stem}@375"), url, html: Some(abs), w: MOBILE.0, h: MOBILE.1 });
        }
    }
    if live {
        let list = std::fs::read_to_string(corpus.join("live.txt")).unwrap_or_default();
        for line in list.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
            let mut it = line.split_whitespace();
            if let (Some(name), Some(url)) = (it.next(), it.next()) {
                out.push(Job { id: format!("live-{name}"), url: url.into(), html: None, w: DESKTOP.0, h: DESKTOP.1 });
            }
        }
    }
    if let Some(s) = only {
        out.retain(|j| j.id.contains(s));
    }
    out
}

fn run(args: &[String]) -> ExitCode {
    let r = root();
    let out_dir = r.join("out");
    std::fs::create_dir_all(&out_dir).unwrap();
    let aether = arg_val(args, "--aether")
        .map(PathBuf::from)
        .unwrap_or_else(|| r.join("../../target/release/aether"));
    let jobs = jobs(args.iter().any(|a| a == "--live"), arg_val(args, "--only").as_deref());
    let refresh = args.iter().any(|a| a == "--refresh-ref");

    // 1. Reference frames. Corpus pages are cached by mtime; live pages always re-shoot.
    let mut tsv = String::new();
    for j in &jobs {
        let refp = out_dir.join(format!("{}.ref.png", j.id));
        let stale = refresh
            || j.html.is_none()
            || match (std::fs::metadata(&refp), j.html.as_ref().map(std::fs::metadata)) {
                (Ok(r), Some(Ok(s))) => r.modified().unwrap() < s.modified().unwrap(),
                _ => true,
            };
        if stale {
            tsv.push_str(&format!("{}\t{}\t{}\t{}\n", j.url, refp.display(), j.w, j.h));
        }
    }
    if !tsv.is_empty() {
        let tsv_path = out_dir.join("ref-jobs.tsv");
        std::fs::write(&tsv_path, &tsv).unwrap();
        let st = Command::new("node")
            .arg(r.join("ref.mjs"))
            .arg(&tsv_path)
            .env("PLAYWRIGHT_BROWSERS_PATH", std::env::var("PLAYWRIGHT_BROWSERS_PATH").unwrap_or("/opt/pw-browsers".into()))
            .status();
        if !matches!(st, Ok(s) if s.success()) {
            eprintln!("aether-audit: reference renderer reported failures (see above)");
        }
    }

    // 2. Aether frames, then score.
    let mut scores: BTreeMap<String, Score> = BTreeMap::new();
    for j in &jobs {
        let a_png = out_dir.join(format!("{}.aether.png", j.id));
        let ledger = out_dir.join(format!("{}.ledger.txt", j.id));
        let _ = std::fs::remove_file(&a_png);
        let mut cmd = Command::new(&aether);
        cmd.arg("render");
        match &j.html {
            Some(p) => {
                cmd.arg("--html").arg(p);
            }
            None => {
                cmd.arg(&j.url);
            }
        }
        cmd.arg("--out").arg(&a_png).arg("--ledger").arg(&ledger);
        cmd.arg("--width").arg(j.w.to_string()).arg("--height").arg(j.h.to_string());
        cmd.stdout(Stdio::null()).stderr(Stdio::null());
        let t0 = Instant::now();
        let mut note = String::new();
        match cmd.spawn() {
            Ok(mut child) => loop {
                if let Ok(Some(_)) = child.try_wait() {
                    break;
                }
                if t0.elapsed() > AETHER_TIMEOUT {
                    let _ = child.kill(); // this child's own pid
                    let _ = child.wait();
                    note = "aether timeout".into();
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            },
            Err(e) => {
                eprintln!("aether-audit: cannot run {}: {e}", aether.display());
                return ExitCode::from(2);
            }
        }
        let s = score(j, &out_dir, &a_png, &ledger, note);
        println!(
            "{:<24} ssim {:.3}  mismatch {:5.1}%  missing {:3}  ({:.1}s){}",
            j.id,
            s.ssim,
            s.mismatch,
            s.missing,
            t0.elapsed().as_secs_f64(),
            if s.note.is_empty() { String::new() } else { format!("  [{}]", s.note) }
        );
        scores.insert(j.id.clone(), s);
    }
    write_reports(&out_dir, &scores);
    ExitCode::SUCCESS
}

fn load(p: &Path) -> Option<RgbaImage> {
    image::open(p).ok().map(|i| i.to_rgba8())
}

fn score(j: &Job, out_dir: &Path, a_png: &Path, ledger: &Path, mut note: String) -> Score {
    let (missing, top) = parse_ledger(ledger);
    let refp = out_dir.join(format!("{}.ref.png", j.id));
    let blank = || RgbaImage::from_pixel(j.w, j.h, Rgba([255, 255, 255, 255]));
    let a = load(a_png).unwrap_or_else(|| {
        if note.is_empty() {
            note = "no aether frame".into();
        }
        blank()
    });
    let Some(r) = load(&refp) else {
        return Score { ssim: 0.0, mismatch: 100.0, missing, top, note: "no reference frame".into() };
    };
    let (w, h) = (r.width(), r.height());
    // Compare on the reference's canvas; anything Aether lacks counts as white.
    let px = |img: &RgbaImage, x: u32, y: u32| -> [u8; 3] {
        if x < img.width() && y < img.height() {
            let p = img.get_pixel(x, y).0;
            [p[0], p[1], p[2]]
        } else {
            [255, 255, 255]
        }
    };
    let mut diff = RgbaImage::new(w, h);
    let mut bad = 0u64;
    let mut ga = vec![0f64; (w * h) as usize];
    let mut gr = vec![0f64; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            let pa = px(&a, x, y);
            let pr = px(&r, x, y);
            let d = (0..3).map(|c| (pa[c] as i32 - pr[c] as i32).abs()).max().unwrap();
            if d > PIXEL_TOL {
                bad += 1;
            }
            let lum = |p: [u8; 3]| 0.299 * p[0] as f64 + 0.587 * p[1] as f64 + 0.114 * p[2] as f64;
            let i = (y * w + x) as usize;
            ga[i] = lum(pa);
            gr[i] = lum(pr);
            let base = 200.0 + gr[i] * 0.2;
            let t = (d as f64 / 128.0).min(1.0);
            let v = (base * (1.0 - t)) as u8;
            diff.put_pixel(x, y, Rgba([(base * (1.0 - t) + 255.0 * t) as u8, v, v, 255]));
        }
    }
    let _ = diff.save(out_dir.join(format!("{}.diff.png", j.id)));
    let ssim = ssim(&down2(&ga, w, h), &down2(&gr, w, h), w / 2, h / 2);
    Score { ssim, mismatch: 100.0 * bad as f64 / (w * h) as f64, missing, top, note }
}

fn down2(g: &[f64], w: u32, h: u32) -> Vec<f64> {
    let (w2, h2) = (w / 2, h / 2);
    let mut o = vec![0f64; (w2 * h2) as usize];
    for y in 0..h2 {
        for x in 0..w2 {
            let i = |xx: u32, yy: u32| g[(yy * w + xx) as usize];
            o[(y * w2 + x) as usize] =
                (i(2 * x, 2 * y) + i(2 * x + 1, 2 * y) + i(2 * x, 2 * y + 1) + i(2 * x + 1, 2 * y + 1)) / 4.0;
        }
    }
    o
}

/// Mean SSIM over 8x8 windows, stride 4 (Wang et al. 2004 constants).
fn ssim(a: &[f64], b: &[f64], w: u32, h: u32) -> f64 {
    const C1: f64 = 6.5025; // (0.01*255)^2
    const C2: f64 = 58.5225; // (0.03*255)^2
    let (mut sum, mut n) = (0.0, 0u64);
    let mut y = 0;
    while y + 8 <= h {
        let mut x = 0;
        while x + 8 <= w {
            let (mut ma, mut mb) = (0.0, 0.0);
            for yy in y..y + 8 {
                for xx in x..x + 8 {
                    let i = (yy * w + xx) as usize;
                    ma += a[i];
                    mb += b[i];
                }
            }
            ma /= 64.0;
            mb /= 64.0;
            let (mut va, mut vb, mut cov) = (0.0, 0.0, 0.0);
            for yy in y..y + 8 {
                for xx in x..x + 8 {
                    let i = (yy * w + xx) as usize;
                    let (da, db) = (a[i] - ma, b[i] - mb);
                    va += da * da;
                    vb += db * db;
                    cov += da * db;
                }
            }
            va /= 63.0;
            vb /= 63.0;
            cov /= 63.0;
            sum += ((2.0 * ma * mb + C1) * (2.0 * cov + C2)) / ((ma * ma + mb * mb + C1) * (va + vb + C2));
            n += 1;
            x += 4;
        }
        y += 4;
    }
    if n == 0 { 0.0 } else { sum / n as f64 }
}

/// Parses Aether's ledger dump: `Category | API Name | Call Count` rows.
fn parse_ledger(p: &Path) -> (usize, Vec<(String, u64)>) {
    let text = std::fs::read_to_string(p).unwrap_or_default();
    let mut rows: Vec<(String, u64)> = text
        .lines()
        .skip_while(|l| !l.starts_with("----"))
        .skip(1)
        .filter_map(|l| {
            let f: Vec<&str> = l.split('|').map(str::trim).collect();
            (f.len() == 3).then(|| (format!("{}:{}", f[0], f[1]), f[2].parse().unwrap_or(0)))
        })
        .collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let n = rows.len();
    rows.truncate(3);
    (n, rows)
}

fn write_reports(out_dir: &Path, scores: &BTreeMap<String, Score>) {
    let mut pages = serde_json::Map::new();
    for (id, s) in scores {
        pages.insert(
            id.clone(),
            json!({ "ssim": round(s.ssim, 4), "mismatch": round(s.mismatch, 2), "missing": s.missing }),
        );
    }
    std::fs::write(
        out_dir.join("scores.json"),
        serde_json::to_string_pretty(&json!({ "pages": pages })).unwrap() + "\n",
    )
    .unwrap();

    let mut rows: Vec<(&String, &Score)> = scores.iter().collect();
    rows.sort_by(|a, b| a.1.ssim.partial_cmp(&b.1.ssim).unwrap());
    let mean = |f: &dyn Fn(&Score) -> f64| scores.values().map(f).sum::<f64>() / scores.len().max(1) as f64;
    let mut md = String::from("# AETHERSEE score — Aether vs Chromium\n\n");
    md.push_str(&format!(
        "{} frames. Mean SSIM **{:.3}**, mean pixel mismatch **{:.1}%**. Worst first.\n\n",
        scores.len(),
        mean(&|s| s.ssim),
        mean(&|s| s.mismatch)
    ));
    md.push_str("SSIM: 8x8-window structural similarity on 2x-downsampled greyscale (1 = identical). ");
    md.push_str(&format!("Mismatch: % of pixels whose largest channel differs by > {PIXEL_TOL}.\n\n"));
    md.push_str("| page | SSIM | mismatch % | missing APIs | top missing (count) | note |\n");
    md.push_str("|---|---:|---:|---:|---|---|\n");
    for (id, s) in rows {
        let top: Vec<String> = s.top.iter().map(|(n, c)| format!("`{n}` ({c})")).collect();
        md.push_str(&format!(
            "| {id} | {:.3} | {:.1} | {} | {} | {} |\n",
            s.ssim,
            s.mismatch,
            s.missing,
            top.join(", "),
            s.note
        ));
    }
    std::fs::write(out_dir.join("SCORE.md"), md).unwrap();
    println!("wrote {}", out_dir.join("SCORE.md").display());
}

fn round(v: f64, d: i32) -> f64 {
    let m = 10f64.powi(d);
    (v * m).round() / m
}

fn gate(args: &[String]) -> ExitCode {
    let r = root();
    let scores_p = r.join("out/scores.json");
    let base_p = r.join("baseline.json");
    let Some(scores) = std::fs::read_to_string(&scores_p).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok())
    else {
        eprintln!("gate: no {} — run `aether-audit run` first", scores_p.display());
        return ExitCode::from(2);
    };
    if args.iter().any(|a| a == "--write") {
        let mut pages = scores["pages"].as_object().cloned().unwrap_or_default();
        pages.retain(|k, _| !k.starts_with("live-"));
        let b = json!({
            "thresholds": { "ssim_drop": 0.02, "mismatch_rise": 2.0 },
            "pages": pages,
        });
        std::fs::write(&base_p, serde_json::to_string_pretty(&b).unwrap() + "\n").unwrap();
        println!("gate: wrote {}", base_p.display());
        return ExitCode::SUCCESS;
    }
    let Some(base) = std::fs::read_to_string(&base_p).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok())
    else {
        eprintln!("gate: no {}", base_p.display());
        return ExitCode::from(2);
    };
    let drop = base["thresholds"]["ssim_drop"].as_f64().unwrap_or(0.02);
    let rise = base["thresholds"]["mismatch_rise"].as_f64().unwrap_or(2.0);
    let mut failed = 0;
    for (id, b) in base["pages"].as_object().into_iter().flatten() {
        let s = &scores["pages"][id];
        let (bs, bm) = (b["ssim"].as_f64().unwrap_or(0.0), b["mismatch"].as_f64().unwrap_or(100.0));
        let verdict = match (s["ssim"].as_f64(), s["mismatch"].as_f64()) {
            (Some(ss), Some(_)) if ss < bs - drop => format!("REGRESSED ssim {bs:.3} -> {ss:.3}"),
            (Some(_), Some(sm)) if sm > bm + rise => format!("REGRESSED mismatch {bm:.1}% -> {sm:.1}%"),
            (Some(ss), Some(sm)) => format!("ok  ssim {bs:.3} -> {ss:.3}  mismatch {bm:.1}% -> {sm:.1}%"),
            _ => "MISSING from scores".into(),
        };
        if !verdict.starts_with("ok") {
            failed += 1;
        }
        println!("{id:<24} {verdict}");
    }
    if failed > 0 {
        println!("gate: RED — {failed} page(s) regressed past baseline (ssim_drop {drop}, mismatch_rise {rise})");
        ExitCode::from(1)
    } else {
        println!("gate: GREEN");
        ExitCode::SUCCESS
    }
}
