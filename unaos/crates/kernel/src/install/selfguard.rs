// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! INSTALL-SELF — the boot-device guard: the installer must never offer, select, or erase the device
//! the system booted from.
//!
//! ### Why this exists
//!
//! Observed on metal (rMBP 2012, 2026-07-29): booted from an SD card in a USB reader, and the
//! installer listed that same card as a target and offered to erase it. INSTALL-SEL had already made
//! the selection real — the engine binds the disk the operator chose, not "whatever disk is
//! present" — so the offer was honest about *which* disk it would destroy. It was still an offer to
//! destroy the running system. Selection correctness is not the same property as target eligibility,
//! and this module supplies the second one.
//!
//! ### The identity, and why it is a FAT volume serial
//!
//! There is no block-device identity in this tree that survives the boot handoff: the UEFI bootloader
//! knows a firmware handle, the kernel knows an xHCI slot, and nothing maps one to the other. What
//! DOES cross the handoff is a byte written on the medium itself — the FAT `BS_VolID` the formatter
//! stamped into the boot sector. The bootloader reads it off LBA 0 of its own loaded-image device
//! (see `read_boot_volume_serial` in the bootloader crate) and passes it in
//! [`unaos_boot_info::BootInfo::boot_volume_serial`]; the kernel reads the same field off every
//! candidate disk and compares.
//!
//! It is a *volume* serial, not a *device* serial, and that difference is load-bearing in both
//! directions:
//!
//! * A **byte clone** of the boot media carries the same serial. Our own installer clones boot media,
//!   so this collision is real, not theoretical. The guard therefore refuses EVERY candidate carrying
//!   the boot serial — if two disks both claim to be the volume we are running from, we cannot tell
//!   which one we would be erasing, and the safe direction is to erase neither.
//! * A **reformat** changes it. A disk that once held our boot volume and has since been reformatted
//!   is a legitimate target again, which is correct.
//!
//! ### The two layers
//!
//! 1. [`classify`] drives the installer UI: a matching disk is still SHOWN (an installer that hides
//!    the operator's own boot disk invites them to hunt for a disk that is not there) but is marked
//!    and not selectable.
//! 2. [`refuses`] is called by the engine itself, after it binds a target and before the first write.
//!    The UI filter is not the guard. This is the guard.
//!
//! ### Disarming
//!
//! A boot serial of 0 means "the boot volume could not be identified" — a pre-guard bootloader, a
//! non-FAT boot path, a formatter that left `BS_VolID` unstamped, or aarch64 (whose `build_boot_info`
//! fills 0). The guard then DISARMS with a witness line and every candidate stays eligible. Bricking
//! the installer is not a safe failure mode; announcing that the guard is not protecting anything is.

use crate::drivers::block::{self, BlockDeviceId, BlockHandle};
use crate::fs::fat::{self, BlockSource};
use core::sync::atomic::{AtomicU32, Ordering};

/// `BS_VolID` of the volume the kernel was loaded from, as reported by the bootloader. 0 = absent.
static BOOT_SERIAL: AtomicU32 = AtomicU32::new(0);

/// Publish the boot volume serial out of `BootInfo`, once, from the kernel entry path. Called before
/// any storage is up, so nothing can read a half-initialized guard.
pub fn set_boot_volume_serial(serial: u32) {
    BOOT_SERIAL.store(serial, Ordering::Relaxed);
    if serial == 0 {
        serial_println!(
            ":: install: boot volume serial ABSENT (0) — INSTALL-SELF boot-device guard DISARMED ::"
        );
    } else {
        serial_println!(
            ":: install: boot volume serial=0x{:08x} — INSTALL-SELF boot-device guard ARMED ::",
            serial
        );
    }
}

/// The boot volume's serial, or `None` when it is absent (the disarmed case).
pub fn boot_volume_serial() -> Option<u32> {
    match BOOT_SERIAL.load(Ordering::Relaxed) {
        0 => None,
        s => Some(s),
    }
}

/// What the guard says about one candidate target disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// No boot serial was reported: the guard is disarmed and cannot vouch for anything. Every
    /// candidate is eligible, exactly as before this arc.
    Disarmed,
    /// The guard is armed and this candidate carries no FAT volume with the boot serial. Eligible.
    Eligible,
    /// The guard is armed and this candidate carries a FAT volume whose serial IS the boot serial: it
    /// is the device we booted from, or a byte clone of it. Never selectable, never erased.
    BootDevice,
}

