// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Facet — owed
//!
//! QUARRY2 M4 (rmbp-ledger B336) — Facet steps ANIMATED frames when the decoded image carries them.
//!
//! The decoder that produces frames is `pixel_core`, a sibling arc NOT in this tree (PIXELCORE, SR25:
//! the shared `no_std` image core both rings link — that is the seam this file's CHARTER says is owed).
//! So the viewer codes against a LOCAL shape, [`Decoded`] `{ w, h, frames: Option<Vec<(u16 delay_ms,
//! Vec<u32> argb)>> }`, behind a LOCAL trait, [`FrameDecoder`]; the fold is ONE adapter
//! (`impl FrameDecoder for <pixel_core's decoder>`) and one line in [`decoder`].
//!
//! Today's only adapter is [`PngStill`] — Facet's own PNG path, which answers `frames: None`, so
//! [`probe`] returns `None` and Facet's streaming PNG decoder runs exactly as before (behaviour
//! unchanged). A `.gif` reaches Facet (FILETYPE types it `image/gif`, the association opens it here)
//! and is refused by name (`reason=not-png`) until the fold.
//!
//! When frames ARE present: frame 0 opens through the same window body (fit, zoom, browse), every
//! frame is resampled once to the base size, and [`tick`] — on Facet's service pass — swaps the base
//! image when [`frame_at`] says the clock has moved on (GIF delays under 20 ms read as 100 ms, the
//! rule every browser applies; the animation loops).
//!
//! Wire: `[facet] anim path= frames= w= h= decoder=` on open, `[facet] anim step=<n>` every 64 steps.

use alloc::string::String;
use alloc::vec::Vec;

use super::{FacetError, Ihdr};

/// One decoded image, the PIXELCORE shape. `frames: None` = a still (the caller keeps its own path).
pub struct Decoded {
    pub w: u32,
    pub h: u32,
    /// `(delay in ms, w*h pixels 0xAARRGGBB)` per frame.
    pub frames: Option<Vec<(u16, Vec<u32>)>>,
}

/// A frame-capable decoder. `head` = the file's first bytes; `read_all` reads the whole file and is
/// called only by a decoder that wants it (a still costs no read).
pub trait FrameDecoder: Sync {
    fn name(&self) -> &'static str;
    fn decode(&self, head: &[u8], read_all: &mut dyn FnMut() -> Option<Vec<u8>>) -> Option<Decoded>;
}

/// Facet's own PNG path as an adapter: a still, always (APNG is not decoded).
pub struct PngStill;

impl FrameDecoder for PngStill {
    fn name(&self) -> &'static str {
        "png-still"
    }
    fn decode(&self, head: &[u8], _read_all: &mut dyn FnMut() -> Option<Vec<u8>>) -> Option<Decoded> {
        if head.len() < 24 || head[..8] != super::SIGNATURE {
            return None;
        }
        let be = |o: usize| u32::from_be_bytes([head[o], head[o + 1], head[o + 2], head[o + 3]]);
        Some(Decoded { w: be(16), h: be(20), frames: None })
    }
}

static PNG_STILL: PngStill = PngStill;

/// THE decoder. PIXELCORE fold: return the pixel_core adapter here (it answers PNG stills as `None`
/// frames and GIFs with frames).
pub fn decoder() -> &'static dyn FrameDecoder {
    &PNG_STILL
}

/// Frames `path` carries, through [`decoder`]; `None` = a still or not an image this decoder takes
/// (Facet's own path then runs unchanged).
pub fn probe(path: &str) -> Option<Decoded> {
    let mt = crate::shell::vfs_mount_table();
    let head = mt.read(path, 0, 64).ok()?;
    let mut read_all = || -> Option<Vec<u8>> {
        let st = mt.stat(path).ok()?;
        if st.size > super::MAX_FILE {
            return None;
        }
        let mut out = Vec::new();
        while (out.len() as u64) < st.size {
            let c = mt.read(path, out.len() as u64, 256 * 1024).ok()?;
            if c.is_empty() {
                break;
            }
            out.extend_from_slice(&c);
        }
        Some(out)
    };
    let d = decoder().decode(&head, &mut read_all)?;
    match &d.frames {
        Some(f) if !f.is_empty() && f.iter().all(|(_, px)| px.len() == d.w as usize * d.h as usize) => Some(d),
        _ => None,
    }
}

/// The frame showing `elapsed_ms` after frame 0 went up (looping). Pure.
pub fn frame_at(delays: &[u16], elapsed_ms: u64) -> usize {
    let d = |x: u16| if x < 20 { 100u64 } else { x as u64 };
    let cycle: u64 = delays.iter().map(|&x| d(x)).sum();
    if delays.is_empty() || cycle == 0 {
        return 0;
    }
    let mut t = elapsed_ms % cycle;
    for (i, &x) in delays.iter().enumerate() {
        if t < d(x) {
            return i;
        }
        t -= d(x);
    }
    0
}

/// Nearest-neighbour resample `src` (`sw x sh`) to `dw x dh`. Pure.
pub fn resample(src: &[u32], sw: usize, sh: usize, dw: usize, dh: usize) -> Vec<u32> {
    if sw == dw && sh == dh {
        return Vec::from(src);
    }
    let mut out = Vec::with_capacity(dw * dh);
    for y in 0..dh {
        let sy = (y * sh / dh.max(1)).min(sh.saturating_sub(1));
        for x in 0..dw {
            let sx = (x * sw / dw.max(1)).min(sw.saturating_sub(1));
            out.push(src.get(sy * sw + sx).copied().unwrap_or(0xFF00_0000) | 0xFF00_0000);
        }
    }
    out
}

