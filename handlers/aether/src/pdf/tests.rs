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

/// PDF4FORMSAVE (SR354): the staged save's name, type and bytes.
fn staged_save(e: &mut AetherEngine) -> (String, String, Vec<u8>) {
    match e.take_pending_save() {
        Some(bandy::SMessage::BrowserSave { name, mime, bytes }) => (name, mime, bytes),
        other => panic!("no BrowserSave staged: {other:?}"),
    }
}

#[test]
fn pdf4formsave_filled_form() {
    let mut e = form_engine_with(form_pdf());
    let q = user(&e, 0, 20.0, 10.0);
    click(&mut e, q);
    e.handle_event(Event::Text("Una".to_string()));
    e.handle_event(Event::KeyDown("Ctrl+s".to_string()));
    let (name, mime, bytes) = staged_save(&mut e);
    assert_eq!((name.as_str(), mime.as_str()), ("test.pdf", "application/pdf"), "the URL's basename, no /Title");
    let pdf = e.pdf.as_ref().unwrap();
    assert_eq!(bytes, pdf.bytes, "the file as it stands");
    assert_eq!(field_value(&bytes, "who"), pdf_core::Object::String(b"Una".to_vec()), "the saved bytes re-parse with the new /V");
    assert!(pdf.focus.is_some(), "the field keeps its focus");
    assert_eq!(pdf.focus.as_ref().unwrap().value, "Una", "the s was not typed into the field");
    assert!(e.take_pending_save().is_none(), "taken once");
    e.handle_event(Event::KeyDown("Meta+s".to_string()));
    assert_eq!(staged_save(&mut e).2, bytes, "Cmd+S (the macOS face's key) stages the same");
}

#[test]
fn pdf4formsave_plain_pdf() {
    let mut e = open(400, 300);
    e.handle_event(Event::KeyDown("Ctrl+s".to_string()));
    let (name, mime, bytes) = staged_save(&mut e);
    assert_eq!(name, "PDF4 Test.pdf", "the /Info /Title");
    assert_eq!(mime, "application/pdf");
    assert_eq!(bytes, two_pages(), "an unedited PDF saves its original bytes");
    e.handle_event(Event::KeyDown("s".to_string()));
    assert!(e.take_pending_save().is_none(), "a bare s stages nothing (Shift is Save As: savedialog KATs)");
}

#[test]
fn pdf4formsave_html_tab_stages_nothing() {
    let mut e = AetherEngine::new();
    e.set_device_size(400, 300);
    e.load_html("https://example.com/", "<html><body><p>hi</p></body></html>", true);
    e.handle_event(Event::KeyDown("Ctrl+s".to_string()));
    assert!(e.take_pending_save().is_none(), "PAGE-SAVE is owed: an HTML tab stages nothing");
}


// ------------------------------------------------------------------------------------------------- PDF4GLYPHPOS / PDF4-HSCROLL / PDF4-THUMBS (SR360)

