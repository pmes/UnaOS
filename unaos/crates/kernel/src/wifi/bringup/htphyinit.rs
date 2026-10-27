// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — driver (the d11's HT-PHY and 2059 radio; no CODEX §2 handler owns a radio register).
//!
//! WIFI9 (rmbp-ledger B625, R110) — rungs S5i (the HT-PHY RE-INIT after S5r's reset, from the pinned
//! tables) and S5c (the channel tune to the 2.4 GHz default, channel 11), receive-only.
//!
//! **Provenance (R110, R95 §2).** Every register number, bit, value and the ORDER of the writes below is
//! a hardware fact read from Linux b43 (GPL-2.0, `drivers/net/wireless/broadcom/b43/`), each cited
//! `file:line` beside the write it governs and collected, fact by fact, in
//! `docs/dev/evidence/wifi9/CITATIONS.md`. No b43 text is copied and no function is transcribed: the
//! accessor layer (one struct counting every write by class, per-element table cursors instead of
//! auto-increment, every wait bounded and its outcome printed) is this tree's own.
//!
//! **The order** is b43's `b43_phy_init` (`phy_common.c:83-113`) as `b43_chip_init` reaches it
//! (`main.c:3278-3280`): analog on (`phy_ht.c:1038-1046`), radio on — `b43_phy_ht_op_software_rfkill`
//! (`phy_ht.c:1019-1035`): `b43_radio_2059_init` (`phy_ht.c:142-169`) then a channel switch — then the
//! PHY init `b43_phy_ht_op_init` (`phy_ht.c:869-1007`), then the channel switch again
//! (`phy_common.c:107`). The MAC is suspended around all of it by the caller (`phy_common.c:455`).
//!
//! **What is NOT done, by name (receive-only, the seat's rule for this rung):** the forced RF sequence
//! RX2TX (`phy_ht.c:984`; only RST2RX runs, under the PA override `:983-986`); the idle-TSSI step, which
//! plays a TX tone through the sample-play engine (`phy_ht.c:1001`, `:592`, `:415-421`); the TX power
//! control setup and TSSI setup (`phy_ht.c:1002-1004`), which need the SPROM's PA coefficients and only
//! shape transmit; TX power control is left DISABLED (`phy_ht.c:1000`). The spur-avoid PLL steps that
//! live in ChipCommon's PMU and the BCMA core clock control (`phy_ht.c:742-748`, `main.c:1204-1218`) are
//! not reached from the d11 window and are skipped: channel 11 takes spuravoid=0 (`phy_ht.c:740-741`),
//! the MAC-side frequency fraction for it is written (`main.c:3063-3066`).
//!
//! **Readback** — the measurables: (1) the 24 init tables read back after the init, with table 0x1a at
//! 0xc0 compared against the LATE contents b43's init leaves there (`phy_ht.c:995-996`): the wire
//! `htphy-tables when=after-reinit … match=<m>/2294`; (2) the nine 2059 init registers (core 1's copy,
//! written with ALL routing, `radio_2059.c:334-335`); (3) every radio and PHY register the tune wrote.

use super::{r16, r32, settle, w32, BAR0_WRAP_OFF, D11_MACCTL, D11_PHY_VER, IOCTL_FGC, WRAP_IOCTL};
use crate::wifi::htphy_tables::{HtTab, Width, B43_HTTAB_1A_C0_LATE, B43_PHY_HT_TABLES_INIT};
use crate::wifi::radio2059::{
    chan_entry, init_table, ChanR2059, R2059_ALL, R2059_C1, R2059_C2, R2059_C3, R2059_RCAL_CONFIG, R2059_RCAL_STATUS,
    R2059_RCCAL_DONE_OSCCAP, R2059_RCCAL_MASTER, R2059_RCCAL_START_R1_Q1_P1, R2059_RCCAL_TRC0, R2059_RCCAL_X1,
    R2059_READ, R2059_RFPLL_MASTER, R2059_RFPLL_MISC_CAL_RESETN, R2059_RFPLL_MISC_EN, R2059_XTAL_CONFIG2, RXTX_REGS, SYN_REGS,
};

// ── d11 MMIO (b43.h) ────────────────────────────────────────────────────────────────────────────
/// `b43.h:93-94` — the core-rev>=24 radio address/data ports (the d11 is rev 29).
const RADIO24_CONTROL: u64 = 0x03D8;
const RADIO24_DATA: u64 = 0x03DA;
/// `b43.h:104-105` — the PHY register address/data ports.
const PHY_CONTROL: u64 = 0x03FC;
const PHY_DATA: u64 = 0x03FE;
/// `b43.h:109` + `:487` — PSM PHY header and its force-clock bit (the B-PHY reset, `phy_ht.c:311-329`).
const PSM_PHY_HDR: u64 = 0x0492;
const PSM_HDR_MAC_PHY_FORCE_CLK: u16 = 0x0004;
/// `b43.h:150-151` — the TSF clock fraction (`b43_mac_switch_freq`, `main.c:3049-3066`).
const TSF_CLK_FRAC_LOW: u64 = 0x062E;
const TSF_CLK_FRAC_HIGH: u64 = 0x0630;
/// `b43.h:498` — `B43_BCMA_IOCTL_MACPHYCLKEN` (`b43_mac_phy_clock_set`, `main.c:3020-3033`).
const IOCTL_MACPHYCLKEN: u32 = 0x0000_0010;
/// `phy_common.h:37` — a posting read every 24 writes in a row (`phy_common.c:225-229`, `:281-284`).
const MAX_WRITES_IN_ROW: u32 = 24;

// ── PHY register routing (phy_common.h:11-26) ───────────────────────────────────────────────────
const fn ofdm(r: u16) -> u16 { r | 0x0400 } // B43_PHY_OFDM, phy_common.h:13,24
const fn extg(r: u16) -> u16 { r | 0x0800 } // B43_PHY_EXTG, phy_common.h:14,26
const fn bmode(r: u16) -> u16 { r | 0x0C00 } // B43_PHY_N_BMODE, phy_common.h:15,22

// ── HT-PHY registers (phy_ht.h) ─────────────────────────────────────────────────────────────────
const HT_BBCFG: u16 = 0x001; // phy_ht.h:8
const HT_BBCFG_RSTCCA: u16 = 0x4000; // phy_ht.h:9
const HT_BBCFG_RSTRX: u16 = 0x8000; // phy_ht.h:10
const HT_BANDCTL: u16 = 0x009; // phy_ht.h:11
const HT_BANDCTL_5GHZ: u16 = 0x0001; // phy_ht.h:12
const HT_TABLE_ADDR: u16 = 0x072; // phy_ht.h:13
const HT_TABLE_DATALO: u16 = 0x073; // phy_ht.h:14
const HT_TABLE_DATAHI: u16 = 0x074; // phy_ht.h:15
const HT_CLASS_CTL: u16 = 0x0B0; // phy_ht.h:16
const HT_CLASS_CTL_CCK_EN: u16 = 0x0001; // phy_ht.h:17
const HT_CLASS_CTL_OFDM_EN: u16 = 0x0002; // phy_ht.h:18
const HT_CLASS_CTL_WAITED_EN: u16 = 0x0004; // phy_ht.h:19
const HT_BW: [u16; 6] = [0x1CE, 0x1CF, 0x1D0, 0x1D1, 0x1D2, 0x1D3]; // phy_ht.h:33-38
const HT_TXPCTL_CMD_C1: u16 = 0x1E7; // phy_ht.h:39
const HT_TXPCTL_EN_BITS: u16 = 0x2000 | 0x4000 | 0x8000; // phy_ht.h:41-43 (COEFF|HWPCTLEN|PCTLEN), phy_ht.c:536-538
const HT_RF_SEQ_MODE: u16 = extg(0x000); // phy_ht.h:72
const HT_RF_SEQ_TRIG: u16 = extg(0x003); // phy_ht.h:75
const HT_RF_SEQ_TRIG_RST2RX: u16 = 0x0020; // phy_ht.h:81
const HT_RF_SEQ_STATUS: u16 = extg(0x004); // phy_ht.h:82
const HT_RF_CTL_CMD: u16 = 0x810; // phy_ht.h:85
const HT_RF_CTL_CMD_FORCE: u16 = 0x0001; // phy_ht.h:86
const HT_RF_CTL_CMD_CHIP0_PU: u16 = 0x0002; // phy_ht.h:87
const HT_RF_CTL_INT: [u16; 3] = [extg(0x04c), extg(0x06c), extg(0x08c)]; // phy_ht.h:89-91
/// `(OVER, AFE)` per core — phy_ht.h:93-98.
const HT_AFE: [(u16, u16); 3] = [(extg(0x110), extg(0x111)), (extg(0x114), extg(0x115)), (extg(0x118), extg(0x119))];
const HT_CLIP1THRES: [u16; 3] = [ofdm(0x00E), ofdm(0x04E), ofdm(0x08E)]; // phy_ht.h:68-70
const HT_B_BBCFG: u16 = bmode(0x001); // phy_ht.h:110
const HT_B_BBCFG_RSTCCA_RSTRX: u16 = 0x4000 | 0x8000; // phy_ht.h:111-112
const HT_TEST: u16 = bmode(0x00A); // phy_ht.h:113

