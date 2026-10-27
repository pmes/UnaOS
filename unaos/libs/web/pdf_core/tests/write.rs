// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! PDF-WRITE (LEDGER SR325) KATs: `Document::save` round trips over every xref shape PDF1 reads (saved -> parsed gives
//! the same pages, boxes, resources, ops and /Info; saving the saved file is a fixed point), the serializer's syntax,
//! `Edit::save_update` (the original bytes a prefix, the new /Info read back), the Builder and `from_rgba` rendered
//! through PDF2's `Page::render`, and pixel_core's compressor inflating back. The PDFCRYPT, PDF2 and PDF3 suites
//! round-trip their own fixtures through their helpers (`saved_reads_back`, `assert_saved_renders_same`).
//! PDF-WRITE-FONTS (SR338): `PageBuilder::text` in the tree's JetBrainsMono-Variable.ttf, subset and read back by PDF3.

mod common;

use common::{Body, PdfBuilder, incremental, replace_bytes, startxref_of, zlib_stored};
use pdf_core::content::ops;
use pdf_core::font::{Fallback, GlyphSink, spans_to_string};
use pdf_core::lexer::Lexer;
use pdf_core::object::parse_object;
use pdf_core::render::{Rendered, TextLog};
use pdf_core::write::{Builder, Edit, Image, PageBuilder, RgbaPage, from_rgba, write_object};
use pdf_core::{Document, Error, Object, Ref, XrefKind};
use svg_core::geom::{Path, Seg};

/// PDF1's inheritance tree (tests/doc.rs) with an /Info dictionary.
fn tree() -> PdfBuilder {
    let mut b = PdfBuilder::new(1);
    b.obj(1, "<< /Type /Catalog /Pages 2 0 R >>")
        .obj(2, "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /Resources << /Font << /F1 7 0 R >> >> /MediaBox [0 0 500 400] /Rotate 90 >>")
        .obj(3, "<< /Type /Pages /Parent 2 0 R /Kids [5 0 R] /Count 1 /CropBox [10 10 490 390] >>")
        .obj(4, "<< /Type /Page /Parent 2 0 R /MediaBox [100 100 0 0] /Rotate -90 /Resources 8 0 R >>")
        .obj(5, "<< /Type /Page /Parent 3 0 R /Contents [9 0 R 10 0 R] >>")
        .obj(6, "<< /Title (Inheritance \\(two\\) levels) /Author <FEFF00C9> >>")
        .obj(7, "<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>")
        .obj(8, "<< /XObject << >> >>")
        .stream(9, "", b"0.5 0 0 RG 1 0 0 1 72.25 -3 cm q")
        .stream(10, "/Filter /AHx", b"51>");
    b.info = Some(6);
    b
}

fn same(a: &Document, b: &Document, what: &str) {
    let (pa, pb) = (a.pages().unwrap(), b.pages().unwrap());
    assert_eq!(pa, pb, "{what}: pages");
    for (p, q) in pa.iter().zip(&pb) {
        assert_eq!(ops(&p.contents), ops(&q.contents), "{what}: ops");
    }
    assert_eq!(a.info(), b.info(), "{what}: info");
}

