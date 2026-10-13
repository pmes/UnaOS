// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! FONTSCRAFT (rmbp-ledger B640, R124) — the host proof of variable-font instancing, the OpenType features the
//! design language asks for, the fallback chain and the per-role atlases, over the faces vendored in the tree's
//! `fonts/` (Inter 4.1 variable, JetBrains Mono 2.304 variable, Noto Sans 2.015 variable; pinned in
//! `unaos/firmware/fonts.pins`). R78: no metal, no QEMU.
//!
//! * Always: `fvar` axes and normalization (with `avar`), `gvar` outlines and `HVAR` advances moving with `wght` and
//!   `opsz`, the default instance byte-identical to the unvaried face, `tnum` / `zero` / `kern`, the engine's chain
//!   Inter → Noto Sans → DejaVu Sans, `opsz` from the pt size, semibold for `bold`, tracking, and the atlases.
//! * Opt-in (`FONTSCRAFT_ORACLE_DIR` holding the release's `Inter.ttc` and `JetBrainsMono-Bold.ttf`): the variable
//!   faces instanced at the static faces' coordinates against those static faces — every printable ASCII glyph's
//!   advance and outline bounding box.
//! * `FONTSCRAFT_RAMP_PNG=<path>`: DESIGN-LANGUAGE §2.2's ramp (both looks) drawn by the engine at scale 2, as PNG.

mod common;

use font_core::ui::{Engine, Role, Style, FEAT_TNUM, FEAT_ZERO};
use font_core::{shape_fallback_with, Direction, Font, ShapeOptions};

fn tree_font(rel: &str) -> &'static [u8] {
    let p = format!("{}/../../../../fonts/{}", env!("CARGO_MANIFEST_DIR"), rel);
    let d = std::fs::read(&p).unwrap_or_else(|e| panic!("{p}: {e} (the vendored faces are part of the tree)"));
    Box::leak(d.into_boxed_slice())
}

const INTER_SHA: &str = "4989b125924991b90d05b2d16e0e388c48f7d5bb8b30539bbf9c755278d0ccaf";
const JB_SHA: &str = "662a196d58f1183bf2d77428b6d5283fe3f45161ab021bea4036bc98e5cac016";
const NOTO_SHA: &str = "fb53504de62e767a54695416123d68d67f71c9b69722175184ad56e7dd13547b";

fn inter() -> Font<'static> {
    let d = tree_font("inter/InterVariable.ttf");
    assert_eq!(common::sha256_hex(d), INTER_SHA, "fonts/inter/InterVariable.ttf is not the pinned file");
    Font::parse(d).unwrap()
}
fn jbmono() -> Font<'static> {
    let d = tree_font("jetbrains-mono/JetBrainsMono-Variable.ttf");
    assert_eq!(common::sha256_hex(d), JB_SHA, "fonts/jetbrains-mono is not the pinned file");
    Font::parse(d).unwrap()
}
fn noto() -> Font<'static> {
    let d = tree_font("noto-sans/NotoSans-Variable.ttf");
    assert_eq!(common::sha256_hex(d), NOTO_SHA, "fonts/noto-sans is not the pinned file");
    Font::parse(d).unwrap()
}
fn dejavu(name: &str) -> Option<Font<'static>> {
    let d = common::load(&format!("/usr/share/fonts/truetype/dejavu/{name}"), None)?;
    Font::parse(Box::leak(d.into_boxed_slice())).ok()
}

fn path_of(f: &Font, c: char) -> font_core::Path {
    f.glyph_path(f.glyph_index(c)).unwrap_or_default()
}

fn area(f: &Font, c: char) -> f64 {
    common::signed_area(&path_of(f, c).cmds).abs()
}

