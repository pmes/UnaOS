// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! PI-UI-2 — the always-on GUI status strip.
//!
//! A single line pinned to the bottom of the panel that surfaces the network / time state a bench
//! user would otherwise only see on the serial console: the mDNS hostname, the settled interface IP
//! (or a `no lease` placeholder), and the wall-clock (or `unsynced` before SNTP has seeded the
//! clock). It is drawn by the render task every frame — after the console repaint, so it always sits
//! on top — and refreshed at ~1 Hz by a periodic wake (see `status_tick` in `main.rs`).
//!
//! **Read-only by construction.** It consumes only public snapshot accessors: [`crate::clock::now`]
//! for the wall clock and [`crate::net_phy::settled_ipv4`] for the interface address. Both are plain
//! atomic / short-lock reads safe to call from the render core while the net + clock state is owned
//! by other cores — no IRQ-masked hold is taken in the render path.
//!
//! All geometry derives from [`crate::ui::Metrics`] (THE METRICS RULE — no absolute pixel sizes), so
//! the strip reads correctly on every panel from the 640×480 QEMU surface to a Retina panel.

//! ## PULSE-2 — the always-running per-core CPU pulse, as an instrument panel
//!
//! PULSE-STRIP put the pulse INSIDE this one-line band as right-aligned per-core bars. On the bench
//! panel `ui::Metrics::for_height(1200)` yields `scale=1`, so the band is 12 px and each bar came out
//! ~30x4 px — about a millimetre tall. Peter's correction at the bench, verbatim: *"i meant for pulse
//! to be in the ~20mm high gap at the bottom of the screen below the other windows not in your fake
//! bar at the bottom like 1mm tall and 4mm long. don't try to fake a desktop just build test tools."*
//!
//! Two things in that are binding and general, and they are why this module now grows a SECOND band:
//!
//! 1. **This panel is a test-tool surface, not a desktop imitation.** There is no taskbar to dock
//!    into and no chrome to imitate; an instrument gets the room it needs to be read at arm's length.
//!    Sizing an instrument to fit inside existing chrome is how it became unreadable.
//! 2. **The bottom gap is real estate, and it was standing empty.** The window tiler packs boxes from
//!    the top; below the lowest window row there is a permanently unused strip of panel above this
//!    status band. That is where the pulse belongs.
//!
//! So the pulse now lives in [`panel_geometry`]'s band — a full-width instrument seated directly above
//! the status line, ~1/13 of the panel tall (92 px on the 1920x1200 bench panel, and never less than
//! three line-pitches), carrying ONE ROW PER CORE: a `c<N>` label, an LED bar tens of pixels tall, and
//! the percent. THE METRICS RULE still holds — every dimension is a function of the panel height or of
//! [`crate::ui::Metrics`], and the band scales from the 640x480 QEMU surface up.
//!
//! ### The LED bar — sensitivity from width, smoothness from gradient
//!
//! Peter again, once the band existed: *"if pulse spans the entire bottom width of the screen there
//! will be more leds to show sensitivity. with the better graphics can you have a gradient inside each
//! led so it scales super smooth."* Both halves are design requirements and they pull on different
//! parts of the meter:
//!
//! * **Sensitivity is LED COUNT, and LED count is width.** [`row_geometry`] stacks the cores as
//!   full-width rows rather than side-by-side quarters precisely so every core gets the whole bar —
//!   ~140 LEDs each on the bench panel instead of ~37. That is the resolution a load change has to
//!   clear before it is visible at all.
//! * **Smoothness is the FILL LENGTH, and the gradient is what makes a length out of lamps.**
//!   [`draw_led_bar`] computes the load's fill as a continuous pixel length; whole LEDs inside it burn
//!   full, the one LED the boundary lands inside is lit in proportion to its coverage, and every LED
//!   carries its own vertical lens gradient. A rising load brightens the next lamp continuously
//!   instead of clicking it on. The stored load went to per-mille for the same reason (a 1% quantum is
//!   a 14 px jump on a 1400 px bar) — see [`classify_load_scaled`], which states the VUG-HONESTY
//!   rule once for both scales.
//! * The scale runs green → amber → red across the bar (colour by POSITION, so a given lamp's colour
//!   is stable and only the length moves). Test instrument, not chrome.
//!
//! The status band itself goes back to what it was before PULSE-STRIP: **text only**, host / ip /
//! clock. The miniature bars are superseded. What PULSE-STRIP got right is kept verbatim and is the
//! substrate here: the same `sched::meter_cpu_*` relaxed reads, the same meter palette and
//! VUG-HONESTY load rule (both now owned by this module — see [`classify_load_scaled`]), the same
//! dirty pacing, the same zero-new-threads plumbing.
//!
//! ### Reserving the band honestly
//!
//! An instrument the tiler can park a window on top of is not an instrument. [`chrome_h`] is the
//! reservation: `wm::place` subtracts it from the panel's vertical budget, so no tiled window box is
//! ever laid out into the pulse band or the status band. This is a tiler bottom-margin rather than a
//! `wcf::reserved`-style box list because the two mechanisms answer different questions — WC-F's list
//! exists so the compositor can *refuse to paint a probe* over a window that is already there, while
//! this must stop the window from being placed in the first place. It also stays clear of
//! `wm::occluders`/WC-I entirely: those govern who wins where regions DO overlap, and after the
//! reservation they no longer overlap. (Occlusion is still inherited for free — this band draws into
//! the `Screen` back buffer and `Screen::present_background` subtracts `wm::occluders()` from every
//! damaged row — so an explicitly `move_to`-pinned window is still handled correctly.)
//!
//! WC-F's own reserved boxes DO sit in these rows (its ramp is 256 px tall against the bottom-left
//! corner, its twins 64 px against the bottom-right) and it paints them straight to the framebuffer at
//! the tail of every composite. So [`panel_geometry`] narrows the pulse band to the horizontal span
//! WC-F leaves free rather than fighting it — 1480 px of the 1920 on the bench panel, which is more
//! width than four cores need.
//!
//! Focus is untouched: nothing here is a view, so nothing here can take TAB or a click.
//!
//! Dirty pacing keeps PULSE-STRIP's contract and only recalibrates its threshold for the finer
//! display. [`tick`] samples at most once per [`PSTRIP_PERIOD_MS`] and draws only when the composed
//! status line changed OR a core's **lit length moved by at least one pixel** — the finest difference
//! this meter can actually show. An idle panel with an unsynced clock still presents nothing at all
//! (an idle core's per-mille load is a hard 0 and its length does not move), so the idle redraw rate is
//! unchanged at ~0/s; the ceiling stays one redraw per sample, i.e. 1 Hz, well under the spec's 5.0/s
//! busy-loop FORBID. The `[pstrip]` rollup reports samples and redraws so the pacing stays checkable.

