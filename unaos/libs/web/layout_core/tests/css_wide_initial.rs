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

/// `background: initial` resets the colour AND the image layer an earlier rule set (SR1015: the rewrite is `none`;
/// before it the image stayed).
#[test]
fn css_wide_initial_background_keeps_image_layer() {
    let s = cascade(&[&[("background", "url(a.png) no-repeat")], &[("background", "initial")]]);
    assert_eq!(s.paint.bg_image.as_deref(), None /* SR1015 */, "background: initial resets the image layer");
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
    for kw in ["revert" /* SR1007 */] {
        let s = cascade(&[&[("margin", "20px")], &[("margin", kw)]]);
        assert_eq!(s.margin, before.margin, "margin: {kw} is unchanged by this arc");
    }
    let s = cascade(&[&[("color", "blue")], &[("color", "initial")]]);
    assert_eq!(s.paint.color, Some((0, 0, 255)), "color: initial is a no-op today (pinned)");
    let s = cascade(&[&[("letter-spacing", "2px")], &[("letter-spacing", "unset")]]);
    assert_eq!(s.paint.letter_spacing, cascade(&[&[("letter-spacing", "2px")]]).paint.letter_spacing, "an inherited unset is not rewritten to 0px");
}

// ---- AETHERCSSWIDENONINHERITEDREST (LEDGER SR848): initial / unset on the rest of the non-inherited arms (named tail block) ----

/// AETHERCSSWIDENONINHERITEDREST (SR848): `position: initial` / `unset` after `absolute` is static (in flow); `z-index:
/// initial` is auto; `float` / `clear: initial` are none.
#[test]
fn css_wide_rest_position_float() {
    let stat = styled(&[("position", "static")]);
    assert_ne!(cascade(&[&[("position", "absolute")]]).position, stat.position, "the earlier absolute (armed)");
    for kw in ["initial", "unset"] {
        let s = cascade(&[&[("position", "absolute")], &[("position", kw)]]);
        assert_eq!((s.position, s.paint.position_kind), (stat.position, Some(0)), "position: {kw} is static");
    }
    let s = cascade(&[&[("z-index", "5")], &[("z-index", "initial")]]);
    assert_eq!(s.paint.z_index, Some(None), "z-index: initial is auto");
    let s = cascade(&[&[("float", "left")], &[("float", "initial")]]);
    assert_eq!(s.paint.float, Some(0), "float: initial is none");
    let s = cascade(&[&[("clear", "both")], &[("clear", "initial")]]);
    assert_eq!(s.paint.clear, Some(0), "clear: initial is none");
}

/// AETHERCSSWIDENONINHERITEDREST (SR848): `overflow: initial` / `unset` after `hidden` is visible and stops clipping;
/// an axis longhand resets its own axis.
#[test]
fn css_wide_rest_overflow_visible() {
    let vis = styled(&[("overflow", "visible")]).paint.overflow;
    assert_ne!(cascade(&[&[("overflow", "hidden")]]).paint.overflow, vis, "the earlier hidden (armed)");
    for kw in ["initial", "unset"] {
        let s = cascade(&[&[("overflow", "hidden")], &[("overflow", kw)]]);
        assert_eq!((s.paint.overflow, s.paint.clip), (vis, Some(false)), "overflow: {kw} is visible");
    }
    let s = styled(&[("overflow", "hidden"), ("overflow-y", "initial")]);
    let o = s.paint.overflow.expect("overflow");
    assert!(o[1] == vis.unwrap()[1] && o[0] != vis.unwrap()[0], "overflow-y: initial resets only y: {o:?}");
}

/// AETHERCSSWIDENONINHERITEDREST (SR848): `opacity: initial` after 0.5 is 1 (unhidden); `transform: initial` after a
/// translate is none (the anim table's literals).
#[test]
fn css_wide_rest_opacity_transform() {
    let s = cascade(&[&[("opacity", "0")], &[("opacity", "initial")]]);
    assert_eq!((s.paint.opacity, s.paint.hidden), (Some(1.0), Some(false)), "opacity: initial is 1");
    let t = cascade(&[&[("transform", "translateX(100px)")]]);
    assert!(t.paint.transform.as_ref().is_some_and(|l| !l.is_empty()), "the earlier translate (armed)");
    let s = cascade(&[&[("transform", "translateX(100px)")], &[("transform", "initial")]]);
    assert_eq!(s.paint.transform, Some(Vec::new()), "transform: initial is none");
}

