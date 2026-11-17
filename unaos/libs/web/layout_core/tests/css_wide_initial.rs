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
/// (AETHER-CSS-WIDE-NONINHERITED-INHERIT); `color: initial` is CanvasText black (SR1021: it beats the earlier colour);
/// an inherited property's `unset` is not rewritten.
#[test]
fn css_wide_initial_leaves_the_rest() {
    let before = cascade(&[&[("margin", "20px")]]);
    for kw in [/* SR1037 fold: revert (SR1037) and inherit (SR1007) both erase margin now; `revert-layer` is the keyword still left as it was */ "revert-layer"] {
        let s = cascade(&[&[("margin", "20px")], &[("margin", kw)]]);
        assert_eq!(s.margin, before.margin, "margin: {kw} is unchanged by this arc");
    }
    let s = cascade(&[&[("color", "blue")], &[("color", "initial")]]);
    assert_eq!(s.paint.color, Some((0, 0, 0)), "color: initial is CanvasText black"); /* SR1021 */
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

/// `background-image: none` clears the image only: the colour stays; `background: red` after an image drops the image
/// (SR1061: the shorthand resets the omitted image; the pin flipped).
#[test]
fn css_wide_bgnone_image_only_and_pins() {
    let s = cascade(&[&[("background", "red url(a.png)")], &[("background-image", "none")]]);
    assert_eq!(s.paint.bg_image, None, "the image goes");
    assert_eq!(s.paint.background, Some((255, 0, 0)), "the colour stays");
    assert_eq!(s.paint.bg_alpha, Some(1.0), "still opaque");
    let s = cascade(&[&[("background-image", "url(a.png)")], &[("background", "red")]]);
    assert_eq!(s.paint.bg_image.as_deref(), None, "background: red resets the image (SR1061, was pinned kept)");
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

// ---- AETHERFONTRELINVALIDSUPERSEDES (LEDGER SR1055): font_rel records only the length declarations an arm accepted (named tail block) ----

/// The value `font_rel` holds for `prop` (the font-relative replay's entry), if any.
fn font_rel_of(s: &SpecifiedStyle<TH>, prop: &str) -> Option<String> {
    s.paint.font_rel.as_ref()?.iter().find(|(p, _)| p == prop).map(|(_, v)| v.clone())
}

/// AETHERFONTRELINVALIDSUPERSEDES (SR1055): `width: 2em; width: bogus` keeps `width -> 2em` in the replay list (it held
/// `bogus`, so the element's width stopped following its font). The first pass's width is None (the arm writes None on
/// refusal: AETHER-CSS-INVALID-DECLARATION-AUDIT), pinned as today.
#[test]
fn font_rel_invalid_width_keeps_earlier() {
    let s = styled(&[("width", "2em"), ("width", "bogus")]);
    assert_eq!(font_rel_of(&s, "width").as_deref(), Some("2em"), "the refused bogus does not displace 2em");
    assert_eq!(s.width, None, "the first pass's None write, pinned as today");
    let one = styled(&[("width", "2em")]);
    assert_eq!(font_rel_of(&one, "width").as_deref(), Some("2em"), "a lone 2em is recorded");
}

/// AETHERFONTRELINVALIDSUPERSEDES (SR1055): a later ACCEPTED value supersedes (`2em; 3em` keeps 3em, one entry).
#[test]
fn font_rel_invalid_later_valid_supersedes() {
    let s = styled(&[("width", "2em"), ("width", "3em")]);
    assert_eq!(font_rel_of(&s, "width").as_deref(), Some("3em"));
    let n = s.paint.font_rel.as_ref().unwrap().iter().filter(|(p, _)| p == "width").count();
    assert_eq!(n, 1, "one entry per property");
}

/// AETHERFONTRELINVALIDSUPERSEDES (SR1055): a css-wide value never reaches an arm and stays recorded (`width: inherit`
/// must re-resolve).
#[test]
fn font_rel_invalid_css_wide_recorded() {
    let s = styled(&[("width", "2em"), ("width", "inherit")]);
    assert_eq!(font_rel_of(&s, "width").as_deref(), Some("inherit"));
}

/// AETHERFONTRELINVALIDSUPERSEDES (SR1055): `padding-left: auto` (refused: padding takes no auto) keeps 1em; a negative
/// padding is ACCEPTED by the arm today (parse_length_percentage_str takes negatives: the audit's), pinned as today.
#[test]
fn font_rel_invalid_padding_refused() {
    let s = styled(&[("padding-left", "1em"), ("padding-left", "auto")]);
    assert_eq!(font_rel_of(&s, "padding-left").as_deref(), Some("1em"), "auto refused: 1em stands");
    let s = styled(&[("padding-left", "1em"), ("padding-left", "-1px")]);
    assert_eq!(font_rel_of(&s, "padding-left").as_deref(), Some("-1px"), "a negative is accepted today (pinned)");
}

/// AETHERFONTRELINVALIDSUPERSEDES (SR1055): the undo puts the displaced entry back at its index, so cascade order holds:
/// `margin-left: 1em; margin: 2em; margin-left: bogus` replays margin-left THEN margin (the shorthand wins).
#[test]
fn font_rel_invalid_keeps_cascade_order() {
    let s = styled(&[("margin-left", "1em"), ("margin", "2em"), ("margin-left", "bogus")]);
    let list: Vec<(String, String)> = s.paint.font_rel.clone().unwrap();
    let want = vec![("margin-left".to_string(), "1em".to_string()), ("margin".to_string(), "2em".to_string())];
    assert_eq!(list, want);
}

/// AETHERFONTRELINVALIDSUPERSEDES (SR1055): the guard that no refusal road was missed: every length property given
/// `1em` then a refused `bogus` keeps `1em`.
#[test]
fn font_rel_invalid_every_length_arm() {
    let props = [
        "width", "height", "min-width", "min-height", "max-width", "max-height", "margin", "margin-top", "margin-right",
        "margin-bottom", "margin-left", "padding", "padding-top", "padding-right", "padding-bottom", "padding-left", "top",
        "right", "bottom", "left", "inset", "gap", "row-gap", "column-gap", "flex-basis", "flex", "border", "border-top",
        "border-right", "border-bottom", "border-left", "border-width", "border-top-width", "border-right-width",
        "border-bottom-width", "border-left-width", "border-radius", "letter-spacing", "line-height", "vertical-align",
        "border-spacing", "outline", "outline-width", "outline-offset", "text-indent",
    ];
    for p in props {
        assert!(layout_core::css::is_length_property(p), "{p} is a length property");
        let s = styled(&[(p, "1em"), (p, "bogus")]);
        assert_eq!(font_rel_of(&s, p).as_deref(), Some("1em"), "{p}: the refused bogus leaves 1em in font_rel");
    }
    let extra = styled(&[("border-radius", "1em"), ("border-radius", "1px 2px 3px 4px 5px")]);
    assert_eq!(font_rel_of(&extra, "border-radius").as_deref(), Some("1em"), "five radii refused through the early return");
}

// ---- AETHERCSSWIDEALL (LEDGER SR1021): css-cascade-4 §3.1 the all shorthand over ONE longhand list (named tail block) ----

use layout_core::css::{ALL_LONGHANDS, ALL_SKIPPED};
use layout_core::paint::{INHERIT_COLOR, INHERIT_WEIGHT};

const ALL_RED: (u8, u8, u8) = (255, 0, 0);

fn all_armed() -> [(&'static str, &'static str); 4] {
    [("color", "rgb(255, 0, 0)"), ("margin-left", "20px"), ("font-weight", "bold"), ("display", "block")]
}

/// The four facts `all: unset` resets: colour and weight to the inherit bit, margin-left 0, display inline.
fn assert_all_reset(s: &SpecifiedStyle<TH>, what: &str) {
    let zero = styled(&[("margin-left", "0px")]).margin[3];
    assert_eq!(s.paint.color, None, "{what}: the colour goes");
    assert!(s.paint.inherits(INHERIT_COLOR), "{what}: color inherits the parent");
    assert_eq!(s.margin[3], zero, "{what}: margin-left is 0");
    assert!(s.paint.weight.is_none() && s.paint.inherits(INHERIT_WEIGHT), "{what}: font-weight inherits");
    assert_eq!(s.paint.display_kind, Some(0), "{what}: display is inline");
}

/// AETHERCSSWIDEALL (SR1021): `all: unset` after colour / margin / weight / display resets each, in one block and as a
/// later rule; the armed twin keeps red / 20px / bold / block.
#[test]
fn css_wide_all_unset_resets() {
    let armed = styled(&all_armed());
    assert_eq!(armed.paint.color, Some(ALL_RED), "armed: red");
    assert_ne!(armed.margin[3], styled(&[("margin-left", "0px")]).margin[3], "armed: 20px");
    assert!(armed.paint.weight.is_some(), "armed: bold");
    assert_eq!(armed.paint.display_kind, Some(1), "armed: block");
    let mut one = all_armed().to_vec();
    one.push(("all", "unset"));
    assert_all_reset(&styled(&one), "one block");
    assert_all_reset(&cascade(&[&all_armed(), &[("all", "unset")]]), "a later rule");
    assert_all_reset(&styled(&[all_armed()[0], all_armed()[1], all_armed()[2], all_armed()[3], ("all", "UNSET")]), "UNSET");
}

/// AETHERCSSWIDEALL (SR1021): `all: initial` writes the initial colour (CanvasText black, SR1021's row), not inherit.
#[test]
fn css_wide_all_initial_black() {
    let s = styled(&[("color", "rgb(255, 0, 0)"), ("all", "initial")]);
    assert_eq!(s.paint.color, Some((0, 0, 0)), "all: initial is black");
    assert!(!s.paint.inherits(INHERIT_COLOR), "not the inherit bit");
    assert_eq!(s.paint.display_kind, Some(0), "display: initial is inline");
}

/// AETHERCSSWIDEALL (SR1021): a later `color` beats an earlier `all: unset` (the bit is masked by the value).
#[test]
fn css_wide_all_later_color_wins() {
    let s = styled(&[("all", "unset"), ("color", "green")]);
    assert_eq!(s.paint.color, Some((0, 128, 0)), "the later green");
    assert!(!s.paint.inherits(INHERIT_COLOR), "the bit no longer applies");
    let s = cascade(&[&[("all", "unset")], &[("color", "green")]]);
    assert_eq!(s.paint.color, Some((0, 128, 0)), "the later rule's green");
    assert!(!s.paint.inherits(INHERIT_COLOR), "the later rule's value clears the bit");
}

/// AETHERCSSWIDEALL (SR1021): `all` leaves `direction` (css-cascade-4 §3.1).
#[test]
fn css_wide_all_keeps_direction() {
    for kw in ["initial", "unset", "inherit"] {
        let s = styled(&[("direction", "rtl"), ("all", kw)]);
        assert_eq!(s.paint.rtl, Some(true), "direction: rtl survives all: {kw}");
    }
}

/// AETHERCSSWIDEALL (SR1021): a non-CSS-wide value is invalid and `revert-layer` is dropped (owed): the style is untouched.
#[test]
fn css_wide_all_bogus_dropped() {
    let before = styled(&all_armed());
    for v in ["bogus", /* SR1028: revert built */ "revert-layer", "red", ""] {
        let mut d = all_armed().to_vec();
        d.push(("all", v));
        let s = styled(&d);
        assert_eq!(s.paint.color, before.paint.color, "all: {v:?} keeps the colour");
        assert_eq!(s.margin, before.margin, "all: {v:?} keeps the margin");
        assert_eq!(s.paint.weight.is_some(), before.paint.weight.is_some(), "all: {v:?} keeps the weight");
        assert_eq!(s.paint.display_kind, before.paint.display_kind, "all: {v:?} keeps display");
    }
}

/// The string literals at brace depth 1 in pattern position of `apply_declaration`'s `match prop`: the arm names.
fn apply_declaration_arm_names(src: &str) -> Vec<String> {
    let start = src.find("pub fn apply_declaration<").expect("apply_declaration");
    let body = &src[start..];
    let m = body.find("match prop {").expect("match prop") + "match prop {".len();
    let b = body.as_bytes();
    let (mut i, mut depth, mut in_pat) = (m, 1i32, true);
    let mut names = Vec::new();
    while i < b.len() && depth > 0 {
        let c = b[i];
        if c == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == b'"' {
            let mut k = i + 1;
            while b[k] != b'"' {
                if b[k] == b'\\' {
                    k += 1;
                }
                k += 1;
            }
            if depth == 1 && in_pat {
                names.push(body[i + 1..k].to_string());
            }
            i = k + 1;
            continue;
        }
        if c == b'\'' {
            // a char literal ('x' or '\x'); anything else (a lifetime) is one byte
            if b.get(i + 1) == Some(&b'\\') && b.get(i + 3) == Some(&b'\'') {
                i += 4;
                continue;
            }
            if b.get(i + 2) == Some(&b'\'') {
                i += 3;
                continue;
            }
        }
        match c {
            b'{' | b'(' | b'[' => depth += 1,
            b'}' | b')' | b']' => {
                depth -= 1;
                if depth == 1 && c == b'}' {
                    in_pat = true;
                }
            }
            b'=' if depth == 1 && in_pat && b.get(i + 1) == Some(&b'>') => {
                in_pat = false;
                i += 2;
                continue;
            }
            b',' if depth == 1 => in_pat = true,
            _ => {}
        }
        i += 1;
    }
    names
}

/// AETHERCSSWIDEALL (SR1021): every arm name of `apply_declaration` is in `ALL_LONGHANDS` or `ALL_SKIPPED` (never both),
/// and every listed name is an arm: a new arm, or a dead list entry, reds this pin.
#[test]
fn css_wide_all_list_covers_arms() {
    let names = apply_declaration_arm_names(include_str!("../src/css.rs"));
    assert!(names.len() > 150, "the scan found the arms: {}", names.len());
    for n in ["all", "display", "color", "direction", "cursor", "font-variant-caps"] {
        assert!(names.iter().any(|x| x == n), "the scan sees the {n} arm");
    }
    for n in &names {
        let l = ALL_LONGHANDS.contains(&n.as_str());
        let k = ALL_SKIPPED.contains(&n.as_str());
        assert!(l || k, "arm {n:?} is in neither ALL_LONGHANDS nor ALL_SKIPPED");
        assert!(!(l && k), "arm {n:?} is in both lists");
    }
    for n in ALL_LONGHANDS.iter().chain(ALL_SKIPPED.iter()) {
        assert!(names.iter().any(|x| x == n), "{n:?} is listed but no arm parses it");
    }
    assert!(!ALL_LONGHANDS.contains(&"direction"), "direction is excluded (css-cascade-4 §3.1)");
}

// ---- AETHERCSSWIDEREVERT (LEDGER SR1028): css-cascade-4 §7.3.2 revert rolls an inherit-bit field back to the tag road (named tail block) ----

use layout_core::paint::{INHERIT_FONT, INHERIT_LINE_HEIGHT, INHERIT_WHITE_SPACE};

/// AETHERCSSWIDEREVERT (SR1028): `color: revert` in a later rule erases the earlier red through the merge (None, no
/// inherit bit: the tag road: the `a` link blue, a control's black, else the parent); the armed twin keeps red.
#[test]
fn css_wide_revert_color_erases_earlier_rule() {
    assert_eq!(cascade(&[&[("color", "red")]]).paint.color, Some(ALL_RED), "armed: red");
    for kw in ["revert", "REVERT"] {
        let s = cascade(&[&[("color", "red")], &[("color", kw)]]);
        assert_eq!(s.paint.color, None, "color: {kw} after red: the field goes");
        assert!(!s.paint.inherits(INHERIT_COLOR), "color: {kw}: no inherit bit, the tag road");
        assert_eq!(s.paint.revert & INHERIT_COLOR, INHERIT_COLOR, "color: {kw}: the mark carries the erase");
    }
    let s = cascade(&[&[("color", "red")], &[("color", "revert")], &[("font-weight", "bold")]]);
    assert_eq!(s.paint.color, None, "an unrelated later rule keeps the erase");
}

/// AETHERCSSWIDEREVERT (SR1028): in one block and across rules the later of value / revert wins.
#[test]
fn css_wide_revert_order() {
    let s = styled(&[("color", "revert"), ("color", "red")]);
    assert_eq!(s.paint.color, Some(ALL_RED), "a later value in the block wins");
    let s = cascade(&[&[("color", "revert")], &[("color", "red")]]);
    assert_eq!(s.paint.color, Some(ALL_RED), "a later rule's value wins");
    assert_eq!(s.paint.revert & INHERIT_COLOR, 0, "the value clears the mark");
    let s = cascade(&[&[("color", "blue")], &[("color", "red"), ("color", "revert")]]);
    assert_eq!(s.paint.color, None, "red then revert in a later block erases the earlier blue");
}

/// AETHERCSSWIDEREVERT (SR1028): inherit and revert: the later wins, in a block and across rules.
#[test]
fn css_wide_revert_inherit_both_orders() {
    let s = cascade(&[&[("color", "inherit")], &[("color", "revert")]]);
    assert!(!s.paint.inherits(INHERIT_COLOR), "revert after inherit drops the bit");
    let s = cascade(&[&[("color", "revert")], &[("color", "inherit")]]);
    assert!(s.paint.inherits(INHERIT_COLOR), "inherit after revert sets the bit");
    assert_eq!(s.paint.revert & INHERIT_COLOR, 0, "and clears the mark");
    let s = styled(&[("color", "inherit"), ("color", "revert")]);
    assert!(!s.paint.inherits(INHERIT_COLOR) && s.paint.color.is_none(), "one block: revert last");
    let s = styled(&[("color", "revert"), ("color", "inherit")]);
    assert!(s.paint.inherits(INHERIT_COLOR), "one block: inherit last");
}

/// AETHERCSSWIDEREVERT (SR1028): `white-space: revert` clears the arm's pair (white_space and nowrap), so `pre` takes
/// `default_white_space` again; `line-height: revert` is None (was `normal`, SR843's pin flipped).
#[test]
fn css_wide_revert_white_space_and_line_height() {
    let armed = cascade(&[&[("white-space", "normal")]]);
    assert!(armed.paint.white_space.is_some(), "armed: normal is a value");
    let s = cascade(&[&[("white-space", "normal")], &[("white-space", "revert")]]);
    assert_eq!((s.paint.white_space, s.paint.nowrap), (None, None), "white-space: revert clears both fields");
    assert!(!s.paint.inherits(INHERIT_WHITE_SPACE), "the tag road, not inherit");
    let s = cascade(&[&[("line-height", "2")], &[("line-height", "revert")]]);
    assert_eq!(s.paint.line_height, None, "line-height: revert erases the 2");
    assert!(!s.paint.inherits(INHERIT_LINE_HEIGHT), "no bit");
}

/// AETHERCSSWIDEREVERT (SR1028): the six sizes have no tag road, so revert == initial: auto, beating the earlier rule.
#[test]
fn css_wide_revert_sizes_auto() {
    let s = cascade(&[&[("width", "100px")], &[("width", "revert")]]);
    assert_eq!(s.width, styled(&[("width", "auto")]).width, "width: revert is auto");
    assert_ne!(cascade(&[&[("width", "100px")]]).width, s.width, "the earlier 100px (armed)");
    for prop in ["height", "min-width", "min-height", "max-width", "max-height"] {
        let s = cascade(&[&[(prop, "100px")], &[(prop, "revert")]]);
        let auto = styled(&[(prop, "auto")]);
        let got = match prop { "height" => s.height, "min-width" => s.min_width, "min-height" => s.min_height, "max-width" => s.max_width, _ => s.max_height };
        let want = match prop { "height" => auto.height, "min-width" => auto.min_width, "min-height" => auto.min_height, "max-width" => auto.max_width, _ => auto.max_height };
        assert_eq!(got, want, "{prop}: revert is auto");
    }
}

/// AETHERCSSWIDEREVERT (SR1028): owed AETHER-CSS-WIDE-REVERT-NONINHERITED: margin / display revert stay dropped (their
/// UA value is the tag road, `ua_margin` / the display table, which no Some reaches): pinned as today.
#[test]
fn css_wide_revert_noninherited_dropped_owed() {
    let before = cascade(&[&[("margin-left", "20px"), ("display", "block")]]);
    let s = cascade(&[&[("margin-left", "20px"), ("display", "block")], &[("margin-left", "revert"), ("display", "revert")]]);
    assert!(s.margin[3].is_none() && before.margin[3].is_some(), "margin-left: revert erases the 20px"); /* SR1037 */
    assert!(s.paint.display_kind.is_none() && before.paint.display_kind.is_some(), "display: revert erases block"); /* SR1037 */
}

/// AETHERCSSWIDEREVERT (SR1028): `font: revert` reverts the seven font fields (no bits, the mark set).
#[test]
fn css_wide_revert_font_shorthand() {
    let s = cascade(&[&[("font-style", "italic"), ("font-size", "20px"), ("font-family", "serif"), ("font-weight", "bold")], &[("font", "revert")]]);
    assert_eq!((s.paint.italic, s.paint.font_size, s.paint.family, s.paint.weight.is_some()), (None, None, None, false), "font: revert clears the fields");
    assert_eq!(s.paint.inherit & INHERIT_FONT, 0, "no inherit bits");
    assert_eq!(s.paint.revert & INHERIT_FONT, INHERIT_FONT, "the seven marks");
}

/// AETHERCSSWIDEREVERT (SR1028): `all: revert` rides SR1021's loop: colour / weight reverted, width auto, direction kept;
/// `all: revert-layer` stays dropped (AETHER-CSS-WIDE-REVERT-LAYER owed).
#[test]
fn css_wide_revert_all() {
    let armed: &[(&str, &str)] = &[("color", "red"), ("font-weight", "bold"), ("width", "10px"), ("direction", "rtl")];
    let s = cascade(&[armed, &[("all", "revert")]]);
    assert_eq!(s.paint.color, None, "all: revert: colour");
    assert!(s.paint.weight.is_none() && !s.paint.inherits(INHERIT_WEIGHT), "all: revert: weight to the tag road");
    assert_eq!(s.width, styled(&[("width", "auto")]).width, "all: revert: width auto");
    assert_eq!(s.paint.rtl, Some(true), "direction survives all: revert");
    let s = cascade(&[armed, &[("all", "revert-layer")]]);
    assert_eq!(s.paint.color, Some(ALL_RED), "all: revert-layer is dropped (owed)");
}
// ---- AETHERCSSWIDEREVERTNONINHERITED (LEDGER SR1037): revert on display / margin / padding / border / background erases to the tag road (named tail block) ----

use layout_core::css::{parse_declaration_block, REVERT_BORDER, REVERT_DISPLAY, REVERT_MARGIN};

/// AETHERCSSWIDEREVERTNONINHERITED (SR1037): `margin: revert` after `margin: 0` erases the four sides (None: `ua_margin`
/// at fold time), `margin-left: revert` the left only.
#[test]
fn css_wide_revert_noninherited_margin() {
    let zero = cascade(&[&[("margin", "0")]]);
    assert!(zero.margin.iter().all(|m| m.is_some()), "armed: margin 0 on four sides");
    for kw in ["revert", "REVERT"] {
        let s = cascade(&[&[("margin", "0")], &[("margin", kw)]]);
        assert_eq!(s.margin, [None; 4], "margin: {kw} erases the four sides");
        assert_eq!(s.revert_spec & REVERT_MARGIN, REVERT_MARGIN, "margin: {kw}: the four marks");
    }
    let twenty = cascade(&[&[("margin", "20px")]]);
    let s = cascade(&[&[("margin", "20px")], &[("margin-left", "revert")]]);
    assert_eq!(s.margin[3], None, "margin-left: revert erases the left");
    assert_eq!(&s.margin[..3], &twenty.margin[..3], "the other three sides keep 20px");
    let s = styled(&[("margin", "20px"), ("margin", "revert")]);
    assert_eq!(s.margin, [None; 4], "one block: the revert erases the block's own earlier value");
}

/// AETHERCSSWIDEREVERTNONINHERITED (SR1037): a reverted `p` margin lays out as the UA 1em (16px) top margin, the armed
/// `margin: 0` twin at 0 (the p sits in a padded div, so no margin collapses through).
#[test]
fn css_wide_revert_noninherited_margin_layout() {
    let doc = page(vec![el("div", &[("style", "padding: 1px")], vec![el("p", &[("id", "p")], vec![text("A")])])]);
    let p = doc.by_id("p");
    let top = |rules: &[&[(&str, &str)]]| {
        let tree = lay_styled(&doc, &Sheet(vec![(p.id, std::rc::Rc::new(cascade(rules)))]), 400.0, 400.0);
        box_of(&tree, &p).1
    };
    let zero = top(&[&[("margin", "0")]]);
    let rev = top(&[&[("margin", "0")], &[("margin", "revert")]]);
    assert!((rev - zero - 16.0).abs() < 0.5, "margin: revert is p's UA 1em: {zero} -> {rev}");
}

/// AETHERCSSWIDEREVERTNONINHERITED (SR1037): `display: revert` erases the display arm's five fields, so the tag road
/// lays the element (a div as a block, a span inline) as with no rule.
#[test]
fn css_wide_revert_noninherited_display() {
    let s = cascade(&[&[("display", "inline")], &[("display", "revert")]]);
    assert_eq!((s.display, s.flex_container, s.paint.flex_container, s.paint.display_kind, s.paint.list_item), (None, None, None, None, None), "display: revert erases the five fields");
    assert_eq!(s.revert_spec & REVERT_DISPLAY, REVERT_DISPLAY, "the mark");
    let s = cascade(&[&[("display", "list-item")], &[("display", "revert")]]);
    assert_eq!(s.paint.list_item, None, "list-item's marker flag goes too");
    for (tag, armed) in [("div", "inline"), ("span", "block")] {
        let doc = page(vec![el(tag, &[("id", "e")], vec![text("Hello")])]);
        let e = doc.by_id("e");
        let width = |rules: &[&[(&str, &str)]]| {
            let tree = lay_styled(&doc, &Sheet(vec![(e.id, std::rc::Rc::new(cascade(rules)))]), 400.0, 400.0);
            box_of(&tree, &e).2
        };
        let plain = width(&[]);
        let rev = width(&[&[("display", armed)], &[("display", "revert")]]);
        assert!((rev - plain).abs() < 0.5, "{tag} display {armed} then revert lays as the tag: {plain} vs {rev}");
    }
}

/// AETHERCSSWIDEREVERTNONINHERITED (SR1037): `padding-top: revert` erases the top side only; `padding: revert` all four.
#[test]
fn css_wide_revert_noninherited_padding() {
    let s = cascade(&[&[("padding", "10px")], &[("padding-top", "revert")]]);
    assert_eq!(s.padding[0], None, "padding-top: revert erases the top");
    assert!(s.padding[1..].iter().all(|p| p.is_some()), "the other sides keep 10px");
    let s = cascade(&[&[("padding", "10px")], &[("padding", "revert")]]);
    assert_eq!(s.padding, [None; 4], "padding: revert erases the four sides");
}

/// AETHERCSSWIDEREVERTNONINHERITED (SR1037): `border: revert` / `border-style: revert` erase the whole border (None: a
/// control's UA frame, nothing elsewhere); `border-left: revert` and `border-left-style: revert` drop the left side.
#[test]
fn css_wide_revert_noninherited_border() {
    for prop in ["border", "border-style"] {
        let s = cascade(&[&[("border", "2px solid red")], &[(prop, "revert")]]);
        assert_eq!((s.paint.border, s.paint.border_style, s.paint.border_alpha), (None, None, None), "{prop}: revert erases the border");
        assert_eq!(s.revert_spec & REVERT_BORDER, REVERT_BORDER, "{prop}: the four marks");
    }
    for prop in ["border-left", "border-left-style"] {
        let s = cascade(&[&[("border", "2px solid red")], &[(prop, "revert")]]);
        let b = s.paint.border.expect("the other sides stay");
        assert!(b[3].is_none() && b[..3].iter().all(|x| x.is_some()), "{prop}: revert drops the left only: {b:?}");
    }
    let s = cascade(&[&[("border", "2px solid red")], &[("border", "revert"), ("border", "1px solid blue")]]);
    assert!(s.paint.border.is_some_and(|b| b.iter().all(|x| x.is_some_and(|(w, _)| w == 1.0))), "a later border in the block wins");
}

/// AETHERCSSWIDEREVERTNONINHERITED (SR1037): `background: revert` / `background-color: revert` erase the colour, so a
/// button paints ButtonFace again (armed red); `background-color: revert` keeps an image, `background: revert` drops it.
#[test]
fn css_wide_revert_noninherited_background() {
    for prop in ["background", "background-color"] {
        let s = cascade(&[&[("background-color", "red")], &[(prop, "revert")]]);
        assert_eq!((s.paint.background, s.paint.bg_alpha), (None, None), "{prop}: revert erases the colour");
    }
    let s = cascade(&[&[("background", "url(a.png) red")], &[("background-color", "revert")]]);
    assert!(s.paint.bg_image.is_some(), "background-color: revert keeps the image");
    let s = cascade(&[&[("background", "url(a.png) red")], &[("background", "revert")]]);
    assert!(s.paint.bg_image.is_none(), "background: revert drops the image (no tag has a UA image)");
    let s = cascade(&[&[("background", "url(a.png)")], &[("background-image", "revert")]]);
    assert!(s.paint.bg_image.is_none(), "background-image: revert is none");
    let doc = page(vec![el("button", &[("id", "b")], vec![text("Go")])]);
    let b = doc.by_id("b");
    let bg = |rules: &[&[(&str, &str)]]| {
        let tree = lay_styled(&doc, &Sheet(vec![(b.id, std::rc::Rc::new(cascade(rules)))]), 400.0, 400.0);
        tree.paint_map.get(&id_of(&tree, &b)).and_then(|p| p.background)
    };
    assert_eq!(bg(&[&[("background-color", "red")]]), Some((255, 0, 0)), "armed: red");
    assert_eq!(bg(&[&[("background-color", "red")], &[("background", "revert")]]), Some(layout_core::BUTTON_FACE), "background: revert is ButtonFace");
}

/// AETHERCSSWIDEREVERTNONINHERITED (SR1037): the later of value / revert wins, in a block and across rules.
#[test]
fn css_wide_revert_noninherited_order() {
    let five = styled(&[("margin", "5px")]).margin;
    let s = cascade(&[&[("margin", "20px")], &[("margin", "revert"), ("margin", "5px")]]);
    assert_eq!(s.margin, five, "revert then 5px in a block: 5px");
    let s = cascade(&[&[("margin", "20px")], &[("margin", "5px"), ("margin", "revert")]]);
    assert_eq!(s.margin, [None; 4], "5px then revert in a block: erased");
    let s = cascade(&[&[("margin", "revert")], &[("margin", "20px")]]);
    assert_eq!(s.margin, styled(&[("margin", "20px")]).margin, "a later rule's margin: 20px wins");
    let cx = ParseCx::default();
    for body in ["margin: 20px; margin: revert !important", "margin: revert !important; margin: 20px"] {
        assert_eq!(parse_declaration_block::<TH>(body, &cx).margin, [None; 4], "{body}: the important revert erases");
    }
}

/// AETHERCSSWIDEREVERTNONINHERITED (SR1037): `all: revert` now reverts margin / padding / display / background / border
/// too (the longhands' side bits; the four border-*-style sides erase the whole border).
#[test]
fn css_wide_revert_noninherited_all() {
    let armed: &[(&str, &str)] = &[("margin", "20px"), ("padding", "3px"), ("display", "inline"), ("background-color", "red"), ("border", "2px solid red")];
    let s = cascade(&[armed, &[("all", "revert")]]);
    assert_eq!((s.margin, s.padding), ([None; 4], [None; 4]), "all: revert: margin and padding");
    assert_eq!((s.display, s.paint.display_kind), (None, None), "all: revert: display");
    assert_eq!(s.paint.background, None, "all: revert: background colour");
    assert_eq!(s.paint.border, None, "all: revert: border");
}
// ---- AETHERCSSALIGNSELFARM (LEDGER SR1068): css-align-3 §6.2 align-self on a flex item (named tail block) ----

use layout_core::taffy::style::AlignSelf;

/// AETHERCSSALIGNSELFARM (SR1068): the align-items keyword map on align-self, SR1064's modifiers through `align_word`,
/// the `-webkit-` alias routed to the same arm; a refused value leaves the field None.
#[test]
fn align_self_keywords() {
    let one = |v: &str| styled(&[("align-self", v)]).align_self;
    assert_eq!(one("center"), Some(AlignSelf::CENTER), "armed: center");
    assert_eq!(one("flex-end"), Some(AlignSelf::END), "flex-end");
    assert_eq!(one("end"), Some(AlignSelf::END), "end");
    assert_eq!(one("flex-start"), Some(AlignSelf::START), "flex-start");
    assert_eq!(one("start"), Some(AlignSelf::START), "start");
    assert_eq!(one("baseline"), Some(AlignSelf::BASELINE), "baseline");
    assert_eq!(one("stretch"), Some(AlignSelf::STRETCH), "stretch");
    assert_eq!(one("normal"), Some(AlignSelf::STRETCH), "normal is stretch on a flex item");
    assert_eq!(one("safe center"), Some(AlignSelf::CENTER), "safe center (SR1064's road)");
    assert_eq!(one("unsafe end"), Some(AlignSelf::END), "unsafe end");
    assert_eq!(one("first baseline"), Some(AlignSelf::BASELINE), "first baseline");
    assert_eq!(styled(&[("-webkit-align-self", "center")]).align_self, Some(AlignSelf::CENTER), "-webkit-align-self");
    for bad in ["bogus", "safe", "left", "safe safe center"] {
        let s = styled(&[("align-self", bad)]);
        assert_eq!(s.align_self, None, "{bad}: refused");
        assert!(!s.align_self_auto, "{bad}: refused, not auto");
    }
}

/// AETHERCSSALIGNSELFARM (SR1068): an explicit `auto` (taffy None) beats an earlier value in the same block and, through
/// the `align_self_auto` marker `merge_specified` reads, an earlier rule's; `initial` / `unset` / `all: initial` are auto
/// (the noninherited_initial row); a later value beats an earlier auto.
#[test]
fn align_self_auto_beats_earlier_rule() {
    let s = styled(&[("align-self", "center"), ("align-self", "auto")]);
    assert_eq!(s.align_self, None, "center then auto in one block");
    assert!(s.align_self_auto, "auto is marked");
    assert_eq!(cascade(&[&[("align-self", "center")]]).align_self, Some(AlignSelf::CENTER), "armed: the earlier rule's center");
    assert_eq!(cascade(&[&[("align-self", "center")], &[("align-self", "auto")]]).align_self, None, "a later rule's auto wins");
    assert_eq!(cascade(&[&[("align-self", "auto")], &[("align-self", "center")]]).align_self, Some(AlignSelf::CENTER), "a later center beats auto");
    for kw in ["initial", "unset"] {
        assert_eq!(cascade(&[&[("align-self", "center")], &[("align-self", kw)]]).align_self, None, "align-self: {kw} is auto");
    }
    assert_eq!(cascade(&[&[("align-self", "center")], &[("all", "initial")]]).align_self, None, "all: initial resets align-self");
    assert_eq!(cascade(&[&[("align-self", "center")], &[("align-self", "inherit")]]).align_self, Some(AlignSelf::CENTER), "inherit is dropped (as align-items)");
}

/// AETHERCSSALIGNSELFARM (SR1068): a float keeps its START / END in either order (Aether's float approximation;
/// Chromium ignores align-self on a float in block flow).
#[test]
fn align_self_float_wins() {
    assert_eq!(styled(&[("float", "left"), ("align-self", "center")]).align_self, Some(AlignSelf::START), "float: left then center");
    assert_eq!(styled(&[("align-self", "center"), ("float", "left")]).align_self, Some(AlignSelf::START), "center then float: left");
    assert_eq!(styled(&[("float", "right"), ("align-self", "auto")]).align_self, Some(AlignSelf::END), "float: right then auto");
    assert_eq!(styled(&[("float", "none"), ("align-self", "center")]).align_self, Some(AlignSelf::CENTER), "float: none does not guard");
}

/// AETHERCSSALIGNSELFARM (SR1068): a 200px-tall flex row with one 50 x 50 item: `align-self: center` puts it at y 75,
/// `flex-end` at y 150; with no align-self it sits at y 0 (armed: before the arm the declaration was dropped).
#[test]
fn align_self_center_layout() {
    let y = |a: Option<&'static str>| {
        let doc = page(vec![el("div", &[("id", "fc")], vec![el("div", &[("id", "k")], vec![])])]);
        let (fc, k) = (doc.by_id("fc"), doc.by_id("k"));
        let row: &[(&str, &str)] = &[("display", "flex"), ("width", "200px"), ("height", "200px")];
        let mut item: Vec<(&str, &str)> = vec![("width", "50px"), ("height", "50px")];
        if let Some(a) = a {
            item.push(("align-self", a));
        }
        let sheet = Sheet(vec![(fc.id, std::rc::Rc::new(cascade(&[row]))), (k.id, std::rc::Rc::new(cascade(&[item.as_slice()])))]);
        let tree = lay_styled(&doc, &sheet, 400.0, 400.0);
        box_of(&tree, &k).1 - box_of(&tree, &fc).1
    };
    let (none, c, e) = (y(None), y(Some("center")), y(Some("flex-end")));
    assert!(none.abs() <= 0.5, "no align-self: the item sits at the top: {none}");
    assert!((c - 75.0).abs() <= 0.5, "align-self: center at y 75: {c}");
    assert!((e - 150.0).abs() <= 0.5, "align-self: flex-end at y 150: {e}");
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
    for v in ["5px", "normal dark", "only", "only only dark", "dark, light" /* SR1031 fold: `revert` is SR1028's erase now (set_revert on INHERIT_COLOR_SCHEME), no longer a dropped value */] {
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

// ---- AETHERCOLORSCHEMESYSTEMCOLORS (LEDGER SR1046): system colour keywords stored as slots, resolved against the color-scheme (named tail block) ----

use layout_core::css::system_color_index;
use layout_core::{system_color, SYSTEM_COLORS};

/// AETHERCOLORSCHEMESYSTEMCOLORS (SR1046): a parent `#p`, its child `#c`, the child's child `#g`, each with its own
/// cascaded declarations; the paint_map entries after `apply_specs` (the system colours resolved).
fn sys_lay(p: &[(&str, &str)], c: &[(&str, &str)], g: &[(&str, &str)]) -> (Tree, N) {
    let doc = page(vec![el("div", &[("id", "p")], vec![el("div", &[("id", "c")], vec![el("div", &[("id", "g"), ("style", "height: 10px")], vec![])])])]);
    let sheet = Sheet(vec![
        (doc.by_id("p").id, std::rc::Rc::new(styled(p))),
        (doc.by_id("c").id, std::rc::Rc::new(styled(c))),
        (doc.by_id("g").id, std::rc::Rc::new(styled(g))),
    ]);
    let tree = lay_styled(&doc, &sheet, 400.0, 300.0);
    (tree, doc)
}

/// AETHERCOLORSCHEMESYSTEMCOLORS (SR1046): `color: CanvasText` keeps css_core's light carrier AND stores the keyword's
/// slot (any case); a plain colour stores no slot; a CSS-wide keyword leaves the slot.
#[test]
fn system_color_parse_slot() {
    let k = system_color_index("canvastext");
    assert!(k.is_some(), "canvastext is a system colour");
    let s = styled(&[("color", "CanvasText")]);
    assert_eq!(s.paint.color, Some((0, 0, 0)), "the light carrier");
    assert_eq!(s.paint.sys_color, k, "the slot");
    assert_eq!(styled(&[("color", "CANVASTEXT")]).paint.sys_color, k, "case-insensitive");
    assert_eq!(styled(&[("color", "black")]).paint.sys_color, None, "a plain colour: no slot");
    assert_eq!(styled(&[("color", "CanvasText"), ("color", "inherit")]).paint.sys_color, k, "inherit leaves the slot");
    let s = styled(&[("background", "Canvas no-repeat")]);
    assert_eq!((s.paint.background, s.paint.sys_background), (Some((255, 255, 255)), system_color_index("canvas")), "background shorthand");
    let s = styled(&[("background-color", "Field")]);
    assert_eq!(s.paint.sys_background, system_color_index("field"), "background-color");
    let s = styled(&[("border", "2px solid ButtonBorder")]);
    assert_eq!(s.paint.sys_border, system_color_index("buttonborder").map(|k| (k, 15)), "border: all four sides");
    let s = styled(&[("border", "2px solid red"), ("border-color", "ButtonBorder")]);
    assert_eq!(s.paint.sys_border, system_color_index("buttonborder").map(|k| (k, 15)), "border-color: all four sides");
}

/// AETHERCOLORSCHEMESYSTEMCOLORS (SR1046): a later plain colour clears the slot (merge_paint carries the slot with its
/// field); a later block that does not write the field keeps it.
#[test]
fn system_color_later_plain_clears() {
    let s = cascade(&[&[("color", "CanvasText")], &[("color", "red")]]);
    assert_eq!((s.paint.color, s.paint.sys_color), (Some((255, 0, 0)), None), "color: red clears the slot");
    let s = cascade(&[&[("color", "CanvasText")], &[("width", "10px")]]);
    assert_eq!(s.paint.sys_color, system_color_index("canvastext"), "an unrelated later block keeps it (armed)");
    let s = cascade(&[&[("background", "Canvas")], &[("background-color", "blue")]]);
    assert_eq!((s.paint.background, s.paint.sys_background), (Some((0, 0, 255)), None), "background-color: blue clears it");
    let s = cascade(&[&[("border", "1px solid ButtonBorder")], &[("border", "1px solid green")]]);
    assert_eq!(s.paint.sys_border, None, "a later plain border clears it");
}

/// AETHERCOLORSCHEMESYSTEMCOLORS (SR1046): a side longhand with a plain colour drops its side from the mask (that side
/// is not recoloured); with the same keyword it stays.
#[test]
fn system_color_border_side_mask() {
    let k = system_color_index("buttonborder").unwrap();
    let s = styled(&[("border", "2px solid ButtonBorder"), ("border-left-color", "black")]);
    assert_eq!(s.paint.sys_border, Some((k, 0b0111)), "border-left-color: black leaves the left side out");
    let s = styled(&[("border", "2px solid ButtonBorder"), ("border-left-color", "ButtonBorder")]);
    assert_eq!(s.paint.sys_border, Some((k, 15)), "the same keyword keeps the side");
    let s = styled(&[("border", "2px solid red"), ("border-top-color", "ButtonBorder")]);
    assert_eq!(s.paint.sys_border, Some((k, 1)), "a side longhand alone: its one side");
}

/// AETHERCOLORSCHEMESYSTEMCOLORS (SR1046): under `color-scheme: dark` on the parent the child's CanvasText is white,
/// Canvas #121212, ButtonBorder the dark frame #858585, LinkText #9e9eff; the light twin keeps css_core's values; a
/// `background-color: inherit` grandchild copies the parent's RESOLVED colour (the pass runs before inherit_box_pass).
#[test]
fn system_color_resolves_dark_tree() {
    let child = [("color", "CanvasText"), ("background-color", "Canvas"), ("border", "2px solid ButtonBorder")];
    let grand = [("background-color", "inherit")];
    let (tree, doc) = sys_lay(&[("color-scheme", "dark")], &child, &grand);
    let c = &tree.paint_map[&id_of(&tree, &doc.by_id("c"))];
    let g = &tree.paint_map[&id_of(&tree, &doc.by_id("g"))];
    assert_eq!(c.color, Some((255, 255, 255)), "dark CanvasText is white");
    assert_eq!(c.background, Some((18, 18, 18)), "dark Canvas is #121212");
    assert_eq!(c.border.map(|b| b.map(|s| s.map(|x| x.1))), Some([Some((133, 133, 133)); 4]), "dark ButtonBorder");
    assert_eq!(g.background, Some((18, 18, 18)), "background-color: inherit takes the resolved #121212");
    let (tree, doc) = sys_lay(&[], &child, &grand);
    let c = &tree.paint_map[&id_of(&tree, &doc.by_id("c"))];
    assert_eq!(c.color, Some((0, 0, 0)), "light twin: black (armed)");
    assert_eq!(c.background, Some((255, 255, 255)), "light twin: white");
    assert_eq!(c.border.map(|b| b.map(|s| s.map(|x| x.1))), Some([Some((0, 0, 0)); 4]), "light twin: css_core's ButtonBorder");
    let (tree, doc) = sys_lay(&[("color-scheme", "dark")], &[("color", "LinkText")], &[]);
    assert_eq!(tree.paint_map[&id_of(&tree, &doc.by_id("c"))].color, Some((158, 158, 255)), "dark LinkText");
    let (tree, doc) = sys_lay(&[("color-scheme", "dark")], &[("color-scheme", "normal"), ("color", "CanvasText")], &[]);
    assert_eq!(tree.paint_map[&id_of(&tree, &doc.by_id("c"))].color, Some((0, 0, 0)), "normal on the element itself is light");
}

/// AETHERCOLORSCHEMESYSTEMCOLORS (SR1046): the ONE table covers css_core's system words, and the light value is
/// css_core's own.
#[test]
fn system_color_table() {
    for (i, (name, _)) in SYSTEM_COLORS.iter().enumerate() {
        let c = css_core::color::named(name).unwrap_or_else(|| panic!("{name} is css_core's"));
        let rgb = (c.r as u8, c.g as u8, c.b as u8);
        assert_eq!(system_color::<TH>(i as u8, None), Some(rgb), "{name}: light is css_core's");
        assert_eq!(system_color_index(name), Some(i as u8));
    }
    assert_eq!(system_color::<TH>(system_color_index("canvas").unwrap(), Some(2)), Some((18, 18, 18)));
    assert_eq!(system_color::<TH>(system_color_index("mark").unwrap(), Some(2)), Some((255, 255, 0)), "a None word keeps light");
}

// ---- AETHERCSSJUSTIFYITEMSSELF (LEDGER SR1088): css-align-3 §6.2 / §6.3 justify-items / justify-self (named tail block) ----

use layout_core::taffy::style::AlignItems;

/// AETHERCSSJUSTIFYITEMSSELF (SR1088): the two arms map the align twins' words (one keyword map, `self_alignment`):
/// center / end / flex-end / start / flex-start / stretch / normal / baseline (`first baseline`, `safe center` as one
/// value through `align_word`); `auto` is justify-self's marker and refused on justify-items; bogus refused; `initial`
/// through the non-inherited table (normal → STRETCH, auto → the marker).
#[test]
fn justify_items_self_words() {
    let one = |p: &str, v: &str| styled(&[(p, v)]);
    let ji = |v: &str| one("justify-items", v).justify_items;
    assert_eq!(ji("center"), Some(AlignItems::CENTER), "armed: justify-items: center");
    for (v, want) in [("end", AlignItems::END), ("flex-end", AlignItems::END), ("start", AlignItems::START), ("flex-start", AlignItems::START),
                      ("stretch", AlignItems::STRETCH), ("normal", AlignItems::STRETCH), ("baseline", AlignItems::BASELINE),
                      ("first baseline", AlignItems::BASELINE), ("safe center", AlignItems::CENTER), ("initial", AlignItems::STRETCH)] {
        assert_eq!(ji(v), Some(want), "justify-items: {v}");
        assert_eq!(ji(v), one("align-items", v).align_items, "justify-items: {v} is align-items' word");
    }
    for bad in ["bogus", "auto", "legacy", "space-between", "safe safe center"] {
        assert_eq!(ji(bad), None, "justify-items: {bad} refused");
    }
    let js = |v: &str| one("justify-self", v);
    assert_eq!(js("center").justify_self, Some(AlignSelf::CENTER), "armed: justify-self: center");
    assert_eq!(js("end").justify_self, Some(AlignSelf::END), "justify-self: end");
    assert_eq!(js("flex-start").justify_self, Some(AlignSelf::START), "justify-self: flex-start");
    assert_eq!(js("normal").justify_self, Some(AlignSelf::STRETCH), "justify-self: normal");
    for (v, want) in [("center", Some(AlignSelf::CENTER)), ("end", Some(AlignSelf::END)), ("baseline", Some(AlignSelf::BASELINE))] {
        assert_eq!(js(v).justify_self, one("align-self", v).align_self, "justify-self: {v} is align-self's word");
        assert_eq!(js(v).justify_self, want);
    }
    for v in ["auto", "initial", "unset"] {
        let s = js(v);
        assert!(s.justify_self.is_none() && s.justify_self_auto, "justify-self: {v} is the auto marker");
        assert!(!s.align_self_auto, "justify-self: {v} leaves align-self's marker alone");
    }
    let s = js("bogus");
    assert!(s.justify_self.is_none() && !s.justify_self_auto, "justify-self: bogus refused");
    let s = one("align-self", "auto");
    assert!(s.align_self_auto && !s.justify_self_auto, "align-self: auto leaves justify-self's marker alone");
}

/// AETHERCSSJUSTIFYITEMSSELF (SR1088): through `merge_specified` a later rule beats an earlier one, and a later
/// `justify-self: auto` erases an earlier `end` (the marker, as SR1068's align-self); `place-content` writes neither.
#[test]
fn justify_items_self_cascade() {
    let c = cascade(&[&[("justify-items", "start")], &[("justify-items", "center")]]);
    assert_eq!(c.justify_items, Some(AlignItems::CENTER), "a later justify-items wins");
    let c = cascade(&[&[("justify-items", "center")], &[("align-items", "end")]]);
    assert_eq!(c.justify_items, Some(AlignItems::CENTER), "align-items leaves justify-items");
    let c = cascade(&[&[("justify-self", "end")], &[("justify-self", "auto")]]);
    assert!(c.justify_self.is_none() && c.justify_self_auto, "a later justify-self: auto beats an earlier end");
    let c = cascade(&[&[("justify-self", "end")], &[("color", "red")]]);
    assert_eq!(c.justify_self, Some(AlignSelf::END), "control: an unrelated later rule keeps end");
    let c = cascade(&[&[("place-items", "center")], &[("justify-items", "end")]]);
    assert_eq!((c.align_items, c.justify_items), (Some(AlignItems::CENTER), Some(AlignItems::END)), "a later longhand beats place-items' half");
    let s = styled(&[("place-content", "center")]);
    assert!(s.justify.is_some() && s.justify_items.is_none() && s.justify_self.is_none(), "place-content: unchanged, justify-content only");
}

/// AETHERCSSJUSTIFYITEMSSELF (SR1088): both are longhands `all` resets, never skipped shorthands.
#[test]
fn justify_items_self_longhands() {
    for n in ["justify-items", "justify-self"] {
        assert!(ALL_LONGHANDS.contains(&n) && !ALL_SKIPPED.contains(&n), "{n} is in ALL_LONGHANDS");
    }
}

/// AETHERCSSJUSTIFYITEMSSELF (SR1088): `fold_into` writes taffy's `justify_items` / `justify_self`; an empty style
/// leaves taffy's defaults (control).
#[test]
fn justify_items_self_fold() {
    let mut st = layout_core::taffy::Style::default();
    styled(&[("justify-items", "center"), ("justify-self", "end")]).fold_into(&mut st);
    assert_eq!(st.justify_items, AlignItems::CENTER, "armed: justify-items folds");
    assert_eq!(st.justify_self, Some(AlignSelf::END), "justify-self folds");
    let (mut a, b): (layout_core::taffy::Style, layout_core::taffy::Style) = (Default::default(), Default::default());
    SpecifiedStyle::<TH>::default().fold_into(&mut a);
    assert_eq!((a.justify_items, a.justify_self), (b.justify_items, b.justify_self), "control: unset leaves taffy's defaults");
}

// ---- AETHERCSSOMWHITESPACELONGHANDSWRITE (LEDGER SR1044): white-space-collapse / text-wrap-mode / text-wrap write the one white-space code (named tail block) ----

use layout_core::css::{white_space_from_longhands, white_space_halves, white_space_longhands};

/// The code a style resolves to over a parent's `base` code (a div: no tag default), as every walker reads it.
fn ws_over(s: &SpecifiedStyle<TH>, base: u8) -> u8 {
    s.paint.white_space.unwrap_or_else(|| white_space_halves(Some(&s.paint), base))
}

/// AETHERCSSOMWHITESPACELONGHANDSWRITE (SR1044): the inverse table round-trips all six codes; the two pairs no code
/// holds are None.
#[test]
fn white_space_longhands_write_table_round_trips() {
    for c in 0..6u8 {
        let (collapse, wrap) = white_space_longhands(c);
        assert_eq!(white_space_from_longhands(collapse, wrap), Some(c), "code {c}");
    }
    assert_eq!(white_space_from_longhands("preserve-breaks", "nowrap"), None);
    assert_eq!(white_space_from_longhands("break-spaces", "nowrap"), None);
}

/// AETHERCSSOMWHITESPACELONGHANDSWRITE (SR1044): `white-space: pre; text-wrap-mode: wrap` is pre-wrap (Chromium), in one
/// block and across two rules; its nowrap is false; no half survives beside the full code.
#[test]
fn white_space_longhands_write_pre_then_wrap() {
    let s = styled(&[("white-space", "pre"), ("text-wrap-mode", "wrap")]);
    assert_eq!((s.paint.white_space, s.paint.nowrap), (Some(3), Some(false)), "one block: pre-wrap");
    assert_eq!((s.paint.ws_collapse, s.paint.ws_wrap), (None, None));
    let s = cascade(&[&[("white-space", "pre")], &[("text-wrap-mode", "wrap")]]);
    assert_eq!((s.paint.white_space, s.paint.nowrap), (Some(3), Some(false)), "across rules: pre-wrap");
    let s = cascade(&[&[("white-space", "pre-wrap")], &[("white-space-collapse", "collapse")]]);
    assert_eq!(s.paint.white_space, Some(0), "pre-wrap with collapse is normal");
    assert_eq!(styled(&[("white-space", "pre")]).paint.white_space, Some(2), "armed: pre alone");
}

/// AETHERCSSOMWHITESPACELONGHANDSWRITE (SR1044): a lone longhand holds no code: it resolves over the parent's code
/// (`text-wrap-mode: nowrap` under pre is pre, under pre-wrap pre, under normal nowrap; `white-space-collapse: preserve`
/// under normal is pre-wrap, pinned).
#[test]
fn white_space_longhands_write_lone_half_over_parent() {
    let s = styled(&[("text-wrap-mode", "nowrap")]);
    assert_eq!(s.paint.white_space, None, "no code at parse time");
    assert_eq!(s.paint.nowrap, Some(true), "its nowrap");
    assert_eq!((ws_over(&s, 2), ws_over(&s, 3), ws_over(&s, 0)), (2, 2, 1));
    let s = styled(&[("white-space-collapse", "preserve")]);
    assert_eq!(s.paint.nowrap, None, "a collapse half leaves nowrap to the parent");
    assert_eq!((ws_over(&s, 0), ws_over(&s, 1)), (3, 2), "preserve over normal is pre-wrap, over nowrap pre");
    let s = styled(&[("white-space-collapse", "preserve-breaks")]);
    assert_eq!(ws_over(&s, 0), 4, "preserve-breaks over normal is pre-line");
    assert_eq!(ws_over(&s, 1), 1, "a pair no code holds keeps the parent's code");
    let s = styled(&[("text-wrap-mode", "wrap")]);
    assert_eq!((s.paint.nowrap, ws_over(&s, 2)), (Some(false), 3), "wrap under pre is pre-wrap, nowrap false");
    assert_eq!(ws_over(&styled(&[("padding", "1px")]), 2), 2, "armed: no half inherits as before");
}

/// AETHERCSSOMWHITESPACELONGHANDSWRITE (SR1044): the shorthand after a longhand wins, in one block and across rules.
#[test]
fn white_space_longhands_write_shorthand_wins() {
    let s = styled(&[("text-wrap-mode", "nowrap"), ("white-space", "pre-wrap")]);
    assert_eq!((s.paint.white_space, s.paint.nowrap, s.paint.ws_wrap), (Some(3), Some(false), None));
    assert_eq!(ws_over(&s, 0), 3);
    let s = cascade(&[&[("white-space-collapse", "preserve"), ("text-wrap-mode", "nowrap")], &[("white-space", "normal")]]);
    assert_eq!((s.paint.white_space, s.paint.ws_collapse, s.paint.ws_wrap), (Some(0), None, None));
    let s = cascade(&[&[("text-wrap-mode", "nowrap")], &[("white-space-collapse", "preserve")]]);
    assert_eq!(ws_over(&s, 0), 2, "two lone halves from two rules combine: pre");
}

/// AETHERCSSOMWHITESPACELONGHANDSWRITE (SR1044): `text-wrap` sets the mode (an omitted mode is wrap: `balance` resets
/// an earlier nowrap); a bogus value, two modes, and `white-space-collapse: pre` (a white-space word) are dropped.
#[test]
fn white_space_longhands_write_text_wrap_and_bogus() {
    assert_eq!(ws_over(&styled(&[("text-wrap", "nowrap")]), 0), 1);
    assert_eq!(ws_over(&styled(&[("text-wrap", "nowrap balance")]), 3), 2);
    assert_eq!(styled(&[("white-space", "nowrap"), ("text-wrap", "balance")]).paint.white_space, Some(0), "balance resets the mode to wrap");
    assert_eq!(styled(&[("white-space", "pre"), ("text-wrap", "WRAP")]).paint.white_space, Some(3), "any case");
    for (p, v) in [("text-wrap", "bogus"), ("text-wrap", "wrap nowrap"), ("text-wrap-mode", "balance"), ("white-space-collapse", "pre"), ("white-space-collapse", "discard"), ("text-wrap-mode", "")] {
        let s = styled(&[(p, v)]);
        assert_eq!((s.paint.white_space, s.paint.ws_collapse, s.paint.ws_wrap, s.paint.nowrap), (None, None, None, None), "{p}: {v} dropped");
    }
}

/// AETHERCSSOMWHITESPACELONGHANDSWRITE (SR1044): a later `white-space: inherit` clears the earlier halves (one block and
/// across rules); a half after it stands over the inherited code.
#[test]
fn white_space_longhands_write_inherit_clears_halves() {
    let s = styled(&[("text-wrap-mode", "nowrap"), ("white-space", "inherit")]);
    assert_eq!((s.paint.ws_wrap, s.paint.nowrap), (None, None));
    let s = cascade(&[&[("white-space-collapse", "preserve")], &[("white-space", "inherit")]]);
    assert_eq!(s.paint.ws_collapse, None, "across rules");
    assert_eq!(ws_over(&s, 0), 0);
    let s = styled(&[("white-space", "inherit"), ("text-wrap-mode", "nowrap")]);
    assert_eq!(ws_over(&s, 3), 2, "the parent's preserve with the later nowrap: pre");
}

// ---- AETHERCSSCARETCOLOR (LEDGER SR1040): caret-color parses into the RGBA marker and inherits through its bit (named tail block) ----

/// AETHERCSSCARETCOLOR (SR1040): `caret-color` (css-ui-4 §4.1, `auto | <color>`): a colour is `Some(Some(rgba))`,
/// `transparent` alpha 0, `auto` / `initial` / `currentcolor` `Some(None)`; `invert` and junk are dropped.
#[test]
fn caret_color_parses() {
    let cc = |v: &str| styled(&[("caret-color", v)]).paint.caret_color;
    assert_eq!(cc("red"), Some(Some((255, 0, 0, 255))), "red");
    assert_eq!(cc("RED"), Some(Some((255, 0, 0, 255))), "case-insensitive");
    assert_eq!(cc("rgb(0, 160, 0)"), Some(Some((0, 160, 0, 255))), "rgb()");
    assert_eq!(cc("transparent"), Some(Some((0, 0, 0, 0))), "transparent: alpha 0, no caret");
    for v in ["auto", "AUTO", "initial", "currentcolor"] {
        assert_eq!(cc(v), Some(None), "{v} is the control's currentcolor");
    }
    for v in ["invert", "bogus", ""] {
        assert_eq!(cc(v), None, "{v:?} is dropped");
    }
}

/// AETHERCSSCARETCOLOR (SR1040): caret-color is inherited: `inherit` and `unset` after a red rule write
/// `INHERIT_CARET_COLOR` and drop the red; `initial` after red is `auto`; a later red clears the bit.
#[test]
fn caret_color_css_wide() {
    use layout_core::paint::INHERIT_CARET_COLOR;
    let red: &[(&str, &str)] = &[("caret-color", "red")];
    let s = cascade(&[red]);
    assert_eq!(s.paint.caret_color, Some(Some((255, 0, 0, 255))), "red (armed)");
    assert!(!s.paint.inherits(INHERIT_CARET_COLOR), "a value is no inherit (armed)");
    for kw in ["inherit", "unset"] {
        let s = cascade(&[red, &[("caret-color", kw)]]);
        assert_eq!(s.paint.caret_color, None, "caret-color: {kw} after red drops the red");
        assert!(s.paint.inherits(INHERIT_CARET_COLOR), "caret-color: {kw} writes the inherit bit");
    }
    let s = cascade(&[red, &[("caret-color", "initial")]]);
    assert_eq!(s.paint.caret_color, Some(None), "caret-color: initial after red is auto");
    let s = cascade(&[&[("caret-color", "inherit")], red]);
    assert_eq!(s.paint.caret_color, Some(Some((255, 0, 0, 255))), "a later red wins");
    assert!(!s.paint.inherits(INHERIT_CARET_COLOR), "a later red clears the bit");
}

// ---- AETHERCSSBACKGROUNDMULTILAYER (LEDGER SR1052): background-image is a comma layer list, painted bottom-up (named tail block) ----

/// AETHERCSSBACKGROUNDMULTILAYER (SR1052): `bg_more`'s urls in list order (the test host parses no gradient).
fn bg_more_urls(s: &SpecifiedStyle<TH>) -> Option<Vec<String>> {
    s.paint.bg_more.as_ref().map(|v| {
        v.iter()
            .map(|l| match l {
                layout_core::paint::BgLayer::Url(u) => u.clone(),
                layout_core::paint::BgLayer::Gradient(_) => "<gradient>".to_string(),
            })
            .collect()
    })
}

/// AETHERCSSBACKGROUNDMULTILAYER (SR1052): `url(a), url(b)` is two layers, the first in `bg_image` (the one-url value
/// is one layer: `bg_more` empty, armed); a `,` inside `url()` / a data: url / `rgb()` does not split.
#[test]
fn background_multi_layer_two_urls_in_order() {
    let one = styled(&[("background-image", "url(a.png)")]);
    assert_eq!((one.paint.bg_image.as_deref(), bg_more_urls(&one)), (Some("a.png"), Some(vec![])), "one layer (armed)");
    let s = styled(&[("background-image", "url(a.png), url(b.png)")]);
    assert_eq!(s.paint.bg_image.as_deref(), Some("a.png"), "the first layer is the top one");
    assert_eq!(bg_more_urls(&s), Some(vec!["b.png".to_string()]), "the second layer");
    let s = styled(&[("background-image", "url(\"a,b.png\"), url(data:image/png;base64,AA,BB)")]);
    assert_eq!(s.paint.bg_image.as_deref(), Some("a,b.png"), "a comma inside url() stays");
    assert_eq!(bg_more_urls(&s), Some(vec!["data:image/png;base64,AA,BB".to_string()]), "a data: url stays whole");
    let s = styled(&[("background", "url(a.png), url(b.png) rgb(0, 160, 0)")]);
    assert_eq!((s.paint.bg_image.as_deref(), bg_more_urls(&s)), (Some("a.png"), Some(vec!["b.png".to_string()])), "shorthand: two layers");
    assert_eq!(s.paint.background, Some((0, 160, 0)), "shorthand: the final layer's colour (rgb() not split)");
}

/// AETHERCSSBACKGROUNDMULTILAYER (SR1052): a url written before a gradient is the first layer (the test host reads no
/// gradient, so the list is the url alone and the gradient field is cleared).
#[test]
fn background_multi_layer_url_before_gradient() {
    let s = styled(&[("background-image", "url(a.png), linear-gradient(red, blue)")]);
    assert_eq!(s.paint.bg_image.as_deref(), Some("a.png"), "the url layer survives the gradient arm");
    assert!(matches!(s.paint.bg_gradient, Some(None)), "one property: the gradient field is this list's");
}

/// AETHERCSSBACKGROUNDMULTILAYER (SR1052): a later `background-image: url(c)` replaces the whole earlier list, and
/// `none` clears it (SR1015's road).
#[test]
fn background_multi_layer_later_rule_replaces_list() {
    let ab: &[(&str, &str)] = &[("background-image", "url(a.png), url(b.png)")];
    let s = cascade(&[ab]);
    assert_eq!(bg_more_urls(&s), Some(vec!["b.png".to_string()]), "the earlier list (armed)");
    let s = cascade(&[ab, &[("background-image", "url(c.png)")]]);
    assert_eq!((s.paint.bg_image.as_deref(), bg_more_urls(&s)), (Some("c.png"), Some(vec![])), "url(c) replaces [a, b]");
    let s = cascade(&[ab, &[("background-image", "none")]]);
    assert_eq!((s.paint.bg_image.as_deref(), bg_more_urls(&s)), (None, Some(vec![])), "none clears every layer");
    let s = cascade(&[ab, &[("background", "red")]]);
    assert_eq!((s.paint.bg_image.as_deref(), bg_more_urls(&s)), (None, Some(vec![])), "background: red resets the list (SR1061, was pinned kept)");
}

// ---- AETHERCSSBACKGROUNDSHORTHANDRESET (LEDGER SR1061): the background shorthand resets every omitted longhand (named tail block) ----

/// AETHERCSSBACKGROUNDSHORTHANDRESET (SR1061): css-backgrounds-3 §3.10: `background: red` after an image in one block
/// paints plain red (no image, no list); `background: red` alone is unchanged.
#[test]
fn background_shorthand_reset_same_block() {
    let s = styled(&[("background-image", "url(a.png)"), ("background", "red")]);
    assert_eq!((s.paint.bg_image.as_deref(), bg_more_urls(&s)), (None, Some(vec![])), "the image goes");
    assert_eq!((s.paint.background, s.paint.bg_alpha), (Some((255, 0, 0)), Some(1.0)), "the colour is red, opaque");
    let alone = styled(&[("background", "red")]);
    assert_eq!((alone.paint.background, alone.paint.bg_alpha, alone.paint.bg_image.as_deref()), (Some((255, 0, 0)), Some(1.0), None), "red alone");
}

/// AETHERCSSBACKGROUNDSHORTHANDRESET (SR1061): `background: blue` after `url(a) repeat-x 10px 20px / cover` resets the
/// image, repeat, position and size to their initials (each as its longhand stores it).
#[test]
fn background_shorthand_reset_repeat_position_size() {
    let first: &[(&str, &str)] = &[("background", "url(a.png) repeat-x 10px 20px / cover")];
    let before = cascade(&[first]);
    assert_eq!((before.paint.bg_image.as_deref(), before.paint.bg_repeat), (Some("a.png"), Some(2)), "the earlier image and repeat-x (armed)");
    let s = cascade(&[first, &[("background", "blue")]]);
    assert_eq!(s.paint.bg_image, None, "no image");
    assert_eq!(s.paint.bg_repeat, styled(&[("background-repeat", "repeat")]).paint.bg_repeat, "repeat is initial");
    assert_eq!(s.paint.bg_position.as_deref(), Some("0% 0%"), "position is initial");
    assert_eq!(s.paint.bg_size.as_deref(), Some("auto"), "size is initial");
    assert_eq!(s.paint.background, Some((0, 0, 255)), "the colour is the later blue");
}

/// AETHERCSSBACKGROUNDSHORTHANDRESET (SR1061): across rules the merge drops the earlier image (the explicit initial
/// wins), and `background: url(b)` after `background-color: red` makes the colour transparent.
#[test]
fn background_shorthand_reset_across_rules() {
    let s = cascade(&[&[("background-image", "url(a.png)")], &[("background", "red")]]);
    assert_eq!(s.paint.bg_image, None, "the earlier rule's image is dropped");
    assert!(matches!(s.paint.bg_gradient, Some(None)), "the gradient field is cleared too");
    assert_eq!(cascade(&[&[("background-color", "red")]]).paint.bg_alpha, Some(1.0), "the earlier red is opaque (armed)");
    let s = cascade(&[&[("background-color", "red")], &[("background", "url(b.png)")]]);
    assert_eq!(s.paint.bg_image.as_deref(), Some("b.png"), "the later image");
    assert_eq!(s.paint.bg_alpha, Some(0.0), "background-color is reset to transparent");
}

/// AETHERCSSBACKGROUNDSHORTHANDRESET (SR1061): `background: inherit` keeps its road (dropped: the earlier rule stands);
/// `background: currentcolor` after an image keeps the marker opaque and drops the image; the gradient arm resets too.
#[test]
fn background_shorthand_reset_keywords_and_gradient_arm() {
    use layout_core::paint::CC_BG;
    let first: &[(&str, &str)] = &[("background", "url(a.png) red")];
    let s = cascade(&[first, &[("background", "inherit")]]);
    assert_eq!((s.paint.bg_image.as_deref(), s.paint.background, s.paint.bg_alpha), (Some("a.png"), Some((255, 0, 0)), Some(1.0)), "inherit: unchanged road");
    let s = styled(&[("background-image", "url(a.png)"), ("background", "currentcolor")]);
    assert_eq!((s.paint.currentcolor & CC_BG, s.paint.bg_alpha, s.paint.bg_image.as_deref()), (CC_BG, Some(1.0), None), "currentcolor after the reset");
    let s = styled(&[("background-color", "currentcolor"), ("background", "url(a.png)")]);
    assert_eq!((s.paint.currentcolor & CC_BG, s.paint.bg_alpha), (0, Some(0.0)), "an omitted colour clears the marker");
    let s = cascade(&[&[("background-repeat", "no-repeat"), ("background-color", "red")], &[("background", "linear-gradient(red, blue)")]]);
    assert_eq!((s.paint.bg_repeat, s.paint.bg_alpha), (Some(0), Some(0.0)), "the gradient arm resets repeat and colour");
}

// ---- AETHERCSSBACKGROUNDLAYERPROPS (LEDGER SR1070): background-repeat / -position / -size are per-layer lists (named tail block) ----

/// AETHERCSSBACKGROUNDLAYERPROPS (SR1070): each layer's (url, repeat, position, size) as the painter reads them through
/// `bg_layers_bottom_up`, TOP layer first.
fn bg_layer_props(s: &SpecifiedStyle<TH>) -> Vec<(Option<String>, Option<u8>, Option<String>, Option<String>)> {
    let mut v: Vec<_> = s
        .paint
        .bg_layers_bottom_up()
        .iter()
        .map(|l| (l.url().map(str::to_string), l.repeat, l.position.map(str::to_string), l.size.map(str::to_string)))
        .collect();
    v.reverse();
    v
}

/// AETHERCSSBACKGROUNDLAYERPROPS (SR1070): `background-repeat: no-repeat, repeat` over two image layers is layer 0
/// no-repeat and layer 1 repeat (it was dropped whole: armed), position / size lists the same by index.
#[test]
fn background_layer_props_longhand_list_by_index() {
    let s = styled(&[
        ("background-image", "url(a.png), url(b.png)"),
        ("background-repeat", "no-repeat, repeat"),
        ("background-position", "center, 0 0"),
        ("background-size", "2px 2px, auto"),
    ]);
    assert_eq!((s.paint.bg_repeat, s.paint.bg_repeat_more.clone()), (Some(1), Some(vec![0])), "the repeat list is stored");
    let p = bg_layer_props(&s);
    assert_eq!(p[0], (Some("a.png".to_string()), Some(1), Some("center".to_string()), Some("2px 2px".to_string())), "layer 0");
    assert_eq!(p[1], (Some("b.png".to_string()), Some(0), Some("0 0".to_string()), Some("auto".to_string())), "layer 1");
    assert_eq!(styled(&[("background-repeat", "no-repeat, bogus")]).paint.bg_repeat, None, "an unparsable item drops the list");
}

/// AETHERCSSBACKGROUNDLAYERPROPS (SR1070): a shorter list repeats cyclically (§2.3): `background-size: cover` covers
/// all three layers, `no-repeat, repeat-x` alternates.
#[test]
fn background_layer_props_shorter_list_cycles() {
    let s = styled(&[
        ("background-image", "url(a.png), url(b.png), url(c.png)"),
        ("background-size", "cover"),
        ("background-repeat", "no-repeat, repeat-x"),
    ]);
    let p = bg_layer_props(&s);
    assert_eq!(p.len(), 3, "three layers");
    assert!(p.iter().all(|l| l.3.as_deref() == Some("cover")), "every layer covers: {p:?}");
    assert_eq!(p.iter().map(|l| l.1).collect::<Vec<_>>(), vec![Some(1), Some(2), Some(1)], "the repeat list cycles");
}

/// AETHERCSSBACKGROUNDLAYERPROPS (SR1070): the shorthand's words are scanned per comma layer: the logo layer keeps its
/// `no-repeat center / 50% auto`, the texture layer its `repeat` and the initials (armed: the old scan let the last
/// repeat word win for both and took `50% auto,` across the comma).
#[test]
fn background_layer_props_shorthand_per_layer() {
    let s = styled(&[("background", "url(a.png) no-repeat center / 50% auto, url(b.png) repeat")]);
    let p = bg_layer_props(&s);
    assert_eq!(p[0], (Some("a.png".to_string()), Some(1), Some("center".to_string()), Some("50% auto".to_string())), "the top layer");
    assert_eq!(p[1], (Some("b.png".to_string()), Some(0), Some("0% 0%".to_string()), Some("auto".to_string())), "the bottom layer");
    let one = styled(&[("background", "url(a.png) no-repeat 0 -304px / 100%")]);
    assert_eq!((one.paint.bg_repeat, one.paint.bg_position.as_deref(), one.paint.bg_size.as_deref()), (Some(1), Some("0 -304px"), Some("100%")), "one layer as before");
}

/// AETHERCSSBACKGROUNDLAYERPROPS (SR1070): across rules a later `background-size` list replaces the whole earlier list,
/// the `background` shorthand resets the lists, and a repeat list from one rule survives a later `background-image`
/// list (four independent longhands).
#[test]
fn background_layer_props_cascade() {
    let imgs: &[(&str, &str)] = &[("background-image", "url(a.png), url(b.png)")];
    let s = cascade(&[imgs, &[("background-size", "10px, 20px")]]);
    assert_eq!(bg_layer_props(&s).iter().map(|l| l.3.clone()).collect::<Vec<_>>(), vec![Some("10px".to_string()), Some("20px".to_string())], "the list (armed)");
    let s = cascade(&[imgs, &[("background-size", "10px, 20px")], &[("background-size", "30px")]]);
    assert_eq!(s.paint.bg_size_more, Some(vec![]), "the later one-value list replaces the earlier tail");
    assert!(bg_layer_props(&s).iter().all(|l| l.3.as_deref() == Some("30px")), "both layers take the later size");
    let s = cascade(&[&[("background-repeat", "no-repeat, repeat-x")], &[("background", "red")]]);
    assert_eq!((s.paint.bg_repeat, s.paint.bg_repeat_more.clone()), (Some(0), Some(vec![])), "the shorthand resets the list");
    assert_eq!((s.paint.bg_position_more.clone(), s.paint.bg_size_more.clone()), (Some(vec![]), Some(vec![])), "and the other two");
    let s = cascade(&[&[("background-repeat", "no-repeat, repeat-x")], imgs]);
    assert_eq!(bg_layer_props(&s).iter().map(|l| l.1).collect::<Vec<_>>(), vec![Some(1), Some(2)], "the repeat list survives a later image list");
}


// ---- AETHERTEXTALIGNJUSTIFY (LEDGER SR1057): css-text-3 §7.1 text-align: justify — the line-finish gap pass (named tail block) ----

/// AETHERTEXTALIGNJUSTIFY (SR1057): lays `<p id=p style="font-size: 10px; width: 200px; {style}">{kids}</p>` with the
/// fixed-advance fonts (a glyph is 5px) and returns each line's text frags as (x, right edge), in line order.
fn justify_lines(style: &str, kids: Vec<Spec>) -> Vec<Vec<(f32, f32)>> {
    let doc = page(vec![el("p", &[("id", "p"), ("style", &format!("margin: 0; font-size: 10px; width: 200px; {style}"))], kids)]);
    let tree = lay(&doc, 600.0, 400.0);
    let il = &tree.inline[&id_of(&tree, &doc.by_id("p"))];
    il.lines
        .iter()
        .map(|l| {
            il.frags
                .iter()
                .filter_map(|f| match f {
                    layout_core::inline::Frag::Text { x, width, baseline, .. } if *baseline == l.2 => Some((*x, *x + *width)),
                    _ => None,
                })
                .collect()
        })
        .collect()
}

fn right_edge(line: &[(f32, f32)]) -> f32 {
    line.iter().map(|f| f.1).fold(0.0, f32::max)
}

/// 15 six-letter words: five per 200px line (natural 170px), three lines.
fn fifteen() -> Vec<Spec> {
    vec![text(&["abcdef"; 15].join(" "))]
}

/// AETHERTEXTALIGNJUSTIFY (SR1057): lines 1-2 end flush at 200px, one frag per word; the last line is start (170);
/// the start twin is ragged; a `<br>`-ended line and a one-word line are not stretched; rtl: the last line sits at
/// the right.
#[test]
fn text_align_justify_lines() {
    let j = justify_lines("text-align: justify", fifteen());
    assert_eq!(j.len(), 3, "three lines: {j:?}");
    for l in &j[..2] {
        assert!((right_edge(l) - 200.0).abs() <= 0.5, "a justified line ends flush at 200: {l:?}");
        assert_eq!(l.len(), 5, "one frag per word on a justified line: {l:?}");
        assert_eq!(l[0].0, 0.0, "it starts at the line start: {l:?}");
    }
    assert!((right_edge(&j[2]) - 170.0).abs() <= 0.5, "the last line is start: {:?}", j[2]);
    let s = justify_lines("text-align: start", fifteen());
    assert!((right_edge(&s[0]) - 170.0).abs() <= 0.5, "the start twin is ragged: {:?}", s[0]);
    assert_eq!(s[0].len(), 1, "the start twin keeps one frag per chunk: {:?}", s[0]);
    let br = justify_lines("text-align: justify", vec![text("abcdef abcdef"), el("br", &[], vec![]), text(&["abcdef"; 10].join(" "))]);
    assert!((right_edge(&br[0]) - 65.0).abs() <= 0.5, "a <br>-ended line is not stretched: {br:?}");
    assert!((right_edge(&br[1]) - 200.0).abs() <= 0.5, "the next full line is: {br:?}");
    let one = justify_lines("text-align: justify", vec![text(&format!("{} abcdef", "a".repeat(36)))]);
    assert!((right_edge(&one[0]) - 180.0).abs() <= 0.5, "a one-word line is not stretched: {one:?}");
    let rtl = justify_lines("direction: rtl; text-align: justify", fifteen());
    assert!((right_edge(&rtl[0]) - 200.0).abs() <= 0.5 && rtl[0][0].0 == 0.0, "rtl: a justified line is flush both sides: {:?}", rtl[0]);
    assert!((right_edge(&rtl[2]) - 200.0).abs() <= 0.5 && (rtl[2][0].0 - 30.0).abs() <= 0.5, "rtl: the last line is start (right): {:?}", rtl[2]);
}

/// AETHERTEXTALIGNJUSTIFY (SR1057): justify is start for the container: a 200px block child stays at x 0 (the
/// -webkit-center twin centres it); the code reads back as its own value (8), not start.
#[test]
fn text_align_justify_container_start() {
    let at = |ta: &str| {
        let doc = page(vec![el("div", &[("style", &format!("text-align: {ta}"))], vec![el("div", &[("id", "d"), ("style", "width: 200px")], vec![text("ab")])])]);
        let tree = lay(&doc, 600.0, 400.0);
        box_of(&tree, &doc.by_id("d")).0
    };
    assert_eq!(at("justify"), 0.0, "justify leaves the narrow block at the start");
    assert_eq!(at("-webkit-center"), 200.0, "the -webkit-center twin centres it");
    assert_eq!(styled(&[("text-align", "justify")]).paint.text_align, Some(8), "justify is code 8");
    assert_eq!(styled(&[("text-align", "start")]).paint.text_align, Some(0), "start stays 0");
}

// ---- AETHERCSSOMBORDERSIDESTYLECOLUMNS (LEDGER SR1103): border_style_word is the exact inverse of border_style_code (named tail block) ----

/// AETHERCSSOMBORDERSIDESTYLECOLUMNS (SR1103): every paint code round-trips through its keyword, and the 3D styles
/// (which paint solid) collapse to code 0 and read back `solid`.
#[test]
fn border_style_word_round_trip() {
    use layout_core::css::{border_style_code, border_style_word};
    for (w, c) in [("solid", 0u8), ("dashed", 1), ("dotted", 2), ("double", 3)] {
        assert!(border_style_code(w) == Some(c) && border_style_word(c) == w, "{w} <-> {c}");
    }
    for w in ["groove", "ridge", "inset", "outset"] {
        assert_eq!(border_style_code(w).map(border_style_word), Some("solid"), "{w}");
    }
    assert_eq!((border_style_code("none"), border_style_code("hidden")), (None, None), "none / hidden clear the side, no code");
}

// ---- AETHERTEXTALIGNLAST (LEDGER SR1062): css-text-3 §7.2 text-align-last (named tail block) ----

/// AETHERTEXTALIGNLAST (SR1062): the frag (x0, x1) per line of `#p` inside `outer`-styled div.
fn align_last_lines(outer: &str, style: &str, kids: Vec<Spec>) -> Vec<Vec<(f32, f32)>> {
    let p = el("p", &[("id", "p"), ("style", &format!("margin: 0; font-size: 10px; width: 200px; {style}"))], kids);
    let doc = page(vec![el("div", &[("style", outer)], vec![p])]);
    let tree = lay(&doc, 600.0, 400.0);
    let il = &tree.inline[&id_of(&tree, &doc.by_id("p"))];
    il.lines
        .iter()
        .map(|l| {
            il.frags
                .iter()
                .filter_map(|f| match f {
                    layout_core::inline::Frag::Text { x, width, baseline, .. } if *baseline == l.2 => Some((*x, *x + *width)),
                    _ => None,
                })
                .collect()
        })
        .collect()
}

fn left_edge(line: &[(f32, f32)]) -> f32 {
    line.iter().map(|f| f.0).fold(f32::MAX, f32::min)
}

/// AETHERTEXTALIGNLAST (SR1062): on a justified paragraph the last line follows text-align-last: center (x0 15),
/// justify (flush at 200, the auto twin stays 170), right (x0 30, right 200); lines 1-2 stay flush at 200.
#[test]
fn text_align_last_justify_paragraph() {
    let c = justify_lines("text-align: justify; text-align-last: center", fifteen());
    assert_eq!(c.len(), 3, "three lines: {c:?}");
    for l in &c[..2] {
        assert!((right_edge(l) - 200.0).abs() <= 0.5 && left_edge(l) == 0.0, "lines 1-2 stay justified: {c:?}");
    }
    assert!((left_edge(&c[2]) - 15.0).abs() <= 0.5 && (right_edge(&c[2]) - 185.0).abs() <= 0.5, "the last line is centred: {:?}", c[2]);
    let j = justify_lines("text-align: justify; text-align-last: justify", fifteen());
    assert!((right_edge(&j[2]) - 200.0).abs() <= 0.5 && left_edge(&j[2]) == 0.0, "justify stretches the last line: {:?}", j[2]);
    let auto = justify_lines("text-align: justify; text-align-last: auto", fifteen());
    assert!((right_edge(&auto[2]) - 170.0).abs() <= 0.5, "armed: auto leaves the last line at start: {:?}", auto[2]);
    let r = justify_lines("text-align: justify; text-align-last: right", fifteen());
    assert!((left_edge(&r[2]) - 30.0).abs() <= 0.5 && (right_edge(&r[2]) - 200.0).abs() <= 0.5, "right: {:?}", r[2]);
    let rtl = justify_lines("direction: rtl; text-align: justify; text-align-last: justify", fifteen());
    assert!((right_edge(&rtl[2]) - 200.0).abs() <= 0.5 && left_edge(&rtl[2]) == 0.0, "rtl: the last line is flush both sides: {:?}", rtl[2]);
}

/// AETHERTEXTALIGNLAST (SR1062): a line ending at a `<br>` takes text-align-last too: centred (100px → x0 50) under
/// center, stretched to 200 under justify; the line after it is justified and the paragraph's last line centred.
#[test]
fn text_align_last_forced_break() {
    let kids = || vec![text("abcdef abcdef abcdef"), el("br", &[], vec![]), text(&["abcdef"; 10].join(" "))];
    let c = justify_lines("text-align: justify; text-align-last: center", kids());
    assert_eq!(c.len(), 3, "three lines: {c:?}");
    assert!((left_edge(&c[0]) - 50.0).abs() <= 0.5 && (right_edge(&c[0]) - 150.0).abs() <= 0.5, "the <br> line is centred: {:?}", c[0]);
    assert!((right_edge(&c[1]) - 200.0).abs() <= 0.5, "the full line after it is justified: {:?}", c[1]);
    assert!((left_edge(&c[2]) - 15.0).abs() <= 0.5, "the last line is centred: {:?}", c[2]);
    let j = justify_lines("text-align: justify; text-align-last: justify", kids());
    assert!((right_edge(&j[0]) - 200.0).abs() <= 0.5 && left_edge(&j[0]) == 0.0, "justify stretches the <br> line: {:?}", j[0]);
    assert_eq!(j[0].len(), 3, "one frag per word on the stretched <br> line: {:?}", j[0]);
}

/// AETHERTEXTALIGNLAST (SR1062): text-align-last applies whatever text-align is: `start` on a centred paragraph puts
/// the last line at x 0 while lines 1-2 stay centred.
#[test]
fn text_align_last_start_on_center() {
    let l = justify_lines("text-align: center; text-align-last: start", fifteen());
    for line in &l[..2] {
        assert!((left_edge(line) - 15.0).abs() <= 0.5, "lines 1-2 centred: {l:?}");
    }
    assert!(left_edge(&l[2]).abs() <= 0.5, "the last line at the start: {:?}", l[2]);
}

/// AETHERTEXTALIGNLAST (SR1062): text-align-last is inherited from a parent div; an explicit `auto` on the child
/// (255) overrides the inherited value.
#[test]
fn text_align_last_inherits() {
    let c = align_last_lines("text-align-last: center", "text-align: justify", fifteen());
    assert!((left_edge(&c[2]) - 15.0).abs() <= 0.5, "the inherited center: {:?}", c[2]);
    let a = align_last_lines("text-align-last: center", "text-align: justify; text-align-last: auto", fifteen());
    assert!(left_edge(&a[2]).abs() <= 0.5 && (right_edge(&a[2]) - 170.0).abs() <= 0.5, "an own auto wins: {:?}", a[2]);
}

/// AETHERTEXTALIGNLAST (SR1062): the parse codes (the text-align table, 255 auto), merge_paint carries the field
/// across rules, and the arm's name is in ALL_LONGHANDS.
#[test]
fn text_align_last_codes() {
    let code = |v: &str| styled(&[("text-align-last", v)]).paint.text_align_last;
    assert_eq!(code("center"), Some(1));
    assert_eq!(code("justify"), Some(8));
    assert_eq!(code("end"), Some(3));
    assert_eq!(code("auto"), Some(255));
    assert_eq!(code("initial"), Some(255));
    assert_eq!(code("inherit"), None);
    assert_eq!(code("bogus"), None);
    assert_eq!(cascade(&[&[("text-align-last", "right")], &[("color", "red")]]).paint.text_align_last, Some(2), "merge_paint keeps it");
    assert!(ALL_LONGHANDS.contains(&"text-align-last"), "the arm is listed for all:");
}

// ---- AETHERCSSOMTEXTDECORATIONCOLUMNS (LEDGER SR1108): text_decoration_style_word / underline_position_word invert the parsers' codes (named tail block) ----

/// AETHERCSSOMTEXTDECORATIONCOLUMNS (SR1108): every text-decoration-style code round-trips through its keyword; the
/// CSS-wide spellings the parser folds (initial / unset) read `solid`.
#[test]
fn text_decoration_style_word_round_trip() {
    use layout_core::css::{deco_style_code, text_decoration_style_word};
    for (w, c) in [("solid", 0u8), ("double", 1), ("dotted", 2), ("dashed", 3), ("wavy", 4)] {
        assert!(deco_style_code(w) == Some(c) && text_decoration_style_word(c) == w, "{w} <-> {c}");
    }
    for w in ["initial", "unset"] {
        assert_eq!(deco_style_code(w).map(text_decoration_style_word), Some("solid"), "{w}");
    }
}

/// AETHERCSSOMTEXTDECORATIONCOLUMNS (SR1108): every text-underline-position code round-trips through its keyword; the
/// spellings the parser folds (initial, and left / right, which only place the line in vertical writing) read their code's word.
#[test]
fn underline_position_word_round_trip() {
    use layout_core::css::{underline_position_code, underline_position_word};
    for (w, c) in [("auto", 0u8), ("from-font", 1), ("under", 2)] {
        assert!(underline_position_code(w) == Some(c) && underline_position_word(c) == w, "{w} <-> {c}");
    }
    for (w, want) in [("initial", "auto"), ("left", "auto"), ("under right", "under"), ("from-font left", "from-font")] {
        assert_eq!(underline_position_code(w).map(underline_position_word), Some(want), "{w}");
    }
}

// ---- AETHERCSSBORDERCOLORSTORED (LEDGER SR1058): a pending per-side border colour; border-color alone builds no frame (named tail block) ----

/// AETHERCSSBORDERCOLORSTORED (SR1058): the RGB a `border: 1px solid <c>` side parses to (the arm's own colour road).
fn stored_rgb(c: &str) -> (u8, u8, u8) {
    styled(&[("border", format!("1px solid {c}").as_str())]).paint.border.expect("border")[0].expect("side").1
}

const STORED_GREY: (u8, u8, u8) = (128, 128, 128);

/// AETHERCSSBORDERCOLORSTORED (SR1058): `border-color: red` alone builds no frame (style none: Chromium paints nothing);
/// before SR1058 it built `Some([Some((1.0, red)); 4])`. The colour pends for the four sides; `currentcolor` pends the
/// grey carrier with its currentcolor answer.
#[test]
fn border_color_stored_alone_no_frame() {
    let red = stored_rgb("red");
    let s = styled(&[("border-color", "red")]);
    assert_eq!(s.paint.border, None, "no frame");
    assert_eq!(s.paint.border_pending, Some([Some((red, false)); 4]), "red pends on four sides");
    let s = styled(&[("border-color", "currentcolor")]);
    assert_eq!((s.paint.border, s.paint.border_pending), (None, Some([Some((STORED_GREY, true)); 4])));
    let s = styled(&[("border-top-color", "red")]);
    assert_eq!((s.paint.border, s.paint.currentcolor), (None, 0), "a lone longhand: no side, no bit");
    assert_eq!(s.paint.border_pending, Some([Some((red, false)), None, None, None]), "the top pends");
}

/// AETHERCSSBORDERCOLORSTORED (SR1058): `border-width` creates the absent sides from the pending colour (the bit from the
/// entry), the rest at the grey carrier with the currentcolor bit, as before. SR1069 (flipped in place): `border-width`
/// creates NO side (style none paints nothing; before SR1069 the 2px frames below); the width is stored, and a style
/// then creates the sides at it from the pending colour, the rest at the grey carrier with the bit.
#[test]
fn border_color_stored_width_creates() {
    let red = stored_rgb("red");
    let s = styled(&[("border-color", "red"), ("border-width", "2px")]);
    assert_eq!((s.paint.border, s.paint.border_width), (None, Some([Some(2.0); 4])), "SR1069: no frame, the width stored");
    let s = styled(&[("border-color", "red"), ("border-width", "2px"), ("border-style", "solid")]);
    assert_eq!(s.paint.border, Some([Some((2.0, red)); 4]));
    assert_eq!(s.paint.currentcolor, 0);
    let s = styled(&[("border-top-color", "red"), ("border-width", "2px"), ("border-style", "solid")]);
    assert_eq!(s.paint.border, Some([Some((2.0, red)), Some((2.0, STORED_GREY)), Some((2.0, STORED_GREY)), Some((2.0, STORED_GREY))]));
    assert_eq!(s.paint.currentcolor, 0b1110, "the top is red, the others the initial currentcolor");
    let s = styled(&[("border-top-color", "currentcolor"), ("border-width", "2px"), ("border-style", "solid")]);
    assert_eq!(s.paint.currentcolor & 1, 1, "a pending currentcolor carries the SR849 bit");
}

/// AETHERCSSBORDERCOLORSTORED (SR1058): a style declaration creates the sides that have a pending colour (either order in
/// the block keeps the 1px frame the fabricated road drew); a style alone still creates none (owed: SIDE-STYLE-WIDTH).
/// SR1069 (flipped in place): the created sides are medium 3px (before: 1px), and a style alone creates them too.
#[test]
fn border_color_stored_style_creates() {
    let red = stored_rgb("red");
    for d in [[("border-color", "red"), ("border-style", "solid")], [("border-style", "solid"), ("border-color", "red")]] {
        assert_eq!(styled(&d).paint.border, Some([Some((3.0, red)); 4]), "{d:?} (SR1069: 3px, was 1px)");
    }
    let s = styled(&[("border-top-color", "red"), ("border-top-style", "solid")]);
    assert_eq!(s.paint.border, Some([Some((3.0, red)), None, None, None]), "the top only (SR1069: 3px)");
    assert_eq!(styled(&[("border-style", "solid")]).paint.border, Some([Some((3.0, STORED_GREY)); 4]), "SR1069: a style alone, medium currentcolor (was None)");
}

/// AETHERCSSBORDERCOLORSTORED (SR1058): an absent entry of an existing border is no longer fabricated when the block
/// declared no style (before: three 1px red sides); with a style word in the block the absent sides are created at 1px
/// (the in-block style rule: `border_style` is one array for the block, so which side declared it is not known:
/// AETHER-CSS-BORDER-STYLE-PER-SIDE owed); an existing side is recoloured as before. SR1069: created at medium 3px.
#[test]
fn border_color_stored_absent_entry_and_existing() {
    let (red, blue) = (stored_rgb("red"), stored_rgb("blue"));
    assert_ne!(red, blue, "armed: two colours");
    let s = styled(&[("border-top", "2px blue"), ("border-color", "red")]);
    assert_eq!(s.paint.border, Some([Some((2.0, red)), None, None, None]), "no style in the block: the top only");
    let s = styled(&[("border-top", "2px solid"), ("border-color", "red")]);
    assert_eq!(s.paint.border, Some([Some((2.0, red)), Some((3.0, red)), Some((3.0, red)), Some((3.0, red))]), "a style word: created at medium 3px (SR1069; was 1px)");
    let s = styled(&[("border", "1px solid blue"), ("border-color", "red")]);
    assert_eq!(s.paint.border, Some([Some((1.0, red)); 4]), "the existing road");
}

/// AETHERCSSBORDERCOLORSTORED (SR1058): css-backgrounds-3 §3.4 / §3.5: the `border` and `border-<side>` shorthands set
/// the colour longhand (omitted = the initial currentcolor), so a pending colour before them does not reach their
/// sides (Chromium paints currentcolor).
#[test]
fn border_color_stored_shorthand_resets() {
    use layout_core::paint::CC_SIDES;
    let s = styled(&[("border-top-color", "red"), ("border-top", "1px solid")]);
    assert_eq!(s.paint.border.expect("border")[0], Some((1.0, STORED_GREY)));
    assert_eq!(s.paint.currentcolor & 1, 1, "the top is currentcolor");
    let s = styled(&[("border-color", "red"), ("border", "1px solid")]);
    assert_eq!(s.paint.border, Some([Some((1.0, STORED_GREY)); 4]));
    assert_eq!(s.paint.currentcolor, CC_SIDES);
}

/// AETHERCSSBORDERCOLORSTORED (SR1058): the merge road: a later rule's pending colour recolours the earlier rule's sides,
/// width kept (before: the later block's fabricated 1px frame replaced the 3px border; a lone side longhand dropped);
/// a later shorthand resets to currentcolor; a later currentcolor sets the side bit; an earlier style-only rule takes a
/// later colour as a 1px frame (the in-block rule across rules; Chromium's medium 3px owed). SR1069: 3px.
#[test]
fn border_color_stored_merge() {
    let (red, blue) = (stored_rgb("red"), stored_rgb("blue"));
    let s = cascade(&[&[("border", "3px solid blue")], &[("border-color", "red")]]);
    assert_eq!(s.paint.border, Some([Some((3.0, red)); 4]), "3px kept, red");
    let s = cascade(&[&[("border", "2px solid blue")], &[("border-top-color", "red")]]);
    assert_eq!(s.paint.border, Some([Some((2.0, red)), Some((2.0, blue)), Some((2.0, blue)), Some((2.0, blue))]));
    let s = cascade(&[&[("border-left-color", "red")], &[("border-left", "1px solid")]]);
    assert_eq!(s.paint.border.expect("border")[3], Some((1.0, STORED_GREY)), "the later shorthand resets");
    assert_eq!(s.paint.currentcolor & 8, 8);
    let s = cascade(&[&[("border", "2px solid red")], &[("border-top-color", "currentcolor")]]);
    assert_eq!(s.paint.currentcolor, 1, "the top bit from the pending entry");
    let s = cascade(&[&[("border-style", "solid")], &[("border-color", "red")]]);
    assert_eq!(s.paint.border, Some([Some((3.0, red)); 4]), "style then colour across rules: the medium 3px frame (SR1069; was 1px)");
    let s = cascade(&[&[("border-color", "red")]]);
    assert_eq!(s.paint.border, None, "a colour alone across the merge: no frame");
    let s = cascade(&[&[("border-style", "none")], &[("border-color", "red")]]);
    assert_eq!(s.paint.border, Some([None; 4]), "an explicit none is not revived");
}

/// AETHERCSSBORDERCOLORSTORED (SR1058): `transparent` forgets a pending entry; `border: inherit` forgets the block's.
/// SR1069 (flipped in place): a style creates the side (`border-width` alone no longer does).
/// SR1112 (flipped in place): `transparent` is pended as a colour (black RGB, alpha 0 on that side), not forgotten.
#[test]
fn border_color_stored_transparent_and_inherit() {
    let s = styled(&[("border-top-color", "red"), ("border-top-color", "transparent"), ("border-width", "2px"), ("border-style", "solid")]);
    assert_eq!(s.paint.border.expect("border")[0], Some((2.0, (0, 0, 0))), "SR1112: the pending transparent (was STORED_GREY)");
    assert_eq!(s.paint.currentcolor & 1, 0, "SR1112: a colour, not currentcolor (was 1)");
    assert_eq!(s.paint.border_alpha.expect("alpha")[0], Some(0.0), "SR1112: alpha 0 on the top");
    assert_eq!(styled(&[("border-color", "red"), ("border", "inherit")]).paint.border_pending, None);
}

// ---- AETHERCSSBORDERSIDESTYLEWIDTH (LEDGER SR1069): a style alone creates medium sides; border-width stores, never fabricates (named tail block) ----

/// AETHERCSSBORDERSIDESTYLEWIDTH (SR1069): css-backgrounds-3 §4.2-4.3: `border-style: solid` alone creates four medium
/// (3px) currentcolor sides (before SR1069: None, nothing painted); `border-top-style: dashed` alone the top only.
#[test]
fn border_side_style_width_style_alone() {
    use layout_core::paint::CC_SIDES;
    let s = styled(&[("border-style", "solid")]);
    assert_eq!(s.paint.border, Some([Some((3.0, STORED_GREY)); 4]), "medium currentcolor sides");
    assert_eq!(s.paint.currentcolor, CC_SIDES, "the four currentcolor bits");
    let s = styled(&[("border-top-style", "dashed")]);
    assert_eq!(s.paint.border, Some([Some((3.0, STORED_GREY)), None, None, None]), "the top only");
    assert_eq!(s.paint.border_style, Some([1, 0, 0, 0]), "dashed stored on the top");
    let s = cascade(&[&[("border-style", "solid")]]);
    assert_eq!(s.paint.border, Some([Some((3.0, STORED_GREY)); 4]), "through the merge");
}

/// AETHERCSSBORDERSIDESTYLEWIDTH (SR1069): a width alone (style none) creates no side — the width is stored in the ONE
/// store `border_width` (before SR1069 `border-width: 2px` fabricated a 2px frame; AETHER-CSS-BORDER-WIDTH-STORED closed).
#[test]
fn border_side_style_width_width_alone() {
    let s = styled(&[("border-width", "2px")]);
    assert_eq!((s.paint.border, s.paint.border_width), (None, Some([Some(2.0); 4])));
    assert_eq!(s.paint.currentcolor, 0, "no side, no bit");
    let s = styled(&[("border-top-width", "2px")]);
    assert_eq!((s.paint.border, s.paint.border_width), (None, Some([Some(2.0), None, None, None])));
    assert_eq!(cascade(&[&[("border-width", "2px")]]).paint.border, None, "nothing through the merge");
}

/// AETHERCSSBORDERSIDESTYLEWIDTH (SR1069): width and style in either order, in one block or across rules, give 2px sides;
/// a style with a colour gives 3px in the colour.
#[test]
fn border_side_style_width_width_and_style() {
    let two = Some([Some((2.0, STORED_GREY)); 4]);
    for d in [[("border-width", "2px"), ("border-style", "solid")], [("border-style", "solid"), ("border-width", "2px")]] {
        assert_eq!(styled(&d).paint.border, two, "{d:?}");
    }
    assert_eq!(cascade(&[&[("border-width", "2px")], &[("border-style", "solid")]]).paint.border, two, "width then style across rules");
    assert_eq!(cascade(&[&[("border-style", "solid")], &[("border-width", "2px")]]).paint.border, two, "style then width across rules");
    let s = styled(&[("border-top-width", "5px"), ("border-style", "solid")]);
    assert_eq!(s.paint.border.expect("sides")[0], Some((5.0, STORED_GREY)), "a side longhand's stored width");
    assert_eq!(s.paint.border.expect("sides")[1], Some((3.0, STORED_GREY)), "the others medium");
    let red = stored_rgb("red");
    assert_eq!(styled(&[("border-style", "solid"), ("border-color", "red")]).paint.border, Some([Some((3.0, red)); 4]));
}

/// AETHERCSSBORDERSIDESTYLEWIDTH (SR1069): the `border` / `border-<side>` shorthands without a width part are medium 3px
/// through the same literal (AETHER-CSS-BORDER-SHORTHAND-MEDIUM closed; the UA `[popover] { border: solid }` is Chromium's
/// 3px); an explicit width stays; `border-style: none` after a border removes the sides; a later side width moves an
/// existing side (the layout box reads `border`).
#[test]
fn border_side_style_width_shorthands_and_none() {
    assert_eq!(styled(&[("border", "solid")]).paint.border, Some([Some((3.0, STORED_GREY)); 4]), "border: solid is medium");
    assert_eq!(styled(&[("border-top", "dashed")]).paint.border.expect("sides")[0], Some((3.0, STORED_GREY)));
    assert_eq!(styled(&[("border", "1px solid")]).paint.border, Some([Some((1.0, STORED_GREY)); 4]), "armed: an explicit width");
    assert_eq!(styled(&[("border", "1px solid"), ("border-style", "none")]).paint.border, Some([None; 4]), "none removes");
    let s = styled(&[("border", "1px solid"), ("border-top-style", "none")]);
    assert_eq!(s.paint.border.expect("sides").map(|x| x.is_some()), [false, true, true, true], "the top removed");
    let s = styled(&[("border", "1px solid"), ("border-top-width", "5px")]);
    assert_eq!(s.paint.border.expect("sides")[0], Some((5.0, STORED_GREY)), "the side synced");
    let s = styled(&[("border-width", "5px"), ("border", "1px solid")]);
    assert_eq!((s.paint.border.expect("sides")[0], s.paint.border_width), (Some((1.0, STORED_GREY)), Some([Some(1.0); 4])), "the shorthand resets the store");
    let s = styled(&[("border", "2px solid"), ("border", "none"), ("border-style", "solid")]);
    assert_eq!(s.paint.border, Some([Some((3.0, STORED_GREY)); 4]), "border: none resets the width to medium");
}

/// AETHERCSSBORDERSIDESTYLEWIDTH (SR1069): `fold_into` with no side: a stored width reaches only a non-zero (UA control)
/// edge, so `border-width: 0` drops a field's 2px UA edge and a div's 0 edge stays 0 (style none).
#[test]
fn border_side_style_width_fold_control_edge() {
    use taffy::style::{LengthPercentage, Style};
    let lp = LengthPercentage::length;
    let mut div = Style::default();
    styled(&[("border-width", "2px")]).fold_into(&mut div);
    assert_eq!(div.border.top, lp(0.0), "a div: no border box");
    let ua = |w: f32| {
        let mut st = Style::default();
        st.border = taffy::geometry::Rect { top: lp(2.0), right: lp(2.0), bottom: lp(2.0), left: lp(2.0) };
        styled(&[("border-width", &*format!("{w}px"))]).fold_into(&mut st);
        st.border
    };
    assert_eq!(ua(0.0).top, lp(0.0), "border-width: 0 drops the UA edge");
    assert_eq!(ua(1.0).left, lp(1.0), "border-width: 1px on the UA edge");
    let mut st = Style::default();
    styled(&[("border-style", "solid")]).fold_into(&mut st);
    assert_eq!(st.border.top, lp(3.0), "a style alone: the 3px side sizes the box");
}

// ---- AETHERCSSALIGNCONTENTSAFE (LEDGER SR1064): safe / unsafe / first|last baseline on the alignment arms (named tail block) ----

use layout_core::css::alignment_words;

/// AETHERCSSALIGNCONTENTSAFE (SR1064): the modifier grammar (css-align-3 §6): `safe` / `unsafe` strip before a
/// content-position word, `first` / `last` before `baseline`; one token passes through (bare `safe` is then refused by
/// the arm as before); malformed orders are None (the arm refuses the whole text).
#[test]
fn alignment_words_grammar() {
    assert_eq!(alignment_words("safe center"), Some(("center", true, 0)), "safe center");
    assert_eq!(alignment_words("unsafe end"), Some(("end", false, 0)), "unsafe end");
    assert_eq!(alignment_words("first baseline"), Some(("baseline", false, 1)), "first baseline");
    assert_eq!(alignment_words("last baseline"), Some(("baseline", false, 2)), "last baseline");
    assert_eq!(alignment_words("center"), Some(("center", false, 0)), "a plain word passes through");
    assert_eq!(alignment_words("safe"), Some(("safe", false, 0)), "bare safe passes through to the arm's refusal");
    for bad in ["safe safe center", "baseline first", "safe baseline", "safe space-between", "safe center end"] {
        assert_eq!(alignment_words(bad), None, "{bad}: refused");
    }
}

/// AETHERCSSALIGNCONTENTSAFE (SR1064): each of the three alignment arms (justify-content, align-content, align-items;
/// align-self has no arm: AETHER-CSS-ALIGN-SELF-ARM) takes `safe center` as its plain `center` twin, `unsafe end` as
/// `end`; align-items takes `first` / `last baseline` as `baseline`; the refused forms leave the field None.
#[test]
fn alignment_words_arms() {
    let one = |p: &str, v: &str| styled(&[(p, v)]);
    assert!(one("justify-content", "center").justify.is_some(), "armed: justify-content center");
    assert_eq!(one("justify-content", "safe center").justify, one("justify-content", "center").justify, "justify-content: safe center");
    assert_eq!(one("justify-content", "unsafe end").justify, one("justify-content", "end").justify, "justify-content: unsafe end");
    assert!(one("align-content", "center").align_content.is_some(), "armed: align-content center");
    assert_eq!(one("align-content", "safe center").align_content, one("align-content", "center").align_content, "align-content: safe center");
    assert_eq!(one("align-content", "unsafe end").align_content, one("align-content", "end").align_content, "align-content: unsafe end");
    assert!(one("align-items", "center").align_items.is_some(), "armed: align-items center");
    assert_eq!(one("align-items", "safe center").align_items, one("align-items", "center").align_items, "align-items: safe center");
    assert_eq!(one("align-items", "unsafe end").align_items, one("align-items", "end").align_items, "align-items: unsafe end");
    assert_eq!(one("align-items", "first baseline").align_items, one("align-items", "baseline").align_items, "align-items: first baseline");
    assert_eq!(one("align-items", "last baseline").align_items, one("align-items", "baseline").align_items, "align-items: last baseline");
    for bad in ["safe", "safe safe center", "baseline first"] {
        assert_eq!(one("justify-content", bad).justify, None, "justify-content: {bad} refused");
        assert_eq!(one("align-content", bad).align_content, None, "align-content: {bad} refused");
        assert_eq!(one("align-items", bad).align_items, None, "align-items: {bad} refused");
    }
}

/// AETHERCSSALIGNCONTENTSAFE (SR1064): a 200px flex row with one 50 x 20 child: `justify-content: safe center` centres
/// it like the plain `center` twin (x 75); before, the declaration was dropped and the child sat at the start.
#[test]
fn alignment_words_safe_center_layout() {
    let x = |jc: &'static str| {
        let doc = page(vec![el("div", &[("id", "fc")], vec![el("div", &[("id", "k"), ("style", "width: 50px; height: 20px")], vec![])])]);
        let fc = doc.by_id("fc");
        let rule: &[(&str, &str)] = &[("display", "flex"), ("width", "200px"), ("height", "20px"), ("justify-content", jc)];
        let tree = lay_styled(&doc, &Sheet(vec![(fc.id, std::rc::Rc::new(cascade(&[rule])))]), 400.0, 400.0);
        box_of(&tree, &doc.by_id("k")).0 - box_of(&tree, &fc).0
    };
    let (c, s) = (x("center"), x("safe center"));
    assert!((c - 75.0).abs() <= 0.5, "armed: center puts the child at 75: {c}");
    assert!((s - c).abs() <= 0.5, "safe center centres like center: {s} vs {c}");
}

// ---- AETHERCSSBORDERPENDINGTRANSPARENT (LEDGER SR1112): a transparent border colour keeps the side at alpha 0 (named tail block) ----

/// AETHERCSSBORDERPENDINGTRANSPARENT (SR1112): `border: 2px solid transparent` keeps four 2px sides (black RGB) at alpha 0
/// each (before SR1112 grey sides with no alpha: a grey frame), and the sides size the box in `fold_into`.
#[test]
fn border_transparent_keeps_shorthand_sides() {
    use taffy::style::{LengthPercentage, Style};
    let s = styled(&[("border", "2px solid transparent")]);
    assert_eq!(s.paint.border, Some([Some((2.0, (0, 0, 0))); 4]));
    assert_eq!(s.paint.border_alpha, Some([Some(0.0); 4]), "alpha 0 on every side");
    assert_eq!(s.paint.currentcolor & 0x0f, 0, "a colour, not currentcolor");
    let s = styled(&[("border-left", "3px solid rgba(0, 0, 0, 0)")]);
    assert_eq!(s.paint.border.expect("border")[3], Some((3.0, (0, 0, 0))), "a zero-alpha rgba is a colour too");
    assert_eq!(s.paint.border_alpha, Some([None, None, None, Some(0.0)]), "the side shorthand writes its side only");
    let mut st = Style::default();
    styled(&[("border", "2px solid transparent")]).fold_into(&mut st);
    assert_eq!(st.border.top, LengthPercentage::length(2.0), "the transparent side sizes the box");
    let s = styled(&[("border", "2px solid transparent"), ("border", "1px solid")]);
    assert_eq!(s.paint.border_alpha, Some([Some(1.0); 4]), "a later shorthand resets the alpha with the colour");
}

/// AETHERCSSBORDERPENDINGTRANSPARENT (SR1112): `border-color: transparent` keeps the sides at alpha 0 (before SR1112
/// `Some([None; 4])`); `border-top-color: transparent` makes the top alpha 0 and leaves the right opaque (before SR1112
/// the top was dropped).
#[test]
fn border_transparent_keeps_per_side_alpha() {
    use layout_core::css::border_side_alpha;
    let s = styled(&[("border", "2px solid red"), ("border-color", "transparent")]);
    assert_eq!(s.paint.border, Some([Some((2.0, (0, 0, 0))); 4]));
    assert_eq!(s.paint.border_alpha, Some([Some(0.0); 4]));
    let s = styled(&[("border", "1px solid red"), ("border-top-color", "transparent")]);
    assert_eq!(s.paint.border.expect("border")[0], Some((1.0, (0, 0, 0))), "the top is kept");
    assert_eq!((border_side_alpha(&s.paint, 0), border_side_alpha(&s.paint, 1)), (0.0, 1.0), "top alpha 0, right alpha 1");
}

/// AETHERCSSBORDERPENDINGTRANSPARENT (SR1112): `border-top-color: transparent` on a side with no entry pends the colour,
/// so a later `border-style: solid` creates the top at medium in it, alpha 0 (before SR1112 the entry was forgotten and
/// the top came up currentcolor).
#[test]
fn border_transparent_keeps_pending_entry() {
    let s = styled(&[("border-top-color", "transparent"), ("border-style", "solid")]);
    assert_eq!(s.paint.border_pending.expect("pending")[0], Some(((0, 0, 0), false)), "the entry is kept");
    assert_eq!(s.paint.border.expect("border")[0], Some((3.0, (0, 0, 0))));
    assert_eq!(s.paint.border_alpha.expect("alpha")[0], Some(0.0));
    assert_eq!(s.paint.border.expect("border")[1], Some((3.0, STORED_GREY)), "the right side currentcolor");
}

/// AETHERCSSBORDERPENDINGTRANSPARENT (SR1112): the alpha merges per side across rules: a later `border-top-color: red`
/// over `border: 2px solid transparent` makes only the top opaque (the reserve-space idiom's hover rule).
#[test]
fn border_transparent_keeps_merge_per_side() {
    use layout_core::css::border_side_alpha;
    let s = cascade(&[&[("border", "2px solid transparent")], &[("border-top-color", "red")]]);
    let red = s.paint.border.expect("border")[0].expect("top").1;
    assert_eq!(s.paint.border.expect("border")[0], Some((2.0, red)), "the top recoloured, width kept");
    assert_ne!(red, (0, 0, 0));
    assert_eq!((border_side_alpha(&s.paint, 0), border_side_alpha(&s.paint, 1)), (1.0, 0.0), "top opaque, right still alpha 0");
    let s = cascade(&[&[("border", "2px solid transparent")], &[("border-color", "red")]]);
    assert_eq!(s.paint.border_alpha, Some([Some(1.0); 4]), "a later border-color: red makes every side opaque");
}

// ---- AETHERCSSPLACESHORTHANDS (LEDGER SR1074): css-align-3 §6.4 / §7.4 / §8.3 place-content / place-items / place-self (named tail block) ----

/// AETHERCSSPLACESHORTHANDS (SR1074): `place-content` writes align-content and justify-content through their own arms
/// (each field equal to its longhand twin), one value or two; a `safe center` pair is ONE value (both halves).
#[test]
fn place_shorthands_content() {
    let one = |p: &str, v: &str| styled(&[(p, v)]);
    let s = one("place-content", "center");
    assert!(s.align_content.is_some() && s.justify.is_some(), "armed: place-content: center writes both");
    assert_eq!(s.align_content, one("align-content", "center").align_content, "center: align half");
    assert_eq!(s.justify, one("justify-content", "center").justify, "center: justify half");
    let s = one("place-content", "start end");
    assert_eq!(s.align_content, one("align-content", "start").align_content, "start end: align start");
    assert_eq!(s.justify, one("justify-content", "end").justify, "start end: justify end");
    let s = one("place-content", "safe center");
    assert_eq!(s.align_content, one("align-content", "center").align_content, "safe center is one value: align");
    assert_eq!(s.justify, one("justify-content", "center").justify, "safe center is one value: justify");
    let s = one("place-content", "safe center end");
    assert_eq!(s.align_content, one("align-content", "center").align_content, "safe center | end: align");
    assert_eq!(s.justify, one("justify-content", "end").justify, "safe center | end: justify");
    for bad in ["bogus", "center end start", "safe", ""] {
        let s = one("place-content", bad);
        assert_eq!((s.align_content, s.justify), (None, None), "{bad:?}: refused, nothing written");
    }
}

/// AETHERCSSPLACESHORTHANDS (SR1074): `place-items` / `place-self` write align-items / align-self (justify-items /
/// justify-self have no arm: the second half is recorded, nothing else written); `place-self: auto` is SR1068's marker.
#[test]
fn place_shorthands_items_self() {
    let one = |p: &str, v: &str| styled(&[(p, v)]);
    let s = one("place-items", "center");
    assert!(s.align_items.is_some(), "armed: place-items: center writes align-items");
    assert_eq!(s.align_items, one("align-items", "center").align_items, "place-items: center");
    assert_eq!(s.justify, None, "justify-content untouched");
    assert_eq!(one("place-items", "first baseline").align_items, one("align-items", "baseline").align_items, "first baseline is one value");
    assert_eq!(one("place-items", "end start").align_items, one("align-items", "end").align_items, "two values: the first");
    assert_eq!(one("place-items", "bogus").align_items, None, "bogus refused");
    assert_eq!(one("place-self", "flex-end").align_self, Some(AlignSelf::END), "place-self: flex-end");
    assert_eq!(one("place-self", "center start").align_self, Some(AlignSelf::CENTER), "place-self: center start");
    let s = one("place-self", "auto");
    assert_eq!(s.align_self, None, "place-self: auto");
    assert!(s.align_self_auto, "place-self: auto carries the auto marker");
    let s = one("place-self", "bogus");
    assert!(s.align_self.is_none() && !s.align_self_auto, "place-self: bogus refused");
    let c = cascade(&[&[("align-items", "flex-start")], &[("place-items", "center")]]);
    assert_eq!(c.align_items, one("align-items", "center").align_items, "a later place-items beats an earlier align-items");
}

/// AETHERCSSPLACESHORTHANDS (SR1074): the three are shorthands: `all` resets their longhands, never them.
#[test]
fn place_shorthands_in_skipped() {
    for n in ["place-content", "place-items", "place-self"] {
        assert!(ALL_SKIPPED.contains(&n) && !ALL_LONGHANDS.contains(&n), "{n} is a skipped shorthand");
    }
}
