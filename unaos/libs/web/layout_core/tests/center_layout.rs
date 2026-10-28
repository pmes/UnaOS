//! CHARTER: Aether (CODEX §2 Web), seam shared-core — AETHERCENTERLAYOUT (SR421): `<center>`, `align=center` and
//! `text-align: -webkit-center` centre a block child narrower than its container (a table, a definite width; HTML
//! rendering §15.3.3, Blink's -webkit-center), while plain `text-align: center` aligns inline content only and leaves
//! the same blocks at the start (CSS 2.2 §16.2); an auto-width block still fills.
mod common;
use common::*;

const VW: f32 = 600.0;

/// The narrow children: a shrink-to-fit table, a 200px div holding a line of text, and an auto-width div.
fn kids() -> Vec<Spec> {
    vec![
        el("table", &[("id", "t")], vec![el("tr", &[], vec![el("td", &[], vec![text("cell")])])]),
        el("div", &[("id", "d"), ("style", "width: 200px")], vec![text("ab")]),
        el("div", &[("id", "f")], vec![text("fill")]),
    ]
}

fn centred(tree: &Tree, doc: &N, id: &str, how: &str) {
    let (x, _, w, _) = box_of(tree, &doc.by_id(id));
    assert!(w > 0.0 && w < VW - 100.0, "{how}: #{id} is narrower than its container ({w})");
    assert!((x + w / 2.0 - VW / 2.0).abs() < 0.5, "{how}: #{id} is centred (x {x} w {w})");
}

fn fills(tree: &Tree, doc: &N, how: &str) {
    let (x, _, w, _) = box_of(tree, &doc.by_id("f"));
    assert_eq!((x, w), (0.0, VW), "{how}: an auto-width block still fills");
}

#[test]
fn center_centres_narrow_blocks() {
    let cases: [(&str, N); 3] = [
        ("<center>", page(vec![el("center", &[], kids())])),
        ("<div align=center>", page(vec![el("div", &[("align", "center")], kids())])),
        ("text-align: -webkit-center", page(vec![el("div", &[("style", "text-align: -webkit-center")], kids())])),
    ];
    for (how, doc) in &cases {
        let tree = lay(doc, VW, 400.0);
        centred(&tree, doc, "t", how);
        centred(&tree, doc, "d", how);
        fills(&tree, doc, how);
    }
}

#[test]
fn plain_center_leaves_blocks() {
    let doc = page(vec![el("div", &[("style", "text-align: center")], kids())]);
    let tree = lay(&doc, VW, 400.0);
    for id in ["t", "d"] {
        let (x, _, w, _) = box_of(&tree, &doc.by_id(id));
        assert!(w > 0.0 && w < VW - 100.0, "#{id} is narrower than its container ({w})");
        assert_eq!(x, 0.0, "plain text-align: center does not move the block #{id}");
    }
    fills(&tree, &doc, "text-align: center");
    // The inline content inside the 200px block is still centred in it.
    let il = &tree.inline[&id_of(&tree, &doc.by_id("d"))];
    let (x, w) = il
        .frags
        .iter()
        .find_map(|f| match f {
            layout_core::inline::Frag::Text { x, width, .. } => Some((*x, *width)),
            _ => None,
        })
        .expect("the 200px block's text line");
    assert!((x + w / 2.0 - 100.0).abs() <= 1.0, "inline text centred inside the block: x {x} w {w}");
}

