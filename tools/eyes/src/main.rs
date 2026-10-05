//! EYES — lets an executor SEE what a program draws and measure it against a reference.
//!
//! A suite (`suites/<name>/suite.toml` + `suites/<name>/cases/*.toml`) lists cases. Each case has
//! a SUBJECT that produces a PNG and an ORACLE that produces the reference PNG; the SCORER diffs
//! them and writes `out/<suite>/<case>.{subject,ref,diff}.png`, `scores.json` and `SCORE.md`; the
//! GATE compares `scores.json` to `suites/<name>/baseline.json` and exits 1 on regression.
//!
//! Subject kinds: `cmd` (any command that writes `{out}`), `xvfb` (any GUI command on a private
//! Xvfb screen, grabbed from Xvfb's own `-fbdir` framebuffer after `wait_ms`), `png` (an existing
//! file). Oracle kinds: `chromium` (Playwright screenshot of `url`), `golden` (a checked-in PNG),
//! `cmd` (another command that writes `{ref}`).
//!
//! Scores: `ssim` — mean structural similarity over 8x8 windows of the 2x-downsampled greyscale
//! frames (1.0 = identical); `mismatch` — % of pixels whose largest channel differs by > 40.
//! `mask` rectangles `[x, y, w, h]` are blanked in both frames before scoring (clocks, cursors).

use image::{Rgba, RgbaImage};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

const PIXEL_TOL: i32 = 40;

#[derive(Deserialize, Default, Clone)]
struct Source {
    kind: Option<String>,
    run: Option<String>,
    path: Option<String>,
    url: Option<String>,
    /// An Aether-format missing-API ledger the subject wrote (count + top 3 go into SCORE.md).
    ledger: Option<String>,
    wait_ms: Option<u64>,
    timeout_s: Option<u64>,
}

impl Source {
    /// Field-wise overlay: `self` wins, `base` fills gaps — but only when the kinds agree,
    /// so a case that switches kind never inherits the default kind's fields.
    fn over(&self, base: &Source) -> Source {
        if self.kind.is_some() && base.kind.is_some() && self.kind != base.kind {
            return self.clone();
        }
        Source {
            kind: self.kind.clone().or(base.kind.clone()),
            run: self.run.clone().or(base.run.clone()),
            path: self.path.clone().or(base.path.clone()),
            url: self.url.clone().or(base.url.clone()),
            ledger: self.ledger.clone().or(base.ledger.clone()),
            wait_ms: self.wait_ms.or(base.wait_ms),
            timeout_s: self.timeout_s.or(base.timeout_s),
        }
    }
}

#[derive(Deserialize, Default)]
struct Defaults {
    width: Option<u32>,
    height: Option<u32>,
    #[serde(default)]
    subject: Source,
    #[serde(default)]
    oracle: Source,
}

#[derive(Deserialize, Default)]
struct Suite {
    #[serde(default)]
    build: Vec<String>,
    #[serde(default)]
    defaults: Defaults,
}

#[derive(Deserialize)]
struct CaseFile {
    #[serde(default)]
    case: Vec<CaseSpec>,
}

#[derive(Deserialize)]
struct CaseSpec {
    name: String,
    page: Option<String>,
    url: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    #[serde(default)]
    subject: Source,
    #[serde(default)]
    oracle: Source,
    #[serde(default)]
    mask: Vec<[u32; 4]>,
    #[serde(default)]
    optional: bool,
}

struct Case {
    name: String,
    optional: bool,
    page: Option<PathBuf>,
    w: u32,
    h: u32,
    subject: Source,
    oracle: Source,
    mask: Vec<[u32; 4]>,
    vars: BTreeMap<&'static str, String>,
}

impl Case {
    fn fill(&self, s: &str) -> String {
        let mut s = s.to_string();
        // Two passes so a placeholder may expand to another ({url} = "file://{page}").
        for _ in 0..2 {
            for (k, v) in &self.vars {
                s = s.replace(&format!("{{{k}}}"), v);
            }
        }
        s
    }
}

struct Score {
    ssim: f64,
    mismatch: f64,
    missing: Option<usize>,
    top: Vec<(String, u64)>,
    note: String,
}

fn tools_dir() -> PathBuf {
    std::env::var_os("EYES_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")))
}

fn repo_dir() -> PathBuf {
    tools_dir().join("../..").canonicalize().unwrap()
}

fn flag(args: &[String], f: &str) -> bool {
    args.iter().any(|a| a == f)
}