#[test]
fn save_round_trips_every_xref_shape() {
    let b = tree();
    let classic = b.classic();
    let update = incremental(
        &classic,
        &[(4, Body::Plain(b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 50 60] /Rotate 180 >>".to_vec()))],
        1,
        11,
    );
    let off = startxref_of(&classic);
    let broken = replace_bytes(&classic, format!("startxref\n{off}\n").as_bytes(), b"startxref\n7\n");
    assert_eq!(Document::parse(&broken).unwrap().xref_kind(), XrefKind::Rebuilt);
    for (what, pdf) in [("classic", classic.clone()), ("compressed", b.compressed()), ("hybrid", b.hybrid()), ("update", update), ("rebuilt", broken)] {
        let doc = Document::parse(&pdf).unwrap();
        let out = doc.save();
        let back = Document::parse(&out).unwrap_or_else(|e| panic!("{what}: {e}"));
        assert_eq!(back.xref_kind(), XrefKind::Table, "{what}");
        assert!(out.starts_with(b"%PDF-1.4\n"), "{what}");
        assert!(!back.trailer().contains(b"Encrypt"));
        same(&doc, &back, what);
        // Saving the saved file changes nothing.
        assert!(back.save() == out, "{what}: not a fixed point");
    }
}

#[test]
fn save_drops_the_unreachable_and_object_streams() {
    let mut b = tree();
    b.obj(20, "<< /Orphan true >>");
    let doc = Document::parse(&b.compressed()).unwrap();
    let back = Document::parse(&doc.save()).unwrap();
    assert!(back.resolve(Ref { num: 20, generation: 0 }).unwrap().is_null());
    // The object-stream members are top-level objects now; no /ObjStm or /XRef stream survives.
    let out = doc.save();
    let text = String::from_utf8_lossy(&out);
    assert!(!text.contains("/ObjStm") && !text.contains("/XRef"));
    assert_eq!(back.resolve(Ref { num: 7, generation: 0 }).unwrap().as_dict().unwrap().name(b"BaseFont"), Some(&b"Courier"[..]));
}

#[test]
fn crypt_filters_are_stripped_and_length_made_direct() {
    let mut b = PdfBuilder::new(1);
    b.obj(1, "<< /Type /Catalog /Pages 2 0 R /A 4 0 R /B 5 0 R >>")
        .obj(2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
        .obj(3, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] >>")
        .stream(4, "/Filter /Crypt /DecodeParms << /Name /Identity >>", b"clear")
        .stream(5, "/Filter [/Crypt /AHx] /DecodeParms [<< /Name /Identity >> null]", b"414243>");
    let doc = Document::parse(&b.classic()).unwrap();
    let back = Document::parse(&doc.save()).unwrap();
    let s4 = back.resolve(Ref { num: 4, generation: 0 }).unwrap();
    let s4 = s4.as_stream().unwrap();
    assert!(!s4.dict.contains(b"Filter") && !s4.dict.contains(b"DecodeParms"));
    assert_eq!(s4.dict.get(b"Length"), Some(&Object::Int(5)));
    let s5 = back.resolve(Ref { num: 5, generation: 0 }).unwrap();
    let s5 = s5.as_stream().unwrap();
    assert_eq!(s5.dict.get(b"Filter"), Some(&Object::Array(vec![Object::Name(b"AHx".to_vec())])));
    assert!(!s5.dict.contains(b"DecodeParms"));
    assert_eq!(back.decode(s5).unwrap().data, b"ABC");
}

fn reparse(o: &Object) -> (Vec<u8>, Object) {
    let mut v = Vec::new();
    write_object(&mut v, o);
    let mut lx = Lexer::new(&v);
    let back = parse_object(&mut lx, 0).unwrap();
    (v, back)
}

#[test]
fn serializer_syntax_reads_back() {
    let cases = [
        Object::Null,
        Object::Bool(true),
        Object::Int(-7),
        Object::Real(1.0),
        Object::Real(-0.25),
        Object::Real(595.276),
        Object::Real(1e-5),
        Object::String(b"a(b)c\\d\re\nf".to_vec()),
        Object::String(vec![0, 1, 2, 255, 0xFE]),
        Object::Name(b"A B#/x\x80".to_vec()),
        Object::Array(vec![Object::Ref(Ref { num: 12, generation: 3 }), Object::Array(vec![]), Object::Int(0)]),
    ];
    for o in &cases {
        let (bytes, back) = reparse(o);
        assert_eq!(&back, o, "{}", String::from_utf8_lossy(&bytes));
    }
    assert_eq!(reparse(&Object::Real(1.0)).0, b"1.0");
    assert_eq!(reparse(&Object::Real(1e-5)).0, b"0.00001");
    assert_eq!(reparse(&Object::Name(b"A B#/x".to_vec())).0, b"/A#20B#23#2Fx");
    assert_eq!(reparse(&Object::String(vec![0, 1, 2, 255])).0, b"<000102FF>");
    assert_eq!(reparse(&Object::String(b"a(b)\r".to_vec())).0, b"(a\\(b\\)\\r)");
    let mut d = pdf_core::Dict::new();
    d.insert(b"Type".to_vec(), Object::Name(b"Page".to_vec()));
    d.insert(b"Kids".to_vec(), Object::Array(vec![Object::Ref(Ref { num: 4, generation: 0 })]));
    let (bytes, back) = reparse(&Object::Dict(d.clone()));
    assert_eq!(bytes, b"<</Type /Page /Kids [4 0 R] >>");
    assert_eq!(back, Object::Dict(d));
}

#[test]
fn save_update_appends_an_edited_info() {
    for pdf in [tree().classic(), tree().compressed()] {
        let doc = Document::parse(&pdf).unwrap();
        let mut e = Edit::new(&doc);
        e.set_info(b"Title", "Saved \u{dc}naOS");
        let out = e.save_update(&pdf).unwrap();
        assert!(out.starts_with(&pdf));
        let back = Document::parse(&out).unwrap();
        assert_eq!(back.info().title.as_deref(), Some("Saved \u{dc}naOS"));
        assert_eq!(back.info().author.as_deref(), Some("\u{c9}"));
        assert_eq!(doc.pages().unwrap(), back.pages().unwrap());
        // The appended section is a classic table whose /Prev is the original's startxref.
        let tail = String::from_utf8_lossy(&out[pdf.len()..]).into_owned();
        assert!(tail.contains("xref\n6 1\n"), "{tail}");
        assert!(tail.contains(&format!("/Prev {}", startxref_of(&pdf))), "{tail}");
        // The same edit saved in full.
        let full = Document::parse(&e.save()).unwrap();
        assert_eq!(full.info().title.as_deref(), Some("Saved \u{dc}naOS"));
        // No edit: nothing appended.
        assert!(Edit::new(&doc).save_update(&pdf).unwrap() == pdf);
    }
}

#[test]
fn save_update_adds_a_missing_info_and_refuses_a_rebuilt_xref() {
    let mut b = tree();
    b.info = None;
    b.objs.remove(&6);
    let pdf = b.classic();
    let doc = Document::parse(&pdf).unwrap();
    let mut e = Edit::new(&doc);
    e.set_info(b"Title", "Fresh");
    let out = e.save_update(&pdf).unwrap();
    let back = Document::parse(&out).unwrap();
    assert_eq!(back.info().title.as_deref(), Some("Fresh"));
    assert_eq!(back.trailer().get(b"Info"), Some(&Object::Ref(Ref { num: 11, generation: 0 })));
    let off = startxref_of(&pdf);
    let broken = replace_bytes(&pdf, format!("startxref\n{off}\n").as_bytes(), b"startxref\n7\n");
    let doc = Document::parse(&broken).unwrap();
    assert!(matches!(Edit::new(&doc).save_update(&broken), Err(Error::Unsupported(_))));
}

fn render(pdf: &[u8], dpi: f64) -> Rendered {
    let doc = Document::parse(pdf).unwrap();
    let pages = doc.pages().unwrap();
    pages[0].render(&doc, dpi, &mut TextLog::default()).unwrap()
}

fn px(r: &Rendered, x: usize, y: usize) -> [u8; 4] {
    let i = (y * r.pixmap.w + x) * 4;
    r.pixmap.data[i..i + 4].try_into().unwrap()
}

fn fnv(data: &[u8]) -> u64 {
    data.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3))
}

#[test]
fn pdf2_page_renders_the_same_after_save() {
    let samples = [255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255];
    let mut b = PdfBuilder::new(1);
    b.obj(1, "<< /Type /Catalog /Pages 2 0 R >>")
        .obj(2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
        .obj(3, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 40 40] /Resources << /XObject << /Im1 5 0 R /Fm 6 0 R >> >> /Contents 4 0 R >>")
        .stream(4, "", b"q 20 0 0 20 10 10 cm /Im1 Do Q /Fm Do")
        .stream(5, "/Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode", &zlib_stored(&samples))
        .stream(6, "/Type /XObject /Subtype /Form /BBox [0 0 40 40] /Matrix [1 0 0 1 2 2]", b"0 0 1 rg 0 0 5 5 re f");
    let pdf = b.classic();
    let before = render(&pdf, 72.0);
    let after = render(&Document::parse(&pdf).unwrap().save(), 72.0);
    assert_eq!(px(&before, 15, 15), [255, 0, 0, 255]);
    assert_eq!(px(&before, 4, 35), [0, 0, 255, 255]);
    assert!(before.pixmap.data == after.pixmap.data);
}

#[test]
fn builder_page_renders_to_the_pinned_pixels() {
    let mut p = PageBuilder::new(40.0, 40.0);
    p.fill_rgb(1.0, 0.0, 0.0).rect(10.0, 10.0, 20.0, 20.0).fill();
    // A quadratic "glyph" (text is drawn as paths): (2,2) -> control (5,8) -> (8,2), peak y = 5 at x = 5.
    p.save().fill_gray(0.0);
    p.path(&Path { segs: vec![Seg::Move((2.0, 2.0)), Seg::Quad((5.0, 8.0), (8.0, 2.0)), Seg::Close] });
    p.fill().restore();
    p.stroke_rgb(0.0, 0.0, 1.0).line_width(2.0).move_to(32.0, 2.0).line_to(32.0, 38.0).stroke();
    let mut b = Builder::new();
    b.title("Builder page").page(p);
    let pdf = b.finish();
    let doc = Document::parse(&pdf).unwrap();
    assert_eq!(doc.version(), "1.4");
    assert_eq!(doc.xref_kind(), XrefKind::Table);
    assert_eq!(doc.info().title.as_deref(), Some("Builder page"));
    let pages = doc.pages().unwrap();
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0].media_box, [0.0, 0.0, 40.0, 40.0]);
    let r = render(&pdf, 72.0);
    assert_eq!((r.pixmap.w, r.pixmap.h), (40, 40));
    assert!(r.refused.is_empty());
    // Device y = 40 - user y: the red square, the paper, the quad's inside, the blue rule.
    assert_eq!(px(&r, 20, 20), [255, 0, 0, 255]);
    assert_eq!(px(&r, 5, 5), [255, 255, 255, 255]);
    assert_eq!(px(&r, 5, 36), [0, 0, 0, 255]);
    assert_eq!(px(&r, 1, 36), [255, 255, 255, 255]);
    assert_eq!(px(&r, 32, 20), [0, 0, 255, 255]);
    let got = fnv(&r.pixmap.data);
    assert_eq!(got, PIN_BUILDER, "fnv pin: got {got:#018x}");
}

