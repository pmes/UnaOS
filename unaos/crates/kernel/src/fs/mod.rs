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
