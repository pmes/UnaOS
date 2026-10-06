// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Facet — shared-core
//!
//! Facet steps ANIMATED frames — QUARRY2 M4 (rmbp-ledger B336) built the stepper and the tick;
//! FACETANIM (B358) gave it a decoder and took the N-frame cost away.
//!
//! The decoder is `pixel_core` (PIXELCORE SR25 + ANIMWEBP SR44: the shared `no_std` image core both
//! rings link — the seam). [`decoder`] returns [`PixelCore`], the one adapter behind the local
//! [`FrameDecoder`] trait: it takes GIF, animated WebP (VP8X animation flag) and APNG (`acTL` before
//! `IDAT`) and opens a [`pixel_core::Animation`] over the file's bytes. That stream composites ONE
//! frame at a time onto ONE canvas (plus one more canvas-sized buffer only while a dispose-to-previous
//! frame is up), so the viewer holds two frames — the decoder's canvas and the view's base image —
//! never N. Every other file (PNG stills, JPEG, BMP, QOI, still WebP) is not taken and Facet's own
//! paths run unchanged.
//!
//! The viewer: frame 0 opens through Facet's window body (fit, zoom, browse); [`tick`] — on Facet's
//! service pass — composites the next frame into the base image when the current one's delay has run
//! out (delays under 20 ms read as 100 ms, the rule browsers apply), loops per the file's loop count
//! (forever / n extra plays / once, then it rests on the last frame), and `p` or space pauses
//! ([`toggle_pause`]). The title carries `frame i/n` ([`title_suffix`]).
//!
//! Wire: `[facet] anim path= frames= loop= w= h= decoder= k=` on open, `[facet] anim step=<n>
//! frame=<i>/<n>` every 64 steps, `[facet] anim end plays=<n>`, `[facet] anim pause=<0|1>`.
//! `tests facetanim`: `:: FACETANIM: gif=3 webp=3 apng=3 frames_held=2 -> PASS ::`.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::{FacetError, Ihdr};

/// The stream an adapter opens: pixel_core's animation over the file's own bytes.
pub type Stream = pixel_core::Animation<Vec<u8>>;

/// A frame-capable decoder. `takes` looks at the file's first bytes only (a file it does not take
/// costs no whole read); `open` gets the whole file.
pub trait FrameDecoder: Sync {
    fn name(&self) -> &'static str;
    fn takes(&self, head: &[u8]) -> bool;
    fn open(&self, bytes: Vec<u8>) -> Result<Stream, FacetError>;
}

/// The pixel_core adapter: GIF, animated WebP, APNG.
pub struct PixelCore;

impl FrameDecoder for PixelCore {
    fn name(&self) -> &'static str {
        "pixel_core"
    }
    fn takes(&self, head: &[u8]) -> bool {
        match pixel_core::sniff(head) {
            Some(pixel_core::Format::Gif) => true,
            // VP8X first (offset 12) with the Animation flag (bit 1 of its flags byte, offset 20).
            Some(pixel_core::Format::WebP) => head.len() > 20 && &head[12..16] == b"VP8X" && head[20] & 0x02 != 0,
            // acTL must come before the first IDAT (APNG §4); look through the head for it.
            Some(pixel_core::Format::Png) => {
                let mut p = 8usize;
                while p + 8 <= head.len() {
                    let len = u32::from_be_bytes([head[p], head[p + 1], head[p + 2], head[p + 3]]) as usize;
                    match &head[p + 4..p + 8] {
                        b"acTL" => return true,
                        b"IDAT" => return false,
                        _ => {}
                    }
                    p = match p.checked_add(12 + len) {
                        Some(n) => n,
                        None => return false,
                    };
                }
                false
            }
            _ => false,
        }
    }
    fn open(&self, bytes: Vec<u8>) -> Result<Stream, FacetError> {
        pixel_core::Animation::new(bytes).map_err(FacetError::Pixel)
    }
}

static PIXEL_CORE: PixelCore = PixelCore;

/// THE decoder.
pub fn decoder() -> &'static dyn FrameDecoder {
    &PIXEL_CORE
}

/// How much of the head [`FrameDecoder::takes`] sees: enough for an APNG's `acTL` after a few
/// ancillary chunks.
const HEAD: usize = 4096;