/// AETHERCSSWIDENONINHERITEDREST (SR848): each inset longhand and the `inset` shorthand reset to auto.
#[test]
fn css_wide_rest_insets_auto() {
    let auto = styled(&[("inset", "auto")]);
    for (i, side) in ["top", "right", "bottom", "left"].iter().enumerate() {
        let s = cascade(&[&[(side, "10px")], &[(side, "initial")]]);
        let got = [s.inset_top, s.inset_right, s.inset_bottom, s.inset_left][i];
        assert_eq!(got, auto.inset_top, "{side}: initial is auto (a Some, so it beats 10px)");
        assert!(got.is_some());
    }
    let s = cascade(&[&[("inset", "10px")], &[("inset", "initial")]]);
    assert_eq!([s.inset_top, s.inset_right, s.inset_bottom, s.inset_left], [auto.inset_top; 4], "inset: initial is auto");
}

/// AETHERCSSWIDENONINHERITEDREST (SR848): the flex longhands and shorthands reset to row / nowrap / 0 / 1 / auto.
#[test]
fn css_wide_rest_flex() {
    let row = styled(&[("flex-direction", "row")]).flex_direction;
    assert_ne!(cascade(&[&[("flex-direction", "column")]]).flex_direction, row, "the earlier column (armed)");
    for kw in ["initial", "unset"] {
        let s = cascade(&[&[("flex-direction", "column")], &[("flex-direction", kw)]]);
        assert_eq!(s.flex_direction, row, "flex-direction: {kw} is row");
    }
    let nowrap = styled(&[("flex-wrap", "nowrap")]).flex_wrap;
    let s = cascade(&[&[("flex-wrap", "wrap")], &[("flex-wrap", "initial")]]);
    assert_eq!(s.flex_wrap, nowrap, "flex-wrap: initial is nowrap");
    let s = cascade(&[&[("flex-flow", "column wrap")], &[("flex-flow", "initial")]]);
    assert_eq!((s.flex_direction, s.flex_wrap), (row, nowrap), "flex-flow: initial is row nowrap");
    let s = cascade(&[&[("flex-grow", "3")], &[("flex-grow", "initial")]]);
    assert_eq!(s.flex_grow, Some(0.0), "flex-grow: initial is 0");
    let s = cascade(&[&[("flex-shrink", "0")], &[("flex-shrink", "initial")]]);
    assert_eq!(s.flex_shrink, Some(1.0), "flex-shrink: initial is 1");
    let auto = styled(&[("flex-basis", "auto")]).flex_basis;
    let s = cascade(&[&[("flex-basis", "50px")], &[("flex-basis", "initial")]]);
    assert_eq!(s.flex_basis, auto, "flex-basis: initial is auto");
    let s = cascade(&[&[("flex", "2 0 10px")], &[("flex", "unset")]]);
    assert_eq!((s.flex_grow, s.flex_shrink, s.flex_basis), (Some(0.0), Some(1.0), auto), "flex: unset is 0 1 auto");
}

/// AETHERCSSWIDENONINHERITEDREST (SR848): align-items / justify-content reset to normal, gap / row-gap to 0;
/// column-gap's keyword stays dropped (AETHER-CSS-WIDE-COLUMN-GAP-NORMAL owed, pinned).
#[test]
fn css_wide_rest_align_gap() {
    let n = styled(&[("align-items", "normal"), ("justify-content", "normal")]);
    let s = cascade(&[&[("align-items", "center"), ("justify-content", "center")], &[("align-items", "initial"), ("justify-content", "initial")]]);
    assert_eq!(s.align_items, n.align_items, "align-items: initial is normal");
    assert_eq!(s.justify, n.justify, "justify-content: initial is normal");
    let zero = styled(&[("gap", "0px")]);
    let s = cascade(&[&[("gap", "10px")], &[("gap", "initial")]]);
    assert_eq!((s.row_gap, s.column_gap), (zero.row_gap, zero.column_gap), "gap: initial is 0");
    let s = cascade(&[&[("row-gap", "10px")], &[("row-gap", "initial")]]);
    assert_eq!(s.row_gap, zero.row_gap, "row-gap: initial is 0");
    let ten = cascade(&[&[("column-gap", "10px")]]);
    let s = cascade(&[&[("column-gap", "10px")], &[("column-gap", "initial")]]);
    assert_eq!(s.column_gap, ten.column_gap, "column-gap: initial unchanged by this arc (owed, pinned)");
}

