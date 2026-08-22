// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// WIFI-1 — the firmware-load path (arc 1 of the BCM4331 association ladder).
//
// ## Scope of THIS arc
// Arc 1 does two read-only things and nothing else:
//   1. `bus::census()` — find the AirPort radio (class 0x02 / subclass **0x80**) in PCI CONFIG space
//      and CROSS-CHECK its identity against the values `bcm4331.md` §0 pinned on metal. Config-space
//      reads only: no BAR mapping, no MMIO, no register write, no bus-master enable, no `cfg:0x80`.
//   2. `firmware::stage_attempt()` — find the user-supplied firmware SET on the program-source FAT
//      volume, bounds-validate each file, classify its container header, and stage the bytes for a
//      later arc to feed to the core.
// It does NOT touch the radio, does not walk the bcma core table, and does not attempt association.
//
// ## The ladder (why this arc stops where it stops)
//   * **Arc 1 (this one) — firmware load.** The set located on media, validated, staged. Radio
//     identity cross-checked. Everything read-only.
//   * **Arc 2 (landed, `bringup.rs`) — bcma core enumeration.** Map BAR0, walk the on-chip core table
//     from our own reads, find the d11 802.11 core + its master wrapper, read the core and wrapper
//     state, and re-measure §S3's enable rule. This is the first arc that WRITES: the backplane
//     window selector always (moved and restored), a backplane register only on the branch where the
//     core does NOT arrive enabled, and then only its reversible half. **§S4's reset prologue and the
//     microcode upload moved OUT of this arc and into arc 3** — the prologue destroys the resident
//     microcode and only a successful upload restores a working state (`bcm4331.md` §5 risk 4), and
//     the upload is blocked on a routing value no source this module may use records. See
//     `bringup.rs`'s header.
//   * **Arc 3 — the prologue, the upload, and up to an association attempt.** PHY/radio init from the staged initvals, a receive
//     path, a scan, and one authenticate/associate exchange, bound to `smolnet` through the existing
//     `net_phy` seam the e1000/genet/vnet paths already ride.
//
// The wider ladder this arc plugs into — S0..S8, the metal captures, and the §S4 decision box — is
// `docs/dev/OS/06_NETWORK_STACK/bcm4331.md`. This module's own notes are
// `docs/dev/OS/06_NETWORK_STACK/wifi_bcma.md`.
//
// ## Legal posture, scoped precisely
// UnaOS ships NO firmware. The set is supplied by the user at runtime on the media
// (`docs/MANIFESTO/CLEAN_ROOM_POLICY.md` §4), never committed to this repository and never baked
// into a media image — which is `bcm4331.md` §S4's option 1, the only shape that keeps the tree
// clean.
//
// ## Clean-room posture — TRUE OF ARC 1 ONLY, and arc 2 says so in its own header
//
// **`mod.rs`, `bus.rs` and `firmware.rs` carry a clean-room claim. `bringup.rs` does NOT, and the
// difference is recorded rather than blurred.**
//
// Those three arc-1 files were written without reading any GPL Linux WiFi driver source, and without
// adopting code or constants from `drivers/bcma.rs`. Their factual inputs are the public PCI base
// specification, the public PCI-SIG vendor registry, and this tree's own metal captures and prose in
// `bcm4331.md`.
//
// **Arc 2's `bringup.rs` is on the other side of that line, by adoption.** Its register-offset and
// EROM-encoding block and its EROM cursor scaffolding are taken from `drivers/bcma.rs` — which states
// in its own header that its "register offsets and EROM encodings follow Linux `drivers/bcma`
// (`bcma_regs.h`, `bcma_driver_chipcommon.h`, `scan.c`) and `b43`'s `B43_MMIO_PHY_VER`" — so
// `bringup.rs` INHERITS that Group-B provenance under `CLEAN_ROOM_POLICY.md` §2. Only its parse
// strategy (tag-driven descriptor consumption) is independent work. An earlier version of this
// paragraph, and of `bringup.rs`'s header, asserted the opposite; the assertion was false and is
// withdrawn. **`src/wifi/` therefore does not carry a subsystem-wide clean-room claim**, and
// `bcm4331.md` §S4's own note — that its upload sequence is "transcribed from the b43 reference
// implementation's *interface*" — stands beside it.
//
// The value of a provenance ledger is that it is true. A recorded taint costs nothing; a laundered
// one costs the credibility of every other claim in the subsystem.
//
// Where a fact could not be pinned from a source legal for THIS implementer it is named UNKNOWN in
// the comments and reported by a witness rather than assumed by the code. One such fact stands: the
// `B43_SHM_UCODE` routing value, which no source available to this module records at all and on
// which arc 2's upload rung refuses. The container header layout (W3) WAS such a fact and is now
// ANSWERED on metal — rmbp1-boot1; `firmware.rs`'s module note carries the answer and its
// provenance (hypothesis-falsification against the bench set's own bytes, no driver source read).
//
// ## Knob and arch gate
// `UNAOS_WIFI=1` arms the `wifi` Cargo feature. Default OFF => this module and its call sites vanish
// and every image is byte-identical to baseline.
//
// The module is additionally `target_arch = "x86_64"`-gated, and `wifi` is stripped by `arroyo`'s
// `arm_features` (the `sdw`/`kbdwit`/`pcicensus`/`bcmarecon` policy): the radio is an x86/rMBP part,
// so this emits not one byte of aarch64 code — but an enabled feature is hashed into cargo's
// `-Cmetadata` fingerprint, and leaving it in would shift every Pi/Jetson media hash for zero
// observable change. Arming the x86 knob must never touch another track's media.
//
// `service()` is called from all THREE of `main.rs`'s storage-ready loop passes (the
// `fatverb_storage_witness` placement): which pass a given x86 build reaches depends on its knobs,
// and the forward-only state machine below runs its census once and its staging pass once
// whichever one runs.
//
// ## WIFI-REARM — the wait is a mechanism, not an absence
// Arc 1 shipped a staging pass that waits for the program-source volume, and Boot D showed the
// weakness of how it waited: the deferral printed once at 14.78 s and the module was silent for the
// remaining nine minutes. The poll was in fact still running every pass — but a capture cannot
// distinguish a live poll from a parked one when both are silent, so the wait could not be
// falsified, and the arc-2 walk that sits behind it had no witness saying why it had not run.
//
// The wait now speaks on a bounded schedule while it waits, announces the arrival, and re-arms the
// staging pass when a mount fails against a transport that has not settled. It is still UNBOUNDED —
// the volume can appear at any time and no instant makes "never" true — and it still gates arc 2,
// for the reason recorded at the call site.
//
// ## WIFI-REACH — the SECOND-handle wait, so a late stick is actually searched
// WIFI-REARM waits for the FIRST volume — the program source. It does not help the two-volume search
// PSRC added, because on the bench the program source (the internal SD card) is registered
// synchronously inside `pci::init`, before the main loop runs, so `program_source()` is `Some` on
// pass 1 and the staging entry fires there — while the USB stick carrying the blobs enumerates many
// passes later, from the deferred SCSI bring-up. At that first attempt `alternate_program_source()`
// is `None`, pass 2 finds nothing, and the pre-WIFI-REACH code printed a terminal INCOMPLETE and ran
// arc 2 an epoch before the stick could be looked at (`sdhc.md` §13.7, the OPEN DEFECT this closes).
//
// WIFI-REACH holds the gate open. `firmware::stage_attempt` gains a `Pending` outcome — incomplete,
// no second handle present — that prints NOTHING terminal; `service` then moves to `S_WAIT_ALT` and
// re-searches when the stick's storage-ready edge fires (`block::take_usb_ready()`, which has no
// other x86 consumer) or a BOUNDED deadline (`ALT_WAIT_MS`) expires. The two invariants both prior
// arcs rest on are preserved by construction: exactly one terminal verdict (the committing attempt
// always prints COMPLETE or INCOMPLETE, and `Pending` is never the last word), and arc 2 runs once
// on a final count (it runs only from `finish_and_park`, never on a `Pending`/`Retry` that is still
// moving). The upload refusal at `B43_SHM_UCODE` in arc 2 is UNTOUCHED — this arc only changes WHEN
// the terminal verdict is reached, never what arc 2 does with it.

