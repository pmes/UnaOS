// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — wm (SOFTY B629: the taskbar — the bottom band, the Start button, the app tiles, the tray and the clock)
//!
//! Under Softy the DOCK's strip tenancy hosts this band: `dock::strip_rect` answers [`rect`] (so `wm::erase_clip`
//! and every reader of `strip::rects` see the taskbar as the bottom furniture), `dock::compose` runs [`compose`]
//! and `dock::press_at` routes to [`press_at`] — one line-neutral hook each. The dock itself draws nothing.
//!
//! THE BAND — flush to the bottom edge, [`k::BAR_H`] tall, the surface tone with a hairline on top. CENTRED: our
//! own Start mark (a faceted crystal, never another vendor's logo), then one tile per app — the dock's PINNED
//! table apps in `dock::DP_PINS` order (read through `dock::dp_is_pinned`, never re-typed), then every running app
//! no pin names. A running app carries a pip; the focused app's pip is a wider accent pill on a lit tile. RIGHT:
//! the tray (network, volume, battery — `video::status`'s facts, our own glyphs) and the clock (time over date).
//!
//! PRESSES — the Start button toggles the Start menu; a tile RAISES its running window (or minimises it when it
//! is already the focused one), else LAUNCHES the app DIRECTLY through `dock::launch_named` — the dock's own pin
//! launch, so R115 holds here exactly as it holds on the dock; the clock toggles the NOTIFICATION CALENDAR (the
//! notification summary over the month); the tray toggles QUICK SETTINGS (Wi-Fi, sound, battery; its gear opens
//! Settings) — SOFTY2 (B641). Every target draws hover and, while the pointer's button is held, pressed.
//!
//! Paint: row-composed through `strip::paint` (`softy::paint` facade on one scanline), damage-driven by one
//! signature exactly as the dock is. No heap on the pass: the window scan reuses a warm pool.
//!
//! SOFTYDOTS (rmbp-ledger B721) — the Una bar's three honest signals, in the Softy look's paint (one mechanism, two
//! paints): the CAMERA in-use light (a camera glyph with the `accent` dot, the tray's first item, while
//! `drivers::xhci::uvc::open_streams` is not 0 — the driver's count read at paint time, never a model a fixture or an
//! app publishes into, BP-video-24), the DISK item (a drive glyph with the warning dot while `status::disk_low()`
//! names a free percent, DISKFULL B699) and the ATTENTION pulse (the tile of a window refused focus dims its pip for
//! `wm::ATTN_MS` — the same `wm::attention` beat the Una dock reads, FOCUSSTEAL B698; the dock's service pass owes the
//! strip a pass at its start and end). While the session is LOCKED (`video::lock`, AUTOLOCK B712) the band paints
//! nothing and takes no press: its pixels are vacated once and the login card owns the glass.
//!
//! Wire: `[softy] taskbar built tiles=<n> tray=<n> h=<px>` on every change of the band's make-up;
//! `[softy] tile press app=<name> -> <launch <how>|raise win=<id>|minimise win=<id>>`;
//! `[softy] tray cam=<on|off> disk=<pct|-> pulse=<owner|0>` on every change of the three signals;
//! `[softy] taskbar hidden reason=lock` once per lock.

use core::sync::atomic::{AtomicU64, Ordering};

use super::paint::{self as p, Surf};
use super::palette as k;
use crate::video::{dock, strip, text::Face, wm};

static SLOT: strip::Slot = strip::Slot::new();
/// The last `(tiles, tray, h)` announced, packed; `u64::MAX` = never.
static BUILT: AtomicU64 = AtomicU64::new(u64::MAX);
/// SOFTYDOTS: the last `(cam, disk, pulse owner)` the wire said, packed; `u64::MAX` = never.
static DOTS: AtomicU64 = AtomicU64::new(u64::MAX);
/// SOFTYDOTS: the pip's alpha while its tile pulses (the dock's "running dot dimmed for one beat").
const PULSE_A: u8 = 64;
/// The warm window-scan buffer (grown once, reused every pass).
static ROWS: crate::sync::Mutex<alloc::vec::Vec<wm::DockEntry>> = crate::sync::Mutex::new(alloc::vec::Vec::new());
/// Fixture: tile presses that launched / raised, and the last outcome word.
pub(super) static PRESSES: AtomicU64 = AtomicU64::new(0);

/// A pin index that names no table app.
const NO_PIN: u8 = 0xFF;
/// Hover targets past the tiles.
const HOT_START: u8 = 0xF0;
const HOT_TRAY: u8 = 0xF1;
const HOT_CLOCK: u8 = 0xF2;
const HOT_NONE: u8 = 0;

/// The band's rect on a `pw` x `ph` panel (flush bottom, full width), or `None` when the panel cannot host it.
pub fn rect(pw: usize, ph: usize) -> Option<strip::Rect> {
    let h = k::px(k::BAR_H);
    strip::frame_flush(strip::Edge::Bottom, h, 2 * h, pw, ph)
}

/// The taskbar's top edge on this panel (the flyouts float above it).
pub fn top(pw: usize, ph: usize) -> usize {
    rect(pw, ph).map(|r| r.1).unwrap_or(ph)
}

#[derive(Clone, Copy)]
struct Tile {
    pin: u8,
    win: wm::WinId,
    owner: u64,
    running: bool,
    focused: bool,
    visible: bool,
    /// SOFTYDOTS: a window of this tile was refused focus within `wm::ATTN_MS` (`wm::attention_lit`).
    pulse: bool,
    title: [u8; wm::MAX_TITLE],
    tlen: u8,
}

impl Tile {
    const fn empty() -> Self {
        Tile { pin: NO_PIN, win: wm::WIN_NONE, owner: 0, running: false, focused: false, visible: false, pulse: false, title: [0; wm::MAX_TITLE], tlen: 0 }
    }
    fn name(&self) -> &[u8] {
        if self.pin != NO_PIN { dock::DP_PINS[self.pin as usize].name.as_bytes() } else { &self.title[..self.tlen as usize] }
    }
}

/// Is table app `i` on this build (the dock's own availability rule: Quarry needs its feature)?
fn available(i: usize) -> bool {
    i != 2 || cfg!(feature = "quarry")
}

