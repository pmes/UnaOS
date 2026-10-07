// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! WIFI6 (rmbp-ledger B439) — rung S4i's MMIO leg: the IN-BOOT unwind of a failed S5 step.
//!
//! CHARTER: Kernel — driver. A child of `bringup`; its pre-image is C2 (WIFI5 M3, `capture.rs`), the
//! EFI's own MMIO words captured on the same boot BEFORE the upload — no second capture, no second
//! store. The one S5 write step the tree has is S5i (`apply_initvals`, bcm4331.md §8).
//!
//! * **Post-check (`wifi3`, read-only):** after the initvals, MACCTL is re-read. bcm4331.md §8 row
//!   S5i names the refutation: "PSM stops (psm-run=0) or mismatch ≈ records" — written here as
//!   `psm-run=1 ∧ 2·mismatch < records`. One line, PASS or FAIL, every staged boot.
//! * **Unwind (`wifi5`, `UNAOS_WIFI5=1`, default OFF):** only on FAIL. Every distinct offset the
//!   step wrote (the SAME staged record list `apply_initvals` walked) gets C2's word back, in reverse
//!   write order, and is re-read. NOT written: offsets C2 did not capture (its read-side-effect skips
//!   and anything past its bound) and the PHY/radio port block 0x3E0..0x3FF (`D11_PHY_VER` 0x3E0 ..
//!   the radio/PHY data ports — no PHY register is written by this arc). Both are counted on the line.
//!   The ucode leg (streaming C1 back) is NOT here: it waits on C1 reading NON-DISTURBING on metal.
//!   The reboot (S4u, confirmed f13→f14) stays the unwind of last resort.

use super::{r32, Writes, D11_MACCTL, MACCTL_PSM_RUN};
#[cfg(feature = "wifi5")]
use super::{r16, w16_u, w32};

/// The PHY/radio port block: never written by the unwind.
#[cfg(feature = "wifi5")]
fn phy_block(off: u16) -> bool {
    (0x3E0..0x400).contains(&off)
}

/// Called right after `apply_initvals` on the `-> UPLOADED` path.
pub(super) fn after_initvals(bar0: u64, wrote: u32, mismatch: u32, w: &mut Writes) {
    let macctl = unsafe { r32(bar0, D11_MACCTL) };
    let psm = (macctl & MACCTL_PSM_RUN) != 0;
    let pass = wrote > 0 && psm && mismatch.saturating_mul(2) < wrote;
    let armed = cfg!(feature = "wifi5");
    serial_println!(
        "[wifi6] s5i post-check records={} mismatch={} macctl={:#010x} psm-run={} want=1 -> {} unwind={} (bcm4331.md §8 S5i refute: psm stops or mismatch near records)",
        wrote, mismatch, macctl, psm as u8,
        if wrote == 0 { "NOT-RUN" } else { crate::tests::boot_word(pass) }, // SMALLFIX7 (B501): a record at boot
        if pass || wrote == 0 { "not-needed" } else if armed { "RUNNING" } else { "not-armed(UNAOS_WIFI5 off; the reboot S4u is the unwind)" }
    );
    if armed {
        super::super::status::set_unwind(1);
    }
    if pass || wrote == 0 {
        return;
    }
    #[cfg(feature = "wifi5")]
    restore(bar0, w);
    #[cfg(not(feature = "wifi5"))]
    let _ = w;
}

#[cfg(feature = "wifi5")]
fn restore(bar0: u64, w: &mut Writes) {
    use super::capture::c2_word;
    // The step's touched set, in write order, from the same staged records it walked.
    let mut touched: alloc::vec::Vec<(u16, bool)> = alloc::vec::Vec::new();
    for role in ["initvals", "bsinitvals"] {
        if let Some(Ok(recs)) = super::super::firmware::with_staged(role, |d| wifi_core::fw::records(d)) {
            for r in &recs {
                touched.retain(|(o, _)| *o != r.offset);
                touched.push((r.offset, r.wide));
            }
        }
    }
    let n = touched.len() as u32;
    let (mut restored, mut bad, mut phy, mut uncap) = (0u32, 0u32, 0u32, 0u32);
    for (off, wide) in touched.iter().rev() {
        let (off, wide) = (*off, *wide);
        if phy_block(off) || (wide && phy_block(off.wrapping_add(2))) {
            phy += 1;
            continue;
        }
        if wide {
            let (Some(lo), Some(hi)) = (c2_word(off), c2_word(off.wrapping_add(2))) else { uncap += 1; continue };
            let v = ((hi as u32) << 16) | lo as u32;
            unsafe { w32(bar0, off as u64, v) };
            w.core_regs += 1;
            if unsafe { r32(bar0, off as u64) } != v { bad += 1; }
        } else {
            let Some(v) = c2_word(off) else { uncap += 1; continue };
            unsafe { w16_u(bar0, off as u64, v) };
            w.core_regs += 1;
            if unsafe { r16(bar0, off as u64) } != v { bad += 1; }
        }
        restored += 1;
    }
    let verified = restored == n && bad == 0;
    let macctl = unsafe { r32(bar0, D11_MACCTL) };
    serial_println!(
        "[wifi6] unwind step=s5i restored={}/{} verified={} readback-differs={} excluded-phy-block={} uncaptured={} macctl-after={:#010x} psm-run={} — pre-image C2 (observed=<efi> source=capture), reverse write order",
        restored, n, verified as u8, bad, phy, uncap, macctl, ((macctl & MACCTL_PSM_RUN) != 0) as u8
    );
    super::super::status::set_unwind(if verified { 2 } else { 3 });
}
