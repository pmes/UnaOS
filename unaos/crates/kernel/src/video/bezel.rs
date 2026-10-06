// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//! CHARTER: Kernel — wm
//!
//! BEZEL (rmbp-ledger B405, MACPARITY row 26) — the brightness / volume bezel. F1/F2 and F10/F11/F12 show a
//! square, centre-bottom of the screen: our glyph (a sun; a speaker with 0–3 waves; a crossed speaker for
//! mute) over a 16-segment bar reading the LIVE value — the backlight register READBACK mapped by
//! BRIGHTSLIDER's own `backlight::pos_for_raw` (the slider's scale), the codec's output amp read back
//! (`hda::vol_readback`, the rings `vol::apply` drives). It re-arms on each press and goes [`BEZEL_MS`] after
//! the last. The row is the toast's: a chromeless compat row (`wm::overlay_open`, owner 0) — `hit_test`
//! never names it, nothing focuses it, it takes no key and no press. No service, no store: the bezel keeps
//! only what to show and until when.
//!
//! [`arm`] is atomics only (the key paths: the decoder's `brightkeys::key`, the HID service's
//! `status::volkey_usage`); [`service`] runs on the desktop pass from `brightkeys::service`, AFTER the
//! backlight write, and also under the login screen and the setter (R86: a compositor draw, not a service).
//! Wire: `[bezel] show kind=<brightness|volume|mute> level=<n>/16 ms=1500 src=<readback|amp|model> win=<n>`,
//! `[bezel] faded kind=<k> after_ms=<n>`. Witness (`tests bezel`, R80: never at boot):
//! `:: BEZEL: brightness=ok volume=ok mute=ok fade_ms=1500 scale=shared -> PASS ::`.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

use super::{metrics, wm};

/// How long the bezel stays after the last key.
pub const BEZEL_MS: u64 = 1500;
/// Segments in the level bar (the backlight's and the amp's 1/16 grid).
pub const SEGS: u8 = 16;

pub const K_NONE: u8 = 0;
pub const K_BRIGHT: u8 = 1;
pub const K_VOL: u8 = 2;
pub const K_MUTE: u8 = 3;
/// PLAYER (B419): F8 on the open player — the state it landed on (no level bar).
pub const K_PLAY: u8 = 4;
pub const K_PAUSE: u8 = 5;

// Logical geometry (the Mac's bezel is ~200 pt square).
const W: usize = 200;
const H: usize = 200;
const BAR_X: usize = 20;
const BAR_Y: usize = 166;
const SEG_W: usize = 9;
const SEG_GAP: usize = 1;
const SEG_H: usize = 8;
const RADIUS: usize = 18;

// Our colours: a dark face (the translucent look owed — the compat row is opaque xRGB), light ink.
const FACE: u32 = 0x0026_2629;
const EDGE: u32 = 0x003C_3C40;
const INK: u32 = 0x00EE_EEF0;
const DIM: u32 = 0x004A_4A50;

/// Armed kind (0 none), set by [`arm`]; the service takes it.
static ARMED: AtomicU8 = AtomicU8::new(K_NONE);
/// What is showing (0 none), its level, the deadline, the last show time, the row.
static SHOWN_KIND: AtomicU8 = AtomicU8::new(K_NONE);
static SHOWN_LEVEL: AtomicU8 = AtomicU8::new(0);
static UNTIL: AtomicU64 = AtomicU64::new(0);
static SHOWN_AT: AtomicU64 = AtomicU64::new(0);
static WIN: AtomicU32 = AtomicU32::new(0);
static HEADLESS: AtomicBool = AtomicBool::new(false);
static GATE: spin::Mutex<()> = spin::Mutex::new(());

/// A key moved brightness / volume / mute: show the bezel on the next desktop pass. Atomics only.
pub fn arm(kind: u8) {
    ARMED.store(kind, Ordering::Release);
}

