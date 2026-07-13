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
//
// You should have received a copy of the GNU Lesser General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! PCA9685 16-channel, 12-bit PWM controller — the actuation-output codec.
//!
//! ENDURO (ROADMAP §4) drives the rover's servo and ESC through a **PCA9685
//! over I2C**: Tegra's native PWM is 8-bit at 50 Hz, insufficient for a servo,
//! so a dedicated 12-bit PWM controller carries the actuation. The PCA9685's
//! I2C register map and prescale math are fully datasheet-specified, so — like
//! the i-BUS wire format in [`ibus`](../ibus) — the register encoding is frozen
//! here with datasheet known-answer tests on the host, long before any I2C wire
//! or 3S LiPo is involved. The future kernel I2C driver emits exactly the
//! `(reg, val)` writes this codec computes.
//!
//! # What this crate is (and is not)
//!
//! This is a **pure codec**: it turns update rates and microsecond pulse widths
//! into the concrete PCA9685 register bytes. It computes bytes; it does **not**
//! move them. There is deliberately **no I2C transport** here — that is a later
//! kernel arc. The `drive` sink speaks microseconds and nothing else; this codec
//! is the µs → 12-bit-duty translator that sits behind it.
//!
//! # Design constraints
//!
//! - `#![no_std]`, **zero runtime dependencies**, `#![forbid(unsafe_code)]`. No
//!   allocation: every helper returns a fixed-size array of [`Write`]s.
//! - All arithmetic is integer. The achieved frequency is lossy (the prescale is
//!   an integer divider), so the API always lets a caller read the **real**
//!   update rate it will get, never just the requested one.
//!
//! # The datasheet contract (frozen by the KATs in `tests/kat_vectors.rs`)
//!
//! - **Prescale (§7.3.5):** `prescale = round(osc_clk / (4096 · update_rate)) − 1`,
//!   internal oscillator [`OSC_CLK_HZ`] = 25 MHz, clamped to
//!   [`PRESCALE_MIN`]..=[`PRESCALE_MAX`] (≈24 Hz..1526 Hz). 50 Hz ⇒ `0x79`.
//! - **Duty (§7.3.3):** a channel's 12-bit ON/OFF counts live at
//!   `LED{n}_ON_L/H`, `LED{n}_OFF_L/H` = `0x06 + 4·n`. A pulse of `c` counts is
//!   `ON = 0`, `OFF = c`. The full-ON / full-OFF special bits are bit 4 of the
//!   respective `_H` register.
//! - **Sequencing (§7.3.1.1):** `PRE_SCALE` (`0xFE`) is writable only while the
//!   `SLEEP` bit of `MODE1` is set. [`frequency_init`] emits the correct ordered
//!   write list (sleep → prescale → wake); [`RESTART_WRITE`] is the optional
//!   post-oscillator-settle restart.

#![no_std]
#![forbid(unsafe_code)]

// ---------------------------------------------------------------------------
// Register map (PCA9685 datasheet §7.3, Table 4 "Register definitions").
// ---------------------------------------------------------------------------

/// `MODE1` register (`0x00`): sleep, restart, auto-increment, sub-address bits.
pub const MODE1: u8 = 0x00;
/// `MODE2` register (`0x01`): output driver configuration.
pub const MODE2: u8 = 0x01;
/// `LED0_ON_L` (`0x06`): base of the 16 per-channel register blocks.
pub const LED0_ON_L: u8 = 0x06;
/// `ALL_LED_ON_L` (`0xFA`): the broadcast channel block (write all 16 at once).
pub const ALL_LED_ON_L: u8 = 0xFA;
/// `PRE_SCALE` register (`0xFE`): the frequency prescaler; writable only in sleep.
pub const PRE_SCALE: u8 = 0xFE;

// MODE1 bits.
/// `MODE1` RESTART bit (`0x80`): write 1 (after the oscillator settles) to
/// restart previously-stopped PWM outputs; it self-clears.
pub const MODE1_RESTART: u8 = 0x80;
/// `MODE1` AI bit (`0x20`): register auto-increment, so a 4-byte channel block
/// can be written in one I2C burst. This codec's write lists assume it is set.
pub const MODE1_AI: u8 = 0x20;
/// `MODE1` SLEEP bit (`0x10`): low-power sleep. `PRE_SCALE` is writable only
/// while this is set.
pub const MODE1_SLEEP: u8 = 0x10;

