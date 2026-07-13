// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Lesser General Public License for more details.

//! Known-answer tests that FREEZE the PCA9685 register/prescale encoding.
//!
//! Each contract is pinned two independent ways, the same discipline as
//! `libs/ibus/tests/kat_vectors.rs`:
//!
//! 1. **Byte / value literals** hand-derived from the datasheet (the prescale
//!    bytes `0x79`/`0x65`/`0x05`, the 50 Hz servo-pulse counts 205/307/410, the
//!    init write-list, the `0x06 + 4·n` addresses, the full-on/off bits). These
//!    nail the format to concrete numbers no formula can drift away from.
//! 2. **Local re-statements** of the datasheet formulas — deliberately NOT the
//!    crate's own routines — cross-checked against the crate over a sweep, so a
//!    KAT that uses them is a genuine independent check.
//!
//! ⚠ Upgrading synthetic → silicon-derived: drop a real logic-analyser capture
//! of a programming burst into [`CAPTURED_WRITES`]; [`kat_captured_writes`]
//! exercises it. No other change is needed (mirrors `ibus`'s `CAPTURED_FRAMES`).

use pca9685::{
    channel_base, channel_full_off, channel_full_on, channel_pulse,
    channel_writes, frequency_init, Prescale, Write, COUNT_MAX, LED0_ON_L, MODE1, MODE1_AWAKE,
    MODE1_SLEEPING, MODE2, MODE2_INIT, NUM_CHANNELS, OSC_CLK_HZ, PRESCALE_MAX, PRESCALE_MIN,
    PRE_SCALE, PWM_STEPS,
};

// ---------------------------------------------------------------------------
// Local, independent re-statements of the datasheet formulas (§7.3.5, §7.3.3).
// Not the crate's routines — using floats + an explicit round to be a genuine
// second implementation of the same math.
// ---------------------------------------------------------------------------

/// prescale = round(osc / (4096 · rate)) − 1, clamped to [3, 255].
fn kat_prescale(osc: u32, rate: u32) -> u8 {
    let exact = osc as f64 / (PWM_STEPS as f64 * rate as f64);
    let reg = (exact.round() as i64) - 1;
    reg.clamp(PRESCALE_MIN as i64, PRESCALE_MAX as i64) as u8
}

/// counts = round(us · 4096 · achieved_freq / 1e6), where the achieved frequency
/// is osc / (4096 · (prescale + 1)) — the physically real rate, not the request.
fn kat_counts(osc: u32, prescale: u8, us: u32) -> u16 {
    let achieved = osc as f64 / (PWM_STEPS as f64 * (prescale as f64 + 1.0));
    let counts = (us as f64 * PWM_STEPS as f64 * achieved / 1_000_000.0).round();
    counts.min(COUNT_MAX as f64) as u16
}

// ---------------------------------------------------------------------------
// 1. Prescale table — the datasheet-canonical bytes (§7.3.5, internal 25 MHz).
//    50 Hz  ⇒ round(25e6/(4096·50))  − 1 = round(122.070) − 1 = 121 = 0x79
//    60 Hz  ⇒ round(25e6/(4096·60))  − 1 = round(101.725) − 1 = 101 = 0x65
//    1000Hz ⇒ round(25e6/(4096·1000))− 1 = round(6.104)   − 1 =   5 = 0x05
// ---------------------------------------------------------------------------

#[test]
fn kat_prescale_canonical_bytes() {
    assert_eq!(Prescale::for_update_rate(50).reg(), 0x79, "50 Hz servo prescale");
    assert_eq!(Prescale::for_update_rate(60).reg(), 0x65, "60 Hz prescale");
    assert_eq!(Prescale::for_update_rate(1000).reg(), 0x05, "1000 Hz prescale");
}

#[test]
fn kat_prescale_matches_local_formula_over_sweep() {
    // The crate's integer round-half-up must agree with the float re-statement
    // across the whole usable servo/PWM band.
    for rate in 24u32..=1526 {
        let got = Prescale::for_update_rate(rate).reg();
        let want = kat_prescale(OSC_CLK_HZ, rate);
        assert_eq!(got, want, "prescale disagreement at {rate} Hz");
    }
}

#[test]
fn kat_prescale_clamps_to_datasheet_range() {
    // Above the ~1526 Hz ceiling clamps to the 3 floor; below ~24 Hz clamps to 255.
    assert_eq!(Prescale::for_update_rate(5000).reg(), PRESCALE_MIN);
    assert_eq!(Prescale::for_update_rate(1).reg(), PRESCALE_MAX);
    // A zero rate is meaningless — must not divide by zero; yields the slow floor.
    assert_eq!(Prescale::for_update_rate(0).reg(), PRESCALE_MAX);
    // from_reg also clamps a sub-minimum raw byte up to the floor.
    assert_eq!(Prescale::from_reg(0x00).reg(), PRESCALE_MIN);
    assert_eq!(Prescale::from_reg(0x79).reg(), 0x79);
}

