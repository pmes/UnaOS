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

//! MENUSTAT — **the desktop's STATUS MODEL: what the menu bar's status area states, and where it
//! got it.** One item this arc: `battery`.
//!
//! # Why this module exists at all — the bar must not read a driver
//!
//! [`super::menubar`] is furniture. It knows about rects, glyph cells, a damage signature and a
//! theme, and it is compiled on BOTH arches (x86 + `wc`, aarch64 + `desktop_firmware`). The battery
//! lives behind `drivers::smc`, which is `all(target_arch = "x86_64", feature = "smc")` — so a bar
//! that named the driver would not COMPILE on the Pi, and the obvious repair (a `cfg` fork inside
//! `compose_row`) would put a board fact in the painter, which is the shape LAWS §4 forbids in a
//! file both arches build.
//!
//! So the bar reads THIS, and this reads whatever source the board has. The status item is
//! **`battery`**; its source is [`Source`], which is `Smc` on an x86 board whose SMC answered the
//! battery keys and `None` everywhere else. No file below `video/` names a machine.
//!
//! # ⛔ The source is resolved by MEASUREMENT, not by compilation
//!
//! This is the distinction the whole module turns on, and getting it backwards would put a battery
//! meter on a machine that has no battery.
//!
//! `feature = "smc"` says *this kernel can talk to an Apple SMC*. It does NOT say *this machine has
//! a pack*. QEMU's `isa-applesmc` — which `arroyo test` attaches under `UNAOS_SMC=1` — answers
//! `REV`/`OSK0` and carries no battery key at all. A source resolved at COMPILE time would give that
//! boot a battery item with nothing behind it.
//!
//! So [`poll`] asks the board for the raw keys and resolves [`Source`] from the ANSWER:
//! `Smc` once the keys decode, `None` when the first poll finds nothing. On QEMU the item is
//! therefore the measured absence — `[menubar] battery absent src=none`, once — and on the Pi and
//! the Orin there is no source arm compiled at all and the same line is reached without a
//! transaction.
//!
//! # Cadence, and why it is 10 s and not the compositor's
//!
//! One SMC transaction costs ~200 µs on the 2012 rMBP (flight 11's scout: 493 index reads across
//! 25631→25730 ms, and the nine curated battery keys inside 25627→25629 ms). Six keys is ~1.2 ms of
//! port I/O per poll, each transaction a bounded handshake against a controller that is known to
//! drop sweeps. Nothing like that may run from `strip::compose_all`, which is masked.
//!
//! So the sweep runs on the DESKTOP SERVICE PASS ([`super::desktop_uefi::desktop_app_service`], the
//! ~1 kHz device-service body — the same worker `wm::pace_service` and `fat::probe_once` ride),
//! self-throttled to [`POLL_MS`]. The first pass after the shell's ignition finds `LAST_POLL_MS`
//! at `0` and sweeps immediately, so the item is on the glass at desktop-ready rather than ten
//! seconds later.
//!
//! What the bar reads is [`bar_item`] / [`reading`]: **relaxed atomic loads and nothing else** — no
//! lock, no port, no allocation. That is what makes it legal on the paint path, and it is why the
//! reading is kept packed in an `AtomicU64` instead of in a `Mutex<Battery>` like the driver's own
//! 1 Hz cache ([`crate::drivers::smc::battery::cached`], which takes two spin locks and therefore
//! cannot be read from a composite).
//!
//! # Holding, and the staleness ceiling
//!
//! BATMON-HOLD's rule, restated at this layer: a failed poll does NOT clobber a good reading. The
//! 2012 SMC drops individual keys, and a meter that goes blank for one sweep is worse than one that
//! holds the last number with an honest age — which is why `age_s` is on the wire.
//!
//! It is BOUNDED, though, because "hold forever" is how a removed pack keeps being reported. Past
//! [`STALE_MS`] the item goes ABSENT rather than stating a number nothing has confirmed in a
//! minute. Six poll periods: long enough that the drop-out rate flight 11 recorded
//! (`retries=0/0` on every witness line, i.e. none at all on that boot) cannot reach it, short
//! enough that a real removal is off the glass inside a minute.

