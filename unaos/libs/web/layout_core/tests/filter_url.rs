//! AETHERSTYLE3B (SR115): `filter: url(#id)` on an HTML box references an inline SVG `<filter>`. The cascade
//! stores the list; `effects::resolve_filter` serializes the referenced element for svg_core; and
//! `effects::filter_rgba_refs` runs the list in order over the box's layer: a `feColorMatrix` saturate(0)
//! filter turns the red box grey, a function after the reference still runs, and a reference to a missing
//! element (or to a non-filter element) is skipped, leaving the layer as it was.
mod common;
use common::*;

#[test]
fn url_filter_references_run_through_svg_core() {
    let doc = page(vec![
        el(
            "svg",
            &[("width", "0"), ("height", "0")],
            vec![el(
                "filter",
                &[("id", "grey")],
                vec![el("feColorMatrix", &[("type", "saturate"), ("values", "0")], vec![])],
            )],
        ),
        el("div", &[("id", "f"), ("style", "width: 10px; height: 10px; background: rgb(255, 0, 0); filter: url(#grey)")], vec![]),
        el("div", &[("id", "notafilter")], vec![]),
    ]);
    let tree = lay(&doc, 100.0, 100.0);
    let f = doc.by_id("f");
    let list = tree.paint_map[&id_of(&tree, &f)].filter.clone().flatten().expect("url() list stored");
    assert_eq!(layout_core::effects::filter_urls(&list), vec!["grey".to_string()]);
    let markup = layout_core::effects::resolve_filter(&f, "grey").expect("the <filter> resolves");
    assert!(markup.starts_with("<filter id=\"grey\">") && markup.contains("<feColorMatrix type=\"saturate\" values=\"0\">"), "{markup}");
    assert!(layout_core::effects::resolve_filter(&f, "notafilter").is_none());

    let cx = layout_core::effects::FilterCx::default();
    let mut layer = vec![0u8; 20 * 20 * 4];
    for yy in 5..15 {
        for xx in 5..15 {
            let i = (yy * 20 + xx) * 4;
            layer[i..i + 4].copy_from_slice(&[255, 0, 0, 255]);
        }
    }
    let resolve = |id: &str| layout_core::effects::resolve_filter(&f, id);
    let bbox = (5.0, 5.0, 10.0, 10.0);
    let mut grey = layer.clone();
    assert!(layout_core::effects::filter_rgba_refs(&mut grey, 20, 20, &list, &cx, bbox, &resolve));
    let c = &grey[(10 * 20 + 10) * 4..(10 * 20 + 10) * 4 + 4];
    assert!(c[0] == c[1] && c[1] == c[2] && c[0] > 0 && c[0] < 200, "saturate(0) greys the red box: {c:?}");
    assert_eq!(c[3], 255);

    // the reference, then a blur: the grey spreads past the box edge
    let mut both = layer.clone();
    assert!(layout_core::effects::filter_rgba_refs(&mut both, 20, 20, "url(#grey) blur(2px)", &cx, bbox, &resolve));
    let edge = &both[(10 * 20 + 3) * 4..(10 * 20 + 3) * 4 + 4];
    assert!(edge[3] > 0, "the function after the reference ran: {edge:?}");

    // unresolvable references are skipped: nothing ran, the layer is untouched
    let mut same = layer.clone();
    assert!(!layout_core::effects::filter_rgba_refs(&mut same, 20, 20, "url(#missing)", &cx, bbox, &resolve));
    assert!(!layout_core::effects::filter_rgba_refs(&mut same, 20, 20, "url(#notafilter)", &cx, bbox, &resolve));
    assert_eq!(same, layer);
    assert!(layout_core::effects::filter_margin_box("url(#grey)", &cx, 10.0, 10.0) >= 25.0);
}
