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

use alloc::string::String;
// FIX: Import the format! macro from alloc
use alloc::format;

use crate::pal::TargetPal;
use crate::user::UserSession;
use crate::pal::GneissPal;

pub struct Console {
    pub current_input: String,
    pub session: UserSession,
    history: alloc::vec::Vec<String>,
    /// Optional mirror for command-output lines (JD11). When set, every `println` line is also
    /// handed to this sink — the tegra console pump installs one that emits the line on the serial
    /// UART, so an attended Orin bench captures a durable, mbench-able output transcript instead of
    /// panel-only text (the panel has no scrollback; only keystrokes echoed to serial before JD11).
    /// `None` on every other surface (x86 / pi render service, headless), so those stay byte-for-byte
    /// unchanged — the sink is inert unless a caller opts in. Platform-neutral by design: the
    /// serial-line FORMAT (and the `tegra:` marker) lives in the tegra-gated caller, not here.
    out_sink: Option<fn(&str)>,
    /// SHELLWIN — this console renders into a compositor WINDOW surface, not the desktop backdrop.
    ///
    /// A window has no menu bar of its own — the bar is the desktop's furniture and composites above
    /// every window — so a windowed shell must NOT reserve `top_chrome_h` rows at the top of its own
    /// surface (that reservation exists to keep the BACKDROP shell clear of the desktop's bar). When
    /// this is set, [`Self::top_y`] drops the chrome term and the prompt/history start one page margin
    /// below the window's own top edge. `false` on every backdrop/headless surface (the desktop-layer
    /// shell, aarch64, wc-off), so those stay byte-for-byte unchanged.
    in_window: bool,
}

impl Console {
    pub fn new() -> Self {
        Self {
            current_input: String::new(),
            session: UserSession::new(),
            history: alloc::vec::Vec::new(),
            out_sink: None,
            in_window: false,
        }
    }

    /// SHELLWIN — mark this console as the tenant of a compositor WINDOW surface (see [`in_window`]).
    /// The render service calls this on the shell-window console so its layout drops the desktop
    /// menu-bar reservation; every backdrop/headless console leaves the default `false`.
    pub fn mark_in_window(&mut self) {
        self.in_window = true;
    }

    /// JD11: install a sink that receives each `println` line (in addition to the panel history).
    /// The tegra console pump uses this to mirror shell command output to serial. Opt-in — unset
    /// surfaces are unaffected. The sink is a plain `fn` pointer (no captured state): it must not
    /// call back into this `Console` (no re-entrancy) and must be free of any lock this call site
    /// could already hold; the tegra sink only touches the serial UART, which `println`'s callers
    /// never hold.
    pub fn set_output_sink(&mut self, sink: fn(&str)) {
        self.out_sink = Some(sink);
    }

    pub fn clear(&mut self) {
        // TERM_RING: retire whatever the transport is still holding as well, otherwise the next drain
        // would repaint lines the operator just cleared. The records are charged as absorbed — they
        // reached the view's owner, which then discarded them; that is a view decision, not transport
        // loss, and the ledger must not book it as one.
        let _ = crate::termring::drain(|_| {});
        self.history.clear();
    }

    /// Place one line in the VIEW's own store. The scrollback is bounded ([`Self::HISTORY_MAX`]) and
    /// drops OLDEST — the opposite of `termring`'s drop-NEWEST, and deliberately so: a transport must
    /// not make a producer wait, but a scrollback that discarded the newest line would stop showing
    /// the present.
    fn place(&mut self, text: &str) {
        self.history.push(String::from(text));
        if self.history.len() > Self::HISTORY_MAX {
            self.history.remove(0);
        }
    }

    /// TERM_RING (M2): move every record the transport is holding into the view's store, in order.
    /// Returns how many. Draining an empty ring is a no-op, so this is safe to call from any repaint
    /// site.
    ///
    /// **Fan-out precondition.** `termring::drain`'s exclusive-drainer contract is satisfied here by
    /// `&mut Console`, but note what that does and does not buy: the borrow is per-`Console` while
    /// `TERM_RING` is one global. With exactly one console — the shape today — the two coincide. A
    /// SECOND view (a `TerminalView`, a log sink) holding its own `&mut Console` would drain records
    /// destined for the first and neither borrow checker nor ring would object; the records would
    /// simply go to the wrong screen. That is the concrete reason fan-out needs the fixed subscriber
    /// array §3 describes rather than a second caller of this method, and it is a precondition to
    /// check before adding one, not a refactor to discover afterwards.
    pub fn drain_output(&mut self) -> u64 {
        let history = &mut self.history;
        let max = Self::HISTORY_MAX;
        crate::termring::drain(|line| {
            history.push(String::from(line));
            if history.len() > max {
                history.remove(0);
            }
        })
    }