#[test]
fn axes_and_normalization() {
    let f = inter();
    let ax: Vec<_> = f.axes().iter().map(|a| (a.tag, a.min, a.default, a.max)).collect();
    assert_eq!(ax, vec![(*b"opsz", 14.0, 14.0, 32.0), (*b"wght", 100.0, 400.0, 900.0)]);
    assert!(f.avar.is_some() && f.gvar.is_some() && f.hvar.is_some() && f.layout_store.is_some(), "Inter carries avar, gvar, HVAR and a GDEF store");
    let mut g = f;
    for (w, want) in [(900.0, 1.0), (100.0, -1.0), (400.0, 0.0), (2000.0, 1.0), (10.0, -1.0)] {
        g.set_variation(*b"wght", w);
        assert_eq!(g.coords()[1], want, "wght {w}");
    }
    g.set_variation(*b"wght", 600.0);
    let c600 = g.coords()[1];
    assert!(c600 > 0.0 && c600 < 1.0, "600 normalizes inside (0, 1): {c600}");
    g.set_variation(*b"opsz", 32.0);
    assert_eq!(g.coords(), &[1.0, c600]);
    g.reset_variations();
    assert!(!g.is_instanced());
    let j = jbmono();
    assert_eq!(j.axes().iter().map(|a| (a.tag, a.min, a.max)).collect::<Vec<_>>(), vec![(*b"wght", 100.0, 800.0)]);
    let n = noto();
    assert_eq!(n.axes().iter().map(|a| (a.tag, a.min, a.max)).collect::<Vec<_>>(), vec![(*b"wght", 400.0, 700.0)]);
    eprintln!("FONTSCRAFT axes: inter wght600 -> {c600:.4} (avar)");
}

#[test]
fn instancing_moves_outlines_and_advances() {
    let f = inter();
    let regular = f.with_variations(&[(*b"wght", 400.0)]);
    let bold = f.with_variations(&[(*b"wght", 700.0)]);
    let thin = f.with_variations(&[(*b"wght", 100.0)]);
    // The default instance is the face as stored: same paths, same advances.
    for c in "Hamburgefonstiv0123é".chars() {
        assert_eq!(path_of(&regular, c), path_of(&f, c), "default instance moved {c}");
        assert_eq!(regular.advance(regular.glyph_index(c)), f.advance(f.glyph_index(c)));
    }
    let mut wider = 0;
    for c in "Hamburgefonstiv".chars() {
        let (gr, gb) = (f.glyph_index(c), bold.glyph_index(c));
        assert!(area(&bold, c) > area(&regular, c) && area(&regular, c) > area(&thin, c), "ink grows with wght: {c}");
        if bold.advance(gb) > regular.advance(gr) {
            wider += 1;
        }
    }
    assert!(wider >= 12, "HVAR widens the bold advances: {wider}/15");
    // A composite (é = e + acute) varies through its components and its offsets.
    let (pr, pb) = (path_of(&regular, 'é'), path_of(&bold, 'é'));
    assert!(pb.x_max - pb.x_min > pr.x_max - pr.x_min, "é widens at 700");
    assert!(pb.contours() == pr.contours() && pb.contours() >= 2);
    // opsz: the Display design (32) is tighter than the Text design (14) at the same weight.
    let text = f.with_variations(&[(*b"opsz", 14.0)]);
    let display = f.with_variations(&[(*b"opsz", 32.0)]);
    let w = |g: &Font| "Hamburgefonstiv".chars().map(|c| g.advance(g.glyph_index(c)) as u32).sum::<u32>();
    assert!(w(&display) < w(&text), "opsz 32 sets tighter: {} vs {}", w(&display), w(&text));
    // JetBrains Mono keeps its cell at every weight while the stems thicken.
    let j = jbmono();
    let (j4, j8) = (j.with_variations(&[(*b"wght", 400.0)]), j.with_variations(&[(*b"wght", 800.0)]));
    for c in "mn0iW".chars() {
        assert_eq!(j8.advance(j8.glyph_index(c)), j4.advance(j4.glyph_index(c)), "mono advance {c}");
        assert!(area(&j8, c) > area(&j4, c), "mono ink {c}");
    }
    // Noto Sans (the fallback) varies too.
    let n = noto();
    let n7 = n.with_variations(&[(*b"wght", 700.0)]);
    assert!(area(&n7, 'a') > area(&n, 'a'));
    eprintln!(
        "FONTSCRAFT instancing: inter 'n' adv 400={} 700={} area 100={:.0} 400={:.0} 700={:.0}; Hamburgefonstiv opsz14={} opsz32={}",
        regular.advance(regular.glyph_index('n')),
        bold.advance(bold.glyph_index('n')),
        area(&thin, 'n'),
        area(&regular, 'n'),
        area(&bold, 'n'),
        w(&text),
        w(&display)
    );
}

