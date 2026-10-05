//! Paint effects: linear gradients (css-images-3 §3.1) and outer box
//! shadows (css-backgrounds-3 §7.1). Parsing lives here too, next to the
//! painter, because both are pure value -> pixels functions.

/// A colour with alpha (0..1).
pub type Rgba = ((u8, u8, u8), f32);

/// Parses a colour with its alpha: rgba()/rgb() with a 4th component,
/// `#rgba`/`#rrggbbaa`, `transparent`, else any opaque colour.
pub fn parse_color_alpha(v: &str) -> Option<Rgba> {
    let t = v.trim().to_ascii_lowercase();
    if t == "transparent" {
        return Some(((0, 0, 0), 0.0));
    }
    if let Some(hex) = t.strip_prefix('#') {
        let n = |s: &str| u8::from_str_radix(s, 16).ok();
        match hex.len() {
            4 => {
                let d: Vec<u8> = hex.chars().filter_map(|c| n(&format!("{c}{c}"))).collect();
                if d.len() == 4 {
                    return Some(((d[0], d[1], d[2]), d[3] as f32 / 255.0));
                }
            }
            8 => {
                let d: Vec<u8> = (0..4).filter_map(|i| n(&hex[i * 2..i * 2 + 2])).collect();
                if d.len() == 4 {
                    return Some(((d[0], d[1], d[2]), d[3] as f32 / 255.0));
                }
            }
            _ => {}
        }
    }
    if let Some(inner) = t.strip_prefix("rgba(").or_else(|| t.strip_prefix("rgb(")).and_then(|s| s.strip_suffix(')')) {
        let parts: Vec<&str> = inner.split([',', ' ', '/']).filter(|s| !s.trim().is_empty()).collect();
        if parts.len() == 4 {
            let a = parts[3].trim();
            let alpha = match a.strip_suffix('%') {
                Some(p) => p.parse::<f32>().ok().map(|p| p / 100.0),
                None => a.parse::<f32>().ok(),
            }?;
            let rgb = crate::css::parse_color_str(&format!("rgb({}, {}, {})", parts[0], parts[1], parts[2]))?;
            return Some((rgb, alpha.clamp(0.0, 1.0)));
        }
    }
    crate::css::parse_color_str(&t).map(|c| (c, 1.0))
}

/// Splits on commas outside parentheses.
fn split_commas(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut start) = (0i32, 0usize);
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                out.push(s[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(s[start..].trim());
    out
}

/// A parsed `linear-gradient()`: direction angle in degrees (CSS: 0 = to
/// top, clockwise) and colour stops with optional positions (fraction of
/// the gradient line when < 0 = unset; px stored as `px` for later).
#[derive(Clone, Debug, PartialEq)]
pub struct Gradient {
    pub angle: f32,
    pub stops: Vec<(Rgba, Option<StopPos>)>,
    pub repeating: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StopPos {
    Fraction(f32),
    Px(f32),
}

/// Finds and parses the first `linear-gradient(...)` /
/// `repeating-linear-gradient(...)` in a background value.
pub fn parse_linear_gradient(value: &str) -> Option<Gradient> {
    let lower = value.to_ascii_lowercase();
    let (start, repeating) = match lower.find("repeating-linear-gradient(") {
        Some(i) => (i + "repeating-linear-gradient(".len(), true),
        None => (lower.find("linear-gradient(")? + "linear-gradient(".len(), false),
    };
    let mut depth = 1;
    let mut end = None;
    for (i, c) in lower[start..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(start + i);
                    break;
                }
            }
            _ => {}
        }
    }
    let args = split_commas(&lower[start..end?]);
    let mut angle = 180.0;
    let mut first = 0;
    if let Some(a) = args.first() {
        if let Some(dir) = a.strip_prefix("to ") {
            let has = |w: &str| dir.split_whitespace().any(|x| x == w);
            angle = match (has("top"), has("bottom"), has("left"), has("right")) {
                (true, _, false, false) => 0.0,
                (_, true, false, false) => 180.0,
                (false, false, true, _) => 270.0,
                (false, false, _, true) => 90.0,
                (true, _, _, true) => 45.0,
                (_, true, _, true) => 135.0,
                (_, true, true, _) => 225.0,
                (true, _, true, _) => 315.0,
                _ => 180.0,
            };
            first = 1;
        } else if let Some(deg) = parse_angle(a) {
            angle = deg;
            first = 1;
        }
    }
    let mut stops = Vec::new();
    for s in &args[first..] {
        // colour [pos [pos]]: the colour may hold spaces inside ().
        let parts = crate::css::split_top_level(s);
        let Some(c) = parts.first().and_then(|c| parse_color_alpha(c)) else { continue };
        let pos = |p: &str| -> Option<StopPos> {
            if let Some(f) = p.strip_suffix('%').and_then(|n| n.trim().parse::<f32>().ok()) {
                Some(StopPos::Fraction(f / 100.0))
            } else {
                crate::css::parse_px(p).map(StopPos::Px)
            }
        };
        let positions: Vec<StopPos> = parts[1..].iter().filter_map(|p| pos(p)).collect();
        if positions.is_empty() {
            stops.push((c, None));
        }
        for p in positions {
            stops.push((c, Some(p)));
        }
    }
    (stops.len() >= 2).then_some(Gradient { angle, stops, repeating })
}