/// What the bezel is about to show (armed) or showing; `K_NONE` when neither. Atomics only.
pub fn indicator() -> u8 {
    match ARMED.load(Ordering::Acquire) {
        K_NONE => SHOWN_KIND.load(Ordering::Acquire),
        k => k,
    }
}

/// The volume key's kind from the model it just wrote (mute wins).
pub fn arm_volume(muted: bool) {
    arm(if muted { K_MUTE } else { K_VOL });
}

fn kind_name(k: u8) -> &'static str {
    match k {
        K_BRIGHT => "brightness",
        K_VOL => "volume",
        K_MUTE => "mute",
        K_PLAY => "play",
        K_PAUSE => "pause",
        _ => "none",
    }
}

/// The brightness segment for a register value: BRIGHTSLIDER's linear scale, the same function the
/// slider's knob is placed by (`pos_for_raw`), over 16.
pub fn bright_segments(raw: u32, max: u32) -> u8 {
    (crate::video::backlight::pos_for_raw(raw, max, SEGS as usize) as u8).min(SEGS)
}

/// The live level for `kind`: `(level, muted, src)`.
fn live(kind: u8) -> (u8, bool, &'static str) {
    if kind == K_BRIGHT {
        let max = crate::video::backlight::panel_range();
        return (bright_segments(crate::video::backlight::cur_raw(), max), false, "readback");
    }
    let (lv, m) = crate::video::status::volume();
    #[cfg(all(target_arch = "x86_64", feature = "hda"))]
    if let Some((alv, am, gain, steps)) = crate::drivers::hda::vol_readback() {
        // The amp holds the model's gain (`apply`'s `level * steps / 16`): the model's level is the one shown
        // (a coarse amp maps several levels to one gain). Otherwise the amp's own level — it is the truth.
        let agrees = gain as u32 == lv.min(16) as u32 * steps as u32 / 16;
        return (if agrees || steps == 0 { lv } else { alv }, am, "amp");
    }
    (lv, m, "model")
}

/// `true` while a bezel is up.
pub fn showing() -> bool {
    SHOWN_KIND.load(Ordering::Acquire) != K_NONE
}

/// The desktop pass: take an armed key (show / re-arm), close an expired bezel.
pub fn service() {
    let Some(_g) = GATE.try_lock() else { return };
    let k = ARMED.swap(K_NONE, Ordering::AcqRel);
    let now = crate::arch::ms();
    if k == K_PLAY || k == K_PAUSE {
        show(k, 0, "player", now);
        return;
    }
    if k != K_NONE {
        let (lv, muted, src) = live(k);
        let kind = if k != K_BRIGHT && muted { K_MUTE } else if k == K_MUTE { K_VOL } else { k };
        show(kind, lv, src, now);
        return;
    }
    let until = UNTIL.load(Ordering::Acquire);
    if until != 0 && now >= until {
        close(now);
    }
}

fn show(kind: u8, lv: u8, src: &str, now: u64) {
    SHOWN_KIND.store(kind, Ordering::Release);
    SHOWN_LEVEL.store(lv, Ordering::Relaxed);
    SHOWN_AT.store(now.max(1), Ordering::Relaxed);
    UNTIL.store(now.saturating_add(BEZEL_MS).max(1), Ordering::Release);
    let win = draw(kind, lv);
    serial_println!("[bezel] show kind={} level={}/{} ms={} src={} win={}", kind_name(kind), lv, SEGS, BEZEL_MS, src, win);
}

fn close(now: u64) {
    let kind = SHOWN_KIND.swap(K_NONE, Ordering::AcqRel);
    UNTIL.store(0, Ordering::Release);
    let win = WIN.swap(0, Ordering::AcqRel);
    if win != 0 {
        wm::close(win);
    }
    if kind != K_NONE {
        serial_println!("[bezel] faded kind={} after_ms={}", kind_name(kind), now.saturating_sub(SHOWN_AT.load(Ordering::Relaxed)));
    }
}

// ── the draw ─────────────────────────────────────────────────────────────────────────────────────────────

