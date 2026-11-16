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

// ---- AETHERCAPTIONCSSOM (LEDGER SR686): top captions first, in DOM order (named tail block) ----

/// AETHERCAPTIONCSSOM (SR686): a top caption the parser left AFTER the rows (a late `<caption>` start tag) lays out
/// above them, as Chromium's table layout places captions by side whatever their DOM position; a bottom caption after
/// the rows stays below.
#[test]
fn top_caption_after_rows_moves_first() {
    let doc = page(vec![el(
        "table",
        &[("style", "width: 600px; border-spacing: 0")],
        vec![
            el("tr", &[], vec![el("td", &[("id", "cell")], vec![text("row")])]),
            el("caption", &[("id", "cap")], vec![text("Quarterly results")]),
            el("caption", &[("id", "low"), ("align", "bottom")], vec![text("Footnote")]),
        ],
    )]);
    let tree = lay(&doc, VW, 400.0);
    let (_, cy, _, ch) = box_of(&tree, &doc.by_id("cap"));
    let (_, ty, _, th) = box_of(&tree, &doc.by_id("cell"));
    let (_, ly, _, lh) = box_of(&tree, &doc.by_id("low"));
    assert!(ch > 0.0 && th > 0.0 && lh > 0.0, "every box has height (caption {ch}, cell {th}, bottom caption {lh})");
    assert!(cy + ch <= ty + 0.5, "the top caption after the row sits above it (caption y {cy} + {ch}, cell y {ty})");
    assert!(ly >= ty + th - 0.5, "the bottom caption stays below the row (caption y {ly}, cell y {ty} + {th})");
}

/// AETHERCAPTIONCSSOM (SR686): captions keep their DOM order on each side, and a re-fold of the SAME tree (anim's
/// `apply_specs` on the live tree an earlier fold reordered) that flips a bottom caption to top puts it back above the
/// rows at its DOM rank.
#[test]
fn caption_order_is_stable_among_captions() {
    let doc = page(vec![el(
        "table",
        &[("style", "width: 600px; border-spacing: 0")],
        vec![
            el("caption", &[("id", "b"), ("align", "bottom")], vec![text("Bottom")]),
            el("tr", &[], vec![el("td", &[("id", "cell")], vec![text("row")])]),
            el("caption", &[("id", "t1")], vec![text("First")]),
            el("caption", &[("id", "t2")], vec![text("Second")]),
        ],
    )]);
    let mut tree = lay(&doc, VW, 400.0);
    let y = |tree: &Tree, id: &str| box_of(tree, &doc.by_id(id)).1;
    let (_, ty, _, th) = box_of(&tree, &doc.by_id("cell"));
    assert!(y(&tree, "t1") < y(&tree, "t2") && y(&tree, "t2") < ty, "the top captions sit above the row in DOM order");
    assert!(y(&tree, "b") >= ty + th - 0.5, "the bottom caption sits below the row");

    let cx = layout_core::css::ParseCx::new(VW, 400.0);
    let mut s = layout_core::css::SpecifiedStyle::<TH>::default();
    layout_core::css::apply_declaration("caption-side", "top", &mut s, &cx);
    let b = id_of(&tree, &doc.by_id("b"));
    layout_core::apply_specs(&mut tree, &[(b, std::rc::Rc::new(s))]);
    let table = tree.taffy.parent(b).expect("the caption's table");
    let order: Vec<layout_core::taffy::NodeId> = tree.taffy.children(table).unwrap();
    let want = [id_of(&tree, &doc.by_id("b")), id_of(&tree, &doc.by_id("t1")), id_of(&tree, &doc.by_id("t2"))];
    assert_eq!(&order[..3], &want[..], "the flipped caption goes back above the rows at its DOM rank (first)");
    layout_core::remeasure(&mut tree, &F);
    let ty = box_of(&tree, &doc.by_id("cell")).1;
    assert!(y(&tree, "b") < y(&tree, "t1") && y(&tree, "t2") < ty, "laid out again: b, t1, t2 above the row");
}
