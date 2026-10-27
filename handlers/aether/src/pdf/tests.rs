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
