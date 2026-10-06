// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! FACET — UnaOS's image viewer. Peter, 2026-09-08: *"i was tempted to double click on one of the
//! screenshots … i believe we already have an image viewer in the vessels can we compile it for
//! UnaOS?"*
//!
//! A kernel-owned compositor window that opens a PNG by path, decodes it, scales it to fit, and puts
//! it on the glass. Its one door is [`super::quarry`]: `<Enter>` or a double-click on a `.PNG` row in
//! the file manager. `docs/CODEX.md` §2 names this handler **Facet — Images — "The Canvas"**, so the
//! name is the manifest's, not a coinage.
//!
//! ## Why this is a kernel window and not the host-native vessel
//!
//! The ask was literally "compile the vessel", and the honest answer is that neither half of it can
//! be compiled for this target, for two independent reasons that are measurements rather than taste:
//!
//! 1. **`vessels/facet` is host-native by construction.** It is a Tokio program (`libs/quartzite`)
//!    that renders through WGPU (`libs/euclase`) and decodes through the `png` crate (`libs/lux`) —
//!    three `std` dependency trees, none of which exists for a `no_std` freestanding target. Porting
//!    it is not a build-flag change; it is a rewrite of everything but the file format.
//! 2. **An EL0 program in this tree cannot host a PNG decoder at all.** DEFLATE's history window is
//!    32 KiB by definition (RFC 1951 §3.2.1, and `selfhost::inflate::WINDOW` is exactly that), while
//!    every EL0 task in this kernel gets a 16 KiB window — `sched.rs`'s "every EL0 task got the
//!    blanket 16 KiB". The decoder's mandatory working set is twice the whole address window it
//!    would have to live in, before a single pixel is stored. That is a fact about the ring, not
//!    about the program, so no amount of care in a userspace port gets past it.
//!
//! So Facet takes the shape the tree already has for a windowed tool that needs room and kernel data:
//! [`super::quarry`]'s shape exactly — a kernel-owned `wm` row over a cached-RAM ARGB8888 surface,
//! presented through the ordinary [`wm::present`] path, opened by `desktop_firmware::activate()`'s
//! tenant family. Nothing here touches the scan-out. The day the EL0 window grows and a `std`-free
//! image crate lands, the decode half below is a pure function over bytes and moves out unchanged.
//!
//! ## Memory — the whole design, and why the full image is never held
//!
//! A 1920x1200 screenshot is 6.9 MB of RGB before it is anything else, and `prtscr` writes those
//! screenshots with STORED deflate blocks, so the FILE is 6.9 MB too. A naive viewer holds three of
//! those at once (file + decoded image + window surface) against a 48 MiB heap shared with the whole
//! desktop. Facet holds NONE of them:
//!
//!   * the FILE streams. [`IdatSource`] pulls the concatenated IDAT payload through the VFS in
//!     [`CHUNK`]-sized reads and hands it to the decoder a byte at a time, so the compressed bytes
//!     never exist as one object;
//!   * the DECODED IMAGE is never materialised. [`RowSink`] receives inflated bytes, reassembles one
//!     scanline, unfilters it against the previous one, and immediately BOXES it down into the
//!     output at target scale. Its whole working set is two scanlines plus one accumulator row;
//!   * the SURFACE is the only real allocation, and it is the window's own — `out_w * out_h * 4`,
//!     `try_reserve_exact`ed before a byte is read, and freed by [`close`].
//!
//! That is the brief's "decode straight into the window surface at target scale rather than holding
//! the full image", and it is why the peak here is ~2.3 MB for a full-panel screenshot rather than
//! ~21 MB. The 32 KiB DEFLATE window is the decoder's and is charged once.
//!
//! ## What it accepts
//!
//! Every non-interlaced PNG: bit depths 1/2/4/8/16, colour types 0 (greyscale), 2 (truecolour),
//! 3 (palette), 4 (grey+alpha) and 6 (RGBA), and **all five** filter types. `prtscr` itself only ever
//! emits depth-8 truecolour with filter 0 (see `video/png.rs`'s `push_row`), but the card also holds
//! PNGs written elsewhere, so decoding only what we encode would be a viewer that works on exactly
//! the files we could already have shown. Adam7 interlacing is REFUSED by name rather than decoded
//! wrong — it needs seven passes and a second address arithmetic, and no file on this medium has it.
//!
//! Alpha is composited against nothing: the colour channels are shown and the alpha channel is
//! dropped. Stated rather than hidden, because a viewer that silently premultiplies is lying about
//! the pixels; the checkerboard a real viewer draws is a later rung.
//!
//! ## Witnesses
//!
//! Every path prints exactly one line and there is no silent failure:
//!
//! ```text
//! [facet] open path=… ihdr=WxH depth=D colour=C -> DECODING
//! [facet] decoded rows=… inflate=OK|<reason> ms=…
//! [facet] present win=N scale=1/k
//! [facet] refuse path=… reason=<not-png|bad-ihdr|inflate-…|too-large|…>
//! ```

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use crate::selfhost::inflate::{self, ByteSource, InflateError, Sink};
use crate::video::wm;

// ── Identity ────────────────────────────────────────────────────────────────────────────────────

/// Facet's owner ASID: kernel FURNITURE in the reserved band, and its OWN slot — `+ 4`, one past
/// Quarry's `+ 3`. Neither [`wm::KERNEL_OWNER_CONSOLE`] nor [`wm::KERNEL_OWNER_DESKTOP`], for the
/// CLOSEISO reason Quarry's header gives: a `close_owner` sweep aimed at the console, the desktop or
/// the file manager must not reap the picture the operator just opened, and vice versa.
///
/// Declared HERE and not in `wm.rs` for Quarry's reason, restated because it is what keeps this arc
/// out of a file another lane owns: [`wm::is_kernel_owner`] already admits the whole
/// `KERNEL_OWNER_BASE ..= +0xFF` band, so a new tenant of it owes `wm.rs` no line.
pub const OWNER: u64 = wm::KERNEL_OWNER_BASE + 4;
const _: () = assert!(OWNER != wm::KERNEL_OWNER_CONSOLE && OWNER != wm::KERNEL_OWNER_DESKTOP);
const _: () = assert!(OWNER != super::quarry::live::OWNER);

/// The live window id, or [`wm::WIN_NONE`].
static WIN: AtomicU32 = AtomicU32::new(wm::WIN_NONE);

/// Presents this window has made. Reported by [`close`], the way Quarry reports its paint count.
static PAINTS: AtomicUsize = AtomicUsize::new(0);

// ── Bounds ──────────────────────────────────────────────────────────────────────────────────────
//
// Every one of these is a loop bound or an allocation bound over ATTACKER-SHAPED input: the file is
// whatever is on the medium, and its IHDR is four bytes that claim a size. Nothing below trusts a
// claimed dimension before it has been multiplied out and checked.

/// The largest PNG file Facet will open. A full-panel `prtscr` capture is ~6.9 MB (stored deflate,
/// `1 + width*3` per row); 64 MiB is ~9x that, which covers a 4K RGBA capture from elsewhere and
/// still refuses a file that could only be a mistake. The file is STREAMED, so this bounds WORK and
/// the IDAT index, not a buffer.
const MAX_FILE: u64 = 64 * 1024 * 1024;

/// The largest image dimension Facet will accept from an IHDR. PNG allows 2^31-1 in each axis; at
/// depth 16 RGBA one scanline of that is 16 GB, and [`RowSink`] allocates two scanlines. 16384 is
/// 8x the bench panel's long edge and bounds a scanline at 128 KB.
const MAX_DIM: u32 = 16384;

/// IDAT chunks Facet will index. The encoder in `video/png.rs` writes ONE, and every writer in
/// practice writes a handful; a file with more than this is either pathological or an attack on the
/// index vector, and it is refused by name.
const MAX_IDAT: usize = 4096;

/// Bytes per VFS read while streaming the IDAT payload.
///
/// Chosen against the same cost model `selfhost::CHUNK` names: the FAT backend re-collects a file's
/// cluster chain from the FIRST cluster on every `read`, so total FAT traffic goes as the SQUARE of
/// the chunk count. At 256 KiB a 6.9 MB screenshot is 27 reads; at 64 KiB it would be 108, for four
/// times the chain walking and the same 6.9 MB of data. A quarter megabyte is noise against the
/// 48 MiB heap and is freed between chunks.
const CHUNK: usize = 256 * 1024;

/// The largest window CONTENT Facet will mint, in source pixels, before the panel's own limit.
///
/// Deliberately smaller than the panel: the picture is a document window on a desktop that already
/// has a file manager, a dock and a menu bar on it, and a viewer that covered the screen would hide
/// the very window it was opened from. 1200x800 fits inside the bench panel's work area with the
/// dock clear, and reduces a 1920x1200 screenshot by exactly 2 — an integer factor, so the box
/// filter below is an average over whole pixels rather than a resample.
const CEIL_W: usize = 1200;
const CEIL_H: usize = 800;

/// Smallest window Facet will mint. Below this there is nothing to look at and the honest answer is
/// to refuse rather than to draw a smear.
const FLOOR_W: usize = 32;
const FLOOR_H: usize = 32;

/// The PNG signature (`\x89PNG\r\n\x1a\n`) — `video/png.rs`'s `SIGNATURE`, which is private to the
/// encoder. Eight bytes, restated rather than re-exported, so the encoder keeps no public surface it
/// did not choose to have.
const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

// ── The image header, and what a refusal is ─────────────────────────────────────────────────────

/// Everything IHDR says, validated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ihdr {
    pub width: u32,
    pub height: u32,
    pub depth: u8,
    pub colour: u8,
    /// IHDR interlace method 1 (Adam7).
    pub interlaced: bool,
}

impl Ihdr {
    /// Channels per pixel for this colour type. `None` for a colour type PNG does not define.
    const fn channels(&self) -> Option<usize> {
        match self.colour {
            0 => Some(1), // greyscale
            2 => Some(3), // truecolour
            3 => Some(1), // palette index
            4 => Some(2), // greyscale + alpha
            6 => Some(4), // truecolour + alpha
            _ => None,
        }
    }

    /// Bits per pixel — the product the filter unit and the scanline length are both derived from.
    fn bits_per_pixel(&self) -> Option<usize> {
        Some(self.channels()? * self.depth as usize)
    }

    /// The filter unit, in bytes: `ceil(bits_per_pixel / 8)`, minimum 1 (PNG §9.2's `bpp`).
    fn filter_unit(&self) -> Option<usize> {
        Some(self.bits_per_pixel()?.div_ceil(8).max(1))
    }

    /// Bytes in one unfiltered scanline, excluding the leading filter byte.
    fn row_bytes(&self) -> Option<usize> {
        (self.width as usize).checked_mul(self.bits_per_pixel()?).map(|b| b.div_ceil(8))
    }

    /// Is this a depth the colour type is allowed to carry (PNG §11.2.2's table)?
    fn depth_legal(&self) -> bool {
        match self.colour {
            0 => matches!(self.depth, 1 | 2 | 4 | 8 | 16),
            3 => matches!(self.depth, 1 | 2 | 4 | 8),
            2 | 4 | 6 => matches!(self.depth, 8 | 16),
            _ => false,
        }
    }
}