static SURF_AT: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
fn surf() -> &'static mut [u32] {
    let n = metrics::size(W) * metrics::size(H);
    let mut p = SURF_AT.load(Ordering::Acquire);
    if p == 0 {
        let b: &'static mut [u32] = alloc::boxed::Box::leak(alloc::vec![0u32; n].into_boxed_slice());
        p = match SURF_AT.compare_exchange(0, b.as_mut_ptr() as usize, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => b.as_mut_ptr() as usize,
            Err(won) => won,
        };
    }
    // SAFETY: one leaked buffer of `n` words, painted on the desktop pass, read by `wm`'s composite.
    unsafe { core::slice::from_raw_parts_mut(p as *mut u32, n) }
}

/// Physical-pixel painter over the surface (`pw` x `ph`). Shapes test each pixel's centre in LOGICAL px x16
/// (integers only: the kernel target is soft-float).
struct P<'a> {
    px: &'a mut [u32],
    pw: usize,
    ph: usize,
}

/// One logical px in the shape tests' units.
const U: i64 = 16;

impl P<'_> {
    /// Fill every physical pixel in the logical box whose centre `(X, Y)` (logical x16) satisfies `f`.
    fn shape(&mut self, x0: usize, y0: usize, x1: usize, y1: usize, c: u32, f: impl Fn(i64, i64) -> bool) {
        let (px0, py0, px1, py1) = (metrics::edge(x0), metrics::edge(y0), metrics::edge(x1).min(self.pw), metrics::edge(y1).min(self.ph));
        let s = metrics::size(100).max(1) as i64;
        for py in py0..py1 {
            let ly = (2 * py as i64 + 1) * 800 / s;
            for px in px0..px1 {
                if f((2 * px as i64 + 1) * 800 / s, ly) {
                    self.px[py * self.pw + px] = c;
                }
            }
        }
    }
}

/// Inside the rounded rect `(0, 0, w, h)` of corner radius `r` (all logical x16).
fn rounded(x: i64, y: i64, w: i64, h: i64, r: i64) -> bool {
    let cx = x.clamp(r, w - r);
    let cy = y.clamp(r, h - r);
    let (dx, dy) = (x - cx, y - cy);
    dx * dx + dy * dy <= r * r
}

/// The eight ray directions of the sun, x1000.
const RAYS: [(i64, i64); 8] = [(1000, 0), (707, 707), (0, 1000), (-707, 707), (-1000, 0), (-707, -707), (0, -1000), (707, -707)];

