// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — driver (the BCM4331 ChipCommon PMU's spur-avoid PLL step; no CODEX §2 handler owns a PMU register).
//! STATIONROAM-PMU (rmbp-ledger B967), R110: Linux v6.11 `drivers/bcma/driver_chipcommon_pmu.c` + `drivers/net/wireless/broadcom/
//! b43/main.c` (GPL-2.0), every register through `window::with_core` on ChipCommon (`super::BCMA_ADDR_BASE`, aperture 0).
//!
//! - Which core holds the PMU: `bcma_pmu_early_init` (`driver_chipcommon_pmu.c:275-290`) — ChipCommon rev >= 35 AND
//!   `BCMA_CC_CAP_EXT` (cc+0xAC) & `AOB_PRESENT` (0x40) = a separate PMU core; else the PMU IS ChipCommon (its block at cc+0x600).
//!   The ChipCommon rev is the metal EROM capture (core[0] rev=37, `bcm4331.md` §S2r; PMU-CCREV owed: nothing latches it live).
//!   The PMU's presence: `BCMA_CC_CAP` (cc+0x04) bit 28; its revision: `BCMA_CC_PMU_CAP` (cc+0x604) & 0xFF
//!   (`bcma_driver_chipcommon.h:21`, `:50`, `:221-226`, `:324-367`).
//! - The PLL update: `bcma_pmu_spuravoid_pllupdate` (`:493-568`), the BCM4331/43431 case (`:534-551`): PLL_CTL0 and PLL_CTL2
//!   through PLLCTL_ADDR/DATA (0x660/0x664, `bcma_pmu_spuravoid_pll_write` `:486-490`), then PMU_CTL |= PLL_UPD (0x400, `:566-567`).
//! - The PHY PLL reset: `b43_wireless_core_phy_pll_reset` (`main.c:1202-1216`, BCMA arm): CHIPCTL_ADDR (0x650) <- 0, CHIPCTL_DATA
//!   (0x654) &= ~4, |= 4, &= ~4.
//!
//! The mode b43 is in is `htphyinit::tune`'s (it latches `set_mode` on a PLL readback match); the boot's 0 is the spur-0 PLL the
//! boot tuned channel 11 under (no PMU write at boot).

use core::sync::atomic::{AtomicU8, Ordering};

use super::window::{with_core, Bus};

const CC_ID: u64 = 0x000;
const CC_CAP: u64 = 0x004;
const CAP_PMU: u32 = 0x1000_0000;
const CC_CAP_EXT: u64 = 0x0AC;
const CAP_EXT_AOB: u32 = 0x0000_0040;
const PMU_CTL: u64 = 0x600;
const PMU_CTL_PLL_UPD: u32 = 0x0000_0400;
const PMU_CAP: u64 = 0x604;
const PMU_CAP_REV: u32 = 0xFF;
const CHIPCTL_ADDR: u64 = 0x650;
const CHIPCTL_DATA: u64 = 0x654;
const PLLCTL_ADDR: u64 = 0x660;
const PLLCTL_DATA: u64 = 0x664;
/// `BCMA_CHIP_ID_BCM4331` — ChipCommon's id[15:0].
const CHIP_4331: u32 = 0x4331;
/// `[METAL]` ChipCommon's core revision, the EROM walk's core[0] (`bcm4331.md` §S2r: rev=37).
pub(super) const CC_REV_METAL: u8 = 37;
/// `bcma_pmu_early_init`: the AOB / separate-PMU-core rule applies from this ChipCommon revision.
const CC_REV_AOB: u8 = 35;
/// `b43_wireless_core_phy_pll_reset`'s CHIPCTL bit.
const CHIPCTL_PLL_RESET: u32 = 0x4;

/// What `probe` read on ChipCommon (zeros when the window did not move).
#[derive(Default, Clone, Copy)]
pub(super) struct Caps {
    pub chipid: u32,
    pub cap: u32,
    pub cap_ext: u32,
    pub pmu_rev: u8,
}

