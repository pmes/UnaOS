// CHARTER: Facet (Images) — host face over the shared cores (RASTERFACE, LEDGER SR234)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! RASTERFACE's KATs, headless the `--synth` way: the script tokens become the very messages the window sends and
//! go through the same `Editor::on`; every result is read back from the raster_core Document.

use bandy::SMessage;
use facet_view::editor::{self, Editor, PixelOrder, tools, witness};
use quartzite::tetra::TetraNode;

const SIZE: (u32, u32) = (800, 600);

fn ed(w: u32, h: u32) -> Editor {
    Editor::new(w, h, SIZE, PixelOrder::Rgba).expect("new")
}

/// The screen point of canvas point `(x, y)` through the editor's view.
fn at(e: &Editor, x: f64, y: f64) -> String {
    let s = e.view.scale();
    format!("{}:{}", e.view.origin.0 + x * s, e.view.origin.1 + y * s)
}

/// Run a script; no bad tokens.
fn run(e: &mut Editor, script: &str) {
    let bad = editor::drive(e, script);
    assert!(bad.is_empty(), "bad tokens {bad:?}");
}

fn last(e: &Editor) -> &str {
    e.tokens.last().map(String::as_str).unwrap_or("")
}

/// KAT 1: `--synth new 64x48` (the built binary): the first token names width / height / layers=1.
#[test]
fn kat1_synth_new_names_the_document() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_facet-view"))
        .args(["--synth", "new", "64x48", "--do", ""])
        .output()
        .expect("run facet-view");
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8(out.stdout).unwrap();
    let first = text.lines().next().unwrap();
    assert_eq!(first, ":: FACET: gesture=new width=64 height=48 layers=1 active=0 undo=0 redo=0 -> ok ::");
    assert!(text.lines().last().unwrap().starts_with(":: FACET: gesture=stat width=64 height=48 zoom=1 "), "{text}");
}

/// KAT 2: a wheel notch up doubles the scale about the cursor (`zoom=2`); a Move-tool drag moves the origin by the
/// drag delta.
#[test]
fn kat2_zoom_and_pan() {
    let mut e = ed(64, 48);
    assert_eq!((e.view.zoom, e.view.origin), (0, (288.0, 276.0)));
    run(&mut e, "move:320:300 wheel:1");
    assert_eq!(e.view.scale(), 2.0);
    assert!(last(&e).starts_with(":: FACET: gesture=zoom zoom=2 origin=256,252 "), "{}", last(&e));
    assert_eq!(e.view.to_canvas(320.0, 300.0), (32.0, 24.0), "the point under the cursor stays put");
    run(&mut e, "tool-move press:100:100 move:120:104 move:130:110 release");
    assert_eq!(e.view.origin, (286.0, 262.0));
    assert!(last(&e).starts_with(":: FACET: gesture=pan dx=30 dy=10 origin=286,262 "), "{}", last(&e));
    // The bounds: 32 is the top.
    run(&mut e, "wheel:1 wheel:1 wheel:1 wheel:1");
    assert_eq!(e.view.scale(), 32.0);
    run(&mut e, "wheel:1");
    assert!(last(&e).contains("why=zoom-bound") && last(&e).ends_with("-> refused ::"));
    assert_eq!(e.hist.past.len(), 0, "view gestures are not ops");
}

/// KAT 3: a brush click at the canvas centre lands the colour on the active layer.
#[test]
fn kat3_brush_lands_pixels() {
    let mut e = ed(64, 48);
    let c = at(&e, 32.0, 24.0);
    run(&mut e, &format!("tool-brush set:color=00ff00ff click:{c}"));
    assert!(last(&e).starts_with(":: FACET: gesture=brush points=1 layer=0 "), "{}", last(&e));
    assert!(last(&e).ends_with("undo=1 redo=0 -> ok ::"));
    let l = &e.doc.layers[0];
    assert_eq!(l.pixel(32, 24), Some([0, 255, 0, 255]));
    assert_eq!(l.pixel(0, 0), Some([0, 0, 0, 0]));
    // The eraser takes it off again, as one more op.
    run(&mut e, &format!("tool-eraser click:{c}"));
    assert_eq!(e.doc.layers[0].pixel(32, 24).unwrap()[3], 0);
    assert_eq!(e.hist.past.len(), 2);
}

