// SPDX-License-Identifier: LGPL-3.0-or-later
//! PDF3 (SR314): fonts and text. Every PDF here is built by PDF1's `PdfBuilder`; the font programs are the tree's own
//! bundled faces (fonts/) or are written by this file (a two-glyph CFF built byte by byte, a Type 1 program written in
//! the clear and eexec-encrypted here). The glyphs are filled by a TEST painter: font_core's own `Rasterizer` through
//! its `Scaled` sink (PDF2's renderer paints through svg_core in the product), one page-sized coverage canvas, y up.
//! Each KAT pins an FNV-1a over that coverage, a few sampled pixels and the extracted text.

mod common;

use common::PdfBuilder;
use font_core::raster::{Rasterizer, Scaled};
use font_core::OutlineSink;
use pdf_core::content::ops;
use pdf_core::font::cmap::CMap;
use pdf_core::font::text::walk;
use pdf_core::font::{Fallback, GlyphSink, IDENTITY, Matrix, apply, mul, spans_to_string};
use pdf_core::Document;

const W: usize = 200;
const H: usize = 50;

fn font_file(rel: &str) -> Vec<u8> {
    std::fs::read(format!("{}/../../../../fonts/{rel}", env!("CARGO_MANIFEST_DIR"))).expect("bundled font")
}

fn fnv(b: &[u8]) -> u64 {
    b.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &x| (h ^ x as u64).wrapping_mul(0x0000_0100_0000_01b3))
}

/// Text space -> device (PDF units = pixels), forwarded to font_core's rasterizer (y up: `Scaled` flips at `oy`).
struct Dev<'r> {
    m: Matrix,
    s: Scaled<'r>,
}

impl Dev<'_> {
    fn p(&self, x: f32, y: f32) -> (f32, f32) {
        let (a, b) = apply(&self.m, x as f64, y as f64);
        (a as f32, b as f32)
    }
}

impl OutlineSink for Dev<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        let (a, b) = self.p(x, y);
        self.s.move_to(a, b);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let (a, b) = self.p(x, y);
        self.s.line_to(a, b);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (a, b) = self.p(x1, y1);
        let (c, d) = self.p(x, y);
        self.s.quad_to(a, b, c, d);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (a, b) = self.p(x1, y1);
        let (c, d) = self.p(x2, y2);
        let (e, f) = self.p(x, y);
        self.s.curve_to(a, b, c, d, e, f);
    }
    fn close(&mut self) {
        self.s.close();
    }
}

/// The test painter's stand-in for PDF2's interpreter on a Type 3 CharProc: `re` rectangles only (`f` implied).
fn type3_proc(proc_: &[u8], m: Matrix, r: &mut Rasterizer) {
    for op in ops(proc_) {
        let n: Vec<f64> = op.operands.iter().filter_map(|o| o.as_f64()).collect();
        if op.operator == b"re" && n.len() == 4 {
            let (x, y, w, h) = (n[0] as f32, n[1] as f32, n[2] as f32, n[3] as f32);
            let mut d = Dev { m, s: Scaled { r: &mut *r, scale: 1.0, ox: 0.0, oy: H as f32 } };
            d.move_to(x, y);
            d.line_to(x + w, y);
            d.line_to(x + w, y + h);
            d.line_to(x, y + h);
            d.close();
        }
    }
}

/// Render page 0's text through `walk` + `GlyphSink::show`; return the coverage and the extracted text.
fn render(pdf: &[u8], fb: Fallback<'_>) -> (Vec<u8>, String) {
    let doc = Document::parse(pdf).unwrap();
    let page = &doc.pages().unwrap()[0];
    let mut r = Rasterizer::new(W, H);
    let mut sink = GlyphSink::new(&doc, fb);
    walk(&doc, &page.contents, &page.resources, IDENTITY, &mut |font, bytes, p, tm, ctm| {
        let shown = sink.show(font, bytes, p);
        let m = mul(tm, ctm);
        {
            let mut d = Dev { m, s: Scaled { r: &mut r, scale: 1.0, ox: 0.0, oy: H as f32 } };
            shown.outline.replay(&mut d);
        }
        for t3 in &shown.type3 {
            type3_proc(&t3.proc_, mul(&t3.m, &m), &mut r);
        }
        shown.advance
    });
    let text = spans_to_string(&page.text(&doc, &fb));
    (r.finish(), text)
}