    /// Emit one line of console output.
    ///
    /// TERM_RING (M2): the line goes through the terminal TRANSPORT rather than straight into the
    /// view's store, so this is no longer the only way a console line can come into existence — any
    /// producer may `termring::console_out`, including from a context that may not allocate or
    /// block. The drain happens immediately here because on today's surfaces the producer runs ON
    /// the render task; a foreign producer's records are picked up by the same drain, in FIFO order
    /// with these, at whichever of the two drain sites reaches them first.
    ///
    /// **The drain comes FIRST, and the order is load-bearing.** If the transport refuses the record
    /// (ring full — it is drop-newest and never blocks) the line is placed directly, and placing it
    /// while records older than it were still in flight would put it AHEAD of up to `TERM_SLOTS` of
    /// them. Draining first empties the ring, so the fallback line lands at the true tail and the
    /// scrollback stays in order even in the overflow case. What the refusal then costs is only the
    /// counted `dropped` charge — the record did not travel by the transport, and the ledger says so
    /// — not the reader's sense of what happened first.
    pub fn println(&mut self, text: &str) {
        self.drain_output();
        if !crate::termring::console_out_str(text) {
            self.place(text);
        }
        self.drain_output();
        // JD11: mirror the line to the output sink if one is installed (tegra bench transcript).
        // After the history push so a panic in the sink can't lose the panel line; the sink is a
        // no-op (`None`) on every non-tegra surface.
        if let Some(sink) = self.out_sink {
            sink(text);
        }
    }

    // Layout (UI-1): every dimension — top/left margin, line pitch, text advance, cursor — derives
    // from the panel's scale metrics (`pal.metrics()`; THE METRICS RULE: no absolute pixel sizes).
    // The full repaint and the per-keystroke fast path share the same derivation so the prompt sits
    // in the same place in both. Top-down terminal fill: history starts at the top and each new line
    // pushes the prompt DOWN; once the screen is full the oldest lines scroll off the top.

    /// Retained scrollback cap. The constraint it has to satisfy is `HISTORY_MAX > page_rows` for the
    /// tallest panel this kernel drives, or the bottom of a full screen would be starved (the old
    /// 25-line cap did exactly that at native resolution). The tallest is a 4K panel: 2160 rows puts
    /// `Metrics::for_height` at scale 2, so `line_h` is 24 and `page_rows` is 88. 256 is therefore
    /// just under three screenfuls there, and many more on anything smaller.
    const HISTORY_MAX: usize = 256;
    /// The console background (Moonstone).
    const BG: u32 = 0x2D2B55;

    /// The single source of truth for page height: rows of history that fit on one screen above the
    /// prompt line, derived from the panel's real usable height and the metrics' line pitch.
    /// Reserves the last row for the prompt/input line; a sane floor (6) keeps tiny/headless
    /// surfaces usable, and there is NO small ceiling — a page is exactly one screenful minus the
    /// prompt. `selftest::Pager` shares this so a pager page and a console screenful are always the
    /// same size.
    pub fn page_rows(pal: &TargetPal) -> usize {
        let m = pal.metrics();
        // DESKTOP semantics — the full-screen pager and the backdrop shell both reserve the desktop's
        // menu-bar chrome. `selftest::Pager` calls this with no `Console` in hand, so it stays a free
        // function computing the chrome-inclusive budget; the windowed shell uses the `&self`
        // [`Self::history_rows`] path, which drops the chrome per [`in_window`].
        let top = crate::ui_status::top_chrome_h(pal.width() as usize, pal.height() as usize)
            .saturating_add(m.margin);
        let usable = (pal.height() as usize).saturating_sub(top) / m.line_h;
        usable.saturating_sub(1).max(6)
    }