/// Table element width (`tables_phy_ht.h:6-12`): the selector picks the access form, never written.
#[derive(Clone, Copy)]
enum Tw {
    B8,
    B16,
    B32,
}

/// Every write this rung makes, by class — printed on the end line (WIFI8's audit discipline).
#[derive(Default, Clone, Copy)]
pub(super) struct Counts {
    /// `0x3D8` — radio register selects (reads and writes).
    pub radio_addr: u32,
    /// `0x3DA` — radio DATA writes: radio-silicon state set by this rung.
    pub radio_data: u32,
    /// PHY address/data port writes, PSM_PHY_HDR, TSF_CLK_FRAC (all d11 core registers).
    pub core_regs: u32,
    /// Of which, PHY DATA writes (a register or a table element set).
    pub phy_data: u32,
    /// The AI wrapper IOCTL (force-clock pairs and MACPHYCLKEN).
    pub wrapper: u32,
    /// Bounded waits that ran out (rcal, rccal, the RF sequence).
    pub timeouts: u32,
}

/// The HT-PHY write/read layer, over the mapped d11 window.
pub(super) struct Ht {
    bar0: u64,
    pub c: Counts,
    in_row: u32,
}

/// # Safety
/// As `super::w32`: the window is mapped and proven to be the d11 core's (checked live by the caller).
unsafe fn w16(base: u64, off: u64, val: u16) {
    core::ptr::write_volatile((base + off) as *mut u16, val);
}

impl Ht {
    pub(super) fn new(bar0: u64) -> Self {
        Ht { bar0, c: Counts::default(), in_row: 0 }
    }
    /// `phy_common.c:281-284` / `:225-229`: a posting read after 24 writes in a row.
    fn post(&mut self, radio: bool) {
        self.in_row += 1;
        if self.in_row > MAX_WRITES_IN_ROW {
            unsafe {
                if radio { let _ = r32(self.bar0, D11_MACCTL); } else { let _ = r16(self.bar0, D11_PHY_VER); }
            }
            self.in_row = 1;
        }
    }
    pub(super) fn phy_r(&mut self, reg: u16) -> u16 {
        self.in_row = 0; // phy_common.c:269
        unsafe { w16(self.bar0, PHY_CONTROL, reg) };
        self.c.core_regs += 1;
        unsafe { r16(self.bar0, PHY_DATA) }
    }
    pub(super) fn phy_w(&mut self, reg: u16, val: u16) {
        self.post(false);
        unsafe { w16(self.bar0, PHY_CONTROL, reg) };
        unsafe { w16(self.bar0, PHY_DATA, val) };
        self.c.core_regs += 2;
        self.c.phy_data += 1;
    }
    /// `phy_ht.c:1085-1091` — the HT-PHY maskset: select, then DATA <- (DATA & mask) | set.
    pub(super) fn phy_maskset(&mut self, reg: u16, mask: u16, set: u16) {
        unsafe { w16(self.bar0, PHY_CONTROL, reg) };
        let v = unsafe { r16(self.bar0, PHY_DATA) };
        unsafe { w16(self.bar0, PHY_DATA, (v & mask) | set) };
        self.c.core_regs += 2;
        self.c.phy_data += 1;
    }
    fn phy_mask(&mut self, reg: u16, mask: u16) {
        self.phy_maskset(reg, mask, 0); // phy_common.c:299-308
    }
    fn phy_set(&mut self, reg: u16, set: u16) {
        self.phy_maskset(reg, 0xFFFF, set); // phy_common.c:310-319
    }
    /// `phy_ht.c:1093-1100` — an HT-PHY radio read sets 0x200 in the register number.
    pub(super) fn radio_r(&mut self, reg: u16) -> u16 {
        self.in_row = 0; // phy_common.c:218
        unsafe { w16(self.bar0, RADIO24_CONTROL, reg | R2059_READ) };
        self.c.radio_addr += 1;
        unsafe { r16(self.bar0, RADIO24_DATA) }
    }
    /// `phy_ht.c:1102-1107`.
    pub(super) fn radio_w(&mut self, reg: u16, val: u16) {
        self.post(true);
        unsafe { w16(self.bar0, RADIO24_CONTROL, reg) };
        unsafe { w16(self.bar0, RADIO24_DATA, val) };
        self.c.radio_addr += 1;
        self.c.radio_data += 1;
    }
    fn radio_maskset(&mut self, reg: u16, mask: u16, set: u16) {
        let v = self.radio_r(reg);
        self.radio_w(reg, (v & mask) | set); // phy_common.c:233-249
    }
    fn radio_mask(&mut self, reg: u16, mask: u16) {
        self.radio_maskset(reg, mask, 0);
    }
    fn radio_set(&mut self, reg: u16, set: u16) {
        self.radio_maskset(reg, 0xFFFF, set);
    }
    /// `phy_common.c:251-264` — poll until `(reg & mask) == val`, `delay_us` apart, `timeout_us` total.
    fn radio_wait(&mut self, reg: u16, mask: u16, val: u16, delay_us: u64, timeout_us: u64) -> (bool, u64) {
        let mut t = 0u64;
        while t < timeout_us {
            if self.radio_r(reg) & mask == val {
                return (true, t);
            }
            let _ = settle(delay_us);
            t += delay_us;
        }
        self.c.timeouts += 1;
        (false, t)
    }
    /// `tables_phy_ht.c:672-697` — one element: the cursor (id << 10 | offset, `tables_phy_ht.h:10-12`),
    /// then DATAHI (bits 31:16) before DATALO for 32-bit, DATALO alone otherwise.
    fn tab_w(&mut self, id: u16, off: u16, w: Tw, val: u32) {
        self.phy_w(HT_TABLE_ADDR, (id << 10) | off);
        if let Tw::B32 = w {
            self.phy_w(HT_TABLE_DATAHI, (val >> 16) as u16);
        }
        self.phy_w(HT_TABLE_DATALO, (val & 0xFFFF) as u16);
    }
    /// `tables_phy_ht.c:606-635`.
    fn tab_r(&mut self, id: u16, off: u16, w: Tw) -> u32 {
        self.phy_w(HT_TABLE_ADDR, (id << 10) | off);
        match w {
            Tw::B8 => (self.phy_r(HT_TABLE_DATALO) & 0xFF) as u32,
            Tw::B16 => self.phy_r(HT_TABLE_DATALO) as u32,
            Tw::B32 => {
                let hi = self.phy_r(HT_TABLE_DATAHI) as u32;
                (hi << 16) | self.phy_r(HT_TABLE_DATALO) as u32
            }
        }
    }
    /// `tables_phy_ht.c:702-745` (`write_few`) with a cursor per element: consecutive offsets.
    fn tab_few16(&mut self, id: u16, off: u16, vals: &[u16]) {
        for (i, v) in vals.iter().enumerate() {
            self.tab_w(id, off + i as u16, Tw::B16, *v as u32);
        }
    }
    /// One region of `B43_PHY_HT_TABLES_INIT` (or the late 0x1a@0xc0 contents).
    fn tab_region(&mut self, t: &HtTab) {
        let w = if matches!(t.width, Width::W32) { Tw::B32 } else { Tw::B16 };
        for i in 0..t.len() {
            self.tab_w(t.id, t.offset + i as u16, w, t.get(i));
        }
    }
    /// `phy_common.c:563-580` (BCMA arm) — the wrapper IOCTL FGC bit on or off.
    fn force_clock(&mut self, on: bool) {
        let v = unsafe { r32(self.bar0, BAR0_WRAP_OFF + WRAP_IOCTL) };
        let n = if on { v | IOCTL_FGC } else { v & !IOCTL_FGC };
        unsafe { w32(self.bar0, BAR0_WRAP_OFF + WRAP_IOCTL, n) };
        self.c.wrapper += 1;
    }
    /// `main.c:3020-3033` (BCMA arm) — MACPHYCLKEN on.
    fn mac_phy_clock_on(&mut self) {
        let v = unsafe { r32(self.bar0, BAR0_WRAP_OFF + WRAP_IOCTL) };
        unsafe { w32(self.bar0, BAR0_WRAP_OFF + WRAP_IOCTL, v | IOCTL_MACPHYCLKEN) };
        self.c.wrapper += 1;
    }
    fn mmio_w16(&mut self, off: u64, val: u16) {
        unsafe { w16(self.bar0, off, val) };
        self.c.core_regs += 1;
    }
    fn mmio_r16(&self, off: u64) -> u16 {
        unsafe { r16(self.bar0, off) }
    }
}