/// Coverage at the pixel whose centre is (x + 0.5, y + 0.5) in PDF units (y up).
fn px(cov: &[u8], x: usize, y: usize) -> u8 {
    cov[(H - 1 - y) * W + x]
}

fn ink(cov: &[u8]) -> usize {
    cov.iter().filter(|&&v| v > 0).count()
}

/// A one-page document: object 4 is the page's font /F1, object 3 the content stream.
fn page(content: &str) -> PdfBuilder {
    let mut b = PdfBuilder::new(1);
    b.obj(1, "<< /Type /Catalog /Pages 2 0 R >>")
        .obj(2, &format!("<< /Type /Pages /Kids [5 0 R] /Count 1 /MediaBox [0 0 {W} {H}] >>"))
        .obj(5, "<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 4 0 R >> >> /Contents 3 0 R >>")
        .stream(3, "", content.as_bytes());
    b
}

fn report(name: &str, cov: &[u8], text: &str) {
    println!("{name}: fnv={:#018x} ink={} text={text:?}", fnv(cov), ink(cov));
}

// ------------------------------------------------------------------------------------------------- (a) standard 14

#[test]
fn helvetica_fallback_hello() {
    let inter = font_file("inter/InterVariable.ttf");
    let fb = Fallback { sans: Some(&inter), mono: None, serif: None };
    let mut b = page("BT /F1 24 Tf 10 15 Td (Hello) Tj ET");
    b.obj(4, "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>");
    let (cov, text) = render(&b.classic(), fb);
    report("helvetica", &cov, &text);
    assert_eq!(text, "Hello");
    // 'H' at x = 10 (Inter's advances, no /Widths): its stems are solid at x = 13 and x = 24, its counter is clear.
    assert_eq!(px(&cov, 13, 20), 255);
    assert_eq!(px(&cov, 24, 25), 255);
    assert_eq!(px(&cov, 18, 25), 0);
    assert_eq!(fnv(&cov), 0x2b5c5fe87849c205);
    // Without a fallback face nothing is drawn, the text still extracts.
    let (cov, text) = render(&b.classic(), Fallback::default());
    assert_eq!(ink(&cov), 0);
    assert_eq!(text, "Hello");
}

// ------------------------------------------------------------------------------------------------- (b) TrueType

fn widths(n: usize, w: u32) -> String {
    (0..n).map(|_| w.to_string()).collect::<Vec<_>>().join(" ")
}

#[test]
fn truetype_embedded() {
    let jbm = font_file("jetbrains-mono/JetBrainsMono-Variable.ttf");
    let mut b = page("BT /F1 20 Tf 10 15 Td (Mono) Tj ET");
    b.obj(4, &format!("<< /Type /Font /Subtype /TrueType /BaseFont /ABCDEF+JetBrainsMono /FirstChar 32 /LastChar 126 /Widths [{}] /Encoding /WinAnsiEncoding /FontDescriptor 6 0 R >>", widths(95, 600)))
        .obj(6, "<< /Type /FontDescriptor /FontName /ABCDEF+JetBrainsMono /Flags 32 /FontBBox [0 -300 600 1000] /ItalicAngle 0 /Ascent 1020 /Descent -300 /CapHeight 730 /StemV 80 /FontFile2 7 0 R >>")
        .stream(7, "", &jbm);
    let (cov, text) = render(&b.classic(), Fallback::default());
    report("truetype", &cov, &text);
    assert_eq!(text, "Mono");
    // Four 12-unit advances from x = 10: glyph 'o' (the 4th) starts at x = 46 and ends before 58.
    assert!(ink(&cov) > 100);
    assert_eq!((60..W).map(|x| (0..H).map(|y| px(&cov, x, y) as u32).sum::<u32>()).sum::<u32>(), 0);
    assert_eq!(fnv(&cov), 0x0b2817cfb4d731df);
}

