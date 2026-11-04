// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Lesser General Public License for more details.
//
// You should have received a copy of the GNU Lesser General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! # comscan — hardware I/O + the Console log viewer
//!
//! Comscan is the telemetry/serial handler (squawk's planned home). Its first
//! landed capability is the **Console app** — the macOS `Console.app`
//! equivalent: a ring-3 program that SUBSCRIBES to the system log feed, keeps a
//! bounded scrollback, and publishes a filtered, tailing view for a vessel to
//! render. The serial / GPIO / Bluetooth / SDR scope in `README.md` remains
//! design-stage; this module is the crate's first working code.
//!
//! ## The log feed — host vs metal
//!
//! - **Host (today).** Every component that logs fires an
//!   [`bandy::SMessage::Log`] `{ level, source, content }` onto the Synapse bus.
//!   The Console app subscribes to that one feed — the same bus every other
//!   host-native handler already reads. There is no separate log daemon.
//! - **Metal (the swap).** The kernel's `TERM_RING`
//!   (`unaos/crates/kernel/src/termring.rs`) is the console-output stream. When
//!   the app runs over a real kernel, the ingest edge reads `TERM_RING` records
//!   instead of `SMessage::Log`; each drained line becomes one
//!   [`LogView::on_log`] call. Nothing else changes — [`LogView`] and the
//!   [`bandy::LogEvent`] contract are transport-agnostic (the same swap the
//!   matrix Finder documented for its FAT↔UnaFS backing).
//!
//! ## Command / render contract
//!
//! View→handler: [`LogEvent::LogFilter`], [`LogEvent::LogSource`],
//! [`LogEvent::LogPause`]. Handler→view: [`LogEvent::LogTail`]. The handler
//! never reacts to its own `LogTail`, so publisher and subscriber safely share
//! one broadcast bus.

pub mod logview;
pub mod pump; // VUGCOMSCAN1 (SR476): the pump, whose only entry is Helm's Verdict::Pass
#[cfg(any(target_os = "linux", target_os = "macos"))] pub mod tty; // VUGCOMSCANTTY (SR484): the host serial port (termios) as the pump's Transport
pub mod tcp; // VUGCOMSCANTCP (SR487): grblHAL / FluidNC raw TCP (tcp://host:port) as the pump's Transport
pub mod ws; // VUGCOMSCANWS (SR499): FluidNC / ESP3D WebSocket (ws://host:port/path) as the pump's Transport over gneiss_pal's RFC 6455 client

pub use logview::LogView;

use bandy::{LogEvent, SMessage, Synapse};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::broadcast::Receiver;

/// Default scrollback depth for the Console app: 4096 records. Bounded by
/// construction (`LogView` never grows past it); an operator who wants more
/// history changes this one number, not the drop policy.
pub const DEFAULT_SCROLLBACK: usize = 4096;

/// Ignite the Console log-viewer capability on the bus.
///
/// Subscribes to the Synapse, folds every [`SMessage::Log`] into a bounded
/// [`LogView`], and folds the view→handler [`SMessage::Logs`] commands into the
/// same view; each state change publishes an [`SMessage::Logs`] carrying a
/// fresh [`LogEvent::LogTail`]. Runs until the bus closes (or a
/// [`SMessage::Kill`] naming `"comscan"` arrives). `cap` is the scrollback
/// depth ([`DEFAULT_SCROLLBACK`] is the usual choice).
pub async fn ignite(synapse: Synapse, cap: usize) {
    let rx = synapse.subscribe();
    serve(synapse, rx, cap).await;
}

/// The Console handler's event loop over an already-open receiver.
///
/// Split out from [`ignite`] so a caller (or a test) can subscribe FIRST and
/// then hand the receiver in, closing the broadcast race where traffic fired
/// between `subscribe()` and the first `recv()` would be lost. [`ignite`] is
/// the ordinary entry; `serve` is the seam.
pub async fn serve(synapse: Synapse, mut rx: Receiver<SMessage>, cap: usize) {
    let mut view = LogView::new(cap);
    let mut card = PumpCard::new(bandy::Origin::System("comscan".to_string())); // VUGCOMSCANJOGFACE (SR579): what the Console says of the pump

    loop {
        match rx.recv().await {
            Ok(SMessage::Log { level, source, content }) => {
                if let Some(tail) = view.on_log(level, source, content) {
                    synapse.fire(SMessage::Logs(tail));
                }
            }
            // Viewer COMMANDS only. A `LogTail` here is our own published
            // output echoing back on the broadcast bus; `LogView::apply`
            // returns `None` for it, so the guard below simply drops it and we
            // never loop on ourselves.
            Ok(SMessage::Logs(ev)) if !matches!(ev, LogEvent::LogTail { .. }) => {
                if let Some(tail) = view.apply(&ev) {
                    synapse.fire(SMessage::Logs(tail));
                }
            }
            // VUGCOMSCANJOGFACE (SR579): the pump's answers the Console says in its own feed (`commissioned <name>`, once).
            Ok(SMessage::Pump(ev)) => {
                if let Some(line) = card.on_pump(&ev) {
                    if let Some(tail) = view.on_log("info", "comscan", line) {
                        synapse.fire(SMessage::Logs(tail));
                    }
                }
            }
            Ok(SMessage::Kill(who)) if who == "comscan" => break,
            // Every other signal on the shared bus, including our own echoed
            // `LogTail`, is not ours to act on.
            Ok(_) => {}
            // The broadcast channel outran this subscriber. The dropped signals
            // are other handlers' traffic; the log feed is best-effort at the
            // viewer, so we resume rather than tear the app down.
            Err(RecvError::Lagged(_)) => continue,
            Err(RecvError::Closed) => break,
        }
    }
}