/// One PLL update: PLL_CTL0/CTL2 before and after (read back through PLLCTL_ADDR/DATA), PMU_CTL after the PLL_UPD write.
pub(super) struct Pll {
    pub pre: [u32; 2],
    pub post: [u32; 2],
    pub pmu_ctl: u32,
    pub ok: bool,
}

/// The refusals, in `READY`'s code order (code = index + 2).
const WHY: [&str; 5] = ["chipid", "no-pmu", "pmu-core-separate", "selector-did-not-take", "restore-mismatch"];
/// 0 unprobed, 1 READY, 2.. REFUSED (`WHY[code - 2]`).
static READY: AtomicU8 = AtomicU8::new(0);
static PMU_REV: AtomicU8 = AtomicU8::new(0);
/// The PLL's spur mode as the station last set it (0 = the boot's).
static MODE: AtomicU8 = AtomicU8::new(0);

pub(super) fn mode() -> u8 {
    MODE.load(Ordering::Relaxed)
}

pub(super) fn set_mode(m: u8) {
    MODE.store(m, Ordering::Relaxed);
}

/// `admit`'s refusal name for a PMU refusal (`pmu:<reason>`).
#[cfg(feature = "wifi10")]
pub(super) fn refusal(e: &str) -> &'static str {
    match e {
        "chipid" => "pmu:chipid",
        "no-pmu" => "pmu:no-pmu",
        "pmu-core-separate" => "pmu:pmu-core-separate",
        "selector-did-not-take" => "pmu:selector-did-not-take",
        "restore-mismatch" => "pmu:restore-mismatch",
        "no-handle" => "pmu:no-handle",
        _ => "pmu:refused",
    }
}

/// ONE window move to ChipCommon, reads only: CC_ID, CC_CAP, CC_CAP_EXT and (when CAP says a PMU) PMU_CAP. Refused by name: not a
/// 4331 (`chipid`), no PMU (`no-pmu`), a separate PMU core (`pmu-core-separate`: rev 37 >= 35 and AOB — its core is not latched).
pub(super) fn probe<B: Bus + ?Sized>(b: &mut B) -> (Caps, Result<(), &'static str>) {
    let r = with_core(b, super::BCMA_ADDR_BASE, |w| {
        let chipid = w.r32(CC_ID);
        let cap = w.r32(CC_CAP);
        let cap_ext = w.r32(CC_CAP_EXT);
        let pmu_rev = if cap & CAP_PMU != 0 { (w.r32(PMU_CAP) & PMU_CAP_REV) as u8 } else { 0 };
        Caps { chipid, cap, cap_ext, pmu_rev }
    });
    let c = match r {
        Ok(c) => c,
        Err(e) => return (Caps::default(), Err(e)),
    };
    let v = if c.chipid & 0xFFFF != CHIP_4331 {
        Err("chipid")
    } else if c.cap & CAP_PMU == 0 {
        Err("no-pmu")
    } else if CC_REV_METAL >= CC_REV_AOB && c.cap_ext & CAP_EXT_AOB != 0 {
        Err("pmu-core-separate")
    } else {
        Ok(())
    };
    (c, v)
}

