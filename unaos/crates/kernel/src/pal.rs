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

    /// UI-1: the scale-derived UI geometry for this surface (THE METRICS RULE — no absolute
    /// pixel sizes in UI code; everything derives from these). A pure function of the panel
    /// height, so deriving per call is cheap and can never go stale.
    fn metrics(&self) -> crate::ui::Metrics {
        crate::ui::Metrics::for_height(self.height() as usize)
    }

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

    /// Draw `text` with the 8×8 base font, scale-aware (UI-1): each glyph pixel renders as a
    /// `scale`×`scale` block and the advance is one metrics cell, so text set at any panel
    /// size keeps its proportions. At scale 1 this is the classic per-pixel path, unchanged.
    fn draw_text(&mut self, x: usize, y: usize, text: &str, color: u32) {
        let m = self.metrics();
        let s = m.scale;
        let mut curr_x = x;
        for ch in text.chars() {
            if ch.is_ascii() {
                let glyph = font8x8::legacy::BASIC_LEGACY[ch as usize];
                for (row, byte) in glyph.iter().enumerate() {
                    for col in 0..crate::ui::BASE_CELL {
                        if (byte & (1 << col)) != 0 {
                            if s == 1 {
                                self.draw_pixel((curr_x + col) as u32, (y + row) as u32, color);
                            } else {
                                self.draw_rect(curr_x + col * s, y + row * s, s, s, color);
                            }
                        }
                    }
                }
            }
            curr_x += m.cell_w;
        }
    }
}

// --- CURSOR ---------------------------------------------------------------------------------
//
// CURSOR-VIS (metal defect fix): the one shared mouse-cursor sprite + position. Before this, the
// GUI console loop drew a bare 10×10 unscaled square and the full-screen views (vug crystal /
// pulse) drew NOTHING — they clear the frame every iteration and consumed mouse events without a
// sprite, so on metal the trackpad decoded (serial MOUSE dx/dy) while no cursor was ever visible.
// Now every screen-owning loop shares this position and draws the same metrics-scaled arrow.
pub mod cursor {
    use super::GneissPal;
    use spin::Mutex;

    /// 8×8 arrow mask, MSB = leftmost pixel.
    const ARROW: [u8; 8] = [
        0b1000_0000,
        0b1100_0000,
        0b1110_0000,
        0b1111_0000,
        0b1111_1000,
        0b1111_1100,
        0b1101_1000,
        0b1000_1100,
    ];
    const FILL: u32 = 0x00FF_FFFF; // white arrow
    const SHADOW: u32 = 0x0010_1014; // near-black drop shadow — visible on any light content

    /// Hot-spot position (arrow tip), lazily centred on first use. One shared position: the
    /// console loop and the full-screen demos all move/draw the same cursor.
    static POS: Mutex<Option<(i32, i32)>> = Mutex::new(None);

    /// Sprite magnification: one step above the text scale so the cursor reads at a glance
    /// (16 px at the 480p QEMU panel, 24 px on the 2880×1800 Retina).
    fn sprite_scale(pal: &impl GneissPal) -> usize {
        pal.metrics().scale + 1
    }

    /// The cursor's bounding square in pixels (for erase).
    pub fn extent(pal: &impl GneissPal) -> usize {
        8 * sprite_scale(pal)
    }

    /// Current position, centring on first use.
    pub fn pos(w: i32, h: i32) -> (i32, i32) {
        *POS.lock().get_or_insert((w / 2, h / 2))
    }

    /// Apply a relative motion report (trackpad/mouse dx,dy), clamped to the panel.
    pub fn move_rel(dx: i32, dy: i32, w: i32, h: i32) {
        let (x, y) = pos(w, h);
        set_clamped(x + dx, y + dy, w, h);
    }

    /// Apply an absolute report in the 0..=32767 HID coordinate space, scaled to the panel.
    pub fn set_abs(ax: i32, ay: i32, w: i32, h: i32) {
        let x = ((ax as i64 * w as i64) / 32767) as i32;
        let y = ((ay as i64 * h as i64) / 32767) as i32;
        set_clamped(x, y, w, h);
    }

    fn set_clamped(x: i32, y: i32, w: i32, h: i32) {
        let cx = x.clamp(0, (w - 1).max(0));
        let cy = y.clamp(0, (h - 1).max(0));
        *POS.lock() = Some((cx, cy));
    }

    /// Paint `color` over the sprite's bounding box (the console loop's erase-before-move; the
    /// full-screen demos clear every frame and never need it).
    pub fn erase(pal: &mut impl GneissPal, color: u32) {
        let (x, y) = pos(pal.width() as i32, pal.height() as i32);
        let e = extent(pal);
        pal.draw_rect(x as usize, y as usize, e, e, color);
    }

    /// Draw the arrow sprite at the current position: a one-block-offset drop shadow first, then
    /// the white fill — visible over dark and light content alike.
    pub fn draw(pal: &mut impl GneissPal) {
        let (x, y) = pos(pal.width() as i32, pal.height() as i32);
        let s = sprite_scale(pal);
        for &(ox, oy, color) in &[(s, s, SHADOW), (0, 0, FILL)] {
            for (row, bits) in ARROW.iter().enumerate() {
                for col in 0..8 {
                    if bits & (0x80 >> col) != 0 {
                        pal.draw_rect(
                            (x as usize) + col * s + ox,
                            (y as usize) + row * s + oy,
                            s,
                            s,
                            color,
                        );
                    }
                }
            }
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
    // RMBP-FIX M3 (x86 EHCI): the internal rMBP keyboard/trackpad ride the EHCI HID path, not xHCI.
    // The outer console loop services them beside the xHCI hooks, but a full-screen demo (vug/pulse)
    // runs its OWN loop inside a shell command and blocks that service — so poll the EHCI HID
    // endpoints HERE too (same call the main loops make), or the built-in keyboard can never post the
    // keystroke that exits the demo. Harmless no-op in QEMU (xHCI-only): with no EHCI HID controller
    // armed the service returns immediately. Same feature gate as the main-loop call sites.
    #[cfg(all(target_arch = "x86_64", feature = "ehcihid"))]
    crate::drivers::ehci::service_ehci_hid();
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
        // UI-1 evidence: announce the derived metrics once per surface bring-up so headless
        // gates can verify the scale layer on every target (x86 GUI, arm virt, Pi render
        // service, Orin panel) without a screen.
        let m = crate::ui::Metrics::for_height(surface.height());
        serial_println!(
            ":: UI1: scale={} cell={}x{} line={} ::",
            m.scale,
            m.cell_w,
            m.cell_h,
            m.line_h
        );
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