use core::sync::atomic::{AtomicU64, AtomicU8, Ordering};

// ---------------------------------------------------------------------------
// The model
// ---------------------------------------------------------------------------

/// **A decoded battery reading.** Every field is a fact some key on the wire carried; there is no
/// placeholder in this struct, which is why the two fields that can legitimately be missing
/// ([`minutes`](Battery::minutes)) are `Option` and the four that cannot are not — a reading with no
/// percent, no current or no voltage is not a reading and [`decode`] answers `None` for it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Battery {
    /// Relative state of charge, percent, `0..=100` (`BRSC`). Clamped at the decode, so no later
    /// consumer has to re-check it and a fill fraction can never exceed the cell.
    pub percent: u16,
    /// Charge flowing INTO the pack. See [`decode`] for which key decides this and why.
    pub charging: bool,
    /// Minutes to full (`B0TF`), when the SMC offers it. `None` while discharging, and `None` on a
    /// machine that does not carry the key.
    pub minutes: Option<u16>,
    /// Instantaneous current, signed mA (`B0AC`): positive into the pack, negative out of it.
    pub ma: i16,
    /// Terminal voltage, mV (`B0AV`).
    pub mv: u16,
}

/// **What the PAINTER reads** — the reading reduced to the part that changes what is on the glass.
///
/// This is not a convenience type. It is the repaint discipline in the type system: the menu bar's
/// damage signature folds THIS and not [`Battery`], so a poll that moves the current by 3 mA or the
/// age by ten seconds cannot cost a repaint, while a percent or a charge-state change must. The two
/// halves of that claim are the fixture's `jitter_quiet` and `change_repaints` legs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BarItem {
    pub percent: u16,
    pub charging: bool,
}

/// Where the status item's facts come from on THIS board. Subsystem-named, never board-named: a
/// second source (an ACPI battery, a Pi UPS HAT) is a new variant and one arm in [`board_raw`], and
/// nothing above this module changes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    /// No poll has resolved the question yet — the desktop service has not run, or its first sweep
    /// has not landed. Distinct from [`Source::None`] on purpose: "not asked" and "asked, nothing
    /// there" are different facts and a witness that conflated them would report an absent battery
    /// on a boot whose service pass never ran.
    Unresolved,
    /// Measured: this board has no battery source. QEMU (an SMC with no battery keys), the Pi, the
    /// Orin.
    None,
    /// The Apple SMC answered the battery keys.
    Smc,
    /// ⛔ **A FIXTURE put this reading here, and no hardware produced it.**
    ///
    /// [`inject`] is the only writer of this variant and [`inject`] is `witness`-gated, so a metal
    /// image has no code path that can set it (the string literal is still linked, because the match
    /// arm compiles — the VARIANT is what is unreachable, and that is the claim) — but the bar's `[menubar] battery` line is UNGATED
    /// and rides a gate boot too, and on the first measured run of this arc the fixture's synthetic
    /// 82 % went out over that line reading `src=smc`. A witness stating a number no instrument
    /// measured is the one thing this tree's instrument laws exist to prevent, and "it is only a
    /// QEMU capture" is not a defence: the capture is read by people looking for facts.
    ///
    /// So the source term names the fixture. `src=fixture` on a wire is unmistakable, and it costs
    /// one variant.
    Fixture,
}

impl Source {
    /// The `src=` term on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Unresolved => "unresolved",
            Source::None => "none",
            Source::Smc => "smc",
            Source::Fixture => "fixture",
        }
    }
}

// ---------------------------------------------------------------------------
// The decode — a PURE function over the wire bytes
// ---------------------------------------------------------------------------

