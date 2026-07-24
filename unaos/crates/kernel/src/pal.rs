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
    /// HID-KEYS: a key-RELEASE edge (payload = the ASCII the matching `Key` press carried).
    /// The HID decoders track each keyboard slot's previous boot report and emit this when a
    /// keycode leaves the pressed set. Consumers that only act on presses ignore it (they match
    /// `Key` and fall through the wildcard); held-state consumers can pair it with `Key`.
    KeyUp(u8),
    Mouse { x: i32, y: i32 },
    MouseAbsolute { x: i32, y: i32 },
    /// CLICK-1: a pointer button-DOWN edge (payload = the report's button bitmask, bit 0 = primary).
    /// Emitted once per press by the HID decoders (edge-detected there); release emits nothing.
    Button(u8),
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
    use core::sync::atomic::{AtomicU64, Ordering};
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

    // CURSOR-HIDE (metal verdict, 2026-07-18): the cursor auto-hides after ~1.5 s without pointer
    // input and reappears instantly on the next report (`move_rel`/`set_abs` stamp the activity
    // clock, and every draw site runs AFTER the frame's input drain). It also starts hidden — no
    // arrow parked mid-screen on a keyboard-only session. `draw()` self-gates on `visible()`, so
    // every existing call site inherits the behaviour without changes.
    /// Auto-hide delay in ms without pointer input.
    const HIDE_AFTER_MS: u64 = 1500;
    /// ms() timestamp of the last pointer report; 0 = never (cursor starts hidden).
    static LAST_INPUT_MS: AtomicU64 = AtomicU64::new(0);

    /// Stamp the pointer-activity clock (a real report just arrived).
    fn touch() {
        // `.max(1)` keeps 0 reserved as the "never" sentinel even if ms() is still 0 at boot.
        LAST_INPUT_MS.store(crate::arch::ms().max(1), Ordering::Relaxed);
    }

    /// Whether the cursor should currently be on screen: some pointer input has ever arrived and
    /// the last report is younger than the auto-hide delay.
    pub fn visible() -> bool {
        let t = LAST_INPUT_MS.load(Ordering::Relaxed);
        t != 0 && crate::arch::ms().wrapping_sub(t) < HIDE_AFTER_MS
    }

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
        touch();
        let (x, y) = pos(w, h);
        set_clamped(x + dx, y + dy, w, h);
    }

    /// Apply an absolute report in the 0..=32767 HID coordinate space, scaled to the panel.
    pub fn set_abs(ax: i32, ay: i32, w: i32, h: i32) {
        touch();
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
    /// the white fill — visible over dark and light content alike. No-op while auto-hidden
    /// (CURSOR-HIDE): call sites need no gating of their own.
    ///
    /// Cost note (metal verdict): each glyph row draws as merged horizontal RUNS (≤2 rects/row)
    /// rather than one rect per set bit — the sprite is a handful of small back-buffer fills per
    /// frame, and inside the full-screen demos (which clear + full-flush every frame anyway) it
    /// adds no flush area at all. The vug slowdown observed alongside CURSOR-VIS was NOT this
    /// sprite: it was the SMC battery sweep the same commit hung on the meter cadence, which on a
    /// non-answering SMC spun multi-second bounded handshake timeouts every second (fixed in
    /// drivers/smc.rs: stuck-sweep early-abort + failure backoff).
    pub fn draw(pal: &mut impl GneissPal) {
        if !visible() {
            return;
        }
        let (x, y) = pos(pal.width() as i32, pal.height() as i32);
        let s = sprite_scale(pal);
        for &(ox, oy, color) in &[(s, s, SHADOW), (0, 0, FILL)] {
            for (row, bits) in ARROW.iter().enumerate() {
                let mut col = 0usize;
                while col < 8 {
                    if bits & (0x80 >> col) != 0 {
                        let mut run = 1usize;
                        while col + run < 8 && bits & (0x80 >> (col + run)) != 0 {
                            run += 1;
                        }
                        pal.draw_rect(
                            (x as usize) + col * s + ox,
                            (y as usize) + row * s + oy,
                            run * s,
                            s,
                            color,
                        );
                        col += run;
                    } else {
                        col += 1;
                    }
                }
            }
        }
    }
}