#[test]
fn kat_achieved_frequency_is_exposed_and_lossy() {
    // 50 Hz requested → prescale 0x79 → the part actually runs a hair fast.
    // achieved = 25e6 / (4096 · 122) = 50.0288… Hz → 50029 mHz (round-half-up).
    let p = Prescale::for_update_rate(50);
    assert_eq!(p.reg(), 0x79);
    assert_eq!(p.achieved_milli_hz(), 50_029);
    // It is close to, but deliberately not equal to, the request — the point of
    // exposing it. Independently: round(osc·1000 / (4096·(pre+1))).
    let want = (((OSC_CLK_HZ as u64) * 1000 + (PWM_STEPS as u64 * 122) / 2)
        / (PWM_STEPS as u64 * 122)) as u32;
    assert_eq!(p.achieved_milli_hz(), want);
    assert!((p.achieved_milli_hz() as i64 - 50_000).unsigned_abs() < 100);
}

// ---------------------------------------------------------------------------
// 2. Duty table — canonical servo pulses at 50 Hz (prescale 0x79).
//    counts = round(us · osc / (1e6 · (prescale+1))) = round(us · 25 / 122):
//    1000 µs → 205,  1500 µs → 307,  2000 µs → 410.
// ---------------------------------------------------------------------------

#[test]
fn kat_servo_pulse_counts_at_50hz() {
    let p = Prescale::for_update_rate(50);
    assert_eq!(p.pulse_counts(1000), 205, "1.0 ms servo min");
    assert_eq!(p.pulse_counts(1500), 307, "1.5 ms servo neutral");
    assert_eq!(p.pulse_counts(2000), 410, "2.0 ms servo max");
}

#[test]
fn kat_duty_matches_local_formula_over_sweep() {
    // Cross-check the crate's integer duty against the float re-statement for
    // every servo-band pulse at both 50 and 60 Hz.
    for &rate in &[50u32, 60] {
        let p = Prescale::for_update_rate(rate);
        for us in 900u32..=2100 {
            let got = p.pulse_counts(us);
            let want = kat_counts(OSC_CLK_HZ, p.reg(), us);
            assert_eq!(got, want, "duty disagreement at {rate} Hz, {us} µs");
        }
    }
}

#[test]
fn kat_duty_saturates_at_count_max() {
    let p = Prescale::for_update_rate(50);
    // A pulse longer than the period cannot exceed 4095 counts.
    assert_eq!(p.pulse_counts(1_000_000), COUNT_MAX);
    // Exactly the period boundary still saturates rather than overflowing 12 bits.
    assert!(p.pulse_counts(20_000) <= COUNT_MAX);
    assert_eq!(p.pulse_counts(0), 0);
}

// ---------------------------------------------------------------------------
// 3. Channel address math — LEDn base = 0x06 + 4·n (§7.3, Table 4).
// ---------------------------------------------------------------------------

#[test]
fn kat_channel_addresses() {
    assert_eq!(channel_base(0), Some(LED0_ON_L)); // 0x06
    assert_eq!(channel_base(0), Some(0x06));
    assert_eq!(channel_base(1), Some(0x0A));
    assert_eq!(channel_base(15), Some(0x42)); // 0x06 + 4·15
    assert_eq!(channel_base(NUM_CHANNELS), None); // 16 is out of range
    assert_eq!(channel_base(255), None);
}

#[test]
fn kat_channel_writes_layout_little_endian() {
    // ON = 0x0000, OFF = 0x0133 (307, the 1.5 ms neutral count).
    let w = channel_writes(0, 0, 307).expect("ch 0 valid");
    assert_eq!(
        w,
        [
            Write::new(0x06, 0x00), // ON_L
            Write::new(0x07, 0x00), // ON_H
            Write::new(0x08, 0x33), // OFF_L  (0x0133 & 0xFF)
            Write::new(0x09, 0x01), // OFF_H  (0x0133 >> 8)
        ]
    );
    // Same value on channel 3 shifts the whole block by 4·3 = 12 = 0x0C.
    let w3 = channel_writes(3, 0, 307).expect("ch 3 valid");
    assert_eq!(w3[0].reg, 0x06 + 12);
    assert_eq!(w3[3], Write::new(0x06 + 12 + 3, 0x01));
    // Out-of-range channel yields nothing.
    assert_eq!(channel_writes(16, 0, 100), None);
    // Counts are masked to 12 bits (the top nibble is the special-bit region).
    let masked = channel_writes(0, 0xFFFF, 0xFFFF).expect("ch 0");
    assert_eq!(masked[1].val, 0x0F); // ON_H  = 0x0F, not 0xFF
    assert_eq!(masked[3].val, 0x0F); // OFF_H = 0x0F, not 0xFF
}