/// Big-endian `u16` from a two-byte SMC payload.
///
/// ⚠ **This is the injection point for the fixture's go-red**, and it is written as its own function
/// for that reason. [`super::menubar::battery_selftest`] flips [`SWAP`] and re-runs the decode over
/// the identical flight-11 bytes; a decoder whose byte order is wrong must then name a wrong
/// percent, which proves the PASS leg is reading the bytes rather than a re-typed constant. Without
/// the injection the fixture is a tautology (`a-check-that-cannot-fire`).
#[inline]
fn be16(b: [u8; 2]) -> u16 {
    #[cfg(feature = "witness")]
    if SWAP.load(Ordering::Relaxed) != 0 {
        return ((b[1] as u16) << 8) | b[0] as u16;
    }
    ((b[0] as u16) << 8) | b[1] as u16
}

/// FIXTURE-ONLY byte-order fault. `0` = the decoder as shipped. Witness-gated, so it does not exist
/// in the metal image, and it is never read by anything but [`be16`].
#[cfg(feature = "witness")]
static SWAP: AtomicU8 = AtomicU8::new(0);

/// MENUSTAT — **the whole decode, from the SMC's raw payloads to a [`Battery`].**
///
/// Pure: no port, no lock, no clock, no atomics but the fixture's injection. Both arches compile it,
/// so the byte-level proof is arch-neutral even though the only source that feeds it today is x86.
///
/// The arguments are the payloads exactly as `drivers::smc::battery::raw` returns them, which is
/// exactly what flight 11's scout printed at 25628 ms:
///
/// | key | flight 11 | decodes to |
/// |---|---|---|
/// | `BNum` | `[01]` | one battery present |
/// | `BRSC` | `[00 52]` | 0x0052 = 82 % |
/// | `B0St` | `[00 80]` | status bits, carried raw — see below |
/// | `B0AC` | `[03 f6]` | 0x03f6 = **+1014 mA** ⇒ charging |
/// | `B0AV` | `[30 17]` | 0x3017 = 12311 mV |
/// | `B0TF` | `[00 6a]` | 0x006a = 106 min to full |
///
/// (`B0FC=[26 ea]` = 9962 mAh and `B0RM=[1f e0]` = 8160 mAh are the same flight's capacity keys;
/// 8160/9962 = 81.9 %, which is the independent corroboration that `BRSC` is a plain percent and not
/// a fixed-point fraction. They feed no field here — see `smc::battery::Raw`.)
///
/// # ⛔ CHARGING COMES FROM THE `B0AC` SIGN, AND `B0St` DOES NOT DECIDE IT
///
/// The brief allowed either key and asked which, and the answer is that only one of them is
/// FALSIFIABLE from what has been flown.
///
///  * `B0AC` is a signed mA reading whose sign is **metal-confirmed on this machine**: the
///    2012 rMBP's adapter flips it, negative = the pack sourcing the machine, positive = charge
///    flowing in. That is not this arc's claim — it is IVY-AC's, recorded at
///    `drivers/smc.rs`'s `AcDerived`, and `derive_ac` has been shipping on it. Flight 11 agrees
///    across five witness lines an hour apart: `amp=1014mA … ac=derived:charging` at 25731 ms,
///    still `charging` at 928074 ms with the percentage climbing 82 → 86.
///  * `B0St` is `[00 80]` in flight 11 — and that is **one sample, in one state**. One observation
///    cannot distinguish "bit 7 means charging" from "bit 7 is always set on this controller" from
///    any other assignment; every bit hypothesis fits a single point equally well. The scout that
///    documented the key calls it `battery 0 status bits` and names no bit, because naming one would
///    have been a guess wearing a measurement's clothes.
///
/// So `B0St` is READ, carried to the wire as raw hex on the rollup line, and used for nothing. When
/// a flight records it in both states the bit becomes decidable and this comment becomes the place
/// the second sample goes.
///
/// The [`DEADBAND_MA`] is IVY-AC's too, and for its reason: the reading dithers a few mA at rest, so
/// a bare `> 0` test would flap the glyph's bolt on sensor noise. Inside the band the pack is
/// treated as NOT charging — a full pack on the adapter settles there, and a bar that showed a bolt
/// on a battery taking no current would be asserting a flow that has stopped.
pub fn decode(
    bnum: Option<u8>,
    brsc: Option<[u8; 2]>,
    b0st: Option<[u8; 2]>,
    b0ac: Option<[u8; 2]>,
    b0av: Option<[u8; 2]>,
    b0tf: Option<[u8; 2]>,
) -> Option<Battery> {
    let _ = b0st; // read and reported, never decisive — see the charging note above.
    // `BNum` is the presence key when it answers: zero packs is a machine with no battery, whatever
    // else replies. A controller that does not carry `BNum` at all is not thereby battery-less, so
    // absence falls through to the fields — which is the same shape `snapshot()`'s `B0Pr` arm takes.
    if bnum == Some(0) {
        return None;
    }
    // The three fields with no honest empty state. A reading missing any of them is not a partial
    // reading, it is not a reading: the caller HOLDS the last good one and lets its age grow
    // (BATMON-HOLD), which is strictly better than painting a meter with a hole in it.
    let percent = be16(brsc?).min(100);
    let ma = be16(b0ac?) as i16;
    let mv = be16(b0av?);
    let charging = ma > DEADBAND_MA;
    // Minutes-to-full is only a fact while charge is flowing in. A stale `B0TF` on a discharging
    // pack would read as "106 minutes until full" beside a falling percentage.
    let minutes = if charging { b0tf.map(be16) } else { None };
    Some(Battery { percent, charging, minutes, ma, mv })
}

