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

// ---- STRIAKSEQSOUND (LEDGER SR9103): the kept-frame set (named tail block) ----
// STRIASEQTHREE (SR9022)'s kept set, moved here so Stria's sequence reels (`stria::nle::SeqKept`, one per reel behind its
// Mutex) and the kernel Player (`video/vplay.rs` `SEQ_KEPT`, one per reel directory) keep frames by ONE rule. Pure: the
// caller owns the lock and takes the stamp (a stat) before a `get` or a `put`, never under the lock.

use alloc::sync::Arc;
use alloc::vec::Vec;

/// At most this many kept frames per reel (a short scrub back and forth reads no file).
pub const KEPT: usize = 8;
/// Stria's byte cap per reel (the newest frame is kept whatever its size); the Player passes its own, smaller cap.
pub const KEPT_BYTES: usize = 128 * 1024 * 1024;

/// The bytes a kept picture holds: RGBA `Vec<u8>` (Stria) or 0RGB `Vec<u32>` (the Player).
pub trait KeptBytes {
    fn kept_bytes(&self) -> usize;
}

impl KeptBytes for Vec<u8> {
    fn kept_bytes(&self) -> usize {
        self.len()
    }
}

impl KeptBytes for Vec<u32> {
    fn kept_bytes(&self) -> usize {
        self.len() * 4
    }
}

/// A reel's kept frames `(file number, w, h, picture, stamp)`, least recently used first, and the byte cap (`.1`). The
/// stamp is the caller's (Stria: length and modified time; the Player: size and mtime): a frame is served only while the
/// file's stamp is still the one it was kept with.
pub struct Kept<P, S>(pub Vec<(u64, u32, u32, Arc<P>, Option<S>)>, pub usize);

impl<P, S> Default for Kept<P, S> {
    fn default() -> Self {
        Kept(Vec::new(), KEPT_BYTES)
    }
}

impl<P: KeptBytes, S: PartialEq> Kept<P, S> {
    /// File number `n`'s kept frame when its stamp is `st` (taken just before); a hit moves to the back, a kept frame whose
    /// file changed or is gone (`st` None) is dropped and not served.
    pub fn get(&mut self, n: u64, st: Option<S>) -> Option<(u32, u32, Arc<P>)> {
        let i = self.0.iter().position(|e| e.0 == n)?;
        let e = self.0.remove(i);
        if st.is_none() || e.4 != st {
            return None;
        }
        let out = (e.1, e.2, e.3.clone());
        self.0.push(e);
        Some(out)
    }
    /// Keep file number `n`'s frame (replacing an older copy of it) at the back, then drop from the front while more than
    /// [`KEPT`] frames or more than `.1` bytes are kept, the newest always staying. A frame with no stamp is not kept (it
    /// would never be served).
    pub fn put(&mut self, n: u64, w: u32, h: u32, px: Arc<P>, stamp: Option<S>) {
        self.0.retain(|e| e.0 != n);
        if stamp.is_none() {
            return;
        }
        self.0.push((n, w, h, px, stamp));
        while self.0.len() > 1 && (self.0.len() > KEPT || self.0.iter().map(|e| e.3.kept_bytes()).sum::<usize>() > self.1) {
            self.0.remove(0);
        }
    }
}
