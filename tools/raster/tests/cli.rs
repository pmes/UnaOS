// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! RASTERCLI (LEDGER SR233) KATs: drive the built `raster` binary (`CARGO_BIN_EXE_raster`, the tools/play-check and
//! tools/una-card way) with `--do` scripts written under cargo's per-target scratch dir, read its one JSON line, and
//! decode its PNG exports with pixel_core.

use std::path::PathBuf;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_raster");

fn scratch(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("raster-cli");
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

/// Run `script` (written to `<name>.do`); `(exit code, the JSON line)`.
fn run(name: &str, script: &str) -> (i32, String) {
    let p = scratch(&format!("{name}.do"));
    std::fs::write(&p, script).unwrap();
    let o = Command::new(BIN).arg("--do").arg(&p).output().unwrap();
    let out = String::from_utf8(o.stdout).unwrap();
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 1, "exactly one status line: {out:?} stderr {}", String::from_utf8_lossy(&o.stderr));
    (o.status.code().unwrap(), lines[0].to_string())
}

/// The raw value of `"key":` in the flat JSON line (strings without their quotes).
fn field<'a>(json: &'a str, key: &str) -> &'a str {
    let pat = format!("\"{key}\":");
    let at = json.find(&pat).unwrap_or_else(|| panic!("{key} in {json}")) + pat.len();
    let rest = &json[at..];
    if let Some(r) = rest.strip_prefix('"') {
        &r[..r.find('"').unwrap()]
    } else {
        &rest[..rest.find([',', '}']).unwrap()]
    }
}

fn png(path: &PathBuf) -> pixel_core::Image {
    pixel_core::decode_png(&std::fs::read(path).unwrap()).unwrap()
}

fn px(img: &pixel_core::Image, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * img.width + x) * 4) as usize;
    [img.rgba[i], img.rgba[i + 1], img.rgba[i + 2], img.rgba[i + 3]]
}

#[test]
fn new_reports_size() {
    let (code, j) = run("new", "new 8 8\n");
    assert_eq!(code, 0, "{j}");
    assert_eq!(field(&j, "ok"), "true");
    assert_eq!((field(&j, "width"), field(&j, "height"), field(&j, "layers")), ("8", "8", "1"));
    assert_eq!(field(&j, "error"), "");
}

#[test]
fn fill_rect_exports_red() {
    let out = scratch("fill.png");
    let s = format!("new 8 8\nlayer add top\nselect rect 2 2 3 3\nfill 1 ff0000ff\nexport {}\n", out.display());
    let (code, j) = run("fill", &s);
    assert_eq!(code, 0, "{j}");
    assert_eq!((field(&j, "layers"), field(&j, "selection"), field(&j, "verbs")), ("2", "rect", "5"));
    let img = png(&out);
    assert_eq!((img.width, img.height), (8, 8));
    for y in 0..8 {
        for x in 0..8 {
            let inside = (2..5).contains(&x) && (2..5).contains(&y);
            assert_eq!(px(&img, x, y), if inside { [255, 0, 0, 255] } else { [0, 0, 0, 0] }, "({x},{y})");
        }
    }
}

#[test]
fn brush_centre_dab() {
    let out = scratch("brush.png");
    let s = format!("new 32 32\nbrush 0 00ff00ff 9 255 0.25 255 normal 16,16\nexport {}\n", out.display());
    let (code, j) = run("brush", &s);
    assert_eq!(code, 0, "{j}");
    assert_eq!(field(&j, "undo"), "1", "the brush is one history step");
    let img = png(&out);
    assert_eq!(px(&img, 16, 16), [0, 255, 0, 255]);
    assert_eq!(px(&img, 0, 0), [0, 0, 0, 0]);
    assert_eq!(px(&img, 31, 31), [0, 0, 0, 0]);
}

