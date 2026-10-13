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
