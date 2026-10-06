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
//!
//! # MENUBATT2 — ⛔ THE POLL MUST STATE THAT IT RAN, BECAUSE SILENCE HERE HAS ALREADY BEEN READ AS A MEASUREMENT
//!
//! This is the rule this module was missing, and its absence produced a wrong finding in a ledger
//! row rather than a wrong pixel on a screen.
//!
//! [`super::menubar`]'s `battery_witness` is the only thing that has ever spoken for this model, and
//! it says NOTHING while [`Source::Unresolved`] — correctly, on its own terms: a line claiming
//! absence before anything asked would be the opposite defect. But that makes THREE different facts
//! print the same nothing:
//!
//!  1. the desktop service pass never ran, so [`poll`] was never called;
//!  2. [`poll`] ran and the source has not resolved;
//!  3. **this code is not in the image at all.**
//!
//! MENUFIRST (rmbp-ledger B156) read flight 11's 2.2 MB for a `[menubar] battery` line, found none,
//! and concluded (2) — *"on an rMBP with a pack the status source never resolved for the whole
//! boot"*. The answer is (3). Flight 11's image is **`56bbe53b`** (2026-09-22 08:12:41;
//! `docs/dev/evidence/rmbp-0922/flight11/`, "image 3"), and `video/status.rs` **does not exist in
//! that commit** — nor does `battery_witness`, nor the poll call in `desktop_uefi`. MENUSTAT folded
//! at `0fb0f4fb`, 2026-09-22 15:42, seven and a half hours later. No poll could have run on that
//! boot, and B148's own status cell already said so: *"the METAL half (a pack that actually answers)
//! is flight 12's"*.
//!
//! So [`poll`] now speaks for ITSELF, from inside the sweep, over `[status] poll`: once on every
//! change of the resolved source — which includes the first sweep of the boot, since [`WIRE_SRC`]
//! starts on a value no source takes — and once per [`WIRE_MS`] thereafter. A capture with no
//! `[status] poll` line is now a capture where the sweep did not run, which is a different sentence
//! from the bar's silence and is falsifiable against `[wc-x] desktop-app` in the same function.
//!
//! ⚠ **What this instrument does NOT buy, stated so the next reader does not over-trust it.** No
//! witness added here could have been in flight 11's image, because the FILE was not. The general
//! cure for that class of error is not an instrument: it is reading a capture against the COMMIT its
//! image was built from before inferring a code path's behaviour from a line's absence.
//!
//! The line is UNGATED inside the furniture gate — no `witness` term — for `battery_witness`'s
//! reason: Peter's captures come off metal and the metal image is built without `witness`. It costs
//! one relaxed load and one compare per SWEEP (~6 per minute), and nothing at all on the ~999
//! service passes a second that return at the throttle without reaching the body.

use core::sync::atomic::{AtomicU64, AtomicU8, Ordering};

use super::strip;

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
/// MENUBATT2 — SWEEPS that answered, which is why only [`poll`] increments it and [`store`] (the
/// fixture's writer too) does not. See the note at that increment.
static ANSWERS: AtomicU64 = AtomicU64::new(0);

/// MENUBATT2 — how long `[status] poll` stays quiet while the resolved source is NOT changing. One
/// minute: six sweeps, the same span [`STALE_MS`] gives a held reading, so a capture read at any
/// point carries at least one statement of the poll's liveness from within the staleness window
/// and a boot cannot go a minute without saying whether the sweep is still running. A CHANGE
/// speaks immediately and does not wait for this.
pub const WIRE_MS: u64 = 60_000;

/// `crate::arch::ms()` of the last `[status] poll` line, `0` = never said.
static WIRE_LAST_MS: AtomicU64 = AtomicU64::new(0);
/// The source byte the last `[status] poll` line REPORTED, as distinct from [`SRC`], which is the
/// source itself. Starts at [`SRC_UNSAID`] — a value no source takes — so the first sweep of the
/// boot is a change and always speaks, whatever it resolved to.
static WIRE_SRC: AtomicU8 = AtomicU8::new(SRC_UNSAID);