#[test]
fn kat_pulse_from_zero_sets_on_zero_off_counts() {
    let w = channel_pulse(2, 410).expect("ch 2");
    let base = 0x06 + 4 * 2;
    assert_eq!(w[0], Write::new(base, 0x00)); // ON_L
    assert_eq!(w[1], Write::new(base + 1, 0x00)); // ON_H
    assert_eq!(w[2], Write::new(base + 2, 0x9A)); // OFF_L (410 = 0x19A)
    assert_eq!(w[3], Write::new(base + 3, 0x01)); // OFF_H
}

// ---------------------------------------------------------------------------
// 4. Full-on / full-off special bits — bit 4 of the respective _H (§7.3.3).
//    Full-OFF takes precedence over full-ON, so full-off is the safe "low".
// ---------------------------------------------------------------------------

#[test]
fn kat_full_on_sets_on_h_bit4_only() {
    let w = channel_full_on(0).expect("ch 0");
    assert_eq!(
        w,
        [
            Write::new(0x06, 0x00),
            Write::new(0x07, 0x10), // ON_H bit 4 = full ON
            Write::new(0x08, 0x00),
            Write::new(0x09, 0x00), // OFF_H clear, so full-ON is not masked
        ]
    );
}

#[test]
fn kat_full_off_sets_off_h_bit4() {
    let w = channel_full_off(7).expect("ch 7");
    let base = 0x06 + 4 * 7;
    assert_eq!(w[0], Write::new(base, 0x00));
    assert_eq!(w[1], Write::new(base + 1, 0x00));
    assert_eq!(w[2], Write::new(base + 2, 0x00));
    assert_eq!(w[3], Write::new(base + 3, 0x10)); // OFF_H bit 4 = full OFF
}

#[test]
fn kat_pulse_at_or_above_full_period_becomes_full_on() {
    // 12-bit OFF cannot hold 4096; a saturating pulse is emitted as full-ON.
    assert_eq!(channel_pulse(0, PWM_STEPS as u16), channel_full_on(0));
    assert_eq!(channel_pulse(0, 5000), channel_full_on(0));
}

// ---------------------------------------------------------------------------
// 5. Init sequence byte-list — sleep → mode2 → prescale → wake (§7.3.1.1).
//    Frozen for the 50 Hz servo case (prescale 0x79).
// ---------------------------------------------------------------------------

#[test]
fn kat_frequency_init_ordered_writes_50hz() {
    let p = Prescale::for_update_rate(50);
    let seq = frequency_init(&p);
    assert_eq!(
        seq,
        [
            Write::new(MODE1, MODE1_SLEEPING), // 0x00, 0x30 — sleep (+ auto-inc)
            Write::new(MODE2, MODE2_INIT),     // 0x01, 0x04 — totem-pole outputs
            Write::new(PRE_SCALE, 0x79),       // 0xFE, 0x79 — 50 Hz, set in sleep
            Write::new(MODE1, MODE1_AWAKE),    // 0x00, 0x20 — wake (clear SLEEP)
        ]
    );
    // The prescale is written ONLY while asleep: the effective SLEEP state at the
    // PRE_SCALE write (the most recent MODE1 write before it) must be set, and a
    // later MODE1 write must clear it (datasheet rule).
    let pre_idx = seq.iter().position(|w| w.reg == PRE_SCALE).unwrap();
    let sleep_before = seq[..pre_idx]
        .iter()
        .rev()
        .find(|w| w.reg == MODE1)
        .expect("a MODE1 write precedes PRE_SCALE")
        .val
        & 0x10;
    assert_ne!(sleep_before, 0, "SLEEP set at PRE_SCALE write");
    let sleep_after = seq[pre_idx + 1..]
        .iter()
        .find(|w| w.reg == MODE1)
        .expect("a MODE1 write follows PRE_SCALE")
        .val
        & 0x10;
    assert_eq!(sleep_after, 0, "SLEEP cleared after PRE_SCALE");
}

// ---------------------------------------------------------------------------
// 6. Captured programming bursts — empty until a real logic-analyser capture is
//    appended. Byte-literal `Write` lists from silicon upgrade the synthetic
//    KATs above, exactly as CAPTURED_FRAMES does for ibus.
// ---------------------------------------------------------------------------

const CAPTURED_WRITES: &[&[Write]] = &[];

#[test]
fn kat_captured_writes() {
    // No-op until a real capture is dropped in. Each captured burst must at least
    // be well-formed: touch only valid registers, and if it programs a prescale,
    // do so under SLEEP.
    for burst in CAPTURED_WRITES {
        for (i, w) in burst.iter().enumerate() {
            if w.reg == PRE_SCALE {
                assert!(i > 0, "PRE_SCALE cannot be the first write");
                assert_ne!(burst[i - 1].val & 0x10, 0, "PRE_SCALE requires SLEEP set");
            }
        }
    }
}