// ── The b43 steps, in b43's order ───────────────────────────────────────────────────────────────

/// `phy_ht.c:1038-1046` — analog on: AFE <- 0x00cd, AFE_OVER <- 0 for each core.
fn switch_analog_on(h: &mut Ht) {
    for (over, afe) in HT_AFE.iter() {
        h.phy_w(*afe, 0x00cd);
        h.phy_w(*over, 0x0000);
    }
}

/// `phy_ht.c:220-234` — the classifier: keep CCK/OFDM/WAITED, replace the masked bits.
fn classifier(h: &mut Ht, mask: u16, val: u16) {
    let allowed = HT_CLASS_CTL_CCK_EN | HT_CLASS_CTL_OFDM_EN | HT_CLASS_CTL_WAITED_EN;
    let mut tmp = h.phy_r(HT_CLASS_CTL) & allowed;
    tmp &= !mask;
    tmp |= val & mask;
    h.phy_maskset(HT_CLASS_CTL, !allowed, tmp);
}

/// `phy_ht.c:175-194` — force an RF sequence and wait (200 x 1 ms) for its status bit to clear.
fn force_rf_sequence(h: &mut Ht, seq: u16) -> bool {
    let save = h.phy_r(HT_RF_SEQ_MODE);
    h.phy_set(HT_RF_SEQ_MODE, 0x3);
    h.phy_set(HT_RF_SEQ_TRIG, seq);
    let mut ok = false;
    for _ in 0..200 {
        if h.phy_r(HT_RF_SEQ_STATUS) & seq == 0 {
            ok = true;
            break;
        }
        let _ = settle(1000);
    }
    if !ok {
        h.c.timeouts += 1;
    }
    h.phy_w(HT_RF_SEQ_MODE, save);
    ok
}

/// `phy_ht.c:236-248` — reset CCA: BBCFG.RSTCCA pulsed under a forced clock, then RST2RX.
fn reset_cca(h: &mut Ht) -> bool {
    h.force_clock(true);
    let bbcfg = h.phy_r(HT_BBCFG);
    h.phy_w(HT_BBCFG, bbcfg | HT_BBCFG_RSTCCA);
    let _ = settle(1);
    h.phy_w(HT_BBCFG, bbcfg & !HT_BBCFG_RSTCCA);
    h.force_clock(false);
    force_rf_sequence(h, HT_RF_SEQ_TRIG_RST2RX)
}

/// `phy_ht.c:511-531` — the TX power fix (its mask is 0 in b43: `:519-522`, so EXTG(0x108) is cleared).
fn tx_power_fix(h: &mut Ht) {
    for i in 0..3u16 {
        let tmp = h.tab_r(26, 0xE8, Tw::B32);
        h.phy_mask(extg(0x108), 0);
        h.tab_w(7, 0x110 + i, Tw::B16, tmp >> 16);
        h.tab_w(13, 0x63 + i * 4, Tw::B8, tmp & 0xFF);
        h.tab_w(13, 0x73 + i * 4, Tw::B8, tmp & 0xFF);
    }
}

/// `phy_ht.c:142-169` + `radio_2059.c:318-336` + `phy_ht.c:80-132` — the 2059 init: pre (RF_CTL_CMD
/// power cycle), the rev-1 init table with ALL routing, the per-core 0x146 bits, the PLL cal reset
/// pulse with the crystal bit, rcal (core 3), rccal (three passes), RFPLL_MASTER bit 3 off.
/// Returns `(rcal_ok, rccal_ok_passes)`.
fn radio_2059_init(h: &mut Ht, phy_rev: u16) -> (bool, u32) {
    // init_pre, phy_ht.c:134-140
    h.phy_mask(HT_RF_CTL_CMD, !HT_RF_CTL_CMD_CHIP0_PU);
    h.phy_set(HT_RF_CTL_CMD, HT_RF_CTL_CMD_FORCE);
    h.phy_mask(HT_RF_CTL_CMD, !HT_RF_CTL_CMD_FORCE);
    h.phy_set(HT_RF_CTL_CMD, HT_RF_CTL_CMD_CHIP0_PU);
    // r2059_upload_inittabs, radio_2059.c:323-335
    if let Some(t) = init_table(phy_rev) {
        for [reg, val] in t.iter() {
            h.radio_w(R2059_ALL | *reg, *val);
        }
    }
    // phy_ht.c:152-153
    for r in [R2059_C1, R2059_C2, R2059_C3] {
        h.radio_set(r | 0x146, 0x3);
    }
    // post init, phy_ht.c:157-161
    h.radio_set(R2059_RFPLL_MISC_CAL_RESETN, 0x0078);
    h.radio_set(R2059_XTAL_CONFIG2, 0x0080);
    let _ = settle(2000);
    h.radio_mask(R2059_RFPLL_MISC_CAL_RESETN, !0x0078);
    h.radio_mask(R2059_XTAL_CONFIG2, !0x0080);
    // rcal, phy_ht.c:80-104
    h.radio_set(R2059_C3 | R2059_RCAL_CONFIG, 0x1);
    let _ = settle(10);
    h.radio_set(R2059_C3 | 0x0BF, 0x1);
    h.radio_maskset(R2059_C3 | 0x19B, 0x3, 0x2);
    h.radio_set(R2059_C3 | R2059_RCAL_CONFIG, 0x2);
    let _ = settle(100);
    h.radio_mask(R2059_C3 | R2059_RCAL_CONFIG, !0x2);
    let (rcal_ok, rcal_us) = h.radio_wait(R2059_C3 | R2059_RCAL_STATUS, 1, 1, 100, 1_000_000);
    h.radio_mask(R2059_C3 | R2059_RCAL_CONFIG, !0x1);
    h.radio_set(0xa, 0x60);
    // rccal, phy_ht.c:107-132
    let mut rccal_ok = 0u32;
    for (m, t) in [(0x61u16, 0xE9u16), (0x69, 0xD5), (0x73, 0x99)] {
        h.radio_w(R2059_RCCAL_MASTER, m);
        h.radio_w(R2059_RCCAL_X1, 0x6E);
        h.radio_w(R2059_RCCAL_TRC0, t);
        h.radio_w(R2059_RCCAL_START_R1_Q1_P1, 0x55);
        if h.radio_wait(R2059_RCCAL_DONE_OSCCAP, 2, 2, 500, 5_000_000).0 {
            rccal_ok += 1;
        }
        h.radio_w(R2059_RCCAL_START_R1_Q1_P1, 0x15);
    }
    h.radio_mask(R2059_RCCAL_MASTER, !0x1);
    // phy_ht.c:168
    h.radio_mask(R2059_RFPLL_MASTER, !0x0008);
    serial_println!(
        "[wifi9] r2059-init inittab=rev{} rcal={} rcal_waited={}us rccal_passes={}/3 — phy_ht.c:142-169, radio_2059.c:318-336",
        phy_rev, if rcal_ok { "done" } else { "TIMEOUT" }, rcal_us, rccal_ok
    );
    (rcal_ok, rccal_ok)
}

/// A tune's verdict.
pub(super) struct Tune {
    pub ch: u16,
    pub writes: u32,
    pub readback_ok: u32,
    pub readback_n: u32,
    pub cca_ok: bool,
}