const SRC_UNRESOLVED: u8 = 0;
const SRC_NONE: u8 = 1;
const SRC_SMC: u8 = 2;
/// See [`Source::Fixture`]. Only [`inject`] writes it, and [`inject`] is `witness`-gated.
const SRC_FIXTURE: u8 = 3;
/// Not a source: the "no `[status] poll` line has been emitted yet" state of [`WIRE_SRC`]. It must
/// stay outside the `SRC_*` range or the first sweep would be able to read as "unchanged".
const SRC_UNSAID: u8 = 0xff;

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
    if !crate::boot::services_gate("status") {
        return; // INSTALLBARE (R86): the bar's status poll is furniture's — nothing polls the SMC under the setter / the login screen
    }
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
    // MENUBATT2 — the sweep's own COST, measured around exactly the six handshakes and nothing else.
    // `now_cycles`/`strip::cycles_to_us` and not `arch::ms()`: the whole sweep is ~1.2 ms on the 2012
    // rMBP, so a millisecond clock would report a two-valued number and a reader could not tell a
    // clean sweep from one that spent its retry budget. This is the term that makes the poll's
    // placement argument (WEDGE-8, the render-core rule) falsifiable on metal instead of asserted
    // from a scout's arithmetic.
    let t0 = crate::arch::now_cycles();
    match board_raw() {
        Some(b) => {
            // MENUBATT2 — ⛔ [`ANSWERS`] IS COUNTED **HERE** AND NOT IN [`store`], AND THE FIRST
            // MEASURED RUN OF THIS ARC IS WHY. It used to sit in `store`, which [`inject`] also
            // calls, so the gate boot's own capture read `[status] poll n=7 answered=3 src=none`:
            // three MENUBATT injections wearing the name of an SMC that had answered nothing. That
            // is not a QEMU-only cosmetic — B156 records that **the flown images are witness
            // builds**, so `inject` exists on metal and the fixture runs there, and flight 12's
            // `answered=` would have carried the same three. A witness whose count can be inflated
            // by a fixture is the `Source::Fixture` lesson one field over, and it is caught the
            // same way: the term names SWEEPS THAT ANSWERED, so only a sweep may increment it.
            ANSWERS.fetch_add(1, Ordering::Relaxed);
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
    let took_us = strip::cycles_to_us(crate::arch::now_cycles().saturating_sub(t0));
    poll_witness(now, took_us);
}

/// MENUBATT2 — **the poll's statement that it RAN**, and the module header's whole argument in one
/// function. See that header for why silence here was not neutral.
///
/// `[status] poll n=4 answered=4 src=smc took_us=1180`
///
///  * `n=` / `answered=` are [`counts`] — sweeps that RAN, and how many the source answered. Two
///    numbers rather than one because `n>0 answered=0` (the source is there and says nothing) and
///    `n=0` (nothing swept) are the two halves MENUFIRST could not separate, and neither is the
///    third case, NO LINE AT ALL, which is now what "the sweep did not run" looks like.
///  * `src=` is the resolved source AT THIS SWEEP. It can never read `unresolved` on a line this
///    function emits — every sweep resolves — which is why the spec FORBIDs that spelling: it would
///    mean the resolve arms stopped covering the match.
///  * `took_us=` is the six handshakes' measured cost, not the scout's ~1.2 ms estimate.
///
/// No lock and no allocation, and it is called from the throttled body, so the ~999 service passes
/// a second between sweeps never reach it. Two cores cannot both be here for one window: the
/// caller's `compare_exchange` on [`LAST_POLL_MS`] has already made the loser return.
fn poll_witness(now: u64, took_us: u64) {
    let src = SRC.load(Ordering::Relaxed);
    let last = WIRE_LAST_MS.load(Ordering::Relaxed);
    // The swap is the change test AND the record of it, in one operation, so a source that moves
    // twice inside one [`WIRE_MS`] window cannot lose the second edge to a read-then-write race.
    let changed = WIRE_SRC.swap(src, Ordering::Relaxed) != src;
    if !changed && last != 0 && now.wrapping_sub(last) < WIRE_MS {
        return;
    }
    WIRE_LAST_MS.store(now.max(1), Ordering::Relaxed);
    let (polls, answers) = counts();
    serial_println!(
        "[status] poll n={} answered={} src={} took_us={}",
        polls,
        answers,
        src_of(src).as_str(),
        took_us
    );
}

/// Publish a reading, stamped with the source that produced it. Split out so the fixture can drive
/// the model without an SMC — and `src` is an ARGUMENT for exactly that reason: the fixture's
/// readings must not leave this module wearing the driver's name (see [`Source::Fixture`]).
/// MENUBATT2 — it does NOT touch [`ANSWERS`]. Publishing a reading and a SWEEP ANSWERING are two
/// different events, and this function serves both writers; see the counter's note in [`poll`].
fn store(b: Battery, now: u64, src: u8) {
    READING.store(pack(b), Ordering::Relaxed);
    VALID.store(1, Ordering::Relaxed);
    AT_MS.store(now, Ordering::Relaxed);
    SRC.store(src, Ordering::Relaxed);
}

/// The stored byte as a [`Source`]. Split out of [`source`] so [`poll_witness`] can name the source
/// of the sweep it just ran from the byte it already loaded, rather than re-reading [`SRC`] and
/// risking a line whose `src=` term belongs to a different sweep than its `took_us=`.
fn src_of(b: u8) -> Source {
    match b {
        SRC_SMC => Source::Smc,
        SRC_FIXTURE => Source::Fixture,
        SRC_NONE => Source::None,
        _ => Source::Unresolved,
    }
}

/// Where the item's facts come from, as resolved by the polls so far.
pub fn source() -> Source {
    src_of(SRC.load(Ordering::Relaxed))
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

/// `(polls run, polls the source ANSWERED)` — for the bar's rollup line and for `[status] poll`.
///
/// MENUBATT2: the second term counts sweeps, never fixture injections, so `n>0 answered=0` is the
/// honest reading of a gate boot whose `isa-applesmc` carries no battery key even while MENUBATT's
/// `inject` has driven a reading through the model three times.
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

// --- BEZEL (B405): the transient readouts moved to the glass --------------------------------------------
//
// The bar's `BRT nn/16` item and the caption's `vol=N/16` / `muted` overlay are RETIRED: the brightness and
// volume keys show `video::bezel` (centre-bottom, 1.5 s), whose `[bezel] show` line carries the values. What
// stays here is the volume MODEL (F10 / F11 / F12 — HID usages 0x43 / 0x44 / 0x45 in the boot report, the
// rMBP keyboard sends the fn-row as plain F-keys — are mute / down / up; a boot-static `level` 0..=16 and a
// mute flag, written to the codec's output amps through `drivers::hda::vol::apply`).

static VOL_LEVEL: AtomicU8 = AtomicU8::new(12);
static VOL_MUTED: AtomicU8 = AtomicU8::new(0);

/// The key's indicator: the bezel is armed or showing (`false` on a build with no desktop).
pub fn bezel_indicator() -> bool {
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    {
        return crate::video::bezel::indicator() != crate::video::bezel::K_NONE;
    }
    #[allow(unreachable_code)]
    false
}

/// Drop the bezel a fixture's key armed (a test's key must not flash on the glass).
pub fn bezel_disarm() {
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    crate::video::bezel::arm(crate::video::bezel::K_NONE);
}

/// `(level, muted)`.
pub fn volume() -> (u8, bool) {
    (VOL_LEVEL.load(Ordering::Relaxed), VOL_MUTED.load(Ordering::Relaxed) != 0)
}

/// Decoder seam: a key usage that just went DOWN. Returns `Some((key, amp_written))` when it was a
/// volume key (0 up / 1 down / 2 mute). Runs in the polled HID service, never an interrupt.
pub fn volkey_usage(usage: u8) -> Option<(u8, bool)> {
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    if (0x40..=0x42).contains(&usage) {
        crate::video::player::media_key(usage - 0x40); // PLAYER (B419): F7/F8/F9 = previous / play-pause / next, latched for the open player
    }
    let key = match usage {
        0x45 => 0u8,
        0x44 => 1,
        0x43 => 2,
        _ => return None,
    };
    let (mut lv, mut muted) = volume();
    match key {
        0 => {
            lv = (lv + 1).min(16);
            muted = false;
        }
        1 => {
            lv = lv.saturating_sub(1);
            muted = false;
        }
        _ => muted = !muted,
    }
    VOL_LEVEL.store(lv, Ordering::Relaxed);
    VOL_MUTED.store(muted as u8, Ordering::Relaxed);
    #[cfg(all(target_arch = "x86_64", feature = "hda"))]
    let written = crate::drivers::hda::vol::apply(lv, muted) > 0;
    #[cfg(not(all(target_arch = "x86_64", feature = "hda")))]
    let written = false;
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    crate::video::bezel::arm_volume(muted); // BEZEL (B405): the next desktop pass shows the amp's level
    Some((key, written))
}

/// `:: VOLKEYS: … ::` — drives up, down, mute through the SAME seam the decoder calls, then restores the
/// level and mute and clears the indicator. PASS = state moved as the key says, the indicator is
/// showing, and the amp verb was issued (or no HDA path exists yet: the probe is deferred past the root
/// pass and QEMU may not carry a codec, so `amp_written=0` there is a fact, not a failure).
#[cfg(feature = "witness")]
pub fn volkeys_selftest() {
    let (l0, m0) = volume();
    VOL_LEVEL.store(12, Ordering::Relaxed);
    VOL_MUTED.store(0, Ordering::Relaxed);
    #[cfg(all(target_arch = "x86_64", feature = "hda"))]
    let ready = crate::drivers::hda::vol::ready();
    #[cfg(not(all(target_arch = "x86_64", feature = "hda")))]
    let ready = false;
    for (usage, name) in [(0x45u8, "up"), (0x44, "down"), (0x43, "mute")] {
        bezel_disarm();
        let r = volkey_usage(usage);
        let (lv, muted) = volume();
        let shown = bezel_indicator();
        let (key_ok, written) = match r {
            Some((_, w)) => (true, w),
            None => (false, false),
        };
        let state_ok = match usage {
            0x45 => lv == 13 && !muted,
            0x44 => lv == 12 && !muted,
            _ => lv == 12 && muted,
        };
        let ok = key_ok && state_ok && shown && (written || !ready);
        serial_println!(
            ":: VOLKEYS: key={} level={}/16 muted={} amp_written={} indicator={} -> {} ::",
            name, lv, muted as u8, written as u8, shown as u8, if ok { "PASS" } else { "FAIL" }
        );
    }
    VOL_LEVEL.store(l0, Ordering::Relaxed);
    VOL_MUTED.store(m0 as u8, Ordering::Relaxed);
    #[cfg(all(target_arch = "x86_64", feature = "hda"))]
    let _ = crate::drivers::hda::vol::apply(l0, m0);
    bezel_disarm();
}

/// SETTINGS (R75): set the volume level (`0..=16`) and mute directly (a slider and a toggle), the same
/// store + amp write + indicator `volkey_usage` does. Returns whether an amp verb was issued.
pub fn set_volume(level: u8, muted: bool) -> bool {
    let lv = level.min(16);
    VOL_LEVEL.store(lv, Ordering::Relaxed);
    VOL_MUTED.store(muted as u8, Ordering::Relaxed);
    #[cfg(all(target_arch = "x86_64", feature = "hda"))]
    let written = crate::drivers::hda::vol::apply(lv, muted) > 0;
    #[cfg(not(all(target_arch = "x86_64", feature = "hda")))]
    let written = false;
    written
}

/// TESTFIX3 — true while the reading in force is a FIXTURE's (`inject`), not the SMC's. The live
/// low-battery monitor (`powerui::lowbat_service`) must not act on it: FLIGHT 19's `tests power`
/// injected 7 % for its panel leg, the device-service pass read it as the battery, posted a REAL
/// "Low Battery" alert (the login screen's modal Alert state), and that modal then swallowed every
/// press for the rest of the run (`[login] press … swallowed=2` -> `[clickroute] … deliver=false`).
pub fn reading_is_fixture() -> bool {
    SRC.load(Ordering::Relaxed) == SRC_FIXTURE
}


// ═══════════════════════════════════════════════════════════════════════════════════════════════════
// STATUSTRAY (rmbp-ledger B426, MACPARITY row 3) — THE STATUS AREA'S OTHER ITEMS: input, network, volume, clock
// ═══════════════════════════════════════════════════════════════════════════════════════════════════
//
// MENUSTAT's rule, extended: the bar reads THIS MODEL, never a driver. The owners' facts (the codec's captured
// amps, the dongle's PHY link, the radio's core, the lease, the keymap's table, the civil clock's anchor) are
// read ONCE per desktop service pass by [`tray_publish`] — the same pass and the same gate as [`poll`] — and
// stored here as relaxed atomics; the painter's [`tray`] is loads only. An item whose fact is UNKNOWN is `None`
// and is not drawn (the clock's honest-or-absent rule generalised); the bar keeps its slots.

/// The tray's items, LEFT to right on the bar. `ITEM_BATTERY` is MENUSTAT's item; it sits in this list so the
/// menu module names every status item one way.
pub const ITEM_INPUT: u8 = 0;
pub const ITEM_NET: u8 = 1;
pub const ITEM_VOLUME: u8 = 2;
pub const ITEM_BATTERY: u8 = 3;
pub const ITEM_CLOCK: u8 = 4;
/// How many status items the model carries (the witness's `items=`).
pub const ITEMS: u8 = 5;
/// No item (a menu that is closed).
pub const ITEM_NONE: u8 = 0xff;

/// The item's wire name.
pub fn item_name(i: u8) -> &'static str {
    match i {
        ITEM_INPUT => "input",
        ITEM_NET => "net",
        ITEM_VOLUME => "volume",
        ITEM_BATTERY => "battery",
        ITEM_CLOCK => "clock",
        _ => "none",
    }
}

/// The network medium.
pub const NET_USB: u8 = 1;
pub const NET_WIFI: u8 = 2;

/// The network item's facts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Net {
    /// [`NET_USB`] (the USB Ethernet dongle) or [`NET_WIFI`] (the radio).
    pub medium: u8,
    /// The PHY link (USB) / an association (Wi-Fi — none is measured yet, so `false`).
    pub up: bool,
    /// The leased address, when there is one.
    pub ip: Option<[u8; 4]>,
}