struct Anim {
    path: String,
    frames: Vec<(u16, Vec<u32>)>,
    shown: usize,
    t0: u64,
    steps: u64,
}

static ANIM: spin::Mutex<Option<Anim>> = spin::Mutex::new(None);

/// Open `path`'s frames: frame 0 through Facet's window body, the rest stepped by [`tick`].
pub(super) fn open_frames(path: &str, bytes: u64, d: Decoded) -> Result<super::Opened, FacetError> {
    let frames = d.frames.unwrap_or_default();
    let (sw, sh) = (d.w as usize, d.h as usize);
    let (k, bw, bh) = super::fit(d.w, d.h, super::BASE_W, super::BASE_H).ok_or(FacetError::NoWindow("fit"))?;
    let base: Vec<(u16, Vec<u32>)> = frames.iter().map(|(t, px)| (*t, resample(px, sw, sh, bw, bh))).collect();
    let first = base.first().map(|f| f.1.clone()).ok_or(FacetError::NoWindow("no-frames"))?;
    let ihdr = Ihdr { width: d.w, height: d.h, depth: 8, colour: 6, interlaced: false };
    serial_println!("[facet] anim path={} frames={} w={} h={} decoder={} k={}", path, base.len(), d.w, d.h, decoder().name(), k);
    let o = super::open_base(path, bytes, ihdr, k, bw, bh, first)?;
    *ANIM.lock() = Some(Anim { path: String::from(path), frames: base, shown: 0, t0: crate::arch::ms(), steps: 0 });
    Ok(o)
}

/// Facet's service pass: step the frame when the clock says so. A closed or replaced view ends the
/// animation. Quiet pass = one uncontended lock.
pub fn tick() {
    let mut g = ANIM.lock();
    let Some(a) = g.as_mut() else { return };
    if !super::is_open() || super::shown() != a.path {
        *g = None;
        return;
    }
    let delays: Vec<u16> = a.frames.iter().map(|f| f.0).collect();
    let want = frame_at(&delays, crate::arch::ms().saturating_sub(a.t0));
    if want == a.shown {
        return;
    }
    let mut v = super::VIEW.lock();
    let Some(view) = v.as_mut() else { return };
    if view.px.len() != a.frames[want].1.len() {
        return;
    }
    view.px.copy_from_slice(&a.frames[want].1);
    a.shown = want;
    a.steps += 1;
    if a.steps % 64 == 1 {
        serial_println!("[facet] anim step={} frame={}/{} path={}", a.steps, want, a.frames.len(), a.path);
    }
    super::refresh(view);
}

/// A two-frame 1x1 GIF89a (graphic-control extensions with 100 ms and 50 ms delays) — the bytes the
/// fold's adapter must decode to two frames.
const TWO_FRAME_GIF: &[u8] = &[
    b'G', b'I', b'F', b'8', b'9', b'a', 1, 0, 1, 0, 0x80, 0, 0, // screen 1x1, 2-colour table
    0xFF, 0xFF, 0xFF, 0, 0, 0, // palette
    0x21, 0xF9, 4, 0, 10, 0, 0, 0, // GCE delay 10 cs
    0x2C, 0, 0, 0, 0, 1, 0, 1, 0, 0, 2, 2, 0x44, 1, 0, // frame 0
    0x21, 0xF9, 4, 0, 5, 0, 0, 0, // GCE delay 5 cs
    0x2C, 0, 0, 0, 0, 1, 0, 1, 0, 0, 2, 2, 0x44, 1, 0, // frame 1
    0x3B,
];

/// `tests quarry2`'s frame leg: `(frames the adapter decoded from a real two-frame GIF — None while
/// the adapter is the PNG still —, the stepper's verdict)`. `Err` names a failed claim.
pub fn selftest_leg() -> Result<(Option<usize>, &'static str), String> {
    // The stepper over a synthetic three-frame decode (delays 100, 50, 0 -> 100).
    let d = [100u16, 50, 0];
    for (t, want) in [(0u64, 0usize), (99, 0), (100, 1), (149, 1), (150, 2), (249, 2), (250, 0), (350, 1)] {
        let got = frame_at(&d, t);
        if got != want {
            return Err(alloc::format!("frame_at({}) = {} want {}", t, got, want));
        }
    }
    let r = resample(&[1, 2, 3, 4], 2, 2, 4, 4);
    if r.len() != 16 || r[0] & 0xFF != 1 || r[3] & 0xFF != 2 || r[15] & 0xFF != 4 {
        return Err(String::from("resample 2x2 -> 4x4"));
    }
    // The still path stays a still: Facet's PNG adapter answers no frames.
    let png_head: [u8; 24] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13, b'I', b'H', b'D', b'R', 0, 0, 0, 2, 0, 0, 0, 2];
    let mut none = || -> Option<Vec<u8>> { None };
    match PNG_STILL.decode(&png_head, &mut none) {
        Some(Decoded { w: 2, h: 2, frames: None }) => {}
        _ => return Err(String::from("the PNG adapter did not answer a 2x2 still")),
    }
    let mut all = || -> Option<Vec<u8>> { Some(Vec::from(TWO_FRAME_GIF)) };
    let frames = decoder().decode(&TWO_FRAME_GIF[..TWO_FRAME_GIF.len().min(64)], &mut all).and_then(|d| d.frames).map(|f| f.len());
    if let Some(n) = frames {
        if n != 2 {
            return Err(alloc::format!("the adapter decoded {} frames from a two-frame GIF", n));
        }
    }
    Ok((frames, "ok"))
}
