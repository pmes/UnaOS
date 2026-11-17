//! AETHERFONTSHORTHANDRESET (SR805): css-fonts-4 §4.3 — `font` is a reset-all shorthand: the longhands its value does
//! not name take their INITIAL values (font-style normal, font-weight 400, font-stretch 100%, font-variant-caps normal,
//! line-height normal, the size absolute as written), a longhand after it still wins by declaration order, an invalid
//! `font` is dropped whole, and font-synthesis (not in the reset set) is untouched.
mod common;
use common::*;
use layout_core::css::{apply_declaration, merge_paint, reapply_declarations, ParseCx, SpecifiedStyle};
use layout_core::FontWeight;

fn styled(decls: &[(&str, &str)]) -> SpecifiedStyle<TH> {
    let cx = ParseCx::default();
    let mut s = SpecifiedStyle::<TH>::default();
    for (prop, value) in decls {
        apply_declaration(prop, value, &mut s, &cx);
    }
    s
}

#[test]
fn font_resets_each_omitted_longhand() {
    let s = styled(&[("font-style", "italic"), ("font", "16px serif")]);
    assert_eq!(s.paint.italic, Some(false), "font-style resets to normal (Some: None would inherit)");
    let s = styled(&[("font-weight", "bold"), ("font", "16px serif")]);
    assert_eq!(s.paint.weight, Some(FontWeight::Abs(400)), "font-weight resets to 400");
    let s = styled(&[("font-variant", "small-caps"), ("font", "16px serif")]);
    assert_eq!(s.paint.small_caps, Some(layout_core::paint::Caps::Normal), "font-variant-caps resets to normal");
    let s = styled(&[("font-stretch", "condensed"), ("font", "16px serif")]);
    assert_eq!(s.paint.stretch, Some(100), "font-stretch resets to 100%");
    let s = styled(&[("line-height", "3"), ("font", "16px serif")]);
    assert_eq!(s.paint.line_height, Some(0.0), "line-height resets to normal");
    assert_eq!(s.paint.font_size, Some(16.0));
    assert!(s.paint.family.is_some());
}

#[test]
fn font_prefix_and_slash_set_their_own() {
    let s = styled(&[("font-style", "normal"), ("font", "italic bold condensed small-caps 16px/2 serif")]);
    assert_eq!(s.paint.italic, Some(true));
    assert_eq!(s.paint.weight, Some(FontWeight::Abs(700)));
    assert_eq!(s.paint.stretch, Some(75));
    assert_eq!(s.paint.small_caps, Some(layout_core::paint::Caps::SmallCaps));
    assert_eq!(s.paint.line_height, Some(2.0), "the shorthand's own /line-height applies after the reset");
}

#[test]
fn longhand_after_font_wins() {
    let s = styled(&[("font", "16px serif"), ("font-style", "italic"), ("font-weight", "bold"), ("line-height", "3")]);
    assert_eq!(s.paint.italic, Some(true), "declaration order: the later longhand wins");
    assert_eq!(s.paint.weight, Some(FontWeight::Abs(700)));
    assert_eq!(s.paint.line_height, Some(3.0));
}

#[test]
fn invalid_font_changes_nothing() {
    for v in ["bogus", "italic bogus", "bold small-caps nope serif"] {
        let s = styled(&[("font-style", "italic"), ("font-weight", "bold"), ("line-height", "3"), ("font", v)]);
        assert_eq!(s.paint.italic, Some(true), "font: {v} is dropped whole");
        assert_eq!(s.paint.weight, Some(FontWeight::Abs(700)), "font: {v}");
        assert_eq!(s.paint.line_height, Some(3.0), "font: {v}");
        assert_eq!(s.paint.small_caps, None, "font: {v}: neither the reset nor the prefix leaks");
        assert_eq!(s.paint.stretch, None, "font: {v}");
        assert_eq!(s.paint.font_size, None, "font: {v}");
    }
    let s = styled(&[("font", "italic bogus")]);
    assert_eq!(s.paint.italic, None, "an invalid font's prefix no longer leaks");
}

#[test]
fn css_wide_keywords_and_system_fonts_unchanged() {
    // Owed (AETHER-CSS-WIDE-REVERT / AETHER-FONT-SHORTHAND-SYSTEM): pinned as today, dropped; inherit / initial / unset are SR810's.
    for v in ["revert", "menu", "status-bar"] {
        let s = styled(&[("font-style", "italic"), ("font", v)]);
        assert_eq!(s.paint.italic, Some(true), "font: {v}");
        assert_eq!(s.paint.font_size, None, "font: {v}");
    }
}

#[test]
fn font_synthesis_survives() {
    let s = styled(&[("font-synthesis", "none"), ("font", "16px serif")]);
    assert_eq!((s.paint.synth_weight, s.paint.synth_style, s.paint.synth_small_caps), (Some(false), Some(false), Some(false)));
}

