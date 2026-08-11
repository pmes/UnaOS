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
//   * **Arc 2 — bcma core enumeration + core reset.** Map BAR0, walk the on-chip core table, find the
//     d11 802.11 core + its wrapper, run §S4's prologue, and stream the staged microcode through the
//     SHM indirect window. This is the first arc that WRITES the device.
//   * **Arc 3 — up to an association attempt.** PHY/radio init from the staged initvals, a receive
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
// **The clean-room claim below is about `src/wifi/` and its implementer, and nothing wider.** These
// three files were written without reading any GPL Linux WiFi driver source; their factual inputs are
// the public PCI base specification, the public PCI-SIG vendor registry, and this tree's own metal
// captures and prose in `bcm4331.md`. That is NOT the sourcing of the sibling module: `drivers/bcma.rs`
// states in its own header that its "register offsets and EROM encodings follow Linux `drivers/bcma`
// (`bcma_regs.h`, `bcma_driver_chipcommon.h`, `scan.c`) and `b43`'s `B43_MMIO_PHY_VER`", and
// `bcm4331.md` §S4 says its upload sequence is "transcribed from the b43 reference implementation's
// *interface*". Anyone extending this module across that boundary inherits `CLEAN_ROOM_POLICY.md`
// §2's two-team rule and should say which side they are on; this file does not launder the sibling's
// sourcing and does not claim a subsystem-wide clean room.
//
// Where a fact could not be pinned from a source legal for THIS implementer it is named UNKNOWN in
// the comments and reported by a witness rather than assumed by the code. Exactly one such fact
// survives arc 1 — the firmware container header layout, on which `bcm4331.md` §S4's own record is
// internally inconsistent — and `firmware::classify_header` reports it instead of resolving it.
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
            STATE.store(S_PARKED, Ordering::Relaxed);
        }
    }
}
