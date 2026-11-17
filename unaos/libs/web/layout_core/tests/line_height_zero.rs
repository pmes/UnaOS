//! CHARTER: Aether (CODEX §2 Web) shared-core — AETHERLINEHEIGHTZERO (SR843): an author `line-height: 0` / `0px` / `0%`
//! is a zero line box, stored as `paint::LINE_HEIGHT_ZERO` apart from `normal` (0.0), and resolved to exactly 0 px.
mod common;
use common::*;
use layout_core::css::{apply_declaration, ParseCx, SpecifiedStyle};
use layout_core::paint::{line_height_length_px, LINE_HEIGHT_ZERO};
use layout_core::{FontSel, Fonts};

fn lh(decls: &[(&str, &str)]) -> Option<f32> {
    let cx = ParseCx::default();
    let mut s = SpecifiedStyle::<TH>::default();
    for (prop, value) in decls {
        apply_declaration(prop, value, &mut s, &cx);
    }
    s.paint.line_height
}

#[test]
fn author_zero_is_the_zero_marker() {
    for v in ["0", "0px", "0%", "0.0"] {
        assert_eq!(lh(&[("line-height", v)]), Some(LINE_HEIGHT_ZERO), "line-height: {v}");
    }
    assert_eq!(lh(&[("font", "12px/0 serif")]), Some(LINE_HEIGHT_ZERO), "the font shorthand's slash road");
}

#[test]
fn normal_keeps_zero_point_zero() {
    for v in ["normal", "initial" /* SR1028: revert is the tag road, None */] {
        assert_eq!(lh(&[("line-height", v)]), Some(0.0), "line-height: {v}");
    }
    assert_ne!(LINE_HEIGHT_ZERO, 0.0, "the marker is not the normal sentinel");
}

#[test]
fn negatives_are_refused() {
    // AETHER-LINE-HEIGHT-NEGATIVE-REFUSED BUILT by SR1017: a negative is invalid, the declaration writes nothing.
    assert_eq!(lh(&[("line-height", "-1")]), None /* SR1017 */);
    assert_eq!(lh(&[("line-height", "-2px")]), None /* SR1017 */);
}

#[test]
fn other_values_unchanged() {
    assert_eq!(lh(&[("line-height", "1.5")]), Some(1.5));
    assert_eq!(lh(&[("line-height", "24px")]), Some(-24.0));
}

#[test]
fn length_px_reads_the_marker_as_exactly_zero() {
    assert_eq!(line_height_length_px(LINE_HEIGHT_ZERO), 0.0);
    assert_eq!(line_height_length_px(-24.0), 24.0);
}

#[test]
fn fonts_line_height_resolves_zero() {
    let sel = FontSel::default();
    assert_eq!(Fonts::line_height(&F, sel, 16.0, LINE_HEIGHT_ZERO), 0.0, "an author zero: a zero line box");
    assert_eq!(Fonts::line_height(&F, sel, 16.0, -24.0), 24.0, "a length");
    assert_eq!(Fonts::line_height(&F, sel, 16.0, 1.5), 24.0, "a number");
    assert_eq!(Fonts::line_height(&F, sel, 16.0, 0.0), 16.0, "normal: the stub's metrics sum 13 + 3 + 0");
}

// ---- AETHERLINEHEIGHTNEGATIVEREFUSED (LEDGER SR1017): a negative line-height is dropped; a % under font-size 0 is the zero marker (named tail block) ----

/// AETHERLINEHEIGHTNEGATIVEREFUSED (SR1017): the arm's write at an explicit `cx` (the font-relative replay's own size).
fn lh_at(cx: &ParseCx, decls: &[(&str, &str)]) -> Option<f32> {
    let mut s = SpecifiedStyle::<TH>::default();
    for (prop, value) in decls {
        apply_declaration(prop, value, &mut s, cx);
    }
    s.paint.line_height
}

/// AETHERLINEHEIGHTNEGATIVEREFUSED (SR1017): css-inline-3 / CSS 2.2 §10.8.1: a negative number, length or % is
/// invalid, so the declaration is dropped and the earlier one stands (it clamped to normal, a negative % became a
/// positive multiplier).
#[test]
fn negative_dropped_keeps_earlier() {
    for v in ["-1", "-2px", "-10%", "-0.5", "-1.5em", "-3vh"] {
        assert_eq!(lh(&[("line-height", "2"), ("line-height", v)]), Some(2.0), "line-height: 2; line-height: {v}");
    }
    assert_eq!(lh(&[("line-height", "24px"), ("line-height", "-1")]), Some(-24.0), "an earlier length stands");
}

/// AETHERLINEHEIGHTNEGATIVEREFUSED (SR1017): a negative zero is not negative (-0.0 < 0.0 is false): the zero marker.
#[test]
fn negative_zero_is_the_zero_marker() {
    for v in ["-0", "-0px", "-0%", "-0.0"] {
        assert_eq!(lh(&[("line-height", v)]), Some(LINE_HEIGHT_ZERO), "line-height: {v}");
    }
}

/// AETHERLINEHEIGHTNEGATIVEREFUSED (SR1017): a % under font-size 0 is a zero line box (the replay's em is 0), not
/// `normal`; `0%` at 16px stays SR843's marker and `50%` at 16px its length (armed).
#[test]
fn percent_under_zero_font_size_is_the_zero_marker() {
    let zero = ParseCx { font: (0.0, 16.0, 0.0, 0.0), ..ParseCx::default() };
    assert_eq!(lh_at(&zero, &[("line-height", "50%")]), Some(LINE_HEIGHT_ZERO), "50% of 0px");
    assert_eq!(lh(&[("line-height", "0%")]), Some(LINE_HEIGHT_ZERO), "0% at 16px");
    assert_eq!(lh(&[("line-height", "50%")]), Some(-8.0), "50% at 16px");
}

/// AETHERLINEHEIGHTNEGATIVEREFUSED (SR1017): a negative calc() is not refused (css-values-4 §10.12 clamps it);
/// pinned as today's clamp to 0.0 (AETHER-LINE-HEIGHT-CALC-NEGATIVE-CLAMP owed: the zero marker).
#[test]
fn negative_calc_keeps_todays_clamp() {
    assert_eq!(lh(&[("line-height", "calc(10px - 20px)")]), Some(0.0));
}

/// AETHERLINEHEIGHTNEGATIVEREFUSED (SR1017): through `layout`, `font-size: 0; line-height: 50%` is the zero marker after
/// the font-relative replay; the 20px twin is its length (armed).
#[test]
fn layout_percent_under_zero_font_size() {
    let doc = page(vec![
        el("div", &[("id", "f0"), ("style", "font-size:0;line-height:50%")], vec![text("Hello")]),
        el("div", &[("id", "f20"), ("style", "font-size:20px;line-height:50%")], vec![text("Hello")]),
    ]);
    let tree = lay(&doc, 800.0, 600.0);
    let at = |id: &str| tree.paint_map.get(&id_of(&tree, &doc.by_id(id))).and_then(|p| p.line_height);
    assert_eq!(at("f0"), Some(LINE_HEIGHT_ZERO), "50% of 0px");
    assert_eq!(at("f20"), Some(-10.0), "50% of 20px");
}