#[test]
fn invert_twice_identity() {
    let (a, b) = (scratch("inv0.png"), scratch("inv2.png"));
    let base = "new 16 16\nselect rect 0 0 8 16\nfill 0 3080c0ff\nselect none\nbrush 0 ff2000c0 5 128 0.25 200 normal 2,2 13,13,128\n";
    let (c0, j0) = run("inv0", &format!("{base}export {}\n", a.display()));
    let (c2, j2) = run("inv2", &format!("{base}adjust 0 invert\nadjust 0 invert\nexport {}\n", b.display()));
    assert_eq!((c0, c2), (0, 0), "{j0} {j2}");
    assert_eq!(field(&j0, "fnv"), field(&j2, "fnv"));
    let (pa, pb) = (std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
    assert_eq!(pa, pb, "byte for byte");
    // One invert really changed the pixels.
    let (_, j1) = run("inv1", &format!("{base}adjust 0 invert\n"));
    assert_ne!(field(&j1, "fnv"), field(&j0, "fnv"));
}

#[test]
fn undo_redo_layer_add() {
    let (_, j) = run("ur0", "new 4 4\nlayer add x\n");
    assert_eq!(field(&j, "layers"), "2");
    let (_, j) = run("ur1", "new 4 4\nlayer add x\nundo\n");
    assert_eq!((field(&j, "layers"), field(&j, "undo"), field(&j, "redo")), ("1", "0", "1"));
    let (code, j) = run("ur2", "new 4 4\nlayer add x\nundo\nredo\n");
    assert_eq!(code, 0, "{j}");
    assert_eq!((field(&j, "layers"), field(&j, "undo"), field(&j, "redo")), ("2", "1", "0"));
    // An empty side is named.
    let (code, j) = run("ur3", "new 4 4\nundo\n");
    assert_eq!((code, field(&j, "error"), field(&j, "last")), (1, "no-undo", "undo"));
}

#[test]
fn save_open_same_stat() {
    let doc = scratch("doc.unaraster");
    let a = format!(
        "new 12 10\nlayer add ink\nlayer opacity 1 128\nlayer blend 1 multiply\nselect ellipse 6 5 4 3\nfill 1 20a0ffff\nfill 0 ffffffff\nlayer offset 1 1 0\nselect poly 1,1 10,1 6,9\nsave {}\nstat\n",
        doc.display()
    );
    let (ca, ja) = run("save", &a);
    let (cb, jb) = run("open", &format!("open {}\nstat\n", doc.display()));
    assert_eq!((ca, cb), (0, 0), "{ja} {jb}");
    for k in ["width", "height", "layers", "selection", "fnv"] {
        assert_eq!(field(&ja, k), field(&jb, k), "{k}: {ja} vs {jb}");
    }
    assert_eq!(field(&jb, "undo"), "0", "open starts a fresh history");
}

#[test]
fn errors_exit_codes() {
    let (code, j) = run("bad-verb", "new 4 4\nfrobnicate 1 2\n");
    assert_eq!(code, 2, "{j}");
    assert_eq!((field(&j, "ok"), field(&j, "error"), field(&j, "last")), ("false", "script", "frobnicate"));
    assert_eq!((field(&j, "line"), field(&j, "verbs"), field(&j, "layers")), ("2", "0", "0"), "nothing ran");
    let (code, j) = run("bad-layer", "new 4 4\nlayer opacity 5 10\n");
    assert_eq!(code, 1, "{j}");
    assert_eq!((field(&j, "ok"), field(&j, "last"), field(&j, "error")), ("false", "layer opacity", "no-layer"));
    assert_eq!((field(&j, "verbs"), field(&j, "line")), ("1", "2"));
    let (code, j) = run("bad-adjust", "new 4 4\nadjust 0 levels 200 100 256 0 255\n");
    assert_eq!((code, field(&j, "last"), field(&j, "error")), (1, "adjust levels", "bad-param"));
    let (code, j) = run("no-doc", "fill 0 ff0000ff\n");
    assert_eq!((code, field(&j, "error")), (1, "no-document"));
    let o = Command::new(BIN).output().unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stdout).contains("\"error\":\"usage\""));
}

#[test]
fn comments_and_blanks() {
    let (code, j) = run("comments", "# a whole-line comment\n\nnew 3 3   # trailing comment\n\n   \nlayer add a\nstat\n");
    assert_eq!(code, 0, "{j}");
    assert_eq!((field(&j, "verbs"), field(&j, "last"), field(&j, "line"), field(&j, "layers")), ("3", "stat", "7", "2"));
}