use crate::pal::GneissPal;
use alloc::format;
use alloc::string::String;
use spin::Mutex;

/// The mDNS / DNS-SD host name the Pi answers on the share segment (net11/net17). A fixed literal —
/// the strip names it for the operator; it is not derived from any driver's private state.
const HOSTNAME: &str = "unaos.local";

/// Strip background (a shade darker than the Moonstone console background, so the bar reads as a
/// distinct chrome band rather than more terminal).
const STRIP_BG: u32 = 0x1B1A3A;
/// Strip foreground text (Aqua — legible on the dark band, distinct from the grey history text).
const STRIP_FG: u32 = 0x7BD0E0;

/// PULSE-2 instrument-panel background — darker than the status band, so the two bottom bands read as
/// two distinct instruments rather than one thick smear of chrome.
const PANEL_BG: u32 = 0x0E0D22;
/// PULSE-2 label / percent text.
const PANEL_FG: u32 = 0x9FB4C8;

// ---------------------------------------------------------------------------------------------
// The meter palette and the VUG-HONESTY display rule.
//
// These are the pulse's own primitives. They were carried by the in-kernel `vug` demo module while
// that module existed and the strip borrowed them; the demo is gone and the strip is the only
// consumer, so they live here now. The rationale comments below are the originals, verbatim.
// ---------------------------------------------------------------------------------------------

// Meter palette.
const METER_DIM: u32 = 0x00_2A2432;
/// PULSE-ALIVE breath colour — clearly brighter than `METER_DIM`, dimmer than a load fill: the one
/// sweeping segment an idle-but-scheduled core lights so "alive and idle" reads at a glance.
const METER_BREATH: u32 = 0x00_5F4E86;
/// Parked dash colour — cooler/dimmer than `METER_DIM` so a broken track reads as "not participating".
const METER_PARKED: u32 = 0x00_3A3550;

/// VUG-HONESTY parked-core marker (a load-array sentinel, disjoint from the 0..=100 percent range). A
/// core whose pulse counters are frozen this window AND that is NOT the demo core is parked /
/// never-scheduled: the display must not fabricate load for it. `load[c] == PARKED` selects a distinct
/// DASHED bar (see [`draw_led_bar`]) — visually separable from an idle 0% bar (a solid dim track) so
/// "idle" and "never woken" never read alike, and the percent column prints `park` instead of a number.
const PARKED: u32 = u32::MAX;

/// VUG-HONESTY — the pure per-core display decision. Given one core's per-window busy/idle tick deltas
/// (`db`/`di`), whether it is the *demo core* (the core executing this render loop), and that loop's own
/// measured render busy% (`own_load`), return the load to display (0..=100) or [`PARKED`]:
///   * `db + di > 0`  → the scheduler accounted this core this window: honest busy fraction.
///   * frozen + demo  → the demo core runs OUTSIDE the scheduler, so its own counters freeze; credit its
///                      measured render load — the honest number for the core doing the drawing.
///   * frozen + other → a core with no scheduling activity this window that is NOT drawing: parked /
///                      never-woken. NEVER fabricate load. (The pre-fix code credited `own_load` to
///                      EVERY frozen core, so a parked AP mirrored the busy demo core and read PINNED —
///                      the display-honesty defect this arc closes. The merged idle/busy-heartbeats made
///                      the *counters* honest and passed the one-shot boot witness, but the LIVE meter
///                      samples per-window deltas: a parked EL2 secondary gets no periodic wake, so its
///                      counters are frozen between windows and this fallback still fabricated.) Report
///                      PARKED instead.
fn classify_load(db: u64, di: u64, is_demo: bool, own_load: u32) -> u32 {
    classify_load_scaled(db, di, is_demo, own_load, 100)
}

