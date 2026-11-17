//! AETHERCSSWIDENONINHERITEDINITIAL (SR832): css-cascade-4 §7.3.1 / §7.3.3 — `initial`, and `unset` on a NON-inherited
//! property, write the property's initial value, so a later rule's keyword beats an earlier rule's value through
//! `merge_specified` / `merge_paint` (which copy only Somes). `revert` and `inherit` stay as they were (owed), and an
//! inherited property is untouched by this arc.
mod common;
use common::*;
use layout_core::css::{apply_declaration, merge_specified, ParseCx, SpecifiedStyle};

fn styled(decls: &[(&str, &str)]) -> SpecifiedStyle<TH> {
    let cx = ParseCx::default();
    let mut s = SpecifiedStyle::<TH>::default();
    for (prop, value) in decls {
        apply_declaration(prop, value, &mut s, &cx);
    }
    s
}

/// The rules in cascade order, each its own block, overlaid as the cascade does.
fn cascade(rules: &[&[(&str, &str)]]) -> SpecifiedStyle<TH> {
    let mut dst = SpecifiedStyle::<TH>::default();
    for r in rules {
        merge_specified(&mut dst, &styled(r));
    }
    dst
}

#[test]
fn css_wide_initial_margin_beats_earlier_rule() {
    let zero = styled(&[("margin", "0px")]).margin;
    assert!(zero.iter().all(|m| m.is_some()), "the reference zero margin is four Somes");
    let before = cascade(&[&[("margin", "20px")]]);
    assert_ne!(before.margin, zero, "the earlier rule's 20px (armed)");
    for kw in ["initial", "unset", "INITIAL"] {
        let s = cascade(&[&[("margin", "20px")], &[("margin", kw)]]);
        assert_eq!(s.margin, zero, "margin: {kw} after margin: 20px is 0 on every side");
    }
    for (i, side) in ["margin-top", "margin-right", "margin-bottom", "margin-left"].iter().enumerate() {
        let s = cascade(&[&[("margin", "20px")], &[(side, "initial")]]);
        assert_eq!(s.margin[i], zero[i], "{side}: initial is 0");
        assert_eq!(s.margin[(i + 1) % 4], before.margin[(i + 1) % 4], "{side}: the other sides keep 20px");
    }
}

#[test]
fn css_wide_initial_padding_unset() {
    let zero = styled(&[("padding", "0px")]).padding;
    let s = cascade(&[&[("padding", "10px")], &[("padding", "unset")]]);
    assert_eq!(s.padding, zero, "padding: unset after padding: 10px is 0");
    let s = cascade(&[&[("padding", "10px")], &[("padding-left", "initial")]]);
    assert_eq!(s.padding[3], zero[3], "padding-left: initial is 0");
    assert_ne!(s.padding[0], zero[0], "padding-top keeps 10px (armed)");
}

#[test]
fn css_wide_initial_display_is_inline() {
    let inline = styled(&[("display", "inline")]);
    for kw in ["initial", "unset"] {
        let s = cascade(&[&[("display", "flex")], &[("display", kw)]]);
        assert_eq!(s.paint.display_kind, Some(0), "display: {kw} is inline (not the tag road's None)");
        assert_eq!(s.paint.list_item, Some(false), "display: {kw}");
        assert_eq!(s.display, inline.display, "display: {kw}: the inline box");
        assert_eq!(s.flex_container, Some(false), "display: {kw}: the earlier flex is gone");
    }
    let none = cascade(&[&[("display", "flex")], &[("display", "none")]]);
    assert_eq!(none.display, styled(&[("display", "none")]).display, "display: none unchanged");
    assert_eq!(styled(&[("display", "none")]).paint.display_kind, None, "display: none keeps its None display_kind");
    assert_ne!(none.display, inline.display, "none is not inline (armed)");
}

#[test]
fn css_wide_initial_background_color_transparent() {
    let red = cascade(&[&[("background-color", "red")]]);
    assert_eq!(red.paint.bg_alpha, Some(1.0), "the earlier red is opaque (armed)");
    let s = cascade(&[&[("background-color", "red")], &[("background-color", "initial")]]);
    assert_eq!(s.paint.bg_alpha, Some(0.0), "background-color: initial is transparent");
    let s = cascade(&[&[("background-color", "red")], &[("background", "unset")]]);
    assert_eq!(s.paint.bg_alpha, Some(0.0), "background: unset clears the colour layer");
}

