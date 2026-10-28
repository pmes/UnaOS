// SPDX-License-Identifier: LGPL-3.0-or-later
//! PDFICC (LEDGER SR358): /ICCBased colour spaces (ISO 32000-1 §8.6.5.5) and JPX `colr` ICC profiles (T.800 I.5.3.3,
//! METH 2 / 3) draw through icc_core, Facet's ONE ICC engine; a profile icc_core refuses keeps the /N fallback. The
//! Adobe RGB reference is computed offline with numpy from the matrices: (200, 100, 50) -> sRGB (227.065, 100.067, 42.350).

mod common;

use common::PdfBuilder;
use pdf_core::Document;
use pdf_core::render::{Rendered, TextLog};

fn render(resources: &str, content: &str, extra: &[(u32, &str, Vec<u8>)]) -> Rendered {
    let mut b = PdfBuilder::new(1);
    b.obj(1, "<< /Type /Catalog /Pages 2 0 R >>");
    b.obj(2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.obj(3, &format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 40 40] /Resources << {resources} >> /Contents 4 0 R >>"));
    b.stream(4, "", content.as_bytes());
    for (n, d, data) in extra {
        b.stream(*n, d, data);
    }
    let bytes = b.classic();
    let doc = Document::parse(&bytes).unwrap();
    let pages = doc.pages().unwrap();
    pages[0].render(&doc, 72.0, &mut TextLog::default()).unwrap()
}

fn px(r: &Rendered, x: usize, y: usize) -> [u8; 4] {
    let p = &r.pixmap;
    let i = (y * p.w + x) * 4;
    [p.data[i], p.data[i + 1], p.data[i + 2], p.data[i + 3]]
}

fn near(got: [u8; 4], want: [u8; 3]) -> bool {
    (0..3).all(|i| got[i].abs_diff(want[i]) <= 1) && got[3] == 255
}

const FILL: &str = "/CS0 cs 0.7843137 0.3921569 0.1960784 sc 10 10 20 20 re f";

#[test]
fn iccbased_adobe_rgb_fill_draws_the_converted_colour() {
    let r = render("/ColorSpace << /CS0 [/ICCBased 5 0 R] >>", FILL, &[(5, "/N 3", icc_core::kat::adobe_rgb_v2())]);
    let got = px(&r, 20, 20);
    assert!(near(got, [227, 100, 42]), "{got:?}");
    // An sRGB profile is the identity (within the 8-bit rounding).
    let r = render("/ColorSpace << /CS0 [/ICCBased 5 0 R] >>", FILL, &[(5, "/N 3", icc_core::kat::srgb_v2())]);
    assert!(near(px(&r, 20, 20), [200, 100, 50]), "{:?}", px(&r, 20, 20));
    // A GRAY profile with /N 1.
    let r = render("/ColorSpace << /CS0 [/ICCBased 5 0 R] >>", "/CS0 cs 0.5 sc 10 10 20 20 re f", &[(5, "/N 1", icc_core::kat::gray_v2(1.0))]);
    let g = px(&r, 20, 20);
    assert!(g[0] == g[1] && g[1] == g[2] && g[0] > 128, "{g:?}");
}

#[test]
fn iccbased_refused_profile_falls_back_to_n() {
    // A named-colour profile is refused by icc_core: /N 3 keeps DeviceRGB, exactly as before PDFICC.
    let r = render("/ColorSpace << /CS0 [/ICCBased 5 0 R] >>", FILL, &[(5, "/N 3", icc_core::kat::named_colour())]);
    assert_eq!(px(&r, 20, 20), [200, 100, 50, 255]);
    // So does a profile whose channel count disagrees with /N (an RGB profile under /N 4 reads as DeviceCMYK).
    let r = render("/ColorSpace << /CS0 [/ICCBased 5 0 R] >>", "/CS0 cs 0 1 1 0 sc 10 10 20 20 re f", &[(5, "/N 4", icc_core::kat::adobe_rgb_v2())]);
    assert_eq!(px(&r, 20, 20), [255, 0, 0, 255]);
}

/// A JP2 file: signature, ftyp, jp2h (ihdr, one colr box with body `colr`), jp2c around the raw codestream `cs`.
fn jp2(cs: &[u8], w: u32, h: u32, nc: u16, colr: &[u8]) -> Vec<u8> {
    let bx = |t: &[u8; 4], body: &[u8]| {
        let mut b = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        b.extend(t);
        b.extend(body);
        b
    };
    let mut ihdr = h.to_be_bytes().to_vec();
    ihdr.extend(w.to_be_bytes());
    ihdr.extend(nc.to_be_bytes());
    ihdr.extend([7, 7, 0, 0]);
    let mut jp2h = bx(b"ihdr", &ihdr);
    jp2h.extend(bx(b"colr", colr));
    let mut f = vec![0, 0, 0, 12, b'j', b'P', b' ', b' ', 0x0D, 0x0A, 0x87, 0x0A];
    f.extend(bx(b"ftyp", b"jp2 \0\0\0\0jp2 "));
    f.extend(bx(b"jp2h", &jp2h));
    f.extend(bx(b"jp2c", cs));
    f
}

#[test]
fn jpx_colr_icc_is_applied() {
    // tests/render.rs `jpx_draws` Im1: a lossless 4 x 4 RCT codestream of rgb(x, y) = (60x + 20, 70y + 10, 30(x + y) + 5).
    let unhex = |s: &str| (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect::<Vec<u8>>();
    let cs = unhex(concat!(
        "ff4fff51002f000000000004000000040000000000000000000000040000000400000000000000000003070101070101",
        "070101ff52000c00000001010104040001ff5c00074040484850ff640025000143726561746564206279204f70656e4a",
        "5045472076657273696f6e20322e352e30ff90000a0000000000520001ff93cfb4140658c0d7d9cfb4140c1d7e66bfdf",
        "80300baf5537d77fc1f38387d40900220d002d0b3c654f01cfc1f38387d4080cf78f0c17359fc3ea038fb4100cf78f0c",
        "18035fffd9",
    ));
    let profile = icc_core::kat::adobe_rgb_v2();
    let mut colr = vec![2, 0, 0];
    colr.extend(&profile);
    let file = jp2(&cs, 4, 4, 3, &colr);
    let d = pdf_core::filters::jpx::decode(&file).unwrap();
    assert_eq!(d.icc.as_deref(), Some(&profile[..]), "METH 2 surfaces the profile");
    // No /ColorSpace: the colr profile rules (§8.9.5).
    let r = render(
        "/XObject << /Im1 5 0 R >>",
        "q 10 0 0 10 10 10 cm /Im1 Do Q",
        &[(5, "/Type /XObject /Subtype /Image /Width 4 /Height 4 /BitsPerComponent 8 /Filter /JPXDecode", file)],
    );
    assert!(r.refused.is_empty(), "{:?}", r.refused);
    let xf = icc_core::Transform::from_icc(&profile).unwrap();
    for (x, y) in [(0usize, 0usize), (3, 0), (1, 2), (3, 3)] {
        let src = [(60 * x + 20) as f32, (70 * y + 10) as f32, (30 * (x + y) + 5) as f32].map(|c| c / 255.0);
        let want = xf.eval(&src).map(|c| (c * 255.0 + 0.5) as u8);
        let got = px(&r, 10 + x * 5 / 2 + 1, 20 + y * 5 / 2 + 1);
        assert!(near(got, want), "({x}, {y}): {got:?} vs {want:?}");
    }
}
