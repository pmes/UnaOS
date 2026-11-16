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

// ---- AETHERCAPTIONCSSOM (LEDGER SR686): the centring rule's align-self is re-derived on a restyle (named tail block) ----

/// AETHERCAPTIONCSSOM (SR686): Chromium re-derives -webkit-center's placement on every style recalc. A `td
/// align=center` centres a nested table #t and 200px blocks #d / #a through `place_narrow_blocks`' align-self; a later
/// restyle of the SAME tree (`apply_specs` + the in-place `remeasure`, anim's re-fold) to `text-align: left` undoes
/// the rule's value: #d's align-self is none again, and #t's is not centre (table.rs's start) and the table sits at
/// the cell's start. An author align-self folded by the same restyle (#a, even the value the rule had written) is kept.
#[test]
fn center_restyle_clears_rule_align_self() {
    use layout_core::taffy::style::AlignSelf;
    let doc = page(vec![el(
        "table",
        &[("style", "width: 600px; border-spacing: 0")],
        vec![el(
            "tr",
            &[],
            vec![el(
                "td",
                &[("id", "cell"), ("align", "center")],
                vec![
                    el("table", &[("id", "t")], vec![el("tr", &[], vec![el("td", &[], vec![text("cell")])])]),
                    el("div", &[("id", "d"), ("style", "width: 200px")], vec![text("ab")]),
                    el("div", &[("id", "a"), ("style", "width: 200px")], vec![text("cd")]),
                ],
            )],
        )],
    )]);
    let mut tree = lay(&doc, VW, 400.0);
    let align = |tree: &Tree, id: &str| tree.taffy.style(id_of(tree, &doc.by_id(id))).unwrap().align_self;
    for id in ["t", "d", "a"] {
        assert_eq!(align(&tree, id), Some(AlignSelf::CENTER), "td align=center: the rule centres #{id}");
    }

    let cx = layout_core::css::ParseCx::new(VW, 400.0);
    let mut left = layout_core::css::SpecifiedStyle::<TH>::default();
    layout_core::css::apply_declaration("text-align", "left", &mut left, &cx);
    let mut author = layout_core::css::SpecifiedStyle::<TH>::default();
    author.align_self = Some(AlignSelf::CENTER);
    let specs = [
        (id_of(&tree, &doc.by_id("cell")), std::rc::Rc::new(left)),
        (id_of(&tree, &doc.by_id("a")), std::rc::Rc::new(author)),
    ];
    layout_core::apply_specs(&mut tree, &specs);
    layout_core::remeasure(&mut tree, &F);

    assert_eq!(align(&tree, "d"), None, "text-align: left: the rule's align-self on #d is undone");
    assert_ne!(align(&tree, "t"), Some(AlignSelf::CENTER), "text-align: left: the nested table is no longer centred");
    let (cx0, _, _, _) = box_of(&tree, &doc.by_id("cell"));
    let (tx, _, tw, _) = box_of(&tree, &doc.by_id("t"));
    assert!(tw > 0.0 && (tx - (cx0 + 1.0)).abs() < 0.5, "the nested table sits at the cell's start (x {tx}, cell {cx0})");
    assert_eq!(align(&tree, "a"), Some(AlignSelf::CENTER), "an author align-self folded by the restyle is kept");
}

// ---- AETHERIMAGEINPUTFACE (LEDGER SR701): a narrow block stays at an rtl column's inline start (named tail block) ----

