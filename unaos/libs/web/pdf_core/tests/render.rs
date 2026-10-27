// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! PDF2 (LEDGER SR313) KATs: hand-built one-page PDFs (40 x 40 pt, rendered at 72 dpi, so device y = 40 - user y)
//! through `Page::render`. Each pins an FNV-1a 64 of the RGBA (a regression pin taken from this renderer's own output)
//! AND sampled pixels whose values follow from the geometry and the colour arithmetic (the real check).

mod common;

use common::PdfBuilder;
use pdf_core::Document;
use pdf_core::render::function::{Function, compile};
use pdf_core::render::{Rendered, TextLog};

fn fnv(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// A one-page PDF: the page's /Resources body, its content, and extra objects numbered from 5.
fn pdf(media: &str, extra_page: &str, resources: &str, content: &str, extra: &[(u32, &str, Option<Vec<u8>>)]) -> Vec<u8> {
    let mut b = PdfBuilder::new(1);
    b.obj(1, "<< /Type /Catalog /Pages 2 0 R >>");
    b.obj(2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.obj(3, &format!("<< /Type /Page /Parent 2 0 R /MediaBox [{media}] {extra_page} /Resources << {resources} >> /Contents 4 0 R >>"));
    b.stream(4, "", content.as_bytes());
    for (n, d, data) in extra {
        match data {
            Some(bytes) => b.stream(*n, d, bytes),
            None => b.obj(*n, d),
        };
    }
    b.classic()
}

fn render_pdf(bytes: &[u8]) -> (Rendered, TextLog) {
    let doc = Document::parse(bytes).unwrap();
    let pages = doc.pages().unwrap();
    let mut log = TextLog::default();
    let r = pages[0].render(&doc, 72.0, &mut log).unwrap();
    (r, log)
}

fn render(resources: &str, content: &str, extra: &[(u32, &str, Option<Vec<u8>>)]) -> Rendered {
    render_pdf(&pdf("0 0 40 40", "", resources, content, extra)).0
}

fn px(r: &Rendered, x: usize, y: usize) -> [u8; 4] {
    let p = &r.pixmap;
    let i = (y * p.w + x) * 4;
    [p.data[i], p.data[i + 1], p.data[i + 2], p.data[i + 3]]
}

const WHITE: [u8; 4] = [255, 255, 255, 255];

fn pin(r: &Rendered, want: u64) {
    let got = fnv(&r.pixmap.data);
    assert_eq!(got, want, "fnv pin: got {got:#018x}");
}

#[test]
fn filled_rect_nonzero() {
    let r = render("", "1 0 0 rg 10 10 20 20 re f", &[]);
    assert_eq!((r.pixmap.w, r.pixmap.h), (40, 40));
    assert_eq!(px(&r, 20, 20), [255, 0, 0, 255]);
    assert_eq!(px(&r, 10, 10), [255, 0, 0, 255]);
    assert_eq!(px(&r, 29, 29), [255, 0, 0, 255]);
    assert_eq!(px(&r, 9, 20), WHITE);
    assert_eq!(px(&r, 30, 20), WHITE);
    assert!(r.refused.is_empty());
    pin(&r, 0x28017ce0ddb06445);
}

#[test]
fn even_odd_ring() {
    let r = render("", "0 0 1 rg 5 5 30 30 re 15 15 10 10 re f*", &[]);
    assert_eq!(px(&r, 20, 20), WHITE, "the hole");
    assert_eq!(px(&r, 8, 20), [0, 0, 255, 255]);
    assert_eq!(px(&r, 2, 20), WHITE);
    pin(&r, 0xd449665ef7881365);
    // The same two rectangles, same direction, nonzero: no hole.
    let r = render("", "0 0 1 rg 5 5 30 30 re 15 15 10 10 re f", &[]);
    assert_eq!(px(&r, 20, 20), [0, 0, 255, 255]);
}

#[test]
fn stroked_dashed_line() {
    // 4 pt wide, butt caps, dashes 6 on 4 off along y = 20 (device rows 18..22).
    let r = render("", "0 g 4 w [6 4] 0 d 0 20 m 40 20 l S", &[]);
    assert_eq!(px(&r, 3, 20), [0, 0, 0, 255]);
    assert_eq!(px(&r, 8, 20), WHITE, "a gap");
    assert_eq!(px(&r, 13, 19), [0, 0, 0, 255]);
    assert_eq!(px(&r, 3, 16), WHITE, "outside the width");
    pin(&r, 0x06bba0e1093686a5);
}

#[test]
fn clipped_fill() {
    let r = render("", "10 10 10 10 re W n 0 1 0 rg 0 0 40 40 re f", &[]);
    assert_eq!(px(&r, 15, 25), [0, 255, 0, 255]);
    assert_eq!(px(&r, 25, 25), WHITE);
    assert_eq!(px(&r, 15, 15), WHITE);
    pin(&r, 0x49c81f440afdb895);
}

#[test]
fn form_xobject_matrix_and_bbox() {
    // The form paints 0..10 in form space, clipped to its /BBox 0..5, under [2 0 0 2 10 10]: user 10..20.
    let r = render(
        "/XObject << /Fm1 5 0 R >>",
        "/Fm1 Do",
        &[(5, "/Type /XObject /Subtype /Form /BBox [0 0 5 5] /Matrix [2 0 0 2 10 10]", Some(b"1 0 0 rg 0 0 10 10 re f".to_vec()))],
    );
    assert_eq!(px(&r, 15, 25), [255, 0, 0, 255]);
    assert_eq!(px(&r, 25, 25), WHITE, "outside the BBox");
    assert_eq!(px(&r, 15, 15), WHITE, "outside the BBox");
    pin(&r, 0x89d2daf93e6f412d);
}

#[test]
fn rgb_flate_image() {
    let samples = [255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255];
    let r = render(
        "/XObject << /Im1 5 0 R >>",
        "q 20 0 0 20 10 10 cm /Im1 Do Q",
        &[(
            5,
            "/Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode",
            Some(common::zlib_stored(&samples)),
        )],
    );
    // Row 0 is the top of the image.
    assert_eq!(px(&r, 15, 15), [255, 0, 0, 255]);
    assert_eq!(px(&r, 25, 15), [0, 255, 0, 255]);
    assert_eq!(px(&r, 15, 25), [0, 0, 255, 255]);
    assert_eq!(px(&r, 25, 25), WHITE);
    assert_eq!(px(&r, 5, 5), WHITE);
    pin(&r, 0xbe1d5255526d4f05);
}

#[test]
fn image_mask_stencil() {
    // 8 x 1, samples 0000 1111: a 0 paints (Decode [0 1]) in the fill colour.
    let r = render(
        "/XObject << /Im1 5 0 R >>",
        "0 0 1 rg q 16 0 0 4 10 10 cm /Im1 Do Q",
        &[(5, "/Type /XObject /Subtype /Image /Width 8 /Height 1 /ImageMask true /BitsPerComponent 1", Some(vec![0b0000_1111]))],
    );
    assert_eq!(px(&r, 11, 28), [0, 0, 255, 255]);
    assert_eq!(px(&r, 17, 28), [0, 0, 255, 255]);
    assert_eq!(px(&r, 20, 28), WHITE);
    assert_eq!(px(&r, 11, 24), WHITE);
    pin(&r, 0x8db37fe21c81db65);
}

/// A hand-assembled 8 x 8 baseline greyscale JPEG (T.81): all-ones quantizer, a DC table whose one code `0` is
/// category 7 and an AC table whose one code `0` is EOB; the scan is DC diff -64 (bits 0 0111111) then EOB (0), padded
/// with ones: 0x3F 0x7F. The block's DC is -64, so every sample is 128 + (-64 / 8) = 120.
fn tiny_jpeg() -> Vec<u8> {
    let mut j = vec![0xFF, 0xD8];
    j.extend_from_slice(&[0xFF, 0xDB, 0x00, 0x43, 0x00]);
    j.extend_from_slice(&[1u8; 64]);
    j.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x0B, 0x08, 0x00, 0x08, 0x00, 0x08, 0x01, 0x01, 0x11, 0x00]);
    for (class, sym) in [(0x00u8, 0x07u8), (0x10, 0x00)] {
        j.extend_from_slice(&[0xFF, 0xC4, 0x00, 0x14, class, 1]);
        j.extend_from_slice(&[0u8; 15]);
        j.push(sym);
    }
    j.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x08, 0x01, 0x01, 0x00, 0x00, 0x3F, 0x00]);
    j.extend_from_slice(&[0x3F, 0x7F]);
    j.extend_from_slice(&[0xFF, 0xD9]);
    j
}

