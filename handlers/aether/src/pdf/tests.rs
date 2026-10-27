// SPDX-License-Identifier: LGPL-3.0-or-later
//! PDF4 (SR324) KATs: PDF1's builder writes the files, the engine opens them as a fetched response would, and the
//! frame the shell receives (`present_rgba`) is read back at pinned pixels.

#[path = "../../../../unaos/libs/web/pdf_core/tests/common/mod.rs"]
mod builder;

use super::{PT, GAP, BACKDROP, ACTIVE_MARK, is_pdf};
use crate::AetherEngine;
use builder::PdfBuilder;

/// Two 200 x 100 pt pages: page 1 a blue rect (20,20)-(80,60) and "Hello" (Helvetica, /Widths 500: 60 pt wide) at
/// (100, 40); page 2 all red. /Info /Title (PDF4 Test).
fn two_pages() -> Vec<u8> {
    let widths = vec!["500"; 95].join(" ");
    let mut b = PdfBuilder::new(1);
    b.obj(1, "<< /Type /Catalog /Pages 2 0 R >>")
        .obj(2, "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 200 100] >>")
        .obj(3, "<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 7 0 R >> >> /Contents 5 0 R >>")
        .obj(4, "<< /Type /Page /Parent 2 0 R /Contents 6 0 R >>")
        .stream(5, "", b"0 0 1 rg 20 20 60 40 re f 0 g BT /F1 24 Tf 100 40 Td (Hello) Tj ET")
        .stream(6, "", b"1 0 0 rg 0 0 200 100 re f")
        .obj(7, &format!("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar 32 /LastChar 126 /Widths [{widths}] >>"))
        .obj(8, "<< /Title (PDF4 Test) >>");
    b.info = Some(8);
    b.classic()
}

const URL: &str = "https://example.com/docs/test.pdf";

fn open(w: u32, h: u32) -> AetherEngine {
    let mut e = AetherEngine::new();
    e.set_device_size(w, h);
    e.load_pdf(URL, two_pages(), true);
    e
}

fn px(frame: &(u32, u32, Vec<u8>), x: usize, y: usize) -> [u8; 3] {
    let o = (y * frame.0 as usize + x) * 4;
    [frame.2[o], frame.2[o + 1], frame.2[o + 2]]
}

#[test]
fn pdf4_open_is_pdf_document() {
    assert!(is_pdf("application/pdf; charset=binary", b""));
    assert!(is_pdf("application/octet-stream", b"%PDF-1.4\n"));
    assert!(is_pdf("", b"%PDF-1.7"));
    assert!(!is_pdf("text/html", b"%PDF-1.4"));
    assert!(!is_pdf("text/html", b"<html>"));
    let e = open(400, 600);
    let pdf = e.pdf.as_ref().expect("a PDF document");
    assert_eq!(pdf.page_count(), 2);
    assert_eq!(e.title, "PDF4 Test", "the /Info /Title is the tab's title");
    assert_eq!(e.current_url(), URL, "the session history holds the PDF's URL");
}

#[test]
fn pdf4_present_pixels() {
    let e = open(400, 600);
    let pdf = e.pdf.as_ref().unwrap();
    // 200 pt = 266.67 CSS px, centred in 400: left 66.67; top GAP.
    let (x, y, w, h) = pdf.page_rect(0, 400.0);
    assert!((x - 200.0 / 3.0).abs() < 1e-9 && y == GAP && (w - 200.0 * PT).abs() < 1e-9 && (h - 100.0 * PT).abs() < 1e-9);
    let f = e.present_rgba();
    assert_eq!((f.0, f.1), (400, 600));
    assert_eq!(px(&f, 2, 2), [BACKDROP.0, BACKDROP.1, BACKDROP.2], "the backdrop");
    // The blue rect's centre: (50, 60 from the top) pt -> (66.67 + 66.67, 8 + 80) px.
    assert_eq!(px(&f, 133, 88), [0, 0, 255], "page 1's blue rect");
    // Paper: (180, 90 from the top) pt -> (306.67, 128) px.
    assert_eq!(px(&f, 306, 128), [255, 255, 255], "page 1's paper");
}