/// PULSE-2 — [`classify_load`] at an arbitrary full-scale, so a display with more resolution than
/// "one bar in ten" can have more resolution than one percent.
///
/// The honesty rule is stated ONCE, here; `classify_load` is this function at `full = 100` and the
/// VUG-HONESTY witness therefore still covers every branch of it. The instrument panel calls it at
/// `full = 1000` (per-mille): its LED bar is ~1400 px wide on the bench panel, so a 1% quantum would
/// be a 14 px jump — the display would step where the machine is smooth, which is exactly the
/// "sensitivity" Peter asked the full width to buy. `own_load` is a percent by contract and is scaled
/// to match. [`PARKED`] is `u32::MAX` and stays disjoint from `0..=full` at any sane scale.
fn classify_load_scaled(db: u64, di: u64, is_demo: bool, own_load: u32, full: u32) -> u32 {
    if db + di > 0 {
        ((db * full as u64) / (db + di)) as u32
    } else if is_demo {
        (own_load as u64 * full as u64 / 100) as u32
    } else {
        PARKED
    }
}

/// VUG-HONESTY witness — deterministic, arch-neutral, framebuffer-free. Exercises [`classify_load`]
/// over the cases the honesty rule must separate and emits one PASS/FAIL serial line. Wired into the
/// `virt` CAPSTONE boot (`arch::sched::run_capstone_boot_core`), so `test-arm` and the GICv3 suite
/// witness a parked core reading PARKED rather than a fabricated pinned bar. Returns true on PASS.
pub fn parked_display_witness() -> bool {
    let busy = classify_load(8, 0, false, 99) == 100; // scheduled + fully busy
    let idle = classify_load(0, 2, false, 99) == 0; // scheduled + idle → honest 0%, NOT own_load
    let half = classify_load(1, 1, false, 0) == 50; // scheduled + half busy
    let demo = classify_load(0, 0, true, 42) == 42; // frozen demo core → its own render load
    let park = classify_load(0, 0, false, 99) == PARKED; // frozen non-demo core → PARKED, not fabricated
    let pass = busy && idle && half && demo && park;
    serial_println!(
        ":: VUG-HONESTY: parked-core display witness {} — a frozen non-demo core reads PARKED (never the demo core's load); scheduled cores read their busy fraction, the demo core its render load ::",
        if pass { "PASS" } else { "FAIL" }
    );
    pass
}

/// The settled interface IPv4 (leased or static-fallback), or `None` before any bring-up completed.
/// Wrapped so the strip compiles on a net-less kernel (the `net_phy` module is gated on the net
/// features): with no net stack it simply reports "no lease".
fn settled_ip() -> Option<[u8; 4]> {
    #[cfg(any(
        feature = "net4",
        feature = "vnet",
        feature = "smolnet",
        feature = "genet"
    ))]
    {
        crate::net_phy::settled_ipv4().map(|(ip, _leased)| ip)
    }
    #[cfg(not(any(
        feature = "net4",
        feature = "vnet",
        feature = "smolnet",
        feature = "genet"
    )))]
    {
        None
    }
}

/// Compose the strip's single line: `unaos.local   ip <a.b.c.d>|no lease   <YYYY-MM-DD HH:MM:SS UTC>|unsynced`.
fn compose() -> String {
    let ip = match settled_ip() {
        Some(ip) => format!("ip {}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]),
        None => String::from("no lease"),
    };
    let time = match crate::clock::now() {
        Some(t) => format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
            t.year, t.month, t.day, t.hour, t.min, t.sec
        ),
        None => String::from("unsynced"),
    };
    format!("{}   {}   {}", HOSTNAME, ip, time)
}

/// Draw the status strip along the bottom line of `pal`. Clears its own band to the strip background
/// first (so it always renders cleanly over whatever the console left there), then draws the text
/// vertically centred within the band. Marks only its one-line band as damage — a ~1-line flush per
/// frame, negligible at the 1 Hz refresh cadence.
///
/// PULSE-2: unconditional, and it draws BOTH bottom bands — the status line and the pulse panel above
/// it. This is the path a Key/Button pass takes, where the console has just repainted over both and
/// they owe an unconditional redraw on top whether or not anything they display changed. The pulse
/// bars are drawn from the LAST SAMPLED loads — no resample, so a keystroke storm cannot turn into a
/// telemetry-read storm. [`tick`] is the paced entry point.
pub fn draw<P: GneissPal>(pal: &mut P) {
    let m = pal.metrics();
    let w = pal.width() as usize;
    let h = pal.height() as usize;
    // Pin to the last line-pitch band. `saturating_sub` keeps a degenerate/tiny panel in-bounds.
    let band_y = h.saturating_sub(m.line_h);

    pal.draw_rect(0, band_y, w, m.line_h, STRIP_BG);

    // Vertically centre the glyph cell within the line-pitch band (the band is `line_h`, the glyph
    // is `cell_h`; the half-cell leading splits above/below). PULSE-2: text only — the miniature bars
    // that shared this line are superseded by the instrument panel above it.
    let text_y = band_y + (m.line_h.saturating_sub(m.cell_h)) / 2;
    pal.draw_text(m.margin, text_y, &compose(), STRIP_FG);
    draw_panel(pal);
}

