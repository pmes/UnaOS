//! AETHERRADIUSCALC (SR416): a math function is ONE component of a box shorthand (css-values-4 §10; css-backgrounds-3
//! §5.1 `border-radius: <length-percentage>{1,4} [ / <length-percentage>{1,4} ]?`). The shorthands read their
//! components with `split_top_level` (and border-radius its `/` with `split_top_level_slash`), so `calc(10px + 10px)`,
//! `min()` / `max()` / `clamp()` and a `calc()` division are never cut at their inner spaces or slash into fragments
//! that drop the whole declaration (all four corners square).
mod common;
use common::*;
use layout_core::css::{apply_declaration, parse_flex_shorthand, split_top_level_slash, ParseCx, SpecifiedStyle};
use layout_core::taffy::style::{Dimension, LengthPercentage};

fn styled(decls: &[(&str, &str)]) -> SpecifiedStyle<TH> {
    let cx = ParseCx::default();
    let mut s = SpecifiedStyle::<TH>::default();
    for (prop, value) in decls {
        apply_declaration(prop, value, &mut s, &cx);
    }
    s
}

fn radius(value: &str) -> Option<[f32; 4]> {
    styled(&[("border-radius", value)]).paint.radius
}

#[test]
fn radius_calc_each_corner() {
    assert_eq!(radius("calc(10px + 10px)"), Some([20.0; 4]), "one calc radius sets all four corners");
    assert_eq!(radius("4px calc(1px + 1px) 6px calc(2px * 4)"), Some([4.0, 2.0, 6.0, 8.0]), "TL TR BR BL, two of them calc");
    assert_eq!(radius("calc(1px + 2px) 5px"), Some([3.0, 5.0, 3.0, 5.0]), "the two-value expansion with a calc");
    for (i, prop) in ["border-top-left-radius", "border-top-right-radius", "border-bottom-right-radius", "border-bottom-left-radius"]
        .iter()
        .enumerate()
    {
        let mut want = [0.0; 4];
        want[i] = 10.0;
        assert_eq!(styled(&[(prop, "calc(5px + 5px)")]).paint.radius, Some(want), "{prop}: calc(5px + 5px)");
    }
    assert_eq!(radius("1px 2px 3px 4px 5px"), None, "five radii stay invalid");
}

#[test]
fn radius_slash_calc_both_sides() {
    assert_eq!(
        radius("calc(4px + 4px) calc(20px / 2) / calc(1px + 1px)"),
        Some([8.0, 10.0, 8.0, 10.0]),
        "the horizontal radii before the top-level slash; the division inside calc() is not the separator"
    );
    assert_eq!(radius("calc(30px / 3)/calc(8px / 2)"), Some([10.0; 4]), "a calc division on both sides, no spaces round the slash");
    assert_eq!(radius("50%/10%"), Some([-0.5; 4]), "a percentage radius keeps its negative-fraction form");
    assert_eq!(split_top_level_slash("calc(20px / 2) 4px / 2px"), ("calc(20px / 2) 4px ", Some(" 2px")));
    assert_eq!(split_top_level_slash("min(4px, 2px)"), ("min(4px, 2px)", None));
}

#[test]
fn radius_min_max_clamp() {
    assert_eq!(radius("min(4px, 10px)"), Some([4.0; 4]));
    assert_eq!(radius("clamp(2px, 6px, 8px)"), Some([6.0; 4]));
    assert_eq!(radius("max(1px, 3px) 2px"), Some([3.0, 2.0, 3.0, 2.0]));
}

#[test]
fn shorthand_function_values() {
    let g = styled(&[("gap", "calc(2px + 3px) 4px")]);
    assert_eq!((g.row_gap, g.column_gap), (Some(LengthPercentage::length(5.0)), Some(LengthPercentage::length(4.0))), "gap");

    let sp = styled(&[("scroll-padding", "calc(1px + 1px) 0")]);
    assert_eq!(sp.paint.scroll_padding, Some([2.0, 0.0, 2.0, 0.0]), "scroll-padding");

    let sm = styled(&[("scroll-margin", "calc(3px + 3px)")]);
    assert_eq!(sm.paint.scroll_margin, Some([6.0; 4]), "scroll-margin");

    let smb = styled(&[("scroll-margin-block", "calc(1px + 2px) 0")]);
    assert_eq!(smb.paint.scroll_margin, Some([3.0, 0.0, 0.0, 0.0]), "scroll-margin-block: start = top, end = bottom");

    let c = styled(&[("columns", "calc(100px + 20px) 3")]);
    assert_eq!((c.paint.col_width, c.paint.col_count), (Some(Some(120.0)), Some(Some(3))), "columns");

    let b = styled(&[("background", "url(x.png) rgb(0 0 255) no-repeat")]);
    assert_eq!(b.paint.background, Some((0, 0, 255)), "background: a space-separated colour function beside an image");

    assert_eq!(parse_flex_shorthand("1 1 calc(10px + 20px)", &ParseCx::default()), Some((1.0, 1.0, Dimension::length(30.0))), "flex");
}