/// `phy_common.c:416-441` (`b43_switch_channel`: the SHM cookie first, then the PHY op, then 8 ms)
/// + `phy_ht.c:807-835` + `:35-77` (radio) + `:763-805` (PHY). The SHM cookie is the caller's (it owns
/// the SHM path); this is the hardware half. 2.4 GHz only (`phy_ht.c:1064-1069`).
fn tune(h: &mut Ht, ch: u16, e: &ChanR2059) -> Tune {
    let before = h.c.radio_data + h.c.phy_data + h.c.core_regs;
    // ── b43_radio_2059_channel_setup, phy_ht.c:35-77 ──
    for (reg, v) in SYN_REGS.iter().zip(e.syn.iter()) {
        h.radio_w(*reg, *v as u16);
    }
    for r in [R2059_C1, R2059_C2, R2059_C3] {
        for (reg, v) in RXTX_REGS.iter().zip(e.rxtx.iter()) {
            h.radio_w(r | *reg, *v as u16);
        }
    }
    let _ = settle(50);
    h.radio_mask(R2059_RFPLL_MISC_EN, !0x1);
    h.radio_mask(R2059_RFPLL_MISC_CAL_RESETN, !0x4);
    h.radio_set(R2059_RFPLL_MISC_CAL_RESETN, 0x4);
    h.radio_set(R2059_RFPLL_MISC_EN, 0x1);
    let _ = settle(300);
    // ── b43_phy_ht_channel_setup, phy_ht.c:763-805, 2.4 GHz arm ──
    h.phy_mask(HT_BANDCTL, !HT_BANDCTL_5GHZ);
    // b43_phy_ht_bphy_reset(false), phy_ht.c:311-329
    let hdr = h.mmio_r16(PSM_PHY_HDR);
    h.mmio_w16(PSM_PHY_HDR, hdr | PSM_HDR_MAC_PHY_FORCE_CLK);
    h.phy_mask(HT_B_BBCFG, 0xFFFF & !HT_B_BBCFG_RSTCCA_RSTRX);
    h.mmio_w16(PSM_PHY_HDR, hdr);
    let bw = [e.phy.bw1, e.phy.bw2, e.phy.bw3, e.phy.bw4, e.phy.bw5, e.phy.bw6];
    for (reg, v) in HT_BW.iter().zip(bw.iter()) {
        h.phy_w(*reg, *v);
    }
    // phy_ht.c:789-797 — not channel 14: OFDM on; 2.4 GHz: HT_TEST bits 0x840 off.
    classifier(h, HT_CLASS_CTL_OFDM_EN, HT_CLASS_CTL_OFDM_EN);
    h.phy_mask(HT_TEST, !0x840);
    tx_power_fix(h); // phy_ht.c:799-800
    // b43_phy_ht_spur_avoid, phy_ht.c:733-761 — channel 11: spuravoid=0 (:740-741). The PLL/PMU steps
    // (:742-748, :752 -> main.c:1204-1218) are ChipCommon/BCMA, outside the d11 window: skipped.
    h.mmio_w16(TSF_CLK_FRAC_LOW, 0x6666); // main.c:3063-3066 (BCM4331, 160 MHz)
    h.mmio_w16(TSF_CLK_FRAC_HIGH, 0x6);
    h.phy_mask(HT_BBCFG, !HT_BBCFG_RSTRX & 0xFFFF); // phy_ht.c:757-758
    let cca_ok = reset_cca(h); // phy_ht.c:760
    h.phy_w(0x017e, 0x3830); // phy_ht.c:804
    crate::arch::sched::sleep_ms(8); // phy_common.c:440 — the radio tunes and stabilises
    let writes = h.c.radio_data + h.c.phy_data + h.c.core_regs - before;

    // ── Readback: every radio register the table wrote, and the six BW registers. ──
    let (mut ok, mut n) = (0u32, 0u32);
    for (reg, v) in SYN_REGS.iter().zip(e.syn.iter()) {
        n += 1;
        if h.radio_r(*reg) & 0xFF == *v as u16 { ok += 1; }
    }
    for r in [R2059_C1, R2059_C2, R2059_C3] {
        for (reg, v) in RXTX_REGS.iter().zip(e.rxtx.iter()) {
            n += 1;
            if h.radio_r(r | *reg) & 0xFF == *v as u16 { ok += 1; }
        }
    }
    for (reg, v) in HT_BW.iter().zip(bw.iter()) {
        n += 1;
        if h.phy_r(*reg) == *v { ok += 1; }
    }
    Tune { ch, writes, readback_ok: ok, readback_n: n, cca_ok }
}

/// `phy_ht.c:869-1007` — `b43_phy_ht_op_init`, the receive subset (the TX-only tail named in the
/// header is not run). Returns the RST2RX verdict.
fn op_init(h: &mut Ht) -> bool {
    // phy_ht.c:881 -> tables_phy_ht.c:794-823, the 24 regions in b43's order.
    for t in B43_PHY_HT_TABLES_INIT.iter() {
        h.tab_region(t);
    }
    h.phy_mask(0x0be, !0x2); // :883
    h.phy_set(0x23f, 0x7ff); // :884-886
    h.phy_set(0x240, 0x7ff);
    h.phy_set(0x241, 0x7ff);
    // zero_extg, phy_ht.c:250-262
    for base in [0x40u16, 0x60, 0x80] {
        for j in 0..4 {
            h.phy_w(extg(base + j), 0);
        }
    }
    for base in [0x40u16, 0x60, 0x80] {
        h.phy_w(extg(base + 0xc), 0);
    }
    h.phy_mask(extg(0), !0x3); // :890
    for (over, _) in HT_AFE.iter() {
        h.phy_w(*over, 0); // :892-894
    }
    h.phy_w(extg(0x103), 0x20); // :896-901
    h.phy_w(extg(0x101), 0x20);
    h.phy_w(0x20d, 0xb8);
    h.phy_w(extg(0x14f), 0xc8);
    h.phy_w(0x70, 0x50);
    h.phy_w(0x1ff, 0x30);
    classifier(h, HT_CLASS_CTL_CCK_EN, HT_CLASS_CTL_CCK_EN); // :906-907 (2.4 GHz)
    h.phy_set(0xb1, 0x91); // :909-915
    h.phy_w(0x32f, 0x0003);
    h.phy_w(0x077, 0x0010);
    h.phy_w(0x0b4, 0x0258);
    h.phy_mask(0x17e, !0x4000);
    h.phy_w(0x0b9, 0x0072);
    h.tab_few16(7, 0x14e, &[0x010f, 0x010f]); // :917-919
    h.tab_few16(7, 0x15e, &[0x010f, 0x010f]);
    h.tab_few16(7, 0x16e, &[0x010f, 0x010f]);
    // afe_unk1, phy_ht.c:265-284
    for (i, (over, afe)) in HT_AFE.iter().enumerate() {
        h.phy_set(*afe, 0x4);
        h.phy_set(*over, 0x4);
        h.phy_mask(*afe, !0x1);
        h.phy_set(*over, 0x1);
        h.tab_w(8, 5 + (i as u16) * 0x10, Tw::B16, 0);
        h.phy_mask(*over, !0x4);
    }
    h.tab_few16(7, 0x130, &[0x777, 0x111, 0x111, 0x777, 0x111, 0x111, 0x777, 0x111, 0x111]); // :923-924
    h.tab_w(7, 0x120, Tw::B16, 0x0777); // :926-927
    h.tab_w(7, 0x124, Tw::B16, 0x0777);
    h.tab_w(8, 0x00, Tw::B16, 0x02); // :929-931
    h.tab_w(8, 0x10, Tw::B16, 0x02);
    h.tab_w(8, 0x20, Tw::B16, 0x02);
    h.tab_few16(8, 0x08, &[0x8e, 0x96, 0x96, 0x96]); // :933-938
    h.tab_few16(8, 0x18, &[0x8f, 0x9f, 0x9f, 0x9f]);
    h.tab_few16(8, 0x28, &[0x8f, 0x9f, 0x9f, 0x9f]);
    h.tab_few16(8, 0x0c, &[0x2, 0x2, 0x2, 0x2]); // :940-942
    h.tab_few16(8, 0x1c, &[0x2, 0x2, 0x2, 0x2]);
    h.tab_few16(8, 0x2c, &[0x2, 0x2, 0x2, 0x2]);
    h.phy_maskset(0x0280, 0xff00, 0x3e); // :944-947
    h.phy_maskset(0x0283, 0xff00, 0x3e);
    h.phy_maskset(ofdm(0x0141), 0xff00, 0x46);
    h.phy_maskset(0x0283, 0xff00, 0x40);
    h.tab_few16(0, 0x8, &[0x09, 0x0e, 0x13, 0x18]); // :949-955 (table 40 as b43 has it)
    h.tab_few16(1, 0x8, &[0x09, 0x0e, 0x13, 0x18]);
    h.tab_few16(40, 0x8, &[0x09, 0x0e, 0x13, 0x18]);
    h.phy_maskset(ofdm(0x24), 0x3f, 0xd); // :957-959
    h.phy_maskset(ofdm(0x64), 0x3f, 0xd);
    h.phy_maskset(ofdm(0xa4), 0x3f, 0xd);
    for r in [0x060u16, 0x064, 0x080, 0x084] {
        h.phy_set(extg(r), 0x1); // :961-964
    }
    for (src, dst) in [(0x144u16, 0x14au16), (0x154, 0x15a), (0x164, 0x16a)] {
        let t = h.tab_r(7, src, Tw::B16); // :966-972
        h.tab_w(7, dst, Tw::B16, t);
    }
    // Reset CCA, :974-979
    h.force_clock(true);
    let tmp = h.phy_r(HT_BBCFG);
    h.phy_w(HT_BBCFG, tmp | HT_BBCFG_RSTCCA);
    h.phy_w(HT_BBCFG, tmp & !HT_BBCFG_RSTCCA);
    h.force_clock(false);
    h.mac_phy_clock_on(); // :981
    // PA override off (save, write 0x0400), RST2RX, PA override on (restore) — phy_ht.c:196-214, :983-986.
    // RX2TX (:984) is NOT forced: this rung is receive-only.
    let mut save = [0u16; 3];
    for (i, r) in HT_RF_CTL_INT.iter().enumerate() {
        save[i] = h.phy_r(*r);
    }
    for r in HT_RF_CTL_INT.iter() {
        h.phy_w(*r, 0x0400);
    }
    let rst2rx = force_rf_sequence(h, HT_RF_SEQ_TRIG_RST2RX);
    for (i, r) in HT_RF_CTL_INT.iter().enumerate() {
        h.phy_w(*r, save[i]);
    }
    classifier(h, 0, 0); // :989
    let clip = [h.phy_r(HT_CLIP1THRES[0]), h.phy_r(HT_CLIP1THRES[1]), h.phy_r(HT_CLIP1THRES[2])]; // :990, :286-291
    // bphy_init, phy_ht.c:293-309 (2.4 GHz, :992-993)
    let mut val: u16 = 0x1E1F;
    for i in 0..16u16 {
        h.phy_w(bmode(0x88 + i), val);
        val = val.wrapping_sub(0x202);
    }
    val = 0x3E3F;
    for i in 0..16u16 {
        h.phy_w(bmode(0x98 + i), val);
        val = val.wrapping_sub(0x202);
    }
    h.phy_w(bmode(0x38), 0x668);
    h.tab_region(&B43_HTTAB_1A_C0_LATE); // :995-996
    tx_power_fix(h); // :999
    // tx_power_ctl(false), phy_ht.c:533-554 (the saved indices are TX-only state, not kept)
    let _ = h.phy_r(HT_TXPCTL_CMD_C1);
    h.phy_mask(HT_TXPCTL_CMD_C1, 0xFFFF & !HT_TXPCTL_EN_BITS);
    serial_println!(
        "[wifi9] htphy-op-init rst2rx={} clip1thres=[{:#06x},{:#06x},{:#06x}] tx_pwr_ctl=off rx2tx=not-forced idle_tssi=not-run(tx-tone) txpctl_setup=not-run tssi_setup=not-run — phy_ht.c:869-1007, receive subset",
        if rst2rx { "done" } else { "TIMEOUT" }, clip[0], clip[1], clip[2]
    );
    rst2rx
}