/// Current (mA) inside which the sign carries no information. IVY-AC's constant and its argument,
/// restated at this layer rather than reached for across the arch gate: `drivers::smc` does not
/// exist on aarch64 and [`decode`] does.
const DEADBAND_MA: i16 = 32;

// ---------------------------------------------------------------------------
// The cache — lock-free, so the painter may read it
// ---------------------------------------------------------------------------

/// How often the desktop service sweeps the source. Ten seconds: a percentage moves once every few
/// minutes on a real pack (flight 11: 82 % at 25.7 s, 86 % at 928 s — four points in fifteen
/// minutes), so anything faster buys no fact and spends ~1.2 ms of port I/O to do it.
pub const POLL_MS: u64 = 10_000;

/// How stale a held reading may be before the item goes absent. Six poll periods — see the header's
/// holding note.
pub const STALE_MS: u64 = 6 * POLL_MS;

/// The last reading, PACKED, so the painter's read is one relaxed load.
///
/// `[7:0]` percent · `[8]` charging · `[9]` minutes valid · `[31:16]` minutes · `[47:32]` mv ·
/// `[63:48]` ma as raw `i16` bits. `0` is "nothing stored", which is unambiguous because a stored
/// reading always sets [`VALID`].
static READING: AtomicU64 = AtomicU64::new(0);
/// `[`READING`] holds a real reading` — `1` once one has ever landed. Separate from the packed word
/// so a genuine all-zero reading (0 %, resting, 0 mV) is not read as "never".
static VALID: AtomicU8 = AtomicU8::new(0);
/// `crate::arch::ms()` when [`READING`] was last WRITTEN — the age the wire reports.
static AT_MS: AtomicU64 = AtomicU64::new(0);
/// [`Source`] as a byte: 0 unresolved, 1 none, 2 smc.
static SRC: AtomicU8 = AtomicU8::new(0);
/// `crate::arch::ms()` of the last sweep, `0` = never. The throttle, and the reason the first
/// service pass after desktop ignition sweeps immediately.
static LAST_POLL_MS: AtomicU64 = AtomicU64::new(0);
/// Sweeps that actually ran, and how many of those the source answered. Both on the wire, so a
/// capture separates "the service never ran" from "it ran and the SMC said nothing".
static POLLS: AtomicU64 = AtomicU64::new(0);
static ANSWERS: AtomicU64 = AtomicU64::new(0);