/// A 200 x 100 pt page over `content`: /F1 Helvetica and /F2 Courier, /Widths 500 except W (1000) and i (250).
fn glyph_page(content: &str) -> Vec<u8> {
    let w: Vec<&str> = (32..127).map(|c| if c == b'W' as i32 { "1000" } else if c == b'i' as i32 { "250" } else { "500" }).collect();
    let w = w.join(" ");
    let mut b = PdfBuilder::new(1);
    b.obj(1, "<< /Type /Catalog /Pages 2 0 R >>")
        .obj(2, "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 100] >>")
        .obj(3, "<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 7 0 R /F2 8 0 R >> >> /Contents 5 0 R >>")
        .stream(5, "", content.as_bytes())
        .obj(7, &format!("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar 32 /LastChar 126 /Widths [{w}] >>"))
        .obj(8, &format!("<< /Type /Font /Subtype /Type1 /BaseFont /Courier /FirstChar 32 /LastChar 126 /Widths [{w}] >>"));
    b.classic()
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

#[test]
fn pdf4glyphpos_selection_snaps_to_glyph() {
    use super::select::{char_x, hit, sub_box};
    let mut e = AetherEngine::new();
    e.set_device_size(400, 600);
    e.load_pdf(URL, glyph_page("BT /F1 24 Tf 100 40 Td (WiW) Tj ET"), true);
    let (x0, y0) = {
        let pdf = e.pdf.as_ref().unwrap();
        let spans = pdf.spans(0);
        let s = &spans[0];
        assert_eq!(s.text, "WiW");
        // W 100..124, i 124..130, W 130..154 (a proportional caret would put boundary 1 at 118).
        assert!(near(char_x(s, 1), 124.0), "{}", char_x(s, 1));
        assert!(near(char_x(s, 3), 154.0));
        let b = sub_box(s, 1, 2);
        assert!(near(b[0], 124.0) && near(b[2], 130.0), "{b:?}");
        assert_eq!(hit(&spans, 110.0, 50.0), Some((0, 0)), "nearest glyph boundary (proportional: 1)");
        let (x0, y0, _, _) = pdf.page_rect(0, 400.0);
        (x0, y0)
    };
    // A drag from pt 110 to pt 129 copies "Wi" (proportional: "i").
    e.press_at(x0 + 110.0 * PT, y0 + 50.0 * PT, 1, false);
    e.selection_pointer_moved(x0 + 129.0 * PT, y0 + 50.0 * PT);
    e.pointer_released();
    assert_eq!(e.selected_text(), "Wi");
}

#[test]
fn pdf4glyphpos_find_across_spans() {
    use super::select::Range;
    let mut e = AetherEngine::new();
    e.set_device_size(400, 600);
    e.load_pdf(URL, glyph_page("BT /F1 24 Tf 100 40 Td (Hel) Tj /F2 24 Tf (lo) Tj /F1 24 Tf 0 -30 Td (abc) Tj ET"), true);
    assert_eq!(e.pdf.as_ref().unwrap().spans(0).len(), 3, "a font change starts a span");
    assert_eq!(e.find("Hello", true), (1, 1));
    let pdf = e.pdf.as_ref().unwrap();
    assert_eq!(pdf.hits[0], Range { page: 0, span: 0, lo: 0, end: 1, hi: 2 });
    let r = pdf.range_rect(&pdf.hits[0]).unwrap();
    assert!(near(r[0], 100.0) && near(r[2], 160.0), "{r:?}");
    assert!(pdf.find_all("lo\nab").is_empty(), "never across a line break");
    assert_eq!(pdf.find_all("abc").len(), 1);
    // Inside the "lo" span (pt 150), below the baseline: paper under the active orange.
    let (x0, y0, _, _) = pdf.page_rect(0, 400.0);
    let (x, y) = ((x0 + 150.0 * PT) as usize, (y0 + (r[3] * PT).floor() - 2.0) as usize);
    let f = e.present_rgba();
    assert_eq!(px(&f, x, y), [ACTIVE_MARK.0, ACTIVE_MARK.1, ACTIVE_MARK.2]);
}

#[test]
fn pdf4hscroll_wide_page_zoom300() {
    let mut e = open(400, 600);
    e.set_zoom_pct(300);
    let vw = e.width as f64;
    let max = e.pdf.as_ref().unwrap().max_scroll_x(vw);
    assert!(near(max, 200.0 * PT + 2.0 * GAP - vw), "max_scroll_x {max} at vw {vw}");
    // The blue rect (pt x 20..80) on device row (8 + 80) x 3: at x 104..344 unscrolled.
    let row = 264;
    let f = e.present_rgba();
    assert_eq!(px(&f, 250, row), [0, 0, 255]);
    assert_ne!(px(&f, 10, row), [0, 0, 255]);
    e.handle_event(Event::Scroll(50.0, 0.0));
    assert!(near(e.scroll_x, 50.0));
    let f = e.present_rgba();
    assert_eq!(px(&f, 10, row), [0, 0, 255], "moved 150 device px left");
    assert_ne!(px(&f, 250, row), [0, 0, 255]);
    // The hit test follows: a drag over "Hello" (pt 101 -> 125) copies "He".
    let (x0, y0, _, _) = e.pdf.as_ref().unwrap().page_rect(0, vw);
    let at = |pt: f64| x0 + pt * PT - 50.0;
    let y = y0 + 52.0 * PT;
    e.press_at(at(101.0), y, 1, false);
    e.selection_pointer_moved(at(125.0), y);
    e.pointer_released();
    assert_eq!(e.selected_text(), "He");
    e.handle_event(Event::KeyDown("Right".to_string()));
    assert!(near(e.scroll_x, 50.0 + 40.0), "Right: a 40 px line ({})", e.scroll_x);
    e.handle_event(Event::Scroll(10_000.0, 0.0));
    assert!(near(e.scroll_x, max), "clamped: {}", e.scroll_x);
    e.handle_event(Event::KeyDown("Left".to_string()));
    assert!(near(e.scroll_x, max - 40.0));
    e.set_zoom_pct(100);
    assert_eq!(e.pdf.as_ref().unwrap().max_scroll_x(e.width as f64), 0.0);
    assert_eq!(e.scroll_x, 0.0, "re-clamped on the zoom");
    e.handle_event(Event::Scroll(50.0, 0.0));
    assert_eq!(e.scroll_x, 0.0, "a page that fits does not scroll sideways");
}

/// SR374 (PDF4-THUMBSCROLL) moved two coordinates of this SR360 KAT: the page-label band (`LABEL_H`, 14 px) under
/// each thumbnail puts page 2's thumbnail at rows 47..64 (was 33..50), so its pixel and the click read (60, 55) (were
/// (60, 41)); every assertion and colour is SR360's.
#[test]
fn pdf4thumbs_strip_and_click() {
    use super::thumbs::{STRIP_BG, STRIP_W, THUMB_ACTIVE};
    let mut e = open(400, 100);
    e.handle_event(Event::KeyDown("F4".to_string()));
    {
        let pdf = e.pdf.as_ref().unwrap();
        assert!(pdf.thumbs && near(pdf.left, STRIP_W));
        assert!(near(pdf.page_rect(0, 400.0).0, STRIP_W + GAP), "the lane starts after the strip");
    }
    let f = e.present_rgba();
    let keys = e.pdf.as_ref().unwrap().cached_keys();
    assert!(keys.contains(&(0, 12_000)) && keys.contains(&(1, 12_000)), "both thumbnails through the one cache: {keys:?}");
    // Thumbnails 34 x 17 device px: page 1 at rows 8..25 (framed: current), page 2 at rows 47..64 (all red).
    assert_eq!(px(&f, 60, 55), [255, 0, 0], "page 2's thumbnail");
    assert_eq!(px(&f, 60, 6), [THUMB_ACTIVE.0, THUMB_ACTIVE.1, THUMB_ACTIVE.2], "the current page's frame");
    assert_eq!(px(&f, 2, 95), [STRIP_BG.0, STRIP_BG.1, STRIP_BG.2]);
    click(&mut e, (60.0, 55.0));
    let top2 = e.pdf.as_ref().unwrap().page_rect(1, 400.0).1;
    assert!(near(e.scroll_y, top2), "the click scrolls to page 2: {} vs {top2}", e.scroll_y);
    assert_eq!(e.selected_text(), "", "a strip press selects nothing");
    e.handle_event(Event::KeyDown("F4".to_string()));
    let pdf = e.pdf.as_ref().unwrap();
    assert!(!pdf.thumbs && pdf.left == 0.0);
    assert!(near(pdf.page_rect(0, 400.0).0, 200.0 / 3.0), "today's centring back");
    let _ = e.present_rgba();
    assert!(e.pdf.as_ref().unwrap().cached_keys().iter().all(|k| k.1 != 12_000), "closed: no thumbnail kept");
}

// =================================================================================================
// PDF4-OUTLINE / PDF4-THUMBSCROLL (SR374): the strip's outline mode, its wheel and the page labels.
// =================================================================================================

/// Two 200 x 100 pt pages (page 2 red) and a 2-level outline: "One" (open, page 1) > "Child" (page 2 /XYZ 0 90),
/// then "Two" (page 2).
fn outline_pages() -> Vec<u8> {
    let mut b = PdfBuilder::new(1);
    b.obj(1, "<< /Type /Catalog /Pages 2 0 R /Outlines 10 0 R >>")
        .obj(2, "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 200 100] >>")
        .obj(3, "<< /Type /Page /Parent 2 0 R >>")
        .obj(4, "<< /Type /Page /Parent 2 0 R /Contents 6 0 R >>")
        .stream(6, "", b"1 0 0 rg 0 0 200 100 re f")
        .obj(10, "<< /Type /Outlines /First 11 0 R /Last 13 0 R /Count 3 >>")
        .obj(11, "<< /Title (One) /Parent 10 0 R /Next 13 0 R /First 12 0 R /Last 12 0 R /Count 1 /Dest [3 0 R /Fit] >>")
        .obj(12, "<< /Title (Child) /Parent 11 0 R /Dest [4 0 R /XYZ 0 90 0] >>")
        .obj(13, "<< /Title (Two) /Parent 10 0 R /Prev 11 0 R /Dest [4 0 R /Fit] >>");
    b.classic()
}

/// `n` 200 x 100 pt pages, no outline.
fn n_pages(n: usize) -> Vec<u8> {
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 3 + i)).collect();
    let mut b = PdfBuilder::new(1);
    b.obj(1, "<< /Type /Catalog /Pages 2 0 R >>")
        .obj(2, &format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 200 100] >>", kids.join(" ")));
    for i in 0..n {
        b.obj(3 + i as u32, "<< /Type /Page /Parent 2 0 R >>");
    }
    b.classic()
}

