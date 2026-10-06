// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Stria — owed B289 (an A/V player window: Stria's domain, CODEX §2; ARCHREVIEW F3 — was declared `Kernel — wm`)
//!
//! PLAYER (rmbp-ledger B419, MACPARITY row 30; cloud review §15 "(b) PLAYER (a window with transport controls)").
//! Quarry's double-click on a sound opened nothing: `hda::play::request_open` played it headless and the only stop
//! was `play stop` at the serial door. This is the window that owns the play: the opener's APPRES icon, the file's
//! name, an info line (codec, rate, channels, duration), a play/pause button, a scrubber that DRAGS (PREFSUI's
//! `capture` seam) and seeks, elapsed / remaining time, a mute glyph and a volume slider on the one volume model
//! (`status::set_volume` — the Settings slider's, the F10–F12 keys', the amp BEZEL reads back).
//!
//! * One player: a second open replaces the first (its play stops, its window closes). Closing the window — the
//!   close box, or any close the WM makes (Cmd-W, the File menu, Quit: the WINID holder clears [`WIN`]) — stops
//!   the play. `tests play` keeps its own door path (the queue in `hda_play.rs`), untouched.
//! * The play stays the HDA driver's (`drivers/hda_play.rs`, its PLAYER tail: `pause`, `seek_to`, `position_ms`,
//!   `facts`). A seek is `method=table`: a WAV's byte offset (`table=pcm`), a coded file's container index through
//!   `audio_core::Decoder::seek` in the `play-dec` job (SEEKTABLE B433: `table=flac|xing|vbri|cbr|mp4`); a format
//!   with no table there is `method=decode-skip table=none` (Ogg and ADTS have one since SEEKTABLE2, B469: `table=ogg|adts`).
//! * F7/F8/F9 (previous / play-pause / next) reach [`media_key`] from `status::volkey_usage` (atomics only); the
//!   next pass acts and arms BEZEL's play/pause glyph. With no player open the keys do nothing (said once).
//! * The info line reads ATTRCOLUMNS' `media:duration_ms` / `media:codec` through `get_attr`; without them, the
//!   decoder's own count (`src=decoder`).
//!
//! Doors (fileview's): [`request_open`] latches (Quarry's `play` opener), [`service`] / [`press_route`] /
//! [`key_route`] are chained from `quarry::live`. Wire: `[player] open …`, `[player] key media=…`,
//! `[player] scrub …`, `[player] volume …`, `[player] closed win=<n> by=<how> stops=<0|1>`; `tests player`
//! (R80: typed, never at boot) → `:: PLAYER: open=ok transport=ok seek=ok volume=shared close_stops=1 -> PASS ::`.
//! Design: docs/dev/evidence/rmbp-1005/player.md.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};

use crate::video::{theme, wm};

/// Kernel-furniture owner slot (clear of quarry 3, facet 4, fileview 5, textedit 6, settings/activity 7).
pub const OWNER: u64 = wm::KERNEL_OWNER_BASE + 0x0C;
const _: () = assert!(OWNER != wm::KERNEL_OWNER_CONSOLE && OWNER != wm::KERNEL_OWNER_DESKTOP);

/// Logical geometry (scaled by `crate::ui::px`).
const W_L: usize = 480;
const H_L: usize = 172;
/// VIDEOPLAYER (B434): the video rect's logical height above the transport (16:9 at W_L; other shapes letterbox).
const VH_L: usize = 270;
const ICON_L: usize = 48;
const TEXT_X: usize = 76;
const SCRUB_Y: usize = 92;
const SCRUB_PAD: usize = 64;
const BTN_X: usize = 16;
const BTN_Y: usize = 116;
const BTN_D: usize = 40;
const MUTE_X: usize = 268;
const VOL_X0: usize = 300;
const VOL_X1: usize = 464;
const CTRL_Y: usize = 136;
/// Arrow-key seek step.
const STEP_MS: u64 = 5_000;

const DIM_TEXT: u32 = super::theme::PLAYER_DIM_TEXT;
const KNOB_EDGE: u32 = super::theme::PLAYER_KNOB_EDGE;

static WIN: AtomicU32 = AtomicU32::new(wm::WIN_NONE);
static PENDING: crate::sync::Mutex<Option<String>> = crate::sync::Mutex::new(None); static PENDING_POSTED: super::svclatch::Latch = super::svclatch::Latch::new(); static ACT_POSTED: super::svclatch::Latch = super::svclatch::Latch::new(); // SVCLATCH (B462): the posted flags the pass reads before the locks
static STATE: crate::sync::Mutex<Option<State>> = crate::sync::Mutex::new(None);
/// F7/F8/F9 latched by the HID service: 0 none, 1 previous, 2 play/pause, 3 next.
static MEDIA: AtomicU8 = AtomicU8::new(0);
/// A transport action the pointer / keys asked for, run on the next pass (an open or a seek re-arms the HDA
/// stream through the probe: never at click-router depth — `facet::request_open`'s reason).
static ACT: crate::sync::Mutex<Option<Act>> = crate::sync::Mutex::new(None);

#[derive(Clone, Copy)]
enum Act {
    Toggle,
    Seek(u64),
}

fn post(a: Act) {
    *ACT.lock() = Some(a); ACT_POSTED.post(); // SVCLATCH (B462)
}

/// What the pointer holds: nothing, the scrubber, the volume knob.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Drag {
    None,
    Scrub,
    Vol,
}

struct State {
    path: String,
    name: String,
    /// Codec, rate, channels — the info line's head; empty until known.
    codec: String,
    detail: String,
    dur_ms: u64,
    /// Where `dur_ms`/`codec` came from: `attrs` (ATTRCOLUMNS), `decoder`, `pending` (the job has not opened yet).
    src: &'static str,
    /// The stream's origin in the file (a seek's landing), and the elapsed the window last showed.
    base_ms: u64,
    shown_ms: u64,
    /// The user's intent: playing (not paused). `ended` = the play drained to its end.
    playing: bool,
    ended: bool,
    err: Option<String>,
    drag: Drag,
    drag_ms: u64,
    drag_samples: u32,
    drag_from: u64,
    w: usize,
    h: usize,
    surf: Vec<u32>,
    /// VIDEOPLAYER (B434): a video's picture rect is the top `oy` rows; the transport paints below it (0 = audio).
    oy: usize,
    video: bool,
    /// The container's facts reached the info line (the `[player] video` line printed).
    #[allow(dead_code)] // read by the video arm only (UNAOS_VIDEO)
    vsaid: bool,
}

// ── the HDA play, behind the one gate that has it ──────────────────────────────────────────────────────────

