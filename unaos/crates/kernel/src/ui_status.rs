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
//!   a 14 px jump on a 1400 px bar) — see `vug::classify_load_scaled`, which states the VUG-HONESTY
//!   rule once for both scales.
//! * The scale runs green → amber → red across the bar (colour by POSITION, so a given lamp's colour
//!   is stable and only the length moves). Test instrument, not chrome.
//!
//! The status band itself goes back to what it was before PULSE-STRIP: **text only**, host / ip /
//! clock. The miniature bars are superseded. What PULSE-STRIP got right is kept verbatim and is the
//! substrate here: the same `sched::meter_cpu_*` relaxed reads, the same [`crate::vug`] widget and
//! palette, the same dirty pacing, the same zero-new-threads plumbing.
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

//! ## PULSE-3 — the strip must read the LIVE load source, not the dispatch-pass meter
//!
//! Peter's attended P64 verdict on the finished gradient instrument: *"gradient good but pulse not
//! real-time"*. The capture (`pi4-r23s1o`) says the same thing in numbers — while three vugs held the
//! cores at a sustained 99% and the vugband workers churned ~1M context switches a window, the strip
//! printed `rollup samples=10 redraws=0 skipped=10`. Ten consecutive seconds in which the meter decided
//! nothing on the panel had moved, against a machine that was visibly moving.
//!
//! The dirty test was not the defect. **The load source was.** PULSE-STRIP inherited `vug`'s VUG-1 M3b
//! feed — `sched::meter_cpu_ticks`, the cumulative `CPU_BUSY`/`CPU_IDLE` counters that `dispatch_next`
//! bumps once per dispatch PASS — and PULSE-2 carried it forward verbatim. Those are pass COUNTS, and
//! the scheduler itself retired that metric two arcs earlier: SCHED-5's standing note, in
//! `arch::aarch64::sched`, is *"TIME, NOT PASSES … it counts scheduler activity, not CPU time"*, and it
//! moved `busy_pct_recent` onto a CNTPCT time base for exactly the reason that bit here. A core running
//! CPU-bound tasks back to back dispatches at a near-constant rate and never reaches the empty-queue
//! branch, so `db/(db+di)` pins at full scale and STAYS there: the ratio is flat while the utilization
//! underneath it wanders. That is why the LEDs did not track, and why the `[spinhunt]` fixture's
//! `load settled c2=53` and SCHED's `c2=99%` in the same window were never reconcilable — they are two
//! different sources, and the strip was reading the wrong one.
//!
//! So [`live_permille`] takes the live SCHED-5/SCHED-7 feed (`sched::core_load(c).busy_pct_recent`, a
//! busy-TIME fraction over a rolling ~250 ms window) as the strip's primary source. It is the same
//! number `top` and the `SCHED: load` witness print, so the instrument and the console can no longer
//! disagree about the same core. `meter_cpu_ticks` stays as the FALLBACK, and it keeps its whole
//! VUG-HONESTY meaning: `core_load` reports `tracked=false` for a core that is not inside `run()`, and
//! for such a core the tick deltas are consulted exactly as before — a frozen non-demo core still reads
//! [`PARKED`] rather than a fabricated bar, and `vug::parked_display_witness` still covers that branch.
//! On x86 (no `core_load`) the source is unchanged in every particular.
//!
//! **Pacing is unchanged and stays.** 1 Hz is not the defect — skipping a window whose values moved is.
//! An idle desktop still reads a hard 0 on every core and still redraws nothing (default-quiet), and the
//! rollup now carries `srcdelta=`: the count of windows in which the SOURCE moved, printed beside the
//! count of windows actually drawn. A stale-source regression is then a plain reading — `srcdelta=0`
//! across a busy window — instead of something only a bench operator with the panel in front of him can
//! see. [`src_witness`] closes the other half at arm time, headless: it reports how many cores return a
//! live number and whether the source's own quantum (1% → 10‰) clears one pixel of lit length on THIS
//! panel, so "the meter cannot show what the source can say" is caught in the gate.

use crate::pal::GneissPal;
use crate::vug::{METER_BREATH, METER_DIM, METER_PARKED, PARKED};
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
/// simply shows its first `PSTRIP_MAX_CPUS` cores. (Kept below vug's `MAX_METER_CPUS` for that reason,
/// not for want of counters.)
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
    /// PULSE-3 — windows in which the SOURCE load moved on at least one core, whether or not that
    /// movement cleared a pixel of lit length. Printed beside `redraws` so a window that was skipped
    /// while the machine was changing is a reading in the log rather than a bench observation.
    srcdelta: u64,
    rollup_ms: u64,
    /// PULSE-3 — has the source witness been re-emitted at a rollup boundary yet? It fires once at
    /// arm time, which is the earliest moment the panel exists and therefore the moment MOST likely to
    /// catch a core that has not yet entered `run()` and to read a transient `live=0`. Re-emitting it
    /// once at the first rollup — ten seconds of settled boot later — means an early fire can never
    /// leave a permanent unexplained `live=0` as the log's only word on the subject.
    src_reemitted: bool,
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
    srcdelta: 0,
    rollup_ms: 0,
    src_reemitted: false,
});