/// AETHERCENTERCELL (SR436): the same narrow children inside ONE cell of a 600px table (`td, th { padding: 1px }`).
fn cell_page(td_attrs: &[(&'static str, &str)]) -> N {
    let mut attrs = vec![("id", "cell")];
    attrs.extend_from_slice(td_attrs);
    page(vec![el(
        "table",
        &[("style", "width: 600px; border-spacing: 0")],
        vec![el("tr", &[], vec![el("td", &attrs, kids())])],
    )])
}

/// The cell's content box: (left, width).
fn cell_content(tree: &Tree, doc: &N) -> (f32, f32) {
    let (x, _, w, _) = box_of(tree, &doc.by_id("cell"));
    (x + 1.0, w - 2.0)
}

/// AETHERCENTERCELL (SR436): a table cell is a block container (CSS 2.2 §17.4), so `td align=center` and
/// `text-align: -webkit-center` on a td centre a nested table and a definite-width block in the cell, as Chrome does;
/// an auto-width block still fills the cell.
#[test]
fn cell_centres_narrow_blocks() {
    let cases: [(&str, N); 2] = [
        ("<td align=center>", cell_page(&[("align", "center")])),
        ("td text-align: -webkit-center", cell_page(&[("style", "text-align: -webkit-center")])),
    ];
    for (how, doc) in &cases {
        let tree = lay(doc, VW, 400.0);
        let (cx, cw) = cell_content(&tree, doc);
        assert!(cw > 400.0, "{how}: the cell is wide ({cw})");
        for id in ["t", "d"] {
            let (x, _, w, _) = box_of(&tree, &doc.by_id(id));
            assert!(w > 0.0 && w < cw - 100.0, "{how}: #{id} is narrower than the cell ({w} of {cw})");
            assert!((x + w / 2.0 - (cx + cw / 2.0)).abs() < 0.5, "{how}: #{id} is centred in the cell (x {x} w {w}, cell {cx} + {cw})");
        }
        let (x, _, w, _) = box_of(&tree, &doc.by_id("f"));
        assert!((x - cx).abs() < 0.5 && (w - cw).abs() < 0.5, "{how}: an auto-width block fills the cell (x {x} w {w}, cell {cx} + {cw})");
    }
}

/// AETHERCENTERCELL (SR436): plain `text-align: center` on a td aligns inline content only (CSS 2.2 §16.2): the
/// nested table and the definite-width block sit at the cell's content-box start.
#[test]
fn cell_plain_center_leaves_blocks() {
    let doc = cell_page(&[("style", "text-align: center")]);
    let tree = lay(&doc, VW, 400.0);
    let (cx, cw) = cell_content(&tree, &doc);
    for id in ["t", "d"] {
        let (x, _, w, _) = box_of(&tree, &doc.by_id(id));
        assert!(w > 0.0 && w < cw - 100.0, "#{id} is narrower than the cell ({w} of {cw})");
        assert!((x - cx).abs() < 0.5, "plain text-align: center in a td does not move the block #{id} (x {x}, cell {cx})");
    }
}

/// AETHERCENTERCAPTION (SR446): the same narrow children inside the `<caption>` of a 600px table (one row so table.rs
/// lays the table; the caption fills the table's content box and has no padding).
fn caption_page(cap_attrs: &[(&'static str, &str)]) -> N {
    let mut attrs = vec![("id", "cap")];
    attrs.extend_from_slice(cap_attrs);
    page(vec![el(
        "table",
        &[("style", "width: 600px; border-spacing: 0")],
        vec![el("caption", &attrs, kids()), el("tr", &[], vec![el("td", &[], vec![text("row")])])],
    )])
}

/// AETHERCENTERCAPTION (SR446): a caption is a block container (CSS 2.2 §17.4), so `caption align=center` and
/// `text-align: -webkit-center` on a caption centre a nested table and a definite-width block in it, as Chrome does;
/// an auto-width block still fills the caption.
#[test]
fn caption_centres_narrow_blocks() {
    let cases: [(&str, N); 2] = [
        ("<caption align=center>", caption_page(&[("align", "center")])),
        ("caption text-align: -webkit-center", caption_page(&[("style", "text-align: -webkit-center")])),
    ];
    for (how, doc) in &cases {
        let tree = lay(doc, VW, 400.0);
        let (cx, _, cw, _) = box_of(&tree, &doc.by_id("cap"));
        assert!(cw > 400.0, "{how}: the caption is wide ({cw})");
        for id in ["t", "d"] {
            let (x, _, w, _) = box_of(&tree, &doc.by_id(id));
            assert!(w > 0.0 && w < cw - 100.0, "{how}: #{id} is narrower than the caption ({w} of {cw})");
            assert!((x + w / 2.0 - (cx + cw / 2.0)).abs() < 0.5, "{how}: #{id} is centred in the caption (x {x} w {w}, caption {cx} + {cw})");
        }
        let (x, _, w, _) = box_of(&tree, &doc.by_id("f"));
        assert!((x - cx).abs() < 0.5 && (w - cw).abs() < 0.5, "{how}: an auto-width block fills the caption (x {x} w {w}, caption {cx} + {cw})");
    }
}

/// AETHERCENTERCAPTION (SR446): plain `text-align: center` on a caption aligns inline content only (CSS 2.2 §16.2):
/// the nested table and the definite-width block sit at the caption's start.
#[test]
fn caption_plain_center_leaves_blocks() {
    let doc = caption_page(&[("style", "text-align: center")]);
    let tree = lay(&doc, VW, 400.0);
    let (cx, _, cw, _) = box_of(&tree, &doc.by_id("cap"));
    for id in ["t", "d"] {
        let (x, _, w, _) = box_of(&tree, &doc.by_id(id));
        assert!(w > 0.0 && w < cw - 100.0, "#{id} is narrower than the caption ({w} of {cw})");
        assert!((x - cx).abs() < 0.5, "plain text-align: center in a caption does not move the block #{id} (x {x}, caption {cx})");
    }
}

/// AETHERCAPTIONUAALIGN (SR452): a bare caption takes Blink's html.css `caption { text-align: -webkit-center }` (the
/// seat's ruling: 5, not the spec's center), so its text line centres in the caption; an author `text-align: left`
/// (a stylesheet rule or `style=""`) overrides the UA value and the line sits at the caption's start.
#[test]
fn caption_ua_centres_text() {
    fn doc_with(cap_attrs: &[(&'static str, &str)]) -> N {
        let mut attrs = vec![("id", "cap")];
        attrs.extend_from_slice(cap_attrs);
        page(vec![el(
            "table",
            &[("style", "width: 600px; border-spacing: 0")],
            vec![el("caption", &attrs, vec![text("Quarterly results")]), el("tr", &[], vec![el("td", &[], vec![text("row")])])],
        )])
    }
    // The caption's first text fragment: (x, width) in the caption's content box, and the caption's width.
    fn line(tree: &Tree, doc: &N) -> (f32, f32, f32) {
        let cap = doc.by_id("cap");
        let (_, _, cw, _) = box_of(tree, &cap);
        let il = &tree.inline[&id_of(tree, &cap)];
        let (x, w) = il
            .frags
            .iter()
            .find_map(|f| match f {
                layout_core::inline::Frag::Text { x, width, .. } => Some((*x, *width)),
                _ => None,
            })
            .expect("the caption's text line");
        (x, w, cw)
    }

    let bare = doc_with(&[]);
    let (x, w, cw) = line(&lay(&bare, VW, 400.0), &bare);
    assert!(cw > 400.0 && w > 0.0 && w < cw - 100.0, "bare caption: a short line in a wide caption (w {w} of {cw})");
    assert!((x + w / 2.0 - cw / 2.0).abs() <= 1.0, "bare caption: the UA -webkit-center centres the text (x {x} w {w} of {cw})");

    let cx = layout_core::css::ParseCx::new(VW, 400.0);
    let mut s = layout_core::css::SpecifiedStyle::<TH>::default();
    layout_core::css::apply_declaration("text-align", "left", &mut s, &cx);
    let sheet = Sheet(vec![(bare.by_id("cap").id, std::rc::Rc::new(s))]);
    let (x, _, _) = line(&lay_styled(&bare, &sheet, VW, 400.0), &bare);
    assert_eq!(x, 0.0, "an author rule caption {{ text-align: left }} wins over the UA value");

    let inline = doc_with(&[("style", "text-align: left")]);
    let (x, _, _) = line(&lay(&inline, VW, 400.0), &inline);
    assert_eq!(x, 0.0, "style=\"text-align: left\" on the caption wins over the UA value");
}