#[test]
fn dct_image() {
    let jpeg = tiny_jpeg();
    let img = pixel_core::jpeg::decode(&jpeg).expect("the hand-built JPEG decodes");
    assert_eq!((img.width, img.height), (8, 8));
    assert_eq!(&img.rgba[0..4], &[120, 120, 120, 255]);
    let r = render(
        "/XObject << /Im1 5 0 R >>",
        "q 8 0 0 8 0 0 cm /Im1 Do Q",
        &[(5, "/Type /XObject /Subtype /Image /Width 8 /Height 8 /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /DCTDecode", Some(jpeg))],
    );
    assert_eq!(px(&r, 4, 36), [120, 120, 120, 255]);
    assert_eq!(px(&r, 4, 30), WHITE);
    assert!(r.refused.is_empty());
    pin(&r, 0xa783c8f446877125);
}

#[test]
fn axial_shading() {
    let sh = |ext: &str| {
        format!("<< /ShadingType 2 /ColorSpace /DeviceRGB /Coords [10 0 30 0] {ext} /Function << /FunctionType 2 /Domain [0 1] /C0 [1 0 0] /C1 [0 0 1] /N 1 >> >>")
    };
    let plain = sh("");
    let r = render("/Shading << /Sh1 5 0 R >>", "/Sh1 sh", &[(5, &plain, None)]);
    assert_eq!(px(&r, 5, 20), WHITE, "no Extend: nothing before the start");
    assert_eq!(px(&r, 35, 20), WHITE, "no Extend: nothing past the end");
    // x = 20.5 -> t = 0.525: red 0.475, blue 0.525 (64 sampled stops: within a few levels).
    let m = px(&r, 20, 20);
    assert!((110..=135).contains(&m[0]) && m[1] == 0 && (120..=145).contains(&m[2]), "{m:?}");
    // x = 11.5 -> t = 0.075: red 0.925 x 255 = 236, blue 19.
    let a = px(&r, 11, 20);
    assert!((233..=239).contains(&a[0]) && (16..=22).contains(&a[2]), "{a:?}");
    pin(&r, 0x4ae6b7b9579435e5);
    let ext = sh("/Extend [true true]");
    let r = render("/Shading << /Sh1 5 0 R >>", "/Sh1 sh", &[(5, &ext, None)]);
    assert_eq!(px(&r, 5, 20), [255, 0, 0, 255]);
    assert_eq!(px(&r, 35, 20), [0, 0, 255, 255]);
}

