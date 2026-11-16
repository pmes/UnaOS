// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Stria — shared-core. The video transition MIX (NLE6MIX, rmbp-ledger B899): the ONE copy of a schedule
//! step's mix math and of the outgoing-run rule, called by Stria's render (`handlers/stria/src/nle.rs` `vmix_rgba`,
//! `vmix_take`) and by the kernel Player's EDL mode (`unaos/crates/kernel/src/video/vplay.rs` `edl_mix`, `edl_take`).
//!
//! * [`mix_step`]: the outgoing frame (in `out`) and the incoming frame `b`, straight RGBA `w x h`, mixed at the
//!   incoming's weight `t` (NLE3, LEDGER SR202): a dissolve is pixel_core's `blend_nonpremult` of the incoming at alpha
//!   `round(255 t)` over the outgoing; a wipe copies the incoming where its SMPTE mask holds (W001 / W002 at
//!   `round(w t)` / `round(h t)`; NLE3c W041 / W101 exact fractions, W201 the clock hand; NLE3cBLACK W003-W006, W021 /
//!   W022, W042, W202-W204; NLEWIPES SR679 the SMPTE families below the tail comment: W007, W023-W026, W043-W047,
//!   W102, W119, W205-W207, W211-W214). The clock angle is `libm::atan2` (no_std: `core` has none).
//! * [`take_outgoing`]: at a transition tick whose outgoing source continues the live decode run (`seek` none) and is
//!   not already the mix run, that run becomes the mix run; outside a transition the mix run is dropped.

use crate::schedule::{Frac, Step};
use crate::{SourceId, TransitionKind, WipeDir};

/// Mix in place: `out` holds the OUTGOING frame and becomes the step's picture; `b` is the INCOMING frame (both straight
/// RGBA, `w x h`); `t` is the incoming's weight (the schedule's `VideoMix::t`).
pub fn mix_step(kind: TransitionKind, t: Frac, out: &mut [u8], b: &[u8], w: u32, h: u32) {
    let den = t.den.max(1);
    let round = |n: i64| (2 * n * t.num + den) / (2 * den);
    match kind {
        TransitionKind::Dissolve => {
            let alpha = round(255).clamp(0, 255) as u8;
            for (o, p) in out.chunks_exact_mut(4).zip(b.chunks_exact(4)) {
                let px = pixel_core::blend_nonpremult([p[0], p[1], p[2], alpha], [o[0], o[1], o[2], o[3]]);
                o.copy_from_slice(&px);
            }
        }
        TransitionKind::Wipe(dir) => {
            let (w, h) = (w as usize, h as usize);
            let (wi, hi, num) = (w as i64, h as i64, t.num);
            for y in 0..h {
                for x in 0..w {
                    let (xi, yi) = (x as i64, y as i64);
                    let incoming = match dir {
                        WipeDir::LeftToRight => xi < round(wi),
                        WipeDir::TopToBottom => yi < round(hi),
                        // NLE3c: exact fractions of t = num / den.
                        WipeDir::Diagonal => (xi + yi) * den < (wi + hi) * num,
                        WipeDir::Box => (2 * xi + 1 - wi).abs() * den < wi * num && (2 * yi + 1 - hi).abs() * den < hi * num,
                        WipeDir::Clock | WipeDir::ClockThree | WipeDir::ClockSix | WipeDir::ClockNine => {
                            clock_from(dir, x, y, w, h) < core::f64::consts::TAU * num as f64 / den as f64
                        }
                        // NLE3cBLACK: the corner boxes (mirrored coordinates), barn doors and the top-right diagonal.
                        WipeDir::CornerTopLeft => xi * den < wi * num && yi * den < hi * num,
                        WipeDir::CornerTopRight => (wi - 1 - xi) * den < wi * num && yi * den < hi * num,
                        WipeDir::CornerBottomRight => (wi - 1 - xi) * den < wi * num && (hi - 1 - yi) * den < hi * num,
                        WipeDir::CornerBottomLeft => xi * den < wi * num && (hi - 1 - yi) * den < hi * num,
                        WipeDir::BarnDoorVertical => (2 * xi + 1 - wi).abs() * den < wi * num,
                        WipeDir::BarnDoorHorizontal => (2 * yi + 1 - hi).abs() * den < hi * num,
                        WipeDir::DiagonalTopRight => (wi - 1 - xi + yi) * den < (wi + hi) * num,
                        // NLEWIPES (SR679): one arm per SMPTE family, the family fns at the file tail.
                        WipeDir::FourBoxCornersIn => four_box(xi, yi, wi, hi, num, den),
                        WipeDir::EdgeBoxTop | WipeDir::EdgeBoxRight | WipeDir::EdgeBoxBottom | WipeDir::EdgeBoxLeft => edge_box(dir, xi, yi, wi, hi, num, den),
                        WipeDir::BowTieVertical | WipeDir::BowTieHorizontal => bow_tie(dir, xi, yi, wi, hi, num, den),
                        WipeDir::BarnDoorDiagonalBottomLeft | WipeDir::BarnDoorDiagonalTopLeft | WipeDir::DoubleBarnDoor => diagonal_doors(dir, xi, yi, wi, hi, num, den),
                        WipeDir::Diamond | WipeDir::Circle => iris(dir, xi, yi, wi, hi, num, den),
                        WipeDir::PinWheelTwoVertical
                        | WipeDir::PinWheelTwoHorizontal
                        | WipeDir::PinWheelFour
                        | WipeDir::FanCenterTop
                        | WipeDir::FanCenterRight
                        | WipeDir::DoubleFanVertical
                        | WipeDir::DoubleFanHorizontal => sweep(dir, x, y, w, h, num, den),
                    };
                    if incoming {
                        let i = (y * w + x) * 4;
                        out[i..i + 4].copy_from_slice(&b[i..i + 4]);
                    }
                }
            }
        }
    }
}

