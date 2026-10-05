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


//! CHARTER: Kernel — driver
//!
//! KFONTPPI (rmbp-ledger B382, R83: the ppi comes from the panel's EDID, firmware or AUX, never a constant) —
//! where the one published EDID base block (`video::EDID_BLOCK`) came from, and the second way in.
//!
//! * **source** — `fw` when `video::init_edid` published a block that passed header + checksum; `aux` when the
//!   iGPU display lane's own AUX read was published because the firmware's was absent or BAD; `none` otherwise.
//! * **offer_aux** — the lane's 128 bytes, checked again here (header, sum 0 mod 256); published only where the
//!   firmware carried nothing trustworthy. Both good and different: the finding is printed, the firmware's kept.
//!   After publishing, `video::dpi::relatch_edid` re-latches a scale latched before any EDID existed — on flight
//!   23 the console's grid latched at the splash takeover, two ms before the PCI probes reached the lane.
//! * **witness** — `:: KFONTPPI: edid_src=… ppi=… scale=… cell=…x… grid=…x… -> PASS ::`, printed once after the
//!   `[kfont] load` line.

use core::sync::atomic::{AtomicU8, Ordering};

pub const SRC_NONE: u8 = 0;
pub const SRC_FW: u8 = 1;
pub const SRC_AUX: u8 = 2;

static SRC: AtomicU8 = AtomicU8::new(SRC_NONE);

/// Record the published block's source (the publisher's call).
pub fn set(src: u8) {
    SRC.store(src, Ordering::Relaxed);
}

/// The published block's source as the wire prints it.
pub fn name() -> &'static str {
    match SRC.load(Ordering::Relaxed) {
        SRC_FW => "fw",
        SRC_AUX => "aux",
        _ => "none",
    }
}