/// WIFI-2 (arc 2) — the write rungs. `#[cfg(feature = "wifi2")]`, `UNAOS_WIFI2=1`, default OFF, so a
/// `UNAOS_WIFI=1`-only boot is the census-and-staging boot arc 1 flew on metal (Boot A) with not one
/// extra device access and not one extra witness string in the image.
///
/// The split is by WRITE, not by convenience: everything arc 1 does is a PCI-config read or a FAT
/// read, while every rung in `bringup` either writes the backplane window selector or depends on a
/// window that was moved by one. Two knobs mean the census can be re-flown at any time — to
/// re-confirm the identity cross-check on a machine, or to isolate a regression — without arming a
/// single write.
#[cfg(feature = "wifi2")]
pub mod bringup;
pub mod bus;
pub mod firmware;

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

/// Where `service()` is in its (strictly forward-only) state machine. There is no path back to an
/// earlier state and no path out of `Parked`, so a failure can never become a retry storm.
const S_START: u8 = 0;
/// Census done and the radio identified; waiting for a program-source block device to appear.
const S_WAIT_STORAGE: u8 = 1;
/// WIFI-REACH: the program source was searched, the set came up INCOMPLETE, and no second populated
/// handle was present to search. Holding for a late-publishing volume — the USB stick carrying the
/// b43 blobs, which enumerates many passes after the internal card that is the program source —
/// until its storage-ready edge fires or a bounded deadline expires. Still strictly forward-only:
/// `S_WAIT_STORAGE` -> `S_WAIT_ALT` -> `S_PARKED`, never a step back.
const S_WAIT_ALT: u8 = 2;
/// Terminal. Either the census refused, or the staging pass has settled.
const S_PARKED: u8 = 3;