fn engine_with(bytes: Vec<u8>, w: u32, h: u32) -> AetherEngine {
    let mut e = AetherEngine::new();
    e.set_device_size(w, h);
    e.load_pdf(URL, bytes, true);
    e
}

fn outline_engine() -> AetherEngine {
    let mut e = engine_with(outline_pages(), 400, 100);
    e.handle_event(Event::KeyDown("F4".to_string()));
    e.handle_event(Event::KeyDown("F4".to_string()));
    e
}

#[test]
fn pdf4outline_rows_indent() {
    use super::thumbs::{rows, row_x, Side, INDENT, LABEL_INK, ROW_ACTIVE, STRIP_BG, STRIP_W};
    let mut e = outline_engine();
    {
        let pdf = e.pdf.as_ref().unwrap();
        assert!(pdf.thumbs && pdf.side == Side::Outline && near(pdf.left, STRIP_W), "F4 twice: the outline");
        let r = rows(pdf);
        assert_eq!(r.len(), 3, "three rows");
        assert_eq!(r.iter().map(|&i| pdf.outline[i].level).collect::<Vec<_>>(), [0, 1, 0]);
        assert!(near(row_x(1) - row_x(0), INDENT), "16 px per level");
    }
    let f = e.present_rgba();
    // Row 0 at rows 8..30 (the current page's band), its open toggle at x 6..14 from row 17; row 1 at rows 30..52.
    assert_eq!(px(&f, 10, 17), [LABEL_INK.0, LABEL_INK.1, LABEL_INK.2], "row 0's open toggle at row_x(0)");
    assert_eq!(px(&f, 7, 20), [ROW_ACTIVE.0, ROW_ACTIVE.1, ROW_ACTIVE.2], "open: the toggle points down (and the current row's band)");
    assert_eq!(px(&f, 2, 41), [STRIP_BG.0, STRIP_BG.1, STRIP_BG.2], "row 1 is not current");
    assert_eq!(px(&f, 26, 41), [STRIP_BG.0, STRIP_BG.1, STRIP_BG.2], "a leaf has no toggle at row_x(1)");
    e.handle_event(Event::KeyDown("F4".to_string()));
    assert!(!e.pdf.as_ref().unwrap().thumbs, "F4 a third time: off");
}

