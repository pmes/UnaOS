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

//! Filesystem layer. Arch-neutral: it builds only on the generic block device
//! ([`crate::drivers::block`]), so the same code runs on the x86_64 (Intel xHCI)
//! and aarch64 (qemu-xhci) storage paths. FAT16/FAT32 with read, in-place write,
//! grow, create/delete, and directory creation/removal (`create_dir`/`remove_dir`).
//! [`unafs`] is the native UnaFS volume: mounted read-only at BeFS-K3, read-WRITE
//! (journaled, one coherent mount) since BeFS-K4, and since K6 the kernel's dedicated
//! ATTRIBUTE volume — the durable home of the U6 owner/grants ACL (the FAT-bridge
//! `UNAFS.ATR` sidecar is retired; a fixed two-mount dispatch {FAT, UnaFS}, no VFS).

pub mod fat;

/// BOOTROOT (orin 22): find the disk this kernel is running FROM, by finding this kernel ON it.
///
/// Arch-neutral by construction and with no board `cfg` anywhere in it — that is the whole point.
/// Peter, 2026-09-08: "boot cold, boot dumb, presume nothing about the machine." The kernel is not
/// told where it came from; it enumerates every disk the board has and compares its own running
/// `.text` against candidate files until it finds itself. Design of record: the module docs.
pub mod bootdisk;

/// SDHC-4c (x86, `sdhcblk` knob): the WRITE PERMIT for the internal SD card — one published,
/// immutable LBA interval, and the single decision point every FAT-layer write to that card passes
/// through. Kept in its own file rather than inside `fat.rs` because it is the whole safety
/// argument of the arc and has to be readable end-to-end in one sitting.
#[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
pub mod sdhc4c;

#[cfg(target_arch = "aarch64")]
pub mod unafs;

/// VFS-1: the unifying virtual-filesystem spine (mount table + resolver + the
/// backend trait, with thin adapters over FAT and native UnaFS). Design of
/// record: `docs/dev/OS/09_FILESYSTEM/vfs.md`. Unconsumed this arc — the spine
/// and doc land alone so the design can be reviewed before consumers move onto
/// it (shell/syscall adoption is a follow-up).
pub mod vfs;

/// HOLOCRON (BT-BOND M1, `holocron` knob): the kernel-side classed-record store — one CRC'd file on
/// the writable FAT volume, a fixed in-RAM table, and a load/flush pair whose whole reason for
/// existing is that the record's producer (the Bluetooth chain, under the `EHCI_HID` mutex) may not
/// be the thing that writes it. Arch-neutral: it drives only [`fat`] + [`crate::hash`], so the same
/// code compiles and runs on the x86_64 and aarch64 storage paths. Design of record:
/// `docs/dev/OS/07_USB_STORAGE/usb_xhci.md` §28. DEFAULT OFF => module and call sites vanish and
/// every artifact is byte-identical.
#[cfg(feature = "holocron")]
pub mod holocron;

/// FATFIX M2 (`UNAOS_FATPERF=1`): the cost instrument for the listing and file-read paths — the
/// measurement behind Peter's "FAT contents VERY SLOW" and the double-click launch delay. See the
/// module for what it prints and why its clock is `CNTVCT_EL0`. aarch64 only, because the two
/// backends it measures are (`vfs.md` §12.4: x86 has no mount table to route through), and because
/// `us_now` reads `CNTFRQ_EL0` — an x86 `now_cycles()` is a TSC whose rate this kernel does not
/// publish, so an x86 arm would print a number in units it could not name.
#[cfg(all(feature = "fatperf", target_arch = "aarch64"))]
pub mod fatperf;

/// Bracket one VFS operation with the sector counter and the microsecond clock, emitting the single
/// `[fatperf] op=… path=… sectors=… us=…` line. Knob-off this is the identity function over `f`.
///
/// Its two call sites are line-neutral edits to lines that already existed in `vfs.rs`, because
/// `vfs.rs` IS compiled into the knob-off `kernel8.img` and panic `Location` records embed line
/// numbers (PI-DESK's measured lesson, `arroyo`'s K8_FEATS block).
///
/// ⚠ AND LINE-NEUTRALITY IS NOT SUFFICIENT — this arc measured that too, and it cost two builds.
/// The sector counter's first form was a second shim of exactly this shape, `perf_note_sectors(n)`,
/// called from `fat.rs`'s two read funnels. Knob-off it inlines to nothing and the source stayed
/// line-neutral, and the image STILL moved — `3a280f9d… -> 08535f64…`, same length, **11997 bytes
/// different**. An `#[inline(always)]` empty function is still a CALL in MIR, and `read_sector` is
/// small and inlined into most of the FAT driver, so one extra MIR statement moved the inliner's
/// cost decision and the drift cascaded through every caller. The fix is that the call must not
/// exist knob-off *at all*: `fat.rs` carries `#[cfg(all(feature = "fatperf", …))]` on the STATEMENT
/// itself, so the statement is gone before MIR, and identity is restored (measured, not reasoned).
/// This wrapper survives in that form only because measurement showed it costs nothing HERE:
/// `MountTable::read_dir`/`read` are not inline candidates the way `read_sector` is.
#[inline(always)]
pub fn perf_op<T>(_op: &str, _path: &str, f: impl FnOnce() -> T) -> T {
    #[cfg(all(feature = "fatperf", target_arch = "aarch64"))]
    {
        return fatperf::measure(_op, _path, f);
    }
    #[cfg(not(all(feature = "fatperf", target_arch = "aarch64")))]
    f()
}

