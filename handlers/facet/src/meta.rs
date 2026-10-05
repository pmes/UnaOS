// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Container metadata, read by Facet itself from the specifications — no crate.
//!
//! What a file DECLARES, independent of who decodes its pixels: stored dimensions, bits per sample,
//! whether it carries alpha, the colour space it claims, the EXIF orientation (EXIF 2.3 §4.6.4 A,
//! tag 0x0112, in a TIFF 6.0 IFD0), and an animation loop count. Sections followed:
//!
//! | format | spec | read here |
//! |---|---|---|
//! | PNG  | W3C PNG 3rd ed. §11 | IHDR, tRNS, sRGB, iCCP (profile name), cICP, gAMA, eXIf, acTL |
//! | JPEG | ITU-T T.81 Annex B; JFIF 1.02; EXIF 2.3 §4.5.4 (APP1); ICC.1 B.4 (APP2) | SOFn, APP1 Exif, APP2 ICC_PROFILE, APP14 Adobe |
//! | GIF  | GIF89a §18, §23, §26; NETSCAPE2.0 application extension | screen descriptor, GCE transparency, loop count |
//! | BMP  | BITMAPINFOHEADER / V4 / V5 | size, bit count, alpha mask, CSType |
//! | QOI  | qoiformat.org 1.0 header | size, channels, colorspace |
//! | WebP | RFC 9649 §2.5–2.7 (RIFF, VP8X, VP8L header), RFC 6386 §9.1 (VP8 frame header) | canvas size, alpha, ICCP, EXIF, ANIM |
//!
//! Every reader is total: malformed or truncated input yields defaults, never a panic — this runs on
//! attacker-shaped bytes before any decoder does.

use crate::source::Format;

/// What a file declares about itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Meta {
    pub width: u32,
    pub height: u32,
    pub bit_depth: u8,
    pub has_alpha: bool,
    /// Human-readable colour declaration (`"sRGB chunk"`, `"ICC profile 'Display P3'"`, ...).
    pub colour: String,
    /// EXIF orientation 1..=8; 1 when absent or out of range.
    pub orientation: u8,
    pub loop_count: Option<u16>,
}

impl Default for Meta {
    fn default() -> Self {
        Meta {
            width: 0,
            height: 0,
            bit_depth: 8,
            has_alpha: false,
            colour: UNTAGGED.into(),
            orientation: 1,
            loop_count: None,
        }
    }
}

const UNTAGGED: &str = "untagged (assumed sRGB)";

fn be16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(b.get(at..at + 2)?.try_into().ok()?))
}
fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}
fn le16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}
fn le32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}
fn le24(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at + 3)?;
    Some(s[0] as u32 | (s[1] as u32) << 8 | (s[2] as u32) << 16)
}

/// Read the metadata of `bytes`, already identified as `format`.
pub fn read(bytes: &[u8], format: Format) -> Meta {
    match format {
        Format::Png => png(bytes),
        Format::Jpeg => jpeg(bytes),
        Format::Gif => gif(bytes),
        Format::Bmp => bmp(bytes),
        Format::Qoi => qoi(bytes),
        Format::WebP => webp(bytes),
    }
}

/// The orientation tag of an EXIF payload: a TIFF stream (`II*\0` / `MM\0*`), optionally behind the
/// JPEG APP1 `Exif\0\0` identifier. Returns 1 for anything absent, malformed or out of 1..=8.
pub fn exif_orientation(payload: &[u8]) -> u8 {
    let t = payload.strip_prefix(b"Exif\0\0").unwrap_or(payload);
    let big = match t.get(0..4) {
        Some([b'I', b'I', 42, 0]) => false,
        Some([b'M', b'M', 0, 42]) => true,
        _ => return 1,
    };
    let r16 = |at: usize| if big { be16(t, at) } else { le16(t, at) };
    let r32 = |at: usize| if big { be32(t, at) } else { le32(t, at) };
    let Some(ifd) = r32(4).map(|v| v as usize) else { return 1 };
    let Some(n) = r16(ifd) else { return 1 };
    for i in 0..n as usize {
        let e = ifd + 2 + i * 12;
        if r16(e) == Some(0x0112) {
            // Type SHORT (3), count 1: the value sits left-justified in the 4-byte field.
            if r16(e + 2) != Some(3) {
                return 1;
            }
            return match r16(e + 8) {
                Some(v @ 1..=8) => v as u8,
                _ => 1,
            };
        }
    }
    1
}

