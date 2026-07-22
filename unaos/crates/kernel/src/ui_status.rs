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

use crate::pal::GneissPal;
use alloc::format;
use alloc::string::String;

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
}