// --- EVENT QUEUE ---
const QUEUE_SIZE: usize = 64;
/// UVUG-6 — public mirror of the ring capacity so the QEMU typematic selftest can size its fill relative to
/// the real backpressure threshold (`QUEUE_SIZE / 2`) instead of a magic number.
pub const QUEUE_SIZE_PUB: usize = QUEUE_SIZE;

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
    /// UVUG-6 — current occupancy (0..QUEUE_SIZE-1). `head - tail` modulo the ring size; correct across wrap
    /// because 2^bits ≡ 0 (mod QUEUE_SIZE) makes the wrapping subtraction agree with the true difference.
    fn len(&self) -> usize {
        self.head.wrapping_sub(self.tail) % QUEUE_SIZE
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

/// UVUG-6 — live EVENT_QUEUE occupancy, read by the host-side typematic backpressure guard so a synthesised
/// key repeat is never injected while the ring is past half full (a stuck/phantom repeat must not be able to
/// starve real HID edges — the self-sustaining wedge symptom).
pub fn event_queue_depth() -> usize {
    crate::arch::without_interrupts(|| EVENT_QUEUE.lock().len())
}

/// UVUG-5 — monotonically bumped every time a HID keyboard slot is torn down (a detach / disconnect /
/// enumeration-recovery teardown), read by the host-side typematic tracker so it can drop a held key that
/// will NEVER see its `Event::KeyUp`. A boot keyboard under `SET_IDLE(0)` sends one press and no further
/// reports until release, so if the device is UNPLUGGED mid-hold there is no release edge — without this the
/// typematic synthesiser would inject `Event::Key` forever at the repeat rate. The xHCI teardown chokepoint
/// (`Slot::reset_soft_state`) bumps this for any slot that was a keyboard; the consumer clears its state when
/// the generation advances. Arch-neutral (a plain counter); harmless on targets with no typematic consumer.
static KEYBOARD_DETACH_GEN: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Record that a HID keyboard slot was torn down (see [`KEYBOARD_DETACH_GEN`]). Called from the xHCI slot
/// teardown path; idempotent and lock-free, safe from any context.
pub fn note_keyboard_detached() {
    KEYBOARD_DETACH_GEN.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
}

/// The current keyboard-detach generation. The host-side typematic tracker compares this against the value it
/// last observed and, on a change, drops any held key (whose `KeyUp` a detach guarantees will never arrive).
pub fn keyboard_detach_gen() -> u64 {
    KEYBOARD_DETACH_GEN.load(core::sync::atomic::Ordering::Relaxed)
}

// --- UVUG-6: host-side typematic key-repeat tracker (HID-report-level) ---
//
// UVUG-5 synthesised a held key's repeat by tracking Key/KeyUp edges as they were DRAINED out of EVENT_QUEUE.
// That was the hole behind the P51 boot wedge: `EventQueue::push` silently DROPS on a full ring, so a `KeyUp`
// pushed while the 64-slot queue was saturated was never enqueued — hence never drained, never observed. The
// tracker then held the key forever and injected `Event::Key` every RATE_MS, which kept the queue full, which
// dropped every subsequent real edge (including new keys and their releases): a self-sustaining wedge that
// exactly matches the capture (keyboard events stop, no detach, key repeat broken).
//
// UVUG-6 moves the observation to the HID REPORT level, BEFORE any EVENT_QUEUE push. The xHCI HID decode calls
// `typematic_note_report` once per keyboard report with the newest press and the FULL currently-held ascii set.
// A release is now learned by the armed key being ABSENT from the latest report's held set — a fact that can
// never be dropped by the queue. Three independent safety layers close every remaining miss class:
//   1. report-level release: armed key not in `held` -> disarm (covers a dropped/missed KeyUp on the queue);
//   2. keyboard-detach generation (UVUG-5): an unplug mid-hold never sends a release report -> disarm;
//   3. positive liveness: no HID report from the keyboard for ~1 s while a key is still "held" -> disarm
//      (covers a release report that never reached the decode at all, or a wedged endpoint).
// Plus a backpressure guard: `typematic_tick` refuses to inject while EVENT_QUEUE is past half full, so even a
// momentarily-stuck repeat can never saturate the ring and starve real input.
//
// State lives HERE (the kernel lib) rather than in the `main` binary so the xHCI decode — which is in the lib
// and cannot call into `main` — can feed it directly at the report level. `main`'s pump calls `typematic_tick`.
//
// LIVENESS/SET_IDLE(0) NOTE: on a keyboard that truly sends ZERO reports while a key is held still (strict
// SET_IDLE(0), no idle re-reports), the 1 s liveness will stop an ongoing repeat while the key is physically
// held. That is a benign, self-correcting degradation (re-press resumes it) and is deliberately preferred over
// the catastrophic, self-sustaining wedge it guards against. The P51-class hardware streams periodic reports,
// so liveness there simply confirms the keyboard is alive.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
mod typematic {
    use core::sync::atomic::{AtomicU32, AtomicU64};
    /// ASCII of the key eligible to repeat, +1 (0 = none). Newest press wins.
    pub(super) static KEY_P1: AtomicU32 = AtomicU32::new(0);
    /// `ms()` at which the next repeat is due.
    pub(super) static NEXT_MS: AtomicU64 = AtomicU64::new(0);
    /// `ms()` of the most recent HID keyboard report seen (0 = none yet) — the liveness signal.
    pub(super) static LAST_REPORT_MS: AtomicU64 = AtomicU64::new(0);
    /// Last `keyboard_detach_gen()` folded by `typematic_tick`; an advance means an unplug -> drop held key.
    pub(super) static SEEN_DETACH_GEN: AtomicU64 = AtomicU64::new(0);
    /// Hold time before the FIRST synthesised repeat (~desktop feel; a tap never repeats).
    pub(super) const DELAY_MS: u64 = 400;
    /// Repeat period once repeating (~25 chars/s).
    pub(super) const RATE_MS: u64 = 40;
    /// Liveness window: a "held" key with no HID report for this long is stale -> drop.
    pub(super) const LIVENESS_MS: u64 = 1000;
}

/// UVUG-6 — feed the typematic tracker one HID keyboard report at the REPORT LEVEL (before any EVENT_QUEUE
/// push). `newest_press` is an ascii that went down THIS report (0 = none); `held` is every ascii currently
/// down. Observing releases here (armed key absent from `held`) rather than from the drained event stream is
/// what closes the UVUG-5 dropped-`KeyUp` hole. A synthesised repeat never comes through here (it is not a HID
/// report), so the initial delay is honoured exactly once per physical press.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
pub fn typematic_note_report(newest_press: u8, held: &[u8]) {
    use core::sync::atomic::Ordering;
    let now = crate::arch::ms();
    typematic::LAST_REPORT_MS.store(now.max(1), Ordering::Relaxed);
    // Report-level release: if the currently-armed key is not in this report's held set, it was released
    // (or was never really held) — disarm. This is the primary, queue-independent release path.
    let kp1 = typematic::KEY_P1.load(Ordering::Relaxed);
    if kp1 != 0 {
        let k = (kp1 - 1) as u8;
        if !held.contains(&k) {
            typematic::KEY_P1.store(0, Ordering::Relaxed);
        }
    }
    // Arm the newest press (newest-wins typematic) and (re)start the initial delay.
    if newest_press != 0 {
        typematic::KEY_P1.store(newest_press as u32 + 1, Ordering::Relaxed);
        typematic::NEXT_MS.store(now.wrapping_add(typematic::DELAY_MS), Ordering::Relaxed);
    }
}

/// UVUG-6 — if a held key's repeat is due, return its ascii and schedule the next one. Returns `None` when no
/// key is held, the repeat is not yet due, the keyboard detached, liveness lapsed, or EVENT_QUEUE is past half
/// full. Called once per USB pump pass, BEFORE the drain, by the host pump in `main`.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
pub fn typematic_tick() -> Option<u8> {
    use core::sync::atomic::Ordering;
    // (2) detach guard: a keyboard unplugged mid-hold never sends its release report.
    let dg = keyboard_detach_gen();
    if typematic::SEEN_DETACH_GEN.swap(dg, Ordering::Relaxed) != dg {
        typematic::KEY_P1.store(0, Ordering::Relaxed);
        return None;
    }
    let kp1 = typematic::KEY_P1.load(Ordering::Relaxed);
    if kp1 == 0 {
        return None;
    }
    let now = crate::arch::ms();
    // (3) positive liveness: a still-"held" key whose keyboard has gone silent for ~1 s is stale -> drop it.
    let last = typematic::LAST_REPORT_MS.load(Ordering::Relaxed);
    if last != 0 && now.wrapping_sub(last) > typematic::LIVENESS_MS && now >= last {
        typematic::KEY_P1.store(0, Ordering::Relaxed);
        return None;
    }
    // backpressure guard: never inject while the ring is past half full (starvation / wedge guard).
    if event_queue_depth() > QUEUE_SIZE / 2 {
        return None;
    }
    let due = typematic::NEXT_MS.load(Ordering::Relaxed);
    // `ms()` is monotonic; guard the wrap window so a rolled clock cannot spew repeats.
    if now.wrapping_sub(due) < (1u64 << 62) && now >= due {
        typematic::NEXT_MS.store(now.wrapping_add(typematic::RATE_MS), Ordering::Relaxed);
        return Some((kp1 - 1) as u8);
    }
    None
}

/// UVUG-6 test aid — force the next repeat "due" now, so the QEMU selftest can exercise the inject/suppress
/// decision deterministically without waiting out the real delay. Not used on any boot path.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
pub fn typematic_test_force_due() {
    use core::sync::atomic::Ordering;
    typematic::NEXT_MS.store(crate::arch::ms(), Ordering::Relaxed);
    // keep liveness satisfied so the force-due path exercises the backpressure/arm logic, not the liveness drop
    typematic::LAST_REPORT_MS.store(crate::arch::ms().max(1), Ordering::Relaxed);
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
    // GUI-WIRE: liveness heartbeat for the app-input watchdog — the active full-screen app's drain
    // loop proves it is still making progress once per pass. No-op when no app owns the screen.
    crate::gui_watchdog::note_progress();
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

    /// VUG-FPS bandwidth witness: bytes the last `render()` (flush) copied to the framebuffer.
    pub fn last_flush_bytes(&self) -> u64 {
        self.surface.last_flush_bytes()
    }

    /// VUG-PAR witness: parallel bands the last `render()` (flush) used (1 = serial / feature-off).
    pub fn last_flush_bands(&self) -> usize {
        self.surface.last_flush_bands()
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
