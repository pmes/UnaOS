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

//! SDHC-4c — the WRITE PERMIT for the internal SD card: a closed, published set of sectors.
//!
//! # What this module is
//!
//! SDHC-4b mounted the rMBP's internal SD card and refused every FAT-layer write to it
//! unconditionally ([`crate::fs::fat`]'s old `refuse_sdhc_write`). 4c replaces that single refusal
//! with a single PERMIT, at the same seam, and the permit is *stricter than a refusal is loose*:
//! it names an absolute LBA range on the wire at reserve time, and thereafter admits a write only
//! when the write's whole span lies inside that range. Everything else is refused, loudly, with the
//! same one-shot witness discipline 4b used.
//!
//! ## THE WRITABLE SECTOR SET, AND WHY IT IS CLOSED
//!
//! SCOPE FIRST, because an earlier wording of this block claimed more than is true and was
//! falsified by a line in the same capture it was written against. What follows describes **the
//! FAT layer's** writable set. The image also contains SDHC-4a's `write_selftest`
//! (`drivers/sdhc.rs:3165`, called unconditionally from `bring_up`), which under the same `sdw`
//! feature writes and then RESTORES `num_blocks - 1` on every armed boot, outside this permit and
//! gated by its own seven-rung ladder. On the bench card that is LBA 60799, and it is on the wire
//! every boot: `:: sdhc: w1 armed=1 lba=60799 ... verify=IDENTICAL restore=IDENTICAL -> PASS ::`
//! (Boots AL/AM/AN/AO). This arc neither adds nor removes that write; it must not pretend the
//! medium sees only its own.
//!
//! Under this arc a build can write to the internal SD card ONLY through
//! [`crate::fs::fat::write_sector`] / `write_sectors` with `source == BlockSource::Sdhc`, and both
//! call [`permit_write`] BEFORE the block layer is touched. `permit_write` admits a span
//! `[lba, lba + count)` if and only if:
//!
//!   1. [`PERMIT_STATE`] is `ST_ARMED`, and
//!   2. `EXTENT_LBA <= lba` and `lba + count <= EXTENT_END`.
//!
//! `EXTENT_LBA` / `EXTENT_END` are written EXACTLY ONCE, by [`arm`], under a compare-exchange on
//! `PERMIT_STATE` (`ST_UNATTEMPTED -> ST_ARMING`). No other function in the kernel stores to them;
//! `permit_write` only loads. So the writable set is a single half-open LBA interval, fixed for the
//! boot before the first write is possible, and it is derived in [`crate::fs::fat`]'s reserve pass
//! from the cluster chain of a file the HOST staged — never from anything computed at write time.
//!
//! The reserve pass additionally checks `lba >= data_start` — but be precise about what that
//! buys, because an earlier wording of this paragraph got it wrong. That inequality is
//! TAUTOLOGICAL and can never fire: `valid_cluster(c)` requires `c >= 2` and
//! `cluster_lba(c) = data_start + (c - 2) * spc`, so a chain-derived `a` is `>= data_start` by
//! construction. The metadata really is unreachable — the boot sector, the reserved sectors, both
//! FAT copies and the FAT16 fixed root all live below `data_start` — but the REASON is
//! `valid_cluster` plus `cluster_lba`'s linearity, and that reason is NOT independent of the BPB:
//! `data_start` appears on both sides, so a dishonest BPB moves the extent and the floor together.
//! The bounds that are genuinely BPB-independent are the partition claim (`in_extent`, a separate
//! on-disk structure) and the device's own `num_blocks` (from the block layer). Those two are what
//! survive a lying BPB, and they are why the set stays bounded even then.
//!
//! **THERE IS NOW EXACTLY ONE KNOB THAT WIDENS THE SET, AND THIS PARAGRAPH USED TO SAY THERE WAS NONE.**
//! Everything above is TRUE WITH `sdw-rw` OFF — the shipped polarity — and RETIRED WITH IT ON: SDHCPOST
//! (B155) makes the volume's posture liftable, and a writable volume whose permit still admitted one
//! extent would refuse every file verb per sector instead. See §SDHCPOST at this file's TAIL.
//!
//! ## WHAT IS *NOT* IN THE SET — the reserve-once idea
//!
//! WITH `sdw-rw` OFF (the shipped polarity, and the whole of what follows): nothing outside the
//! reserved file's own data clusters. In particular NO FAT entry, NO directory sector, NO allocation
//! and NO free. The kernel never creates, grows, deletes or renames the reserved file: it ADOPTS a
//! host-staged one or refuses. That is why there is no lock here and none is claimed — a path that
//! mutates no shared FAT structure has nothing to serialize against, and the x86 FAT-mutator count
//! (`fs/fat.rs` FAT-MUTATOR ROSTER) stays at one, on the *boot* volume, unchanged. [`FAT_MUTATIONS`]
//! is the instrument that can falsify that claim: incremented at `with_fat_lock_src` /
//! `with_dir_lock_src` whenever the source is `Sdhc`. **WITH `sdw-rw` ON the claim is RETIRED, not
//! merely strained** — a file verb on a writable volume IS a FAT mutator; §SDHCPOST below says so.
//!
//! ## FRGUARD composition
//!
//! FRGUARD (`drivers/block.rs`) answers a different question about a different handle: may the
//! *Default* slot be written, given the boot volume's `BS_VolID`. Its predicate refuses in exactly
//! one state, `BM_SUBSTITUTED` — the boot volume positively located on the `Sdhc` handle. That is
//! the configuration (Boot AI-2) in which THIS arc's target IS the boot medium, and it is the case
//! the extent bound exists for: a write to the medium the kernel booted from is admitted only
//! inside the reserved file's own clusters, checked at the point the CMD24 is about to be issued —
//! one level BELOW where FRGUARD checks, and per-sector rather than per-handle. FRGUARD is neither
//! consulted nor weakened here; it keeps refusing `Default` writes in that state exactly as before,
//! and the reserve witness names the boot serial so the two verdicts can be read together.
//!
//! ## The witnesses
//!
//! Four lines, each able to say NO:
//!
//! ```text
//! :: SDHC4C: reserve NAME=UNALOG.BIN cluster=K size=S runs=1 lba=[A..B) permit=ARMED ::
//! :: SDHC4C: reserve NAME=UNALOG.BIN ... permit=UNARMED (<reason>) — the card stays READ-ONLY ::
//! :: SDHC4C: in-place write ok bytes=W lba=[A..B) readback=MATCH fnv=0x... ::
//! :: SDHC4C: tally fat-mutations-on-sdhc=0 permits=N refusals=0 cmd24=N armed=1 ::   (`sectors=` under `sdw-rw`)
//! ```
//!
//! The must-not-appear line is `:: SDHC4C: permit REFUSED ...`. A refusal is not a device fault, so
//! it surfaces as [`FatError::Unsupported`] — the same mapping 4b used, which the VFS renders as
//! "read-only volume" rather than as a disk error.