/// The outgoing-run rule (NLE3): outside a transition (`s.vmix` none) `mix` is dropped; at a transition tick whose
/// outgoing source continues the live run (`seek` none), is not already `mix`'s and is `live`'s, `live` moves into `mix`
/// (the step's own source then opens its run at the cut, as a plain cut does). A transition from black has no run to
/// take. Returns whether `live` moved.
pub fn take_outgoing<R>(s: &Step, live: &mut Option<R>, mix: &mut Option<R>, src_of: impl Fn(&R) -> SourceId) -> bool {
    let Some(m) = s.vmix else {
        *mix = None;
        return false;
    };
    if m.src.is_some() && m.seek.is_none() && mix.as_ref().map(&src_of) != m.src && live.as_ref().map(&src_of) == m.src {
        *mix = live.take();
        return true;
    }
    false
}

/// `x % m` into `[0, m)` (std's `f64::rem_euclid` for a positive `m`, written for `core`).
fn rem_tau(x: f64) -> f64 {
    let m = core::f64::consts::TAU;
    let r = x % m;
    if r < 0.0 { r + m } else { r }
}

/// NLE3c: the angle of pixel `(x, y)`'s centre about the frame's centre, clockwise from twelve o'clock, in `[0, 2 pi)`.
fn clock_angle(x: usize, y: usize, w: usize, h: usize) -> f64 {
    let (dx, dy) = (x as f64 + 0.5 - w as f64 / 2.0, y as f64 + 0.5 - h as f64 / 2.0);
    rem_tau(libm::atan2(dx, -dy))
}