/// A regression pin taken from this renderer's own output (the sampled pixels above are the real check).
const PIN_BUILDER: u64 = 0x9ba2_67e4_8af5_0665;

#[test]
fn builder_images_jpeg_passthrough_and_length_check() {
    let jpeg = b"\xFF\xD8not really a jpeg\xFF\xD9".to_vec();
    let mut p = PageBuilder::new(10.0, 10.0);
    p.image(Image::Jpeg { width: 2, height: 2, components: 3, data: jpeg.clone() }, [10.0, 0.0, 0.0, 10.0, 0.0, 0.0]).unwrap();
    p.image(Image::Gray { width: 2, height: 1, data: vec![0, 255] }, [10.0, 0.0, 0.0, 5.0, 0.0, 0.0]).unwrap();
    assert!(matches!(p.image(Image::Rgb { width: 2, height: 2, data: vec![0; 11] }, [1.0; 6]), Err(Error::Unsupported(_))));
    let mut b = Builder::new();
    b.page(p);
    let doc = Document::parse(&b.finish()).unwrap();
    let page = &doc.pages().unwrap()[0];
    let xo = doc.get(page.resources.get(b"XObject").unwrap());
    let xo = xo.as_dict().unwrap();
    let im0 = doc.get(xo.get(b"Im0").unwrap());
    let im0 = im0.as_stream().unwrap();
    assert_eq!(im0.dict.name(b"Filter"), Some(&b"DCTDecode"[..]));
    assert_eq!(im0.data, jpeg);
    let im1 = doc.get(xo.get(b"Im1").unwrap());
    let im1 = im1.as_stream().unwrap();
    assert_eq!(im1.dict.name(b"Filter"), Some(&b"FlateDecode"[..]));
    assert_eq!(im1.dict.name(b"ColorSpace"), Some(&b"DeviceGray"[..]));
    assert_eq!(doc.decode(im1).unwrap().data, vec![0, 255]);
    assert_eq!(ops(&page.contents).iter().filter(|o| o.operator == b"Do").count(), 2);
}