/// Read the 24 regions back; 0x1a@0xc0 against the late contents. `(matched, elems)`; one row per region.
fn readback(h: &mut Ht) -> (u32, u32) {
    let (mut m, mut n) = (0u32, 0u32);
    for t in B43_PHY_HT_TABLES_INIT.iter() {
        let want: &HtTab = if t.id == 0x1a && t.offset == 0x0c0 { &B43_HTTAB_1A_C0_LATE } else { t };
        let w = if matches!(t.width, Width::W32) { Tw::B32 } else { Tw::B16 };
        let (mut mm, mut first) = (0u32, None::<(usize, u32, u32)>);
        for i in 0..t.len() {
            let got = h.tab_r(t.id, t.offset + i as u16, w);
            if got == want.get(i) { mm += 1; } else if first.is_none() { first = Some((i, got, want.get(i))); }
        }
        m += mm;
        n += t.len() as u32;
        if mm != t.len() as u32 {
            let (i, g, wv) = first.unwrap_or((0, 0, 0));
            serial_println!(
                "[wifi9] httab when=after-reinit name={} id={:#04x} off={:#05x} n={} match={} first_mismatch={} got={:#x} want={:#x}",
                want.name, t.id, t.offset, t.len(), mm, i, g, wv
            );
        }
    }
    (m, n)
}

/// The rung's verdicts, for `rx.rs` and `tests wifi`.
pub(super) struct Verdict {
    pub ok: bool,
    pub match_: u32,
    pub elems: u32,
    pub tune: Option<Tune>,
    pub c: Counts,
}

/// S5i + S5c. `phy_rev` from PHY_VER (gated by the caller: type 7, rev <= 1 — `main.c:4517-4520`).
/// Precondition: the MAC is suspended (`phy_common.c:455`), the windows are on the d11 core and its
/// wrapper (checked live by `live::handle`).
pub(super) fn run(bar0: u64, phy_rev: u16, ch: u16, chan_cookie: &mut dyn FnMut(u16)) -> Verdict {
    let mut h = Ht::new(bar0);
    let freq = 2407 + 5 * ch; // 2.4 GHz channels 1..13
    let Some(e) = chan_entry(freq) else {
        serial_println!(":: wifi9: htphy-reinit REFUSED reason=no-channel-row ch={} freq={} — radio_2059.c:338-351 finds no row ::", ch, freq);
        return Verdict { ok: false, match_: 0, elems: 0, tune: None, c: h.c };
    };
    // b43_phy_init (phy_common.c:97-98) -> switch_analog(on), radio on (phy_ht.c:1029-1034).
    switch_analog_on(&mut h);
    let (rcal_ok, rccal) = radio_2059_init(&mut h, phy_rev);
    chan_cookie(ch);
    let _first = tune(&mut h, ch, e);
    // ops->init (phy_common.c:100).
    let rst2rx = op_init(&mut h);
    // b43_switch_channel(phy->channel) (phy_common.c:107).
    chan_cookie(ch);
    let t = tune(&mut h, ch, e);
    // Readback: the tables, then the 2059 init registers (core 1's copy).
    let (m, n) = readback(&mut h);
    let (mut rm, mut rn) = (0u32, 0u32);
    if let Some(it) = init_table(phy_rev) {
        for [reg, val] in it.iter() {
            rn += 1;
            if h.radio_r(*reg | R2059_C1) & 0xFF == *val & 0xFF { rm += 1; }
        }
    }
    serial_println!(
        ":: wifi9: htphy-tables when=after-reinit tables={} elems={} match={}/{} table_1a_0xc0=late r2059_init={}/{} -> {} — the loss S5r measured, repaid: b43's tables_phy_ht.c:794-823 regions written through TABLE_ADDR/DATAHI/DATALO (phy_ht.h:13-15) and read back ::",
        B43_PHY_HT_TABLES_INIT.len(), n, m, n, rm, rn, if m == n { "MATCH" } else if m > 0 { "PARTIAL" } else { "DIFFERS" }
    );
    let ok = m == n && rm == rn && rcal_ok && rst2rx;
    serial_println!(
        ":: wifi9: htphy-reinit phy_rev={} rcal={} rccal={}/3 rst2rx={} wrote-radio-addr={}(audited) wrote-radio-data={}(audited) wrote-core-regs={}(audited; phy-data={}) wrote-wrapper={}(audited — FGC pairs + MACPHYCLKEN) timeouts={} -> {} — phy_common.c:83-113 order: analog on, 2059 init, tune, op_init, tune ::",
        phy_rev, rcal_ok as u8, rccal, rst2rx as u8, h.c.radio_addr, h.c.radio_data, h.c.core_regs, h.c.phy_data, h.c.wrapper,
        h.c.timeouts, if ok { "DONE" } else { "PARTIAL" }
    );
    let readback_ok = t.readback_ok == t.readback_n;
    serial_println!(
        ":: wifi9: channel-tune ch={} radio=2059 freq={} writes={} readback={} cca={} -> {} — phy_ht.c:35-77 + :763-805, SHM 0x00A0 cookie (b43.h:255, phy_common.c:425-432), default channel 11 (phy_ht.c:1074-1077) ::",
        t.ch, freq, t.writes,
        if readback_ok { alloc::string::String::from("MATCH") } else { alloc::format!("{}/{}", t.readback_ok, t.readback_n) },
        if t.cca_ok { "rst2rx" } else { "TIMEOUT" },
        if readback_ok { "DONE" } else { "PARTIAL" }
    );
    Verdict { ok, match_: m, elems: n, tune: Some(t), c: h.c }
}

// ── WIFI10 (B646): the TX-side steps of `b43_phy_ht_op_init` the receive subset skipped ────────────
/// `phy_ht.h:62`, `:100` — the per-core TX power control command registers beside `HT_TXPCTL_CMD_C1`.
#[cfg(feature = "wifi10")]
const HT_TXPCTL_CMD_C2: u16 = 0x222;
#[cfg(feature = "wifi10")]
const HT_TXPCTL_CMD_C3: u16 = extg(0x164);
/// `phy_ht.h:76` — the RX2TX trigger.
#[cfg(feature = "wifi10")]
const HT_RF_SEQ_TRIG_RX2TX: u16 = 0x0001;

/// What `tx_prep` did and read.
#[cfg(feature = "wifi10")]
pub(super) struct TxPrep {
    pub rx2tx: bool,
    pub rst2rx: bool,
    /// `TXPCTL_CMD_C1..C3` as left by the init tables (the fixed power index the frame leaves at).
    pub txpctl: [u16; 3],
    /// `TXPCTL_CMD_C1 & EN_BITS` after: 0 = power control OFF, as this rung holds it.
    pub pctl_en: u16,
    pub c: Counts,
}