#![cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

use super::fat::FatError;

/// The reserved file's 8.3 name on the card's FAT volume. Staged by the HOST; the kernel never
/// creates it (see the module doc — creation would be a directory mutation).
pub const RESERVE_NAME: &str = "UNALOG.BIN";

/// Minimum on-disk size the staged file must have for the permit to arm, in bytes. 64 KiB = 128
/// sectors. A smaller file is a REFUSAL, never a short reservation: short-writing later is exactly
/// the failure mode reserve-once exists to remove.
pub const RESERVE_BYTES: u32 = 64 * 1024;

/// No reserve pass has run yet. Refuses.
const ST_UNATTEMPTED: u8 = 0;
/// A reserve pass has claimed the right to publish the extent and has not finished. Refuses.
/// Distinct from `ST_UNATTEMPTED` so a second caller cannot re-enter `arm` and overwrite a
/// half-published extent — the same load-bearing CAS the flight recorder's `RESERVED` uses.
const ST_ARMING: u8 = 1;
/// The extent is published and immutable. The ONLY state that admits a write.
const ST_ARMED: u8 = 2;
/// A reserve pass ran and refused. Permanent for the boot; the card stays read-only, i.e. the arc
/// degrades to exactly SDHC-4b. Refuses.
const ST_UNARMED: u8 = 3;