fn arg_val(args: &[String], f: &str) -> Option<String> {
    args.iter().position(|a| a == f).and_then(|i| args.get(i + 1).cloned())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = "usage: eyes run <suite> [--only SUBSTR] [--all] [--refresh-ref] [--no-build]\n       \
                 eyes gate <suite> [--accept]\n       eyes list";
    match (args.first().map(String::as_str), args.get(1)) {
        (Some("run"), Some(suite)) => run(suite, &args[2..]),
        (Some("gate"), Some(suite)) => gate(suite, flag(&args, "--accept")),
        (Some("list"), _) => {
            let mut v: Vec<String> = std::fs::read_dir(tools_dir().join("suites"))
                .map(|d| d.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_string()).collect())
                .unwrap_or_default();
            v.sort();
            println!("{}", v.join("\n"));
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("{usage}");
            ExitCode::from(2)
        }
    }
}

fn load_suite(name: &str, all: bool, only: Option<&str>) -> Result<(Suite, Vec<Case>), String> {
    let dir = tools_dir().join("suites").join(name);
    let suite: Suite = match std::fs::read_to_string(dir.join("suite.toml")) {
        Ok(t) => toml::from_str(&t).map_err(|e| format!("suite.toml: {e}"))?,
        Err(_) => Suite::default(),
    };
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir.join("cases"))
        .map_err(|e| format!("{}: {e}", dir.join("cases").display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    files.sort();
    let outdir = tools_dir().join("out").join(name);
    let mut cases = Vec::new();
    for f in files {
        let cf: CaseFile = toml::from_str(&std::fs::read_to_string(&f).unwrap())
            .map_err(|e| format!("{}: {e}", f.display()))?;
        for c in cf.case {
            if (c.optional && !all) || only.is_some_and(|o| !c.name.contains(o)) {
                continue;
            }
            let w = c.width.or(suite.defaults.width).unwrap_or(800);
            let h = c.height.or(suite.defaults.height).unwrap_or(600);
            let page = c.page.as_ref().map(|p| dir.join(p).canonicalize().unwrap_or_else(|_| dir.join(p)));
            let mut vars = BTreeMap::new();
            vars.insert("repo", repo_dir().display().to_string());
            vars.insert("suite", dir.display().to_string());
            vars.insert("outdir", outdir.display().to_string());
            vars.insert("case", c.name.clone());
            vars.insert("out", outdir.join(format!("{}.subject.png", c.name)).display().to_string());
            vars.insert("ref", outdir.join(format!("{}.ref.png", c.name)).display().to_string());
            vars.insert("w", w.to_string());
            vars.insert("h", h.to_string());
            vars.insert("page", page.as_ref().map(|p| p.display().to_string()).unwrap_or_default());
            vars.insert("url", c.url.clone().unwrap_or_default());
            cases.push(Case {
                name: c.name,
                optional: c.optional,
                page,
                w,
                h,
                subject: c.subject.over(&suite.defaults.subject),
                oracle: c.oracle.over(&suite.defaults.oracle),
                mask: c.mask,
                vars,
            });
        }
    }
    Ok((suite, cases))
}

fn sh(cmd: &str) -> Command {
    let mut c = Command::new("sh");
    c.arg("-c").arg(cmd);
    c
}

/// Waits for a child up to `timeout`; on expiry kills THAT pid (never by name).
fn wait_or_kill(child: &mut Child, timeout: Duration) -> Result<bool, String> {
    let t0 = Instant::now();
    loop {
        if let Some(st) = child.try_wait().map_err(|e| e.to_string())? {
            return Ok(st.success());
        }
        if t0.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("timeout after {}s", timeout.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn run(suite_name: &str, args: &[String]) -> ExitCode {
    let (suite, cases) = match load_suite(suite_name, flag(args, "--all"), arg_val(args, "--only").as_deref()) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("eyes: {e}");
            return ExitCode::from(2);
        }
    };
    let suite_dir = tools_dir().join("suites").join(suite_name);
    let outdir = tools_dir().join("out").join(suite_name);
    std::fs::create_dir_all(&outdir).unwrap();
    if !flag(args, "--no-build") {
        for b in &suite.build {
            println!("eyes: build: {b}");
            if !sh(b).current_dir(repo_dir()).status().is_ok_and(|s| s.success()) {
                eprintln!("eyes: build step failed");
                return ExitCode::from(2);
            }
        }
    }

    // 1. Oracles. Chromium jobs batch into one browser; a reference for a case with a local
    //    page is cached until the page changes (`--refresh-ref` forces a re-shoot).
    let refresh = flag(args, "--refresh-ref");
    let mut notes: BTreeMap<String, String> = BTreeMap::new();
    let mut tsv = String::new();
    for c in &cases {
        let refp = PathBuf::from(&c.vars["ref"]);
        match c.oracle.kind.as_deref() {
            Some("chromium") => {
                let fresh = !refresh
                    && c.page.as_ref().is_some_and(|p| match (std::fs::metadata(&refp), std::fs::metadata(p)) {
                        (Ok(r), Ok(s)) => r.modified().unwrap() >= s.modified().unwrap(),
                        _ => false,
                    });
                if !fresh {
                    let _ = std::fs::remove_file(&refp);
                    let url = c.fill(c.oracle.url.as_deref().unwrap_or("{url}"));
                    tsv.push_str(&format!("{url}\t{}\t{}\t{}\n", refp.display(), c.w, c.h));
                }
            }
            Some("golden") => {
                let src = suite_dir.join(c.fill(c.oracle.path.as_deref().unwrap_or("")));
                if let Err(e) = std::fs::copy(&src, &refp) {
                    notes.insert(c.name.clone(), format!("golden {}: {e}", src.display()));
                }
            }
            Some("cmd") => {
                let cmd = c.fill(c.oracle.run.as_deref().unwrap_or("false"));
                let _ = std::fs::remove_file(&refp);
                let r = sh(&cmd)
                    .current_dir(repo_dir())
                    .stdout(Stdio::null())
                    .spawn()
                    .map_err(|e| e.to_string())
                    .and_then(|mut ch| wait_or_kill(&mut ch, Duration::from_secs(c.oracle.timeout_s.unwrap_or(90))));
                if let Err(e) = r {
                    notes.insert(c.name.clone(), format!("oracle cmd: {e}"));
                }
            }
            other => {
                notes.insert(c.name.clone(), format!("unknown oracle kind {other:?}"));
            }
        }
    }
    if !tsv.is_empty() {
        let tsv_path = outdir.join("ref-jobs.tsv");
        std::fs::write(&tsv_path, &tsv).unwrap();
        let ok = Command::new("node")
            .arg(tools_dir().join("ref.mjs"))
            .arg(&tsv_path)
            .env(
                "PLAYWRIGHT_BROWSERS_PATH",
                std::env::var("PLAYWRIGHT_BROWSERS_PATH").unwrap_or("/opt/pw-browsers".into()),
            )
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            eprintln!("eyes: chromium oracle reported failures (above)");
        }
    }

    // 2. Subjects, then score.
    let mut scores: BTreeMap<String, Score> = BTreeMap::new();
    for c in &cases {
        let out = PathBuf::from(&c.vars["out"]);
        let _ = std::fs::remove_file(&out);
        let t0 = Instant::now();
        let r = match c.subject.kind.as_deref() {
            Some("cmd") => {
                let cmd = c.fill(c.subject.run.as_deref().unwrap_or("false"));
                sh(&cmd)
                    .current_dir(repo_dir())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .map_err(|e| e.to_string())
                    .and_then(|mut ch| wait_or_kill(&mut ch, Duration::from_secs(c.subject.timeout_s.unwrap_or(90))))
                    .map(|_| ())
            }
            Some("xvfb") => xvfb_subject(c, &out),
            Some("png") => {
                let src = suite_dir.join(c.fill(c.subject.path.as_deref().unwrap_or("")));
                std::fs::copy(&src, &out).map(|_| ()).map_err(|e| format!("{}: {e}", src.display()))
            }
            other => Err(format!("unknown subject kind {other:?}")),
        };
        if let Err(e) = r {
            notes.entry(c.name.clone()).or_insert(format!("subject: {e}"));
        }
        let s = score(c, &outdir, notes.remove(&c.name).unwrap_or_default());
        println!(
            "{:<26} ssim {:.3}  mismatch {:5.1}%  {}({:.1}s){}",
            c.name,
            s.ssim,
            s.mismatch,
            s.missing.map(|m| format!("missing {m:3}  ")).unwrap_or_default(),
            t0.elapsed().as_secs_f64(),
            if s.note.is_empty() { String::new() } else { format!("  [{}]", s.note) }
        );
        scores.insert(c.name.clone(), s);
    }
    let optional: Vec<&str> = cases.iter().filter(|c| c.optional).map(|c| c.name.as_str()).collect();
    write_reports(suite_name, &outdir, &scores, &optional, arg_val(args, "--only").is_some());
    ExitCode::SUCCESS
}

/// Runs a GUI command on a private Xvfb screen and grabs the screen from Xvfb's own
/// memory-mapped framebuffer file (`-fbdir`, XWD format) — no X client tools needed.
fn xvfb_subject(c: &Case, out: &Path) -> Result<(), String> {
    let fbdir = PathBuf::from(&c.vars["outdir"]).join(format!("{}.fb", c.name));
    let _ = std::fs::remove_dir_all(&fbdir);
    std::fs::create_dir_all(&fbdir).map_err(|e| e.to_string())?;
    let display = (90..200)
        .find(|n| {
            !Path::new(&format!("/tmp/.X11-unix/X{n}")).exists() && !Path::new(&format!("/tmp/.X{n}-lock")).exists()
        })
        .ok_or("no free X display")?;
    let mut xvfb = Command::new("Xvfb")
        .arg(format!(":{display}"))
        .args(["-screen", "0", &format!("{}x{}x24", c.w, c.h), "-nolisten", "tcp", "-fbdir"])
        .arg(&fbdir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Xvfb: {e}"))?;
    let sock = format!("/tmp/.X11-unix/X{display}");
    let t0 = Instant::now();
    while !Path::new(&sock).exists() && t0.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(50));
    }
    let result = (|| {
        let mut app = sh(&format!("exec {}", c.fill(c.subject.run.as_deref().unwrap_or("false"))))
            .env("DISPLAY", format!(":{display}"))
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(PathBuf::from(&c.vars["outdir"]).join(format!("{}.stderr.txt", c.name))).map_err(|e| e.to_string())?)
            .spawn()
            .map_err(|e| e.to_string())?;
        std::thread::sleep(Duration::from_millis(c.subject.wait_ms.unwrap_or(3000)));
        if let Ok(Some(st)) = app.try_wait() {
            return Err(format!("GUI process exited early ({st})"));
        }
        let grab = std::fs::read(fbdir.join("Xvfb_screen0")).map_err(|e| format!("framebuffer: {e}"));
        // `exec` made the shell BE the app, so this kills the app's own pid.
        let _ = Command::new("kill").arg("-TERM").arg(app.id().to_string()).status();
        let _ = wait_or_kill(&mut app, Duration::from_secs(5)); // SIGKILLs the same pid after 5 s
        decode_xwd(&grab?)?.save(out).map_err(|e| e.to_string())
    })();
    // Our own Xvfb pid: SIGTERM so it removes its lock and socket; SIGKILL only if it hangs.
    let _ = Command::new("kill").arg("-TERM").arg(xvfb.id().to_string()).status();
    let _ = wait_or_kill(&mut xvfb, Duration::from_secs(5));
    let _ = std::fs::remove_dir_all(&fbdir);
    result
}