// ---------------------------------------------------------------------------------------------
// PULSE-STRIP — per-core CPU bars docked in the strip's right-hand end.
// ---------------------------------------------------------------------------------------------

/// Upper bound on the rows the instrument will draw. The band is a fixed fraction of the panel and
/// splits into one row per core, so past this the rows stop being tall enough to read: a wider machine
/// simply shows its first `PSTRIP_MAX_CPUS` cores. (Capped for that reason, not for want of counters —
/// `sched::meter_cpu_count()` may well report more.)
pub const PSTRIP_MAX_CPUS: usize = 8;

/// The pulse sample period. One sample per second is the strip's own refresh cadence; sampling
/// faster would only add telemetry reads the 1 Hz redraw could never show.
pub const PSTRIP_PERIOD_MS: u64 = 1000;

/// Rollup period for the `[pstrip]` witness.
const PSTRIP_ROLLUP_MS: u64 = 10_000;

/// The strip's pulse state. Owned by the render task (the only caller) but held behind a lock so the
/// module has no `static mut`: one uncontended acquire per second. The *telemetry* reads it feeds on
/// (`sched::meter_cpu_ticks`) are lock-free relaxed loads taken outside any scheduler path — the
/// introspection-only contract `pulse`/`top` already work under.
struct PulseState {
    /// Whether `prev` has been seeded (the first window has no delta to report).
    armed: bool,
    ncpu: usize,
    /// Previous `(busy, idle)` tick snapshot per core.
    prev: [(u64, u64); PSTRIP_MAX_CPUS],
    /// Last window's displayed load per core (`0..=100`, or [`PARKED`]).
    load: [u32; PSTRIP_MAX_CPUS],
    /// `ms()` of the last sample.
    last_ms: u64,
    /// FNV-1a of the last composed text line — the cheap "did the text change" test that avoids
    /// keeping a `String` alive across frames.
    text_hash: u64,
    /// Rollup accumulators.
    samples: u64,
    redraws: u64,
    rollup_ms: u64,
}

static PULSE: Mutex<PulseState> = Mutex::new(PulseState {
    armed: false,
    ncpu: 0,
    prev: [(0, 0); PSTRIP_MAX_CPUS],
    load: [0; PSTRIP_MAX_CPUS],
    last_ms: 0,
    text_hash: 0,
    samples: 0,
    redraws: 0,
    rollup_ms: 0,
});