/// KAT 4: Layer+ -> layers=2; Undo -> layers=1.
#[test]
fn kat4_layer_add_and_undo() {
    let mut e = ed(64, 48);
    run(&mut e, "layer-add");
    assert_eq!(last(&e), ":: FACET: gesture=layer op=add index=1 layers=2 active=1 undo=1 redo=0 -> ok ::");
    run(&mut e, "undo");
    assert_eq!(last(&e), ":: FACET: gesture=undo layers=1 active=0 undo=0 redo=1 -> ok ::");
    assert_eq!(e.doc.layers.len(), 1);
    run(&mut e, "key:Ctrl+Shift+Z");
    assert_eq!(e.doc.layers.len(), 2);
    run(&mut e, "key:Ctrl+z undo");
    assert!(last(&e).contains("why=no-undo") && last(&e).ends_with("-> refused ::"));
}

/// KAT 5: a Rect drag then a Fill click: the composite is the colour inside the rect, transparent outside.
#[test]
fn kat5_rect_then_fill() {
    let mut e = ed(64, 48);
    let (a, b, c) = (at(&e, 8.0, 8.0), at(&e, 24.0, 20.0), at(&e, 1.0, 1.0));
    run(&mut e, &format!("layer-add tool-select-rect press:{a} move:{b} release"));
    assert!(last(&e).starts_with(":: FACET: gesture=select-rect x=8 y=8 w=16 h=12 "), "{}", last(&e));
    run(&mut e, &format!("tool-fill set:color=ff0000ff click:{c}"));
    assert!(last(&e).starts_with(":: FACET: gesture=fill layer=1 rgba=ff0000ff "), "{}", last(&e));
    let comp = e.doc.composite();
    for y in 0..48u32 {
        for x in 0..64u32 {
            let i = ((y * 64 + x) * 4) as usize;
            let inside = (8..24).contains(&x) && (8..20).contains(&y);
            let want = if inside { [255, 0, 0, 255] } else { [0, 0, 0, 0] };
            assert_eq!(comp[i..i + 4], want, "({x},{y})");
        }
    }
}

/// KAT 6: Invert twice: the composite equals the original.
#[test]
fn kat6_invert_twice() {
    let mut e = ed(32, 32);
    let (p, q) = (at(&e, 4.0, 4.0), at(&e, 28.0, 20.0));
    run(&mut e, &format!("set:color=3080c0ff set:diameter=7 tool-brush press:{p} move:{q} release"));
    let before = e.doc.composite();
    run(&mut e, "adjust-invert");
    assert!(last(&e).starts_with(":: FACET: gesture=adjust kind=invert layer=0 "), "{}", last(&e));
    assert_ne!(e.doc.composite(), before);
    run(&mut e, "adjust-invert");
    assert_eq!(e.doc.composite(), before);
    assert_eq!(e.hist.past.len(), 3);
}

/// KAT 7: save then open through a scratch file: the composite and the layers equal.
#[test]
fn kat7_save_open_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("pic.unarastr");
    let path = file.to_str().unwrap();
    let mut e = ed(40, 30);
    let (a, b) = (at(&e, 5.0, 5.0), at(&e, 30.0, 25.0));
    run(&mut e, &format!("layer-add set:opacity=200 set:blend=multiply set:color=ff8000ff tool-brush press:{a} move:{b} release"));
    let saved = e.doc.clone();
    run(&mut e, &format!("set:save={path}"));
    assert!(last(&e).starts_with(&format!(":: FACET: gesture=save path={path} ")), "{}", last(&e));
    run(&mut e, "layer-add adjust-invert");
    assert_ne!(e.doc, saved);
    run(&mut e, &format!("set:open={path}"));
    assert!(last(&e).ends_with("layers=2 active=1 undo=0 redo=0 -> ok ::"), "{}", last(&e));
    assert_eq!(e.doc.composite(), saved.composite());
    assert_eq!(e.doc.layers.len(), 2);
    assert_eq!((e.doc.layers[1].opacity, tools::blend_word(e.doc.layers[1].blend)), (200, "multiply"));
    // The binary opens it too.
    let opened = Editor::open(path, SIZE, PixelOrder::Rgba).unwrap();
    assert_eq!(opened.doc, e.doc);
    // Export writes the composite through pixel_core's writer.
    let png = dir.path().join("pic.png");
    run(&mut e, &format!("set:export={}", png.to_str().unwrap()));
    let img = pixel_core::decode_png(&std::fs::read(&png).unwrap()).unwrap();
    assert_eq!((img.width, img.height), (40, 30));
}