/// Why Facet would not, or could not, show a file. Every variant is a distinct falsifiable claim, so
/// the `reason=` on the refusal line names WHICH one broke rather than saying "bad file".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FacetError {
    /// The VFS declined the path, or the path is a directory.
    Vfs(String),
    /// Zero bytes, or more than [`MAX_FILE`].
    Size(u64),
    /// The first eight bytes are not the PNG signature.
    NotPng,
    /// The file ends inside a chunk header, a chunk payload, or the IHDR.
    Truncated,
    /// IHDR is malformed: a zero dimension, a dimension over [`MAX_DIM`], an undefined colour type, a
    /// depth that colour type may not carry, a compression/filter method other than 0.
    BadIhdr(&'static str),
    /// Adam7. Refused by name — see the module header.
    Interlaced,
    /// A palette image whose PLTE is missing, malformed, or too short for an index the image uses.
    BadPalette(&'static str),
    /// No IDAT chunk at all, or more than [`MAX_IDAT`] of them.
    BadIdat(&'static str),
    /// The zlib/DEFLATE decode failed. Carries the decoder's own reason.
    Inflate(InflateError),
    /// The inflated stream held a filter byte PNG does not define.
    BadFilter(u8),
    /// The inflated stream was not exactly `height` scanlines long.
    RowCount { got: u32, want: u32 },
    /// The allocator declined the window surface. Reported BEFORE any pixel is read.
    OutOfMemory(usize),
    /// The panel is missing, busy, below the floor, or `wm` refused the row.
    NoWindow(&'static str),
    /// PIXELCORE: `pixel_core::decode` refused a non-PNG file. Carries its own reason.
    Pixel(pixel_core::Error),
}

impl FacetError {
    /// The `reason=` token for the refusal line. Stable, lower-case, hyphenated — a token a spec or
    /// an `awk` can match on, not prose.
    pub fn reason(&self) -> String {
        match self {
            FacetError::Vfs(s) => alloc::format!("vfs-{}", s),
            FacetError::Size(n) => alloc::format!("too-large({} bytes, max {})", n, MAX_FILE),
            FacetError::NotPng => String::from("not-png"),
            FacetError::Truncated => String::from("truncated"),
            FacetError::BadIhdr(w) => alloc::format!("bad-ihdr({})", w),
            FacetError::Interlaced => String::from("interlaced-adam7-over-3Mpx"),
            FacetError::BadPalette(w) => alloc::format!("bad-palette({})", w),
            FacetError::BadIdat(w) => alloc::format!("bad-idat({})", w),
            // The decoder's own sentence, HYPHENATED: `inflate_reason` writes prose ("zlib trailer
            // adler-32 mismatch") because SELFHOST-2 prints it inside a sentence, and a `reason=`
            // token that carries spaces is one an `awk` or a spec cannot match as one field. The
            // wording is the decoder's; only the separator is this module's.
            FacetError::Inflate(e) => {
                let mut s = alloc::format!("inflate-{}", inflate::inflate_reason(*e));
                // SAFETY-FREE: ASCII in, ASCII out — `inflate_reason`'s strings are literals with no
                // multi-byte characters, so a byte-wise substitution cannot split one.
                s = s.chars().map(|c| if c == ' ' { '-' } else { c }).collect();
                s
            }
            FacetError::BadFilter(f) => alloc::format!("bad-filter({})", f),
            FacetError::RowCount { got, want } => {
                alloc::format!("row-count({} of {})", got, want)
            }
            FacetError::OutOfMemory(n) => alloc::format!("alloc({} bytes)", n),
            FacetError::NoWindow(w) => alloc::format!("no-window({})", w),
            FacetError::Pixel(e) => alloc::format!("pixel_core-{}", e).chars().map(|c| if c == ' ' { '-' } else { c }).collect(),
        }
    }
}

// ── Scale arithmetic (pure) ─────────────────────────────────────────────────────────────────────

/// The integer reduction factor that fits `iw x ih` inside `bw x bh`, and the size it produces.
///
/// INTEGER, and only downward. `k = 1` for anything that already fits: Facet never upscales, because
/// a nearest-neighbour blow-up of a small icon is a worse answer than the icon, and `wm`'s own
/// compositor scale already magnifies a window on a large panel if the desktop wants that.
///
/// The output is `iw/k x ih/k` with the remainder DROPPED, which is what makes [`RowSink`]'s
/// accumulator exact: every output pixel is the mean of exactly `k*k` source pixels, so no divisor
/// varies along an edge and no partial block has to be special-cased. At most `k-1` source rows and
/// columns are discarded — one pixel at 1920x1200 into 960x600, and never more than a hairline.
pub fn fit(iw: u32, ih: u32, bw: usize, bh: usize) -> Option<(usize, usize, usize)> {
    if iw == 0 || ih == 0 || bw == 0 || bh == 0 {
        return None;
    }
    let (iw, ih) = (iw as usize, ih as usize);
    let k = iw.div_ceil(bw).max(ih.div_ceil(bh)).max(1);
    let (ow, oh) = (iw / k, ih / k);
    if ow == 0 || oh == 0 {
        return None;
    }
    Some((k, ow, oh))
}

/// Paeth's predictor, PNG §9.4 verbatim. `a` = left, `b` = above, `c` = upper-left.
#[inline]
fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i32 + b as i32 - c as i32;
    let (pa, pb, pc) = ((p - a as i32).abs(), (p - b as i32).abs(), (p - c as i32).abs());
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// Undo one scanline's filter IN PLACE, against the already-unfiltered `prev`.
///
/// Pure, and separated from the sink so the fixture can drive all five filter types over known bytes
/// without an inflate, a window or a volume. `bpp` is the filter unit — [`Ihdr::filter_unit`] — and
/// `prev` must be the same length as `cur` (all-zero for the first row, which is what PNG defines).
pub fn unfilter(filter: u8, cur: &mut [u8], prev: &[u8], bpp: usize) -> Result<(), FacetError> {
    let n = cur.len();
    match filter {
        0 => {}
        1 => {
            for i in bpp..n {
                cur[i] = cur[i].wrapping_add(cur[i - bpp]);
            }
        }
        2 => {
            for i in 0..n {
                cur[i] = cur[i].wrapping_add(prev[i]);
            }
        }
        3 => {
            for i in 0..n {
                let a = if i >= bpp { cur[i - bpp] as u32 } else { 0 };
                let b = prev[i] as u32;
                cur[i] = cur[i].wrapping_add(((a + b) / 2) as u8);
            }
        }
        4 => {
            for i in 0..n {
                let a = if i >= bpp { cur[i - bpp] } else { 0 };
                let b = prev[i];
                let c = if i >= bpp { prev[i - bpp] } else { 0 };
                cur[i] = cur[i].wrapping_add(paeth(a, b, c));
            }
        }
        other => return Err(FacetError::BadFilter(other)),
    }
    Ok(())
}

// ── The IDAT stream, pulled through the VFS ─────────────────────────────────────────────────────

/// One IDAT chunk's payload, as a file range.
#[derive(Clone, Copy)]
struct Span {
    off: u64,
    len: usize,
}

/// A [`ByteSource`] over the CONCATENATED IDAT payloads, read through the VFS in [`CHUNK`] pieces.
///
/// PNG defines the compressed image as the concatenation of every IDAT chunk's data field with the
/// chunk framing removed (§5.4), so this is not a convenience — a decoder handed one chunk at a time
/// would break every back-reference that crosses a boundary. The spans are indexed once by
/// [`index_chunks`] and walked here, which is also why a corrupted chunk LENGTH is caught during the
/// index rather than as a mysterious `TruncatedInput` halfway through the picture.
///
/// A failed read surfaces as `None`, i.e. as `InflateError::TruncatedInput`, and is also recorded in
/// `io_error` so the refusal line can name the VFS's own words instead of the decoder's guess.
struct IdatSource<'a> {
    mt: &'a crate::fs::vfs::MountTable,
    path: &'a str,
    spans: &'a [Span],
    /// Index of the span being read.
    span: usize,
    /// Bytes already consumed from the current span.
    done: usize,
    buf: Vec<u8>,
    /// Read cursor within `buf`.
    at: usize,
    io_error: Option<String>,
}

impl<'a> IdatSource<'a> {
    fn new(mt: &'a crate::fs::vfs::MountTable, path: &'a str, spans: &'a [Span]) -> Self {
        Self { mt, path, spans, span: 0, done: 0, buf: Vec::new(), at: 0, io_error: None }
    }

    /// Refill `buf` from the next unread stretch. `false` at the true end of the IDAT stream.
    fn refill(&mut self) -> bool {
        while self.span < self.spans.len() {
            let s = self.spans[self.span];
            if self.done >= s.len {
                self.span += 1;
                self.done = 0;
                continue;
            }
            let want = core::cmp::min(CHUNK, s.len - self.done);
            match self.mt.read(self.path, s.off + self.done as u64, want) {
                Ok(b) if !b.is_empty() => {
                    self.done += b.len();
                    self.buf = b;
                    self.at = 0;
                    return true;
                }
                Ok(_) => {
                    self.io_error = Some(String::from("short-read"));
                    return false;
                }
                Err(e) => {
                    self.io_error = Some(vfs_why(e));
                    return false;
                }
            }
        }
        false
    }
}

impl ByteSource for IdatSource<'_> {
    #[inline]
    fn next(&mut self) -> Option<u8> {
        if self.at >= self.buf.len() && !self.refill() {
            return None;
        }
        let b = self.buf[self.at];
        self.at += 1;
        Some(b)
    }
}

/// A [`ByteSource`] over a slice — the fixture's source, and the shape any future in-memory caller
/// (a clipboard, a network fetch) already has.
pub struct SliceSource<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> SliceSource<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }
}

impl ByteSource for SliceSource<'_> {
    #[inline]
    fn next(&mut self) -> Option<u8> {
        let b = *self.bytes.get(self.at)?;
        self.at += 1;
        Some(b)
    }
}

// ── The row sink: unfilter, convert, box-downscale, straight into the surface ───────────────────

/// The consumer of every inflated byte, and the reason no decoded image is ever held.
///
/// It is a state machine over three facts — "am I owed a filter byte", "how much of this scanline do
/// I have", and "how many source rows are in the accumulator" — and its whole working set is
/// `2 * row_bytes` (the current and previous scanlines, which PNG's filters require) plus
/// `3 * out_w` accumulator words. For a 1920x1200 truecolour-8 image that is 11.5 KB + 11.5 KB, not
/// 6.9 MB.
///
/// The box filter is a plain sum-and-divide over `k*k` whole source pixels — see [`fit`] for why the
/// divisor is constant. It is a real downscale rather than a nearest-neighbour drop: a screenshot of
/// 8-pixel text point-sampled at 1/2 loses half its strokes, and the whole point of opening
/// `SCREEN6.PNG` is to READ what is in it.
struct RowSink<'a> {
    ihdr: Ihdr,
    palette: &'a [u8],
    /// Filter unit, in bytes.
    bpp: usize,
    /// Unfiltered scanline length, in bytes.
    row_bytes: usize,
    cur: Vec<u8>,
    prev: Vec<u8>,
    /// Bytes of `cur` filled so far.
    fill: usize,
    /// The filter byte for the row being assembled, once seen.
    filter: Option<u8>,
    /// Source rows completed.
    rows: u32,

    /// Reduction factor.
    k: usize,
    out_w: usize,
    out_h: usize,
    /// `3 * out_w` running sums for the output row being built.
    acc: Vec<u32>,
    /// Source rows already folded into `acc`.
    acc_rows: usize,
    /// Output row being built.
    out_y: usize,
    /// The window surface, ARGB8888, `out_w * out_h` words.
    px: &'a mut [u32],

    /// The first thing that went wrong, if anything. Set once; every later byte is refused, which is
    /// what turns a bad row into `InflateError::SinkRejected` and stops the decode immediately.
    err: Option<FacetError>,
}