/// The registrar key of a table app's icon (`appres` names the editor `textedit`).
pub(super) fn icon_key(pin: usize) -> &'static str {
    match dock::DP_PINS[pin].name {
        "editor" => "textedit",
        n => n,
    }
}

/// Which table app (if any) a window row belongs to: the kernel windows by owner, a program by its app name, else
/// by the caption's first word.
fn pin_of(e: &wm::DockEntry) -> u8 {
    let o = e.owner_asid;
    if o == wm::KERNEL_OWNER_CONSOLE {
        return 0;
    }
    if o == wm::KERNEL_OWNER_DESKTOP {
        return 1;
    }
    if o == crate::video::textedit::OWNER {
        return 5;
    }
    let mut nm = [0u8; wm::MAX_TITLE];
    let l = if wm::is_kernel_owner(o) { 0 } else { wm::app_name_of(o, &mut nm) };
    let (src, n) = if l > 0 { (&nm[..], l) } else { (&e.title[..], e.title_len.min(wm::MAX_TITLE)) };
    let end = src[..n].iter().position(|&c| c == b' ' || c == b':').unwrap_or(n);
    for (i, sp) in dock::DP_PINS.iter().enumerate() {
        if available(i) && sp.name.as_bytes().eq_ignore_ascii_case(&src[..end]) {
            return i as u8;
        }
    }
    NO_PIN
}

/// Everything a pass draws, gathered once.
#[derive(Clone, Copy)]
struct Model {
    n: usize,
    t: [Tile; k::TILES_MAX],
    /// Tray: `net` 0 none / 1 down / 2 up; `vol` 0 none / 1 + level (0..=16), 0x80 = muted; `bat` 0 none / 1 + percent, 0x80.. charging.
    net: u8,
    vol: u8,
    bat: u16,
    /// SOFTYDOTS: a camera stream is open (the driver's count), and the root's free percent + 1 while low (0 = not).
    cam: bool, #[cfg(feature = "uac")] mic: bool, // SOFTYMIC (B749): the microphone's capture pipe runs (the driver's count). LINE-NEUTRAL fold, code first.
    disk: u8,
    clock: [u8; 8], // crawl A 18: room for the 12-hour AM/PM
    date: [u8; 10],
    clock_set: bool,
    hot: u8,
    pressed: u8,
    start_open: bool,
    cal_open: bool,
    /// SOFTY2: the quick-settings flyout is open (the tray draws lit).
    qs_open: bool,
}

/// The band's geometry for a model on a `pw`-wide panel (strip-local px).
#[derive(Clone, Copy)]
struct Layout {
    x0: usize,
    ty: usize,
    tw: usize,
    th: usize,
    pitch: usize,
    clock_x: usize,
    clock_w: usize,
    tray_x: usize,
    tray_w: usize,
    tray_n: usize,
}

impl Model {
    fn read(rows: &[wm::DockEntry]) -> Model {
        let mut m = Model {
            n: 0, t: [Tile::empty(); k::TILES_MAX], net: 0, vol: 0, bat: 0, cam: false, #[cfg(feature = "uac")] mic: false, disk: 0, clock: [b' '; 8], date: [b' '; 10],
            clock_set: false, hot: HOT_NONE, pressed: HOT_NONE, start_open: super::startmenu::open_kind() == 1,
            cal_open: super::startmenu::open_kind() == 2, qs_open: super::startmenu::open_kind() == 3,
        };
        // Pins first, in the table's order.
        for i in 0..dock::DP_PINS.len() {
            if m.n < k::TILES_MAX && available(i) && dock::dp_is_pinned(i) {
                m.t[m.n] = Tile { pin: i as u8, ..Tile::empty() };
                m.n += 1;
            }
        }
        // Then each running window: it lights its app's tile, or becomes a tile of its own (one per owner).
        for e in rows.iter() {
            let pin = pin_of(e);
            let slot = if pin != NO_PIN {
                (0..m.n).find(|&j| m.t[j].pin == pin)
            } else {
                (0..m.n).find(|&j| m.t[j].pin == NO_PIN && m.t[j].owner == e.owner_asid)
            };
            let j = match slot {
                Some(j) => j,
                None if m.n < k::TILES_MAX => {
                    let mut t = Tile { pin, ..Tile::empty() };
                    let l = e.title_len.min(wm::MAX_TITLE);
                    t.title[..l].copy_from_slice(&e.title[..l]);
                    t.tlen = l as u8;
                    m.t[m.n] = t;
                    m.n += 1;
                    m.n - 1
                }
                None => continue,
            };
            let t = &mut m.t[j];
            // The focused window wins the tile; otherwise the first (lowest id) running one.
            if !t.running || (e.focused && !t.focused) {
                t.win = e.id;
                t.owner = e.owner_asid;
                t.visible = e.visible;
            }
            t.running = true;
            t.focused |= e.focused;
            t.pulse |= wm::attention_lit(e.owner_asid);
        }
        // SOFTYDOTS: the camera light from the DRIVER'S count (the menu bar's `camdot` reads the same call), the disk
        // item from the status owner's one fact.
        #[cfg(feature = "uvc")]
        {
            m.cam = crate::drivers::xhci::uvc::open_streams() != 0; #[cfg(feature = "uac")] { m.mic = crate::drivers::usb::uac::capture_armed() != 0; } // SOFTYMIC (B749): the mic light from the DRIVER'S count (the menu bar's `micdot` reads the same call). LINE-NEUTRAL fold, code first.
        }
        m.disk = crate::video::status::disk_low().map_or(0, |p| p.min(100) + 1);
        // The tray.
        if let Some(tr) = crate::video::status::tray() {
            m.net = match tr.net { None => 0, Some(n) if n.up || n.ip.is_some() => 2, Some(_) => 1 };
            m.vol = match tr.volume { None => 0, Some((lv, mute)) => (1 + lv.min(16)) | if mute { 0x80 } else { 0 } };
        }
        if let Some((b, _)) = crate::video::status::reading() {
            m.bat = 1 + b.percent.min(100) | if b.charging { 0x8000 } else { 0 };
        }
        if let Some(secs) = crate::clock::try_unix_now() {
            // TZLOCALE (B701): the clock and the date through the one locale (UTC + ymd-24 paints what it always did).
            crate::clock::locale::fmt_time(crate::clock::locale::Painter::Taskbar, secs, &mut m.clock);
            crate::clock::locale::fmt_date(crate::clock::locale::Painter::Taskbar, secs, &mut m.date);
            m.clock_set = true;
        }
        m
    }

