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

//! ## PULSE-STRIP — the always-running per-core CPU pulse
//!
//! Peter's ask ("an always running pulse in a strip window/app at the bottom") is answered by
//! EXTENDING this strip rather than by adding a second bottom band or a windowed app. The reasons
//! are structural, not stylistic: (a) a bottom band already exists, is already always-on, is already
//! the panel's bottom chrome, and a second one would either fight it for the same rows or push the
//! console up a line — the strip IS the dock; (b) a window would be focusable, z-ordered and killable
//! (the three things the ask says it must never be), and would cost a task in a thread table KILLBOUND
//! just bounded to 8; (c) this strip already runs on the render task's own 1 Hz refresh, so the pulse
//! costs ZERO new threads, ZERO new presents and one extra sample per second.
//!
//! Coexistence falls out of the same choice. The console's bottom-row layout, the click hit-test
//! (`main.rs::click1_hit_test`) and the compositor's view of the band are unchanged, because the band
//! is unchanged — only its right-hand pixels now carry bars instead of background. WC-I occlusion is
//! inherited for free: the strip draws into the `Screen` back buffer and `Screen::present_background`
//! subtracts `wm::occluders()` from every damaged row, so the strip has never written under a window
//! and still does not. Focus is untouched: nothing here is a view, so nothing here can take TAB.
//!
//! Dirty-pacing is the load itself. [`tick`] samples at most once per [`PSTRIP_PERIOD_MS`] and draws
//! only when the composed line OR a per-core load *quantized to a bar segment* changed, so an idle
//! panel with an unsynced clock presents nothing at all. (Once SNTP seeds the clock the seconds field
//! changes every second and the strip presents at 1 Hz — that is the strip's pre-existing cadence, not
//! a cost this arc added.) The `[pstrip]` rollup reports both counts so the pacing is checkable.

use crate::pal::GneissPal;
use crate::vug::{draw_pulse_bar, METER_PURPLE, PARKED, PULSE_SEGS};
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
/// PULSE-STRIP: unconditional. This is the path a Key/Button pass takes, where the console has just
/// repainted over the band and the strip owes an unconditional redraw on top whether or not anything
/// it displays changed. The pulse bars are drawn from the LAST SAMPLED loads — no resample, so a
/// keystroke storm cannot turn into a telemetry-read storm. [`tick`] is the paced entry point.
pub fn draw<P: GneissPal>(pal: &mut P) {
    let m = pal.metrics();
    let w = pal.width() as usize;
    let h = pal.height() as usize;
    // Pin to the last line-pitch band. `saturating_sub` keeps a degenerate/tiny panel in-bounds.
    let band_y = h.saturating_sub(m.line_h);

    pal.draw_rect(0, band_y, w, m.line_h, STRIP_BG);

    // Vertically centre the glyph cell within the line-pitch band (the band is `line_h`, the glyph
    // is `cell_h`; the half-cell leading splits above/below).
    let text_y = band_y + (m.line_h.saturating_sub(m.cell_h)) / 2;
    pal.draw_text(m.margin, text_y, &compose(), STRIP_FG);
    draw_pulse(pal, band_y);
}

// ---------------------------------------------------------------------------------------------
// PULSE-STRIP — per-core CPU bars docked in the strip's right-hand end.
// ---------------------------------------------------------------------------------------------

/// Upper bound on the bars the strip will draw. The strip is one line tall and shares it with the
/// host/ip/clock line, so it caps well below vug's `MAX_METER_CPUS`: past this the bars stop being
/// readable at strip height anyway, and a wider machine simply shows its first `PSTRIP_MAX_CPUS`.
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

/// Quantize a displayed load to the bar segment it lights. Two loads that light the same segments
/// draw the same pixels, so treating them as equal is what makes "unchanged" mean "identical frame"
/// rather than "identical number" — the difference between a paced strip and a 1 Hz repaint.
/// [`PARKED`] maps to its own bucket so a park↔idle transition is never quantized away.
fn quantize(load: u32) -> u32 {
    if load == PARKED {
        return u32::MAX;
    }
    ((load as usize * PULSE_SEGS + 50) / 100).min(PULSE_SEGS) as u32
}