#[test]
fn pdf4outline_click_child() {
    use pdf_core::annots::{Dest, View};
    let mut e = outline_engine();
    let _ = e.present_rgba();
    click(&mut e, (40.0, 41.0));
    let want = e.pdf.as_ref().unwrap().dest_y(&Dest { page: 1, view: View::Xyz { left: Some(0.0), top: Some(90.0), zoom: None } });
    let top2 = e.pdf.as_ref().unwrap().page_rect(1, 400.0).1;
    assert!(want > top2 && near(e.scroll_y, want), "the child's /XYZ: {} vs {want}", e.scroll_y);
    assert_eq!(e.scroll_x, 0.0);
    assert_eq!(e.selected_text(), "", "an outline press selects nothing");
}

#[test]
fn pdf4outline_toggle_collapses() {
    use super::thumbs::{rows, LABEL_INK};
    let mut e = outline_engine();
    let _ = e.present_rgba();
    click(&mut e, (10.0, 19.0));
    assert_eq!(rows(e.pdf.as_ref().unwrap()).len(), 2, "closed: the child hidden");
    assert_eq!(e.scroll_y, 0.0, "a toggle does not scroll");
    let f = e.present_rgba();
    assert_eq!(px(&f, 7, 20), [LABEL_INK.0, LABEL_INK.1, LABEL_INK.2], "closed: the toggle points right");
    click(&mut e, (10.0, 19.0));
    assert_eq!(rows(e.pdf.as_ref().unwrap()).len(), 3, "open again");
}

