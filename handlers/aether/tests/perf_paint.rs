//! AETHERPAINTPERF (LEDGER SR130): how much of the document a frame pays for, before and after the scopes.
//!
//! Three fixtures, each driven twice on a fresh engine — BEFORE with every scope off (`perf::set_scoped(false)`:
//! whole cascade, every box painted, no glyph-run cache, viewport damage per animation frame) and AFTER with them
//! on — reading `aether::perf` counters over the measured frames:
//!
//! * `list` — 2,000 rows (`.row:nth-child(odd)` zebra), one visible row's text changes by script: the restyle
//!   re-cascades that row only; the box build and the taffy passes stay whole-page (laid_out is the tree);
//!   the frame after the relayout damages the viewport, and the paint index paints only the boxes on screen;
//!   the unchanged rows redraw from the glyph-run cache.
//! * `anim` — 300 boxes, one with a `background-color` CSS animation: ten frames, each damaging only the
//!   animated box's ink, no cascade, no layout.
//! * `keys` — a form field and 200 paragraphs, 20 typed characters: each frame damages the field only.
//!
//! Each fixture also checks BYTE IDENTITY: the scoped frame equals the frame the whole-page path paints from
//! the same state (scopes off, a whole relayout where it re-samples nothing, viewport damage).
//!
//! Run: `cargo test -p aether --test perf_paint -- --nocapture` (each fixture prints one `PAINTPERF` line).

use aether::{perf, AetherEngine};

const URL: &str = "http://paintperf.test/";

fn load(html: &str, scoped: bool) -> AetherEngine {
    perf::set_scoped(scoped);
    let mut e = AetherEngine::new();
    e.load_html(URL, html, true);
    if let Some(js) = e.js_engine.as_mut() {
        aether::event_loop::settle(js, 1000);
    }
    e.relayout(); // the load's first cascade and layout (the shell's first frame does this; headless, we do)
    e.tick();
    e.render_frame(); // the first frame: viewport damage from the load
    e
}

fn eval(e: &mut AetherEngine, src: &str) -> String {
    let js = e.js_engine.as_mut().expect("page has a script engine");
    js.eval_string(src).unwrap_or_else(|err| format!("THROW {err}"))
}

/// A whole-page frame of the engine's current state: scopes off, the viewport damaged.
fn whole_frame(e: &mut AetherEngine, relayout: bool) -> Vec<u8> {
    perf::set_scoped(false);
    if relayout {
        e.relayout();
    }
    e.damage_rects.push((0, 0, e.width, e.height));
    e.render_frame();
    perf::set_scoped(true);
    e.surface().to_vec()
}

fn same_frame(scoped: &[u8], whole: &[u8], width: u32, what: &str) {
    if scoped != whole {
        let i = scoped.iter().zip(whole).position(|(a, b)| a != b).unwrap_or(0) / 4;
        panic!("{what}: the scoped frame differs from the whole-page frame, first at pixel ({}, {})", i as u32 % width, i as u32 / width);
    }
}

fn line(fixture: &str, elements: usize, before: &perf::Counters, after: &perf::Counters) {
    println!(
        "PAINTPERF fixture={fixture} elements={elements} before: cascaded={} laid_out={} painted={} glyph_hits={} | after: cascaded={} laid_out={} painted={} skipped={} glyph_hits={} scoped_restyles={} whole_restyles={}",
        before.cascaded, before.laid_out, before.painted, before.glyph_hits,
        after.cascaded, after.laid_out, after.painted, after.skipped, after.glyph_hits, after.scoped_restyles, after.whole_restyles,
    );
}

fn list_html() -> String {
    let rows: String = (0..2000).map(|i| format!("<div class=\"row\">Row {i}</div>")).collect();
    format!(
        "<!doctype html><html><head><style>\
         body {{ margin: 0; font: 14px sans-serif }}\
         .row {{ height: 18px; padding: 0 4px }}\
         .row:nth-child(odd) {{ background: rgb(238, 238, 238) }}\
         .row.sel {{ color: rgb(200, 0, 0) }}\
         </style></head><body><div id=\"list\">{rows}</div></body></html>"
    )
}

/// One visible row's text changes (textContent: the row's children are replaced).
fn list_measure(scoped: bool) -> (perf::Counters, AetherEngine) {
    let mut e = load(&list_html(), scoped);
    perf::reset();
    assert_eq!(eval(&mut e, "document.getElementById('list').children[5].textContent = 'Row five, changed'; 'ok'"), "ok");
    e.tick(); // the mutation relayouts
    e.render_frame();
    (perf::counters(), e)
}

#[test]
fn list_row_text_change() {
    let elements = 2005; // html, head, style, body, #list and 2,000 rows
    let (before, _) = list_measure(false);
    let (after, mut e) = list_measure(true);
    line("list", elements, &before, &after);
    assert!(before.cascaded >= 2000, "the whole cascade runs every element: {before:?}");
    assert_eq!((after.scoped_restyles, after.whole_restyles), (1, 0), "the restyle is scoped: {after:?}");
    // The changed row's children changed and a selector reads sibling structure: the row's subtree re-cascades.
    assert!(after.cascaded <= 4, "a one-row change re-cascades at most 4 of {elements} elements: {after:?}");
    // The viewport (800x600) shows ~35 rows of 18 px: the index paints those and the ancestors.
    assert!(after.painted <= 100, "the paint touches at most 100 of {elements} boxes (5%): {after:?}");
    assert!(after.skipped >= 1900, "the off-screen rows are skipped: {after:?}");
    assert!(after.glyph_hits >= 20, "the unchanged visible rows redraw from the run cache: {after:?}");
    let scoped = e.surface().to_vec();
    let whole = whole_frame(&mut e, true);
    same_frame(&scoped, &whole, e.width, "list");
}

