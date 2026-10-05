//! Dev tool: side-by-side Chromium | svg_core | |diff|×4 for one vector.
//! `svgcompare <in.svg> <chrome.png> <out.png> W H` (fonts from $SVGCORE_FONTS).
#[path = "../tests/common/mod.rs"]
mod common;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let svg = svg_core::Svg::parse(&std::fs::read(&a[1]).unwrap());
    let (w, h): (usize, usize) = (a[4].parse().unwrap(), a[5].parse().unwrap());
    let fonts = std::env::var("SVGCORE_FONTS").ok().map(|d| common::fonts_from(std::path::Path::new(&d)));
    let opts = svg_core::Options { fonts: fonts.as_ref(), image_decoder: Some(common::decoder), languages: vec![] };
    let ours = match svg {
        Ok(s) => s.render_rgba(w as u32, h as u32, &opts).unwrap(),
        Err(e) => {
            eprintln!("parse error {e:?}");
            vec![0; w * h * 4]
        }
    };
    let o = common::over_white(&ours);
    let c = pixel_core::decode(&std::fs::read(&a[2]).unwrap()).unwrap();
    let c: Vec<u8> = c.rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
    let (m, wi) = common::score(&o, &c);
    eprintln!("mean {m:.3} within8 {:.2}%", wi * 100.0);
    let ow = w * 3;
    let mut out = vec![255u8; ow * h * 4];
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 3;
            for (k, src) in [&c, &o].iter().enumerate() {
                let d = (y * ow + k * w + x) * 4;
                out[d..d + 3].copy_from_slice(&src[i..i + 3]);
            }
            let d = (y * ow + 2 * w + x) * 4;
            for ch in 0..3 {
                let v = (c[i + ch] as i32 - o[i + ch] as i32).unsigned_abs() * 4;
                out[d + ch] = 255 - v.min(255) as u8;
            }
        }
    }
    common::write_png(std::path::Path::new(&a[3]), ow, h, &out);
}
