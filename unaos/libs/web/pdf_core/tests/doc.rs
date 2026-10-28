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

// PDF4-OUTLINE (SR374): the outline tree, its destinations through the one resolver, and the refusals.

/// Three pages; the outline: "Chapter 1" (open, /Dest page 1) > "Sec 1.1" (/Dest page 2 /XYZ) and a UTF-16BE "Sec 1.2"
/// (closed by /Count -1, /A /GoTo named "two"), then "Chapter 2" (/A /URI).
fn outline_pdf() -> Vec<u8> {
    let mut b = PdfBuilder::new(1);
    b.obj(1, "<< /Type /Catalog /Pages 2 0 R /Outlines 10 0 R /Names << /Dests << /Names [(two) [4 0 R /Fit]] >> >> >>")
        .obj(2, "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 200 100] >>")
        .obj(3, "<< /Type /Page /Parent 2 0 R >>")
        .obj(4, "<< /Type /Page /Parent 2 0 R >>")
        .obj(5, "<< /Type /Page /Parent 2 0 R >>")
        .obj(10, "<< /Type /Outlines /First 11 0 R /Last 14 0 R /Count 3 >>")
        .obj(11, "<< /Title (Chapter 1) /Parent 10 0 R /Next 14 0 R /First 12 0 R /Last 13 0 R /Count 2 /Dest [3 0 R /Fit] >>")
        .obj(12, "<< /Title (Sec 1.1) /Parent 11 0 R /Next 13 0 R /Dest [4 0 R /XYZ 0 80 0] >>")
        .obj(13, "<< /Title <FEFF0053006500630020003100AE0032> /Parent 11 0 R /Prev 12 0 R /Count -1 /First 15 0 R /Last 15 0 R /A << /S /GoTo /D (two) >> >>")
        .obj(14, "<< /Title (Chapter 2) /Parent 10 0 R /Prev 11 0 R /A << /S /URI /URI (https://example.com/) >> >>")
        .obj(15, "<< /Title (Deep) /Parent 13 0 R /Dest [5 0 R /FitH 50] >>");
    b.classic()
}

#[test]
fn outline_tree_and_dests() {
    use pdf_core::annots::{Dest, LinkAction, View};
    let doc = Document::parse(&outline_pdf()).unwrap();
    let o = doc.outlines().unwrap();
    let titles: Vec<&str> = o.iter().map(|i| i.title.as_str()).collect();
    assert_eq!(titles, ["Chapter 1", "Sec 1.1", "Sec 1\u{AE}2", "Deep", "Chapter 2"], "pre-order, UTF-16BE decoded");
    assert_eq!(o.iter().map(|i| i.level).collect::<Vec<_>>(), [0, 1, 1, 2, 0]);
    assert_eq!(o.iter().map(|i| i.kids).collect::<Vec<_>>(), [2, 0, 1, 0, 0]);
    assert_eq!(o.iter().map(|i| i.open).collect::<Vec<_>>(), [true, false, false, false, false], "/Count -1 is closed");
    assert_eq!(o[0].action, Some(LinkAction::GoTo(Dest { page: 0, view: View::Fit })));
    assert_eq!(o[1].action, Some(LinkAction::GoTo(Dest { page: 1, view: View::Xyz { left: Some(0.0), top: Some(80.0), zoom: None } })));
    assert_eq!(o[2].action, Some(LinkAction::GoTo(Dest { page: 1, view: View::Fit })), "/A /GoTo by name through /Names /Dests");
    assert_eq!(o[3].action, Some(LinkAction::GoTo(Dest { page: 2, view: View::FitH(Some(50.0)) })));
    assert_eq!(o[4].action, Some(LinkAction::Uri("https://example.com/".into())));
}

