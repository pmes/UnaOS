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

// ---- AETHERRTLMARGINAUTO (LEDGER SR781): an rtl column's over-constrained margins bind margin-right (named tail block) ----

/// AETHERRTLMARGINAUTO (SR781, AETHER-RTL-MARGIN-AUTO): CSS 2.2 §10.3.3 in rtl: a definite-width block with neither
/// margin auto is over-constrained and margin-left is the one ignored, so it sits margin-right from the right edge
/// (SR711's `place_narrow_blocks` END: taffy's End places it at free + margin-left). One auto margin takes the slack and
/// two centre, either direction (taffy's auto margins); the ltr twin keeps margin-left; a `direction` restyle round
/// trip re-derives the placement through SR686's `rule_align` undo.
#[test]
fn rtl_margins_over_constrained() {
    let col = |col_style: &str, kid_style: &str| {
        page(vec![el("div", &[("id", "col"), ("style", col_style)], vec![el("div", &[("id", "n"), ("style", kid_style)], vec![text("ab")])])])
    };
    let offset_in = |tree: &Tree, doc: &N| {
        let (cx, _, cw, _) = box_of(tree, &doc.by_id("col"));
        let (x, _, _, _) = box_of(tree, &doc.by_id("n"));
        assert_eq!(cw, 300.0, "the column is 300 wide");
        x - cx
    };
    let m = "width: 100px; margin-left: 10px; margin-right: 20px";
    let cases: [(&str, N, f32); 6] = [
        ("rtl 10 / 20", col("width: 300px; direction: rtl", m), 180.0),
        ("ltr 10 / 20", col("width: 300px", m), 10.0),
        ("rtl auto / 20", col("width: 300px; direction: rtl", "width: 100px; margin-left: auto; margin-right: 20px"), 180.0),
        ("rtl 0 auto", col("width: 300px; direction: rtl", "width: 100px; margin: 0 auto"), 100.0),
        ("rtl 400 wide, 10 / 20", col("width: 300px; direction: rtl", "width: 400px; margin-left: 10px; margin-right: 20px"), -120.0),
        ("rtl text-align: center, 10 / 20", col("width: 300px; direction: rtl; text-align: center", m), 180.0),
    ];
    for (how, doc, want) in &cases {
        let tree = lay(doc, VW, 400.0);
        assert_eq!(offset_in(&tree, doc), *want, "{how}: the block's offset in the 300px column");
    }

    let doc = col("width: 300px", m);
    let mut tree = lay(&doc, VW, 400.0);
    assert_eq!(offset_in(&tree, &doc), 10.0, "ltr: margin-left binds");
    let cx = layout_core::css::ParseCx::new(VW, 400.0);
    for (dir, want) in [("rtl", 180.0), ("ltr", 10.0)] {
        let mut spec = layout_core::css::SpecifiedStyle::<TH>::default();
        layout_core::css::apply_declaration("direction", dir, &mut spec, &cx);
        let specs = [(id_of(&tree, &doc.by_id("col")), std::rc::Rc::new(spec))];
        layout_core::apply_specs(&mut tree, &specs);
        layout_core::remeasure(&mut tree, &F);
        assert_eq!(offset_in(&tree, &doc), want, "direction: {dir} restyle: the block's offset");
    }
} // AETHERMARGINAUTOOVERFLOW (SR1065): SR781's fn closes here (SR782's test below was nested in it and never ran)

// ---- AETHERCENTERRESTYLEITEMS (LEDGER SR782): the rule's container align-items / justify-content undone (named tail block) ----