    fn tray_n(&self) -> usize {
        let n = self.cam as usize + (self.disk != 0) as usize + (self.net != 0) as usize + (self.vol != 0) as usize + (self.bat != 0) as usize; #[cfg(feature = "uac")] let n = n + self.mic as usize; n // SOFTYMIC (B749): the mic light is a tray item while lit. LINE-NEUTRAL fold, code first.
    }

    /// SOFTYDOTS: the live pulse's owner on this band (0 = none), for the wire.
    fn pulse_owner(&self) -> u64 {
        self.t[..self.n].iter().find(|t| t.pulse).map(|t| t.owner).unwrap_or(0)
    }

    fn layout(&self, pw: usize) -> Layout {
        let (tw, th, gap) = (k::px(k::TILE_W), k::px(k::TILE_H), k::px(k::TILE_GAP));
        let pitch = tw + gap;
        let cnt = 1 + self.n;
        let gw = cnt * pitch - gap;
        let x0 = (pw.saturating_sub(gw) / 2).max(k::px(k::EDGE_PAD));
        let ty = (k::px(k::BAR_H) - th) / 2;
        let face = Face::Ui;
        let cw = if self.clock_set { p::text_w(&self.clock, false, face).max(p::text_w(&self.date, false, face)) } else { p::text_w(b"--:--", false, face) };
        let clock_w = cw + 2 * k::px(k::TRAY_PAD);
        let clock_x = pw.saturating_sub(k::px(k::EDGE_PAD) + clock_w);
        let tray_n = self.tray_n();
        let (ic, ig) = (k::px(k::TRAY_ICON), k::px(k::TRAY_GAP));
        let tray_w = if tray_n == 0 { 0 } else { tray_n * ic + (tray_n - 1) * ig + 2 * k::px(k::TRAY_PAD) };
        let tray_x = clock_x.saturating_sub(gap + tray_w);
        Layout { x0, ty, tw, th, pitch, clock_x, clock_w, tray_x, tray_w, tray_n }
    }

    /// What is under strip-local `(x, y)`: a tile index (`1 + k`, 1 = Start), the tray, the clock, or nothing.
    fn hit(&self, l: &Layout, x: usize, y: usize) -> u8 {
        if y < l.ty || y >= l.ty + l.th {
            return HOT_NONE;
        }
        if x >= l.x0 && x < l.x0 + (1 + self.n) * l.pitch {
            let i = (x - l.x0) / l.pitch;
            if (x - l.x0) % l.pitch < l.tw {
                return if i == 0 { HOT_START } else { i as u8 };
            }
        }
        if l.tray_w > 0 && x >= l.tray_x && x < l.tray_x + l.tray_w {
            return HOT_TRAY;
        }
        if x >= l.clock_x && x < l.clock_x + l.clock_w {
            return HOT_CLOCK;
        }
        HOT_NONE
    }

    fn signature(&self, r: strip::Rect) -> u64 {
        let mut h = strip::fnv1a_u64(strip::FNV_BASIS, ((r.2 as u64) << 32) | r.3 as u64);
        for t in self.t[..self.n].iter() {
            h = strip::fnv1a_u64(h, (t.pin as u64) | (t.win as u64) << 8 | (t.running as u64) << 40 | (t.focused as u64) << 41 | (t.visible as u64) << 42 | (t.pulse as u64) << 43);
            for &b in t.name() {
                h = strip::fnv1a(h, b);
            }
        }
        h = strip::fnv1a_u64(h, self.net as u64 | (self.vol as u64) << 8 | (self.bat as u64) << 16 | (self.hot as u64) << 32 | (self.pressed as u64) << 40 | (self.cam as u64) << 48 | (self.disk as u64) << 56); #[cfg(feature = "uac")] { h = strip::fnv1a_u64(h, self.mic as u64); } // SOFTYMIC (B749): the mic light repaints the band the pass the count moves. LINE-NEUTRAL fold, code first.
        // SOFTY2: the quick-settings state, and the appearance epoch (dark / accent) — a theme change repaints the band.
        h = strip::fnv1a_u64(h, (self.start_open as u64) | (self.cal_open as u64) << 1 | (self.clock_set as u64) << 2 | (self.qs_open as u64) << 3 | (crate::video::theme::epoch() as u64) << 32);
        for &b in self.clock.iter().chain(self.date.iter()) {
            h = strip::fnv1a(h, b);
        }
        strip::seal(h)
    }
}

/// THE START MARK — ours: a faceted crystal (a rhombus, its left facet in the accent and its right one lighter,
/// a girdle line across), centred in a `d` box at `(x, y)`.
pub(super) fn start_mark(s: &mut Surf, x: usize, y: usize, d: usize) {
    let half = d / 2;
    let (cx, cy) = (x + half, y + half);
    let light = p::blend(k::ACCENT(), k::ON_ACCENT, 90);
    for yy in y..y + d {
        let dy = if yy >= cy { yy - cy } else { cy - yy };
        let hw = half.saturating_sub(dy); // the diamond's half-width at this row
        if hw == 0 {
            continue;
        }
        p::fill(s, (cx - hw, yy, hw, 1), k::ACCENT(), 255);
        p::fill(s, (cx, yy, hw, 1), light, 255);
    }
    p::hairline_h(s, cx - half / 2, cy - half / 3, half, k::ON_ACCENT, 120);
}

/// One tile's icon, `size` square at `(x, y)`: the registrar's icon, else our monogram.
pub(super) fn tile_icon(s: &mut Surf, pin: u8, title: &[u8], x: usize, y: usize, size: usize) {
    if pin != NO_PIN {
        if p::app_icon(s, icon_key(pin as usize), x, y, size) {
            return;
        }
        super::chrome::monogram(s, x, y, size, k::APP_HUES[pin as usize % k::APP_HUES.len()]);
        return;
    }
    super::chrome::monogram(s, x, y, size, super::chrome::hue_of(title));
}