impl<'a> RowSink<'a> {
    #[allow(clippy::too_many_arguments)]
    fn new(
        ihdr: Ihdr,
        palette: &'a [u8],
        k: usize,
        out_w: usize,
        out_h: usize,
        px: &'a mut [u32],
    ) -> Result<Self, FacetError> {
        let bpp = ihdr.filter_unit().ok_or(FacetError::BadIhdr("colour type"))?;
        let row_bytes = ihdr.row_bytes().ok_or(FacetError::BadIhdr("scanline overflow"))?;
        let mut cur = Vec::new();
        let mut prev = Vec::new();
        let mut acc = Vec::new();
        if cur.try_reserve_exact(row_bytes).is_err()
            || prev.try_reserve_exact(row_bytes).is_err()
            || acc.try_reserve_exact(out_w * 3).is_err()
        {
            return Err(FacetError::OutOfMemory(row_bytes * 2 + out_w * 12));
        }
        cur.resize(row_bytes, 0);
        prev.resize(row_bytes, 0);
        acc.resize(out_w * 3, 0);
        Ok(Self {
            ihdr,
            palette,
            bpp,
            row_bytes,
            cur,
            prev,
            fill: 0,
            filter: None,
            rows: 0,
            k,
            out_w,
            out_h,
            acc,
            acc_rows: 0,
            out_y: 0,
            px,
            err: None,
        })
    }

    #[inline]
    fn rgb(&self, x: usize) -> (u8, u8, u8) {
        pix_rgb(&self.ihdr, self.palette, &self.cur, x)
    }

    /// Fold one completed source scanline into the accumulator, emitting an output row every `k`.
    fn consume_row(&mut self) {
        // Source rows past `out_h * k` are the dropped remainder — see [`fit`].
        if self.out_y >= self.out_h {
            return;
        }
        let k = self.k;
        for x in 0..self.out_w * k {
            let (r, g, b) = self.rgb(x);
            let o = (x / k) * 3;
            self.acc[o] += r as u32;
            self.acc[o + 1] += g as u32;
            self.acc[o + 2] += b as u32;
        }
        self.acc_rows += 1;
        if self.acc_rows < k {
            return;
        }
        let div = (k * k) as u32;
        let base = self.out_y * self.out_w;
        for x in 0..self.out_w {
            let o = x * 3;
            let r = (self.acc[o] / div) & 0xFF;
            let g = (self.acc[o + 1] / div) & 0xFF;
            let b = (self.acc[o + 2] / div) & 0xFF;
            self.px[base + x] = 0xFF00_0000 | (r << 16) | (g << 8) | b;
        }
        for v in self.acc.iter_mut() {
            *v = 0;
        }
        self.acc_rows = 0;
        self.out_y += 1;
    }
}

impl Sink for RowSink<'_> {
    fn push(&mut self, byte: u8) -> Result<(), ()> {
        if self.err.is_some() {
            return Err(());
        }
        // More scanlines than IHDR declared: refuse rather than run past the surface.
        if self.rows >= self.ihdr.height {
            self.err = Some(FacetError::RowCount { got: self.rows + 1, want: self.ihdr.height });
            return Err(());
        }
        let Some(f) = self.filter else {
            self.filter = Some(byte);
            return Ok(());
        };
        self.cur[self.fill] = byte;
        self.fill += 1;
        if self.fill < self.row_bytes {
            return Ok(());
        }
        // A full scanline: unfilter it against the previous one, fold it, and rotate the buffers so
        // the row just finished becomes `prev` with no copy.
        {
            let bpp = self.bpp;
            let (cur, prev) = (&mut self.cur, &self.prev);
            if let Err(e) = unfilter(f, cur, prev, bpp) {
                self.err = Some(e);
                return Err(());
            }
        }
        self.consume_row();
        core::mem::swap(&mut self.cur, &mut self.prev);
        self.fill = 0;
        self.filter = None;
        self.rows += 1;
        Ok(())
    }
}

// ── Pixel conversion (shared by the row sink and the Adam7 sink) ────────────────────────────────

/// Read the `n`-th sample of the `x`-th pixel out of the unfiltered scanline, normalised to 8
/// bits. Sub-byte depths are unpacked MSB-first, which is PNG's order (§7.2); depth 16 keeps the
/// high byte, which is exactly what a truncation to 8 bits per channel is.
#[inline]
fn pix_sample(ihdr: &Ihdr, cur: &[u8], x: usize, chan: usize, chans: usize) -> u8 {
    let d = ihdr.depth as usize;
    match d {
        8 => *cur.get(x * chans + chan).unwrap_or(&0),
        16 => *cur.get((x * chans + chan) * 2).unwrap_or(&0),
        _ => {
            // 1, 2 or 4 bits — only ever one channel (greyscale or palette index).
            let idx = x * chans + chan;
            let per = 8 / d;
            let byte = *cur.get(idx / per).unwrap_or(&0);
            let shift = 8 - d - (idx % per) * d;
            let raw = (byte >> shift) & ((1u16 << d) - 1) as u8;
            if ihdr.colour == 3 {
                raw // a palette INDEX is not a level; it must not be scaled
            } else {
                // Scale the level to full range by bit replication: 1 -> 0xFF, 0b10 -> 0xAA.
                let max = ((1u16 << d) - 1) as u32;
                ((raw as u32 * 255 + max / 2) / max) as u8
            }
        }
    }
}

/// The `x`-th pixel of the current scanline as an 8-bit RGB triple.
#[inline]
fn pix_rgb(ihdr: &Ihdr, palette: &[u8], cur: &[u8], x: usize) -> (u8, u8, u8) {
    match ihdr.colour {
        0 => {
            let g = pix_sample(ihdr, cur, x, 0, 1);
            (g, g, g)
        }
        2 => (pix_sample(ihdr, cur, x, 0, 3), pix_sample(ihdr, cur, x, 1, 3), pix_sample(ihdr, cur, x, 2, 3)),
        3 => {
            let i = pix_sample(ihdr, cur, x, 0, 1) as usize * 3;
            match palette.get(i..i + 3) {
                Some(e) => (e[0], e[1], e[2]),
                // An index past the PLTE. `decode`'s pre-check refuses a palette that is short
                // for the DEPTH, so reaching here needs a file that is both short and lying;
                // black is the bounded answer and the image still shows.
                None => (0, 0, 0),
            }
        }
        4 => {
            let g = pix_sample(ihdr, cur, x, 0, 2);
            (g, g, g)
        }
        _ => (pix_sample(ihdr, cur, x, 0, 4), pix_sample(ihdr, cur, x, 1, 4), pix_sample(ihdr, cur, x, 2, 4)),
    }
}


// ── Reading the container ───────────────────────────────────────────────────────────────────────

/// The VFS's own words for a refusal — Quarry's `why`, in this module's vocabulary so a `[facet]`
/// line never has to say "an error occurred".
fn vfs_why(e: crate::fs::vfs::VfsError) -> String {
    use crate::fs::vfs::VfsError as E;
    match e {
        E::NoSuchVolume => String::from("no-such-volume"),
        E::NoSuchPath => String::from("enoent"),
        E::NotADirectory => String::from("enotdir"),
        E::IsADirectory => String::from("eisdir"),
        E::Denied => String::from("eacces"),
        E::Unsupported => String::from("enotsup"),
        E::Backend(s) => alloc::format!("backend({})", s),
    }
}

/// Parse the 13-byte IHDR payload.
pub fn parse_ihdr(d: &[u8]) -> Result<Ihdr, FacetError> {
    if d.len() < 13 {
        return Err(FacetError::Truncated);
    }
    let ihdr = Ihdr {
        width: u32::from_be_bytes([d[0], d[1], d[2], d[3]]),
        height: u32::from_be_bytes([d[4], d[5], d[6], d[7]]),
        depth: d[8],
        colour: d[9],
        interlaced: d[12] == 1,
    };
    if ihdr.width == 0 || ihdr.height == 0 {
        return Err(FacetError::BadIhdr("zero dimension"));
    }
    if ihdr.width > MAX_DIM || ihdr.height > MAX_DIM {
        return Err(FacetError::BadIhdr("dimension over 16384"));
    }
    if ihdr.channels().is_none() {
        return Err(FacetError::BadIhdr("undefined colour type"));
    }
    if !ihdr.depth_legal() {
        return Err(FacetError::BadIhdr("depth illegal for this colour type"));
    }
    if d[10] != 0 {
        return Err(FacetError::BadIhdr("compression method != 0"));
    }
    if d[11] != 0 {
        return Err(FacetError::BadIhdr("filter method != 0"));
    }
    if d[12] > 1 {
        return Err(FacetError::BadIhdr("interlace method"));
    }
    // The scanline arithmetic must not overflow before anything allocates against it.
    if ihdr.row_bytes().is_none() {
        return Err(FacetError::BadIhdr("scanline overflow"));
    }
    Ok(ihdr)
}

/// Walk the chunk list once: validate the signature, parse IHDR, index every IDAT span, and pick up
/// PLTE if there is one.
///
/// It reads only chunk HEADERS (8 bytes each) plus IHDR and PLTE, never an IDAT payload — that is
/// [`IdatSource`]'s job and it happens during the inflate. The walk is bounded by the file size and
/// by [`MAX_IDAT`], and a chunk whose length would run past the end of the file is a truncation
/// rather than a seek into nothing.
fn index_chunks(
    mt: &crate::fs::vfs::MountTable,
    path: &str,
    size: u64,
) -> Result<(Ihdr, Vec<Span>, Vec<u8>), FacetError> {
    let head = mt.read(path, 0, 8).map_err(|e| FacetError::Vfs(vfs_why(e)))?;
    if head.len() < 8 || head[..8] != SIGNATURE[..] {
        return Err(FacetError::NotPng);
    }
    let mut ihdr: Option<Ihdr> = None;
    let mut spans: Vec<Span> = Vec::new();
    let mut palette: Vec<u8> = Vec::new();
    let mut at: u64 = 8;
    loop {
        if at + 8 > size {
            return Err(FacetError::Truncated);
        }
        let hdr = mt.read(path, at, 8).map_err(|e| FacetError::Vfs(vfs_why(e)))?;
        if hdr.len() < 8 {
            return Err(FacetError::Truncated);
        }
        let len = u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]) as u64;
        // A PNG chunk length is a u32 that may not exceed 2^31-1 (§5.3), and it must fit the file
        // with its 4-byte CRC. Both are checked here so no later read can be handed a bad range.
        if len > 0x7FFF_FFFF || at + 8 + len + 4 > size {
            return Err(FacetError::Truncated);
        }
        let kind = [hdr[4], hdr[5], hdr[6], hdr[7]];
        let data_at = at + 8;
        match &kind {
            b"IHDR" => {
                let d = mt.read(path, data_at, 13).map_err(|e| FacetError::Vfs(vfs_why(e)))?;
                ihdr = Some(parse_ihdr(&d)?);
            }
            b"PLTE" => {
                if len % 3 != 0 || len == 0 || len > 256 * 3 {
                    return Err(FacetError::BadPalette("length not 3n, or over 256 entries"));
                }
                palette = mt
                    .read(path, data_at, len as usize)
                    .map_err(|e| FacetError::Vfs(vfs_why(e)))?;
            }
            b"IDAT" => {
                if spans.len() >= MAX_IDAT {
                    return Err(FacetError::BadIdat("more than 4096 chunks"));
                }
                if len > 0 {
                    spans.push(Span { off: data_at, len: len as usize });
                }
            }
            b"IEND" => break,
            _ => {}
        }
        at = data_at + len + 4;
    }
    let ihdr = ihdr.ok_or(FacetError::BadIhdr("no IHDR chunk"))?;
    if spans.is_empty() {
        return Err(FacetError::BadIdat("no IDAT chunk"));
    }
    if ihdr.colour == 3 {
        // A palette image needs enough entries for every index its DEPTH can express. Checked here,
        // once, rather than per pixel: `RowSink::rgb`'s `None` arm then covers only a file that is
        // internally inconsistent, and cannot be the ordinary case.
        let need = 1usize << ihdr.depth;
        if palette.len() < need * 3 {
            return Err(FacetError::BadPalette("PLTE shorter than the depth's index range"));
        }
    }
    Ok((ihdr, spans, palette))
}

// ── Adam7 ───────────────────────────────────────────────────────────────────────────────────────

