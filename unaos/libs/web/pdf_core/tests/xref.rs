// SPDX-License-Identifier: LGPL-3.0-or-later
//! PDF1 (SR300): the cross-reference in every shape the builder writes, incremental updates and reconstruction.

mod common;

use common::{Body, PdfBuilder, incremental};
use pdf_core::{Document, Error, Object, Ref, XrefKind};

const CONTENT: &[u8] = b"BT /F1 24 Tf 72 720 Td (Hello) Tj ET";

/// One page, a font, an /Info: the object list every shape is written from.
fn one_page() -> PdfBuilder {
    let mut b = PdfBuilder::new(1);
    b.obj(1, "<< /Type /Catalog /Pages 2 0 R >>")
        .obj(2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
        .obj(3, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>")
        .stream(4, "", CONTENT)
        .obj(5, "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
        .obj(6, "<< /Title (Hello) /Producer (pdf_core tests) >>");
    b.info = Some(6);
    b
}

fn check_one_page(doc: &Document) {
    let pages = doc.pages().unwrap();
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0].media_box, [0.0, 0.0, 612.0, 792.0]);
    assert_eq!(pages[0].contents, CONTENT);
    assert!(pages[0].resources.get(b"Font").is_some());
    assert_eq!(doc.info().title.as_deref(), Some("Hello"));
}

#[test]
fn classic_table() {
    let pdf = one_page().classic();
    let doc = Document::parse(&pdf).unwrap();
    assert_eq!(doc.xref_kind(), XrefKind::Table);
    assert_eq!(doc.version(), "1.4");
    check_one_page(&doc);
}

#[test]
fn classic_table_with_19_byte_rows() {
    let pdf = one_page().classic();
    let s = common::replace_bytes(&common::replace_bytes(&pdf, b" n \n", b" n\n"), b" f \n", b" f\n");
    let doc = Document::parse(&s).unwrap();
    assert_eq!(doc.xref_kind(), XrefKind::Table);
    check_one_page(&doc);
}

#[test]
fn xref_stream_and_object_stream() {
    let pdf = one_page().compressed();
    let doc = Document::parse(&pdf).unwrap();
    assert_eq!(doc.xref_kind(), XrefKind::Stream);
    check_one_page(&doc);
    // Object 5 lives in the object stream.
    let font = doc.resolve(Ref { num: 5, generation: 0 }).unwrap();
    assert_eq!(font.as_dict().unwrap().name(b"BaseFont"), Some(&b"Helvetica"[..]));
}

#[test]
fn hybrid_xrefstm() {
    let pdf = one_page().hybrid();
    let doc = Document::parse(&pdf).unwrap();
    assert_eq!(doc.xref_kind(), XrefKind::Table);
    check_one_page(&doc);
}

#[test]
fn incremental_update_replaces_a_page() {
    let base = one_page().classic();
    let upd = incremental(
        &base,
        &[
            (3, Body::Plain(b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 7 0 R >>".to_vec())),
            (7, Body::Stream(Vec::new(), b"0 0 m 10 10 l S".to_vec())),
        ],
        1,
        8,
    );
    let doc = Document::parse(&upd).unwrap();
    assert_eq!(doc.xref_kind(), XrefKind::Table);
    let pages = doc.pages().unwrap();
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0].media_box, [0.0, 0.0, 200.0, 100.0]);
    assert_eq!(pages[0].contents, b"0 0 m 10 10 l S");
    // The older section still supplies what the update did not touch.
    assert_eq!(doc.info().title.as_deref(), Some("Hello"));
    // An update over a compressed base.
    let base = one_page().compressed();
    let upd = incremental(&base, &[(3, Body::Plain(b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 50 60] >>".to_vec()))], 1, 9);
    let pages = Document::parse(&upd).unwrap().pages().unwrap();
    assert_eq!(pages[0].media_box, [0.0, 0.0, 50.0, 60.0]);
}

#[test]
fn broken_startxref_reconstructs() {
    let pdf = one_page().classic();
    let at = pdf.windows(9).rposition(|w| w == b"startxref").unwrap();
    let mut broken = pdf[..at].to_vec();
    broken.extend_from_slice(b"startxref\n999999999\n%%EOF\n");
    let doc = Document::parse(&broken).unwrap();
    assert_eq!(doc.xref_kind(), XrefKind::Rebuilt);
    check_one_page(&doc);
}

#[test]
fn shifted_offsets_reconstruct() {
    // Bytes inserted after the header move every object; the table's offsets now name the wrong bytes.
    let pdf = one_page().classic();
    let mut shifted = pdf[..15].to_vec();
    shifted.extend_from_slice(b"% junk inserted by a broken tool\n");
    shifted.extend_from_slice(&pdf[15..]);
    let doc = Document::parse(&shifted).unwrap();
    assert_eq!(doc.xref_kind(), XrefKind::Rebuilt);
    check_one_page(&doc);
}

#[test]
fn compressed_without_xref_reconstructs_through_the_object_stream() {
    let pdf = one_page().compressed();
    let at = pdf.windows(9).rposition(|w| w == b"startxref").unwrap();
    let mut broken = pdf[..at].to_vec();
    broken.extend_from_slice(b"%%EOF\n");
    let doc = Document::parse(&broken).unwrap();
    assert_eq!(doc.xref_kind(), XrefKind::Rebuilt);
    check_one_page(&doc);
}

#[test]
fn prev_loop_terminates() {
    let pdf = one_page().classic();
    let xref = common::startxref_of(&pdf);
    let s = common::replace_bytes(&pdf, b"trailer\n<<", format!("trailer\n<< /Prev {xref}").as_bytes());
    let doc = Document::parse(&s).unwrap();
    assert_eq!(doc.xref_kind(), XrefKind::Table);
    check_one_page(&doc);
}

#[test]
fn encrypt_is_refused_by_name() {
    let mut b = one_page();
    b.obj(9, "<< /Filter /Standard /V 1 /R 2 >>");
    b.trailer_extra = "/Encrypt 9 0 R".into();
    assert_eq!(Document::parse(&b.classic()).err(), Some(Error::Encrypted));
    assert_eq!(Document::parse(&b.compressed()).err(), Some(Error::Encrypted));
}

#[test]
fn free_and_missing_objects_are_null() {
    let doc = Document::parse(&one_page().classic()).unwrap();
    assert_eq!(doc.resolve(Ref { num: 40, generation: 0 }).unwrap(), Object::Null);
    assert_eq!(doc.resolve(Ref { num: 0, generation: 65535 }).unwrap(), Object::Null);
}

#[test]
fn garbage_is_an_error_not_a_panic() {
    assert!(Document::parse(b"").is_err());
    // A catalog with no xref and no /Pages: reconstruction finds it, the page tree is missing.
    let doc = Document::parse(b"%PDF-1.7\n1 0 obj << /Type /Catalog >> endobj\n").unwrap();
    assert_eq!(doc.xref_kind(), XrefKind::Rebuilt);
    assert_eq!(doc.pages().err(), Some(Error::Missing));
    let pdf = one_page().classic();
    for cut in (0..pdf.len()).step_by(17) {
        let _ = Document::parse(&pdf[..cut]).and_then(|d| d.pages());
    }
}