fn parse_angle(a: &str) -> Option<f32> {
    let a = a.trim();
    for (unit, k) in [("deg", 1.0), ("grad", 0.9), ("rad", 180.0 / std::f32::consts::PI), ("turn", 360.0)] {
        if let Some(n) = a.strip_suffix(unit).and_then(|n| n.trim().parse::<f32>().ok()) {
            return Some(n * k);
        }
    }
    None
}

/// Resolved stops (fraction, colour) per css-images-3 §3.4.3: missing
/// first/last are 0/1, a position below an earlier one is raised to it,
/// runs of unpositioned stops spread evenly between their neighbours.
fn resolve_stops(g: &Gradient, len: f32) -> Vec<(f32, Rgba)> {
    let n = g.stops.len();
    let mut pos: Vec<Option<f32>> = g
        .stops
        .iter()
        .map(|(_, p)| p.map(|p| match p { StopPos::Fraction(f) => f, StopPos::Px(px) => if len > 0.0 { px / len } else { 0.0 } }))
        .collect();
    if pos[0].is_none() {
        pos[0] = Some(0.0);
    }
    if pos[n - 1].is_none() {
        pos[n - 1] = Some(1.0);
    }
    let mut maxp = f32::MIN;
    for p in pos.iter_mut().flatten() {
        if *p < maxp {
            *p = maxp;
        }
        maxp = *p;
    }
    let mut i = 0;
    while i < n {
        if pos[i].is_none() {
            let a = i - 1;
            let mut b = i;
            while pos[b].is_none() {
                b += 1;
            }
            let (pa, pb) = (pos[a].unwrap(), pos[b].unwrap());
            for (k, p) in pos.iter_mut().enumerate().take(b).skip(i) {
                *p = Some(pa + (pb - pa) * (k - a) as f32 / (b - a) as f32);
            }
            i = b;
        }
        i += 1;
    }
    pos.into_iter().zip(g.stops.iter()).map(|(p, (c, _))| (p.unwrap(), *c)).collect()
}

/// The gradient's colour at line position `t` (premultiplied interpolation).
fn color_at(stops: &[(f32, Rgba)], t: f32) -> Rgba {
    if t <= stops[0].0 {
        return stops[0].1;
    }
    for w in stops.windows(2) {
        let ((p0, c0), (p1, c1)) = (w[0], w[1]);
        if t <= p1 {
            let f = if p1 > p0 { (t - p0) / (p1 - p0) } else { 1.0 };
            let a = c0.1 + (c1.1 - c0.1) * f;
            let ch = |x: u8, y: u8| -> u8 {
                let v = (x as f32 * c0.1) + (y as f32 * c1.1 - x as f32 * c0.1) * f;
                if a > 0.0 { (v / a).round().clamp(0.0, 255.0) as u8 } else { 0 }
            };
            return ((ch(c0.0 .0, c1.0 .0), ch(c0.0 .1, c1.0 .1), ch(c0.0 .2, c1.0 .2)), a);
        }
    }
    stops[stops.len() - 1].1
}