/// PNG §8.2's seven passes: `(x0, y0, dx, dy)`.
const ADAM7: [(usize, usize, usize, usize); 7] =
    [(0, 0, 8, 8), (4, 0, 8, 8), (0, 4, 4, 8), (2, 0, 4, 4), (0, 2, 2, 4), (1, 0, 2, 2), (0, 1, 1, 2)];

/// Pixels per row and rows of pass `p` for a `w x h` image (either may be 0: the pass is empty and
/// carries no filter bytes at all).
fn adam7_dims(p: usize, w: usize, h: usize) -> (usize, usize) {
    let (x0, y0, dx, dy) = ADAM7[p];
    ((w + dx - 1 - x0.min(w + dx - 1)) / dx, (h + dy - 1 - y0.min(h + dy - 1)) / dy)
}

/// The interlaced counterpart of [`RowSink`]: each pass is its own little image with its own filter
/// chain, scattered into the full-size `px` (k = 1 only — an interlaced image cannot be reduced
/// row by row, which is why it is refused over the base cap).
struct Adam7Sink<'a> {
    ihdr: Ihdr,
    palette: &'a [u8],
    bpp: usize,
    pass: usize,
    pw: usize,
    ph: usize,
    row_bytes: usize,
    row: usize,
    cur: Vec<u8>,
    prev: Vec<u8>,
    fill: usize,
    filter: Option<u8>,
    px: &'a mut [u32],
    done: bool,
    err: Option<FacetError>,
}

impl<'a> Adam7Sink<'a> {
    fn new(ihdr: Ihdr, palette: &'a [u8], px: &'a mut [u32]) -> Result<Self, FacetError> {
        let bpp = ihdr.filter_unit().ok_or(FacetError::BadIhdr("colour type"))?;
        let max = ihdr.row_bytes().ok_or(FacetError::BadIhdr("scanline overflow"))?;
        let (mut cur, mut prev) = (Vec::new(), Vec::new());
        if cur.try_reserve_exact(max).is_err() || prev.try_reserve_exact(max).is_err() {
            return Err(FacetError::OutOfMemory(max * 2));
        }
        cur.resize(max, 0);
        prev.resize(max, 0);
        let mut s = Self {
            ihdr, palette, bpp, pass: 0, pw: 0, ph: 0, row_bytes: 0, row: 0, cur, prev, fill: 0,
            filter: None, px, done: false, err: None,
        };
        s.enter_pass(0);
        Ok(s)
    }

    /// Move to the first non-empty pass at or after `p`; `done` when there is none.
    fn enter_pass(&mut self, mut p: usize) {
        while p < 7 {
            let (pw, ph) = adam7_dims(p, self.ihdr.width as usize, self.ihdr.height as usize);
            if pw > 0 && ph > 0 {
                self.pass = p;
                self.pw = pw;
                self.ph = ph;
                self.row = 0;
                self.row_bytes = (pw * self.ihdr.bits_per_pixel().unwrap_or(8)).div_ceil(8);
                for v in self.prev.iter_mut() {
                    *v = 0;
                }
                return;
            }
            p += 1;
        }
        self.done = true;
    }
}

impl Sink for Adam7Sink<'_> {
    fn push(&mut self, byte: u8) -> Result<(), ()> {
        if self.err.is_some() {
            return Err(());
        }
        if self.done {
            self.err = Some(FacetError::RowCount { got: self.ihdr.height + 1, want: self.ihdr.height });
            return Err(());
        }
        let Some(f) = self.filter else {
            self.filter = Some(byte);
            return Ok(());
        };
        self.cur[self.fill] = byte;
        self.fill += 1;
        if self.fill < self.row_bytes {
            return Ok(());
        }
        let n = self.row_bytes;
        if let Err(e) = unfilter(f, &mut self.cur[..n], &self.prev[..n], self.bpp) {
            self.err = Some(e);
            return Err(());
        }
        let (x0, y0, dx, dy) = ADAM7[self.pass];
        let y = y0 + self.row * dy;
        let w = self.ihdr.width as usize;
        for i in 0..self.pw {
            let (r, g, b) = pix_rgb(&self.ihdr, self.palette, &self.cur[..n], i);
            self.px[y * w + x0 + i * dx] = 0xFF00_0000 | ((r as u32) << 16) | ((g as u32) << 8) | b as u32;
        }
        core::mem::swap(&mut self.cur, &mut self.prev);
        self.fill = 0;
        self.filter = None;
        self.row += 1;
        if self.row >= self.ph {
            let p = self.pass + 1;
            self.enter_pass(p);
        }
        Ok(())
    }
}

// ── Decoding ────────────────────────────────────────────────────────────────────────────────────

/// What a completed decode produced.
pub struct Decoded {
    pub ihdr: Ihdr,
    /// Reduction factor — 1 means "shown at native size".
    pub k: usize,
    pub out_w: usize,
    pub out_h: usize,
    /// Source scanlines the decoder actually produced.
    pub rows: u32,
}

/// Decode a PNG from any [`ByteSource`] of its IDAT stream into `px` at `out_w x out_h`.
///
/// Split out from [`open_path`] so the fixture can drive the WHOLE pipeline — zlib, all five
/// filters, the colour conversion and the box filter — over synthetic bytes with no volume, no panel
/// and no window. That is the difference between a fixture that tests the plumbing and one that
/// tests the decode.
pub fn decode_into<S: ByteSource>(
    ihdr: Ihdr,
    palette: &[u8],
    src: &mut S,
    k: usize,
    out_w: usize,
    out_h: usize,
    px: &mut [u32],
) -> Result<Decoded, FacetError> {
    if ihdr.interlaced {
        if k != 1 || out_w != ihdr.width as usize || out_h != ihdr.height as usize {
            return Err(FacetError::Interlaced);
        }
        let mut sink = Adam7Sink::new(ihdr, palette, px)?;
        let r = inflate::zlib_inflate(src, &mut sink);
        if let Some(e) = sink.err.clone() {
            return Err(e);
        }
        r.map_err(FacetError::Inflate)?;
        if !sink.done {
            return Err(FacetError::RowCount { got: 0, want: ihdr.height });
        }
        return Ok(Decoded { ihdr, k, out_w, out_h, rows: ihdr.height });
    }
    let mut sink = RowSink::new(ihdr, palette, k, out_w, out_h, px)?;
    let r = inflate::zlib_inflate(src, &mut sink);
    // The sink's own error outranks the decoder's: `SinkRejected` is what the decoder says when THIS
    // module refused a byte, and the reason it refused is the finding.
    if let Some(e) = sink.err.clone() {
        return Err(e);
    }
    r.map_err(FacetError::Inflate)?;
    if sink.rows != ihdr.height {
        return Err(FacetError::RowCount { got: sink.rows, want: ihdr.height });
    }
    Ok(Decoded { ihdr, k, out_w, out_h, rows: sink.rows })
}

// ── Lifecycle ───────────────────────────────────────────────────────────────────────────────────

/// IMGVIEW — the BASE image is what zoom and pan re-sample, so unlike the first viewer this one
/// holds a decoded picture: at most `BASE_W x BASE_H` pixels (12 MB at the cap), box-reduced by an
/// integer `k` only when the file is larger. The window surface is a separate viewport buffer.
const BASE_W: usize = 2048;
const BASE_H: usize = 1536;
/// Zoom steps, percent of the BASE image.
const ZSTEPS: [u32; 8] = [25, 50, 75, 100, 150, 200, 300, 400];
/// Letterbox colour.
const BG: u32 = 0xFF20_2020;

/// Everything the window needs to re-render itself.
struct View {
    path: String,
    /// Base image, 0xFFRRGGBB, `bw * bh`.
    px: Vec<u32>,
    bw: usize,
    bh: usize,
    /// Integer reduction of the base against the file.
    k: usize,
    src_w: u32,
    src_h: u32,
    ihdr: Ihdr,
    bytes: u64,
    mtime: Option<crate::fs::vfs::VfsTime>,
    /// Viewport size (the window content).
    vw: usize,
    vh: usize,
    /// Zoom, percent of the base image.
    zoom: u32,
    fit_zoom: u32,
    /// Top-left of the zoomed image in viewport coordinates (re-clamped by [`layout`]).
    ox: i32,
    oy: i32,
    info: bool,
}

static VIEW: spin::Mutex<Option<View>> = spin::Mutex::new(None);

/// The viewport surface — the window's own buffer, ARGB8888 `vw * vh` words. Re-rendered IN PLACE (it
/// is never reallocated while the row names it) and freed by [`close`].
static SURF: spin::Mutex<Vec<u32>> = spin::Mutex::new(Vec::new());

/// The path currently on the glass, for the census line, browse and the idempotent re-open.
static SHOWN: spin::Mutex<String> = spin::Mutex::new(String::new());

/// The path a gesture asked for, waiting for a pass that is allowed to open it.
static PENDING: spin::Mutex<Option<String>> = spin::Mutex::new(None);

/// Ask for `path` to be opened. **This is what a click or a key press calls, and [`open_path`] is
/// not.** THE LATCH IS NOT CEREMONY — it is `dock::press_at`'s law: the click router runs on a 16 KiB
/// kernel stack and [`open_inner`] walks the chunk list, streams megabytes and inflates them. The
/// gesture stores a path; [`service`] opens it from the render pass. A second request before the
/// first is drained REPLACES it.
pub fn request_open(path: &str) {
    *PENDING.lock() = Some(String::from(path));
}

/// A viewer command, queued by the router-band input hooks and applied by [`service`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Cmd {
    ZoomIn,
    ZoomOut,
    Fit,
    Actual,
    Next,
    Prev,
    Delete,
    Info,
    Wheel(i8),
}

static CMDS: spin::Mutex<Vec<Cmd>> = spin::Mutex::new(Vec::new());
/// Drag anchor (screen coordinates of the last sample); `DRAG_ON` says whether a drag is live.
static DRAG_ON: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static DRAG_X: core::sync::atomic::AtomicI32 = core::sync::atomic::AtomicI32::new(0);
static DRAG_Y: core::sync::atomic::AtomicI32 = core::sync::atomic::AtomicI32::new(0);

fn push_cmd(c: Cmd) {
    let mut q = CMDS.lock();
    if q.len() < 16 {
        q.push(c);
    }
}

/// Drain a pending open request, queued commands and a live drag. Safe to call on every pass: a
/// quiet pass is two uncontended locks and an atomic read.
///
/// Chained from [`super::quarry::live::service`], which is already drained from both places this
/// desktop services furniture, so Facet needs no drain site of its own.
pub fn service() {
    let want = PENDING.lock().take();
    if let Some(p) = want {
        open_path(&p);
    }
    let cmds: Vec<Cmd> = core::mem::take(&mut *CMDS.lock());
    for c in cmds {
        apply(c);
    }
    if DRAG_ON.load(Ordering::Relaxed) {
        drag_poll();
    }
    anim::tick(); // QUARRY2 (B336): step an animated image's frames
}

/// Is Facet's window live?
pub fn is_open() -> bool {
    WIN.load(Ordering::Relaxed) != wm::WIN_NONE
}

/// The file currently shown, or `""`.
pub fn shown() -> String {
    SHOWN.lock().clone()
}

/// Close the window and free the surface and the base image.
pub fn close() {
    let id = WIN.swap(wm::WIN_NONE, Ordering::Relaxed);
    if id == wm::WIN_NONE {
        return;
    }
    DRAG_ON.store(false, Ordering::Relaxed);
    // `wm::close` FIRST — the row must stop naming the buffer before the buffer goes away. It spins
    // on the drain barrier, so it is called with no lock of ours held.
    wm::close(id);
    *VIEW.lock() = None;
    *SURF.lock() = Vec::new();
    SHOWN.lock().clear();
    anim::stop(); // FACETANIM (B358): the animation ends with its window
    serial_println!("[facet] closed win={} paints={}", id, PAINTS.load(Ordering::Relaxed));
}

