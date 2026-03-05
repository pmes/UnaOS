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

use crate::writer::Writer;
use gneiss_pal::{Event, GneissPal};
use lazy_static::lazy_static;
use spin::Mutex;

// --- EVENT QUEUE ---
const QUEUE_SIZE: usize = 64;

struct EventQueue {
    buffer: [Event; QUEUE_SIZE],
    head: usize,
    tail: usize,
}

impl EventQueue {
    const fn new() -> Self {
        Self {
            buffer: [Event::None; QUEUE_SIZE],
            head: 0,
            tail: 0,
        }
    }
    fn push(&mut self, event: Event) {
        let next = (self.head + 1) % QUEUE_SIZE;
        if next != self.tail {
            self.buffer[self.head] = event;
            self.head = next;
        }
    }
    fn pop(&mut self) -> Option<Event> {
        if self.head == self.tail {
            None
        } else {
            let event = self.buffer[self.tail];
            self.tail = (self.tail + 1) % QUEUE_SIZE;
            Some(event)
        }
    }
}

lazy_static! {
    static ref EVENT_QUEUE: Mutex<EventQueue> = Mutex::new(EventQueue::new());
}

pub fn push_event(event: Event) {
    x86_64::instructions::interrupts::without_interrupts(|| {
        EVENT_QUEUE.lock().push(event);
    });
}

fn pop_event() -> Option<Event> {
    x86_64::instructions::interrupts::without_interrupts(|| EVENT_QUEUE.lock().pop())
}

// --- PAL IMPLEMENTATION ---
pub struct TargetPal<'a> {
    pub writer: &'a mut Writer,
}

impl<'a> TargetPal<'a> {
    // NEW: The constructor main.rs was looking for
    pub fn new(writer: &'a mut Writer) -> Self {
        Self { writer }
    }
}

impl<'a> GneissPal for TargetPal<'a> {
    fn draw_pixel(&mut self, x: u32, y: u32, color: u32) {
        self.writer.write_pixel(x as usize, y as usize, color);
    }

    fn poll_event(&mut self) -> Event {
        match pop_event() {
            Some(e) => e,
            None => Event::None,
        }
    }

    fn render(&mut self) {}

    fn width(&self) -> u32 {
        self.writer.width() as u32
    }

    fn height(&self) -> u32 {
        self.writer.height() as u32
    }
}