/// `phy_ht.c:196-213` — the PA override: save `RF_CTL_INT_C1..C3` and write 0x0400 (off), or restore (on).
#[cfg(feature = "wifi10")]
fn pa_override(h: &mut Ht, save: &mut [u16; 3], enable: bool) {
    for (i, reg) in HT_RF_CTL_INT.iter().enumerate() {
        if enable {
            h.phy_w(*reg, save[i]);
        } else {
            save[i] = h.phy_r(*reg);
        }
    }
    if !enable {
        for reg in HT_RF_CTL_INT.iter() {
            h.phy_w(*reg, 0x0400);
        }
    }
}

/// `phy_ht.c:983-986` — under the PA override, force the RF sequencer RX2TX then RST2RX (the TX path
/// sequenced once, the radio left in RX), then `:999-1000` tx power fix + power control OFF, and the
/// three `TXPCTL_CMD` words read for the wire. The idle-TSSI tone, the power-control setup (SPROM PA
/// coefficients: `drivers/bcma/sprom.c`, outside the arc's sources) and the TSSI setup are NOT run.
#[cfg(feature = "wifi10")]
pub(super) fn tx_prep(h: &mut Ht) -> TxPrep {
    let mut save = [0u16; 3];
    pa_override(h, &mut save, false);
    let rx2tx = force_rf_sequence(h, HT_RF_SEQ_TRIG_RX2TX);
    let rst2rx = force_rf_sequence(h, HT_RF_SEQ_TRIG_RST2RX);
    pa_override(h, &mut save, true);
    tx_power_fix(h);
    h.phy_mask(HT_TXPCTL_CMD_C1, 0xFFFF & !HT_TXPCTL_EN_BITS);
    let txpctl = [h.phy_r(HT_TXPCTL_CMD_C1), h.phy_r(HT_TXPCTL_CMD_C2), h.phy_r(HT_TXPCTL_CMD_C3)];
    let pctl_en = txpctl[0] & HT_TXPCTL_EN_BITS;
    serial_println!(
        "[wifi10] htphy-tx-prep rx2tx={} rst2rx={} txpctl_cmd=[{:#06x},{:#06x},{:#06x}] tx_pwr_ctl={} idle_tssi=not-run txpctl_setup=not-run(sprom-pa-coefficients: drivers/bcma/sprom.c) tssi_setup=not-run wrote-core-regs={} timeouts={} — phy_ht.c:196-213, :983-986, :999-1000",
        if rx2tx { "done" } else { "TIMEOUT" }, if rst2rx { "done" } else { "TIMEOUT" }, txpctl[0], txpctl[1], txpctl[2],
        if pctl_en == 0 { "off" } else { "ON?" }, h.c.core_regs, h.c.timeouts
    );
    TxPrep { rx2tx, rst2rx, txpctl, pctl_en, c: h.c }
}

// ── WIFI11 (B653): HT-PHY TX power control from the SPROM's per-core PA coefficients ──────────────
// `b43_phy_ht_op_init`'s tail (`phy_ht.c:998-1004`) after `tx_prep`'s tx-power-fix + control OFF: the
// idle TSSI measured under a TX tone (`:573-607`), the control set up from SPROM maxpwr_2g / pa_2g
// (`:621-729`), the TSSI mux (`:609-619`), control ON (`:533-571`). Registers from `phy_ht.h`, cited.
#[cfg(feature = "wifi10")]
const HT_IQLOCAL_CMDGCTL: u16 = 0x0C2; // phy_ht.h:20
#[cfg(feature = "wifi10")]
const HT_SAMP_CMD: u16 = 0x0C3; // phy_ht.h:21
#[cfg(feature = "wifi10")]
const HT_SAMP_CMD_STOP: u16 = 0x0002; // phy_ht.h:22
#[cfg(feature = "wifi10")]
const HT_SAMP_LOOP_CNT: u16 = 0x0C4; // phy_ht.h:23
#[cfg(feature = "wifi10")]
const HT_SAMP_WAIT_CNT: u16 = 0x0C5; // phy_ht.h:24
#[cfg(feature = "wifi10")]
const HT_SAMP_DEP_CNT: u16 = 0x0C6; // phy_ht.h:25
#[cfg(feature = "wifi10")]
const HT_SAMP_STAT: u16 = 0x0C7; // phy_ht.h:26
#[cfg(feature = "wifi10")]
const HT_TSSIMODE: u16 = 0x122; // phy_ht.h:30
#[cfg(feature = "wifi10")]
const HT_TSSIMODE_EN: u16 = 0x0001; // phy_ht.h:31
#[cfg(feature = "wifi10")]
const HT_TXPCTL_CMD_INIT: u16 = 0x007F; // phy_ht.h:40, :63, :101 (the same field in C1..C3)
#[cfg(feature = "wifi10")]
const HT_TXPCTL_CMD_PCTLEN: u16 = 0x8000; // phy_ht.h:43
#[cfg(feature = "wifi10")]
const HT_TXPCTL_N: u16 = 0x1E8; // phy_ht.h:44
#[cfg(feature = "wifi10")]
const HT_TXPCTL_N_TSSID: u16 = 0x00FF; // phy_ht.h:45
#[cfg(feature = "wifi10")]
const HT_TXPCTL_N_NPTIL2: u16 = 0x0700; // phy_ht.h:47-48 (shift 8)
#[cfg(feature = "wifi10")]
const HT_TXPCTL_IDLE_TSSI: u16 = 0x1E9; // phy_ht.h:49
#[cfg(feature = "wifi10")]
const HT_TXPCTL_IDLE_TSSI_C1: u16 = 0x003F; // phy_ht.h:50
#[cfg(feature = "wifi10")]
const HT_TXPCTL_IDLE_TSSI_C2: u16 = 0x3F00; // phy_ht.h:52-53 (shift 8)
#[cfg(feature = "wifi10")]
const HT_TXPCTL_IDLE_TSSI_BINF: u16 = 0x8000; // phy_ht.h:54
#[cfg(feature = "wifi10")]
const HT_TXPCTL_TARG_PWR: u16 = 0x1EA; // phy_ht.h:55 (C1 bits 7:0, C2 bits 15:8, :56-59)
#[cfg(feature = "wifi10")]
const HT_TX_PCTL_STATUS: [u16; 3] = [0x1ED, 0x1EE, extg(0x169)]; // phy_ht.h:60-61, :108
#[cfg(feature = "wifi10")]
const HT_RSSI: [u16; 3] = [0x219, 0x21A, 0x21B]; // phy_ht.h:64-66
#[cfg(feature = "wifi10")]
const HT_RF_SEQ_MODE_CA_OVER: u16 = 0x0001; // phy_ht.h:73
#[cfg(feature = "wifi10")]
const HT_TXPCTL_IDLE_TSSI2: u16 = extg(0x165); // phy_ht.h:102 (C3 bits 5:0, :103-104)
#[cfg(feature = "wifi10")]
const HT_TXPCTL_TARG_PWR2: u16 = extg(0x166); // phy_ht.h:105 (C3 bits 7:0, :106-107)
/// `phy_ht.c:575` — the per-core register bases the idle-TSSI step saves and drives (0x840 = EXTG 0x40).
#[cfg(feature = "wifi10")]
const HT_IDLE_TSSI_BASE: [u16; 3] = [0x840, 0x860, 0x880];
/// `phy_ht.c:470-477` — the twelve registers `poll_rssi` saves around the RSSI selection.
#[cfg(feature = "wifi10")]
const HT_POLL_RSSI_SAVE: [u16; 12] = [
    HT_AFE[0].1, HT_AFE[0].0, 0x848, 0x841, HT_AFE[1].1, HT_AFE[1].0, 0x868, 0x861, HT_AFE[2].1, HT_AFE[2].0, 0x888, 0x881,
];
/// `phy_ht.c:361` — the sample-play buffer, table 0x11 (0x4400 = 17 << 10 in the cursor form), 20 << 3 entries.
#[cfg(feature = "wifi10")]
const HT_SAMP_TABLE: u16 = 0x11;
#[cfg(feature = "wifi10")]
const HT_SAMP_LEN: u16 = 20 << 3;

/// The per-core inputs (SPROM, `ident::CorePwr`): the target power (dBm/4) and the three PA
/// coefficients a1, b0, b1 as `phy_ht.c:633-636` names them.
#[cfg(feature = "wifi10")]
#[derive(Clone, Copy)]
pub(super) struct PwrIn {
    pub target: u8,
    pub a1: i16,
    pub b0: i16,
    pub b1: i16,
}

