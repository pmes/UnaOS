//! CHARTER: Kernel — driver (the trackpad's gesture recogniser: per-contact samples -> two-finger scroll, pinch, three-finger swipe; the gestures are the system's, not each app's — research C §8; PINCHZOOM rmbp-ledger B735)
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

//! PINCHZOOM (rmbp-ledger B735) — ONE pure state machine from per-contact samples to gestures, and its routes.
//!
//! # The contacts
//!
//! The rMBP pad's TYPE2 decode (`drivers/ehci/mod.rs`, `Wsp2Frame`) carries the finger COUNT and TWO contact
//! records (finger[0], finger[1]: position and `touch_major`). No per-finger id and no pressure are decoded, so a
//! [`Contact`]'s `id` is its record index and `touch` (the record's `touch_major`, non-zero = in contact) stands in
//! for pressure. Two positions carry scroll and pinch; the swipe reads finger[0] with the count.
//!
//! # The machine ([`Recogniser::step`])
//!
//! Families by the contacts present: TWO (n == 2, both records touching) and THREE (n >= 3, the swipe on). A family
//! change, a press (the pad's click or a held button) or a lift ends the current gesture; a scroll ends with
//! [`Out::ScrollEnd`] carrying its totals.
//!
//! - TWO starts undecided (distance `d0`, centroid `c0`) and LOCKS once: `|d - d0| >= PINCH_LOCK` and larger than
//!   the centroid's travel is a PINCH; centroid travel `>= SCROLL_LOCK` is a SCROLL.
//! - SCROLL: the centroid's delta per sample, direct mapping, no inertia; the lock sample carries the catch-up from
//!   `c0`, so the deltas sum to the hand's travel exactly.
//! - PINCH: `scale = d / d0` (percent) and the centre each sample; a STEP +1 when `d >= 1.30 x anchor` (-1 at
//!   `<= 0.77 x`), the anchor moving to `d` at each step.
//! - THREE: ONE swipe per touch once finger[0] travels `>= SWIPE_LOCK` horizontally and at least twice its vertical
//!   travel — fingers moving LEFT = the next window (+1), RIGHT = the previous (-1).
//!
//! [`Recogniser::claims`] is pinch or three: those samples the decoder must not also turn into wheel or pointer motion.
//!
//! # The routes ([`feed`], the decoder's one call)
//!
//! - pinch step -> the ZOOM lens, `zoom::set_factor_at(zoom::stepped(want, step), None, true)` (atomics only, the
//!   chord's regime; the pad is relative, so the centre is the pointer — the point the pinch addresses);
//! - scroll -> the decoder's own `Event::Wheel` push (TRACKPADPANE), gated by [`Recogniser::claims`];
//! - swipe -> a latch ([`take_swipe`]) the window system drains on its device-service pass (`wm::pace_service` ->
//!   `wm::gesture_swipe`), because a decoder never calls `wm` (BP-input §3).
//!
//! WIRE: `[input] gesture=<scroll|pinch|swipe> fingers=<n> dx=<n> dy=<n> scale=<r> ms=<n>` (+ `z=` on a pinch step,
//! `dir=` on a swipe; a scroll at its end with its totals), the first [`WIT_MAX`] of each kind per boot (a hand at the
//! glass — R80). `tests pinchzoom`: `:: PINCHZOOM: pinch=ok scroll=ok swipe=ok contacts=<real|synthetic> -> PASS ::`.

use core::sync::atomic::{AtomicI8, AtomicU32, AtomicU8, Ordering::*};

/// Distance change (sensor units) that locks two fingers as a pinch. [ONE-SOURCE: glass guess — the Wellspring
/// sensor reads ~100 units/mm, so ~4 mm of spread; refuted by Peter's hand.]
pub const PINCH_LOCK: i64 = 400;
/// Centroid travel (sensor units, both axes summed) that locks two fingers as a scroll. [ONE-SOURCE: glass guess.]
pub const SCROLL_LOCK: i64 = 250;
/// finger[0]'s horizontal travel (sensor units) that fires a three-finger swipe. [ONE-SOURCE: glass guess, ~12 mm.]
pub const SWIPE_LOCK: i64 = 1200;
/// A pinch step: the distance at or past this percentage of the anchor steps up …
pub const STEP_UP_PCT: i64 = 130;
/// … and at or under this one steps down.
pub const STEP_DN_PCT: i64 = 77;
/// Witness lines per gesture kind per boot.
const WIT_MAX: u8 = 16;

/// One contact of a sample: its record index, position (sensor units) and `touch_major` (the pressure proxy).
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Contact {
    pub id: u8,
    pub x: i32,
    pub y: i32,
    pub touch: i32,
}

/// One decoded frame as the machine reads it.
#[derive(Clone, Copy, Debug)]
pub struct Sample {
    /// Fingers the frame reports (may exceed the records carried).
    pub n: u8,
    /// The records carried, `None` when absent or not in contact.
    pub c: [Option<Contact>; 2],
    pub ms: u64,
    /// The pad's click or a held button: no gesture while pressed.
    pub pressed: bool,
    /// Three fingers may swipe (off while three-finger drag owns them).
    pub swipe_on: bool,
}