/// Bar geometry for a panel: `(seg_w, seg_h, gap, bar_w, total_w)`. Derived entirely from
/// [`crate::ui::Metrics`] (THE METRICS RULE) — a third-cell-wide segment, a half-cell-tall track, so
/// the bars read as instrument marks inside the chrome band rather than as a second text row.
fn bar_geometry(m: &crate::ui::Metrics, ncpu: usize) -> (usize, usize, usize, usize, usize) {
    let seg_w = (m.cell_w / 3).max(2);
    let seg_h = (m.cell_h / 2).max(3);
    let gap = m.scale.max(1);
    let bar_w = PULSE_SEGS * (seg_w + gap);
    // One cell of air between adjacent cores' bars.
    let total_w = ncpu * bar_w + ncpu.saturating_sub(1) * m.cell_w;
    (seg_w, seg_h, gap, bar_w, total_w)
}

/// Draw the per-core bars into the strip band from the last sampled loads. Right-aligned, so the
/// host/ip/clock line keeps the left end it has always had and the two never collide: if the panel is
/// too narrow to seat the bars clear of the text, they are skipped entirely for this frame (a 640×480
/// QEMU surface with a long SNTP-seeded clock line is the realistic case).
fn draw_pulse<P: GneissPal>(pal: &mut P, band_y: usize) {
    let st = PULSE.lock();
    if !st.armed || st.ncpu == 0 {
        return;
    }
    let m = pal.metrics();
    let w = pal.width() as usize;
    let (seg_w, seg_h, gap, bar_w, total_w) = bar_geometry(&m, st.ncpu);
    // Text extent this frame + one cell of clearance.
    let text_end = m.margin + compose().chars().count() * m.cell_w + m.cell_w;
    if total_w + m.margin > w || w - total_w - m.margin < text_end {
        return;
    }
    let mut x = w - m.margin - total_w;
    let y = band_y + m.line_h.saturating_sub(seg_h) / 2;
    for c in 0..st.ncpu {
        draw_pulse_bar(pal, x, y, seg_w, seg_h, gap, st.load[c], METER_PURPLE);
        x += bar_w + m.cell_w;
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
            let (seg_w, seg_h, gap, bar_w, total_w) = bar_geometry(&m, st.ncpu);
            let band_y = (pal.height() as usize).saturating_sub(m.line_h);
            serial_println!(
                "[pstrip] armed cores={} band=(0,{},{}x{}) bars=(x={},w={}) bar={}x{} seg={}x{} gap={} period={}ms",
                st.ncpu,
                band_y,
                pal.width(),
                m.line_h,
                (pal.width() as usize).saturating_sub(m.margin + total_w),
                total_w,
                bar_w,
                seg_h,
                seg_w,
                seg_h,
                gap,
                PSTRIP_PERIOD_MS
            );
            changed = true; // first frame must paint the bars
        } else if now.wrapping_sub(st.last_ms) >= PSTRIP_PERIOD_MS {
            st.last_ms = now;
            st.samples += 1;
            let demo = crate::arch::sched::meter_current_cpu();
            for c in 0..st.ncpu {
                let (b, i) = crate::arch::sched::meter_cpu_ticks(c);
                let db = b.wrapping_sub(st.prev[c].0);
                let di = i.wrapping_sub(st.prev[c].1);
                st.prev[c] = (b, i);
                // `own_load` is 0: the strip is drawn by a SCHEDULED task, so its core's counters
                // tick like every other core's and the demo-core fallback never fires here. Passing a
                // fabricated render load would be exactly the dishonesty VUG-HONESTY closed.
                let new = crate::vug::classify_load(db, di, c == demo, 0);
                if quantize(new) != quantize(st.load[c]) {
                    changed = true;
                }
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
