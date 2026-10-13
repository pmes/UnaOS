//! AETHERSTYLE3 (SR103): `filter` / `backdrop-filter` through svg_core's hook. The fixture's filtered box is
//! laid out, its paint style carries the list the cascade stored, and the box's background rasterized into a
//! layer goes through `effects::filter_rgba`: grayscale(1) turns its red pixels grey, blur() spreads them past
//! the box edge.
mod common;
use common::*;

#[test]
fn filter_grayscale_and_blur_pixels() {
    let doc = page(vec![
        el("div", &[("id", "f"), ("style", "width: 10px; height: 10px; background: rgb(255, 0, 0); filter: grayscale(1)")], vec![]),
        el("div", &[("id", "b"), ("style", "width: 10px; height: 10px; backdrop-filter: blur(2px)")], vec![]),
        el("div", &[("id", "bad"), ("style", "filter: frobnicate(3)")], vec![]),
    ]);
    let tree = lay(&doc, 100.0, 100.0);
    let f = doc.by_id("f");
    let (x, y, w, h) = box_of(&tree, &f);
    assert_eq!((x, y, w, h), (0.0, 0.0, 10.0, 10.0));
    let p = &tree.paint_map[&id_of(&tree, &f)];
    let list = p.filter.clone().flatten().expect("filter stored");
    assert_eq!(list, "grayscale(1)");
    assert_eq!(tree.paint_map[&id_of(&tree, &doc.by_id("b"))].backdrop_filter.clone().flatten().as_deref(), Some("blur(2px)"));
    assert!(tree.paint_map[&id_of(&tree, &doc.by_id("bad"))].filter.is_none(), "an invalid list is not stored");

    // rasterize the box's background into a 20x20 layer at (5, 5) and filter it
    let cx = layout_core::effects::FilterCx::default();
    let mut layer = vec![0u8; 20 * 20 * 4];
    for yy in 5..15 {
        for xx in 5..15 {
            let i = (yy * 20 + xx) * 4;
            layer[i..i + 4].copy_from_slice(&[255, 0, 0, 255]);
        }
    }
    let mut grey = layer.clone();
    assert!(layout_core::effects::filter_rgba(&mut grey, 20, 20, &list, &cx));
    let c = &grey[(10 * 20 + 10) * 4..(10 * 20 + 10) * 4 + 4];
    assert_eq!(c[0], c[1]);
    assert_eq!(c[1], c[2]);
    assert!(c[0] > 30 && c[0] < 80, "red's luminance (0.2126) in grey: {c:?}");
    assert_eq!(c[3], 255);

    let mut blurred = layer.clone();
    assert!(layout_core::effects::filter_rgba(&mut blurred, 20, 20, "blur(2px)", &cx));
    let out = &blurred[(10 * 20 + 3) * 4..(10 * 20 + 3) * 4 + 4];
    assert!(out[3] > 0, "blur spreads alpha 2px outside the box: {out:?}");
    assert!(layout_core::effects::filter_margin("blur(2px)", &cx) >= 6.0);
    assert!(!layout_core::effects::filter_rgba(&mut layer, 20, 20, "none", &cx));
}