/// What one sample produced.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Out {
    None,
    /// A locked scroll's delta this sample (sensor units).
    Scroll { dx: i32, dy: i32 },
    /// A scroll ended (lift, press or another family): its totals and duration.
    ScrollEnd { dx: i32, dy: i32, ms: u64 },
    /// A locked pinch: the step this sample (-1, 0, +1), `d / d0` in percent, the centre, the duration.
    Pinch { step: i8, scale_pct: u32, cx: i32, cy: i32, ms: u64 },
    /// A swipe fired: +1 next, -1 previous; finger[0]'s travel; the duration.
    Swipe { dir: i8, fingers: u8, dx: i32, dy: i32, ms: u64 },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Idle,
    Two,
    Scroll,
    Pinch,
    Three,
    Swiped,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Family {
    None,
    Two,
    Three,
}

/// The recogniser. Pure: no clock, no lock, no I/O.
pub struct Recogniser {
    mode: Mode,
    n: u8,
    t0: u64,
    d0: i64,
    anchor: i64,
    c0: (i32, i32),
    last: (i32, i32),
    sum: (i32, i32),
}

fn dist(a: Contact, b: Contact) -> i64 {
    let (dx, dy) = ((a.x as i64 - b.x as i64).unsigned_abs(), (a.y as i64 - b.y as i64).unsigned_abs());
    (dx * dx + dy * dy).isqrt() as i64
}

fn mid(a: Contact, b: Contact) -> (i32, i32) {
    (((a.x as i64 + b.x as i64) / 2) as i32, ((a.y as i64 + b.y as i64) / 2) as i32)
}

impl Recogniser {
    pub const fn new() -> Self {
        Recogniser { mode: Mode::Idle, n: 0, t0: 0, d0: 1, anchor: 1, c0: (0, 0), last: (0, 0), sum: (0, 0) }
    }

    /// The fingers belong to a gesture: the decoder must not also make wheel or pointer motion of them.
    pub fn claims(&self) -> bool {
        matches!(self.mode, Mode::Pinch | Mode::Three | Mode::Swiped)
    }

    fn family(&self) -> Family {
        match self.mode {
            Mode::Idle => Family::None,
            Mode::Two | Mode::Scroll | Mode::Pinch => Family::Two,
            Mode::Three | Mode::Swiped => Family::Three,
        }
    }

    /// End the current gesture (a scroll reports its totals).
    fn finish(&mut self, ms: u64) -> Out {
        let out = if self.mode == Mode::Scroll {
            Out::ScrollEnd { dx: self.sum.0, dy: self.sum.1, ms: ms.saturating_sub(self.t0) }
        } else {
            Out::None
        };
        self.mode = Mode::Idle;
        out
    }

    /// One sample in, at most one gesture out.
    pub fn step(&mut self, s: &Sample) -> Out {
        let fam = match (s.pressed, s.n, s.c[0], s.c[1]) {
            (true, ..) => Family::None,
            (_, 2, Some(_), Some(_)) => Family::Two,
            (_, n, Some(_), _) if n >= 3 && s.swipe_on => Family::Three,
            _ => Family::None,
        };
        if fam != self.family() {
            let end = self.finish(s.ms);
            self.n = s.n;
            self.t0 = s.ms;
            self.sum = (0, 0);
            match fam {
                Family::None => {}
                Family::Two => {
                    let (a, b) = (s.c[0].unwrap_or_default(), s.c[1].unwrap_or_default());
                    self.d0 = dist(a, b).max(1);
                    self.anchor = self.d0;
                    self.c0 = mid(a, b);
                    self.last = self.c0;
                    self.mode = Mode::Two;
                }
                Family::Three => {
                    let a = s.c[0].unwrap_or_default();
                    self.c0 = (a.x, a.y);
                    self.mode = Mode::Three;
                }
            }
            return end;
        }
        self.n = s.n;
        let ms = s.ms.saturating_sub(self.t0);
        match self.mode {
            Mode::Idle | Mode::Swiped => Out::None,
            Mode::Two | Mode::Scroll | Mode::Pinch => {
                let (a, b) = (s.c[0].unwrap_or_default(), s.c[1].unwrap_or_default());
                let d = dist(a, b);
                let c = mid(a, b);
                if self.mode == Mode::Two {
                    let dd = (d - self.d0).abs();
                    let ct = (c.0 as i64 - self.c0.0 as i64).abs() + (c.1 as i64 - self.c0.1 as i64).abs();
                    if dd >= PINCH_LOCK && dd > ct {
                        self.mode = Mode::Pinch;
                    } else if ct >= SCROLL_LOCK {
                        self.mode = Mode::Scroll;
                        self.last = self.c0;
                    } else {
                        return Out::None;
                    }
                }
                if self.mode == Mode::Scroll {
                    let (dx, dy) = (c.0.wrapping_sub(self.last.0), c.1.wrapping_sub(self.last.1));
                    self.last = c;
                    self.sum = (self.sum.0.wrapping_add(dx), self.sum.1.wrapping_add(dy));
                    return if dx == 0 && dy == 0 { Out::None } else { Out::Scroll { dx, dy } };
                }
                let step = if d * 100 >= self.anchor * STEP_UP_PCT {
                    self.anchor = d.max(1);
                    1
                } else if d * 100 <= self.anchor * STEP_DN_PCT {
                    self.anchor = d.max(1);
                    -1
                } else {
                    0
                };
                Out::Pinch { step, scale_pct: (d * 100 / self.d0).clamp(0, u32::MAX as i64) as u32, cx: c.0, cy: c.1, ms }
            }
            Mode::Three => {
                let a = s.c[0].unwrap_or_default();
                let (dx, dy) = (a.x as i64 - self.c0.0 as i64, a.y as i64 - self.c0.1 as i64);
                if dx.abs() >= SWIPE_LOCK && dx.abs() >= 2 * dy.abs() {
                    self.mode = Mode::Swiped;
                    let dir = if dx < 0 { 1 } else { -1 };
                    return Out::Swipe { dir, fingers: s.n, dx: dx as i32, dy: dy as i32, ms };
                }
                Out::None
            }
        }
    }
}