#[cfg(all(target_arch = "x86_64", feature = "hda-tone"))]
mod hw {
    use alloc::string::String;
    use crate::drivers::hda::play;
    pub const LIVE: bool = true;
    pub fn open(path: &str) -> Result<(), String> { play::open_player(path) }
    pub fn stop() { play::stop_all() }
    pub fn busy() -> bool { play::busy() }
    pub fn pause(on: bool) -> Option<bool> { play::pause(on) }
    pub fn run_bit() -> bool { play::run_bit() }
    pub fn seek(path: &str, ms: u64) -> Result<(u64, &'static str), String> { play::seek_to(path, ms) }
    pub fn position_ms() -> u64 { play::position_ms() }
    pub fn facts(path: &str) -> Option<(u32, u16, u16, Option<u64>, &'static str)> { play::facts(path) }
    pub fn pump_until(ms: u64, f: impl Fn() -> bool) -> bool { play::pump_until(ms, f) }
    pub fn fixture_wav() -> alloc::vec::Vec<u8> { play::fixture_wav() }
}

#[cfg(not(all(target_arch = "x86_64", feature = "hda-tone")))]
mod hw {
    use alloc::string::String;
    pub const LIVE: bool = false;
    pub fn open(_: &str) -> Result<(), String> { Err(String::from("no audio in this build (UNAOS_HDA+UNAOS_HDATONE arm it)")) }
    pub fn stop() {}
    pub fn busy() -> bool { false }
    pub fn pause(_: bool) -> Option<bool> { None }
    pub fn run_bit() -> bool { false }
    pub fn seek(_: &str, _: u64) -> Result<(u64, &'static str), String> { Err(String::from("no audio in this build")) }
    pub fn position_ms() -> u64 { 0 }
    pub fn facts(_: &str) -> Option<(u32, u16, u16, Option<u64>, &'static str)> { None }
    pub fn pump_until(_: u64, f: impl Fn() -> bool) -> bool { f() }
    pub fn fixture_wav() -> alloc::vec::Vec<u8> { alloc::vec::Vec::new() }
}

#[inline]
fn px(n: usize) -> usize {
    crate::ui::px(n)
}

pub fn is_open() -> bool {
    WIN.load(Ordering::Relaxed) != wm::WIN_NONE
}

pub fn shown() -> String {
    STATE.lock().as_ref().map(|s| s.path.clone()).unwrap_or_default()
}

/// Latch `path` for [`service`] (click-router safe). Quarry's `play` opener.
pub fn request_open(path: &str) {
    *PENDING.lock() = Some(String::from(path)); PENDING_POSTED.post(); // SVCLATCH (B462)
}

/// F7 (`0`), F8 (`1`), F9 (`2`) went down. Atomics only (the HID service's context).
pub fn media_key(k: u8) {
    if k <= 2 {
        MEDIA.store(k + 1, Ordering::Release);
    }
}

fn leaf(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn mmss(ms: u64) -> String {
    let s = ms / 1000;
    alloc::format!("{}:{:02}", s / 60, s % 60)
}

/// ATTRCOLUMNS' facts on the file, when the volume carries them: `(duration_ms, codec)`.
fn attr_facts(path: &str) -> (Option<u64>, Option<String>) {
    use crate::fs::vfs::{AttrValue, KERNEL_PRINCIPAL};
    let mt = crate::shell::vfs_mount_table();
    let d = match mt.get_attr(path, crate::fs::attrfacts::DURATION, KERNEL_PRINCIPAL) {
        Ok(AttrValue::Int(v)) if v > 0 => Some(v as u64),
        _ => None,
    };
    let c = match mt.get_attr(path, crate::fs::attrfacts::CODEC, KERNEL_PRINCIPAL) {
        Ok(AttrValue::Str(s)) if !s.is_empty() => Some(s),
        _ => None,
    };
    (d, c)
}

/// Fill the info line: the attributes first, the decoder's own count for whatever they lack. `true` once known.
fn learn(st: &mut State) -> bool {
    if st.src != "pending" {
        return true;
    }
    let (ad, ac) = attr_facts(&st.path);
    let f = hw::facts(&st.path);
    if let Some((rate, ch, _bits, frames, codec)) = f {
        st.detail = alloc::format!("{} Hz, {}", rate, if ch == 1 { String::from("mono") } else if ch == 2 { String::from("stereo") } else { alloc::format!("{} ch", ch) });
        let dd = frames.map(|n| n * 1000 / rate.max(1) as u64);
        st.dur_ms = ad.or(dd).unwrap_or(0);
        st.codec = ac.clone().unwrap_or_else(|| String::from(codec));
        st.src = if ad.is_some() && ac.is_some() { "attrs" } else if ad.is_some() || ac.is_some() { "attrs+decoder" } else { "decoder" };
    } else if let (Some(d), Some(c)) = (ad, ac) {
        st.dur_ms = d;
        st.codec = c;
        st.src = "attrs";
    } else {
        return false;
    }
    serial_println!("[player] info path={} codec={} duration_ms={} {} src={}", st.path, st.codec, st.dur_ms, st.detail, st.src);
    true
}

/// **Open `path` in the Player and play it.** Replaces an open player. The window opens even when the play is
/// refused (the reason is its info line).
pub fn open(path: &str) -> Result<u32, String> {
    let pi = crate::video::panel_info_nonblocking().ok_or_else(|| String::from("panel busy"))?;
    let (pw, ph) = (pi.width, pi.height);
    let w = px(W_L).min(pw.saturating_sub(2 * wm::BORDER()).max(1));
    let video = vid::wants(path); // VIDEOPLAYER (B434): a video gets the picture rect above the transport
    let h = px(H_L).min(ph.saturating_sub(wm::TITLE_H() + 2 * wm::BORDER()).max(1));
    let h = if video { (h + px(VH_L)).min(ph.saturating_sub(wm::TITLE_H() + 2 * wm::BORDER() + crate::ui_status::chrome_h(ph) + crate::ui_status::top_chrome_h(pw, ph)).max(h)) } else { h };
    let oy = if video { h.saturating_sub(px(H_L)) } else { 0 };
    let len = w * h;
    let mut surf: Vec<u32> = Vec::new();
    if surf.try_reserve_exact(len).is_err() {
        return Err(String::from("out of memory"));
    }
    surf.resize(len, theme::content_fill());
    surf[..oy * w].fill(0);
    if is_open() || STATE.lock().is_some() {
        close("replace");
    }
    let err = if video { vid::start(path); None } else { hw::open(path).err() }; // VIDEOPLAYER: the video job first; its sound opens when the container names one

    let mut st = State {
        path: String::from(path),
        name: String::from(leaf(path)),
        codec: String::new(),
        detail: String::new(),
        dur_ms: 0,
        src: "pending",
        base_ms: 0,
        shown_ms: 0,
        playing: err.is_none(),
        ended: false,
        err,
        drag: Drag::None,
        drag_ms: 0,
        drag_samples: 0,
        drag_from: 0,
        w,
        h,
        surf,
        oy,
        video,
        vsaid: false,
    };
    if !video {
        let _ = learn(&mut st);
    }
    paint(&mut st);
    let (_s, ow, oh) = wm::spawn_geometry_native(w, h).ok_or_else(|| String::from("geometry unavailable"))?;
    let wtop = crate::ui_status::top_chrome_h(pw, ph);
    let ox = pw.saturating_sub(ow) / 2;
    let oy = wtop + ph.saturating_sub(wtop).saturating_sub(crate::ui_status::chrome_h(ph)).saturating_sub(oh) / 3;
    let base = st.surf.as_ptr() as usize;
    let title = alloc::format!("Player - {}", st.name);
    let id = wm::create_at_native(OWNER, base, len * 4, w as u32, h as u32, (w * 4) as u32, title.as_bytes(), ox + wm::BORDER(), oy + wm::TITLE_H() + wm::BORDER());
    if id == wm::WIN_NONE {
        hw::stop();
        return Err(String::from("window create failed"));
    }
    let (lv, muted) = crate::video::status::volume();
    serial_println!(
        "[player] open win={} path={} codec={} duration_ms={} src={} vol={}/16 muted={} play={}",
        id, path, if st.codec.is_empty() { "-" } else { &st.codec }, st.dur_ms, st.src, lv, muted as u8,
        match &st.err { None => String::from("started"), Some(e) => alloc::format!("refused ({})", e) }
    );
    *STATE.lock() = Some(st);
    WIN.store(id, Ordering::Relaxed);
    wm::winid_register_holder(&WIN, "player");
    wm::focus_changed(OWNER);
    let _ = wm::present(id);
    Ok(id)
}

/// Close the window (if any) and stop the play. `by` names the door on the wire.
pub fn close(by: &str) {
    let id = WIN.swap(wm::WIN_NONE, Ordering::Relaxed);
    let had = STATE.lock().take();
    if let Some(st) = had.as_ref() {
        if st.drag != Drag::None {
            super::capture::cancel();
        }
    }
    if had.is_none() && id == wm::WIN_NONE {
        return;
    }
    hw::stop();
    vid::stop(); // VIDEOPLAYER (B434): the picture job ends with the window
    if id != wm::WIN_NONE {
        wm::close(id);
    }
    let stops = !hw::busy() && !hw::run_bit();
    serial_println!("[player] closed win={} by={} stops={}", id, by, stops as u8);
    drop(had); // the surface goes after the row stops naming it
}

/// The desktop pass (chained from `quarry::live::service`): the latch, a close the WM made, the media keys, the
/// clock and the end of the play.
pub fn service() {
    ensure_registered();
    let want = PENDING_POSTED.take(&PENDING); // SVCLATCH (B462): no lock on a quiet pass
    if let Some(p) = want {
        match open(&p) {
            Ok(_) => serial_println!("[quarry] open PLAY consumed=player path={}", p),
            Err(e) => serial_println!("[quarry] open PLAY consumed=refused path={} reason={}", p, e),
        }
    }
    // the WM closed the row (Cmd-W, File > Close Window, Quit): its holder cleared WIN — the play stops with it
    if WIN.load(Ordering::Relaxed) == wm::WIN_NONE && STATE.try_lock().map(|g| g.is_some()).unwrap_or(false) {
        close("wm");
    }
    let k = MEDIA.swap(0, Ordering::AcqRel);
    if k != 0 {
        media(k);
    }
    let a = ACT_POSTED.take(&ACT); // SVCLATCH (B462)
    match a {
        Some(Act::Toggle) => toggle(),
        Some(Act::Seek(ms)) => {
            let _ = seek(ms);
        }
        None => {}
    }
    tick();
}

/// F7/F8/F9 on the player (BEZEL shows the play/pause it lands on).
fn media(k: u8) {
    let name = match k {
        1 => "prev",
        2 => "playpause",
        _ => "next",
    };
    if !is_open() {
        serial_println!("[player] key media={} -> none (no player open)", name);
        return;
    }
    let act = match k {
        1 => {
            seek(0);
            "seek0"
        }
        2 => {
            toggle();
            if playing() { "play" } else { "pause" }
        }
        _ => {
            to_end();
            "end"
        }
    };
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    crate::video::bezel::arm_transport(playing());
    serial_println!("[player] key media={} -> {}", name, act);
}

fn playing() -> bool {
    STATE.lock().as_ref().map(|s| s.playing).unwrap_or(false)
}

/// The clock: elapsed from the ring, the end of the play, a repaint when the shown second changes.
fn tick() {
    let id = WIN.load(Ordering::Relaxed);
    if id == wm::WIN_NONE {
        return;
    }
    if is_video() {
        return vid::tick(); // VIDEOPLAYER (B434): the picture's clock and present
    }
    let busy = hw::busy();
    let pos = if busy { hw::position_ms() } else { 0 };
    let Some(mut g) = STATE.try_lock() else { return };
    let Some(st) = g.as_mut() else { return };
    let mut dirty = false;
    if st.src == "pending" && learn(st) {
        dirty = true;
    }
    if st.playing && !busy && st.err.is_none() {
        st.playing = false;
        st.ended = true;
        serial_println!("[player] ended path={} at_ms={}", st.path, st.dur_ms.max(st.shown_ms));
        dirty = true;
    }
    let el = if st.drag == Drag::Scrub {
        st.drag_ms
    } else if st.ended {
        st.dur_ms.max(st.shown_ms)
    } else if pos == u64::MAX || !busy {
        st.shown_ms
    } else {
        let e = st.base_ms + pos;
        if st.dur_ms > 0 { e.min(st.dur_ms) } else { e }
    };
    if el / 1000 != st.shown_ms / 1000 || el * 400 / st.dur_ms.max(1) != st.shown_ms * 400 / st.dur_ms.max(1) {
        dirty = true;
    }
    st.shown_ms = el;
    if dirty {
        paint(st);
        drop(g);
        let _ = wm::present(id);
    }
}

fn repaint() {
    let id = WIN.load(Ordering::Relaxed);
    if let Some(st) = STATE.lock().as_mut() {
        paint(st);
    }
    if id != wm::WIN_NONE {
        let _ = wm::present(id);
    }
}

/// Play/pause (Space, the button, F8). An ended or refused play starts again from the top.
fn toggle() {
    if is_video() {
        return vid::toggle(); // VIDEOPLAYER (B434)
    }
    let (path, playing, ended, err) = match STATE.lock().as_ref() {
        Some(s) => (s.path.clone(), s.playing, s.ended, s.err.is_some()),
        None => return,
    };
    if ended || err || (!hw::busy() && !hw::run_bit()) {
        let r = hw::open(&path);
        if let Some(s) = STATE.lock().as_mut() {
            s.base_ms = 0;
            s.shown_ms = 0;
            s.ended = false;
            s.playing = r.is_ok();
            s.err = r.err();
        }
    } else {
        let _ = hw::pause(playing);
        if let Some(s) = STATE.lock().as_mut() {
            s.playing = !playing;
        }
    }
    repaint();
}

/// Seek to `ms` (clamped to the duration); the play resumes there.
fn seek(ms: u64) -> Option<(u64, &'static str)> {
    if is_video() {
        return vid::seek(ms); // VIDEOPLAYER (B434): 0 restarts; elsewhere owed (SEEKTABLE B433)
    }
    let (path, dur) = match STATE.lock().as_ref() {
        Some(s) => (s.path.clone(), s.dur_ms),
        None => return None,
    };
    let ms = if dur > 0 { ms.min(dur.saturating_sub(1)) } else { ms };
    let r = hw::seek(&path, ms);
    let out = r.as_ref().ok().copied();
    if let Some(s) = STATE.lock().as_mut() {
        match r {
            Ok((landed, _)) => {
                s.base_ms = landed;
                s.shown_ms = landed;
                s.playing = true;
                s.ended = false;
                s.err = None;
            }
            Err(e) => {
                serial_println!("[player] seek to_ms={} refused reason={}", ms, e);
                s.err = Some(e);
                s.playing = false;
            }
        }
    }
    repaint();
    out
}

/// F9 / "next" with one file: the play ends here.
fn to_end() {
    hw::stop();
    vid::stop();
    if let Some(s) = STATE.lock().as_mut() {
        s.playing = false;
        s.ended = true;
        s.shown_ms = s.dur_ms;
    }
    repaint();
}

/// The volume slider / mute glyph: the one model (`status::set_volume`, the amp BEZEL reads back).
fn set_volume(level: u8, muted: bool) -> bool {
    crate::video::status::set_volume(level.min(16), muted)
}

// ── geometry and hit tests (window-local physical px) ──────────────────────────────────────────────────────

fn scrub_x(w: usize) -> (usize, usize) {
    (px(SCRUB_PAD), w.saturating_sub(px(SCRUB_PAD)).max(px(SCRUB_PAD) + 1))
}

fn vol_x() -> (usize, usize) {
    (px(VOL_X0), px(VOL_X1))
}

fn frac_at(x: usize, x0: usize, x1: usize, n: u64) -> u64 {
    let x = x.clamp(x0, x1);
    (x - x0) as u64 * n / (x1 - x0).max(1) as u64
}

/// Panel point → window-local physical px, when it lies in the content.
fn local(x: i32, y: i32) -> Option<(usize, usize)> {
    let id = WIN.load(Ordering::Relaxed);
    let info = wm::info(id)?;
    let sc = info.scale.max(1);
    let lx = (x - info.x as i32).max(0) as usize / sc;
    let ly = (y - info.y as i32).max(0) as usize / sc;
    Some((lx, ly))
}

/// Pointer: the close box, the button, the scrubber, the mute glyph, the volume slider; a raise elsewhere.
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
        close("closebox");
        return true;
    }
    let Some(info) = wm::info(id) else { return false };
    if x < info.x as i32 || y < info.y as i32 {
        return false;
    }
    let Some((lx, ly)) = local(x, y) else { return false };
    if lx >= info.w || ly >= info.h {
        return false;
    }
    wm::focus_changed(OWNER);
    let (w, dur, oy) = match STATE.lock().as_ref() {
        Some(s) => (s.w, s.dur_ms, s.oy),
        None => return true,
    };
    if ly < oy {
        serial_println!("[player] press video -> {}", if playing() { "pause" } else { "play" }); // VIDEOPLAYER (B434): a click on the picture is play/pause
        post(Act::Toggle);
        return true;
    }
    let ly = ly - oy;
    let (s0, s1) = scrub_x(w);
    let (v0, v1) = vol_x();
    if lx >= px(BTN_X) && lx < px(BTN_X + BTN_D) && ly >= px(BTN_Y) && ly < px(BTN_Y + BTN_D) {
        serial_println!("[player] press button -> {}", if playing() { "pause" } else { "play" });
        post(Act::Toggle);
    } else if ly + px(12) >= px(SCRUB_Y) && ly < px(SCRUB_Y + 14) && lx + px(8) >= s0 && lx < s1 + px(8) && dur > 0 {
        let ms = frac_at(lx, s0, s1, dur);
        if let Some(s) = STATE.lock().as_mut() {
            s.drag = Drag::Scrub;
            s.drag_ms = ms;
            s.drag_from = s.shown_ms;
            s.drag_samples = 0;
        }
        super::capture::begin(scrub_motion, scrub_release);
        repaint();
    } else if ly + px(12) >= px(CTRL_Y) && ly < px(CTRL_Y + 12) && lx >= px(MUTE_X) && lx < px(MUTE_X + 24) {
        let (lv, m) = crate::video::status::volume();
        let wrote = set_volume(lv, !m);
        serial_println!("[player] mute on={} level={}/16 amp_written={}", (!m) as u8, lv, wrote as u8);
        repaint();
    } else if ly + px(12) >= px(CTRL_Y) && ly < px(CTRL_Y + 12) && lx + px(8) >= v0 && lx < v1 + px(8) {
        let lv = frac_at(lx, v0, v1, 16) as u8;
        let from = crate::video::status::volume().0;
        if let Some(s) = STATE.lock().as_mut() {
            s.drag = Drag::Vol;
            s.drag_from = from as u64;
            s.drag_samples = 0;
        }
        set_volume(lv, false);
        super::capture::begin(vol_motion, vol_release);
        repaint();
    }
    true
}

fn scrub_motion(x: i32, y: i32) {
    let Some((lx, _)) = local(x, y) else { return };
    let changed = match STATE.lock().as_mut() {
        Some(s) if s.drag == Drag::Scrub => {
            let (s0, s1) = scrub_x(s.w);
            let ms = frac_at(lx, s0, s1, s.dur_ms);
            s.drag_samples += 1;
            let c = ms != s.drag_ms;
            s.drag_ms = ms;
            c
        }
        _ => false,
    };
    if changed {
        repaint();
    }
}

fn scrub_release(x: i32, y: i32) {
    scrub_motion(x, y);
    let (to, from, n) = match STATE.lock().as_mut() {
        Some(s) if s.drag == Drag::Scrub => {
            s.drag = Drag::None;
            (s.drag_ms, s.drag_from, s.drag_samples)
        }
        _ => return,
    };
    serial_println!("[player] scrub drag samples={} from_ms={} to_ms={}", n, from, to);
    post(Act::Seek(to));
}

fn vol_motion(x: i32, y: i32) {
    let Some((lx, _)) = local(x, y) else { return };
    let lv = {
        let mut g = STATE.lock();
        let Some(s) = g.as_mut().filter(|s| s.drag == Drag::Vol) else { return };
        s.drag_samples += 1;
        let (v0, v1) = vol_x();
        frac_at(lx, v0, v1, 16) as u8
    };
    if crate::video::status::volume() != (lv, false) {
        set_volume(lv, false);
        repaint();
    }
}

fn vol_release(x: i32, y: i32) {
    vol_motion(x, y);
    let (from, n) = match STATE.lock().as_mut() {
        Some(s) if s.drag == Drag::Vol => {
            s.drag = Drag::None;
            (s.drag_from, s.drag_samples)
        }
        _ => return,
    };
    let (lv, m) = crate::video::status::volume();
    serial_println!("[player] volume drag samples={} from={}/16 to={}/16 muted={} {}", n, from, lv, m as u8, amp_says(lv, m));
    repaint();
}

/// The amp's readback against the model, as BEZEL reads it: `src=amp agrees=<0|1>` or `src=model`.
fn amp_says(lv: u8, m: bool) -> String {
    #[cfg(all(target_arch = "x86_64", feature = "hda"))]
    if let Some((_alv, am, gain, steps)) = crate::drivers::hda::vol_readback() {
        let agrees = (steps == 0 || gain as u32 == lv.min(16) as u32 * steps as u32 / 16) && am == m;
        return alloc::format!("src=amp agrees={}", agrees as u8);
    }
    let _ = (lv, m);
    String::from("src=model")
}

/// Keys while the Player holds focus: Space play/pause, ←/→ five seconds.
pub fn key_route(ev: crate::pal::Event) -> bool {
    if !is_open() || wm::focus_asid() != OWNER {
        return false;
    }
    match ev {
        crate::pal::Event::Key(b' ') => {
            post(Act::Toggle);
            true
        }
        crate::pal::Event::Key(0x1D) | crate::pal::Event::Key(0x1C) => {
            let back = matches!(ev, crate::pal::Event::Key(0x1D));
            let cur = STATE.lock().as_ref().map(|s| s.shown_ms).unwrap_or(0);
            post(Act::Seek(if back { cur.saturating_sub(STEP_MS) } else { cur + STEP_MS }));
            true
        }
        _ => false,
    }
}

// ── the paint ──────────────────────────────────────────────────────────────────────────────────────────────

fn fill(st: &mut State, x: usize, y: usize, w: usize, h: usize, c: u32) {
    let y = y + st.oy; // VIDEOPLAYER (B434): the transport paints below the picture
    for yy in y..(y + h).min(st.h) {
        let row = yy * st.w;
        for xx in x..(x + w).min(st.w) {
            st.surf[row + xx] = c;
        }
    }
}

fn disc(st: &mut State, cx: usize, cy: usize, r: usize, c: u32) {
    let cy = cy + st.oy;
    let r2 = (r * r) as i64;
    for yy in cy.saturating_sub(r)..(cy + r + 1).min(st.h) {
        for xx in cx.saturating_sub(r)..(cx + r + 1).min(st.w) {
            let (dx, dy) = (xx as i64 - cx as i64, yy as i64 - cy as i64);
            if dx * dx + dy * dy <= r2 {
                st.surf[yy * st.w + xx] = c;
            }
        }
    }
}

/// A right-pointing triangle in the box `(x, y, s, s)`.
fn wedge(st: &mut State, x: usize, y: usize, s: usize, c: u32) {
    for r in 0..s {
        let half = s / 2;
        let d = if r <= half { r } else { s - 1 - r };
        let len = d * s / half.max(1);
        fill(st, x, y + r, len.min(s), 1, c);
    }
}

fn text(st: &mut State, x: usize, y: usize, s: &str, ink: u32, bold: bool) -> usize {
    let (w, h) = (st.w, st.h);
    super::text::draw_text(&mut st.surf, w, w.saturating_sub(px(8)), h, x, y + st.oy, s.as_bytes(), ink, bold, super::text::Face::Ui)
}

fn slider(st: &mut State, x0: usize, x1: usize, y: usize, done: usize, knob: bool) {
    let th = px(4).max(2);
    fill(st, x0, y - th / 2, x1 - x0, th, theme::scroll_thumb());
    fill(st, x0, y - th / 2, done.min(x1) - x0, th, theme::accent());
    if knob {
        disc(st, done, y, px(8), theme::frame_line());
        disc(st, done, y, px(7), KNOB_EDGE);
        disc(st, done, y, px(4), theme::accent());
    }
}

fn paint(st: &mut State) {
    let o = st.oy * st.w; // VIDEOPLAYER (B434): the picture rect above `oy` is the job's, never cleared here
    for p in st.surf[o..].iter_mut() {
        *p = theme::content_fill();
    }
    let (w, h) = (st.w, st.h);
    // the opener's APPRES icon (the player block), a plain tile when the registrar is busy this frame
    if !crate::fs::appres::blit_key_icon(&mut st.surf, w, h, px(16), px(14) + st.oy, px(ICON_L), "player") {
        fill(st, px(16), px(14), px(ICON_L), px(ICON_L), theme::accent());
    }
    let name = st.name.clone();
    text(st, px(TEXT_X), px(16), &name, theme::content_text(), true);
    let info = if let Some(e) = &st.err {
        alloc::format!("cannot play: {}", e)
    } else if st.src == "pending" {
        String::from("opening...")
    } else {
        let mut s = alloc::format!("{}   {}", st.codec.to_ascii_uppercase(), st.detail);
        if st.dur_ms > 0 {
            s.push_str(&alloc::format!("   {}", mmss(st.dur_ms)));
        }
        s
    };
    text(st, px(TEXT_X), px(42), &info, DIM_TEXT, false);
    // the scrubber and the times
    let el = st.shown_ms;
    let (s0, s1) = scrub_x(w);
    let done = s0 + if st.dur_ms > 0 { ((s1 - s0) as u64 * el.min(st.dur_ms) / st.dur_ms) as usize } else { 0 };
    slider(st, s0, s1, px(SCRUB_Y), done, st.dur_ms > 0);
    let ty = px(SCRUB_Y).saturating_sub(px(8));
    text(st, px(16), ty, &mmss(el), theme::content_text(), false);
    let rem = alloc::format!("-{}", mmss(st.dur_ms.saturating_sub(el)));
    text(st, s1 + px(10), ty, &rem, theme::content_text(), false);
    // play / pause
    let (bx, by, bd) = (px(BTN_X), px(BTN_Y), px(BTN_D));
    disc(st, bx + bd / 2, by + bd / 2, bd / 2, theme::frame_line());
    disc(st, bx + bd / 2, by + bd / 2, bd / 2 - px(1).max(1), theme::button_face());
    if st.playing {
        let (gw, gh) = (px(5), px(16));
        fill(st, bx + bd / 2 - px(7), by + (bd - gh) / 2, gw, gh, theme::button_text());
        fill(st, bx + bd / 2 + px(2), by + (bd - gh) / 2, gw, gh, theme::button_text());
    } else {
        let g = px(16);
        wedge(st, bx + (bd - g) / 2 + px(2), by + (bd - g) / 2, g, theme::button_text());
    }
    // the state word beside the button
    let word = if st.err.is_some() { "stopped" } else if st.ended { "ended" } else if st.playing { "playing" } else { "paused" };
    text(st, px(BTN_X + BTN_D + 12), px(CTRL_Y - 8), word, DIM_TEXT, false);
    // mute glyph: a speaker, crossed when muted
    let (lv, muted) = crate::video::status::volume();
    let (mx, my) = (px(MUTE_X), px(CTRL_Y));
    fill(st, mx, my - px(4), px(6), px(8), theme::button_text());
    for i in 0..px(8) {
        fill(st, mx + px(6) + i, my - px(4) - i, 1, px(8) + 2 * i, theme::button_text());
    }
    if muted {
        for i in 0..px(10) {
            fill(st, mx + px(15) + i, my - px(5) + i, px(2), px(2), theme::ctrl_close());
            fill(st, mx + px(15) + i, my + px(5) - i, px(2), px(2), theme::ctrl_close());
        }
    }
    // volume slider: the model's 0..16, the scale BEZEL's segments and the amp readback share
    let (v0, v1) = vol_x();
    let vd = v0 + (v1 - v0) * if muted { 0 } else { lv.min(16) as usize } / 16;
    slider(st, v0, v1, px(CTRL_Y), vd, true);
}

// ── `tests player` (R80: typed; registered from the desktop pass, never run at boot) ─────────────────────────

fn ensure_registered() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, Ordering::AcqRel) {
        crate::tests::register("player", fixture);
        vid::register(); // VIDEOPLAYER (B434): `tests videoplayer`
    }
}

/// `tests player` on TEST.WAV (staged in `/system/test-f`, else the user's home; else the PLAYWAV fixture body
/// written to a scratch file): open → the window and a running stream; transport → pause clears RUN, play sets
/// it again; seek → 1000 ms lands through the table (`method=table`, exact) and runs; volume → the slider's level is the one model and the amp
/// reads it back; close → the window and the play are gone.
pub fn fixture() {
    if !hw::LIVE {
        serial_println!(":: PLAYER: reason=no-audio-in-this-build -> SKIP ::");
        return;
    }
    let mt = crate::shell::vfs_mount_table();
    let p = crate::fs::vfs::KERNEL_PRINCIPAL;
    let mut cands: Vec<String> = Vec::new();
    if let Some(c) = crate::fs::volumes::testf_find(&mt, "TEST.WAV") {
        cands.push(c);
    }
    #[cfg(feature = "login")]
    {
        let mut b = [0u8; crate::fs::users::NAME_MAX];
        if let Some(n) = crate::fs::users::whoami(&mut b) {
            if let Ok(name) = core::str::from_utf8(&b[..n]) {
                cands.push(alloc::format!("/home/{}/TEST.WAV", name));
            }
        }
    }
    cands.push(String::from("/home/TEST.WAV"));
    let mut scratch: Option<String> = None;
    let path = match cands.iter().find(|c| mt.stat(c).is_ok()) {
        Some(c) => c.clone(),
        None => {
            let body = hw::fixture_wav();
            let mut got = None;
            for c in ["/home/PLAYERFX.WAV", "/PLAYERFX.WAV"] {
                let _ = mt.unlink(c, p);
                if mt.create(c, crate::fs::vfs::NodeKind::File, p).is_err() {
                    continue;
                }
                let mut off = 0usize;
                while off < body.len() {
                    match mt.write(c, off as u64, &body[off..(off + 4096).min(body.len())], p) {
                        Ok(n) if n > 0 => off += n,
                        _ => break,
                    }
                }
                if off == body.len() {
                    got = Some(String::from(c));
                    break;
                }
                let _ = mt.unlink(c, p);
            }
            match got {
                Some(c) => {
                    scratch = Some(c.clone());
                    c
                }
                None => {
                    serial_println!(":: PLAYER: reason=no-TEST.WAV-and-no-writable-path -> FAIL ::");
                    return;
                }
            }
        }
    };
    // open: the window, the path, a stream that runs
    let opened = open(&path);
    let ran = opened.is_ok() && hw::pump_until(1500, hw::run_bit);
    let open_ok = opened.is_ok() && is_open() && shown() == path && ran;
    // transport: pause clears RUN, play sets it again
    toggle();
    let paused_ok = !playing() && !hw::run_bit() && hw::busy();
    toggle();
    let resumed_ok = playing() && hw::pump_until(1000, hw::run_bit);
    let transport_ok = open_ok && paused_ok && resumed_ok;
    // seek: 1000 ms through the table (a WAV's is its byte offset: exact), and the stream runs from there
    let sk = seek(1000);
    let seek_ok = matches!(sk, Some((l, "table")) if l.abs_diff(1000) <= 1) && hw::pump_until(1000, hw::run_bit)
        && STATE.lock().as_ref().map(|s| s.base_ms == sk.map(|v| v.0).unwrap_or(0)).unwrap_or(false);
    // volume: the slider's level is the model's, and the amp agrees (BEZEL's reading)
    let (l0, m0) = crate::video::status::volume();
    let target = if l0 >= 8 { l0 - 3 } else { l0 + 3 };
    let (v0, v1) = vol_x();
    let x_for = v0 + (v1 - v0) * target as usize / 16;
    if let Some(s) = STATE.lock().as_mut() {
        s.drag = Drag::Vol;
        s.drag_from = l0 as u64;
    }
    // the slider's own arithmetic at that x, then its apply (the press/motion path minus the pointer)
    let lv = frac_at(x_for, v0, v1, 16) as u8;
    set_volume(lv, false);
    if let Some(s) = STATE.lock().as_mut() {
        s.drag = Drag::None;
    }
    let model_ok = crate::video::status::volume() == (target, false) && lv == target;
    let amp = amp_says(target, false);
    let shared = model_ok && (amp == "src=model" || amp.ends_with("agrees=1"));
    set_volume(l0, m0);
    // close: the window goes, the play stops
    close("fixture");
    let close_stops = !is_open() && !hw::busy() && !hw::run_bit() && STATE.lock().is_none();
    if let Some(c) = scratch {
        let _ = mt.unlink(&c, p);
    }
    let (coded, coded_ok) = coded_tables(); // SEEKTABLE2 (B469): Ogg and ADTS seek from their own data
    let ok = open_ok && transport_ok && seek_ok && shared && close_stops && coded_ok;
    let w = |b: bool| if b { "ok" } else { "bad" };
    serial_println!(
        "[player] fixture path={} ran={} paused={} resumed={} seek={:?} vol={}->{} {} ",
        path, ran as u8, paused_ok as u8, resumed_ok as u8, sk, l0, target, amp
    );
    serial_println!(
        ":: PLAYER: open={} transport={} seek={} method={} volume={} close_stops={} coded={} -> {} ::",
        w(open_ok), w(transport_ok), w(seek_ok), sk.map(|v| v.1).unwrap_or("none"), if shared { "shared" } else { "split" }, close_stops as u8, coded,
        if ok { "PASS" } else { "FAIL" }
    );
}

/// XHCIMEDIA (B438): take the pending media latch (`0` none, `1 + key`) — `tests xhcimedia` reads the router's effect.
pub fn media_take() -> u8 {
    MEDIA.swap(0, Ordering::AcqRel)
}

// ── VIDEOPLAYER (rmbp-ledger B434, MACPARITY row 30) — the Player plays video ─────────────────────────────────
// The window above gains a picture rect (the top `oy` rows); the job, the queue and the clock are `video::vplay`'s
// (Stria's fulfiller over demux_core + vp8_core + av1_core, R79). The transport is the audio player's: play/pause
// drives the ring and the picture clock together; a seek in a video is OWED (SEEKTABLE B433) except to 0 (F7).

fn is_video() -> bool {
    STATE.lock().as_ref().map(|s| s.video).unwrap_or(false)
}

#[cfg(not(all(target_arch = "x86_64", feature = "videoplayer")))]
mod vid {
    pub fn wants(_: &str) -> bool { false }
    pub fn start(_: &str) {}
    pub fn stop() {}
    pub fn register() {}
    pub fn tick() {}
    pub fn toggle() {}
    pub fn seek(_: u64) -> Option<(u64, &'static str)> { None }
}

#[cfg(all(target_arch = "x86_64", feature = "videoplayer"))]
mod vid {
    use super::{hw, paint, repaint, wm, Ordering, State, STATE, WIN};
    use crate::video::vplay;
    use alloc::string::String;