fn anim_html() -> String {
    let boxes: String = (0..300).map(|i| if i == 150 { "<div class=\"b\" id=\"a\"></div>".to_string() } else { "<div class=\"b\"></div>".to_string() }).collect();
    format!(
        "<!doctype html><html><head><style>\
         body {{ margin: 0 }}\
         #grid {{ display: flex; flex-wrap: wrap; width: 800px }}\
         .b {{ width: 30px; height: 30px; margin: 4px; background-color: rgb(0, 120, 200) }}\
         #a {{ animation: pulse 1s linear infinite }}\
         @keyframes pulse {{ from {{ background-color: rgb(255, 0, 0) }} to {{ background-color: rgb(0, 0, 255) }} }}\
         </style></head><body><div id=\"grid\">{boxes}</div></body></html>"
    )
}

/// Ten animation frames; the largest per-frame counts.
fn anim_measure(scoped: bool) -> (perf::Counters, perf::Counters, AetherEngine) {
    let mut e = load(&anim_html(), scoped);
    e.tick(); // the animation's first frame starts it
    e.render_frame();
    perf::reset();
    let mut worst = perf::Counters::default();
    for _ in 0..10 {
        std::thread::sleep(std::time::Duration::from_millis(5));
        e.tick();
        e.render_frame();
        let f = perf::last_frame();
        worst.cascaded = worst.cascaded.max(f.cascaded);
        worst.laid_out = worst.laid_out.max(f.laid_out);
        worst.painted = worst.painted.max(f.painted);
        worst.skipped = worst.skipped.max(f.skipped);
    }
    (perf::counters(), worst, e)
}

#[test]
fn one_animated_box() {
    let elements = 305; // html, head, style, body, #grid and 300 boxes
    let (before, before_worst, _) = anim_measure(false);
    let (after, worst, mut e) = anim_measure(true);
    line("anim", elements, &before, &after);
    println!("PAINTPERF fixture=anim per-frame-max before: painted={} | after: cascaded={} laid_out={} painted={}", before_worst.painted, worst.cascaded, worst.laid_out, worst.painted);
    assert!(before_worst.painted >= 300, "viewport damage paints every box: {before_worst:?}");
    assert_eq!((worst.cascaded, worst.laid_out), (0, 0), "a paint-only animation frame neither cascades nor lays out");
    // The document node, html, body, #grid and #a.
    assert!(worst.painted <= 8, "an animation frame paints at most 8 of {elements} boxes: {worst:?}");
    let scoped = e.surface().to_vec();
    let whole = whole_frame(&mut e, false); // no relayout: it would sample the animation at a later time
    same_frame(&scoped, &whole, e.width, "anim");
}

fn keys_html() -> String {
    let paras: String = (0..200).map(|i| format!("<p>Paragraph {i} of the page under the field.</p>")).collect();
    format!(
        "<!doctype html><html><head><style>body {{ margin: 0; font: 14px sans-serif }} #f {{ width: 300px }}</style></head>\
         <body><form><input id=\"f\" autofocus></form>{paras}</body></html>"
    )
}

/// Twenty typed characters, a frame after each; the largest per-frame counts.
fn keys_measure(scoped: bool) -> (perf::Counters, perf::Counters, AetherEngine) {
    let mut e = load(&keys_html(), scoped);
    perf::reset();
    let mut worst = perf::Counters::default();
    for k in 0..20 {
        let ch = (b'a' + (k % 26) as u8) as char;
        e.handle_event(aether::api::events::Event::Text(ch.to_string()));
        e.render_frame();
        let f = perf::last_frame();
        worst.cascaded = worst.cascaded.max(f.cascaded);
        worst.laid_out = worst.laid_out.max(f.laid_out);
        worst.painted = worst.painted.max(f.painted);
    }
    (perf::counters(), worst, e)
}

#[test]
fn twenty_keystrokes() {
    let elements = 206; // html, head, style, body, form, input and 200 paragraphs
    let (before, before_worst, _) = keys_measure(false);
    let (after, worst, mut e) = keys_measure(true);
    line("keys", elements, &before, &after);
    println!("PAINTPERF fixture=keys per-frame-max before: painted={} | after: cascaded={} laid_out={} painted={}", before_worst.painted, worst.cascaded, worst.laid_out, worst.painted);
    assert_eq!(eval(&mut e, "document.getElementById('f').value"), "abcdefghijklmnopqrst");
    assert!(before_worst.painted >= 200, "the classic walk reaches every box: {before_worst:?}");
    assert_eq!((worst.cascaded, worst.laid_out), (0, 0), "typing without a listener neither cascades nor lays out");
    // The document node, html, body, form and the field.
    assert!(worst.painted <= 8, "a keystroke frame paints at most 8 of {elements} boxes: {worst:?}");
    let scoped = e.surface().to_vec();
    let whole = whole_frame(&mut e, true);
    same_frame(&scoped, &whole, e.width, "keys");
}

#[test]
fn wire_line_shape() {
    let c = perf::Counters { cascaded: 1, laid_out: 2, painted: 3, glyph_hits: 4, ..Default::default() };
    assert_eq!(perf::wire_line(&c), ":: paintperf: cascaded=1 laid_out=2 painted=3 glyph_hits=4 ::");
}