/// `probe` once per boot, its verdict cached and said once: `Ok(pmu_rev)` or the refusal.
pub(super) fn ready<B: Bus + ?Sized>(b: &mut B) -> Result<u8, &'static str> {
    match READY.load(Ordering::Relaxed) {
        0 => {}
        1 => return Ok(PMU_REV.load(Ordering::Relaxed)),
        n => return Err(WHY[(n as usize - 2).min(WHY.len() - 1)]),
    }
    let (c, v) = probe(b);
    serial_println!(
        "[wifi10] pmu probe chipid={:#010x} cc_rev={}(erom) cap={:#010x} pmu={} cap_ext={:#010x} aob={} pmu_rev={} -> {}{} — driver_chipcommon_pmu.c:275-290, bcma_driver_chipcommon.h:21,50,221-226,338-339 (STATIONROAM-PMU B967)",
        c.chipid, CC_REV_METAL, c.cap, (c.cap & CAP_PMU != 0) as u8, c.cap_ext, (c.cap_ext & CAP_EXT_AOB != 0) as u8, c.pmu_rev,
        if v.is_ok() { "READY" } else { "REFUSED reason=" }, v.err().unwrap_or("")
    );
    match v {
        Ok(()) => {
            PMU_REV.store(c.pmu_rev, Ordering::Relaxed);
            READY.store(1, Ordering::Relaxed);
            Ok(c.pmu_rev)
        }
        Err(e) => {
            let code = WHY.iter().position(|w| *w == e).unwrap_or(0) as u8 + 2;
            READY.store(code, Ordering::Relaxed);
            Err(e)
        }
    }
}

/// `driver_chipcommon_pmu.c:534-551` — the BCM4331 rows, `(PLL_CTLn, value)`: spur 1 = 0x11500014 / 0x0F600a08, else (spur 0)
/// 0x11100014 / 0x03000a08 (spur 2, 0x0FC00a08, is never asked by b43's HT-PHY).
pub(super) fn pll_table(mode: u8) -> [(u32, u32); 2] {
    if mode == 1 {
        [(0, 0x1150_0014), (2, 0x0F60_0A08)]
    } else {
        [(0, 0x1110_0014), (2, 0x0300_0A08)]
    }
}

/// `bcma_pmu_spuravoid_pllupdate(cc, mode)` on ChipCommon's PMU: each row's PLL_CTLn read (bcma's `bcma_chipco_pll_read` shape:
/// ADDR, a posting ADDR read, DATA), written, read back; then PMU_CTL |= PLL_UPD. `ok` = every row read back as written.
/// The caller latches `set_mode` on `ok` (the KAT runs this on a mock and must not move the station's mode).
pub(super) fn spur_avoid<B: Bus + ?Sized>(b: &mut B, mode: u8) -> Result<Pll, &'static str> {
    let rows = pll_table(mode);
    with_core(b, super::BCMA_ADDR_BASE, |w| {
        let (mut pre, mut post) = ([0u32; 2], [0u32; 2]);
        for (i, (reg, val)) in rows.iter().enumerate() {
            w.w32(PLLCTL_ADDR, *reg);
            let _ = w.r32(PLLCTL_ADDR);
            pre[i] = w.r32(PLLCTL_DATA);
            w.w32(PLLCTL_ADDR, *reg);
            w.w32(PLLCTL_DATA, *val);
            w.w32(PLLCTL_ADDR, *reg);
            let _ = w.r32(PLLCTL_ADDR);
            post[i] = w.r32(PLLCTL_DATA);
        }
        let ctl = w.r32(PMU_CTL) | PMU_CTL_PLL_UPD;
        w.w32(PMU_CTL, ctl);
        let pmu_ctl = w.r32(PMU_CTL);
        let ok = post[0] == rows[0].1 && post[1] == rows[1].1;
        Pll { pre, post, pmu_ctl, ok }
    })
}

/// `b43_wireless_core_phy_pll_reset` (`main.c:1202-1216`, BCMA arm) on ChipCommon: CHIPCTL_ADDR <- 0; DATA &= ~4, |= 4, &= ~4.
/// `Ok(DATA after)` (bit 2 clear = the reset pulse ended).
pub(super) fn pll_reset<B: Bus + ?Sized>(b: &mut B) -> Result<u32, &'static str> {
    with_core(b, super::BCMA_ADDR_BASE, |w| {
        w.w32(CHIPCTL_ADDR, 0);
        let v = w.r32(CHIPCTL_DATA);
        w.w32(CHIPCTL_DATA, v & !CHIPCTL_PLL_RESET);
        let v = w.r32(CHIPCTL_DATA);
        w.w32(CHIPCTL_DATA, v | CHIPCTL_PLL_RESET);
        let v = w.r32(CHIPCTL_DATA);
        w.w32(CHIPCTL_DATA, v & !CHIPCTL_PLL_RESET);
        w.r32(CHIPCTL_DATA)
    })
}