impl Default for Recogniser {
    fn default() -> Self {
        Self::new()
    }
}

static REC: crate::sync::Mutex<Recogniser> = crate::sync::Mutex::new(Recogniser::new());
/// A swipe the window system has not taken yet: +1 next, -1 previous, 0 none.
static SWIPE: AtomicI8 = AtomicI8::new(0);
/// Two-contact samples the pad has fed this boot (`tests pinchzoom` says `contacts=real` past zero).
static REAL: AtomicU32 = AtomicU32::new(0);
/// Witness lines spent per kind: scroll, pinch, swipe.
static WIT: [AtomicU8; 3] = [AtomicU8::new(0), AtomicU8::new(0), AtomicU8::new(0)];

fn wit(k: usize) -> bool {
    WIT[k].fetch_add(1, Relaxed) < WIT_MAX
}

/// The decoder's one call (`tpgest::shape`, per TYPE2 frame): the frame's count and two records `(x, y, touch)`,
/// the frame's ms, pressed (click or a held button), swipe on. Steps the machine, routes what it produced, and
/// returns `true` when the fingers belong to a gesture (the decoder then makes no wheel and no pointer motion).
pub fn feed(n: u8, r0: (i32, i32, i32), r1: (i32, i32, i32), ms: u64, pressed: bool, swipe_on: bool) -> bool {
    let rec = |id: u8, (x, y, touch): (i32, i32, i32)| (touch != 0).then_some(Contact { id, x, y, touch });
    let s = Sample { n, c: [if n >= 1 { rec(0, r0) } else { None }, if n >= 2 { rec(1, r1) } else { None }], ms, pressed, swipe_on };
    if s.c[1].is_some() {
        REAL.fetch_add(1, Relaxed);
    }
    let (out, claim) = {
        let mut r = REC.lock();
        let o = r.step(&s);
        (o, r.claims())
    };
    route(out);
    claim
}

/// The hand's routes (the fixture calls the targets directly, so the latch is never taken twice).
fn route(out: Out) {
    match out {
        Out::Pinch { step, scale_pct, ms, .. } if step != 0 => {
            let z = super::stepped(super::want().0, step);
            super::set_factor_at(z, None, true);
            if wit(1) {
                serial_println!("[input] gesture=pinch fingers=2 dx=0 dy=0 scale={}.{:02} ms={} z={} step={}", scale_pct / 100, scale_pct % 100, ms, z, step);
            }
        }
        Out::Swipe { dir, fingers, dx, dy, ms } => {
            SWIPE.store(dir, Release);
            if wit(2) {
                serial_println!("[input] gesture=swipe fingers={} dx={} dy={} scale=1.00 ms={} dir={}", fingers, dx, dy, ms, if dir > 0 { "next" } else { "prev" });
            }
        }
        Out::ScrollEnd { dx, dy, ms } => {
            if wit(0) {
                serial_println!("[input] gesture=scroll fingers=2 dx={} dy={} scale=1.00 ms={}", dx, dy, ms);
            }
        }
        _ => {}
    }
}

/// The swipe not yet acted on (+1 next, -1 previous, 0 none), taken once — `wm::pace_service`'s drain.
#[inline]
pub fn take_swipe() -> i8 {
    if SWIPE.load(Relaxed) == 0 {
        return 0;
    }
    SWIPE.swap(0, AcqRel)
}

/// Two-contact samples the pad has fed this boot.
pub fn real_samples() -> u32 {
    REAL.load(Relaxed)
}