/// PULSE-3 — the strip's PRIMARY load source: this core's live busy-TIME fraction, in per-mille.
///
/// `sched::core_load(c).busy_pct_recent` is SCHED-5/SCHED-7's rolling ~250 ms CNTPCT accounting — the
/// number `top` and the `SCHED: load` heartbeat report — so the instrument and the console are reading
/// one feed. `None` means "no live number for this core": either the arch has no such accounting (x86)
/// or SCHED-8's `tracked` flag says the core is not currently inside `run()` and its slot is a frozen
/// snapshot. `None` sends the caller to the `meter_cpu_ticks` fallback, which is where VUG-HONESTY's
/// PARKED decision lives — so this function never has to invent a load, and never gets the chance to.
///
/// Resolution note: the source is a percent, so its quantum is 10‰. That is coarser than the meter's
/// 1‰ storage and deliberately not smoothed or interpolated here — a fabricated intermediate value is
/// exactly the dishonesty VUG-HONESTY closed. [`src_witness`] reports what one quantum is worth in
/// pixels on the live panel so the trade is checkable rather than assumed.
#[allow(unused_variables)]
fn live_permille(cpu: usize) -> Option<u32> {
    #[cfg(target_arch = "aarch64")]
    {
        let ld = crate::arch::sched::core_load(cpu);
        if ld.tracked {
            return Some((ld.busy_pct_recent * 10).min(PERMILLE_FULL));
        }
        None
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        None
    }
}

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
/// gradient LEDs exist for would be stepping under the gradient. See `vug::classify_load_scaled`.
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

/// The source quantum, in per-mille: [`live_permille`] is derived from a PERCENT, so the smallest step
/// the live feed can express is 10‰. The meter stores per-mille; this is what the source can actually
/// deliver into it.
const SRC_QUANTUM_PERMILLE: u32 = 10;

/// The bar width, in pixels, below which ONE source quantum cannot resolve to a whole pixel. The
/// quantum is 1/100 of full scale, so a bar narrower than 100 px cannot render a 1% step however honest
/// the feed is. See [`src_witness`] for why this bound has to exist *inside* the witness rather than in
/// the spec: it separates a geometry fact from a source fact, and only the spec's `armed`-line FORBIDs
/// can speak about geometry.
const SRC_RESOLVABLE_BAR_PX: usize = 100;