/// Does this name end `.PNG`, case-insensitively? Quarry's routing test.
pub fn is_png_name(name: &str) -> bool {
    let n = name.as_bytes();
    n.len() > 4 && n[n.len() - 4..].eq_ignore_ascii_case(b".png")
}

/// The name a window carries: the DOCUMENT's, not the app's (Peter's R36).
fn title_of(path: &str) -> String {
    String::from(path.rsplit('/').next().unwrap_or(path))
}

fn colour_name(c: u8) -> &'static str {
    match c {
        2 => "rgb",
        6 => "rgba",
        3 => "pal",
        _ => "gray",
    }
}

/// `(folder, sorted image names with their file times)` for `path`'s folder — PNG only (no JPEG
/// decoder exists in this tree). Case-insensitive order, so Next/Prev are stable.
fn siblings(path: &str) -> (String, Vec<(String, Option<crate::fs::vfs::VfsTime>)>) {
    let cut = path.rfind('/').unwrap_or(0);
    let dir = if cut == 0 { String::from("/") } else { String::from(&path[..cut]) };
    let mut v: Vec<(String, Option<crate::fs::vfs::VfsTime>)> = Vec::new();
    if let Ok(ents) = crate::shell::vfs_mount_table().read_dir(&dir) {
        for e in ents {
            if matches!(e.kind, crate::fs::vfs::NodeKind::File) && is_png_name(&e.name) {
                v.push((e.name, e.mtime));
            }
        }
    }
    v.sort_by(|a, b| a.0.to_ascii_lowercase().cmp(&b.0.to_ascii_lowercase()));
    (dir, v)
}

fn join_path(dir: &str, name: &str) -> String {
    if dir.ends_with('/') { alloc::format!("{}{}", dir, name) } else { alloc::format!("{}/{}", dir, name) }
}

/// Zoomed size and the effective top-left, written back into the view. Centred when the zoomed
/// picture is smaller than the viewport on that axis; clamped so no gap opens when it is larger.
fn layout(v: &mut View) -> (i32, i32, i64, i64) {
    let zw = ((v.bw as u64 * v.zoom as u64) / 100).max(1) as i64;
    let zh = ((v.bh as u64 * v.zoom as u64) / 100).max(1) as i64;
    let (vw, vh) = (v.vw as i64, v.vh as i64);
    let ox = if zw <= vw { (vw - zw) / 2 } else { (v.ox as i64).clamp(vw - zw, 0) };
    let oy = if zh <= vh { (vh - zh) / 2 } else { (v.oy as i64).clamp(vh - zh, 0) };
    v.ox = ox as i32;
    v.oy = oy as i32;
    (v.ox, v.oy, zw, zh)
}

/// Re-render the viewport from the base image: nearest at >= 100 %, area average below.
fn render(v: &mut View) {
    let (ox, oy, zw, zh) = layout(v);
    let mut s = SURF.lock();
    if s.len() != v.vw * v.vh {
        return;
    }
    let z = v.zoom.max(1) as i64;
    for y in 0..v.vh {
        let zy = y as i64 - oy as i64;
        for x in 0..v.vw {
            let zx = x as i64 - ox as i64;
            s[y * v.vw + x] = if zx < 0 || zy < 0 || zx >= zw || zy >= zh {
                BG
            } else if z >= 100 {
                let sx = ((zx * 100 / z) as usize).min(v.bw - 1);
                let sy = ((zy * 100 / z) as usize).min(v.bh - 1);
                v.px[sy * v.bw + sx]
            } else {
                let sx0 = (zx * 100 / z) as usize;
                let sy0 = (zy * 100 / z) as usize;
                let sx1 = (((zx + 1) * 100 / z) as usize).max(sx0 + 1).min(v.bw);
                let sy1 = (((zy + 1) * 100 / z) as usize).max(sy0 + 1).min(v.bh);
                let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
                for yy in sy0.min(v.bh - 1)..sy1 {
                    for xx in sx0.min(v.bw - 1)..sx1 {
                        let p = v.px[yy * v.bw + xx];
                        r += (p >> 16) & 0xFF;
                        g += (p >> 8) & 0xFF;
                        b += p & 0xFF;
                        n += 1;
                    }
                }
                let n = n.max(1);
                0xFF00_0000 | ((r / n) << 16) | ((g / n) << 8) | (b / n)
            };
        }
    }
    if v.info {
        let face = crate::video::text::Face::Body;
        let ch = face.cell_h();
        let h = (ch + 4).min(v.vh);
        let y0 = v.vh - h;
        for p in s[y0 * v.vw..].iter_mut() {
            *p = 0xFF10_1010;
        }
        let t = match v.mtime {
            Some(t) => alloc::format!("{:04}-{:02}-{:02} {:02}:{:02}", t.year, t.month, t.day, t.hour, t.min),
            None => String::from("no time"),
        };
        let line = alloc::format!(
            "{}x{}  {} bytes  {}-bit {}  {}",
            v.src_w, v.src_h, v.bytes, v.ihdr.depth, colour_name(v.ihdr.colour), t
        );
        let vw = v.vw;
        crate::video::text::draw_text(&mut s, vw, vw, v.vh, 4, y0 + 2, line.as_bytes(), 0x00F0_F0F0, false, face);
    }
}

fn title_for(v: &View) -> String {
    // FACETANIM (B358): ` - frame i/n` while an animation is shown.
    alloc::format!("{} - {}x{} - {}%{}", title_of(&v.path), v.src_w, v.src_h, (v.zoom / v.k.max(1) as u32).max(1), anim::title_suffix())
}

#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
fn retitle(id: wm::WinId, t: &str) {
    wm::retitle(id, t.as_bytes());
}
#[cfg(not(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
fn retitle(_id: wm::WinId, _t: &str) {}

/// Render, retitle and present after any change to the view.
fn refresh(v: &mut View) {
    render(v);
    let id = WIN.load(Ordering::Relaxed);
    retitle(id, &title_for(v));
    let _ = wm::present(id);
    PAINTS.fetch_add(1, Ordering::Relaxed);
}

/// Mint the window over the already-filled [`SURF`] (`vw x vh`). Centred in the work area.
fn mint(title: &str, vw: usize, vh: usize) -> Result<wm::WinId, FacetError> {
    let pi = crate::video::panel_info_nonblocking().ok_or(FacetError::NoWindow("panel-busy"))?;
    let (pw, ph) = (pi.width, pi.height);
    let Some((_scale, ow, oh)) = wm::spawn_geometry(vw, vh) else {
        return Err(FacetError::NoWindow("geometry-unavailable"));
    };
    let wtop = crate::ui_status::top_chrome_h(pw, ph);
    let ox = pw.saturating_sub(ow) / 2;
    let oy = wtop
        + ph.saturating_sub(wtop).saturating_sub(crate::ui_status::chrome_h(ph)).saturating_sub(oh) / 2;
    let base = SURF.lock().as_ptr() as usize;
    let id = wm::create_at(
        OWNER,
        base,
        vw * vh * 4,
        vw as u32,
        vh as u32,
        (vw * 4) as u32,
        title.as_bytes(),
        ox + wm::BORDER(),
        oy + wm::TITLE_H() + wm::BORDER(),
    );
    if id == wm::WIN_NONE {
        return Err(FacetError::NoWindow("create-failed"));
    }
    WIN.store(id, Ordering::Relaxed);
    wm::winid_register_holder(&WIN, "facet");
    wm::focus_changed(OWNER);
    Ok(id)
}

/// M3 — a decode failure is shown IN a window: the file name and the reason.
fn show_message(path: &str, reason: &str) {
    close();
    let (vw, vh) = (460usize, 64usize);
    let mut surf: Vec<u32> = Vec::new();
    if surf.try_reserve_exact(vw * vh).is_err() {
        return;
    }
    surf.resize(vw * vh, 0xFFF5_F2EA);
    let face = crate::video::text::Face::Body;
    let ch = face.cell_h();
    let l1 = alloc::format!("Cannot show {}", title_of(path));
    let l2 = alloc::format!("reason: {}", reason);
    crate::video::text::draw_text(&mut surf, vw, vw - 8, vh, 8, 8, l1.as_bytes(), 0x0021_201E, false, face);
    crate::video::text::draw_text(&mut surf, vw, vw - 8, vh, 8, 8 + ch + 4, l2.as_bytes(), 0x00A0_2020, false, face);
    *SURF.lock() = surf;
    match mint(&title_of(path), vw, vh) {
        Ok(id) => {
            *SHOWN.lock() = String::from(path);
            let _ = wm::present(id);
        }
        Err(_) => {
            *SURF.lock() = Vec::new();
        }
    }
}

/// What a successful open reports.
struct Opened {
    id: wm::WinId,
    ihdr: Ihdr,
    k: usize,
    zoom: u32,
    fit: bool,
    browse_n: usize,
    out_w: usize,
    out_h: usize,
}

/// **Open `path` in Facet.** Idempotent per PATH; a different file replaces the window (one canvas).
/// A failure is a `[facet] refuse` line, a `[facet] decode refused reason=` line AND a message
/// window — never nothing.
pub fn open_path(path: &str) {
    if shown() == path && is_open() && VIEW.lock().is_some() {
        let id = WIN.load(Ordering::Relaxed);
        wm::focus_changed(OWNER);
        serial_println!("[facet] open SKIP reason=already-shown win={} path={}", id, path);
        return;
    }
    match open_inner(path) {
        Ok(o) => {
            serial_println!(
                "[facet] present win={} scale=1/{} src={}x{} shown={}x{} title={}",
                o.id, o.k, o.ihdr.width, o.ihdr.height, o.out_w, o.out_h, title_of(path)
            );
            serial_println!(
                ":: IMGVIEW: path={} WxH={}x{} zoom={} fit={} browse_n={} colour={} -> PASS ::",
                path, o.ihdr.width, o.ihdr.height, (o.zoom / o.k.max(1) as u32).max(1), o.fit as u8,
                o.browse_n, colour_name(o.ihdr.colour)
            );
        }
        Err(e) => {
            let why = e.reason();
            serial_println!("[facet] refuse path={} reason={}", path, why);
            serial_println!("[facet] decode refused reason={} path={}", why, path);
            show_message(path, &why);
        }
    }
}

