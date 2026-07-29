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

    /// FOCUS-VIS — whether a REAL pointer report has ever arrived this boot (the `visible()` predicate
    /// without the auto-hide half). Distinct from `visible()` because it answers a different question:
    /// "does this machine have a working pointer?", not "should the arrow be on screen right now?".
    ///
    /// It was introduced so the EL0 input router could keep the sprite alive while an app holds focus
    /// WITHOUT arming a cursor that never existed (QEMU raspi4b delivers no HID pointer, but the
    /// boot-time `input_router_selftest` pushes a synthetic `Event::Mouse` through the real router).
    ///
    /// EL0IN-FOCUS retired that use: this latch can only ever be SET by the very code it was gating, so
    /// on a boot where an EL0 app took focus before the first real pointer report it stayed false
    /// forever and the cursor was dead until the operator TAB'd back to the shell. The router now
    /// scopes the guard to the selftest's own call instead (`ROUTER_SELFTEST` in `main.rs`). Kept as the
    /// honest "does this machine have a working pointer?" predicate, distinct from `visible()`.
    pub fn has_reported() -> bool {
        LAST_INPUT_MS.load(Ordering::Relaxed) != 0
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
    /// Returns `true` if the event was stored, `false` if the ring was full and it was dropped.
    /// UVUG-10 — the drop verdict used to be swallowed here; it is now reported so `push_event`
    /// can account for it (see `EVQ_*`).
    fn push(&mut self, event: Event) -> bool {
        let next = (self.head + 1) % QUEUE_SIZE;
        if next != self.tail {
            self.buffer[self.head] = event;
            self.head = next;
            true
        } else {
            false
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

// --- UVUG-10: EVENT_QUEUE PRODUCER/CONSUMER ACCOUNTING ---
//
// P55b read `[uvug9] shell-path input key=<climbing> ptr=0` on metal for the whole boot, while the xHCI
// `MOUSE-1` witness reported a live pointer with real deltas.
//
// THE LOSS IS AT OR AFTER THIS QUEUE — that much is now settled, not suspected. `push_event(Event::Mouse)`
// (drivers/xhci/mod.rs:2278/2286) precedes the `MOUSE-1` print in straight-line code with no platform fork
// between them, so P55b's `last dx=3 dy=5` is direct proof that pointer events WERE pushed. The earlier
// all-zero-report-buffer theory is refuted by that same fact.
//
// LEADING THEORY (unified, and the one UVUG-10's fixture gate already kills): the boot `input_launcher`
// fixture's orphan held `el0_input_active()` for the entire boot, so the router's EL0 branch
// (`route_input_to_active_el0`, main.rs) swallowed the whole queue into a ring nothing would ever read —
// while KEYS still reached the shell, because `input_service`'s UART path calls `gui_send` DIRECTLY and
// never touches EVENT_QUEUE at all. That single mechanism explains "ptr=0 but key climbs", explains "from
// boot", and requires no defect anywhere else. If it is right, gating the fixture off metal is also the
// mouse fix.
//
// These counters exist to prove or refute that in ONE boot rather than by argument. They sit at the one
// choke point every producer must pass, are classified (pointer vs key), and count DROPS separately:
// `EventQueue::push` silently discards on a full ring, and pointer traffic (~125 reports/s from a moving
// mouse) outnumbers keystrokes by two orders of magnitude, so a stalled drain starves the very class under
// investigation — invisibly, before this arc. Re-circulated events are deliberately EXCLUDED (see
// `requeue_event`). Plain relaxed atomics, no lock, no cfg gate: arch-neutral, one `fetch_add` on a path
// that already takes a spinlock.
//
// BASELINE — the boot selftests are producers too. `main::input_router_selftest` pushes a synthetic
// `Mouse{3,-4}` plus two `Key`s, and the typematic/uvug6 selftests push more keys, all before any HID
// traffic exists. So "nothing was ever produced" reads as **`push ptr=1`**, NOT `push ptr=0`; the QEMU
// battery's own line is `push ptr=1 key=38 / drop ptr=0 key=0 / pop=40`. Read the pointer counter against
// that floor of 1, or a healthy zero-HID boot looks like a live pointer.
//
// P56 VERDICT TABLE — read `[uvug10] evq` against `[uvug9] shell-path`, with the fixture now gated off
// metal (so no orphan should exist and `el0_input_active()` should be 0 all boot):
//   * EXPECTED: `[uvug9] ptr` climbs normally with a moving mouse and `push ptr` climbs with it. The orphan
//     theory was right and the fixture gate was the fix; nothing further is owed.
//   * `[uvug9] ptr=0` STILL, with no orphan alive -> the orphan theory is refuted and the hunt RESUMES at
//     or after this queue. These counters then discriminate:
//       - `push ptr > 1` with `drop ptr ≈ push ptr` -> produced, then discarded by a saturated ring: the
//         drain is not keeping up (check `depth` and the `[click2]` channel depth alongside).
//       - `push ptr > 1` with `drop ptr = 0` -> produced and stored, then consumed by SOMEONE ELSE before
//         the shell drain. `pop` far above the router's own `[uvug9]` totals names that second consumer;
//         the candidates are an EL0 focus ring and `el0_input_set_active`'s pre-launch discard.
//       - `push ptr = 1` (the selftest floor, unmoved) -> against the settled xHCI finding this should be
//         unreachable; if it happens, the pointer endpoint itself stopped completing this boot and the
//         question is back in the driver lane. Confirm against `MOUSE-1`'s report count before concluding.
/// Pointer-class events (Mouse / MouseAbsolute / Button) offered to the queue.
static EVQ_PUSH_PTR: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// Key-class events (Key / KeyUp) offered to the queue.
static EVQ_PUSH_KEY: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// Pointer-class events DROPPED because the ring was full.
static EVQ_DROP_PTR: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// Key-class events DROPPED because the ring was full.
static EVQ_DROP_KEY: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// Events successfully popped by ANY consumer (the router drain, an EL0 focus discard, `pump_and_poll`, …).
static EVQ_POP: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// UVUG-10 — `(push_ptr, push_key, drop_ptr, drop_key, pop)`. Read by the router's `[uvug10] evq` witness.
pub fn event_queue_stats() -> (u64, u64, u64, u64, u64) {
    use core::sync::atomic::Ordering::Relaxed;
    (
        EVQ_PUSH_PTR.load(Relaxed),
        EVQ_PUSH_KEY.load(Relaxed),
        EVQ_DROP_PTR.load(Relaxed),
        EVQ_DROP_KEY.load(Relaxed),
        EVQ_POP.load(Relaxed),
    )
}

pub fn push_event(event: Event) {
    use core::sync::atomic::Ordering::Relaxed;
    let is_ptr = matches!(
        event,
        Event::Mouse { .. } | Event::MouseAbsolute { .. } | Event::Button(_)
    );
    let is_key = matches!(event, Event::Key(_) | Event::KeyUp(_));
    if is_ptr {
        EVQ_PUSH_PTR.fetch_add(1, Relaxed);
    } else if is_key {
        EVQ_PUSH_KEY.fetch_add(1, Relaxed);
    }
    let stored = crate::arch::without_interrupts(|| EVENT_QUEUE.lock().push(event));
    if !stored {
        if is_ptr {
            EVQ_DROP_PTR.fetch_add(1, Relaxed);
        } else if is_key {
            EVQ_DROP_KEY.fetch_add(1, Relaxed);
        }
    }
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
// UVUG-9 — LIVENESS IS NOW EVIDENCE-GATED (the ~10-repeat stop, P54b metal fact 3).
//
// UVUG-6 flagged this exact failure as a "benign, self-correcting degradation" and shipped it. P54b measured
// it: holding a key at the shell repeated about ten times and then stopped dead. That is layer 3 firing on a
// perfectly healthy keyboard, and the arithmetic matches the constants precisely — a strict SET_IDLE(0) boot
// keyboard sends ONE report on the press and then nothing at all while the key is held still, so
// `LAST_REPORT_MS` freezes at the press. Repeats begin at `DELAY_MS` (400 ms) and run every `RATE_MS` (40 ms)
// until `now - last > LIVENESS_MS` (1000 ms) disarms the key: the window is 400..1000 ms, i.e. ~15 repeats,
// and shorter once the press report's own latency is counted. "About ten, then it stops" is not a mystery
// symptom; it is this guard's designed behaviour meeting hardware the guard was never valid for.
//
// The flaw is that layer 3 infers "the keyboard is wedged" from silence, on a device class whose CORRECT
// behaviour under SET_IDLE(0) is silence. Silence carries no information here, so the guard cannot be sound.
//
// Fix: only trust silence from a keyboard that has DEMONSTRATED it does not stay silent. `typematic_note_report`
// sets `STREAMS_WHILE_HELD` when it sees a report that is not a fresh press while the armed key is still down —
// positive proof that this device re-reports during a hold, which is exactly the P51-class hardware layer 3 was
// written for. With that evidence the 1 s window applies unchanged and the P51 wedge stays shut. Without it,
// silence is expected and the bound becomes `HOLD_MAX_MS`, a coarse backstop that still keeps the catastrophic
// case finite while letting a held key repeat for as long as anyone actually holds one.
//
// Layers 1 and 2 are untouched and are the real release paths for a SET_IDLE(0) keyboard: the release EDGE does
// produce a report (that is what ends a hold), layer 1 reads it directly from the report's held set, and a
// mid-hold unplug is layer 2's. Layer 3 only ever covered the residue where a release report never reaches the
// decode at all — and the backpressure guard independently prevents a stuck repeat from starving real input in
// the meantime. The evidence gate is cleared on a keyboard detach, so a swapped device re-earns its own verdict.
//
// PAL-TYPEMATIC — CHAIN B CLOSED: THE LAPSE NOW RE-ARMS, AND THE VERDICT IS SCOPED TO ITS HOLD.
//
// KEYSTAT (audit arc, commit 3b5cd2e5) traced the surviving "key repeat times out" report to two predicates in
// this file and specified the repair for whoever owned `pal.rs`. Both halves land here, verbatim to that spec.
//
//   1. THE LAPSE DID NOT RE-ARM. Every disarm above stores `KEY_P1 = 0`, and the ONLY writer that puts a key
//      back is the PRESS-edge arm at the bottom of `typematic_note_report` (`newest_press != 0`). So a report
//      that positively proves the key is STILL HELD — `held.contains(&k)`, the strongest evidence the tracker
//      ever receives — could not restart the repeat: the operator had to lift and re-press. The liveness guard
//      disarmed on SILENCE and then ignored the very evidence that refuted its own inference. Fixed: a lapse
//      parks the key in `LAPSED_P1` instead of forgetting it, and the next report whose held set still contains
//      it re-arms at `DELAY_MS` (a fresh initial delay, so a re-arm feels like the hold it is and cannot
//      free-run). A report WITHOUT the key clears the parked slot — a real release ends the hold for good, and
//      layers 1/2 keep their absolute authority: detach and release both clear `LAPSED_P1` outright, so this
//      can never resurrect the P51 stuck-repeat wedge.
//
//   2. THE STREAMING VERDICT WAS BOOT-WIDE. `STREAMS_WHILE_HELD` latched on the first hold that produced
//      `IDLE_RUN_TO_LATCH` idle re-reports and was STICKY until detach, so ONE streaming hold imposed the tight
//      `LIVENESS_MS` window on EVERY later hold of that boot — including holds during which the device happens
//      not to re-report at all, which then stopped after ~15 repeats, SILENTLY (the tight window's disarm is
//      deliberately quiet, so it is indistinguishable at the bench from the ~10-repeat stop UVUG-9 exists to
//      have removed). Fixed: the verdict is scoped to the hold that earned it. A report with an EMPTY held set
//      is the end of the hold, and it clears the verdict and its evidence. The P51 protection is not weakened:
//      a genuinely streaming keyboard re-earns the latch within `IDLE_RUN_TO_LATCH` report periods (tens of ms
//      at any real polling interval), i.e. long before `DELAY_MS` has even elapsed and the first repeat is due.
//
// Witness (`[keystat]`): a re-arm names itself for the first few per hold, and every hold that produced repeats
// closes with one rollup line — repeats, re-arms, and which window was in force. Bounded by human key holds.
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
    /// Liveness window for a keyboard PROVEN to re-report during a hold (see `STREAMS_WHILE_HELD`): silence
    /// this long from such a device is genuinely anomalous -> drop the held key.
    pub(super) const LIVENESS_MS: u64 = 1000;
    /// UVUG-9 — the backstop window for a keyboard that has NOT proven it re-reports during a hold (a strict
    /// `SET_IDLE(0)` device, whose correct behaviour is total silence while a key is held still). Silence says
    /// nothing about health there, so it cannot be read as a wedge; this bound exists only so the pathological
    /// case stays finite. Thirty seconds is far longer than any real key hold and far shorter than forever.
    pub(super) const HOLD_MAX_MS: u64 = 30_000;
    /// UVUG-9 — sticky evidence that THIS keyboard emits genuine IDLE RE-REPORTS while a key is held down
    /// (0 = not yet observed, 1 = observed). Cleared on a keyboard detach so a newly attached device re-earns
    /// the verdict. See `typematic_note_report` for why the test is "held set byte-identical to the previous
    /// report" and not merely "no press edge".
    pub(super) static STREAMS_WHILE_HELD: AtomicU32 = AtomicU32::new(0);
    /// UVUG-9 — the PREVIOUS report's held-ascii set, packed (see `pack_held`), so an idle re-report can be
    /// told apart from a report that merely carried no PRESS edge. Bit 63 marks the value valid (no previous
    /// report yet == 0). Cleared on detach alongside the verdict it feeds.
    pub(super) static PREV_HELD: AtomicU64 = AtomicU64::new(0);
    /// UVUG-9 — how many CONSECUTIVE reports have arrived with the held set unchanged and no press edge. Reset
    /// by any change to the held set. See `IDLE_RUN_TO_LATCH`.
    pub(super) static IDLE_RUN: AtomicU32 = AtomicU32::new(0);
    /// UVUG-9 — consecutive unchanged-held reports required before believing this keyboard idle-re-reports.
    ///
    /// One such report is not proof, and the residual case the byte-identical test alone cannot see is a key
    /// that maps to ascii 0 (F-keys, and anything else absent from `HID_SCANCODE_TO_ASCII`) being tapped while
    /// another key is held: the ascii projection this function receives discards the very keycode that changed,
    /// so press and release both look like unchanged-held reports. Closing that properly would mean feeding the
    /// tracker raw KEYCODES, which is a `drivers::xhci` signature change and outside this arc's lane. A run
    /// threshold closes it from this side instead: a tap yields two such reports, whereas a keyboard that truly
    /// idle-re-reports produces them continuously for as long as the key is held. Four is comfortably above a
    /// tap (or a double-tap) and is reached within a few report periods of a genuine hold.
    pub(super) const IDLE_RUN_TO_LATCH: u32 = 4;

    // --- PAL-TYPEMATIC: the re-arm slot and the hold accounting ---

    /// PAL-TYPEMATIC — the key a LIVENESS/BACKSTOP lapse disarmed, +1 (0 = none parked). Not a second armed
    /// slot: nothing repeats off it. It exists so the next report can be asked the one question the old code
    /// never asked — "is that key STILL down?" — and re-arm if the answer is yes. Cleared by a report that
    /// does not contain it (a real release), by a fresh press, and by a detach.
    pub(super) static LAPSED_P1: AtomicU32 = AtomicU32::new(0);
    /// PAL-TYPEMATIC — the key the CURRENT hold is about, +1, kept for the hold-end rollup line (the armed
    /// slot is already 0 by the time the release report is being processed).
    pub(super) static HOLD_KEY: AtomicU32 = AtomicU32::new(0);
    /// PAL-TYPEMATIC — repeats emitted during the current hold (rollup, then reset at hold end).
    pub(super) static HOLD_REPEATS: AtomicU32 = AtomicU32::new(0);
    /// PAL-TYPEMATIC — lapse re-arms during the current hold (rollup, then reset at hold end). A non-zero
    /// value is the bench-visible proof that chain B fired AND was recovered from, rather than ending the hold.
    pub(super) static HOLD_REARMS: AtomicU32 = AtomicU32::new(0);
    /// PAL-TYPEMATIC — boot totals, carried on every rollup so a single line answers "is repeat flowing at all".
    pub(super) static BOOT_REPEATS: AtomicU64 = AtomicU64::new(0);
    pub(super) static BOOT_REARMS: AtomicU64 = AtomicU64::new(0);
    /// PAL-TYPEMATIC — how many re-arms name themselves per hold before the rollup takes over. A pathological
    /// device could re-arm on every report; the serial line must not become the new backpressure.
    pub(super) const REARM_LOG_MAX: u32 = 3;
}

/// UVUG-9 — pack a held-ascii set into one word for exact comparison against the previous report: bit 63 =
/// valid, bits 56..62 = length, bytes 0..5 = the set in report order. A HID boot report carries at most six
/// keycodes, so nothing is lost. Order-sensitive by design: a reordered set compares unequal, which only ever
/// costs a missed latch (the safe direction — the conservative `HOLD_MAX_MS` window stays in force).
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
fn pack_held(held: &[u8]) -> u64 {
    let n = held.len().min(6);
    let mut v: u64 = (1 << 63) | ((n as u64) << 56);
    for (i, &b) in held.iter().take(n).enumerate() {
        v |= (b as u64) << (i * 8);
    }
    v
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
    // UVUG-9: snapshot this report's held set and the previous one, for the idle-re-report test below.
    let cur_packed = pack_held(held);
    let prev_packed = typematic::PREV_HELD.swap(cur_packed, Ordering::Relaxed);
    // UVUG-9: maintain the consecutive-idle-re-report run. Any press edge, or any change to the held set,
    // means this report carried real user action and breaks the run.
    let idle_report = newest_press == 0 && prev_packed == cur_packed;
    let idle_run = if idle_report {
        typematic::IDLE_RUN.fetch_add(1, Ordering::Relaxed) + 1
    } else {
        typematic::IDLE_RUN.store(0, Ordering::Relaxed);
        0
    };
    // PAL-TYPEMATIC (2) — the HOLD ENDED. An empty held set is the unambiguous "nothing is down any more",
    // and it is where the streaming verdict earned by THIS hold expires. Boot-wide stickiness is what let one
    // streaming hold impose the 1 s window on every later hold of the boot; scoping it here means a keyboard
    // that re-reports only sometimes gets the conservative backstop on the holds where it does not, instead of
    // a silent stop after ~15 repeats. A device that really does stream re-earns the latch within
    // `IDLE_RUN_TO_LATCH` report periods — well inside `DELAY_MS`, i.e. before the first repeat is even due —
    // so the P51 wedge guard is armed by the time there is anything for it to guard.
    if held.is_empty() {
        // Rollup FIRST: its `window=` field reads `STREAMS_WHILE_HELD`, and it must report the window that was
        // actually in force during the hold being closed, not the cleared one the next hold will start from.
        typematic_hold_rollup();
        typematic::STREAMS_WHILE_HELD.store(0, Ordering::Relaxed);
        typematic::IDLE_RUN.store(0, Ordering::Relaxed);
        typematic::LAPSED_P1.store(0, Ordering::Relaxed);
    }
    let kp1 = typematic::KEY_P1.load(Ordering::Relaxed);
    if kp1 != 0 {
        let k = (kp1 - 1) as u8;
        if !held.contains(&k) {
            typematic::KEY_P1.store(0, Ordering::Relaxed);
        } else if idle_run >= typematic::IDLE_RUN_TO_LATCH {
            // UVUG-9: a TRUE IDLE RE-REPORT — the armed key is still down and this report's held set is
            // byte-identical to the previous one, so the report carried no press edge AND no release edge. It
            // conveys nothing except that the keyboard is still talking, which is precisely the evidence
            // `LIVENESS_MS` (silence == wedge) needs to be a sound inference. Sticky until the device detaches.
            //
            // "No PRESS edge" alone would NOT have been sound, and the difference is the whole point: it also
            // matches (a) a two-key rollover RELEASE (press a, press b, release a — no press edge, armed b
            // still held) and (b) tapping any key that maps to ascii 0, e.g. an F-key, while holding. Both are
            // ordinary things a strict `SET_IDLE(0)` keyboard does, and either one latching just once would
            // re-impose the 1 s window for the rest of the boot — resurrecting the exact ~10-repeat stop this
            // arc exists to remove. Requiring the held set to be UNCHANGED excludes (a) outright: a rollover
            // release shrinks the set. It does NOT by itself exclude (b) — the ascii projection this function
            // receives has already discarded the non-ascii keycode, so its press and release both arrive as
            // unchanged-held reports — which is what `IDLE_RUN_TO_LATCH` is for.
            typematic::STREAMS_WHILE_HELD.store(1, Ordering::Relaxed);
        }
    } else {
        // PAL-TYPEMATIC (1) — THE RE-ARM. Nothing is armed, but a lapse may have parked a key here. This
        // report carries the exact evidence the lapse's inference lacked: if the parked key is still in the
        // held set, the keyboard was never wedged and the operator never let go, so the repeat resumes. If it
        // is absent the hold is genuinely over and the slot is dropped — a release always wins, which is why
        // this cannot reopen the P51 hole (an armed key whose release is missed is layer 1/2's problem, and
        // both of those clear `LAPSED_P1` too).
        let lapsed = typematic::LAPSED_P1.load(Ordering::Relaxed);
        if lapsed != 0 {
            let k = (lapsed - 1) as u8;
            if held.contains(&k) {
                typematic::KEY_P1.store(lapsed, Ordering::Relaxed);
                // A fresh initial delay, not the repeat rate: the re-arm re-enters the hold at its start, so a
                // device re-reporting faster than `DELAY_MS` can never turn re-arming into a free-running spew.
                typematic::NEXT_MS.store(now.wrapping_add(typematic::DELAY_MS), Ordering::Relaxed);
                let n = typematic::HOLD_REARMS.fetch_add(1, Ordering::Relaxed) + 1;
                typematic::BOOT_REARMS.fetch_add(1, Ordering::Relaxed);
                if n <= typematic::REARM_LOG_MAX {
                    serial_println!(
                        "[keystat] typematic re-arm — key={:#04x} still held after a liveness lapse; repeat resumed at delay={}ms (hold re-arms={} boot re-arms={})",
                        k,
                        typematic::DELAY_MS,
                        n,
                        typematic::BOOT_REARMS.load(Ordering::Relaxed)
                    );
                }
            } else {
                typematic::LAPSED_P1.store(0, Ordering::Relaxed);
            }
        }
    }
    // Arm the newest press (newest-wins typematic) and (re)start the initial delay.
    if newest_press != 0 {
        typematic::KEY_P1.store(newest_press as u32 + 1, Ordering::Relaxed);
        typematic::NEXT_MS.store(now.wrapping_add(typematic::DELAY_MS), Ordering::Relaxed);
        // A press supersedes any parked lapse (newest-wins), and names the hold for the rollup.
        typematic::LAPSED_P1.store(0, Ordering::Relaxed);
        typematic::HOLD_KEY.store(newest_press as u32 + 1, Ordering::Relaxed);
    }
}

/// PAL-TYPEMATIC — close out a hold: if it produced anything, say so in one line, then reset the per-hold
/// counters. Called on the report that ends the hold (empty held set) and on a detach, so a hold that ends by
/// unplug is accounted exactly like one that ends by release. Silent for a hold that never repeated (a tap),
/// which is the overwhelming majority of key presses.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
fn typematic_hold_rollup() {
    use core::sync::atomic::Ordering;
    let repeats = typematic::HOLD_REPEATS.swap(0, Ordering::Relaxed);
    let rearms = typematic::HOLD_REARMS.swap(0, Ordering::Relaxed);
    let key = typematic::HOLD_KEY.swap(0, Ordering::Relaxed);
    if repeats == 0 && rearms == 0 {
        return;
    }
    serial_println!(
        "[keystat] typematic hold end — key={:#04x} repeats={} re-arms={} window={}ms (boot: repeats={} re-arms={})",
        key.wrapping_sub(1) as u8,
        repeats,
        rearms,
        if typematic::STREAMS_WHILE_HELD.load(Ordering::Relaxed) != 0 {
            typematic::LIVENESS_MS
        } else {
            typematic::HOLD_MAX_MS
        },
        typematic::BOOT_REPEATS.load(Ordering::Relaxed),
        typematic::BOOT_REARMS.load(Ordering::Relaxed)
    );
}

/// PAL-TYPEMATIC — the ONE place a liveness/backstop lapse disarms a hold (`typematic_tick`'s layer-3 arm and
/// the selftest's forced-lapse aid both go through it). Clears the armed slot and PARKS the key, so the next
/// report that still contains it re-arms instead of the operator having to lift and re-press.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
fn typematic_lapse_disarm(k: u8) {
    use core::sync::atomic::Ordering;
    typematic::KEY_P1.store(0, Ordering::Relaxed);
    typematic::LAPSED_P1.store(k as u32 + 1, Ordering::Relaxed);
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
        // UVUG-9: the streaming verdict belongs to the DEVICE, not the boot — a detach means the next report
        // may come from different hardware, which must earn its own verdict rather than inherit this one. The
        // evidence it was derived from goes with it, or a run straddling the detach could re-latch on a mix of
        // two keyboards' reports.
        typematic_hold_rollup(); // before the verdict is cleared — see the same ordering in `note_report`
        typematic::STREAMS_WHILE_HELD.store(0, Ordering::Relaxed);
        typematic::PREV_HELD.store(0, Ordering::Relaxed);
        typematic::IDLE_RUN.store(0, Ordering::Relaxed);
        // PAL-TYPEMATIC: a detach is layer 2 and it is ABSOLUTE — the parked lapse goes with the armed key, or
        // the next keyboard's first report could re-arm a hold that belonged to hardware no longer present.
        typematic::LAPSED_P1.store(0, Ordering::Relaxed);
        return None;
    }
    let kp1 = typematic::KEY_P1.load(Ordering::Relaxed);
    if kp1 == 0 {
        return None;
    }
    let now = crate::arch::ms();
    // (3) positive liveness, UVUG-9 evidence-gated: silence only means "wedged" on a keyboard that has PROVEN
    // it re-reports during a hold. On a strict SET_IDLE(0) device silence is the correct behaviour of a key
    // being held, and reading it as a wedge is what stopped repeat after ~10 characters on metal (P54b).
    let last = typematic::LAST_REPORT_MS.load(Ordering::Relaxed);
    let window = if typematic::STREAMS_WHILE_HELD.load(Ordering::Relaxed) != 0 {
        typematic::LIVENESS_MS
    } else {
        typematic::HOLD_MAX_MS
    };
    if last != 0 && now.wrapping_sub(last) > window && now >= last {
        // PAL-TYPEMATIC: park the key rather than forget it. Either window may be wrong about a hold that is
        // still physically in progress — that is the whole of chain B — and the cheapest possible refutation
        // is the next report's held set. `typematic_note_report` performs it.
        typematic_lapse_disarm((kp1 - 1) as u8);
        // UVUG-9: name the BACKSTOP when it is what fired. A repeat that simply stops is indistinguishable at
        // the bench from the bug this arc fixed, so the coarse 30 s bound must say so out loud — one line per
        // disarm, which is self-limiting at a minimum of `HOLD_MAX_MS` apart. The tight `LIVENESS_MS` disarm
        // stays silent: it is the ordinary, expected end of a hold on a streaming keyboard.
        if window == typematic::HOLD_MAX_MS {
            serial_println!(
                "[uvug9] typematic hold-max — key held {}s without an idle re-report from this keyboard; repeat disarmed by the BACKSTOP, not by a release (PAL-TYPEMATIC: the next report that still holds the key re-arms it)",
                typematic::HOLD_MAX_MS / 1000
            );
        }
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
        // PAL-TYPEMATIC witness: count every emission, per hold and for the boot. `[keystat] typematic hold
        // end` reports both, so the bench can see repeats FLOWING rather than inferring it from the screen.
        typematic::HOLD_REPEATS.fetch_add(1, Ordering::Relaxed);
        typematic::BOOT_REPEATS.fetch_add(1, Ordering::Relaxed);
        if typematic::HOLD_KEY.load(Ordering::Relaxed) == 0 {
            typematic::HOLD_KEY.store(kp1, Ordering::Relaxed);
        }
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

/// UVUG-9 test aid — read the `STREAMS_WHILE_HELD` verdict, so the selftest can assert that ordinary hold-time
/// traffic (a rollover release, a non-ascii tap) does NOT latch it. Not used on any boot path.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
pub fn typematic_test_streams_latched() -> bool {
    typematic::STREAMS_WHILE_HELD.load(core::sync::atomic::Ordering::Relaxed) != 0
}

/// UVUG-9 test aid — clear all typematic tracker state so a selftest leg starts from a known baseline
/// (equivalent to a fresh keyboard, without faking a detach generation). Not used on any boot path.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
pub fn typematic_test_reset() {
    use core::sync::atomic::Ordering;
    typematic::KEY_P1.store(0, Ordering::Relaxed);
    typematic::STREAMS_WHILE_HELD.store(0, Ordering::Relaxed);
    typematic::PREV_HELD.store(0, Ordering::Relaxed);
    typematic::IDLE_RUN.store(0, Ordering::Relaxed);
    typematic::LAST_REPORT_MS.store(0, Ordering::Relaxed);
    // PAL-TYPEMATIC: the re-arm slot and the hold accounting are tracker state too — a leg that inherited a
    // parked key from the previous leg would re-arm on a key it never pressed.
    typematic::LAPSED_P1.store(0, Ordering::Relaxed);
    typematic::HOLD_KEY.store(0, Ordering::Relaxed);
    typematic::HOLD_REPEATS.store(0, Ordering::Relaxed);
    typematic::HOLD_REARMS.store(0, Ordering::Relaxed);
}

/// PAL-TYPEMATIC test aid — the currently armed repeat key, if any. Lets the selftest assert the re-arm
/// directly (state), not only through an emitted repeat (behaviour). Not used on any boot path.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
pub fn typematic_test_armed() -> Option<u8> {
    let kp1 = typematic::KEY_P1.load(core::sync::atomic::Ordering::Relaxed);
    (kp1 != 0).then(|| (kp1 - 1) as u8)
}

/// PAL-TYPEMATIC test aid — fire the LIVENESS LAPSE on whatever is armed, through the same
/// `typematic_lapse_disarm` seam `typematic_tick`'s layer-3 arm uses. Driving the lapse by the CLOCK instead
/// would mean stalling the selftest for a real `LIVENESS_MS`/`HOLD_MAX_MS`, or putting a test hook in the
/// production window comparison — this touches neither. Not used on any boot path.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
pub fn typematic_test_force_lapse() {
    let kp1 = typematic::KEY_P1.load(core::sync::atomic::Ordering::Relaxed);
    if kp1 != 0 {
        typematic_lapse_disarm((kp1 - 1) as u8);
    }
}

/// UVUG-9 — the consecutive unchanged-held reports required to latch the streaming verdict, mirrored so the
/// selftest sizes its idle-re-report run against the real threshold rather than a magic number.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
pub const TYPEMATIC_IDLE_RUN_TO_LATCH: u32 = typematic::IDLE_RUN_TO_LATCH;

fn pop_event() -> Option<Event> {
    let ev = crate::arch::without_interrupts(|| EVENT_QUEUE.lock().pop());
    if ev.is_some() {
        // UVUG-10: total consumption, across EVERY consumer — the router drain, the EL0 focus-change
        // discard, `pump_and_poll`. `push - drop - pop` is the live ring occupancy; a `pop` count far
        // above the router drain's own `[uvug9]` totals names a second consumer as the thief.
        EVQ_POP.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }
    ev
}

/// UVUG-10 — the RE-CIRCULATION seam: pop / re-push an event WITHOUT touching the accounting counters.
///
/// `main::pump_usb_into_gui`'s `SCREEN_APP_ACTIVE` branch does a non-destructive PEEK: it drains the ring
/// into a fixed buffer to look for a pending Button, then re-pushes every event in its original order and
/// hands the queue back to the full-screen app's own drain. That is not production and not consumption —
/// the same events go round again on the next pass, ~250×/s while a kernel app owns the panel. Counted
/// through `push_event`/`pop_event` it would inflate `push`, `pop` AND (once the ring is deep) `drop` by
/// orders of magnitude, in exactly the state where a stalled drain is the thing under suspicion — the
/// witness would manufacture its own false positive. These two functions keep the peek invisible to the
/// counters, so `push`/`pop` continue to mean "entered the pipeline once" / "left the pipeline for good".
pub fn requeue_event(event: Event) {
    crate::arch::without_interrupts(|| {
        EVENT_QUEUE.lock().push(event);
    });
}

/// WC-TAB — the THIRD outcome of the re-circulation seam: an event taken with `peek_event_uncounted` that
/// is deliberately NOT returned.
///
/// The seam was written for a strict peek: pop uncounted, re-push uncounted, nothing enters or leaves the
/// pipeline. The compositor's shell-side TAB breaks that symmetry — the TAB itself is consumed by the
/// window system, and on a real focus change the events held in the peek buffer are discarded (they are
/// out of the queue only because the peek holds them; `el0_input_set_active` drains `EVENT_QUEUE` on every
/// focus change and would have taken them itself). Those events entered through `push_event` and were
/// COUNTED, so leaving them uncounted on the way out drifts `[uvug10] evq`'s `push - drop - pop`
/// occupancy permanently high — by the buffer size plus one per consumed cycle. Call this with the number
/// actually dropped so `pop` keeps meaning "left the pipeline for good".
pub fn note_uncounted_discard(count: usize) {
    if count > 0 {
        EVQ_POP.fetch_add(count as u64, core::sync::atomic::Ordering::Relaxed);
    }
}

/// The pop half of the re-circulation seam (see [`requeue_event`]). Every event taken with this MUST be
/// returned with `requeue_event` — or, if it is deliberately consumed, accounted with
/// [`note_uncounted_discard`]; otherwise the accounting silently loses it.
pub fn peek_event_uncounted() -> Option<Event> {
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
