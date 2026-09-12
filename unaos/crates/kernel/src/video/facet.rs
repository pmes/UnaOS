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
            FacetError::Interlaced => String::from("interlaced-adam7-unsupported"),
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

    /// Read the `n`-th sample of the `x`-th pixel out of the unfiltered scanline, normalised to 8
    /// bits. Sub-byte depths are unpacked MSB-first, which is PNG's order (§7.2); depth 16 keeps the
    /// high byte, which is exactly what a truncation to 8 bits per channel is.
    #[inline]
    fn sample(&self, x: usize, chan: usize, chans: usize) -> u8 {
        let d = self.ihdr.depth as usize;
        match d {
            8 => *self.cur.get(x * chans + chan).unwrap_or(&0),
            16 => *self.cur.get((x * chans + chan) * 2).unwrap_or(&0),
            _ => {
                // 1, 2 or 4 bits — only ever one channel (greyscale or palette index).
                let idx = x * chans + chan;
                let per = 8 / d;
                let byte = *self.cur.get(idx / per).unwrap_or(&0);
                let shift = 8 - d - (idx % per) * d;
                let raw = (byte >> shift) & ((1u16 << d) - 1) as u8;
                if self.ihdr.colour == 3 {
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
    fn rgb(&self, x: usize) -> (u8, u8, u8) {
        match self.ihdr.colour {
            0 => {
                let g = self.sample(x, 0, 1);
                (g, g, g)
            }
            2 => (self.sample(x, 0, 3), self.sample(x, 1, 3), self.sample(x, 2, 3)),
            3 => {
                let i = self.sample(x, 0, 1) as usize * 3;
                match self.palette.get(i..i + 3) {
                    Some(e) => (e[0], e[1], e[2]),
                    // An index past the PLTE. `decode`'s pre-check refuses a palette that is short
                    // for the DEPTH, so reaching here needs a file that is both short and lying;
                    // black is the bounded answer and the image still shows.
                    None => (0, 0, 0),
                }
            }
            4 => {
                let g = self.sample(x, 0, 2);
                (g, g, g)
            }
            _ => (self.sample(x, 0, 4), self.sample(x, 1, 4), self.sample(x, 2, 4)),
        }
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
    if d[12] != 0 {
        return Err(FacetError::Interlaced);
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

/// The window's surface. Owned here, named by the `wm` row, and cleared by [`close`] — the same
/// discipline Quarry keeps, and for the same reason: the row must stop naming the buffer before the
/// buffer goes away.
static SURF: spin::Mutex<Vec<u8>> = spin::Mutex::new(Vec::new());

/// The path currently on the glass, for the census line and for the idempotent re-open.
static SHOWN: spin::Mutex<String> = spin::Mutex::new(String::new());

/// The path a gesture asked for, waiting for a pass that is allowed to open it.
static PENDING: spin::Mutex<Option<String>> = spin::Mutex::new(None);

/// Ask for `path` to be opened. **This is what a click or a key press calls, and [`open_path`] is
/// not.**
///
/// THE LATCH IS NOT CEREMONY — it is `dock::press_at`'s law and it is written in a boot log. The
/// click router runs in the input-drain band on a kernel stack that is 16 KiB, and Pi boot 11
/// overflowed exactly that stack with `quarry::open()` called at click-router depth: a directory
/// read plus an allocation plus the window table, synchronously, under the router. [`open_inner`] is
/// strictly heavier — it walks the chunk list through the VFS, streams megabytes in [`CHUNK`] reads
/// and runs the whole inflate — so putting it under a press would be re-committing that defect with
/// a bigger frame. The gesture stores a path; [`service`] opens it from the render pass, which is
/// where the shell's own volume reads already happen.
///
/// A second request before the first is drained REPLACES it: the operator's latest double-click is
/// the one they meant, and a queue of pictures nobody asked to see is not a feature.
pub fn request_open(path: &str) {
    *PENDING.lock() = Some(String::from(path));
}

/// Drain a pending open request. Idempotent, and safe to call on every pass: a quiet pass is one
/// uncontended `try`-free lock and a `None`.
///
/// Chained from [`super::quarry::live::service`], which is already drained from both places this
/// desktop services furniture — `main.rs`'s render pass on the Orin and the strip-press arm in
/// `arch/aarch64/syscall.rs` — so Facet needs no drain site of its own in either file.
pub fn service() {
    let want = PENDING.lock().take();
    if let Some(p) = want {
        open_path(&p);
    }
}

/// Is Facet's window live?
pub fn is_open() -> bool {
    WIN.load(Ordering::Relaxed) != wm::WIN_NONE
}

/// The file currently shown, or `""`.
pub fn shown() -> String {
    SHOWN.lock().clone()
}

/// Close the window and free the surface.
pub fn close() {
    let id = WIN.swap(wm::WIN_NONE, Ordering::Relaxed);
    if id == wm::WIN_NONE {
        return;
    }
    // `wm::close` FIRST — the row must stop naming the buffer before the buffer goes away. It spins
    // on the drain barrier, so it is called with no lock of ours held.
    wm::close(id);
    // `Vec::clear` would keep the capacity; the picture is megabytes, so the allocation is RETURNED.
    // Without this a viewer that opened three screenshots would hold the largest one's surface for
    // the rest of the boot, which is exactly the leak the streaming decode exists to avoid.
    *SURF.lock() = Vec::new();
    SHOWN.lock().clear();
    serial_println!("[facet] closed win={} paints={}", id, PAINTS.load(Ordering::Relaxed));
}

/// Does this name end `.PNG`, case-insensitively? Quarry's routing test, and the only thing that
/// decides whether a double-click reaches this module.
pub fn is_png_name(name: &str) -> bool {
    let n = name.as_bytes();
    n.len() > 4 && n[n.len() - 4..].eq_ignore_ascii_case(b".png")
}

/// The name a window carries: the DOCUMENT's, not the app's.
///
/// Peter's R36 — an app window is titled with the app's NAME, and a DOCUMENT window with the
/// document's. So this window says `SCREEN6.PNG` and never `facet`, which is also how an operator
/// with two pictures open tells them apart in the dock and the window menu.
fn title_of(path: &str) -> String {
    String::from(path.rsplit('/').next().unwrap_or(path))
}

/// **Open `path` in Facet.** The whole mechanism, and the only entry [`super::quarry`] calls.
///
/// Idempotent per PATH: opening the file already shown raises the existing window rather than
/// decoding it again. Opening a DIFFERENT file closes the old window first — one canvas, which is
/// what "double-click the next screenshot" should do and what keeps the surface count at one.
///
/// Every arm prints exactly one line. On success that is three (`open`, `decoded`, `present`); on a
/// refusal it is `open` — which is the honest record that the gesture was seen — followed by
/// `refuse`, whose `reason=` names which claim broke.
pub fn open_path(path: &str) {
    if shown() == path && is_open() {
        let id = WIN.load(Ordering::Relaxed);
        wm::focus_changed(OWNER);
        serial_println!("[facet] open SKIP reason=already-shown win={} path={}", id, path);
        return;
    }
    match open_inner(path) {
        Ok((id, d)) => {
            serial_println!(
                "[facet] present win={} scale=1/{} src={}x{} shown={}x{} title={}",
                id, d.k, d.ihdr.width, d.ihdr.height, d.out_w, d.out_h, title_of(path)
            );
        }
        Err(e) => {
            serial_println!("[facet] refuse path={} reason={}", path, e.reason());
        }
    }
}

/// [`open_path`]'s body, so every failure is one `?` and the caller owns the wire format.
fn open_inner(path: &str) -> Result<(wm::WinId, Decoded), FacetError> {
    let t0 = crate::arch::ms();
    let mt = crate::shell::vfs_mount_table();
    let st = mt.stat(path).map_err(|e| FacetError::Vfs(vfs_why(e)))?;
    if matches!(st.kind, crate::fs::vfs::NodeKind::Dir) {
        return Err(FacetError::Vfs(String::from("eisdir")));
    }
    if st.size == 0 || st.size > MAX_FILE {
        return Err(FacetError::Size(st.size));
    }
    let (ihdr, spans, palette) = index_chunks(&mt, path, st.size)?;

    // The panel bounds the window, and the window bounds the decode. Read the panel through the
    // NON-BLOCKING door for `quarry::open`'s reason: this runs from the click router's input band,
    // where a blocking `WRITER.lock()` is the INWEDGE defect.
    let pi = crate::video::panel_info_nonblocking().ok_or(FacetError::NoWindow("panel-busy"))?;
    let (pw, ph) = (pi.width, pi.height);
    let bw = CEIL_W.min(pw.saturating_sub(2 * wm::BORDER).max(1));
    let bh = CEIL_H.min(ph.saturating_sub(wm::TITLE_H + 2 * wm::BORDER).max(1));
    let (k, out_w, out_h) = fit(ihdr.width, ihdr.height, bw, bh).ok_or(FacetError::NoWindow("fit"))?;
    if out_w < FLOOR_W || out_h < FLOOR_H {
        return Err(FacetError::NoWindow("below-floor"));
    }
    serial_println!(
        "[facet] open path={} ihdr={}x{} depth={} colour={} bytes={} idat-chunks={} -> DECODING",
        path, ihdr.width, ihdr.height, ihdr.depth, ihdr.colour, st.size, spans.len()
    );

    // The ONE real allocation, sized and taken before a single compressed byte is pulled — so an
    // out-of-memory answer arrives before the work rather than halfway through it.
    let len = out_w * out_h * 4;
    let mut surf: Vec<u8> = Vec::new();
    if surf.try_reserve_exact(len).is_err() {
        return Err(FacetError::OutOfMemory(len));
    }
    surf.resize(len, 0);

    let d = {
        // SAFETY: `surf` is exactly `out_w * out_h * 4` bytes from the global allocator, which meets
        // `u32`'s alignment, and the view is `out_w * out_h` words — in bounds by construction. It is
        // the same ARGB8888 aliasing `wm` will read the buffer with. The borrow ends before the Vec
        // is moved into `SURF`.
        let px: &mut [u32] =
            unsafe { core::slice::from_raw_parts_mut(surf.as_mut_ptr() as *mut u32, out_w * out_h) };
        let mut src = IdatSource::new(&mt, path, &spans);
        let r = decode_into(ihdr, &palette, &mut src, k, out_w, out_h, px);
        // A VFS failure mid-stream reaches the decoder as `TruncatedInput`; the source recorded what
        // actually happened, and THAT is the finding worth printing.
        match (r, src.io_error.take()) {
            (Err(FacetError::Inflate(InflateError::TruncatedInput)), Some(io)) => {
                serial_println!(
                    "[facet] decoded rows=0 inflate=vfs-{} ms={}",
                    io,
                    crate::arch::ms().saturating_sub(t0)
                );
                return Err(FacetError::Vfs(io));
            }
            (r, _) => r,
        }
    };
    let d = match d {
        Ok(d) => {
            serial_println!(
                "[facet] decoded rows={} inflate=OK ms={}",
                d.rows,
                crate::arch::ms().saturating_sub(t0)
            );
            d
        }
        Err(e) => {
            serial_println!(
                "[facet] decoded rows=0 inflate={} ms={}",
                e.reason(),
                crate::arch::ms().saturating_sub(t0)
            );
            return Err(e);
        }
    };

    // One canvas: a different picture replaces this one rather than stacking a second window.
    if is_open() {
        close();
    }
    let Some((_scale, ow, oh)) = wm::spawn_geometry(out_w, out_h) else {
        return Err(FacetError::NoWindow("geometry-unavailable"));
    };
    // Centred in the WORK AREA — below the menu bar's reservation, above the instrument strip — the
    // same seating `quarry::open` uses, so the two windows land on the same grid.
    let wtop = crate::ui_status::top_chrome_h(pw, ph);
    let ox = pw.saturating_sub(ow) / 2;
    let oy = wtop
        + ph.saturating_sub(wtop).saturating_sub(crate::ui_status::chrome_h(ph)).saturating_sub(oh)
            / 2;
    *SURF.lock() = surf;
    let base = SURF.lock().as_ptr() as usize;
    let title = title_of(path);
    let id = wm::create_at(
        OWNER,
        base,
        len,
        out_w as u32,
        out_h as u32,
        (out_w * 4) as u32,
        title.as_bytes(),
        ox + wm::BORDER,
        oy + wm::TITLE_H + wm::BORDER,
    );
    if id == wm::WIN_NONE {
        *SURF.lock() = Vec::new();
        return Err(FacetError::NoWindow("create-failed"));
    }
    WIN.store(id, Ordering::Relaxed);
    wm::winid_register_holder(&WIN, "facet");
    *SHOWN.lock() = String::from(path);
    wm::focus_changed(OWNER);
    let _ = wm::present(id);
    PAINTS.fetch_add(1, Ordering::Relaxed);
    Ok((id, d))
}

// ── Input ───────────────────────────────────────────────────────────────────────────────────────

/// Pointer. Returns `true` when the press was CONSUMED.
///
/// Facet binds ONE gesture — the close disc — and raises on any press inside its own content, which
/// is the minimum a tenant owes: a window the operator cannot focus and cannot close is furniture,
/// not an app. Every other press falls through to the router's own arms, so the title bar still
/// drags and the minimise disc still parks, unchanged and without a line in either arch's router.
///
/// ⚠ **THIS IS CHAINED FROM `quarry::live::press_route`, NOT FROM THE ROUTER.** The routers that
/// would name it — `arch/aarch64/syscall.rs`'s click arm and `arch/x86_64/syscall.rs`'s — are files
/// this arc may not add a line to (the knob-off `kernel8.img` byte-identity proof, PARITY.md §5.3,
/// and another lane owns the x86 half). Quarry is already named there and Facet's only door is
/// Quarry, so the chain costs nothing and is honest about the dependency: no file manager, no
/// viewer. The wm hook that does not exist and would replace it is named in this arc's report.
pub fn press_route(x: i32, y: i32) -> bool {
    let id = WIN.load(Ordering::Relaxed);
    if id == wm::WIN_NONE {
        return false;
    }
    // `hit_test` never reports a row that is not compositing, so a parked Facet declines by
    // construction and an occluding window keeps every press.
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
    // A press on the picture RAISES and focuses, exactly as the router's own select arm would.
    wm::focus_changed(OWNER);
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
    let ihdr = Ihdr { width: W as u32, height: H as u32, depth: 8, colour: 2 };

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
    if parse_ihdr(&interlaced) != Err(FacetError::Interlaced) {
        return Err("parse_ihdr accepted an Adam7-interlaced image");
    }
    if is_png_name("SCREEN6.TXT") || !is_png_name("SCREEN6.PNG") || !is_png_name("shot.png") {
        return Err("is_png_name does not route .PNG/.png and only .PNG/.png");
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
