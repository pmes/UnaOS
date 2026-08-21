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

/// HOLOCRON (BT-BOND M1, `holocron` knob): the kernel-side classed-record store — one CRC'd file on
/// the writable FAT volume, a fixed in-RAM table, and a load/flush pair whose whole reason for
/// existing is that the record's producer (the Bluetooth chain, under the `EHCI_HID` mutex) may not
/// be the thing that writes it. Arch-neutral: it drives only [`fat`] + [`crate::hash`], so the same
/// code compiles and runs on the x86_64 and aarch64 storage paths. Design of record:
/// `docs/dev/OS/07_USB_STORAGE/usb_xhci.md` §28. DEFAULT OFF => module and call sites vanish and
/// every artifact is byte-identical.
#[cfg(feature = "holocron")]
pub mod holocron;
