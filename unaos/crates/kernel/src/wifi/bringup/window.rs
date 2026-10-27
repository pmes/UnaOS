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
//! through aperture 0 alone. The bring-up's own moves (wifi2 R2-R8, `bringup.rs` `bringup_once`/`explore`/`reach_d11`) are
//! `move_to` (R3/R4/R6, a chain) and ONE `restore` (R8, the recorded pre-image) (WINDOW-AUDIT rmbp-ledger B970).
//!
//! Preconditions of every caller: the d11 OWNED (the Station taken out of `STATION`, so nothing else reaches BAR0+0), the MAC
//! suspended (`rx::suspend`, as `join::retune` does), no spinlock held. While the window is moved, BAR0+0 is NOT the d11.

use crate::arch::pci::{read_config_32, write_config_32};

/// The window's four operations, so the move can be judged against a mock (the `pmu` KAT row) as well as the device.
/// `r32`/`w32` take an offset inside aperture 0 (< 0x1000).
#[cfg_attr(not(feature = "wifi9"), allow(dead_code))]
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
/// (WINDOW-AUDIT B970: the two halves are `move_to` and `restore`; the bus trace is unchanged.)
#[cfg_attr(not(feature = "wifi9"), allow(dead_code))]
pub(super) fn with_core<B: Bus + ?Sized, R>(b: &mut B, core: u32, f: impl FnOnce(&mut B) -> R) -> Result<R, &'static str> {
    let pre = match move_to(b, core) {
        Ok(pre) => pre,
        Err(e) => {
            b.set_cfg80(e.prev);
            return Err("selector-did-not-take");
        }
    };
    let r = f(b);
    if restore(b, pre).is_err() {
        return Err("restore-mismatch");
    }
    Ok(r)
}

// ── WINDOW-AUDIT (rmbp-ledger B970): the explicit shape — a move and its restore as two calls ──────────────────────────────────

/// A move whose selector did not read back the core: `prev` is the selector before the write, `readback` what it read after.
pub(super) struct Refused {
    pub prev: u32,
    pub readback: u32,
}

/// `cfg:0x80` <- `core`, its readback checked. `Ok(prev)`: the selector before the write. `Err(Refused)`: the readback was not
/// `core` — NOTHING is written back; the caller owns its restore (the wifi2 ladder's is R8, `with_core`'s is its own).
pub(super) fn move_to<B: Bus + ?Sized>(b: &mut B, core: u32) -> Result<u32, Refused> {
    let prev = b.cfg80();
    b.set_cfg80(core);
    let readback = b.cfg80();
    if readback != core {
        return Err(Refused { prev, readback });
    }
    Ok(prev)
}

/// `cfg:0x80` <- `prev`, its readback checked. `Err(readback)`: the selector did not read back `prev`.
pub(super) fn restore<B: Bus + ?Sized>(b: &mut B, prev: u32) -> Result<(), u32> {
    b.set_cfg80(prev);
    let readback = b.cfg80();
    if readback != prev {
        return Err(readback);
    }
    Ok(())
}

#[cfg(feature = "wifi10")]
const KAT_D11: u32 = 0x1800_1000;
#[cfg(feature = "wifi10")]
const KAT_CC: u32 = 0x1800_0000;
#[cfg(feature = "wifi10")]
const KAT_EROM: u32 = 0x1800_3000;

/// A selector behind a mock: every selector write traced (the first 8), every aperture access counted.
#[cfg(feature = "wifi10")]
struct Mock {
    cfg: u32,
    takes: bool,
    writes: [u32; 8],
    n: usize,
    mmio: u32,
}

#[cfg(feature = "wifi10")]
impl Mock {
    fn new(takes: bool) -> Mock {
        Mock { cfg: KAT_D11, takes, writes: [0; 8], n: 0, mmio: 0 }
    }
    fn trace(&self, want: &[u32]) -> bool {
        self.n == want.len() && self.writes[..self.n] == *want
    }
}

#[cfg(feature = "wifi10")]
impl Bus for Mock {
    fn cfg80(&mut self) -> u32 {
        self.cfg
    }
    fn set_cfg80(&mut self, v: u32) {
        if self.n < self.writes.len() {
            self.writes[self.n] = v;
        }
        self.n += 1;
        if self.takes {
            self.cfg = v;
        }
    }
    fn r32(&mut self, _off: u64) -> u32 {
        self.mmio += 1;
        0
    }
    fn w32(&mut self, _off: u64, _v: u32) {
        self.mmio += 1;
    }
}

/// `window` (WINDOW-AUDIT B970): the wifi2 ladder's chain from the d11 pre-image — move_to(CC) = Ok(d11), move_to(EROM page) =
/// Ok(CC), move_to(d11) = Ok(EROM), restore(d11) = Ok, trace [CC, EROM, d11, d11], ONE restore closes it; a selector that does not
/// take -> Err{prev d11, readback d11}, trace [CC] (no write-back), no MMIO; `with_core` refused -> trace [CC, d11], `f` not run,
/// no MMIO, `selector-did-not-take`; `with_core` ok -> trace [CC, d11], `f` saw CC; a restore that does not read back -> Err(readback).
#[cfg(feature = "wifi10")]
pub(in crate::wifi) fn kat() -> bool {
    let mut m = Mock::new(true);
    let mut ok = matches!(move_to(&mut m, KAT_CC), Ok(KAT_D11)) && matches!(move_to(&mut m, KAT_EROM), Ok(KAT_CC))
        && matches!(move_to(&mut m, KAT_D11), Ok(KAT_EROM)) && restore(&mut m, KAT_D11).is_ok()
        && m.trace(&[KAT_CC, KAT_EROM, KAT_D11, KAT_D11]) && m.cfg == KAT_D11 && m.mmio == 0;

    let mut m = Mock::new(false);
    ok &= matches!(move_to(&mut m, KAT_CC), Err(Refused { prev: KAT_D11, readback: KAT_D11 })) && m.trace(&[KAT_CC]) && m.mmio == 0;

    let mut m = Mock::new(false);
    ok &= with_core(&mut m, KAT_CC, |b| b.r32(0)) == Err("selector-did-not-take") && m.trace(&[KAT_CC, KAT_D11]) && m.mmio == 0;

    let mut m = Mock::new(true);
    ok &= with_core(&mut m, KAT_CC, |b| b.cfg80()) == Ok(KAT_CC) && m.trace(&[KAT_CC, KAT_D11]) && m.cfg == KAT_D11;

    let mut m = Mock::new(true);
    ok &= move_to(&mut m, KAT_CC).is_ok();
    m.takes = false;
    ok &= restore(&mut m, KAT_D11) == Err(KAT_CC) && m.trace(&[KAT_CC, KAT_D11]);
    ok
}