/// **What the bar and the menus read** — one snapshot of the tray's facts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Tray {
    /// `(level 0..=16, muted)` once an output amp exists to drive; `None` with no sink.
    pub volume: Option<(u8, bool)>,
    /// `None` with no network device at all.
    pub net: Option<Net>,
    /// The input item's one fact: the PC table (Command role on Alt) is selected.
    pub pc: bool,
    /// The civil clock is anchored (SNTP, RTC or `date -s`).
    pub clock: bool,
}

static TRAY_VOL: AtomicU8 = AtomicU8::new(0); // 1 = a sink exists
static TRAY_NET: AtomicU64 = AtomicU64::new(0); // medium | up<<8 | has_ip<<9 | ip<<16
static TRAY_CLOCK: AtomicU8 = AtomicU8::new(0);
static TRAY_SEEN: AtomicU8 = AtomicU8::new(0); // 1 once a pass published

/// **The owners' pass** — called beside [`poll`] on the desktop service pass. Reads only the owners' own
/// accessors (each a relaxed load: no port, no lock that a composite holds), so it is not throttled.
pub fn tray_publish() {
    if !crate::boot::services_gate("status") {
        return; // INSTALLBARE (R86): furniture's — nothing is published under the setter / the login screen
    }
    #[cfg(all(target_arch = "x86_64", feature = "hda"))]
    let sink = crate::drivers::hda::vol::ready();
    #[cfg(not(all(target_arch = "x86_64", feature = "hda")))]
    let sink = false;
    TRAY_VOL.store(sink as u8, Ordering::Relaxed);
    #[allow(unused_mut)]
    let mut net: u64 = 0;
    #[cfg(feature = "usbnet")]
    {
        use crate::drivers::xhci::usbnet as u;
        if u::kind() != 0 || u::is_up() {
            net = NET_USB as u64 | ((u::link_up() as u64) << 8);
        }
    }
    #[cfg(all(target_arch = "x86_64", feature = "wifi"))]
    if net == 0 && crate::wifi::status::d11_up() {
        net = NET_WIFI as u64; // the radio's core is up; no association is measured, so `up` stays 0
    }
    #[cfg(all(target_arch = "x86_64", feature = "smolnet"))]
    if net != 0 {
        if let Some(ip) = crate::smolnet::lease_ip() {
            net |= (1 << 9) | ((u32::from_be_bytes(ip) as u64) << 16);
        }
    }
    TRAY_NET.store(net, Ordering::Relaxed);
    TRAY_CLOCK.store(crate::clock::try_unix_now().is_some() as u8, Ordering::Relaxed);
    TRAY_SEEN.store(1, Ordering::Relaxed);
    super::statusmenu::ensure_registered(); // `tests statustray` (R80: registered on the desktop pass, never run at boot)
}