#[test]
fn from_rgba_prints_what_you_see() {
    let mut rgba = Vec::new();
    for i in 0..12u8 {
        rgba.extend_from_slice(&[i * 20, 255 - i * 20, ((i as u32 * 37) % 256) as u8, if i == 5 { 128 } else if i == 6 { 0 } else { 255 }]);
    }
    let over_white = |px: &[u8]| -> [u8; 4] {
        let a = px[3] as u32;
        let c = |v: u8| ((v as u32 * a + 255 * (255 - a) + 127) / 255) as u8;
        [c(px[0]), c(px[1]), c(px[2]), 255]
    };
    let pdf = from_rgba(&[RgbaPage { width: 4, height: 3, rgba: &rgba }], 72.0).unwrap();
    let doc = Document::parse(&pdf).unwrap();
    assert_eq!(doc.pages().unwrap()[0].media_box, [0.0, 0.0, 4.0, 3.0]);
    let r = render(&pdf, 72.0);
    assert_eq!((r.pixmap.w, r.pixmap.h), (4, 3));
    for y in 0..3 {
        for x in 0..4 {
            let i = (y * 4 + x) * 4;
            assert_eq!(px(&r, x, y), over_white(&rgba[i..i + 4]), "pixel {x},{y}");
        }
    }
    let two = from_rgba(&[RgbaPage { width: 4, height: 3, rgba: &rgba }, RgbaPage { width: 2, height: 2, rgba: &rgba[..16] }], 144.0).unwrap();
    let pages = Document::parse(&two).unwrap().pages().unwrap();
    assert_eq!(pages.len(), 2);
    assert_eq!(pages[0].media_box, [0.0, 0.0, 2.0, 1.5]);
    assert_eq!(pages[1].media_box, [0.0, 0.0, 1.0, 1.0]);
    assert!(matches!(from_rgba(&[RgbaPage { width: 4, height: 3, rgba: &rgba[..40] }], 72.0), Err(Error::Unsupported(_))));
    assert!(matches!(from_rgba(&[], 0.0), Err(Error::Unsupported(_))));
}