#[test]
fn pdf4thumbscroll_wheel() {
    // Six pages: thumbnails every 39 device px from row 8 (the strip's content 242 px), a 100 px frame.
    let mut e = engine_with(n_pages(6), 400, 100);
    e.handle_event(Event::KeyDown("F4".to_string()));
    let _ = e.present_rgba();
    assert_eq!(e.pdf.as_ref().unwrap().strip_off.get(), 0.0);
    e.pointer_moved(10.0, 50.0);
    assert!(e.pdf_wheel(0.0, 40.0), "the strip takes a wheel over it");
    assert_eq!(e.pdf.as_ref().unwrap().strip_off.get(), 40.0);
    assert_eq!(e.scroll_y, 0.0, "the page stack does not move");
    let _ = e.present_rgba();
    assert_eq!(e.pdf.as_ref().unwrap().strip_off.get(), 40.0, "a repaint on the same page keeps the wheel's offset");
    assert!(e.pdf_wheel(0.0, 1000.0));
    assert_eq!(e.pdf.as_ref().unwrap().strip_off.get(), 142.0, "clamped to the content");
    assert!(e.pdf_wheel(0.0, -102.0));
    e.pointer_moved(300.0, 50.0);
    assert!(!e.pdf_wheel(0.0, 40.0), "over the pages the wheel goes on to the one clamp");
    // Page 2 (thumbnail rows 47..64) is inside the window [40, 140]: no re-centring.
    let top = e.pdf.as_ref().unwrap().page_rect(1, 400.0).1;
    e.handle_event(Event::Scroll(0.0, top));
    let _ = e.present_rgba();
    assert_eq!(e.pdf.as_ref().unwrap().strip_off.get(), 40.0, "the current thumbnail in view: the strip stays");
    // Page 6 (rows 203..220) is outside: re-centred, clamped.
    let top = e.pdf.as_ref().unwrap().page_rect(5, 400.0).1;
    e.handle_event(Event::Scroll(0.0, top - e.scroll_y));
    let _ = e.present_rgba();
    assert_eq!(e.pdf.as_ref().unwrap().strip_off.get(), 142.0, "outside: re-centred on it");
}

#[test]
fn pdf4thumbs_labels() {
    use super::thumbs::STRIP_BG;
    if crate::fonts::face(&crate::fonts::FontSel::new(crate::fonts::SANS, 400, false)).is_none() {
        return; // no face installed: nothing to draw the labels with
    }
    let mut e = open(400, 100);
    e.handle_event(Event::KeyDown("F4".to_string()));
    let f = e.present_rgba();
    // Page 1's label band: rows 25..39 under its thumbnail, the number centred on x 60.
    let ink = (25..39).flat_map(|y| (50..70).map(move |x| (x, y))).any(|(x, y)| px(&f, x, y) != [STRIP_BG.0, STRIP_BG.1, STRIP_BG.2]);
    assert!(ink, "page 1's number under its thumbnail");
}

/// SAVEDIALOG (SR364): the staged Save As's name, type and bytes.
fn staged_save_as(e: &mut AetherEngine) -> (String, String, Vec<u8>) {
    match e.take_pending_save() {
        Some(bandy::SMessage::BrowserSaveAs { name, mime, bytes }) => (name, mime, bytes),
        other => panic!("no BrowserSaveAs staged: {other:?}"),
    }
}