fn paint(kind: u8, lv: u8) {
    let (pw, ph) = (metrics::size(W), metrics::size(H));
    let mut p = P { px: surf(), pw, ph };
    // The square: an edge ring outside the radius, the face inside it (opaque — see the owed note).
    p.px.fill(FACE);
    let (wf, hf, r) = (W as i64 * U, H as i64 * U, RADIUS as i64 * U);
    p.shape(0, 0, W, H, EDGE, |x, y| !rounded(x - U, y - U, wf - 2 * U, hf - 2 * U, r - U));
    // The glyph, centred at (100, 80).
    let (cx, cy) = (100 * U, 80 * U);
    match kind {
        K_BRIGHT => {
            p.shape(80, 60, 120, 100, INK, |x, y| { let (dx, dy) = (x - cx, y - cy); dx * dx + dy * dy <= (16 * U) * (16 * U) });
            for (ux, uy) in RAYS {
                p.shape(60, 40, 140, 120, INK, move |x, y| {
                    let (dx, dy) = (x - cx, y - cy);
                    let along = (dx * ux + dy * uy) / 1000;
                    let across = (dx * uy - dy * ux) / 1000;
                    (22 * U..=34 * U).contains(&along) && across.abs() <= 3 * U
                });
            }
        }
        K_VOL | K_MUTE => {
            // the speaker: a box and a flared cone
            p.shape(50, 66, 66, 94, INK, |_, _| true);
            p.shape(66, 50, 90, 110, INK, move |x, y| 24 * (y - cy).abs() <= 14 * 24 * U + 16 * (x - 66 * U));
            if kind == K_MUTE {
                // crossed: two strokes where the waves go
                let (mx, half, t) = (120 * U, 16 * U, 4 * U);
                p.shape(100, 60, 140, 100, INK, move |x, y| { let (dx, dy) = (x - mx, y - cy); dx.abs() <= half && ((dx - dy).abs() <= t || (dx + dy).abs() <= t) });
            } else {
                let waves = if lv == 0 { 0 } else if lv <= 5 { 1 } else if lv <= 11 { 2 } else { 3 };
                for w in 0..waves {
                    let rr = (16 + 12 * w as i64) * U;
                    let (lo, hi) = ((rr - 5 * U / 2) * (rr - 5 * U / 2), (rr + 5 * U / 2) * (rr + 5 * U / 2));
                    p.shape(90, 30, 150, 130, INK, move |x, y| {
                        let (dx, dy) = (x - 88 * U, y - cy);
                        let d2 = dx * dx + dy * dy;
                        dx > 0 && 5 * dy.abs() < 6 * dx && d2 >= lo && d2 <= hi
                    });
                }
            }
        }
        K_PLAY => {
            // the play wedge, pointing right
            p.shape(76, 50, 132, 110, INK, move |x, y| { let dx = x - 78 * U; dx >= 0 && (y - cy).abs() * 52 <= 30 * (52 * U - dx) });
        }
        K_PAUSE => {
            p.shape(78, 54, 94, 106, INK, |_, _| true);
            p.shape(106, 54, 122, 106, INK, |_, _| true);
        }
        _ => {}
    }
    if kind == K_PLAY || kind == K_PAUSE {
        return; // no level bar for the transport glyphs
    }
    // The level bar: 16 segments, lit to `lv` (mute lights none).
    let lit = if kind == K_MUTE { 0 } else { lv.min(SEGS) as usize };
    for i in 0..SEGS as usize {
        let x = BAR_X + i * (SEG_W + SEG_GAP);
        metrics::fill(p.px, pw, x, BAR_Y, SEG_W, SEG_H, if i < lit { INK } else { DIM });
    }
}

/// Paint and open (or re-present) the row. Returns the row id (0: headless, no panel, aarch64).
fn draw(kind: u8, lv: u8) -> u32 {
    if HEADLESS.load(Ordering::Relaxed) {
        return 0;
    }
    #[cfg(all(target_arch = "x86_64", feature = "wc"))]
    {
        let (pw, ph) = { let i = super::WRITER.lock().info(); (i.width, i.height) };
        if pw == 0 || ph == 0 {
            return 0;
        }
        paint(kind, lv);
        let cur = WIN.load(Ordering::Acquire);
        if cur != 0 && wm::present(cur) {
            return cur;
        }
        let (sw, sh) = (metrics::size(W), metrics::size(H));
        let x = pw.saturating_sub(sw) / 2;
        let y = ph.saturating_sub(sh + metrics::size(96));
        let id = wm::overlay_open(surf().as_mut_ptr() as usize, sw * sh * 4, sw, sh, x, y);
        WIN.store(if id == wm::WIN_NONE { 0 } else { id }, Ordering::Release);
        return if id == wm::WIN_NONE { 0 } else { id };
    }
    #[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
    {
        let _ = (kind, lv, paint as fn(u8, u8));
        0 // aarch64: wire-only (no chromeless overlay row there yet — the toast's shape)
    }
}

// ── `tests bezel` (R80: registered from the desktop pass, never run at boot) ────────────────────────────────

/// Register `tests bezel` once (called from `brightkeys::service`, the desktop pass).
pub fn ensure_registered() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, Ordering::AcqRel) {
        crate::tests::register("bezel", fixture);
    }
}