#[test]
fn pixel_core_deflate_inflates_back() {
    let long: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8 ^ (i / 1000) as u8).collect();
    for data in [Vec::new(), b"hello hello hello hello".to_vec(), long] {
        let z = pixel_core::deflate::compress(&data).unwrap();
        assert_eq!(&z[..2], &[0x78, 0x01]);
        assert!(pdf_core::filters::flate(&z).unwrap() == data);
    }
}

// ------------------------------------------------------------------------------------------- PDF-WRITE-FONTS (SR338)

fn font_file(rel: &str) -> Vec<u8> {
    std::fs::read(format!("{}/../../../../fonts/{rel}", env!("CARGO_MANIFEST_DIR"))).expect("bundled font")
}

const JBM: &str = "jetbrains-mono/JetBrainsMono-Variable.ttf";

/// Page 0 through PDF2 with PDF3's GlyphSink at 72 dpi: the ink (255 - red) and the extracted text.
fn glyph_render(pdf: &[u8]) -> (Vec<u8>, String) {
    let doc = Document::parse(pdf).unwrap();
    let page = &doc.pages().unwrap()[0];
    let fb = Fallback::default();
    let mut sink = GlyphSink::new(&doc, fb);
    let r = page.render(&doc, 72.0, &mut sink).unwrap();
    assert!(r.refused.is_empty(), "{:?}", r.refused);
    let cov = r.pixmap.data.chunks_exact(4).map(|p| 255 - p[0]).collect();
    (cov, spans_to_string(&page.text(&doc, &fb)))
}