/// PULSE-3 — the arm-time source witness. Headless, deterministic, one line, no framebuffer read.
///
/// Two questions a replay could not previously answer about a panel nobody can see, and the two halves
/// of the P64 defect:
///
/// * **Is the strip on the live feed?** `live=k/n` counts the cores for which [`live_permille`] returns
///   a number this boot. `k=0` is the PULSE-3 regression exactly — every core back on the dispatch-pass
///   fallback, which is where "not real-time" came from. (A core legitimately outside `run()` reports
///   `tracked=false` and is not counted; that is why the assertion is `k>0`, not `k==n`.)
/// * **Can the meter show what the source can say?** `stepres=` is the lit-length movement, in pixels,
///   of ONE source quantum on THIS panel's bar geometry. Zero means the display quantizes the feed away
///   — a real-time source feeding a meter too coarse to render its steps, which would present as the
///   same frozen bars for a different reason. `mono=` checks the fill is strictly increasing across the
///   scale, so a geometry that has collapsed to a constant is caught rather than read as "steady load".
///
/// **A red here must name the right subsystem.** `stepres` is `bar_w / 100`, so on any panel whose bar
/// is narrower than [`SRC_RESOLVABLE_BAR_PX`] it is zero *for a geometry reason* — a shrunken
/// `UNAOS_FBW`, a WC-F reservation that grew, a layout regression. Letting the spec FORBID `stepres=0px`
/// unconditionally would report every one of those as a SOURCE regression, which is the opposite of
/// what this arc is about. So the witness refuses to state a pixel resolution it cannot honestly
/// attribute: below the bound it prints `stepres=coarse` and verdicts `SKIP-GEOM`, and the geometry
/// FORBIDs on the `armed` line (`panel=…0x…`, `row_h=0`, single-digit `leds=`) are what go red instead.
/// `stepres=0px` is then only ever printed by a panel wide enough to have resolved the step — where it
/// genuinely does mean the display is quantizing a live feed away.
///
/// **x86 has no `core_load`** and is deliberately unchanged by PULSE-3, so there is no live feed to be
/// on or off there and `live=0/n FAIL` would be a standing untruth in every x86 log. The witness reports
/// `live=n/a` and verdicts `SKIP-ARCH` instead; the x86 log carries no FAIL and the "unchanged in every
/// particular" claim stays true.
fn src_witness(g: &RowGeom, ncpu: usize) {
    let mono = lit_px(g, 250) < lit_px(g, 500) && lit_px(g, 500) < lit_px(g, PERMILLE_FULL);
    let resolvable = g.bar_w >= SRC_RESOLVABLE_BAR_PX;
    let stepres = lit_px(g, SRC_QUANTUM_PERMILLE).saturating_sub(lit_px(g, 0));
    let step = if resolvable {
        format!("{}px", stepres)
    } else {
        String::from("coarse")
    };
    #[cfg(target_arch = "aarch64")]
    {
        let live = (0..ncpu).filter(|c| live_permille(*c).is_some()).count();
        let verdict = if live == 0 || !mono {
            "FAIL"
        } else if !resolvable {
            "SKIP-GEOM"
        } else if stepres == 0 {
            "FAIL"
        } else {
            "PASS"
        };
        serial_println!(
            "[pstrip] src live={}/{} quantum={} stepres={} mono={} {}",
            live,
            ncpu,
            SRC_QUANTUM_PERMILLE,
            step,
            if mono { "yes" } else { "no" },
            verdict
        );
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        let _ = ncpu;
        serial_println!(
            "[pstrip] src live=n/a quantum={} stepres={} mono={} SKIP-ARCH",
            SRC_QUANTUM_PERMILLE,
            step,
            if mono { "yes" } else { "no" }
        );
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
    // PULSE-3: did the SOURCE move this window, independently of whether the picture did?
    let mut srcmoved = false;
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
            src_witness(&g, st.ncpu);
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
                // PULSE-3 — the LIVE busy-time fraction first; the dispatch-pass deltas only where
                // there is no live number. The deltas are still consumed unconditionally above (the
                // `prev` snapshot has to advance every window either way, or a core that later falls
                // back would classify against a window minutes wide).
                let new = live_permille(c).unwrap_or_else(|| {
                    crate::vug::classify_load_scaled(db, di, c == demo, 0, PERMILLE_FULL)
                });
                if new != st.load[c] {
                    srcmoved = true;
                }
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
        if srcmoved {
            st.srcdelta += 1;
        }
        // Rate-limited rollup: samples taken vs frames actually drawn. `redraws` well below `samples`
        // is the dirty-pacing proof; the per-second rate is what the spec's busy-loop FORBID reads.
        let span = now.wrapping_sub(st.rollup_ms);
        if span >= PSTRIP_ROLLUP_MS {
            // Fixed-point tenths: at a 10 s rollup an integer /s would truncate every honest rate
            // to 0 and the spec's busy-loop FORBID would have nothing to bite on.
            let rate_x10 = st.redraws.saturating_mul(10_000) / span.max(1);
            serial_println!(
                "[pstrip] rollup samples={} redraws={} skipped={} srcdelta={} rate={}.{}/s period={}ms",
                st.samples,
                st.redraws,
                st.samples.saturating_sub(st.redraws),
                st.srcdelta,
                rate_x10 / 10,
                rate_x10 % 10,
                PSTRIP_PERIOD_MS
            );
            // PULSE-3 — re-state the source witness ONCE, at the first rollup. The arm-time fire is
            // taken the instant the panel exists, before every core has necessarily entered `run()`,
            // so a transient `live=0` there is possible and would otherwise stand as the log's only
            // word on the feed. Ten seconds of settled boot later the answer is not transient. Once
            // only: this is insurance against an early fire, not a second periodic witness.
            if !st.src_reemitted {
                st.src_reemitted = true;
                let m = pal.metrics();
                let (px, _py, bw, bh) = panel_geometry(pal.width() as usize, pal.height() as usize);
                if let Some(g) = row_geometry(&m, px, bw, bh, st.ncpu) {
                    src_witness(&g, st.ncpu);
                }
            }
            st.samples = 0;
            st.redraws = 0;
            st.srcdelta = 0;
            st.rollup_ms = now;
        }
    }
    if changed {
        draw(pal);
    }
    changed
}