/// The profile description of an ICC profile (ICC.1:2010 §9.2.41 `desc` tag: v2 `desc` type or v4
/// `mluc` type), when it can be read.
pub fn icc_description(icc: &[u8]) -> Option<String> {
    let count = be32(icc, 128)? as usize;
    for i in 0..count.min(256) {
        let e = 132 + i * 12;
        if icc.get(e..e + 4)? == b"desc" {
            let off = be32(icc, e + 4)? as usize;
            let len = be32(icc, e + 8)? as usize;
            let tag = icc.get(off..off.checked_add(len)?)?;
            return match tag.get(0..4)? {
                b"desc" => {
                    let n = be32(tag, 8)? as usize;
                    let s = tag.get(12..12 + n)?;
                    let s = s.split(|&c| c == 0).next()?;
                    Some(String::from_utf8_lossy(s).into_owned())
                }
                b"mluc" => {
                    // First record: lang(2) country(2) length(4) offset(4), UTF-16BE.
                    let rlen = be32(tag, 20)? as usize;
                    let roff = be32(tag, 24)? as usize;
                    let raw = tag.get(roff..roff + rlen)?;
                    let u: Vec<u16> = raw.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
                    Some(String::from_utf16_lossy(&u).trim_end_matches('\0').to_string())
                }
                _ => None,
            };
        }
    }
    None
}

fn icc_label(icc: &[u8]) -> String {
    match icc_description(icc) {
        Some(d) if !d.is_empty() => format!("ICC profile '{d}' ({} bytes)", icc.len()),
        _ => format!("ICC profile ({} bytes)", icc.len()),
    }
}

fn png(b: &[u8]) -> Meta {
    let mut m = Meta::default();
    let mut colour_type = 0u8;
    let mut trns = false;
    let (mut srgb, mut iccp, mut cicp, mut gama) = (None::<String>, None::<String>, None::<String>, None::<String>);
    let mut at = 8usize;
    while let (Some(len), Some(kind)) = (be32(b, at), b.get(at + 4..at + 8)) {
        let len = len as usize;
        let Some(data) = b.get(at + 8..at + 8 + len) else { break };
        match kind {
            b"IHDR" if len >= 13 => {
                m.width = be32(data, 0).unwrap_or(0);
                m.height = be32(data, 4).unwrap_or(0);
                m.bit_depth = data[8];
                colour_type = data[9];
            }
            b"tRNS" => trns = true,
            b"sRGB" => srgb = Some("sRGB chunk".into()),
            b"iCCP" => {
                let name = data.split(|&c| c == 0).next().unwrap_or(&[]);
                iccp = Some(format!("ICC profile '{}' (iCCP)", String::from_utf8_lossy(name)));
            }
            b"cICP" if len >= 4 => {
                cicp = Some(format!("cICP primaries {} transfer {}", data[0], data[1]));
            }
            b"gAMA" if len >= 4 => {
                gama = Some(format!("gAMA {:.5}", be32(data, 0).unwrap_or(0) as f64 / 100_000.0));
            }
            b"eXIf" => m.orientation = exif_orientation(data),
            b"acTL" if len >= 8 => {
                let plays = be32(data, 4).unwrap_or(0);
                m.loop_count = Some(plays.min(u16::MAX as u32) as u16);
            }
            b"IEND" => break,
            _ => {}
        }
        at += 12 + len;
    }
    m.has_alpha = matches!(colour_type, 4 | 6) || trns;
    // The PNG 3rd edition precedence: cICP > iCCP > sRGB > gAMA/cHRM.
    m.colour = cicp.or(iccp).or(srgb).or(gama).unwrap_or_else(|| UNTAGGED.into());
    m
}