fn dict_at(doc: &Document, o: &Object) -> pdf_core::Dict {
    doc.get(o).as_dict().unwrap().clone()
}

/// The font program, the Type0 dict and the CIDFont dict behind a page's `/F{id}`.
fn embedded(doc: &Document, page: usize, key: &[u8]) -> (Vec<u8>, pdf_core::Dict, pdf_core::Dict) {
    let pg = &doc.pages().unwrap()[page];
    let fonts = dict_at(doc, pg.resources.get(b"Font").unwrap());
    let t0 = dict_at(doc, fonts.get(key).unwrap());
    let kids = doc.get(t0.get(b"DescendantFonts").unwrap());
    let cid = dict_at(doc, &kids.as_array().unwrap()[0]);
    let desc = dict_at(doc, cid.get(b"FontDescriptor").unwrap());
    let ff = doc.get(desc.get(b"FontFile2").unwrap());
    let prog = doc.decode(ff.as_stream().unwrap()).unwrap().data;
    (prog, t0, cid)
}

/// An svg_core path from a font_core glyph path: font units scaled by `k`, moved to (x, y).
fn glyph_svg(p: &font_core::Path, k: f64, x: f64, y: f64) -> Path {
    use font_core::PathCmd;
    let f = |a: f32, b: f32| (x + a as f64 * k, y + b as f64 * k);
    let mut segs = Vec::new();
    for c in &p.cmds {
        segs.push(match *c {
            PathCmd::MoveTo(a, b) => Seg::Move(f(a, b)),
            PathCmd::LineTo(a, b) => Seg::Line(f(a, b)),
            PathCmd::QuadTo(a, b, c2, d) => Seg::Quad(f(a, b), f(c2, d)),
            PathCmd::CubicTo(a, b, c2, d, e, g) => Seg::Cubic(f(a, b), f(c2, d), f(e, g)),
            PathCmd::Close => Seg::Close,
        });
    }
    Path { segs }
}