/// Our tray glyphs, `d` square at `(x, y)` in `ink`.
pub(super) fn glyph_net(s: &mut Surf, x: usize, y: usize, d: usize, up: bool, ink: u32) {
    // Four rising bars; a down link draws them faint with a slash.
    let a = if up { 255 } else { 90 };
    let bw = (d / 6).max(1);
    for i in 0..4 {
        let h = d * (i + 1) / 4;
        p::rrect(s, (x + i * (bw + bw / 2 + 1), y + d - h, bw, h), bw / 2, ink, a);
    }
    if !up {
        p::line(s, x, y + d - 1, x + d - 1, y, k::px(1), ink, 255);
    }
}

pub(super) fn glyph_vol(s: &mut Surf, x: usize, y: usize, d: usize, level: u8, muted: bool, ink: u32) {
    let t = k::px(1);
    let (bx, by, bh) = (x + d / 8, y + d * 3 / 8, d / 4);
    p::fill(s, (bx, by, d / 6, bh + 1), ink, 255);
    // The cone.
    let cx = bx + d / 6;
    p::line(s, cx, by, cx + d / 4, y + d / 8, t, ink, 255);
    p::line(s, cx, by + bh, cx + d / 4, y + d - d / 8, t, ink, 255);
    p::hairline_v(s, cx + d / 4, y + d / 8, d - d / 4, ink, 255);
    let wx = cx + d / 4 + d / 8;
    if muted {
        p::line(s, wx, y + d / 3, wx + d / 4, y + d - d / 3, t, ink, 255);
        p::line(s, wx + d / 4, y + d / 3, wx, y + d - d / 3, t, ink, 255);
        return;
    }
    // The waves: one or two short strokes by the level.
    if level > 0 {
        p::line(s, wx, y + d * 3 / 8, wx + d / 10, y + d / 2, t, ink, 255);
        p::line(s, wx + d / 10, y + d / 2, wx, y + d * 5 / 8, t, ink, 255);
    }
    if level > 8 {
        let wx = wx + d / 6;
        p::line(s, wx, y + d / 4, wx + d / 7, y + d / 2, t, ink, 255);
        p::line(s, wx + d / 7, y + d / 2, wx, y + d * 3 / 4, t, ink, 255);
    }
}

pub(super) fn glyph_bat(s: &mut Surf, x: usize, y: usize, d: usize, pct: u16, charging: bool, ink: u32) {
    let (w, h) = (d - d / 8, d / 2);
    let (bx, by) = (x, y + (d - h) / 2);
    p::stroke_rrect(s, (bx, by, w, h), k::px(2), ink, 255);
    p::fill(s, (bx + w, by + h / 3, (d / 8).max(1), h / 3), ink, 255);
    let inner = w.saturating_sub(4 * k::px(1));
    let fw = inner * pct.min(100) as usize / 100;
    let col = if pct < 15 && !charging { k::CLOSE_HOT() } else { ink };
    p::fill(s, (bx + 2 * k::px(1), by + 2 * k::px(1), fw, h.saturating_sub(4 * k::px(1))), col, 255);
    if charging {
        let mx = bx + w / 2;
        p::line(s, mx + d / 10, by - 1, mx - d / 10, by + h / 2, k::px(1), k::ACCENT(), 255);
        p::line(s, mx - d / 10, by + h / 2, mx + d / 10, by + h + 1, k::px(1), k::ACCENT(), 255);
    }
}

/// SOFTYDOTS: the badge of a tray glyph `d` square at `(x, y)` — a disc `(bx, by, b)` at its top-right corner.
fn badge(x: usize, y: usize, d: usize) -> (usize, usize, usize) {
    let b = (d * 3 / 8).max(3);
    (x + d - b, y, b)
}

/// SOFTYDOTS: tray item `n`'s glyph box `(x, y, d)` (0 = the leftmost) in layout `l` — the painter's and the fixture's
/// one placement.
fn tray_item(l: &Layout, n: usize) -> (usize, usize, usize) {
    let (ic, ig) = (k::px(k::TRAY_ICON), k::px(k::TRAY_GAP));
    (l.tray_x + k::px(k::TRAY_PAD) + n * (ic + ig), l.ty + (l.th - ic) / 2, ic)
}

/// SOFTYDOTS: the camera in-use light — our video-camera glyph (a rounded body, the lens's flare) in `ink`, the
/// `accent` dot at its top right (the menu bar's `camdot` glyph, drawn with this look's facade).
pub(super) fn glyph_cam(s: &mut Surf, x: usize, y: usize, d: usize, ink: u32) {
    let t = k::px(1);
    let (bw, bh) = (d * 5 / 8, d / 2);
    let by = y + d * 3 / 8;
    p::stroke_rrect(s, (x, by, bw, bh), k::px(2), ink, 255);
    let lx = x + bw;
    let mid = by + bh / 2;
    p::line(s, lx, mid, x + d - 1, by, t, ink, 255);
    p::line(s, lx, mid, x + d - 1, by + bh - 1, t, ink, 255);
    p::hairline_v(s, x + d - 1, by, bh, ink, 255);
    let (bx, byy, b) = badge(x, y, d);
    p::disc(s, bx, byy, b, k::ACCENT(), 255);
}

/// SOFTYDOTS: the disk item — a drive (a rounded box, its activity light) in `ink`, the warning dot (`CLOSE_HOT`, the
/// low battery's red) at its top right while the root is under the low line.
pub(super) fn glyph_disk(s: &mut Surf, x: usize, y: usize, d: usize, ink: u32) {
    let (w, h) = (d - d / 8, d / 2);
    let by = y + d * 3 / 8;
    p::stroke_rrect(s, (x, by, w, h), k::px(2), ink, 255);
    let led = (d / 8).max(1);
    p::fill(s, (x + w - 2 * led - k::px(1), by + (h - led) / 2, led, led), ink, 255);
    let (bx, byy, b) = badge(x, y, d);
    p::disc(s, bx, byy, b, k::CLOSE_HOT(), 255);
}