/// LOGIN M1 (`login` knob, RULINGS R51): the human-user record store (`/USERS.DAT`) and the
/// login session — name, salted SHA-256 credential, home path; `login`/`logout`; the M1 fixture.
/// Arch-neutral: it drives the VFS mount table + [`crate::hash`] only. DEFAULT OFF => the module
/// and every call site vanish; both arches byte-identical. Declared at the file tail so the
/// knob-off `fs/mod.rs` line numbering above is untouched.
#[cfg(feature = "login")]
pub mod users;

/// NSGEN (LEDGER SR3): **the VFS NAMESPACE GENERATION — one arch-neutral counter that every
/// listing-changing mutation the mount table performs advances exactly once.**
///
/// SR3's defect was that Quarry's cache-invalidation stamp was `block::usb_publish_gen()` and
/// NOTHING ELSE, so the only event that could invalidate a cached directory listing was a USB
/// mass-storage arrival. A `mkdir`, an `rmdir` (RMDIR/SO18 shipped the capability the same week), an
/// `rm`, an `mv` or a write that changes a file's size left the cache serving a listing that no
/// longer matched the volume, on every board, until somebody plugged a stick in.
///
/// **Why the counter lives HERE and not in a driver.** The event Quarry needs to hear about is a
/// NAMESPACE event, not a BLOCK event: "a name appeared, vanished or changed size under some mount".
/// `drivers/block.rs` cannot see one — it never learns that `create_dir` ran. The one place every
/// namespace mutation on every board passes through is [`vfs::MountTable`]'s write half, which is
/// arch-neutral and backend-neutral by construction, so the bump sits there (through [`ns_bump`])
/// and the counter sits beside it in `fs/`. A consumer that wants BOTH facts adds the two monotone
/// numbers — which is exactly what `video/quarry/live.rs`'s `volume_gen()` now does on aarch64, so
/// the USB half SR3's row credits is preserved rather than replaced.
///
/// Monotone and never reset: a consumer compares it with the value it last saw and cares only that
/// it MOVED. `Release` on the bump / `Acquire` on the read so a reader that sees the new generation
/// also sees the mutation's own stores; no lock, so it is safe to ask on an input band (SR3's
/// invariant) and costs one relaxed-class atomic per cache access.
pub static NS_GEN: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// NSGEN: read the namespace generation. See [`NS_GEN`].
#[inline]
pub fn ns_gen() -> u64 {
    NS_GEN.load(core::sync::atomic::Ordering::Acquire)
}

/// NSGEN: advance [`NS_GEN`] iff the mutation SUCCEEDED, and hand the result straight back.
///
/// A `Result`-shaped wrapper rather than a bare `bump()` statement for two reasons, both of them
/// about the seam it is used at. (1) It makes every call site in `fs/vfs.rs` a LINE-NEUTRAL edit —
/// the dispatch line that was already there is wrapped in place, no source line is added, and no
/// `panic::Location` recorded below it in that 2.6k-line file moves (the rule `perf_op` above is
/// written to and the reason `fatperf`'s two call sites have the shape they do). (2) A REFUSED
/// mutation must not advance the generation: `mkdir` onto an existing name, a write to a read-only
/// volume or a cross-volume `mv` change no listing anywhere, and a counter that moved for them would
/// make every consumer's cache churn on exactly the operations that changed nothing. The fixture
/// asserts both polarities (`video/quarry/live.rs`'s `stamp_selftest`).
#[inline]
pub fn ns_bump<T, E>(r: Result<T, E>) -> Result<T, E> {
    if r.is_ok() {
        NS_GEN.fetch_add(1, core::sync::atomic::Ordering::Release);
    }
    r
}