#[test]
fn savedialog_ctrl_shift_s_stages_save_as() {
    let mut e = open(400, 300);
    e.handle_event(Event::KeyDown("Ctrl+Shift+S".to_string()));
    let (name, mime, bytes) = staged_save_as(&mut e);
    assert_eq!((name.as_str(), mime.as_str()), ("PDF4 Test.pdf", "application/pdf"), "the Save's own name and type");
    assert_eq!(bytes, two_pages(), "the Save's own bytes");
    assert!(e.take_pending_save().is_none(), "taken once");
    e.handle_event(Event::KeyDown("Meta+Shift+s".to_string()));
    assert_eq!(staged_save_as(&mut e).2, bytes, "Cmd+Shift+S (the macOS face's key) stages the same");
    e.handle_event(Event::KeyDown("Ctrl+s".to_string()));
    assert_eq!(staged_save(&mut e).2, bytes, "plain Ctrl+S is still the Downloads save");
    e.handle_event(Event::KeyDown("Ctrl+Alt+Shift+S".to_string()));
    assert!(e.take_pending_save().is_none(), "Alt stages nothing");
}

#[test]
fn savedialog_html_tab_stages_nothing() {
    let mut e = AetherEngine::new();
    e.set_device_size(400, 300);
    e.load_html("https://example.com/", "<html><body><p>hi</p></body></html>", true);
    e.handle_event(Event::KeyDown("Ctrl+Shift+S".to_string()));
    assert!(e.take_pending_save().is_none(), "PAGE-SAVE is owed: an HTML tab stages no Save As either");
}

// =================================================================================================
// PDF4-STRIPKEYS / PDF4-FIT / PDF4-OUTLINE-CONTRAST (SR389): the strip's keys, the fit modes, the dark strip's ink floor.
// =================================================================================================

fn key(e: &mut AetherEngine, k: &str) {
    e.handle_event(Event::KeyDown(k.to_string()));
}

/// The strip's pixels in the focus ring's colour.
fn ring_px(e: &AetherEngine) -> usize {
    let c = crate::render::focusring::theme().color;
    let f = e.present_rgba();
    (0..f.1 as usize).flat_map(|y| (0..super::thumbs::STRIP_W as usize).map(move |x| (x, y))).filter(|&(x, y)| px(&f, x, y) == [c.0, c.1, c.2]).count()
}

#[test]
fn pdf4stripkeys_tab_down_enter() {
    let mut e = engine_with(n_pages(3), 400, 100);
    key(&mut e, "Tab");
    assert_eq!(e.pdf.as_ref().unwrap().strip_focus, None, "a closed strip takes no Tab");
    key(&mut e, "F4");
    let _ = e.present_rgba();
    assert_eq!(ring_px(&e), 0, "no ring before the strip is focused");
    key(&mut e, "Tab");
    assert_eq!(e.pdf.as_ref().unwrap().strip_focus, Some(0), "Tab: the strip, on the current page's thumbnail");
    key(&mut e, "Down");
    assert_eq!(e.pdf.as_ref().unwrap().strip_focus, Some(1));
    assert_eq!(e.scroll_y, 0.0, "a focus move does not scroll the pages");
    key(&mut e, "Return");
    let top2 = e.pdf.as_ref().unwrap().page_rect(1, 400.0).1;
    assert!(near(e.scroll_y, top2), "Enter lands on page 2: {} vs {top2}", e.scroll_y);
    assert!(ring_px(&e) > 0, "the focused thumbnail carries the focus ring");
    key(&mut e, "Up");
    key(&mut e, "Up");
    assert_eq!(e.pdf.as_ref().unwrap().strip_focus, Some(0), "Up sticks at the first");
    key(&mut e, "End");
    assert_eq!(e.pdf.as_ref().unwrap().strip_focus, Some(2));
    key(&mut e, "Shift+Tab");
    assert_eq!(e.pdf.as_ref().unwrap().strip_focus, None, "Shift+Tab: back to the page");
    assert_eq!(ring_px(&e), 0, "no ring with the page focused");
}