/// The permit state. See the `ST_*` constants; only [`arm`] and [`disarm`] store to it.
static PERMIT_STATE: AtomicU8 = AtomicU8::new(ST_UNATTEMPTED);

/// First absolute LBA of the reserved extent (inclusive). Written once by [`arm`], under the CAS,
/// with `Release`; read with `Acquire` only after `PERMIT_STATE == ST_ARMED`, so an armed read
/// necessarily sees the published value.
static EXTENT_LBA: AtomicU64 = AtomicU64::new(0);
/// One past the last absolute LBA of the reserved extent (EXCLUSIVE). Same publication discipline.
/// Initialised to 0 so that even a torn read of a half-published pair yields an EMPTY interval,
/// which admits nothing — the initial values fail closed on their own.
static EXTENT_END: AtomicU64 = AtomicU64::new(0);

/// How many write spans the permit admitted this boot.
static PERMITS: AtomicU32 = AtomicU32::new(0);
/// How many it refused. Non-zero is a finding.
static REFUSALS: AtomicU32 = AtomicU32::new(0);
/// SECTORS admitted. The STATIC keeps 4c's name and the WIRE carries the truth: `cmd24=` on a `ro`
/// build (the loop really is CMD24), `sectors=` under `sdw-rw` (the path is CMD25) — see `tally`. A
static CMD24: AtomicU32 = AtomicU32::new(0); // rename here costs 8 bytes of .bss ORDER, for nothing.
/// FAT-table or directory RMWs ATTEMPTED against the `Sdhc` source. Must be 0. Incremented at the
/// two lock wrappers in `fs/fat.rs`, which every such RMW funnels through.
static FAT_MUTATIONS: AtomicU32 = AtomicU32::new(0);

/// One-shot latch for the refusal witness, so a retrying writer names the refusal once rather than
/// once per sector — the shape 4b's `SDHC_RO_REFUSED` established and Boot AG exercised.
static REFUSED_ONCE: AtomicBool = AtomicBool::new(false);
/// One-shot latch for the mutation witness.
static MUTATION_ONCE: AtomicBool = AtomicBool::new(false);

/// THE BOUNDS PREDICATE. Pure: reads three atomics, stores nothing, prints nothing, and is the only
/// place the writable set is defined. [`permit_write`] and [`selftest_bounds`] both call it, which
/// is what makes the self-test a test OF THE PERMIT rather than of a copy of its arithmetic.
///
/// `true` iff the permit is armed and `[lba, lba + count)` lies wholly inside the reserved extent.
/// A zero `count` is FALSE — an empty span is a caller bug, not a permission.
#[inline]
fn in_reserved_extent(lba: u64, count: u64) -> bool {
    if PERMIT_STATE.load(Ordering::Acquire) != ST_ARMED {
        return false;
    }
    if count == 0 {
        return false;
    }
    let Some(end) = lba.checked_add(count) else {
        return false; // an overflowing span can never be inside a finite interval
    };
    let a = EXTENT_LBA.load(Ordering::Acquire);
    let b = EXTENT_END.load(Ordering::Acquire);
    lba >= a && end <= b
}