    /// A video type (`video/*` by the one type check, `fs::filetype`). I/O: the file's head, on the service pass.
    pub fn wants(path: &str) -> bool {
        crate::fs::filetype::type_of(path).0.starts_with("video/")
    }
    pub fn start(path: &str) {
        vplay::start(path);
    }
    pub fn stop() {
        vplay::stop();
    }
    pub fn register() {
        crate::tests::register("videoplayer", fixture);
    }

    fn restart(st: &mut State) {
        hw::stop();
        vplay::start(&st.path);
        st.vsaid = false;
        st.base_ms = 0;
        st.shown_ms = 0;
        st.ended = false;
        st.playing = true;
        st.err = None;
        let o = st.oy * st.w;
        st.surf[..o].fill(0);
    }

    /// Play/pause (Space, the button, a click on the picture, F8). An ended or refused video starts again.
    pub fn toggle() {
        {
            let mut g = STATE.lock();
            let Some(st) = g.as_mut() else { return };
            if st.ended || st.err.is_some() {
                restart(st);
                serial_println!("[player] video restart path={}", st.path);
            } else {
                let on = st.playing;
                if hw::busy() {
                    let _ = hw::pause(on);
                }
                vplay::pause(on);
                st.playing = !on;
            }
        }
        repaint();
    }