/// AETHERCSSWIDENONINHERITEDREST (SR848): the background layer longhands reset to repeat / `0% 0%` / auto; the
/// shorthand keeps SR832's colour-only rewrite (AETHER-CSS-WIDE-BACKGROUND-IMAGE-NONE owed).
#[test]
fn css_wide_rest_background_layers() {
    let s = cascade(&[&[("background-repeat", "no-repeat")], &[("background-repeat", "initial")]]);
    assert_eq!(s.paint.bg_repeat, Some(0), "background-repeat: initial is repeat");
    let s = cascade(&[&[("background-position", "10px 20px")], &[("background-position", "initial")]]);
    assert_eq!(s.paint.bg_position.as_deref(), Some("0% 0%"), "background-position: initial is 0% 0%");
    let s = cascade(&[&[("background-size", "cover")], &[("background-size", "initial")]]);
    assert_eq!(s.paint.bg_size.as_deref(), Some("auto"), "background-size: initial is auto");
}

// ---- AETHERCSSWIDEBACKGROUNDIMAGENONE (LEDGER SR1015): an explicit background-image none clears an earlier rule's image (named tail block) ----

/// AETHERCSSWIDEBACKGROUNDIMAGENONE (SR1015): `background-image: none` (and `initial` / `unset`, rewritten to none)
/// after a url clears the image and marks the gradient layer cleared (Some(None)).
#[test]
fn css_wide_bgnone_image_none_beats_earlier_rule() {
    let before = cascade(&[&[("background-image", "url(a.png)")]]);
    assert_eq!(before.paint.bg_image.as_deref(), Some("a.png"), "the earlier url (armed)");
    for kw in ["none", "initial", "unset", "NONE"] {
        let s = cascade(&[&[("background-image", "url(a.png)")], &[("background-image", kw)]]);
        assert_eq!(s.paint.bg_image, None, "background-image: {kw} clears the earlier url");
        assert!(matches!(s.paint.bg_gradient, Some(None)), "background-image: {kw} clears the gradient layer");
    }
}

/// The shorthand's empty spellings reset every layer: no image, transparent, repeat / `0% 0%` / auto.
#[test]
fn css_wide_bgnone_shorthand_resets_layers() {
    let first: &[(&str, &str)] = &[("background", "url(a.png) red no-repeat 10px 20px / cover")];
    let before = cascade(&[first]);
    assert_eq!(before.paint.bg_image.as_deref(), Some("a.png"), "the earlier image (armed)");
    assert_eq!(before.paint.bg_repeat, Some(1), "the earlier no-repeat (armed)");
    for kw in ["none", "initial", "unset", "transparent", "none transparent"] {
        let s = cascade(&[first, &[("background", kw)]]);
        assert_eq!(s.paint.bg_image, None, "background: {kw}: no image");
        assert_eq!(s.paint.bg_alpha, Some(0.0), "background: {kw}: transparent");
        assert_eq!(s.paint.bg_repeat, Some(0), "background: {kw}: repeat");
        assert_eq!(s.paint.bg_position.as_deref(), Some("0% 0%"), "background: {kw}: 0% 0%");
        assert_eq!(s.paint.bg_size.as_deref(), Some("auto"), "background: {kw}: auto");
    }
}

/// A later url after `none` wins (a later rule, and later in the same block); url then none in one block clears.
#[test]
fn css_wide_bgnone_later_url_wins() {
    let a: &[(&str, &str)] = &[("background-image", "url(a.png)")];
    let s = cascade(&[a, &[("background-image", "none")], &[("background-image", "url(b.png)")]]);
    assert_eq!(s.paint.bg_image.as_deref(), Some("b.png"), "a later rule's url after none");
    let s = cascade(&[a, &[("background-image", "none"), ("background-image", "url(b.png)")]]);
    assert_eq!(s.paint.bg_image.as_deref(), Some("b.png"), "none; url(b) in one block");
    let s = cascade(&[a, &[("background-image", "url(b.png)"), ("background-image", "none")]]);
    assert_eq!(s.paint.bg_image, None, "url(b); none in one block");
    let mid = cascade(&[a, &[("background-image", "none")]]);
    let mut again = SpecifiedStyle::<TH>::default();
    merge_specified(&mut again, &styled(a));
    merge_specified(&mut again, &mid);
    assert_eq!(again.paint.bg_image, None, "a merged none overlaid again still clears");
}