fn shaped(f: &Font, s: &str, feats: &[[u8; 4]]) -> Vec<font_core::GlyphPos> {
    shape_fallback_with(&[f], s, &ShapeOptions::default(), Direction::Ltr, feats).into_iter().map(|(_, g)| g).collect()
}

#[test]
fn features_tnum_zero_kern() {
    let f = inter();
    let plain = shaped(&f, "10", &[]);
    let tab = shaped(&f, "10", &[*b"tnum"]);
    assert!(plain[0].x_advance < plain[1].x_advance, "proportional 1 is narrower than 0");
    assert_eq!(tab[0].x_advance, tab[1].x_advance, "tnum: every figure on one advance");
    let z = shaped(&f, "0", &[*b"zero"]);
    assert_ne!(z[0].glyph, plain[1].glyph, "zero: the slashed zero");
    for wght in [400.0, 700.0] {
        let g = f.with_variations(&[(*b"wght", wght)]);
        let av = shaped(&g, "AV", &[]);
        let sum = g.advance(g.glyph_index('A')) as i32 + g.advance(g.glyph_index('V')) as i32;
        let got: i32 = av.iter().map(|p| p.x_advance).sum();
        assert!(got < sum, "kern AV at {wght}: {got} vs {sum}");
        eprintln!("FONTSCRAFT kern AV wght={wght}: {} units", got - sum);
    }
}

/// A visible base character (no space, control, mark or default ignorable): one cluster, one glyph.
fn plain_char(c: char) -> bool {
    !c.is_whitespace() && !c.is_control() && !font_core::ucd::is_mark(c) && !font_core::ucd::is_default_ignorable(c)
}

fn engine() -> (Engine<'static>, usize, Option<usize>) {
    let mut e = Engine::new(1 << 20);
    e.add_face("inter", Role::Sans, false, inter());
    e.add_face("jetbrains-mono", Role::Mono, false, jbmono());
    e.add_face("noto-sans", Role::Fallback, false, noto());
    let mut dv = None;
    if let Some(d) = dejavu("DejaVuSans.ttf") {
        e.add_face("dejavu-sans", Role::Fallback, false, d);
        dv = Some(3);
    }
    if let Some(d) = dejavu("DejaVuSans-Bold.ttf") {
        e.add_face("dejavu-sans-bold", Role::Fallback, true, d);
    }
    (e, 2, dv)
}