#[test]
fn pdf4stripkeys_outline_tree() {
    use super::thumbs::rows;
    use pdf_core::annots::{Dest, View};
    let mut e = outline_engine();
    let _ = e.present_rgba();
    key(&mut e, "Tab");
    assert_eq!(e.pdf.as_ref().unwrap().strip_focus, Some(0), "the current page's item, \"One\"");
    key(&mut e, "Left");
    assert_eq!(rows(e.pdf.as_ref().unwrap()).len(), 2, "Left closes the parent");
    assert_eq!(e.scroll_x, 0.0, "the strip took Left, not the hscroll");
    key(&mut e, "Right");
    assert_eq!(rows(e.pdf.as_ref().unwrap()).len(), 3, "Right expands it");
    assert!(ring_px(&e) > 0, "the focused row carries the focus ring");
    key(&mut e, "Down");
    assert_eq!(e.pdf.as_ref().unwrap().strip_focus, Some(1), "the child");
    key(&mut e, "Return");
    let want = e.pdf.as_ref().unwrap().dest_y(&Dest { page: 1, view: View::Xyz { left: Some(0.0), top: Some(90.0), zoom: None } });
    assert!(near(e.scroll_y, want), "Enter follows the child's /XYZ: {} vs {want}", e.scroll_y);
    key(&mut e, "End");
    assert_eq!(e.pdf.as_ref().unwrap().strip_focus, Some(2));
    key(&mut e, "Home");
    assert_eq!(e.pdf.as_ref().unwrap().strip_focus, Some(0));
    key(&mut e, "F4");
    assert_eq!(e.pdf.as_ref().unwrap().strip_focus, None, "the strip closed: its focus goes");
}

#[test]
fn pdf4fit_width_page() {
    use super::thumbs::STRIP_W;
    let mut e = open(400, 600);
    let sw = e.pdf.as_ref().unwrap().stack_w();
    key(&mut e, "w");
    assert_eq!(e.zoom_pct(), (400.0 / sw * 100.0).floor() as u32, "fit-width: the frame over the stack's width");
    assert!(e.pdf_status().unwrap().ends_with(" \u{b7} fit-w"), "{:?}", e.pdf_status());
    key(&mut e, "F4");
    assert_eq!(e.zoom_pct(), ((400.0 - STRIP_W) / sw * 100.0).floor() as u32, "the strip open: the strip-less width over the stack's");
    e.set_device_size(600, 600);
    assert_eq!(e.zoom_pct(), ((600.0 - STRIP_W) / sw * 100.0).floor() as u32, "a resize re-fits");
    let fit = e.zoom_pct();
    assert_eq!(e.zoom_step(1), crate::chrome::zoom::step(fit, 1));
    assert_eq!(e.pdf.as_ref().unwrap().fit, None, "a ladder step drops the mode");
    assert!(!e.pdf_status().unwrap().contains("fit"), "{:?}", e.pdf_status());
    e.set_device_size(600, 200);
    key(&mut e, "p");
    let ph = 100.0 * PT + 2.0 * GAP;
    let want = (((600.0 - STRIP_W) / sw).min(200.0 / ph) * 100.0).floor() as u32;
    assert_eq!(e.zoom_pct(), want, "fit-page: the tallest page and its gaps fit the height");
    assert!(want < ((600.0 - STRIP_W) / sw * 100.0).floor() as u32, "the height binds here");
    assert!(e.pdf_status().unwrap().ends_with(" \u{b7} fit-p"), "{:?}", e.pdf_status());
}

#[test]
fn pdf4outline_contrast_floor() {
    use super::thumbs::{ink_floor, INK_FLOOR, STRIP_BG};
    use crate::fonts::raster::luminance;
    assert!(luminance(STRIP_BG) < INK_FLOOR);
    let dark = (60, 0, 0);
    let lifted = ink_floor(dark);
    let l = luminance(lifted);
    assert!((INK_FLOOR..=INK_FLOOR + 3).contains(&l), "a dark /C is lifted to the floor: {lifted:?} at {l}");
    assert!(lifted.0 > lifted.1 && lifted.0 > lifted.2, "its hue holds: {lifted:?}");
    assert_eq!(ink_floor((200, 220, 255)), (200, 220, 255), "an ink above the floor is unchanged");
    assert_eq!(luminance(ink_floor((0, 0, 0))), INK_FLOOR, "black lands on the floor grey");
}