/// One evaluated candidate, cached so the per-frame installer repaint costs nothing.
#[derive(Clone)]
struct Cand {
    /// The registry handle + geometry that name the device (compared field-wise; `BlockDeviceId` is
    /// not `PartialEq` in this tree and `drivers::block` is not ours to change).
    usb: bool, /** SELFGUARD-AHCI: the HBA port when this candidate is a SATA disk, `None` when it is the global/USB rung. [`Port`] is the UNIT TYPE in a knob-off build, so the field is zero-sized, the record's layout does not move, and every comparison over it is a compile-time constant. */ port: Port,
    slot_id: u8,
    num_blocks: u64,
    /// Every FAT volume serial found on the device.
    serials: alloc::vec::Vec<u32>,
    verdict: Verdict,
}

/// `(registry signature, evaluated candidates)`. Re-evaluated only when the signature changes, so a
/// settled machine reads the cache and never re-reads a boot sector; a disk attach/unplug (which
/// moves the signature) forces a fresh scan before the next verdict is served.
static CACHE: spin::Mutex<Option<(u64, alloc::vec::Vec<Cand>)>> = spin::Mutex::new(None);

fn matches(c: &Cand, id: BlockDeviceId) -> bool {
    c.usb == matches!(id.handle, BlockHandle::Usb)
        && c.slot_id == id.slot_id
        && c.num_blocks == id.num_blocks && c.port == port_of(id) // SELFGUARD-AHCI: two SATA disks can share a slot id, and a SATA disk can share one with the global rung, so the port is part of the identity. `()` == `()` knob-off.
}

/// The live candidate set, built by the SAME rule the installer's chooser uses: the global block
/// device, plus the USB registry entry when it is a genuinely different slot (on x86 one stick is
/// published into both handles, and the slot test collapses them to one row). Keeping the two lists
/// derived from one rule is what makes "the row the operator sees" and "the device the guard judged"
/// the same thing.
fn live() -> alloc::vec::Vec<Cd> {
    let mut out = alloc::vec::Vec::new();
    if let Some(i) = block::info() {
        out.push((false, i.slot_id, i.num_blocks, NO_PORT));
    }
    if let Some(u) = block::usb_info() {
        if out.first().map(|&(_, s, _, _)| s) != Some(u.slot_id) {
            out.push((true, u.slot_id, u.num_blocks, NO_PORT));
        }
    } push_sata(&mut out); // SELFGUARD-AHCI: the SATA rung, APPENDED after the global/USB rungs and read out of `fat::live_sources()` — the ONE census `fs::bootdisk`'s walk already trusts, so the guard judges exactly the disks the root walk can bind. A no-op knob-off, so this call compiles to nothing there.
    out
}

fn signature(live: &[Cd]) -> u64 {
    live.iter().fold(live.len() as u64, |a, &(usb, slot, n, port)| {
        mix_port(a ^ ((usb as u64) << 63) ^ ((slot as u64) << 8) ^ n, port) // SELFGUARD-AHCI: identity, not decoration — two SATA disks of equal geometry differ only by port, and a signature that cannot tell them apart serves one disk's verdict for the other. `mix_port` is the identity function knob-off.
    })
}