/// Compose strip row `j` of the band at panel rect `r`: the face (SOFTYDARK B694: in Dark, `SURFACE` at the VEIL's
/// tint over the blurred wallpaper, DESIGN-LANGUAGE §2.4; flat in Light), then the hairline and the items.
fn paint_row(out: &mut [u32], m: &Model, l: &Layout, r: strip::Rect, j: usize) {
    let w = r.2;
    p::veil_row(out, w, r.0, r.1 + j, k::SURFACE(), k::VEIL_A());
    let mut s = Surf::row(out, w, j);
    p::hairline_h(&mut s, 0, 0, w, k::STROKE(), k::STROKE_A());
    // The tiles: Start, then the apps.
    for i in 0..=m.n {
        let x = l.x0 + i * l.pitch;
        let hot = if i == 0 { HOT_START } else { i as u8 };
        let lit = if i == 0 { m.start_open } else { m.t[i - 1].focused };
        let back = if m.pressed == hot { Some((k::PRESS(), k::PRESS_A())) } else if m.hot == hot { Some((k::HOVER(), k::HOVER_A())) } else if lit { Some((k::HOVER(), k::HOVER_A() / 2)) } else { None };
        if let Some((c, a)) = back {
            p::rrect(&mut s, (x, l.ty, l.tw, l.th), k::px(k::TILE_R), c, a);
            if m.hot == hot || lit {
                p::stroke_rrect(&mut s, (x, l.ty, l.tw, l.th), k::px(k::TILE_R), k::STROKE(), k::STROKE_A() / 2);
            }
        }
        let ic = k::px(k::TILE_ICON);
        let (ix, iy) = (x + (l.tw - ic) / 2, l.ty + (l.th - ic) / 2);
        if i == 0 {
            start_mark(&mut s, ix, iy, ic);
            continue;
        }
        let t = &m.t[i - 1];
        tile_icon(&mut s, t.pin, t.name(), ix, iy, ic);
        if t.running {
            let pw = k::px(if t.focused { k::PIP_W_ON } else { k::PIP_W });
            let ph = k::px(k::PIP_H);
            let py = l.ty + l.th - ph - k::px(1);
            p::rrect(&mut s, (x + (l.tw - pw) / 2, py, pw, ph), ph / 2, if t.focused { k::ACCENT_INK() } else { k::PIP() }, if t.pulse { PULSE_A } else { 255 }); // SOFTYDOTS: a refused window's tile dims its pip for one beat
        }
    }
    // The tray group.
    if l.tray_n > 0 {
        if m.pressed == HOT_TRAY {
            p::rrect(&mut s, (l.tray_x, l.ty, l.tray_w, l.th), k::px(k::TILE_R), k::PRESS(), k::PRESS_A());
        } else if m.hot == HOT_TRAY || m.qs_open {
            p::rrect(&mut s, (l.tray_x, l.ty, l.tray_w, l.th), k::px(k::TILE_R), k::HOVER(), if m.qs_open && m.hot != HOT_TRAY { k::HOVER_A() / 2 } else { k::HOVER_A() });
        }
        // SOFTYDOTS: the camera light and the disk item lead the group (the leftmost, as on the Una bar).
        let mut n = 0; #[cfg(feature = "uac")] { if m.mic { let (x, y, ic) = tray_item(l, n); glyph_mic(&mut s, x, y, ic, k::TEXT()); n += 1; } } // SOFTYMIC (B749): the mic light leads the tray, left of the camera's (the Una bar's order). LINE-NEUTRAL fold, code first.
        if m.cam {
            let (x, y, ic) = tray_item(l, n);
            glyph_cam(&mut s, x, y, ic, k::TEXT());
            n += 1;
        }
        if m.disk != 0 {
            let (x, y, ic) = tray_item(l, n);
            glyph_disk(&mut s, x, y, ic, k::TEXT());
            n += 1;
        }
        if m.net != 0 {
            let (x, y, ic) = tray_item(l, n);
            glyph_net(&mut s, x, y, ic, m.net == 2, k::TEXT());
            n += 1;
        }
        if m.vol != 0 {
            let (x, y, ic) = tray_item(l, n);
            glyph_vol(&mut s, x, y, ic, (m.vol & 0x7F) - 1, m.vol & 0x80 != 0, k::TEXT());
            n += 1;
        }
        if m.bat != 0 {
            let (x, y, ic) = tray_item(l, n);
            glyph_bat(&mut s, x, y, ic, (m.bat & 0x7FFF) - 1, m.bat & 0x8000 != 0, k::TEXT());
        }
    }
    // The clock: time over date, right-aligned in its box.
    if m.pressed == HOT_CLOCK {
        p::rrect(&mut s, (l.clock_x, l.ty, l.clock_w, l.th), k::px(k::TILE_R), k::PRESS(), k::PRESS_A());
    } else if m.hot == HOT_CLOCK || m.cal_open {
        p::rrect(&mut s, (l.clock_x, l.ty, l.clock_w, l.th), k::px(k::TILE_R), k::HOVER(), if m.cal_open { k::HOVER_A() / 2 } else { k::HOVER_A() });
    }
    let face = Face::Ui;
    let ch = face.cell_h();
    let right = l.clock_x + l.clock_w - k::px(k::TRAY_PAD);
    if m.clock_set {
        let mid = l.ty + l.th / 2;
        let tw = p::text_w(&m.clock, false, face);
        p::text(&mut s, right - tw, mid.saturating_sub(ch), tw + 1, &m.clock, k::TEXT(), false, face);
        let dw = p::text_w(&m.date, false, face);
        p::text(&mut s, right - dw, mid, dw + 1, &m.date, k::TEXT(), false, face);
    } else {
        let tw = p::text_w(b"--:--", false, face);
        p::text(&mut s, right - tw, l.ty + (l.th - ch) / 2, tw + 1, b"--:--", k::TEXT_2(), false, face);
    }
}

/// The pointer's hover target on the band (strip-local), from the pointer's panel position.
fn hover(m: &Model, l: &Layout, r: strip::Rect, cx: i32, cy: i32) -> u8 {
    if cx < 0 || cy < 0 {
        return HOT_NONE;
    }
    let (x, y) = (cx as usize, cy as usize);
    if y < r.1 || y >= r.1 + r.3 || x >= r.2 {
        return HOT_NONE;
    }
    m.hit(l, x - r.0, y - r.1)
}