#[test]
fn pdf4_scroll_page2() {
    let mut e = open(400, 100);
    assert_eq!(e.pdf_status().as_deref(), Some("Page 1 / 2"), "the page under the viewport's centre");
    let top2 = e.pdf.as_ref().unwrap().page_rect(1, 400.0).1;
    assert!((top2 - (GAP + 100.0 * PT + GAP)).abs() < 1e-9);
    e.handle_event(crate::api::events::Event::Scroll(0.0, top2));
    assert!((e.scroll_y - top2).abs() < 1e-9);
    let f = e.present_rgba();
    assert_eq!(px(&f, 200, 2), [255, 0, 0], "page 2's red at the viewport top");
    assert_eq!(e.pdf_status().as_deref(), Some("Page 2 / 2"));
    e.handle_event(crate::api::events::Event::Scroll(0.0, 10_000.0));
    let max = e.pdf.as_ref().unwrap().max_scroll(100.0);
    assert!((e.scroll_y - max).abs() < 1e-9, "clamped to the stack: {} vs {max}", e.scroll_y);
}

#[test]
fn pdf4_zoom_cache_key() {
    let mut e = open(400, 600);
    let _ = e.present_rgba();
    assert_eq!(e.pdf.as_ref().unwrap().cached_keys(), vec![(0, 96_000), (1, 96_000)]);
    e.set_zoom_pct(200);
    let f = e.present_rgba();
    assert_eq!(e.pdf.as_ref().unwrap().cached_keys(), vec![(0, 192_000), (1, 192_000)], "re-rendered at 192 dpi");
    // At 200 % the CSS viewport is 200 px: the page (266.67 px) sits at GAP; the blue rect spans 60 pt = 160 device px
    // on device row (8 + 80) x 2.
    let row = 176;
    let blue = (0..400).filter(|&x| px(&f, x, row) == [0, 0, 255]).count();
    assert!((159..=161).contains(&blue), "blue run {blue}");
    assert_eq!(px(&f, 16 + 100, row), [0, 0, 255]);
}

#[test]
fn pdf4_find_hello() {
    let mut e = open(400, 600);
    assert_eq!(e.find("Hello", true), (1, 1));
    assert_eq!(e.find("nothing here", true), (0, 0));
    assert_eq!(e.find("hello", true), (1, 1), "case-insensitive, the find bar's matcher");
    let pdf = e.pdf.as_ref().unwrap();
    let r = pdf.range_rect(&pdf.hits[0]).expect("the hit's box");
    assert!((r[0] - 100.0).abs() < 1e-6 && (r[2] - 160.0).abs() < 1e-6, "five 12 pt advances from x = 100: {r:?}");
    // Below the baseline (no descenders in "Hello"), inside the box: paper multiplied by the active orange.
    let (x0, y0, _, _) = pdf.page_rect(0, 400.0);
    let x = (x0 + (r[0] * PT).ceil() + 1.0) as usize;
    let y = (y0 + (r[3] * PT).floor() - 2.0) as usize;
    let f = e.present_rgba();
    assert_eq!(px(&f, x, y), [ACTIVE_MARK.0, ACTIVE_MARK.1, ACTIVE_MARK.2]);
    e.find_close();
    assert!(e.pdf.as_ref().unwrap().hits.is_empty());
    assert_eq!(e.selected_text(), "", "the active hit's selection goes with the bar");
}