/// Decodes a 32-bpp ZPixmap XWD (what Xvfb -fbdir writes) into RGBA.
fn decode_xwd(b: &[u8]) -> Result<RgbaImage, String> {
    if b.len() < 100 {
        return Err("short xwd".into());
    }
    let u = |i: usize| -> u32 { u32::from_be_bytes([b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]]) };
    let (header, format, w, h) = (u(0) as usize, u(2), u(4), u(5));
    let (byte_order, bpp, bpl) = (u(7), u(11) as usize, u(12) as usize);
    let (rm, gm, bm) = (u(14), u(15), u(16));
    let ncolors = u(19) as usize;
    if format != 2 || bpp != 32 || rm == 0 || gm == 0 || bm == 0 {
        return Err(format!("unsupported xwd (format {format}, {bpp} bpp)"));
    }
    let data = header + ncolors * 12;
    let mut img = RgbaImage::new(w, h);
    for y in 0..h as usize {
        for x in 0..w as usize {
            let o = data + y * bpl + x * 4;
            let p = b.get(o..o + 4).ok_or("truncated xwd")?;
            let v = if byte_order == 0 {
                u32::from_le_bytes([p[0], p[1], p[2], p[3]])
            } else {
                u32::from_be_bytes([p[0], p[1], p[2], p[3]])
            };
            let ch = |m: u32| ((v & m) >> m.trailing_zeros()) as u8;
            img.put_pixel(x as u32, y as u32, Rgba([ch(rm), ch(gm), ch(bm), 255]));
        }
    }
    Ok(img)
}