#[test]
fn builder_text_round_trips_through_pdf3() {
    let jbm = font_file(JBM);
    let src = font_core::Font::parse(&jbm).unwrap();
    let s = "Hello UnaOS";
    let mut b = Builder::new();
    let f = b.font(&jbm).unwrap();
    let mut p = PageBuilder::new(200.0, 50.0);
    p.fill_gray(0.0).text(&f, 20.0, 10.0, 15.0, s);
    b.page(p);
    let pdf = b.finish();
    let (cov, text) = glyph_render(&pdf);
    assert_eq!(text, s);

    // The font objects: Type0 / Identity-H over CIDFontType2, /CIDToGIDMap /Identity, a subset-tagged name.
    let doc = Document::parse(&pdf).unwrap();
    let (prog, t0, cid) = embedded(&doc, 0, b"F0");
    assert_eq!(t0.name(b"Subtype"), Some(&b"Type0"[..]));
    assert_eq!(t0.name(b"Encoding"), Some(&b"Identity-H"[..]));
    assert_eq!(cid.name(b"Subtype"), Some(&b"CIDFontType2"[..]));
    assert_eq!(cid.name(b"CIDToGIDMap"), Some(&b"Identity"[..]));
    let base = t0.name(b"BaseFont").unwrap();
    assert!(base.len() > 7 && base[6] == b'+' && base[..6].iter().all(|c| c.is_ascii_uppercase()), "{}", String::from_utf8_lossy(base));

    // The subset parses in font_core, is far smaller than the source, and its sfnt checksum adjustment holds.
    let sub = font_core::Font::parse(&prog).unwrap();
    println!("subset: {} bytes of {}, {} glyphs", prog.len(), jbm.len(), sub.num_glyphs);
    assert!(prog.len() * 10 < jbm.len());
    let sum = prog.chunks(4).fold(0u32, |a, w| {
        let mut x = [0u8; 4];
        x[..w.len()].copy_from_slice(w);
        a.wrapping_add(u32::from_be_bytes(x))
    });
    assert_eq!(sum, 0xB1B0_AFBA);
    // New ids in first-use order: each subset glyph draws and advances like the source glyph.
    let (order, closure) = used_glyphs(&src, &[s]);
    assert_eq!(sub.num_glyphs as usize, 1 + closure.len());
    for (k, &old) in order.iter().enumerate() {
        let new = k as u16 + 1;
        assert_eq!(sub.glyph_path(new), src.glyph_path(old), "glyph {old} -> {new}");
        assert_eq!(sub.advance(new), src.advance(old));
    }

    // PIXELS, exact: the same CIDs drawn with the FULL source face embedded (source gids, the same widths).
    let k = 1000.0 / src.units_per_em as f64;
    let cids: String = s.chars().map(|c| format!("{:04X}", src.glyph_index(c))).collect();
    let w: String = order.iter().map(|&g| format!("{g} [{}]", src.advance(g) as f64 * k)).collect::<Vec<_>>().join(" ");
    let mut full = PdfBuilder::new(1);
    full.obj(1, "<< /Type /Catalog /Pages 2 0 R >>")
        .obj(2, "<< /Type /Pages /Kids [5 0 R] /Count 1 /MediaBox [0 0 200 50] >>")
        .obj(5, "<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 4 0 R >> >> /Contents 3 0 R >>")
        .stream(3, "", format!("0 g BT /F1 20 Tf 10 15 Td <{cids}> Tj ET").as_bytes())
        .obj(4, "<< /Type /Font /Subtype /Type0 /BaseFont /JetBrainsMono /Encoding /Identity-H /DescendantFonts [6 0 R] >>")
        .obj(6, &format!("<< /Type /Font /Subtype /CIDFontType2 /BaseFont /JetBrainsMono /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor 7 0 R /DW 1000 /W [{w}] /CIDToGIDMap /Identity >>"))
        .obj(7, "<< /Type /FontDescriptor /FontName /JetBrainsMono /Flags 4 /FontBBox [0 -300 600 1000] /FontFile2 8 0 R >>")
        .stream(8, "", &jbm);
    let (full_cov, _) = glyph_render(&full.classic());
    assert!(full_cov == cov, "the subset draws different ink from the full face");

    // PIXELS, against the builder's own path-text: PageBuilder::path raises the quads to cubics while GlyphSink keeps
    // them, so svg_core flattens them differently: a measured, pinned tolerance.
    let mut pp = PageBuilder::new(200.0, 50.0);
    pp.fill_gray(0.0);
    let (scale, mut pen) = (20.0 / src.units_per_em as f64, 10.0);
    for c in s.chars() {
        let gid = src.glyph_index(c);
        if let Some(gp) = src.glyph_path(gid) {
            pp.path(&glyph_svg(&gp, scale, pen, 15.0));
        }
        pp.fill();
        pen += src.advance(gid) as f64 * scale;
    }
    let mut pb = Builder::new();
    pb.page(pp);
    let r = render(&pb.finish(), 72.0);
    let path_cov: Vec<u8> = r.pixmap.data.chunks_exact(4).map(|p| 255 - p[0]).collect();
    let max_diff = cov.iter().zip(&path_cov).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
    let (ink_text, ink_path): (u64, u64) = (cov.iter().map(|&v| v as u64).sum(), path_cov.iter().map(|&v| v as u64).sum());
    println!("text vs path-text: max pixel diff {max_diff}, ink {ink_text} vs {ink_path}, fnv {:#018x}", fnv(&cov));
    assert!(max_diff <= TEXT_PATH_MAX_DIFF, "max pixel diff {max_diff}");
    assert!(ink_text.abs_diff(ink_path) * 1000 <= ink_text, "ink {ink_text} vs {ink_path}");
    assert_eq!(fnv(&cov), PIN_TEXT, "fnv pin: got {:#018x}", fnv(&cov));
}