#[test]
fn engine_chain_opsz_weight_tracking_atlases() {
    let (mut e, noto_slot, dv_slot) = engine();
    e.set_px_per_pt(2.0);
    let (i, n) = (inter(), noto());
    // Inter -> Noto Sans: the first code point Noto has and Inter does not.
    let only_noto = n.unicode_map().into_iter().map(|(cp, _)| cp).find(|&cp| cp > 0x7F && char::from_u32(cp).is_some_and(|c| i.glyph_index(c) == 0 && plain_char(c))).unwrap();
    let s = char::from_u32(only_noto).unwrap().to_string();
    let st = Style::new(Role::Sans, false, 26.0);
    assert_eq!(e.placed(s.as_bytes(), st)[0].face as usize, noto_slot, "U+{only_noto:04X} falls to Noto Sans");
    // -> DejaVu Sans: a code point neither Inter nor Noto has.
    if let (Some(dslot), Some(d)) = (dv_slot, dejavu("DejaVuSans.ttf")) {
        let only_dv = d.unicode_map().into_iter().map(|(cp, _)| cp).find(|&cp| cp > 0x7F && char::from_u32(cp).is_some_and(|c| i.glyph_index(c) == 0 && n.glyph_index(c) == 0 && plain_char(c))).unwrap();
        let s = char::from_u32(only_dv).unwrap().to_string();
        assert_eq!(e.placed(s.as_bytes(), st)[0].face as usize, dslot, "U+{only_dv:04X} falls to DejaVu Sans");
        eprintln!("FONTSCRAFT chain: U+{only_noto:04X} -> noto-sans, U+{only_dv:04X} -> dejavu-sans");
    } else {
        eprintln!("SKIP chain's DejaVu step: DejaVu not installed");
    }
    // opsz from the size in pt (26 px at 2 px/pt = 13 pt -> opsz 14 clamp; 80 px = 40 pt -> 32 clamp), wght for bold.
    let body = e.primary_instance(&st).unwrap();
    assert_eq!(body.variation_value(*b"opsz"), Some(14.0));
    let disp = e.primary_instance(&Style::new(Role::Sans, false, 80.0).with_weight(700)).unwrap();
    assert_eq!(disp.variation_value(*b"opsz"), Some(32.0));
    let semi = e.primary_instance(&Style::new(Role::Sans, true, 26.0)).unwrap();
    let mid = e.primary_instance(&Style::new(Role::Sans, false, 40.0)).unwrap();
    let opsz20 = mid.variation_value(*b"opsz").unwrap();
    assert!(opsz20 > 14.0 && opsz20 < 32.0, "40 px at 2 px/pt (20 pt) lands inside the opsz axis: {opsz20}");
    let w600 = semi.coords()[1];
    let mut want = inter();
    want.set_variation(*b"wght", 600.0);
    assert_eq!(w600, want.coords()[1], "bold asks a variable slot for semibold (600)");
    let word = b"Hamburgefonstiv";
    let (wr, wb) = (e.measure(word, st), e.measure(word, Style::new(Role::Sans, true, 26.0)));
    assert!(wb > wr, "semibold measures wider: {wb} vs {wr}");
    let wt = e.measure(word, st.with_tracking(0.01));
    assert!((wt - (wr + 15.0 * 0.26)).abs() < 0.01, "tracking adds 0.01 em per glyph: {wt} vs {wr}");
    let nums = e.measure(b"1111", st.with_features(FEAT_TNUM));
    let zeros = e.measure(b"0000", st.with_features(FEAT_TNUM | FEAT_ZERO));
    assert!((nums - zeros).abs() < 0.01, "tnum figures share one advance: {nums} vs {zeros}");
    // Per-role atlases: three roles, three atlases; the body redrawn after a display size is all hits.
    let mut px = vec![0x00FF_FFFFu32; 1200 * 200];
    let roles = [st, Style::new(Role::Sans, true, 40.0).with_weight(600), Style::new(Role::Sans, false, 80.0).with_weight(700)];
    for r in roles {
        e.draw_0rgb(&mut px, 1200, 1200, 200, 4.0, 120.0, b"Sphinx of black quartz 0123", r, 0x1D1D1F);
    }
    assert!(e.stats.atlases >= 3, "{:?}", e.stats);
    let misses = e.stats.misses;
    e.draw_0rgb(&mut px, 1200, 1200, 200, 4.0, 120.0, b"Sphinx of black quartz 0123", st, 0x1D1D1F);
    assert_eq!(e.stats.misses, misses, "the body atlas survived the display draws");
    eprintln!("FONTSCRAFT engine: {:?}", e.stats);
    // A small budget drops whole atlases, least recently used first, and stays under its bound.
    let mut small = Engine::new(48 * 1024);
    small.add_face("inter", Role::Sans, false, inter());
    for k in 0..6 {
        let r = Style::new(Role::Sans, false, 24.0 + 12.0 * k as f32);
        small.draw_0rgb(&mut px, 1200, 1200, 200, 4.0, 150.0, b"ABCDEFGHIJKLMNOPQRSTUVWXYZ", r, 0);
        assert!(small.stats.cache_bytes <= 48 * 1024, "{:?}", small.stats);
    }
    assert!(small.stats.flushes > 0 && small.stats.evictions > 0, "{:?}", small.stats);
}

