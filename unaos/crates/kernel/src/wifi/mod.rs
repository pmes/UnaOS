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
//   2. `firmware::stage_once()` — find the user-supplied firmware SET on the program-source FAT
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
// the comments and reported by a witness rather than assumed by the code. Two such facts stand: the
// firmware container header layout, on which `bcm4331.md` §S4's own record is internally inconsistent
// (`firmware::classify_header` reports it instead of resolving it), and the `B43_SHM_UCODE` routing
// value, which no source available to this module records at all and on which arc 2's upload rung
// refuses.
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
// and the forward-only state machine below speaks exactly once whichever one runs.

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

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

/// Where `service()` is in its (strictly forward-only) state machine. There is no path back to an
/// earlier state and no path out of `Parked`, so a failure can never become a retry storm.
const S_START: u8 = 0;
/// Census done and the radio identified; waiting for a program-source block device to appear.
const S_WAIT_STORAGE: u8 = 1;
/// Terminal. Either the census refused, or the staging pass has completed.
const S_PARKED: u8 = 2;

static STATE: AtomicU8 = AtomicU8::new(S_START);
static DEFER_ANNOUNCED: AtomicBool = AtomicBool::new(false);

/// Drive the WiFi firmware-load path from the main loop (call every iteration; a single relaxed
/// atomic load once parked). Forward-only:
///
///   `S_START` — run the PCI census once. No subclass-0x80 Broadcom function => one honest refusal,
///   `S_PARKED` forever. The radio found => `S_WAIT_STORAGE`.
///
///   `S_WAIT_STORAGE` — the firmware set lives on the program-source FAT volume, which on x86 is the
///   USB-storage block device that enumerates asynchronously. Announce the deferral ONCE (so a boot
///   where storage never comes up still has an honest last word), then poll
///   `block::program_source()` cheaply until it is present, run the staging pass exactly once, and
///   park on whatever it reported.
///
///   `S_PARKED` — returns immediately, forever. Never retries, never panics, never blocks boot.
pub fn service() {
    match STATE.load(Ordering::Relaxed) {
        S_PARKED => (),
        S_START => {
            if bus::census() {
                STATE.store(S_WAIT_STORAGE, Ordering::Relaxed);
            } else {
                STATE.store(S_PARKED, Ordering::Relaxed);
            }
        }
        _ => {
            // The presence gate must ask the SAME question the mount will: `program_source()`, not
            // `info()`. On a machine booted from the internal SD reader the global handle is empty
            // while a program-bearing volume is mounted — gating on `info()` would park this path
            // forever on exactly the configuration that has the firmware.
            if crate::drivers::block::program_source().is_none() {
                if !DEFER_ANNOUNCED.swap(true, Ordering::Relaxed) {
                    serial_println!(
                        ":: wifi: firmware staging deferred — no program-source block device yet (the set lives on that FAT volume) ::"
                    );
                }
                return;
            }
            firmware::stage_once();
            // Arc 2 runs HERE and only here — after the staging pass, so `staged_count()` is final
            // and arc 2's completeness gate reads a settled number rather than a race. It is inside
            // the same forward-only step, so it runs at most once per boot and the state moves to
            // `S_PARKED` whatever it reports.
            //
            // One consequence, stated rather than discovered: a boot where the program-source block
            // device never appears parks in `S_WAIT_STORAGE` and never reaches arc 2 at all. That is
            // the SAME deferral arc 1 already announces once on its own line, and the alternative —
            // walking the backplane on a timer while storage is still enumerating — would put a
            // window write in a race with the very pass that decides whether an upload may follow.
            #[cfg(feature = "wifi2")]
            bringup::bringup_once();
            STATE.store(S_PARKED, Ordering::Relaxed);
        }
    }
}
