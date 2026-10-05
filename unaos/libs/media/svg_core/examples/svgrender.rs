//! Dev tool: `cargo run --release -p svg_core --example svgrender -- in.svg out.png [W H]`
//! Fonts from $SVGCORE_FONTS (a directory).
#[path = "../tests/common/mod.rs"]
mod common;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&a[1]).unwrap();
    let svg = svg_core::Svg::parse(&bytes).expect("parse");
    let (mut w, mut h) = svg.size();
    if a.len() >= 5 {
        w = a[3].parse().unwrap();
        h = a[4].parse().unwrap();
    }
    let fonts = std::env::var("SVGCORE_FONTS").ok().map(|d| common::fonts_from(std::path::Path::new(&d)));
    let opts = svg_core::Options { fonts: fonts.as_ref(), image_decoder: Some(common::decoder), languages: vec![] };
    let t = std::time::Instant::now();
    let rgba = svg.render_rgba(w, h, &opts).unwrap();
    eprintln!("{}x{} in {:?}", w, h, t.elapsed());
    common::write_png(std::path::Path::new(&a[2]), w as usize, h as usize, &rgba);
}