/// The source glyphs `texts` show, in first-use order, and that list plus every composite component they reach.
fn used_glyphs(src: &font_core::Font<'_>, texts: &[&str]) -> (Vec<u16>, Vec<u16>) {
    let mut order: Vec<u16> = Vec::new();
    for c in texts.iter().flat_map(|t| t.chars()) {
        let g = src.glyph_index(c);
        if g != 0 && !order.contains(&g) {
            order.push(g);
        }
    }
    let font_core::Outlines::Glyf(g) = src.outlines else { panic!("glyf face") };
    let mut closure = order.clone();
    let mut i = 0;
    while i < closure.len() {
        for c in g.components(closure[i]).unwrap_or_default() {
            if !closure.contains(&c.glyph) {
                closure.push(c.glyph);
            }
        }
        i += 1;
    }
    (order, closure)
}

/// SR338: the largest per-pixel coverage difference measured between embedded text and the builder's path-text.
const TEXT_PATH_MAX_DIFF: u8 = 1;
/// A regression pin of the embedded-text page's ink (the round trip above is the real check).
const PIN_TEXT: u64 = 0x5852_a6f7_27f1_b3c7;

#[test]
fn builder_text_font_shared_and_cff_refused() {
    let jbm = font_file(JBM);
    let mut b = Builder::new();
    let f = b.font(&jbm).unwrap();
    let _unused = b.font(&jbm).unwrap();
    for word in ["Hi", "Mono"] {
        let mut p = PageBuilder::new(100.0, 30.0);
        p.text(&f, 12.0, 5.0, 10.0, word);
        b.page(p);
    }
    let pdf = b.finish();
    assert_eq!(String::from_utf8_lossy(&pdf).matches("/Subtype /Type0").count(), 1);
    let doc = Document::parse(&pdf).unwrap();
    let pages = doc.pages().unwrap();
    let fonts: Vec<pdf_core::Dict> = pages.iter().map(|p| dict_at(&doc, p.resources.get(b"Font").unwrap())).collect();
    assert_eq!(fonts[0].get(b"F0"), fonts[1].get(b"F0"));
    assert!(!fonts[0].contains(b"F1") && !fonts[1].contains(b"F1"));
    let fb = Fallback::default();
    assert_eq!(spans_to_string(&pages[0].text(&doc, &fb)), "Hi");
    assert_eq!(spans_to_string(&pages[1].text(&doc, &fb)), "Mono");
    // One subset for both pages: .notdef, H, i, M, o, n and the components of the composites among them.
    let (prog, _, _) = embedded(&doc, 1, b"F0");
    let src = font_core::Font::parse(&jbm).unwrap();
    let (order, closure) = used_glyphs(&src, &["Hi", "Mono"]);
    assert_eq!(order.len(), 5);
    println!("Hi + Mono: {} glyphs used, {} with components", order.len(), closure.len());
    assert_eq!(font_core::Font::parse(&prog).unwrap().num_glyphs as usize, 1 + closure.len());

    // CFF / OpenType-CFF refused by name; bytes that are no font refused.
    let mut otto = jbm.clone();
    otto[..4].copy_from_slice(b"OTTO");
    let mut b = Builder::new();
    match b.font(&otto) {
        Err(Error::Unsupported(m)) => assert!(m.starts_with("PDF-WRITE-CFF"), "{m}"),
        other => panic!("{other:?}"),
    }
    assert!(matches!(b.font(b"not a font"), Err(Error::Unsupported(_))));

    // A face loaded but never shown changes nothing: the bytes are those of a builder without fonts.
    let mut plain = Builder::new();
    plain.title("t").page(PageBuilder::new(10.0, 10.0));
    let mut loaded = Builder::new();
    loaded.font(&jbm).unwrap();
    loaded.title("t").page(PageBuilder::new(10.0, 10.0));
    let (a, z) = (plain.finish(), loaded.finish());
    assert!(a == z);
    assert!(!String::from_utf8_lossy(&a).contains("/Font"));
}
