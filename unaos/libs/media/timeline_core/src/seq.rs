// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: STRIAKSEQ (LEDGER SR9065) — the image-sequence reel's rules, ONE copy both faces call: the frame formats
//! ([`format`]), the numbered file name ([`number`]), the Player's reel-directory shape ([`dir_shape`]) and the upright
//! decode ([`decode`]). Stria's loader (`stria::nle::Sources::add_sequence`, `seq_frame`) and the kernel Player's EDL mode
//! (`video/vplay.rs` `seq_open` / `seq_frame`) read a sequence through these alone. Pure: no I/O, no lock.

use alloc::format;
use alloc::string::String;

/// The ONE table of sequence frame formats (STRIASEQMORE SR978's, moved): the suffix (case-insensitive) names the pixel_core
/// format its files must sniff as, and that format's name for a refusal (`not a <NAME> header`). `None`: not a sequence
/// format (TIFF / raw, SVG and everything else are refused at the suffix).
pub fn format(suffix: &str) -> Option<(pixel_core::Format, &'static str)> {
    use pixel_core::Format;
    let s = suffix.to_ascii_lowercase();
    [(".png", Format::Png, "PNG"), (".jpg", Format::Jpeg, "JPEG"), (".jpeg", Format::Jpeg, "JPEG"), (".gif", Format::Gif, "GIF"), (".bmp", Format::Bmp, "BMP"), (".qoi", Format::Qoi, "QOI"), (".webp", Format::WebP, "WebP")]
        .into_iter()
        .find(|e| s.ends_with(e.0))
        .map(|e| (e.1, e.2))
}

/// The file number a directory entry `name` carries under a sequence pattern `<stem>%0<width>d<suffix>` (width 0: `%d`):
/// `name` is `stem`, digits only, `suffix`, and the digits are the number written at that width (`0007` at width 4, `7` at
/// width 0; `007` at width 4 is no member). `None` for any other name (STRIAIMPORTIMAGESEQUENCE SR888's scan rule, moved).
pub fn number(stem: &str, width: usize, suffix: &str, name: &str) -> Option<u64> {
    let mid = name.strip_prefix(stem)?.strip_suffix(suffix)?;
    if mid.is_empty() || !mid.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n = mid.parse::<u64>().ok()?;
    (format!("{n:0width$}") == mid).then_some(n)
}

/// The kernel Player's reel DIRECTORY (`<dir>/<REEL>/`, frames named `<digits><ext>`, no stem): `(width, suffix)` read from
/// the name that sorts first among those that are digits followed by a [`format`] suffix. The suffix is kept as spelt
/// (FAT's `.PNG`); the width is the digit count when the digits start with `0` and are two or more, else 0 (`%d`). The
/// members are then the names [`number`] takes with stem `""`. `None`: no such name.
pub fn dir_shape<'a>(names: impl Iterator<Item = &'a str>) -> Option<(usize, String)> {
    let first = names
        .filter(|n| {
            let d = n.bytes().take_while(u8::is_ascii_digit).count();
            let ext = &n[d..];
            d > 0 && ext.len() > 1 && ext.starts_with('.') && !ext[1..].contains('.') && format(ext).is_some()
        })
        .min()?;
    let d = first.bytes().take_while(u8::is_ascii_digit).count();
    let width = if d > 1 && first.starts_with('0') { d } else { 0 };
    Some((width, String::from(&first[d..])))
}

/// A sequence file's picture: pixel_core's sniffing decode, turned upright by its EXIF orientation (STRIASEQTHREE SR9022's
/// rule, moved): a 5..=8 orientation swaps width and height.
pub fn decode(bytes: &[u8]) -> Result<pixel_core::Image, pixel_core::Error> {
    let mut img = pixel_core::decode(bytes)?;
    img.apply_orientation();
    Ok(img)
}