fn announce(m: &Model, h: usize) {
    let key = (m.n as u64) | (m.tray_n() as u64) << 16 | (h as u64) << 32;
    if BUILT.swap(key, Ordering::Relaxed) != key {
        serial_println!("[softy] taskbar built tiles={} tray={} h={}", m.n + 1, m.tray_n() + 1, h);
    }
    // SOFTYDOTS: the three signals' edges.
    let o = m.pulse_owner();
    let dk = (m.cam as u64) | (m.disk as u64) << 1 | (o & 0xFFFF_FFFF) << 16; #[cfg(feature = "uac")] mic_wire(m.mic); // SOFTYMIC (B749): `[mic] dot=<on|off> by= look=softy` on each edge (file tail). LINE-NEUTRAL fold, code first.
    if DOTS.swap(dk, Ordering::Relaxed) != dk {
        let mut pct = [b'-'; 3];
        let pl = if m.disk == 0 { 1 } else { let v = m.disk - 1; if v >= 100 { pct = *b"100"; 3 } else if v >= 10 { pct[0] = b'0' + v / 10; pct[1] = b'0' + v % 10; 2 } else { pct[0] = b'0' + v; 1 } };
        serial_println!("[softy] tray cam={} disk={} pulse={}", if m.cam { "on" } else { "off" }, core::str::from_utf8(&pct[..pl]).unwrap_or("-"), o);
    }
}

/// SOFTYDOTS: the session is locked (`video::lock`'s veil) — the band leaves the glass, the login card owns it.
#[inline]
fn locked() -> bool {
    #[cfg(feature = "login")]
    {
        return crate::video::lock::veiled();
    }
    #[allow(unreachable_code)]
    {
        false
    }
}

/// **The band's pass** (from `dock::compose` under Softy, inside `strip::compose_all`). `true` = it painted.
pub fn compose() -> bool {
    let Some(fb) = crate::video::panel_snapshot().filter(|f| f.is_ready()) else { return false };
    let (pw, ph) = (fb.width(), fb.height());
    // SOFTYDOTS (M2): one lock, two paints — under Softy the locked view has no taskbar (vacated once per lock).
    if locked() {
        if SLOT.packed() != 0 {
            serial_println!("[softy] taskbar hidden reason=lock");
            return vacate();
        }
        return false;
    }
    let (cx, cy) = crate::pal::cursor::pos(pw as i32, ph as i32);
    super::chrome::hover_track(cx, cy);
    let r = rect(pw, ph);
    let Some(r) = r else {
        if SLOT.packed() != 0 {
            let old = SLOT.rect();
            SLOT.clear();
            return strip::vacate("softy", old, None, false);
        }
        return false;
    };
    let (m, clobbered) = {
        let Some(mut rows) = ROWS.try_lock() else { return false };
        let (n, clob) = wm::dock_scan(&mut rows, SLOT.rect());
        (Model::read(&rows[..n]), clob)
    };
    let mut m = m;
    let l = m.layout(pw);
    m.hot = hover(&m, &l, r, cx, cy);
    // SOFTY2: the pressed state, latched from the pointer's level (it was drawn but never set).
    m.pressed = if crate::pal::cursor::button_down() { m.hot } else { HOT_NONE };
    announce(&m, r.3);
    let sig = m.signature(r);
    if sig == SLOT.sig() && SLOT.packed() == strip::pack_rect(Some(r)) && !clobbered {
        return false;
    }
    let old = SLOT.packed();
    strip::settle("softy", Some(r));
    if !strip::paint("softy", r, |out, j| paint_row(out, &m, &l, r, j)) {
        return false;
    }
    if old != 0 && old != strip::pack_rect(Some(r)) {
        strip::vacate("softy", strip::unpack_rect(old), Some(r), false);
    }
    SLOT.store(sig, Some(r));
    true
}

/// The band leaves the glass (a switch back to the UnaOS look): its pixels are erased once, the slot cleared.
pub fn vacate() -> bool {
    if SLOT.packed() == 0 {
        return false;
    }
    let r = SLOT.rect();
    SLOT.clear();
    BUILT.store(u64::MAX, Ordering::Relaxed);
    strip::vacate("softy", r, None, false)
}

/// The band's rect while it is on the glass (the fixture's geometry read).
pub fn painted_rect() -> Option<strip::Rect> {
    if SLOT.packed() == 0 { None } else { Some(SLOT.rect()) }
}

fn panel() -> Option<(usize, usize)> {
    let fb = *crate::video::WRITER.lock();
    if !fb.is_ready() {
        return None;
    }
    Some((fb.width(), fb.height()))
}

/// **The band's press arm** (from `dock::press_at` under Softy, inside `strip::press_route`): `true` = consumed.
pub fn press_at(x: i32, y: i32) -> bool {
    if x < 0 || y < 0 || locked() {
        return false;
    }
    let Some((pw, ph)) = panel() else { return false };
    let open = super::startmenu::open_kind();
    if super::startmenu::press_inside(x, y) {
        return true;
    }
    let Some(r) = rect(pw, ph) else { return false };
    let (px, py) = (x as usize, y as usize);
    let on_bar = py >= r.1 && py < r.1 + r.3 && px < r.2;
    let m = {
        let mut rows = ROWS.lock();
        let (n, _) = wm::dock_scan(&mut rows, (0, 0, 0, 0));
        Model::read(&rows[..n])
    };
    let l = m.layout(pw);
    let hot = if on_bar { m.hit(&l, px - r.0, py - r.1) } else { HOT_NONE };
    // An open flyout closes on any press outside it — unless that press is its own toggle, which toggles it.
    if open != 0 && !((open == 1 && hot == HOT_START) || (open == 2 && hot == HOT_CLOCK) || (open == 3 && hot == HOT_TRAY)) {
        super::startmenu::close("outside");
    }
    if !on_bar {
        return false;
    }
    match hot {
        HOT_START => {
            super::startmenu::toggle(1, "button");
        }
        // SOFTY2: the clock opens the notification calendar — the month with the notification summary over it.
        HOT_CLOCK => {
            super::startmenu::toggle(2, "button");
        }
        // SOFTY2: the tray opens QUICK SETTINGS (sound, Wi-Fi, battery); its gear launches Settings.
        HOT_TRAY => {
            super::startmenu::toggle(3, "button");
        }
        HOT_NONE => {}
        i => tile_press(&m.t[i as usize - 1]),
    }
    true
}

