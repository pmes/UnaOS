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
    surf.resize(len, theme::surface_content());
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
    let title = nle_title(&st.path).unwrap_or_else(|| alloc::format!("Player - {}", st.name)); // NLE6KEYSTAB (rmbp-ledger B975): an EDL opens as the NLE, `NLE - <stem>` (file tail)
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
    *STATE.lock() = Some(st); PLAYER_OPEN.store(true, Ordering::Release); // SMALLFIX5 (B480): the posted `player open` flag
    WIN.store(id, Ordering::Relaxed);
    wm::winid_register_holder(&WIN, "player"); nle_tab(id, path); // NLE6KEYSTAB (rmbp-ledger B975): an EDL's window is LISTED in the wc window list (file tail)
    wm::focus_changed(OWNER);
    let _ = wm::present(id);
    Ok(id)
}

/// Close the window (if any) and stop the play. `by` names the door on the wire.
pub fn close(by: &str) {
    let id = WIN.swap(wm::WIN_NONE, Ordering::Relaxed);
    let had = STATE.lock().take(); PLAYER_OPEN.store(false, Ordering::Release); // SMALLFIX5 (B480)
    if let Some(st) = had.as_ref() {
        if st.drag != Drag::None {
            super::capture::cancel();
        }
    }
    if had.is_none() && id == wm::WIN_NONE {
        return;
    }
    hw::stop();
    vid::stop(); edit_close(); // VIDEOPLAYER (B434): the picture job ends with the window. NLE6KEYS (rmbp-ledger B961): the Player's editor state goes with it (file tail)
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
    if WIN.load(Ordering::Relaxed) == wm::WIN_NONE && PLAYER_OPEN.load(Ordering::Acquire) && STATE.try_lock().map(|g| g.is_some()).unwrap_or(false) {
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
    edit_pass(); tick(); // NLE6KEYS (rmbp-ledger B961): a posted editing key, run on the pass (file tail)
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
    if ly < oy && edit_press_route(lx, ly) { return true; } if ly < oy { // NLE6FACEDRAG (rmbp-ledger B975): a press on the EDL's view band begins a drag (file tail)
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
        _ => edit_post(ev), // NLE6KEYS (rmbp-ledger B961): an EDL's editing keys, posted for the pass (file tail)
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
    fill(st, x0, y - th / 2, x1 - x0, th, theme::line_strong());
    fill(st, x0, y - th / 2, done.min(x1) - x0, th, theme::accent());
    if knob {
        disc(st, done, y, px(8), theme::line_subtle());
        disc(st, done, y, px(7), KNOB_EDGE);
        disc(st, done, y, px(4), theme::accent());
    }
}

fn paint(st: &mut State) {
    let o = st.oy * st.w; // VIDEOPLAYER (B434): the picture rect above `oy` is the job's, never cleared here
    for p in st.surf[o..].iter_mut() {
        *p = theme::surface_content();
    }
    let (w, h) = (st.w, st.h);
    // the opener's APPRES icon (the player block), a plain tile when the registrar is busy this frame
    if !crate::fs::appres::blit_key_icon(&mut st.surf, w, h, px(16), px(14) + st.oy, px(ICON_L), "player") {
        fill(st, px(16), px(14), px(ICON_L), px(ICON_L), theme::accent());
    }
    let name = st.name.clone();
    text(st, px(TEXT_X), px(16), &name, theme::text_primary(), true);
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
    text(st, px(16), ty, &mmss(el), theme::text_primary(), false);
    let rem = alloc::format!("-{}", mmss(st.dur_ms.saturating_sub(el)));
    text(st, s1 + px(10), ty, &rem, theme::text_primary(), false);
    // play / pause
    let (bx, by, bd) = (px(BTN_X), px(BTN_Y), px(BTN_D));
    disc(st, bx + bd / 2, by + bd / 2, bd / 2, theme::line_subtle());
    disc(st, bx + bd / 2, by + bd / 2, bd / 2 - px(1).max(1), theme::surface_raised());
    if st.playing {
        let (gw, gh) = (px(5), px(16));
        fill(st, bx + bd / 2 - px(7), by + (bd - gh) / 2, gw, gh, theme::text_primary());
        fill(st, bx + bd / 2 + px(2), by + (bd - gh) / 2, gw, gh, theme::text_primary());
    } else {
        let g = px(16);
        wedge(st, bx + (bd - g) / 2 + px(2), by + (bd - g) / 2, g, theme::text_primary());
    }
    // the state word beside the button
    let word = if st.err.is_some() { "stopped" } else if st.ended { "ended" } else if st.playing { "playing" } else { "paused" };
    text(st, px(BTN_X + BTN_D + 12), px(CTRL_Y - 8), word, DIM_TEXT, false);
    // mute glyph: a speaker, crossed when muted
    let (lv, muted) = crate::video::status::volume();
    let (mx, my) = (px(MUTE_X), px(CTRL_Y));
    fill(st, mx, my - px(4), px(6), px(8), theme::text_primary());
    for i in 0..px(8) {
        fill(st, mx + px(6) + i, my - px(4) - i, 1, px(8) + 2 * i, theme::text_primary());
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
                if mt.put_file(c, &body, p).is_ok() { // VFSPUTSWEEP (B983)
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
                    if let Some(k) = crate::fs::vfs::home_unwritable(&mt, "/home") { serial_println!(":: PLAYER: reason=no-TEST.WAV -> SKIP reason=home-not-writable({}) ::", k); } else { serial_println!(":: PLAYER: reason=no-TEST.WAV-and-no-writable-path -> FAIL ::"); } // QEMUSKIP (B613): a home the volume refuses is SKIP, never FAIL
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
    let (coded, coded_ok, probe_timeout) = coded_tables(); // SEEKTABLE2 (B469): Ogg and ADTS seek from their own data; SMALLFIX5 (B480): a hung probe FAILS, never wedges
    let ok = open_ok && transport_ok && seek_ok && shared && close_stops && coded_ok;
    let w = |b: bool| if b { "ok" } else { "bad" };
    serial_println!(
        "[player] fixture path={} ran={} paused={} resumed={} seek={:?} vol={}->{} {} ",
        path, ran as u8, paused_ok as u8, resumed_ok as u8, sk, l0, target, amp
    );
    serial_println!(
        ":: PLAYER: open={} transport={} seek={} method={} volume={} close_stops={} coded={}{} -> {} ::",
        w(open_ok), w(transport_ok), w(seek_ok), sk.map(|v| v.1).unwrap_or("none"), if shared { "shared" } else { "split" }, close_stops as u8, coded,
        if probe_timeout { " probe=timeout" } else { "" }, if ok { "PASS" } else { "FAIL" }
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
        vplay::is_edl(path) || crate::fs::filetype::type_of(path).0.starts_with("video/") // NLE6 (SR207): an EDL plays in the picture rect
    }
    pub fn start(path: &str) {
        vplay::start(path);
    }
    pub fn stop() {
        vplay::stop();
    }
    pub fn register() {
        crate::tests::register("videoplayer", fixture);
        crate::tests::register("nle", super::nle_fx::fixture); // NLE6 (SR207): `tests nle`, the EDL mode
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

    /// 0 (F7, Home) restarts. VPLAYAUDIO (B475): any other point seeks the picture (the job's keyframe seek,
    /// `vplay::start_at`) and the sound together — the sound through `play::seek_to` → `play-dec` → `Decoder::seek`
    /// over the job's shared Demuxer (`[play] seek … table=mp4|matroska`); the audio clock resumes from the landing.
    pub fn seek(ms: u64) -> Option<(u64, &'static str)> {
        let (path, sound) = {
            let mut g = STATE.lock();
            let st = g.as_mut()?;
            if ms == 0 {
                restart(st);
                serial_println!("[player] seek to_ms=0 landed_ms=0 method=restart video=1");
                drop(g);
                repaint();
                return Some((0, "restart"));
            }
            let ms = if st.dur_ms > 0 { ms.min(st.dur_ms.saturating_sub(1)) } else { ms };
            let sound = vplay::facts().is_some_and(|f| f.audio_ok);
            hw::stop();
            vplay::start_at(&st.path, ms);
            st.base_ms = ms;
            st.shown_ms = ms;
            st.ended = false;
            st.playing = true;
            st.err = None;
            ((st.path.clone(), ms), sound)
        };
        let (path, ms) = path;
        // the sound after the picture's job is minted: play-dec's first door is the new job's share
        let r = if sound { Some(hw::seek(&path, ms)) } else { None };
        vplay::audio_expected(matches!(r, Some(Ok(_))));
        if let (Some(Ok((landed, _))), Some(st)) = (r.as_ref(), STATE.lock().as_mut()) {
            st.base_ms = *landed;
        }
        serial_println!(
            "[player] seek to_ms={} video=keyframe sound={} path={}",
            ms, match &r { Some(Ok((_, m))) => *m, Some(Err(_)) => "refused", None => "none" }, path
        );
        repaint();
        Some((ms, "keyframe"))
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
        dirty |= shown; let band = if shown || st.err.is_some() { None } else { vplay::band_repaint(&mut st.surf, w, oy) }; // NLE6DRAGSNAP (rmbp-ledger B977): no frame this pass: the EDL's view band repainted when a drag marked it (vplay.rs tail)
        let mut controls = false;
        if st.playing && st.vsaid && st.err.is_none() && vplay::finished() && !busy {
            st.playing = false;
            st.ended = true;
            let (dec, pres, drop, sync, smax, clk) = vplay::stats(); let pu = vplay::present_us(); // KERNELSCALECOLS (rmbp-ledger B978): the present cost and the queue depth
            serial_println!(
                "[player] ended path={} at_ms={} frames={} presented={} dropped={} av_sync_ms={} max={} clock={} present_us={} present_max_us={} queue={} filter={}",
                st.path, clock, dec, pres, drop, sync, smax, clk, pu.0, pu.1, pu.2, pu.3
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
        if let (false, false, Some((y0, y1))) = (dirty, controls, band) { drop(g); let _ = wm::present_rows(id, y0, y1); return; } if dirty || controls { // NLE6DRAGSNAP (rmbp-ledger B977): a band-only repaint presents its rows alone
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
fn coded_tables() -> (String, bool, bool) {
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
        // SMALLFIX5 (B480) item 7: a probe that does not answer in 3 s (or one still hung from an earlier run) ends
        // the walk — no further file read, no further task — and PLAYER prints `probe=timeout` and FAILS.
        if !play::table_probe_start(&path, 200) {
            serial_println!("[player] coded seek path={} probe=timeout reason=earlier-probe-unanswered -> rest skipped", path);
            tok.push_str(key);
            tok.push_str(":skip");
            return (tok, false, true);
        }
        let done = play::pump_until(3000, || play::table_probe_done().is_some());
        let v = play::table_probe_done();
        if v.is_none() {
            serial_println!("[player] coded seek path={} probe=timeout ms=3000 done={} -> rest skipped", path, done as u8);
            tok.push_str(key);
            tok.push_str(":none");
            return (tok, false, true);
        }
        let good = matches!(v, Some(Ok((t, true, _, _, _))) if t == want);
        match &v {
            Some(Ok((t, e, l, b, s))) => serial_println!("[player] coded seek path={} table={} exact={} to_ms=200 landed_ms={} byte={} sample={}", path, t, *e as u8, l, b, s),
            Some(Err(e)) => serial_println!("[player] coded seek path={} refused ({})", path, e),
            None => {} // SMALLFIX5 (B480): answered above (probe=timeout)
        }
        ok &= good;
        tok.push_str(key);
        tok.push_str(if good { ":table" } else { ":none" });
    }
    (tok, ok, false)
}

#[cfg(not(all(target_arch = "x86_64", feature = "hda-tone")))]
fn coded_tables() -> (String, bool, bool) {
    (String::from("-"), true, false)
}

// ── SMALLFIX5 (rmbp-ledger B480) item 2 — the `player open` flag (the SVCLATCH latch shape) ─────────────────────
/// Raised beside the one store of `Some` into [`STATE`], lowered beside its one `take`: the desktop pass's "did the
/// WM close the row" check reads it before `STATE.try_lock()`, so with no Player open the pass takes no lock.
static PLAYER_OPEN: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// `tests smallfix5`: the flag agrees with the state it fronts (`None` when the state is locked by someone else).
pub fn open_flag_agrees() -> Option<bool> {
    let flag = PLAYER_OPEN.load(Ordering::Acquire);
    STATE.try_lock().map(|g| g.is_some() == flag)
}

// ── NLE6 (LEDGER SR207) — `tests nle`: the Player's EDL mode over two synthetic reels ─────────────────────────────
// R80: typed, never at boot; registered beside `tests videoplayer` (`vid::register`, the same gate). The fixture writes
// two utp1 reels through demux_core's own writer (`build::test_pattern_track` + `TestPatternPacket`, video only: the
// kernel has no PCM door yet, NLE6-MIX) and an EDL through timeline_core's writer (`edl::write`) of a cut list with a gap
// (NLE2's synth_fixture shape), opens the EDL in the Player and runs the pass to the end. `agree` compares each presented
// frame's ordinal (`vplay::Frame.ord`: the kernel's utp1 picture is a flat field, the counter rides the frame) with the
// schedule's source tick at that output tick (the job's plan); `seeks` must equal the plan's seek steps and every queued
// frame must be presented or dropped. NLE6MIX (rmbp-ledger B899): the cut list carries a dissolve A->B and a W001 wipe to
// black; every transition tick is mixed (`mixed=8`, none refused) and the two mid-transition frames' FNV-1a 64 equal
// `MIX_KAT`, pinned on the host by timeline_core's `vmix` test through the SAME `vmix::mix_step`. KERNELCOLOUR (rmbp-ledger B952): a third reel, NLEFXC (VP8 with a `colr` of BT.709 limited), at ticks 35..38 converts through yuv_core's `pick` + `to_rgba`; its middle frame's FNV equals `COLOUR_KAT` (`colour=1/1`).
#[cfg(all(target_arch = "x86_64", feature = "videoplayer"))]
mod nle_fx {
    use super::{hw, STATE};
    use crate::video::vplay;
    use alloc::string::String;
    use alloc::vec::Vec;
    use timeline_core::{Clip, EditError, Kind, Timeline, TransitionKind, WipeDir};

    const FPS: u32 = 25;
    /// The reels' frame ordinals start here (a frame names its reel and its index).
    const BASE: [u32; 2] = [1000, 2000];
    const REELS: [&str; 4] = ["NLEFXA", "NLEFXB", "NLEFXC", "NLEFXD"]; // SWAPBACKVPLAY (rmbp-ledger B956): D = an mp4v reel no decoder fits (module tail)
    const FRAMES: u32 = 40;
    /// NLE6MIX (B899): `(output tick, FNV-1a 64 of the mixed 64x48 RGBA)` — tick 11 the dissolve at 2/5 (a flat 42), tick
    /// 22 the wipe to black at 3/5 (columns 0..37 black); timeline_core `vmix::tests::kernel_fixture_pins` re-derives both.
    const MIX_KAT: [(i64, u64); 2] = [(11, 0x565d_af03_7163_1325), (22, 0x575d_e51b_bec6_ed25)];
    /// KERNELCOLOUR (B952): `(output tick, FNV-1a 64 of the 64x48 RGBA)` of NLEFXC's middle frame. Its `colr` says BT.709 limited,
    /// so yuv_core's `pick` converts by BT.709 (the BT.601 size default would give 0xe0cb_8241_1331_0525). Derived on the host by
    /// the arc's kat.py: ffmpeg's (normative) VP8 decode of `NLEFXC_KEY`, COLOURONE's integer kernel (shift 8, nearest chroma), FNV-1a 64.
    const COLOUR_KAT: (i64, u64) = (37, 0x2948_5fdd_7b85_f725);
    /// NLEFXC's one picture: a libvpx keyframe, 64x48, four flat quadrants (Cb 176 | 94 split at luma column 32, rows at 24).
    const NLEFXC_KEY: [u8; 112] = [
        0xf0, 0x05, 0x00, 0x9d, 0x01, 0x2a, 0x40, 0x00, 0x30, 0x00, 0x00, 0x07, 0x08, 0x85, 0x85, 0x88,
        0x85, 0x84, 0x88, 0x00, 0x28, 0x10, 0x13, 0xae, 0xa8, 0x5f, 0xc0, 0x7f, 0x00, 0x79, 0x4a, 0x98,
        0x8f, 0x01, 0xfc, 0x01, 0xfd, 0x15, 0xdc, 0x01, 0x05, 0x12, 0x35, 0xcb, 0xb6, 0x86, 0xd6, 0xb4,
        0x40, 0x6d, 0x27, 0x31, 0x84, 0x56, 0x4b, 0x08, 0x00, 0xfe, 0xf1, 0x94, 0x53, 0xe8, 0x96, 0x9f,
        0xf8, 0x6a, 0x0e, 0x47, 0x76, 0x17, 0xec, 0xc1, 0x54, 0x1e, 0x73, 0x9d, 0xd0, 0x5e, 0xd0, 0x0b,
        0x7f, 0x16, 0x3d, 0x14, 0xa5, 0xfe, 0xea, 0xc0, 0x00, 0x76, 0x4e, 0x4e, 0x20, 0x03, 0x93, 0x2f,
        0xa9, 0x38, 0xff, 0xfe, 0xe6, 0x70, 0x00, 0x0c, 0x1b, 0xb6, 0x58, 0x03, 0x5e, 0x76, 0x2a, 0x00,
    ];

    /// A utp1 reel: `FRAMES` 64x48 test-pattern frames at `FPS`, a keyframe every 10, ordinals `base..`.
    fn reel(base: u32) -> Vec<u8> {
        use demux_core::build;
        let mut video = build::test_pattern_track(1, 64, 48, FPS, FRAMES, 10);
        for (i, s) in video.samples.iter_mut().enumerate() {
            s.data = build::TestPatternPacket { frame: base + i as u32, width: 64, height: 48 }.encode();
        }
        build::mp4(&[video, sound(base)], &build::Mp4Options::default()) // NLE6MIXAUDIO (rmbp-ledger B953): + the reel's PCM sound (module tail)
    }

    /// KERNELCOLOUR (B952): a VP8 reel of 4 keyframes (`NLEFXC_KEY` each) whose `vp08` sample entry carries a `colr` of BT.709 limited.
    fn vp8_reel() -> Vec<u8> {
        use demux_core::build;
        let mut video = build::test_pattern_track(1, 64, 48, FPS, 4, 1);
        video.spec.fourcc = *b"vp08";
        video.spec.config_box = Some(*b"vpcC");
        video.spec.codec_id = "V_VP8";
        // vpcC v1: profile 0, level 1.0, 8-bit 4:2:0 limited, BT.709 primaries / transfer / matrix, no init data
        video.spec.config = alloc::vec![1, 0, 0, 0, 0, 10, 0x82, 1, 1, 1, 0, 0];
        for s in video.samples.iter_mut() {
            s.data = NLEFXC_KEY.to_vec();
        }
        let bt709 = demux_core::colour::Colour { primaries: 1, transfer: 1, matrix: 1, full_range: Some(false), mastering: None, light: None };
        build::mp4_with(&[video], &build::Mp4Options::default(), &build::Mp4Extras { colour: alloc::vec![(1, bt709)], ..Default::default() })
    }

    /// The cut list: A 13..23 at 0, B 0..10 at 10, a gap 20..25, A 23..33 at 25; NLE6MIX: a 4-tick dissolve on the cut at
    /// 10 and a 4-tick W001 wipe to black at 20 (tick 24 stays a gap); KERNELCOLOUR (B952): C 0..4 at 35, after the last cut.
    fn timeline() -> Result<Timeline, EditError> {
        let mut tl = Timeline::new(FPS);
        let a = tl.add_source(REELS[0], FRAMES as i64);
        let b = tl.add_source(REELS[1], FRAMES as i64);
        let c = tl.add_source(REELS[2], 4); let d = tl.add_source(REELS[3], 4); // SWAPBACKVPLAY (rmbp-ledger B956): D 0..4 at 39 (module tail)
        tl.add_track(Kind::Video);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 13, 23, 0), Clip::new(b, 0, 10, 10), Clip::new(a, 23, 33, 25), Clip::new(c, 0, 4, 35), Clip::new(d, 0, 4, 39)]; with_v2(&mut tl, b); // NLE6MIXFACE (rmbp-ledger B957): V2 over V1 before the dissolve (module tail)
        tl.add_transition(0, 10, TransitionKind::Dissolve, 4)?.add_transition_to_black(0, 20, TransitionKind::Wipe(WipeDir::LeftToRight), 4)
    }

    pub fn fixture() {
        let mt = crate::shell::vfs_mount_table();
        let p = crate::fs::vfs::KERNEL_PRINCIPAL;
        let tl = match timeline().map(with_audio) { // NLE6MIXAUDIO (rmbp-ledger B953): + an audio track mirroring the cuts (module tail)
            Ok(t) => t,
            Err(e) => {
                serial_println!(":: NLE: reason=transition ({:?}) -> FAIL ::", e);
                return;
            }
        };
        let edl = match timeline_core::edl::write(&tl, "NLEFX") {
            Ok(t) => t,
            Err(e) => {
                serial_println!(":: NLE: reason=edl-write ({}) -> FAIL ::", e);
                return;
            }
        };
        let files: [(String, Vec<u8>); 5] = [
            (alloc::format!("{}.MP4", REELS[0]), reel(BASE[0])),
            (alloc::format!("{}.MP4", REELS[1]), reel(BASE[1])),
            (alloc::format!("{}.MP4", REELS[2]), vp8_reel()),
            (alloc::format!("{}.MP4", REELS[3]), standin_reel()), // SWAPBACKVPLAY (rmbp-ledger B956)
            (String::from("NLEFX.EDL"), edl.into_bytes()),
        ];
        let put = |c: &str, body: &[u8]| -> bool { mt.put_file(c, body, p).is_ok() }; // NLE6EXPORTONE (rmbp-ledger B963): the ONE VFS whole-file writer (fs/vfs.rs tail)
        let Some(dir) = ["/home", ""].into_iter().find(|d| files.iter().all(|(n, b)| put(&alloc::format!("{}/{}", d, n), b))) else {
            serial_println!(":: NLE: reason=fixture-write (neither /home nor / took the reels) -> SKIP ::");
            return;
        };
        let path = alloc::format!("{}/NLEFX.EDL", dir);
        let t0 = crate::arch::ms(); env_take(true); // NLE6MIXAUDIO (rmbp-ledger B953): the mixer's witness tap armed (module tail)
        let opened = super::open(&path);
        let open_ok = opened.is_ok() && super::is_open() && super::shown() == path;
        loop {
            hw::pump_until(4, || false); // the HDA service, as `tests videoplayer` (the desktop pass is not running)
            super::vid::tick();
            let (ended, err) = STATE.lock().as_ref().map(|s| (s.ended, s.err.is_some())).unwrap_or((true, true));
            if ended || err || crate::arch::ms().saturating_sub(t0) > 20_000 {
                break;
            }
            crate::arch::sched::sleep_ms(4);
        }
        let (_dec, pres, drop, _sync, _smax, _clk) = vplay::stats();
        let shown = vplay::shown();
        let rep = vplay::edl_report().unwrap_or_default();
        let err = STATE.lock().as_ref().and_then(|s| s.err.clone());
        let ended = STATE.lock().as_ref().map(|s| s.ended).unwrap_or(false);
        let face = face_leg(); super::close("fixture"); let env = env_take(false); // NLE6MIXAUDIO (rmbp-ledger B953): the periods mixed during the run
        // the schedule's ordinal at an output tick: the reel's base + the step's source tick
        let want = |t: i64| rep.plan.iter().find(|s| s.0 == t).and_then(|s| s.1).map(|(src, st)| BASE.get(src as usize).copied().unwrap_or(0) + st as u32);
        let agree = shown.iter().filter(|(t, n)| want(*t) == Some(*n)).count();
        let seek_steps = rep.plan.iter().filter(|s| s.2).count() as u64;
        let kat = MIX_KAT.iter().filter(|k| rep.mixes.contains(*k)).count();
        let mix_ok = rep.mixed == 8 && rep.mix_refused == 0 && kat == MIX_KAT.len(); let mix_ok = mix_ok && face_verdict(&rep, face); // NLE6MIXFACE (rmbp-ledger B957): the fresh outgoing run and the seek preview (module tail)
        let colour = rep.coded.iter().filter(|k| **k == COLOUR_KAT).count(); // KERNELCOLOUR (B952): NLEFXC's middle frame by BT.709
        let ok = open_ok && err.is_none() && ended && pres > 0 && !shown.is_empty() && agree == shown.len() && pres + drop == rep.frames && rep.seeks == seek_steps && mix_ok && colour == 1 && standin_ok(&rep); let (env_ok, env_word) = env_verdict(&env, &rep); let ok = ok && env_ok; let (exp_ok, exp_word) = export_verdict(dir); let ok = ok && exp_ok; let (key_ok, key_word) = keys_leg(dir, &put); let ok = ok && key_ok; let aac_ok = export_aac_leg(dir); let ok = ok && aac_ok; let (drag_ok, drag_word) = drag_leg(dir, &put); let ok = ok && drag_ok; // NLE6FACEDRAG (rmbp-ledger B975): the view band's drags through the pointer door and the NLE tab (module tail). NLE6KEYS (rmbp-ledger B961): the editing keys through the key door (module tail). NLE6MIXAUDIO (rmbp-ledger B953): the dissolve's sound (module tail)
        serial_println!(
            "[nle] fixture edl={} presented={} dropped={} queued={} gaps={} seek_steps={} ms={}{}",
            path, pres, drop, rep.frames, rep.gaps, seek_steps, crate::arch::ms().saturating_sub(t0), err.map(|e| alloc::format!(" err={}", e)).unwrap_or_default()
        );
        serial_println!(
            ":: NLE: frames={} seeks={} runup={} agree={}/{} counter=frame-ord mix={} kat={}/{} colour={}/1 env={} xfades={} swaps={} standin={} export={} keys={} drag={} -> {} ::",
            pres, rep.seeks, rep.runup, agree, shown.len(), rep.mixed, kat, MIX_KAT.len(), colour, env_word, rep.xfades, rep.swaps, rep.stand_in_frames, exp_word, key_word, drag_word, if ok { "PASS" } else { "FAIL" }
        );
    }

    // ── NLE6MIXAUDIO (rmbp-ledger B953): the dissolve's sound ──────────────────────────────────────────────────────
    // Each reel carries a mono PCM16 48 kHz track (demux_core's `pcm16_track`, `sowt`: the kernel plays it through
    // audio_core's PCM door): reel A a +-8000 square of period 48, reel B one of period 160 (orthogonal over a 480-frame
    // mixer period). The audio track mirrors the video cuts, so the dissolve at tick 10 is an audio crossfade (NLE3b): B
    // on the track's source rising over A at its handle on the second source. The mixer's witness tap gives each mixed
    // period's mean square; counted from the first audible period (timeline frame 0), the dissolve is periods 40..56
    // (tick 10 = 19200 frames, 4 ticks = 16 periods) and the RMS of periods 41, 43 .. 55 must be `ENV_KAT` (+-2%): the
    // LINEAR curve's dip, pinned on the host by `audio_core::ramp` `tests::kernel_fixture_envelope` through the SAME
    // `ramp::at` and the mixer's arithmetic. No period mixed (no sink) is `skip(no-sink)`; no audio module, `skip(no-audio)`.
    const ENV_KAT: [u32; 8] = [7304, 6503, 5936, 5672, 5754, 6168, 6854, 7741];

    /// Reel `base`'s sound: `FRAMES` frames' worth of a +-8000 square (half period 24 for reel A, 80 for reel B).
    fn sound(base: u32) -> demux_core::build::MediaTrack {
        let half = if base == BASE[0] { 24 } else { 80 };
        let n = FRAMES as usize * 48_000 / FPS as usize;
        let pcm: Vec<i16> = (0..n).map(|i| if (i / half) % 2 == 0 { 8000 } else { -8000 }).collect();
        demux_core::build::pcm16_track(2, 48_000, 1, &pcm, 960)
    }

    /// An audio track whose clips are the video track's (the dissolve then implies the crossfade, NLE3b).
    fn with_audio(mut tl: Timeline) -> Timeline {
        let a = tl.add_track(Kind::Audio);
        tl.tracks[a].clips = tl.tracks[0].clips.clone();
        tl
    }

    #[cfg(any(feature = "uvc", feature = "hda-tone"))]
    fn env_take(arm: bool) -> Vec<u64> {
        crate::audio::mixer::tap(arm)
    }

    #[cfg(not(any(feature = "uvc", feature = "hda-tone")))]
    fn env_take(_: bool) -> Vec<u64> {
        Vec::new()
    }

    /// The envelope's verdict and its wire word (`ok`, `FAIL`, `skip(<why>)`); the detail line always.
    fn env_verdict(env: &[u64], rep: &vplay::EdlReport) -> (bool, &'static str) {
        if cfg!(not(any(feature = "uvc", feature = "hda-tone"))) {
            return (true, "skip(no-audio)");
        }
        let Some(p0) = env.iter().position(|&m| m > 0) else {
            serial_println!("[nle] fixture env periods=0 xfades={} refused={} underruns={}", rep.xfades, rep.snd_refused, rep.snd_underruns);
            return (true, "skip(no-sink)");
        };
        let got: Vec<u32> = (0..ENV_KAT.len()).map(|q| env.get(p0 + 41 + 2 * q).map(|&m| m.isqrt() as u32).unwrap_or(0)).collect();
        let near = got.iter().zip(ENV_KAT.iter()).all(|(&g, &w)| (g as i64 - w as i64).abs() * 50 <= w as i64);
        let ok = near && rep.xfades == 1 && rep.snd_refused == 0 && rep.snd_underruns == 0;
        serial_println!(
            "[nle] fixture env={:?} want={:?} p0={} periods={} xfades={} refused={} underruns={} frames={}",
            got, ENV_KAT, p0, env.len(), rep.xfades, rep.snd_refused, rep.snd_underruns, rep.snd_frames
        );
        (ok, if ok { "ok" } else { "FAIL" })
    }

    // ── NLE6MIXFACE (rmbp-ledger B957): V2 over V1's dissolve — a FRESH outgoing run — and the face's seek preview ───────
    // A transition's outgoing is the previous clip on its own track (timeline_core `vmix_for`). V2 here shows B 30..34 at
    // ticks 6..10, over V1's A: at the cut V1's outgoing A no longer continues the live run (V2's B), so the schedule seeks
    // it and the job opens a fresh outgoing run (`mix_seeks` 1; A from its keyframe 20, the run-up 20..22 unseen). Then the
    // Player's own seek door (`vid::seek`, the scrubber's) lands at tick 10, and the job's first queued frame there must be
    // the MIXED one (vplay `face_preview`). `PREVIEW_KAT` is tick 10, the dissolve at k=0 (t = 1/5, alpha 51): outgoing A
    // ord 1023 (level 51) under incoming B ord 2000 (level 28), a flat 46; host-derived by the arc's kat.py (pixel_core's
    // `blend_nonpremult`, which reproduces `MIX_KAT`'s tick 11) and re-derived by timeline_core `vmix::tests::kernel_fixture_pins`
    // through the SAME `vmix::mix_step`.
    const PREVIEW_KAT: (i64, u64) = (10, 0x575a_f060_ca99_7325);

    /// V2: B 30..34 at 6 (reel NLEFXB again: no new reel), ending at V1's dissolve cut.
    fn with_v2(tl: &mut Timeline, b: timeline_core::SourceId) {
        let v2 = tl.add_track(Kind::Video);
        tl.tracks[v2].clips = alloc::vec![Clip::new(b, 30, 34, 6)];
    }

    /// The face's preview: the Player seeks to `PREVIEW_KAT`'s tick through `vid::seek`; the seek job's first queued frame
    /// (`vplay::face_preview_take`), or `None` when the Player is closed or nothing lands in 5 s.
    fn face_leg() -> Option<(i64, bool, u64)> {
        let _ = vplay::face_preview_take();
        let ms = (PREVIEW_KAT.0 * 1000 / FPS as i64) as u64;
        super::vid::seek(ms)?;
        let t0 = crate::arch::ms();
        loop {
            hw::pump_until(4, || false);
            super::vid::tick();
            if let Some(p) = vplay::face_preview_take() {
                return Some(p);
            }
            if crate::arch::ms().saturating_sub(t0) > 5_000 {
                return None;
            }
            crate::arch::sched::sleep_ms(4);
        }
    }

    /// The verdict: a fresh outgoing run opened (`mix_seeks >= 1`), the play-through mixed `PREVIEW_KAT`, and the seek showed it.
    fn face_verdict(rep: &vplay::EdlReport, face: Option<(i64, bool, u64)>) -> bool {
        let pin = rep.mixes.contains(&PREVIEW_KAT);
        let ok = rep.mix_seeks >= 1 && pin && face == Some((PREVIEW_KAT.0, true, PREVIEW_KAT.1));
        let pv = face.map(|(t, m, f)| alloc::format!("tick={},mixed={},fnv={:x}", t, m as u8, f)).unwrap_or_else(|| String::from("none"));
        serial_println!("[nle] fixture face mix_seeks={} pin={} preview={} -> {}", rep.mix_seeks, pin as u8, pv, if ok { "ok" } else { "FAIL" });
        ok
    }

    // ── SWAPBACKVPLAY (rmbp-ledger B956): the reel no decoder fits ───────────────────────────────────────────────
    // NLEFXD is an `mp4v` reel (MPEG-4 Part 2 Visual: `Codec::Other`, decoded by no core), 64x48, 4 frames, video only, cut at
    // ticks 39..43 after C: the EDL plays on with the labelled stand-in (`vplay::Pic::open`; `swaps=1 standin=4`) where it ended
    // `codec-refused` before. Its tick-40 picture (source tick 1: counter 1) hashes to `STANDIN_KAT`, demux_core's ONE painter's
    // 64x48 frame 1, pinned on the host by `build::tests::stand_in_kat` and by gneiss_pal's `stand_in_paints_the_shared_kat`
    // (Stria's stand-in paints the same bytes). The mid-stream swap has no fixture: no kernel core refuses a stream mid-stream
    // yet (SWAPBACK-VPLAY-MID, handlers.md §5).
    const STANDIN_KAT: (i64, u64) = (40, 0xdb29_ef26_80d9_5484);

    /// NLEFXD: `test_pattern_track` re-labelled `mp4v` with no config box (Stria's `codec_without_decoder_plays_the_labelled_stand_in` shape).
    fn standin_reel() -> Vec<u8> {
        use demux_core::build;
        let mut video = build::test_pattern_track(1, 64, 48, FPS, 4, 1);
        video.spec.fourcc = *b"mp4v";
        video.spec.config_box = None;
        build::mp4(&[video], &build::Mp4Options::default())
    }

    /// The stand-in's verdict: one run went to it, its four pictures were shown, and tick 40 is the shared painter's frame.
    fn standin_ok(rep: &vplay::EdlReport) -> bool {
        rep.swaps == 1 && rep.stand_in_frames == 4 && rep.coded.contains(&STANDIN_KAT)
    }

    // ── NLE6EXPORT (rmbp-ledger B959): the fixture's EDL exported on UnaOS and re-demuxed ─────────────────────────────
    // After the play-through (the Player closed), `nle export`'s door (`video::export::start`) exports NLEFX.EDL with two B
    // pictures to `<dir>/NLEFX.MP4` through vplay's Encode sink, and the file is read back through demux_core: the avc1
    // track's sample count must equal the timeline's ticks and the export's frames, a sample with pts != dts says `ctts` was
    // written, and (with an audio module) a 48 kHz stereo track whose capture covers the ticks. The kernel has no H.264
    // decoder: the container only (NLE6EXPORT-HOSTKAT owed: the host decodes the file through h264_core).
    fn export_verdict(dir: &str) -> (bool, &'static str) {
        use crate::video::export;
        let ticks = timeline().map(with_audio).map(|t| t.end()).unwrap_or(0).max(0) as u64;
        let t0 = crate::arch::ms();
        let (edl, out) = (alloc::format!("{}/NLEFX.EDL", dir), alloc::format!("{}/NLEFX.MP4", dir));
        let jid = match export::start(&edl, &out, export::Opts { bframes: 2, ..export::Opts::default() }) {
            Ok(j) => j,
            Err(e) => {
                serial_println!("[nle] fixture export refused ({}) -> FAIL", e);
                return (false, "FAIL");
            }
        };
        let rep = loop {
            if let Some(r) = export::report(jid) {
                break Some(r);
            }
            if crate::arch::ms().saturating_sub(t0) > 30_000 {
                break None;
            }
            crate::arch::sched::sleep_ms(4);
        };
        let rep = match rep {
            Some(r) if r.err.is_none() => r,
            r => {
                serial_println!("[nle] fixture export out={} err={} -> FAIL", out, r.and_then(|r| r.err).unwrap_or_else(|| String::from("timeout")));
                return (false, "FAIL");
            }
        };
        let d = match vplay::read_all(&out).and_then(|b| demux_core::Demuxer::open(b).map_err(|e| alloc::format!("demux: {}", e))) {
            Ok(d) => d,
            Err(e) => {
                serial_println!("[nle] fixture export out={} reread={} -> FAIL", out, e);
                return (false, "FAIL");
            }
        };
        let vi = d.video_track().and_then(|t| d.track_index(t.id));
        let samples = vi.map(|i| d.track_samples(i).count() as u64).unwrap_or(0);
        let ctts = vi.is_some_and(|i| d.track_samples(i).any(|s| s.pts != s.dts));
        let audio_ok = d.audio_track().is_some_and(|t| t.sample_rate == 48_000 && t.channels == 2) && rep.audio_target > 0 && rep.audio_mixed >= rep.audio_target && rep.periods > 0; // NLE6EXPORTSTREAMPCM (rmbp-ledger B976): the sound entered the file before finish
        let need_audio = cfg!(any(feature = "uvc", feature = "hda-tone"));
        let (col_ok, col_word) = export_colour(&d, vi); let ok = samples == ticks && rep.frames == ticks && ctts && (audio_ok || !need_audio) && bitrate_verdict(dir) && col_ok && mkv_verdict(dir); let sz_ok = size_verdict(dir); let ok = ok && sz_ok; let pol_ok = policies_verdict(dir, sz_ok); let ok = ok && pol_ok; let px_ok = pixels_verdict(dir); let ok = ok && px_ok; // EXPORTSIZEPOLICYPIXEL (rmbp-ledger B998): NLEFXG's exports decoded back (module tail). EXPORTSIZEFIXTUREPOLICIES (rmbp-ledger B993): the same reel under Fill / Stretch / Adopt (module tail). EXPORTSIZEFIXTURE (rmbp-ledger B990): the two-size AV1 reel exported (module tail). NLE6EXPORTMKV (rmbp-ledger B966): the .mkv export (module tail). H264ENCCOLOURHOST (LEDGER SR377): the re-read file's colour (module tail). NLE6EXPORTONE (rmbp-ledger B963): the `--bitrate` export (module tail), behind the first
        serial_println!(
            "[nle] fixture export out={} samples={} ticks={} ctts={} colour={} audio={} mixed={}/{} periods={} bytes={} ms={} -> {}",
            out, samples, ticks, ctts as u8, col_word, audio_ok as u8, rep.audio_mixed, rep.audio_target, rep.periods, rep.bytes, rep.ms, if ok { "ok" } else { "FAIL" }
        );
        (ok, if !ok { "FAIL" } else if need_audio { "ok" } else { "skip(no-audio)" })
    }

    // ── NLE6EXPORTONE (rmbp-ledger B963): `nle export --bitrate 40` over the fixture ─────────────────────────────────────
    // The same EDL exported again at a CBR target through h264enc_core's `Config.rate` (no B pictures) to `<dir>/NLEFXR.MP4`:
    // the video samples' bytes must land within 25% of the target over the timeline's ticks (40 kbit/s x 39 / 25 s = 7800
    // bytes at the fixture's 39 ticks). 40, not 200: 64x48 is 12 macroblocks and the rate controller cannot spend below
    // QP 16 (`rc::QP_MIN`), so 200 kbit/s (1000 bytes a frame) would read the encoder's QP floor, not its rate control.
    fn bitrate_verdict(dir: &str) -> bool {
        use crate::video::export;
        const KBPS: u32 = 40;
        let ticks = timeline().map(with_audio).map(|t| t.end()).unwrap_or(0).max(0) as u64;
        let target = KBPS as u64 * 1000 * ticks / FPS as u64 / 8;
        let (edl, out) = (alloc::format!("{}/NLEFX.EDL", dir), alloc::format!("{}/NLEFXR.MP4", dir));
        let t0 = crate::arch::ms();
        let jid = match export::start(&edl, &out, export::Opts { bitrate: KBPS, ..export::Opts::default() }) {
            Ok(j) => j,
            Err(e) => {
                serial_println!("[nle] fixture export bitrate={} refused ({}) -> FAIL", KBPS, e);
                return false;
            }
        };
        let rep = loop {
            if let Some(r) = export::report(jid) {
                break Some(r);
            }
            if crate::arch::ms().saturating_sub(t0) > 30_000 {
                break None;
            }
            crate::arch::sched::sleep_ms(4);
        };
        let (bytes, err) = match rep {
            Some(r) if r.err.is_none() => (r.video_bytes, None),
            r => (0, Some(r.and_then(|r| r.err).unwrap_or_else(|| String::from("timeout")))),
        };
        let dev = (bytes as i64 - target as i64) * 100 / (target.max(1) as i64);
        let ok = err.is_none() && target > 0 && dev.abs() <= 25;
        serial_println!(
            "[nle] fixture export bitrate={} video_bytes={} target={} dev_pct={} out={}{} -> {}",
            KBPS, bytes, target, dev, out, err.map(|e| alloc::format!(" err={}", e)).unwrap_or_default(), if ok { "ok" } else { "FAIL" }
        );
        ok
    }

    // ── NLE6KEYS (rmbp-ledger B961): the editing keys through the key door ─────────────────────────────────────────
    // NLEKEY.EDL cuts NLEFXA 0..20 at 0 on one video track (an existing reel, no new file). The Player opens it, seeks to
    // tick 8 through its own seek door, and the key door (`edit_post` + `edit_pass`: key_route's arm minus its focus gate,
    // which a headless test does not hold) takes `s` (split at 8), `]` (the right half's in point +1: a one-tick gap at 8)
    // and `u` (undo). After each key the Player's editor timeline must be the timeline_core op applied by hand; after the
    // last job plays out its plan is the one-run cut (a seek at tick 0 only), the saved file is `KEY_EDL`, and the view band
    // of the edited state hashes to `KEY_VIEW_KAT`, both pinned on the host by timeline_core `view::tests::kernel_keys_view_pin`
    // through the SAME `edl::write` and `view::paint_in`. KERNELSIZEPIN (rmbp-ledger B1001): then `f` (`size_keys`, module tail).
    const KEY_EDL: &str = "TITLE: NLEKEY\nFCM: NON-DROP FRAME\n* UNA FPS 25\n* UNA SOURCE NLEFXA 00:00:01:15\n\n001  NLEFXA   V     C        00:00:00:00 00:00:00:08 00:00:00:00 00:00:00:08\n002  NLEFXA   V     C        00:00:00:08 00:00:00:20 00:00:00:08 00:00:00:20\n";
    /// FNV-1a 64 of the view band (Role::Tracks, 320x56, RGBA) of NLEFXA split at 8, the right half selected, playhead 8.
    const KEY_VIEW_KAT: u64 = 0x53f5_5489_46f1_502f;

    /// The keys leg's verdict and its wire word (`ok`, `FAIL`); the detail line always.
    fn keys_leg(dir: &str, put: &dyn Fn(&str, &[u8]) -> bool) -> (bool, &'static str) {
        use timeline_core::editor::EditorState;
        use timeline_core::view;
        let mut tl = Timeline::new(FPS);
        let a = tl.add_source(REELS[0], FRAMES as i64);
        tl.add_track(Kind::Video);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 0, 20, 0)];
        let path = alloc::format!("{}/NLEKEY.EDL", dir);
        let wrote = timeline_core::edl::write(&tl, "NLEKEY").map(|t| put(&path, t.as_bytes())).unwrap_or(false);
        if !wrote || super::open(&path).is_err() {
            serial_println!("[nle] fixture keys path={} wrote={} -> FAIL", path, wrote as u8);
            super::close("fixture");
            return (false, "FAIL");
        }
        let t0 = crate::arch::ms();
        loop {
            hw::pump_until(4, || false);
            super::vid::tick();
            let (said, err) = STATE.lock().as_ref().map(|s| (s.vsaid, s.err.is_some())).unwrap_or((true, true));
            if said || err || crate::arch::ms().saturating_sub(t0) > 5_000 {
                break;
            }
            crate::arch::sched::sleep_ms(4);
        }
        let split = tl.split(0, 8).ok();
        let trimmed = split.as_ref().and_then(|t| t.trim_in(0, 1, 9).ok());
        let want = [split.clone(), trimmed, split.clone()];
        let _ = super::vid::seek(8 * 1000 / FPS as u64);
        let mut edits = 0usize;
        for (k, w) in [b's', b']', b'u'].into_iter().zip(want.iter()) {
            super::edit_post(crate::pal::Event::Key(k));
            super::edit_pass();
            if w.is_some() && vplay::edit_timeline() == *w {
                edits += 1;
            }
        }
        let (size_ok, size_word) = size_keys(&path, &split, a);
        let t1 = crate::arch::ms();
        loop {
            hw::pump_until(4, || false);
            super::vid::tick();
            let (ended, err) = STATE.lock().as_ref().map(|s| (s.ended, s.err.is_some())).unwrap_or((true, true));
            if ended || err || crate::arch::ms().saturating_sub(t1) > 10_000 {
                break;
            }
            crate::arch::sched::sleep_ms(4);
        }
        let rep = vplay::edl_report().unwrap_or_default();
        let plan_want: Vec<(i64, Option<(u32, i64)>, bool)> = (0..20).map(|t| (t, Some((a.0, t)), t == 0)).collect();
        let plan_ok = rep.plan == plan_want;
        let saved_ok = vplay::read_all(&path).ok().as_deref() == Some(KEY_EDL.as_bytes());
        let fnv = vplay::edit_timeline()
            .map(|t| {
                let mut st = EditorState::new(t);
                let _ = st.select_at(0, 8);
                st.seek(8);
                let l = view::Layout::of_role(view::Role::Tracks, 320, 56, &st);
                view::fnv1a(&view::paint_in(&st, l, view::Order::Rgba, None, None))
            })
            .unwrap_or(0);
        super::close("fixture");
        let ok = edits == 3 && plan_ok && saved_ok && size_ok && fnv == KEY_VIEW_KAT;
        serial_println!(
            "[nle] fixture keys path={} edits={}/3 plan={} saved={} size={} view={:x} want={:x} -> {}",
            path, edits, if plan_ok { "ok" } else { "FAIL" }, if saved_ok { "ok" } else { "FAIL" }, size_word, fnv, KEY_VIEW_KAT, if ok { "ok" } else { "FAIL" }
        );
        (ok, if ok { "ok" } else { "FAIL" })
    }

    // ── NLE6EXPORTAAC (rmbp-ledger B960): the same EDL exported with its sound as AAC-LC and the track decoded back ───────
    // A second export of NLEFX.EDL (`--aac 128`, `<dir>/NLEFXA.MP4`) after the PCM16 one: the file's audio track must be
    // demux_core's `Codec::Aac` at 48 kHz stereo with ceil(target / 1024) + 1 samples (the encoder's padded last block and
    // its drain block), and every sample must decode through audio_core's `AacDecoder` (the kernel links the decoder
    // Stria's host export is checked against) with sound in it. No audio module: nothing was captured, no second export.
    fn export_aac_leg(dir: &str) -> bool {
        use crate::video::export;
        if cfg!(not(any(feature = "uvc", feature = "hda-tone"))) {
            serial_println!("[nle] fixture export audio=aac skip(no-audio)");
            return true;
        }
        let t0 = crate::arch::ms();
        let (edl, out) = (alloc::format!("{}/NLEFX.EDL", dir), alloc::format!("{}/NLEFXA.MP4", dir));
        let jid = match export::start(&edl, &out, export::Opts { aac_kbps: 128, ..export::Opts::default() }) {
            Ok(j) => j,
            Err(e) => {
                serial_println!("[nle] fixture export audio=aac refused ({}) -> FAIL", e);
                return false;
            }
        };
        let rep = loop {
            if let Some(r) = export::report(jid) {
                break Some(r);
            }
            if crate::arch::ms().saturating_sub(t0) > 30_000 {
                break None;
            }
            crate::arch::sched::sleep_ms(4);
        };
        let rep = match rep {
            Some(r) if r.err.is_none() => r,
            r => {
                serial_println!("[nle] fixture export audio=aac out={} err={} -> FAIL", out, r.and_then(|r| r.err).unwrap_or_else(|| String::from("timeout")));
                return false;
            }
        };
        let d = match vplay::read_all(&out).and_then(|b| demux_core::Demuxer::open(b).map_err(|e| alloc::format!("demux: {}", e))) {
            Ok(d) => d,
            Err(e) => {
                serial_println!("[nle] fixture export audio=aac out={} reread={} -> FAIL", out, e);
                return false;
            }
        };
        let Some(t) = d.audio_track().filter(|t| t.codec == demux_core::Codec::Aac && t.sample_rate == 48_000 && t.channels == 2) else {
            serial_println!("[nle] fixture export audio=aac out={} track=missing -> FAIL", out);
            return false;
        };
        let units: Vec<Vec<u8>> = d.track_index(t.id).map(|i| d.track_samples(i).map(|s| d.packet_at(s).data).collect()).unwrap_or_default();
        let mut dec = match audio_core::aac::Asc::parse(&t.config).and_then(|a| a.layout().and_then(|l| audio_core::aac::AacDecoder::new(a.sf_index, l))) {
            Ok(dc) => dc,
            Err(e) => {
                serial_println!("[nle] fixture export audio=aac out={} asc={:?} -> FAIL", out, e);
                return false;
            }
        };
        let (mut decoded, mut loud) = (0usize, false);
        for u in &units {
            if dec.decode_block(&mut audio_core::bits::BitReader::new(u), 0).is_err() {
                break;
            }
            decoded += 1;
            loud |= dec.out.iter().any(|c| c.iter().any(|v| v.abs() > 1.0 / 256.0));
        }
        let bytes: usize = units.iter().map(|u| u.len()).sum();
        let ok = rep.audio_target > 0 && units.len() as u64 == rep.audio_target.div_ceil(1024) + 1 && decoded == units.len() && loud;
        serial_println!(
            "[nle] fixture export audio=aac units={} decoded={} bytes={} target={} loud={} -> {}",
            units.len(), decoded, bytes, rep.audio_target, loud as u8, if ok { "ok" } else { "FAIL" }
        );
        ok
    }

    // ── NLE6FACEDRAG (rmbp-ledger B975): the view band's drags through the pointer door, and the NLE tab ─────────────────
    // NLEDRG.EDL cuts NLEFXA 0..20 at 0 on one video track (an existing reel, no new file). The Player opens it as the NLE
    // (`NLE - NLEDRG`, LISTED: `wm::wl_rows` carries the row, appswitch's `apps()` names it `nle`). Once a frame painted the view
    // band, the pointer door (`edit_press_at` / `edit_motion_at` / `edit_release_at` + `edit_pass`: press_route's arm minus the
    // window hit and the capture seam, which a headless test does not hold) drags the clip body +3 ticks (`move_clip(0, 0, 0,
    // 3)`), then its out handle -5 ticks (`trim_out(0, 0, 15)`); after each the Player's editor timeline must be that op applied
    // by hand, and the saved file is `DRAG_EDL`, pinned on the host by timeline_core `view::drag_tests::kernel_drag_pin` through
    // the SAME `view::Drag` and `edl::write`. NLE6DRAGSNAP (rmbp-ledger B977): the press snaps, so NLEDRG.EDL carries a second
    // clip (NLEFXA 20..30 at 30) and a third drag, a body press at `x_of(5) + 1` moved +94 px (a raw 11 ticks), lands the first
    // clip's out edge 2 px short of the second's `at` 30 and snaps to it: `move_clip(0, 0, 0, 15)`, `snapped=edge`.
    const DRAG_EDL: &str = "TITLE: NLEDRG\nFCM: NON-DROP FRAME\n* UNA FPS 25\n* UNA SOURCE NLEFXA 00:00:01:15\n\n001  NLEFXA   V     C        00:00:00:00 00:00:00:15 00:00:00:15 00:00:01:05\n002  NLEFXA   V     C        00:00:00:20 00:00:01:05 00:00:01:05 00:00:01:15\n";

    /// The drag leg's verdict and its wire word (`ok`, `FAIL`); the detail line always.
    fn drag_leg(dir: &str, put: &dyn Fn(&str, &[u8]) -> bool) -> (bool, &'static str) {
        let mut tl = Timeline::new(FPS);
        let a = tl.add_source(REELS[0], FRAMES as i64);
        tl.add_track(Kind::Video);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 0, 20, 0), Clip::new(a, 20, 30, 30)];
        let path = alloc::format!("{}/NLEDRG.EDL", dir);
        let wrote = timeline_core::edl::write(&tl, "NLEDRG").map(|t| put(&path, t.as_bytes())).unwrap_or(false);
        let Some(id) = (if wrote { super::open(&path).ok() } else { None }) else {
            serial_println!("[nle] fixture drag path={} wrote={} -> FAIL", path, wrote as u8);
            super::close("fixture");
            return (false, "FAIL");
        };
        let mut rows = Vec::new();
        let n = crate::video::wm::wl_rows(&mut rows);
        let listed = rows[..n].iter().any(|r| r.id == id && &r.title[..r.len] == b"NLE - NLEDRG");
        let tab = listed && crate::video::appswitch::apps().iter().any(|a| a.owner == super::OWNER && a.name == "nle");
        let t0 = crate::arch::ms();
        let band = loop {
            hw::pump_until(4, || false);
            super::vid::tick();
            let b = vplay::band_layout(&path);
            let err = STATE.lock().as_ref().map(|s| s.err.is_some()).unwrap_or(true);
            if b.is_some() || err || crate::arch::ms().saturating_sub(t0) > 5_000 {
                break b;
            }
            crate::arch::sched::sleep_ms(4);
        };
        let want_move = tl.move_clip(0, 0, 0, 3).ok();
        let want_trim = want_move.as_ref().and_then(|t| t.trim_out(0, 0, 15).ok());
        let want_snap = want_trim.as_ref().and_then(|t| t.move_clip(0, 0, 0, 15).ok());
        let mut drags = 0usize;
        if let Some((y0, l)) = band {
            let z = l.zoom as isize;
            for (step, w) in [(0usize, &want_move), (1, &want_trim), (2, &want_snap)] {
                let (y0, l) = vplay::band_layout(&path).unwrap_or((y0, l));
                let y = y0 + (l.row_y(0) + timeline_core::view::ROW_H as i64 / 2) as usize;
                let (x, dx) = match step {
                    0 => ((l.x_of(5) + l.zoom as i64 / 2) as usize, 3 * z),
                    1 => ((l.x_of(23) - 2) as usize, -5 * z),
                    _ => ((l.x_of(5) + 1) as usize, 94),
                };
                let pressed = super::edit_press_at(x, y);
                let to = (x as isize + dx).max(0) as usize;
                super::edit_motion_at(to, y);
                super::edit_release_at(to, y);
                super::edit_pass();
                if pressed && w.is_some() && vplay::edit_timeline() == *w {
                    drags += 1;
                }
            }
        }
        let saved_ok = vplay::read_all(&path).ok().as_deref() == Some(DRAG_EDL.as_bytes());
        super::close("fixture");
        let ok = tab && drags == 3 && saved_ok;
        serial_println!(
            "[nle] fixture drag path={} drags={}/3 saved={} tab={} band={} -> {}",
            path, drags, if saved_ok { "ok" } else { "FAIL" }, if tab { "ok" } else { "FAIL" }, band.is_some() as u8, if ok { "ok" } else { "FAIL" }
        );
        (ok, if ok { "ok" } else { "FAIL" })
    }

    // ── H264ENCCOLOURHOST (LEDGER SR377): EXPORT-COLOUR-PIN — the exported file's colour read back (handlers.md §5 NLEPROBE-HDR) ──
    // `export_verdict` re-demuxes NLEFX.MP4; the avc1 track's `Track::colour` (the `colr` `Exp::colour_entry` wrote, the same
    // colour the SPS VUI states: the kernel reads the container only, it links no h264_core) must state a Kr matrix and an
    // explicit range. Token `colour=<matrix>/<limited|full>` (the fixture's tick 0 is a utp1 reel: the RGBA path, 6/limited),
    // `none` when the file states no colour.
    fn export_colour(d: &demux_core::Demuxer, vi: Option<usize>) -> (bool, String) {
        let c = vi.and_then(|i| d.tracks().get(i)).and_then(|t| t.colour);
        match c {
            Some(c) if yuv_core::kr_kb(c.matrix).is_some() && c.full_range.is_some() => {
                (true, alloc::format!("{}/{}", c.matrix, if c.full_range == Some(true) { "full" } else { "limited" }))
            }
            Some(c) => (false, alloc::format!("{}/{:?}", c.matrix, c.full_range)),
            None => (false, String::from("none")),
        }
    }

    // ── NLE6EXPORTMKV (rmbp-ledger B966): the fixture's EDL exported as Matroska and re-demuxed ───────────────────────────
    // The same EDL with the same knobs as the MP4 export (two B pictures) to `<dir>/NLEFXK.MKV`: the container is the out
    // path's extension (`build::Container::of`), the stream `build::MkvStream` behind the export's ONE `build::MuxStream`.
    // Read back through demux_core: the video track holds the timeline's ticks, each sample's (size, keyframe, bytes) is
    // NLEFX.MP4's and its pts the MP4's within the 1 ms TimestampScale (Matroska stores no dts), and an indexed open finds
    // the Cues. Its own line; the `export=` token rides `export_verdict`'s `ok`.
    fn mkv_verdict(dir: &str) -> bool {
        use crate::video::export;
        let ticks = timeline().map(with_audio).map(|t| t.end()).unwrap_or(0).max(0) as u64;
        let (edl, out, mp4) = (alloc::format!("{}/NLEFX.EDL", dir), alloc::format!("{}/NLEFXK.MKV", dir), alloc::format!("{}/NLEFX.MP4", dir));
        let rep = match export::start(&edl, &out, export::Opts { bframes: 2, ..export::Opts::default() }) {
            Ok(jid) => await_export(jid),
            Err(e) => Err(e),
        };
        let open = |p: &str| vplay::read_all(p).and_then(|b| demux_core::Demuxer::open(b).map_err(|e| alloc::format!("demux: {}", e)));
        let rows = |d: &demux_core::Demuxer| -> Vec<(u32, bool, i64, Vec<u8>)> {
            let Some(i) = d.video_track().and_then(|t| d.track_index(t.id)) else { return Vec::new() };
            let t = &d.tracks()[i];
            d.track_samples(i).map(|s| (s.size, s.keyframe, t.to_ns(s.pts), d.sample_data(s).to_vec())).collect()
        };
        let (bytes, samples, why) = match rep.and_then(|r| Ok((r.bytes, open(&out)?, open(&mp4)?))) {
            Ok((bytes, k, m)) => {
                let (a, b) = (rows(&k), rows(&m));
                let same = a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x.0 == y.0 && x.1 == y.1 && x.3 == y.3 && (x.2 - y.2).abs() <= 1_000_000);
                let cues = vplay::read_all(&out).ok().and_then(|f| demux_core::Demuxer::open_indexed(f).ok()).map(|d| d.index().len()).unwrap_or(0);
                let why = if k.format() != demux_core::Format::Matroska { Some(String::from("not-matroska")) } else if !same { Some(String::from("samples-differ")) } else if cues == 0 { Some(String::from("no-cues")) } else { None };
                (bytes, a.len() as u64, why)
            }
            Err(e) => (0, 0, Some(e)),
        };
        let ok = why.is_none() && samples == ticks;
        serial_println!(
            "[nle] fixture export container=mkv bytes={} samples={}{} -> {}",
            bytes, samples, why.map(|w| alloc::format!(" why={}", w)).unwrap_or_default(), if ok { "ok" } else { "FAIL" }
        );
        ok
    }

    /// NLE6EXPORTMKV (B966): wait for export `jid`'s report (30 s), its error as Err. (NLEFX-AWAIT owed: export_verdict's and
    /// bitrate_verdict's own loops onto this one.)
    fn await_export(jid: u32) -> Result<crate::video::export::Report, String> {
        let t0 = crate::arch::ms();
        loop {
            if let Some(r) = crate::video::export::report(jid) {
                return match r.err.clone() {
                    None => Ok(r),
                    Some(e) => Err(e),
                };
            }
            if crate::arch::ms().saturating_sub(t0) > 30_000 {
                return Err(String::from("timeout"));
            }
            crate::arch::sched::sleep_ms(4);
        }
    }

    // ── EXPORTSIZEFIXTURE (rmbp-ledger B990): a two-size AV1 reel exported (handlers.md §5 NLEPROBE-HDR's EXPORT-SIZE-FIXTURE) ───────
    // NLEFXE is libaom's conformance pair `av1-1-b8-01-size-66x66` then `av1-1-b8-01-size-16x16` (av1_core `tests/vectors.txt`: fetched at
    // test time, never in the tree, so canned here as `NLEFXC_KEY` is): each stream's two temporal units verbatim (TU0 = temporal
    // delimiter + sequence header + key frame, TU1 = an inter frame), 672 B, SR379's `av1inbandsize_new_sequence_decodes_at_the_new_size`
    // order. The 16x16 key's new sequence header starts a new coded video sequence (av1_core `Decoder::sequence`), so vplay's `Pic` inside
    // the export job prints `[vplay] size switch at 2000 66x66->16x16`, and the export (`Exp::sized` / `push_sized`, B985, at the reel's
    // default Letterbox policy, SR394) letterboxes the 16x16 frames into the file's first size: `[nle] export size switch 66x66->16x16
    // scaled`. NLESZ.EDL cuts NLEFXE 0..4 at 0 (video only); NLESZ.MP4 is read back through demux_core: 4 avc1 samples at 66x66. `sizes=`
    // counts the distinct sequence-header sizes of the canned bytes (av1_core's header parse, no decode here), so the leg cannot pass on a
    // one-size reel. Its own line; the `export=` token rides `export_verdict`'s `ok`.
    const NLEFXE_66K: [u8; 369] = [
        0x12, 0x00, 0x0a, 0x0a, 0x00, 0x00, 0x00, 0x03, 0x34, 0x18, 0x2f, 0xff, 0xcc, 0x02, 0x32, 0xe0,
        0x02, 0x10, 0x00, 0xd0, 0x80, 0x00, 0x03, 0x00, 0x24, 0x32, 0xe9, 0x02, 0x04, 0xb6, 0xa7, 0x12,
        0x03, 0x42, 0x5d, 0xcd, 0x47, 0x0e, 0x79, 0x4f, 0x78, 0x3a, 0x91, 0x5d, 0x0e, 0x9a, 0x27, 0x04,
        0x66, 0x91, 0x28, 0xd9, 0xa2, 0x63, 0xb5, 0x0e, 0x6c, 0x08, 0x17, 0xf6, 0xa9, 0x8c, 0xc3, 0x2a,
        0x96, 0xc1, 0x69, 0x42, 0xbd, 0x67, 0x19, 0x07, 0xb8, 0xbe, 0x65, 0xb2, 0xd1, 0x35, 0x16, 0x62,
        0xb7, 0x3c, 0x82, 0xcb, 0x5b, 0x9f, 0x38, 0xde, 0x5c, 0x1b, 0xa9, 0x0a, 0x08, 0xdb, 0xa4, 0xb9,
        0xfa, 0x90, 0x7a, 0xef, 0x73, 0xf2, 0xcb, 0x21, 0xae, 0x94, 0x11, 0x17, 0x5e, 0xe2, 0x5a, 0x70,
        0xd0, 0xf0, 0x3b, 0xcc, 0x25, 0x24, 0x07, 0x9f, 0x24, 0x61, 0x71, 0x59, 0x1d, 0x42, 0xa3, 0xe1,
        0xb1, 0x11, 0x0c, 0x19, 0xe5, 0x42, 0x48, 0xc6, 0x22, 0x89, 0x19, 0x65, 0xd6, 0xba, 0x14, 0x45,
        0x26, 0x38, 0xe0, 0x1d, 0xc0, 0xe4, 0x64, 0x53, 0x1e, 0xff, 0x29, 0x31, 0xc8, 0x90, 0xb5, 0x1b,
        0x9d, 0x87, 0xef, 0x76, 0xe0, 0xe4, 0x9c, 0xe1, 0xd1, 0x45, 0xcc, 0xf6, 0x97, 0x3f, 0x37, 0x8a,
        0xa9, 0xb6, 0x48, 0xda, 0x06, 0x64, 0xc1, 0x7f, 0x27, 0x0d, 0x46, 0xf3, 0x07, 0x13, 0x5a, 0x3c,
        0xb5, 0xfe, 0x9c, 0x37, 0xeb, 0x19, 0x83, 0x83, 0x3a, 0x88, 0xbd, 0x8c, 0xe5, 0xe6, 0x5d, 0xf8,
        0xd3, 0x9b, 0x56, 0xc3, 0x78, 0x99, 0x5a, 0xa6, 0x78, 0xa8, 0x50, 0xbf, 0xf0, 0xea, 0xde, 0xe3,
        0x19, 0xf8, 0x88, 0xcf, 0x41, 0xc2, 0xa4, 0x2d, 0x64, 0x71, 0x7b, 0x99, 0x8c, 0x1d, 0xa2, 0x45,
        0xa7, 0xd7, 0x0f, 0x32, 0xa7, 0xc8, 0x46, 0x86, 0xbe, 0xf9, 0x47, 0xb9, 0xcf, 0x01, 0x53, 0xc3,
        0x39, 0x93, 0x5d, 0x87, 0xff, 0x67, 0xec, 0x92, 0x92, 0x97, 0xc4, 0x07, 0x57, 0xce, 0xb6, 0xd3,
        0xec, 0x71, 0xb1, 0xf0, 0x2f, 0x23, 0x15, 0x62, 0xfb, 0x40, 0x12, 0x76, 0xe3, 0x00, 0x45, 0xa9,
        0x31, 0x4e, 0xbd, 0x13, 0x77, 0x0a, 0xad, 0x46, 0x41, 0x30, 0x55, 0x65, 0xcc, 0xc8, 0x5e, 0xa2,
        0x29, 0x42, 0xe6, 0x07, 0xd3, 0xf6, 0x6a, 0xcf, 0x00, 0xb2, 0x76, 0x2a, 0x7c, 0xe2, 0x4c, 0x53,
        0xfd, 0xcd, 0x5e, 0x83, 0x01, 0x20, 0x67, 0x81, 0x62, 0x8e, 0x2a, 0x57, 0x1d, 0x76, 0x32, 0x60,
        0x62, 0x38, 0x8c, 0xb0, 0x50, 0xb4, 0xac, 0x2b, 0xab, 0x46, 0x13, 0x00, 0x0a, 0x91, 0xfa, 0x19,
        0xb5, 0xc2, 0xad, 0x85, 0x95, 0x93, 0x77, 0x59, 0x7f, 0x8d, 0xd4, 0x85, 0x95, 0xc0, 0xa8, 0x04,
        0x70,
    ];
    const NLEFXE_66I: [u8; 41] = [
        0x12, 0x00, 0x32, 0x25, 0x30, 0x03, 0xc3, 0x00, 0xa7, 0x2e, 0x7b, 0x42, 0x00, 0x82, 0x80, 0x20,
        0xc0, 0x08, 0xa0, 0x00, 0x28, 0x00, 0xf1, 0x67, 0x33, 0x33, 0x7e, 0xae, 0x51, 0x0d, 0x33, 0xdf,
        0xb3, 0x88, 0x36, 0xb8, 0xa3, 0x16, 0x2d, 0x45, 0xf4,
    ];
    const NLEFXE_16K: [u8; 183] = [
        0x12, 0x00, 0x0a, 0x0a, 0x00, 0x00, 0x00, 0x01, 0x9f, 0xfb, 0xff, 0xf3, 0x00, 0x80, 0x32, 0xa6,
        0x01, 0x10, 0x00, 0x87, 0x80, 0x00, 0x03, 0x00, 0x00, 0x00, 0x40, 0x00, 0x9e, 0x86, 0x5b, 0xb2,
        0x22, 0xb5, 0x58, 0x4d, 0x68, 0xe6, 0x37, 0x54, 0x42, 0x7b, 0x84, 0xce, 0xdf, 0x9f, 0xec, 0xab,
        0x07, 0x4d, 0xf6, 0xe1, 0x5e, 0x9e, 0x27, 0xbf, 0x93, 0x2f, 0x47, 0x0d, 0x7b, 0x7c, 0x45, 0x8d,
        0xcf, 0x26, 0xf7, 0x6c, 0x06, 0xd7, 0x8c, 0x2e, 0xf5, 0x2c, 0xb0, 0x8a, 0x31, 0xac, 0x69, 0xf5,
        0xcd, 0xd8, 0x71, 0x5d, 0xaf, 0xf8, 0x96, 0x43, 0x8c, 0x9c, 0x23, 0x6f, 0xab, 0xd0, 0x35, 0x43,
        0xdf, 0x81, 0x12, 0xe3, 0x7d, 0xec, 0x22, 0xb0, 0x30, 0x54, 0x32, 0x9f, 0x90, 0xc0, 0x5d, 0x64,
        0x9b, 0x0f, 0x75, 0x31, 0x84, 0x3a, 0x57, 0xd7, 0x5f, 0x03, 0x6e, 0x7f, 0x43, 0x17, 0x6d, 0x08,
        0xc3, 0x81, 0x8a, 0xae, 0x73, 0x1c, 0xa8, 0xa7, 0xe4, 0x9c, 0xa9, 0x5b, 0x3f, 0xd1, 0xeb, 0x75,
        0x3a, 0x7f, 0x22, 0x77, 0x38, 0x64, 0x1c, 0x77, 0xdb, 0xcd, 0xef, 0xb7, 0x08, 0x45, 0x8e, 0x7f,
        0xea, 0xa3, 0xd0, 0x81, 0xc9, 0xc1, 0xbc, 0x93, 0x9b, 0x41, 0xb1, 0xa1, 0x42, 0x17, 0x98, 0x3f,
        0x1e, 0x95, 0xdf, 0x68, 0x7c, 0xb7, 0x98,
    ];
    const NLEFXE_16I: [u8; 79] = [
        0x12, 0x00, 0x32, 0x4b, 0x30, 0x03, 0xc3, 0x00, 0xa7, 0x2e, 0x46, 0x8a, 0x00, 0x00, 0x03, 0x00,
        0x00, 0x50, 0xc0, 0x20, 0x00, 0xf0, 0xb1, 0x2f, 0x43, 0xf3, 0xbb, 0xe6, 0x5c, 0xbe, 0xe6, 0x53,
        0xbc, 0xaa, 0x61, 0x7c, 0x7e, 0x0a, 0x04, 0x1b, 0xa2, 0x87, 0x81, 0xe8, 0xa6, 0x85, 0xfe, 0xc2,
        0x71, 0xb9, 0xf8, 0xc0, 0x78, 0x9f, 0x52, 0x4f, 0xa7, 0x8f, 0x55, 0x96, 0x79, 0x90, 0xaa, 0x2b,
        0x6d, 0x0a, 0xa7, 0x05, 0x2a, 0xf8, 0xfc, 0xc9, 0x7d, 0x9d, 0x4a, 0x61, 0x16, 0xb1, 0x65,
    ];
    const SIZE_TUS: [&[u8]; 4] = [&NLEFXE_66K, &NLEFXE_66I, &NLEFXE_16K, &NLEFXE_16I];

    /// NLEFXE: `test_pattern_track` re-labelled `av01` (av1C: profile 0, level 0, 8-bit 4:2:0, no configOBUs: the in-band headers rule),
    /// its four samples `SIZE_TUS`, keyframes at 0 and 2.
    fn size_reel(tus: [&[u8]; 4]) -> Vec<u8> {
        use demux_core::build;
        let mut video = build::test_pattern_track(1, 66, 66, FPS, 4, 2);
        video.spec.fourcc = *b"av01";
        video.spec.config_box = Some(*b"av1C");
        video.spec.codec_id = "V_AV1";
        video.spec.config = alloc::vec![0x81, 0x00, 0x0c, 0x00];
        for (s, tu) in video.samples.iter_mut().zip(tus) {
            s.data = tu.to_vec();
        }
        build::mp4(&[video], &build::Mp4Options::default())
    }

    /// The distinct `max_frame_width x max_frame_height` the canned temporal units' sequence headers state.
    fn size_count(tus: [&[u8]; 4]) -> usize {
        use av1_core::obu::{split_obus, SequenceHeader};
        let mut seen: Vec<(u32, u32)> = Vec::new();
        for tu in tus {
            for o in split_obus(tu).unwrap_or_default().iter().filter(|o| o.obu_type == 1) {
                if let Ok(s) = SequenceHeader::parse(o.payload) {
                    let wh = (s.max_frame_width_minus_1 + 1, s.max_frame_height_minus_1 + 1);
                    if !seen.contains(&wh) {
                        seen.push(wh);
                    }
                }
            }
        }
        seen.len()
    }

    /// The two-size leg: NLEFXE.MP4 and NLESZ.EDL written through the ONE VFS writer, exported to NLESZ.MP4, re-demuxed.
    fn size_verdict(dir: &str) -> bool {
        size_leg(dir, "NLESZ", timeline_core::SizePolicy::Letterbox, "NLEFXE", SIZE_TUS)
    }

    /// EXPORTSIZEFIXTUREPOLICIES (rmbp-ledger B993): B990's leg under the reel policy `fit`: NLEFXE.MP4 and `<stem>.EDL` (the
    /// policy set by `Timeline::set_size_policy`, so the EDL carries `* UNA SIZE NLEFXE <fit>` unless Letterbox, re-parsed to pin
    /// the round trip) written through the ONE VFS writer, exported to `<stem>.MP4` and re-demuxed; ` fit=<name>` on the line
    /// unless Letterbox (B990's line byte-identical). Adopt: the export must be refused by name and its file gone.
    fn size_leg(dir: &str, stem: &str, fit: timeline_core::SizePolicy, reel_id: &str, tus: [&[u8]; 4]) -> bool {
        use crate::video::export;
        use timeline_core::SizePolicy;
        let mt = crate::shell::vfs_mount_table();
        let put = |c: &str, body: &[u8]| -> bool { mt.put_file(c, body, crate::fs::vfs::KERNEL_PRINCIPAL).is_ok() };
        let mut tl = Timeline::new(FPS);
        let e = tl.add_source(reel_id, 4);
        tl.add_track(Kind::Video);
        tl.tracks[0].clips = alloc::vec![Clip::new(e, 0, 4, 0)];
        let (reel, edl, out) = (alloc::format!("{}/{}.MP4", dir, reel_id), alloc::format!("{}/{}.EDL", dir, stem), alloc::format!("{}/{}.MP4", dir, stem));
        let sizes = size_count(tus);
        let tl = tl.set_size_policy(e, fit).ok();
        let text = tl.as_ref().and_then(|tl| timeline_core::edl::write(tl, stem).ok());
        let round = text.as_deref().and_then(|t| timeline_core::edl::parse(t, FPS).ok()).and_then(|t| t.sources.iter().find(|s| s.reel == reel_id).map(|s| s.size_policy)) == Some(fit);
        if fit == SizePolicy::Adopt {
            let _ = mt.unlink(&out, crate::fs::vfs::KERNEL_PRINCIPAL); // a stale file from an earlier run must not read as "not gone"
        }
        let wrote = put(&reel, &size_reel(tus)) && text.as_deref().map(|t| put(&edl, t.as_bytes())).unwrap_or(false);
        let rep = if tl.is_none() {
            Err(String::from("fixture-policy"))
        } else if !wrote {
            Err(String::from("fixture-write"))
        } else if !round {
            Err(String::from("edl-policy"))
        } else {
            match export::start(&edl, &out, export::Opts::default()) {
                Ok(jid) => await_export(jid),
                Err(e) => Err(e),
            }
        };
        if fit == SizePolicy::Adopt {
            let (refused, why) = match &rep {
                Ok(_) => (false, Some(String::from("exported"))),
                Err(m) if m.starts_with("encode-refused") && m.contains("adopt") && m.contains("(NLE-SIZE-POLICY)") => (true, None),
                Err(m) => (false, Some(m.clone())),
            };
            let gone = vplay::read_all(&out).is_err();
            let ok = sizes == 2 && refused && gone;
            serial_println!(
                "[nle] fixture export sizes={} reel={} out={} fit=adopt refused={} gone={}{} -> {}",
                sizes, reel_id, out, refused as u8, gone as u8, why.map(|w| alloc::format!(" why={}", w)).unwrap_or_default(), if ok { "ok" } else { "FAIL" }
            );
            return ok;
        }
        let open = |p: &str| vplay::read_all(p).and_then(|b| demux_core::Demuxer::open(b).map_err(|e| alloc::format!("demux: {}", e)));
        let (frames, bytes, samples, first, why) = match rep.and_then(|r| Ok((r, open(&out)?))) {
            Ok((r, d)) => {
                let vi = d.video_track().and_then(|t| d.track_index(t.id));
                let samples = vi.map(|i| d.track_samples(i).count() as u64).unwrap_or(0);
                let first = d.video_track().map(|t| (t.width, t.height)).unwrap_or((0, 0));
                (r.frames, r.bytes, samples, first, None)
            }
            Err(e) => (0, 0, 0, (0, 0), Some(e)),
        };
        let ok = why.is_none() && sizes == 2 && samples == 4 && frames == 4 && first == (66, 66);
        let tok = if fit == SizePolicy::Letterbox { String::new() } else { alloc::format!(" fit={}", fit.name()) };
        serial_println!(
            "[nle] fixture export sizes={} reel={} out={} samples={} first={}x{} frames={} bytes={}{}{} -> {}",
            sizes, reel_id, out, samples, first.0, first.1, frames, bytes, tok, why.map(|w| alloc::format!(" why={}", w)).unwrap_or_default(), if ok { "ok" } else { "FAIL" }
        );
        ok
    }

    // ── EXPORTSIZEFIXTUREPOLICIES (rmbp-ledger B993): NLEFXE under each size policy (handlers.md §5 NLEPROBE-HDR's EXPORT-SIZE-FIXTURE-POLICIES) ─
    // B990's leg (`size_leg`) again over the same reel and the same export door, the reel's policy set on the timeline: NLESZF.EDL
    // Fill and NLESZS.EDL Stretch exported to NLESZF / NLESZS.MP4 (the export's `[nle] export size switch 66x66->16x16 scaled
    // fit=<name>`, each read back at 4 samples and 66x66), NLESZA.EDL Adopt refused by name (`encode-refused ... adopt ...
    // (NLE-SIZE-POLICY)`, `Exp::sized`) and its partial file unlinked by the existing `abandon` (only if the encoder had begun the
    // file: `gone=1` holds either way). NLEFXE's two sizes are both 1:1, so every policy covers the whole 66x66 box: a "no bars"
    // pixel sample could not tell Fill from Letterbox, and the kernel names no H.264 decoder (h264_core only through h264enc_core):
    // the geometry pin is owed (EXPORT-SIZE-POLICY-PIXEL: a non-square second size, decoded back). `policies=` counts the four legs
    // that passed (B990's Letterbox leg passed in as `letterbox`).
    fn policies_verdict(dir: &str, letterbox: bool) -> bool {
        use timeline_core::SizePolicy;
        let legs = [("NLESZF", SizePolicy::Fill), ("NLESZS", SizePolicy::Stretch), ("NLESZA", SizePolicy::Adopt)];
        let n = letterbox as u32 + legs.iter().map(|&(stem, fit)| size_leg(dir, stem, fit, "NLEFXE", SIZE_TUS) as u32).sum::<u32>();
        let ok = n == 4;
        serial_println!("[nle] fixture export policies={} -> {}", n, if ok { "ok" } else { "FAIL" });
        ok
    }

    // ── EXPORTSIZEPOLICYPIXEL (rmbp-ledger B998): NLEFXG's exports decoded back (handlers.md §5 NLEPROBE-HDR's EXPORT-SIZE-POLICY-PIXEL) ─
    // NLEFXE's two sizes are both 1:1, so B993's policies all cover the box. NLEFXG keeps NLEFXE's 66x66 pair and switches to libaom's
    // `av1-1-b8-01-size-16x64` (not in av1_core's vectors.txt: fetched from aom-test-data, sha1 dd44aad500c7ca0fc97e3d8f0abed3c83b24c79c
    // = libaom's test/test-data.sha1, sha256 a6764971146e487f369159400efe937ad049c4e06e7e98edb2f35724b378c39c), its two temporal units
    // verbatim (TU0 = temporal delimiter + sequence header + key frame, TU1 = an inter frame), canned as `NLEFXE_66K` is. 16x64 into
    // 66x66: Letterbox paints the picture at columns 25..40 (`yuv_core::scale::fit`) over black bars, Fill takes its centre 16x16
    // over the whole box, Stretch the whole picture. Each policy's export (B993's `size_leg`, the reel as its parameter) is decoded
    // back through h264_core: the export's gop is one second, so the first sample after the switch (2) is a P picture and samples
    // 0..=2 are decoded in order; frame 2's centre luma row is read in three spans (the 16x64 picture's luma is 133..255, a bar ~16).
    // Adopt stays B993's refusal pin. `pixels=` counts the three legs that passed.
    const NLEFXG_16K: [u8; 507] = [
        0x12, 0x00, 0x0a, 0x0a, 0x00, 0x00, 0x00, 0x01, 0xaf, 0xfe, 0xff, 0xfc, 0xc0, 0x20, 0x32, 0xea,
        0x03, 0x10, 0x00, 0x8d, 0x80, 0x00, 0x40, 0x00, 0x30, 0x00, 0x00, 0x00, 0x08, 0xee, 0x30, 0xd7,
        0x67, 0xdc, 0x81, 0x29, 0xcd, 0x2b, 0x03, 0x9a, 0x16, 0x6a, 0xe7, 0x29, 0xc9, 0x80, 0x6d, 0xdc,
        0x19, 0x0e, 0x6e, 0x89, 0xde, 0x12, 0x82, 0x1c, 0xfa, 0x57, 0x75, 0x88, 0x2a, 0xa9, 0x01, 0xa3,
        0x6b, 0x1d, 0x97, 0xc2, 0xd5, 0x05, 0x72, 0x83, 0xfe, 0x45, 0x08, 0x0a, 0x10, 0x35, 0xe2, 0x36,
        0xf7, 0xf2, 0xc9, 0x5a, 0x6d, 0x4d, 0x7b, 0x6c, 0x97, 0x4c, 0x01, 0x39, 0xbb, 0x74, 0x73, 0x7b,
        0x5d, 0xef, 0x69, 0x8f, 0x83, 0x66, 0xde, 0xd0, 0x63, 0x88, 0xce, 0x58, 0xf1, 0x09, 0x47, 0x2e,
        0x4d, 0xea, 0x95, 0xe6, 0xa6, 0x04, 0x02, 0xdf, 0xd0, 0x05, 0x2e, 0x41, 0x00, 0x3c, 0x5a, 0x30,
        0xc7, 0x1d, 0x81, 0x96, 0xdd, 0x53, 0x5f, 0x53, 0xb5, 0x96, 0x8f, 0x6d, 0xa7, 0xd5, 0xda, 0x7c,
        0xbe, 0x12, 0x0e, 0xe2, 0xed, 0x0d, 0xde, 0xc8, 0x7f, 0x4b, 0xb1, 0x72, 0xbf, 0x96, 0xbd, 0x2b,
        0x27, 0x39, 0x7a, 0x1a, 0x63, 0x7f, 0xa3, 0x71, 0x8c, 0xda, 0x60, 0x8d, 0xb8, 0xf6, 0x19, 0xfa,
        0x1e, 0xb8, 0x06, 0xe6, 0x5e, 0x95, 0xb0, 0x8b, 0x24, 0xfa, 0x90, 0x11, 0x39, 0x00, 0xdb, 0x82,
        0x1c, 0x4c, 0x50, 0xc4, 0xda, 0x4d, 0x37, 0x57, 0x3a, 0xf1, 0xfa, 0xb3, 0xf2, 0x14, 0xbb, 0x88,
        0xab, 0x66, 0x03, 0x38, 0x23, 0xff, 0x80, 0x9a, 0x02, 0x59, 0xfc, 0xee, 0x1c, 0xef, 0x48, 0x25,
        0x03, 0x85, 0xce, 0xaa, 0x14, 0xcf, 0x79, 0x19, 0xbb, 0x52, 0x17, 0x16, 0x2e, 0x5f, 0x12, 0xd0,
        0xac, 0xca, 0x86, 0xd5, 0xf4, 0xd6, 0x26, 0x67, 0x2a, 0xae, 0xd1, 0xf3, 0x91, 0x83, 0xb6, 0xe2,
        0x1b, 0x06, 0xbb, 0x3a, 0x8a, 0x04, 0x6b, 0xea, 0xfb, 0x5e, 0xb7, 0x15, 0xda, 0x2f, 0xe3, 0xf9,
        0x7d, 0x1f, 0x8c, 0x30, 0x70, 0xc6, 0x17, 0xbc, 0xa3, 0xdf, 0xe2, 0xfa, 0xa7, 0xa1, 0x16, 0x35,
        0xf6, 0x87, 0x51, 0x9d, 0x01, 0x9e, 0x8b, 0xb6, 0x58, 0x44, 0xcd, 0xee, 0x05, 0x3e, 0x4d, 0x23,
        0xa0, 0xea, 0x4e, 0x90, 0xd6, 0x2f, 0x5f, 0x64, 0xa0, 0xe9, 0x22, 0x34, 0x56, 0x3a, 0xe3, 0xa0,
        0x7a, 0xf3, 0x11, 0x15, 0x6d, 0xce, 0x3c, 0x24, 0xcd, 0x8d, 0x2c, 0x33, 0xf2, 0xab, 0x41, 0x2a,
        0x86, 0x64, 0x4f, 0xcd, 0xba, 0x9b, 0x0f, 0x4c, 0x4b, 0xe9, 0xe0, 0x74, 0x73, 0x4c, 0xd4, 0x64,
        0x86, 0x3b, 0x9f, 0x11, 0x60, 0xf0, 0x3a, 0xc1, 0x3a, 0xf8, 0x6c, 0x29, 0x03, 0x46, 0xfa, 0x0a,
        0x81, 0x60, 0x4a, 0x3a, 0x49, 0xab, 0xd2, 0xf6, 0x9f, 0x2c, 0xeb, 0xee, 0x2d, 0x17, 0x29, 0x83,
        0x7c, 0xab, 0xc0, 0xc5, 0x75, 0x5b, 0xf0, 0x83, 0xb9, 0x36, 0xf5, 0x74, 0xd2, 0x2c, 0x3a, 0x98,
        0x1e, 0x48, 0x81, 0xcb, 0x46, 0xec, 0xc0, 0x20, 0x6e, 0xc9, 0xa8, 0xf5, 0xf3, 0xae, 0x8e, 0xd6,
        0x1c, 0x06, 0x43, 0xee, 0x8f, 0x29, 0x2b, 0x64, 0xe3, 0x1e, 0x7d, 0xf7, 0x22, 0x4d, 0x81, 0x0d,
        0xa9, 0x79, 0x99, 0x61, 0x31, 0xc4, 0x94, 0x16, 0xa4, 0x01, 0xa3, 0x63, 0x98, 0x38, 0x37, 0x49,
        0x42, 0xf6, 0x46, 0xa9, 0x3c, 0x33, 0xb3, 0x4e, 0x5b, 0xeb, 0x6f, 0x53, 0x27, 0x16, 0x79, 0xc8,
        0x2e, 0xae, 0xce, 0xa9, 0x7d, 0x7e, 0x91, 0x05, 0xea, 0xd9, 0x4b, 0xdc, 0xa4, 0x85, 0x8b, 0x55,
        0xf4, 0x53, 0xf5, 0xbb, 0x4e, 0x24, 0x67, 0xc5, 0x08, 0xa3, 0xe4, 0x6b, 0x23, 0xb1, 0x4e, 0x42,
        0xa6, 0xeb, 0x16, 0x8c, 0x5d, 0xec, 0x9b, 0x4c, 0xa1, 0x41, 0x60,
    ];
    const NLEFXG_16I: [u8; 182] = [
        0x12, 0x00, 0x32, 0xb1, 0x01, 0x30, 0x03, 0xc3, 0x00, 0xa7, 0x2e, 0x7a, 0x3a, 0x00, 0x81, 0x00,
        0x00, 0xc0, 0x00, 0x00, 0x40, 0x28, 0x00, 0xea, 0x5f, 0xa3, 0x43, 0xd7, 0x69, 0x98, 0x9f, 0x56,
        0xc8, 0x1f, 0xa2, 0x20, 0x5c, 0xac, 0x42, 0x7e, 0x70, 0xe5, 0x13, 0x2a, 0xbe, 0x1c, 0x97, 0x52,
        0x0f, 0x6f, 0x63, 0x63, 0xed, 0x20, 0x41, 0xb2, 0xbd, 0xcd, 0x86, 0xf1, 0x54, 0xbc, 0xb7, 0xc8,
        0x28, 0x1b, 0x63, 0x2d, 0x9e, 0xc9, 0xa8, 0x03, 0xe9, 0x89, 0xb0, 0x3d, 0x4e, 0xb7, 0x64, 0x38,
        0xb9, 0x9b, 0x81, 0xbd, 0x10, 0x82, 0xcb, 0x5c, 0xdb, 0x51, 0x4b, 0xac, 0xfa, 0xe5, 0x80, 0x6e,
        0x80, 0x6d, 0x7b, 0x11, 0x3c, 0x15, 0x61, 0x8b, 0xa5, 0xee, 0xb3, 0x21, 0x51, 0x47, 0xce, 0xb3,
        0x7c, 0x0c, 0x35, 0xa0, 0x23, 0xa1, 0xbd, 0xd8, 0x33, 0x38, 0x66, 0xd2, 0x20, 0x0c, 0x38, 0xba,
        0xc4, 0xaf, 0xa0, 0xcc, 0xd0, 0x56, 0xb2, 0x73, 0x53, 0x08, 0x45, 0x06, 0x88, 0x92, 0x0c, 0x3c,
        0x7e, 0x37, 0x99, 0x82, 0x0f, 0x07, 0xfc, 0x75, 0x95, 0xff, 0x73, 0xc0, 0x75, 0x58, 0xbb, 0x3a,
        0x6c, 0x3c, 0xe6, 0x4f, 0x46, 0x46, 0xe6, 0xec, 0xa6, 0x66, 0xab, 0x9e, 0xf2, 0x07, 0x73, 0xb8,
        0xbe, 0xb1, 0xc2, 0x47, 0x73, 0xf8,
    ];
    const PIXEL_TUS: [&[u8]; 4] = [&NLEFXE_66K, &NLEFXE_66I, &NLEFXG_16K, &NLEFXG_16I];

    /// `path`'s frame 2 (h264_core over samples 0..=2), its centre luma row's mean in three spans: left x 0..12, middle
    /// x w/2-4..w/2+4, right x w-14..w-2. Ok(true) = bars (left and right < 48, middle > 96); Ok(false) = no bars (all three > 96).
    fn row_bars(path: &str) -> Result<(bool, [u32; 3]), String> {
        let d = vplay::read_all(path).and_then(|b| demux_core::Demuxer::open(b).map_err(|e| alloc::format!("demux: {}", e)))?;
        let t = d.video_track().ok_or_else(|| String::from("no-video"))?;
        let i = d.track_index(t.id).ok_or_else(|| String::from("no-video"))?;
        let mut dec = h264_core::Decoder::new();
        dec.configure_avcc(&t.config).map_err(|e| alloc::format!("{}", e))?;
        for (n, s) in d.track_samples(i).take(3).enumerate() {
            dec.set_tag(n as i64);
            dec.decode_sample(d.sample_data(s)).map_err(|e| alloc::format!("{}", e))?;
        }
        dec.flush();
        let f = dec.take_frames().into_iter().find(|f| f.tag == 2).ok_or_else(|| String::from("no-frame-2"))?;
        let (w, h) = (f.width as usize, f.height as usize);
        if w < 32 || h < 2 || f.planes[0].len() < w * h {
            return Err(alloc::format!("frame {}x{}", w, h));
        }
        let row = &f.planes[0][(h / 2) * w..(h / 2 + 1) * w];
        let mean = |a: usize, b: usize| row[a..b].iter().map(|&v| v as u32).sum::<u32>() / (b - a) as u32;
        let luma = [mean(0, 12), mean(w / 2 - 4, w / 2 + 4), mean(w - 14, w - 2)];
        let [l, m, r] = luma;
        if l < 48 && r < 48 && m > 96 {
            Ok((true, luma))
        } else if l > 96 && r > 96 && m > 96 {
            Ok((false, luma))
        } else {
            Err(alloc::format!("row l={} m={} r={}", l, m, r))
        }
    }

    /// One policy: B993's `size_leg` over NLEFXG as `<stem>`, then `row_bars` on `<stem>.MP4`; ok iff both and bars == `want`.
    fn pixel_leg(dir: &str, stem: &str, fit: timeline_core::SizePolicy, want: bool) -> bool {
        let got = if size_leg(dir, stem, fit, "NLEFXG", PIXEL_TUS) { row_bars(&alloc::format!("{}/{}.MP4", dir, stem)) } else { Err(String::from("export")) };
        let ok = matches!(got, Ok((b, _)) if b == want);
        let (bars, luma, why) = match &got {
            Ok((b, [l, m, r])) => (if *b { "yes" } else { "no" }, alloc::format!(" luma={}/{}/{}", l, m, r), String::new()),
            Err(e) => ("?", String::new(), alloc::format!(" why={}", e)),
        };
        serial_println!("[nle] fixture export pixel fit={} bars={} reel=NLEFXG{}{} -> {}", fit.name(), bars, luma, why, if ok { "ok" } else { "FAIL" });
        ok
    }

    /// NLEPXL Letterbox (bars), NLEPXF Fill and NLEPXS Stretch (no bars).
    fn pixels_verdict(dir: &str) -> bool {
        use timeline_core::SizePolicy;
        let legs = [("NLEPXL", SizePolicy::Letterbox, true), ("NLEPXF", SizePolicy::Fill, false), ("NLEPXS", SizePolicy::Stretch, false)];
        let n = legs.iter().map(|&(stem, fit, want)| pixel_leg(dir, stem, fit, want) as u32).sum::<u32>();
        let ok = n == 3;
        serial_println!("[nle] fixture export pixels={} -> {}", n, if ok { "ok" } else { "FAIL" });
        ok
    }

    // ── KERNELSIZEPIN (rmbp-ledger B1001): the size key and the present it moves (handlers.md §5 NLEPROBE-HDR's KERNEL-SIZE-PIN) ─
    // After keys_leg's `s ] u` (the timeline is `split`, the playhead at 8 on the right half), `f` through the same key door
    // (`edit_post` + `edit_pass`): the reel NLEFXA's policy goes Letterbox -> Fill, the editor timeline must be
    // `Timeline::set_size_policy` applied by hand and the saved file `KEY_EDL` with the one `* UNA SIZE NLEFXA fill` line after
    // its SOURCE line (`edl::write` omits Letterbox). The pass then runs until the Player presents a frame (`vplay::stats`'s
    // presented counter) whose picture rows (`STATE.surf` rows 0..oy/2: the view band takes at most the lower half) hold no
    // black word: Fill covers the box (B997's `present_fit`, wire `[vplay] present fit=fill reel=NLEFXA ...`); the top half's
    // FNV-1a 64 (`paper::fnv1a`) is printed (its KAT is owed, KERNEL-PRESENT-PIXEL: the flat field's level steps with the
    // presented ordinal). Back at tick 8, `f` three more times (stretch, adopt, letterbox): the timeline is `split` again and
    // the file `KEY_EDL` byte for byte, so keys_leg's own checks after it are unchanged.

    /// The size key's verdict and its wire word (`ok`, `FAIL`); the detail line always.
    fn size_keys(path: &str, split: &Option<Timeline>, a: timeline_core::SourceId) -> (bool, &'static str) {
        let fill = split.as_ref().and_then(|t| t.set_size_policy(a, timeline_core::SizePolicy::Fill).ok());
        let src_line = "* UNA SOURCE NLEFXA 00:00:01:15\n";
        let fill_edl = KEY_EDL.replace(src_line, &alloc::format!("{}* UNA SIZE NLEFXA fill\n", src_line));
        let p0 = vplay::stats().1;
        super::edit_post(crate::pal::Event::Key(b'f'));
        super::edit_pass();
        let fill_ok = fill.is_some() && vplay::edit_timeline() == fill;
        let edl_ok = vplay::read_all(path).ok().as_deref() == Some(fill_edl.as_bytes());
        let (mut bars, mut hash) = (usize::MAX, 0u64);
        let t0 = crate::arch::ms();
        loop {
            hw::pump_until(4, || false);
            super::vid::tick();
            let n = vplay::stats().1.saturating_sub(p0);
            let (err, sample) = STATE
                .lock()
                .as_ref()
                .map(|s| {
                    let top = (s.oy / 2) * s.w;
                    let px = &s.surf[..top.min(s.surf.len())];
                    (s.err.is_some(), Some((px.iter().filter(|&&w| (w & 0x00FF_FFFF) == 0).count(), crate::video::paper::fnv1a(px))))
                })
                .unwrap_or((true, None));
            if let Some((b, h)) = sample.filter(|_| n > 0) {
                (bars, hash) = (b, h);
            }
            if (n > 0 && bars == 0) || err || crate::arch::ms().saturating_sub(t0) > 3_000 {
                break;
            }
            crate::arch::sched::sleep_ms(4);
        }
        let presents = vplay::stats().1.saturating_sub(p0);
        let _ = super::vid::seek(8 * 1000 / FPS as u64);
        for _ in 0..3 {
            super::edit_post(crate::pal::Event::Key(b'f'));
            super::edit_pass();
        }
        let back_ok = split.is_some() && vplay::edit_timeline() == *split && vplay::read_all(path).ok().as_deref() == Some(KEY_EDL.as_bytes());
        let ok = fill_ok && edl_ok && presents > 0 && bars == 0 && back_ok;
        serial_println!(
            "[nle] fixture keys size reel=NLEFXA fit=fill edl={} presents={} bars={} hash={:x} back={} -> {}",
            if fill_ok && edl_ok { "ok" } else { "FAIL" }, presents, bars, hash, if back_ok { "ok" } else { "FAIL" }, if ok { "ok" } else { "FAIL" }
        );
        (ok, if ok { "ok" } else { "FAIL" })
    }
}

// ── NLE6KEYS (rmbp-ledger B961): the Player's editing-key door ─────────────────────────────────────────────────────────
// `key_route`'s last arm posts a byte that names an editor verb (`vplay::edit_verb`) while the Player shows an EDL with an
// editor state (`vplay::edit_live`); the next pass runs it (`edit_pass`): the verb on the shared state (`vplay::edit_key`,
// which saves the EDL), then the scrubber's seek door at the playhead, so the job re-schedules the edited timeline. A
// paused Player stays paused. Without the video arm the door takes nothing.

#[cfg(all(target_arch = "x86_64", feature = "videoplayer"))]
static EDIT_KEY: AtomicU8 = AtomicU8::new(0);

/// `key_route`'s arm (router depth: a store, no I/O).
#[cfg(all(target_arch = "x86_64", feature = "videoplayer"))]
fn edit_post(ev: crate::pal::Event) -> bool {
    let crate::pal::Event::Key(b) = ev else { return false };
    if crate::video::vplay::edit_verb(b).is_none() || !crate::video::vplay::edit_live(&shown()) {
        return false;
    }
    EDIT_KEY.store(b, Ordering::Release);
    true
}

/// The pass: the posted key, then the seek door at the playhead it leaves (the facts re-read: the timeline's length moves).
#[cfg(all(target_arch = "x86_64", feature = "videoplayer"))]
fn edit_pass() {
    let d = EDIT_DRAG.swap(false, Ordering::AcqRel); // NLE6FACEDRAG (rmbp-ledger B975): a released drag first; a key waits a pass
    let k = if d { 0 } else { EDIT_KEY.swap(0, Ordering::AcqRel) };
    if k == 0 && !d {
        return;
    }
    let Some((path, ms, playing)) = STATE.lock().as_ref().map(|s| (s.path.clone(), s.shown_ms, s.playing)) else { return };
    let to = if d { crate::video::vplay::edit_drag_end(&path) } else { crate::video::vplay::edit_key(&path, k, ms) };
    let Some(to) = to else { return };
    let _ = vid::seek(to);
    if let Some(st) = STATE.lock().as_mut() {
        st.vsaid = false;
    }
    if !playing {
        vid::toggle();
    }
}

#[cfg(all(target_arch = "x86_64", feature = "videoplayer"))]
fn edit_close() {
    if crate::video::vplay::edit_drag_take() {
        super::capture::cancel(); // NLE6FACEDRAG (rmbp-ledger B975): a drag live at the close lets the pointer go
    }
    EDIT_DRAG.store(false, Ordering::Release);
    let _ = wm::list_furniture(OWNER, false); // NLE6KEYSTAB (rmbp-ledger B975): the NLE leaves the wc window list
    crate::video::vplay::edit_close();
}

#[cfg(not(all(target_arch = "x86_64", feature = "videoplayer")))]
fn edit_post(_: crate::pal::Event) -> bool {
    false
}

#[cfg(not(all(target_arch = "x86_64", feature = "videoplayer")))]
fn edit_pass() {}

#[cfg(not(all(target_arch = "x86_64", feature = "videoplayer")))]
fn edit_close() {}

// ── NLE6KEYSTAB + NLE6FACEDRAG (rmbp-ledger B975): the NLE tab and the view band's pointer door ──────────────────────────────
// handlers.md §5 NLE6's NLE6KEYS-TAB and NLE6FACE-DRAG. THE TAB: the Player opened on an EDL is titled `NLE - <stem>` (the
// Player's own `Player - <leaf>` shape, ASCII) and its owner is LISTED in the wc window list while it is open
// (`wm::list_furniture`: `cycle_order` / `wl_rows` then carry it, so appswitch's Cmd-Tab, the Window menu and WINCYCLE see an
// app `nle`; appswitch activates it through its existing door); `edit_close` unlists it. Wire: `[nle] tab win=<n> title="NLE -
// <stem>" listed=<0|1>`. THE DRAG: `press_route`'s picture arm asks `edit_press_route` first: a press on the painted view band
// begins the host's `view::Drag` (`vplay::edit_press`) and takes the pointer (the capture seam, as the scrubber does); the
// motion updates it (`vplay::edit_drag_move`, no I/O); the release posts `EDIT_DRAG` and the pass (`edit_pass`) runs
// `vplay::edit_drag_end` (the commit and the save), then the same seek door the keys use. A press off the band stays play/pause.

#[cfg(all(target_arch = "x86_64", feature = "videoplayer"))]
static EDIT_DRAG: AtomicBool = AtomicBool::new(false);

/// The NLE window's title for an EDL (`None`: a plain Player window).
#[cfg(all(target_arch = "x86_64", feature = "videoplayer"))]
fn nle_title(path: &str) -> Option<String> {
    crate::video::vplay::is_edl(path).then(|| alloc::format!("NLE - {}", crate::video::vplay::edl_stem(path)))
}

/// An EDL's window joins the wc window list (the tab).
#[cfg(all(target_arch = "x86_64", feature = "videoplayer"))]
fn nle_tab(id: wm::WinId, path: &str) {
    if !crate::video::vplay::is_edl(path) {
        return;
    }
    let listed = wm::list_furniture(OWNER, true);
    serial_println!("[nle] tab win={} title=\"NLE - {}\" listed={}", id, crate::video::vplay::edl_stem(path), listed as u8);
}

/// A press at surface px `(lx, ly)` on the shown EDL's view band began a drag (no pointer capture: the fixture's door).
#[cfg(all(target_arch = "x86_64", feature = "videoplayer"))]
fn edit_press_at(lx: usize, ly: usize) -> bool {
    let p = shown();
    crate::video::vplay::edit_live(&p) && crate::video::vplay::edit_press(&p, lx, ly)
}

/// `press_route`'s picture arm: a drag begun takes the pointer until the release.
#[cfg(all(target_arch = "x86_64", feature = "videoplayer"))]
fn edit_press_route(lx: usize, ly: usize) -> bool {
    if !edit_press_at(lx, ly) {
        return false;
    }
    super::capture::begin(edit_motion, edit_release);
    true
}

#[cfg(all(target_arch = "x86_64", feature = "videoplayer"))]
fn edit_motion_at(lx: usize, ly: usize) {
    crate::video::vplay::edit_drag_move(lx, ly);
}

/// The release at surface px: the last motion, then the end posted for the pass (the commit saves: never at router depth).
#[cfg(all(target_arch = "x86_64", feature = "videoplayer"))]
fn edit_release_at(lx: usize, ly: usize) {
    edit_motion_at(lx, ly);
    EDIT_DRAG.store(true, Ordering::Release);
}

#[cfg(all(target_arch = "x86_64", feature = "videoplayer"))]
fn edit_motion(x: i32, y: i32) {
    if let Some((lx, ly)) = local(x, y) {
        edit_motion_at(lx, ly);
    }
}

#[cfg(all(target_arch = "x86_64", feature = "videoplayer"))]
fn edit_release(x: i32, y: i32) {
    match local(x, y) {
        Some((lx, ly)) => edit_release_at(lx, ly),
        None => EDIT_DRAG.store(true, Ordering::Release),
    }
}

#[cfg(not(all(target_arch = "x86_64", feature = "videoplayer")))]
fn nle_title(_: &str) -> Option<String> {
    None
}

#[cfg(not(all(target_arch = "x86_64", feature = "videoplayer")))]
fn nle_tab(_: wm::WinId, _: &str) {}

#[cfg(not(all(target_arch = "x86_64", feature = "videoplayer")))]
fn edit_press_route(_: usize, _: usize) -> bool {
    false
}

// ── KERNELSCALEONEARM (rmbp-ledger B980): the Player's `vid` verb (handlers.md §5 NLEPROBE-HDR's VPLAY-QUEUE-RUNTIME, SCALE-AREA) ──
// The queue depth and the present's downscale filter at runtime, for measuring the present cost on metal: the Player's own verb,
// not a Principia key (a debugging aid must not outlive a boot). Each sets the NEXT value; vplay's next job start (an open, a
// seek) latches it, so `[player] ended ... queue=<n> filter=<nearest|area>` describes the whole play.
#[cfg(all(target_arch = "x86_64", feature = "videoplayer"))]
pub fn shell_verb(args: &[&str], console: &mut crate::console::Console) {
    use crate::video::vplay;
    const USAGE: &str = "usage: vid | vid queue <1..16> | vid filter area|nearest";
    match (args.first().copied(), args.get(1).copied(), args.len()) {
        (None, _, _) => {
            let (qn, an, ql, al) = vplay::knobs();
            console.println(&alloc::format!("vid: queue={}/{} filter={}/{} (live/next)", ql, qn, vplay::filter_word(al), vplay::filter_word(an)));
        }
        (Some("queue"), Some(n), 2) => match n.parse::<u32>() {
            Ok(n) if vplay::set_queue(n) => console.println(&alloc::format!("vid: queue={} at the next open", n)),
            _ => console.println(USAGE),
        },
        (Some("filter"), Some(f @ ("area" | "nearest")), 2) => {
            vplay::set_filter(f == "area");
            console.println(&alloc::format!("vid: filter={} at the next open{}", f, if f == "area" { " (downscales >= 2x)" } else { "" }));
        }
        _ => console.println(USAGE),
    }
}