/// `background-image: none` clears the image only: the colour stays; `background: red` after an image keeps the image
/// (pinned: AETHER-CSS-BACKGROUND-SHORTHAND-RESET owed).
#[test]
fn css_wide_bgnone_image_only_and_pins() {
    let s = cascade(&[&[("background", "red url(a.png)")], &[("background-image", "none")]]);
    assert_eq!(s.paint.bg_image, None, "the image goes");
    assert_eq!(s.paint.background, Some((255, 0, 0)), "the colour stays");
    assert_eq!(s.paint.bg_alpha, Some(1.0), "still opaque");
    let s = cascade(&[&[("background-image", "url(a.png)")], &[("background", "red")]]);
    assert_eq!(s.paint.bg_image.as_deref(), Some("a.png"), "background: red keeps the image (pinned, owed)");
}

// ---- AETHERCSSALIGNCONTENTFIELD (LEDGER SR1016): align-content its own field; column-gap an explicit normal (named tail block) ----

/// AETHERCSSALIGNCONTENTFIELD (SR1016): a flex container (the rules cascaded into ONE sheet entry) over the given
/// children (`style=""`), laid out at 400 x 400; returns each child's (y, height) relative to the container.
fn align_content_lay(rules: &[&[(&str, &str)]], kids: &[(&'static str, &'static str)]) -> Vec<(f32, f32)> {
    let doc = page(vec![el("div", &[("id", "fc")], kids.iter().map(|(id, st)| el("div", &[("id", id), ("style", st)], vec![])).collect())]);
    let fc = doc.by_id("fc");
    let tree = lay_styled(&doc, &Sheet(vec![(fc.id, std::rc::Rc::new(cascade(rules)))]), 400.0, 400.0);
    let (_, cy, ..) = box_of(&tree, &fc);
    kids.iter().map(|(id, _)| { let (_, y, _, h) = box_of(&tree, &doc.by_id(id)); (y - cy, h) }).collect()
}

const AC_FLEX: &[(&str, &str)] = &[("display", "flex"), ("flex-wrap", "wrap"), ("width", "100px"), ("height", "200px")];

/// AETHERCSSALIGNCONTENTFIELD (SR1016): `align-content` writes its own field and leaves `align_items` alone (the shared
/// arm wrote align_items: center centred the items, space-between was refused); initial / unset reset it to normal.
#[test]
fn align_content_own_field() {
    use layout_core::taffy::style::AlignContent;
    let s = styled(&[("align-content", "space-between")]);
    assert_eq!(s.align_content, Some(AlignContent::SPACE_BETWEEN), "space-between reaches align_content");
    assert_eq!(s.align_items, None, "align-items untouched");
    let s = styled(&[("align-content", "center")]);
    assert_eq!((s.align_content, s.align_items), (Some(AlignContent::CENTER), None), "center: align-content only");
    let s = styled(&[("align-items", "center"), ("align-content", "flex-end")]);
    assert_eq!(s.align_items, styled(&[("align-items", "center")]).align_items, "align-items keeps center");
    assert_eq!(s.align_content, Some(AlignContent::FLEX_END), "align-content flex-end beside it");
    for kw in ["initial", "unset"] {
        let s = cascade(&[&[("align-content", "center")], &[("align-content", kw)]]);
        assert_eq!(s.align_content, Some(AlignContent::NORMAL), "align-content: {kw} is normal");
    }
    assert_eq!(styled(&[("align-content", "bogus")]).align_content, None, "an unknown value is dropped");
}