/// Read `path` whole (bounded by Facet's non-PNG cap) when the decoder takes its head.
fn read_taken(path: &str) -> Option<Vec<u8>> {
    let mt = crate::shell::vfs_mount_table();
    let head = mt.read(path, 0, HEAD).ok()?;
    if !decoder().takes(&head) {
        return None;
    }
    let st = mt.stat(path).ok()?;
    if st.size == 0 || st.size > super::MAX_FOREIGN {
        return None;
    }
    let mut out = Vec::new();
    out.try_reserve_exact(st.size as usize).ok()?;
    while (out.len() as u64) < st.size {
        let c = mt.read(path, out.len() as u64, 256 * 1024).ok()?;
        if c.is_empty() {
            break;
        }
        out.extend_from_slice(&c);
    }
    Some(out)
}

/// The stream for `path`, frame 0 composited, through [`decoder`]; `None` = not a file the decoder
/// takes, or one it could not open (Facet's own path then runs and names the reason).
pub fn probe(path: &str) -> Option<Stream> {
    let bytes = read_taken(path)?;
    match decoder().open(bytes) {
        Ok(s) => Some(s),
        Err(e) => {
            serial_println!("[facet] anim probe path={} refused={} (falling back)", path, e.reason());
            None
        }
    }
}

/// A delay as played: under 20 ms reads as 100 ms (the rule every browser applies).
fn played(ms: u32) -> u64 {
    if ms < 20 { 100 } else { ms as u64 }
}

