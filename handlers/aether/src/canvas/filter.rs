//! The 2D context's `filter` (HTML §4.12.5.1.19, Filter Effects 1 §13 — AETHERDEBT SR93): the CSS
//! `<filter-value-list>` the attribute holds, parsed into [`Filter`]s and applied to each drawing's layer
//! before its shadow and compositing (the drawing model's step "apply the filter to image A").
//!
//! The filter functions are the shorthand ones: `blur()`, `brightness()`, `contrast()`, `drop-shadow()`,
//! `grayscale()`, `hue-rotate()`, `invert()`, `opacity()`, `saturate()`, `sepia()`. The colour functions
//! are their Filter Effects §13.2 colour matrices (and the component transfers written as matrices) over
//! unpremultiplied RGBA; `blur()` is the three-box Gaussian the shadow uses (`raster::blur`), per channel
//! over premultiplied colour; `drop-shadow()` is a blurred, coloured, offset copy of the alpha composited
//! under the layer. A `url()` reference (an SVG filter) is not supported: such a list does not parse, so
//! the setter leaves the attribute unchanged, as for any value that does not parse.

use super::paint::Rgba;

/// One filter function.
#[derive(Clone, Debug)]
pub enum Filter {
    /// Gaussian blur, standard deviation in device pixels.
    Blur(f64),
    /// A 4×5 colour matrix (row-major, the fifth column an offset) over unpremultiplied RGBA in 0..1.
    Matrix([f32; 20]),
    /// `drop-shadow(dx dy blur color)`.
    DropShadow { dx: f64, dy: f64, blur: f64, color: Rgba },
}

