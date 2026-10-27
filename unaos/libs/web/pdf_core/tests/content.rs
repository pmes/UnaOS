// SPDX-License-Identifier: LGPL-3.0-or-later
//! PDF1 (SR300): the content-stream tokenizer over a page with inline images.

mod common;

use common::PdfBuilder;
use pdf_core::content::ops;
use pdf_core::{Document, Object};

#[test]
fn operands_and_operators() {
    let o = ops(b"q 1 0 0 1 72 72 cm /F1 12 Tf [(A) -120 (B)] TJ 0.5 g true null Q");
    let names: Vec<&[u8]> = o.iter().map(|o| o.operator.as_slice()).collect();
    assert_eq!(names, [&b"q"[..], b"cm", b"Tf", b"TJ", b"g", b"Q"]);
    assert_eq!(o[1].operands.len(), 6);
    assert_eq!(o[2].operands[0], Object::Name(b"F1".to_vec()));
    assert_eq!(o[3].operands[0].as_array().unwrap().len(), 3);
    assert_eq!(o[5].operands, [Object::Bool(true), Object::Null]);
}

#[test]
fn inline_image_ei_rule() {
    // The data holds `EI` glued to other bytes, then ` EI ` followed by binary: neither ends the image.
    let mut page = b"q BI /W 4 /H 1 /BPC 8 /CS /G ID xEIy EI \xff\xfe\x01 EI Q".to_vec();
    page.extend_from_slice(b" 0 0 m");
    let o = ops(&page);
    let names: Vec<&[u8]> = o.iter().map(|o| o.operator.as_slice()).collect();
    assert_eq!(names, [&b"q"[..], b"BI", b"Q", b"m"]);
    let d = o[1].operands[0].as_dict().unwrap();
    assert_eq!(d.int(b"W"), Some(4));
    assert_eq!(d.name(b"CS"), Some(&b"G"[..]));
    assert_eq!(o[1].operands[1], Object::String(b"xEIy EI \xff\xfe\x01".to_vec()));
}

#[test]
fn inline_image_with_length() {
    let o = ops(b"BI /W 2 /H 1 /L 4 ID \nEI\n EI Q");
    assert_eq!(o[0].operator, b"BI");
    assert_eq!(o[0].operands[1], Object::String(b"\nEI\n".to_vec()));
    assert_eq!(o[1].operator, b"Q");
}

#[test]
fn a_page_with_an_inline_image() {
    let content = b"q 8 0 0 8 0 0 cm BI /W 2 /H 2 /BPC 8 /CS /G /F /AHx ID 00FFFF00> EI Q";
    let mut b = PdfBuilder::new(1);
    b.obj(1, "<< /Type /Catalog /Pages 2 0 R >>")
        .obj(2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
        .obj(3, "<< /Type /Page /Contents 4 0 R >>")
        .stream(4, "/Filter /FlateDecode", &common::zlib_stored(content));
    let doc = Document::parse(&b.compressed()).unwrap();
    let p = doc.pages().unwrap();
    let o = ops(&p[0].contents);
    assert_eq!(o.len(), 4);
    assert_eq!(o[2].operands[1], Object::String(b"00FFFF00>".to_vec()));
}