/// The frame showing `elapsed_ms` after frame 0 went up (looping). Pure. (QUARRY2's stepper over a
/// known delay list; the viewer itself steps on each frame's delay as it is composited.)
pub fn frame_at(delays: &[u16], elapsed_ms: u64) -> usize {
    let cycle: u64 = delays.iter().map(|&x| played(x as u32)).sum();
    if delays.is_empty() || cycle == 0 {
        return 0;
    }
    let mut t = elapsed_ms % cycle;
    for (i, &x) in delays.iter().enumerate() {
        if t < played(x as u32) {
            return i;
        }
        t -= played(x as u32);
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

/// Box-reduce straight RGBA (`iw` wide) by `k` into `0xFFRRGGBB` base pixels (`ow x oh`), alpha
/// dropped — the reduction Facet's non-PNG open uses, so an animated frame looks like a still one.
pub fn reduce_into(rgba: &[u8], iw: usize, k: usize, ow: usize, oh: usize, out: &mut [u32]) {
    let n = (k * k) as u32;
    for oy in 0..oh {
        for ox in 0..ow {
            let (mut r, mut g, mut b) = (0u32, 0u32, 0u32);
            for sy in oy * k..oy * k + k {
                for sx in ox * k..ox * k + k {
                    let i = (sy * iw + sx) * 4;
                    if let Some(p) = rgba.get(i..i + 3) {
                        r += p[0] as u32;
                        g += p[1] as u32;
                        b += p[2] as u32;
                    }
                }
            }
            out[oy * ow + ox] = 0xFF00_0000 | ((r / n) << 16) | ((g / n) << 8) | (b / n);
        }
    }
}

struct Anim {
    path: String,
    src: Stream,
    k: usize,
    bw: usize,
    bh: usize,
    /// `crate::arch::ms()` at which the frame on screen has had its time.
    due: u64,
    /// Complete plays so far.
    plays: u32,
    /// Resting on the last frame (the loop count is spent, or the file broke).
    rest: bool,
    /// `ms()` when a pause began.
    paused_at: Option<u64>,
    steps: u64,
}

static ANIM: crate::sync::Mutex<Option<Anim>> = crate::sync::Mutex::new(None);
/// The title's view of the animation (read by `facet::title_for` while [`tick`] holds [`ANIM`], so
/// it is atomics, never the lock).
static T_ON: AtomicBool = AtomicBool::new(false);
static T_I: AtomicUsize = AtomicUsize::new(0);
static T_N: AtomicUsize = AtomicUsize::new(0);
static PAUSED: AtomicBool = AtomicBool::new(false);

/// ` - frame i/n` (1-based, `paused` appended) while an animation is shown, else `""`.
pub fn title_suffix() -> String {
    if !T_ON.load(Ordering::Relaxed) {
        return String::new();
    }
    alloc::format!(
        " - frame {}/{}{}",
        T_I.load(Ordering::Relaxed) + 1,
        T_N.load(Ordering::Relaxed),
        if PAUSED.load(Ordering::Relaxed) { " paused" } else { "" }
    )
}

/// The pause key: flip the pause (the next [`tick`] honours it). `false` when no animation is shown
/// (the key is then not Facet's).
pub fn toggle_pause() -> bool {
    if !T_ON.load(Ordering::Relaxed) {
        return false;
    }
    let p = !PAUSED.fetch_xor(true, Ordering::Relaxed);
    serial_println!("[facet] anim pause={}", p as u8);
    true
}

/// End the animation (Facet closed, or another file replaced it).
pub fn stop() {
    T_ON.store(false, Ordering::Relaxed);
    PAUSED.store(false, Ordering::Relaxed);
    if let Some(mut g) = ANIM.try_lock() {
        *g = None;
    }
}

/// Open `path`'s stream: frame 0 through Facet's window body, the rest stepped by [`tick`].
pub(super) fn open_frames(path: &str, bytes: u64, mut src: Stream) -> Result<super::Opened, FacetError> {
    let (w, h) = (src.width(), src.height());
    let (k, bw, bh) = super::fit(w, h, super::BASE_W, super::BASE_H).ok_or(FacetError::NoWindow("fit"))?;
    let f0 = match src.next_frame() {
        Some(Ok(f)) => f,
        Some(Err(e)) => return Err(FacetError::Pixel(e)),
        None => return Err(FacetError::NoWindow("no-frames")),
    };
    let mut first: Vec<u32> = Vec::new();
    if first.try_reserve_exact(bw * bh).is_err() {
        return Err(FacetError::OutOfMemory(bw * bh * 4));
    }
    first.resize(bw * bh, 0xFF00_0000);
    reduce_into(src.canvas(), w as usize, k, bw, bh, &mut first);
    let ihdr = Ihdr { width: w, height: h, depth: 8, colour: 6, interlaced: false };
    let n = src.frame_count();
    serial_println!(
        "[facet] anim path={} frames={} loop={} w={} h={} decoder={} k={}",
        path, n, loop_name(src.loop_count()), w, h, decoder().name(), k
    );
    PAUSED.store(false, Ordering::Relaxed);
    T_I.store(0, Ordering::Relaxed);
    T_N.store(n, Ordering::Relaxed);
    T_ON.store(n > 1, Ordering::Relaxed);
    let o = match super::open_base(path, bytes, ihdr, k, bw, bh, first) {
        Ok(o) => o,
        Err(e) => {
            T_ON.store(false, Ordering::Relaxed);
            return Err(e);
        }
    };
    if n > 1 {
        let due = crate::arch::ms() + played(f0.delay_ms);
        *ANIM.lock() = Some(Anim { path: String::from(path), src, k, bw, bh, due, plays: 0, rest: false, paused_at: None, steps: 0 });
    }
    Ok(o)
}

fn loop_name(l: Option<u16>) -> String {
    match l {
        Some(0) => String::from("forever"),
        Some(n) => alloc::format!("{}", n),
        None => String::from("once"),
    }
}

/// Composite the next frame (looping per the loop count) into `px` (`bw x bh`). `Ok(Some(i, delay))`
/// = frame `i` is up; `Ok(None)` = the animation has played out and rests. Shared by [`tick`] and
/// `tests facetanim`, so the fixture drives the viewer's own step.
fn advance(src: &mut Stream, plays: &mut u32, k: usize, bw: usize, bh: usize, px: &mut [u32]) -> Result<Option<(usize, u32)>, FacetError> {
    let f = match src.next_frame() {
        Some(Ok(f)) => f,
        Some(Err(e)) => return Err(FacetError::Pixel(e)),
        None => {
            *plays += 1;
            let again = match src.loop_count() {
                Some(0) => true,
                Some(n) => *plays <= n as u32,
                None => false,
            };
            if !again {
                return Ok(None);
            }
            src.rewind();
            match src.next_frame() {
                Some(Ok(f)) => f,
                Some(Err(e)) => return Err(FacetError::Pixel(e)),
                None => return Ok(None),
            }
        }
    };
    reduce_into(src.canvas(), src.width() as usize, k, bw, bh, px);
    Ok(Some((f.index, f.delay_ms)))
}

/// Facet's service pass: step the frame when its delay has run out. A closed or replaced view ends
/// the animation. Quiet pass = one uncontended lock and a clock read.
pub fn tick() {
    let mut g = ANIM.lock();
    let Some(a) = g.as_mut() else { return };
    if !super::is_open() || super::shown() != a.path {
        *g = None;
        T_ON.store(false, Ordering::Relaxed);
        return;
    }
    let now = crate::arch::ms();
    if PAUSED.load(Ordering::Relaxed) {
        if a.paused_at.is_none() {
            a.paused_at = Some(now);
            if let Some(view) = super::VIEW.lock().as_mut() {
                super::refresh(view); // the title says "paused"
            }
        }
        return;
    }
    if let Some(t) = a.paused_at.take() {
        a.due += now.saturating_sub(t);
        if let Some(view) = super::VIEW.lock().as_mut() {
            super::refresh(view);
        }
    }
    if a.rest || now < a.due {
        return;
    }
    let mut v = super::VIEW.lock();
    let Some(view) = v.as_mut() else { return };
    if view.px.len() != a.bw * a.bh {
        return;
    }
    match advance(&mut a.src, &mut a.plays, a.k, a.bw, a.bh, &mut view.px) {
        Ok(Some((i, delay))) => {
            // Behind by more than a frame (a long pass): restart the clock instead of racing.
            a.due = if now.saturating_sub(a.due) > 1000 { now } else { a.due } + played(delay);
            a.steps += 1;
            T_I.store(i, Ordering::Relaxed);
            T_N.store(a.src.frame_count(), Ordering::Relaxed);
            if a.steps % 64 == 1 {
                serial_println!("[facet] anim step={} frame={}/{} path={}", a.steps, i + 1, a.src.frame_count(), a.path);
            }
            super::refresh(view);
        }
        Ok(None) => {
            a.rest = true;
            serial_println!("[facet] anim end plays={} frames={} path={}", a.plays, a.src.frame_count(), a.path);
        }
        Err(e) => {
            a.rest = true;
            serial_println!("[facet] anim end plays={} broke={} path={}", a.plays, e.reason(), a.path);
        }
    }
}

/// A two-frame 1x1 GIF89a (graphic-control extensions with 100 ms and 50 ms delays).
const TWO_FRAME_GIF: &[u8] = &[
    b'G', b'I', b'F', b'8', b'9', b'a', 1, 0, 1, 0, 0x80, 0, 0, // screen 1x1, 2-colour table
    0xFF, 0xFF, 0xFF, 0, 0, 0, // palette
    0x21, 0xF9, 4, 0, 10, 0, 0, 0, // GCE delay 10 cs
    0x2C, 0, 0, 0, 0, 1, 0, 1, 0, 0, 2, 2, 0x44, 1, 0, // frame 0
    0x21, 0xF9, 4, 0, 5, 0, 0, 0, // GCE delay 5 cs
    0x2C, 0, 0, 0, 0, 1, 0, 1, 0, 0, 2, 2, 0x44, 1, 0, // frame 1
    0x3B,
];

/// `tests quarry2`'s frame leg: `(frames the adapter decoded from a real two-frame GIF, the stepper's
/// verdict)`. Since FACETANIM the adapter is pixel_core and the GIF MUST play two frames (100, 50 ms).
/// `Err` names a failed claim.
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
    // A PNG still is not taken: Facet's own streaming path keeps it.
    let png_head: [u8; 24] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13, b'I', b'H', b'D', b'R', 0, 0, 0, 2, 0, 0, 0, 2];
    if decoder().takes(&png_head) {
        return Err(String::from("the adapter took a PNG without acTL"));
    }
    if !decoder().takes(TWO_FRAME_GIF) {
        return Err(String::from("the adapter did not take a GIF"));
    }
    let mut s = decoder().open(Vec::from(TWO_FRAME_GIF)).map_err(|e| alloc::format!("open: {}", e.reason()))?;
    let mut delays = Vec::new();
    while let Some(f) = s.next_frame() {
        delays.push(f.map_err(|e| alloc::format!("frame: {}", e))?.delay_ms);
    }
    if delays != [100, 50] || s.frame_count() != 2 {
        return Err(alloc::format!("the adapter played {:?} ({} frames) from a two-frame GIF", delays, s.frame_count()));
    }
    Ok((Some(2), "ok"))
}