#[test]
fn pdf4_select_copy_hello() {
    let mut e = open(400, 600);
    let (cx, cy) = {
        let pdf = e.pdf.as_ref().unwrap();
        let s = pdf.spans(0)[0].clone();
        assert_eq!(s.text, "Hello");
        let (x0, y0, _, _) = pdf.page_rect(0, 400.0);
        (x0 + (s.bbox[0] + s.bbox[2]) / 2.0 * PT, y0 + (s.bbox[1] + s.bbox[3]) / 2.0 * PT)
    };
    e.press_at(cx, cy, 2, false);
    e.pointer_released();
    assert_eq!(e.selected_text(), "Hello");
    assert!(matches!(e.copy_selection(), Some(bandy::SMessage::BrowserClipboardSet(t)) if t == "Hello"));
    // A drag from the span's left edge to its middle selects a prefix.
    e.press_at(cx - 30.0 * PT, cy, 1, false);
    e.selection_pointer_moved(cx, cy);
    e.pointer_released();
    let t = e.selected_text();
    assert!(t == "He" || t == "Hel", "prefix {t:?}");
    e.select_all();
    assert_eq!(e.selected_text(), "Hello");
    // A press on bare paper clears it.
    e.press_at(2.0, 2.0, 1, false);
    assert_eq!(e.selected_text(), "");
}

#[test]
fn pdf4_password_page() {
    let mut b = PdfBuilder::new(1);
    b.obj(1, "<< /Type /Catalog /Pages 2 0 R >>")
        .obj(2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
        .obj(3, "<< /Type /Page /Parent 2 0 R >>")
        .obj(9, &format!("<< /Filter /Standard /V 1 /R 2 /P -4 /O <{}> /U <{}> >>", "11".repeat(32), "22".repeat(32)));
    b.trailer_extra = "/Encrypt 9 0 R /ID [<00112233445566778899aabbccddeeff> <00112233445566778899aabbccddeeff>]".to_string();
    let mut e = AetherEngine::new();
    e.load_pdf(URL, b.classic(), true);
    assert!(e.pdf.is_none(), "a PDF that needs a password is not opened");
    assert_eq!(e.title, "test.pdf");
    let body = e.document.as_ref().unwrap().text_contents();
    assert!(body.contains("password-protected"), "{body}");
}

#[test]
fn pdf4_html_after_pdf() {
    let mut e = open(400, 600);
    assert!(e.pdf.is_some());
    e.load_html("https://example.com/", "<html><head><title>Back to HTML</title></head><body>hi</body></html>", true);
    assert!(e.pdf.is_none(), "load_impl drops the PDF");
    assert_eq!(e.title, "Back to HTML");
    assert_eq!(e.pdf_status(), None);
}

// =================================================================================================
// PDF4ANNOTS (SR343): links and form widgets on the page stack.
// =================================================================================================

use crate::api::events::Event;

/// Three 40 x 40 pt pages (pdf_core's `annot_pdf`: page objects 3, 5, 7). Page 1, top row (y 30-40): a /URI link,
/// a /Dest [page 2 /XYZ 0 30] link, a /Named /NextPage link (x 0-30) and a check box with /On (blue) and /Off (green)
/// appearances (x 30-40); second row (y 20-30): a relative /URI, a `javascript:` /URI and /Named /FirstPage (x 0-30);
/// a text field (/V empty, generated appearance, /Helv 10) at [2 2 38 18]. Page 2 red.
fn form_pdf() -> Vec<u8> {
    builder::annot_pdf(
        "/AcroForm 30 0 R",
        &[("/Annots [20 0 R 21 0 R 22 0 R 23 0 R 24 0 R 25 0 R 40 0 R 41 0 R]", ""), ("", "1 0 0 rg 0 0 40 40 re f"), ("", "")],
        &[
            (20, "<< /Subtype /Link /Rect [0 30 10 40] /A << /S /URI /URI (https://unaos.org/a) >> >>", None),
            (21, "<< /Subtype /Link /Rect [10 30 20 40] /Dest [5 0 R /XYZ 0 30 0] >>", None),
            (22, "<< /Subtype /Link /Rect [20 30 30 40] /A << /S /Named /N /NextPage >> >>", None),
            (23, "<< /Subtype /Link /Rect [0 20 10 30] /A << /S /URI /URI (next.pdf) >> >>", None),
            (24, "<< /Subtype /Link /Rect [10 20 20 30] /A << /S /URI /URI (javascript:void) >> >>", None),
            (25, "<< /Subtype /Link /Rect [20 20 30 30] /A << /S /Named /N /FirstPage >> >>", None),
            (30, "<< /Fields [40 0 R 41 0 R] /DA (/Helv 10 Tf 0 g) /DR << /Font << /Helv 31 0 R >> >> >>", None),
            (31, "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>", None),
            (40, "<< /T (who) /FT /Tx /V () /DA (/Helv 10 Tf 0 g) /Subtype /Widget /Rect [2 2 38 18] >>", None),
            (41, "<< /T (ok) /FT /Btn /V /Off /AS /Off /Subtype /Widget /Rect [30 30 40 40] /AP << /N << /On 50 0 R /Off 51 0 R >> >> >>", None),
            (50, "/Subtype /Form /BBox [0 0 10 10]", Some(b"0 0 1 rg 0 0 10 10 re f")),
            (51, "/Subtype /Form /BBox [0 0 10 10]", Some(b"0 1 0 rg 0 0 10 10 re f")),
        ],
    )
}

fn form_engine_with(bytes: Vec<u8>) -> AetherEngine {
    let mut e = AetherEngine::new();
    e.set_device_size(400, 60);
    e.load_pdf(URL, bytes, true);
    e
}

/// The viewport point over user-space (`ux`, `uy`) of page `page`.
fn user(e: &AetherEngine, page: usize, ux: f64, uy: f64) -> (f64, f64) {
    let (x, y, _, _) = e.pdf.as_ref().unwrap().page_rect(page, 400.0);
    (x + ux * PT, y + (40.0 - uy) * PT - e.scroll_y)
}

fn click(e: &mut AetherEngine, (x, y): (f64, f64)) {
    e.handle_event(Event::MouseDown(x, y));
    e.handle_event(Event::MouseUp(x, y));
}

fn fnv(data: &[u8]) -> u64 {
    data.iter().fold(0xcbf29ce484222325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x100000001b3))
}

