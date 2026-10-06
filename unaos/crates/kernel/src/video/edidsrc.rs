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

/// The iGPU display lane's AUX-read EDID base block (I2C-over-AUX at 0x50, eight 16-byte reads). Published to
/// `video::EDID_BLOCK` with `src=aux` only when the firmware's block is absent or BAD and these 128 bytes pass
/// the header and the checksum; when both are good and differ, the firmware's is kept and the finding printed.
/// Prints one line, always:
/// `:: video: edid-aux hdr=<OK|BAD> sum=<OK|BAD> native=<w>x<h> ppi=<n> fw=<absent|bad|same|differs> -> <published|kept-fw|refused> relatch=<none|a->b> [console=<c>x<r>, 0x0 = not moved] ::`
pub fn offer_aux(block: &[u8; 128]) {
    let hdr_ok = block[0..8] == super::EDID_HEADER;
    let sum_ok = block.iter().fold(0u8, |acc, b| acc.wrapping_add(*b)) == 0;
    let d = &block[54..72];
    let pclk = (d[0] as u32) | ((d[1] as u32) << 8);
    let (nat_w, nat_h) = if pclk == 0 {
        (0u32, 0u32)
    } else {
        ((d[2] as u32) | (((d[4] as u32) >> 4) << 8), (d[5] as u32) | (((d[7] as u32) >> 4) << 8))
    };
    let raw = super::edid_block_raw();
    let fw = match (raw, super::edid_block()) {
        (None, _) => "absent",
        (Some(_), None) => "bad",
        (Some(_), Some(f)) if f == *block => "same",
        (Some(_), Some(_)) => "differs",
    };
    #[allow(unused_mut)]
    let mut con = (0usize, 0usize);
    let (verdict, before, after) = if !(hdr_ok && sum_ok) {
        ("refused", 0, 0)
    } else if fw == "absent" || fw == "bad" {
        *super::EDID_BLOCK.lock() = Some(*block);
        super::EDID_OK.store(true, Ordering::Release);
        set(SRC_AUX);
        let (b, a) = super::dpi::relatch_edid();
        #[cfg(all(target_arch = "x86_64", feature = "wc"))]
        if a != 0 {
            con = super::fbcon::regrid_panel().map_or((0, 0), |(_, _, c, r)| (c, r)); // the panel console follows
        }
        ("published", b, a)
    } else {
        ("kept-fw", 0, 0)
    };
    let yn = |b: bool| if b { "OK" } else { "BAD" };
    let ppi = super::dpi::edid_native().map_or(0, |(_, p)| p);
    if before == 0 {
        serial_println!(
            ":: video: edid-aux hdr={} sum={} native={}x{} ppi={} fw={} -> {} relatch=none ::",
            yn(hdr_ok), yn(sum_ok), nat_w, nat_h, ppi, fw, verdict
        );
    } else {
        serial_println!(
            ":: video: edid-aux hdr={} sum={} native={}x{} ppi={} fw={} -> {} relatch={}->{} console={}x{} ::",
            yn(hdr_ok), yn(sum_ok), nat_w, nat_h, ppi, fw, verdict,
            super::dpi::scale_str(before), super::dpi::scale_str(after), con.0, con.1
        );
    }
}

/// The KFONTPPI witness, once, after `[kfont] load` (the faces are sized by then from `ppi`):
/// `:: KFONTPPI: edid_src=<fw|aux|none> ppi=<n> scale=<s> cell=<w>x<h> grid=<c>x<r> -> PASS ::`.
/// PASS: an EDID was published and the ppi the faces were sized at is non-zero. FAIL: an EDID was published and
/// the ppi is 0. SKIP: no EDID on this boot (QEMU, a firmware and lane that read none) — not a fault.
pub fn witness(ppi: u32) {
    let src = name();
    let s2 = super::dpi::scale_x2();
    let (gw, gh) = super::text::grid_cell();
    let (pw, ph) = super::panel_info_nonblocking().map_or((0, 0), |i| (i.width, i.height));
    let verdict = match (src, ppi) {
        ("none", _) => "skipped reason=no-edid",
        (_, 0) => "declined reason=edid-without-ppi",
        _ => "armed",
    };
    serial_println!(
        ":: KFONTPPI: edid_src={} ppi={} scale={} cell={}x{} grid={}x{} -> {} ::",
        src, ppi, super::dpi::scale_str(s2), gw, gh, pw / gw.max(1), ph / gh.max(1), verdict
    );
}