// ------------------------------------------------------------------------------------------------- (c) CFF

fn cff_num(v: i32) -> Vec<u8> {
    match v {
        -107..=107 => vec![(v + 139) as u8],
        _ => {
            let b = (v as i16).to_be_bytes();
            vec![28, b[0], b[1]]
        }
    }
}

fn index(items: &[Vec<u8>]) -> Vec<u8> {
    if items.is_empty() {
        return vec![0, 0];
    }
    let mut out = (items.len() as u16).to_be_bytes().to_vec();
    out.push(2); // offSize 2
    let mut off = 1u16;
    out.extend(off.to_be_bytes());
    for it in items {
        off += it.len() as u16;
        out.extend(off.to_be_bytes());
    }
    for it in items {
        out.extend(it);
    }
    out
}

/// A dict operand as the fixed 5-byte form (29 + i32), so the Top DICT's length does not depend on the offsets.
fn dict_i32(v: i32) -> Vec<u8> {
    let mut o = vec![29];
    o.extend(v.to_be_bytes());
    o
}

/// A two-glyph CFF (TN #5176): glyphs .notdef, "A" (SID 34: a 500-unit square) and "B" (SID 35: a triangle), the
/// ISOAdobe-style custom charset format 0, Type 2 charstrings, a Private DICT with defaultWidthX 600.
fn tiny_cff() -> Vec<u8> {
    let op = |o: u8| vec![o];
    let cs = |parts: &[Vec<u8>]| parts.concat();
    let notdef = cs(&[op(14)]);
    // 100 100 rmoveto 500 hlineto 500 vlineto -500 hlineto endchar
    let a = cs(&[cff_num(100), cff_num(100), op(21), cff_num(500), op(6), cff_num(500), op(7), cff_num(-500), op(6), op(14)]);
    // 100 100 rmoveto 500 0 rlineto -250 500 rlineto endchar
    let b = cs(&[cff_num(100), cff_num(100), op(21), cff_num(500), cff_num(0), op(5), cff_num(-250), cff_num(500), op(5), op(14)]);
    let header = vec![1u8, 0, 4, 2];
    let name = index(&[b"Tiny".to_vec()]);
    let strings = index(&[]);
    let gsubrs = index(&[]);
    let charset = [vec![0u8], 34u16.to_be_bytes().to_vec(), 35u16.to_be_bytes().to_vec()].concat();
    let charstrings = index(&[notdef, a, b]);
    let private = [cff_num(600), vec![20]].concat();
    // Top DICT: charset (15), CharStrings (17), Private (18: size offset) — 5 + 1 + 5 + 1 + 5 + 5 + 1 bytes.
    let top_len = 23usize;
    let top_index_len = 2 + 1 + 4 + top_len;
    let charset_off = header.len() + name.len() + top_index_len + strings.len() + gsubrs.len();
    let cs_off = charset_off + charset.len();
    let priv_off = cs_off + charstrings.len();
    let top = [dict_i32(charset_off as i32), vec![15], dict_i32(cs_off as i32), vec![17], dict_i32(private.len() as i32), dict_i32(priv_off as i32), vec![18]].concat();
    assert_eq!(top.len(), top_len);
    let top_index = index(&[top]);
    assert_eq!(top_index.len(), top_index_len);
    [header, name, top_index, strings, gsubrs, charset, charstrings, private].concat()
}

