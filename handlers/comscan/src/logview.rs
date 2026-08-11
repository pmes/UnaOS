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

//! # `LogView` — the Console app's scrollback core
//!
//! The pure, synchronous heart of the Console log viewer (the macOS
//! `Console.app` equivalent). It is deliberately free of the bus and of async:
//! it takes records IN, applies view commands, and answers with the snapshot the
//! vessel renders. `ignite` (in `lib.rs`) is the only part that touches
//! `Synapse`; everything falsifiable lives here so the tests need no runtime.
//!
//! ## Scrollback is drop-OLDEST — the opposite of `termring`
//!
//! The kernel's `TERM_RING` is a TRANSPORT and drops the NEWEST record it cannot
//! carry (the backlog is the symptom; the newest line is the one the producer
//! could not afford to wait for). A scrollback is the other role: it must keep
//! showing the PRESENT, so when it overflows it evicts the OLDEST line. Both
//! bounds are hard — the ring never grows past its cap — and both count what
//! they drop so a truncated view says so.

use bandy::state::{LogLine, LogSource};
use bandy::LogEvent;
use std::collections::VecDeque;

/// The bounded log scrollback plus the active view query.
///
/// Invariant: `ring.len() <= cap` at all times, and `cap >= 1`. Every evicted
/// record bumps `dropped`, which only ever grows (an eviction is a real event,
/// like `termring`'s ledger — it is not rewound).
pub struct LogView {
    cap: usize,
    ring: VecDeque<LogLine>,
    seq: u64,
    dropped: u64,
    filter: String,
    source: LogSource,
    paused: bool,
}

impl LogView {
    /// A viewer holding at most `cap` records. `cap` is clamped up to 1 — a
    /// zero-capacity scrollback could never hold the line it just ingested and
    /// would loop dropping it, so the type refuses to represent that.
    pub fn new(cap: usize) -> Self {
        let cap = cap.max(1);
        Self {
            cap,
            ring: VecDeque::with_capacity(cap),
            seq: 0,
            dropped: 0,
            filter: String::new(),
            source: LogSource::All,
            paused: false,
        }
    }

    /// Ingest one raw record (an `SMessage::Log` on host, a `TERM_RING` line on
    /// metal). Assigns the next monotonic `seq`, appends at the tail, and evicts
    /// from the head until the cap holds — drop-OLDEST, so the present survives.
    /// Returns the `seq` the record was given.
    pub fn ingest(
        &mut self,
        level: impl Into<String>,
        source: impl Into<String>,
        content: impl Into<String>,
    ) -> u64 {
        let seq = self.seq;
        self.seq += 1;
        self.ring.push_back(LogLine {
            seq,
            level: level.into(),
            source: source.into(),
            content: content.into(),
        });
        while self.ring.len() > self.cap {
            self.ring.pop_front();
            self.dropped += 1;
        }
        seq
    }

    /// Records currently held (never exceeds the cap).
    pub fn len(&self) -> usize {
        self.ring.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ring.is_empty()
    }

    /// The configured hard bound.
    pub fn cap(&self) -> usize {
        self.cap
    }

    /// Records evicted by the bound since construction. Monotonic.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    pub fn set_filter(&mut self, q: impl Into<String>) {
        self.filter = q.into();
    }

    pub fn set_source(&mut self, s: LogSource) {
        self.source = s;
    }

    pub fn set_paused(&mut self, p: bool) {
        self.paused = p;
    }

    /// Does a line pass the active source facet AND text filter?
    fn matches(&self, line: &LogLine) -> bool {
        let source_ok = match &self.source {
            LogSource::All => true,
            LogSource::Subsystem(s) => &line.source == s,
        };
        if !source_ok {
            return false;
        }
        if self.filter.is_empty() {
            return true;
        }
        let needle = self.filter.to_lowercase();
        line.content.to_lowercase().contains(&needle)
            || line.source.to_lowercase().contains(&needle)
            || line.level.to_lowercase().contains(&needle)
    }

    /// The filtered scrollback snapshot, oldest→newest, that the view renders.
    pub fn snapshot(&self) -> Vec<LogLine> {
        self.ring.iter().filter(|l| self.matches(l)).cloned().collect()
    }

    /// The render message for the bus: the filtered snapshot plus the honest
    /// eviction count and the pause flag.
    pub fn tail(&self) -> LogEvent {
        LogEvent::LogTail {
            lines: self.snapshot(),
            dropped: self.dropped,
            paused: self.paused,
        }
    }

    /// Apply a viewer COMMAND and return the `LogTail` to publish. A command is
    /// a deliberate user action (set a filter, pick a source, toggle the lock),
    /// so it ALWAYS answers with a fresh tail — the effect must be visible at
    /// once, even under scroll-lock. A `LogTail` handed in is the handler's own
    /// output and yields `None` (it never reacts to itself).
    pub fn apply(&mut self, ev: &LogEvent) -> Option<LogEvent> {
        match ev {
            LogEvent::LogFilter(q) => {
                self.set_filter(q.clone());
                Some(self.tail())
            }
            LogEvent::LogSource(s) => {
                self.set_source(s.clone());
                Some(self.tail())
            }
            LogEvent::LogPause(p) => {
                self.set_paused(*p);
                Some(self.tail())
            }
            LogEvent::LogTail { .. } => None,
        }
    }

    /// Ingest a raw record and return the tail to publish — or `None` while the
    /// tail is paused. Paused ingest still GROWS the ring (and may evict); it
    /// just does not move the view. This is Console.app's scroll-lock exactly:
    /// the log keeps flowing behind the frozen pane.
    pub fn on_log(
        &mut self,
        level: impl Into<String>,
        source: impl Into<String>,
        content: impl Into<String>,
    ) -> Option<LogEvent> {
        self.ingest(level, source, content);
        if self.paused {
            None
        } else {
            Some(self.tail())
        }
    }
}