// --- VUGCOMSCANHOSTDESK (LEDGER SR561): the host's pump desk ignited on the bus (named tail hunk) ---

/// Ignite Comscan's pump desk on the bus beside the Console: `pump::host_parts` (THE composition: the identity hook,
/// the hello on so a first bind commissions an unnamed board, the ws / tcp / serial opener) for `machine`, the
/// receiver subscribed FIRST, then [`pump::serve_pump`] spawned at [`pump::PUMP_TICK`]. No port is opened until a
/// `PumpStart`, so with no controller attached the desk idles and says nothing. The desk is returned beside the task:
/// arming (Helm's `Verdict::Pass`) happens in process only, through it. Call inside a tokio runtime context.
pub fn host_desk(
    synapse: Synapse,
    machine: make_core::Machine,
) -> (std::sync::Arc<std::sync::Mutex<pump::PumpDesk>>, tokio::task::JoinHandle<()>) {
    let (desk, open) = pump::host_parts(machine);
    let desk = std::sync::Arc::new(std::sync::Mutex::new(desk));
    let rx = synapse.subscribe();
    let task = tokio::spawn(pump::serve_pump(synapse, rx, desk.clone(), open, pump::PUMP_TICK));
    (desk, task)
}

// --- VUGCOMSCANJOGFACE (LEDGER SR579): the Console's pump card, a pure core the vessel binds pixels to (named tail hunk) ---

/// A button on the Console's pump card: home, or a jog of one axis by the card's step, plus or minus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pad {
    Home,
    XPlus,
    XMinus,
    YPlus,
    YMinus,
    ZPlus,
    ZMinus,
}

/// The Console's pump card, in [`LogView`]'s shape (a pure core; the vessel draws it): the jog pad's two fields (the
/// step in mm and the feed in mm/min) and its buttons as the bus requests the desk already serves
/// ([`bandy::PumpEvent::PumpHome`] / [`bandy::PumpEvent::PumpJog`]; no new transport, and no second validator: the
/// desk refuses a jog it cannot say by name), and what the Console says of the pump's answers: `commissioned <name>`
/// the first time a port takes a name. [`serve`] holds one for what the Console says; a face holds one for its pad.
pub struct PumpCard {
    principal: bandy::Origin,
    step: f64,
    feed: f64,
    said: Vec<(String, String)>,
}

impl PumpCard {
    /// The step a fresh card jogs by, mm.
    pub const STEP_MM: f64 = 1.0;
    /// The feed a fresh card jogs at, mm/min.
    pub const FEED_MM_MIN: f64 = 600.0;

    /// A card whose requests carry `principal` (the message-security law), at [`PumpCard::STEP_MM`] and
    /// [`PumpCard::FEED_MM_MIN`].
    pub fn new(principal: bandy::Origin) -> PumpCard {
        PumpCard { principal, step: Self::STEP_MM, feed: Self::FEED_MM_MIN, said: Vec::new() }
    }

    /// The step field, mm.
    pub fn step(&self) -> f64 {
        self.step
    }

    /// The feed field, mm/min.
    pub fn feed(&self) -> f64 {
        self.feed
    }

    /// Set the step field as typed (the desk names a jog it cannot say).
    pub fn set_step(&mut self, mm: f64) {
        self.step = mm;
    }

    /// Set the feed field as typed (the desk names a feed it refuses).
    pub fn set_feed(&mut self, mm_per_min: f64) {
        self.feed = mm_per_min;
    }

    /// The request a press of `pad` posts: [`Pad::Home`] the home, an axis a jog of that axis by plus or minus the
    /// step at the feed.
    pub fn press(&self, pad: Pad) -> SMessage {
        let principal = self.principal.clone();
        let s = self.step;
        let (dx, dy, dz) = match pad {
            Pad::Home => return SMessage::Pump(bandy::PumpEvent::PumpHome { principal }),
            Pad::XPlus => (s, 0.0, 0.0),
            Pad::XMinus => (-s, 0.0, 0.0),
            Pad::YPlus => (0.0, s, 0.0),
            Pad::YMinus => (0.0, -s, 0.0),
            Pad::ZPlus => (0.0, 0.0, s),
            Pad::ZMinus => (0.0, 0.0, -s),
        };
        SMessage::Pump(bandy::PumpEvent::PumpJog { principal, dx, dy, dz, feed: self.feed })
    }

    /// What the Console says of a pump answer: `commissioned <name>` the first time `(port, name)` arrives in a
    /// [`bandy::PumpEvent::Commissioned`]; nothing for a repeat or any other answer.
    pub fn on_pump(&mut self, ev: &bandy::PumpEvent) -> Option<String> {
        let bandy::PumpEvent::Commissioned { port, name } = ev else { return None };
        let key = (port.clone(), name.clone());
        if self.said.contains(&key) {
            return None;
        }
        self.said.push(key);
        Some(format!("commissioned {name}"))
    }
}