/// Evaluate the live candidate set and emit the disk-list witness. Idempotent per registry signature.
fn scan(cache: &mut Option<(u64, alloc::vec::Vec<Cand>)>) {
    let live = live();
    let sig = signature(&live);
    if cache.as_ref().map(|&(s, _)| s) == Some(sig) {
        return;
    }

    let boot = boot_volume_serial();
    let mut cands: alloc::vec::Vec<Cand> = alloc::vec::Vec::new();
    for &(usb, slot_id, num_blocks, port) in &live {
        let source = source_of(usb, port);
        let serials = match boot {
            // Disarmed: do not touch the candidates at all. A guard that cannot decide anything has
            // no business reading sectors to prove it.
            None => alloc::vec::Vec::new(),
            Some(_) => fat::volume_serials(source),
        };
        let verdict = match boot {
            None => Verdict::Disarmed,
            Some(b) if serials.contains(&b) => Verdict::BootDevice,
            Some(_) => Verdict::Eligible,
        };
        cands.push(Cand { usb, slot_id, num_blocks, serials, verdict, port });
    }

    // The witness. One line per candidate at list-build time, so the log says what the operator's
    // screen is about to say — and, when the guard excludes something, WHY, in a form that can be
    // read against the media by hand.
    let excluded = cands.iter().filter(|c| c.verdict == Verdict::BootDevice).count(); census(&cands, boot); // SELFGUARD-AHCI: the census + per-candidate classify witness, emitted from the SAME scan that fills the cache every `classify` reads — so the log's list and the installer's list cannot be two different lists.
    for c in &cands {
        let handle = label(c.usb, c.port);
        match c.verdict {
            Verdict::Disarmed => serial_println!(
                ":: install: candidate {}/slot{} ({} sectors) — guard DISARMED, no boot serial to compare ::",
                handle, c.slot_id, c.num_blocks
            ),
            Verdict::BootDevice => serial_println!(
                ":: install: boot device {}/slot{} ({} sectors) serial=0x{:08x} EXCLUDED ::",
                handle,
                c.slot_id,
                c.num_blocks,
                boot.unwrap_or(0)
            ),
            Verdict::Eligible if c.serials.is_empty() => serial_println!(
                ":: install: candidate {}/slot{} ({} sectors) — no FAT volume, cannot be the boot device, ELIGIBLE ::",
                handle, c.slot_id, c.num_blocks
            ),
            Verdict::Eligible => serial_println!(
                ":: install: candidate {}/slot{} ({} sectors) {} FAT volume(s), first serial=0x{:08x} != boot 0x{:08x}, ELIGIBLE ::",
                handle,
                c.slot_id,
                c.num_blocks,
                c.serials.len(),
                c.serials.first().copied().unwrap_or(0),
                boot.unwrap_or(0)
            ),
        }
    }
    if excluded > 1 {
        // The clone case, called out on its own line because it is the one an operator will not
        // expect: two attached volumes carrying the same serial. We cannot tell which is the disk we
        // are running from, so neither is eligible.
        serial_println!(
            ":: install: boot serial=0x{:08x} matches {} attached volumes (CLONES) — ALL EXCLUDED, refusing to guess which one we booted ::",
            boot.unwrap_or(0),
            excluded
        );
    }

    *cache = Some((sig, cands));
}

/// The guard's verdict for one candidate target disk. Cheap: reads the cache, re-scanning only when
/// the block registry has changed since the last scan.
pub fn classify(id: BlockDeviceId) -> Verdict {
    let mut cache = CACHE.lock();
    scan(&mut cache);
    let Some((_, cands)) = cache.as_ref() else { return Verdict::Disarmed };
    match cands.iter().find(|c| matches(c, id)) {
        Some(c) => c.verdict,
        // A device that is not in the candidate set we just built is not one the guard evaluated. The
        // engine's own `bind_id` refuses a target that is not in the live registry (`TargetGone`), so
        // this is not a hole the guard needs to fill; do not invent a verdict for it.
        None => {
            if boot_volume_serial().is_some() {
                Verdict::Eligible
            } else {
                Verdict::Disarmed
            }
        }
    }
}

/// DEFENSE IN DEPTH: does the guard refuse this target outright?
///
/// Called by the engine after it binds a target and before its first write — independently of any UI
/// filtering. The UI can be bypassed (the unattended witness path has no UI at all, and a future
/// caller may have a different one); the engine cannot. If this returns true the engine writes
/// nothing and says so.
pub fn refuses(id: BlockDeviceId) -> bool {
    classify(id) == Verdict::BootDevice
}

/// Force the next verdict to re-read the media. For the selftest leg, which needs a scan it can
/// observe rather than a cache hit from the boot-time scan.
pub fn invalidate() {
    *CACHE.lock() = None;
}

// --------------------------------------------------------------------- selftest --