/// AETHERCENTERRESTYLEITEMS (SR782): the container side of SR686's restyle. `td align=center` (a column) takes the
/// rule's `align_items: CENTER`; a restyle of the SAME tree (`apply_specs` + the in-place `remeasure`) to `text-align:
/// left` puts it back to NORMAL, so the 200px #d sits at the cell's content start instead of staying centred by the
/// cell's cross axis. A plain `center` restyle re-derives it (CENTER, #d at the start, SR436) and a later `left` undoes
/// the new mark. An author `align-items: center` folded on a flex column (the fold lands it) and an author
/// `justify-content: center` on a row cell (the rule's justify-only mark) survive the undo.
#[test]
fn center_restyle_undoes_rule_align_items() {
    use layout_core::taffy::style::{AlignItems, JustifyContent};
    use std::rc::Rc;
    let cx = layout_core::css::ParseCx::new(VW, 400.0);
    let spec = |decls: &[(&str, &str)]| {
        let mut s = layout_core::css::SpecifiedStyle::<TH>::default();
        for (n, v) in decls {
            layout_core::css::apply_declaration(n, v, &mut s, &cx);
        }
        Rc::new(s)
    };
    let restyle = |tree: &mut Tree, doc: &N, id: &str, decls: &[(&str, &str)]| {
        let specs = [(id_of(tree, &doc.by_id(id)), spec(decls))];
        layout_core::apply_specs(tree, &specs);
        layout_core::remeasure(tree, &F);
    };
    let items = |tree: &Tree, doc: &N, id: &str| tree.taffy.style(id_of(tree, &doc.by_id(id))).unwrap().align_items;
    let justify = |tree: &Tree, doc: &N, id: &str| tree.taffy.style(id_of(tree, &doc.by_id(id))).unwrap().justify_content;
    let at_start = |tree: &Tree, doc: &N, how: &str| {
        let (cx0, _, _, _) = box_of(tree, &doc.by_id("cell"));
        let (dx, _, dw, _) = box_of(tree, &doc.by_id("d"));
        assert!(dw > 0.0 && (dx - (cx0 + 1.0)).abs() < 0.5, "{how}: #d sits at the cell's content start (x {dx}, cell {cx0})");
    };

    // (a) SR686's fixture: the rule's align_items on the cell is undone by a text-align: left restyle.
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
                ],
            )],
        )],
    )]);
    let mut tree = lay(&doc, VW, 400.0);
    assert_eq!(items(&tree, &doc, "cell"), AlignItems::CENTER, "td align=center: the rule centres the cell's column");
    restyle(&mut tree, &doc, "cell", &[("text-align", "left")]);
    assert_eq!(items(&tree, &doc, "cell"), AlignItems::NORMAL, "text-align: left: the rule's align_items is undone");
    at_start(&tree, &doc, "text-align: left");

    // (b) -webkit-center -> plain center re-derives (and re-marks); a later left undoes the new mark.
    let mut tree = lay(&doc, VW, 400.0);
    restyle(&mut tree, &doc, "cell", &[("text-align", "center")]);
    assert_eq!(items(&tree, &doc, "cell"), AlignItems::CENTER, "text-align: center: the rule's align_items is re-derived");
    at_start(&tree, &doc, "text-align: center");
    restyle(&mut tree, &doc, "cell", &[("text-align", "left")]);
    assert_eq!(items(&tree, &doc, "cell"), AlignItems::NORMAL, "center then left: the re-derived mark is undone");

    // (c) An author align-items: center that the fold lands (a flex column) survives the undo.
    let fx = page(vec![el(
        "div",
        &[("id", "fx"), ("align", "center"), ("style", "display: flex; flex-direction: column; width: 600px")],
        vec![el("div", &[("style", "width: 200px")], vec![text("ab")])],
    )]);
    let flex = [("display", "flex"), ("flex-direction", "column"), ("width", "600px")];
    let mut tree = lay(&fx, VW, 400.0);
    assert_eq!(items(&tree, &fx, "fx"), AlignItems::CENTER, "align=center on a flex column: the rule's align_items");
    let mut decls = flex.to_vec();
    decls.extend([("align-items", "center"), ("text-align", "left")]);
    restyle(&mut tree, &fx, "fx", &decls);
    assert_eq!(items(&tree, &fx, "fx"), AlignItems::CENTER, "an author align-items: center folded by the restyle is kept");
    let mut tree = lay(&fx, VW, 400.0);
    let mut decls = flex.to_vec();
    decls.push(("text-align", "left"));
    restyle(&mut tree, &fx, "fx", &decls);
    assert_eq!(items(&tree, &fx, "fx"), AlignItems::NORMAL, "control: with no author align-items the column is NORMAL");

    // (d) The justify-only mark: a 300px block holding inline content only is a row, and the -webkit-center it inherits
    // from `div align=center` makes the rule write its justify_content alone. A restyle of the ANCESTOR to text-align:
    // left (the row box gets no spec of its own) undoes it; an author justify-content: center folded on the row is kept.
    // (A text-align declaration on the row box itself folds its own justify, css.rs `"left"` -> START, and drops the mark.)
    let row = page(vec![el("div", &[("id", "wrap"), ("align", "center")], vec![el("div", &[("id", "row"), ("style", "width: 300px")], vec![text("inline only")])])]);
    let mut tree = lay(&row, VW, 400.0);
    assert_eq!(justify(&tree, &row, "row"), JustifyContent::CENTER, "inherited -webkit-center on a row box: the rule's justify_content");
    restyle(&mut tree, &row, "wrap", &[("text-align", "left")]);
    assert_eq!(justify(&tree, &row, "row"), JustifyContent::NORMAL, "the ancestor's text-align: left: the rule's justify_content is undone");
    let mut tree = lay(&row, VW, 400.0);
    let specs = [(id_of(&tree, &row.by_id("wrap")), spec(&[("text-align", "left")])), (id_of(&tree, &row.by_id("row")), spec(&[("justify-content", "center")]))];
    layout_core::apply_specs(&mut tree, &specs);
    layout_core::remeasure(&mut tree, &F);
    assert_eq!(justify(&tree, &row, "row"), JustifyContent::CENTER, "an author justify-content: center folded by the restyle is kept");
}