fn jpeg(b: &[u8]) -> Meta {
    let mut m = Meta::default();
    let mut icc: Vec<(u8, Vec<u8>)> = Vec::new();
    let mut adobe = false;
    let mut at = 2usize;
    while at + 4 <= b.len() {
        if b[at] != 0xFF {
            break;
        }
        let marker = b[at + 1];
        if marker == 0xFF {
            at += 1; // fill byte
            continue;
        }
        if marker == 0xD8 || (0xD0..=0xD7).contains(&marker) || marker == 0x01 {
            at += 2;
            continue;
        }
        if marker == 0xD9 || marker == 0xDA {
            break; // EOI, or the scan: every header segment we read precedes the first SOS
        }
        let Some(len) = be16(b, at + 2).map(|v| v as usize) else { break };
        let Some(seg) = b.get(at + 4..at + 2 + len.max(2)) else { break };
        match marker {
            // SOF0..SOF15 except DHT (C4), JPG (C8), DAC (CC).
            0xC0..=0xCF if !matches!(marker, 0xC4 | 0xC8 | 0xCC) && seg.len() >= 6 => {
                m.bit_depth = seg[0];
                m.height = be16(seg, 1).unwrap_or(0) as u32;
                m.width = be16(seg, 3).unwrap_or(0) as u32;
            }
            0xE1 if seg.starts_with(b"Exif\0\0") => m.orientation = exif_orientation(seg),
            0xE2 if seg.starts_with(b"ICC_PROFILE\0") && seg.len() > 14 => icc.push((seg[12], seg[14..].to_vec())),
            0xEE if seg.starts_with(b"Adobe") => adobe = true,
            _ => {}
        }
        at += 2 + len;
    }
    if !icc.is_empty() {
        icc.sort_by_key(|c| c.0);
        let whole: Vec<u8> = icc.into_iter().flat_map(|c| c.1).collect();
        m.colour = icc_label(&whole);
    } else if adobe {
        m.colour = "Adobe APP14, no profile (assumed sRGB)".into();
    }
    m
}

fn gif(b: &[u8]) -> Meta {
    let mut m = Meta::default();
    m.width = le16(b, 6).unwrap_or(0) as u32;
    m.height = le16(b, 8).unwrap_or(0) as u32;
    let Some(&packed) = b.get(10) else { return m };
    m.bit_depth = (packed & 7) + 1;
    let mut at = 13usize;
    if packed & 0x80 != 0 {
        at += 3 << ((packed & 7) + 1);
    }
    // Skip a run of data sub-blocks starting at `at`; returns the index after the terminator.
    let skip_blocks = |mut at: usize| -> Option<usize> {
        loop {
            let n = *b.get(at)? as usize;
            at += 1 + n;
            if n == 0 {
                return Some(at);
            }
        }
    };
    while let Some(&intro) = b.get(at) {
        match intro {
            0x21 => {
                let Some(&label) = b.get(at + 1) else { break };
                if label == 0xF9 && b.get(at + 3).is_some_and(|p| p & 1 != 0) {
                    m.has_alpha = true;
                }
                if label == 0xFF && b.get(at + 3..at + 14) == Some(b"NETSCAPE2.0") {
                    // Sub-block: len 3, id 1, loop count u16 LE.
                    if b.get(at + 14) == Some(&3) && b.get(at + 15) == Some(&1) {
                        m.loop_count = le16(b, at + 16);
                    }
                }
                let Some(next) = skip_blocks(at + 2) else { break };
                at = next;
            }
            0x2C => {
                let Some(&lp) = b.get(at + 9) else { break };
                at += 10;
                if lp & 0x80 != 0 {
                    at += 3 << ((lp & 7) + 1);
                }
                at += 1; // LZW minimum code size
                let Some(next) = skip_blocks(at) else { break };
                at = next;
            }
            _ => break, // 0x3B trailer, or garbage
        }
    }
    m
}

fn bmp(b: &[u8]) -> Meta {
    let mut m = Meta::default();
    let hsize = le32(b, 14).unwrap_or(0);
    m.width = le32(b, 18).map(|v| (v as i32).unsigned_abs()).unwrap_or(0);
    m.height = le32(b, 22).map(|v| (v as i32).unsigned_abs()).unwrap_or(0);
    let bpp = le16(b, 28).unwrap_or(0);
    m.bit_depth = match bpp {
        16 => 5,
        24 | 32 => 8,
        n => n.min(8) as u8,
    };
    // V3-with-alpha (56) and later headers carry an alpha mask at offset 14+40+12.
    m.has_alpha = bpp == 32 && hsize >= 56 && le32(b, 66).unwrap_or(0) != 0;
    if hsize >= 108 {
        m.colour = match b.get(70..74) {
            Some(b"BGRs") => "sRGB (BMP LCS_sRGB)".into(),
            Some(b"BGRW") => "Windows colour space (BMP LCS_WINDOWS_COLOR_SPACE)".into(),
            Some(b"DEBM") => "embedded ICC profile (BMP PROFILE_EMBEDDED)".into(),
            Some(b"KNIL") => "linked ICC profile (BMP PROFILE_LINKED)".into(),
            _ => "calibrated RGB (BMP LCS_CALIBRATED_RGB)".into(),
        };
    }
    m
}

fn qoi(b: &[u8]) -> Meta {
    let mut m = Meta::default();
    m.width = be32(b, 4).unwrap_or(0);
    m.height = be32(b, 8).unwrap_or(0);
    m.has_alpha = b.get(12) == Some(&4);
    m.colour = match b.get(13) {
        Some(1) => "linear (QOI colorspace 1)".into(),
        _ => "sRGB with linear alpha (QOI colorspace 0)".into(),
    };
    m
}