/// AETHERCSSALIGNCONTENTFIELD (SR1016): a wrapping 100 x 200 flex container with two 100 x 20 items (two lines):
/// space-between puts the second line at the bottom (180), normal (= stretch for flex; initial) splits the height into
/// two 100px lines, flex-start packs them (20); a nowrap container has one line and the property shows nothing.
#[test]
fn align_content_space_between_lines() {
    let kids: &[(&'static str, &'static str)] = &[("a", "width: 100px; height: 20px"), ("b", "width: 100px; height: 20px")];
    let sb = align_content_lay(&[AC_FLEX, &[("align-content", "space-between")]], kids);
    assert_eq!((sb[0].0, sb[1].0), (0.0, 180.0), "space-between: lines at the top and bottom");
    let ini = align_content_lay(&[AC_FLEX, &[("align-content", "space-between")], &[("align-content", "initial")]], kids);
    assert_eq!((ini[0].0, ini[1].0), (0.0, 100.0), "initial = normal = stretch: two 100px lines");
    let fs = align_content_lay(&[AC_FLEX, &[("align-content", "flex-start")]], kids);
    assert_eq!((fs[0].0, fs[1].0), (0.0, 20.0), "flex-start packs the lines");
    let nw = align_content_lay(&[AC_FLEX, &[("flex-wrap", "nowrap"), ("align-content", "flex-end")]], &[("a", "width: 50px; height: 20px"), ("b", "width: 50px; height: 20px")]);
    assert_eq!((nw[0].0, nw[1].0), (0.0, 0.0), "a single-line container ignores align-content");
}

/// AETHERCSSALIGNCONTENTFIELD (SR1016): align-items center and align-content flex-end on one container both apply, in
/// either order: line 1 [a 50x20, b 50x10] and line 2 [c 100x20] pack at the bottom (160, 180), b centred in its line.
#[test]
fn align_content_and_items_independent() {
    let kids: &[(&'static str, &'static str)] =
        &[("a", "width: 50px; height: 20px"), ("b", "width: 50px; height: 10px"), ("c", "width: 100px; height: 20px")];
    for decls in [
        [("align-items", "center"), ("align-content", "flex-end")],
        [("align-content", "flex-end"), ("align-items", "center")],
    ] {
        let r = align_content_lay(&[AC_FLEX, &decls], kids);
        assert_eq!((r[0].0, r[1].0, r[2].0), (160.0, 165.0, 180.0), "{decls:?}: both properties apply");
    }
}

/// AETHERCSSALIGNCONTENTFIELD (SR1016): `column-gap: normal` / initial / unset after 40px is an explicit normal
/// (Some(None)), so the multicol gutter is 1em again (20px at 20px), where it kept 40px before.
#[test]
fn column_gap_explicit_normal() {
    let s = cascade(&[&[("column-gap", "40px")]]);
    assert_eq!(s.paint.col_gap, Some(Some(40.0)), "40px (armed)");
    for kw in ["normal", "initial", "unset"] {
        let s = cascade(&[&[("column-gap", "40px")], &[("column-gap", kw)]]);
        assert_eq!(s.paint.col_gap, Some(None), "column-gap: {kw} after 40px is an explicit normal");
    }
    let gap = |rules: &[&[(&str, &str)]]| {
        let doc = page(vec![el("div", &[("id", "mc")], (0..4).map(|_| el("div", &[("style", "height: 20px")], vec![])).collect())]);
        let mc = doc.by_id("mc");
        let tree = lay_styled(&doc, &Sheet(vec![(mc.id, std::rc::Rc::new(cascade(rules)))]), 800.0, 600.0);
        tree.multicol[&id_of(&tree, &mc)].gap
    };
    let base: &[(&str, &str)] = &[("width", "410px"), ("column-count", "2"), ("font-size", "20px"), ("column-gap", "40px")];
    assert_eq!(gap(&[base]), 40.0, "the 40px gutter (armed)");
    assert_eq!(gap(&[base, &[("column-gap", "normal")]]), 20.0, "a later normal: 1em");
    assert_eq!(gap(&[base, &[("column-gap", "initial")]]), 20.0, "a later initial: 1em");
}

// ---- AETHERCSSWIDENONINHERITEDINHERIT (LEDGER SR1007): inherit on a non-inherited property takes the parent's resolved value (named tail block) ----

use layout_core::css::{INHERIT_BOX_PADDING, INHERIT_BOX_MARGIN};

