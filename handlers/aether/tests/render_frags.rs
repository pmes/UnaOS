//! AETHERRENDER3 (SR126): the paint of what AETHERSTYLE3C (SR122) laid out — one pixel test per item over the
//! headless frame harness (AetherEngine at 400x300, `load_html`, `render_frame`, the BGRA surface; tests/a11y.rs's
//! shape). Written, not run (R107): the seat runs `cargo test -p aether --test render_frags` at the fold.
//!
//! 1. a decorated box split across two columns paints once per `col_frags` fragment (slice and clone);
//! 2. a `text-combine-upright: all` run is compressed into its 1em cell;
//! 3. the caret of a vertical editing host is `layout_core::hit::caret_rect`'s horizontal bar;
//! 4. the `filter` raster path APPS/AETHER.ELF's painter now calls (`layout_core::effects::filter_rgba_refs` over a
//!    layer built from a 0xRRGGBB canvas exactly as user-aether paint.rs `filter_group` builds it) — the ring-3
//!    crate has no test harness, so the shared call is witnessed here.

use std::sync::OnceLock;

use aether::api::events::Event;
use aether::dom::NodeRef;
use aether::AetherEngine;

const URL: &str = "https://render.test/page.html";

fn init() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("aether-render-frags-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        aether::storage::set_root_dir(Some(dir));
    });
}

fn page(html: &str) -> AetherEngine {
    init();
    let mut e = AetherEngine::new();
    e.handle_event(Event::Resize(400, 300));
    e.load_html(URL, html, true);
    frame(&mut e);
    e
}

fn frame(e: &mut AetherEngine) {
    e.damage_rects.push((0, 0, e.width, e.height));
    e.render_frame();
}

fn dom(e: &AetherEngine, id: &str) -> NodeRef {
    e.document
        .as_ref()
        .unwrap()
        .document_node()
        .descendants()
        .find(|n| n.as_element().is_some_and(|el| el.attributes.borrow().get("id") == Some(id)))
        .unwrap_or_else(|| panic!("no #{id}"))
}

/// The surface pixel at (`x`, `y`) as RGB.
fn pixel(e: &AetherEngine, x: u32, y: u32) -> (u8, u8, u8) {
    let i = ((y * e.width + x) * 4) as usize;
    (e.surface[i + 2], e.surface[i + 1], e.surface[i])
}

/// The taffy box of the element `id`.
fn box_id(e: &AetherEngine, id: &str) -> taffy::prelude::NodeId {
    let n = dom(e, id);
    let layout = e.layout_tree.as_ref().unwrap();
    *layout.node_map.iter().find(|(_, m)| **m == n).map(|(k, _)| k).unwrap_or_else(|| panic!("#{id} has no box"))
}

const BLACK: (u8, u8, u8) = (0, 0, 0);
const RED: (u8, u8, u8) = (255, 0, 0);
const WHITE: (u8, u8, u8) = (255, 255, 255);

fn multicol(mode: &str) -> String {
    let blocks = r#"<div style="height:40px"></div>"#.repeat(4);
    format!(
        r#"<body style="margin:0"><div style="width:410px;columns:2;column-gap:10px"><div id="d" style="border:5px solid rgb(0,0,0);padding:5px 0;background:rgb(255,0,0);box-decoration-break:{mode}">{blocks}</div></div></body>"#
    )
}