/// [`open_path`]'s body, so every failure is one `?` and the caller owns the wire format.
fn open_inner(path: &str) -> Result<Opened, FacetError> {
    let t0 = crate::arch::ms();
    let mt = crate::shell::vfs_mount_table();
    let st = mt.stat(path).map_err(|e| FacetError::Vfs(vfs_why(e)))?;
    if matches!(st.kind, crate::fs::vfs::NodeKind::Dir) {
        return Err(FacetError::Vfs(String::from("eisdir")));
    }
    if st.size == 0 || st.size > MAX_FILE {
        return Err(FacetError::Size(st.size));
    }
    if let Some(d) = anim::probe(path) { close(); return anim::open_frames(path, st.size, d); } // QUARRY2 (B336) + FACETANIM (B358): GIF / animated WebP / APNG stream through the pixel_core adapter
    // PIXELCORE (SR25): a file whose first eight bytes are not the PNG signature goes to
    // `pixel_core::decode` whole (JPEG, GIF frame 0, BMP, QOI, WebP lossless) and is box-reduced into
    // the same base image; a PNG keeps the streaming path below unchanged.
    let (ihdr, k, bw, bh, px) = if is_foreign(&mt, path)? {
        let (ihdr, k, bw, bh, px) = decode_foreign(&mt, path, st.size, BASE_W, BASE_H)?;
        serial_println!(
            "[facet] open path={} pixel_core={}x{} bytes={} -> DECODED ms={}",
            path, ihdr.width, ihdr.height, st.size, crate::arch::ms().saturating_sub(t0)
        );
        close();
        (ihdr, k, bw, bh, px)
    } else {
        let (ihdr, spans, palette) = index_chunks(&mt, path, st.size)?;
        let (k, bw, bh) = fit(ihdr.width, ihdr.height, BASE_W, BASE_H).ok_or(FacetError::NoWindow("fit"))?;
        if ihdr.interlaced && k != 1 {
            return Err(FacetError::Interlaced);
        }
        serial_println!(
            "[facet] open path={} ihdr={}x{} depth={} colour={} interlaced={} bytes={} idat-chunks={} -> DECODING",
            path, ihdr.width, ihdr.height, ihdr.depth, ihdr.colour, ihdr.interlaced as u8, st.size, spans.len()
        );
        // One canvas, and one base image at a time: the old window and its base go BEFORE the decode, so
        // the peak is a single base (a failed decode then shows the message window in its place).
        close();
        let mut px: Vec<u32> = Vec::new();
        if px.try_reserve_exact(bw * bh).is_err() {
            return Err(FacetError::OutOfMemory(bw * bh * 4));
        }
        px.resize(bw * bh, 0xFF00_0000);
        let mut src = IdatSource::new(&mt, path, &spans);
        let r = decode_into(ihdr, &palette, &mut src, k, bw, bh, &mut px);
        let d = match (r, src.io_error.take()) {
            (Err(FacetError::Inflate(InflateError::TruncatedInput)), Some(io)) => {
                serial_println!("[facet] decoded rows=0 inflate=vfs-{} ms={}", io, crate::arch::ms().saturating_sub(t0));
                return Err(FacetError::Vfs(io));
            }
            (r, _) => r,
        };
        match &d {
            Ok(d) => serial_println!("[facet] decoded rows={} inflate=OK ms={}", d.rows, crate::arch::ms().saturating_sub(t0)),
            Err(e) => {
                serial_println!("[facet] decoded rows=0 inflate={} ms={}", e.reason(), crate::arch::ms().saturating_sub(t0));
            }
        }
        d?;
        (ihdr, k, bw, bh, px)
    };

    open_base(path, st.size, ihdr, k, bw, bh, px) // QUARRY2 (B336): the window body, shared with the animated-frame open (`anim::open_frames`)
}

/// The window body over a decoded BASE image (`bw x bh`, reduction `k` against the file): fit, mint,
/// present. Split out of [`open_inner`] by QUARRY2 so an animated image's frame 0 opens through the
/// same body.
fn open_base(path: &str, bytes: u64, ihdr: Ihdr, k: usize, bw: usize, bh: usize, px: Vec<u32>) -> Result<Opened, FacetError> {
    let pi = crate::video::panel_info_nonblocking().ok_or(FacetError::NoWindow("panel-busy"))?;
    let (pw, ph) = (pi.width, pi.height);
    let win_w = CEIL_W.min(pw.saturating_sub(2 * wm::BORDER()).max(1));
    let win_h = CEIL_H.min(ph.saturating_sub(wm::TITLE_H() + 2 * wm::BORDER()).max(1));
    // Fit-to-window: reduce when larger than the window, never enlarge on open; the window takes the
    // fitted shape (floor 32x32, the only place a letterbox can appear on open).
    let zf = ((win_w * 100 / bw).min(win_h * 100 / bh).min(100)).max(1) as u32;
    let vw = (bw * zf as usize / 100).max(FLOOR_W);
    let vh = (bh * zf as usize / 100).max(FLOOR_H);
    let (dir, list) = siblings(path);
    let _ = dir;
    let leaf = title_of(path);
    let mtime = list.iter().find(|(n, _)| n.eq_ignore_ascii_case(&leaf)).and_then(|(_, t)| *t);
    let mut surf: Vec<u32> = Vec::new();
    if surf.try_reserve_exact(vw * vh).is_err() {
        return Err(FacetError::OutOfMemory(vw * vh * 4));
    }
    surf.resize(vw * vh, BG);
    *SURF.lock() = surf;
    let mut v = View {
        path: String::from(path),
        px,
        bw,
        bh,
        k,
        src_w: ihdr.width,
        src_h: ihdr.height,
        ihdr,
        bytes,
        mtime,
        vw,
        vh,
        zoom: zf,
        fit_zoom: zf,
        ox: 0,
        oy: 0,
        info: false,
    };
    render(&mut v);
    let id = match mint(&title_for(&v), vw, vh) {
        Ok(id) => id,
        Err(e) => {
            *SURF.lock() = Vec::new();
            return Err(e);
        }
    };
    *SHOWN.lock() = String::from(path);
    let fit_flag = zf < 100;
    let out = Opened { id, ihdr, k, zoom: zf, fit: fit_flag, browse_n: list.len(), out_w: vw, out_h: vh };
    *VIEW.lock() = Some(v);
    let _ = wm::present(id);
    PAINTS.fetch_add(1, Ordering::Relaxed);
    Ok(out)
}

// ── PIXELCORE: every other format pixel_core decodes ───────────────────────────────────────────

/// The largest NON-PNG file Facet will open. Unlike a PNG (streamed, never held), a JPEG/GIF/BMP/QOI/
/// WebP is read whole and decoded whole by `pixel_core::decode`, so this bounds a real buffer: the file
/// plus its RGBA (`MAX_PIXELS`-bounded by pixel_core) sit on the heap together for the decode.
const MAX_FOREIGN: u64 = 16 * 1024 * 1024;

/// Is `path` something other than a PNG (first eight bytes are not the signature)?
fn is_foreign(mt: &crate::fs::vfs::MountTable, path: &str) -> Result<bool, FacetError> {
    let head = mt.read(path, 0, 8).map_err(|e| FacetError::Vfs(vfs_why(e)))?;
    Ok(head.len() < 8 || head[..8] != SIGNATURE)
}

/// Read `path` whole, `pixel_core::decode_first_frame` it (FACETANIM: a still; an animation that reaches
/// here — the adapter refused it — shows frame 0 without compositing the rest), and
/// box-reduce the RGBA by `fit`'s integer `k` into `0xFFRRGGBB` base pixels. Alpha is dropped, exactly
/// as the PNG path drops it. Returns `(synthetic Ihdr: depth 8 colour 6, k, out_w, out_h, px)`.
fn decode_foreign(
    mt: &crate::fs::vfs::MountTable,
    path: &str,
    size: u64,
    bw: usize,
    bh: usize,
) -> Result<(Ihdr, usize, usize, usize, Vec<u32>), FacetError> {
    if size > MAX_FOREIGN {
        return Err(FacetError::Size(size));
    }
    let mut bytes: Vec<u8> = Vec::new();
    if bytes.try_reserve_exact(size as usize).is_err() {
        return Err(FacetError::OutOfMemory(size as usize));
    }
    while (bytes.len() as u64) < size {
        let want = core::cmp::min(CHUNK as u64, size - bytes.len() as u64) as usize;
        let b = mt.read(path, bytes.len() as u64, want).map_err(|e| FacetError::Vfs(vfs_why(e)))?;
        if b.is_empty() {
            return Err(FacetError::Vfs(String::from("short-read")));
        }
        bytes.extend_from_slice(&b);
    }
    let img = pixel_core::decode_first_frame(&bytes).map_err(FacetError::Pixel)?;
    drop(bytes);
    let ihdr = Ihdr { width: img.width, height: img.height, depth: 8, colour: 6, interlaced: false };
    let (k, ow, oh) = fit(img.width, img.height, bw, bh).ok_or(FacetError::NoWindow("fit"))?;
    let mut px: Vec<u32> = Vec::new();
    if px.try_reserve_exact(ow * oh).is_err() {
        return Err(FacetError::OutOfMemory(ow * oh * 4));
    }
    let iw = img.width as usize;
    let n = (k * k) as u32;
    for oy in 0..oh {
        for ox in 0..ow {
            let (mut r, mut g, mut b) = (0u32, 0u32, 0u32);
            for sy in oy * k..oy * k + k {
                for sx in ox * k..ox * k + k {
                    let p = &img.rgba[(sy * iw + sx) * 4..(sy * iw + sx) * 4 + 3];
                    r += p[0] as u32;
                    g += p[1] as u32;
                    b += p[2] as u32;
                }
            }
            px.push(0xFF00_0000 | ((r / n) << 16) | ((g / n) << 8) | (b / n));
        }
    }
    Ok((ihdr, k, ow, oh, px))
}

// ── Commands ────────────────────────────────────────────────────────────────────────────────────

/// The pointer in viewport coordinates when it is over the content, else the viewport centre.
fn anchor_point(vw: usize, vh: usize) -> (i32, i32) {
    let id = WIN.load(Ordering::Relaxed);
    if let (Some(pi), Some(info)) = (crate::video::panel_info_nonblocking(), wm::info(id)) {
        let (x, y) = crate::pal::cursor::pos(pi.width as i32, pi.height as i32);
        let sc = info.scale.max(1) as i32;
        let (cx, cy) = ((x - info.x as i32) / sc, (y - info.y as i32) / sc);
        if x >= info.x as i32 && y >= info.y as i32 && cx < vw as i32 && cy < vh as i32 {
            return (cx, cy);
        }
    }
    ((vw / 2) as i32, (vh / 2) as i32)
}

/// Set the zoom keeping the image point under `(cx, cy)` fixed.
fn set_zoom(v: &mut View, z: u32, cx: i32, cy: i32) {
    let (ox, oy, _, _) = layout(v);
    let old = v.zoom.max(1) as i64;
    let (bx, by) = ((cx as i64 - ox as i64) * 100 / old, (cy as i64 - oy as i64) * 100 / old);
    v.zoom = z.clamp(1, 400);
    v.ox = (cx as i64 - bx * v.zoom as i64 / 100) as i32;
    v.oy = (cy as i64 - by * v.zoom as i64 / 100) as i32;
}

fn step_up(z: u32) -> u32 {
    ZSTEPS.iter().copied().find(|&s| s > z).unwrap_or(400)
}

fn step_down(z: u32) -> u32 {
    ZSTEPS.iter().rev().copied().find(|&s| s < z).unwrap_or(ZSTEPS[0])
}

/// Neighbour of the shown file in its folder: `(path, count)`, wrapping; `None` when it is alone.
fn neighbour(path: &str, forward: bool) -> (Option<String>, usize) {
    let (dir, list) = siblings(path);
    let leaf = title_of(path);
    let n = list.len();
    let Some(i) = list.iter().position(|(nm, _)| nm.eq_ignore_ascii_case(&leaf)) else {
        return (None, n);
    };
    if n < 2 {
        return (None, n);
    }
    let j = if forward { (i + 1) % n } else { (i + n - 1) % n };
    (Some(join_path(&dir, &list[j].0)), n)
}

/// Apply one queued command.
fn apply(c: Cmd) {
    if !is_open() {
        return;
    }
    let path = shown();
    match c {
        Cmd::Next | Cmd::Prev => {
            let (nb, n) = neighbour(&path, c == Cmd::Next);
            match nb {
                Some(p) => {
                    serial_println!("[facet] browse {} n={} -> {}", if c == Cmd::Next { "next" } else { "prev" }, n, p);
                    open_path(&p);
                }
                None => serial_println!("[facet] browse n={} -> none (alone in folder)", n),
            }
        }
        Cmd::Delete => {
            let (nb, _) = neighbour(&path, true);
            match crate::fs::trash::trash(&path) {
                Ok(_) => {
                    serial_println!("[facet] trash path={} -> ok", path);
                    match nb {
                        Some(p) => open_path(&p),
                        None => close(),
                    }
                }
                Err(e) => serial_println!("[facet] trash path={} -> refused {}", path, e),
            }
        }
        _ => {
            let mut g = VIEW.lock();
            let Some(v) = g.as_mut() else { return };
            let (cx, cy) = anchor_point(v.vw, v.vh);
            match c {
                Cmd::ZoomIn => {
                    let z = step_up(v.zoom);
                    set_zoom(v, z, cx, cy)
                }
                Cmd::ZoomOut => {
                    let z = step_down(v.zoom);
                    set_zoom(v, z, cx, cy)
                }
                Cmd::Wheel(d) => {
                    let z = if d > 0 { step_up(v.zoom) } else { step_down(v.zoom) };
                    set_zoom(v, z, cx, cy)
                }
                Cmd::Fit => {
                    let z = v.fit_zoom;
                    set_zoom(v, z, cx, cy)
                }
                Cmd::Actual => set_zoom(v, 100, cx, cy),
                Cmd::Info => v.info = !v.info,
                _ => {}
            }
            refresh(v);
        }
    }
}