/// Paints a linear gradient over a w x h box at (x0, y0); `blend(x, y, c, a)`.
#[allow(clippy::too_many_arguments)]
pub fn paint_gradient(
    g: &Gradient, x0: f32, y0: f32, w: f32, h: f32, screen_w: u32, screen_h: u32,
    coverage: &dyn Fn(f32, f32) -> f32, blend: &mut dyn FnMut(u32, u32, (u8, u8, u8), f32),
) {
    let rad = g.angle.to_radians();
    let (s, c) = (rad.sin(), rad.cos());
    // §3.1.1: the gradient line passes through the centre; its length makes
    // the corners hit 0% and 100%.
    let len = (w * s).abs() + (h * c).abs();
    let stops = resolve_stops(g, len);
    let (first, last) = (stops[0].0, stops[stops.len() - 1].0);
    let (cx, cy) = (x0 + w / 2.0, y0 + h / 2.0);
    let sx0 = x0.floor().max(0.0) as u32;
    let sy0 = y0.floor().max(0.0) as u32;
    let sx1 = ((x0 + w).ceil().max(0.0) as u32).min(screen_w);
    let sy1 = ((y0 + h).ceil().max(0.0) as u32).min(screen_h);
    for py in sy0..sy1 {
        for px in sx0..sx1 {
            let (fx, fy) = (px as f32 + 0.5, py as f32 + 0.5);
            let cov = coverage(fx, fy);
            if cov <= 0.0 {
                continue;
            }
            let mut t = if len > 0.0 { ((fx - cx) * s - (fy - cy) * c) / len + 0.5 } else { 0.0 };
            if g.repeating && last > first {
                t = first + (t - first).rem_euclid(last - first);
            }
            let (col, a) = color_at(&stops, t);
            blend(px, py, col, a * cov);
        }
    }
}

/// One outer box shadow: x/y offset, blur radius, spread, colour.
#[derive(Clone, Debug, PartialEq)]
pub struct Shadow {
    pub dx: f32,
    pub dy: f32,
    pub blur: f32,
    pub spread: f32,
    pub color: Rgba,
}

/// Parses `box-shadow` (outer shadows; `inset` ones are skipped).
pub fn parse_box_shadow(value: &str) -> Vec<Shadow> {
    let mut out = Vec::new();
    if value.trim().eq_ignore_ascii_case("none") {
        return out;
    }
    for layer in split_commas(value) {
        let parts = crate::css::split_top_level(layer);
        if parts.iter().any(|p| p.eq_ignore_ascii_case("inset")) {
            continue;
        }
        let mut lens = Vec::new();
        let mut color = ((0, 0, 0), 1.0);
        for p in parts {
            if let Some(l) = crate::css::parse_px(p).filter(|_| p.starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '.')) {
                lens.push(l);
            } else if let Some(c) = parse_color_alpha(p) {
                color = c;
            }
        }
        if lens.len() >= 2 {
            out.push(Shadow {
                dx: lens[0],
                dy: lens[1],
                blur: lens.get(2).copied().unwrap_or(0.0).max(0.0),
                spread: lens.get(3).copied().unwrap_or(0.0),
                color,
            });
        }
    }
    out
}

/// erf by Abramowitz & Stegun 7.1.26 (|error| < 1.5e-7).
fn erf(x: f32) -> f32 {
    let s = x.signum();
    let x = x.abs();
    let t = 1.0 / (1.0 + 0.327_591_1 * x);
    let y = 1.0 - (((((1.061_405_4 * t - 1.453_152_1) * t) + 1.421_413_7) * t - 0.284_496_74) * t + 0.254_829_6) * t * (-x * x).exp();
    s * y
}