fn load(p: &Path) -> Option<RgbaImage> {
    image::open(p).ok().map(|i| i.to_rgba8())
}

fn score(c: &Case, outdir: &Path, mut note: String) -> Score {
    let (missing, top) = match &c.subject.ledger {
        Some(l) => parse_ledger(Path::new(&c.fill(l))),
        None => (None, Vec::new()),
    };
    let blank = || RgbaImage::from_pixel(c.w, c.h, Rgba([255, 255, 255, 255]));
    let a = load(Path::new(&c.vars["out"])).unwrap_or_else(|| {
        if note.is_empty() {
            note = "no subject frame".into();
        }
        blank()
    });
    let Some(r) = load(Path::new(&c.vars["ref"])) else {
        if note.is_empty() {
            note = "no reference frame".into();
        }
        return Score { ssim: 0.0, mismatch: 100.0, missing, top, note };
    };
    let (w, h) = (r.width(), r.height());
    let masked =
        |x: u32, y: u32| c.mask.iter().any(|m| x >= m[0] && x < m[0] + m[2] && y >= m[1] && y < m[1] + m[3]);
    // Compare on the reference's canvas; anything the subject lacks counts as white.
    let px = |img: &RgbaImage, x: u32, y: u32| -> [u8; 3] {
        if !masked(x, y) && x < img.width() && y < img.height() {
            let p = img.get_pixel(x, y).0;
            [p[0], p[1], p[2]]
        } else {
            [255, 255, 255]
        }
    };
    let lum = |p: [u8; 3]| 0.299 * p[0] as f64 + 0.587 * p[1] as f64 + 0.114 * p[2] as f64;
    let mut diff = RgbaImage::new(w, h);
    let mut bad = 0u64;
    let mut ga = vec![0f64; (w * h) as usize];
    let mut gr = vec![0f64; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            let (pa, pr) = (px(&a, x, y), px(&r, x, y));
            let d = (0..3).map(|i| (pa[i] as i32 - pr[i] as i32).abs()).max().unwrap();
            if d > PIXEL_TOL {
                bad += 1;
            }
            let i = (y * w + x) as usize;
            ga[i] = lum(pa);
            gr[i] = lum(pr);
            // Heat: the reference faded to light grey, red where the frames differ; masks blue.
            let base = 200.0 + gr[i] * 0.2;
            let t = (d as f64 / 128.0).min(1.0);
            let v = (base * (1.0 - t)) as u8;
            let heat = if masked(x, y) {
                Rgba([180, 200, 255, 255])
            } else {
                Rgba([(base * (1.0 - t) + 255.0 * t) as u8, v, v, 255])
            };
            diff.put_pixel(x, y, heat);
        }
    }
    let _ = diff.save(outdir.join(format!("{}.diff.png", c.name)));
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

