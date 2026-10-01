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
    /// TERMCOLOR (R75) — parallel to `history`: line `i`'s run-length attribute spans (empty = plain, no allocation).
    attrs: alloc::vec::Vec<alloc::vec::Vec<crate::termcolor::Span>>,
    /// TERMCOLOR — the running SGR pen, carried across lines like a terminal's.
    pen: crate::termcolor::Attr,
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
    /// TERMSEL/TERMSEL2 — the selection and the caret. Changed only by the action consumer
    /// (`terminal_action_in`, via [`Self::act`]), `main::handle_key`'s edits and [`Self::pointer`];
    /// read by the painters (`draw_prompt_line`, `draw_row_band`), which paint it.
    pub sel: crate::video::termsel::LineSel,
    /// TERMSEL2 — how many lines have been dropped off the front of [`Self::history`], so
    /// `hist_base + i` is line `i`'s ABSOLUTE number: the row a selection records, which does not
    /// change as newer output pushes the line up the screen.
    hist_base: u64,
    /// TERMWRAP — the edit line's visual-row count at the last full repaint (`draw`), so the per-keystroke path knows when a wrap changed the page layout and owes a full repaint.
    last_edit_rows: core::cell::Cell<usize>,
    /// SCROLLBACK (R75) — how many history ENTRIES the view is scrolled up from the bottom (0 = live view). Entry-relative, not row-relative, so a resize that re-wraps lines cannot strand it.
    view_off: usize,
    /// SCROLLBACK — lines that arrived while `view_off > 0` (the `[N new lines]` marker); zero again at the bottom.
    new_lines: usize,
    /// SCROLLBACK — absolute row below which the LIVE view shows (`clear` / Ctrl-L moves it; the lines above stay reachable by scrolling). `clear --all` drops them instead.
    clear_abs: u64,
}