/// AETHERCSSWIDENONINHERITEDINHERIT (SR1007): one parent / child / grandchild fixture, the parent's and the child's
/// cascaded styles given (the grandchild a 10px tall block, or two 30px boxes when `pair`).
fn inherit_box_lay(parent: &[(&str, &str)], child: &[&[(&str, &str)]], pair: bool) -> (Tree, N) {
    let kids = if pair {
        vec![el("div", &[("id", "g"), ("style", "width: 30px; height: 10px")], vec![]), el("div", &[("id", "g2"), ("style", "width: 30px; height: 10px")], vec![])]
    } else {
        vec![el("div", &[("id", "g"), ("style", "height: 10px")], vec![])]
    };
    let doc = page(vec![el("div", &[("id", "p")], vec![el("div", &[("id", "c")], kids)])]);
    let sheet = Sheet(vec![
        (doc.by_id("p").id, std::rc::Rc::new(styled(parent))),
        (doc.by_id("c").id, std::rc::Rc::new(cascade(child))),
    ]);
    let tree = lay_styled(&doc, &sheet, 400.0, 300.0);
    (tree, doc)
}

/// AETHERCSSWIDENONINHERITEDINHERIT (SR1007): `padding: inherit` clears an earlier rule's padding and writes the four
/// padding bits; a later `padding-top` (same block or a later rule) beats its side's bit; margin likewise per side.
#[test]
fn inherit_box_bits_clear_and_lose_to_later() {
    let s = cascade(&[&[("padding", "20px")], &[("padding", "inherit")]]);
    assert_eq!(s.padding, [None; 4], "padding: inherit clears the earlier 20px");
    assert_eq!(s.inherits_box(), INHERIT_BOX_PADDING, "the four padding bits");
    let s = styled(&[("padding", "inherit"), ("padding-top", "3px")]);
    assert!(s.padding[0].is_some(), "the later padding-top holds its value");
    assert_eq!(s.inherits_box(), INHERIT_BOX_PADDING & !16, "padding-top's bit is beaten in the same block");
    let s = cascade(&[&[("padding", "inherit")], &[("padding-top", "3px")]]);
    assert_eq!(s.inherits_box(), INHERIT_BOX_PADDING & !16, "and by a later rule (merge_specified)");
    let s = cascade(&[&[("margin", "20px")], &[("margin-left", "inherit")]]);
    assert_eq!(s.margin[3], None, "margin-left: inherit clears its side");
    assert!(s.margin[0].is_some(), "margin-top keeps 20px");
    assert_eq!(s.inherits_box(), 8 & INHERIT_BOX_MARGIN, "the left side's bit only");
}

/// AETHERCSSWIDENONINHERITEDINHERIT (SR1007): an unlisted non-inherited property's `inherit` is dropped as before
/// (z-index: AETHER-CSS-WIDE-NONINHERITED-INHERIT-REST owed, pinned).
#[test]
fn inherit_box_unlisted_dropped() {
    let s = cascade(&[&[("z-index", "5")], &[("z-index", "inherit")]]);
    assert_eq!(s.paint.z_index, Some(Some(5)), "z-index: inherit keeps the earlier 5 (owed)");
    assert_eq!(s.inherit_box, 0, "no bit");
}

/// AETHERCSSWIDENONINHERITEDINHERIT (SR1007): `.p { padding: 20px } .p > .c { padding: inherit }` (after an earlier
/// 5px): the grandchild sits 20px into the child's box (the 5px twin, armed, 5px).
#[test]
fn inherit_box_padding_layout() {
    let (tree, doc) = inherit_box_lay(&[("padding", "20px")], &[&[("padding", "5px")]], false);
    let (cx, cy, ..) = box_of(&tree, &doc.by_id("c"));
    let (gx, gy, ..) = box_of(&tree, &doc.by_id("g"));
    assert_eq!((gx - cx, gy - cy), (5.0, 5.0), "the child's own 5px (armed)");
    let (tree, doc) = inherit_box_lay(&[("padding", "20px")], &[&[("padding", "5px")], &[("padding", "inherit")]], false);
    let (cx, cy, ..) = box_of(&tree, &doc.by_id("c"));
    let (gx, gy, ..) = box_of(&tree, &doc.by_id("g"));
    assert_eq!((gx - cx, gy - cy), (20.0, 20.0), "padding: inherit is the parent's 20px");
}