/// AETHERIMAGEINPUTFACE (SR701, AETHER-CENTER-RTL): plain center / right / end align inline content only (CSS 2.2 §16.2),
/// so a narrow block stays at the column's inline start, which in an rtl containing block is the RIGHT edge (§10.3.3:
/// margin-left is ignored). -webkit-right stays the physical right and -webkit-center the middle in either direction;
/// the column's direction decides, never the child's own `dir`; the ltr column keeps its block at the left.
#[test]
fn rtl_column_starts_narrow_blocks_at_the_right() {
    let col = |col_attrs: &[(&'static str, &str)], kid_attrs: &[(&'static str, &str)]| {
        let mut ca: Vec<(&'static str, &str)> = vec![("id", "col")];
        ca.extend_from_slice(col_attrs);
        let mut ka: Vec<(&'static str, &str)> = vec![("id", "n"), ("style", "width: 100px")];
        ka.extend_from_slice(kid_attrs);
        page(vec![el("div", &ca, vec![el("div", &ka, vec![text("ab")])])])
    };
    let offset = |doc: &N| {
        let tree = lay(doc, VW, 400.0);
        let (cx, _, cw, _) = box_of(&tree, &doc.by_id("col"));
        let (x, _, w, _) = box_of(&tree, &doc.by_id("n"));
        assert_eq!((cw, w), (300.0, 100.0), "the column is 300 wide and the block 100");
        x - cx
    };
    let cases: [(&str, N, f32); 7] = [
        ("dir=rtl text-align: center", col(&[("dir", "rtl"), ("style", "width: 300px; text-align: center")], &[]), 200.0),
        ("direction: rtl text-align: center", col(&[("style", "width: 300px; direction: rtl; text-align: center")], &[]), 200.0),
        ("dir=rtl text-align: right", col(&[("dir", "rtl"), ("style", "width: 300px; text-align: right")], &[]), 200.0),
        ("a dir=ltr child in a dir=rtl column", col(&[("dir", "rtl"), ("style", "width: 300px; text-align: center")], &[("dir", "ltr")]), 200.0),
        ("dir=rtl -webkit-right", col(&[("dir", "rtl"), ("style", "width: 300px; text-align: -webkit-right")], &[]), 200.0),
        ("dir=rtl -webkit-center", col(&[("dir", "rtl"), ("style", "width: 300px; text-align: -webkit-center")], &[]), 100.0),
        ("ltr text-align: center", col(&[("style", "width: 300px; text-align: center")], &[]), 0.0),
    ];
    for (how, doc, want) in &cases {
        assert_eq!(offset(doc), *want, "{how}: the 100px block's offset in the 300px column");
    }
    // Inherited through an auto-width block: the text-align sits on the inner column, the dir on its parent.
    let doc = page(vec![el(
        "div",
        &[("dir", "rtl"), ("style", "width: 300px")],
        vec![el("div", &[("id", "col"), ("style", "text-align: center")], vec![el("div", &[("id", "n"), ("style", "width: 100px")], vec![text("ab")])])],
    )]);
    assert_eq!(offset(&doc), 200.0, "dir=rtl inherited by the centred column");
}

// ---- AETHERRTLBLOCKSTART (LEDGER SR711): an rtl column with no / start text-align starts its narrow blocks at the right (named tail block) ----

/// AETHERRTLBLOCKSTART (SR711, AETHER-RTL-BLOCK-START): with no text-align, or `text-align: start`, an rtl column puts a
/// definite-width block at its inline start, the RIGHT edge (CSS 2.2 §10.3.3). `text-align: left` (4) does too since SR724;
/// `align=left` (7, -webkit-left) stays at the physical left; the column's direction decides, never the child's;
/// a restyle to `direction: ltr` undoes the rule's align-self through SR686's `rule_align` (the block is back at 0).
#[test]
fn rtl_column_starts_narrow_blocks_with_no_align() {
    use layout_core::taffy::style::AlignSelf;
    let col = |col_attrs: &[(&'static str, &str)], kid_attrs: &[(&'static str, &str)]| {
        let mut ca: Vec<(&'static str, &str)> = vec![("id", "col")];
        ca.extend_from_slice(col_attrs);
        let mut ka: Vec<(&'static str, &str)> = vec![("id", "n"), ("style", "width: 100px")];
        ka.extend_from_slice(kid_attrs);
        page(vec![el("div", &ca, vec![el("div", &ka, vec![text("ab")])])])
    };
    let offset_in = |tree: &Tree, doc: &N| {
        let (cx, _, cw, _) = box_of(tree, &doc.by_id("col"));
        let (x, _, w, _) = box_of(tree, &doc.by_id("n"));
        assert_eq!((cw, w), (300.0, 100.0), "the column is 300 wide and the block 100");
        x - cx
    };
    let cases: [(&str, N, f32); 7] = [
        ("dir=rtl, no text-align", col(&[("dir", "rtl"), ("style", "width: 300px")], &[]), 200.0),
        ("direction: rtl, no text-align", col(&[("style", "width: 300px; direction: rtl")], &[]), 200.0),
        ("dir=rtl text-align: start", col(&[("dir", "rtl"), ("style", "width: 300px; text-align: start")], &[]), 200.0),
        ("dir=rtl text-align: left (inline start, SR724)", col(&[("dir", "rtl"), ("style", "width: 300px; text-align: left")], &[]), 200.0),
        ("dir=rtl align=left", col(&[("dir", "rtl"), ("align", "left"), ("style", "width: 300px")], &[]), 0.0),
        ("a dir=rtl child in an ltr column", col(&[("style", "width: 300px")], &[("dir", "rtl")]), 0.0),
        ("ltr, no text-align", col(&[("style", "width: 300px")], &[]), 0.0),
    ];
    for (how, doc, want) in &cases {
        let tree = lay(doc, VW, 400.0);
        assert_eq!(offset_in(&tree, doc), *want, "{how}: the 100px block's offset in the 300px column");
    }

    let doc = col(&[("dir", "rtl"), ("style", "width: 300px")], &[]);
    let mut tree = lay(&doc, VW, 400.0);
    let align = |tree: &Tree| tree.taffy.style(id_of(tree, &doc.by_id("n"))).unwrap().align_self;
    assert_eq!(align(&tree), Some(AlignSelf::END), "dir=rtl: the rule ends the block");
    let cx = layout_core::css::ParseCx::new(VW, 400.0);
    let mut ltr = layout_core::css::SpecifiedStyle::<TH>::default();
    layout_core::css::apply_declaration("direction", "ltr", &mut ltr, &cx);
    let specs = [(id_of(&tree, &doc.by_id("col")), std::rc::Rc::new(ltr))];
    layout_core::apply_specs(&mut tree, &specs);
    layout_core::remeasure(&mut tree, &F);
    assert_eq!(align(&tree), None, "direction: ltr restyle: the rule's align-self is undone");
    assert_eq!(offset_in(&tree, &doc), 0.0, "direction: ltr restyle: the block is back at the left");
}

// ---- AETHERTEXTALIGNCODES (LEDGER SR724): CSS `left` (4) and the physical `-webkit-left` / `align=left` (7) split (named tail block) ----

/// AETHERTEXTALIGNCODES (SR724, AETHER-RTL-BLOCK-LEFT, AETHER-RTL-WEBKIT-LEFT): CSS `text-align: left` aligns inline
/// content only (CSS 2.2 §16.2), so in an rtl column a definite-width block sits at the inline start, the RIGHT edge;
/// `align=left` and `-webkit-left` / `-moz-left` are Blink's physical left (code 7) and keep it at 0. In ltr all three
/// leave it at 0; an auto-width block still fills (code 7 never writes the column's align_items); and the inline
/// formatter reads 7 as left, like 4, where an rtl paragraph with no text-align starts its line at the right.
#[test]
fn left_and_webkit_left_split() {
    let col = |col_attrs: &[(&'static str, &str)], kid_style: &str| {
        let mut ca: Vec<(&'static str, &str)> = vec![("id", "col")];
        ca.extend_from_slice(col_attrs);
        page(vec![el("div", &ca, vec![el("div", &[("id", "n"), ("style", kid_style)], vec![text("ab")])])])
    };
    let place = |doc: &N| {
        let tree = lay(doc, VW, 400.0);
        let (cx, _, cw, _) = box_of(&tree, &doc.by_id("col"));
        let (x, _, w, _) = box_of(&tree, &doc.by_id("n"));
        assert_eq!(cw, 300.0, "the column is 300 wide");
        (x - cx, w)
    };
    let narrow = "width: 100px";
    let cases: [(&str, N, f32); 10] = [
        ("dir=rtl text-align: left", col(&[("dir", "rtl"), ("style", "width: 300px; text-align: left")], narrow), 200.0),
        ("dir=rtl align=left", col(&[("dir", "rtl"), ("align", "left"), ("style", "width: 300px")], narrow), 0.0),
        ("dir=rtl -webkit-left", col(&[("dir", "rtl"), ("style", "width: 300px; text-align: -webkit-left")], narrow), 0.0),
        ("dir=rtl -moz-left", col(&[("dir", "rtl"), ("style", "width: 300px; text-align: -moz-left")], narrow), 0.0),
        ("direction: rtl -webkit-left", col(&[("style", "width: 300px; direction: rtl; text-align: -webkit-left")], narrow), 0.0),
        ("ltr text-align: left", col(&[("style", "width: 300px; text-align: left")], narrow), 0.0),
        ("ltr align=left", col(&[("align", "left"), ("style", "width: 300px")], narrow), 0.0),
        ("ltr -webkit-left", col(&[("style", "width: 300px; text-align: -webkit-left")], narrow), 0.0),
        ("dir=rtl align=left, auto width", col(&[("dir", "rtl"), ("align", "left"), ("style", "width: 300px")], ""), 0.0),
        ("dir=rtl -webkit-left, auto width", col(&[("dir", "rtl"), ("style", "width: 300px; text-align: -webkit-left")], ""), 0.0),
    ];
    for (how, doc, want) in &cases {
        let (x, w) = place(doc);
        assert_eq!(x, *want, "{how}: the block's offset in the 300px column");
        let auto = how.ends_with("auto width");
        assert_eq!(w, if auto { 300.0 } else { 100.0 }, "{how}: the block's width");
    }

    // The inline formatter: text directly in a 300px rtl column.
    let line_x = |style: &str, attrs: &[(&'static str, &str)]| {
        let mut a: Vec<(&'static str, &str)> = vec![("id", "p"), ("dir", "rtl"), ("style", style)];
        a.extend_from_slice(attrs);
        let doc = page(vec![el("div", &a, vec![text("ab")])]);
        let tree = lay(&doc, VW, 400.0);
        tree.inline[&id_of(&tree, &doc.by_id("p"))]
            .frags
            .iter()
            .find_map(|f| match f {
                layout_core::inline::Frag::Text { x, .. } => Some(*x),
                _ => None,
            })
            .expect("the column's text line")
    };
    assert!(line_x("width: 300px", &[]) > 100.0, "control: an rtl line with no text-align starts at the right");
    assert_eq!(line_x("width: 300px; text-align: -webkit-left", &[]), 0.0, "-webkit-left: the line is at the left");
    assert_eq!(line_x("width: 300px; text-align: left", &[]), 0.0, "left: the line is at the left");
    assert_eq!(line_x("width: 300px", &[("align", "left")]), 0.0, "align=left: the line is at the left");
}
