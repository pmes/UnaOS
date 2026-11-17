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
    for v in [/* SR1028: revert built; SR1022: the six system fonts now apply (system_font_* below) */ "bogus-system"] {
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

// ---- AETHERCSSWIDEINHERITEDREST (LEDGER SR831): the explicit inherit bit for white-space / direction / list-style-type (named tail block) ----

use layout_core::paint::{INHERIT_DIRECTION, INHERIT_LIST_STYLE, INHERIT_WHITE_SPACE};

/// AETHERCSSWIDEINHERITEDREST (SR831): each property (and the alias or shorthand whose arm writes the same field) with a
/// value it takes and the bit its explicit inherit sets.
const REST_BITS: [(&str, &str, u32); 5] = [
    ("white-space", "pre", INHERIT_WHITE_SPACE),
    ("white-space-collapse", "preserve", INHERIT_WHITE_SPACE),
    ("direction", "rtl", INHERIT_DIRECTION),
    ("list-style-type", "square", INHERIT_LIST_STYLE),
    ("list-style", "square inside", INHERIT_LIST_STYLE),
];

/// AETHERCSSWIDEINHERITEDREST (SR831): `inherit` / `unset` (any case) on each of the three inherited properties clears
/// the earlier value in the block (white-space: its `nowrap` pair too) and sets exactly that property's bit.
#[test]
fn css_wide_inherited_rest_sets_its_bit() {
    for (prop, value, bit) in REST_BITS {
        let s = styled(&[(prop, value)]);
        assert!(s.paint.valued() & bit != 0, "{prop}: {value} holds a value (armed)");
        for kw in ["inherit", "unset", "INHERIT"] {
            let s = styled(&[(prop, value), (prop, kw)]);
            assert_eq!(s.paint.inherit, bit, "{prop}: {kw} sets its bit only");
            assert!(s.paint.inherits(bit), "{prop}: {kw} inherits");
            assert_eq!(s.paint.white_space, None, "{prop}: {kw}: no white-space value");
            assert_eq!(s.paint.nowrap, None, "{prop}: {kw}: no nowrap value");
            assert_eq!(s.paint.rtl, None, "{prop}: {kw}: no direction value");
            assert_eq!(s.paint.list_style, None, "{prop}: {kw}: no list-style value");
        }
    }
}

/// AETHERCSSWIDEINHERITEDREST (SR831): a later value in the same block beats the bit (`inherits` is false while the
/// field holds one) and stands.
#[test]
fn css_wide_inherited_rest_later_value_wins() {
    for (prop, value, bit) in REST_BITS {
        let s = styled(&[(prop, "inherit"), (prop, value)]);
        assert!(!s.paint.inherits(bit), "{prop}: the later {value} beats the bit");
    }
    let s = styled(&[("white-space", "inherit"), ("white-space", "pre")]);
    assert_eq!((s.paint.white_space, s.paint.nowrap), (Some(2), Some(true)), "the later pre stands with its nowrap");
    let s = styled(&[("direction", "inherit"), ("direction", "rtl")]);
    assert_eq!(s.paint.rtl, Some(true));
    let s = styled(&[("list-style-type", "inherit"), ("list-style-type", "square")]);
    assert_eq!(s.paint.list_style, Some(3));
}

/// AETHERCSSWIDEINHERITEDREST (SR831): across rules, a later inherit clears the earlier rule's value and sets the bit; an
/// unrelated later rule keeps it; a still later value clears it (merge_paint's `merge_inherit`).
#[test]
fn css_wide_inherited_rest_merge_paint() {
    for (prop, value, bit) in REST_BITS {
        let mut dst = styled(&[(prop, value)]);
        merge_paint(&mut dst.paint, &styled(&[(prop, "inherit")]).paint);
        assert!(dst.paint.inherits(bit), "{prop}: the later inherit set the bit");
        assert_eq!(dst.paint.valued() & bit, 0, "{prop}: the later inherit cleared the earlier value");
        merge_paint(&mut dst.paint, &styled(&[("padding", "1px")]).paint);
        assert!(dst.paint.inherits(bit), "{prop}: an unrelated later rule keeps the bit");
        merge_paint(&mut dst.paint, &styled(&[(prop, value)]).paint);
        assert!(!dst.paint.inherits(bit), "{prop}: a later value clears the bit");
        assert_eq!(dst.paint.inherit & bit, 0);
    }
}

/// AETHERCSSWIDEINHERITEDREST (SR831): a `pre` rule's nowrap does not survive a later rule's `white-space: inherit`
/// (the arm writes the pair, set_inherit clears the pair).
#[test]
fn css_wide_inherited_rest_white_space_clears_nowrap() {
    let mut dst = styled(&[("white-space", "nowrap")]);
    assert_eq!(dst.paint.nowrap, Some(true), "the earlier rule's nowrap (armed)");
    merge_paint(&mut dst.paint, &styled(&[("white-space", "unset")]).paint);
    assert_eq!((dst.paint.white_space, dst.paint.nowrap), (None, None));
    assert!(dst.paint.inherits(INHERIT_WHITE_SPACE));
}

/// AETHERCSSWIDEINHERITEDREST (SR831): `font: inherit` reaches the seven font fields only: the three values survive
/// and their bits stay clear (INHERIT_FONT is still 127).
#[test]
fn font_inherit_leaves_inherited_rest() {
    let mut dst = styled(&[("white-space", "pre"), ("direction", "rtl"), ("list-style-type", "square")]);
    merge_paint(&mut dst.paint, &styled(&[("font", "inherit")]).paint);
    assert_eq!((dst.paint.white_space, dst.paint.rtl, dst.paint.list_style), (Some(2), Some(true), Some(3)));
    assert_eq!(dst.paint.inherit, INHERIT_FONT);
    assert_eq!(INHERIT_FONT & (INHERIT_WHITE_SPACE | INHERIT_DIRECTION | INHERIT_LIST_STYLE), 0);
}

// ---- AETHERCSSCOLORCURRENTCOLOR (LEDGER SR841): color: currentcolor writes the colour inherit bit (named tail block) ----

/// AETHERCSSCOLORCURRENTCOLOR (SR841): css-color-4 §6.2: `currentcolor` on `color` itself is `inherit`: in any case it
/// clears an earlier colour in the block and sets the colour bit (the `is_neutral_keyword` drop kept the earlier colour);
/// a later value in the block beats it.
#[test]
fn currentcolor_on_color_sets_inherit_bit() {
    for kw in ["currentcolor", "currentColor", "CURRENTCOLOR"] {
        let s = styled(&[("color", "blue"), ("color", kw)]);
        assert_eq!(s.paint.inherit, INHERIT_COLOR, "color: {kw} sets the colour bit");
        assert_eq!(s.paint.color, None, "color: {kw} clears the earlier colour");
    }
    let s = styled(&[("color", "currentcolor"), ("color", "green")]);
    assert!(!s.paint.inherits(INHERIT_COLOR), "a later value beats the bit");
    assert!(s.paint.color.is_some(), "the later colour stands");
}

/// AETHERCSSCOLORCURRENTCOLOR (SR841): `.a { color: blue } .a.b { color: currentcolor }`: the later rule clears the
/// earlier rule's colour and sets the bit through merge_paint's `merge_inherit`.
#[test]
fn merge_paint_currentcolor_beats_earlier_rule() {
    let mut dst = styled(&[("color", "blue")]);
    assert!(dst.paint.color.is_some(), "the earlier rule's colour (armed)");
    merge_paint(&mut dst.paint, &styled(&[("color", "currentcolor")]).paint);
    assert_eq!(dst.paint.color, None, "the later currentcolor cleared the earlier colour");
    assert!(dst.paint.inherits(INHERIT_COLOR), "the later currentcolor set the bit");
}

/// AETHERCSSCOLORCURRENTCOLOR (SR841): the OTHER colour properties are pinned as measured, none of them touching the
/// inherit bits: outline-color's `Some(None)` is THE currentColor marker (render's focusring `current_color` reads it);
/// border has no marker and paints the arm's grey (128, 128, 128); background-color drops it, an earlier background
/// stands (AETHER-CSS-CURRENTCOLOR-REST owed); a font longhand's `currentcolor` sets no bit.
#[test]
fn currentcolor_other_properties_pinned() {
    let s = styled(&[("color", "red"), ("outline-color", "currentcolor")]);
    assert_eq!(s.paint.outline_color, Some(None), "outline-color: currentcolor is the marker");
    assert_eq!(s.paint.inherit, 0);
    let s = styled(&[("color", "red"), ("border", "1px solid currentcolor")]);
    assert_eq!(s.paint.border, Some([Some((1.0, (128, 128, 128))); 4]), "border: currentcolor paints grey today (owed)");
    assert_eq!(s.paint.inherit, 0);
    let s = styled(&[("color", "red"), ("background-color", "blue"), ("background-color", "currentcolor")]);
    assert!(s.paint.currentcolor & layout_core::paint::CC_BG != 0, "the later currentcolor writes the CC_BG marker"); /* SR849 */
    assert!(s.paint.background.is_some(), "armed: blue parsed");
    assert_eq!(s.paint.inherit, 0);
    let s = styled(&[("font-style", "italic"), ("font-style", "currentcolor")]);
    assert_eq!(s.paint.inherit, 0, "font-style: currentcolor sets no bit");
}

// ---- AETHERCSSCURRENTCOLORREST (LEDGER SR849): border / background currentColor marker (named tail block) ----

/// AETHERCSSCURRENTCOLORREST (SR849): the border arms write the side bits of `PaintStyle::currentcolor`: an explicit
/// `currentcolor` and an omitted colour (css-backgrounds-3 §3.1: border-color's initial value is currentcolor) set
/// them, a concrete colour clears them; the side shorthand touches its own bit only; `border-color: currentcolor` and
/// `border-width` alone build 1px / grey-carrier sides with the bits set (SR1069: `border-width` alone builds none; a style does).
#[test]
fn currentcolorrest_border_marker() {
    use layout_core::paint::CC_SIDES;
    fn cc(d: &[(&str, &str)]) -> u8 {
        styled(d).paint.currentcolor
    }
    assert_eq!(cc(&[("border", "2px solid currentcolor")]), CC_SIDES, "explicit currentcolor");
    assert_eq!(cc(&[("border", "2px solid currentColor")]), CC_SIDES, "case-insensitive");
    assert_eq!(cc(&[("border", "2px solid red")]), 0, "a concrete colour (armed)");
    assert_eq!(cc(&[("border", "2px solid")]), CC_SIDES, "an omitted colour is the initial currentcolor");
    assert_eq!(cc(&[("border", "2px solid currentcolor"), ("border-color", "blue")]), 0, "a later border-color clears");
    assert_eq!(cc(&[("border", "2px solid blue"), ("border-left", "2px solid currentcolor")]), 8, "the left bit only");
    assert_eq!(cc(&[("border", "2px solid currentcolor"), ("border-top", "2px solid blue")]), CC_SIDES & !1, "the top bit cleared");
    let s = styled(&[("border-color", "currentcolor")]);
    assert_eq!(s.paint.currentcolor, 0, "SR1098 (re-pinned in place): an absent side carries no live bit; the pending entry carries the answer and creation sets the bit (was CC_SIDES)");
    assert_eq!((s.paint.border, s.paint.border_pending), (None, Some([Some(((128, 128, 128), true)); 4])), "SR1058: no frame built (style none); the grey carrier pends with its currentcolor answer");
    assert_eq!(cc(&[("border-width", "3px")]), 0, "SR1069 (flipped in place): border-width alone creates no side, no bit (was CC_SIDES)");
    assert_eq!(cc(&[("border-width", "3px"), ("border-style", "solid")]), CC_SIDES, "SR1069: the style-created sides take currentcolor");
    assert_eq!(cc(&[("border", "2px solid red"), ("border-width", "3px")]), 0, "border-width keeps the red sides");
}

/// AETHERCSSCURRENTCOLORREST (SR849): `background-color: currentcolor` writes the carrier, an opaque alpha and CC_BG;
/// a later concrete colour (the longhand or the shorthand) clears it; `transparent` leaves it (alpha 0 paints nothing).
#[test]
fn currentcolorrest_background_marker() {
    use layout_core::paint::CC_BG;
    let s = styled(&[("background-color", "rgba(0, 0, 255, 0.5)"), ("background-color", "currentcolor")]);
    assert_eq!(s.paint.currentcolor, CC_BG);
    assert!(s.paint.background.is_some(), "the carrier makes the background paint");
    assert_eq!(s.paint.bg_alpha, Some(1.0), "currentcolor is opaque here");
    let s = styled(&[("background-color", "currentcolor"), ("background-color", "blue")]);
    assert_eq!(s.paint.currentcolor, 0, "a later colour clears");
    assert_eq!(s.paint.background, styled(&[("background-color", "blue")]).paint.background);
    let s = styled(&[("background-color", "currentcolor"), ("background", "red")]);
    assert_eq!(s.paint.currentcolor, 0, "a later shorthand colour clears");
    assert_eq!(styled(&[("background", "currentcolor")]).paint.currentcolor, CC_BG, "the shorthand's currentcolor");
    assert_eq!(styled(&[("background-color", "red")]).paint.currentcolor, 0, "armed: a colour sets nothing");
}

/// AETHERCSSCURRENTCOLORREST (SR849): merge_paint's carry: the side bits follow a later rule's `border`, CC_BG its
/// `background`; a later rule with neither keeps the earlier bits.
#[test]
fn currentcolorrest_merge_carries() {
    use layout_core::paint::{CC_BG, CC_SIDES};
    let mut dst = styled(&[("border", "2px solid red"), ("background-color", "currentcolor")]);
    assert_eq!(dst.paint.currentcolor, CC_BG);
    merge_paint(&mut dst.paint, &styled(&[("border", "2px solid currentcolor")]).paint);
    assert_eq!(dst.paint.currentcolor, CC_SIDES | CC_BG, "the later border's bits, the background bit kept");
    merge_paint(&mut dst.paint, &styled(&[("color", "blue")]).paint);
    assert_eq!(dst.paint.currentcolor, CC_SIDES | CC_BG, "a rule with neither keeps both");
    merge_paint(&mut dst.paint, &styled(&[("border", "2px solid blue"), ("background-color", "green")]).paint);
    assert_eq!(dst.paint.currentcolor, 0, "later concrete colours clear both");
}

/// AETHERCSSCURRENTCOLORREST (SR849): the paint-time resolution: a set bit takes the ink, a clear one keeps its colour.
#[test]
fn currentcolorrest_ink_kat() {
    use layout_core::paint::{bg_ink, border_ink, CC_BG};
    let g = (128, 128, 128);
    let b = (0, 0, 255);
    let sides = [Some((2.0, g)), Some((2.0, b)), None, Some((1.0, g))];
    let red = (255, 0, 0);
    assert_eq!(border_ink(sides, 1 | 4 | 8, red), [Some((2.0, red)), Some((2.0, b)), None, Some((1.0, red))]);
    assert_eq!(border_ink(sides, 0, red), sides);
    assert_eq!(bg_ink((0, 0, 0), CC_BG, red), red);
    assert_eq!(bg_ink(b, 15, red), b, "side bits do not touch the background");
}

// ---- AETHERCSSBORDERSIDECOLOR (LEDGER SR1014): the border-*-color longhands write one side's colour (named tail block) ----

/// AETHERCSSBORDERSIDECOLOR (SR1014): the colour of a parsed `border: 1px solid <c>` side, so the KATs read the same
/// parse road the arm uses.
fn side_color_rgb(c: &str) -> (u8, u8, u8) {
    styled(&[("border", format!("1px solid {c}").as_str())]).paint.border.expect("border parsed")[0].expect("side").1
}

/// AETHERCSSBORDERSIDECOLOR (SR1014): `border-left-color: red` on a blue border writes the left side only, its width
/// kept, and clears the left currentcolor bit only; each of the four names writes its own side.
#[test]
fn border_side_color_writes_one_side() {
    let (red, blue) = (side_color_rgb("red"), side_color_rgb("blue"));
    assert_ne!(red, blue, "armed: two colours");
    let s = styled(&[("border", "3px solid blue"), ("border-left-color", "red")]);
    assert_eq!(s.paint.border, Some([Some((3.0, blue)), Some((3.0, blue)), Some((3.0, blue)), Some((3.0, red))]));
    assert_eq!(s.paint.currentcolor, 0);
    for (i, name) in ["border-top-color", "border-right-color", "border-bottom-color", "border-left-color"].iter().enumerate() {
        let b = styled(&[("border", "2px solid blue"), (*name, "red")]).paint.border.expect("border");
        for (j, side) in b.iter().enumerate() {
            assert_eq!(*side, Some((2.0, if i == j { red } else { blue })), "{name}: side {j}");
        }
    }
}

/// AETHERCSSBORDERSIDECOLOR (SR1014): `currentcolor` sets that side's bit only (the grey carrier written); a concrete
/// colour after a currentcolor border clears that side's bit only; `initial` is currentcolor (SR832's table).
#[test]
fn border_side_color_currentcolor_bits() {
    use layout_core::paint::CC_SIDES;
    let s = styled(&[("border", "2px solid red"), ("border-left-color", "currentcolor")]);
    assert_eq!(s.paint.currentcolor, 8, "the left bit only");
    assert_eq!(s.paint.border.expect("border")[3], Some((2.0, (128, 128, 128))), "the grey carrier");
    let s = styled(&[("border", "2px solid currentcolor"), ("border-top-color", "red")]);
    assert_eq!(s.paint.currentcolor, CC_SIDES & !1, "the top bit cleared, the others kept");
    let s = styled(&[("border", "2px solid red"), ("border-left-color", "blue"), ("border-left-color", "initial")]);
    assert_eq!(s.paint.currentcolor, 8, "initial is the initial currentcolor");
    let s = styled(&[("border", "2px solid red"), ("border-right-color", "unset")]);
    assert_eq!(s.paint.currentcolor, 2, "unset on a non-inherited longhand is initial");
}

/// AETHERCSSBORDERSIDECOLOR (SR1014): a longhand on a borderless style creates no side (style none: no ink, as
/// Chromium); `transparent` drops that side only; an invalid value or `inherit` leaves the side as it was.
/// SR1112 (flipped in place): `transparent` keeps the side (black RGB) at alpha 0 on that side only.
#[test]
fn border_side_color_alone_transparent_invalid() {
    assert_eq!(styled(&[("border-top-color", "red")]).paint.border, None, "no border created");
    assert_eq!(styled(&[("border-top-color", "red")]).paint.currentcolor, 0);
    let s = styled(&[("border", "2px solid blue"), ("border-left", "none"), ("border-left-color", "red")]);
    assert_eq!(s.paint.border.expect("border")[3], None, "an absent side stays absent");
    let blue = side_color_rgb("blue");
    let s = styled(&[("border", "2px solid blue"), ("border-bottom-color", "transparent")]);
    assert_eq!(s.paint.border, Some([Some((2.0, blue)), Some((2.0, blue)), Some((2.0, (0, 0, 0))), Some((2.0, blue))]), "SR1112: the bottom kept (was None)");
    assert_eq!(s.paint.border_alpha, Some([Some(1.0), Some(1.0), Some(0.0), Some(1.0)]), "SR1112: alpha 0 on the bottom only");
    let s = styled(&[("border", "2px solid blue"), ("border-right-color", "rgba(0, 0, 0, 0)")]);
    assert_eq!(s.paint.border.expect("border")[1], Some((2.0, (0, 0, 0))), "SR1112: a zero-alpha rgba is transparent: kept at alpha 0 (was None)");
    assert_eq!(s.paint.border_alpha.expect("alpha")[1], Some(0.0));
    for bad in ["bogus", "inherit", "12px"] {
        let s = styled(&[("border", "2px solid blue"), ("border-left-color", bad)]);
        assert_eq!(s.paint.border, Some([Some((2.0, blue)); 4]), "{bad}: unchanged");
        assert_eq!(s.paint.currentcolor, 0, "{bad}: bits unchanged");
    }
    let s = styled(&[("border", "2px solid blue"), ("border-color", "transparent")]);
    assert_eq!(s.paint.border, Some([Some((2.0, (0, 0, 0))); 4]), "SR1112: border-color: transparent keeps the sides (was Some([None; 4]))");
    assert_eq!(s.paint.border_alpha, Some([Some(0.0); 4]), "SR1112: alpha 0 on every side");
}

// ---- AETHERCSSTEXTDECORATIONCOLOR (LEDGER SR1018): text-decoration-color and the shorthand's colour word (named tail block) ----

/// AETHERCSSTEXTDECORATIONCOLOR (SR1018): the longhand: a colour is `Some(Some(c))`; currentcolor, initial and unset
/// (not inherited: unset == initial) are `Some(None)`; inherit, transparent and an invalid value write nothing.
#[test]
fn text_decoration_color_longhand() {
    let red = Some(Some((255, 0, 0)));
    assert_eq!(styled(&[("text-decoration-color", "red")]).paint.decoration_color, red);
    assert_eq!(styled(&[("text-decoration-color", "rgb(0, 160, 0)")]).paint.decoration_color, Some(Some((0, 160, 0))));
    for v in ["currentcolor", "currentColor", "initial", "unset"] {
        assert_eq!(styled(&[("text-decoration-color", v)]).paint.decoration_color, Some(None), "{v}");
    }
    for v in ["inherit", "transparent", "bogus"] {
        assert_eq!(styled(&[("text-decoration-color", v)]).paint.decoration_color, None, "{v}");
    }
    let s = styled(&[("text-decoration-color", "red"), ("text-decoration-color", "bogus")]);
    assert_eq!(s.paint.decoration_color, red, "an invalid later value is dropped");
    assert_eq!(styled(&[("color", "red")]).paint.decoration_color, None, "armed: color alone writes none");
}

/// AETHERCSSTEXTDECORATIONCOLOR (SR1018): the `text-decoration` shorthand writes its colour word, and without one resets
/// the colour to currentcolor; `text-decoration-line` keeps it.
#[test]
fn text_decoration_color_shorthand() {
    let s = styled(&[("text-decoration", "underline red")]);
    assert_eq!(s.paint.underline, Some(true));
    assert_eq!(s.paint.decoration_color, Some(Some((255, 0, 0))));
    let s = styled(&[("text-decoration", "rgb(0, 160, 0) line-through")]);
    assert_eq!(s.paint.line_through, Some(true));
    assert_eq!(s.paint.decoration_color, Some(Some((0, 160, 0))));
    let s = styled(&[("text-decoration-color", "red"), ("text-decoration", "underline")]);
    assert_eq!(s.paint.decoration_color, Some(None), "the shorthand resets the colour to currentcolor");
    let s = styled(&[("text-decoration-color", "red"), ("text-decoration-line", "underline")]);
    assert_eq!(s.paint.decoration_color, Some(Some((255, 0, 0))), "the line longhand keeps the colour");
    let s = styled(&[("text-decoration", "underline red"), ("text-decoration-color", "currentcolor")]);
    assert_eq!(s.paint.decoration_color, Some(None), "a later longhand wins");
    let s = styled(&[("text-decoration-color", "red"), ("text-decoration", "inherit")]);
    assert_eq!(s.paint.decoration_color, Some(Some((255, 0, 0))), "inherit leaves it");
}

/// AETHERCSSTEXTDECORATIONCOLOR (SR1018): merge_paint carries the colour later-wins; a rule without it keeps the earlier.
#[test]
fn text_decoration_color_merge() {
    let mut dst = styled(&[("text-decoration-color", "red")]);
    merge_paint(&mut dst.paint, &styled(&[("color", "blue")]).paint);
    assert_eq!(dst.paint.decoration_color, Some(Some((255, 0, 0))), "a rule without it keeps red");
    merge_paint(&mut dst.paint, &styled(&[("text-decoration-color", "currentcolor")]).paint);
    assert_eq!(dst.paint.decoration_color, Some(None), "a later currentcolor wins");
    merge_paint(&mut dst.paint, &styled(&[("text-decoration", "underline blue")]).paint);
    assert_eq!(dst.paint.decoration_color, Some(Some((0, 0, 255))), "a later shorthand colour wins");
}

// ---- AETHERFONTSHORTHANDSYSTEM (LEDGER SR1022): the six system-font keywords of font (named tail block) ----

/// AETHERFONTSHORTHANDSYSTEM (SR1022): css-fonts-4 §4.3.1's six system-font keywords.
const SYSTEM_FONT_WORDS: [&str; 6] = ["caption", "icon", "menu", "message-box", "small-caption", "status-bar"];

/// AETHERFONTSHORTHANDSYSTEM (SR1022): `font: menu` after `font: italic bold 20px serif` is the initial road plus the
/// control family at CONTROL_FONT_PX: upright, 400, stretch 100, caps normal, line-height normal, absolute size.
#[test]
fn system_font_sets_the_control_font() {
    let s = styled(&[("font", "italic bold 20px serif"), ("font", "menu")]);
    assert_eq!(s.paint.font_size, Some(layout_core::CONTROL_FONT_PX), "the control size");
    assert_eq!(s.paint.font_size_rel, Some((2, 0.0)), "absolute");
    assert_eq!(s.paint.weight, Some(FontWeight::Abs(400)));
    assert_eq!(s.paint.italic, Some(false));
    assert_eq!(s.paint.stretch, Some(100));
    assert_eq!(s.paint.small_caps, Some(layout_core::paint::Caps::Normal));
    assert_eq!(s.paint.line_height, Some(0.0), "line-height normal (the initial road)");
    assert_eq!(s.paint.family, Some(<TH as layout_core::Host>::control_family()), "the UA control family");
    let twin = styled(&[("font", "italic bold 20px serif")]);
    assert_eq!((twin.paint.font_size, twin.paint.weight, twin.paint.italic), (Some(20.0), Some(FontWeight::Abs(700)), Some(true)), "armed twin");
    assert_ne!(twin.paint.family, s.paint.family, "armed twin keeps serif");
}

/// AETHERFONTSHORTHANDSYSTEM (SR1022): the six keywords are not distinguished (Chromium on Linux).
#[test]
fn system_font_six_words_agree() {
    let menu = font_fields(&font_wide("menu"));
    for w in SYSTEM_FONT_WORDS {
        assert_eq!(font_fields(&font_wide(w)), menu, "font: {w}");
    }
}

/// AETHERFONTSHORTHANDSYSTEM (SR1022): a later longhand still wins by declaration order.
#[test]
fn system_font_later_longhand_wins() {
    let s = styled(&[("font", "menu"), ("font-size", "20px")]);
    assert_eq!(s.paint.font_size, Some(20.0));
    assert_eq!(s.paint.family, Some(<TH as layout_core::Host>::control_family()));
}

/// AETHERFONTSHORTHANDSYSTEM (SR1022): an earlier `2em` size's relative mark does not survive (font_size_rel absolute),
/// and the font-relative replay keeps line-height normal and the control size.
#[test]
fn system_font_replay_keeps_control_size() {
    let mut s = styled(&[("font-size", "2em"), ("line-height", "3"), ("padding", "1em"), ("font", "status-bar")]);
    assert_eq!(s.paint.font_size_rel, Some((2, 0.0)), "the 2em mark is gone");
    let list = s.paint.font_rel.clone().expect("length declarations kept");
    assert!(list.iter().any(|(p, v)| p == "line-height" && v == "normal"), "the line-height entry was replaced: {list:?}");
    let cx = ParseCx::default();
    let spec: SpecifiedStyle<TH> = reapply_declarations(&list, &cx);
    merge_paint(&mut s.paint, &spec.paint);
    assert_eq!(s.paint.line_height, Some(0.0), "the replay does not bring line-height 3 back");
    assert_eq!(s.paint.font_size, Some(layout_core::CONTROL_FONT_PX), "the control size");
}

/// AETHERFONTSHORTHANDSYSTEM (SR1022): an unknown word is still an invalid `font` (dropped whole).
#[test]
fn system_font_bogus_dropped() {
    let s = styled(&[("font-style", "italic"), ("font", "bogus-system")]);
    assert_eq!(s.paint.italic, Some(true));
    assert_eq!(s.paint.font_size, None);
    assert_eq!(s.paint.family, None);
}

// ---- AETHERCSSTEXTDECORATIONSTYLETHICKNESS (LEDGER SR1038): text-decoration-style / -thickness and the shorthand's rest (named tail block) ----

/// AETHERCSSTEXTDECORATIONSTYLETHICKNESS (SR1038): the style longhand: the five words as 0 solid 1 double 2 dotted
/// 3 dashed 4 wavy; initial / unset are solid; an invalid value writes nothing and a later invalid one is dropped.
#[test]
fn text_decoration_style_longhand() {
    for (v, k) in [("solid", 0u8), ("double", 1), ("dotted", 2), ("dashed", 3), ("wavy", 4), ("WAVY", 4), ("initial", 0), ("unset", 0)] {
        assert_eq!(styled(&[("text-decoration-style", v)]).paint.decoration_style, Some(k), "{v}");
    }
    for v in ["inherit", "groove", "bogus", "2px"] {
        assert_eq!(styled(&[("text-decoration-style", v)]).paint.decoration_style, None, "{v}");
    }
    let s = styled(&[("text-decoration-style", "wavy"), ("text-decoration-style", "bogus")]);
    assert_eq!(s.paint.decoration_style, Some(4), "an invalid later value is dropped");
    assert_eq!(styled(&[("text-decoration-line", "underline")]).paint.decoration_style, None, "armed: the line longhand writes none");
}

/// AETHERCSSTEXTDECORATIONSTYLETHICKNESS (SR1038): the thickness longhand: a length in px (em at the parse context's
/// font), 0 accepted; auto / from-font / initial / unset are `Some(None)` (the face's); a negative length, a bare non-zero
/// number, a percentage and an invalid word write nothing.
#[test]
fn text_decoration_thickness_longhand() {
    assert_eq!(styled(&[("text-decoration-thickness", "2px")]).paint.decoration_thickness, Some(Some(2.0)));
    assert_eq!(styled(&[("text-decoration-thickness", "0")]).paint.decoration_thickness, Some(Some(0.0)));
    let em = ParseCx::default().font.0 * 0.1;
    let t = styled(&[("text-decoration-thickness", "0.1em")]).paint.decoration_thickness.flatten().expect("0.1em parses");
    assert!((t - em).abs() < 1e-4, "0.1em = {t}, want {em}");
    for v in ["auto", "from-font", "initial", "unset"] {
        assert_eq!(styled(&[("text-decoration-thickness", v)]).paint.decoration_thickness, Some(None), "{v}");
    }
    for v in ["-1px", "3", "10%", "bogus", "inherit"] {
        assert_eq!(styled(&[("text-decoration-thickness", v)]).paint.decoration_thickness, None, "{v}");
    }
}

/// AETHERCSSTEXTDECORATIONSTYLETHICKNESS (SR1038): the shorthand reads line, style, colour and thickness in any order in
/// its ONE pass; the omitted ones reset (solid, auto, currentcolor); `inherit` leaves all three; the line longhand none.
#[test]
fn text_decoration_shorthand_rest_any_order() {
    for v in ["underline wavy red 2px", "2px red wavy underline", "red underline 2px wavy", "wavy 2px underline red"] {
        let s = styled(&[("text-decoration", v)]);
        assert_eq!(s.paint.underline, Some(true), "{v}");
        assert_eq!(s.paint.decoration_style, Some(4), "{v}");
        assert_eq!(s.paint.decoration_color, Some(Some((255, 0, 0))), "{v}");
        assert_eq!(s.paint.decoration_thickness, Some(Some(2.0)), "{v}");
    }
    let s = styled(&[("text-decoration", "line-through dotted from-font")]);
    assert_eq!(s.paint.line_through, Some(true));
    assert_eq!(s.paint.decoration_style, Some(2));
    assert_eq!(s.paint.decoration_thickness, Some(None));
    let s = styled(&[("text-decoration-style", "wavy"), ("text-decoration-thickness", "3px"), ("text-decoration", "underline")]);
    assert_eq!(s.paint.decoration_style, Some(0), "the shorthand resets the style to solid");
    assert_eq!(s.paint.decoration_thickness, Some(None), "the shorthand resets the thickness to auto");
    let s = styled(&[("text-decoration-style", "wavy"), ("text-decoration-thickness", "3px"), ("text-decoration-line", "underline")]);
    assert_eq!(s.paint.decoration_style, Some(4), "the line longhand keeps the style");
    assert_eq!(s.paint.decoration_thickness, Some(Some(3.0)), "the line longhand keeps the thickness");
    let s = styled(&[("text-decoration-style", "dashed"), ("text-decoration", "inherit")]);
    assert_eq!(s.paint.decoration_style, Some(3), "inherit leaves it");
    let s = styled(&[("text-decoration", "underline double"), ("text-decoration-style", "dotted")]);
    assert_eq!(s.paint.decoration_style, Some(2), "a later longhand wins");
}

/// AETHERCSSTEXTDECORATIONSTYLETHICKNESS (SR1038): merge_paint carries both later-wins; a rule without them keeps them.
#[test]
fn text_decoration_style_thickness_merge() {
    let mut dst = styled(&[("text-decoration-style", "wavy"), ("text-decoration-thickness", "4px")]);
    merge_paint(&mut dst.paint, &styled(&[("color", "blue")]).paint);
    assert_eq!((dst.paint.decoration_style, dst.paint.decoration_thickness), (Some(4), Some(Some(4.0))), "a rule without them keeps them");
    merge_paint(&mut dst.paint, &styled(&[("text-decoration", "underline")]).paint);
    assert_eq!((dst.paint.decoration_style, dst.paint.decoration_thickness), (Some(0), Some(None)), "a later shorthand resets both");
    merge_paint(&mut dst.paint, &styled(&[("text-decoration-style", "double")]).paint);
    assert_eq!(dst.paint.decoration_style, Some(1), "a later longhand wins");
}

// ---- AETHERCSSTEXTUNDERLINEOFFSET (LEDGER SR1045): text-underline-offset / -position arms (named tail block) ----

/// AETHERCSSTEXTUNDERLINEOFFSET (SR1045): the offset longhand: a length in px (negative and 0 accepted; em at the parse
/// context's font), a percentage of 1em; auto / initial / unset are `Some(None)`; a bare non-zero number, an invalid
/// word and `inherit` (not inherited, no bit) write nothing.
#[test]
fn text_underline_offset_longhand() {
    assert_eq!(styled(&[("text-underline-offset", "3px")]).paint.underline_offset, Some(Some(3.0)));
    assert_eq!(styled(&[("text-underline-offset", "-2px")]).paint.underline_offset, Some(Some(-2.0)));
    assert_eq!(styled(&[("text-underline-offset", "0")]).paint.underline_offset, Some(Some(0.0)));
    let em = ParseCx::default().font.0 * 0.2;
    for v in ["0.2em", "20%"] {
        let o = styled(&[("text-underline-offset", v)]).paint.underline_offset.flatten().expect("parses");
        assert!((o - em).abs() < 1e-3, "{v} = {o}, want {em}");
    }
    for v in ["auto", "initial", "unset"] {
        assert_eq!(styled(&[("text-underline-offset", v)]).paint.underline_offset, Some(None), "{v}");
    }
    for v in ["3", "bogus", "inherit"] {
        assert_eq!(styled(&[("text-underline-offset", v)]).paint.underline_offset, None, "{v}");
    }
    let s = styled(&[("text-underline-offset", "5px"), ("text-underline-offset", "initial")]);
    assert_eq!(s.paint.underline_offset, Some(None), "initial resets an earlier length to auto");
}

/// AETHERCSSTEXTUNDERLINEOFFSET (SR1045): the position longhand: auto / initial 0, from-font 1, under 2; left / right keep
/// the horizontal part (bare = auto); invalid combinations write nothing; inherit / unset write the inherit bit and clear
/// an earlier value, and a later value beats the bit.
#[test]
fn text_underline_position_longhand() {
    use layout_core::paint::INHERIT_UNDERLINE_POSITION;
    for (v, want) in [("auto", 0), ("initial", 0), ("from-font", 1), ("under", 2), ("under left", 2), ("right under", 2), ("from-font right", 1), ("left", 0), ("right", 0)] {
        assert_eq!(styled(&[("text-underline-position", v)]).paint.underline_position, Some(want), "{v}");
    }
    for v in ["bogus", "under from-font", "left right", "under under"] {
        assert_eq!(styled(&[("text-underline-position", v)]).paint.underline_position, None, "{v}");
    }
    for v in ["inherit", "unset"] {
        let s = styled(&[("text-underline-position", "under"), ("text-underline-position", v)]);
        assert_eq!(s.paint.underline_position, None, "{v} clears the earlier under");
        assert!(s.paint.inherits(INHERIT_UNDERLINE_POSITION), "{v} writes the inherit bit");
    }
    let s = styled(&[("text-underline-position", "inherit"), ("text-underline-position", "under")]);
    assert_eq!(s.paint.underline_position, Some(2));
    assert!(!s.paint.inherits(INHERIT_UNDERLINE_POSITION), "a later value beats the bit");
    assert!(!styled(&[("text-underline-position", "under")]).paint.inherits(INHERIT_UNDERLINE_POSITION), "armed");
}

/// AETHERCSSTEXTUNDERLINEOFFSET (SR1045): merge_paint carries both later-wins; the `text-decoration` shorthand (which does
/// not include them) leaves both; a later `inherit` on the position clears it through the bit.
#[test]
fn text_underline_merge() {
    let mut dst = styled(&[("text-underline-offset", "4px"), ("text-underline-position", "under")]);
    merge_paint(&mut dst.paint, &styled(&[("color", "blue")]).paint);
    assert_eq!((dst.paint.underline_offset, dst.paint.underline_position), (Some(Some(4.0)), Some(2)), "a rule without them keeps them");
    merge_paint(&mut dst.paint, &styled(&[("text-decoration", "underline wavy")]).paint);
    assert_eq!((dst.paint.underline_offset, dst.paint.underline_position), (Some(Some(4.0)), Some(2)), "the shorthand leaves them");
    merge_paint(&mut dst.paint, &styled(&[("text-underline-offset", "auto")]).paint);
    assert_eq!(dst.paint.underline_offset, Some(None), "a later auto wins");
    merge_paint(&mut dst.paint, &styled(&[("text-underline-position", "inherit")]).paint);
    assert_eq!(dst.paint.underline_position, None, "a later inherit clears it");
    assert!(dst.paint.inherits(layout_core::paint::INHERIT_UNDERLINE_POSITION));
}