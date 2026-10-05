//! CSS Color Level 4 (the sRGB part): named colors, `transparent`, `currentcolor`, the system colors (as
//! Chromium's light scheme resolves them), hex notations, `rgb()` / `rgba()` / `hsl()` / `hsla()` / `hwb()`
//! in both the legacy (comma) and modern (space, `/ alpha`, `none`) syntaxes.

use alloc::string::String;
use alloc::vec::Vec;

use crate::parser::{trim_ws, CV};
use crate::tokenizer::Token;

/// An sRGB color, channels 0–255 (unclamped fractions kept), alpha 0–1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgba {
    pub r: f64,
    pub g: f64,
    pub b: f64,
    pub a: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Color {
    Rgba(Rgba),
    CurrentColor,
}

impl Rgba {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Rgba {
        Rgba { r: r as f64, g: g as f64, b: b as f64, a: 1.0 }
    }
    /// Chromium's serialization of a legacy sRGB color: `rgb(r, g, b)` / `rgba(r, g, b, a)`, 8-bit channels.
    pub fn serialize(&self) -> String {
        let c = |v: f64| {
            let v = if v.is_nan() { 0.0 } else { v };
            let v = v.clamp(0.0, 255.0);
            // 1e-6 absorbs float error at exact halves (hwb(200 10% 20%) green is exactly 144.5)
            (v + 0.5 + 1e-6) as u32
        };
        let a = self.a.clamp(0.0, 1.0);
        if a >= 1.0 {
            alloc::format!("rgb({}, {}, {})", c(self.r), c(self.g), c(self.b))
        } else {
            // the shortest decimal that round-trips through 8-bit alpha
            let a8 = (a * 255.0 + 0.5) as u32;
            let mut s = alloc::format!("{}", a8 as f64 / 255.0);
            for digits in 1..=6 {
                let t = alloc::format!("{:.*}", digits, a8 as f64 / 255.0);
                let back: f64 = t.parse().unwrap_or(0.0);
                if (back * 255.0 + 0.5) as u32 == a8 {
                    s = t;
                    break;
                }
            }
            let s = if s.contains('.') { String::from(s.trim_end_matches('0').trim_end_matches('.')) } else { s };
            alloc::format!("rgba({}, {}, {}, {})", c(self.r), c(self.g), c(self.b), s)
        }
    }
}