#[test]
fn cff_type1c() {
    let cff = tiny_cff();
    let parsed = font_core::cff::Cff::parse(&cff, 0).expect("the hand-built CFF parses");
    assert_eq!(parsed.glyph_name(1).as_deref(), Some("A"));
    assert_eq!(parsed.glyph_name(2).as_deref(), Some("B"));
    let mut b = page("BT /F1 20 Tf 10 10 Td (AB) Tj ET");
    b.obj(4, "<< /Type /Font /Subtype /Type1 /BaseFont /ABCDEF+Tiny /FirstChar 65 /LastChar 66 /Widths [600 600] /FontDescriptor 6 0 R >>")
        .obj(6, "<< /Type /FontDescriptor /FontName /ABCDEF+Tiny /Flags 32 /FontBBox [0 0 600 600] /FontFile3 7 0 R >>")
        .stream(7, "/Subtype /Type1C", &cff);
    let (cov, text) = render(&b.classic(), Fallback::default());
    report("cff", &cov, &text);
    assert_eq!(text, "AB");
    // 'A': the square 100..600 units at 20 pt = x 12..22, y 12..22; 'B' starts at x = 22.
    assert_eq!(px(&cov, 15, 15), 255);
    assert_eq!(px(&cov, 15, 23), 0);
    assert_eq!(px(&cov, 11, 15), 0);
    // 'B': the triangle 24..34 at its base (y 12..13), its apex at x = 29, y = 22.
    assert_eq!(px(&cov, 29, 13), 255);
    assert_eq!(px(&cov, 25, 20), 0);
    assert_eq!(fnv(&cov), 0x770526419867acf9);
}

// ------------------------------------------------------------------------------------------------- (d) Type 1

fn t1_num(v: i32) -> Vec<u8> {
    match v {
        -107..=107 => vec![(v + 139) as u8],
        108..=1131 => {
            let w = v - 108;
            vec![(w / 256 + 247) as u8, (w % 256) as u8]
        }
        -1131..=-108 => {
            let w = -v - 108;
            vec![(w / 256 + 251) as u8, (w % 256) as u8]
        }
        _ => {
            let mut o = vec![255];
            o.extend(v.to_be_bytes());
            o
        }
    }
}

/// eexec / charstring encryption (Type 1 spec §7): the inverse of font_core's `decrypt`.
fn encrypt(plain: &[u8], key: u16) -> Vec<u8> {
    let mut r = key;
    plain
        .iter()
        .map(|&p| {
            let c = p ^ (r >> 8) as u8;
            r = (c as u16).wrapping_add(r).wrapping_mul(52845).wrapping_add(22719);
            c
        })
        .collect()
}

fn charstring(plain: &[u8]) -> Vec<u8> {
    encrypt(&[&[0u8, 0, 0, 0][..], plain].concat(), 4330)
}

/// The cleartext and the eexec-encrypted private part of a Type 1 font with glyphs A (a square closed through
/// subroutine 0), acute, Aacute (seac of A + acute) and B (a flex double hump).
fn tiny_type1() -> (Vec<u8>, Vec<u8>) {
    let n = |v: i32| t1_num(v);
    let op = |o: u8| vec![o];
    let esc = |o: u8| vec![12, o];
    let subr0 = [op(9), op(11)].concat(); // closepath return
    let a = [n(0), n(600), op(13), n(100), n(100), op(21), n(400), op(6), n(400), op(7), n(-400), op(6), n(0), op(10), op(14)].concat();
    let acute = [n(0), n(300), op(13), n(100), n(500), op(21), n(100), op(6), n(100), op(7), n(-100), op(6), op(9), op(14)].concat();
    let aacute = [n(0), n(600), op(13), n(0), n(50), n(300), n(65), n(194), esc(6)].concat();
    let flex_pt = |dx: i32, dy: i32| [n(dx), n(dy), op(21), n(0), n(2), esc(16)].concat();
    let b = [
        n(0), n(600), op(13), n(100), n(100), op(21),
        n(0), n(1), esc(16), // flex start
        flex_pt(200, 0), flex_pt(-150, 100), flex_pt(100, 0), flex_pt(50, -100), flex_pt(50, 100), flex_pt(100, 0), flex_pt(50, -100),
        n(50), n(500), n(100), n(3), n(0), esc(16), esc(17), esc(17), esc(33),
        op(9), op(14),
    ]
    .concat();
    let notdef = [n(0), n(500), op(13), op(14)].concat();
    let clear = b"%!PS-AdobeFont-1.0: TinyT1 001\n11 dict begin\n/FontName /TinyT1 def\n/FontMatrix [0.001 0 0 0.001 0 0] readonly def\n/Encoding StandardEncoding def\ncurrentdict end\ncurrentfile eexec\r\n".to_vec();
    let mut private: Vec<u8> = b"dup /Private 8 dict dup begin\n/RD{string currentfile exch readstring pop}executeonly def\n/ND{noaccess def}executeonly def\n/NP{noaccess put}executeonly def\n/lenIV 4 def\n/Subrs 1 array\n".to_vec();
    let s0 = charstring(&subr0);
    private.extend(format!("dup 0 {} RD ", s0.len()).bytes());
    private.extend(&s0);
    private.extend(b" NP\nND\n2 index /CharStrings 5 dict dup begin\n");
    for (name, cs) in [(".notdef", notdef), ("A", a), ("acute", acute), ("Aacute", aacute), ("B", b)] {
        let e = charstring(&cs);
        private.extend(format!("/{name} {} RD ", e.len()).bytes());
        private.extend(&e);
        private.extend(b" ND\n");
    }
    private.extend(b"end\nend\nreadonly put\nnoaccess put\ndup /FontName get exch definefont pop\nmark currentfile closefile\n");
    // Four lead bytes chosen so the cipher text does not start with four hex digits (a reader would take it for PFA).
    let enc = encrypt(&[&[0xA5u8, 0x5A, 0x0F, 0xF0][..], &private].concat(), 55665);
    assert!(!enc[..4].iter().all(|b| b.is_ascii_hexdigit()));
    (clear, enc)
}