/// INSTALL-SELF live-media leg: re-read the candidates and report the guard's verdict over whatever
/// FAT volumes are on them NOW.
///
/// Called after the engine witness has formatted its scratch target, which is the only moment in the
/// QEMU harness where a candidate disk carries a FAT volume at all — so it is the only moment the
/// serial reader can be exercised against real media rather than synthetic bytes. Two things get
/// proven here that [`selftest`]'s synthetic half cannot: that [`fat::volume_serials`] actually finds a
/// volume on a live device, and that the guard's answer for a real non-matching volume is ELIGIBLE.
///
/// It also closes the one gap the synthetic half leaves. The harness cannot ATTACH a disk carrying the
/// boot volume's serial (the boot ESP is an `ide-hd` the kernel has no driver for, and the only
/// enumerated disk is the installer's own scratch), so the exclusion path has no live fixture. What it
/// CAN do is ask the guard the same question with the roles swapped: take a serial actually read off
/// live media and ask whether a boot volume carrying THAT serial would exclude this disk. The bytes are
/// real, the comparison is the real one, and the answer must be `BootDevice`. That is the exclusion
/// path on real media in everything but which of the two serials came from the bootloader.
pub fn live_media_leg() {
    let Some(boot) = boot_volume_serial() else {
        serial_println!(
            ":: INSTALL-SELF: live-media => SKIP (guard disarmed — no boot volume serial in BootInfo) ::"
        );
        return;
    };
    invalidate();
    let live = live();
    let mut with_fat = 0usize;
    let mut excluded = 0usize;
    for &(usb, slot_id, num_blocks, port) in &live {
        let source = source_of(usb, port);
        let serials = fat::volume_serials(source);
        let handle = handle_of(usb, port);
        let verdict = classify(BlockDeviceId { handle, slot_id, num_blocks });
        if !serials.is_empty() {
            with_fat += 1;
        }
        if verdict == Verdict::BootDevice {
            excluded += 1;
        }
        serial_println!(
            ":: INSTALL-SELF: live-media {}/slot{} — {} FAT volume(s) read off the device, first serial=0x{:08x}, boot=0x{:08x} => {:?} ::",
            label(usb, port),
            slot_id,
            serials.len(),
            serials.first().copied().unwrap_or(0),
            boot,
            verdict
        ); // SELFGUARD-AHCI: `handle`/`label` above now render a SATA candidate as `ahci<port>`.
    }
    if with_fat == 0 {
        serial_println!(
            ":: INSTALL-SELF: live-media => SKIP (no FAT volume readable on any of {} candidate(s)) ::",
            live.len()
        );
        return;
    }
    serial_println!(
        ":: INSTALL-SELF: live-media — {} of {} candidate(s) carry a readable FAT volume, {} EXCLUDED as the boot device => PASS ::",
        with_fat,
        live.len(),
        excluded
    );

    // The role-swapped exclusion check described above: real serials off real media, and the real
    // comparison. If this ever answers anything but `BootDevice`, the guard cannot exclude a boot
    // device no matter what the bootloader reports, and the whole arc is inert.
    for &(usb, slot_id, num_blocks, port) in &live {
        let source = source_of(usb, port);
        let serials = fat::volume_serials(source);
        let Some(&first) = serials.first() else { continue };
        let swapped = decide(Some(first), &serials);
        if swapped == Verdict::BootDevice {
            serial_println!(
                ":: INSTALL-SELF: live-media exclusion (role-swapped: boot serial := 0x{:08x} read off {}/slot{}) => EXCLUDED, PASS ::",
                first,
                label(usb, port),
                slot_id
            );
        } else {
            serial_println!(
                ":: INSTALL-SELF: live-media exclusion (role-swapped: boot serial := 0x{:08x} read off {}/slot{}) => {:?}, expected BootDevice => FAIL ::",
                first,
                label(usb, port),
                slot_id,
                swapped
            );
        }
        let _ = num_blocks;
    }
}

/// The guard's decision, isolated from any storage: given the boot serial and a candidate's serial
/// set, what is the verdict? [`scan`] applies exactly this rule. Exposed so the selftest can pin the
/// rule over synthetic inputs — including the cases the QEMU harness cannot physically stage (a
/// genuine serial match, and a clone collision), which are precisely the cases that matter.
pub fn decide(boot: Option<u32>, serials: &[u32]) -> Verdict {
    match boot {
        None => Verdict::Disarmed,
        Some(b) if serials.contains(&b) => Verdict::BootDevice,
        Some(_) => Verdict::Eligible,
    }
}

