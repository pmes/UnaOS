// SPDX-License-Identifier: LGPL-3.0-or-later
//! PDF1 (SR300): the page tree (inheritance, loops) and the /Info strings.

mod common;

use common::PdfBuilder;
use pdf_core::{Document, Error};

#[test]
fn page_tree_inheritance_two_levels() {
    let mut b = PdfBuilder::new(1);
    b.obj(1, "<< /Type /Catalog /Pages 2 0 R >>")
        .obj(2, "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /Resources << /Font << /F1 7 0 R >> >> /MediaBox [0 0 500 400] /Rotate 90 >>")
        .obj(3, "<< /Type /Pages /Parent 2 0 R /Kids [5 0 R] /Count 1 /CropBox [10 10 490 390] >>")
        .obj(4, "<< /Type /Page /Parent 2 0 R /MediaBox [100 100 0 0] /Rotate -90 /Resources 8 0 R >>")
        .obj(5, "<< /Type /Page /Parent 3 0 R /Contents [9 0 R 10 0 R] >>")
        .obj(7, "<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>")
        .obj(8, "<< /XObject << >> >>")
        .stream(9, "", b"q")
        .stream(10, "/Filter /AHx", b"51>");
    for pdf in [b.classic(), b.compressed()] {
        let doc = Document::parse(&pdf).unwrap();
        let p = doc.pages().unwrap();
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].media_box, [0.0, 0.0, 500.0, 400.0]);
        assert_eq!(p[0].crop_box, [10.0, 10.0, 490.0, 390.0]);
        assert_eq!(p[0].rotate, 90);
        assert!(p[0].resources.get(b"Font").is_some());
        assert_eq!(p[0].contents, b"q\nQ");
        // The second page's own attributes win; the box is normalised; the crop box defaults to the media box.
        assert_eq!(p[1].media_box, [0.0, 0.0, 100.0, 100.0]);
        assert_eq!(p[1].crop_box, p[1].media_box);
        assert_eq!(p[1].rotate, 270);
        assert!(p[1].resources.get(b"Font").is_none());
        assert!(p[1].resources.get(b"XObject").is_some());
        assert!(p[1].contents.is_empty());
    }
}

#[test]
fn page_defaults() {
    let mut b = PdfBuilder::new(1);
    b.obj(1, "<< /Type /Catalog /Pages 2 0 R >>").obj(2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>").obj(3, "<< /Type /Page >>");
    let p = Document::parse(&b.classic()).unwrap().pages().unwrap();
    assert_eq!(p[0].media_box, [0.0, 0.0, 612.0, 792.0]);
    assert_eq!(p[0].rotate, 0);
}

#[test]
fn kids_loop_is_refused() {
    let mut b = PdfBuilder::new(1);
    b.obj(1, "<< /Type /Catalog /Pages 2 0 R >>").obj(2, "<< /Type /Pages /Kids [3 0 R] >>").obj(3, "<< /Type /Pages /Kids [2 0 R] >>");
    assert_eq!(Document::parse(&b.classic()).unwrap().pages().err(), Some(Error::Loop));
    let mut b = PdfBuilder::new(1);
    b.obj(1, "<< /Type /Catalog /Pages 2 0 R >>").obj(2, "<< /Type /Pages /Kids [3 0 R 3 0 R] >>").obj(3, "<< /Type /Page >>");
    assert_eq!(Document::parse(&b.classic()).unwrap().pages().err(), Some(Error::Loop));
}

#[test]
fn info_strings() {
    let mut b = PdfBuilder::new(1);
    b.obj(1, "<< /Type /Catalog /Pages 2 0 R >>")
        .obj(2, "<< /Type /Pages /Kids [] /Count 0 >>")
        .obj(3, "<< /Title (\\376\\377\\000H\\000i) /Author (Caf\\351 \\200) /Subject <FEFF00E9> /Producer 4 0 R >>")
        .obj(4, "(indirect)");
    b.info = Some(3);
    let doc = Document::parse(&b.classic()).unwrap();
    let i = doc.info();
    assert_eq!(i.title.as_deref(), Some("Hi"));
    assert_eq!(i.author.as_deref(), Some("Caf\u{E9} \u{2022}"));
    assert_eq!(i.subject.as_deref(), Some("\u{E9}"));
    assert_eq!(i.producer.as_deref(), Some("indirect"));
    assert_eq!(i.creator, None);
    assert!(doc.pages().unwrap().is_empty());
}