/// THE SINGLE DECISION POINT for every FAT-layer write to the internal SD card.
///
/// Called from `fs::fat::write_sector` and `fs::fat::write_sectors` BEFORE the block layer is
/// reached, so a refused write issues no CMD24 and takes no card lock. Returns
/// [`FatError::Unsupported`] on refusal — a refusal is a policy answer, not a device fault.
///
/// On success it accounts the span (`PERMITS`, `SECTORS`) so the tally line can be compared against
/// the bytes the reserve pass says it wrote.
pub fn permit_write(site: &str, lba: u64, count: u64) -> Result<(), FatError> {
    permit_span(site, lba, count)?;
    PERMITS.fetch_add(1, Ordering::Relaxed);
    CMD24.fetch_add(count.min(u32::MAX as u64) as u32, Ordering::Relaxed);
    Ok(())
}

/// [`permit_write`] WITHOUT the accounting: the same bound, the same refusal witness, but it does
/// not add to `PERMITS`/`SECTORS`.
///
/// This exists for `write_sectors`' whole-run pre-check. That check and the per-chunk checks cover
/// overlapping sectors, so accounting both would inflate the count above the sectors the card
/// actually sees — and `count == bytes / 512` is a falsifiable prediction of this arc, so the
/// instrument must count card traffic, not gate crossings. `REFUSALS` IS incremented here: a
/// refusal is never double-counted, because the run check returning `Err` means the loop is never
/// entered.
pub fn permit_span(site: &str, lba: u64, count: u64) -> Result<(), FatError> { #[cfg(feature = "sdw-rw")] if sdhcpost_admits(site, lba, count) { return Ok(()); } // SDHCPOST (B155): the FIRST rung of the ladder, and it exists only in an `sdw-rw` build. With the volume's posture `rw` the reserved extent is no longer THE writable set, so a bound that still admitted one extent would refuse every file verb sector by sector — the half-finished mutation `BlockSource::write_veto` exists to prevent, arriving one layer lower. The extent rung below is UNTOUCHED and is still the whole gate whenever the posture is `ro`, which is every shipped build. See §SDHCPOST at this file's tail.
    if in_reserved_extent(lba, count) {
        return Ok(());
    }
    REFUSALS.fetch_add(1, Ordering::Relaxed);
    if !REFUSED_ONCE.swap(true, Ordering::Relaxed) {
        let st = PERMIT_STATE.load(Ordering::Acquire);
        let reason = match st {
            ST_UNATTEMPTED => "no reserve pass has run",
            ST_ARMING => "a reserve pass is still publishing the extent",
            ST_UNARMED => "the reserve pass REFUSED; the card is read-only this boot",
            _ => "the span is OUTSIDE the reserved extent",
        };
        serial_println!(
            ":: SDHC4C: permit REFUSED at {} lba={} count={} extent=[{}..{}) state={} — {} \
             (first, once) ::",
            site,
            lba,
            count,
            EXTENT_LBA.load(Ordering::Acquire),
            EXTENT_END.load(Ordering::Acquire),
            st,
            reason
        );
    }
    Err(FatError::Unsupported)
}