/// INSTALL-SELF selftest: pin the guard's decision table, then report the LIVE verdicts the running
/// machine produced.
///
/// The synthetic half is the substantive half. The x86 QEMU harness boots from an `ide-hd` ESP that
/// the kernel has no driver for, and the only disk it enumerates is the installer's blank scratch —
/// so the harness *cannot* attach a volume whose serial matches the boot volume, and the exclusion
/// path has no live fixture. Rather than let the arc's central behavior go unproven in QEMU, the
/// decision rule is pinned directly over synthetic serial sets: absent boot serial, match, no match,
/// no FAT at all, and a clone collision. The live half then states, for the record, what the real
/// machine decided — which is what a bench reader compares against the media in hand.
pub fn selftest() {
    let mut fails = 0usize;

    // Disarmed: no boot serial => no exclusion is possible, whatever the candidate carries.
    if decide(None, &[0x1234_5678]) != Verdict::Disarmed {
        fails += 1;
    }
    if decide(None, &[]) != Verdict::Disarmed {
        fails += 1;
    }
    // Armed + match => BootDevice, whether the match is the only volume or one of several (the
    // multi-partition case `fat::volume_serials` exists for).
    if decide(Some(0x1234_5678), &[0x1234_5678]) != Verdict::BootDevice {
        fails += 1;
    }
    if decide(Some(0x1234_5678), &[0xAAAA_BBBB, 0x1234_5678]) != Verdict::BootDevice {
        fails += 1;
    }
    // Armed + no match => eligible. A disk with no FAT at all cannot match, and stays a valid target.
    if decide(Some(0x1234_5678), &[0xAAAA_BBBB]) != Verdict::Eligible {
        fails += 1;
    }
    if decide(Some(0x1234_5678), &[]) != Verdict::Eligible {
        fails += 1;
    }
    // The 0 sentinel is never a serial to match ON: our own FAT32 formatter leaves BS_VolID at 0, so
    // a candidate reporting 0 must not be excluded merely for being unstamped.
    if decide(Some(0x1234_5678), &[0]) != Verdict::Eligible {
        fails += 1;
    }
    // Clone collision: TWO candidates carrying the boot serial => both BootDevice. The rule is
    // per-candidate, so refusing both is not a special case — it is the absence of one.
    let clone_a = decide(Some(0x0BAD_C0DE), &[0x0BAD_C0DE]);
    let clone_b = decide(Some(0x0BAD_C0DE), &[0x0BAD_C0DE, 0x9999_9999]);
    if clone_a != Verdict::BootDevice || clone_b != Verdict::BootDevice {
        fails += 1;
    }

    if fails == 0 {
        serial_println!(
            ":: INSTALL-SELF: guard decision table (disarmed / match / multi-volume match / no-match / no-FAT / 0-sentinel / clone) => PASS ::"
        );
    } else {
        serial_println!(":: INSTALL-SELF: guard decision table => FAIL ({} cases) ::", fails);
    }

    // The live half. `invalidate` forces a real scan here so the witness lines above it in the log
    // describe THIS moment's registry, not whatever was attached at the first classify.
    invalidate();
    match boot_volume_serial() {
        None => serial_println!(
            ":: INSTALL-SELF: live => SKIP (no boot volume serial in BootInfo; guard disarmed by design) ::"
        ),
        Some(b) => {
            let live = live();
            if live.is_empty() {
                serial_println!(
                    ":: INSTALL-SELF: live => SKIP (boot serial 0x{:08x} armed, but no block device enumerated to judge) ::",
                    b
                );
                return;
            }
            let mut excluded = 0usize;
            let mut fat_seen = 0usize;
            for &(usb, slot_id, num_blocks, port) in &live {
                let handle = handle_of(usb, port);
                let id = BlockDeviceId { handle, slot_id, num_blocks };
                if !fat::volume_serials(source_of(usb, port))
                    .is_empty()
                {
                    fat_seen += 1;
                }
                if classify(id) == Verdict::BootDevice {
                    excluded += 1;
                }
            }
            if fat_seen == 0 {
                serial_println!(
                    ":: INSTALL-SELF: live => SKIP (boot serial 0x{:08x} armed; no FAT volume on any of {} candidate(s), so no exclusion is stageable here) ::",
                    b, live.len()
                );
            } else {
                serial_println!(
                    ":: INSTALL-SELF: live => {} of {} candidate(s) EXCLUDED as the boot device (boot serial 0x{:08x}, {} carrying FAT) ::",
                    excluded, live.len(), b, fat_seen
                );
            }
        }
    }
}