    /// 0 (F7, Home) restarts; any other point is owed until the container's tables land (SEEKTABLE B433).
    pub fn seek(ms: u64) -> Option<(u64, &'static str)> {
        let out = {
            let mut g = STATE.lock();
            let st = g.as_mut()?;
            if ms == 0 {
                restart(st);
                serial_println!("[player] seek to_ms=0 landed_ms=0 method=restart video=1");
                Some((0, "restart"))
            } else {
                serial_println!("[player] seek to_ms={} video=owed (SEEKTABLE B433: keyframe seek rides the container's tables)", ms);
                None
            }
        };
        repaint();
        out
    }

    /// The pass: the container's facts (once — the info line, the glass line, the sound), then the clock and at
    /// most one present.
    pub fn tick() {
        let id = WIN.load(Ordering::Relaxed);
        if id == wm::WIN_NONE {
            return;
        }
        let mut sound: Option<String> = None;
        let mut dirty = false;
        {
            let Some(mut g) = STATE.try_lock() else { return };
            let Some(st) = g.as_mut() else { return };
            if !st.vsaid {
                if let Some(f) = vplay::facts() {
                    st.vsaid = true;
                    st.codec = String::from(f.codec);
                    st.detail = alloc::format!("{}x{}, {} fps, sound {}", f.w, f.h, f.fps(), f.audio);
                    st.dur_ms = f.dur_ms;
                    st.src = "container";
                    serial_println!(
                        "[player] video codec={} size={}x{} fps={} frames={} duration_ms={} audio={} path={}",
                        f.codec, f.w, f.h, f.fps(), f.frames, f.dur_ms, f.audio, st.path
                    );
                    if f.audio_ok && st.playing {
                        sound = Some(st.path.clone());
                    } else {
                        vplay::audio_expected(false);
                    }
                    dirty = true;
                } else if let Some(e) = vplay::err() {
                    st.vsaid = true;
                    serial_println!("[player] video refused path={} reason={}", st.path, e);
                    st.err = Some(e);
                    st.playing = false;
                    dirty = true;
                }
                if dirty {
                    paint(st);
                }
            }
        }
        if let Some(p) = sound {
            let r = hw::open(&p);
            vplay::audio_expected(r.is_ok());
            serial_println!("[player] video sound={} path={}", match &r { Ok(_) => String::from("container"), Err(e) => alloc::format!("none ({})", e) }, p);
        }
        let busy = hw::busy();
        let pos = if busy { hw::position_ms() } else { u64::MAX };
        let Some(mut g) = STATE.try_lock() else { return };
        let Some(st) = g.as_mut() else { return };
        let audio = if busy && st.playing && pos != u64::MAX && pos > 0 { Some(st.base_ms + pos) } else { None };
        let (w, oy) = (st.w, st.oy);
        let (shown, clock) = if st.err.is_none() { vplay::present(&mut st.surf, w, oy, audio, st.playing) } else { (false, st.shown_ms) };
        dirty |= shown;
        let mut controls = false;
        if st.playing && st.vsaid && st.err.is_none() && vplay::finished() && !busy {
            st.playing = false;
            st.ended = true;
            let (dec, pres, drop, sync, smax, clk) = vplay::stats();
            serial_println!(
                "[player] ended path={} at_ms={} frames={} presented={} dropped={} av_sync_ms={} max={} clock={}",
                st.path, clock, dec, pres, drop, sync, smax, clk
            );
            controls = true;
        }
        let el = if st.ended { st.dur_ms.max(st.shown_ms) } else if st.dur_ms > 0 { clock.min(st.dur_ms) } else { clock };
        if el / 1000 != st.shown_ms / 1000 || el * 400 / st.dur_ms.max(1) != st.shown_ms * 400 / st.dur_ms.max(1) {
            controls = true;
        }
        st.shown_ms = el;
        if controls {
            paint(st);
        }
        if dirty || controls {
            drop(g);
            let _ = wm::present(id);
        }
    }