/// The rewrite of `background: initial` is the colour only: the image layer an earlier rule set stays, as it does
/// today (AETHER-CSS-WIDE-NONINHERITED-REST owed).
#[test]
fn css_wide_initial_background_keeps_image_layer() {
    let s = cascade(&[&[("background", "url(a.png) no-repeat")], &[("background", "initial")]]);
    assert_eq!(s.paint.bg_image.as_deref(), Some("a.png"), "the image layer stays (pinned, owed)");
    assert_eq!(s.paint.bg_alpha, Some(0.0));
}

#[test]
fn css_wide_initial_width_unset_is_auto() {
    let s = cascade(&[&[("width", "100px")], &[("width", "unset")]]);
    assert_eq!(s.width, styled(&[("width", "auto")]).width, "width: unset is auto");
    assert_eq!(s.paint.has_width, Some(false), "an auto width is not a specified width");
    assert_ne!(cascade(&[&[("width", "100px")]]).width, s.width, "the earlier 100px (armed)");
    for prop in ["height", "min-width", "min-height", "max-width", "max-height"] {
        let s = cascade(&[&[(prop, "100px")], &[(prop, "initial")]]);
        let auto = styled(&[(prop, "auto")]);
        let got = match prop { "height" => s.height, "min-width" => s.min_width, "min-height" => s.min_height, "max-width" => s.max_width, _ => s.max_height };
        let want = match prop { "height" => auto.height, "min-width" => auto.min_width, "min-height" => auto.min_height, "max-width" => auto.max_width, _ => auto.max_height };
        assert_eq!(got, want, "{prop}: initial is auto (taffy's auto max is no maximum)");
        assert!(got.is_some(), "{prop}: a Some, so it beats the earlier rule");
    }
}

#[test]
fn css_wide_initial_border_none() {
    let b = cascade(&[&[("border", "2px solid red")]]);
    assert!(b.paint.border.is_some_and(|s| s.iter().all(|x| x.is_some())), "the earlier border (armed)");
    for kw in ["initial", "unset"] {
        let s = cascade(&[&[("border", "2px solid red")], &[("border", kw)]]);
        assert_eq!(s.paint.border, Some([None; 4]), "border: {kw} paints no border");
        let s = cascade(&[&[("border", "2px solid red")], &[("border-style", kw)]]);
        assert_eq!(s.paint.border, Some([None; 4]), "border-style: {kw} is none");
    }
    let s = styled(&[("border", "2px solid red"), ("border-left", "initial")]);
    let sides = s.paint.border.expect("sides");
    assert!(sides[3].is_none() && sides[0].is_some(), "border-left: initial in the block drops only the left side");
}

/// `border-width: initial` is medium: ONE literal, 3px (pinned).
#[test]
fn css_wide_initial_border_width_medium() {
    let s = cascade(&[&[("border", "2px solid red")], &[("border-width", "initial")]]);
    let sides = s.paint.border.expect("sides");
    assert!(sides.iter().all(|x| x.is_some_and(|(w, _)| w == 3.0)), "medium is 3px: {sides:?}");
}

/// Left as they were (owed): `revert` (AETHER-CSS-WIDE-REVERT-UA), `inherit` on a non-inherited property
/// (AETHER-CSS-WIDE-NONINHERITED-INHERIT); `color: initial` is pinned as TODAY (a no-op: the earlier colour stands);
/// an inherited property's `unset` is not rewritten.
#[test]
fn css_wide_initial_leaves_the_rest() {
    let before = cascade(&[&[("margin", "20px")]]);
    for kw in ["revert", "inherit"] {
        let s = cascade(&[&[("margin", "20px")], &[("margin", kw)]]);
        assert_eq!(s.margin, before.margin, "margin: {kw} is unchanged by this arc");
    }
    let s = cascade(&[&[("color", "blue")], &[("color", "initial")]]);
    assert_eq!(s.paint.color, Some((0, 0, 255)), "color: initial is a no-op today (pinned)");
    let s = cascade(&[&[("letter-spacing", "2px")], &[("letter-spacing", "unset")]]);
    assert_eq!(s.paint.letter_spacing, cascade(&[&[("letter-spacing", "2px")]]).paint.letter_spacing, "an inherited unset is not rewritten to 0px");
}