#[test]
fn font_size_relative_mark_resets() {
    let s = styled(&[("font-size", "2em"), ("font", "16px serif")]);
    assert_eq!(s.paint.font_size, Some(16.0));
    assert_eq!(s.paint.font_size_rel, Some((2, 0.0)), "an earlier em size's relative mark does not survive");
    let s = styled(&[("font", "1.5em serif")]);
    assert_eq!(s.paint.font_size_rel, Some((0, 1.5)), "the shorthand's em size is of the parent's");
}

#[test]
fn font_relative_replay_keeps_line_height_normal() {
    let mut s = styled(&[("line-height", "3"), ("padding", "1em"), ("font", "16px serif")]);
    let list = s.paint.font_rel.clone().expect("length declarations kept");
    assert!(list.iter().any(|(p, v)| p == "line-height" && v == "normal"), "the reset replaced the line-height entry: {list:?}");
    let cx = ParseCx::default();
    let spec: SpecifiedStyle<TH> = reapply_declarations(&list, &cx);
    merge_paint(&mut s.paint, &spec.paint);
    assert_eq!(s.paint.line_height, Some(0.0), "the font-relative replay does not bring line-height 3 back");
}

// ---- AETHERFONTSHORTHANDINHERIT (LEDGER SR810): the CSS-wide keywords on font reach its longhands (named tail block) ----

/// AETHERFONTSHORTHANDINHERIT (SR810): every longhand `font` resets, set earlier in the same block.
const FONT_ALL: [(&str, &str); 7] = [
    ("font-style", "italic"),
    ("font-size", "20px"),
    ("font-weight", "bold"),
    ("font-stretch", "condensed"),
    ("font-variant", "small-caps"),
    ("line-height", "3"),
    ("font-family", "serif"),
];

fn font_wide(kw: &str) -> SpecifiedStyle<TH> {
    let mut decls = FONT_ALL.to_vec();
    decls.push(("font", kw));
    styled(&decls)
}

/// AETHERFONTSHORTHANDINHERIT (SR810): the fields `font` resets, for a field-by-field comparison.
#[allow(clippy::type_complexity)]
fn font_fields(s: &SpecifiedStyle<TH>) -> (Option<bool>, Option<FontWeight>, Option<u16>, Option<layout_core::paint::Caps>, Option<f32>, Option<f32>, Option<(u8, f32)>, Option<u16>) {
    let p = &s.paint;
    (p.italic, p.weight, p.stretch, p.small_caps, p.line_height, p.font_size, p.font_size_rel, p.family)
}

#[test]
fn font_inherit_clears_the_block() {
    let s = font_wide("inherit");
    assert_eq!(s.paint.italic, None, "font-style inherits");
    assert_eq!(s.paint.weight, None, "font-weight inherits");
    assert_eq!(s.paint.stretch, None, "font-stretch inherits");
    assert_eq!(s.paint.small_caps, None, "font-variant-caps inherits");
    assert_eq!(s.paint.line_height, None, "line-height inherits");
    assert_eq!(s.paint.family, None, "font-family inherits");
    assert_eq!(s.paint.font_size, None, "no absolute size survives");
    assert_eq!(s.paint.font_size_rel, Some((0, 1.0)), "the size is 1 x the parent's (a control's default does not apply)");
}

#[test]
fn font_unset_is_inherit() {
    assert_eq!(font_fields(&font_wide("unset")), font_fields(&font_wide("inherit")), "every font longhand is inherited");
}

#[test]
fn font_initial_takes_the_initials() {
    let s = font_wide("initial");
    assert_eq!(s.paint.italic, Some(false));
    assert_eq!(s.paint.weight, Some(FontWeight::Abs(400)));
    assert_eq!(s.paint.stretch, Some(100));
    assert_eq!(s.paint.small_caps, Some(layout_core::paint::Caps::Normal), "font-variant-caps initial");
    assert_eq!(s.paint.line_height, Some(0.0), "line-height normal");
    assert_eq!(s.paint.font_size, Some(16.0), "medium");
    assert_eq!(s.paint.font_size_rel, Some((2, 0.0)), "absolute");
    assert_eq!(s.paint.family, Some(layout_core::STANDARD), "the standard family");
}

#[test]
fn longhand_after_font_inherit_wins() {
    let s = styled(&[("font-style", "italic"), ("font", "inherit"), ("font-weight", "bold")]);
    assert_eq!(s.paint.weight, Some(FontWeight::Abs(700)), "declaration order: the later longhand wins");
    assert_eq!(s.paint.italic, None);
}

#[test]
fn font_inherit_replay_keeps_line_height_inherited() {
    let mut s = styled(&[("line-height", "1.5em"), ("padding", "1em"), ("font", "inherit")]);
    let list = s.paint.font_rel.clone().expect("length declarations kept");
    assert!(list.iter().any(|(p, v)| p == "line-height" && v == "inherit"), "the line-height entry was replaced: {list:?}");
    let cx = ParseCx::default();
    let spec: SpecifiedStyle<TH> = reapply_declarations(&list, &cx);
    merge_paint(&mut s.paint, &spec.paint);
    assert_eq!(s.paint.line_height, None, "the font-relative replay does not bring line-height 1.5em back");
}