/// Publish the reserved extent and arm the permit. Returns `false` if the permit was already
/// attempted this boot — the extent is published EXACTLY once, and a second attempt is a caller bug
/// that must not silently rewrite the writable set.
///
/// `end` is EXCLUSIVE. The caller ([`crate::fs::fat`]'s reserve pass) has already proven the
/// interval lies in the volume's data region, inside both the volume and the partition extent, and
/// inside the device's `num_blocks`; this function re-checks only the interval's own sanity, so a
/// degenerate or inverted range can never become the writable set.
#[allow(clippy::too_many_arguments)]
pub fn arm(
    name: &str,
    cluster: u32,
    size: u32,
    runs: usize,
    start: u64,
    end: u64,
    boot_serial: u32,
    vol_serial: u32,
) -> bool {
    if PERMIT_STATE
        .compare_exchange(ST_UNATTEMPTED, ST_ARMING, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        serial_println!(
            ":: SDHC4C: arm REFUSED — the permit was already attempted this boot (state={}); the \
             reserved extent is published exactly once ::",
            PERMIT_STATE.load(Ordering::Acquire)
        );
        return false;
    }
    // `start == 0` is rejected as hard as an inverted range: LBA 0 is the MBR/boot sector, it is
    // below every possible `data_start`, and an extent anchored there would make the self-test's
    // "one sector below" case degenerate into a sector INSIDE the extent. No legitimate data
    // cluster ever lands on LBA 0.
    if end <= start || start == 0 {
        // Degenerate. Leave the extent at its fail-closed (0, 0) and land in the permanent refusal.
        PERMIT_STATE.store(ST_UNARMED, Ordering::Release);
        serial_println!(
            ":: SDHC4C: reserve NAME={} cluster={} size={} runs={} lba=[{}..{}) permit=UNARMED \
             (empty, inverted, or LBA-0-anchored extent) — the card stays READ-ONLY ::",
            name, cluster, size, runs, start, end
        );
        return false;
    }
    // Publish the interval BEFORE the state that makes it readable. `permit_write` loads the state
    // with Acquire first and only then the bounds, so it cannot observe ST_ARMED with a stale pair.
    EXTENT_LBA.store(start, Ordering::Release);
    EXTENT_END.store(end, Ordering::Release);
    PERMIT_STATE.store(ST_ARMED, Ordering::Release);
    serial_println!(
        ":: SDHC4C: reserve NAME={} cluster={} size={} runs={} lba=[{}..{}) sectors={} \
         permit=ARMED (host-staged, adopt-only: the kernel creates/grows/deletes nothing on this \
         volume; boot-serial=0x{:08x} card-vol-serial=0x{:08x}) ::",
        name,
        cluster,
        size,
        runs,
        start,
        end,
        end - start,
        boot_serial,
        vol_serial
    );
    true
}

/// Record a permanent refusal to arm, naming the reason on the wire. The card stays read-only for
/// the boot and the arc degrades to exactly SDHC-4b. This is the honest-failure form the arc is
/// REQUIRED to be able to come back with; a silent skip would be a defect.
pub fn disarm(name: &str, reason: &str) {
    // A refusal may be recorded from `ST_UNATTEMPTED` (the normal path) or from `ST_ARMING` (a
    // reserve pass that claimed and then failed a later check). Never from `ST_ARMED`: once the
    // extent is published it is immutable, and quietly retracting it would make the witness a lie.
    let prev = PERMIT_STATE.load(Ordering::Acquire);
    if prev == ST_ARMED {
        serial_println!(
            ":: SDHC4C: disarm IGNORED ({}) — the extent is already published and is immutable ::",
            reason
        );
        return;
    }
    PERMIT_STATE.store(ST_UNARMED, Ordering::Release);
    serial_println!(
        ":: SDHC4C: reserve NAME={} permit=UNARMED ({}) — the card stays READ-ONLY (SDHC-4b \
         behaviour, no CMD24 is reachable from the FAT layer) ::",
        name, reason
    );
}

/// Claim the right to run the one reserve pass. `false` means one already ran (or is running).
pub fn claim_reserve_pass() -> bool {
    PERMIT_STATE.load(Ordering::Acquire) == ST_UNATTEMPTED
}

/// Is the permit armed? For the reserve pass's own post-checks and the write pass's gate.
pub fn armed() -> bool {
    PERMIT_STATE.load(Ordering::Acquire) == ST_ARMED
}

/// The published extent as `[start, end)`, or `(0, 0)` when unarmed.
pub fn extent() -> (u64, u64) {
    if !armed() {
        return (0, 0);
    }
    (
        EXTENT_LBA.load(Ordering::Acquire),
        EXTENT_END.load(Ordering::Acquire),
    )
}