// ---- AETHERCSSWIDECOLORCONTROLSDARK (LEDGER SR1031): `color-scheme` parsed into PaintStyle::color_scheme; the ONE UA colour table (named tail block) ----

/// AETHERCSSWIDECOLORCONTROLSDARK (SR1031): css-color-adjust-1 §2.1 values as bits (bit 0 light, bit 1 dark): `normal`
/// / `initial` Some(0) (a value, so it resets under a dark root), `only` and custom idents ignored.
#[test]
fn color_scheme_parses_bits() {
    let cs = |v: &str| styled(&[("color-scheme", v)]).paint.color_scheme;
    assert_eq!(cs("dark"), Some(2), "dark");
    assert_eq!(cs("light"), Some(1), "light");
    assert_eq!(cs("light dark"), Some(3), "light dark: both bits");
    assert_eq!(cs("dark light"), Some(3), "dark light: both bits");
    assert_eq!(cs("only dark"), Some(2), "only is ignored");
    assert_eq!(cs("dark only"), Some(2), "only after the keyword");
    assert_eq!(cs("foo dark"), Some(2), "a custom ident is ignored");
    assert_eq!(cs("normal"), Some(0), "normal is the no-bit value, not None");
    assert_eq!(cs("initial"), Some(0), "initial is normal");
    assert_eq!(styled(&[]).paint.color_scheme, None, "unspecified is None (armed)");
}

/// AETHERCSSWIDECOLORCONTROLSDARK (SR1031): an invalid value drops the declaration: an earlier value stands.
#[test]
fn color_scheme_invalid_dropped() {
    for v in ["5px", "normal dark", "only", "only only dark", "dark, light", "revert"] {
        assert_eq!(styled(&[("color-scheme", v)]).paint.color_scheme, None, "{v:?} is invalid");
        assert_eq!(styled(&[("color-scheme", "dark"), ("color-scheme", v)]).paint.color_scheme, Some(2), "{v:?} leaves the earlier dark");
    }
}

/// AETHERCSSWIDECOLORCONTROLSDARK (SR1031): `inherit` / `unset` (an inherited property) write the INHERIT_COLOR_SCHEME
/// bit past an earlier rule's value; a later value beats the bit.
#[test]
fn color_scheme_inherit_bit() {
    use layout_core::paint::{inherit_bit, INHERIT_COLOR_SCHEME};
    assert_eq!(inherit_bit("color-scheme"), INHERIT_COLOR_SCHEME);
    for kw in ["inherit", "unset"] {
        let s = cascade(&[&[("color-scheme", "dark")], &[("color-scheme", kw)]]);
        assert_eq!(s.paint.color_scheme, None, "color-scheme: {kw} drops the earlier dark");
        assert!(s.paint.inherits(INHERIT_COLOR_SCHEME), "color-scheme: {kw} sets the inherit bit");
    }
    let s = cascade(&[&[("color-scheme", "inherit")], &[("color-scheme", "dark")]]);
    assert_eq!(s.paint.color_scheme, Some(2), "a later dark beats the inherit");
    assert!(!s.paint.inherits(INHERIT_COLOR_SCHEME), "the later value clears the bit");
    let s = cascade(&[&[("color-scheme", "dark")], &[("color", "red")]]);
    assert_eq!(s.paint.color_scheme, Some(2), "a later rule without color-scheme keeps it (merge_paint carries the field)");
}

/// AETHERCSSWIDECOLORCONTROLSDARK (SR1031): the ONE table: dark only for dark-and-not-light; `light dark`, `normal`
/// and unspecified take the light row; BUTTON_FACE is the light row's face.
#[test]
fn color_scheme_ua_colours_rows() {
    use layout_core::{scheme_dark, ua_colours, BUTTON_FACE, UA_DARK, UA_LIGHT};
    assert_eq!(*ua_colours(Some(2)), UA_DARK);
    for s in [Some(3), Some(1), Some(0), None] {
        assert_eq!(*ua_colours(s), UA_LIGHT, "{s:?} is light");
        assert!(!scheme_dark(s));
    }
    assert!(scheme_dark(Some(2)));
    assert_eq!(BUTTON_FACE, UA_LIGHT.face);
    assert_eq!(UA_LIGHT.face, (239, 239, 239));
    assert_eq!((UA_DARK.face, UA_DARK.field, UA_DARK.canvas, UA_DARK.ink), ((107, 107, 107), (59, 59, 59), (18, 18, 18), (255, 255, 255)));
}