/// Pan while the primary button is held: follow the pointer since the last sample.
fn drag_poll() {
    if !crate::pal::cursor::button_down() {
        DRAG_ON.store(false, Ordering::Relaxed);
        return;
    }
    let id = WIN.load(Ordering::Relaxed);
    let (Some(pi), Some(info)) = (crate::video::panel_info_nonblocking(), wm::info(id)) else { return };
    let (x, y) = crate::pal::cursor::pos(pi.width as i32, pi.height as i32);
    let (dx, dy) = (x - DRAG_X.load(Ordering::Relaxed), y - DRAG_Y.load(Ordering::Relaxed));
    if dx == 0 && dy == 0 {
        return;
    }
    DRAG_X.store(x, Ordering::Relaxed);
    DRAG_Y.store(y, Ordering::Relaxed);
    let sc = info.scale.max(1) as i32;
    let mut g = VIEW.lock();
    if let Some(v) = g.as_mut() {
        v.ox += dx / sc;
        v.oy += dy / sc;
        refresh(v);
    }
}

// ── Input ───────────────────────────────────────────────────────────────────────────────────────

/// Keys and wheel, chained from `quarry::live::key_route`. QUEUES a command (router stack depth);
/// [`service`] applies it. Keys only while this window holds focus; the wheel only when the pointer
/// is over it. `+`/`=` `-`/`_` zoom, `0` fit, `1` 100 %, Left/`[` Right/`]` browse, Delete trash,
/// `i` info, `p`/space pause an animation.
pub fn key_route(ev: crate::pal::Event) -> bool {
    let id = WIN.load(Ordering::Relaxed);
    if id == wm::WIN_NONE {
        return false;
    }
    match ev {
        crate::pal::Event::Wheel(d) => {
            let Some(pi) = crate::video::panel_info_nonblocking() else { return false };
            let (x, y) = crate::pal::cursor::pos(pi.width as i32, pi.height as i32);
            match wm::hit_test(x, y) {
                Some((w, _, _)) if w == id && d != 0 => {
                    push_cmd(Cmd::Wheel(d));
                    true
                }
                _ => false,
            }
        }
        crate::pal::Event::Key(c) => {
            if wm::focus_asid() != OWNER {
                return false;
            }
            // FACETANIM (B358): `p` / space pause an animation (atomics only — router depth).
            if matches!(c, b'p' | b'P' | b' ') && anim::toggle_pause() {
                return true;
            }
            let cmd = match c {
                b'+' | b'=' => Cmd::ZoomIn,
                b'-' | b'_' => Cmd::ZoomOut,
                b'0' => Cmd::Fit,
                b'1' => Cmd::Actual,
                0x1D | b'[' => Cmd::Prev,
                0x1C | b']' => Cmd::Next,
                0x7F => Cmd::Delete,
                b'i' | b'I' => Cmd::Info,
                _ => return false,
            };
            push_cmd(cmd);
            true
        }
        _ => false,
    }
}

/// Pointer. Returns `true` when the press was CONSUMED: the close disc closes, a press on the
/// picture raises/focuses and arms a pan drag (applied by [`service`]). Chained from
/// `quarry::live::press_route`.
pub fn press_route(x: i32, y: i32) -> bool {
    let id = WIN.load(Ordering::Relaxed);
    if id == wm::WIN_NONE {
        return false;
    }
    match wm::hit_test(x, y) {
        Some((w, _, _)) if w == id => {}
        _ => return false,
    }
    if wm::close_box_hit(id, x, y) {
        serial_println!("[facet] press close win={} at ({},{})", id, x, y);
        close();
        return true;
    }
    let Some(info) = wm::info(id) else {
        return false;
    };
    if x < info.x as i32 || y < info.y as i32 {
        return false; // chrome — the router owns it (drag, minimise)
    }
    let scale = info.scale.max(1);
    let (sx, sy) = ((x as usize - info.x) / scale, (y as usize - info.y) / scale);
    if sx >= info.w || sy >= info.h {
        return false;
    }
    wm::focus_changed(OWNER);
    DRAG_X.store(x, Ordering::Relaxed);
    DRAG_Y.store(y, Ordering::Relaxed);
    DRAG_ON.store(true, Ordering::Relaxed);
    true
}

// ── The fixture ─────────────────────────────────────────────────────────────────────────────────

/// FACETPNG — **a synthetic PNG exercising ALL FIVE filter types decodes to the exact pixels it was
/// built from, and a corrupted IDAT is REFUSED by name.**
///
/// # Why a built fixture and not a file on the volume
///
/// A fixture that reads `SCREEN6.PNG` proves the plumbing on a machine that happens to have taken a
/// screenshot, and proves nothing anywhere else — QEMU's volume has no capture on it, so the gate
/// would be vacuous exactly where it runs. This one BUILDS its image, so it is a claim about the
/// DECODER and it holds on every board, every arch and in the headless battery:
///
///   * legs 1-5 use every PNG filter type, one per row — `prtscr` only ever writes filter 0 (see
///     `video/png.rs::push_row`), so a decoder tested against our own output would have four
///     untested branches and the card holds PNGs from elsewhere;
///   * the payload is a real zlib stream with a real Adler-32, inflated by
///     `selfhost::inflate::zlib_inflate` — the SAME entry the file path takes, not a shortcut;
///   * the verdict is a CHECKSUM over the decoded pixels, not a row count. A decoder that unfiltered
///     wrong would still produce the right number of rows.
///   * leg 6 is the NEGATIVE: one byte of the IDAT payload is flipped, and the decode must FAIL with
///     a named reason. Without it a green board could not tell a working Adler check from an absent
///     one — and "never a silent failure" is the property this module is built around.
#[cfg(feature = "witness")]
pub fn selftest() {
    use core::sync::atomic::AtomicBool;
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::AcqRel) {
        return;
    }
    match selftest_result() {
        Ok((sum, want, reason)) => serial_println!(
            ":: FACETPNG: filters=0,1,2,3,4 zlib=selfhost::inflate::zlib_inflate checksum={:#010x} expected={:#010x} corrupt-idat-refused={} :: {} ::",
            sum, want, reason,
            if sum == want { "PASS" } else { "FAIL" }
        ),
        Err(why) => serial_println!(":: FACETPNG: {} :: FAIL ::", why),
    }
}

/// The fixture's legs, as a pure function — no window, no panel, no volume, so it cannot be skipped
/// by a DECLINE in anything it is proving the arithmetic of.
#[cfg(feature = "witness")]
fn selftest_result() -> Result<(u32, u32, String), &'static str> {
    // A 5x5 truecolour-8 image whose pixel (x, y) is (16x, 16y, x^y) — no row equal to its
    // neighbour, so an unfilter that copied the wrong row cannot pass.
    const W: usize = 5;
    const H: usize = 5;
    let mut raw = [[0u8; W * 3]; H];
    for (y, row) in raw.iter_mut().enumerate() {
        for x in 0..W {
            row[x * 3] = (x * 16) as u8;
            row[x * 3 + 1] = (y * 16) as u8;
            row[x * 3 + 2] = ((x ^ y) * 16) as u8;
        }
    }
    // Filter each row with a DIFFERENT type — 0,1,2,3,4 — which is legal PNG (§9.2: the filter is
    // per scanline) and is the whole point of the fixture.
    let mut filtered: Vec<u8> = Vec::new();
    for y in 0..H {
        let f = y as u8; // 0..=4
        filtered.push(f);
        let prev = if y == 0 { [0u8; W * 3] } else { raw[y - 1] };
        for i in 0..W * 3 {
            let a = if i >= 3 { raw[y][i - 3] } else { 0 };
            let b = prev[i];
            let c = if i >= 3 { prev[i - 3] } else { 0 };
            let cur = raw[y][i];
            filtered.push(match f {
                0 => cur,
                1 => cur.wrapping_sub(a),
                2 => cur.wrapping_sub(b),
                3 => cur.wrapping_sub(((a as u32 + b as u32) / 2) as u8),
                _ => cur.wrapping_sub(paeth(a, b, c)),
            });
        }
    }
    let idat = zlib_stored(&filtered);
    let ihdr = Ihdr { width: W as u32, height: H as u32, depth: 8, colour: 2, interlaced: false };

    // Leg A — decode at 1:1 and checksum the pixels.
    let mut px = [0u32; W * H];
    let d = decode_into(ihdr, &[], &mut SliceSource::new(&idat), 1, W, H, &mut px)
        .map_err(|_| "decode of the five-filter fixture failed")?;
    if d.rows != H as u32 {
        return Err("the five-filter fixture decoded the wrong number of rows");
    }
    let sum = px.iter().fold(0u32, |a, p| a.wrapping_mul(31).wrapping_add(*p));
    let mut want = 0u32;
    for y in 0..H {
        for x in 0..W {
            let p = 0xFF00_0000
                | ((raw[y][x * 3] as u32) << 16)
                | ((raw[y][x * 3 + 1] as u32) << 8)
                | raw[y][x * 3 + 2] as u32;
            want = want.wrapping_mul(31).wrapping_add(p);
        }
    }
    if sum != want {
        return Err("the five-filter fixture decoded the wrong PIXELS (unfilter is wrong)");
    }

    // Leg B — the box filter. A 4x4 image of one colour reduces to 2x2 of that colour, which is the
    // claim `fit`'s constant divisor rests on.
    let (k, ow, oh) = fit(4, 4, 2, 2).ok_or("fit(4x4 -> 2x2) declined")?;
    if (k, ow, oh) != (2, 2, 2) {
        return Err("fit did not choose the integer factor 2 for 4x4 into 2x2");
    }
    if fit(100, 100, 1000, 1000) != Some((1, 100, 100)) {
        return Err("fit upscaled an image that already fitted");
    }

    // Leg C — the NEGATIVE. Flip one byte of the compressed payload; the decode must refuse.
    let mut bad = idat.clone();
    let last = bad.len() - 6; // inside the stored payload, clear of the 4-byte Adler trailer
    bad[last] ^= 0xFF;
    let mut px2 = [0u32; W * H];
    // The witness carries `reason()`'s OWN string — the exact token `[facet] refuse path=… reason=`
    // would print — rather than a category invented here, so this leg covers the wire format too and
    // a reader can match the fixture's text against a real refusal's.
    let reason = match decode_into(ihdr, &[], &mut SliceSource::new(&bad), 1, W, H, &mut px2) {
        Err(e) => e.reason(),
        Ok(_) => return Err("a corrupted IDAT decoded successfully — the trailer is not checked"),
    };
    if reason != FacetError::Inflate(InflateError::AdlerMismatch).reason() {
        // A flipped payload byte must fail the ADLER check specifically. Any other refusal would
        // mean the corruption was caught by luck (a bad filter byte, a short row) rather than by the
        // trailer, and this leg exists to prove the trailer is checked.
        return Err("a corrupted IDAT was refused, but not by the adler-32 trailer");
    }

    // Leg D — a file that is not a PNG is refused at the signature, and a bad IHDR by name.
    if parse_ihdr(&[0; 13]) != Err(FacetError::BadIhdr("zero dimension")) {
        return Err("parse_ihdr accepted a zero dimension");
    }
    let mut interlaced = [0u8; 13];
    interlaced[3] = 4;
    interlaced[7] = 4;
    interlaced[8] = 8;
    interlaced[9] = 2;
    interlaced[12] = 1;
    if !matches!(parse_ihdr(&interlaced), Ok(h) if h.interlaced) {
        return Err("parse_ihdr did not accept an Adam7 image");
    }
    if is_png_name("SCREEN6.TXT") || !is_png_name("SCREEN6.PNG") || !is_png_name("shot.png") {
        return Err("is_png_name does not route .PNG/.png and only .PNG/.png");
    }

    // Leg E — Adam7: a 5x5 truecolour image sent as seven passes (filter 0) decodes to the same pixels
    // as the raster order above, so the pass geometry and the scatter are checked, not just parsed.
    {
        let mut inter: Vec<u8> = Vec::new();
        for p in 0..7 {
            let (pw, ph) = adam7_dims(p, W, H);
            if pw == 0 || ph == 0 {
                continue;
            }
            let (x0, y0, dx, dy) = ADAM7[p];
            for r in 0..ph {
                inter.push(0);
                for i in 0..pw {
                    inter.extend_from_slice(&raw[y0 + r * dy][(x0 + i * dx) * 3..(x0 + i * dx) * 3 + 3]);
                }
            }
        }
        let z = zlib_stored(&inter);
        let ih = Ihdr { width: W as u32, height: H as u32, depth: 8, colour: 2, interlaced: true };
        let mut pa = [0u32; W * H];
        decode_into(ih, &[], &mut SliceSource::new(&z), 1, W, H, &mut pa)
            .map_err(|_| "decode of the Adam7 fixture failed")?;
        let s7 = pa.iter().fold(0u32, |a, p| a.wrapping_mul(31).wrapping_add(*p));
        if s7 != want {
            return Err("the Adam7 fixture decoded the wrong PIXELS");
        }
    }

    Ok((sum, want, reason))
}

