// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — driver (the BCM4331 backplane window; no CODEX §2 handler owns a bus selector). STATIONROAM-PMU
//! (rmbp-ledger B967): THE scoped `cfg:0x80` move — the selector pointed at another core for the length of one closure, then
//! put back on the core it held (the d11, `live::handle`'s proof) and the restore read back, BEFORE returning.
//!
//! BAR0 is two 4 KiB apertures (`super::BAR0_MAP_LEN`): BAR0+0 decodes the backplane address in `cfg:0x80`
//! (`super::CFG_BAR0_WIN`), BAR0+0x1000 the d11 wrapper (`cfg:0xAC`, never touched here). A core's registers are 4 KiB
//! (`super::BCMA_CORE_SIZE`), so ChipCommon (`super::BCMA_ADDR_BASE`), its PMU block (cc+0x600..0x668) included, is reachable
//! through aperture 0 alone. The bring-up's own moves (wifi2 R2-R8, `bringup.rs` `bringup_once`/`explore`/`reach_d11`) stay
//! inline for now (WINDOW-AUDIT owed).
//!
//! Preconditions of every caller: the d11 OWNED (the Station taken out of `STATION`, so nothing else reaches BAR0+0), the MAC
//! suspended (`rx::suspend`, as `join::retune` does), no spinlock held. While the window is moved, BAR0+0 is NOT the d11.

use crate::arch::pci::{read_config_32, write_config_32};

/// The window's four operations, so the move can be judged against a mock (the `pmu` KAT row) as well as the device.
/// `r32`/`w32` take an offset inside aperture 0 (< 0x1000).
pub(super) trait Bus {
    fn cfg80(&mut self) -> u32;
    fn set_cfg80(&mut self, v: u32);
    fn r32(&mut self, off: u64) -> u32;
    fn w32(&mut self, off: u64, v: u32);
}

/// The device: the radio's PCI function (config space) and the mapped BAR0 VA, both from `live::handle()`.
pub(super) struct Pci {
    pub bus: u8,
    pub dev: u8,
    pub func: u8,
    pub bar0: u64,
}

impl Bus for Pci {
    fn cfg80(&mut self) -> u32 {
        unsafe { read_config_32(self.bus, self.dev, self.func, super::CFG_BAR0_WIN) }
    }
    fn set_cfg80(&mut self, v: u32) {
        unsafe { write_config_32(self.bus, self.dev, self.func, super::CFG_BAR0_WIN, v) };
    }
    fn r32(&mut self, off: u64) -> u32 {
        unsafe { super::r32(self.bar0, off & 0x0FFF) }
    }
    fn w32(&mut self, off: u64, v: u32) {
        unsafe { super::w32(self.bar0, off & 0x0FFF, v) };
    }
}

/// `cfg:0x80` <- `core`, its readback checked; `f` over aperture 0; `cfg:0x80` <- the pre-image, its readback checked.
/// `Err("selector-did-not-take")`: the selector did not read back `core` — the pre-image is written back and `f` is NOT run.
/// `Err("restore-mismatch")`: `f` ran but the selector did not read back the pre-image (BAR0+0 is not the d11: the caller loses).
pub(super) fn with_core<B: Bus + ?Sized, R>(b: &mut B, core: u32, f: impl FnOnce(&mut B) -> R) -> Result<R, &'static str> {
    let pre = b.cfg80();
    b.set_cfg80(core);
    if b.cfg80() != core {
        b.set_cfg80(pre);
        return Err("selector-did-not-take");
    }
    let r = f(b);
    b.set_cfg80(pre);
    if b.cfg80() != pre {
        return Err("restore-mismatch");
    }
    Ok(r)
}