/// The field `name`'s /V in a file.
fn field_value(bytes: &[u8], name: &str) -> pdf_core::Object {
    let doc = pdf_core::Document::parse(bytes).unwrap();
    doc.form().unwrap().fields.into_iter().find(|f| f.name == name).unwrap().value
}

fn sections(bytes: &[u8]) -> usize {
    bytes.windows(9).filter(|w| w == b"startxref").count()
}

#[test]
fn pdf4annots_link_hover_click() {
    let mut e = form_engine_with(form_pdf());
    let p = user(&e, 0, 5.0, 35.0);
    e.handle_event(Event::MouseMove(p.0, p.1));
    assert_eq!(e.take_cursor_change().as_deref(), Some("pointer"));
    assert_eq!(e.hovered_link().as_deref(), Some("https://unaos.org/a"), "the status line shows the URI");
    click(&mut e, p);
    assert_eq!(e.take_pending_nav().map(|n| n.url).as_deref(), Some("https://unaos.org/a"), "the <a> path's navigation");
    // A relative /URI resolves against the PDF's URL.
    let q = user(&e, 0, 5.0, 25.0);
    click(&mut e, q);
    assert_eq!(e.take_pending_nav().map(|n| n.url).as_deref(), Some("https://example.com/docs/next.pdf"));
    // A javascript: URI is never a navigation, and shows nothing.
    let js = user(&e, 0, 15.0, 25.0);
    e.handle_event(Event::MouseMove(js.0, js.1));
    assert_eq!(e.hovered_link(), None);
    click(&mut e, js);
    assert!(e.take_pending_nav().is_none());
    // A press on the link released off it does nothing; off every link the status line is the page indicator.
    let off = user(&e, 0, 35.0, 25.0);
    e.handle_event(Event::MouseDown(p.0, p.1));
    e.handle_event(Event::MouseUp(off.0, off.1));
    assert!(e.take_pending_nav().is_none());
    e.handle_event(Event::MouseMove(off.0, off.1));
    assert_eq!(e.take_cursor_change().as_deref(), Some("default"));
    assert_eq!(e.hovered_link(), None);
    assert_eq!(e.pdf_status().as_deref(), Some("Page 1 / 3"));
}