const SRC_UNRESOLVED: u8 = 0;
const SRC_NONE: u8 = 1;
const SRC_SMC: u8 = 2;
/// See [`Source::Fixture`]. Only [`inject`] writes it, and [`inject`] is `witness`-gated.
const SRC_FIXTURE: u8 = 3;

fn pack(b: Battery) -> u64 {
    (b.percent as u64 & 0xff)
        | ((b.charging as u64) << 8)
        | ((b.minutes.is_some() as u64) << 9)
        | ((b.minutes.unwrap_or(0) as u64) << 16)
        | ((b.mv as u64) << 32)
        | ((b.ma as u16 as u64) << 48)
}

fn unpack(v: u64) -> Battery {
    Battery {
        percent: (v & 0xff) as u16,
        charging: (v >> 8) & 1 != 0,
        minutes: if (v >> 9) & 1 != 0 { Some((v >> 16) as u16) } else { None },
        mv: (v >> 32) as u16,
        ma: (v >> 48) as u16 as i16,
    }
}

/// THE BOARD'S SOURCE — the one arm in this file that names a driver, and the only one.
///
/// `drivers::smc` is `all(target_arch = "x86_64", feature = "smc")`, so the `cfg` here is that gate
/// verbatim. On every other shape the function is the second body and the board simply has no
/// battery; nothing above this line forks on an arch or a knob.
#[cfg(all(target_arch = "x86_64", feature = "smc"))]
fn board_raw() -> Option<Battery> {
    let r = crate::drivers::smc::battery::raw();
    decode(r.bnum, r.brsc, r.b0st, r.b0ac, r.b0av, r.b0tf)
}

/// No battery source compiled for this board — see the other arm.
#[cfg(not(all(target_arch = "x86_64", feature = "smc")))]
fn board_raw() -> Option<Battery> {
    None
}

/// MENUSTAT — **the poll.** Called from the desktop service pass on every pass; sweeps the source at
/// most once per [`POLL_MS`] and does nothing at all on the ~999 passes in between.
///
/// The throttle is a `compare_exchange` rather than a load-then-store because the device-service
/// body is a scheduled task that can be re-entered on another core; a loser skips, which is correct
/// — the counters are cumulative and the next window reports everything. (`strip::Ledger::tick`'s
/// own argument, and the same shape.)
///
/// ⛔ **It must not be called from a composite.** [`board_raw`] can spend six bounded SMC
/// handshakes, and `strip::compose_all` runs masked. The one call site is
/// [`super::desktop_uefi::desktop_app_service`].
pub fn poll() {
    let now = crate::arch::ms();
    let last = LAST_POLL_MS.load(Ordering::Relaxed);
    if last != 0 && now.wrapping_sub(last) < POLL_MS {
        return;
    }
    if LAST_POLL_MS
        .compare_exchange(last, now.max(1), Ordering::AcqRel, Ordering::Relaxed)
        .is_err()
    {
        return;
    }
    POLLS.fetch_add(1, Ordering::Relaxed);
    match board_raw() {
        Some(b) => {
            store(b, now, SRC_SMC);
        }
        // A sweep that answered nothing. If the source has NEVER answered, that is the measured
        // absence and it is now resolved. If it HAS, this is a drop-out and the held reading stands
        // — its age grows, and `bar_item` takes it off the glass past `STALE_MS` rather than this
        // arm deciding the pack is gone on one bad sweep (BATMON-HOLD's rule at this layer).
        None => {
            let _ = SRC.compare_exchange(
                SRC_UNRESOLVED,
                SRC_NONE,
                Ordering::AcqRel,
                Ordering::Relaxed,
            );
        }
    }
}