// MODE2 bits.
/// `MODE2` OUTDRV bit (`0x04`): outputs configured as totem-pole (vs open-drain).
/// This is the power-on default and the ENDURO configuration.
pub const MODE2_OUTDRV: u8 = 0x04;

/// `MODE1` value used awake: auto-increment on, sleep clear.
pub const MODE1_AWAKE: u8 = MODE1_AI;
/// `MODE1` value used while programming the prescale: auto-increment on, asleep.
pub const MODE1_SLEEPING: u8 = MODE1_AI | MODE1_SLEEP;
/// `MODE2` value written at init: totem-pole outputs.
pub const MODE2_INIT: u8 = MODE2_OUTDRV;

// ---------------------------------------------------------------------------
// Constants of the part.
// ---------------------------------------------------------------------------

/// Number of independent PWM channels.
pub const NUM_CHANNELS: u8 = 16;
/// PWM resolution: counts per period. The ON/OFF counters are 12-bit (0..4096).
pub const PWM_STEPS: u32 = 4096;
/// Maximum valid 12-bit count. Counts saturate here; `4096` means "full on".
pub const COUNT_MAX: u16 = 4095;

/// The PCA9685 internal oscillator, 25 MHz (datasheet §7.3.5). The external-clock
/// path (EXTCLK) is out of scope for ENDURO; the codec assumes the internal clock.
pub const OSC_CLK_HZ: u32 = 25_000_000;

/// Minimum `PRE_SCALE` value (datasheet §7.3.5): 3, i.e. the ~1526 Hz ceiling.
pub const PRESCALE_MIN: u8 = 0x03;
/// Maximum `PRE_SCALE` value: 255, i.e. the ~24 Hz floor.
pub const PRESCALE_MAX: u8 = 0xFF;

/// Oscillator stabilisation delay after clearing `SLEEP`, microseconds
/// (datasheet §7.3.1.1: ≥500 µs before the outputs — and any [`RESTART_WRITE`] —
/// are valid). The codec emits no delays; the driver honours this between the
/// wake write of [`frequency_init`] and [`RESTART_WRITE`].
pub const OSC_STABILIZE_US: u32 = 500;

/// A single I2C register write: `val` to register `reg`. The codec's whole
/// output is sequences of these; the transport is someone else's job.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Write {
    /// Target register address.
    pub reg: u8,
    /// Byte to write.
    pub val: u8,
}

impl Write {
    /// Construct a write.
    #[inline]
    pub const fn new(reg: u8, val: u8) -> Self {
        Self { reg, val }
    }
}

// ---------------------------------------------------------------------------
// Prescale / frequency.
// ---------------------------------------------------------------------------

/// A computed `PRE_SCALE` value bound to the oscillator it was derived from.
///
/// Because the prescaler is an integer divider, the achieved update rate is
/// almost never exactly the requested one; [`achieved_milli_hz`](Self::achieved_milli_hz)
/// reports what the part will actually run at. Duty counts
/// ([`pulse_counts`](Self::pulse_counts)) are computed against that achieved
/// rate, so a microsecond pulse maps to the physically correct width.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Prescale {
    reg: u8,
    osc_hz: u32,
}

impl Prescale {
    /// Build directly from a raw `PRE_SCALE` byte (clamped to the valid range),
    /// assuming the internal [`OSC_CLK_HZ`] oscillator.
    #[inline]
    pub const fn from_reg(reg: u8) -> Self {
        Self::from_reg_with_osc(reg, OSC_CLK_HZ)
    }

    /// Build from a raw `PRE_SCALE` byte and an explicit oscillator frequency.
    /// The byte is clamped to [`PRESCALE_MIN`]..=[`PRESCALE_MAX`].
    #[inline]
    pub const fn from_reg_with_osc(reg: u8, osc_hz: u32) -> Self {
        let reg = if reg < PRESCALE_MIN {
            PRESCALE_MIN
        } else {
            reg
        };
        Self { reg, osc_hz }
    }