/// Paints outer shadows of a box (x0, y0, w, h, radii) outside its border
/// box (§7.1.1: an outer shadow is clipped by the box's own border box).
/// The blur is a Gaussian of sigma = blur / 2 (§7.1.2), applied to the
/// shadow shape's signed distance (exact for straight edges).
#[allow(clippy::too_many_arguments)]
pub fn paint_shadows(
    shadows: &[Shadow], x0: f32, y0: f32, w: f32, h: f32, radii: [f32; 4], screen_w: u32, screen_h: u32,
    sdf: &dyn Fn(f32, f32, f32, f32, f32, f32, [f32; 4]) -> f32,
    blend: &mut dyn FnMut(u32, u32, (u8, u8, u8), f32),
) {
    for s in shadows.iter().rev() {
        let sigma = s.blur / 2.0;
        let (bx0, by0) = (x0 + s.dx - s.spread, y0 + s.dy - s.spread);
        let (bx1, by1) = (x0 + w + s.dx + s.spread, y0 + h + s.dy + s.spread);
        let r = radii.map(|v| if v > 0.0 { (v + s.spread).max(0.0) } else { 0.0 });
        let reach = s.blur + 1.0;
        let sx0 = (bx0 - reach).floor().max(0.0) as u32;
        let sy0 = (by0 - reach).floor().max(0.0) as u32;
        let sx1 = ((bx1 + reach).ceil().max(0.0) as u32).min(screen_w);
        let sy1 = ((by1 + reach).ceil().max(0.0) as u32).min(screen_h);
        for py in sy0..sy1 {
            for px in sx0..sx1 {
                let (fx, fy) = (px as f32 + 0.5, py as f32 + 0.5);
                let inside_box = (0.5 - sdf(fx, fy, x0, y0, x0 + w, y0 + h, radii)).clamp(0.0, 1.0);
                if inside_box >= 1.0 {
                    continue;
                }
                let d = sdf(fx, fy, bx0, by0, bx1, by1, r);
                let a = if sigma > 0.0 {
                    0.5 * (1.0 - erf(d / (sigma * std::f32::consts::SQRT_2)))
                } else {
                    (0.5 - d).clamp(0.0, 1.0)
                };
                let a = a * s.color.1 * (1.0 - inside_box);
                if a > 0.002 {
                    blend(px, py, s.color.0, a);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gradient_parse_and_stops_kat() {
        let g = parse_linear_gradient("linear-gradient(#5b2a86, #3a1c5a)").unwrap();
        assert_eq!(g.angle, 180.0);
        let s = resolve_stops(&g, 100.0);
        assert_eq!((s[0].0, s[1].0), (0.0, 1.0));
        let g = parse_linear_gradient("linear-gradient(to right, red, lime 20%, blue)").unwrap();
        assert_eq!(g.angle, 90.0);
        let s = resolve_stops(&g, 100.0);
        assert_eq!(s.iter().map(|x| x.0).collect::<Vec<_>>(), vec![0.0, 0.2, 1.0]);
        // Midpoint of black -> white is mid grey.
        let g = parse_linear_gradient("linear-gradient(90deg, #000, #fff)").unwrap();
        let s = resolve_stops(&g, 100.0);
        let (c, a) = color_at(&s, 0.5);
        assert!((c.0 as i32 - 128).abs() <= 1 && a == 1.0);
    }

    #[test]
    fn box_shadow_parse_kat() {
        let s = parse_box_shadow("0 1px 4px rgba(0,0,0,.2)");
        assert_eq!(s.len(), 1);
        assert_eq!((s[0].dx, s[0].dy, s[0].blur, s[0].spread), (0.0, 1.0, 4.0, 0.0));
        assert!((s[0].color.1 - 0.2).abs() < 1e-6);
        assert!(parse_box_shadow("inset 0 0 3px red").is_empty());
        assert!((erf(1.0) - 0.842_700_8).abs() < 1e-5);
    }
}
