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
}

impl Console {
    pub fn new() -> Self {
        Self {
            current_input: String::new(),
            session: UserSession::new(),
            history: alloc::vec::Vec::new(),
        }
    }

    pub fn clear(&mut self) {
        self.history.clear();
    }

    pub fn println(&mut self, text: &str) {
        self.history.push(String::from(text));
        if self.history.len() > 25 {
            self.history.remove(0);
        }
    }

    // Layout constants shared by the full repaint and the per-keystroke fast path so the prompt sits
    // in the same place in both. Top-down terminal fill: history starts at the top and each new line
    // pushes the prompt DOWN; once the screen is full the oldest lines scroll off the top.
    const TOP: usize = 20;
    const LINE_H: usize = 20;

    /// Rows of history shown above the prompt: everything that fits from `TOP` down, reserving the
    /// last row for the prompt/input line itself.
    fn history_rows(&self, pal: &TargetPal) -> usize {
        let usable = (pal.height() as usize).saturating_sub(Self::TOP) / Self::LINE_H;
        usable.saturating_sub(1)
    }

    /// The y of the prompt/input line: directly below the last shown history line (so on a fresh
    /// screen the prompt sits at the top and walks down as output arrives; once full it pins to the
    /// last usable row because the history is scrolled).
    fn prompt_y(&self, pal: &TargetPal) -> usize {
        let rows = self.history_rows(pal);
        let shown = self.history.len().min(rows);
        Self::TOP + shown * Self::LINE_H
    }

    pub fn draw(&self, pal: &mut TargetPal) {
        pal.clear_screen(0x2D2B55); // Moonstone Background

        // Show the last `history_rows` lines (scroll the oldest off the top when full), top-down.
        let rows = self.history_rows(pal);
        let skip = self.history.len().saturating_sub(rows);
        let mut y = Self::TOP;
        for line in self.history.iter().skip(skip) {
            pal.draw_text(20, y, line, 0xAAAAAA);
            y += Self::LINE_H;
        }

        // Prompt directly below the last output line.
        let prompt_y = y;
        let prompt = format!("{}@unaos:~$ ", self.session.username);
        pal.draw_text(20, prompt_y, &prompt, 0x00FF00); // Green Prompt

        let input_x = 20 + (prompt.len() * 8);
        pal.draw_text(input_x, prompt_y, &self.current_input, 0xFFFFFF);

        let cursor_x = input_x + (self.current_input.len() * 8);
        pal.draw_rect(cursor_x, prompt_y, 8, 16, 0xFFFFFF);
    }

    /// Repaint ONLY the prompt/input line. This is the per-keystroke path: typing changes just
    /// that line, so we clear its strip and redraw it instead of repainting the whole screen.
    /// With damage tracking that means each keystroke flushes ~one text row, not the full frame —
    /// the difference between snappy and unusable at native resolution. Use `draw()` for the full
    /// repaint after command output (history changes).
    pub fn draw_input_line(&self, pal: &mut TargetPal) {
        let prompt_y = self.prompt_y(pal);
        // Clear the input-line strip (cursor cells are 16px tall) back to the background.
        pal.draw_rect(0, prompt_y, pal.width() as usize, 16, 0x2D2B55);

        let prompt = format!("{}@unaos:~$ ", self.session.username);
        pal.draw_text(20, prompt_y, &prompt, 0x00FF00);

        let input_x = 20 + (prompt.len() * 8);
        pal.draw_text(input_x, prompt_y, &self.current_input, 0xFFFFFF);

        let cursor_x = input_x + (self.current_input.len() * 8);
        pal.draw_rect(cursor_x, prompt_y, 8, 16, 0xFFFFFF);
    }
}