#[test]
fn pdf4annots_goto_xyz() {
    let mut e = form_engine_with(form_pdf());
    let top2 = e.pdf.as_ref().unwrap().page_rect(1, 400.0).1;
    let g = user(&e, 0, 15.0, 35.0);
    e.handle_event(Event::MouseMove(g.0, g.1));
    assert_eq!(e.hovered_link().as_deref(), Some("Page 2"));
    click(&mut e, g);
    assert!((e.scroll_y - (top2 + 10.0 * PT)).abs() < 1e-9, "page 2's /XYZ top 30 at the viewport top: {}", e.scroll_y);
    assert_eq!(e.pdf_status().as_deref(), Some("Page 2 / 3"));
    // /Named /NextPage from page 1: page 2's top.
    e.handle_event(Event::Scroll(0.0, -10_000.0));
    let n = user(&e, 0, 25.0, 35.0);
    e.handle_event(Event::MouseMove(n.0, n.1));
    assert_eq!(e.hovered_link().as_deref(), Some("Page 2"));
    click(&mut e, n);
    assert!((e.scroll_y - top2).abs() < 1e-9, "{}", e.scroll_y);
    // /Named /FirstPage: page 1's top.
    e.handle_event(Event::Scroll(0.0, 20.0 - e.scroll_y));
    let q = user(&e, 0, 25.0, 25.0);
    click(&mut e, q);
    assert!((e.scroll_y - GAP).abs() < 1e-9, "{}", e.scroll_y);
}

#[test]
fn pdf4annots_text_field_types() {
    let mut e = form_engine_with(form_pdf());
    let orig = e.pdf.as_ref().unwrap().bytes.clone();
    // The field's box on the frame: user [2 2 38 18].
    let (a, b) = (user(&e, 0, 2.0, 18.0), user(&e, 0, 38.0, 2.0));
    let ink = |f: &(u32, u32, Vec<u8>)| {
        let mut n = 0;
        for y in (a.1.ceil() as usize + 1)..(b.1.floor() as usize - 1) {
            for x in (a.0.ceil() as usize + 1)..(b.0.floor() as usize - 1) {
                if px(f, x, y)[0] < 100 {
                    n += 1;
                }
            }
        }
        n
    };
    let f0 = e.present_rgba();
    assert_eq!(ink(&f0), 0, "the empty field");
    let p = user(&e, 0, 20.0, 10.0);
    e.handle_event(Event::MouseMove(p.0, p.1));
    assert_eq!(e.take_cursor_change().as_deref(), Some("text"));
    click(&mut e, p);
    assert!(e.pdf.as_ref().unwrap().focus.is_some(), "a click focuses the text field");
    e.handle_event(Event::Text("Hi".to_string()));
    e.handle_event(Event::KeyDown("BackSpace".to_string()));
    e.handle_event(Event::Text("o".to_string()));
    let pdf = e.pdf.as_ref().unwrap();
    assert_eq!(pdf.focus.as_ref().unwrap().value, "Ho");
    assert_eq!(field_value(&pdf.bytes, "who"), pdf_core::Object::String(b"Ho".to_vec()), "the update carries /V");
    assert!(pdf.bytes.starts_with(&orig), "an incremental update: the original bytes a prefix");
    assert_eq!(sections(&pdf.bytes), sections(&orig) + 1, "one focus session, one update section");
    assert!(pdf.spans(0).iter().any(|s| s.text.contains("Ho")), "the regenerated appearance's text");
    assert_eq!(pdf.form_refused, None);
    let f1 = e.present_rgba();
    assert!(ink(&f1) > 0, "the typed text is painted");
    assert_ne!(fnv(&f0.2), fnv(&f1.2));
    // Return blurs; the keys go back to the page.
    e.handle_event(Event::KeyDown("Return".to_string()));
    assert!(e.pdf.as_ref().unwrap().focus.is_none());
}

