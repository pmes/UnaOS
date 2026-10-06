//! RAWCORE (B444): pixel_core's raw route on the SYNTHETIC ARW (no real Sony file on this machine).

use raw_core::synth::{self, Coding};

#[test]
fn raw_route_preview_full_mime_facts() {
    let f = synth::arw(16, 16, &synth::ramp(16, 16), Coding::Plain14);
    assert_eq!(pixel_core::sniff(&f), None, "TIFF is not one of the six sniffed formats");
    assert_eq!(pixel_core::mime_of(&f), Some("image/x-sony-arw"));
    // FAST: the embedded JPEG through pixel_core's own decoder = the JPEG decoded alone.
    let p = pixel_core::raw::decode_preview(&f).unwrap();
    let j = pixel_core::decode_jpeg(synth::PREVIEW_JPEG).unwrap();
    assert_eq!((p.width, p.height, &p.rgba), (j.width, j.height, &j.rgba));
    // FULL: the 16x16 mosaic developed.
    let img = pixel_core::decode(&f).unwrap();
    assert_eq!((img.width, img.height, img.rgba.len()), (16, 16, 16 * 16 * 4));
    assert!(img.rgba.chunks(4).all(|p| p[3] == 255));
    assert_eq!(pixel_core::decode_first_frame(&f).unwrap().rgba, img.rgba);
    let x = pixel_core::facts_of(&f).unwrap();
    assert_eq!((x.width, x.height, x.animated), (16, 16, false));
    // A cRAW file develops the same way.
    let codes: Vec<u16> = (0..64 * 2).map(|i| 200 + (i % 64) as u16).collect();
    let c = synth::arw(64, 2, &codes, Coding::Craw);
    assert_eq!(pixel_core::decode(&c).unwrap().width, 64);
}

#[test]
fn plain_tiff_types_as_tiff_and_garbage_refuses() {
    let mut f = synth::arw(16, 16, &synth::ramp(16, 16), Coding::Plain14);
    let p = f.windows(4).position(|w| w == b"SONY").unwrap();
    f[p..p + 4].copy_from_slice(b"ACME");
    assert_eq!(pixel_core::mime_of(&f), Some("image/tiff"));
    assert!(pixel_core::decode(b"II\x2a\x00\xff\xff\xff\xff").is_err());
}