/// NLE3cBLACK (LEDGER SR323): pixel `(x, y)`'s clock angle measured from where `dir`'s hand starts (W201 twelve, W202
/// three, W203 six, W204 nine o'clock), clockwise, in `[0, 2 pi)`; W201's is [`clock_angle`] itself.
fn clock_from(dir: WipeDir, x: usize, y: usize, w: usize, h: usize) -> f64 {
    let q = match dir {
        WipeDir::ClockThree => 1.0,
        WipeDir::ClockSix => 2.0,
        WipeDir::ClockNine => 3.0,
        _ => return clock_angle(x, y, w, h),
    };
    rem_tau(clock_angle(x, y, w, h) - q * core::f64::consts::TAU / 4.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schedule::VideoMix;
    use crate::Kind;
    use alloc::vec::Vec;

    fn fnv(bytes: &[u8]) -> u64 {
        bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x0000_0100_0000_01B3))
    }

    /// The kernel's utp1 picture (vplay `Pic::Utp1`): a flat field at level `28 + ord % 200`, opaque.
    fn utp1(ord: u32) -> Vec<u8> {
        let l = (28 + ord % 200) as u8;
        [l, l, l, 255].repeat(64 * 48)
    }

    /// The pins of the kernel's `tests nle` (player.rs `nle_fx::MIX_KAT`), re-derived through THIS fn: tick 11 is the
    /// dissolve A->B at k=1 of 4 (outgoing A ord 1024, incoming B ord 2001, t = 2/5 -> a flat 42); tick 22 is the W001
    /// wipe to black at k=2 of 4 (outgoing B ord 2012, black incoming, t = 3/5 -> columns 0..37 black).
    #[test]
    fn kernel_fixture_pins() {
        let mut d = utp1(1024);
        mix_step(TransitionKind::Dissolve, Frac { num: 2, den: 5 }, &mut d, &utp1(2001), 64, 48);
        assert!(d.chunks_exact(4).all(|p| p == [42, 42, 42, 255]));
        assert_eq!(fnv(&d), 0x565d_af03_7163_1325);
        let mut k = utp1(2012);
        mix_step(TransitionKind::Wipe(WipeDir::LeftToRight), Frac { num: 3, den: 5 }, &mut k, &[0u8, 0, 0, 255].repeat(64 * 48), 64, 48);
        assert_eq!(&k[37 * 4..38 * 4], &[0, 0, 0, 255]);
        assert_eq!(&k[38 * 4..39 * 4], &[40, 40, 40, 255]);
        assert_eq!(fnv(&k), 0x575d_e51b_bec6_ed25);
        // NLE6MIXFACE (rmbp-ledger B957): tick 10, the dissolve at k=0 (outgoing A ord 1023 under B ord 2000, t = 1/5) -> a flat 46.
        let mut p = utp1(1023);
        mix_step(TransitionKind::Dissolve, Frac { num: 1, den: 5 }, &mut p, &utp1(2000), 64, 48);
        assert!(p.chunks_exact(4).all(|q| q == [46, 46, 46, 255]) && fnv(&p) == 0x575a_f060_ca99_7325);
    }

    /// The clock hands: a quarter turn of W201 holds exactly the top-right quadrant (no 1-ulp edge on a 64x48 grid).
    #[test]
    fn clock_quarter() {
        let mut o = [0u8, 0, 0, 255].repeat(64 * 48);
        mix_step(TransitionKind::Wipe(WipeDir::Clock), Frac { num: 1, den: 4 }, &mut o, &[255u8, 255, 255, 255].repeat(64 * 48), 64, 48);
        for (i, p) in o.chunks_exact(4).enumerate() {
            let (x, y) = (i % 64, i / 64);
            assert_eq!(p[0] == 255, x >= 32 && y < 24, "({x},{y})");
        }
    }

    #[test]
    fn take_outgoing_rule() {
        let (a, b) = (SourceId(0), SourceId(1));
        let step = |vmix: Option<VideoMix>| Step { kind: Kind::Video, track: Some(0), src: Some(b), src_t: 0, out_t: 10, len: 1, seek: Some(0), vmix };
        let mixing = |seek: Option<i64>| Some(VideoMix { kind: TransitionKind::Dissolve, src: Some(a), src_t: 23, seek, t: Frac { num: 1, den: 5 } });
        // the live run on the outgoing source moves into the mix slot
        let (mut live, mut mix) = (Some(a), None);
        assert!(take_outgoing(&step(mixing(None)), &mut live, &mut mix, |r| *r));
        assert_eq!((live, mix), (None, Some(a)));
        // already the mix run: nothing moves
        let mut live = Some(b);
        assert!(!take_outgoing(&step(mixing(None)), &mut live, &mut mix, |r| *r));
        assert_eq!((live, mix), (Some(b), Some(a)));
        // a seek on the outgoing: the live run is not taken
        let (mut live, mut mix) = (Some(a), None);
        assert!(!take_outgoing(&step(mixing(Some(0))), &mut live, &mut mix, |r| *r));
        assert_eq!((live, mix), (Some(a), None));
        // outside a transition the mix slot is dropped
        let mut mix = Some(a);
        assert!(!take_outgoing(&step(None), &mut live, &mut mix, |r| *r));
        assert_eq!(mix, None);
    }
}

// ---- NLEWIPES (LEDGER SR679): the SMPTE wipe families (named tail block) ----
//
// The codes and names are the W3C SMIL 3.0 Transition Effects Table 1 (SMPTE 258M-1993 governs); it publishes the shapes as
// illustrations, so each fn states the geometry it draws. Centred doubled coordinates: X = 2x + 1 - w, Y = 2y + 1 - h
// (|X| < w, |Y| < h), so u = X / w and v = Y / h lie in (-1, 1); f = num / den is the (eased) incoming weight. Every
// test is an exact integer comparison except `sweep`, which reads the clock arms' `clock_angle`.

/// W007 fourBoxWipe cornersIn: a box from each corner, `f` of its quadrant's width and height (a pixel's distance from
/// its nearest vertical / horizontal frame edge against `f w / 2` / `f h / 2`).
fn four_box(x: i64, y: i64, w: i64, h: i64, num: i64, den: i64) -> bool {
    2 * x.min(w - 1 - x) * den < w * num && 2 * y.min(h - 1 - y) * den < h * num
}