/// A tile's verb: raise its running window (minimise it when it is already the focused, visible one), else launch.
fn tile_press(t: &Tile) {
    PRESSES.fetch_add(1, Ordering::Relaxed);
    let name = core::str::from_utf8(t.name()).unwrap_or("?");
    if t.running && t.win != wm::WIN_NONE {
        if t.focused && t.visible {
            let out = wm::minimise(t.win);
            serial_println!("[softy] tile press app={} -> minimise win={} {}", name, t.win, out);
        } else {
            let ok = wm::wl_focus(t.win);
            serial_println!("[softy] tile press app={} -> raise win={} ok={}", name, t.win, ok as u8);
        }
        return;
    }
    if t.pin != NO_PIN {
        let how = dock::launch_named(dock::DP_PINS[t.pin as usize].name).unwrap_or("absent");
        serial_println!("[softy] tile press app={} -> launch {}", name, how);
    }
}

/// Fixture: the band's tile count (Start included) and the strip-local centre of tile `i` (0 = Start) on a
/// `pw` x `ph` panel, from the same model and layout the painter uses.
pub fn probe(pw: usize, ph: usize, i: usize) -> Option<(usize, usize, usize)> {
    let r = rect(pw, ph)?;
    let m = {
        let mut rows = ROWS.lock();
        let (n, _) = wm::dock_scan(&mut rows, (0, 0, 0, 0));
        Model::read(&rows[..n])
    };
    let l = m.layout(pw);
    if i > m.n {
        return None;
    }
    Some((m.n + 1, r.0 + l.x0 + i * l.pitch + l.tw / 2, r.1 + l.ty + l.th / 2))
}

/// Fixture: the pin index of tile `i` (1-based past Start), when it names a table app.
pub fn probe_pin(i: usize) -> Option<usize> {
    let m = {
        let mut rows = ROWS.lock();
        let (n, _) = wm::dock_scan(&mut rows, (0, 0, 0, 0));
        Model::read(&rows[..n])
    };
    if i == 0 || i > m.n || m.t[i - 1].pin == NO_PIN { None } else { Some(m.t[i - 1].pin as usize) }
}

/// SOFTY2 fixture: the panel centre of the clock (`which` = 2, it opens the calendar) or the tray group (3, quick
/// settings) on a `pw` x `ph` panel, from the painter's own model and layout; `None` when the band has no such
/// target (no tray item is known yet).
pub fn probe_hot(pw: usize, ph: usize, which: u8) -> Option<(usize, usize)> {
    let r = rect(pw, ph)?;
    let m = {
        let mut rows = ROWS.lock();
        let (n, _) = wm::dock_scan(&mut rows, (0, 0, 0, 0));
        Model::read(&rows[..n])
    };
    let l = m.layout(pw);
    let y = r.1 + l.ty + l.th / 2;
    match which {
        2 => Some((r.0 + l.clock_x + l.clock_w / 2, y)),
        3 if l.tray_w > 0 => Some((r.0 + l.tray_x + l.tray_w / 2, y)),
        _ => None,
    }
}

// ── SOFTYDOTS (rmbp-ledger B721) fixture probes: the painter's own model, layout and row painter, offscreen ──────────

/// The band's live model (the press arm's read, no hover).
#[cfg(feature = "witness")]
fn model_now() -> Model {
    let mut rows = ROWS.lock();
    let (n, _) = wm::dock_scan(&mut rows, (0, 0, 0, 0));
    Model::read(&rows[..n])
}

/// Strip-local pixel `(x, y)` of the band painted from model `m` on a `pw` x `ph` panel (one row, offscreen: the glass
/// is not touched).
#[cfg(feature = "witness")]
fn px_at(m: &Model, pw: usize, ph: usize, x: usize, y: usize) -> u32 {
    let Some(r) = rect(pw, ph) else { return 0 };
    let l = m.layout(pw);
    let mut row = alloc::vec![0u32; r.2];
    paint_row(&mut row, m, &l, r, y);
    row.get(x).copied().unwrap_or(0)
}

/// What `tests softydots` reads: each signal's pixel painted ON and OFF from the live model with that one input
/// changed (a synthetic stream count, a forced low disk, a pulsing tile), the colour each must be, and the live reads.
#[cfg(feature = "witness")]
pub struct Dots {
    pub cam_on: u32,
    pub cam_off: u32,
    pub accent: u32,
    pub disk_on: u32,
    pub disk_off: u32,
    pub warn: u32,
    pub pip_on: u32,
    pub pip_off: u32,
    pub pip: u32,
    /// The live model's camera light agrees with the driver's count.
    pub cam_live: bool,
    /// A live tile's owner given the beat (`wm::attention`) reads `pulse` back (`None`: no running tile).
    pub pulse_live: Option<bool>,
}