    /// Compute the prescale for a target update rate (Hz) on the internal
    /// oscillator: `round(osc / (4096 · rate)) − 1`, clamped to the valid range.
    ///
    /// A `rate` of 0 is meaningless and yields the slowest supported rate
    /// ([`PRESCALE_MAX`]) rather than dividing by zero.
    #[inline]
    pub fn for_update_rate(update_hz: u32) -> Self {
        Self::for_update_rate_with_osc(update_hz, OSC_CLK_HZ)
    }

    /// As [`for_update_rate`](Self::for_update_rate) but with an explicit
    /// oscillator frequency (for a part driven from EXTCLK, or for KATs).
    pub fn for_update_rate_with_osc(update_hz: u32, osc_hz: u32) -> Self {
        if update_hz == 0 {
            return Self {
                reg: PRESCALE_MAX,
                osc_hz,
            };
        }
        // prescale + 1 = round(osc / (4096 · rate)), round-half-up in integers.
        let denom = PWM_STEPS as u64 * update_hz as u64;
        let plus_one = (osc_hz as u64 + denom / 2) / denom;
        // Datasheet: PRE_SCALE = (that) − 1, register range [3, 255].
        let reg = plus_one.saturating_sub(1);
        let reg = reg.clamp(PRESCALE_MIN as u64, PRESCALE_MAX as u64) as u8;
        Self { reg, osc_hz }
    }

    /// The raw `PRE_SCALE` register byte.
    #[inline]
    pub const fn reg(&self) -> u8 {
        self.reg
    }

    /// The oscillator frequency this prescale was computed against.
    #[inline]
    pub const fn osc_hz(&self) -> u32 {
        self.osc_hz
    }

    /// The **achieved** update rate in milli-hertz (Hz × 1000):
    /// `round(osc · 1000 / (4096 · (prescale + 1)))`. Milli-hertz keeps the
    /// result exact in integers; divide by 1000.0 for Hz.
    pub fn achieved_milli_hz(&self) -> u32 {
        let denom = PWM_STEPS as u64 * (self.reg as u64 + 1);
        let num = self.osc_hz as u64 * 1000;
        ((num + denom / 2) / denom) as u32
    }

    /// The number of 12-bit counts for a `us`-microsecond high pulse at the
    /// achieved rate: `round(us · osc / (1e6 · (prescale + 1)))`, saturating to
    /// [`COUNT_MAX`]. (Algebraically `round(us · 4096 · achieved_freq / 1e6)`,
    /// but computed straight from the prescale to stay exact and integer.)
    pub fn pulse_counts(&self, us: u32) -> u16 {
        let denom = 1_000_000u64 * (self.reg as u64 + 1);
        let num = us as u64 * self.osc_hz as u64;
        let counts = (num + denom / 2) / denom;
        if counts > COUNT_MAX as u64 {
            COUNT_MAX
        } else {
            counts as u16
        }
    }
}

// ---------------------------------------------------------------------------
// Channels / duty.
// ---------------------------------------------------------------------------

/// The `LEDn_ON_L` base register for channel `ch` (`0x06 + 4·ch`), or `None`
/// when `ch >= `[`NUM_CHANNELS`]. The four bytes at `base..base+4` are
/// `ON_L, ON_H, OFF_L, OFF_H`.
#[inline]
pub fn channel_base(ch: u8) -> Option<u8> {
    if ch < NUM_CHANNELS {
        Some(LED0_ON_L + 4 * ch)
    } else {
        None
    }
}

/// Build the four register writes (`ON_L, ON_H, OFF_L, OFF_H`) that set channel
/// `ch` to raw 12-bit ON/OFF counters. `on`/`off` are masked to 12 bits. Returns
/// `None` for an out-of-range channel.
///
/// This is the general form; most callers want [`channel_pulse`] (a high pulse
/// from count 0) or the [`channel_full_on`] / [`channel_full_off`] specials.
pub fn channel_writes(ch: u8, on: u16, off: u16) -> Option<[Write; 4]> {
    let base = channel_base(ch)?;
    let on = on & 0x0FFF;
    let off = off & 0x0FFF;
    Some([
        Write::new(base, (on & 0xFF) as u8),
        Write::new(base + 1, (on >> 8) as u8),
        Write::new(base + 2, (off & 0xFF) as u8),
        Write::new(base + 3, (off >> 8) as u8),
    ])
}

