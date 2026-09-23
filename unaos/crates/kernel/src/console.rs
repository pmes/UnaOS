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
    /// TERMSEL — the selection over [`Self::current_input`]. Changed only by
    /// `video::clipboard::terminal_action` (the selection actions, `Cut`) and by
    /// `main::handle_key`'s edit rule; read by [`Self::draw_prompt_line`], which paints it.
    pub sel: crate::video::termsel::LineSel,
    /// TERMSEL2 — how many lines have been dropped off the front of [`Self::history`], so
    /// `hist_base + i` is line `i`'s ABSOLUTE number: the row a selection records, which does not
    /// change as newer output pushes the line up the screen.
    hist_base: u64,
}

impl Console {
    pub fn new() -> Self {
        Self {
            current_input: String::new(),
            session: UserSession::new(),
            history: alloc::vec::Vec::new(),
            out_sink: None,
            in_window: false,
            sel: crate::video::termsel::LineSel::new(),
            hist_base: 0,
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
            self.hist_base += 1;
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
        let base = &mut self.hist_base;
        let max = Self::HISTORY_MAX;
        crate::termring::drain(|line| {
            history.push(String::from(line));
            if history.len() > max {
                history.remove(0);
                *base += 1;
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
        // SHELLWIN — a windowed shell reserves NO menu-bar chrome (the bar is desktop furniture that
        // composites above every window). A backdrop shell keeps the reservation, so this is the old
        // expression unchanged on every surface where [`in_window`] is `false`. TERMSEL2: the body
        // is [`Self::top_y_for`], so the pointer's cell lookup and the painter share it.
        self.top_y_for(pal.metrics(), pal.width() as usize, pal.height() as usize)
    }

    /// Rows of history shown above the prompt: everything that fits from `TOP` down, reserving the
    /// last row for the prompt/input line itself. Computed from [`Self::top_y`] so a windowed shell
    /// (no chrome) and a backdrop shell (chrome reserved) each get the budget for their own surface;
    /// for a backdrop shell this is identical to [`Self::page_rows`] by construction.
    fn history_rows(&self, pal: &TargetPal) -> usize {
        self.history_rows_for(pal.metrics(), pal.width() as usize, pal.height() as usize)
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

        // TERMSEL — the selection as an INVERSE-VIDEO band: the selected cells filled with the text
        // colour, their characters redrawn in the background colour. Here and nowhere else, so the
        // full repaint and the per-keystroke path (both call this) paint the same band. One cell
        // tall, exactly as the cursor below is. No witness: a repaint is not a state change (the
        // model prints those).
        // TERMSEL2: `cols_on(EDIT_ROW, …)` — the editable line's part of ANY selection, one that
        // started in the scrollback included; for a selection wholly on this line it is `range`.
        let len = self.current_input.len();
        if let Some((lo, hi)) = self.sel.cols_on(crate::video::termsel::EDIT_ROW, len, len) {
            let band_x = input_x + m.text_w(lo);
            pal.draw_rect(band_x, prompt_y, m.text_w(hi - lo), m.cell_h, 0xFFFFFF);
            pal.draw_text(band_x, prompt_y, self.current_input.get(lo..hi).unwrap_or(""), Self::BG);
        }

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
        for (i, line) in self.history.iter().enumerate().skip(skip) {
            pal.draw_text(m.margin, y, line, 0xAAAAAA);
            // TERMSEL2 — a selection's band on a SCROLLBACK row, the same inverse video the editable
            // line gets (`draw_prompt_line`), read from the same model (`LineSel::cols_on`).
            self.draw_row_band(pal, y, self.hist_base + i as u64, line);
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

// --- TERMSEL2 — the pointer on the terminal's text ------------------------------------------------
//
// Tail-appended in its own `impl` so the layout code above keeps its lines. The console is the one
// party that knows where a cell is — the prompt's width, the page's top, how many history rows are
// shown — so it is the one that turns a pointer note (surface pixels, `video::termsel::Press`) into a
// cell, and it does it with the SAME derivation the painter draws with: [`Console::top_y_for`] and
// [`Console::history_rows_for`] are what `top_y`/`history_rows` compute, from the metrics and the
// surface size rather than from a `TargetPal`, so a fixture with no panel can ask exactly the
// question the render service asks.
impl Console {
    /// The prompt's width in cells (`draw_prompt_line` prints `{user}@unaos:~$ `).
    pub fn prompt_cells(&self) -> usize {
        self.session.username.len() + "@unaos:~$ ".len()
    }

    /// [`Self::top_y`] for a surface `w`x`h` with metrics `m`.
    fn top_y_for(&self, m: crate::ui::Metrics, w: usize, h: usize) -> usize {
        let chrome = if self.in_window { 0 } else { crate::ui_status::top_chrome_h(w, h) };
        chrome.saturating_add(m.margin)
    }

    /// [`Self::history_rows`] for a surface `w`x`h` with metrics `m`.
    fn history_rows_for(&self, m: crate::ui::Metrics, w: usize, h: usize) -> usize {
        let usable = h.saturating_sub(self.top_y_for(m, w, h)) / m.line_h;
        usable.saturating_sub(1).max(6)
    }

    /// The CELL under surface pixel `(lx, ly)`: `(row, col, cells)` — the row an absolute scrollback
    /// line or `termsel::EDIT_ROW`, the column the character cell under the pointer clamped to one
    /// past the row's last character, and the row's cell count. A point above the first shown row
    /// reads as that row, a point below the prompt as the prompt, a point left of a row's text as its
    /// first cell — a drag that leaves the text keeps a cell to report.
    pub fn cell_at(&self, m: crate::ui::Metrics, w: usize, h: usize, lx: i32, ly: i32) -> (u64, usize, usize) {
        let top = self.top_y_for(m, w, h);
        let shown = self.history.len().min(self.history_rows_for(m, w, h));
        let skip = self.history.len() - shown;
        let (lx, ly) = (lx.max(0) as usize, ly.max(0) as usize);
        let vrow = ly.saturating_sub(top) / m.line_h;
        let (row, x0, cells) = if vrow < shown {
            let i = skip + vrow;
            (self.hist_base + i as u64, m.margin, self.history[i].chars().count())
        } else {
            (
                crate::video::termsel::EDIT_ROW,
                m.margin + m.text_w(self.prompt_cells()),
                self.current_input.len(),
            )
        };
        let col = core::cmp::min(lx.saturating_sub(x0) / m.cell_w, cells);
        (row, col, cells)
    }

    /// The text of row `row` (an absolute scrollback line or `termsel::EDIT_ROW`), or `""` for a line
    /// that has left the scrollback.
    pub fn row_text(&self, row: u64) -> &str {
        if row == crate::video::termsel::EDIT_ROW {
            return &self.current_input;
        }
        match row.checked_sub(self.hist_base) {
            Some(i) if (i as usize) < self.history.len() => &self.history[i as usize],
            _ => "",
        }
    }

    /// One pointer note, against the console's own layout on a surface `w`x`h` with metrics `m`.
    /// Returns the repaint owed (see [`Self::repaint`]).
    pub fn pointer_at(&mut self, p: &crate::video::termsel::Press, m: crate::ui::Metrics, w: usize, h: usize) -> u8 {
        let (row, col, _) = self.cell_at(m, w, h, p.lx, p.ly);
        let len = self.current_input.len();
        let text = if row == crate::video::termsel::EDIT_ROW {
            self.current_input.as_str()
        } else {
            match row.checked_sub(self.hist_base) {
                Some(i) if (i as usize) < self.history.len() => self.history[i as usize].as_str(),
                _ => "",
            }
        };
        self.sel.pointer(p.kind, p.ms, row, col, text, len)
    }

    /// [`Self::pointer_at`] on the surface `pal` draws.
    pub fn pointer(&mut self, p: &crate::video::termsel::Press, pal: &TargetPal) -> u8 {
        self.pointer_at(p, pal.metrics(), pal.width() as usize, pal.height() as usize)
    }

    /// Pay a repaint the selection model asked for: 1 the input line, 2 the whole terminal (a band
    /// in the scrollback moved). Returns whether anything was painted.
    pub fn repaint(&self, code: u8, pal: &mut TargetPal) -> bool {
        match code {
            0 => false,
            1 => {
                self.draw_input_line(pal);
                true
            }
            _ => {
                self.draw(pal);
                true
            }
        }
    }

    /// The selection's band on scrollback row `row` (text `line`, drawn at `y`): the selected cells
    /// filled with the text colour and their characters redrawn in the background colour.
    fn draw_row_band(&self, pal: &mut TargetPal, y: usize, row: u64, line: &str) {
        let m = pal.metrics();
        let cells = line.chars().count();
        if let Some((lo, hi)) = self.sel.cols_on(row, cells, self.current_input.len()) {
            let x = m.margin + m.text_w(lo);
            pal.draw_rect(x, y, m.text_w(hi - lo), m.cell_h, 0xFFFFFF);
            let part: String = line.chars().skip(lo).take(hi - lo).collect();
            pal.draw_text(x, y, &part, Self::BG);
        }
    }

    /// TERMSEL2 — what the terminal does with a resolved desktop ACTION, with its whole text in hand
    /// (`video::clipboard::terminal_action_in` gets the scrollback, which a pointer selection can
    /// reach), and the repaint that action owes paid on `pal`. Returns whether anything was painted,
    /// so the caller can mark its window dirty as it does for a keystroke.
    pub fn act(&mut self, a: crate::video::keymap::Action, pal: &mut TargetPal) -> bool {
        let (_, r) = crate::video::clipboard::terminal_action_in(
            a,
            &mut self.current_input,
            &mut self.sel,
            &self.history,
            self.hist_base,
        );
        self.repaint(r, pal)
    }

    /// Fixture seam: [`Self::act`] without a surface — the action's field and repaint code.
    #[cfg(feature = "witness")]
    pub fn act_for_fixture(&mut self, a: crate::video::keymap::Action) -> (&'static str, u8) {
        crate::video::clipboard::terminal_action_in(a, &mut self.current_input, &mut self.sel, &self.history, self.hist_base)
    }

    /// Fixture seam: place a scrollback line WITHOUT the transport. `println` drains the global
    /// `TERM_RING`, and a fixture's console draining it would steal the records the real console is
    /// owed (the fan-out precondition on [`Self::drain_output`]).
    #[cfg(feature = "witness")]
    pub fn place_for_fixture(&mut self, text: &str) {
        self.place(text);
    }
}