static STATE: AtomicU8 = AtomicU8::new(S_START);
static DEFER_ANNOUNCED: AtomicBool = AtomicBool::new(false);

// ── WVAL-REPLAY: the census-ABSENT leg, and why it exists. ────────────────────────────────────────
//
// Everything this subsystem does BEFORE a device is touched is byte-level work on files: the FAT
// search, the bounds checks, `classify_header`'s container verdict, and arc 2's `validate_set()`
// dry-run over the whole staged set. None of it needs a radio. But the state machine's first rung is
// the census, and QEMU models no BCM4331 — so on every QEMU boot the census refuses, `S_PARKED` is
// stored, and not one line of that byte-level work runs. The classifier that WCLS-RECORD just taught
// the record framing to therefore had exactly one gate: a bench round on the rMBP.
//
// `wifival` gives it a second. On the ABSENT branch the module records the fact in `CENSUS_ABSENT`,
// says so out loud, and proceeds to `S_WAIT_STORAGE` — the staging pass then runs against the FAT
// volume as it always would, and the terminal `finish_and_park` routes to `bringup::validate_replay()`
// instead of `bringup::bringup_once()`.
//
// The routing is on the RECORDED census outcome, not on the feature: with the radio PRESENT this
// latch is false and `finish_and_park` calls `bringup_once()` exactly as it does today, so an image
// built with `wifival` behaves identically on metal. A knob that quietly re-routed the metal path
// would be a worse instrument than the absence it is fixing.
#[cfg(feature = "wifival")]
static CENSUS_ABSENT: AtomicBool = AtomicBool::new(false);

// ── WIFI-REARM: the wait, made visible and re-armable. ─────────────────────────────────────────────
//
// Boot D's capture is the whole motivation and it is worth quoting the shape rather than the lines:
// arc 1's census MATCHED at 14.77 s, the deferral printed at 14.78 s, and then this module said
// nothing for the remaining ~9 minutes of uptime. On that boot nothing ever enumerated
// (`xHCI: … no mass-storage device enumerated`), so there was nothing to stage — but the CAPTURE
// cannot show that, because a service still polling every pass and a service that gave up produce
// byte-identical serial: silence. A witness that reads the same whether the mechanism is alive or
// dead is not evidence of either, which is exactly the class of instrument this tree keeps catching.
//
// Two things change. The wait now SPEAKS on a bounded schedule, so its liveness is falsifiable from
// the capture; and a staging attempt that fails on a transport that has not settled is re-armed
// rather than spent (`firmware::StageOutcome`).
//
// What deliberately does NOT change: the wait itself is UNBOUNDED. A program-source volume can
// appear at any time — a stick plugged in after boot, or a storage lane that comes up late — and
// there is no instant at which "it will never arrive" becomes true. So the heartbeat count is
// capped, not the wait: after `MAX_SPOKEN_WAITS` lines the last one says the polling continues
// silently, and an arrival at minute nine is still announced and still staged.