    /// `tests videoplayer` (R80: typed): TEST.WEBM (then TEST.MP4) in the Player to its end, the pass driven here
    /// (the shell holds the render task while a test runs). One witness line per file.
    pub fn fixture() {
        let mt = crate::shell::vfs_mount_table();
        let mut any = false;
        for name in ["TEST.WEBM", "TEST.MP4"] {
            let mut cands: alloc::vec::Vec<String> = alloc::vec::Vec::new();
            if let Some(c) = crate::fs::volumes::testf_find(&mt, name) {
                cands.push(c);
            }
            cands.push(alloc::format!("/home/{}", name));
            let Some(path) = cands.into_iter().find(|c| mt.stat(c).is_ok()) else {
                serial_println!(":: VIDEOPLAYER: path={} reason=no-file (system/test-f or /home) -> SKIP ::", name);
                continue;
            };
            any = true;
            run_one(&path);
        }
        if !any {
            serial_println!(":: VIDEOPLAYER: reason=no-TEST.WEBM-or-TEST.MP4 -> SKIP ::");
        }
    }

    fn run_one(path: &str) {
        let t0 = crate::arch::ms();
        let opened = super::open(path);
        let open_ok = opened.is_ok() && super::is_open() && super::shown() == path;
        let mut limit = 20_000u64;
        loop {
            hw::pump_until(4, || false); // the HDA service (the ring's feed) — the desktop pass is not running
            tick();
            let (vsaid, dur, ended, err) = STATE.lock().as_ref().map(|s| (s.vsaid, s.dur_ms, s.ended, s.err.is_some())).unwrap_or((true, 0, true, true));
            if vsaid && dur > 0 {
                limit = limit.min(dur + 8_000);
            }
            if ended || err || crate::arch::ms().saturating_sub(t0) > limit {
                break;
            }
            crate::arch::sched::sleep_ms(4);
        }
        let f = vplay::facts();
        let (dec, pres, drop, sync, smax, clk) = vplay::stats();
        let err = STATE.lock().as_ref().and_then(|s| s.err.clone());
        let ended = STATE.lock().as_ref().map(|s| s.ended).unwrap_or(false);
        super::close("fixture");
        let frames = f.as_ref().map(|f| f.frames).unwrap_or(0);
        let ok = open_ok && err.is_none() && ended && frames > 0 && dec == frames && pres > 0 && pres + drop == dec;
        serial_println!(
            "[player] video fixture path={} codec={} decoded={} presented={} dropped={} av_sync_max_ms={} ms={}{}",
            path, f.as_ref().map(|f| f.codec).unwrap_or("-"), dec, pres, drop, smax, crate::arch::ms().saturating_sub(t0),
            err.map(|e| alloc::format!(" err={}", e)).unwrap_or_default()
        );
        serial_println!(
            ":: VIDEOPLAYER: open={} frames={} dropped={} fps={} av_sync_ms={} clock={} codec={} -> {} ::",
            if open_ok { "ok" } else { "bad" }, pres, drop, f.as_ref().map(|f| f.fps()).unwrap_or(0), sync, clk,
            f.as_ref().map(|f| f.codec).unwrap_or("-"), if ok { "PASS" } else { "FAIL" }
        );
    }
}

// ── SEEKTABLE2 (rmbp-ledger B469) — `tests player` reads the coded tables (Ogg Vorbis, Ogg Opus, ADTS) ────────────

/// Each staged coded sample through the core's own index (`play::table_probe_*`, on a worker task): its `[player]
/// coded seek` line and the token `ogg:table,opus:table,adts:table` (`:none` = decode-skip, `:-` = not staged).
/// `ok` = every staged one answered from a table.
#[cfg(all(target_arch = "x86_64", feature = "hda-tone"))]
fn coded_tables() -> (String, bool) {
    use crate::drivers::hda::play;
    let mt = crate::shell::vfs_mount_table();
    let mut tok = String::new();
    let mut ok = true;
    for (name, key, want) in [("TEST.OGG", "ogg", "ogg"), ("TEST.OPUS", "opus", "ogg"), ("TEST.AAC", "adts", "adts")] {
        if !tok.is_empty() {
            tok.push(',');
        }
        let Some(path) = crate::fs::volumes::testf_find(&mt, name) else {
            tok.push_str(key);
            tok.push_str(":-");
            continue;
        };
        play::table_probe_start(&path, 200);
        let done = play::pump_until(3000, || play::table_probe_done().is_some());
        let v = play::table_probe_done();
        let good = matches!(v, Some(Ok((t, true, _, _, _))) if t == want);
        match &v {
            Some(Ok((t, e, l, b, s))) => serial_println!("[player] coded seek path={} table={} exact={} to_ms=200 landed_ms={} byte={} sample={}", path, t, *e as u8, l, b, s),
            Some(Err(e)) => serial_println!("[player] coded seek path={} refused ({})", path, e),
            None => serial_println!("[player] coded seek path={} timeout=3000ms done={}", path, done as u8),
        }
        ok &= good;
        tok.push_str(key);
        tok.push_str(if good { ":table" } else { ":none" });
    }
    (tok, ok)
}

#[cfg(not(all(target_arch = "x86_64", feature = "hda-tone")))]
fn coded_tables() -> (String, bool) {
    (String::from("-"), true)
}