/// `tests facetanim` (FACETANIM M3): open the three staged 3-frame fixtures through [`decoder`], drive
/// the viewer's own step ([`advance`] into a base image) to frame 1 and read the pixel back — green at
/// (3,3), red at (0,0) — then play to the end and count.
///
/// `:: FACETANIM: gif=3 webp=3 apng=3 frames_held=2 -> PASS ::`
pub fn selftest() {
    const FIXTURES: [(&str, &str); 3] = [("gif", "ANIM3.GIF"), ("webp", "ANIM3.WEBP"), ("apng", "ANIM3.PNG")];
    let mut parts: Vec<String> = Vec::new();
    let (mut fails, mut absent, mut held_max) = (0usize, 0usize, 0usize);
    for (tag, leaf) in FIXTURES {
        let found = ["/apps", ""].iter().map(|d| alloc::format!("{}/{}", d, leaf)).find(|p| crate::shell::vfs_mount_table().stat(p).is_ok());
        let Some(path) = found else {
            serial_println!("[facetanim] {} absent (/apps/{} not staged)", tag, leaf);
            parts.push(alloc::format!("{}=absent", tag));
            absent += 1;
            continue;
        };
        match leg(&path) {
            Ok((n, held)) => {
                held_max = held_max.max(held);
                parts.push(alloc::format!("{}={}", tag, n));
            }
            Err(e) => {
                serial_println!("[facetanim] {} FAIL path={}: {}", tag, path, e);
                parts.push(alloc::format!("{}=FAIL", tag));
                fails += 1;
            }
        }
    }
    let verdict = if fails > 0 {
        "FAIL"
    } else if absent > 0 {
        "SKIP"
    } else {
        "PASS"
    };
    serial_println!(":: FACETANIM: {} frames_held={} -> {} ::", parts.join(" "), held_max, verdict);
}