/// Milliseconds between the "still waiting" heartbeats. Slow enough to be free beside a service pass
/// that runs at ~1 kHz (one `ticks()` read and one compare on every other pass), fast enough that a
/// capture shows the mechanism alive within one screenful.
const WAIT_SPEAK_MS: u64 = 10_000;
/// How many heartbeats before the wait goes quiet. Six covers the first minute — well past the
/// ~34 s at which Boot D's xHCI reported it had enumerated nothing — and then stops, because a line
/// per 10 s for the rest of a nine-minute boot would push the arrival announcement off the top of a
/// scrollback for no added information.
const MAX_SPOKEN_WAITS: u32 = 6;
/// How many staging attempts a boot may spend on a present-but-unsettled transport before the module
/// takes the deferral as this boot's answer. Bounded so the failure mode is a printed exhaustion,
/// never an unbounded mount loop against a device that answers nothing.
const MAX_STAGE_ATTEMPTS: u32 = 8;
/// Backoff between staging attempts. A mount is a real transaction against the transport; without
/// this the retry would run at service-pass rate, which is the busy spin the bound above exists to
/// make impossible in the other direction too.
const STAGE_BACKOFF_MS: u64 = 1_000;

/// `ticks()` at the first pass that found no program source. `0` = the deferral has not fired.
static DEFER_SINCE_MS: AtomicU64 = AtomicU64::new(0);
/// `ticks()` after which the next heartbeat, or the next staging attempt, may speak/run.
static NEXT_SPEAK_MS: AtomicU64 = AtomicU64::new(0);
static NEXT_ATTEMPT_MS: AtomicU64 = AtomicU64::new(0);
/// Heartbeats printed so far, and staging attempts started so far. Two counters because they measure
/// two different things: how long the volume has been missing, and how many times it was there and
/// would not answer.
static SPOKEN_WAITS: AtomicU32 = AtomicU32::new(0);
static STAGE_ATTEMPTS: AtomicU32 = AtomicU32::new(0);

// ── WIFI-REACH: the second-handle wait. ──────────────────────────────────────────────────────────
//
// The reachability bug this closes is `sdhc.md` §13.7: on the bench the program source (the internal
// SD card) is registered synchronously inside `pci::init`, before the main loop runs, so
// `program_source()` is already `Some(Sdhc)` on pass 1 and the staging entry fires there — while the
// USB stick carrying the blobs enumerates many passes later, from the deferred SCSI bring-up. At the
// moment the first staging attempt runs, `alternate_program_source()` is therefore `None`, the
// two-volume search's pass 2 finds no alternate, and the pre-WIFI-REACH code printed a terminal
// INCOMPLETE and ran arc 2 an epoch before the stick could ever be looked at.
//
// The fix holds the gate open. `stage_attempt` returns `Pending` (no terminal line) when the set is
// incomplete and no alternate handle was present; `service` moves to `S_WAIT_ALT` and re-attempts
// when the stick's storage-ready edge fires — `block::take_usb_ready()`, which has NO other x86
// consumer (its only consumer `fat::piusb27_service` is `#[cfg(target_arch = "aarch64")]`, and this
// module is x86-only, so the two live on disjoint arches and cannot race — `sdhc.md` §13.7) — or the
// deadline below expires. Unlike the program-source wait, THIS wait is BOUNDED: a second handle
// either appears in the first tens of seconds of a boot or it is not coming, and a partial set must
// eventually get its honest INCOMPLETE verdict so arc 2 can refuse and the boot can proceed.

/// How long to hold for a second populated handle after the program source came up short. 30 s is
/// well past the ~34 s at which Boot D's xHCI had already reported it enumerated nothing, while still
/// bounding the hold so a boot with no stick reaches its terminal verdict promptly.
const ALT_WAIT_MS: u64 = 30_000;

