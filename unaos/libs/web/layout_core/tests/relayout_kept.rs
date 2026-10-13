//! AETHERPAINTPERF2 (SR132): a relayout that keeps the previous tree (`layout_core::relayout_kept`: a fresh box tree
//! `retarget`ed onto the kept boxes, the style rewrites run on the twin, the kept taffy tree synced and laid out)
//! lays out exactly what a fresh layout lays out — run once and run twice — for a fixture exercising each style
//! rewrite `remeasure` makes: `fix_display_contexts`, `propagate_text_align`, `collapse_inline_whitespace`,
//! `rescale_ua_margins`, the % width pass, the block widths, margin collapsing, the inline heights it sets and
//! restores, vertical writing, replaced sizes; a table and a multicol container (laid out whole). And a kept
//! relayout re-lays only what moved: nothing when nothing changed, the changed row and its ancestors when one
//! row's text changed.
mod common;
use common::*;

const VP: (f32, f32) = (800.0, 600.0);

fn key(n: &N) -> u64 {
    n.id as u64
}

/// Every box's border box and every inline formatting root's line boxes, by DOM node.
fn geometry(tree: &Tree, doc: &N) -> Vec<(usize, (f32, f32, f32, f32), Vec<(f32, f32, f32)>)> {
    let mut out = Vec::new();
    for i in 0..doc.doc.nodes.len() {
        let Some((tid, _)) = tree.node_map.iter().find(|(_, m)| m.id == i) else { continue };
        let n = N { doc: doc.doc.clone(), id: i };
        let lines = tree.inline.get(tid).map(|il| il.lines.clone()).unwrap_or_default();
        out.push((i, box_of(tree, &n), lines));
    }
    out
}

/// A fresh layout, a kept relayout of it, and a kept relayout of that: all three the same.
fn twice_equals_once(what: &str, doc: &N) -> Tree {
    let fresh = lay(doc, VP.0, VP.1);
    let want = geometry(&fresh, doc);
    let once = layout_core::relayout_kept(&D(doc.clone()), &NoSheet, &mut F, VP, fresh, &key);
    assert_eq!(geometry(&once, doc), want, "{what}: a kept relayout lays out what a fresh layout does");
    let twice = layout_core::relayout_kept(&D(doc.clone()), &NoSheet, &mut F, VP, once, &key);
    assert_eq!(geometry(&twice, doc), want, "{what}: a second kept relayout lays out the same again");
    twice
}

#[test]
fn display_contexts() {
    let doc = page(vec![el(
        "div",
        &[],
        vec![el("span", &[("style", "display: block")], vec![text("block span")]), text(" run "), el("i", &[], vec![text("inline")])],
    )]);
    twice_equals_once("fix_display_contexts", &doc);
}

#[test]
fn text_align() {
    let doc = page(vec![el("div", &[("style", "text-align: center; width: 300px")], vec![el("span", &[], vec![text("centred")])])]);
    twice_equals_once("propagate_text_align", &doc);
}

#[test]
fn inline_whitespace() {
    let doc = page(vec![el("p", &[], vec![el("b", &[], vec![text("a")]), text(" "), el("i", &[], vec![text("b")]), text("  c  ")])]);
    twice_equals_once("collapse_inline_whitespace", &doc);
}

#[test]
fn ua_margins() {
    let doc = page(vec![
        el("p", &[("style", "font-size: 32px")], vec![text("big paragraph")]),
        el("h1", &[], vec![text("heading")]),
        el("ul", &[], vec![el("li", &[], vec![text("item")])]),
    ]);
    twice_equals_once("rescale_ua_margins", &doc);
}

#[test]
fn percent_widths() {
    let doc = page(vec![el("div", &[("style", "width: 400px")], vec![el("div", &[("style", "width: calc(50% + 4px); height: 10px")], vec![])])]);
    twice_equals_once("the % width pass", &doc);
}

#[test]
fn block_widths_and_collapsed_margins() {
    let doc = page(vec![
        el("div", &[("style", "margin: 10px 20px; padding: 5px")], vec![text("padded")]),
        el("div", &[("style", "margin: 10px auto; max-width: 200px")], vec![text("centred")]),
        el("p", &[], vec![text("a")]),
        el("p", &[], vec![text("b")]),
        el("div", &[("style", "margin: 12px 0")], vec![el("p", &[], vec![text("c")])]),
    ]);
    twice_equals_once("block widths and margin collapsing", &doc);
}

#[test]
fn inline_heights() {
    let words = "word ".repeat(80);
    let doc = page(vec![el("div", &[("style", "width: 200px")], vec![el("p", &[], vec![text(&words)]), el("p", &[], vec![text("short")])])]);
    twice_equals_once("the inline heights", &doc);
}

#[test]
fn vertical_writing() {
    let doc = page(vec![el("div", &[("style", "writing-mode: vertical-rl; height: 100px")], vec![text("vertical text")])]);
    twice_equals_once("vertical writing", &doc);
}

#[test]
fn replaced_sizes() {
    let doc = page(vec![el("div", &[], vec![el("img", &[("width", "20"), ("height", "10")], vec![]), text(" after")])]);
    twice_equals_once("replaced sizes", &doc);
}

#[test]
fn table_and_multicol_lay_out_whole() {
    let doc = page(vec![
        el("table", &[], vec![el("tbody", &[], vec![el("tr", &[], vec![el("td", &[], vec![text("a")]), el("td", &[], vec![text("bb")])])])]),
        el("div", &[("style", "width: 410px; columns: 2; column-gap: 10px")], vec![el("div", &[("style", "height: 50px")], vec![]), el("div", &[("style", "height: 50px")], vec![])]),
    ]);
    twice_equals_once("a table and a multicol container", &doc);
}

fn rows(changed: Option<usize>) -> N {
    page(vec![el(
        "div",
        &[("id", "list")],
        (0..200)
            .map(|i| {
                let t = if Some(i) == changed { format!("Row {i}, changed") } else { format!("Row {i}") };
                el("div", &[("style", "height: 18px")], vec![text(&t)])
            })
            .collect(),
    )])
}

#[test]
fn unchanged_rows_relay_nothing() {
    let doc = rows(None);
    let tree = twice_equals_once("200 fixed-height rows", &doc);
    assert_eq!(tree.relaid, 0, "a kept relayout of an unchanged tree re-lays no box");
}

#[test]
fn one_row_text_relays_the_row() {
    let before = rows(None);
    let after = rows(Some(5));
    let fresh = lay(&before, VP.0, VP.1);
    let kept = layout_core::relayout_kept(&D(before.clone()), &NoSheet, &mut F, VP, fresh, &key);
    let tree = layout_core::relayout_kept(&D(after.clone()), &NoSheet, &mut F, VP, kept, &key);
    let want = lay(&after, VP.0, VP.1);
    assert_eq!(geometry(&tree, &after), geometry(&want, &after), "the kept relayout of the changed rows is the fresh layout");
    // The document node, html, body, #list, the row and its text.
    assert!(tree.relaid <= 8, "one row's text change re-lays the row and its ancestors only: {}", tree.relaid);
    assert!(want.relaid >= 400, "a fresh layout lays out every box: {}", want.relaid);
}