/// (1) The SR122 layout fixture (layout_core tests/multicol_fragment.rs): a 180px decorated box in a two-column
/// 410px container. `slice`: column height 90, fragments 0..90 (shift 0) and 90..180 (shift (210, -90)) — the box's
/// top border only on the first, its bottom border only on the second, both moved pieces filled red, nothing below
/// the columns. `clone`: column height 100, fragments (0, 100) and (80, 100) shifted (210, -80): two 100px boxes,
/// each with all four borders. Before SR126 the box painted once, 180px tall in the first column.
#[test]
fn box_decoration_break_paints_each_column_fragment() {
    let e = page(&multicol("slice"));
    let frags = e.layout_tree.as_ref().unwrap().col_frags.get(&box_id(&e, "d")).cloned().expect("the box fragments");
    assert_eq!(frags.len(), 2, "{frags:?}");
    assert_eq!(pixel(&e, 100, 2), BLACK, "slice: the first piece carries the block-start border");
    assert_eq!(pixel(&e, 2, 50), BLACK, "slice: the inline-start border on the first piece");
    assert_eq!(pixel(&e, 100, 50), RED);
    assert_eq!(pixel(&e, 100, 88), RED, "slice: no block-end border at the first piece's foot");
    assert_eq!(pixel(&e, 310, 2), RED, "slice: no block-start border on the second piece");
    assert_eq!(pixel(&e, 310, 50), RED, "slice: the second piece is painted in the second column");
    assert_eq!(pixel(&e, 407, 50), BLACK, "slice: the inline-end border on the second piece");
    assert_eq!(pixel(&e, 310, 87), BLACK, "slice: the block-end border on the last piece");
    assert_eq!(pixel(&e, 100, 120), WHITE, "slice: nothing below the 90px columns (the unfragmented box reached 180)");
    assert_eq!(pixel(&e, 205, 50), WHITE, "the column gap");

    let e = page(&multicol("clone"));
    assert_eq!(pixel(&e, 100, 2), BLACK, "clone: the first box's top border");
    assert_eq!(pixel(&e, 100, 50), RED);
    assert_eq!(pixel(&e, 100, 97), BLACK, "clone: the first box gets its own bottom border");
    assert_eq!(pixel(&e, 310, 2), BLACK, "clone: the second box gets its own top border");
    assert_eq!(pixel(&e, 310, 50), RED);
    assert_eq!(pixel(&e, 310, 97), BLACK, "clone: the second box's bottom border");
    assert_eq!(pixel(&e, 100, 150), WHITE, "clone: nothing below the 100px columns");
}

/// (2) `1234` combined upright at 20px in a one-line vertical-rl box: its cell is 20px of inline length with a scale
/// below 1 (the run's natural width is wider). Every inked pixel lies inside the cell's physical square (x 0..20,
/// y 0..20) and the compressed glyphs span it; before SR126 the run drew at its natural width and the pixels past
/// the cell went through the turned mapping, below the cell.
#[test]
fn text_combine_upright_compresses_into_its_cell() {
    let e = page(
        r#"<body style="margin:0"><div id="v" style="writing-mode:vertical-rl;font-size:20px;line-height:20px;width:20px;height:200px;color:rgb(0,0,0)"><span style="text-combine-upright:all">1234</span></div></body>"#,
    );
    let il = e.layout_tree.as_ref().unwrap().inline.get(&box_id(&e, "v")).cloned().expect("the vertical IFC");
    assert_eq!(il.combined.len(), 1, "{:?}", il.combined);
    let c = il.combined[0];
    assert_eq!((c.cell.x, c.cell.w), (0.0, 20.0), "{c:?}");
    assert!(c.scale < 1.0, "the natural run is wider than 1em: {c:?}");
    let (mut n, mut xmin, mut xmax, mut ymax) = (0, u32::MAX, 0, 0);
    for y in 0..200 {
        for x in 0..60 {
            if pixel(&e, x, y) != WHITE {
                n += 1;
                xmin = xmin.min(x);
                xmax = xmax.max(x);
                ymax = ymax.max(y);
            }
        }
    }
    assert!(n > 0, "the combined run paints");
    assert!(xmax < 20 && ymax < 21, "all ink inside the 1em cell: x {xmin}..={xmax}, y ..={ymax}");
    assert!(xmin <= 5 && xmax >= 14, "compressed across the cell, not cut: x {xmin}..={xmax}");
}