/// KAT 8: a gesture on a bad layer index: `-> refused` and the Document unchanged.
#[test]
fn kat8_bad_layer_is_refused() {
    let mut e = ed(16, 16);
    let c = at(&e, 3.0, 3.0);
    let (doc, undo) = (e.doc.clone(), e.hist.past.len());
    run(&mut e, &format!("active:5 tool-fill click:{c}"));
    assert_eq!(last(&e), ":: FACET: gesture=fill layer=5 rgba=000000ff why=no-layer layers=1 active=5 undo=0 redo=0 -> refused ::");
    run(&mut e, &format!("tool-brush click:{c} adjust-invert set:opacity=9 layer-eye"));
    assert!(e.tokens.iter().rev().take(4).all(|t| t.contains("why=no-layer") && t.ends_with("-> refused ::")), "{:?}", e.tokens);
    assert_eq!((e.doc.clone(), e.hist.past.len()), (doc, undo));
    run(&mut e, "set:diameter=x");
    assert!(last(&e).contains("gesture=field key=diameter why=bad-field"));
}

/// The tree: the tool rail, the one surface, the layer / adjust / file row, the field line; every button fires a
/// verb the editor takes, and the layers panel's clicks are the same ops.
#[test]
fn tree_buttons_are_editor_verbs_and_panel_clicks() {
    let TetraNode::VStack(top) = editor::tree() else { panic!("not a VStack") };
    assert!(matches!(&top[1], TetraNode::Surface { id } if id == editor::SURFACE));
    let mut e = ed(16, 16);
    let mut n = 0;
    for row in [&top[0], &top[2]] {
        let TetraNode::HStack(buttons) = row else { panic!("not a row") };
        for b in buttons {
            let TetraNode::Button { action: SMessage::Input { target, text }, .. } = b else { panic!("not an Input button") };
            assert_eq!(target, editor::TARGET);
            assert!(tools::is_verb(text) && e.verb(text), "{text}");
            n += 1;
        }
    }
    assert_eq!(n, 8 + 4 + 6 + 8 + 2);
    // The panel: two layers, the top one's row first; a click on the bottom row's eye hides layer 0.
    let mut e = ed(16, 16);
    run(&mut e, "layer-add");
    let px = (SIZE.0 - editor::layers::PANEL_W) as f64;
    run(&mut e, &format!("click:{}:{}", px + 60.0, 10.0));
    assert!(last(&e).starts_with(":: FACET: gesture=layer op=select index=1 "), "{}", last(&e));
    run(&mut e, &format!("click:{}:{}", px + 10.0, editor::layers::ROW_H as f64 + 10.0));
    assert!(last(&e).starts_with(":: FACET: gesture=layer op=visible index=0 "), "{}", last(&e));
    assert!(!e.doc.layers[0].visible);
    // The frame is the surface's size; the fnv in the stat token is the composite's.
    let SMessage::SurfaceBlit { width, height, pixels, url } = e.frame() else { panic!("no blit") };
    assert_eq!((url.as_str(), width, height, pixels.len()), (editor::SURFACE, 800, 600, 800 * 600 * 4));
    assert!(e.stat().contains(&format!("fnv={:016x}", witness::fnv1a(&e.doc.composite()))));
}