/// Wrap `raw` in a zlib stream of STORED deflate blocks — `video/png.rs`'s technique, restated here
/// because that module's block writer is private to its encoder.
///
/// Stored blocks are a legal DEFLATE stream (RFC 1951 §3.2.4) and exercise the decoder's stored arm,
/// the header parse and the Adler trailer. The Huffman arms are exercised by SELFHOST-2's own
/// fixture over the same [`inflate::deflate_body`] — one decoder, two fixtures, no branch untested.
#[cfg(feature = "witness")]
fn zlib_stored(raw: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(&[0x78, 0x01]); // CMF/FLG: deflate, 32 KiB window, (0x78<<8|0x01) % 31 == 0
    let mut left = raw;
    loop {
        let n = core::cmp::min(left.len(), 65535);
        let final_block = n == left.len();
        out.push(if final_block { 1 } else { 0 });
        out.extend_from_slice(&(n as u16).to_le_bytes());
        out.extend_from_slice(&(!(n as u16)).to_le_bytes());
        out.extend_from_slice(&left[..n]);
        left = &left[n..];
        if final_block {
            break;
        }
    }
    out.extend_from_slice(&crate::video::png::adler32(raw).to_be_bytes());
    out
}

// ── WALLPAPER — the same decoder, pointed at the desktop backdrop ─────────────────────────────────
// `video/wallpaper.rs` wants a decoded, box-downscaled picture in a heap buffer instead of a window
// surface. This is `open_inner`'s decode half verbatim (stat, chunk index, `fit`, `decode_into` over
// the `IdatSource`) with the window half dropped, so the wallpaper path shares every filter and every
// refusal with the viewer instead of growing a second PNG reader. `cap` bounds the FILE (the brief's
// 4 MB); `bw x bh` bounds the decode. Returns `(pixels 0x00RRGGBB, out_w, out_h, src_w, src_h)`.
pub fn decode_file(
    path: &str,
    cap: u64,
    bw: usize,
    bh: usize,
) -> Result<(Vec<u32>, usize, usize, u32, u32), FacetError> {
    let mt = crate::shell::vfs_mount_table();
    let st = mt.stat(path).map_err(|e| FacetError::Vfs(vfs_why(e)))?;
    if matches!(st.kind, crate::fs::vfs::NodeKind::Dir) {
        return Err(FacetError::Vfs(String::from("eisdir")));
    }
    if st.size == 0 || st.size > cap.min(MAX_FILE) {
        return Err(FacetError::Size(st.size));
    }
    // FACETANIM (B358): a non-PNG picture (JPEG, GIF, WebP, ...) is its FIRST FRAME only —
    // `decode_first_frame` never composites (or keeps) frames 1..N of an animation.
    if is_foreign(&mt, path)? {
        let (ihdr, _k, out_w, out_h, px) = decode_foreign(&mt, path, st.size, bw, bh)?;
        return Ok((px, out_w, out_h, ihdr.width, ihdr.height));
    }
    let (ihdr, spans, palette) = index_chunks(&mt, path, st.size)?;
    let (k, out_w, out_h) = fit(ihdr.width, ihdr.height, bw, bh).ok_or(FacetError::NoWindow("fit"))?;
    let mut px: Vec<u32> = Vec::new();
    if px.try_reserve_exact(out_w * out_h).is_err() {
        return Err(FacetError::OutOfMemory(out_w * out_h * 4));
    }
    px.resize(out_w * out_h, 0);
    let mut src = IdatSource::new(&mt, path, &spans);
    let r = decode_into(ihdr, &palette, &mut src, k, out_w, out_h, &mut px);
    match (r, src.io_error.take()) {
        (Err(FacetError::Inflate(InflateError::TruncatedInput)), Some(io)) => Err(FacetError::Vfs(io)),
        (Err(e), _) => Err(e),
        (Ok(d), _) => Ok((px, out_w, out_h, d.ihdr.width, d.ihdr.height)),
    }
}

// ── IMGVIEW fixture ─────────────────────────────────────────────────────────────────────────────

/// One PNG chunk: length, type, data, CRC over type+data.
#[cfg(feature = "witness")]
fn push_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let mut body: Vec<u8> = Vec::new();
    body.extend_from_slice(kind);
    body.extend_from_slice(data);
    out.extend_from_slice(&body);
    out.extend_from_slice(&crate::video::png::crc32(&body).to_be_bytes());
}

/// A whole PNG from already-filtered scanlines, hand-built (the SHOTZIP encoder writes depth-8 RGB
/// only, so RGBA and palette files cannot come from it).
#[cfg(feature = "witness")]
fn build_png(w: u32, h: u32, depth: u8, colour: u8, plte: &[u8], filtered: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(&SIGNATURE);
    let mut ih = [0u8; 13];
    ih[..4].copy_from_slice(&w.to_be_bytes());
    ih[4..8].copy_from_slice(&h.to_be_bytes());
    ih[8] = depth;
    ih[9] = colour;
    push_chunk(&mut out, b"IHDR", &ih);
    if !plte.is_empty() {
        push_chunk(&mut out, b"PLTE", plte);
    }
    push_chunk(&mut out, b"IDAT", &zlib_stored(filtered));
    push_chunk(&mut out, b"IEND", &[]);
    out
}

/// IMGVIEW — write a 64x64 RGBA and a 16x16 palette PNG under `/home/<user>/`, open the first,
/// zoom, browse to a neighbour, open the palette image, browse back, close, unlink both.
#[cfg(feature = "witness")]
pub fn imgview_selftest() {
    use crate::fs::vfs::{NodeKind, KERNEL_PRINCIPAL as P};
    let fail = |why: &str| serial_println!(":: IMGVIEW: {} :: FAIL ::", why);
    if crate::video::panel_info_nonblocking().is_none() {
        serial_println!(":: IMGVIEW: no panel :: SKIP ::");
        return;
    }
    let home = crate::fs::trash::home_base();
    let (pa, pb) = (alloc::format!("{}/IVTESTA.PNG", home), alloc::format!("{}/IVTESTB.PNG", home));
    // 64x64 RGBA, filter 0.
    let mut a: Vec<u8> = Vec::new();
    for y in 0..64usize {
        a.push(0);
        for x in 0..64usize {
            a.extend_from_slice(&[(x * 4) as u8, (y * 4) as u8, ((x ^ y) * 4) as u8, 255]);
        }
    }
    // 16x16 palette, depth 4, 16 entries, index (x + y) & 15.
    let mut plte: Vec<u8> = Vec::new();
    for i in 0..16u8 {
        plte.extend_from_slice(&[i * 16, 255 - i * 16, i * 8]);
    }
    let mut b: Vec<u8> = Vec::new();
    for y in 0..16usize {
        b.push(0);
        for x in (0..16usize).step_by(2) {
            b.push((((x + y) & 15) as u8) << 4 | (((x + 1 + y) & 15) as u8));
        }
    }
    let files = [(&pa, build_png(64, 64, 8, 6, &[], &a)), (&pb, build_png(16, 16, 4, 3, &plte, &b))];
    let mt = crate::shell::vfs_mount_table();
    for (p, bytes) in files.iter() {
        let _ = mt.unlink(p, P);
        if mt.create(p, NodeKind::File, P).is_err() || mt.write(p, 0, bytes, P).is_err() {
            fail("could not write the fixture PNGs");
            let _ = mt.unlink(&pa, P);
            let _ = mt.unlink(&pb, P);
            return;
        }
    }
    let view = |f: &dyn Fn(&View) -> bool| VIEW.lock().as_ref().map(|v| f(v)).unwrap_or(false);
    open_path(&pa);
    let opened = is_open() && shown() == pa && view(&|v| v.src_w == 64 && v.ihdr.colour == 6);
    apply(Cmd::ZoomIn);
    let zoomed = view(&|v| v.zoom == 150);
    let zoom = VIEW.lock().as_ref().map(|v| v.zoom).unwrap_or(0);
    let fitted = {
        apply(Cmd::Fit);
        view(&|v| v.zoom == v.fit_zoom)
    };
    apply(Cmd::ZoomIn);
    let (_, list) = siblings(&pa);
    let n = list.len();
    apply(Cmd::Next);
    let browsed = n >= 2 && shown() != pa && is_open();
    open_path(&pb);
    let pal_ok = shown() == pb && view(&|v| v.src_w == 16 && v.ihdr.colour == 3 && v.px[0] == 0xFF00_FF00);
    apply(Cmd::Prev);
    let back = shown() != pb && is_open();
    // A refused file shows a message window, not nothing.
    let junk = alloc::format!("{}/IVTESTC.PNG", home);
    let _ = mt.unlink(&junk, P);
    let junk_ok = mt.create(&junk, NodeKind::File, P).is_ok() && mt.write(&junk, 0, b"not a png at all", P).is_ok() && {
        open_path(&junk);
        is_open() && shown() == junk && VIEW.lock().is_none()
    };
    close();
    let closed = !is_open();
    let _ = mt.unlink(&junk, P);
    let _ = mt.unlink(&pa, P);
    let _ = mt.unlink(&pb, P);
    let ok = opened && zoomed && fitted && browsed && pal_ok && back && junk_ok && closed;
    serial_println!(
        ":: IMGVIEW: path={} WxH=64x64 zoom={} fit={} browse_n={} colour=rgba pal={} refusal-window={} -> {} ::",
        pa, zoom, fitted as u8, n, pal_ok as u8, junk_ok as u8, if ok { "PASS" } else { "FAIL" }
    );
}

// QUARRY2 (B336): animated frames behind a local decoder trait (the PIXELCORE fold is one adapter) —
// a child module, so no `video/mod.rs` line.
#[path = "facet_anim.rs"]
pub mod anim;

/// APPRES (B398): the live Facet window id (the dock draws its icon on that tile), or [`wm::WIN_NONE`].
pub fn win() -> wm::WinId {
    WIN.load(Ordering::Relaxed)
}