#[test]
fn pdf4annots_checkbox_toggles() {
    let mut e = form_engine_with(form_pdf());
    let c = user(&e, 0, 35.0, 35.0);
    let at = (c.0 as usize, c.1 as usize);
    assert_eq!(px(&e.present_rgba(), at.0, at.1), [0, 255, 0], "/Off's green");
    e.handle_event(Event::MouseMove(c.0, c.1));
    assert_eq!(e.take_cursor_change().as_deref(), Some("pointer"));
    click(&mut e, c);
    let f = e.present_rgba();
    assert_eq!(px(&f, at.0, at.1), [0, 0, 255], "/On's blue");
    assert_eq!(fnv(&f.2), CHECKED_FNV, "the checked frame");
    let pdf = e.pdf.as_ref().unwrap();
    assert_eq!(field_value(&pdf.bytes, "ok"), pdf_core::Object::Name(b"On".to_vec()));
    assert_eq!(pdf.annots(0)[7].dict.get(b"AS"), Some(&pdf_core::Object::Name(b"On".to_vec())));
    click(&mut e, c);
    assert_eq!(px(&e.present_rgba(), at.0, at.1), [0, 255, 0], "toggled back off");
    assert_eq!(field_value(&e.pdf.as_ref().unwrap().bytes, "ok"), pdf_core::Object::Name(b"Off".to_vec()));
}

/// `pdf4annots_checkbox_toggles`' frame with the box checked (no text on page 1's paper: font-independent).
const CHECKED_FNV: u64 = 0x223cdd76788ae19;

#[test]
fn pdf4annots_save_update_bytes() {
    let mut e = form_engine_with(form_pdf());
    let q = user(&e, 0, 35.0, 35.0);
    click(&mut e, q);
    let q = user(&e, 0, 20.0, 10.0);
    click(&mut e, q);
    e.handle_event(Event::Text("Una".to_string()));
    let q = user(&e, 0, 39.0, 25.0);
    click(&mut e, q);
    assert!(e.pdf.as_ref().unwrap().focus.is_none(), "a press on bare paper blurs the field");
    // The bytes a save writes (PDF4-FORMSAVE owes the verb): both fills read back.
    let saved = e.pdf.as_ref().unwrap().bytes.clone();
    assert_eq!(field_value(&saved, "ok"), pdf_core::Object::Name(b"On".to_vec()));
    assert_eq!(field_value(&saved, "who"), pdf_core::Object::String(b"Una".to_vec()));
    assert_eq!(sections(&saved), 3, "the original, the check box's update, the text field's");
    // save_update refuses a file whose xref was rebuilt: the fill is refused by name and the page stays as it was.
    let pdf = form_pdf();
    let off = builder::startxref_of(&pdf);
    let broken = builder::replace_bytes(&pdf, format!("startxref\n{off}\n").as_bytes(), b"startxref\n7\n");
    let mut e = form_engine_with(broken.clone());
    let q = user(&e, 0, 35.0, 35.0);
    click(&mut e, q);
    let p = e.pdf.as_ref().unwrap();
    assert!(p.form_refused.as_deref().is_some_and(|r| r.contains("rebuilt")), "{:?}", p.form_refused);
    assert_eq!(p.bytes, broken);
    assert_eq!(field_value(&p.bytes, "ok"), pdf_core::Object::Name(b"Off".to_vec()));
}