fn trailer() -> Vec<u8> {
    let mut t = Vec::new();
    for _ in 0..8 {
        t.extend([b'0'; 64]);
        t.push(b'\n');
    }
    t.extend(b"cleartomark\n");
    t
}

#[test]
fn eexec_roundtrip() {
    let plain = b"dup /Private 8 dict dup begin".to_vec();
    assert_eq!(font_core::type1::decrypt(&encrypt(&plain, 55665), 55665, 0), plain);
    assert_eq!(font_core::type1::standard_encoding_name(65), Some("A"));
    assert_eq!(font_core::type1::standard_encoding_name(194), Some("acute"));
    assert_eq!(font_core::type1::standard_encoding_name(0x80), None);
    let (clear, enc) = tiny_type1();
    let raw = [clear.clone(), enc.clone(), b"\n".to_vec(), trailer()].concat();
    let hex: String = enc.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().chunks(32).map(|c| c.concat() + "\n").collect();
    let pfa = [clear.clone(), hex.into_bytes(), trailer()].concat();
    let seg = |ty: u8, d: &[u8]| [vec![0x80, ty], (d.len() as u32).to_le_bytes().to_vec(), d.to_vec()].concat();
    let pfb = [seg(1, &clear), seg(2, &enc), seg(1, &trailer()), vec![0x80, 3]].concat();
    let mut outlines = Vec::new();
    for prog in [&raw, &pfa, &pfb] {
        let f = font_core::type1::Type1Font::parse(prog).expect("type 1 parses");
        assert_eq!(f.font_name, "TinyT1");
        assert_eq!(f.glyph_names().collect::<Vec<_>>(), [".notdef", "A", "Aacute", "B", "acute"]);
        let mut p = font_core::Path::new();
        assert_eq!(f.outline("Aacute", &mut p), Some(600.0));
        // seac: the base square plus the accent offset by (adx - asb + sbx, ady) = (50, 300).
        assert_eq!(p.contours(), 2);
        assert_eq!((p.x_min, p.y_min, p.x_max, p.y_max), (100.0, 100.0, 500.0, 900.0));
        let mut fl = font_core::Path::new();
        assert_eq!(f.outline("B", &mut fl), Some(600.0));
        assert_eq!(fl.cmds.iter().filter(|c| matches!(c, font_core::PathCmd::CubicTo(..))).count(), 2);
        outlines.push((p, fl));
    }
    assert!(outlines.windows(2).all(|w| w[0] == w[1]));
}