#[test]
fn cmyk_fill() {
    // Naive CMYK: R = (1-C)(1-K), G = (1-M)(1-K), B = (1-Y)(1-K); 0 1 1 0 is red.
    let r = render("", "0 1 1 0 k 10 10 20 20 re f 0 0 0 0.5 k 0 0 5 5 re f", &[]);
    assert_eq!(px(&r, 20, 20), [255, 0, 0, 255]);
    assert_eq!(px(&r, 2, 37), [128, 128, 128, 255]);
    pin(&r, 0x56954d03d04399cc);
}

#[test]
fn separation_type4_tint() {
    // tint t -> RGB (t, 1-t, 0) through a PostScript calculator function.
    let r = render(
        "/ColorSpace << /CS1 [/Separation /Spot /DeviceRGB 5 0 R] >>",
        "/CS1 cs 0.25 scn 10 10 20 20 re f",
        &[(5, "/FunctionType 4 /Domain [0 1] /Range [0 1 0 1 0 1]", Some(b"{ dup 1 exch sub 0 }".to_vec()))],
    );
    assert_eq!(px(&r, 20, 20), [64, 191, 0, 255]);
    pin(&r, 0x4289ea9707d39fe5);
}

#[test]
fn text_goes_to_the_sink() {
    let bytes = pdf(
        "0 0 40 40",
        "",
        "/Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >>",
        "BT /F1 12 Tf 5 5 Td (Hi) Tj [(A) -500 (B)] TJ ET",
        &[],
    );
    let (r, log) = render_pdf(&bytes);
    assert_eq!(log.runs.len(), 3);
    assert_eq!(log.runs[0].font_name, b"F1");
    assert_eq!(log.runs[0].size, 12.0);
    assert_eq!((log.runs[0].tm.e, log.runs[0].tm.f), (5.0, 5.0));
    assert_eq!(log.runs[0].bytes, b"Hi");
    // The recorder advances nothing; the TJ number moves the pen 500/1000 x 12 = 6 to the right.
    assert_eq!(log.runs[2].bytes, b"B");
    assert_eq!(log.runs[2].tm.e, 11.0);
    assert!(r.pixmap.data.iter().all(|&b| b == 255), "this arc draws nothing for text");
}