/// AETHERCSSWIDENONINHERITEDINHERIT (SR1007): `width: inherit` is the parent's 120px (after an earlier 50px);
/// `margin: inherit` is the parent's 15px.
#[test]
fn inherit_box_width_margin_layout() {
    let (tree, doc) = inherit_box_lay(&[("width", "120px")], &[&[("width", "50px")], &[("width", "inherit")]], false);
    let (_, _, w, _) = box_of(&tree, &doc.by_id("c"));
    assert_eq!(w, 120.0, "width: inherit is the parent's 120px");
    let (tree, doc) = inherit_box_lay(&[("margin", "15px")], &[&[("margin", "inherit")]], false);
    let (px, ..) = box_of(&tree, &doc.by_id("p"));
    let (cx, ..) = box_of(&tree, &doc.by_id("c"));
    assert_eq!((px, cx - px), (15.0, 15.0), "margin: inherit is the parent's 15px");
}

/// AETHERCSSWIDENONINHERITEDINHERIT (SR1007): `display: inherit` under a flex parent makes the child a flex container:
/// its two 30px kids share a row (the block twin, armed, stacks them).
#[test]
fn inherit_box_display_flex_layout() {
    let (tree, doc) = inherit_box_lay(&[("display", "flex")], &[&[("display", "block")]], true);
    let (_, y1, ..) = box_of(&tree, &doc.by_id("g"));
    let (_, y2, ..) = box_of(&tree, &doc.by_id("g2"));
    assert_eq!(y2 - y1, 10.0, "display: block stacks (armed)");
    let (tree, doc) = inherit_box_lay(&[("display", "flex")], &[&[("display", "block")], &[("display", "inherit")]], true);
    let (x1, y1, ..) = box_of(&tree, &doc.by_id("g"));
    let (x2, y2, ..) = box_of(&tree, &doc.by_id("g2"));
    assert_eq!((y1, x2 - x1), (y2, 30.0), "display: inherit is flex: one row");
    assert_eq!(tree.paint_map[&id_of(&tree, &doc.by_id("c"))].flex_container, Some(true), "the resolved paint says flex");
}

/// AETHERCSSWIDENONINHERITEDINHERIT (SR1007): border / background-color / position: inherit carry the parent's resolved
/// paint into the child's (the child's earlier blue background beaten).
#[test]
fn inherit_box_paint_families() {
    let parent = [("border", "4px solid rgb(255, 0, 0)"), ("background-color", "rgb(255, 0, 0)"), ("position", "relative")];
    let child: &[&[(&str, &str)]] = &[&[("background-color", "rgb(0, 0, 255)")], &[("border", "inherit"), ("background-color", "inherit"), ("position", "inherit")]];
    let (tree, doc) = inherit_box_lay(&parent, child, false);
    let p = &tree.paint_map[&id_of(&tree, &doc.by_id("p"))];
    let c = &tree.paint_map[&id_of(&tree, &doc.by_id("c"))];
    assert!(p.border.is_some_and(|b| b[0].is_some()), "the parent's border (armed)");
    assert_eq!(c.border, p.border, "border: inherit");
    assert_eq!(c.background, Some((255, 0, 0)), "background-color: inherit beats the earlier blue");
    assert_eq!(c.position_kind, Some(1), "position: inherit is relative");
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

// ---- AETHERCOLORSCHEMEMETA (LEDGER SR1047): `light dark` follows the host's preferred scheme (named tail block) ----

/// AETHERCOLORSCHEMEMETA (SR1047): the pure answer with the preference explicit (no process-wide flip in this binary:
/// SR1031's `color_scheme_ua_colours_rows` reads the default light preference).
#[test]
fn color_scheme_prefers_for_table() {
    use layout_core::scheme_dark_for;
    assert!(!scheme_dark_for(Some(3), false), "light dark, host light: light");
    assert!(scheme_dark_for(Some(3), true), "light dark, host dark: dark");
    assert!(!scheme_dark_for(Some(1), true), "light alone stays light");
    assert!(scheme_dark_for(Some(2), false), "dark alone is dark whatever the host");
    for s in [None, Some(0)] {
        assert!(!scheme_dark_for(s, true), "{s:?} (unspecified / normal) stays light under a dark host");
    }
}