// ── The `pmu` KAT row (wifi/data.rs): a mock bus, no hardware ───────────────────────────────────────────────────────────────

/// The d11's backplane address (the selector's resting value on the metal, `bringup.rs` pre-image).
#[cfg(feature = "wifi10")]
const KAT_D11: u32 = 0x1800_1000;

/// A ChipCommon behind a mock selector: the PMU's PLL and CHIPCTL banks behind their ADDR registers, every selector write and
/// every CHIPCTL DATA write traced, any MMIO while the selector is NOT on ChipCommon counted (`off_core`).
#[cfg(feature = "wifi10")]
struct Mock {
    cfg: u32,
    takes: bool,
    id: u32,
    cap: u32,
    cap_ext: u32,
    pmu_ctl: u32,
    pmu_cap: u32,
    pll_a: u32,
    pll: [u32; 6],
    cc_a: u32,
    chipctl: [u32; 4],
    cfg_writes: alloc::vec::Vec<u32>,
    chipctl_w: alloc::vec::Vec<u32>,
    mmio: u32,
    off_core: u32,
}

#[cfg(feature = "wifi10")]
impl Mock {
    fn new(id: u32, cap: u32, cap_ext: u32) -> Mock {
        Mock {
            cfg: KAT_D11,
            takes: true,
            id,
            cap,
            cap_ext,
            pmu_ctl: 0x0000_0001,
            pmu_cap: 0x0000_0C11,
            pll_a: 0,
            pll: [0x1110_0014, 0x040C_0C06, 0x0300_0A08, 0, 0x2000_05C0, 0x8888_8815],
            cc_a: 0,
            chipctl: [0x0000_0005, 0, 0, 0],
            cfg_writes: alloc::vec::Vec::new(),
            chipctl_w: alloc::vec::Vec::new(),
            mmio: 0,
            off_core: 0,
        }
    }
    fn on_cc(&mut self) -> bool {
        self.mmio += 1;
        if self.cfg != super::BCMA_ADDR_BASE {
            self.off_core += 1;
            return false;
        }
        true
    }
    fn paired(&self) -> bool {
        self.cfg == KAT_D11 && self.off_core == 0 && self.cfg_writes.len() % 2 == 0
            && self.cfg_writes.chunks(2).all(|c| c[0] == super::BCMA_ADDR_BASE && c[1] == KAT_D11)
    }
}

#[cfg(feature = "wifi10")]
impl Bus for Mock {
    fn cfg80(&mut self) -> u32 {
        self.cfg
    }
    fn set_cfg80(&mut self, v: u32) {
        self.cfg_writes.push(v);
        if self.takes {
            self.cfg = v;
        }
    }
    fn r32(&mut self, off: u64) -> u32 {
        if !self.on_cc() {
            return 0xFFFF_FFFF;
        }
        match off {
            CC_ID => self.id,
            CC_CAP => self.cap,
            CC_CAP_EXT => self.cap_ext,
            PMU_CTL => self.pmu_ctl,
            PMU_CAP => self.pmu_cap,
            CHIPCTL_ADDR => self.cc_a,
            CHIPCTL_DATA => self.chipctl[(self.cc_a & 3) as usize],
            PLLCTL_ADDR => self.pll_a,
            PLLCTL_DATA => self.pll[(self.pll_a % 6) as usize],
            _ => 0,
        }
    }
    fn w32(&mut self, off: u64, v: u32) {
        if !self.on_cc() {
            return;
        }
        match off {
            PMU_CTL => self.pmu_ctl = v,
            CHIPCTL_ADDR => self.cc_a = v,
            CHIPCTL_DATA => {
                self.chipctl[(self.cc_a & 3) as usize] = v;
                self.chipctl_w.push(v);
            }
            PLLCTL_ADDR => self.pll_a = v,
            PLLCTL_DATA => self.pll[(self.pll_a % 6) as usize] = v,
            _ => {}
        }
    }
}

