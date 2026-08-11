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