#[test]
fn jpx_refused_grey_box() {
    let r = render(
        "/XObject << /Im1 5 0 R >>",
        "q 10 0 0 10 10 10 cm /Im1 Do Q",
        &[(5, "/Type /XObject /Subtype /Image /Width 4 /Height 4 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /JPXDecode", Some(vec![0, 0, 0, 12]))],
    );
    assert_eq!(px(&r, 15, 25), [128, 128, 128, 255]);
    assert_eq!(px(&r, 25, 25), WHITE);
    assert_eq!(r.refused, vec!["JPXDecode"]);
}

#[test]
fn rotate_90() {
    // Rotate 90: the page turns clockwise; the 40 x 20 media box renders 20 wide, 40 tall, its lower-left at the top-left.
    let (r, _) = render_pdf(&pdf("0 0 40 20", "/Rotate 90", "", "1 0 0 rg 0 0 10 10 re f", &[]));
    assert_eq!((r.pixmap.w, r.pixmap.h), (20, 40));
    assert_eq!(px(&r, 5, 5), [255, 0, 0, 255]);
    assert_eq!(px(&r, 15, 5), WHITE);
    assert_eq!(px(&r, 5, 15), WHITE);
}

fn calc(src: &[u8], domain: Vec<f64>, range: Vec<f64>) -> Function {
    Function::Calculator { domain, range, program: compile(src).unwrap() }
}

#[test]
fn type4_calculator() {
    // min of two inputs.
    let f = calc(b"{ 2 copy gt { exch } if pop }", vec![0.0, 1.0, 0.0, 1.0], vec![0.0, 1.0]);
    assert_eq!(f.eval(&[0.7, 0.3]), vec![0.3]);
    assert_eq!(f.eval(&[0.2, 0.9]), vec![0.2]);
    let f = calc(b"{ 0.5 gt { 1 } { 0 } ifelse }", vec![0.0, 1.0], vec![0.0, 1.0]);
    assert_eq!(f.eval(&[0.7]), vec![1.0]);
    assert_eq!(f.eval(&[0.2]), vec![0.0]);
    let f = calc(b"{ pop 1 2 3 3 1 roll }", vec![0.0, 1.0], vec![0.0, 9.0, 0.0, 9.0, 0.0, 9.0]);
    assert_eq!(f.eval(&[0.5]), vec![3.0, 1.0, 2.0]);
    let f = calc(b"{ 2 index 1 index add exch pop exch pop 5 3 idiv add 7 3 mod add }", vec![0.0, 9.0, 0.0, 9.0], vec![0.0, 99.0]);
    // stack: x y -> 2 index on [x y] is out of range -> the error result (the range's clipped zeros).
    assert_eq!(f.eval(&[1.0, 2.0]), vec![0.0]);
    let f = calc(b"{ 4 bitshift 3 and 90 sin add }", vec![0.0, 99.0], vec![0.0, 99.0]);
    assert_eq!(f.eval(&[5.0]), vec![0.0], "real input: bitshift needs an int -> the error result");
    let f = calc(b"{ cvi 4 bitshift 1 or 90 sin add }", vec![0.0, 99.0], vec![0.0, 999.0]);
    assert_eq!(f.eval(&[5.0]), vec![82.0]);
}