/// FNV-1a over the composed line. Only used to detect change, never stored as content.
fn hash(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// Full scale of the stored per-core load. Per-mille, not percent: the bar is ~1400 px wide on the
/// bench panel, so a 1% quantum would move the fill 14 px at a time and the "super smooth" scaling the
/// gradient LEDs exist for would be stepping under the gradient. See [`classify_load_scaled`].
pub const PERMILLE_FULL: u32 = 1000;

// ---------------------------------------------------------------------------------------------
// PULSE-2 — the instrument panel's geometry.
// ---------------------------------------------------------------------------------------------

/// The pulse band's height for a panel `ph` pixels tall: a **thirteenth of the panel**, floored at
/// eight glyph cells so a small surface still gets a band it can seat one row per core in, and capped
/// at a quarter of the panel so a degenerate surface cannot be all instrument.
///
/// THE METRICS RULE, applied to an instrument rather than to type. A cell-derived height is what put
/// PULSE-STRIP's bars at 4 px on the bench panel: `Metrics::for_height(1200)` is `scale=1` (the scale
/// step is 900 rows and 1200 does not reach 2x), so *everything* derived from `cell_h` is 8 px there
/// however the panel grows. An instrument that must be read across a room is a function of the PANEL,
/// not of the type size — so this is a panel fraction, and it yields 92 px at 1200 rows (Peter's
/// "~20mm" on the bench panel) and 36 px on the 640x480 QEMU surface.
pub fn band_h(ph: usize) -> usize {
    let m = crate::ui::Metrics::for_height(ph);
    // The `cell_h * 8` floor is what makes the 640x480 gate surface work: a thirteenth of 480 is 36 px,
    // which splits four ways into 9 px rows that cannot hold a glyph. Eight cells (64 px there) gives
    // every core a row taller than the type beside it. On the bench panel the fraction is the larger
    // term (92 px vs 64), so the floor never binds where it would cost real estate.
    (ph / 13).max(m.cell_h * 8).min(ph / 4)
}

/// **The reservation.** Total panel rows the bottom chrome owns: the pulse band plus the status band.
/// `wm::place` subtracts this from its vertical budget, so no tiled window is ever laid out over the
/// instrument. Public because the tiler is the only caller that matters and the number must be
/// computed in exactly one place — a second copy of this arithmetic is how a reserved region and the
/// thing reserving it drift apart.
pub fn chrome_h(ph: usize) -> usize {
    let m = crate::ui::Metrics::for_height(ph);
    band_h(ph).saturating_add(m.line_h)
}

/// The horizontal span `[x0, x1)` of the pulse band that is free of WC-F's reserved probe boxes.
///
/// WC-F paints scan-out ground truth straight into the framebuffer at the tail of every composite, and
/// its two marks hug the BOTTOM edge — the ramp against the left corner (264x256 px), the twins against
/// the right (144x64). Those rows are exactly the rows this band wants. Neither can move: WC-F's marks
/// are photographed by the bench operator and its geometry is a witnessed constant. So the instrument
/// yields the corners and takes the middle, which on the 1920x1200 bench panel is still 1480 px —
/// far more width than four cores need — and 200 px on the 640x480 gate surface.
///
/// Cut on which half of the panel a box sits in: a left-hand box pushes `x0` past its right edge, a
/// right-hand box pulls `x1` back to its left edge. Boxes that do not overlap the band's rows are
/// ignored. Outside the WC-F build (x86, non-baremetal, no `witness`) nothing paints here and the band
/// is the full panel width.
#[allow(unused_variables)]
fn free_span(pw: usize, ph: usize, y0: usize, y1: usize) -> (usize, usize) {
    // `mut` is live only in the WC-F build below; on x86 / non-baremetal the span is returned whole.
    #[allow(unused_mut)]
    let (mut x0, mut x1) = (0usize, pw);
    #[cfg(all(target_arch = "aarch64", feature = "witness", feature = "baremetal"))]
    if let Some(boxes) = crate::video::wcf::reserved(pw, ph) {
        for (bx, by, bw, bh) in boxes {
            if by >= y1 || by.saturating_add(bh) <= y0 {
                continue; // clear of the band's rows
            }
            if bx.saturating_add(bw / 2) < pw / 2 {
                x0 = x0.max(bx.saturating_add(bw));
            } else {
                x1 = x1.min(bx);
            }
        }
    }
    if x1 <= x0 {
        (0, 0)
    } else {
        (x0, x1)
    }
}

/// The pulse instrument's box on a `pw` x `ph` panel: `(x, y, w, h)`, seated directly above the status
/// band and horizontally clear of WC-F. `w == 0` means the panel cannot seat the instrument at all.
pub fn panel_geometry(pw: usize, ph: usize) -> (usize, usize, usize, usize) {
    let m = crate::ui::Metrics::for_height(ph);
    let bh = band_h(ph);
    let y = ph.saturating_sub(m.line_h).saturating_sub(bh);
    let (x0, x1) = free_span(pw, ph, y, y + bh);
    (x0, y, x1.saturating_sub(x0), bh)
}

/// One core's row inside the instrument, as `RowGeom`. `None` when the band cannot seat a legible row.
///
/// **Stacked full-width rows, not side-by-side quarters.** Both fit the ~92 px band, and the choice is
/// decided by what Peter asked the width to buy: *"if pulse spans the entire bottom width of the screen
/// there will be more leds to show sensitivity."* Quarters would give each core ~370 px and ~37 LEDs;
/// stacked rows give each core the WHOLE 1400 px and ~140 — four times the resolution, which is the
/// entire point. The cost is row height (20 px instead of 92), and 20 px is still five times the bar
/// PULSE-STRIP drew and comfortably taller than the 8 px label beside it. Sensitivity wins.
///
/// Layout of a row, left to right: `c<N>` label, the LED bar taking every pixel between, then the
/// percent right-aligned to one decimal. The bar is the instrument; the text is the annotation.
#[derive(Clone, Copy)]
struct RowGeom {
    /// Lit width of one LED (the pitch less the dark gutter).
    led_w: usize,
    /// LED height — the bar's track height.
    led_h: usize,
    /// Dark gutter between LEDs.
    gap: usize,
    /// Number of LEDs across the bar. The sensitivity number.
    nled: usize,
    bar_x: usize,
    bar_w: usize,
    row_h: usize,
}

/// Target LED pitch as a fraction of the row height. A pitch of about half the row height gives a
/// clearly-articulated LED (a wide-ish block, not a hairline) while still putting ~140 of them across
/// the bench panel's bar. Derived from the row, so a small panel gets fewer, fatter LEDs rather than an
/// unreadable comb.
fn led_pitch_for(row_h: usize) -> usize {
    (row_h / 2).max(3)
}

fn row_geometry(
    m: &crate::ui::Metrics,
    px: usize,
    pw: usize,
    ph_band: usize,
    ncpu: usize,
) -> Option<RowGeom> {
    if ncpu == 0 {
        return None;
    }
    let pad = (m.line_h / 2).max(2);
    let row_h = ph_band.saturating_sub(2 * pad) / ncpu;
    // A row must be tall enough to hold a glyph cell AND a bar worth calling a bar.
    if row_h < m.cell_h.max(6) {
        return None;
    }
    let led_h = row_h.saturating_sub((row_h / 4).max(1)).max(4);
    // `c0` + a space; ` 62.4%` right-aligned. Both in cells, so they track the type.
    let label_w = m.cell_w * 3;
    let val_w = m.cell_w * 7;
    let bar_x = px + pad + label_w;
    let avail = pw
        .saturating_sub(2 * pad)
        .saturating_sub(label_w)
        .saturating_sub(val_w);
    let pitch = led_pitch_for(row_h);
    let nled = avail / pitch;
    if nled < 8 {
        return None; // fewer LEDs than the old ten-segment bar had: not an instrument
    }
    let gap = (pitch / 4).max(1);
    Some(RowGeom {
        led_w: pitch.saturating_sub(gap),
        led_h,
        gap,
        nled,
        bar_x,
        bar_w: nled * pitch,
        row_h,
    })
}

// ---------------------------------------------------------------------------------------------
// PULSE-2 — the LED bar. Gradient-filled segments over a continuous lit length.
// ---------------------------------------------------------------------------------------------

/// Scale a packed `0x00RRGGBB` colour by `num/den`, per channel, saturating-free (the product of a
/// byte and a small numerator cannot overflow `u32`).
fn shade(c: u32, num: u32, den: u32) -> u32 {
    let den = den.max(1);
    let ch = |sh: u32| (((c >> sh) & 0xFF) * num / den).min(255) << sh;
    ch(16) | ch(8) | ch(0)
}

/// Linear blend `a`→`b` by `num/den`, per channel.
fn mix(a: u32, b: u32, num: u32, den: u32) -> u32 {
    let den = den.max(1);
    let num = num.min(den);
    let ch = |sh: u32| {
        let (x, y) = ((a >> sh) & 0xFF, (b >> sh) & 0xFF);
        ((x * (den - num) + y * num) / den) << sh
    };
    ch(16) | ch(8) | ch(0)
}

// The instrument scale: green through amber to red across the bar. A VU-meter ramp, not chrome — the
// level reads before any digit does, which is the whole job of a bench instrument.
const LED_GREEN: u32 = 0x00_2ECC71;
const LED_AMBER: u32 = 0x00_F1C40F;
const LED_RED: u32 = 0x00_E74C3C;

/// The base colour of LED `s` of `n`: green below 60% of full scale, ramping through amber to red at
/// the top. Position on the SCALE, not on the load — so a given LED is always the same colour and the
/// meter's shape is stable while its length moves.
fn led_hue(s: usize, n: usize) -> u32 {
    let n = n.max(1);
    let pos = s * 1000 / n;
    if pos < 600 {
        LED_GREEN
    } else if pos < 850 {
        mix(LED_GREEN, LED_AMBER, (pos - 600) as u32, 250)
    } else {
        mix(LED_AMBER, LED_RED, (pos - 850) as u32, 150)
    }
}

/// Vertical gradient bands per LED. The lens look: bright through the middle, falling off top and
/// bottom. 8 bands is smooth at 15 px and costs 8 rects per LED rather than one per row.
const LED_BANDS: usize = 8;

/// Draw one LED at `(x, y)` with intensity `num/den` of its base colour — `0` draws the dark track, a
/// partial value draws the fractional tail of the fill. Each LED carries its own vertical gradient, so
/// the block reads as a lit lamp rather than as a flat rectangle.
fn draw_led<P: GneissPal>(pal: &mut P, x: usize, y: usize, w: usize, h: usize, c: u32, lit: u32) {
    let bands = LED_BANDS.min(h.max(1));
    let base = mix(METER_DIM, c, lit, 255);
    for b in 0..bands {
        let y0 = y + b * h / bands;
        let y1 = y + (b + 1) * h / bands;
        // Triangular profile across the band index: 0 at the edges, 1 in the middle. Kept in
        // 0..=255 fixed point, floored at 60% so an LED's rim is dimmer but never black.
        let t = if bands <= 1 {
            255
        } else {
            let d = (2 * b as i32 - (bands as i32 - 1)).unsigned_abs();
            255 - (255 * d / (bands as u32 - 1)).min(255)
        };
        let f = 154 + 101 * t / 255;
        pal.draw_rect(x, y0, w, y1.saturating_sub(y0), shade(base, f, 255));
    }
}

/// Draw one core's LED bar for a per-mille load (or [`PARKED`]).
///
/// **Smoothness is the lit LENGTH, not the lit COUNT.** `lit_px` is the load's fraction of the whole
/// bar in pixels; every LED fully inside it burns at full intensity, every LED fully outside draws the
/// dark track, and the ONE LED the boundary falls inside is lit in proportion to how much of it the
/// length covers. So a load creeping upward brightens the next lamp continuously and then hands over
/// to the one after it — a meter that scales smoothly instead of clicking between whole segments.
///
/// PARKED and idle keep their PULSE-STRIP/VUG-HONESTY meanings verbatim: a parked core draws a cool
/// dashed track (never confusable with 0%), and an idle-but-scheduled core breathes a sweeping block
/// (PULSE-ALIVE — Peter's "pulse shows 1 CPU" defect). Both scale to the new LED count.
fn draw_led_bar<P: GneissPal>(pal: &mut P, g: &RowGeom, y: usize, permille: u32) -> usize {
    let pitch = g.led_w + g.gap;
    if permille == PARKED {
        // Dashes in groups, not alternating LEDs: at ~140 LEDs a 1-on-1-off dash is a grey haze.
        let group = (g.nled / 16).max(1);
        for s in 0..g.nled {
            if (s / group) % 2 == 0 {
                draw_led(pal, g.bar_x + s * pitch, y, g.led_w, g.led_h, METER_PARKED, 255);
            } else {
                pal.draw_rect(g.bar_x + s * pitch, y, g.led_w, g.led_h, PANEL_BG);
            }
        }
        return g.bar_x + g.bar_w;
    }
    if permille == 0 {
        let block = (g.nled / 10).max(1);
        let phase = ((crate::arch::ms() / 300) as usize) % g.nled;
        for s in 0..g.nled {
            let on = (s + g.nled - phase) % g.nled < block;
            let c = if on { METER_BREATH } else { METER_DIM };
            draw_led(pal, g.bar_x + s * pitch, y, g.led_w, g.led_h, c, 255);
        }
        return g.bar_x + g.bar_w;
    }
    let lit_px = lit_px(g, permille);
    for s in 0..g.nled {
        let x = s * pitch;
        // Coverage of THIS lamp's lit area by the fill length, in 0..=255.
        let covered = lit_px.saturating_sub(x).min(g.led_w);
        let lit = (covered * 255 / g.led_w.max(1)) as u32;
        draw_led(
            pal,
            g.bar_x + x,
            y,
            g.led_w,
            g.led_h,
            led_hue(s, g.nled),
            lit,
        );
    }
    g.bar_x + g.bar_w
}

/// The fill length in pixels for a per-mille load — the single source of truth for both the draw and
/// the dirty test, so "the picture changed" and "we redrew" can never disagree.
fn lit_px(g: &RowGeom, permille: u32) -> usize {
    if permille == PARKED {
        return usize::MAX;
    }
    g.bar_w * (permille as usize).min(PERMILLE_FULL as usize) / PERMILLE_FULL as usize
}

/// Draw the instrument panel from the last sampled loads: clear the band, then one labelled LED row
/// per core. Nothing here samples — [`tick`] owns the telemetry cadence.
fn draw_panel<P: GneissPal>(pal: &mut P) {
    let st = PULSE.lock();
    if !st.armed || st.ncpu == 0 {
        return;
    }
    let m = pal.metrics();
    let (px, py, pw, ph_band) = panel_geometry(pal.width() as usize, pal.height() as usize);
    if pw == 0 || ph_band == 0 {
        return;
    }
    pal.draw_rect(px, py, pw, ph_band, PANEL_BG);
    let g = match row_geometry(&m, px, pw, ph_band, st.ncpu) {
        Some(g) => g,
        // Too small to seat rows: the band is painted (the reserved region is still visibly the
        // instrument's) and nothing false is said about the cores.
        None => return,
    };
    let pad = (m.line_h / 2).max(2);
    for c in 0..st.ncpu {
        let ry = py + pad + c * g.row_h;
        // Centre the glyph cell against the bar's track.
        let ty = ry + (g.led_h.saturating_sub(m.cell_h)) / 2;
        pal.draw_text(px + pad, ty, &format!("c{}", c), PANEL_FG);
        let end = draw_led_bar(pal, &g, ry, st.load[c]);
        let val = if st.load[c] == PARKED {
            String::from("  park")
        } else {
            format!("{:>3}.{}%", st.load[c] / 10, st.load[c] % 10)
        };
        pal.draw_text(end + g.gap, ty, &val, PANEL_FG);
    }
}

/// PULSE-STRIP — the PACED entry point, called on the strip's own ~1 Hz refresh pulse (an
/// `Event::Timer`). Samples the per-core meters at most once per [`PSTRIP_PERIOD_MS`], then redraws
/// the band **only if** the composed text line or a quantized per-core load actually changed.
/// Returns whether it drew — the caller presents only on `true`, so an idle panel whose clock has not
/// been seeded costs a sample and nothing else.
pub fn tick<P: GneissPal>(pal: &mut P) -> bool {
    let now = crate::arch::ms();
    let mut changed = false;
    {
        let mut st = PULSE.lock();
        if st.rollup_ms == 0 {
            st.rollup_ms = now;
        }
        if !st.armed {
            st.ncpu = PSTRIP_MAX_CPUS.min(crate::arch::sched::meter_cpu_count());
            for c in 0..st.ncpu {
                st.prev[c] = crate::arch::sched::meter_cpu_ticks(c);
            }
            st.armed = true;
            st.last_ms = now;
            let m = pal.metrics();
            let (pw, ph) = (pal.width() as usize, pal.height() as usize);
            let (px, py, bw, bh) = panel_geometry(pw, ph);
            let g = row_geometry(&m, px, bw, bh, st.ncpu).unwrap_or(RowGeom {
                led_w: 0,
                led_h: 0,
                gap: 0,
                nled: 0,
                bar_x: 0,
                bar_w: 0,
                row_h: 0,
            });
            // The LOOK of a panel nobody can see headless: the reserved band's box, the per-core row
            // pitch, and the LED metrics. All derived from the panel height and `ui::Metrics`, so a
            // hard-coded pixel would show up here as a constant that does not track `UNAOS_FB*`.
            // `leds=` is the SENSITIVITY number — LEDs per core bar, i.e. how fine a load change the
            // meter can articulate before the gradient takes over inside a single lamp.
            serial_println!(
                "[pstrip] armed cores={} panel=({},{},{}x{}) row_h={} bar=(x={},w={}) leds={} led={}x{} gap={} bands={} full={} strip_h={} reserved={} period={}ms",
                st.ncpu,
                px,
                py,
                bw,
                bh,
                g.row_h,
                g.bar_x,
                g.bar_w,
                g.nled,
                g.led_w,
                g.led_h,
                g.gap,
                LED_BANDS,
                PERMILLE_FULL,
                m.line_h,
                chrome_h(ph),
                PSTRIP_PERIOD_MS
            );
            changed = true; // first frame must paint the bars
        } else if now.wrapping_sub(st.last_ms) >= PSTRIP_PERIOD_MS {
            st.last_ms = now;
            st.samples += 1;
            let demo = crate::arch::sched::meter_current_cpu();
            // The dirty test is the DRAWN LENGTH, in pixels, of this panel's actual bar — not the
            // number behind it. PULSE-STRIP quantized to whole bar segments because whole segments
            // were all it could draw; the gradient LEDs render a continuous fill, so the finest
            // visible difference is one pixel of lit length and that is exactly the threshold:
            // redraw when any core's `lit_px` moves by >= 1. Finer than that is invisible, coarser
            // than that would step under a gradient built to be smooth. Idle cost is unchanged and
            // still ~zero: an idle core's per-mille load is a hard 0 and its length does not move, so
            // an idle panel redraws only when the STATUS TEXT changes (once SNTP seeds the clock),
            // and not at all before that.
            let m = pal.metrics();
            let (px, py, bw, bh) = panel_geometry(pal.width() as usize, pal.height() as usize);
            let _ = py;
            let geo = row_geometry(&m, px, bw, bh, st.ncpu);
            for c in 0..st.ncpu {
                let (b, i) = crate::arch::sched::meter_cpu_ticks(c);
                let db = b.wrapping_sub(st.prev[c].0);
                let di = i.wrapping_sub(st.prev[c].1);
                st.prev[c] = (b, i);
                // `own_load` is 0: the strip is drawn by a SCHEDULED task, so its core's counters
                // tick like every other core's and the demo-core fallback never fires here. Passing a
                // fabricated render load would be exactly the dishonesty VUG-HONESTY closed.
                let new = classify_load_scaled(db, di, c == demo, 0, PERMILLE_FULL);
                changed |= match &geo {
                    Some(g) => lit_px(g, new) != lit_px(g, st.load[c]),
                    // No seatable geometry: fall back to the number itself, so a panel too small to
                    // draw still never claims "unchanged" about a load that moved.
                    None => new != st.load[c],
                };
                // PULSE-ALIVE's breath sweep is deliberately NOT a dirty source, exactly as it was
                // not one in PULSE-STRIP. It is wall-clock animation on a core reading a hard 0, so
                // making it dirty would redraw the whole panel every single sample and turn the
                // pacing proof (`skipped=` in the rollup) into a constant zero — a 1 Hz repaint
                // wearing a flag, which is the thing the spec's FORBID exists to catch. The breath
                // advances on the frames the panel draws for other reasons, which once the clock is
                // seeded is every second anyway.
                st.load[c] = new;
            }
        }
        // The text line changes on its own clock (lease settling, SNTP seeding, the seconds field).
        let h = hash(&compose());
        if h != st.text_hash {
            st.text_hash = h;
            changed = true;
        }
        if changed {
            st.redraws += 1;
        }
        // Rate-limited rollup: samples taken vs frames actually drawn. `redraws` well below `samples`
        // is the dirty-pacing proof; the per-second rate is what the spec's busy-loop FORBID reads.
        let span = now.wrapping_sub(st.rollup_ms);
        if span >= PSTRIP_ROLLUP_MS {
            // Fixed-point tenths: at a 10 s rollup an integer /s would truncate every honest rate
            // to 0 and the spec's busy-loop FORBID would have nothing to bite on.
            let rate_x10 = st.redraws.saturating_mul(10_000) / span.max(1);
            serial_println!(
                "[pstrip] rollup samples={} redraws={} skipped={} rate={}.{}/s period={}ms",
                st.samples,
                st.redraws,
                st.samples.saturating_sub(st.redraws),
                rate_x10 / 10,
                rate_x10 % 10,
                PSTRIP_PERIOD_MS
            );
            st.samples = 0;
            st.redraws = 0;
            st.rollup_ms = now;
        }
    }
    if changed {
        draw(pal);
    }
    changed
}