/// **The painter's read**: relaxed loads, no lock, no allocation — legal from `strip::compose_all`. Before
/// the first [`tray_publish`] every item is unknown, so nothing is drawn (the input item included: the
/// tray appears with the session, not before it).
pub fn tray() -> Option<Tray> {
    if TRAY_SEEN.load(Ordering::Relaxed) == 0 {
        return None;
    }
    let n = TRAY_NET.load(Ordering::Relaxed);
    let net = match (n & 0xff) as u8 {
        0 => None,
        medium => Some(Net {
            medium,
            up: n & (1 << 8) != 0,
            ip: if n & (1 << 9) != 0 { Some(((n >> 16) as u32).to_be_bytes()) } else { None },
        }),
    };
    Some(Tray {
        volume: if TRAY_VOL.load(Ordering::Relaxed) != 0 { Some(volume()) } else { None },
        net,
        pc: crate::video::keymap::pc_selected(),
        clock: TRAY_CLOCK.load(Ordering::Relaxed) != 0,
    })
}

/// Is item `i`'s fact known (drawn when the bar can seat it)? The battery answers from MENUSTAT; the clock
/// is always drawn (CLOCKBAR's `--:--` stands until the anchor) and its menu says when it is not set.
pub fn item_known(t: &Tray, i: u8) -> bool {
    match i {
        ITEM_INPUT | ITEM_CLOCK => true,
        ITEM_NET => t.net.is_some(),
        ITEM_VOLUME => t.volume.is_some(),
        ITEM_BATTERY => bar_item().is_some(),
        _ => false,
    }
}