/// Parses a `<filter-value-list>` (or `none`: the empty list). None when it does not parse.
pub fn parse(s: &str) -> Option<Vec<Filter>> {
    let t = s.trim();
    if t.eq_ignore_ascii_case("none") {
        return Some(Vec::new());
    }
    if t.is_empty() {
        return None;
    }
    let b: Vec<char> = t.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        while i < b.len() && (b[i].is_alphanumeric() || b[i] == '-') {
            i += 1;
        }
        if i >= b.len() || b[i] != '(' || i == start {
            return None;
        }
        let name: String = b[start..i].iter().collect::<String>().to_ascii_lowercase();
        i += 1;
        let arg_start = i;
        let mut depth = 1;
        while i < b.len() {
            match b[i] {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        if depth != 0 {
            return None;
        }
        let args: String = b[arg_start..i].iter().collect();
        i += 1;
        out.push(function(&name, args.trim())?);
    }
    Some(out)
}

/// A number or percentage amount (`""` → `default`); negative refused.
fn amount(a: &str, default: f64) -> Option<f64> {
    if a.is_empty() {
        return Some(default);
    }
    let v = match a.strip_suffix('%') {
        Some(p) => p.trim().parse::<f64>().ok()? / 100.0,
        None => a.parse::<f64>().ok()?,
    };
    (v.is_finite() && v >= 0.0).then_some(v)
}

/// A `<length>` in px (`0` unitless); `em` at the canvas's 10 px default font size.
fn length(a: &str) -> Option<f64> {
    let a = a.trim();
    let v = if let Some(n) = a.strip_suffix("px") {
        n.parse::<f64>().ok()?
    } else if let Some(n) = a.strip_suffix("em") {
        n.parse::<f64>().ok()? * 10.0
    } else {
        let n = a.parse::<f64>().ok()?;
        if n != 0.0 {
            return None;
        }
        n
    };
    v.is_finite().then_some(v)
}

/// An `<angle>` in degrees (`0` unitless).
fn angle(a: &str) -> Option<f64> {
    let a = a.trim();
    if a.is_empty() {
        return Some(0.0);
    }
    let (n, k) = if let Some(n) = a.strip_suffix("deg") {
        (n, 1.0)
    } else if let Some(n) = a.strip_suffix("grad") {
        (n, 0.9)
    } else if let Some(n) = a.strip_suffix("rad") {
        (n, 180.0 / std::f64::consts::PI)
    } else if let Some(n) = a.strip_suffix("turn") {
        (n, 360.0)
    } else {
        let v = a.parse::<f64>().ok()?;
        return (v == 0.0).then_some(0.0);
    };
    let v = n.parse::<f64>().ok()? * k;
    v.is_finite().then_some(v)
}

fn function(name: &str, a: &str) -> Option<Filter> {
    let m = |rows: [f64; 20]| {
        let mut o = [0f32; 20];
        for (d, s) in o.iter_mut().zip(rows.iter()) {
            *d = *s as f32;
        }
        Filter::Matrix(o)
    };
    Some(match name {
        "blur" => {
            let r = if a.is_empty() { 0.0 } else { length(a)? };
            if r < 0.0 {
                return None;
            }
            Filter::Blur(r)
        }
        "brightness" => {
            let v = amount(a, 1.0)?;
            m([v, 0.0, 0.0, 0.0, 0.0, 0.0, v, 0.0, 0.0, 0.0, 0.0, 0.0, v, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0])
        }
        "contrast" => {
            let v = amount(a, 1.0)?;
            let o = 0.5 - 0.5 * v;
            m([v, 0.0, 0.0, 0.0, o, 0.0, v, 0.0, 0.0, o, 0.0, 0.0, v, 0.0, o, 0.0, 0.0, 0.0, 1.0, 0.0])
        }
        "opacity" => {
            let v = amount(a, 1.0)?.min(1.0);
            m([1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, v, 0.0])
        }
        "invert" => {
            let v = amount(a, 1.0)?.min(1.0);
            let d = 1.0 - 2.0 * v;
            m([d, 0.0, 0.0, 0.0, v, 0.0, d, 0.0, 0.0, v, 0.0, 0.0, d, 0.0, v, 0.0, 0.0, 0.0, 1.0, 0.0])
        }
        "grayscale" => {
            let x = 1.0 - amount(a, 1.0)?.min(1.0);
            m([
                0.2126 + 0.7874 * x, 0.7152 - 0.7152 * x, 0.0722 - 0.0722 * x, 0.0, 0.0,
                0.2126 - 0.2126 * x, 0.7152 + 0.2848 * x, 0.0722 - 0.0722 * x, 0.0, 0.0,
                0.2126 - 0.2126 * x, 0.7152 - 0.7152 * x, 0.0722 + 0.9278 * x, 0.0, 0.0,
                0.0, 0.0, 0.0, 1.0, 0.0,
            ])
        }
        "sepia" => {
            let x = 1.0 - amount(a, 1.0)?.min(1.0);
            m([
                0.393 + 0.607 * x, 0.769 - 0.769 * x, 0.189 - 0.189 * x, 0.0, 0.0,
                0.349 - 0.349 * x, 0.686 + 0.314 * x, 0.168 - 0.168 * x, 0.0, 0.0,
                0.272 - 0.272 * x, 0.534 - 0.534 * x, 0.131 + 0.869 * x, 0.0, 0.0,
                0.0, 0.0, 0.0, 1.0, 0.0,
            ])
        }
        "saturate" => {
            let s = amount(a, 1.0)?;
            m([
                0.213 + 0.787 * s, 0.715 - 0.715 * s, 0.072 - 0.072 * s, 0.0, 0.0,
                0.213 - 0.213 * s, 0.715 + 0.285 * s, 0.072 - 0.072 * s, 0.0, 0.0,
                0.213 - 0.213 * s, 0.715 - 0.715 * s, 0.072 + 0.928 * s, 0.0, 0.0,
                0.0, 0.0, 0.0, 1.0, 0.0,
            ])
        }
        "hue-rotate" => {
            let r = angle(a)?.to_radians();
            let (c, s) = (r.cos(), r.sin());
            m([
                0.213 + c * 0.787 - s * 0.213, 0.715 - c * 0.715 - s * 0.715, 0.072 - c * 0.072 + s * 0.928, 0.0, 0.0,
                0.213 - c * 0.213 + s * 0.143, 0.715 + c * 0.285 + s * 0.140, 0.072 - c * 0.072 - s * 0.283, 0.0, 0.0,
                0.213 - c * 0.213 - s * 0.787, 0.715 - c * 0.715 + s * 0.715, 0.072 + c * 0.928 + s * 0.072, 0.0, 0.0,
                0.0, 0.0, 0.0, 1.0, 0.0,
            ])
        }
        "drop-shadow" => drop_shadow(a)?,
        _ => return None,
    })
}

/// `drop-shadow(<color>? && <length>{2,3})` — the colour defaults to opaque black (a canvas has no
/// `currentColor` but its own: black).
fn drop_shadow(a: &str) -> Option<Filter> {
    // Split at top-level whitespace (a colour function keeps its parentheses).
    let mut parts: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut depth = 0;
    for ch in a.chars() {
        match ch {
            '(' => {
                depth += 1;
                cur.push(ch);
            }
            ')' => {
                depth -= 1;
                cur.push(ch);
            }
            c if c.is_whitespace() && depth == 0 => {
                if !cur.is_empty() {
                    parts.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        parts.push(cur);
    }
    let mut lens = Vec::new();
    let mut color: Option<Rgba> = None;
    for p in parts {
        if let Some(l) = length(&p) {
            lens.push(l);
        } else if color.is_none() {
            color = Some(parse_color(&p)?);
        } else {
            return None;
        }
    }
    if lens.len() < 2 || lens.len() > 3 {
        return None;
    }
    let blur = lens.get(2).copied().unwrap_or(0.0);
    if blur < 0.0 {
        return None;
    }
    Some(Filter::DropShadow { dx: lens[0], dy: lens[1], blur, color: color.unwrap_or([0.0, 0.0, 0.0, 1.0]) })
}

fn parse_color(s: &str) -> Option<Rgba> {
    let cv = css_core::parser::parse_component_values(s.trim());
    match css_core::color::parse_color(&cv)? {
        css_core::color::Color::Rgba(c) => Some([(c.r / 255.0).clamp(0.0, 1.0), (c.g / 255.0).clamp(0.0, 1.0), (c.b / 255.0).clamp(0.0, 1.0), c.a.clamp(0.0, 1.0)]),
        css_core::color::Color::CurrentColor => Some([0.0, 0.0, 0.0, 1.0]),
    }
}

/// Applies a colour matrix to premultiplied pixels (unpremultiplied for the matrix, then back).
pub fn apply_matrix(data: &mut [[f32; 4]], m: &[f32; 20]) {
    for p in data.iter_mut() {
        let a = p[3];
        let (r, g, b) = if a > 0.0 { (p[0] / a, p[1] / a, p[2] / a) } else { (0.0, 0.0, 0.0) };
        let row = |k: usize| (m[k * 5] * r + m[k * 5 + 1] * g + m[k * 5 + 2] * b + m[k * 5 + 3] * a + m[k * 5 + 4]).clamp(0.0, 1.0);
        let (nr, ng, nb, na) = (row(0), row(1), row(2), row(3));
        *p = [nr * na, ng * na, nb * na, na];
    }
}

/// Blurs a `w × h` premultiplied layer by `sigma`, padded by three standard deviations each side.
/// Returns (pixels, new width, new height, pad).
pub fn blur_layer(data: &[[f32; 4]], w: usize, h: usize, sigma: f64) -> (Vec<[f32; 4]>, usize, usize, usize) {
    let pad = (sigma * 3.0).ceil() as usize + 1;
    let (nw, nh) = (w + 2 * pad, h + 2 * pad);
    if nw.saturating_mul(nh) > super::context::MAX_PIXELS {
        return (data.to_vec(), w, h, 0);
    }
    let mut out = vec![[0f32; 4]; nw * nh];
    for c in 0..4 {
        let mut plane = vec![0f32; nw * nh];
        for y in 0..h {
            for x in 0..w {
                plane[(y + pad) * nw + x + pad] = data[y * w + x][c];
            }
        }
        super::raster::blur(&mut plane, nw, nh, sigma);
        for (o, v) in out.iter_mut().zip(plane.iter()) {
            o[c] = *v;
        }
    }
    (out, nw, nh, pad)
}