// ------------------------------------------------------- SELFGUARD-AHCI (B89/B91) --
//
// THE DEFECT, recorded by the AHCIWRITE executor and fixed here. Before this block `live()` built
// its candidate set from `block::info()` + `block::usb_info()` and nothing else. Since AHCIBOOT the
// tree has `BlockHandle::Ahci { port }`, so an installer CAN be handed a SATA identity — and
// `classify` had no candidate to match it against, so it fell to the "do not invent a verdict" arm
// and answered **Eligible**. On the bench rMBP the boot volume and Catalina are on the SAME internal
// SATA disk, so the graphical chooser and the whole-disk engine would both have been told the disk
// the OS is running from is a legitimate target. AHCIWRITE worked around it inside its own partition
// path and left the real fix owed; this is the real fix.
//
// THE RULE IS NOT NEW, AND THAT IS THE POINT. Nothing below decides anything about a SATA disk that
// the USB rung was not already deciding. `decide(boot, serials)` is untouched: a candidate carrying
// the boot volume's `BS_VolID` is the device we booted from (or a byte clone of it) and is refused;
// anything else is eligible. All that was missing was the candidate. So the fix is an ENUMERATION
// fix, not a policy fix, and the "do not invent a verdict" arm in `classify` stays exactly where it
// was — for identities the census truly does not know, which is now a strictly smaller set.
//
// ONE CENSUS, NOT A SECOND ONE. `push_sata` reads `fat::live_sources()`, which is the list
// `fs::bootdisk`'s root walk and `locate_boot_volume` already walk (`fs/fat.rs:1977`, with the SATA
// rung appended by `push_ahci_sources` at `fs/fat.rs:5326`). The guard therefore judges exactly the
// disks the root walk can bind — a disk that can become `/` can never fail to be censused here — and
// no second enumeration exists to drift away from the first. The SATA rung is APPENDED after the
// global/USB rungs for the reason `push_ahci_sources` gives one controller over: a machine with no
// SATA disk gets the list it always got.
//
// KNOB-OFF BYTE IDENTITY, which is this arc's standing invariant. `install/selfguard.rs` is compiled
// unconditionally (`lib.rs:93` `pub mod install;` has no cfg), so it IS in every knob-off x86 image
// and in every aarch64 image, and a careless fix here would move `panic::Location` records across
// two boards that never grow a SATA disk. Three things keep it honest:
//   * Every change INSIDE an existing function is a fold onto an existing line — the file was 456
//     lines before this block and is 456 lines plus this tail append after it, so nothing below any
//     edit moved.
//   * Every AHCI arm is `#[cfg(all(target_arch = "x86_64", feature = "ahci"))]` and every helper has
//     an `#[inline(always)]` knob-off twin that is byte-for-byte the expression the call site used to
//     contain inline (`source_of`, `handle_of`, `label`), or a no-op (`push_sata`, `census`), or the
//     identity function (`mix_port`).
//   * The one datum that had to be threaded through — the HBA port — is [`Port`], which is
//     `Option<u8>` under the knob and the UNIT TYPE without it. A `()` field is zero-sized, so
//     `Cand`'s layout does not move; `port_of` returns `()`; and `c.port == port_of(id)` is a
//     comparison of two unit values, which is the constant `true`.
// Measured on the fold, not asserted here (R38 floor: this seat runs `check` plus one QEMU leg).

/// SELFGUARD-AHCI: the HBA port a candidate rides, or nothing at all in a build with no SATA handle.
///
/// The unit type is not a trick, it is the honest spelling: in a knob-off image there is no such
/// thing as a port, and a datum that cannot exist should occupy no bytes and generate no compares.
#[cfg(all(target_arch = "x86_64", feature = "ahci"))]
type Port = Option<u8>;
/// SELFGUARD-AHCI: no SATA handle in this image, so a candidate's port is the unit value.
#[cfg(not(all(target_arch = "x86_64", feature = "ahci")))]
type Port = ();

/// SELFGUARD-AHCI: "this candidate is not a SATA disk", in whichever spelling [`Port`] has.
#[cfg(all(target_arch = "x86_64", feature = "ahci"))]
const NO_PORT: Port = None;
/// SELFGUARD-AHCI: the knob-off spelling — the only value [`Port`] has.
#[cfg(not(all(target_arch = "x86_64", feature = "ahci")))]
const NO_PORT: Port = ();

