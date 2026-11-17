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
    assert_eq!(s.paint.small_caps, Some(false), "font-variant-caps resets to normal");
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
    assert_eq!(s.paint.small_caps, Some(true));
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
fn font_fields(s: &SpecifiedStyle<TH>) -> (Option<bool>, Option<FontWeight>, Option<u16>, Option<bool>, Option<f32>, Option<f32>, Option<(u8, f32)>, Option<u16>) {
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
    assert_eq!(s.paint.font_size_rel, None, "no relative mark: the size bit (SR815) takes the parent's, not a control's default"); assert_eq!(s.paint.inherit, layout_core::paint::INHERIT_FONT, "SR815: the seven explicit inherit bits");
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
    assert_eq!(s.paint.small_caps, Some(false));
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

// ---- AETHERCSSWIDELONGHANDS (LEDGER SR815): the explicit inherit bit per font field (named tail block) ----

use layout_core::paint::{INHERIT_FAMILY, INHERIT_FONT, INHERIT_FONT_SIZE, INHERIT_ITALIC, INHERIT_LINE_HEIGHT, INHERIT_SMALL_CAPS, INHERIT_STRETCH, INHERIT_WEIGHT};

/// AETHERCSSWIDELONGHANDS (SR815): each font longhand with the bit its explicit inherit sets.
const LONGHAND_BITS: [(&str, &str, u32); 7] = [
    ("font-style", "italic", INHERIT_ITALIC),
    ("font-weight", "bold", INHERIT_WEIGHT),
    ("font-stretch", "condensed", INHERIT_STRETCH),
    ("font-variant-caps", "small-caps", INHERIT_SMALL_CAPS),
    ("font-size", "20px", INHERIT_FONT_SIZE),
    ("line-height", "3", INHERIT_LINE_HEIGHT),
    ("font-family", "serif", INHERIT_FAMILY),
];

/// AETHERCSSWIDELONGHANDS (SR815): a longhand's `inherit` / `unset` (any case) clears its earlier value and sets its
/// bit; a later value in the same block beats the bit; `font-variant: inherit` sets the caps bit.
#[test]
fn css_wide_longhand_sets_its_bit() {
    for (prop, value, bit) in LONGHAND_BITS {
        for kw in ["inherit", "unset", "INHERIT"] {
            let s = styled(&[(prop, value), (prop, kw)]);
            assert_eq!(s.paint.inherit, bit, "{prop}: {kw} sets its bit");
            assert_eq!(s.paint.valued() & bit, 0, "{prop}: {kw} clears the earlier value");
            assert!(s.paint.inherits(bit), "{prop}: {kw} inherits");
        }
        let s = styled(&[(prop, "inherit"), (prop, value)]);
        assert!(!s.paint.inherits(bit), "{prop}: a later value beats the bit");
        assert_ne!(s.paint.valued() & bit, 0, "{prop}: the later value stands");
    }
    let s = styled(&[("font-variant", "small-caps"), ("font-variant", "inherit")]);
    assert!(s.paint.inherits(INHERIT_SMALL_CAPS) && s.paint.small_caps.is_none(), "font-variant: inherit reaches its caps half");
    let s = styled(&[("color", "inherit")]);
    assert_eq!(s.paint.inherit, layout_core::paint::INHERIT_COLOR, "color: inherit sets the colour bit (AETHERCSSWIDECOLOR SR823)");
}

/// AETHERCSSWIDELONGHANDS (SR815): across rules, a later rule's explicit inherit clears the earlier rule's value (the
/// cascade's "later declaration wins"), for each of the seven fields.
#[test]
fn merge_paint_inherit_beats_earlier_rule() {
    for (prop, value, bit) in LONGHAND_BITS {
        let mut dst = styled(&[(prop, value)]);
        assert_ne!(dst.paint.valued() & bit, 0, "{prop}: the earlier rule's value (armed)");
        let later = styled(&[(prop, "inherit")]);
        merge_paint(&mut dst.paint, &later.paint);
        assert!(dst.paint.inherits(bit), "{prop}: the later inherit wins");
        assert_eq!(dst.paint.valued() & bit, 0, "{prop}: the earlier value is gone");
    }
}

/// AETHERCSSWIDELONGHANDS (SR815): a later rule's value clears the earlier rule's bit; a later rule that says nothing
/// about the field keeps it; a rule carrying inherit then a value (same block) does not carry the bit.
#[test]
fn merge_paint_later_value_clears_bit() {
    let mut dst = styled(&[("font-style", "inherit"), ("font-size", "inherit")]);
    merge_paint(&mut dst.paint, &styled(&[("color", "red")]).paint);
    assert_eq!(dst.paint.inherit, INHERIT_ITALIC | INHERIT_FONT_SIZE, "an unrelated later rule keeps the bits");
    merge_paint(&mut dst.paint, &styled(&[("font-style", "normal")]).paint);
    assert_eq!(dst.paint.italic, Some(false));
    assert_eq!(dst.paint.inherit, INHERIT_FONT_SIZE, "the later value cleared the style bit only");
    merge_paint(&mut dst.paint, &styled(&[("font-size", "inherit"), ("font-size", "2em")]).paint);
    assert_eq!(dst.paint.inherit, 0, "inherit then a value in one block: the value wins across the merge");
    assert_eq!(dst.paint.font_size_rel, Some((0, 2.0)));
}

/// AETHERCSSWIDELONGHANDS (SR815): `font: inherit` / `font: unset` set the seven bits and beat an earlier rule's
/// longhands; `font: initial` writes values (SR805), no bits.
#[test]
fn font_inherit_sets_seven_bits() {
    for kw in ["inherit", "unset"] {
        let mut dst = styled(&FONT_ALL);
        merge_paint(&mut dst.paint, &styled(&[("font", kw)]).paint);
        assert_eq!(dst.paint.inherit, INHERIT_FONT, "font: {kw}");
        for (_, _, bit) in LONGHAND_BITS {
            assert!(dst.paint.inherits(bit), "font: {kw} inherits bit {bit}");
        }
    }
    let s = font_wide("initial");
    assert_eq!(s.paint.valued(), INHERIT_FONT, "font: initial writes every field");
    assert!(!(0..7).any(|b| s.paint.inherits(1 << b)), "font: initial inherits nothing");
}

/// AETHERCSSWIDELONGHANDS (SR815): the font-relative replay keeps line-height's explicit inherit across a merge onto
/// an earlier rule's line-height (the replayed entry is `inherit`).
#[test]
fn font_inherit_replay_keeps_bit() {
    let mut s = styled(&[("line-height", "1.5em"), ("padding", "1em")]);
    merge_paint(&mut s.paint, &styled(&[("font", "inherit")]).paint);
    let list = s.paint.font_rel.clone().expect("length declarations kept");
    assert!(list.iter().any(|(p, v)| p == "line-height" && v == "inherit"), "the later entry replaced the earlier: {list:?}");
    let cx = ParseCx::default();
    let spec: SpecifiedStyle<TH> = reapply_declarations(&list, &cx);
    merge_paint(&mut s.paint, &spec.paint);
    assert!(s.paint.inherits(INHERIT_LINE_HEIGHT), "the replay keeps line-height inherited");
    assert_eq!(s.paint.line_height, None);
}

// ---- AETHERCSSWIDECOLOR (LEDGER SR823): the explicit inherit bit for color (named tail block) ----

use layout_core::paint::INHERIT_COLOR;

/// AETHERCSSWIDECOLOR (SR823): `color: inherit` / `unset` (any case) clears an earlier colour in the block and sets the
/// colour bit (css-cascade-4 §7.3.3: color is inherited, so unset == inherit); a later value in the block beats it.
#[test]
fn css_wide_color_sets_its_bit() {
    for kw in ["inherit", "unset", "INHERIT"] {
        let s = styled(&[("color", "blue"), ("color", kw)]);
        assert_eq!(s.paint.inherit, INHERIT_COLOR, "color: {kw} sets the colour bit");
        assert_eq!(s.paint.color, None, "color: {kw} clears the earlier colour");
        assert!(s.paint.inherits(INHERIT_COLOR), "color: {kw} inherits");
    }
    let s = styled(&[("color", "inherit"), ("color", "green")]);
    assert!(!s.paint.inherits(INHERIT_COLOR), "a later value beats the bit");
    assert!(s.paint.color.is_some(), "the later colour stands");
}

/// AETHERCSSWIDECOLOR (SR823): across rules, a later `color: inherit` clears the earlier rule's colour and sets the bit;
/// a still later rule's colour clears the bit again (merge_paint's `merge_inherit`).
#[test]
fn merge_paint_color_inherit_beats_earlier_rule() {
    let mut dst = styled(&[("color", "blue")]);
    assert!(dst.paint.color.is_some(), "the earlier rule's colour");
    merge_paint(&mut dst.paint, &styled(&[("color", "inherit")]).paint);
    assert_eq!(dst.paint.color, None, "the later inherit cleared the earlier colour");
    assert!(dst.paint.inherits(INHERIT_COLOR), "the later inherit set the bit");
    merge_paint(&mut dst.paint, &styled(&[("padding", "1px")]).paint);
    assert!(dst.paint.inherits(INHERIT_COLOR), "an unrelated later rule keeps the bit");
    merge_paint(&mut dst.paint, &styled(&[("color", "green")]).paint);
    assert!(!dst.paint.inherits(INHERIT_COLOR), "a later colour clears the bit");
    assert_eq!(dst.paint.inherit & INHERIT_COLOR, 0);
    assert!(dst.paint.color.is_some(), "the later colour stands");
}

/// AETHERCSSWIDECOLOR (SR823): `font: inherit` reaches the seven font fields only: an earlier colour survives and the
/// colour bit stays clear (INHERIT_FONT is 127).
#[test]
fn font_inherit_leaves_color() {
    let mut dst = styled(&[("color", "blue")]);
    merge_paint(&mut dst.paint, &styled(&[("font", "inherit")]).paint);
    assert!(dst.paint.color.is_some(), "font: inherit keeps the colour");
    assert!(!dst.paint.inherits(INHERIT_COLOR), "font: inherit does not set the colour bit");
    assert_eq!(dst.paint.inherit, INHERIT_FONT);
}
