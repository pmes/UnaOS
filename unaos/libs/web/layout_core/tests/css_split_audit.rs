//! AETHERCSSSPLITAUDIT (SR426): the value splits SR416 left — the background shorthand's position / size / repeat
//! scans, the depth-0 `/` of background, container, font and clip-path `round`, and anim.rs's numeric list — read
//! their components with `split_top_level` and their `/` with `split_top_level_slash`, so a function value
//! (`rgb(0 0 0 / 50%)`, `calc(10px + 5px)`, `calc(20px / 2)`) is one component, never cut at its inner space or slash.
mod common;
use common::*;
use layout_core::anim::{parse_value, AnimValue, Num};
use layout_core::css::{apply_declaration, ParseCx, SpecifiedStyle};
use layout_core::paint::ClipShape;

fn styled(decls: &[(&str, &str)]) -> SpecifiedStyle<TH> {
    let cx = ParseCx::default();
    let mut s = SpecifiedStyle::<TH>::default();
    for (prop, value) in decls {
        apply_declaration(prop, value, &mut s, &cx);
    }
    s
}

#[test]
fn background_slash_colour() {
    let s = styled(&[("background", "rgb(0 0 0 / 50%)")]);
    assert_eq!(s.paint.background, Some((0, 0, 0)), "the colour function is the layer's colour");
    assert_eq!(s.paint.bg_alpha, Some(0.5), "its alpha after the slash inside the function");
    assert_eq!(s.paint.bg_position, None, "the slash inside rgb() is not the position / size separator");
    assert_eq!(s.paint.bg_size, None, "no size from the fragment after the colour's slash");
}

#[test]
fn background_calc_position_size() {
    let s = styled(&[("background", "url(a.png) calc(10px + 5px) 0 / calc(50% - 2px) auto no-repeat")]);
    assert_eq!(s.paint.bg_position.as_deref(), Some("calc(10px + 5px) 0"), "a calc x offset stays one component");
    assert_eq!(s.paint.bg_size.as_deref(), Some("calc(50% - 2px) auto"), "the size pair counts a calc as one token");
    assert_eq!(s.paint.bg_repeat, Some(1), "the repeat keyword still found");
}

#[test]
fn position_token_math() {
    let s = styled(&[("background", "url(a.png) min(4px, 8px) clamp(1px, 2px, 3px) no-repeat")]);
    assert_eq!(s.paint.bg_position.as_deref(), Some("min(4px, 8px) clamp(1px, 2px, 3px)"), "min() / clamp() are positions");
    let s = styled(&[("background", "url(a.png) MAX(1px, 2px) 0 #00ff00")]);
    assert_eq!(s.paint.bg_position.as_deref(), Some("MAX(1px, 2px) 0"), "the function name is case-insensitive");
    assert_eq!(s.paint.background, Some((0, 255, 0)), "the colour after a math-function position");
}

#[test]
fn container_slash() {
    let s = styled(&[("container", "card / inline-size")]);
    assert_eq!(s.paint.container_name, Some(vec!["card".to_string()]));
    assert_eq!(s.paint.container_type, Some(2), "inline-size after the depth-0 slash");
}

#[test]
fn font_calc_size_and_line_height() {
    let s = styled(&[("font", "12px/calc(10px + 6px) serif")]);
    assert_eq!(s.paint.font_size, Some(12.0));
    assert_eq!(s.paint.line_height, Some(-16.0), "a calc line-height is one component after the slash");
    assert!(s.paint.family.is_some(), "the family list follows the size component");
    let s = styled(&[("font", "italic calc(10px + 2px) serif")]);
    assert_eq!(s.paint.font_size, Some(12.0), "a calc size is not cut at its inner spaces");
    assert_eq!(s.paint.italic, Some(true));
    assert!(s.paint.family.is_some());
}

#[test]
fn clip_path_round_calc() {
    let s = styled(&[("clip-path", "inset(10px round calc(20px / 2))")]);
    match s.paint.clip_path {
        Some(Some(ClipShape::Inset(_, rad))) => assert_eq!(rad, [10.0, 0.0], "the calc division is the radius, not the / separator"),
        other => panic!("inset() with a calc round radius parses: {other:?}"),
    }
    match styled(&[("clip-path", "inset(10px round 4px / 2px)")]).paint.clip_path {
        Some(Some(ClipShape::Inset(_, rad))) => assert_eq!(rad, [4.0, 0.0], "the horizontal radius before the depth-0 slash"),
        other => panic!("inset() round with a slash parses: {other:?}"),
    }
}

#[test]
fn anim_calc_interpolates() {
    let cx = ParseCx::default();
    assert_eq!(parse_value::<TH>("width", "calc(10px + 5px)", &cx), AnimValue::Nums(vec![Num::Px(15.0)]), "a calc length is one number");
    assert_eq!(
        parse_value::<TH>("margin", "calc(1px + 1px) 4px", &cx),
        AnimValue::Nums(vec![Num::Px(2.0), Num::Px(4.0)]),
        "a calc component beside a plain one"
    );
    assert!(matches!(parse_value::<TH>("width", "1px rgb(0 0 0)", &cx), AnimValue::Discrete(_)), "a non-math function stays discrete");
}