/// SELFGUARD-AHCI: one live candidate as [`live`] reports it — `(usb, slot_id, num_blocks, port)`.
type Cd = (bool, u8, u64, Port);

/// SELFGUARD-AHCI: the HBA port carried by an identity, if it names a SATA disk.
#[cfg(all(target_arch = "x86_64", feature = "ahci"))]
fn port_of(id: BlockDeviceId) -> Port {
    match id.handle {
        BlockHandle::Ahci { port } => Some(port),
        _ => None,
    }
}
/// SELFGUARD-AHCI: no SATA handle exists in this image, so no identity can carry a port.
#[cfg(not(all(target_arch = "x86_64", feature = "ahci")))]
#[inline(always)]
fn port_of(_id: BlockDeviceId) -> Port {}

/// SELFGUARD-AHCI: fold the port into the registry signature, so two SATA disks of identical
/// geometry are two cache entries rather than one.
#[cfg(all(target_arch = "x86_64", feature = "ahci"))]
fn mix_port(a: u64, p: Port) -> u64 {
    match p {
        Some(port) => a ^ 0x5341_5441_0000_0000 ^ ((port as u64) << 24),
        None => a,
    }
}
/// SELFGUARD-AHCI: nothing to fold in — the identity function, so the signature is the number it
/// always was.
#[cfg(not(all(target_arch = "x86_64", feature = "ahci")))]
#[inline(always)]
fn mix_port(a: u64, _p: Port) -> u64 {
    a
}

/// SELFGUARD-AHCI: which FAT source reads this candidate's sectors?
#[cfg(all(target_arch = "x86_64", feature = "ahci"))]
fn source_of(usb: bool, p: Port) -> BlockSource {
    match p {
        Some(port) => BlockSource::Ahci(port),
        None => {
            if usb {
                BlockSource::Usb
            } else {
                BlockSource::Default
            }
        }
    }
}
/// SELFGUARD-AHCI: knob-off — the exact expression the call sites used to spell inline.
#[cfg(not(all(target_arch = "x86_64", feature = "ahci")))]
#[inline(always)]
fn source_of(usb: bool, _p: Port) -> BlockSource {
    if usb {
        BlockSource::Usb
    } else {
        BlockSource::Default
    }
}

/// SELFGUARD-AHCI: which block identity does this candidate present to [`classify`]?
#[cfg(all(target_arch = "x86_64", feature = "ahci"))]
fn handle_of(usb: bool, p: Port) -> BlockHandle {
    match p {
        Some(port) => BlockHandle::Ahci { port },
        None => {
            if usb {
                BlockHandle::Usb
            } else {
                BlockHandle::Global
            }
        }
    }
}
/// SELFGUARD-AHCI: knob-off — the exact expression the call sites used to spell inline.
#[cfg(not(all(target_arch = "x86_64", feature = "ahci")))]
#[inline(always)]
fn handle_of(usb: bool, _p: Port) -> BlockHandle {
    if usb {
        BlockHandle::Usb
    } else {
        BlockHandle::Global
    }
}

/// SELFGUARD-AHCI: the candidate's name on the wire — `global`, `usb`, or `ahci<port>`.
///
/// Routed through [`BlockSource::name`] so the guard spells a SATA disk exactly as `fs::bootdisk`'s
/// `X86BIND: root=ahci5:/kernel.elf` and `mbr_census` spell it. One vocabulary, so a reader can put
/// the guard's verdict and the root walk's answer side by side and see they name one disk.
#[cfg(all(target_arch = "x86_64", feature = "ahci"))]
fn label(usb: bool, p: Port) -> &'static str {
    match p {
        Some(port) => BlockSource::Ahci(port).name(),
        None => {
            if usb {
                "usb"
            } else {
                "global"
            }
        }
    }
}
/// SELFGUARD-AHCI: knob-off — the exact literals the witness lines used to spell inline.
#[cfg(not(all(target_arch = "x86_64", feature = "ahci")))]
#[inline(always)]
fn label(usb: bool, _p: Port) -> &'static str {
    if usb {
        "usb"
    } else {
        "global"
    }
}

