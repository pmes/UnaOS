// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! RASTERCORE (LEDGER SR230) host KATs. Each test's comment says what it measures; the expected numbers are
//! hand-computed (docs/dev/evidence/raster/RASTERCORE.md, KATs).

use raster_core::{BlendMode, Command, Document, Error, History, Layer, Mask, Selection, decode, encode};

const BLUE: [u8; 4] = [0, 0, 255, 255];
const RED: [u8; 4] = [255, 0, 0, 255];

fn px(buf: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
}

/// A one-pixel document: `base` under `top` (opacity 255, `mode`).
fn pair(base: [u8; 4], top: [u8; 4], mode: BlendMode) -> [u8; 4] {
    let mut d = Document::new(1, 1).unwrap();
    d.layers.push(Layer::filled("base", 1, 1, base));
    let mut t = Layer::filled("top", 1, 1, top);
    t.blend = mode;
    d.layers.push(t);
    let out = d.composite();
    [out[0], out[1], out[2], out[3]]
}

// (1) A 4x4 two-layer document: Normal at opacity 255 shows the top exactly; at 128 the hand-computed
// blend_nonpremult value: a' = round(255 * 128 / 255) = 128, dfa = 255 * (256 - 128) >> 8 = 127, out_a = 255,
// scale = floor(2^24 / 255) = 65793; r = (255 * 128 * 65793) >> 24 = 127, b = (255 * 127 * 65793) >> 24 = 126.
#[test]
fn composite_normal_4x4() {
    let mut d = Document::new(4, 4).unwrap();
    d.layers.push(Layer::filled("base", 4, 4, BLUE));
    let mut top = Layer::filled("top", 2, 2, RED);
    top.offset = (1, 1);
    d.layers.push(top);
    let out = d.composite();
    assert_eq!(out.len(), 4 * 4 * 4);
    for y in 0..4 {
        for x in 0..4 {
            let inside = (1..3).contains(&x) && (1..3).contains(&y);
            assert_eq!(px(&out, 4, x, y), if inside { RED } else { BLUE }, "opacity 255 at ({x},{y})");
        }
    }
    let half = d.apply(&Command::SetOpacity { layer: 1, opacity: 128 }).unwrap();
    let out = half.composite();
    for y in 0..4 {
        for x in 0..4 {
            let inside = (1..3).contains(&x) && (1..3).contains(&y);
            assert_eq!(px(&out, 4, x, y), if inside { [127, 0, 126, 255] } else { BLUE }, "opacity 128 at ({x},{y})");
        }
    }
    // A hidden layer is skipped.
    let hidden = d.apply(&Command::SetVisible { layer: 1, visible: false }).unwrap();
    assert!(hidden.composite().chunks_exact(4).all(|p| p == BLUE));
}

// (2) The separable formulas on one opaque pair, base [200,100,50] under top [100,200,250]:
// Multiply round(b*s/255) = [78, 78, 49]; Screen b+s-mul = [222, 222, 251]; Difference |b-s| = [100, 100, 200].
#[test]
fn separable_modes_one_pair() {
    let (b, s) = ([200, 100, 50, 255], [100, 200, 250, 255]);
    assert_eq!(pair(b, s, BlendMode::Multiply), [78, 78, 49, 255]);
    assert_eq!(pair(b, s, BlendMode::Screen), [222, 222, 251, 255]);
    assert_eq!(pair(b, s, BlendMode::Difference), [100, 100, 200, 255]);
    assert_eq!(pair(b, s, BlendMode::Darken), [100, 100, 50, 255]);
    assert_eq!(pair(b, s, BlendMode::Lighten), [200, 200, 250, 255]);
    // Overlay: b=200 >= 128 -> 255 - round(2*55*155/255) = 188; b=100 -> round(2*100*200/255) = 157;
    // b=50 -> round(2*50*250/255) = 98.
    assert_eq!(pair(b, s, BlendMode::Overlay), [188, 157, 98, 255]);
    assert_eq!(pair(b, s, BlendMode::Normal), s);
}

// The pixel_core fact the compositor's opaque identity exists for: blend_nonpremult returns src - 1 per channel for
// an opaque source (floor(2^24 / 255) * 255 = 2^24 - 1).
#[test]
fn blend_nonpremult_opaque_off_by_one() {
    assert_eq!(pixel_core::blend_nonpremult([200, 100, 50, 255], [1, 2, 3, 255]), [199, 99, 49, 255]);
}

// (3) Erase at alpha 255 over an opaque pixel leaves alpha 0; at opacity 128 it leaves round(255 * 127 / 255) = 127.
#[test]
fn erase_clears_alpha() {
    assert_eq!(pair(RED, [0, 0, 0, 255], BlendMode::Erase)[3], 0);
    let mut d = Document::new(1, 1).unwrap();
    d.layers.push(Layer::filled("base", 1, 1, RED));
    let mut e = Layer::filled("eraser", 1, 1, [0, 0, 0, 255]);
    e.blend = BlendMode::Erase;
    e.opacity = 128;
    d.layers.push(e);
    assert_eq!(d.composite(), vec![255, 0, 0, 127]);
}

