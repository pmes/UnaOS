//! ABSSHRINK (LEDGER SR420): CSS 2.2 §10.3.7 — an absolutely positioned box with `width: auto` and `left` or
//! `right` auto is shrink-to-fit; with both insets set it fills the space between them. The builder's `100%`
//! width placeholder used to reach taffy on such a box, so a `right: 8px` box laid out as wide as its containing
//! block at `x = -8`. The expected values are §10.3.7's (a Chrome page gives the same): the containing block is a
//! `position: relative; width: 300px` div at the page origin, "Sign in" is 7 characters of the fixed-advance
//! 16px font, 56 px.
mod common;
use common::*;
use layout_core::taffy;

fn cb(kid: Spec) -> N {
    page(vec![el("div", &[("id", "cb"), ("style", "position: relative; width: 300px; height: 100px")], vec![kid])])
}

fn abs_box(style: &str, words: &str) -> (f32, f32, f32, f32) {
    let doc = cb(el("div", &[("id", "box"), ("style", style)], vec![text(words)]));
    let tree = lay(&doc, 400.0, 300.0);
    let (cx, cy, _, _) = box_of(&tree, &doc.by_id("cb"));
    let (x, y, w, h) = box_of(&tree, &doc.by_id("box"));
    (x - cx, y - cy, w, h)
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.01
}

#[test]
fn one_auto_inset_shrinks_to_fit() {
    // A: right set, left auto — the box hugs the right inset.
    let (x, _, w, _) = abs_box("position: absolute; top: 0; right: 8px", "Sign in");
    assert!(near(w, 56.0) && near(x, 236.0), "right: 8px shrinks to its text at 300-8-56: x {x} w {w}");
    // B: left set, right auto.
    let (x, _, w, _) = abs_box("position: absolute; top: 0; left: 8px", "Sign in");
    assert!(near(w, 56.0) && near(x, 8.0), "left: 8px shrinks to its text: x {x} w {w}");
    // E: a text longer than the available width (300 - 8). §10.3.7 gives 292. Measured: 392 — the text leaf's
    // MaxContent probe is capped at the VIEWPORT (`tree.rs` `vw_cap`, 400), not taffy's available width, and the
    // box's inline row is a flex row whose text item has `flex_shrink: 0`, so the line is never clamped to 292
    // (owed ABS-WRAPFILL, LEDGER SR420). The box still sits on its right inset.
    let (x, _, w, h) = abs_box("position: absolute; top: 0; right: 8px", &"word ".repeat(100));
    assert!(w > 260.0 && w <= 400.0, "a long text measures up to the viewport cap (ABS-WRAPFILL): w {w}");
    assert!((x + w - 292.0).abs() < 0.5, "its right edge on the inset: x {x} w {w}");
    assert!(h > 20.0, "and wraps: h {h}");
}

#[test]
fn both_insets_stretch_between_them() {
    // C: neither inset auto — the width is what the insets leave.
    let (x, _, w, _) = abs_box("position: absolute; top: 0; left: 8px; right: 8px", "Sign in");
    assert!(near(w, 284.0) && near(x, 8.0), "left and right 8px fill 300-16: x {x} w {w}");
}

#[test]
fn no_inset_shrinks_at_its_static_place() {
    // D: no inset — the static position (Aether keeps such a box in flow), shrink-to-fit.
    let (x, _, w, _) = abs_box("position: absolute", "Sign in");
    assert!(near(w, 56.0) && near(x, 0.0), "no inset shrinks to its text at the static x: x {x} w {w}");
}

#[test]
fn author_width_is_kept_and_the_placeholder_restored() {
    // F: an author `width: 100%` is a width, not the placeholder: 300 wide, placed by the right inset at -8.
    let (x, _, w, _) = abs_box("position: absolute; top: 0; right: 8px; width: 100%", "Sign in");
    assert!(near(w, 300.0) && near(x, -8.0), "an author width: 100% stays: x {x} w {w}");
    // The `auto` is for the run only: the placeholder is back after it, and a second layout answers the same.
    let doc = cb(el("div", &[("id", "box"), ("style", "position: absolute; top: 0; right: 8px")], vec![text("Sign in")]));
    let mut tree = lay(&doc, 400.0, 300.0);
    let id = id_of(&tree, &doc.by_id("box"));
    assert_eq!(tree.taffy.style(id).unwrap().size.width, taffy::style::Dimension::percent(1.0), "placeholder restored");
    let mut ids: Vec<taffy::NodeId> = tree.node_map.iter().map(|(t, _)| *t).collect();
    ids.sort_by_key(|t| u64::from(*t));
    let before: Vec<taffy::Style> = ids.iter().map(|t| tree.taffy.style(*t).unwrap().clone()).collect();
    let first = box_of(&tree, &doc.by_id("box"));
    layout_core::remeasure(&mut tree, &F);
    let after: Vec<taffy::Style> = ids.iter().map(|t| tree.taffy.style(*t).unwrap().clone()).collect();
    assert!(before == after, "a second layout leaves every style as the first left it");
    assert_eq!(box_of(&tree, &doc.by_id("box")), first, "and the same box");
}