fn static_face(ttc: &'static [u8], full_name: &str) -> Option<Font<'static>> {
    (0..Font::face_count(ttc)).filter_map(|i| Font::parse_face(ttc, i).ok()).find(|f| font_core::name::values(f, 4).iter().any(|n| n == full_name))
}

/// The variable face at `coords` against a static face cut from the same masters: (glyphs, advances within 1 unit,
/// boxes within 1.5 units).
fn compare(var: &Font, stat: &Font) -> (usize, usize, usize) {
    let (mut n, mut adv_ok, mut box_ok) = (0, 0, 0);
    for c in (0x21u8..0x7F).map(|b| b as char) {
        let (gv, gs) = (var.glyph_index(c), stat.glyph_index(c));
        if gv == 0 || gs == 0 {
            continue;
        }
        n += 1;
        if (var.advance(gv) as i32 - stat.advance(gs) as i32).abs() <= 1 {
            adv_ok += 1;
        }
        let (a, b) = (var.glyph_path(gv).unwrap_or_default(), stat.glyph_path(gs).unwrap_or_default());
        if a.is_empty() && b.is_empty()
            || [(a.x_min, b.x_min), (a.y_min, b.y_min), (a.x_max, b.x_max), (a.y_max, b.y_max)].iter().all(|(p, q)| (p - q).abs() <= 1.5)
        {
            box_ok += 1;
        }
    }
    (n, adv_ok, box_ok)
}

#[test]
fn oracle_static_instances() {
    let Ok(dir) = std::env::var("FONTSCRAFT_ORACLE_DIR") else { return eprintln!("SKIP: FONTSCRAFT_ORACLE_DIR unset") };
    let ttc: &'static [u8] = Box::leak(std::fs::read(format!("{dir}/Inter.ttc")).unwrap().into_boxed_slice());
    let f = inter();
    for (name, settings) in [
        ("Inter Bold", [(*b"opsz", 14.0), (*b"wght", 700.0)]),
        ("Inter Light", [(*b"opsz", 14.0), (*b"wght", 300.0)]),
        ("Inter Display SemiBold", [(*b"opsz", 32.0), (*b"wght", 600.0)]),
        ("Inter Display Black", [(*b"opsz", 32.0), (*b"wght", 900.0)]),
    ] {
        let stat = static_face(ttc, name).unwrap_or_else(|| panic!("{name} not in Inter.ttc"));
        let (n, a, b) = compare(&f.with_variations(&settings), &stat);
        eprintln!("FONTSCRAFT oracle {name}: glyphs={n} advance_within_1={a} bbox_within_1.5={b}");
        assert_eq!(a, n, "{name}: every advance within a unit");
        assert!(b * 100 >= n * 95, "{name}: {b}/{n} boxes");
    }
    let jb: &'static [u8] = Box::leak(std::fs::read(format!("{dir}/JetBrainsMono-Bold.ttf")).unwrap().into_boxed_slice());
    let stat = Font::parse(jb).unwrap();
    let (n, a, b) = compare(&jbmono().with_variations(&[(*b"wght", 700.0)]), &stat);
    eprintln!("FONTSCRAFT oracle JetBrains Mono Bold: glyphs={n} advance_within_1={a} bbox_within_1.5={b}");
    assert_eq!(a, n);
    assert!(b * 100 >= n * 95, "{b}/{n}");
}

// ---------------------------------------------------------------------------------------------- the ramp, as PNG

/// DESIGN-LANGUAGE §2.2 (the same rows as the kernel's `theme::TYPE_RAMP`): (role, Una size/line, Softy size/line,
/// weight, tracking in basis points, mono).
const RAMP: [(&str, (u8, u8), (u8, u8), u16, i16, bool); 10] = [
    ("type.caption", (11, 14), (12, 16), 400, 50, false),
    ("type.footnote", (12, 16), (12, 16), 400, 25, false),
    ("type.body", (13, 18), (14, 20), 400, 0, false),
    ("type.body-strong", (13, 18), (14, 20), 600, 0, false),
    ("type.headline", (15, 20), (16, 22), 600, -25, false),
    ("type.title3", (17, 22), (18, 24), 600, -50, false),
    ("type.title2", (20, 26), (20, 28), 600, -75, false),
    ("type.title1", (26, 32), (28, 36), 700, -100, false),
    ("type.display", (40, 48), (40, 52), 700, -150, false),
    ("type.mono", (12, 18), (13, 20), 400, 0, true),
];

fn crc32(d: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in d {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
    }
    !c
}

fn png(w: usize, h: usize, px: &[u32]) -> Vec<u8> {
    let mut raw = Vec::with_capacity(h * (1 + 3 * w));
    for y in 0..h {
        raw.push(0);
        for x in 0..w {
            let p = px[y * w + x];
            raw.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8]);
        }
    }
    let mut z = vec![0x78, 0x01];
    let chunks: Vec<&[u8]> = raw.chunks(65535).collect();
    for (k, c) in chunks.iter().enumerate() {
        z.push((k + 1 == chunks.len()) as u8);
        z.extend_from_slice(&(c.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(c.len() as u16)).to_le_bytes());
        z.extend_from_slice(c);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut chunk = |t: &[u8; 4], data: &[u8]| {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut c = t.to_vec();
        c.extend_from_slice(data);
        out.extend_from_slice(&c);
        out.extend_from_slice(&crc32(&c).to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(b"IHDR", &ihdr);
    chunk(b"IDAT", &z);
    chunk(b"IEND", &[]);
    out
}

#[test]
fn ramp_png() {
    const SCALE: f32 = 2.0;
    const W: usize = 1800;
    let (mut e, _, _) = engine();
    e.set_px_per_pt(SCALE);
    let (bg, ink, dim) = (0x00F5_F5F7u32, 0x001D_1D1Fu32, 0x006E_6E73u32);
    let pad = (24.0 * SCALE) as usize;
    let mut rows: Vec<(usize, Style, String, f32)> = Vec::new(); // (line px, style, text, label size)
    for (look, softy) in [("Una", false), ("Softy", true)] {
        rows.push(((24.0 * SCALE) as usize, Style::new(Role::Sans, true, 13.0 * SCALE), format!("{look} — base {} pt", if softy { 14 } else { 13 }), 0.0));
        for (name, una, sft, wght, bp, mono) in RAMP {
            let (size, line) = if softy { sft } else { una };
            let st = Style::new(if mono { Role::Mono } else { Role::Sans }, false, size as f32 * SCALE).with_weight(wght).with_tracking(bp as f32 / 10_000.0);
            let sample = if mono { "fn main() { let x = 0x1F; } // 0O 1lI" } else if size >= 26 { "Sphinx of black quartz" } else { "Sphinx of black quartz, judge my vow — 0123456789" };
            rows.push(((line as f32 * SCALE) as usize, st, format!("{name}  {size}/{line}  {wght}|{sample}"), 11.0 * SCALE));
        }
        rows.push(((18.0 * SCALE) as usize, Style::new(Role::Sans, false, 13.0 * SCALE).with_features(FEAT_TNUM | FEAT_ZERO), String::from("tnum+zero|12:00:10  1,111.00  0.10 — Ελληνικά Кириллица Tiếng Việt"), 11.0 * SCALE));
        rows.push(((16.0 * SCALE) as usize, Style::new(Role::Sans, false, 13.0 * SCALE), String::new(), 0.0));
    }
    let h = pad * 2 + rows.iter().map(|r| r.0).sum::<usize>();
    let mut px = vec![bg; W * h];
    let mut y = pad;
    let label_x = pad as f32;
    let text_x = (pad + (230.0 * SCALE) as usize) as f32;
    let mut inked_rows = 0;
    for (line, st, txt, lsz) in &rows {
        let (label, body) = match txt.split_once('|') {
            Some((l, b)) => (l, b),
            None => ("", txt.as_str()),
        };
        if !body.is_empty() {
            let base = y as f32 + e.baseline_in_line(st.role, st.size, *line as f32);
            let before = px.iter().filter(|&&p| p != bg).count();
            let x = if label.is_empty() { label_x } else { text_x };
            e.draw_0rgb(&mut px, W, W, h, x, base, body.as_bytes(), *st, ink);
            if !label.is_empty() {
                e.draw_0rgb(&mut px, W, W, h, label_x, base, label.as_bytes(), Style::new(Role::Mono, false, *lsz), dim);
            }
            if px.iter().filter(|&&p| p != bg).count() > before {
                inked_rows += 1;
            }
        }
        y += line;
    }
    let want = rows.iter().filter(|r| !r.2.is_empty()).count();
    assert_eq!(inked_rows, want, "every ramp row put ink on the canvas");
    eprintln!("FONTSCRAFT ramp: {W}x{h} rows={want} {:?}", e.stats);
    if let Ok(p) = std::env::var("FONTSCRAFT_RAMP_PNG") {
        std::fs::write(&p, png(W, h, &px)).unwrap();
        eprintln!("FONTSCRAFT ramp written: {p}");
    }
}

// ------------------------------------------------------------------------------------------------ FONTS2 (B650)

/// FONTS2 (rmbp-ledger B650): `MVAR` moves the metrics of an instance — the numbers are fontTools 4.66.1's
/// instancer on the pinned files (docs/dev/evidence/fonts2/RESUME.md): Inter's underline (`undo`, `unds`) at `wght`
/// 400 / 700 / 900, Noto Sans' x-height (`xhgt`) 536 → 546. No vendored face carries `hasc`/`hdsc`/`hlgp`, so the
/// line box is the same at every weight, and the engine's line metrics are those of the REGULAR instance.
#[test]
fn mvar_metrics_move_with_the_instance() {
    let f = inter();
    let m = f.mvar.expect("Inter 4.1 carries MVAR");
    let tags: Vec<String> = m.tags().iter().map(|t| String::from_utf8_lossy(t).into_owned()).collect();
    assert_eq!(tags, ["stro", "strs", "undo", "unds", "xhgt"]);
    assert!(!m.has(*b"hasc") && !m.has(*b"hdsc") && !m.has(*b"hlgp"));
    assert_eq!(f.metric_delta(*b"undo"), 0.0, "the default instance has no delta");
    let at = |w: f32| f.with_variations(&[(*b"wght", w)]);
    assert_eq!(at(400.0).underline(), Some((-348, 140)));
    assert_eq!(at(700.0).underline(), Some((-323, 190)));
    assert_eq!(at(900.0).underline(), Some((-302, 232)));
    for w in [100.0, 400.0, 700.0, 900.0] {
        let g = at(w);
        assert_eq!(g.x_height(), Some(1118), "Inter xh at {w}");
        assert_eq!(g.line_metrics(), f.line_metrics(), "Inter line box at {w}");
    }
    let n = noto();
    assert_eq!(n.mvar.map(|m| m.record_count()), Some(2));
    assert_eq!(n.with_variations(&[(*b"wght", 400.0)]).x_height(), Some(536));
    assert_eq!(n.with_variations(&[(*b"wght", 700.0)]).x_height(), Some(546));
    assert_eq!(n.with_variations(&[(*b"wght", 900.0)]).x_height(), Some(546));
    assert!(jbmono().mvar.is_none(), "JetBrains Mono carries no MVAR");
    let (e, _, _) = engine();
    let lm = e.line_metrics(Role::Sans, 26.0).unwrap();
    let (a, d, g) = f.line_metrics();
    let s = 26.0 / f.units_per_em as f32;
    assert_eq!(lm, (a as f32 * s, -(d as f32) * s, (a as f32 - d as f32 + g as f32) * s));
}

/// FONTS2 (B650): the GSUB/GPOS FeatureVariations reader (OpenType 1.9, "OpenType Layout Common Table Formats":
/// FeatureVariations, ConditionSet, Condition Table Format 1, FeatureTableSubstitution) on the synthetic probe — the
/// vendored faces carry none (their GSUB/GPOS are version 1.0), which this also pins.
#[test]
fn gsub_feature_variations_probe() {
    use font_core::layout::{Gsub, FEATURE_VARIATIONS_PROBE};
    let g = Gsub::parse(FEATURE_VARIATIONS_PROBE).expect("the probe parses").0;
    assert_eq!(g.feature_variation_count(), 1);
    for (c, sel, want) in [(0.0, None, 0u16), (0.25, None, 0), (-0.5, None, 0), (0.5, Some(0), 1), (1.0, Some(0), 1)] {
        assert_eq!(g.selected_variation(&[c]), sel, "selected at {c}");
        assert_eq!(g.feature_lookups_at(&[*b"DFLT"], *b"ss01", &[c]), Some(vec![want]), "ss01 at {c}");
    }
    assert_eq!(g.selected_variation(&[]), None, "a missing axis reads 0");
    assert_eq!(g.feature_lookups(&[*b"DFLT"], *b"ss01"), Some(vec![0]), "the default instance's view");
    for f in [inter(), jbmono(), noto()] {
        assert_eq!(f.gsub.map_or(0, |t| t.0.feature_variation_count()), 0);
        assert_eq!(f.gpos.map_or(0, |t| t.0.feature_variation_count()), 0);
    }
}