/// Set channel `ch` to a high pulse of `counts` 12-bit steps starting at count 0
/// (`ON = 0`, `OFF = counts`). `counts` saturates at [`COUNT_MAX`]; a value of
/// `4096` or above is emitted as the [`channel_full_on`] special instead (there
/// is no `OFF = 4096` in 12 bits). Returns `None` for an out-of-range channel.
pub fn channel_pulse(ch: u8, counts: u16) -> Option<[Write; 4]> {
    if counts as u32 >= PWM_STEPS {
        return channel_full_on(ch);
    }
    channel_writes(ch, 0, counts)
}

/// Set channel `ch` to a high pulse of `us` microseconds at `prescale`'s achieved
/// rate (via [`Prescale::pulse_counts`]). Returns `None` for an out-of-range
/// channel. This is the ENDURO µs → duty path the `drive` sink feeds.
pub fn channel_pulse_us(ch: u8, prescale: &Prescale, us: u32) -> Option<[Write; 4]> {
    channel_pulse(ch, prescale.pulse_counts(us))
}

/// Drive channel `ch` **fully on** using the datasheet full-ON special bit
/// (bit 4 of `LEDn_ON_H`); `OFF` is cleared so full-ON is not masked by full-OFF.
/// Returns `None` for an out-of-range channel.
pub fn channel_full_on(ch: u8) -> Option<[Write; 4]> {
    let base = channel_base(ch)?;
    Some([
        Write::new(base, 0x00),
        Write::new(base + 1, 0x10), // LEDn_ON_H bit 4 = full ON
        Write::new(base + 2, 0x00),
        Write::new(base + 3, 0x00),
    ])
}

/// Drive channel `ch` **fully off** using the datasheet full-OFF special bit
/// (bit 4 of `LEDn_OFF_H`). Full-OFF takes precedence over full-ON, so this is
/// the safe "output low" encoding. Returns `None` for an out-of-range channel.
pub fn channel_full_off(ch: u8) -> Option<[Write; 4]> {
    let base = channel_base(ch)?;
    Some([
        Write::new(base, 0x00),
        Write::new(base + 1, 0x00),
        Write::new(base + 2, 0x00),
        Write::new(base + 3, 0x10), // LEDn_OFF_H bit 4 = full OFF
    ])
}

// ---------------------------------------------------------------------------
// Init / sequencing.
// ---------------------------------------------------------------------------

/// The ordered register writes that put the part at `prescale` and wake it
/// (datasheet §7.3.1.1). In order:
///
/// 1. `(MODE1, `[`MODE1_SLEEPING`]`)` — enter sleep so `PRE_SCALE` is writable.
/// 2. `(MODE2, `[`MODE2_INIT`]`)` — totem-pole outputs (writable in any state).
/// 3. `(PRE_SCALE, prescale)` — set the frequency (legal only while asleep).
/// 4. `(MODE1, `[`MODE1_AWAKE`]`)` — clear `SLEEP`; the oscillator starts.
///
/// After write 4 the caller must wait [`OSC_STABILIZE_US`] before the outputs
/// are valid; only then may it optionally issue [`RESTART_WRITE`]. The codec
/// emits no delays — timing belongs to the transport.
pub fn frequency_init(prescale: &Prescale) -> [Write; 4] {
    [
        Write::new(MODE1, MODE1_SLEEPING),
        Write::new(MODE2, MODE2_INIT),
        Write::new(PRE_SCALE, prescale.reg()),
        Write::new(MODE1, MODE1_AWAKE),
    ]
}

/// The optional restart write, issued **after** [`OSC_STABILIZE_US`] has elapsed
/// since the wake write of [`frequency_init`]: `(MODE1, AI | RESTART)`. Restarts
/// any PWM channels that were stopped by a prior sleep; the RESTART bit
/// self-clears. Fresh power-on init does not require it.
pub const RESTART_WRITE: Write = Write::new(MODE1, MODE1_AWAKE | MODE1_RESTART);