// (4) Rasterized counts: rect 5x3 = 15; ellipse (8,8,5,3) = 48 vs pi*15 = 47.12 (within 1); triangle
// (1,1)-(21,4)-(6,17) = 152 vs shoelace 152.5 (within 2). Pixel centres, as measured before the build.
#[test]
fn selection_counts() {
    let r = Selection::Rect { x: 2, y: 3, w: 5, h: 3 }.rasterize(16, 16);
    assert_eq!(r.count(), 15);
    assert_eq!(r.at(2, 3), 255);
    assert_eq!(r.at(7, 3), 0);

    let e = Selection::Ellipse { cx: 8, cy: 8, rx: 5, ry: 3 }.rasterize(16, 16).count();
    let area = core::f64::consts::PI * 5.0 * 3.0;
    assert!((e as f64 - area).abs() <= 1.0, "ellipse {e} vs {area}");
    assert_eq!(e, 48);

    let pts = vec![(1, 1), (21, 4), (6, 17)];
    let shoelace = {
        let mut a = 0i64;
        for i in 0..3 {
            let (p, q) = (pts[i], pts[(i + 1) % 3]);
            a += p.0 as i64 * q.1 as i64 - q.0 as i64 * p.1 as i64;
        }
        a.abs() as f64 / 2.0
    };
    let t = Selection::Polygon(pts).rasterize(24, 24).count();
    assert!((t as f64 - shoelace).abs() <= 2.0, "triangle {t} vs {shoelace}");
    assert_eq!(t, 152);
}

// (5) Magic wand on 8x8: colour A on x < 4 plus an isolated A pixel at (6,6) inside colour B. Tolerance 0 from (1,1)
// selects exactly the 32 connected left-half pixels (not the island); tolerance 255 selects all 64.
#[test]
fn magic_wand_regions() {
    let (a, b) = ([10, 20, 30, 255], [200, 210, 220, 255]);
    let mut l = Layer::filled("img", 8, 8, b);
    for y in 0..8 {
        for x in 0..8 {
            if x < 4 || (x, y) == (6, 6) {
                let i = (y * 8 + x) * 4;
                l.pixels[i..i + 4].copy_from_slice(&a);
            }
        }
    }
    let mut d = Document::new(8, 8).unwrap();
    d.layers.push(l);
    let Selection::Coverage(m) = Selection::magic_wand(&d, 0, 1, 1, 0).unwrap() else { panic!("not a coverage") };
    assert_eq!(m.count(), 32);
    for y in 0..8 {
        for x in 0..8 {
            assert_eq!(m.at(x, y) == 255, x < 4, "({x},{y})");
        }
    }
    let Selection::Coverage(all) = Selection::magic_wand(&d, 0, 1, 1, 255).unwrap() else { panic!("not a coverage") };
    assert_eq!(all.count(), 64);
    assert_eq!(Selection::magic_wand(&d, 1, 1, 1, 0), Err(Error::NoLayer));
    assert_eq!(Selection::magic_wand(&d, 0, 8, 1, 0), Err(Error::Selection));
}

fn three_layers() -> Document {
    let mut d = Document::new(8, 8).unwrap();
    d.layers.push(Layer::filled("a", 4, 4, RED));
    let mut b = Layer::filled("b", 4, 4, [0, 255, 0, 255]);
    b.offset = (2, 2);
    d.layers.push(b);
    let mut c = Layer::filled("c", 4, 4, BLUE);
    c.offset = (4, 4);
    d.layers.push(c);
    d.selection = Some(Selection::Rect { x: 0, y: 0, w: 3, h: 3 });
    d.validate().unwrap();
    d
}