// ---- AETHERMARGINAUTOOVERFLOW (LEDGER SR1065): an over-constrained auto margin is 0, the box overflows to the end side (named tail block) ----

/// AETHERMARGINAUTOOVERFLOW (SR1065): CSS 2.2 §10.3.3: a definite width wider than its containing block treats its
/// auto margins as zero, so the box overflows to the end side (Chromium: a 600px `margin: 0 auto` box in a 400px
/// parent sits at x 0). Before SR1065 taffy's `resolve_cross_axis_auto_margins` handed the auto margins the negative
/// free space: x -100 (both auto), -200 (`margin-left: auto` alone), rtl -100. In rtl the box's right edge stays at
/// the container's (margin-left is the one ignored: place_narrow_blocks' END). A box that fits still centres; a
/// `max-width: 100%` twin is clamped and never over-constrained; a block image takes the same road.
#[test]
fn margin_auto_overflow_starts_at_the_start_edge() {
    let col = |col_style: &str, kid: Spec| page(vec![el("div", &[("id", "col"), ("style", col_style)], vec![kid])]);
    let div = |kid_style: &str| el("div", &[("id", "n"), ("style", kid_style)], vec![text("ab")]);
    let placed = |doc: &N| {
        let tree = lay(doc, VW, 400.0);
        let (cx, _, cw, _) = box_of(&tree, &doc.by_id("col"));
        let (x, _, w, _) = box_of(&tree, &doc.by_id("n"));
        assert_eq!(cw, 400.0, "the column is 400 wide");
        (x - cx, w)
    };
    let ltr = "width: 400px";
    let rtl = "width: 400px; direction: rtl";
    let cases: [(&str, N, (f32, f32)); 7] = [
        ("ltr 600 0 auto (was -100)", col(ltr, div("width: 600px; margin: 0 auto")), (0.0, 600.0)),
        ("ltr 600 margin-left auto (was -200)", col(ltr, div("width: 600px; margin-left: auto; margin-right: 0")), (0.0, 600.0)),
        ("ltr 200 0 auto still centres", col(ltr, div("width: 200px; margin: 0 auto")), (100.0, 200.0)),
        ("rtl 600 0 auto overflows left (was -100)", col(rtl, div("width: 600px; margin: 0 auto")), (-200.0, 600.0)),
        ("rtl 600 margin-right auto", col(rtl, div("width: 600px; margin-left: 0; margin-right: auto")), (-200.0, 600.0)),
        ("ltr max-width 100% twin", col(ltr, div("width: 600px; max-width: 100%; margin: 0 auto")), (0.0, 400.0)),
        (
            "ltr block img 600 0 auto",
            col(ltr, el("img", &[("id", "n"), ("style", "display: block; width: 600px; height: 10px; margin: 0 auto")], vec![])),
            (0.0, 600.0),
        ),
    ];
    for (how, doc, want) in &cases {
        assert_eq!(placed(doc), *want, "{how}: (offset in the column, width)");
    }
}