    /// SHELLDESK — **the shell's first row: the top of the glass MINUS the desktop scene's furniture.**
    ///
    /// The shell is a tenant of the desktop scene, not the scene itself. `crate::ui_status::top_chrome_h`
    /// is the reservation (the menu bar's own rect, read from the bar), and `m.margin` is the page
    /// margin the shell has always kept — so this is the old `m.margin` on every surface with no bar,
    /// which is every aarch64 boot, every x86 build without `wc`, and every x86 boot whose shell has
    /// not enabled one.
    ///
    /// Used by all three layout sites (the page budget, the prompt line, the full repaint) for the
    /// reason the module header already gives: the full repaint and the per-keystroke fast path share
    /// one derivation, or the prompt lands in two different places depending on which drew it.
    fn top_y(&self, pal: &TargetPal) -> usize {
        let m = pal.metrics();
        // SHELLWIN — a windowed shell reserves NO menu-bar chrome (the bar is desktop furniture that
        // composites above every window). A backdrop shell keeps the reservation, so this is the old
        // expression unchanged on every surface where [`in_window`] is `false`.
        let chrome = if self.in_window {
            0
        } else {
            crate::ui_status::top_chrome_h(pal.width() as usize, pal.height() as usize)
        };
        chrome.saturating_add(m.margin)
    }

    /// Rows of history shown above the prompt: everything that fits from `TOP` down, reserving the
    /// last row for the prompt/input line itself. Computed from [`Self::top_y`] so a windowed shell
    /// (no chrome) and a backdrop shell (chrome reserved) each get the budget for their own surface;
    /// for a backdrop shell this is identical to [`Self::page_rows`] by construction.
    fn history_rows(&self, pal: &TargetPal) -> usize {
        let m = pal.metrics();
        let usable = (pal.height() as usize).saturating_sub(self.top_y(pal)) / m.line_h;
        usable.saturating_sub(1).max(6)
    }

    /// The y of the prompt/input line: directly below the last shown history line (so on a fresh
    /// screen the prompt sits at the top and walks down as output arrives; once full it pins to the
    /// last usable row because the history is scrolled).
    fn prompt_y(&self, pal: &TargetPal) -> usize {
        let m = pal.metrics();
        let rows = self.history_rows(pal);
        let shown = self.history.len().min(rows);
        self.top_y(pal) + shown * m.line_h
    }

    /// Draw the prompt + live input + cursor at `prompt_y`. Shared by the full repaint and the
    /// per-keystroke fast path so the two can never disagree. The cursor is BY CONSTRUCTION exactly
    /// one metrics cell (`cell_w`×`cell_h`) — the same cell the glyph renderer fills — so it is
    /// always precisely one character in size, at every scale (the old hardcoded 8×16 block stood
    /// twice the 8×8 text height).
    fn draw_prompt_line(&self, pal: &mut TargetPal, prompt_y: usize) {
        let m = pal.metrics();
        let prompt = format!("{}@unaos:~$ ", self.session.username);
        pal.draw_text(m.margin, prompt_y, &prompt, 0x00FF00); // Green Prompt

        let input_x = m.margin + m.text_w(prompt.len());
        pal.draw_text(input_x, prompt_y, &self.current_input, 0xFFFFFF);

        let cursor_x = input_x + m.text_w(self.current_input.len());
        pal.draw_rect(cursor_x, prompt_y, m.cell_w, m.cell_h, 0xFFFFFF); // exactly one cell
    }

    pub fn draw(&self, pal: &mut TargetPal) {
        let m = pal.metrics();
        pal.clear_screen(Self::BG);

        // Show the last `history_rows` lines (scroll the oldest off the top when full), top-down.
        let rows = self.history_rows(pal);
        let skip = self.history.len().saturating_sub(rows);
        let mut y = self.top_y(pal);
        for line in self.history.iter().skip(skip) {
            pal.draw_text(m.margin, y, line, 0xAAAAAA);
            y += m.line_h;
        }

        // Prompt directly below the last output line.
        self.draw_prompt_line(pal, y);
    }

    /// Repaint ONLY the prompt/input line. This is the per-keystroke path: typing changes just
    /// that line, so we clear its strip and redraw it instead of repainting the whole screen.
    /// With damage tracking that means each keystroke flushes ~one text row, not the full frame —
    /// the difference between snappy and unusable at native resolution. Use `draw()` for the full
    /// repaint after command output (history changes).
    pub fn draw_input_line(&self, pal: &mut TargetPal) {
        let m = pal.metrics();
        let prompt_y = self.prompt_y(pal);
        // Clear the input-line strip (one full line pitch) back to the background.
        pal.draw_rect(0, prompt_y, pal.width() as usize, m.line_h, Self::BG);
        self.draw_prompt_line(pal, prompt_y);
    }
}