/// Publish a reading, stamped with the source that produced it. Split out so the fixture can drive
/// the model without an SMC — and `src` is an ARGUMENT for exactly that reason: the fixture's
/// readings must not leave this module wearing the driver's name (see [`Source::Fixture`]).
fn store(b: Battery, now: u64, src: u8) {
    READING.store(pack(b), Ordering::Relaxed);
    VALID.store(1, Ordering::Relaxed);
    AT_MS.store(now, Ordering::Relaxed);
    SRC.store(src, Ordering::Relaxed);
    ANSWERS.fetch_add(1, Ordering::Relaxed);
}

/// Where the item's facts come from, as resolved by the polls so far.
pub fn source() -> Source {
    match SRC.load(Ordering::Relaxed) {
        SRC_SMC => Source::Smc,
        SRC_FIXTURE => Source::Fixture,
        SRC_NONE => Source::None,
        _ => Source::Unresolved,
    }
}

/// **The full reading plus its age in ms**, or `None` when there is no source or the held reading
/// has gone past [`STALE_MS`]. For the wire; the painter wants [`bar_item`].
pub fn reading() -> Option<(Battery, u64)> {
    let src = SRC.load(Ordering::Relaxed);
    if VALID.load(Ordering::Relaxed) == 0 || (src != SRC_SMC && src != SRC_FIXTURE) {
        return None;
    }
    let age = crate::arch::ms().wrapping_sub(AT_MS.load(Ordering::Relaxed));
    if age > STALE_MS {
        return None;
    }
    Some((unpack(READING.load(Ordering::Relaxed)), age))
}

/// **What the painter reads** — the displayed state alone, or `None` when the item is absent.
///
/// Three relaxed loads and one clock read, no lock and no allocation, which is what makes it legal
/// from `strip::compose_all`. The age is consulted (the staleness ceiling is part of *is there an
/// item*) but never returned: an age in the damage signature would repaint the bar every second.
pub fn bar_item() -> Option<BarItem> {
    let (b, _age) = reading()?;
    Some(BarItem { percent: b.percent, charging: b.charging })
}

/// `(polls run, polls the source answered)` — for the bar's rollup line.
pub fn counts() -> (u64, u64) {
    (POLLS.load(Ordering::Relaxed), ANSWERS.load(Ordering::Relaxed))
}

// ---------------------------------------------------------------------------
// Fixture access
// ---------------------------------------------------------------------------

/// FIXTURE — publish a reading as if a poll had produced it. Witness-gated: it does not exist in the
/// metal image. The fixture that uses it restores the model to what it found.
#[cfg(feature = "witness")]
pub fn inject(b: Option<Battery>) {
    match b {
        Some(b) => store(b, crate::arch::ms(), SRC_FIXTURE),
        None => {
            VALID.store(0, Ordering::Relaxed);
            READING.store(0, Ordering::Relaxed);
            SRC.store(SRC_NONE, Ordering::Relaxed);
        }
    }
}

/// FIXTURE — the model's raw state, so a fixture can put back exactly what it found rather than a
/// state it invented.
#[cfg(feature = "witness")]
pub fn snapshot_state() -> (u64, u8, u64, u8) {
    (
        READING.load(Ordering::Relaxed),
        VALID.load(Ordering::Relaxed),
        AT_MS.load(Ordering::Relaxed),
        SRC.load(Ordering::Relaxed),
    )
}

/// FIXTURE — restore what [`snapshot_state`] read.
#[cfg(feature = "witness")]
pub fn restore_state(s: (u64, u8, u64, u8)) {
    READING.store(s.0, Ordering::Relaxed);
    VALID.store(s.1, Ordering::Relaxed);
    AT_MS.store(s.2, Ordering::Relaxed);
    SRC.store(s.3, Ordering::Relaxed);
}

/// FIXTURE — arm/disarm the decoder's byte-order fault. See [`be16`].
#[cfg(feature = "witness")]
pub fn set_byte_swap(on: bool) {
    SWAP.store(on as u8, Ordering::Relaxed);
}
