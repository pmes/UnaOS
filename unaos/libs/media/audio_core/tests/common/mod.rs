//! Test-time vector fetching: `vectors.txt` lists `<kind> <sha256> <local> <url>`; a missing file is
//! fetched with `curl` (through the session's HTTPS proxy) and its sha256 checked with `sha256sum`;
//! offline → the vector is skipped, never failed.
#![allow(dead_code)]
pub mod synth;
pub mod oggmux;
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct Vector {
    pub kind: String,
    pub sha: String,
    pub path: PathBuf,
    pub url: String,
}

pub fn root() -> PathBuf { Path::new(env!("CARGO_MANIFEST_DIR")).join("tests") }

pub fn vectors(kind: &str) -> Vec<Vector> {
    let txt = std::fs::read_to_string(root().join("vectors.txt")).unwrap();
    txt.lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| {
            let mut it = l.split_whitespace();
            Vector {
                kind: it.next().unwrap().into(),
                sha: it.next().unwrap().into(),
                path: root().join("vectors").join(it.next().unwrap()),
                url: it.next().unwrap().into(),
            }
        })
        .filter(|v| v.kind == kind)
        .collect()
}

fn sha256(p: &Path) -> Option<String> {
    let o = Command::new("sha256sum").arg(p).output().ok()?;
    String::from_utf8(o.stdout).ok()?.split_whitespace().next().map(String::from)
}

/// The vector's bytes, fetching it if needed; None = unavailable (offline): skip.
pub fn fetch(v: &Vector) -> Option<Vec<u8>> {
    if !v.path.exists() {
        std::fs::create_dir_all(v.path.parent().unwrap()).ok()?;
        let tmp = v.path.with_extension("part");
        let ok = Command::new("curl").args(["-sSfL", "-m", "300", "-o"]).arg(&tmp).arg(&v.url).status().map(|s| s.success()).unwrap_or(false);
        if !ok { let _ = std::fs::remove_file(&tmp); eprintln!("SKIP (offline?) {}", v.url); return None; }
        std::fs::rename(&tmp, &v.path).ok()?;
    }
    let got = sha256(&v.path)?;
    assert_eq!(got, v.sha, "sha256 of {}", v.path.display());
    std::fs::read(&v.path).ok()
}

pub fn big() -> bool { std::env::var("AUDIO_CORE_BIG").map(|v| v == "1").unwrap_or(false) }

// ── the Chromium oracle ─────────────────────────────────────────────────────────────────────────────

/// Chromium's decode of a file: planar f32.
pub struct Ref {
    pub ch: usize,
    pub frames: usize,
    pub rate: u32,
    pub data: Vec<Vec<f32>>,
}

fn read_ref(p: &Path) -> Option<Ref> {
    let b = std::fs::read(p).ok()?;
    let u = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as usize;
    let (ch, frames, rate) = (u(0), u(4), u(8) as u32);
    let mut data = vec![];
    for c in 0..ch {
        let o = 12 + c * frames * 4;
        data.push(b[o..o + frames * 4].chunks_exact(4).map(|x| f32::from_le_bytes(x.try_into().unwrap())).collect());
    }
    Some(Ref { ch, frames, rate, data })
}

/// Reference PCM for each (file, rate), produced by `oracle/chromium-oracle.cjs` in ONE browser session for
/// every job whose cached reference is missing. None for a job = no reference (no node/Chromium, or
/// Chromium refused the file): the caller skips it.
pub fn chromium(jobs: &[(PathBuf, u32)]) -> Vec<Option<Ref>> {
    let dir = root().join("vectors").join("oracle");
    std::fs::create_dir_all(&dir).unwrap();
    let out_of = |p: &Path, r: u32| {
        let rel = p.strip_prefix(root().join("vectors")).unwrap_or(p).to_string_lossy().replace(['/', ' '], "_");
        dir.join(format!("{}.r{}.f32", rel, r))
    };
    let missing: Vec<String> = jobs
        .iter()
        .filter(|(p, r)| !out_of(p, *r).exists())
        .map(|(p, r)| format!("{{\"in\":{:?},\"rate\":{},\"out\":{:?}}}", p.to_string_lossy(), r, out_of(p, *r).to_string_lossy()))
        .collect();
    if !missing.is_empty() && std::env::var("AUDIO_CORE_NO_ORACLE").is_err() {
        let jf = dir.join("jobs.json");
        std::fs::write(&jf, format!("[{}]", missing.join(","))).unwrap();
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("oracle").join("chromium-oracle.cjs");
        let st = Command::new("node")
            .arg(&script)
            .arg(&jf)
            .env("NODE_PATH", std::env::var("NODE_PATH").unwrap_or_else(|_| "/opt/node22/lib/node_modules".into()))
            .env("PLAYWRIGHT_BROWSERS_PATH", std::env::var("PLAYWRIGHT_BROWSERS_PATH").unwrap_or_else(|_| "/opt/pw-browsers".into()))
            .output();
        match st {
            Ok(o) => eprint!("{}", String::from_utf8_lossy(&o.stdout)),
            Err(e) => eprintln!("SKIP oracle: node unavailable ({})", e),
        }
    }
    jobs.iter().map(|(p, r)| read_ref(&out_of(p, *r))).collect()
}

/// How our interleaved decode compares with a reference.
#[derive(Debug, Default, Clone, Copy)]
pub struct Cmp {
    pub frames_ours: usize,
    pub frames_ref: usize,
    pub compared: usize,
    pub mismatches: usize,
    pub max_abs: f64,
    pub snr_db: f64,
}

pub fn compare(ours: &[f32], ch: usize, r: &Ref, offset: isize) -> Cmp {
    let fo = ours.len() / ch.max(1);
    let mut c = Cmp { frames_ours: fo, frames_ref: r.frames, ..Default::default() };
    if ch != r.ch { c.mismatches = usize::MAX; return c; }
    let (mut sig, mut err) = (0f64, 0f64);
    for i in 0..r.frames {
        let j = i as isize + offset;
        if j < 0 || j as usize >= fo { continue; }
        for k in 0..ch {
            let a = ours[j as usize * ch + k] as f64;
            let b = r.data[k][i] as f64;
            if a.to_bits() != b.to_bits() && a != b { c.mismatches += 1; }
            c.max_abs = c.max_abs.max((a - b).abs());
            sig += b * b;
            err += (a - b) * (a - b);
        }
        c.compared += 1;
    }
    c.snr_db = if err == 0.0 { f64::INFINITY } else { 10.0 * (sig / err).log10() };
    c
}
