//! CHARTER: Aether (CODEX §2 Web), seam shared-core — AETHERCELLBLOCKIFY (SR447): a td whose inline child the author
//! made `display: block` lays out as a block container (CSS 2.2 §9.2.1.1, §17.4): the block child takes its own line,
//! the inline runs beside it flow in anonymous block boxes above and below it, and SR436's cell rule centres a
//! definite-width author block under `td align=center`; the inverse (block tags made inline-block) share one line.
mod common;
use common::*;
use layout_core::inline::Frag;
use layout_core::taffy::NodeId;

const VW: f32 = 600.0;

/// ONE cell of a 600px table (`td, th { padding: 1px }`) holding `kids`.
fn cell_page(td_attrs: &[(&'static str, &str)], kids: Vec<Spec>) -> N {
    let mut attrs = vec![("id", "cell")];
    attrs.extend_from_slice(td_attrs);
    page(vec![el(
        "table",
        &[("style", "width: 600px; border-spacing: 0")],
        vec![el("tr", &[], vec![el("td", &attrs, kids)])],
    )])
}

/// The cell's content box: (left, width).
fn cell_content(tree: &Tree, doc: &N) -> (f32, f32) {
    let (x, _, w, _) = box_of(tree, &doc.by_id("cell"));
    (x + 1.0, w - 2.0)
}

/// A box's border box in page coordinates (anonymous boxes included): (x, y, w, h).
fn abs(tree: &Tree, id: NodeId) -> (f32, f32, f32, f32) {
    let l = tree.taffy.layout(id).unwrap();
    let (mut x, mut y) = (l.location.x, l.location.y);
    let mut cur = id;
    while let Some(p) = tree.taffy.parent(cur) {
        let pl = tree.taffy.layout(p).unwrap();
        x += pl.location.x;
        y += pl.location.y;
        cur = p;
    }
    (x, y, l.size.width, l.size.height)
}

/// The inline formatting context root whose fragments draw the text `t`.
fn ifc_of(tree: &Tree, t: &str) -> NodeId {
    *tree
        .inline
        .iter()
        .find(|(_, il)| il.frags.iter().any(|f| matches!(f, Frag::Text { text, .. } if text.trim() == t)))
        .unwrap_or_else(|| panic!("a line draws {t:?}"))
        .0
}

/// `<td align=center>a <b>b</b> <span style="display:block; width:200px">B</span> c</td>`: the author block is
/// centred in the cell, "a" and "b" share one line above it and "c" is on a line below it.
#[test]
fn cell_author_block_centres() {
    let doc = cell_page(
        &[("align", "center")],
        vec![
            text("a "),
            el("b", &[("id", "bb")], vec![text("b")]),
            text(" "),
            el("span", &[("id", "s"), ("style", "display: block; width: 200px")], vec![text("B")]),
            text(" c"),
        ],
    );
    let tree = lay(&doc, VW, 400.0);
    let (cx, cw) = cell_content(&tree, &doc);
    assert!(cw > 400.0, "the cell is wide ({cw})");
    let (sx, sy, sw, sh) = box_of(&tree, &doc.by_id("s"));
    assert!((sw - 200.0).abs() < 0.5, "the author block keeps its 200px width ({sw})");
    assert!((sx + sw / 2.0 - (cx + cw / 2.0)).abs() < 0.5, "the author block is centred in the cell (x {sx} w {sw}, cell {cx} + {cw})");
    let (before, after) = (ifc_of(&tree, "a"), ifc_of(&tree, "c"));
    let bid = id_of(&tree, &doc.by_id("bb"));
    let carries_b = tree.inline[&before].frags.iter().any(|f| match f {
        Frag::Text { text, .. } => text.trim() == "b",
        Frag::Box { node, .. } | Frag::Atomic { node, .. } => *node == bid,
    });
    assert!(carries_b, "\"a\" and <b>b</b> flow in one line box");
    assert_ne!(before, after, "\"c\" is not on the line of \"a\"");
    assert_eq!(tree.inline[&before].lines.len(), 1, "\"a b\" is one line");
    let (_, by, _, bh) = abs(&tree, before);
    let (_, ay, _, _) = abs(&tree, after);
    assert!(by + bh <= sy + 0.5, "the \"a b\" line sits above the author block ({by} + {bh} vs {sy})");
    assert!(ay >= sy + sh - 0.5, "the \"c\" line sits below the author block ({ay} vs {sy} + {sh})");
}

/// An auto-width author block fills the cell and its text centres by the inherited align; plain text-align: center
/// leaves a definite-width author block at the cell's content start (CSS 2.2 §16.2).
#[test]
fn cell_author_block_fills() {
    let doc = cell_page(
        &[("align", "center")],
        vec![text("x "), el("span", &[("id", "s"), ("style", "display: block")], vec![text("fill")]), text(" y")],
    );
    let tree = lay(&doc, VW, 400.0);
    let (cx, cw) = cell_content(&tree, &doc);
    let (sx, _, sw, _) = box_of(&tree, &doc.by_id("s"));
    assert!((sx - cx).abs() < 0.5 && (sw - cw).abs() < 0.5, "the auto-width author block fills the cell (x {sx} w {sw}, cell {cx} + {cw})");
    let il = &tree.inline[&ifc_of(&tree, "fill")];
    let (x, w) = il
        .frags
        .iter()
        .find_map(|f| match f {
            Frag::Text { x, width, text, .. } if text.trim() == "fill" => Some((*x, *width)),
            _ => None,
        })
        .expect("the author block's text");
    assert!((x + w / 2.0 - sw / 2.0).abs() <= 1.0 || (x + w / 2.0 - (sx + sw / 2.0)).abs() <= 1.0, "its text is centred in it: x {x} w {w}, block {sx} + {sw}");

    let doc = cell_page(
        &[("style", "text-align: center")],
        vec![text("x "), el("span", &[("id", "s"), ("style", "display: block; width: 200px")], vec![text("B")]), text(" y")],
    );
    let tree = lay(&doc, VW, 400.0);
    let (cx, _) = cell_content(&tree, &doc);
    let (sx, _, sw, _) = box_of(&tree, &doc.by_id("s"));
    assert!((sw - 200.0).abs() < 0.5, "the author block keeps its 200px width ({sw})");
    assert!((sx - cx).abs() < 0.5, "plain text-align: center leaves the author block at the cell's start (x {sx}, cell {cx})");
}

/// The same rule the other way: two divs made `display: inline-block` in a td flow side by side on one line.
#[test]
fn cell_inline_block_children_share_a_line() {
    let ib = "display: inline-block";
    let doc = cell_page(&[], vec![el("div", &[("id", "p"), ("style", ib)], vec![text("one")]), el("div", &[("id", "q"), ("style", ib)], vec![text("two")])]);
    let tree = lay(&doc, VW, 400.0);
    let (px, py, pw, _) = box_of(&tree, &doc.by_id("p"));
    let (qx, qy, _, _) = box_of(&tree, &doc.by_id("q"));
    assert!(pw < VW / 2.0, "the inline-block shrinks to its text ({pw})");
    assert!((py - qy).abs() < 0.5, "the two inline-blocks share one line (y {py} vs {qy})");
    assert!(qx >= px + pw - 0.5, "the second follows the first on the line (x {px} + {pw} vs {qx})");
}
