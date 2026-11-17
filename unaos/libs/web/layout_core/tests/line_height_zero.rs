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
    for v in ["normal", "initial", "revert"] {
        assert_eq!(lh(&[("line-height", v)]), Some(0.0), "line-height: {v}");
    }
    assert_ne!(LINE_HEIGHT_ZERO, 0.0, "the marker is not the normal sentinel");
}

#[test]
fn negatives_keep_todays_clamp() {
    // AETHER-LINE-HEIGHT-NEGATIVE-REFUSED owed: pinned as today (the clamp reads normal), not the zero marker.
    assert_eq!(lh(&[("line-height", "-1")]), Some(0.0));
    assert_eq!(lh(&[("line-height", "-2px")]), Some(0.0));
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