impl Console {
    pub fn new() -> Self {
        Self {
            current_input: String::new(),
            session: UserSession::new(),
            history: alloc::vec::Vec::new(),
            attrs: alloc::vec::Vec::new(),
            pen: crate::termcolor::Attr::DEFAULT,
            out_sink: None,
            in_window: false,
            sel: crate::video::termsel::LineSel::new(),
            hist_base: 0,
            last_edit_rows: core::cell::Cell::new(1),
            view_off: 0,
            new_lines: 0,
            clear_abs: 0,
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
        // SCROLLBACK — `clear` blanks the LIVE view and KEEPS the lines (scroll up to read them); `clear_all` drops them. Pending transport records are placed first so they sit above the mark, not on the fresh screen.
        self.drain_output();
        self.clear_abs = self.hist_base + self.history.len() as u64;
        self.view_off = 0;
        self.new_lines = 0;
        self.last_edit_rows.set(usize::MAX);
    }

    /// SCROLLBACK — `clear --all`: the screen AND the scrollback go. `hist_base` advances past the dropped lines so no stale selection row can alias a new line.
    pub fn clear_all(&mut self) {
        let _ = crate::termring::drain(|_| {});
        self.hist_base += self.history.len() as u64;
        self.history.clear();
        self.attrs.clear();
        self.clear_abs = self.hist_base;
        self.view_off = 0;
        self.new_lines = 0;
        self.sel.end_selection();
        self.last_edit_rows.set(usize::MAX);
    }

    /// Place one line in the VIEW's own store. The scrollback is bounded ([`Self::HISTORY_MAX`]) and
    /// drops OLDEST — the opposite of `termring`'s drop-NEWEST, and deliberately so: a transport must
    /// not make a producer wait, but a scrollback that discarded the newest line would stop showing
    /// the present.
    fn place(&mut self, text: &str) {
        // TERMCOLOR — escapes are parsed at ingest: the store keeps plain text + spans.
        let n = push_parsed(&mut self.history, &mut self.attrs, &mut self.hist_base, &mut self.clear_abs, &mut self.pen, text);
        self.note_new(n);
    }

    /// SCROLLBACK — `n` lines were appended. A scrolled-up view is ANCHORED (its offset grows with the tail so the rows on the glass do not move) and the marker counts them; the live view is untouched.
    fn note_new(&mut self, n: usize) {
        if n > 0 && self.view_off > 0 {
            self.view_off = core::cmp::min(self.view_off + n, self.history.len().saturating_sub(1));
            self.new_lines += n;
            self.last_edit_rows.set(usize::MAX);
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
        let (history, attrs, base, clear_abs, pen) = (&mut self.history, &mut self.attrs, &mut self.hist_base, &mut self.clear_abs, &mut self.pen);
        let mut placed = 0usize;
        let n = crate::termring::drain(|line| { placed += push_parsed(history, attrs, base, clear_abs, pen, line); });
        self.note_new(placed);
        n
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
    pub const HISTORY_MAX: usize = 2000; // SCROLLBACK (R75): was 256 — the store keeps whole lines (they re-wrap on resize), bounded here, oldest dropped.
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
    #[allow(dead_code)] // TERMWRAP: superseded by `layout_for` (visual rows); kept as the entry-count budget's name
    fn history_rows(&self, pal: &TargetPal) -> usize {
        self.history_rows_for(pal.metrics(), pal.width() as usize, pal.height() as usize)
    }

    /// The y of the prompt/input line: directly below the last shown history line (so on a fresh
    /// screen the prompt sits at the top and walks down as output arrives; once full it pins to the
    /// last usable row because the history is scrolled).
    fn prompt_y(&self, pal: &TargetPal) -> usize {
        let m = pal.metrics();
        // TERMWRAP M2 — the history above costs VISUAL rows (a long line wraps), not one per entry.
        let (_, vrows) = self.layout_for(m, pal.width() as usize, pal.height() as usize);
        self.top_y(pal) + vrows * m.line_h
    }

    /// Draw the prompt + live input + cursor at `prompt_y`. Shared by the full repaint and the
    /// per-keystroke fast path so the two can never disagree. The cursor is BY CONSTRUCTION exactly
    /// one metrics cell (`cell_w`×`cell_h`) — the same cell the glyph renderer fills — so it is
    /// always precisely one character in size, at every scale (the old hardcoded 8×16 block stood
    /// twice the 8×8 text height).
    fn draw_prompt_line(&self, pal: &mut TargetPal, prompt_y: usize) {
        let m = pal.metrics();
        let prompt = format!("{}@unaos:~$ ", self.session.username);
        // TERMWRAP M1 — the prompt + input is ONE run of cells wrapped at the window's column count.
        let cols = self.cols_for(m, pal.width() as usize);
        let pc = prompt.len();
        let len = self.current_input.len();
        let rows = self.edit_rows_for(cols);
        for r in 0..rows {
            let (lo, hi) = (r * cols, r * cols + cols);
            let y = prompt_y + r * m.line_h;
            if lo < pc {
                pal.draw_text(m.margin, y, prompt.get(lo..hi.min(pc)).unwrap_or(""), crate::video::theme::TERM_ACCENT); // TERMCOLOR: the prompt in the accent colour
            }
            let (a, b) = (lo.max(pc), hi.min(pc + len));
            if a < b {
                pal.draw_text(m.margin + m.text_w(a - lo), y, self.current_input.get(a - pc..b - pc).unwrap_or(""), 0xFFFFFF);
            }
        }

        // TERMSEL — the selection as an INVERSE-VIDEO band (see `draw_band_span`, which splits it
        // across the wrapped rows). Here and nowhere else, so the full repaint and the per-keystroke
        // path (both call this) paint the same band. No witness: a repaint is not a state change.
        // TERMSEL2: `cols_on(EDIT_ROW, …)` — the editable line's part of ANY selection, one that
        // started in the scrollback included; for a selection wholly on this line it is `range`.
        if let Some((lo, hi)) = self.sel.cols_on(crate::video::termsel::EDIT_ROW, len, len) {
            self.draw_band_span(pal, prompt_y, cols, pc + lo, pc + hi, &self.current_input, pc);
        }

        // TERMSEL2 M3 — the CARET: a Mac-style insertion BAR at the caret's cell boundary, one font
        // stroke wide and one cell tall, in the theme's accent; hidden while this line shows a band.
        // TERMWRAP: the caret's flat cell is folded to (row, col) by `wrap_rc`, so it follows the wrap.
        if self.sel.cols_on(crate::video::termsel::EDIT_ROW, len, len).is_none() {
            let (cr, cc) = crate::video::termsel::wrap_rc(pc + self.sel.caret_col(len), cols);
            let cursor_x = m.margin + m.text_w(cc);
            pal.draw_rect(cursor_x, prompt_y + cr * m.line_h, m.scale.max(1), m.cell_h, crate::video::theme::ACCENT);
        }
    }

    pub fn draw(&self, pal: &mut TargetPal) {
        let m = pal.metrics();
        pal.clear_screen(Self::BG);
        GEOM_COLS.store(self.cols_for(m, pal.width() as usize), core::sync::atomic::Ordering::Relaxed); // TERMCOLOR M3 — TIOCGWINSZ's answer
        GEOM_ROWS.store(self.history_rows_for(m, pal.width() as usize, pal.height() as usize) + 1, core::sync::atomic::Ordering::Relaxed);

        // Show the newest lines that fit (scroll the oldest off the top when full), top-down.
        // TERMWRAP M2: an entry is `visual_rows(len, cols)` rows, drawn one `cols`-wide chunk per row.
        let (w, h) = (pal.width() as usize, pal.height() as usize);
        let cols = self.cols_for(m, w);
        let (skip, end, _) = self.layout_span(m, w, h);
        let mut y = self.top_y(pal);
        for (i, line) in self.history.iter().enumerate().skip(skip).take(end - skip) {
            let rows = crate::video::termsel::visual_rows(line.chars().count(), cols);
            let spans: &[crate::termcolor::Span] = self.attrs.get(i).map(|v| v.as_slice()).unwrap_or(&[]);
            for r in 0..rows {
                if spans.is_empty() {
                    pal.draw_text(m.margin, y + r * m.line_h, crate::video::termsel::row_slice(line, r, cols), crate::video::theme::TERM_FG);
                } else {
                    Self::draw_span_row(pal, m, y + r * m.line_h, line, spans, r, cols); // TERMCOLOR
                }
            }
            // TERMSEL2 — a selection's band on a SCROLLBACK row, the same inverse video the editable
            // line gets (`draw_prompt_line`), read from the same model (`LineSel::cols_on`).
            self.draw_row_band(pal, y, self.hist_base + i as u64, line);
            y += rows * m.line_h;
        }
        self.last_edit_rows.set(self.edit_rows_for(cols));

        // SCROLLBACK — scrolled up: no prompt (it is below the glass); the marker and the bar instead.
        if self.view_off > 0 {
            self.draw_scroll_furniture(pal, m, w, h, skip, end);
            return;
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
        // TERMWRAP: a change in the edit line's row count moves the history budget, so it owes the
        // full repaint; otherwise clear the whole (possibly multi-row) strip.
        let er = self.edit_rows_for(self.cols_for(m, pal.width() as usize));
        if er != self.last_edit_rows.get() || self.view_off > 0 {
            self.draw(pal);
            return;
        }
        let prompt_y = self.prompt_y(pal);
        // Clear the input-line strip (`er` line pitches) back to the background.
        pal.draw_rect(0, prompt_y, pal.width() as usize, er * m.line_h, Self::BG);
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
        let cols = self.cols_for(m, w);
        let (skip, end, vrows) = self.layout_span(m, w, h);
        let (lx, ly) = (lx.max(0) as usize, ly.max(0) as usize);
        // TERMWRAP M3 — fold the pixel into (visual_row, cell column) FIRST; the visual row then
        // picks the entry (each costs `visual_rows` bands) and the flat offset LineSel keeps.
        let vrow = ly.saturating_sub(top) / m.line_h;
        let px = core::cmp::min(lx.saturating_sub(m.margin) / m.cell_w, cols.saturating_sub(1));
        if vrow < vrows {
            let mut start = 0usize;
            for i in skip..end {
                let cells = self.history[i].chars().count();
                let cost = crate::video::termsel::visual_rows(cells, cols);
                if vrow < start + cost {
                    let col = core::cmp::min((vrow - start) * cols + px, cells);
                    return (self.hist_base + i as u64, col, cells);
                }
                start += cost;
            }
        }
        let pc = self.prompt_cells();
        let len = self.current_input.len();
        let er = core::cmp::min(vrow.saturating_sub(vrows), self.edit_rows_for(cols) - 1);
        let col = core::cmp::min((er * cols + px).saturating_sub(pc), len);
        (crate::video::termsel::EDIT_ROW, col, len)
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
            let cols = self.cols_for(m, pal.width() as usize);
            self.draw_band_span(pal, y, cols, lo, hi, line, 0);
        }
    }

    /// TERMSEL2 — what the terminal does with a resolved desktop ACTION, with its whole text in hand
    /// (`video::clipboard::terminal_action_in` gets the scrollback, which a pointer selection can
    /// reach), and the repaint that action owes paid on `pal`. Returns whether anything was painted,
    /// so the caller can mark its window dirty as it does for a keystroke.
    pub fn act(&mut self, a: crate::video::keymap::Action, pal: &mut TargetPal) -> bool {
        if let Some(painted) = self.scroll_action(a, pal) { return painted; } // SCROLLBACK — view motion, not an edit
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

// --- TERMWRAP — the view wraps at the window's column count ----------------------------------------
//
// Tail-appended. The model stays flat (a byte offset IS a cell column, `termsel.rs`); only the view
// folds: `cols_for` says how wide a row is, `termsel::visual_rows`/`wrap_rc`/`row_slice` fold a flat
// run of cells into rows, and the painter, the layout and the pointer all use these same helpers.
impl Console {
    /// Cells per visual row on a surface `w` wide (page margin each side; at least 1).
    pub fn cols_for(&self, m: crate::ui::Metrics, w: usize) -> usize {
        (w.saturating_sub(2 * m.margin) / m.cell_w).max(1)
    }

    /// Visual rows the edit line occupies: prompt + input + one cell for a caret at the end.
    pub fn edit_rows_for(&self, cols: usize) -> usize {
        crate::video::termsel::visual_rows(self.prompt_cells() + self.current_input.len() + 1, cols)
    }

    /// `(skip, vrows)`: the first history entry shown and the visual rows the shown entries cost.
    /// Walks back from the newest entry while the entries fit the row budget left after the edit
    /// line's extra rows.
    fn layout_for(&self, m: crate::ui::Metrics, w: usize, h: usize) -> (usize, usize) {
        let (skip, _, used) = self.layout_span(m, w, h);
        (skip, used)
    }

    /// SCROLLBACK — `(skip, end, vrows)`: the shown entries are `skip..end`. `end` is the tail minus `view_off`; the walk back stops at the clear mark in the LIVE view (`view_off == 0`) and at the front when scrolled.
    fn layout_span(&self, m: crate::ui::Metrics, w: usize, h: usize) -> (usize, usize, usize) {
        let cols = self.cols_for(m, w);
        let budget = self.history_rows_for(m, w, h).saturating_sub(self.edit_rows_for(cols) - 1);
        let end = self.history.len() - core::cmp::min(self.view_off, self.history.len());
        let floor = if self.view_off == 0 { core::cmp::min(self.clear_abs.saturating_sub(self.hist_base) as usize, end) } else { 0 };
        let (mut skip, mut used) = (end, 0usize);
        while skip > floor {
            let cost = crate::video::termsel::visual_rows(self.history[skip - 1].chars().count(), cols);
            if used + cost > budget {
                break;
            }
            used += cost;
            skip -= 1;
        }
        (skip, end, used)
    }

    /// Paint the inverse-video band over flat cells `lo..hi` of `line` (whose cell `i` is flat cell
    /// `base + i`), split across the wrapped rows it spans; `y0` is the first row's y.
    fn draw_band_span(&self, pal: &mut TargetPal, y0: usize, cols: usize, lo: usize, hi: usize, line: &str, base: usize) {
        if lo >= hi {
            return;
        }
        let m = pal.metrics();
        for r in (lo / cols)..=((hi - 1) / cols) {
            let (a, b) = (lo.max(r * cols), hi.min(r * cols + cols));
            let x = m.margin + m.text_w(a - r * cols);
            let y = y0 + r * m.line_h;
            pal.draw_rect(x, y, m.text_w(b - a), m.cell_h, 0xFFFFFF);
            let part: String = line.chars().skip(a - base).take(b - a).collect();
            pal.draw_text(x, y, &part, Self::BG);
        }
    }
}

// --- SCROLLBACK (R75) — the view's offset, the wheel/key routes, the marker and the bar ----------------
//
// Tail-appended. The model (`history`, absolute rows, `LineSel`) is unchanged: a selection records
// ABSOLUTE rows (`hist_base + i`), so it already survives the view moving; `layout_span`/`cell_at`/`draw`
// just all read the same `end = len - view_off`, which is what makes a click on a scrolled-back row
// resolve to that row's buffer number and not to the screen row it happens to occupy.
impl Console {
    /// Entries scrolled up (0 = live).
    pub fn view_off(&self) -> usize { self.view_off }
    /// Lines that arrived while scrolled up (the marker's N).
    pub fn new_lines(&self) -> usize { self.new_lines }
    /// Retained entries.
    pub fn history_len(&self) -> usize { self.history.len() }

    /// Largest offset: the one whose page is the oldest full page.
    fn max_off(&self, m: crate::ui::Metrics, w: usize, h: usize) -> usize {
        let cols = self.cols_for(m, w);
        let budget = self.history_rows_for(m, w, h).saturating_sub(self.edit_rows_for(cols) - 1);
        let (mut used, mut k) = (0usize, 0usize);
        for e in self.history.iter() {
            let c = crate::video::termsel::visual_rows(e.chars().count(), cols);
            if used + c > budget { break; }
            used += c;
            k += 1;
        }
        self.history.len().saturating_sub(k.max(1))
    }

    /// Move the view `delta` entries (positive = toward OLDER). Returns whether it moved.
    pub fn scroll_by_at(&mut self, delta: isize, m: crate::ui::Metrics, w: usize, h: usize) -> bool {
        let max = self.max_off(m, w, h) as isize;
        let old = self.view_off;
        let n = (old as isize + delta).clamp(0, max) as usize;
        self.view_off = n;
        if n == 0 { self.new_lines = 0; }
        self.last_edit_rows.set(usize::MAX);
        n != old
    }

    /// Jump to the oldest page (`top`) or back to the live bottom.
    pub fn scroll_to_at(&mut self, top: bool, m: crate::ui::Metrics, w: usize, h: usize) -> bool {
        let d = if top { self.history.len() as isize } else { -(self.view_off as isize) };
        self.scroll_by_at(d, m, w, h)
    }

    /// Entries in one PgUp/PgDn step: a page less one line of overlap.
    pub fn page_step_at(&self, m: crate::ui::Metrics, w: usize, h: usize) -> isize {
        let cols = self.cols_for(m, w);
        (self.history_rows_for(m, w, h).saturating_sub(self.edit_rows_for(cols) - 1).saturating_sub(1)).max(1) as isize
    }

    /// The buffer row (absolute number, text) at the top of the glass — the fixture's assertion.
    pub fn top_row_at(&self, m: crate::ui::Metrics, w: usize, h: usize) -> (u64, &str) {
        let (skip, end, _) = self.layout_span(m, w, h);
        if skip >= end { return (self.hist_base + skip as u64, ""); }
        (self.hist_base + skip as u64, self.history[skip].as_str())
    }

    /// A key that TYPES (printable, CR/LF, BS/DEL) snaps the view back to the live bottom and owes a full repaint (`last_edit_rows` poisoned so the caller's `draw_input_line` repaints everything). Called from `handle_key`'s head.
    pub fn snap_for_key(&mut self, c: u8) {
        if self.view_off > 0 && (c >= 0x20 || c == b'\n' || c == b'\r' || c == 8) {
            self.view_off = 0;
            self.new_lines = 0;
            self.last_edit_rows.set(usize::MAX);
        }
    }

    /// Wheel detents (positive = up, as `Event::Wheel`): 3 entries each. PLAIN wheel scrolls the shell window — it reaches this console only when no ring-3 window took it, so the pointer-focus rule already made it the shell's; Shift is not visible at the event, so Shift+wheel is not a separate route. Returns whether anything was painted.
    pub fn wheel(&mut self, d: i8, pal: &mut TargetPal) -> bool {
        let (m, w, h) = (pal.metrics(), pal.width() as usize, pal.height() as usize);
        if self.scroll_by_at(d as isize * 3, m, w, h) { self.draw(pal); true } else { false }
    }

    /// The four scroll ACTIONS (Shift+PgUp/PgDn, Cmd/Ctrl+Home/End). `None` = not a scroll action.
    pub fn scroll_action(&mut self, a: crate::video::keymap::Action, pal: &mut TargetPal) -> Option<bool> {
        use crate::video::keymap::Action;
        let (m, w, h) = (pal.metrics(), pal.width() as usize, pal.height() as usize);
        let moved = match a {
            Action::ScrollPageUp => { let s = self.page_step_at(m, w, h); self.scroll_by_at(s, m, w, h) }
            Action::ScrollPageDown => { let s = self.page_step_at(m, w, h); self.scroll_by_at(-s, m, w, h) }
            Action::ScrollTop => self.scroll_to_at(true, m, w, h),
            Action::ScrollBottom => self.scroll_to_at(false, m, w, h),
            _ => return None,
        };
        if moved { self.draw(pal); }
        Some(moved)
    }

    /// The `[N new lines]` marker (in the freed prompt row) and the 4 px bar at the right edge.
    fn draw_scroll_furniture(&self, pal: &mut TargetPal, m: crate::ui::Metrics, w: usize, h: usize, skip: usize, end: usize) {
        let top = self.top_y_for(m, w, h);
        let rows = self.history_rows_for(m, w, h);
        let page_h = rows * m.line_h;
        let total = self.history.len().max(1);
        let shown = (end - skip).max(1);
        let thumb_h = core::cmp::max(page_h * shown / total, 8).min(page_h);
        let span = total.saturating_sub(shown).max(1);
        let thumb_y = top + (page_h - thumb_h) * skip.min(span) / span;
        pal.draw_rect(w.saturating_sub(4), top, 4, page_h, 0x3A3868);
        pal.draw_rect(w.saturating_sub(4), thumb_y, 4, thumb_h, crate::video::theme::ACCENT);
        if self.new_lines > 0 {
            let y = top + rows * m.line_h;
            pal.draw_rect(0, y, w, m.line_h, 0x3A3868);
            let msg = format!("[{} new lines]", self.new_lines);
            pal.draw_text(m.margin, y, &msg, 0xFFFFFF);
        }
    }
}

/// SCROLLBACK fixture (panel-less console; `tests scrollback`): print 300 lines, scroll up 100 and
/// assert the top buffer row, print one more and assert the view did not move and the marker counts it,
/// resolve a pointer cell on the scrolled view to its BUFFER row (selection coordinates), snap back on a
/// typed key, then `clear` keeps and `clear --all` drops.
#[cfg(all(feature = "witness", target_arch = "x86_64"))]
pub fn scrollback_selftest() {
    const W: usize = 640;
    const H: usize = 480;
    let m = crate::ui::Metrics::for_height(H);
    let mut con = Console::new();
    con.mark_in_window();
    for i in 0..300 { con.place_for_fixture(&format!("line {:03}", i)); }
    let cols = con.cols_for(m, W);
    let (abs0, _) = con.top_row_at(m, W, H);
    let lx = (m.margin + m.cell_w / 2) as i32;
    let ly = (m.margin + m.cell_h / 2) as i32;
    let (live_row, _, _) = con.cell_at(m, W, H, lx, ly);
    let moved = con.scroll_by_at(100, m, W, H);
    let view_off = con.view_off();
    let (abs1, text1) = con.top_row_at(m, W, H);
    let top_ok = moved && view_off == 100 && abs1 + 100 == abs0 && text1 == format!("line {:03}", abs1);
    con.place_for_fixture("line 300");
    let (abs2, text2) = con.top_row_at(m, W, H);
    let marker_ok = con.new_lines() == 1 && con.view_off() == 101 && abs2 == abs1 && text2 == text1;
    let (srow, _, _) = con.cell_at(m, W, H, lx, ly);
    let sel_ok = srow == abs1 && live_row == abs0 && con.row_text(srow) == text1;
    con.snap_for_key(b'x');
    let snap_ok = con.view_off() == 0 && con.new_lines() == 0;
    let n0 = con.history_len();
    con.clear();
    let keep = con.history_len() == n0 && con.top_row_at(m, W, H).1.is_empty() && con.scroll_by_at(1, m, W, H);
    con.clear_all();
    let clear_ok = keep && snap_ok && con.history_len() == 0;
    let ok = top_ok && marker_ok && sel_ok && clear_ok;
    let t = |b: bool| if b { "ok" } else { "bad" };
    serial_println!(
        ":: SCROLLBACK: rows={} cols={} view_off={} marker_ok={} sel_ok={} clear_ok={} -> {} ::",
        Console::HISTORY_MAX, cols, view_off, t(marker_ok), t(sel_ok), t(clear_ok), if ok { "PASS" } else { "FAIL" }
    );
}

// --- TERMCOLOR (R75) — ingest, the span painter, the style seam, the geometry ------------------------
static GEOM_COLS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(80);
static GEOM_ROWS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(25);
/// The console's last painted geometry `(cols, rows)` — what a LINUXABI program's `TIOCGWINSZ` is told.
pub fn geometry() -> (usize, usize) {
    (GEOM_COLS.load(core::sync::atomic::Ordering::Relaxed).max(1), GEOM_ROWS.load(core::sync::atomic::Ordering::Relaxed).max(1))
}

/// Place one ingested line (escapes parsed) into the store; returns how many lines were appended (0 or 1).
fn push_parsed(
    history: &mut alloc::vec::Vec<String>, attrs: &mut alloc::vec::Vec<alloc::vec::Vec<crate::termcolor::Span>>,
    base: &mut u64, clear_abs: &mut u64, pen: &mut crate::termcolor::Attr, text: &str,
) -> usize {
    let (line, spans) = if crate::termcolor::needs_parse(text, pen) {
        let p = crate::termcolor::parse_line(pen, text);
        if p.cleared { *clear_abs = *base + history.len() as u64; } // ESC[2J: the live view starts below everything so far
        if p.cleared && p.text.is_empty() { return 0; }
        (p.text, p.spans)
    } else {
        (String::from(text), alloc::vec::Vec::new())
    };
    history.push(line);
    attrs.push(spans);
    while history.len() > Console::HISTORY_MAX {
        history.remove(0);
        attrs.remove(0);
        *base += 1;
    }
    1
}

impl Console {
    /// Paint visual row `r` of `line` as attribute runs (bg rect first, then the glyph run).
    fn draw_span_row(pal: &mut TargetPal, m: crate::ui::Metrics, y: usize, line: &str, spans: &[crate::termcolor::Span], r: usize, cols: usize) {
        use crate::termcolor as tc;
        let (lo, hi) = (r * cols, r * cols + cols);
        let mut cuts: alloc::vec::Vec<usize> = alloc::vec::Vec::with_capacity(spans.len() + 2);
        cuts.push(lo);
        for s in spans { let st = s.start as usize; if st > lo && st < hi { cuts.push(st); } }
        cuts.push(hi);
        for w in cuts.windows(2) {
            let (a, b) = (w[0], w[1]);
            let seg: String = line.chars().skip(a).take(b - a).collect();
            if seg.is_empty() { break; }
            let at = tc::attr_at(spans, a);
            let x = m.margin + m.text_w(a - lo);
            if at.bg & tc::SET != 0 {
                pal.draw_rect(x, y, m.text_w(seg.chars().count()), m.cell_h, at.bg & 0x00FF_FFFF);
            }
            let mut fg = tc::resolve(at.fg, if at.bold { 0x00FF_FFFF } else { crate::video::theme::TERM_FG });
            if at.bold && at.fg & tc::SET != 0 { fg = tc::lighten(fg); }
            pal.draw_text(x, y, &seg, fg);
        }
    }

    /// TERMCOLOR M2 — the SGR prefix for a palette pick (`theme::TERM_*`): `style(TERM_RED)` is `ESC[31m`.
    pub fn style(fg: u8) -> String { format!("\x1b[{}m", fg) }
    /// Print `text` in colour `fg` (one line; the pen is reset after it so the colour never leaks).
    pub fn println_styled(&mut self, fg: u8, text: &str) {
        let line = format!("{}{}\x1b[0m", Self::style(fg), text);
        self.println(&line);
    }
    /// Lines in the live view (those since the last `clear` / `ESC[2J`).
    pub fn live_rows(&self) -> usize { (self.hist_base + self.history.len() as u64).saturating_sub(self.clear_abs) as usize }
    /// Line `i`'s attribute spans (fixture / test read).
    pub fn spans_of(&self, i: usize) -> alloc::vec::Vec<crate::termcolor::Span> { self.attrs.get(i).cloned().unwrap_or_default() }
    pub fn hist_base_for_fixture(&self) -> u64 { self.hist_base }
}
