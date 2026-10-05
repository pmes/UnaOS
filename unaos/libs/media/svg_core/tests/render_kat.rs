//! Known-answer renders: every expected pixel below follows from the spec by hand (exact areas, exact colours).
mod common;

use svg_core::{Options, Svg};

fn render(src: &str, w: u32, h: u32) -> Vec<u8> {
    Svg::parse(src.as_bytes()).unwrap().render_rgba(w, h, &Options::default()).unwrap()
}

fn px(img: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [img[i], img[i + 1], img[i + 2], img[i + 3]]
}

const NS: &str = r##"xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink""##;

#[test]
fn rect_viewbox_and_half_pixel_edges() {
    // viewBox 0 0 10 10 onto 20x20: the rect 2.25..7.25 maps to 4.5..14.5 → half-covered edge pixels.
    let img = render(&format!(r##"<svg {NS} viewBox="0 0 10 10"><rect x="2.25" y="2.25" width="5" height="5" fill="#ff0000"/></svg>"##), 20, 20);
    assert_eq!(px(&img, 20, 8, 8), [255, 0, 0, 255]);
    assert_eq!(px(&img, 20, 4, 8), [255, 0, 0, 128]);
    assert_eq!(px(&img, 20, 4, 4), [255, 0, 0, 64]);
    assert_eq!(px(&img, 20, 3, 8)[3], 0);
}

#[test]
fn fill_rules_opacity_and_group_layer() {
    let s = format!(
        r##"<svg {NS} width="30" height="10">
        <path d="M0 0H10V10H0Z M2 2H8V8H2Z" fill-rule="evenodd" fill="blue"/>
        <path d="M10 0H20V10H10Z M12 2H18V8H12Z" fill="blue"/>
        <g opacity="0.5"><rect x="20" width="10" height="10" fill="lime"/><rect x="20" width="10" height="10" fill="lime"/></g></svg>"##
    );
    let img = render(&s, 30, 10);
    assert_eq!(px(&img, 30, 5, 5)[3], 0); // even-odd hole
    assert_eq!(px(&img, 30, 15, 5), [0, 0, 255, 255]); // nonzero: same winding → filled
    // Group opacity applies once to the composited group (not 0.75 from two blended halves).
    assert_eq!(px(&img, 30, 25, 5), [0, 255, 0, 128]);
}

#[test]
fn stroke_joins_and_dashes() {
    let s = format!(r##"<svg {NS} width="40" height="20"><path d="M5 10H35" stroke="black" stroke-width="4" stroke-dasharray="5 5" fill="none"/></svg>"##);
    let img = render(&s, 40, 20);
    assert_eq!(px(&img, 40, 7, 10)[3], 255); // in a dash (5..10)
    assert_eq!(px(&img, 40, 12, 10)[3], 0); // in a gap (10..15)
    assert_eq!(px(&img, 40, 17, 9)[3], 255);
    assert_eq!(px(&img, 40, 17, 12)[3], 0); // beyond the half width
}

#[test]
fn linear_gradient_midpoint_and_spread() {
    let s = format!(
        r##"<svg {NS} width="100" height="2"><linearGradient id="g" x1="0" x2="0.5" spreadMethod="reflect">
        <stop offset="0" stop-color="#000"/><stop offset="1" stop-color="#fff"/></linearGradient>
        <rect width="100" height="2" fill="url(#g)"/></svg>"##
    );
    let img = render(&s, 100, 2);
    // Pixel centre 25.5 → t = 0.51 → 130; reflected at 74.5 → t = 0.51 → 130.
    assert_eq!(px(&img, 100, 25, 0)[0], 130);
    assert_eq!(px(&img, 100, 74, 0)[0], 130);
    assert_eq!(px(&img, 100, 0, 0)[0], 3);
}

#[test]
fn clip_path_mask_and_use() {
    let s = format!(
        r##"<svg {NS} width="20" height="10">
        <defs><clipPath id="c"><rect width="5" height="10"/></clipPath>
        <mask id="m" maskUnits="userSpaceOnUse"><rect x="10" width="10" height="10" fill="#808080"/></mask>
        <rect id="r" width="10" height="10" fill="red"/></defs>
        <use xlink:href="#r" clip-path="url(#c)"/>
        <use xlink:href="#r" x="10" fill="blue" mask="url(#m)"/></svg>"##
    );
    let img = render(&s, 20, 10);
    assert_eq!(px(&img, 20, 2, 5), [255, 0, 0, 255]);
    assert_eq!(px(&img, 20, 7, 5)[3], 0);
    // Mask luminance of #808080 = 128/255; the use's fill does not override the rect's own fill.
    assert_eq!(px(&img, 20, 15, 5), [255, 0, 0, 128]);
}

#[test]
fn style_sheet_and_inheritance() {
    let s = format!(
        r##"<svg {NS} width="10" height="10"><style>.a {{ fill: lime }} #b {{ fill: blue !important }}</style>
        <g fill="red"><rect class="a" width="5" height="10"/><rect id="b" x="5" width="5" height="10" style="fill: red"/></g></svg>"##
    );
    let img = render(&s, 10, 10);
    assert_eq!(px(&img, 10, 2, 5), [0, 255, 0, 255]);
    assert_eq!(px(&img, 10, 7, 5), [0, 0, 255, 255]);
}

#[test]
fn intrinsic_sizes_and_refusals() {
    assert_eq!(Svg::parse(format!(r##"<svg {NS} width="2in" height="1in"/>"##).as_bytes()).unwrap().size(), (192, 96));
    assert_eq!(Svg::parse(format!(r##"<svg {NS} viewBox="0 0 40 20" width="80"/>"##).as_bytes()).unwrap().size(), (80, 40));
    assert_eq!(Svg::parse(format!(r##"<svg {NS}/>"##).as_bytes()).unwrap().size(), (100, 100));
    assert!(Svg::parse(b"<svg/>").is_err()); // no SVG namespace
    assert!(Svg::parse(b"<html xmlns='http://www.w3.org/2000/svg'/>").is_err());
    assert!(Svg::parse(b"not xml").is_err());
    assert!(svg_core::sniff_svg(b"\xEF\xBB\xBF<?xml version='1.0'?>\n<!-- x --><svg>"));
    assert!(svg_core::sniff_svg(b"  <!DOCTYPE svg PUBLIC '-//W3C//DTD SVG 1.1//EN' 'x'><svg>"));
    assert!(!svg_core::sniff_svg(b"<?xml version='1.0'?><html/>"));
    assert!(!svg_core::sniff_svg(b"\x89PNG"));
}

#[test]
fn text_draws_with_fonts() {
    let dir = std::path::Path::new("/usr/share/fonts/truetype/dejavu");
    if !dir.exists() {
        eprintln!("SKIP: no DejaVu fonts");
        return;
    }
    let mut fonts = common::fonts_from(dir);
    fonts.default_family = "DejaVu Sans".into();
    let s = format!(r##"<svg {NS} width="60" height="30"><text x="5" y="22" font-family="DejaVu Sans" font-size="20">Hi</text></svg>"##);
    let svg = Svg::parse(s.as_bytes()).unwrap();
    let opts = Options { fonts: Some(&fonts), ..Default::default() };
    let img = svg.render_rgba(60, 30, &opts).unwrap();
    let ink: u32 = img.chunks_exact(4).map(|p| p[3] as u32).sum();
    // "Hi" at 20px: H stem + crossbar + i ≈ 60–80 px² of ink.
    assert!(ink > 40 * 255 && ink < 110 * 255, "ink {}", ink / 255);
    // Without fonts the text is skipped, never a panic.
    assert!(svg.render_rgba(60, 30, &Options::default()).unwrap().iter().all(|&b| b == 0));
}

#[test]
fn filters_region_flood_offset_morphology() {
    // feFlood fills exactly the userSpaceOnUse filter region; half-transparent blue survives the
    // linearRGB round trip (1.0 is 1.0 in both spaces).
    let s = format!(
        r##"<svg {NS} width="20" height="20"><filter id="f" filterUnits="userSpaceOnUse" x="5" y="5" width="10" height="10">
        <feFlood flood-color="#00f" flood-opacity="0.5"/></filter><rect width="1" height="1" filter="url(#f)"/></svg>"##
    );
    let img = render(&s, 20, 20);
    assert_eq!(px(&img, 20, 5, 5), [0, 0, 255, 128]);
    assert_eq!(px(&img, 20, 14, 14), [0, 0, 255, 128]);
    assert_eq!(px(&img, 20, 4, 10)[3], 0);
    assert_eq!(px(&img, 20, 15, 10)[3], 0);
    // feOffset by 3 user units, then feMorphology dilate radius 1 (a 2 px bar becomes 4 px).
    let s = format!(
        r##"<svg {NS} width="20" height="4"><filter id="f" filterUnits="userSpaceOnUse" x="0" y="0" width="20" height="4">
        <feOffset dx="3"/><feMorphology operator="dilate" radius="1 0"/></filter>
        <rect x="2" width="2" height="4" fill="#f00" filter="url(#f)"/></svg>"##
    );
    let img = render(&s, 20, 4);
    let row: Vec<u8> = (0..10).map(|x| px(&img, 20, x, 1)[3]).collect();
    assert_eq!(row, vec![0, 0, 0, 0, 255, 255, 255, 255, 0, 0]);
}

#[test]
fn filters_color_interpolation_linear_vs_srgb() {
    // feComponentTransfer slope 0.5 on red: in sRGB 255 → 127 (truncated table); in linearRGB the halved
    // linear value 127 maps back to sRGB 187 (1.055·(127/255)^(1/2.4) − 0.055 = 0.734).
    let doc = |cif: &str| {
        format!(
            r##"<svg {NS} width="4" height="4"><filter id="f" color-interpolation-filters="{cif}"><feComponentTransfer>
            <feFuncR type="linear" slope="0.5"/></feComponentTransfer></filter>
            <rect width="4" height="4" fill="#f00" filter="url(#f)"/></svg>"##
        )
    };
    assert_eq!(px(&render(&doc("sRGB"), 4, 4), 4, 2, 2), [127, 0, 0, 255]);
    assert_eq!(px(&render(&doc("linearRGB"), 4, 4), 4, 2, 2), [187, 0, 0, 255]);
}

#[test]
fn filters_blur_keeps_mass_and_symmetry() {
    // stdDeviation 2 → box size d = ⌊2·3·√(2π)/4 + 0.5⌋ = 4 (even): the two half-pixel-shifted boxes and the
    // d+1 box make a symmetric kernel; a 1 px line keeps its integral (±rounding) and its symmetry.
    let s = format!(
        r##"<svg {NS} width="41" height="1"><filter id="f" filterUnits="userSpaceOnUse" x="0" y="0" width="41" height="1"
        color-interpolation-filters="sRGB"><feGaussianBlur stdDeviation="2 0"/></filter>
        <rect x="20" width="1" height="1" filter="url(#f)"/></svg>"##
    );
    let img = render(&s, 41, 1);
    let a: Vec<u32> = (0..41).map(|x| px(&img, 41, x, 0)[3] as u32).collect();
    for k in 0..41 {
        assert_eq!(a[k], a[40 - k]);
    }
    let sum: u32 = a.iter().sum();
    assert!((250..=260).contains(&sum), "mass {sum}");
    // Support: 3d − 1 = 11 taps → x = 15..=25.
    assert_eq!((a[14], a[26]), (0, 0));
    assert!(a[15] > 0 && a[25] > 0);
}

#[test]
fn filters_css_functions_and_invalid_references() {
    // A missing filter renders the element unfiltered; an empty <filter> renders nothing; hue-rotate(180deg)
    // of pure red (the spec's matrix) gives (0, 109, 109)-ish cyan.
    let s = format!(
        r##"<svg {NS} width="30" height="10"><filter id="empty"/>
        <rect width="10" height="10" fill="#f00" filter="url(#missing)"/>
        <rect x="10" width="10" height="10" fill="#f00" filter="url(#empty)"/>
        <rect x="20" width="10" height="10" fill="#f00" filter="hue-rotate(180deg)"/></svg>"##
    );
    let img = render(&s, 30, 10);
    assert_eq!(px(&img, 30, 5, 5), [255, 0, 0, 255]);
    assert_eq!(px(&img, 30, 15, 5)[3], 0);
    let c = px(&img, 30, 25, 5);
    // R' = 0.213 − 0.787 = −0.574 → 0; G' = 0.213 + 0.213 = 0.426 → 109; B' = 0.213 + 0.213 → 109.
    assert_eq!(c, [0, 109, 109, 255]);
}