#[test]
fn type1_eexec() {
    let (clear, enc) = tiny_type1();
    let prog = [clear.clone(), enc.clone(), b"\n".to_vec(), trailer()].concat();
    let mut b = page("BT /F1 20 Tf 10 10 Td (ABC) Tj ET");
    b.obj(4, "<< /Type /Font /Subtype /Type1 /BaseFont /TinyT1 /FirstChar 65 /LastChar 67 /Widths [600 600 600] /Encoding << /Type /Encoding /Differences [66 /Aacute /B] >> /FontDescriptor 6 0 R >>")
        .obj(6, "<< /Type /FontDescriptor /FontName /TinyT1 /Flags 32 /FontBBox [0 0 600 900] /FontFile 7 0 R >>")
        .stream(7, &format!("/Length1 {} /Length2 {} /Length3 {}", clear.len(), enc.len() + 1, trailer().len()), &prog);
    let (cov, text) = render(&b.classic(), Fallback::default());
    report("type1", &cov, &text);
    assert_eq!(text, "A\u{C1}B");
    // 'A' square 100..500 units at 20 pt: x 12..20, y 12..20.
    assert_eq!(px(&cov, 15, 15), 255);
    assert_eq!(px(&cov, 21, 15), 0);
    // 'Aacute' at x = 22: its accent 150..250 x 800..900 units = x 25..27, y 26..28.
    assert_eq!(px(&cov, 25, 26), 255);
    assert_eq!(px(&cov, 25, 23), 0);
    assert_eq!(fnv(&cov), 0x25406a2034d34375);
}

// ------------------------------------------------------------------------------------------------- (e) Type 0

const TO_UNICODE_2B: &str = "/CIDInit /ProcSet findresource begin 12 dict begin begincmap\n/CMapName /Test-UCS def\n1 begincodespacerange <0000> <FFFF> endcodespacerange\n2 beginbfchar <0040> <0048> <00E1> <0065> endbfchar\n3 beginbfrange <0113> <0113> <006C> <0122> <0122> <006F> <001A> <001B> [<0042> <0043>] endbfrange\nendcmap CMapName currentdict /CMap defineresource pop end end\n";

fn type0_doc(content: &str, encoding: &str, to_unicode: &str, cmap: Option<&str>) -> Vec<u8> {
    let jbm = font_file("jetbrains-mono/JetBrainsMono-Variable.ttf");
    let mut b = page(content);
    b.obj(4, &format!("<< /Type /Font /Subtype /Type0 /BaseFont /JetBrainsMono /Encoding {encoding} /DescendantFonts [6 0 R] /ToUnicode 8 0 R >>"))
        .obj(6, "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /JetBrainsMono /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor 7 0 R /DW 600 /CIDToGIDMap /Identity >>")
        .obj(7, "<< /Type /FontDescriptor /FontName /JetBrainsMono /Flags 4 /FontBBox [0 -300 600 1000] /FontFile2 9 0 R >>")
        .stream(8, "", to_unicode.as_bytes())
        .stream(9, "", &jbm);
    if let Some(c) = cmap {
        b.stream(10, "/Type /CMap /CMapName /Test-1B", c.as_bytes());
    }
    b.classic()
}