const NAMED: &[(&str, u32)] = &[
    ("aliceblue", 0xf0f8ff), ("antiquewhite", 0xfaebd7), ("aqua", 0x00ffff), ("aquamarine", 0x7fffd4), ("azure", 0xf0ffff),
    ("beige", 0xf5f5dc), ("bisque", 0xffe4c4), ("black", 0x000000), ("blanchedalmond", 0xffebcd), ("blue", 0x0000ff),
    ("blueviolet", 0x8a2be2), ("brown", 0xa52a2a), ("burlywood", 0xdeb887), ("cadetblue", 0x5f9ea0), ("chartreuse", 0x7fff00),
    ("chocolate", 0xd2691e), ("coral", 0xff7f50), ("cornflowerblue", 0x6495ed), ("cornsilk", 0xfff8dc), ("crimson", 0xdc143c),
    ("cyan", 0x00ffff), ("darkblue", 0x00008b), ("darkcyan", 0x008b8b), ("darkgoldenrod", 0xb8860b), ("darkgray", 0xa9a9a9),
    ("darkgreen", 0x006400), ("darkgrey", 0xa9a9a9), ("darkkhaki", 0xbdb76b), ("darkmagenta", 0x8b008b), ("darkolivegreen", 0x556b2f),
    ("darkorange", 0xff8c00), ("darkorchid", 0x9932cc), ("darkred", 0x8b0000), ("darksalmon", 0xe9967a), ("darkseagreen", 0x8fbc8f),
    ("darkslateblue", 0x483d8b), ("darkslategray", 0x2f4f4f), ("darkslategrey", 0x2f4f4f), ("darkturquoise", 0x00ced1), ("darkviolet", 0x9400d3),
    ("deeppink", 0xff1493), ("deepskyblue", 0x00bfff), ("dimgray", 0x696969), ("dimgrey", 0x696969), ("dodgerblue", 0x1e90ff),
    ("firebrick", 0xb22222), ("floralwhite", 0xfffaf0), ("forestgreen", 0x228b22), ("fuchsia", 0xff00ff), ("gainsboro", 0xdcdcdc),
    ("ghostwhite", 0xf8f8ff), ("gold", 0xffd700), ("goldenrod", 0xdaa520), ("gray", 0x808080), ("green", 0x008000),
    ("greenyellow", 0xadff2f), ("grey", 0x808080), ("honeydew", 0xf0fff0), ("hotpink", 0xff69b4), ("indianred", 0xcd5c5c),
    ("indigo", 0x4b0082), ("ivory", 0xfffff0), ("khaki", 0xf0e68c), ("lavender", 0xe6e6fa), ("lavenderblush", 0xfff0f5),
    ("lawngreen", 0x7cfc00), ("lemonchiffon", 0xfffacd), ("lightblue", 0xadd8e6), ("lightcoral", 0xf08080), ("lightcyan", 0xe0ffff),
    ("lightgoldenrodyellow", 0xfafad2), ("lightgray", 0xd3d3d3), ("lightgreen", 0x90ee90), ("lightgrey", 0xd3d3d3), ("lightpink", 0xffb6c1),
    ("lightsalmon", 0xffa07a), ("lightseagreen", 0x20b2aa), ("lightskyblue", 0x87cefa), ("lightslategray", 0x778899), ("lightslategrey", 0x778899),
    ("lightsteelblue", 0xb0c4de), ("lightyellow", 0xffffe0), ("lime", 0x00ff00), ("limegreen", 0x32cd32), ("linen", 0xfaf0e6),
    ("magenta", 0xff00ff), ("maroon", 0x800000), ("mediumaquamarine", 0x66cdaa), ("mediumblue", 0x0000cd), ("mediumorchid", 0xba55d3),
    ("mediumpurple", 0x9370db), ("mediumseagreen", 0x3cb371), ("mediumslateblue", 0x7b68ee), ("mediumspringgreen", 0x00fa9a), ("mediumturquoise", 0x48d1cc),
    ("mediumvioletred", 0xc71585), ("midnightblue", 0x191970), ("mintcream", 0xf5fffa), ("mistyrose", 0xffe4e1), ("moccasin", 0xffe4b5),
    ("navajowhite", 0xffdead), ("navy", 0x000080), ("oldlace", 0xfdf5e6), ("olive", 0x808000), ("olivedrab", 0x6b8e23),
    ("orange", 0xffa500), ("orangered", 0xff4500), ("orchid", 0xda70d6), ("palegoldenrod", 0xeee8aa), ("palegreen", 0x98fb98),
    ("paleturquoise", 0xafeeee), ("palevioletred", 0xdb7093), ("papayawhip", 0xffefd5), ("peachpuff", 0xffdab9), ("peru", 0xcd853f),
    ("pink", 0xffc0cb), ("plum", 0xdda0dd), ("powderblue", 0xb0e0e6), ("purple", 0x800080), ("rebeccapurple", 0x663399),
    ("red", 0xff0000), ("rosybrown", 0xbc8f8f), ("royalblue", 0x4169e1), ("saddlebrown", 0x8b4513), ("salmon", 0xfa8072),
    ("sandybrown", 0xf4a460), ("seagreen", 0x2e8b57), ("seashell", 0xfff5ee), ("sienna", 0xa0522d), ("silver", 0xc0c0c0),
    ("skyblue", 0x87ceeb), ("slateblue", 0x6a5acd), ("slategray", 0x708090), ("slategrey", 0x708090), ("snow", 0xfffafa),
    ("springgreen", 0x00ff7f), ("steelblue", 0x4682b4), ("tan", 0xd2b48c), ("teal", 0x008080), ("thistle", 0xd8bfd8),
    ("tomato", 0xff6347), ("turquoise", 0x40e0d0), ("violet", 0xee82ee), ("wheat", 0xf5deb3), ("white", 0xffffff),
    ("whitesmoke", 0xf5f5f5), ("yellow", 0xffff00), ("yellowgreen", 0x9acd32),
    // CSS Color 4 §6.2 system colors, as Chromium 141 resolves them in the light color scheme (probed)
    ("canvas", 0xffffff), ("canvastext", 0x000000), ("linktext", 0x0000ee), ("visitedtext", 0x551a8b),
    ("activetext", 0xff0000), ("buttonface", 0xefefef), ("buttontext", 0x000000), ("buttonborder", 0x000000),
    ("field", 0xffffff), ("fieldtext", 0x000000), ("graytext", 0x808080), ("threedface", 0xefefef),
    ("highlighttext", 0x000000), ("selecteditem", 0x1967d2), ("selecteditemtext", 0xffffff), ("mark", 0xffff00),
    ("marktext", 0x000000), ("accentcolor", 0xefefef), ("accentcolortext", 0x000000),
];

