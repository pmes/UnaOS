//! CSS Color Module Level 3/4 as SVG uses it: the 147 named colours + `transparent`, `#rgb`, `#rgba`,
//! `#rrggbb`, `#rrggbbaa`, `rgb()`/`rgba()` (integers, percentages, modern space syntax with `/ alpha`),
//! `hsl()`/`hsla()`, and the system-independent keyword `currentColor` (resolved by the caller).

use crate::fmath::round;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    /// 0..=1
    pub a: f32,
}

impl Color {
    pub const BLACK: Color = Color { r: 0, g: 0, b: 0, a: 1.0 };
    pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color { r, g, b, a: 1.0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParsedColor {
    Color(Color),
    CurrentColor,
}

fn clamp_u8(v: f64) -> u8 {
    round(v.clamp(0.0, 255.0)) as u8
}

/// Parse one colour value (leading/trailing whitespace allowed).
pub fn parse(s: &str) -> Option<ParsedColor> {
    let s = s.trim();
    if s.eq_ignore_ascii_case("currentcolor") {
        return Some(ParsedColor::CurrentColor);
    }
    parse_color(s).map(ParsedColor::Color)
}

pub fn parse_color(s: &str) -> Option<Color> {
    let s = s.trim();
    if let Some(h) = s.strip_prefix('#') {
        let hex = |c: u8| (c as char).to_digit(16).map(|v| v as u8);
        let b = h.as_bytes();
        let d: Option<alloc::vec::Vec<u8>> = b.iter().map(|&c| hex(c)).collect();
        let d = d?;
        return match d.len() {
            3 => Some(Color::rgb(d[0] * 17, d[1] * 17, d[2] * 17)),
            4 => Some(Color { r: d[0] * 17, g: d[1] * 17, b: d[2] * 17, a: (d[3] * 17) as f32 / 255.0 }),
            6 => Some(Color::rgb(d[0] * 16 + d[1], d[2] * 16 + d[3], d[4] * 16 + d[5])),
            8 => Some(Color { r: d[0] * 16 + d[1], g: d[2] * 16 + d[3], b: d[4] * 16 + d[5], a: (d[6] * 16 + d[7]) as f32 / 255.0 }),
            _ => None,
        };
    }
    let lower = s.to_ascii_lowercase();
    if let Some(open) = lower.find('(') {
        let fname = lower[..open].trim();
        let inner = lower[open + 1..].strip_suffix(')')?;
        let (main, alpha) = match inner.split_once('/') {
            Some((m, a)) => (m, Some(a.trim())),
            None => (inner, None),
        };
        let mut parts: alloc::vec::Vec<&str> =
            if main.contains(',') { main.split(',').map(|p| p.trim()).collect() } else { main.split_whitespace().collect() };
        let mut alpha = alpha;
        if alpha.is_none() && parts.len() == 4 {
            alpha = parts.pop();
        }
        if parts.len() != 3 {
            return None;
        }
        let a = match alpha {
            Some(a) => {
                if let Some(p) = a.strip_suffix('%') {
                    (p.trim().parse::<f64>().ok()? / 100.0).clamp(0.0, 1.0)
                } else {
                    a.parse::<f64>().ok()?.clamp(0.0, 1.0)
                }
            }
            None => 1.0,
        } as f32;
        match fname {
            "rgb" | "rgba" => {
                // Legacy (comma) syntax: the three channels must all be numbers or all percentages.
                if main.contains(',') {
                    let pc = parts.iter().filter(|p| p.ends_with('%')).count();
                    if pc != 0 && pc != 3 {
                        return None;
                    }
                }
                let ch = |p: &str| -> Option<u8> {
                    if let Some(v) = p.strip_suffix('%') {
                        Some(clamp_u8(v.trim().parse::<f64>().ok()? * 255.0 / 100.0))
                    } else {
                        Some(clamp_u8(p.parse::<f64>().ok()?))
                    }
                };
                return Some(Color { r: ch(parts[0])?, g: ch(parts[1])?, b: ch(parts[2])?, a });
            }
            "hsl" | "hsla" => {
                let h = parts[0].trim_end_matches("deg").parse::<f64>().ok()?;
                let sat = parts[1].strip_suffix('%')?.trim().parse::<f64>().ok()?.clamp(0.0, 100.0) / 100.0;
                let l = parts[2].strip_suffix('%')?.trim().parse::<f64>().ok()?.clamp(0.0, 100.0) / 100.0;
                let h = ((h % 360.0) + 360.0) % 360.0 / 360.0;
                let t2 = if l <= 0.5 { l * (sat + 1.0) } else { l + sat - l * sat };
                let t1 = l * 2.0 - t2;
                let hue = |mut h: f64| {
                    if h < 0.0 {
                        h += 1.0;
                    }
                    if h > 1.0 {
                        h -= 1.0;
                    }
                    if h * 6.0 < 1.0 {
                        t1 + (t2 - t1) * h * 6.0
                    } else if h * 2.0 < 1.0 {
                        t2
                    } else if h * 3.0 < 2.0 {
                        t1 + (t2 - t1) * (2.0 / 3.0 - h) * 6.0
                    } else {
                        t1
                    }
                };
                return Some(Color {
                    r: clamp_u8(hue(h + 1.0 / 3.0) * 255.0),
                    g: clamp_u8(hue(h) * 255.0),
                    b: clamp_u8(hue(h - 1.0 / 3.0) * 255.0),
                    a,
                });
            }
            _ => return None,
        }
    }
    if lower == "transparent" {
        return Some(Color { r: 0, g: 0, b: 0, a: 0.0 });
    }
    NAMED.binary_search_by(|(n, _)| n.cmp(&lower.as_str())).ok().map(|i| {
        let v = NAMED[i].1;
        Color::rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
    })
}

/// CSS Color 3 §4.3 extended colour keywords, sorted for binary search.
static NAMED: &[(&str, u32)] = &[
    ("aliceblue", 0xf0f8ff), ("antiquewhite", 0xfaebd7), ("aqua", 0x00ffff), ("aquamarine", 0x7fffd4),
    ("azure", 0xf0ffff), ("beige", 0xf5f5dc), ("bisque", 0xffe4c4), ("black", 0x000000),
    ("blanchedalmond", 0xffebcd), ("blue", 0x0000ff), ("blueviolet", 0x8a2be2), ("brown", 0xa52a2a),
    ("burlywood", 0xdeb887), ("cadetblue", 0x5f9ea0), ("chartreuse", 0x7fff00), ("chocolate", 0xd2691e),
    ("coral", 0xff7f50), ("cornflowerblue", 0x6495ed), ("cornsilk", 0xfff8dc), ("crimson", 0xdc143c),
    ("cyan", 0x00ffff), ("darkblue", 0x00008b), ("darkcyan", 0x008b8b), ("darkgoldenrod", 0xb8860b),
    ("darkgray", 0xa9a9a9), ("darkgreen", 0x006400), ("darkgrey", 0xa9a9a9), ("darkkhaki", 0xbdb76b),
    ("darkmagenta", 0x8b008b), ("darkolivegreen", 0x556b2f), ("darkorange", 0xff8c00), ("darkorchid", 0x9932cc),
    ("darkred", 0x8b0000), ("darksalmon", 0xe9967a), ("darkseagreen", 0x8fbc8f), ("darkslateblue", 0x483d8b),
    ("darkslategray", 0x2f4f4f), ("darkslategrey", 0x2f4f4f), ("darkturquoise", 0x00ced1), ("darkviolet", 0x9400d3),
    ("deeppink", 0xff1493), ("deepskyblue", 0x00bfff), ("dimgray", 0x696969), ("dimgrey", 0x696969),
    ("dodgerblue", 0x1e90ff), ("firebrick", 0xb22222), ("floralwhite", 0xfffaf0), ("forestgreen", 0x228b22),
    ("fuchsia", 0xff00ff), ("gainsboro", 0xdcdcdc), ("ghostwhite", 0xf8f8ff), ("gold", 0xffd700),
    ("goldenrod", 0xdaa520), ("gray", 0x808080), ("green", 0x008000), ("greenyellow", 0xadff2f),
    ("grey", 0x808080), ("honeydew", 0xf0fff0), ("hotpink", 0xff69b4), ("indianred", 0xcd5c5c),
    ("indigo", 0x4b0082), ("ivory", 0xfffff0), ("khaki", 0xf0e68c), ("lavender", 0xe6e6fa),
    ("lavenderblush", 0xfff0f5), ("lawngreen", 0x7cfc00), ("lemonchiffon", 0xfffacd), ("lightblue", 0xadd8e6),
    ("lightcoral", 0xf08080), ("lightcyan", 0xe0ffff), ("lightgoldenrodyellow", 0xfafad2), ("lightgray", 0xd3d3d3),
    ("lightgreen", 0x90ee90), ("lightgrey", 0xd3d3d3), ("lightpink", 0xffb6c1), ("lightsalmon", 0xffa07a),
    ("lightseagreen", 0x20b2aa), ("lightskyblue", 0x87cefa), ("lightslategray", 0x778899), ("lightslategrey", 0x778899),
    ("lightsteelblue", 0xb0c4de), ("lightyellow", 0xffffe0), ("lime", 0x00ff00), ("limegreen", 0x32cd32),
    ("linen", 0xfaf0e6), ("magenta", 0xff00ff), ("maroon", 0x800000), ("mediumaquamarine", 0x66cdaa),
    ("mediumblue", 0x0000cd), ("mediumorchid", 0xba55d3), ("mediumpurple", 0x9370db), ("mediumseagreen", 0x3cb371),
    ("mediumslateblue", 0x7b68ee), ("mediumspringgreen", 0x00fa9a), ("mediumturquoise", 0x48d1cc), ("mediumvioletred", 0xc71585),
    ("midnightblue", 0x191970), ("mintcream", 0xf5fffa), ("mistyrose", 0xffe4e1), ("moccasin", 0xffe4b5),
    ("navajowhite", 0xffdead), ("navy", 0x000080), ("oldlace", 0xfdf5e6), ("olive", 0x808000),
    ("olivedrab", 0x6b8e23), ("orange", 0xffa500), ("orangered", 0xff4500), ("orchid", 0xda70d6),
    ("palegoldenrod", 0xeee8aa), ("palegreen", 0x98fb98), ("paleturquoise", 0xafeeee), ("palevioletred", 0xdb7093),
    ("papayawhip", 0xffefd5), ("peachpuff", 0xffdab9), ("peru", 0xcd853f), ("pink", 0xffc0cb),
    ("plum", 0xdda0dd), ("powderblue", 0xb0e0e6), ("purple", 0x800080), ("rebeccapurple", 0x663399),
    ("red", 0xff0000), ("rosybrown", 0xbc8f8f), ("royalblue", 0x4169e1), ("saddlebrown", 0x8b4513),
    ("salmon", 0xfa8072), ("sandybrown", 0xf4a460), ("seagreen", 0x2e8b57), ("seashell", 0xfff5ee),
    ("sienna", 0xa0522d), ("silver", 0xc0c0c0), ("skyblue", 0x87ceeb), ("slateblue", 0x6a5acd),
    ("slategray", 0x708090), ("slategrey", 0x708090), ("snow", 0xfffafa), ("springgreen", 0x00ff7f),
    ("steelblue", 0x4682b4), ("tan", 0xd2b48c), ("teal", 0x008080), ("thistle", 0xd8bfd8),
    ("tomato", 0xff6347), ("turquoise", 0x40e0d0), ("violet", 0xee82ee), ("wheat", 0xf5deb3),
    ("white", 0xffffff), ("whitesmoke", 0xf5f5f5), ("yellow", 0xffff00), ("yellowgreen", 0x9acd32),
];

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn colours() {
        assert!(NAMED.windows(2).all(|w| w[0].0 < w[1].0));
        assert_eq!(NAMED.len(), 148);
        assert_eq!(parse_color("#0f8"), Some(Color::rgb(0, 255, 136)));
        assert_eq!(parse_color("#00ff0080").unwrap().a, 128.0 / 255.0);
        assert_eq!(parse_color("rgb(10%, 50%, 100%)"), Some(Color::rgb(26, 128, 255)));
        assert_eq!(parse_color("RGBA(1,2,3,0.5)"), Some(Color { r: 1, g: 2, b: 3, a: 0.5 }));
        assert_eq!(parse_color("rgb(1 2 3 / 50%)"), Some(Color { r: 1, g: 2, b: 3, a: 0.5 }));
        assert_eq!(parse_color("hsl(120, 100%, 25%)"), Some(Color::rgb(0, 128, 0)));
        assert_eq!(parse_color("SeaGreen"), Some(Color::rgb(0x2e, 0x8b, 0x57)));
        assert_eq!(parse("currentColor"), Some(ParsedColor::CurrentColor));
        assert_eq!(parse_color("#12"), None);
        assert_eq!(parse_color("nonsense"), None);
    }
}