// (6) Every command on a 3-layer document: apply through History::commit, undo gives the original, redo gives the
// edited document, and the edit changed something.
#[test]
fn every_command_undo_redo() {
    let d0 = three_layers();
    let cmds = vec![
        Command::AddLayer { index: 1, layer: Layer::filled("new", 2, 2, [1, 2, 3, 4]) },
        Command::RemoveLayer { index: 2 },
        Command::MoveLayer { from: 0, to: 2 },
        Command::SetOpacity { layer: 0, opacity: 77 },
        Command::SetBlend { layer: 1, blend: BlendMode::Screen },
        Command::SetVisible { layer: 2, visible: false },
        Command::SetOffset { layer: 1, offset: (-2, 3) },
        Command::Rename { layer: 0, name: "renamed".into() },
        Command::FillSelection { layer: 1, rgba: [9, 8, 7, 255] },
        Command::ClearSelection { layer: 0 },
        Command::SetMask { layer: 2, mask: Some(Mask::filled(4, 4, 100)) },
        Command::SetSelection(Some(Selection::Ellipse { cx: 4, cy: 4, rx: 3, ry: 3 })),
        Command::PastePixels { layer: 0, x: 1, y: 1, w: 2, h: 2, rgba: vec![50; 16] },
    ];
    assert_eq!(cmds.len(), 13);
    for cmd in &cmds {
        let mut h = History::new();
        let d1 = h.commit(&d0, cmd).unwrap();
        assert_ne!(d1, d0, "{cmd:?} changed nothing");
        let back = h.undo(d1.clone()).unwrap();
        assert_eq!(back, d0, "undo of {cmd:?}");
        let fwd = h.redo(back).unwrap();
        assert_eq!(fwd, d1, "redo of {cmd:?}");
        assert!(h.redo(fwd.clone()).is_none());
    }
    // The fill and the clear touched only the selection: layer b's (0,0) is canvas (2,2), inside the 3x3 rect.
    let filled = d0.apply(&cmds[8]).unwrap();
    assert_eq!(filled.layers[1].pixel(0, 0), Some([9, 8, 7, 255]));
    assert_eq!(filled.layers[1].pixel(1, 0), Some([0, 255, 0, 255]));
    let cleared = d0.apply(&cmds[9]).unwrap();
    assert_eq!(cleared.layers[0].pixel(2, 2), Some([0, 0, 0, 0]));
    assert_eq!(cleared.layers[0].pixel(3, 2), Some(RED));
    // Refusals.
    assert_eq!(d0.apply(&Command::RemoveLayer { index: 3 }), Err(Error::NoLayer));
    let paste = Command::PastePixels { layer: 0, x: 3, y: 3, w: 2, h: 2, rgba: vec![0; 16] };
    assert_eq!(d0.apply(&paste), Err(Error::Bounds));
    let off = Command::SetSelection(Some(Selection::Rect { x: 6, y: 6, w: 3, h: 3 }));
    assert_eq!(d0.apply(&off), Err(Error::Selection));
    assert_eq!(d0.apply(&Command::SetMask { layer: 0, mask: Some(Mask::filled(3, 4, 1)) }), Err(Error::Mask));
}

fn rich_document() -> Document {
    let mut d = three_layers();
    let l = &mut d.layers[1];
    l.name = "Ebene \u{fc}".into();
    l.opacity = 200;
    l.blend = BlendMode::Overlay;
    l.visible = false;
    l.offset = (-1, 5);
    let mut m = Mask::filled(4, 4, 0);
    for (i, c) in m.coverage.iter_mut().enumerate() {
        *c = (i * 17) as u8;
    }
    l.mask = Some(m);
    for (i, p) in d.layers[0].pixels.iter_mut().enumerate() {
        *p = (i * 7 % 256) as u8; // varied RGBA, partial alphas included
    }
    d.layers[2].blend = BlendMode::Erase;
    d.selection = Some(Selection::Polygon(vec![(0, 0), (8, 1), (3, 8)]));
    d
}

// (7) encode -> decode is the identity for a document with a mask, a selection and every field off-default (and
// again with a coverage selection from the magic wand).
#[test]
fn container_round_trip() {
    let d = rich_document();
    let bytes = encode(&d).unwrap();
    assert_eq!(&bytes[0..8], b"UNARASTR");
    assert_eq!(decode(&bytes).unwrap(), d);

    let mut w = d.clone();
    w.selection = Some(Selection::magic_wand(&d, 2, 5, 5, 0).unwrap());
    assert_eq!(decode(&encode(&w).unwrap()).unwrap(), w);
}

// (8) A truncated container and a wrong magic each decode to their named Error.
#[test]
fn container_refusals() {
    let bytes = encode(&rich_document()).unwrap();
    assert_eq!(decode(&bytes[..bytes.len() - 5]), Err(Error::Truncated));
    assert_eq!(decode(&bytes[..10]), Err(Error::Truncated));
    let mut bad = bytes.clone();
    bad[0] = b'X';
    assert_eq!(decode(&bad), Err(Error::Magic));
    let mut ver = bytes.clone();
    ver[8] = 2;
    assert_eq!(decode(&ver), Err(Error::Version));
    assert_eq!(Error::Truncated.word(), "truncated");
    assert_eq!(Error::Magic.word(), "magic");
}

// (9) validate rejects a layer whose pixel length is wrong — directly, through apply, and through encode.
#[test]
fn validate_rejects_bad_pixels() {
    let mut d = three_layers();
    d.layers[1].pixels.pop();
    assert_eq!(d.validate(), Err(Error::Dimensions));
    assert_eq!(d.apply(&Command::SetOpacity { layer: 0, opacity: 1 }), Err(Error::Dimensions));
    assert_eq!(encode(&d), Err(Error::Dimensions));
}
