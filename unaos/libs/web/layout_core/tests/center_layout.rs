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