/// (3) A collapsed selection after `a` in a vertical-rl `contenteditable`: the painter draws
/// `layout_core::hit::caret_rect`'s bar — 1px tall, across the 20px column — in the text colour; without the
/// selection that row is not a solid bar.
#[test]
fn vertical_editing_host_caret_is_a_horizontal_bar() {
    let mut e = page(
        r#"<body style="margin:0"><div id="ed" contenteditable style="writing-mode:vertical-rl;font-size:20px;line-height:20px;width:20px;height:200px;color:rgb(0,0,255)">abc</div></body>"#,
    );
    let text = dom(&e, "ed").first_child().expect("the text node");
    let point = layout_core::hit::Point { node: text.id().0, offset: 1 };
    aether::selection::set(Some(aether::selection::Selection::collapsed(point)));
    frame(&mut e);
    let boxes = aether::render::text_boxes();
    let tb = boxes.iter().find(|b| b.ifc == box_id(&e, "ed")).expect("the painted text box").clone();
    let layout = e.layout_tree.as_ref().unwrap();
    let il = layout.inline.get(&tb.ifc).unwrap();
    let frag_text = match &il.frags[tb.frag] {
        aether::layout::inline::Frag::Text { text, .. } => text.clone(),
        _ => panic!("not a text fragment"),
    };
    let lb = layout_core::hit::TextBox { ifc: tb.ifc, frag: tb.frag, node: tb.node, x: tb.x, w: tb.w, top: tb.top, bottom: tb.bottom };
    let (cx, cy, cw, ch) = layout_core::hit::caret_rect(il, &lb, aether::selection::caret_x(&tb.adv, &frag_text, 1));
    assert_eq!(ch, 1.0, "a horizontal bar");
    assert!(cw >= 19.0, "across the column: {cw}");
    let (x0, y0) = (cx.floor() as u32, cy.floor() as u32);
    let row: Vec<_> = (x0..x0 + cw.round() as u32).map(|x| pixel(&e, x, y0)).collect();
    assert!(row.iter().all(|&p| p == (0, 0, 255)), "the caret bar at y {y0}: {row:?}");
    aether::selection::set(None);
    frame(&mut e);
    let row: Vec<_> = (x0..x0 + cw.round() as u32).map(|x| pixel(&e, x, y0)).collect();
    assert!(row.iter().any(|&p| p != (0, 0, 255)), "no bar without the caret: {row:?}");
}

/// (4) The ring-3 painter's filter group, step for step (user-aether paint.rs `filter_group`): a 0xRRGGBB canvas,
/// a red square painted over its white snapshot, the changed pixels as an opaque straight-alpha layer, run through
/// `layout_core::effects::filter_rgba_refs` — a filter function (`grayscale(1)`) and a `url(#g)` to a page
/// `<filter>` (`feColorMatrix saturate 0`, the markup `resolve_filter` hands over from the arena's `attrs`) — and
/// composited source-over: the square comes out grey, the backdrop untouched.
#[test]
fn ring3_filter_raster_path_greys_the_group() {
    for value in ["grayscale(1)", "url(#g)"] {
        let (w, h) = (20usize, 20usize);
        let snap = vec![0x00FF_FFFFu32; w * h];
        let mut px = snap.clone();
        for y in 5..15 {
            for x in 5..15 {
                px[y * w + x] = 0x00FF_0000;
            }
        }
        let mut layer = vec![0u8; w * h * 4];
        for i in 0..w * h {
            if px[i] != snap[i] {
                let c = px[i];
                layer[i * 4..i * 4 + 4].copy_from_slice(&[(c >> 16) as u8, (c >> 8) as u8, c as u8, 255]);
            }
        }
        let fcx = layout_core::effects::FilterCx { scale: 1.0, font_size: 16.0, viewport: (400.0, 300.0), color: (0, 0, 0) };
        let resolve = |id: &str| (id == "g").then(|| r#"<filter id="g"><feColorMatrix type="saturate" values="0"/></filter>"#.to_string());
        assert!(layout_core::effects::filter_rgba_refs(&mut layer, w, h, value, &fcx, (5.0, 5.0, 10.0, 10.0), &resolve), "{value}: the list ran");
        for i in 0..w * h {
            let b = snap[i];
            let (r, g, bl) = layout_core::effects::over(((b >> 16) as u8, (b >> 8) as u8, b as u8), [layer[i * 4], layer[i * 4 + 1], layer[i * 4 + 2], layer[i * 4 + 3]]);
            px[i] = ((r as u32) << 16) | ((g as u32) << 8) | bl as u32;
        }
        let mid = px[10 * w + 10];
        let (r, g, b) = ((mid >> 16) as u8, (mid >> 8) as u8, mid as u8);
        assert!(r == g && g == b && r < 255, "{value}: the red square is grey: {mid:06x}");
        assert_eq!(px[0], 0x00FF_FFFF, "{value}: the backdrop outside the group is untouched");
    }
}