/// W023-W026 boxWipe topCenter / rightCenter / bottomCenter / leftCenter: a box from an edge's midpoint, centred on the
/// edge's axis as W101 (`|X| < f w`) and anchored on the edge as W003 (`y < f h`).
fn edge_box(dir: WipeDir, x: i64, y: i64, w: i64, h: i64, num: i64, den: i64) -> bool {
    let (cx, cy) = ((2 * x + 1 - w).abs() * den < w * num, (2 * y + 1 - h).abs() * den < h * num);
    match dir {
        WipeDir::EdgeBoxTop => cx && y * den < h * num,
        WipeDir::EdgeBoxRight => cy && (w - 1 - x) * den < w * num,
        WipeDir::EdgeBoxBottom => cx && (h - 1 - y) * den < h * num,
        _ => cy && x * den < w * num,
    }
}

/// W043 / W044 bowTieWipe vertical / horizontal: `|v| - |u| > 1 - 2 f` (vertical: triangles on the top and bottom edges
/// whose apexes meet at the centre at f = 1/2, the hourglass `|v| > |u|`, then widen to the sides); horizontal swaps u, v.
fn bow_tie(dir: WipeDir, x: i64, y: i64, w: i64, h: i64, num: i64, den: i64) -> bool {
    let (ax, ay) = ((2 * x + 1 - w).abs(), (2 * y + 1 - h).abs());
    let lead = if dir == WipeDir::BowTieVertical { ay * w - ax * h } else { ax * h - ay * w };
    lead * den > w * h * (den - 2 * num)
}

/// W045 / W046 barnDoorWipe diagonalBottomLeft / diagonalTopLeft: a band opening about the bottom-left to top-right
/// diagonal (`|u + v| < 2 f`) / the top-left to bottom-right diagonal (`|u - v| < 2 f`); W047 miscDiagonalWipe
/// doubleBarnDoor: both bands.
fn diagonal_doors(dir: WipeDir, x: i64, y: i64, w: i64, h: i64, num: i64, den: i64) -> bool {
    let (xx, yy) = (2 * x + 1 - w, 2 * y + 1 - h);
    let bl = (xx * h + yy * w).abs() * den < 2 * w * h * num;
    let tl = (xx * h - yy * w).abs() * den < 2 * w * h * num;
    match dir {
        WipeDir::BarnDoorDiagonalBottomLeft => bl,
        WipeDir::BarnDoorDiagonalTopLeft => tl,
        _ => bl || tl,
    }
}

/// W102 irisWipe diamond: `|u| + |v| < 2 f`; W119 ellipseWipe circle: `X^2 + Y^2 < f^2 (w^2 + h^2)` (the radius
/// reaches the corners at f = 1), in i128 (an eased `den` reaches `(dur + 1)^3`, its square past i64).
fn iris(dir: WipeDir, x: i64, y: i64, w: i64, h: i64, num: i64, den: i64) -> bool {
    let (xx, yy) = (2 * x + 1 - w, 2 * y + 1 - h);
    if dir == WipeDir::Diamond {
        return (xx.abs() * h + yy.abs() * w) * den < 2 * w * h * num;
    }
    let (xx, yy, w, h, num, den) = (xx as i128, yy as i128, w as i128, h as i128, num as i128, den as i128);
    (xx * xx + yy * yy) * den * den < (w * w + h * h) * num * num
}

/// W205-W207 pinWheelWipe, W211 / W212 fanWipe centerTop / centerRight, W213 / W214 doubleFanWipe fanOutVertical /
/// fanOutHorizontal: ONE angular family over `n` blades starting at clock angle `q` (`clock_angle`, clockwise from
/// twelve): with `p = 2 pi / n` and `r = (a - q) mod p`, a pinwheel's hands sweep one way (`r < p f`), a fan opens both
/// ways from its start (`min(r, p - r) < p f / 2`).
fn sweep(dir: WipeDir, x: usize, y: usize, w: usize, h: usize, num: i64, den: i64) -> bool {
    let quarter = core::f64::consts::TAU / 4.0;
    let (n, q, fan) = match dir {
        WipeDir::PinWheelTwoVertical => (2.0, 0.0, false),
        WipeDir::PinWheelTwoHorizontal => (2.0, quarter, false),
        WipeDir::PinWheelFour => (4.0, 0.0, false),
        WipeDir::FanCenterTop => (1.0, 0.0, true),
        WipeDir::FanCenterRight => (1.0, quarter, true),
        WipeDir::DoubleFanVertical => (2.0, 0.0, true),
        _ => (2.0, quarter, true),
    };
    let p = core::f64::consts::TAU / n;
    let r = rem_tau(clock_angle(x, y, w, h) - q) % p;
    let f = num as f64 / den as f64;
    if fan { r.min(p - r) < p * f / 2.0 } else { r < p * f }
}
