//! AETHERFONT (SR61) text oracle: 50 strings × 12/16/24/48 px, each in one face (or family list), laid out and
//! painted by Aether's own page path (`aether render`) and by Chromium (`tests/text_oracle/chromium.mjs`, the
//! EYES flags: no hinting, grayscale AA, device scale 1) from the SAME generated page.
//!
//! - widths: Aether's shaped width of each string (`fonts::lines::Advancer`, what layout measures with) against
//!   Chromium's laid-out width of the same text (`Range.getBoundingClientRect`), gate 0.5 px. Chromium's widths are
//!   frozen in `tests/data/text_oracle_widths.tsv`, so the width gate runs on a host without Chromium.
//! - rasters (live; needs Chromium + node, else SKIP): each row of the two screenshots is registered by the best
//!   integer shift within ±4 px, then every glyph Aether placed is scored by the mean |coverage difference| over
//!   its glyph box (coverage = 255 − grey, black text on white): the share of glyphs within 8 levels. Two pages
//!   per size: `sys` names the installed families (Chromium applies fontconfig's `hintslight` to system fonts —
//!   FreeType's light auto-hinter, which Aether does not implement, so this share is reported, not gated), and
//!   `web` loads the very face file Aether selected for each row through `@font-face` (data: URI; web fonts are
//!   rasterized unhinted by Chromium) — the like-for-like raster gate, ≥ 90 % within 8.
//!
//! `AETHER_TEXT_ORACLE_FREEZE=1` rewrites the frozen widths from a live run; `AETHER_TEXT_ORACLE_OUT=<dir>` keeps the
//! pages and both screenshots.

use aether::fonts::{self, lines::Advancer, FontSel};
use std::path::{Path, PathBuf};

const SIZES: [f32; 4] = [12.0, 16.0, 24.0, 48.0];
const LEFT: f32 = 10.0;

struct Row {
    family: String,
    weight: u16,
    italic: bool,
    text: String,
}

fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("\\u") {
        out.push_str(&rest[..i]);
        let hex = &rest[i + 2..(i + 6).min(rest.len())];
        match u32::from_str_radix(hex, 16).ok().and_then(char::from_u32) {
            Some(c) if hex.len() == 4 => {
                out.push(c);
                rest = &rest[i + 6..];
            }
            _ => {
                out.push_str("\\u");
                rest = &rest[i + 2..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn rows() -> Vec<Row> {
    let src = include_str!("text_oracle/strings.tsv");
    src.lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| {
            let f: Vec<&str> = l.splitn(4, '\t').collect();
            Row { family: f[0].to_string(), weight: f[1].parse().unwrap(), italic: f[2] == "italic", text: unescape(f[3]) }
        })
        .collect()
}

fn step(size: f32) -> f32 {
    (size * 1.6).ceil() + 8.0
}

fn page_dims(size: f32, n: usize) -> (u32, u32) {
    (1800, (20.0 + n as f32 * step(size)).ceil() as u32)
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The page of one size. `web`: every row's family is replaced by an `@font-face` family whose source is the
/// file of the face Aether selects for that row (descriptors = that face's own weight/style, so synthesis is
/// decided the same way by both engines).
fn page(size: f32, rows: &[Row], web: bool) -> String {
    use base64::Engine as _;
    let mut faces: Vec<(String, String)> = Vec::new(); // (face key, css family)
    let mut css = String::new();
    let mut fam_of = Vec::new();
    for r in rows {
        if !web {
            fam_of.push(r.family.clone());
            continue;
        }
        let f = fonts::face(&sel_of(r)).expect("a face");
        let d = fonts::db::db();
        let info = d
            .faces
            .iter()
            .find(|i| i.families.first() == Some(&f.family) && i.style == f.style)
            .expect("the face's file");
        let key = format!("{}#{}", info.path.display(), info.index);
        let name = match faces.iter().find(|(k, _)| *k == key) {
            Some((_, n)) => n.clone(),
            None => {
                let n = format!("F{}", faces.len());
                let b64 = base64::engine::general_purpose::STANDARD.encode(std::fs::read(&info.path).unwrap());
                let st = info.style;
                css.push_str(&format!(
                    "@font-face{{font-family:{n};src:url(data:font/ttf;base64,{b64});font-weight:{};font-style:{}}}\n",
                    st.weight,
                    if st.slant == fonts::db::Slant::Normal { "normal" } else { "italic" }
                ));
                faces.push((key, n.clone()));
                n
            }
        };
        fam_of.push(name);
    }
    let mut h = format!("<!DOCTYPE html><html><head><meta charset=\"utf-8\"><style>{css}html,body{{margin:0;background:#fff;color:#000}}.row{{position:absolute;left:10px;white-space:pre;line-height:normal}}</style></head><body>\n");
    for (i, r) in rows.iter().enumerate() {
        let top = 10.0 + i as f32 * step(size);
        h.push_str(&format!(
            "<div class=\"row\" style=\"top:{top}px;font-family:{};font-size:{size}px;font-weight:{};font-style:{}\">{}</div>\n",
            fam_of[i].replace('"', "&quot;"),
            r.weight,
            if r.italic { "italic" } else { "normal" },
            esc(&r.text)
        ));
    }
    h.push_str("</body></html>\n");
    h
}

fn sel_of(r: &Row) -> FontSel {
    let list = fonts::parse_family_list(&r.family).expect("family list");
    FontSel::new(fonts::intern_family_list(list), r.weight, r.italic)
}

fn aether_width(r: &Row, size: f32) -> f32 {
    Advancer::new(sel_of(r), size, 0.0).str(&r.text)
}

fn widths_file() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/text_oracle_widths.tsv")
}

/// (size, row index) → Chromium width, from the frozen file.
fn frozen() -> Option<Vec<(f32, usize, f32)>> {
    let s = std::fs::read_to_string(widths_file()).ok()?;
    Some(
        s.lines()
            .filter(|l| !l.starts_with('#'))
            .filter_map(|l| {
                let f: Vec<&str> = l.split('\t').collect();
                Some((f.first()?.parse().ok()?, f.get(1)?.parse().ok()?, f.get(2)?.parse().ok()?))
            })
            .collect(),
    )
}

#[test]
fn text_widths_vs_chromium_frozen() {
    let rows = rows();
    assert_eq!(rows.len(), 50);
    let Some(fz) = frozen() else {
        println!("SKIP: no frozen Chromium widths (run the live oracle with AETHER_TEXT_ORACLE_FREEZE=1)");
        return;
    };
    if fonts::db::db().faces.is_empty() {
        println!("SKIP: no fonts on this host");
        return;
    }
    let mut worst = 0.0f32;
    let mut bad = Vec::new();
    for &(size, i, cw) in &fz {
        let aw = aether_width(&rows[i], size);
        let d = (aw - cw).abs();
        worst = worst.max(d);
        if d > 0.5 {
            bad.push(format!("{size}px #{i} {:?}: aether {aw:.3} chromium {cw:.3}", rows[i].text));
        }
    }
    println!("text oracle widths: {}/{} within 0.5 px, worst {worst:.4} px", fz.len() - bad.len(), fz.len());
    assert!(bad.is_empty(), "widths off by more than 0.5 px:\n{}", bad.join("\n"));
}

fn rgba_of(path: &Path) -> (u32, u32, Vec<u8>) {
    let img = pixel_core::decode(&std::fs::read(path).unwrap()).unwrap();
    (img.width, img.height, img.rgba)
}

fn cov(img: &(u32, u32, Vec<u8>), x: i32, y: i32) -> i32 {
    if x < 0 || y < 0 || x >= img.0 as i32 || y >= img.1 as i32 {
        return 0;
    }
    let o = ((y as u32 * img.0 + x as u32) * 4) as usize;
    let p = &img.2[o..o + 3];
    255 - ((p[0] as i32 * 299 + p[1] as i32 * 587 + p[2] as i32 * 114) / 1000)
}

fn chromium_available() -> bool {
    Path::new("/opt/pw-browsers").exists()
        && std::process::Command::new("node").arg("--version").output().is_ok_and(|o| o.status.success())
}

/// Scores one row: registration within ±4 px, then each glyph Aether placed → (glyphs within 8, glyphs).
fn score_row(r: &Row, size: f32, top: f32, width: f32, ours: &(u32, u32, Vec<u8>), chrome: &(u32, u32, Vec<u8>)) -> (usize, usize) {
    let t = top.round() as i32;
    let band = (t - 4, t + step(size) as i32 - 4);
    let mut best = (i64::MAX, 0, 0);
    for dy in -4..=4 {
        for dx in -4..=4 {
            let mut e: i64 = 0;
            for y in band.0..band.1 {
                for x in 0..(LEFT as i32 + width as i32 + 20).min(chrome.0 as i32) {
                    e += (cov(ours, x + dx, y + dy) - cov(chrome, x, y)).abs() as i64;
                }
            }
            if e < best.0 {
                best = (e, dx, dy);
            }
        }
    }
    let (_, dx, dy) = best;
    let adv = Advancer::new(sel_of(r), size, 0.0);
    let Some(face) = fonts::face(&adv.sel) else { return (0, 0) };
    let baseline = (top + fonts::baseline_offset(face, size, 0.0)).round();
    let (mut within, mut total) = (0, 0);
    for (g, x, gy) in adv.place(&r.text) {
        let (ix, sub) = fonts::raster::split_x(LEFT + x);
        let Some(m) = fonts::raster::glyph(g.face, g.gid, size, sub) else { continue };
        let by = (baseline + gy).round() as i32;
        let mut err = 0i64;
        for yy in 0..m.h as i32 {
            for xx in 0..m.w as i32 {
                let (px, py) = (ix + m.left + xx, by + m.top + yy);
                err += (cov(ours, px + dx, py + dy) - cov(chrome, px, py)).abs() as i64;
            }
        }
        total += 1;
        if err as f64 / ((m.w * m.h) as f64).max(1.0) <= 8.0 {
            within += 1;
        }
    }
    (within, total)
}

fn parse_nums(line: &str, key: &str) -> Vec<f32> {
    line.split(&format!("\"{key}\":"))
        .skip(1)
        .map(|s| s.split([',', '}']).next().unwrap().parse::<f32>().unwrap())
        .collect()
}

#[test]
fn text_rasters_and_widths_vs_chromium_live() {
    if !chromium_available() || std::env::var_os("AETHER_TEXT_ORACLE_SKIP").is_some() {
        println!("SKIP: Chromium/node not available");
        return;
    }
    if fonts::db::db().faces.is_empty() {
        println!("SKIP: no fonts on this host");
        return;
    }
    let rows = rows();
    let dir = std::env::var_os("AETHER_TEXT_ORACLE_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join(format!("aether-text-oracle-{}", std::process::id())));
    std::fs::create_dir_all(&dir).unwrap();
    let variants = ["sys", "web"];
    let mut jobs = String::new();
    for v in variants {
        for &size in &SIZES {
            let (w, h) = page_dims(size, rows.len());
            let p = dir.join(format!("{v}-{size}.html"));
            std::fs::write(&p, page(size, &rows, v == "web")).unwrap();
            jobs.push_str(&format!("{}\t{}\t{w}\t{h}\n", p.display(), dir.join(format!("chromium-{v}-{size}.png")).display()));
            let out = std::process::Command::new(env!("CARGO_BIN_EXE_aether"))
                .args(["render", "--html"])
                .arg(&p)
                .args(["--width", &w.to_string(), "--height", &h.to_string(), "--out"])
                .arg(dir.join(format!("aether-{v}-{size}.png")))
                .output()
                .expect("aether render");
            assert!(out.status.success(), "aether render: {}", String::from_utf8_lossy(&out.stderr));
        }
    }
    std::fs::write(dir.join("jobs.tsv"), &jobs).unwrap();
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/text_oracle/chromium.mjs");
    let out = std::process::Command::new("node")
        .arg(&script)
        .arg(dir.join("jobs.tsv"))
        .env("PLAYWRIGHT_BROWSERS_PATH", "/opt/pw-browsers")
        .output()
        .expect("node");
    if !out.status.success() {
        println!("SKIP: the Chromium oracle did not run: {}", String::from_utf8_lossy(&out.stderr));
        return;
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().filter(|l| l.starts_with('{')).collect();
    assert_eq!(lines.len(), variants.len() * SIZES.len());
    let mut freeze = String::from("# Chromium laid-out widths (Range.getBoundingClientRect) of tests/text_oracle/strings.tsv: size\trow\twidth\n");
    let (mut wbad, mut wn, mut worst_w) = (0usize, 0usize, 0.0f32);
    let mut web = (0usize, 0usize);
    for (k, line) in lines.iter().enumerate() {
        let (v, size) = (variants[k / SIZES.len()], SIZES[k % SIZES.len()]);
        let ws = parse_nums(line, "w");
        let tops = parse_nums(line, "top");
        assert_eq!(ws.len(), rows.len());
        let chrome = rgba_of(&dir.join(format!("chromium-{v}-{size}.png")));
        let ours = rgba_of(&dir.join(format!("aether-{v}-{size}.png")));
        let (mut sw, mut st) = (0, 0);
        for (i, r) in rows.iter().enumerate() {
            if v == "sys" {
                freeze.push_str(&format!("{size}\t{i}\t{:.4}\n", ws[i]));
            }
            let aw = aether_width(r, size);
            let d = (aw - ws[i]).abs();
            worst_w = worst_w.max(d);
            wn += 1;
            if d > 0.5 {
                wbad += 1;
                println!("width {v} {size}px #{i} {:?}: aether {aw:.3} chromium {:.3}", r.text, ws[i]);
            }
            let (a, b) = score_row(r, size, tops[i], ws[i], &ours, &chrome);
            if std::env::var_os("AETHER_TEXT_ORACLE_ROWS").is_some() {
                println!("  {v} {size}px #{i:2} {a:3}/{b:3} {:?}", r.text);
            }
            sw += a;
            st += b;
        }
        println!("text oracle {v} {size}px: {sw}/{st} glyphs within 8 levels ({:.1} %)", 100.0 * sw as f64 / st.max(1) as f64);
        if v == "web" {
            web.0 += sw;
            web.1 += st;
        }
    }
    println!(
        "text oracle TOTAL: web rasters {}/{} within 8 ({:.1} %); widths {}/{wn} within 0.5 px, worst {worst_w:.4} px",
        web.0,
        web.1,
        100.0 * web.0 as f64 / web.1.max(1) as f64,
        wn - wbad
    );
    if std::env::var_os("AETHER_TEXT_ORACLE_FREEZE").is_some() {
        std::fs::write(widths_file(), freeze).unwrap();
    }
    if std::env::var_os("AETHER_TEXT_ORACLE_OUT").is_none() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    assert_eq!(wbad, 0, "every width within 0.5 px of Chromium's");
    assert!(web.0 as f64 >= 0.90 * web.1 as f64, "at least 90 % of web-font glyphs within 8 levels");
}