/// `tests statustray` fixture hook: replace the published facts (the next [`tray_publish`] restores the live ones).
#[cfg(feature = "witness")]
pub fn tray_inject(vol: bool, net: u64, clock: bool) {
    TRAY_VOL.store(vol as u8, Ordering::Relaxed);
    TRAY_NET.store(net, Ordering::Relaxed);
    TRAY_CLOCK.store(clock as u8, Ordering::Relaxed);
    TRAY_SEEN.store(1, Ordering::Relaxed);
}

/// The raw published words, for the fixture's save/restore.
#[cfg(feature = "witness")]
pub fn tray_raw() -> (u8, u64, u8, u8) {
    (TRAY_VOL.load(Ordering::Relaxed), TRAY_NET.load(Ordering::Relaxed), TRAY_CLOCK.load(Ordering::Relaxed), TRAY_SEEN.load(Ordering::Relaxed))
}

#[cfg(feature = "witness")]
pub fn tray_restore(r: (u8, u64, u8, u8)) {
    TRAY_VOL.store(r.0, Ordering::Relaxed);
    TRAY_NET.store(r.1, Ordering::Relaxed);
    TRAY_CLOCK.store(r.2, Ordering::Relaxed);
    TRAY_SEEN.store(r.3, Ordering::Relaxed);
}