/// Absolute `ticks()` deadline for the second-handle hold. `0` until `S_WAIT_ALT` is entered; a
/// verdict is committed on the first pass at or after it.
static ALT_DEADLINE_MS: AtomicU64 = AtomicU64::new(0);
/// Set once the storage-ready edge has fired: from then on the alternate is re-attempted on the
/// staging backoff rather than only polled for, because the edge is consume-once and will not re-arm.
static ALT_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Milliseconds since boot — the calibrated 1 kHz APIC timebase (`arch::ticks()` IS ms on x86; see
/// its doc). A `u64` ms counter does not wrap in any uptime this machine will see, so the
/// `now < NEXT_*` comparisons above need no wrap handling.
///
/// (Ordering note for the statics above: single-writer throughout — `service()` is called from the
/// main loop's device-service pass, one core, one call site per boot path. `Relaxed` is the same
/// ordering `wcx`'s storage wait uses for the same reason, and no other module reads them.)
fn now_ms() -> u64 {
    crate::arch::ticks()
}

/// Drive the WiFi firmware-load path from the main loop (call every iteration; a single relaxed
/// atomic load once parked). Forward-only:
///
///   `S_START` — run the PCI census once. No subclass-0x80 Broadcom function => one honest refusal,
///   `S_PARKED` forever. The radio found => `S_WAIT_STORAGE`.
///
///   `S_WAIT_STORAGE` — the firmware set lives on a FAT volume; poll `block::program_source()` and,
///   once present, run the staging pass. A `Retry` re-arms under a bounded budget; a `Pending`
///   (WIFI-REACH: incomplete, no second handle yet) hands off to `S_WAIT_ALT`; a `Settled` — or the
///   retry budget exhausted — runs arc 2 and parks. See [`wait_storage_pass`].
///
///   `S_WAIT_ALT` — WIFI-REACH: the program source came up short and a late-publishing volume (the
///   USB stick with the blobs) may still carry the missing roles. Hold arc 2 and the terminal
///   verdict until the storage-ready edge fires or a bounded deadline expires, then re-search and
///   settle. See [`wait_alt_pass`].
///
///   `S_PARKED` — returns immediately, forever. Never panics, never blocks boot.
pub fn service() {
    match STATE.load(Ordering::Relaxed) {
        S_PARKED => (),
        S_START => {
            if bus::census() {
                STATE.store(S_WAIT_STORAGE, Ordering::Relaxed);
            } else {
                #[cfg(feature = "wifival")]
                {
                    // WVAL-REPLAY. The census has already printed its own honest refusal line above;
                    // this one names what happens INSTEAD of the park, so a capture can never show a
                    // replay boot that reads like a metal boot.
                    CENSUS_ABSENT.store(true, Ordering::Relaxed);
                    serial_println!(
                        ":: wifi: census=ABSENT — wifival REPLAY armed: staging + set-validation dry-run proceed, NO device rung will run ::"
                    );
                    STATE.store(S_WAIT_STORAGE, Ordering::Relaxed);
                }
                #[cfg(not(feature = "wifival"))]
                STATE.store(S_PARKED, Ordering::Relaxed);
            }
        }
        S_WAIT_STORAGE => wait_storage_pass(),
        S_WAIT_ALT => wait_alt_pass(),
        _ => (),
    }
}

/// `S_WAIT_STORAGE`: wait for the program-source volume, then run the staging pass and route its
/// outcome. Split out of [`service`] so `S_WAIT_ALT` can share [`finish_and_park`].
fn wait_storage_pass() {
    // The presence gate must ask the SAME question the mount will: `program_source()`, not `info()`.
    // On a machine booted from the internal SD reader the global handle is empty while a
    // program-bearing volume is mounted — gating on `info()` would park this path forever on exactly
    // the configuration that has the firmware.
    if crate::drivers::block::program_source().is_none() {
        wait_for_volume();
        return;
    }
    let now = now_ms();
    // Backoff, and ONLY after a deferred attempt: `NEXT_ATTEMPT_MS` is 0 until an attempt returns
    // `Retry`, so the first attempt on a boot whose volume was already there runs on the very pass
    // that saw it — the pre-WIFI-REARM timing, unchanged.
    if now < NEXT_ATTEMPT_MS.load(Ordering::Relaxed) {
        return;
    }
    let n = STAGE_ATTEMPTS.fetch_add(1, Ordering::Relaxed) + 1;
    if n == 1 && DEFER_ANNOUNCED.load(Ordering::Relaxed) {
        // Only on a boot that actually deferred, and only ONCE: this line reports an EVENT (the
        // volume arrived), not an attempt. Printing it per attempt would put up to eight "APPEARED
        // after waited=" lines with eight different `waited=` values in the capture, and a reader
        // counting arrivals would count eight. Attempts 2..8 are already narrated by the `staging
        // re-armed` line below and by `stage_attempt`'s own DEFERRED line, so nothing is lost.
        //
        // `n == 1` is the arrival because the counter only advances here, past the presence gate:
        // the first attempt of the boot is by construction the first pass on which a handle was
        // there to attempt against. A boot that never waited prints no line at all, because it did
        // not resume anything.
        let waited = now.saturating_sub(DEFER_SINCE_MS.load(Ordering::Relaxed));
        serial_println!(
            ":: wifi: program-source volume APPEARED after waited={}ms (handles={}) — resuming firmware staging at attempt n={}/{} ::",
            waited, crate::drivers::block::source_census(), n, MAX_STAGE_ATTEMPTS
        );
    }
    // Non-committing attempt: the program-source pass may still hand off to the second-handle wait.
    match firmware::stage_attempt(false) {
        firmware::StageOutcome::Retry(stage) if n < MAX_STAGE_ATTEMPTS => {
            // Not this boot's answer. Stay in `S_WAIT_STORAGE`, re-check the handle from the top on
            // a later pass (it may even have gone away again), and do NOT run arc 2: its completeness
            // gate would read a `staged_count()` that is still moving, which is the exact drift the
            // "arc 2 runs after staging" rule exists to prevent.
            NEXT_ATTEMPT_MS.store(now.saturating_add(STAGE_BACKOFF_MS), Ordering::Relaxed);
            serial_println!(
                ":: wifi: staging re-armed — attempt n={}/{} deferred at stage={}, next attempt in {}ms ::",
                n, MAX_STAGE_ATTEMPTS, stage, STAGE_BACKOFF_MS
            );
            return;
        }
        firmware::StageOutcome::Retry(stage) => {
            // Budget spent. The deferral becomes this boot's terminal answer, OUT LOUD — the failure
            // mode of a bounded retry must be a printed line, never a quiet downgrade to the same
            // silence WIFI-REARM exists to remove.
            serial_println!(
                ":: wifi: firmware NOT staged — {} attempts all deferred at stage={} against a present handle (handles={}); giving up staging for this boot ::",
                n, stage, crate::drivers::block::source_census()
            );
        }
        firmware::StageOutcome::Pending => {
            // WIFI-REACH: the program source was searched and came up INCOMPLETE, and there was no
            // second populated handle to try — the classic bench timing, card-early / stick-late.
            // Do NOT print a terminal verdict and do NOT run arc 2; hold for the stick.
            enter_alt_hold(now);
            return;
        }
        firmware::StageOutcome::Settled => (),
    }
    finish_and_park();
}

/// WIFI-REACH: move from `S_WAIT_STORAGE` into the bounded second-handle hold. Arms the deadline and
/// the heartbeat schedule, and prints the single HELD line that tells a capture the verdict is being
/// withheld ON PURPOSE — the honest replacement for the pre-WIFI-REACH premature INCOMPLETE.
fn enter_alt_hold(now: u64) {
    ALT_DEADLINE_MS.store(now.saturating_add(ALT_WAIT_MS), Ordering::Relaxed);
    // Re-arm the heartbeat statics for the alt phase. On the bench the program source was present
    // immediately, so `wait_for_volume` never ran and these are still at their init; on a boot where
    // both waits happen, this reset gives the alt wait its own fresh heartbeat budget.
    SPOKEN_WAITS.store(0, Ordering::Relaxed);
    NEXT_SPEAK_MS.store(now.saturating_add(WAIT_SPEAK_MS), Ordering::Relaxed);
    STATE.store(S_WAIT_ALT, Ordering::Relaxed);
    serial_println!(
        ":: wifi: firmware set INCOMPLETE on the program source and NO second populated handle present yet — HELD up to {}ms for a late-publishing volume (e.g. a USB stick carrying the b43 set); terminal verdict and arc 2 DEFERRED until its storage-ready edge fires or the deadline expires ::",
        ALT_WAIT_MS
    );
}

/// `S_WAIT_ALT`: the bounded second-handle wait. Re-search the two volumes when the USB storage-ready
/// edge fires (a stick just enumerated) or the deadline expires, and settle with exactly one terminal
/// verdict. The state does NOT leave `S_WAIT_ALT` until a `Settled`/exhausted outcome, so arc 2 still
/// runs at most once per boot and never on a moving count.
fn wait_alt_pass() {
    let now = now_ms();
    let deadline_hit = now >= ALT_DEADLINE_MS.load(Ordering::Relaxed);
    // The USB storage-ready edge: raised by `publish_usb_geometry` when a stick enumerates, and with
    // NO other x86 consumer (sdhc.md §13.7), so consuming it here is free. Once it has fired we keep
    // re-attempting on the staging backoff, because the edge is consume-once and will not re-arm.
    if crate::drivers::block::take_usb_ready() {
        if !ALT_ACTIVE.swap(true, Ordering::Relaxed) {
            NEXT_ATTEMPT_MS.store(0, Ordering::Relaxed); // attempt on this very pass
            serial_println!(
                ":: wifi: second-handle wait — USB storage-ready edge fired (handles={}); re-running the two-volume firmware search for the missing roles ::",
                crate::drivers::block::source_census()
            );
        }
    }
    let active = ALT_ACTIVE.load(Ordering::Relaxed);
    if !deadline_hit && !active {
        // Nothing has enumerated yet and the deadline has not arrived: keep the wait visible.
        alt_heartbeat(now);
        return;
    }
    if !deadline_hit && now < NEXT_ATTEMPT_MS.load(Ordering::Relaxed) {
        return; // backoff between alt attempts
    }
    let n = STAGE_ATTEMPTS.fetch_add(1, Ordering::Relaxed) + 1;
    // `commit == deadline_hit`: at/after the deadline the verdict is FORCED (no more `Pending`);
    // before it, an incomplete-with-no-alternate result is still held.
    match firmware::stage_attempt(deadline_hit) {
        firmware::StageOutcome::Retry(stage) if !deadline_hit && n < MAX_STAGE_ATTEMPTS => {
            NEXT_ATTEMPT_MS.store(now.saturating_add(STAGE_BACKOFF_MS), Ordering::Relaxed);
            serial_println!(
                ":: wifi: second-handle staging re-armed — attempt n={}/{} deferred at stage={}, next attempt in {}ms ::",
                n, MAX_STAGE_ATTEMPTS, stage, STAGE_BACKOFF_MS
            );
            return;
        }
        firmware::StageOutcome::Retry(stage) => {
            serial_println!(
                ":: wifi: firmware NOT staged — second-handle attempts exhausted or deadline reached, last deferred at stage={} (handles={}); giving up staging for this boot ::",
                stage, crate::drivers::block::source_census()
            );
        }
        firmware::StageOutcome::Pending => {
            // Only reachable with `deadline_hit == false` (a committing attempt never returns
            // `Pending`): the edge fired but the alternate is still not actually searchable (a
            // published-then-vanished stick, or a spurious re-enum). Not terminal — keep holding
            // until the deadline forces the verdict.
            return;
        }
        firmware::StageOutcome::Settled => (),
    }
    finish_and_park();
}

/// The bounded heartbeat for the second-handle wait — the `S_WAIT_ALT` twin of [`wait_for_volume`]'s
/// heartbeat, so a capture can tell a live hold from a dead one. Costs one `ticks()` read and one
/// compare per pass, and nothing once the heartbeats are spent.
fn alt_heartbeat(now: u64) {
    if now < NEXT_SPEAK_MS.load(Ordering::Relaxed) {
        return;
    }
    let spoken = SPOKEN_WAITS.fetch_add(1, Ordering::Relaxed) + 1;
    if spoken > MAX_SPOKEN_WAITS {
        // Quiet from here. Nothing is disarmed — the edge poll and the deadline check above still run
        // every pass — so this branch must never store a state or a latch.
        SPOKEN_WAITS.store(MAX_SPOKEN_WAITS + 1, Ordering::Relaxed);
        NEXT_SPEAK_MS.store(u64::MAX, Ordering::Relaxed);
        return;
    }
    NEXT_SPEAK_MS.store(now.saturating_add(WAIT_SPEAK_MS), Ordering::Relaxed);
    let remaining = ALT_DEADLINE_MS.load(Ordering::Relaxed).saturating_sub(now);
    serial_println!(
        ":: wifi: second-handle wait n={}/{} — set still incomplete, no alternate handle yet (handles={}); ~{}ms until the terminal verdict is forced{} ::",
        spoken, MAX_SPOKEN_WAITS, crate::drivers::block::source_census(), remaining,
        if spoken == MAX_SPOKEN_WAITS {
            " — last heartbeat: the hold continues SILENTLY, and an arriving stick's edge still fires the search"
        } else {
            ""
        },
    );
}

/// Run arc 2 (once, on the now-final `staged_count()`) and park permanently. Shared terminal of both
/// `S_WAIT_STORAGE` and `S_WAIT_ALT`.
///
/// Arc 2 runs HERE and only here, and the precondition is stated precisely because a reviewer will
/// check it against the code: it runs once the staging pass has reached its TERMINAL answer for this
/// boot — a `Settled`, or a `Retry` budget exhausted. `staged_count()` is final in both: PSRC's
/// two-volume `stage_attempt` can return `Retry` from its SECOND (alternate) volume after the first
/// has already staged a role, so the exhaustion arm may be reached with a PARTIAL count — what makes
/// the count final is not its value but that no further attempt runs (`S_PARKED` on the next line, so
/// `STAGED` cannot move after arc 2 reads it). A partial set is still `< FW_SET_LEN`, so arc 2's
/// completeness gate refuses exactly as it would on 0.
///
/// What is EXCLUDED is the non-terminal `Retry` and WIFI-REACH's `Pending`, both of which return
/// before reaching here: those are the cases where another attempt is still coming and the count is
/// still moving. `Pending` in particular is why the pre-WIFI-REACH code could run arc 2 on a
/// starved count an epoch before the stick — holding it in `S_WAIT_ALT` is the fix. Arc 2 therefore
/// runs at most once per boot, and never on a moving count.
///
/// WVAL-REPLAY adds ONE branch here and it is taken only when the census came up ABSENT — a state
/// arc 2 could never be reached from before, because that census outcome used to park immediately.
/// On that branch arc 2's device rungs are not merely skipped, they are not called at all: the
/// replay leg is a different function with no PCI, MMIO or window-selector access anywhere in it.
fn finish_and_park() {
    #[cfg(feature = "wifival")]
    if CENSUS_ABSENT.load(Ordering::Relaxed) {
        bringup::validate_replay();
        STATE.store(S_PARKED, Ordering::Relaxed);
        return;
    }
    #[cfg(feature = "wifi2")]
    bringup::bringup_once();
    STATE.store(S_PARKED, Ordering::Relaxed);
}

/// The `S_WAIT_STORAGE` no-volume arm: announce the deferral once, then keep the poll VISIBLE for a
/// bounded number of heartbeats. Costs one `ticks()` read and one compare per pass once the deferral
/// has fired, and nothing at all once the heartbeats are spent.
fn wait_for_volume() {
    let now = now_ms();
    if !DEFER_ANNOUNCED.swap(true, Ordering::Relaxed) {
        // `.max(1)` keeps `0` meaning "not started" even in the vanishing case where `ticks()` reads
        // 0 on the pass that defers — the same guard `wcx`'s storage wait uses.
        DEFER_SINCE_MS.store(now.max(1), Ordering::Relaxed);
        NEXT_SPEAK_MS.store(now.saturating_add(WAIT_SPEAK_MS), Ordering::Relaxed);
        // Unchanged from arc 1 except for the tail: this is the line Boot D flew, and the tail is
        // the promise the heartbeats below keep.
        serial_println!(
            ":: wifi: firmware staging deferred — no program-source block device yet (the set lives on that FAT volume); re-checking every pass, staging re-arms whenever it appears ::"
        );
        return;
    }
    if now < NEXT_SPEAK_MS.load(Ordering::Relaxed) {
        return;
    }
    let spoken = SPOKEN_WAITS.fetch_add(1, Ordering::Relaxed) + 1;
    if spoken > MAX_SPOKEN_WAITS {
        // Quiet from here. Nothing is disarmed — the `program_source()` poll above still runs on
        // every pass — so this branch must never store a state or a latch. Keep the counter from
        // wrapping on a long boot and stop reading the clock's comparison against a stale target.
        SPOKEN_WAITS.store(MAX_SPOKEN_WAITS + 1, Ordering::Relaxed);
        NEXT_SPEAK_MS.store(u64::MAX, Ordering::Relaxed);
        return;
    }
    NEXT_SPEAK_MS.store(now.saturating_add(WAIT_SPEAK_MS), Ordering::Relaxed);
    let waited = now.saturating_sub(DEFER_SINCE_MS.load(Ordering::Relaxed));
    serial_println!(
        ":: wifi: still deferred n={}/{} waited={}ms — no program-source volume yet (handles={}); the poll is LIVE{} ::",
        spoken, MAX_SPOKEN_WAITS, waited, crate::drivers::block::source_census(),
        if spoken == MAX_SPOKEN_WAITS {
            " — last heartbeat: the poll continues SILENTLY from here and an arrival is still announced and staged"
        } else {
            ""
        },
    );
}