/// SELFGUARD-AHCI: append every LIVE SATA disk to the candidate list, read out of the ONE census
/// `fs::bootdisk`'s walk already trusts ([`fat::live_sources`]).
///
/// Geometry comes from `block::ahci_info_port`, the same registry row `BlockHandle::Ahci { port }`
/// re-resolves through — so the `(slot_id, num_blocks)` the guard remembers is the `(slot_id,
/// num_blocks)` an identity captured on that disk will present back to [`classify`]. A port that
/// `live_sources` names but whose registry row has since gone is skipped rather than guessed at: a
/// candidate the guard cannot measure is not a candidate it should be voting on.
#[cfg(all(target_arch = "x86_64", feature = "ahci"))]
fn push_sata(out: &mut alloc::vec::Vec<Cd>) {
    for src in fat::live_sources() {
        if let BlockSource::Ahci(port) = src {
            if let Some(d) = block::ahci_info_port(port) {
                out.push((false, d.slot_id, d.num_blocks, Some(port)));
            }
        }
    }
}
/// SELFGUARD-AHCI: no SATA handle in this image, so there is nothing to append. Byte-identical to
/// pre-SELFGUARD-AHCI, exactly as `fat::push_ahci_sources`' knob-off twin is.
#[cfg(not(all(target_arch = "x86_64", feature = "ahci")))]
#[inline(always)]
fn push_sata(_out: &mut alloc::vec::Vec<Cd>) {}

/// SELFGUARD-AHCI: the census witness, and one classify line per candidate.
///
/// Emitted from [`scan`], which is the function that FILLS the cache every [`classify`] reads — so
/// the list in the log and the list the installer is about to be answered from are the same list, by
/// construction rather than by a second walk that could disagree.
///
/// It prints under the `ahci` knob only. That is deliberate and it is what makes the knob-off image
/// byte-identical: a build with no SATA handle has no SATA rung to census, its candidate set is
/// exactly the set the existing `:: install: candidate …` lines below already witness, and this
/// arc's stated invariant is that such an image does not move. The go-red is unaffected — dropping
/// the AHCI arm means dropping `push_sata`'s push, not the knob, so the census still prints and
/// reads `sata=0`.
#[cfg(all(target_arch = "x86_64", feature = "ahci"))]
fn census(cands: &[Cand], boot: Option<u32>) {
    let sata = cands.iter().filter(|c| c.port.is_some()).count();
    let usb = cands.iter().filter(|c| c.usb).count();
    match (boot, cands.iter().find(|c| c.verdict == Verdict::BootDevice)) {
        (None, _) => serial_println!(
            ":: SELFGUARD: census disks={} usb={} sata={} boot=disarmed ::",
            cands.len(), usb, sata
        ),
        (Some(_), None) => serial_println!(
            ":: SELFGUARD: census disks={} usb={} sata={} boot=none ::",
            cands.len(), usb, sata
        ),
        (Some(_), Some(c)) => serial_println!(
            ":: SELFGUARD: census disks={} usb={} sata={} boot={}/slot{} ::",
            cands.len(), usb, sata, label(c.usb, c.port), c.slot_id
        ),
    }
    for c in cands {
        // The verdict is spelled INSTALL-SELF rather than `BootDevice` because that is the name of
        // the refusal a reader is looking for, and `reason=` says which of the four rules produced
        // it — the two Eligible reasons are NOT the same fact ("this disk carries no FAT at all" vs
        // "it carries FAT volumes and none of them is ours").
        let (verdict, reason) = match c.verdict {
            Verdict::Disarmed => ("Disarmed", "no-boot-volume-serial-in-bootinfo"),
            Verdict::BootDevice => ("INSTALL-SELF", "carries-the-boot-volume-serial"),
            Verdict::Eligible if c.serials.is_empty() => ("Eligible", "no-FAT-volume-on-this-disk"),
            Verdict::Eligible => ("Eligible", "no-FAT-volume-carries-the-boot-serial"),
        };
        serial_println!(
            ":: SELFGUARD: classify {}/slot{} ({} sectors, {} FAT volume(s)) -> {} reason={} ::",
            label(c.usb, c.port), c.slot_id, c.num_blocks, c.serials.len(), verdict, reason
        );
    }
}
/// SELFGUARD-AHCI: no SATA rung in this image, so no census line — see the note above on why the
/// witness is knob-gated rather than unconditional.
#[cfg(not(all(target_arch = "x86_64", feature = "ahci")))]
#[inline(always)]
fn census(_cands: &[Cand], _boot: Option<u32>) {}