fn webp(b: &[u8]) -> Meta {
    let mut m = Meta::default();
    let mut at = 12usize;
    while let (Some(kind), Some(len)) = (b.get(at..at + 4), le32(b, at + 4)) {
        let len = len as usize;
        let Some(data) = b.get(at + 8..at + 8 + len) else { break };
        match kind {
            b"VP8X" if len >= 10 => {
                m.has_alpha = data[0] & 0x10 != 0;
                m.width = le24(data, 4).unwrap_or(0) + 1;
                m.height = le24(data, 7).unwrap_or(0) + 1;
            }
            b"VP8L" if len >= 5 && data[0] == 0x2F && m.width == 0 => {
                let v = le32(data, 1).unwrap_or(0);
                m.width = (v & 0x3FFF) + 1;
                m.height = ((v >> 14) & 0x3FFF) + 1;
                m.has_alpha = (v >> 28) & 1 != 0;
            }
            b"VP8 " if len >= 10 && m.width == 0 && data.get(3..6) == Some(&[0x9D, 0x01, 0x2A]) => {
                m.width = (le16(data, 6).unwrap_or(0) & 0x3FFF) as u32;
                m.height = (le16(data, 8).unwrap_or(0) & 0x3FFF) as u32;
            }
            b"ICCP" => m.colour = icc_label(data),
            b"EXIF" => m.orientation = exif_orientation(data),
            b"ANIM" if len >= 6 => m.loop_count = le16(data, 4),
            _ => {}
        }
        at += 8 + len + (len & 1);
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal little-endian TIFF with one IFD0 entry: Orientation = `o`.
    pub(crate) fn tiff_orientation(o: u16, big: bool) -> Vec<u8> {
        let mut t = Vec::new();
        let p16 = |t: &mut Vec<u8>, v: u16| t.extend(if big { v.to_be_bytes() } else { v.to_le_bytes() });
        let p32 = |t: &mut Vec<u8>, v: u32| t.extend(if big { v.to_be_bytes() } else { v.to_le_bytes() });
        t.extend(if big { *b"MM\0*" } else { *b"II*\0" });
        p32(&mut t, 8);
        p16(&mut t, 1);
        p16(&mut t, 0x0112);
        p16(&mut t, 3);
        p32(&mut t, 1);
        p16(&mut t, o);
        p16(&mut t, 0);
        p32(&mut t, 0);
        t
    }

    #[test]
    fn exif_orientation_both_byte_orders_and_prefix() {
        for o in 1..=8 {
            assert_eq!(exif_orientation(&tiff_orientation(o, false)), o as u8);
            assert_eq!(exif_orientation(&tiff_orientation(o, true)), o as u8);
            let mut p = b"Exif\0\0".to_vec();
            p.extend(tiff_orientation(o, true));
            assert_eq!(exif_orientation(&p), o as u8);
        }
        assert_eq!(exif_orientation(&tiff_orientation(9, false)), 1);
        assert_eq!(exif_orientation(b"II*\0\xff\xff\xff\xff"), 1);
        assert_eq!(exif_orientation(b""), 1);
    }

    #[test]
    fn readers_are_total_on_truncation() {
        let png = crate::png::encode(3, 2, &[7u8; 24]);
        for n in 0..png.len() {
            let _ = read(&png[..n], Format::Png);
            let _ = read(&png[..n], Format::Jpeg);
            let _ = read(&png[..n], Format::Gif);
            let _ = read(&png[..n], Format::Bmp);
            let _ = read(&png[..n], Format::Qoi);
            let _ = read(&png[..n], Format::WebP);
        }
        let m = read(&png, Format::Png);
        assert_eq!((m.width, m.height, m.bit_depth, m.has_alpha), (3, 2, 8, true));
    }

    #[test]
    fn icc_v2_desc() {
        // Header (128) + count 1 + one tag entry, then a `desc` tag.
        let mut icc = vec![0u8; 128];
        icc.extend(1u32.to_be_bytes());
        icc.extend(b"desc");
        icc.extend(144u32.to_be_bytes());
        let mut tag = b"desc\0\0\0\0".to_vec();
        tag.extend(5u32.to_be_bytes());
        tag.extend(b"sRGB\0");
        icc.extend((tag.len() as u32).to_be_bytes());
        icc.extend(tag);
        assert_eq!(icc_description(&icc).as_deref(), Some("sRGB"));
    }
}
