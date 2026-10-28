//! CHARTER: Aether (CODEX §2 Web), seam shared-core — AETHERCAPTIONSIDE (SR460): a `<caption>`'s align= is Blink's
//! caption-side presentation attribute (HTMLTableCaptionElement: top | bottom only), never text-align: align=bottom
//! places the caption below the table's rows, align=top / none keeps it above, align=left leaves the UA
//! -webkit-center text alignment alone.
mod common;
use common::*;

const VW: f32 = 600.0;

fn doc_with(cap_attrs: &[(&'static str, &str)]) -> N {
    let mut attrs = vec![("id", "cap")];
    attrs.extend_from_slice(cap_attrs);
    page(vec![el(
        "table",
        &[("style", "width: 600px; border-spacing: 0")],
        vec![
            el("caption", &attrs, vec![text("Quarterly results")]),
            el("tr", &[], vec![el("td", &[("id", "cell")], vec![text("row")])]),
        ],
    )])
}

#[test]
fn caption_align_is_caption_side() {
    // Placement: bottom goes below the row; top and no align stay above it.
    for (how, attrs, below) in [
        ("align=bottom", &[("align", "bottom")][..], true),
        ("align=BOTTOM", &[("align", " BOTTOM ")][..], true),
        ("align=top", &[("align", "top")][..], false),
        ("bare caption", &[][..], false),
        ("align=left", &[("align", "left")][..], false),
    ] {
        let doc = doc_with(attrs);
        let tree = lay(&doc, VW, 400.0);
        let (_, cy, _, ch) = box_of(&tree, &doc.by_id("cap"));
        let (_, ty, _, th) = box_of(&tree, &doc.by_id("cell"));
        assert!(ch > 0.0 && th > 0.0, "{how}: both boxes have height (caption {ch}, cell {th})");
        if below {
            assert!(cy >= ty + th - 0.5, "{how}: the caption sits below the row (caption y {cy}, cell y {ty} + {th})");
        } else {
            assert!(cy + ch <= ty + 0.5, "{how}: the caption sits above the row (caption y {cy} + {ch}, cell y {ty})");
        }
    }

    // Alignment: align=left is not text-align on a caption, so the UA -webkit-center still centres the line.
    let doc = doc_with(&[("align", "left")]);
    let tree = lay(&doc, VW, 400.0);
    let cap = doc.by_id("cap");
    let (_, _, cw, _) = box_of(&tree, &cap);
    let (x, w) = tree.inline[&id_of(&tree, &cap)]
        .frags
        .iter()
        .find_map(|f| match f {
            layout_core::inline::Frag::Text { x, width, .. } => Some((*x, *width)),
            _ => None,
        })
        .expect("the caption's text line");
    assert!(cw > 400.0 && w > 0.0 && w < cw - 100.0, "align=left: a short line in a wide caption (w {w} of {cw})");
    assert!((x + w / 2.0 - cw / 2.0).abs() <= 1.0, "align=left: the caption's text stays centred (x {x} w {w} of {cw})");
}

/// AETHERCAPTIONSIDECSS (SR462): the `caption-side` property, carried in `PaintStyle::caption_side`; align= is its
/// lowest-priority source, so style="" and the author cascade beat it, and the property inherits from the table.
#[test]
fn caption_side_property_beats_align() {
    let placed = |tree: &Tree, doc: &N| {
        let (_, cy, _, ch) = box_of(tree, &doc.by_id("cap"));
        let (_, ty, _, th) = box_of(tree, &doc.by_id("cell"));
        assert!(ch > 0.0 && th > 0.0, "both boxes have height (caption {ch}, cell {th})");
        (cy >= ty + th - 0.5, cy + ch <= ty + 0.5)
    };

    let doc = doc_with(&[("style", "caption-side: bottom")]);
    let (below, _) = placed(&lay(&doc, VW, 400.0), &doc);
    assert!(below, "style=\"caption-side: bottom\" places the caption below the row");

    let doc = doc_with(&[("align", "bottom"), ("style", "caption-side: top")]);
    let (_, above) = placed(&lay(&doc, VW, 400.0), &doc);
    assert!(above, "style=\"caption-side: top\" beats align=bottom: the caption stays above the row");

    // An author rule on the TABLE: the bare caption inherits bottom (the cascade folded in `apply_specs`).
    let doc = page(vec![el(
        "table",
        &[("id", "tbl"), ("style", "width: 600px; border-spacing: 0")],
        vec![
            el("caption", &[("id", "cap")], vec![text("Quarterly results")]),
            el("tr", &[], vec![el("td", &[("id", "cell")], vec![text("row")])]),
        ],
    )]);
    let cx = layout_core::css::ParseCx::new(VW, 400.0);
    let mut s = layout_core::css::SpecifiedStyle::<TH>::default();
    layout_core::css::apply_declaration("caption-side", "bottom", &mut s, &cx);
    let sheet = Sheet(vec![(doc.by_id("tbl").id, std::rc::Rc::new(s))]);
    let (below, _) = placed(&lay_styled(&doc, &sheet, VW, 400.0), &doc);
    assert!(below, "an author rule table {{ caption-side: bottom }} is inherited: the caption sits below the row");
}