/// What `tx_pwr_ctl` measured, wrote and read back.
#[cfg(feature = "wifi10")]
pub(super) struct TxPctl {
    /// `TXPCTL_CMD_C1 & EN_BITS == EN_BITS` after the enable.
    pub on: bool,
    /// The idle TSSI per core (`phy_ht.c:600-602`).
    pub idle: [u8; 3],
    /// `TX_PCTL_STATUS_C1..C3` read after the enable: the hardware's current power index per core.
    pub status: [u16; 3],
    /// `TXPCTL_CMD_C1..C3` read after the enable.
    pub cmd: [u16; 3],
    /// `TXPCTL_TARG_PWR` and `TXPCTL_TARG_PWR2` read back.
    pub targ: [u16; 2],
    /// `TXPCTL_IDLE_TSSI` and `TXPCTL_IDLE_TSSI2` read back.
    pub idle_regs: [u16; 2],
    /// The TX tone ran to its status bit (`phy_ht.c:404-411`).
    pub tone_ok: bool,
    /// The sample-play engine's status before `stop_playback` (`phy_ht.c:341`).
    pub samp_stat: u16,
    pub c: Counts,
}

/// `phy_ht.c:359-372` — the sample buffer: `HT_SAMP_LEN` 32-bit zero elements of table 0x11 from
/// offset 0 (b43 sets the cursor once and lets it advance; this tree sets it per element).
#[cfg(feature = "wifi10")]
fn load_samples(h: &mut Ht) -> u16 {
    for i in 0..HT_SAMP_LEN {
        h.tab_w(HT_SAMP_TABLE, i, Tw::B32, 0);
    }
    HT_SAMP_LEN
}

/// `phy_ht.c:374-413` — run the loaded samples: depth = samps-1, loops 0xFFFF (endless; not
/// decremented), wait 0; RF_SEQ_MODE.CA_OVER set around SAMP_CMD bit 0; the status bit polled 100 x
/// 10 us. `bb_mult` receives HTTAB16(13, 0x63 + i*4) on the first run (`:382-385`). Returns the poll verdict.
#[cfg(feature = "wifi10")]
fn run_samples(h: &mut Ht, samps: u16, loops: u16, wait: u16, bb_mult: &mut [Option<u16>; 3]) -> bool {
    for (i, m) in bb_mult.iter_mut().enumerate() {
        if m.is_none() {
            *m = Some(h.tab_r(13, 0x63 + (i as u16) * 4, Tw::B16) as u16);
        }
    }
    h.phy_w(HT_SAMP_DEP_CNT, samps - 1);
    let loops = if loops != 0xFFFF { loops - 1 } else { loops };
    h.phy_w(HT_SAMP_LOOP_CNT, loops);
    h.phy_w(HT_SAMP_WAIT_CNT, wait);
    let save_seq_mode = h.phy_r(HT_RF_SEQ_MODE);
    h.phy_set(HT_RF_SEQ_MODE, HT_RF_SEQ_MODE_CA_OVER);
    // :397-399 — b43 masks with ~0 (no bit cleared; its own TODO); the read-modify-writes are kept in order.
    h.phy_mask(HT_SAMP_CMD, 0xFFFF);
    h.phy_mask(HT_SAMP_CMD, 0xFFFF);
    h.phy_mask(HT_IQLOCAL_CMDGCTL, 0xFFFF);
    h.phy_set(HT_SAMP_CMD, 0x1);
    let mut ok = false;
    for _ in 0..100 {
        if h.phy_r(HT_RF_SEQ_STATUS) & 1 == 0 {
            ok = true;
            break;
        }
        let _ = settle(10);
    }
    if !ok {
        h.c.timeouts += 1;
    }
    h.phy_w(HT_RF_SEQ_MODE, save_seq_mode);
    ok
}

/// `phy_ht.c:335-357` — stop playback: SAMP_STAT bit 0 -> SAMP_CMD.STOP, else bit 1 ->
/// IQLOCAL_CMDGCTL &= 0x7FFF; SAMP_CMD &= ~4; the saved bb_mult restored to HTTAB16(13, 0x63/0x67 + i*4).
#[cfg(feature = "wifi10")]
fn stop_playback(h: &mut Ht, bb_mult: &[Option<u16>; 3]) -> u16 {
    let stat = h.phy_r(HT_SAMP_STAT);
    if stat & 0x1 != 0 {
        h.phy_set(HT_SAMP_CMD, HT_SAMP_CMD_STOP);
    } else if stat & 0x2 != 0 {
        h.phy_mask(HT_IQLOCAL_CMDGCTL, 0x7FFF);
    }
    h.phy_mask(HT_SAMP_CMD, !0x0004);
    for (i, m) in bb_mult.iter().enumerate() {
        if let Some(v) = m {
            h.tab_w(13, 0x63 + (i as u16) * 4, Tw::B16, *v as u32);
            h.tab_w(13, 0x67 + (i as u16) * 4, Tw::B16, *v as u32);
        }
    }
    stat
}

/// `phy_ht.c:427-465` — RSSI selection, all three cores (`core_sel` 5), type TSSI 2G: AFE_Cx |= 0x300
/// then |= 0xC00, AFE_Cx_OVER |= 0x200 then |= 0x400, radio C3|0xBF |= 1, radio Cx|0x159 <- 0x11.
#[cfg(feature = "wifi10")]
fn rssi_select_tssi_2g(h: &mut Ht) {
    for (core, (over, afe)) in HT_AFE.iter().enumerate() {
        h.phy_set(*afe, 0x3 << 8);
        h.phy_set(*afe, 0x3 << 10);
        h.phy_set(*over, 0x1 << 9);
        h.phy_set(*over, 0x1 << 10);
        h.radio_set(R2059_C3 | 0xbf, 0x1);
        h.radio_w([R2059_C1, R2059_C2, R2059_C3][core] | 0x159, 0x11);
    }
}

/// `phy_ht.c:467-509` — one sample of the three RSSI words under the TSSI-2G selection, the twelve
/// registers saved and restored; each core's two 6-bit fields sign-extended (`:496-501`). Returns
/// `[C1 low, C1 high, C2 low, C2 high, C3 low, C3 high]`.
#[cfg(feature = "wifi10")]
fn poll_rssi_tssi_2g(h: &mut Ht) -> [i32; 6] {
    let mut save = [0u16; 12];
    for (i, r) in HT_POLL_RSSI_SAVE.iter().enumerate() {
        save[i] = h.phy_r(*r);
    }
    rssi_select_tssi_2g(h);
    let sx = |v: u16| -> i32 { (((v & 0x3F) << 2) as u8 as i8 >> 2) as i32 };
    let mut buf = [0i32; 6];
    for (c, r) in HT_RSSI.iter().enumerate() {
        let t = h.phy_r(*r);
        buf[2 * c] = sx(t);
        buf[2 * c + 1] = sx(t >> 8);
    }
    for (i, r) in HT_POLL_RSSI_SAVE.iter().enumerate() {
        h.phy_w(*r, save[i]);
    }
    buf
}

/// `phy_ht.c:573-607` — the idle TSSI: per core save base+6, +7, +0 and write +6 <- 0, +7 &= ~0xF,
/// +0 |= 0x400, +0 |= 0x1000; the TX tone (`:415-421`: samples loaded, run endless); 20 us; one RSSI
/// poll; playback stopped; CCA reset; the saved registers restored. idle[c] = the core's low field & 0xFF.
#[cfg(feature = "wifi10")]
fn idle_tssi(h: &mut Ht) -> ([u8; 3], bool, u16) {
    let mut save = [[0u16; 3]; 3];
    for (core, base) in HT_IDLE_TSSI_BASE.iter().enumerate() {
        save[core][1] = h.phy_r(base + 6);
        save[core][2] = h.phy_r(base + 7);
        save[core][0] = h.phy_r(base + 0);
        h.phy_w(base + 6, 0);
        h.phy_mask(base + 7, !0xF);
        h.phy_set(base + 0, 0x0400);
        h.phy_set(base + 0, 0x1000);
    }
    let mut bb_mult = [None::<u16>; 3];
    let samp = load_samples(h);
    let tone_ok = run_samples(h, samp, 0xFFFF, 0, &mut bb_mult);
    let _ = settle(20);
    let buf = poll_rssi_tssi_2g(h);
    let stat = stop_playback(h, &bb_mult);
    let _ = reset_cca(h);
    let idle = [(buf[0] & 0xff) as u8, (buf[2] & 0xff) as u8, (buf[4] & 0xff) as u8];
    for (core, base) in HT_IDLE_TSSI_BASE.iter().enumerate() {
        h.phy_w(base + 0, save[core][0]);
        h.phy_w(base + 6, save[core][1]);
        h.phy_w(base + 7, save[core][2]);
    }
    (idle, tone_ok, stat)
}

