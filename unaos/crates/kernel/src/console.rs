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
}

impl Console {
    pub fn new() -> Self {
        Self {
            current_input: String::new(),
            session: UserSession::new(),
            history: alloc::vec::Vec::new(),
            out_sink: None,
        }
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
        self.history.clear();
    }

    pub fn println(&mut self, text: &str) {
        self.history.push(String::from(text));
        // Retain enough scrollback to fill the tallest panels we run on (native 4K ~= 90 rows at
        // the scale-2 line pitch). Bounded so the buffer can't grow without limit; large enough that
        // a full screen is always drawable (the old 25-line cap starved the bottom third at native
        // resolution).
        if self.history.len() > Self::HISTORY_MAX {
            self.history.remove(0);
        }
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

    /// Retained scrollback cap — generous enough that even a 4K panel's worth of rows is drawable.
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
        let usable = (pal.height() as usize).saturating_sub(m.margin) / m.line_h;
        usable.saturating_sub(1).max(6)
    }

    /// Rows of history shown above the prompt: everything that fits from `TOP` down, reserving the
    /// last row for the prompt/input line itself.
    fn history_rows(&self, pal: &TargetPal) -> usize {
        Self::page_rows(pal)
    }

    /// The y of the prompt/input line: directly below the last shown history line (so on a fresh
    /// screen the prompt sits at the top and walks down as output arrives; once full it pins to the
    /// last usable row because the history is scrolled).
    fn prompt_y(&self, pal: &TargetPal) -> usize {
        let m = pal.metrics();
        let rows = self.history_rows(pal);
        let shown = self.history.len().min(rows);
        m.margin + shown * m.line_h
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
        let mut y = m.margin;
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