/// `pmu` (STATIONROAM-PMU B967): CAP bit 28 clear -> `no-pmu`; AOB set -> `pmu-core-separate`; chip 0x4360 -> `chipid`; a 4331 with
/// a PMU -> ready, pmu_rev 0x11 read; spur_avoid(1) writes 0x11500014 / 0x0F600a08 (PLL_CTL1 untouched) and sets PLL_UPD; (0)
/// writes 0x11100014 / 0x03000a08 back; pll_reset writes clear, set, clear and ends with bit 2 clear; a selector that does not
/// take -> `selector-did-not-take`, no MMIO, the PLL untouched; every move is the pair [ChipCommon, d11] with no MMIO off-core;
/// pll_table(0/1) are bcma's rows. Never touches `MODE`/`READY` (the station's).
#[cfg(feature = "wifi10")]
pub(super) fn kat() -> bool {
    let id = 0x1392_4331;
    let mut m = Mock::new(id, 0, 0);
    let mut ok = probe(&mut m).1 == Err("no-pmu") && m.paired() && m.cfg_writes.len() == 2;
    let mut m = Mock::new(id, CAP_PMU, CAP_EXT_AOB);
    ok &= probe(&mut m).1 == Err("pmu-core-separate") && m.paired();
    let mut m = Mock::new(0x1392_4360, CAP_PMU, 0);
    ok &= probe(&mut m).1 == Err("chipid") && m.paired();

    let mut m = Mock::new(id, CAP_PMU, 0);
    let (c, v) = probe(&mut m);
    ok &= v.is_ok() && c.pmu_rev == 0x11 && c.chipid == id && c.cap == CAP_PMU;
    let p1 = spur_avoid(&mut m, 1);
    ok &= p1.as_ref().map_or(false, |p| p.ok && p.pre == [0x1110_0014, 0x0300_0A08] && p.post == [0x1150_0014, 0x0F60_0A08]
        && p.pmu_ctl & PMU_CTL_PLL_UPD != 0);
    ok &= m.pll[0] == 0x1150_0014 && m.pll[2] == 0x0F60_0A08 && m.pll[1] == 0x040C_0C06 && m.pmu_ctl & PMU_CTL_PLL_UPD != 0;
    m.pmu_ctl = 0x0000_0001;
    let p0 = spur_avoid(&mut m, 0);
    ok &= p0.as_ref().map_or(false, |p| p.ok && p.pre == [0x1150_0014, 0x0F60_0A08] && p.post == [0x1110_0014, 0x0300_0A08]);
    ok &= m.pll[0] == 0x1110_0014 && m.pll[2] == 0x0300_0A08;
    let r = pll_reset(&mut m);
    ok &= r == Ok(0x0000_0001) && m.chipctl_w[..] == [0x0000_0001, 0x0000_0005, 0x0000_0001] && m.cc_a == 0;
    ok &= m.paired() && m.cfg_writes.len() == 8;

    let mut m = Mock::new(id, CAP_PMU, 0);
    m.takes = false;
    ok &= matches!(spur_avoid(&mut m, 1), Err("selector-did-not-take")) && m.mmio == 0 && m.cfg == KAT_D11 && m.pll[0] == 0x1110_0014
        && m.cfg_writes[..] == [super::BCMA_ADDR_BASE, KAT_D11];

    ok &= pll_table(1) == [(0, 0x1150_0014), (2, 0x0F60_0A08)] && pll_table(0) == [(0, 0x1110_0014), (2, 0x0300_0A08)];
    ok
}