// ---- AETHERTABLEALIGN (LEDGER SR1071): a table's align= is auto margins / float, never text-align (named tail block) ----

const TW: f32 = 400.0;

/// AETHERTABLEALIGN (SR1071): a one-cell table with the given attributes (`#t`, cell `#c` holding "cell") in a 400px body.
fn align_table(attrs: &[(&'static str, &str)]) -> Spec {
    let mut a = vec![("id", "t")];
    a.extend_from_slice(attrs);
    el("table", &a, vec![el("tr", &[], vec![el("td", &[("id", "c")], vec![text("cell")])])])
}

fn table_paint(tree: &Tree, doc: &N, id: &str) -> layout_core::PaintStyle<TH> {
    tree.paint_map.get(&id_of(tree, &doc.by_id(id))).cloned().unwrap_or_default()
}

/// AETHERTABLEALIGN (SR1071): HTML rendering §15.3.11: `<table align=center>` is the two auto margins, so the BOX centres
/// (an author-width table and a shrink-to-fit one; before SR1071 both sat at x 0) and the cells' text is NOT centred (the
/// table carried text-align 5 and the cell's line sat in the middle of its 200px cell).
#[test]
fn table_align_center_centres_the_box_not_the_text() {
    let doc = page(vec![align_table(&[("align", "center"), ("style", "width: 200px; border-spacing: 0")])]);
    let tree = lay(&doc, TW, 400.0);
    let (x, _, w, _) = box_of(&tree, &doc.by_id("t"));
    assert_eq!((x, w), (100.0, 200.0), "align=center: the 200px table box is centred in the 400px body");
    assert!(matches!(table_paint(&tree, &doc, "t").text_align, None | Some(0)), "align=center sets no text-align on a table (the fixture is quirks mode: the table quirk's 0)");
    let il = &tree.inline[&id_of(&tree, &doc.by_id("c"))];
    let (tx, tw) = il
        .frags
        .iter()
        .find_map(|f| match f {
            layout_core::inline::Frag::Text { x, width, .. } => Some((*x, *width)),
            _ => None,
        })
        .expect("the cell's text line");
    assert!(tw > 0.0 && tw < 150.0, "the cell's text is narrower than the cell ({tw})");
    assert!(tx < 2.0, "the cell's text starts at the cell's content start, not centred (x {tx})");

    let doc = page(vec![align_table(&[("align", "center")])]);
    let tree = lay(&doc, TW, 400.0);
    let (x, _, w, _) = box_of(&tree, &doc.by_id("t"));
    assert!(w > 0.0 && w < TW - 100.0, "a shrink-to-fit table is narrower than the body ({w})");
    assert!((x + w / 2.0 - TW / 2.0).abs() < 0.5, "align=center: the shrink-to-fit table is centred (x {x} w {w})");

    let doc = page(vec![align_table(&[("style", "margin: 0 auto")])]);
    let tree = lay(&doc, TW, 400.0);
    let (x, _, w, _) = box_of(&tree, &doc.by_id("t"));
    assert!(w > 0.0 && w < TW - 100.0 && (x + w / 2.0 - TW / 2.0).abs() < 0.5, "an author margin: 0 auto shrink-to-fit table centres (x {x} w {w})");
}

/// AETHERTABLEALIGN (SR1071): align=right / left are `float: right` / `float: left` through the float arm's own road: the
/// box sits at the edge and a following block is placed exactly as under the author float (no wrap among block siblings:
/// the float road's IFC-only limit). align=middle is nothing for a table.
#[test]
fn table_align_left_right_float() {
    let p = || el("p", &[("id", "p")], vec![text("after")]);
    let hint = page(vec![align_table(&[("align", "right"), ("style", "width: 200px; border-spacing: 0")]), p()]);
    let author = page(vec![align_table(&[("style", "float: right; width: 200px; border-spacing: 0")]), p()]);
    let (th, ta) = (lay(&hint, TW, 400.0), lay(&author, TW, 400.0));
    let (x, _, w, _) = box_of(&th, &hint.by_id("t"));
    assert_eq!((x, w), (200.0, 200.0), "align=right: the table box sits at the right edge");
    assert_eq!(table_paint(&th, &hint, "t").float, Some(2), "align=right is float: right");
    assert!(matches!(table_paint(&th, &hint, "t").text_align, None | Some(0)), "align=right sets no text-align on a table (the fixture is quirks mode: the table quirk's 0)");
    assert_eq!(box_of(&th, &hint.by_id("t")), box_of(&ta, &author.by_id("t")), "align=right: the table as under float: right");
    assert_eq!(box_of(&th, &hint.by_id("p")), box_of(&ta, &author.by_id("p")), "align=right: the following p as under float: right");

    let doc = page(vec![align_table(&[("align", "left"), ("style", "width: 200px; border-spacing: 0")]), p()]);
    let tree = lay(&doc, TW, 400.0);
    assert_eq!(box_of(&tree, &doc.by_id("t")).0, 0.0, "align=left: the table box sits at the left edge");
    assert_eq!(table_paint(&tree, &doc, "t").float, Some(1), "align=left is float: left");
    assert!(matches!(table_paint(&tree, &doc, "t").text_align, None | Some(0)), "align=left sets no text-align on a table (the fixture is quirks mode: the table quirk's 0)");

    let doc = page(vec![align_table(&[("align", "middle"), ("style", "width: 200px; border-spacing: 0")])]);
    let tree = lay(&doc, TW, 400.0);
    assert_eq!(box_of(&tree, &doc.by_id("t")).0, 0.0, "align=middle: nothing for a table");
    assert!(matches!(table_paint(&tree, &doc, "t").text_align, None | Some(0)), "align=middle sets no text-align on a table (the fixture is quirks mode: the table quirk's 0)");
    assert_eq!(table_paint(&tree, &doc, "t").float, None, "align=middle is no float");
}

/// AETHERTABLEALIGN (SR1071): the hint is the lowest-priority source: an author `margin-left: 0` beats it (x 0); the
/// `<div align=center>` road (text-align -webkit-center) and the caption's align= (caption-side, UA -webkit-center)
/// are unchanged; an rtl `<table align=center>` is still centred (taffy's auto margins outrank SR711's align-self END).
#[test]
fn table_align_css_wins_div_caption_rtl() {
    let doc = page(vec![align_table(&[("align", "center"), ("style", "margin-left: 0; width: 200px; border-spacing: 0")])]);
    let tree = lay(&doc, TW, 400.0);
    assert_eq!(box_of(&tree, &doc.by_id("t")).0, 0.0, "style=\"margin-left: 0\" beats align=center");

    let doc = page(vec![el("div", &[("id", "d"), ("align", "center")], vec![text("ab")])]);
    let tree = lay(&doc, TW, 400.0);
    assert_eq!(table_paint(&tree, &doc, "d").text_align, Some(5), "<div align=center> keeps -webkit-center");

    let doc = page(vec![el(
        "table",
        &[("id", "t")],
        vec![el("caption", &[("id", "cap"), ("align", "bottom")], vec![text("cap")]), el("tr", &[], vec![el("td", &[], vec![text("cell")])])],
    )]);
    let tree = lay(&doc, TW, 400.0);
    let cap = table_paint(&tree, &doc, "cap");
    assert_eq!((cap.text_align, cap.caption_side), (Some(5), Some(true)), "<caption align=bottom>: caption-side bottom, the UA -webkit-center");

    let doc = page(vec![el("div", &[("dir", "rtl")], vec![align_table(&[("align", "center"), ("style", "width: 200px; border-spacing: 0")])])]);
    let tree = lay(&doc, TW, 400.0);
    assert_eq!(box_of(&tree, &doc.by_id("t")).0, 100.0, "rtl: align=center still centres the table box");
}