/// SOFTYDOTS fixture (M3): the three signals through [`paint_row`] on a `pw` x `ph` panel. `None` when the panel
/// cannot host the band.
#[cfg(feature = "witness")]
pub fn dots_probe(pw: usize, ph: usize) -> Option<Dots> {
    rect(pw, ph)?;
    let mut base = model_now();
    #[cfg(feature = "uvc")]
    let cam_live = base.cam == (crate::drivers::xhci::uvc::open_streams() != 0);
    #[cfg(not(feature = "uvc"))]
    let cam_live = !base.cam;
    base.hot = HOT_NONE;
    base.pressed = HOT_NONE; #[cfg(feature = "uac")] { base.mic = false; } // SOFTYMIC (B749): a live mic light would take tray item 0 from the camera and disk legs. LINE-NEUTRAL fold, code first.
    let centre = |m: &Model| {
        let l = m.layout(pw);
        let (x, y, d) = tray_item(&l, 0);
        let (bx, by, b) = badge(x, y, d);
        (bx + b / 2, by + b / 2)
    };
    // 1. The camera: a synthetic count of 1 -> the accent dot at the tray's first item; 0 -> no dot there.
    let (mut on, mut off) = (base, base);
    on.cam = true;
    off.cam = false;
    let (cx, cy) = centre(&on);
    let (cam_on, cam_off) = (px_at(&on, pw, ph, cx, cy), px_at(&off, pw, ph, cx, cy));
    // 2. The disk: forced low (3 % free) with no camera -> the warning dot leads the tray; not low -> none.
    let (mut on, mut off) = (base, base);
    on.cam = false;
    on.disk = 4;
    off.cam = false;
    off.disk = 0;
    let (dx, dy) = centre(&on);
    let (disk_on, disk_off) = (px_at(&on, pw, ph, dx, dy), px_at(&off, pw, ph, dx, dy));
    // 3. The pulse: the first tile running and unfocused, its pip's centre with the beat off and on.
    let mut m = base;
    if m.n == 0 {
        m.n = 1;
        m.t[0] = Tile::empty();
    }
    m.t[0].running = true;
    m.t[0].focused = false;
    let l = m.layout(pw);
    let ph_ = k::px(k::PIP_H);
    let (px, py) = (l.x0 + l.pitch + l.tw / 2, l.ty + l.th - ph_ - k::px(1) + ph_ / 2);
    m.t[0].pulse = false;
    let pip_off = px_at(&m, pw, ph, px, py);
    m.t[0].pulse = true;
    let pip_on = px_at(&m, pw, ph, px, py);
    // 4. The live beat: a running tile's owner given `wm::attention` reads back pulsing (the dock's own state).
    let pulse_live = base.t[..base.n].iter().find(|t| t.running && t.owner != 0).map(|t| t.owner).map(|o| {
        wm::attention(o, "for=tests");
        let m2 = model_now();
        m2.t[..m2.n].iter().any(|t| t.owner == o && t.pulse)
    });
    Some(Dots { cam_on, cam_off, accent: k::ACCENT(), disk_on, disk_off, warn: k::CLOSE_HOT(), pip_on, pip_off, pip: k::PIP(), cam_live, pulse_live })
}

// ── SOFTYMIC (rmbp-ledger B749, BP-video-24) — TAIL-APPENDED: the microphone's in-use light in the Softy look ──────────
// The camera light's shape for the microphone: while the USB Audio Class capture pipe runs (`uac::capture_armed`, the
// driver's own count — the menu bar's `micdot` reads the same call) the tray's first item is a microphone glyph with
// the `accent` badge, left of the camera's light. Erased whole with `uac` off (every call site is a same-line fold).

/// The last mic-light state `[mic] dot=` said (`0` = off: nothing is said before the first light).
#[cfg(feature = "uac")]
static MIC_SAID: AtomicU64 = AtomicU64::new(0);

/// SOFTYMIC: the microphone — a capsule, its cradle, the stem and the foot in `ink`, the `accent` badge at its top right
/// (the menu bar's `micdot` glyph, drawn with this look's facade).
#[cfg(feature = "uac")]
pub(super) fn glyph_mic(s: &mut Surf, x: usize, y: usize, d: usize, ink: u32) {
    let t = k::px(1);
    let (cw, ch) = ((d * 5 / 16).max(2), d * 9 / 16);
    let cx0 = x + d * 5 / 16;
    let mx = cx0 + cw / 2;
    p::stroke_rrect(s, (cx0, y + d / 8, cw, ch), cw / 2, ink, 255);
    let (side, lip, low) = (d * 9 / 32, y + d * 7 / 16, y + d * 11 / 16);
    let (lx, rx) = (mx.saturating_sub(side), mx + side);
    let bot = y + d * 12 / 16;
    p::line(s, lx, lip, lx + d / 16, low, t, ink, 255);
    p::line(s, lx + d / 16, low, mx, bot, t, ink, 255);
    p::line(s, mx, bot, rx - d / 16, low, t, ink, 255);
    p::line(s, rx - d / 16, low, rx, lip, t, ink, 255);
    p::hairline_v(s, mx, bot, d * 3 / 16, ink, 255);
    p::hairline_h(s, mx.saturating_sub(d / 8), y + d * 15 / 16, d / 4, ink, 255);
    let (bx, byy, b) = badge(x, y, d);
    p::disc(s, bx, byy, b, k::ACCENT(), 255);
}

/// SOFTYMIC: `[mic] dot=<on|off> by=<app|-> look=softy` on each edge of the band's mic light.
#[cfg(feature = "uac")]
fn mic_wire(on: bool) {
    if MIC_SAID.swap(on as u64, Ordering::Relaxed) == on as u64 {
        return;
    }
    let mut nb = [0u8; 32];
    let (_, n) = crate::audio::mic_holder(&mut nb);
    let by = core::str::from_utf8(&nb[..n]).ok().filter(|b| !b.is_empty()).unwrap_or("-");
    serial_println!("[mic] dot={} by={} look=softy", if on { "on" } else { "off" }, by);
}

/// SOFTYMIC fixture (M3): the tray's first item painted through [`paint_row`] from the live model with the mic light
/// on and off (camera off, so the mic leads), offscreen: `(badge centre on, off, accent, live model agrees with the
/// driver)`. `None` when the panel cannot host the band.
#[cfg(all(feature = "uac", feature = "witness"))]
pub fn mic_probe(pw: usize, ph: usize) -> Option<(u32, u32, u32, bool)> {
    rect(pw, ph)?;
    let mut base = model_now();
    let live = base.mic == (crate::drivers::usb::uac::capture_armed() != 0);
    base.hot = HOT_NONE;
    base.pressed = HOT_NONE;
    let (mut on, mut off) = (base, base);
    on.mic = true;
    on.cam = false;
    off.mic = false;
    off.cam = false;
    let l = on.layout(pw);
    let (x, y, d) = tray_item(&l, 0);
    let (bx, by, b) = badge(x, y, d);
    let (cx, cy) = (bx + b / 2, by + b / 2);
    Some((px_at(&on, pw, ph, cx, cy), px_at(&off, pw, ph, cx, cy), k::ACCENT(), live))
}