/// A FAT-table or directory read-modify-write was ATTEMPTED against the `Sdhc` source. Must never
/// happen: this arc's whole safety argument is that the card acquires a writer that is not a FAT
/// mutator. Counting rather than asserting is deliberate — the write itself is refused anyway by
/// [`permit_write`] (a FAT or directory sector is below `data_start` and therefore outside the
/// extent by construction), so the counter's job is to make the ATTEMPT visible, which a refusal
/// count alone would not distinguish from an out-of-range data write.
pub fn note_fat_mutation(site: &str) { #[cfg(feature = "sdw-rw")] if sdhcpost_note_expected_mutation(site) { return; } // SDHCPOST (B155): on a volume the posture made WRITABLE, a FAT-table or directory RMW is what a file verb IS — it is EXPECTED, and counting it as the defect below would make the instrument cry wolf on every screenshot. It is counted separately and said separately; the defect line keeps its exact meaning for every `ro` build, which is all of them by default.
    FAT_MUTATIONS.fetch_add(1, Ordering::Relaxed);
    if !MUTATION_ONCE.swap(true, Ordering::Relaxed) {
        serial_println!(
            ":: SDHC4C: !! FAT MUTATION ATTEMPTED on the Sdhc source at {} — this arc's invariant \
             is that the card has no FAT mutator; the write below will be refused by the extent \
             bound, but the ATTEMPT is a defect (first, once) ::",
            site
        );
    }
}

/// SELF-TEST of the bounds predicate, using the predicate itself.
///
/// Runs immediately after [`arm`] and attempts four spans that MUST be refused: the sector just
/// below the extent, the sector just above it, a span that straddles the top edge, and a span whose
/// length overflows. It calls [`in_reserved_extent`] — the same function `permit_write` calls — so
/// it cannot pass against a copy of the arithmetic that has drifted from the real one.
///
/// If any of them is admitted, that is a live medium-destroying defect and the permit is DISARMED
/// on the spot: the card reverts to read-only rather than shipping a bound that does not hold.
/// Deliberately does NOT go through `permit_write`, so a passing self-test leaves `REFUSALS` at 0
/// and the must-not-appear refusal line absent — the self-test must not forge the arc's own
/// failure signature.
pub fn selftest_bounds() {
    let (a, b) = extent();
    if a == 0 && b == 0 {
        return; // not armed; nothing to test
    }
    let cases: [(&str, u64, u64); 4] = [
        ("one sector below", a.saturating_sub(1), 1),
        ("one sector above", b, 1),
        ("straddling the top edge", b - 1, 2),
        ("length overflow", a, u64::MAX),
    ];
    let mut leaked = 0u32;
    for (what, lba, count) in cases {
        if in_reserved_extent(lba, count) {
            leaked += 1;
            serial_println!(
                ":: SDHC4C: !! SELFTEST FAILED — the bounds predicate ADMITTED {} (lba={} count={}) \
                 against extent=[{}..{}) ::",
                what, lba, count, a, b
            );
        }
    }
    if leaked > 0 {
        PERMIT_STATE.store(ST_UNARMED, Ordering::Release);
        EXTENT_LBA.store(0, Ordering::Release);
        EXTENT_END.store(0, Ordering::Release);
        serial_println!(
            ":: SDHC4C: permit DISARMED by its own self-test ({} of 4 out-of-extent spans were \
             admitted) — the card stays READ-ONLY ::",
            leaked
        );
        return;
    }
    serial_println!(
        ":: SDHC4C: selftest bounds extent=[{}..{}) — 4/4 out-of-extent spans REFUSED (below, \
         above, straddle, overflow); the permit admits nothing outside its extent ::",
        a, b
    );
}