/// `phy_ht.c:621-729`, the 2.4 GHz arm (`:632-637`): TSSIMODE.EN; CMD_C1 &= ~PCTLEN; IDLE_TSSI |= 0x4000;
/// CMD_C1..C3 INIT field <- 0x19; IDLE_TSSI |= BINF; the idle values into IDLE_TSSI (C1, C2) and
/// IDLE_TSSI2 (C3); TXPCTL_N TSSID 0xF0, NPTIL2 3; the targets into TARG_PWR (C1, C2) and TARG_PWR2 (C3);
/// per core the 64-entry estimate table HTTAB16(26 + c, 0..64): num = 8*(16*b0 + b1*i),
/// den = 32768 + a1*i, pwr = max((4*num + den/2) / den, -8) (`:719-729`; C's truncating division = Rust's).
#[cfg(feature = "wifi10")]
fn tx_power_ctl_setup(h: &mut Ht, p: &[PwrIn; 3], idle: &[u8; 3]) {
    h.phy_set(HT_TSSIMODE, HT_TSSIMODE_EN);
    h.phy_mask(HT_TXPCTL_CMD_C1, !HT_TXPCTL_CMD_PCTLEN & 0xFFFF);
    h.phy_set(HT_TXPCTL_IDLE_TSSI, 0x4000);
    h.phy_maskset(HT_TXPCTL_CMD_C1, !HT_TXPCTL_CMD_INIT, 0x19);
    h.phy_maskset(HT_TXPCTL_CMD_C2, !HT_TXPCTL_CMD_INIT, 0x19);
    h.phy_maskset(HT_TXPCTL_CMD_C3, !HT_TXPCTL_CMD_INIT, 0x19);
    h.phy_set(HT_TXPCTL_IDLE_TSSI, HT_TXPCTL_IDLE_TSSI_BINF);
    h.phy_maskset(HT_TXPCTL_IDLE_TSSI, !HT_TXPCTL_IDLE_TSSI_C1, idle[0] as u16);
    h.phy_maskset(HT_TXPCTL_IDLE_TSSI, !HT_TXPCTL_IDLE_TSSI_C2, (idle[1] as u16) << 8);
    h.phy_maskset(HT_TXPCTL_IDLE_TSSI2, !0x003F, idle[2] as u16);
    h.phy_maskset(HT_TXPCTL_N, !HT_TXPCTL_N_TSSID, 0xf0);
    h.phy_maskset(HT_TXPCTL_N, !HT_TXPCTL_N_NPTIL2, 0x3 << 8);
    h.phy_maskset(HT_TXPCTL_TARG_PWR, !0x00FF, p[0].target as u16);
    h.phy_maskset(HT_TXPCTL_TARG_PWR, !0xFF00, (p[1].target as u16) << 8);
    h.phy_maskset(HT_TXPCTL_TARG_PWR2, !0x00FF, p[2].target as u16);
    for (c, pc) in p.iter().enumerate() {
        let (a1, b0, b1) = (pc.a1 as i32, pc.b0 as i32, pc.b1 as i32);
        for i in 0..64i32 {
            let num = 8 * (16 * b0 + b1 * i);
            let den = 32768 + a1 * i;
            let pwr = core::cmp::max((4 * num + den / 2) / den, -8);
            h.tab_w(26 + c as u16, i as u16, Tw::B16, pwr as u32);
        }
    }
}

/// `phy_ht.c:609-619` — the TSSI mux: per core radio 0x8BF |= 1 (C3 | 0xBF), radio Cx|0x159 <- 0x11.
#[cfg(feature = "wifi10")]
fn tssi_setup(h: &mut Ht) {
    for r in [R2059_C1, R2059_C2, R2059_C3] {
        h.radio_set(0x8bf, 0x1);
        h.radio_w(r | 0x0159, 0x0011);
    }
}

/// `phy_ht.c:533-571`, the enable arm (`:556-568`): CMD_C1 |= EN_BITS; the 5 GHz 0x32 writes do not
/// apply; b43's saved indices start at INIT + 1 = 0x80 (`:862-864`), above INIT, so no CMD index is
/// written and the hardware starts from the INIT field the setup wrote (0x19).
#[cfg(feature = "wifi10")]
fn tx_power_ctl_on(h: &mut Ht) {
    h.phy_set(HT_TXPCTL_CMD_C1, HT_TXPCTL_EN_BITS);
}

/// The whole tail (`phy_ht.c:1001-1004`) after `tx_prep`: idle TSSI, setup from `p`, TSSI mux, control
/// ON, then the readback. The MAC is suspended by the caller throughout, as b43's `op_init` runs
/// (`phy_common.c:455`). The tone is the first signal this tree has ever put through the PA.
#[cfg(feature = "wifi10")]
pub(super) fn tx_pwr_ctl(h: &mut Ht, p: &[PwrIn; 3]) -> TxPctl {
    let before = h.c;
    let (idle, tone_ok, samp_stat) = idle_tssi(h);
    serial_println!(
        "[wifi11] idle-tssi tone={} samp_stat={:#06x} idle=[{},{},{}] wrote-phy-data={} wrote-radio-data={} timeouts={} — phy_ht.c:573-607 (tone :415-421, run :374-413, rssi :467-509, stop :335-357)",
        if tone_ok { "ran" } else { "TIMEOUT" }, samp_stat, idle[0], idle[1], idle[2],
        h.c.phy_data - before.phy_data, h.c.radio_data - before.radio_data, h.c.timeouts - before.timeouts
    );
    tx_power_ctl_setup(h, p, &idle);
    tssi_setup(h);
    tx_power_ctl_on(h);
    let cmd = [h.phy_r(HT_TXPCTL_CMD_C1), h.phy_r(HT_TXPCTL_CMD_C2), h.phy_r(HT_TXPCTL_CMD_C3)];
    let status = [h.phy_r(HT_TX_PCTL_STATUS[0]), h.phy_r(HT_TX_PCTL_STATUS[1]), h.phy_r(HT_TX_PCTL_STATUS[2])];
    let targ = [h.phy_r(HT_TXPCTL_TARG_PWR), h.phy_r(HT_TXPCTL_TARG_PWR2)];
    let idle_regs = [h.phy_r(HT_TXPCTL_IDLE_TSSI), h.phy_r(HT_TXPCTL_IDLE_TSSI2)];
    let on = cmd[0] & HT_TXPCTL_EN_BITS == HT_TXPCTL_EN_BITS;
    serial_println!(
        "[wifi11] txpctl-setup target=[{},{},{}]dBm/4 a1=[{},{},{}] b0=[{},{},{}] b1=[{},{},{}] init_idx=0x19 cmd=[{:#06x},{:#06x},{:#06x}] en_bits={:#06x} targ_pwr={:#06x}/{:#06x} idle_tssi={:#06x}/{:#06x} status=[{:#06x},{:#06x},{:#06x}] est_tables=3x64(HTTAB16 26..28) tssi_mux=set wrote-phy-data={} wrote-radio-data={} -> {} — phy_ht.c:621-729, :609-619, :533-571",
        p[0].target, p[1].target, p[2].target, p[0].a1, p[1].a1, p[2].a1, p[0].b0, p[1].b0, p[2].b0, p[0].b1, p[1].b1, p[2].b1,
        cmd[0], cmd[1], cmd[2], cmd[0] & HT_TXPCTL_EN_BITS, targ[0], targ[1], idle_regs[0], idle_regs[1],
        status[0], status[1], status[2], h.c.phy_data - before.phy_data, h.c.radio_data - before.radio_data,
        if on { "ON" } else { "NOT-ON" }
    );
    TxPctl { on, idle, status, cmd, targ, idle_regs, tone_ok, samp_stat, c: h.c }
}

// ── STATIONROAM-XCHAN (rmbp-ledger B898): the kept station's channel switch ─────────────────────────────────────────────

/// `phy_common.c:416-441` (`b43_switch_channel`) -> `phy_ht.c:807-835` (`b43_phy_ht_op_switch_channel`): the hardware half of a
/// channel switch on the PHY `run` initialised this boot — ONLY `tune` (the 2059 PLL, the PHY channel setup, tx_power_fix, the
/// 8 ms settle), the same fn `run` calls twice. No one-shot: b43's switch re-runs no init, no tx_prep, no power-control setup
/// (2.4 GHz power control reads the band's SPROM block, `phy_ht.c` tx_power_ctl_setup) and touches no DMA. Channels 1..12 only:
/// 13/14 take spuravoid=1 (`phy_ht.c:740-741`), whose PMU PLL step is outside the d11 window (STATIONROAM-SWEEP owed). Refused
/// BEFORE any write. Preconditions (the caller, `join::retune`): the MAC suspended, the SHM cookie written, no spinlock held
/// (`tune` sleeps), the d11 owned (the Station taken out of `STATION`).
#[cfg(feature = "wifi10")]
pub(super) fn switch_channel(bar0: u64, ch: u16) -> Result<Tune, &'static str> {
    if !(1..=12).contains(&ch) {
        return Err("spuravoid");
    }
    let Some(e) = chan_entry(2407 + 5 * ch) else { return Err("no-channel-row") };
    let mut h = Ht::new(bar0);
    Ok(tune(&mut h, ch, e))
}