#[test]
fn outline_next_loop_refused() {
    let mut b = PdfBuilder::new(1);
    b.obj(1, "<< /Type /Catalog /Pages 2 0 R /Outlines 10 0 R >>")
        .obj(2, "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 100] >>")
        .obj(3, "<< /Type /Page /Parent 2 0 R >>")
        .obj(10, "<< /Type /Outlines /First 11 0 R >>")
        .obj(11, "<< /Title (A) /Next 12 0 R >>")
        .obj(12, "<< /Title (B) /Next 11 0 R >>");
    let doc = Document::parse(&b.classic()).unwrap();
    assert_eq!(doc.outlines(), Err(Error::Loop), "a broken /Next chain is refused by name");
}

#[test]
fn outline_absent_is_empty() {
    let mut b = PdfBuilder::new(1);
    b.obj(1, "<< /Type /Catalog /Pages 2 0 R >>")
        .obj(2, "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 100] >>")
        .obj(3, "<< /Type /Page /Parent 2 0 R >>");
    assert_eq!(Document::parse(&b.classic()).unwrap().outlines(), Ok(Vec::new()));
}

// =================================================================================================
// PDF4-OUTLINE-ACTIONS / PDF4-OUTLINE-STYLE (LEDGER SR387): refused kinds named, /Named carried, /C and /F read.
// =================================================================================================

#[test]
fn outline_actions_refused_and_style() {
    use pdf_core::annots::LinkAction;
    let mut b = PdfBuilder::new(1);
    b.obj(1, "<< /Type /Catalog /Pages 2 0 R /Outlines 10 0 R >>")
        .obj(2, "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 200 100] >>")
        .obj(3, "<< /Type /Page /Parent 2 0 R >>")
        .obj(4, "<< /Type /Page /Parent 2 0 R >>")
        .obj(10, "<< /Type /Outlines /First 11 0 R /Last 16 0 R /Count 6 >>")
        .obj(11, "<< /Title (Remote) /Next 12 0 R /A << /S /GoToR /F (other.pdf) /D [0 /Fit] >> /C [1 0 0] /F 3 >>")
        .obj(12, "<< /Title (Launch) /Next 13 0 R /A << /S /Launch /F (calc.exe) >> >>")
        .obj(13, "<< /Title (Script) /Next 14 0 R /A << /S /JavaScript /JS (app.alert(1)) >> >>")
        .obj(14, "<< /Title (Next) /Next 15 0 R /A << /S /Named /N /NextPage >> /F 2 >>")
        .obj(15, "<< /Title (Print) /Next 16 0 R /A << /S /Named /N /Print >> /C [0 2 -1] >>")
        .obj(16, "<< /Title (Plain) /Dest [4 0 R /Fit] /C [1 0] >>");
    let o = Document::parse(&b.classic()).unwrap().outlines().unwrap();
    assert_eq!(o.len(), 6);
    assert_eq!(o[0].action, Some(LinkAction::Refused("GoToR")), "/GoToR refused by name");
    assert_eq!(o[1].action, Some(LinkAction::Refused("Launch")), "/Launch refused by name");
    assert_eq!(o[2].action, Some(LinkAction::Refused("JavaScript")), "/JavaScript refused by name");
    assert_eq!(o[3].action, Some(LinkAction::Named(b"NextPage".to_vec())), "a standard /Named is carried for the face");
    assert_eq!(o[4].action, Some(LinkAction::Named(b"Print".to_vec())), "a viewer's own /Named is carried too");
    assert_eq!(o[0].color, Some([1.0, 0.0, 0.0]), "/C read");
    assert!(o[0].italic && o[0].bold, "/F 3: italic and bold");
    assert!(!o[3].italic && o[3].bold, "/F 2: bold only");
    assert_eq!(o[4].color, Some([0.0, 1.0, 0.0]), "/C components clamped to 0..1");
    assert_eq!(o[5].color, None, "a /C of two numbers is none");
    assert!(!o[5].italic && !o[5].bold && o[1].color.is_none(), "no /F, no /C: plain");
}
