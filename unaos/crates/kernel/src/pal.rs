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

use crate::video::Screen;
use lazy_static::lazy_static;
use spin::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Timer,
    Key(u8),
    Mouse { x: i32, y: i32 },
    MouseAbsolute { x: i32, y: i32 },
    None,
    Unknown,
}

pub trait GneissPal {
    fn draw_pixel(&mut self, x: u32, y: u32, color: u32);
    fn poll_event(&mut self) -> Event;
    fn render(&mut self);

    fn width(&self) -> u32;
    fn height(&self) -> u32;

    fn clear_screen(&mut self, color: u32) {
        for y in 0..self.height() {
            for x in 0..self.width() {
                self.draw_pixel(x, y, color);
            }
        }
    }

    fn draw_rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: u32) {
        for row in 0..h {
            for col in 0..w {
                self.draw_pixel((x + col) as u32, (y + row) as u32, color);
            }
        }
    }

    /// Draw a line between two signed pixel points. Default walks Bresenham through `draw_pixel`;
    /// `TargetPal` overrides it to draw into the damage-tracked back buffer. The `vug` wireframe op.
    fn draw_line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, color: u32) {
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;
        let (mut x, mut y) = (x0, y0);
        loop {
            if x >= 0 && y >= 0 {
                self.draw_pixel(x as u32, y as u32, color);
            }
            if x == x1 && y == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
        }
    }

    /// Fill a triangle given three signed pixel points. Default is a scanline fill through
    /// `draw_line`; `TargetPal` overrides it with the back buffer's rasteriser. The `vug` facet op.
    fn fill_triangle(&mut self, a: (i32, i32), b: (i32, i32), c: (i32, i32), color: u32) {
        let mut p = [a, b, c];
        p.sort_unstable_by_key(|v| v.1);
        let (p0, p1, p2) = (p[0], p[1], p[2]);
        let edge_x = |q: (i32, i32), r: (i32, i32), y: i32| -> i32 {
            if r.1 == q.1 { q.0 } else { q.0 + (r.0 - q.0) * (y - q.1) / (r.1 - q.1) }
        };
        let mut y = p0.1;
        while y <= p2.1 {
            let xa = edge_x(p0, p2, y);
            let xb = if y < p1.1 { edge_x(p0, p1, y) } else { edge_x(p1, p2, y) };
            self.draw_line(xa, y, xb, y, color);
            y += 1;
        }
    }

    fn draw_text(&mut self, x: usize, y: usize, text: &str, color: u32) {
        let mut curr_x = x;
        for ch in text.chars() {
            if ch.is_ascii() {
                let glyph = font8x8::legacy::BASIC_LEGACY[ch as usize];
                for (row, byte) in glyph.iter().enumerate() {
                    for col in 0..8 {
                        if (byte & (1 << col)) != 0 {
                            self.draw_pixel((curr_x + col) as u32, (y + row) as u32, color);
                        }
                    }
                }
            }
            curr_x += 8;
        }
    }
}

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
    crate::arch::without_interrupts(|| {
        EVENT_QUEUE.lock().push(event);
    });
}

fn pop_event() -> Option<Event> {
    crate::arch::without_interrupts(|| EVENT_QUEUE.lock().pop())
}

/// Drain one queued input event without a GUI surface. Used by the `usbdebug` boot mode to print
/// live keypresses / mouse deltas straight to the framebuffer console (the normal path consumes
/// events through `TargetPal::poll_event`).
pub fn next_event() -> Option<Event> {
    pop_event()
}

/// Pump every polled input source into the event queue, then return the next queued event.
///
/// Interactive full-screen demos (e.g. `vug`) run their own animation loop *inside* a shell
/// command, so the outer console pump — which normally polls the controllers each frame — is
/// blocked. This lets such a loop keep input flowing: it polls the USB HID controller (x86 GUI +
/// the Orin panel both deliver keyboard/mouse through xHCI) and, on aarch64, drains the UART, then
/// returns one event. The bare-metal Pi routes keys through a separate scheduled input service /
/// channel rather than this queue, so that path is unaffected here.
pub fn pump_and_poll() -> Option<Event> {
    if let Some(x) = crate::drivers::xhci::XHCI_CONTROLLER.lock().as_mut() {
        x.poll_events();
    }
    #[cfg(target_arch = "aarch64")]
    while let Some(byte) = crate::arch::poll_input() {
        push_event(Event::Key(byte));
    }
    pop_event()
}

// --- PAL IMPLEMENTATION ---
pub struct TargetPal<'a> {
    pub surface: &'a mut Screen,
}

impl<'a> TargetPal<'a> {
    pub fn new(surface: &'a mut Screen) -> Self {
        Self { surface }
    }
}

impl<'a> GneissPal for TargetPal<'a> {
    fn draw_pixel(&mut self, x: u32, y: u32, color: u32) {
        self.surface.put_pixel(x as usize, y as usize, color);
    }

    // Override the trait defaults to use the surface's bulk ops: one back-buffer fill + one
    // damage union, instead of a per-pixel loop that unions a 1x1 rect a million times.
    fn clear_screen(&mut self, color: u32) {
        self.surface.fill_screen(color);
    }

    fn draw_rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: u32) {
        self.surface.fill_rect(x, y, w, h, color);
    }

    fn draw_line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, color: u32) {
        self.surface.draw_line(x0, y0, x1, y1, color);
    }

    fn fill_triangle(&mut self, a: (i32, i32), b: (i32, i32), c: (i32, i32), color: u32) {
        self.surface.fill_triangle(a, b, c, color);
    }

    fn poll_event(&mut self) -> Event {
        match pop_event() {
            Some(e) => e,
            None => Event::None,
        }
    }

    /// Present the frame: flush the damaged region of the back buffer to the framebuffer.
    fn render(&mut self) {
        self.surface.flush();
    }

    fn width(&self) -> u32 {
        self.surface.width() as u32
    }

    fn height(&self) -> u32 {
        self.surface.height() as u32
    }
}
