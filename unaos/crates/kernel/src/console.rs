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
use gneiss_pal::GneissPal;

pub struct Console {
    pub current_input: String,
    pub session: UserSession,
}

impl Console {
    pub fn new() -> Self {
        Self {
            current_input: String::new(),
            session: UserSession::new(),
        }
    }

    pub fn draw(&self, pal: &mut TargetPal) {
        pal.clear_screen(0x2D2B55); // Moonstone Background

        let line_height = 20;
        let mut y = 20;

        for line in &self.session.history {
            if y + line_height > pal.height() as usize {
                break;
            }
            pal.draw_text(20, y, line, 0xAAAAAA);
            y += line_height;
        }

        let prompt_y = pal.height() as usize - 40;
        if prompt_y > y {
            let prompt = format!("{}@unaos:~$ ", self.session.username);
            pal.draw_text(20, prompt_y, &prompt, 0x00FF00); // Green Prompt

            let input_x = 20 + (prompt.len() * 8);
            pal.draw_text(input_x, prompt_y, &self.current_input, 0xFFFFFF);

            let cursor_x = input_x + (self.current_input.len() * 8);
            pal.draw_rect(cursor_x, prompt_y, 8, 16, 0xFFFFFF);
        }
    }
}