/// Parses an Aether ledger dump (`Category | API Name | Call Count` rows after a `----` line).
fn parse_ledger(p: &Path) -> (Option<usize>, Vec<(String, u64)>) {
    let Ok(text) = std::fs::read_to_string(p) else { return (None, Vec::new()) };
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
    (Some(n), rows)
}

fn write_reports(suite: &str, outdir: &Path, scores: &BTreeMap<String, Score>, optional: &[&str], merge: bool) {
    // An `--only` run updates its cases inside the last full run's scores, so the gate still
    // sees every case.
    let mut cases = if merge {
        std::fs::read_to_string(outdir.join("scores.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| v["cases"].as_object().cloned())
            .unwrap_or_default()
    } else {
        serde_json::Map::new()
    };
    for (id, s) in scores {
        let mut v = json!({ "ssim": round(s.ssim, 4), "mismatch": round(s.mismatch, 2) });
        if let Some(m) = s.missing {
            v["missing"] = json!(m);
        }
        if optional.contains(&id.as_str()) {
            v["optional"] = json!(true);
        }
        cases.insert(id.clone(), v);
    }
    std::fs::write(outdir.join("scores.json"), serde_json::to_string_pretty(&json!({ "cases": cases })).unwrap() + "\n")
        .unwrap();

    let mut rows: Vec<(&String, &Score)> = scores.iter().collect();
    rows.sort_by(|a, b| a.1.ssim.partial_cmp(&b.1.ssim).unwrap());
    let req: Vec<&Score> = scores.iter().filter(|(k, _)| !optional.contains(&k.as_str())).map(|(_, s)| s).collect();
    let n = req.len().max(1) as f64;
    let mut md = format!("# EYES score — suite `{suite}`\n\n");
    md.push_str(&format!(
        "{} cases ({} optional). Mean over required cases: SSIM **{:.3}**, pixel mismatch **{:.1}%**. Worst first.\n\n",
        scores.len(),
        scores.len() - req.len(),
        req.iter().map(|s| s.ssim).sum::<f64>() / n,
        req.iter().map(|s| s.mismatch).sum::<f64>() / n
    ));
    md.push_str("SSIM: 8x8-window structural similarity on 2x-downsampled greyscale (1 = identical). ");
    md.push_str(&format!("Mismatch: % of pixels whose largest channel differs by > {PIXEL_TOL}.\n\n"));
    md.push_str("| case | SSIM | mismatch % | missing APIs | top missing (count) | note |\n|---|---:|---:|---:|---|---|\n");
    for (id, s) in rows {
        let top: Vec<String> = s.top.iter().map(|(n, c)| format!("`{n}` ({c})")).collect();
        md.push_str(&format!(
            "| {id}{} | {:.3} | {:.1} | {} | {} | {} |\n",
            if optional.contains(&id.as_str()) { " (optional)" } else { "" },
            s.ssim,
            s.mismatch,
            s.missing.map(|m| m.to_string()).unwrap_or("-".into()),
            top.join(", "),
            s.note
        ));
    }
    std::fs::write(outdir.join("SCORE.md"), md).unwrap();
    println!("eyes: wrote {}", outdir.join("SCORE.md").display());
}

fn round(v: f64, d: i32) -> f64 {
    let m = 10f64.powi(d);
    (v * m).round() / m
}

fn gate(suite: &str, accept: bool) -> ExitCode {
    let scores_p = tools_dir().join("out").join(suite).join("scores.json");
    let base_p = tools_dir().join("suites").join(suite).join("baseline.json");
    let read = |p: &Path| std::fs::read_to_string(p).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok());
    let Some(scores) = read(&scores_p) else {
        eprintln!("gate: no {} — run `eyes run {suite}` first", scores_p.display());
        return ExitCode::from(2);
    };
    let old = read(&base_p);
    if accept {
        // Optional (network) cases never enter a baseline; thresholds survive a rewrite.
        let mut cases = scores["cases"].as_object().cloned().unwrap_or_default();
        cases.retain(|_, v| v.get("optional").is_none());
        let thresholds = old
            .as_ref()
            .map(|o| o["thresholds"].clone())
            .filter(|t| t.is_object())
            .unwrap_or(json!({ "ssim_drop": 0.02, "mismatch_rise": 2.0 }));
        let b = json!({ "thresholds": thresholds, "cases": cases });
        std::fs::write(&base_p, serde_json::to_string_pretty(&b).unwrap() + "\n").unwrap();
        println!("gate: accepted — wrote {}", base_p.display());
        return ExitCode::SUCCESS;
    }
    let Some(base) = old else {
        eprintln!("gate: no {} (first run: `run.sh {suite} --accept`)", base_p.display());
        return ExitCode::from(2);
    };
    let drop = base["thresholds"]["ssim_drop"].as_f64().unwrap_or(0.02);
    let rise = base["thresholds"]["mismatch_rise"].as_f64().unwrap_or(2.0);
    let mut failed = 0;
    for (id, b) in base["cases"].as_object().into_iter().flatten() {
        let s = &scores["cases"][id];
        let (bs, bm) = (b["ssim"].as_f64().unwrap_or(0.0), b["mismatch"].as_f64().unwrap_or(100.0));
        let verdict = match (s["ssim"].as_f64(), s["mismatch"].as_f64()) {
            (Some(ss), Some(_)) if ss < bs - drop => format!("REGRESSED ssim {bs:.3} -> {ss:.3}"),
            (Some(_), Some(sm)) if sm > bm + rise => format!("REGRESSED mismatch {bm:.1}% -> {sm:.1}%"),
            (Some(ss), Some(sm)) => format!("ok  ssim {bs:.3} -> {ss:.3}  mismatch {bm:.1}% -> {sm:.1}%"),
            _ => "MISSING from scores (run the whole suite, not --only)".into(),
        };
        if !verdict.starts_with("ok") {
            failed += 1;
        }
        println!("{id:<26} {verdict}");
    }
    if failed > 0 {
        println!(
            "gate: RED — {failed} case(s) regressed past {} (ssim_drop {drop}, mismatch_rise {rise})",
            base_p.display()
        );
        ExitCode::from(1)
    } else {
        println!("gate: GREEN ({suite})");
        ExitCode::SUCCESS
    }
}