/// FNV-1a (32-bit) over `data`. The read-back checksum: an echo that cannot fail proves nothing, so
/// the verify pass hashes what it MEANT to write and what the card GAVE BACK, and prints both.
pub fn fnv1a(data: &[u8]) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for &b in data {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// The closing tally. Printed by the reserve pass whatever the outcome, so "nothing happened" and
/// "the pass did not run" are distinguishable in a capture.
pub fn tally() { #[cfg(feature = "sdw-rw")] if sdhcpost_tally() { return; } // SDHCPOST (B155): the counter's NAME must name the command the card saw, so the two polarities print two lines and neither lies. `ro` build: the CMD24 loop below is the write path, and the line below is byte-for-byte the one every capture since 4c has carried. `sdw-rw` build: the path is CMD25, so `cmd24=` would be false and `sdhcpost_tally` prints `sectors=` instead. That is also why `knoboff sdw` can be byte-identical while the rename still lands.
    serial_println!(
        ":: SDHC4C: tally fat-mutations-on-sdhc={} permits={} refusals={} cmd24={} armed={} ::",
        FAT_MUTATIONS.load(Ordering::Relaxed),
        PERMITS.load(Ordering::Relaxed),
        REFUSALS.load(Ordering::Relaxed),
        CMD24.load(Ordering::Relaxed),
        armed() as u8
    );
}

// ═══════ SDHCPOST (rmbp-ledger B155) — what a WRITABLE volume does to this module's argument ══════
//
// READ THE MODULE DOC FIRST: everything it says is true with `sdw-rw` OFF, which is every shipped
// build. This section is what changes when it is ON, and it is stated as a RETIREMENT rather than as
// an exception, because two of 4c's load-bearing claims simply stop being true:
//
//   * "There is no knob that widens the set." There is now exactly one, and it is this one.
//   * "The card acquires a writer that is not a FAT mutator." On a writable volume a file verb IS a
//     FAT mutator — it allocates clusters, links the FAT and publishes a directory entry. That is
//     not a defect to be caught, it is the feature that was asked for.
//
// WHY THE BOUND HAD TO GO RATHER THAN WIDEN. The obvious smaller change — keep the permit and widen
// the extent to cover a `Pictures/Screenshots` path — cannot work, and the reason is worth writing
// down because it is the argument for the whole shape. A capture does not write one extent: it
// CREATES a file (a directory-sector RMW), GROWS it (an `alloc_cluster` per 32 KiB, each one a FAT
// write to every copy) and publishes its size LAST (another directory RMW). Those sectors live BELOW
// `data_start`, where the module doc proves the extent can never reach. A widened extent would admit
// the data and refuse the metadata, i.e. refuse the file. So the permit is not the gate any more.
//
// WHAT IS THE GATE, THEN. Three things, none of them new and none of them this module's:
//
//   1. THE POSTURE, asked in advance and once, at the mount — `drivers::block`'s §SDHCPOST, via
//      `fat::BlockSource::write_veto`. This is the layer that can say no BEFORE a multi-step verb
//      gets part-way, which is the whole reason `write_veto` exists.
//   2. THE BLOCK LAYER'S OWN BOUNDS — `write_block_sdhc` and `write_blocks_sdhc_mb` both check the
//      LBA against the published geometry, and the driver checks it again against the card's own
//      capacity.
//   3. THE DRIVER'S FOUR GATES, per write, unchanged since 4a/4b: the build gate, the write-protect
//      PIN re-read at the moment of the write, and the card's own CSD PERM/TMP_WRITE_PROTECT.
//
// WHAT IS HONESTLY LOST, and B155 is where Peter decides whether to pay it. FAT has no journal. A
// power cut between the FAT write and the directory write leaves a cluster chain nothing points at;
// a cut between two FAT copies leaves them disagreeing. 4c's reserve-once shape had NO exposure to
// either, because it never touched a FAT or a directory sector at all. An `sdw-rw` boot has the
// exposure of any FAT writer. That is the trade, it is not mitigated here, and it is why the knob
// is opt-in and the DEFAULT posture is unchanged.

/// SDHCPOST: does the volume's own POSTURE admit this span, ahead of the reserved-extent bound?
///
/// Returns `true` only when `drivers::block::sdhc_writes_admitted()` says the mount is writable —
/// i.e. `sdw-rw` is built, the write-protect pin read at mount said enabled, and the block layer has
/// a live write path. It is deliberately NOT a second copy of that decision: it asks the one
/// definition, exactly as the `Default` arm of `BlockSource::write_veto` asks `default_writable()`.
///
/// LBA 0 is refused here even so. No file verb can name the MBR — the volume lives inside a
/// partition and `cluster_lba` cannot address below it — so this can only fire on a caller bug or a
/// dishonest BPB, and a posture is not a licence to write the partition table.
#[cfg(feature = "sdw-rw")]
fn sdhcpost_admits(site: &str, lba: u64, count: u64) -> bool {
    if count == 0 || lba == 0 || !crate::drivers::block::sdhc_writes_admitted() {
        return false;
    }
    if !RW_SAID.swap(true, Ordering::Relaxed) {
        serial_println!(
            ":: SDHC4C: permit BYPASSED at {} lba={} count={} \u{2014} `sdw-rw` is built and the \
             posture says sdhc=rw, so the reserved-extent bound is NOT the gate this boot; the gate \
             is the mount posture (SDHCPOST) plus the block layer's bounds and the driver's \
             per-write WP/CSD gates (first, once) ::",
            site, lba, count
        );
    }
    RW_PERMITS.fetch_add(1, Ordering::Relaxed);
    true
}

/// SDHCPOST: a FAT-table or directory RMW on a volume the posture made writable. `true` means this
/// module has accounted it as EXPECTED and the caller must not also count it as the defect.
///
/// The two counters are kept apart on purpose. `FAT_MUTATIONS` means "an attempt was made that this
/// arc's invariant says cannot happen" and must stay readable as that on every `ro` capture;
/// `RW_MUTATIONS` means "a file verb did what a writable volume is for". Folding them would retire
/// an instrument in order to avoid renaming it.
#[cfg(feature = "sdw-rw")]
fn sdhcpost_note_expected_mutation(site: &str) -> bool {
    if !crate::drivers::block::sdhc_writes_admitted() {
        return false;
    }
    RW_MUTATIONS.fetch_add(1, Ordering::Relaxed);
    if !RW_MUTATION_SAID.swap(true, Ordering::Relaxed) {
        serial_println!(
            ":: SDHC4C: mutation expected on the Sdhc source at {} \u{2014} the volume's posture is \
             rw (`sdw-rw`), so a FAT-table or directory RMW is a file verb doing its job, not the \
             invariant breach the `!!` line reports on a read-only build (first, once) ::",
            site
        );
    }
    true
}

/// SDHCPOST: the rw half of the closing tally, printed beside 4c's own so a capture shows both the
/// reserve-once accounting and the posture accounting without either line changing shape.
#[cfg(feature = "sdw-rw")]
fn sdhcpost_tally() -> bool {
    serial_println!(
        ":: SDHC4C: tally fat-mutations-on-sdhc={} permits={} refusals={} sectors={} armed={} \
         posture={} permits-by-posture={} expected-mutations={} ::",
        FAT_MUTATIONS.load(Ordering::Relaxed),
        PERMITS.load(Ordering::Relaxed),
        REFUSALS.load(Ordering::Relaxed),
        CMD24.load(Ordering::Relaxed),
        armed() as u8,
        if crate::drivers::block::sdhc_writes_admitted() { "rw" } else { "ro" },
        RW_PERMITS.load(Ordering::Relaxed),
        RW_MUTATIONS.load(Ordering::Relaxed)
    );
    true
}

/// SDHCPOST: spans admitted by the POSTURE rather than by the reserved extent.
#[cfg(feature = "sdw-rw")]
static RW_PERMITS: AtomicU32 = AtomicU32::new(0);
/// SDHCPOST: FAT/directory RMWs accounted as EXPECTED. See [`sdhcpost_note_expected_mutation`].
#[cfg(feature = "sdw-rw")]
static RW_MUTATIONS: AtomicU32 = AtomicU32::new(0);
/// SDHCPOST: one-shot latch for the bypass witness.
#[cfg(feature = "sdw-rw")]
static RW_SAID: AtomicBool = AtomicBool::new(false);
/// SDHCPOST: one-shot latch for the expected-mutation witness.
#[cfg(feature = "sdw-rw")]
static RW_MUTATION_SAID: AtomicBool = AtomicBool::new(false);
