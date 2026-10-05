//! M4: pixel_core's `svg` format — sniff, intrinsic-size decode, requested-size decode, and raster images
//! inside an SVG decoded by pixel_core.
mod common;

const SVG: &[u8] = br##"<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" width="8" height="4" viewBox="0 0 2 1"><rect width="1" height="1" fill="#0000ff"/></svg>"##;

#[test]
fn sniff_and_decode_intrinsic() {
    assert_eq!(pixel_core::sniff(SVG), Some(pixel_core::Format::Svg));
    let img = pixel_core::decode(SVG).unwrap();
    assert_eq!((img.width, img.height), (8, 4));
    assert_eq!(&img.rgba[0..4], &[0, 0, 255, 255]);
    assert_eq!(&img.rgba[(4 * 4)..(4 * 4 + 4)], &[0, 0, 0, 0]); // x = 4 is past the blue half
    assert!(img.frames.is_none());
    assert_eq!(pixel_core::svg::intrinsic_size(SVG).unwrap(), (8, 4));
}

#[test]
fn decode_at_requested_size() {
    let img = pixel_core::svg::decode_at(SVG, Some((20, 10)), None).unwrap();
    assert_eq!((img.width, img.height), (20, 10));
    assert_eq!(&img.rgba[(9 * 4)..(9 * 4 + 4)], &[0, 0, 255, 255]);
    assert_eq!(img.rgba[(10 * 4) + 3], 0);
    assert!(pixel_core::svg::decode_at(SVG, Some((0, 10)), None).is_err());
    assert!(pixel_core::decode(b"<?xml version='1.0'?><svg").is_err());
}

#[test]
fn raster_image_inside_svg() {
    // A 2x1 PNG (red, green) placed 1:1 and then scaled ×2 with image-rendering pixelated.
    let dir = std::env::temp_dir().join(format!("svgcore-face-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("t.png");
    common::write_png(&p, 2, 1, &[255, 0, 0, 255, 0, 255, 0, 255]);
    let b64 = base64(&std::fs::read(&p).unwrap());
    let _ = std::fs::remove_dir_all(&dir);
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="4" height="2">
        <image width="4" height="2" image-rendering="pixelated" xlink:href="data:image/png;base64,{b64}"/></svg>"#
    );
    let img = pixel_core::decode(svg.as_bytes()).unwrap();
    assert_eq!(&img.rgba[0..4], &[255, 0, 0, 255]);
    assert_eq!(&img.rgba[12..16], &[0, 255, 0, 255]);
}

fn base64(b: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                s.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}