/// `tests bezel`: a brightness key, a volume key and the mute key through the REAL key paths, each shown by
/// the bezel at the LIVE level (register readback / amp readback); the fade closes the row at its deadline;
/// the key grid and the slider share one scale. Restores the backlight register, the volume and the mute.
pub fn fixture() {
    let max = crate::video::backlight::max_now();
    let prev_raw = crate::video::backlight::cur_raw();
    let (l0, m0) = crate::video::status::volume();
    // brightness: one Up (or Down at the top) through the key path and the desktop pass's writer.
    let up = crate::video::backlight::step_raw(prev_raw, max, true).is_some();
    crate::video::brightkeys::key(if up { crate::video::keymap::Action::BrightnessUp } else { crate::video::keymap::Action::BrightnessDown });
    crate::video::brightkeys::service();
    let rb = crate::video::backlight::readback_now().unwrap_or(crate::video::backlight::cur_raw());
    let b_lv = bright_segments(rb, max);
    let b_ok = SHOWN_KIND.load(Ordering::Acquire) == K_BRIGHT && SHOWN_LEVEL.load(Ordering::Relaxed) == b_lv;
    let slider = crate::video::settings::slider_sync(rb);
    let _ = crate::video::backlight::set_raw_via(prev_raw, "bezel-restore");
    let _ = crate::video::settings::slider_sync(crate::video::backlight::cur_raw());
    // volume: F12 (up) through the decoder's seam, shown at the amp's level.
    crate::video::status::set_volume(l0.min(15), false);
    let _ = crate::video::status::volkey_usage(0x45);
    service();
    let (v_lv, v_m, v_src) = live(K_VOL);
    let v_ok = SHOWN_KIND.load(Ordering::Acquire) == K_VOL && SHOWN_LEVEL.load(Ordering::Relaxed) == v_lv && !v_m
        && v_lv == crate::video::status::volume().0;
    // mute: F10 toggles, the bezel says mute, the amp's mute bit agrees.
    let _ = crate::video::status::volkey_usage(0x43);
    service();
    let (_, m_m, _) = live(K_MUTE);
    let m_ok = SHOWN_KIND.load(Ordering::Acquire) == K_MUTE && m_m && crate::video::status::volume().1;
    let win = WIN.load(Ordering::Acquire);
    // fade: still up before the deadline, gone at it.
    service();
    let held = showing();
    UNTIL.store(crate::arch::ms().saturating_sub(1).max(1), Ordering::Release);
    service();
    let gone = !showing() && WIN.load(Ordering::Acquire) == 0;
    let fade_ok = held && gone;
    crate::video::status::set_volume(l0, m0);
    // scale: every key level lands on its own segment, and the slider's linear map agrees with it.
    let mut keys = true;
    let mut l = crate::video::backlight::FLOOR;
    while l <= crate::video::backlight::STEPS {
        keys &= bright_segments(crate::video::backlight::raw_for(l, max), max) == l;
        keys &= bright_segments(crate::video::backlight::raw_for_pos(l as usize, SEGS as usize, max), max) == l;
        l += 1;
    }
    let shared = keys && slider;
    serial_println!(
        "[bezel] fixture bright_level={}/16 reg={} vol_level={}/16 src={} win={} held={} gone={} keys={} slider_sync={}",
        b_lv, rb, v_lv, v_src, win, held as u8, gone as u8, keys as u8, slider as u8
    );
    let ok = b_ok && v_ok && m_ok && fade_ok && shared;
    let w = |b: bool| if b { "ok" } else { "bad" };
    serial_println!(
        ":: BEZEL: brightness={} volume={} mute={} fade_ms={} scale={} -> {} ::",
        w(b_ok), w(v_ok), w(m_ok), if fade_ok { BEZEL_MS } else { 0 }, if shared { "shared" } else { "split" },
        if ok { "PASS" } else { "FAIL" }
    );
}

/// PLAYER (B419): F8 toggled the open player; show what it landed on. Atomics only.
pub fn arm_transport(playing: bool) {
    arm(if playing { K_PLAY } else { K_PAUSE });
}