/// One fixture: `(frames played, frames held at most = the base image + the decoder's buffers)`.
fn leg(path: &str) -> Result<(usize, usize), String> {
    let bytes = read_taken(path).ok_or_else(|| String::from("the adapter did not take it"))?;
    let mut src = decoder().open(bytes).map_err(|e| e.reason())?;
    if (src.width(), src.height(), src.frame_count()) != (8, 8, 3) {
        return Err(alloc::format!("{}x{} frames={}", src.width(), src.height(), src.frame_count()));
    }
    // An 8x8 file fits the base at k = 1: the base image IS the frame.
    let (k, bw, bh) = super::fit(8, 8, super::BASE_W, super::BASE_H).ok_or_else(|| String::from("fit"))?;
    let mut base = alloc::vec![0xFF00_0000u32; bw * bh];
    let mut plays = 0u32;
    let mut held = 0usize;
    fn step(src: &mut Stream, plays: &mut u32, k: usize, bw: usize, bh: usize, base: &mut [u32], held: &mut usize) -> Result<(usize, u32), String> {
        let r = advance(src, plays, k, bw, bh, base).map_err(|e| e.reason())?;
        *held = (*held).max(1 + src.buffers_held());
        r.ok_or_else(|| String::from("ended early"))
    }
    let (i0, d0) = step(&mut src, &mut plays, k, bw, bh, &mut base, &mut held)?;
    let (i1, _) = step(&mut src, &mut plays, k, bw, bh, &mut base, &mut held)?;
    let at = |base: &[u32], x: usize, y: usize| base[y * bw + x] & 0x00FF_FFFF;
    let (g, r) = (at(&base, 3, 3), at(&base, 0, 0));
    serial_println!("[facetanim] {} steps={},{} delay0={} frame1 px(3,3)={:06x} px(0,0)={:06x}", path, i0, i1, d0, g, r);
    if (i0, i1, d0) != (0, 1, 200) || g != 0x00_FF00 || r != 0xFF_0000 {
        return Err(alloc::format!("frame 1 read back px(3,3)={:06x} px(0,0)={:06x}", g, r));
    }
    let (i2, _) = step(&mut src, &mut plays, k, bw, bh, &mut base, &mut held)?;
    if i2 != 2 || at(&base, 3, 3) != 0x00_00FF {
        return Err(String::from("frame 2 is not blue"));
    }
    // Loop forever: the fourth step is frame 0 again, after one complete play.
    let (i3, _) = step(&mut src, &mut plays, k, bw, bh, &mut base, &mut held)?;
    if i3 != 0 || plays != 1 || at(&base, 3, 3) != 0xFF_0000 {
        return Err(alloc::format!("the loop did not come back to frame 0 (index {} plays {})", i3, plays));
    }
    Ok((src.frame_count(), held))
}