#[test]
fn type0_identity_h() {
    // gids measured in JetBrainsMono-Variable.ttf: H 64, e 225, l 275, o 290, B 26, C 27.
    let pdf = type0_doc("BT /F1 20 Tf 10 15 Td <0040 00E1 0113 0113 0122> Tj <001A001B> Tj ET", "/Identity-H", TO_UNICODE_2B, None);
    let (cov, text) = render(&pdf, Fallback::default());
    report("type0", &cov, &text);
    assert_eq!(text, "HelloBC");
    assert!(ink(&cov) > 100);
    // Seven 12-unit advances from x = 10 end at x = 94.
    assert_eq!((96..W).map(|x| (0..H).map(|y| px(&cov, x, y) as u32).sum::<u32>()).sum::<u32>(), 0);
    assert_eq!(fnv(&cov), 0x7dd013c7cb2be79c);
    // The same glyphs through an EMBEDDED one-byte CMap (cidchar + cidrange) draw the same pixels.
    let cmap = "/CIDInit /ProcSet findresource begin 12 dict begin begincmap\n/CMapName /Test-1B def\n1 begincodespacerange <00> <FF> endcodespacerange\n4 begincidchar <48> 64 <65> 225 <6C> 275 <6F> 290 endcidchar\n1 begincidrange <42> <43> 26 endcidrange\nendcmap CMapName currentdict /CMap defineresource pop end end\n";
    let to_u = "begincmap 1 begincodespacerange <00> <FF> endcodespacerange 1 beginbfrange <20> <7E> <0020> endbfrange endcmap";
    let pdf = type0_doc("BT /F1 20 Tf 10 15 Td (HelloBC) Tj ET", "10 0 R", to_u, Some(cmap));
    let (cov2, text2) = render(&pdf, Fallback::default());
    assert_eq!(text2, "HelloBC");
    assert_eq!(cov2, cov);
}

// ------------------------------------------------------------------------------------------------- (f) ToUnicode

#[test]
fn tounicode_bfrange() {
    let m = CMap::parse(TO_UNICODE_2B.as_bytes());
    assert_eq!(m.unicode(0x40).as_deref(), Some("H"));
    assert_eq!(m.unicode(0x113).as_deref(), Some("l"));
    assert_eq!(m.unicode(0x1A).as_deref(), Some("B"));
    assert_eq!(m.unicode(0x1B).as_deref(), Some("C"));
    assert_eq!(m.unicode(0x1C), None);
    // Base-string ranges increment the last UTF-16 unit; surrogate pairs and ligatures (multi-unit destinations).
    let m = CMap::parse(b"2 begincodespacerange <00> <7F> <8000> <FFFF> endcodespacerange\n2 beginbfchar <05> <D83DDE00> <8001> <00660069> endbfchar\n1 beginbfrange <10> <12> <0066> endbfrange\n");
    assert_eq!(m.unicode(5).as_deref(), Some("\u{1F600}"));
    assert_eq!(m.unicode(0x8001).as_deref(), Some("fi"));
    assert_eq!(m.unicode(0x12).as_deref(), Some("h"));
    // Mixed one- and two-byte codes split by the codespace.
    assert_eq!(m.codes(&[0x10, 0x80, 0x01, 0x05]), vec![(0x10, 1), (0x8001, 2), (0x05, 1)]);
    // Identity-H: two-byte codes, CID = code.
    let id = CMap::predefined(b"Identity-H");
    assert_eq!(id.codes(&[0x01, 0x02]), vec![(0x0102, 2)]);
    assert_eq!(id.cid(0x0102), 0x0102);
    assert!(!id.vertical && CMap::predefined(b"Identity-V").vertical);
}

// ------------------------------------------------------------------------------------------------- (g) Type 3

#[test]
fn type3_square() {
    let mut b = page("BT /F1 20 Tf 10 10 Td (AA) Tj ET");
    b.obj(4, "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 750 750] /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /square 6 0 R >> /Encoding << /Type /Encoding /Differences [65 /square] >> /FirstChar 65 /LastChar 65 /Widths [1000] /Resources << >> >>")
        .stream(6, "", b"1000 0 0 0 750 750 d1 0 0 750 750 re f");
    let (cov, text) = render(&b.classic(), Fallback::default());
    report("type3", &cov, &text);
    // A glyph name with no Unicode extracts as U+FFFD.
    assert_eq!(text, "\u{FFFD}\u{FFFD}");
    // Two 15-unit squares, the second one advance (1000 x 0.001 x 20 = 20) to the right.
    assert_eq!(px(&cov, 17, 17), 255);
    assert_eq!(px(&cov, 27, 17), 0);
    assert_eq!(px(&cov, 37, 17), 255);
    assert_eq!(px(&cov, 17, 26), 0);
    assert_eq!(ink(&cov), 2 * 15 * 15);
    assert_eq!(fnv(&cov), 0x726d158504492575);
}