fn from_u32(v: u32) -> Rgba {
    Rgba::rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// A named color (case-insensitive), `transparent` included.
pub fn named(name: &str) -> Option<Rgba> {
    let l = name.to_ascii_lowercase();
    if l == "transparent" {
        return Some(Rgba { r: 0.0, g: 0.0, b: 0.0, a: 0.0 });
    }
    NAMED.iter().find(|(n, _)| *n == l).map(|(_, v)| from_u32(*v))
}

fn hex(s: &str) -> Option<Rgba> {
    if !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let d: Vec<u32> = s.chars().map(|c| c.to_digit(16).unwrap()).collect();
    let (r, g, b, a) = match d.len() {
        3 => (d[0] * 17, d[1] * 17, d[2] * 17, 255),
        4 => (d[0] * 17, d[1] * 17, d[2] * 17, d[3] * 17),
        6 => (d[0] * 16 + d[1], d[2] * 16 + d[3], d[4] * 16 + d[5], 255),
        8 => (d[0] * 16 + d[1], d[2] * 16 + d[3], d[4] * 16 + d[5], d[6] * 16 + d[7]),
        _ => return None,
    };
    Some(Rgba { r: r as f64, g: g as f64, b: b as f64, a: a as f64 / 255.0 })
}

/// Parse a `<color>` from (whitespace-trimmed) component values.
pub fn parse_color(v: &[CV]) -> Option<Color> {
    match trim_ws(v) {
        [CV::Token(Token::Ident(s))] => {
            if s.eq_ignore_ascii_case("currentcolor") {
                Some(Color::CurrentColor)
            } else {
                named(s).map(Color::Rgba)
            }
        }
        [CV::Token(Token::Hash { value, .. })] => hex(value).map(Color::Rgba),
        [CV::Function(name, args)] => function(&name.to_ascii_lowercase(), args).map(Color::Rgba),
        _ => None,
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Arg {
    Num(f64),
    Pct(f64),
    Angle(f64),
    None,
}

fn args_of(args: &[CV]) -> Option<(Vec<Arg>, Option<Arg>, bool)> {
    // returns (channels, alpha, legacy-comma-syntax)
    let legacy = args.iter().any(|c| matches!(c, CV::Token(Token::Comma)));
    let mut chans = Vec::new();
    let mut alpha = None;
    let mut after_slash = false;
    let conv = |c: &CV| -> Option<Arg> {
        Some(match c {
            CV::Token(Token::Number(n)) => Arg::Num(n.value),
            CV::Token(Token::Percentage(n)) => Arg::Pct(n.value),
            CV::Token(Token::Dimension(n, u)) => match u.to_ascii_lowercase().as_str() {
                "deg" => Arg::Angle(n.value),
                "rad" => Arg::Angle(n.value * 180.0 / core::f64::consts::PI),
                "grad" => Arg::Angle(n.value * 0.9),
                "turn" => Arg::Angle(n.value * 360.0),
                _ => return None,
            },
            CV::Token(Token::Ident(s)) if s.eq_ignore_ascii_case("none") => Arg::None,
            CV::Function(..) => {
                let calc = crate::values::parse_math(c)?;
                let cx = crate::values::LengthContext { font_size: 16.0, root_font_size: 16.0, viewport_width: 0.0, viewport_height: 0.0, percent_basis: None };
                match calc.resolve(&cx)? {
                    crate::values::Resolved::Number(n) => Arg::Num(n),
                    crate::values::Resolved::LengthPercentage { pct, has_len: false, .. } => Arg::Pct(pct),
                    crate::values::Resolved::Other(crate::values::Dim::Angle, d) => Arg::Angle(d),
                    _ => return None,
                }
            }
            _ => return None,
        })
    };
    if legacy {
        for part in args.split(|c| matches!(c, CV::Token(Token::Comma))) {
            match trim_ws(part) {
                [c] => chans.push(conv(c)?),
                _ => return None,
            }
        }
        if chans.contains(&Arg::None) {
            return None;
        }
        if chans.len() == 4 {
            alpha = chans.pop();
        }
    } else {
        for c in args.iter().filter(|c| !c.is_whitespace()) {
            if c.is_delim('/') {
                if after_slash {
                    return None;
                }
                after_slash = true;
                continue;
            }
            if after_slash {
                if alpha.is_some() {
                    return None;
                }
                alpha = Some(conv(c)?);
            } else {
                chans.push(conv(c)?);
            }
        }
        if after_slash && alpha.is_none() {
            return None;
        }
    }
    if chans.len() != 3 {
        return None;
    }
    Some((chans, alpha, legacy))
}

fn alpha_of(a: Option<Arg>) -> Option<f64> {
    Some(match a {
        None => 1.0,
        Some(Arg::Num(n)) => n,
        Some(Arg::Pct(p)) => p / 100.0,
        Some(Arg::None) => 0.0,
        Some(Arg::Angle(_)) => return None,
    }
    .clamp(0.0, 1.0))
}

fn hsl_to_rgb(h: f64, s: f64, l: f64) -> (f64, f64, f64) {
    let h = ((h % 360.0) + 360.0) % 360.0;
    let f = |n: f64| {
        let k = (n + h / 30.0) % 12.0;
        let a = s * l.min(1.0 - l);
        l - a * (k - 3.0).min(9.0 - k).min(1.0).max(-1.0)
    };
    (f(0.0) * 255.0, f(8.0) * 255.0, f(4.0) * 255.0)
}

fn function(name: &str, args: &[CV]) -> Option<Rgba> {
    let (ch, a, legacy) = args_of(args)?;
    let alpha = alpha_of(a)?;
    match name {
        "rgb" | "rgba" => {
            // legacy syntax: all numbers or all percentages
            if legacy {
                let pct = matches!(ch[0], Arg::Pct(_));
                if ch.iter().any(|c| matches!(c, Arg::Pct(_)) != pct) {
                    return None;
                }
            }
            let c = |x: Arg| -> Option<f64> {
                Some(match x {
                    Arg::Num(n) => n,
                    Arg::Pct(p) => p * 2.55,
                    Arg::None => 0.0,
                    Arg::Angle(_) => return None,
                }
                .clamp(0.0, 255.0))
            };
            Some(Rgba { r: c(ch[0])?, g: c(ch[1])?, b: c(ch[2])?, a: alpha })
        }
        "hsl" | "hsla" | "hwb" => {
            let h = match ch[0] {
                Arg::Num(n) | Arg::Angle(n) => n,
                Arg::None => 0.0,
                Arg::Pct(_) => return None,
            };
            let p = |x: Arg| -> Option<f64> {
                Some(match x {
                    Arg::Pct(p) => p / 100.0,
                    Arg::Num(n) if !legacy => n / 100.0,
                    Arg::None => 0.0,
                    _ => return None,
                })
            };
            let (x, y) = (p(ch[1])?, p(ch[2])?);
            if name == "hwb" {
                if legacy {
                    return None;
                }
                let (w, bk) = (x.clamp(0.0, 1.0), y.clamp(0.0, 1.0));
                if w + bk >= 1.0 {
                    let g = w / (w + bk) * 255.0;
                    return Some(Rgba { r: g, g, b: g, a: alpha });
                }
                let (r, g, b) = hsl_to_rgb(h, 1.0, 0.5);
                let f = |c: f64| (c / 255.0 * (1.0 - w - bk) + w) * 255.0;
                return Some(Rgba { r: f(r), g: f(g), b: f(b), a: alpha });
            }
            let (r, g, b) = hsl_to_rgb(h, x.clamp(0.0, 1.0), y.clamp(0.0, 1.0));
            Some(Rgba { r, g, b, a: alpha })
        }
        _ => None,
    }
}
