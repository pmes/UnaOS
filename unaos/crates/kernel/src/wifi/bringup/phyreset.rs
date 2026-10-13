// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! WIFI8 (rmbp-ledger B529, R110) — rung S5r: the PHY reset through the AI wrapper, with the two
//! read-backs that make it a measurement — the HT-PHY table RAM against b43's init contents, and the
//! 2059 radio's init registers — taken BEFORE and AFTER the reset.
//!
//! CHARTER: Kernel — driver.
//!
//! **What this rung writes** (every write counted in [`super::PhyWrites`] and printed by `phy_end`):
//! the wrapper `IOCTL` three times (the reset sequence below), the PHY address port `d11+0x3FC`
//! (to select `TABLE_ADDR` and the two data registers), the PHY data port `d11+0x3FE` (ONLY to
//! load `TABLE_ADDR`, the table cursor — never a PHY register that configures the radio path), and
//! the core-rev>=24 radio address port `d11+0x3D8` with the READ flag set. No radio DATA write, no
//! channel tune, no transmit path: the seat's rule for this arc (rmbp-queue, 2026-10-13).
//!
//! **The reset sequence** is b43's `b43_bcma_phy_reset` (`main.c:1233-1245`) followed by
//! `b43_phy_take_out_of_reset`'s BCMA arm (`phy_common.c:371-392`), as hardware FACTS under R110
//! (bit names from `b43.h:496-504`; no code copied):
//!   1. `IOCTL <- IOCTL | PHY_RESET(0x8) | PHY_CLKEN(0x4)`, settle 2 us;
//!   2. `IOCTL <- (IOCTL & !PHY_RESET) | FGC(0x2)`, settle 1 us;
//!   3. `IOCTL <- IOCTL & !FGC`, settle 1 us.
//! On the measured pre-image `0x2055` (CLK|PHY_CLKEN|MACPHYCLKEN|BW_20|GMODE, bcm4331.md §7 S3)
//! step 3 lands the word back on `0x2055` byte for byte — the IOCTL unwind is the sequence itself.
//! What is NOT unwound, and is said so on the wire: the PHY's internal state. A reset puts the
//! PHY's registers and table RAM at their power-on values; the platform firmware's HT-PHY init that
//! S5-0 relied on is gone from that moment, and the re-init (b43's `b43_phy_ht_op_init`, the table
//! upload this file only READS) is the owed arc. The read-back AFTER the reset measures exactly that
//! loss (`survived=`), which is the number the re-init arc needs.
//!
//! **Table read-back** (`tables_phy_ht.c` `b43_httab_read`, `tables_phy_ht.h:6-12`, `phy_ht.h:13-15`):
//! `TABLE_ADDR` (PHY reg 0x072) takes `id << 10 | offset`; a 16-bit element is one `TABLE_DATALO`
//! (0x073) read, a 32-bit element is `TABLE_DATAHI` (0x074) then `TABLE_DATALO`. A posting read of
//! `PHY_VER` every 24 PHY writes (`phy_common.c:225-229`). The cursor is written per element — no
//! reliance on auto-increment.
//!
//! **Radio read-back**: core rev >= 24 reaches the radio through `0x3D8`/`0x3DA` (`b43.h:93-94`,
//! `main.c:4557-4568`); an HT-PHY read sets 0x200 in the register number (`phy_ht.c:1095-1099`).
//! The nine `r2059_phy_rev1_init` pairs are written by b43 with `R2059_ALL` routing, so core 1's
//! copy (`R2059_C1`) is what is read here.
//!
//! **Gate**: PHY type 7 at revision <= 1 (`main.c:4517-4520`) and radio version 0x2059
//! (`main.c:4614-4616`), both from `phy_once`'s reads this boot; and `UPLOAD_PROVEN` through it.

use super::{r16, r32, settle, w32, PhyWrites, BAR0_WRAP_OFF, D11_MACCTL, D11_PHY_VER, IOCTL_FGC, IOCTL_PHY_RESET, MACCTL_PSM_RUN, WRAP_IOCTL};
use crate::wifi::htphy_tables::{HtTab, Width, B43_HTTAB_1A_C0_LATE, B43_PHY_HT_TABLES_INIT};
use crate::wifi::radio2059::{init_table, R2059_C1, R2059_READ};
use crate::wifi::status;

/// `b43.h:104-105` — the PHY register address and data ports.
const D11_PHY_CONTROL: u64 = 0x03FC;
const D11_PHY_DATA: u64 = 0x03FE;
/// `b43.h:93-94` — the core-rev>=24 radio address and data ports.
const D11_RADIO_CONTROL24: u64 = 0x03D8;
const D11_RADIO_DATA24: u64 = 0x03DA;
/// `phy_ht.h:13-15` — the table cursor and its two data registers.
const PHY_HT_TABLE_ADDR: u16 = 0x072;
const PHY_HT_TABLE_DATALO: u16 = 0x073;
const PHY_HT_TABLE_DATAHI: u16 = 0x074;
/// `b43.h:496-504` — the AI wrapper IOCTL bits this rung composes (PHY_RESET is `super`'s).
const IOCTL_PHY_CLKEN: u32 = 0x0000_0004;
/// `phy_common.c:225-229` — the posting-read cadence.
const POSTING_EVERY: u32 = 24;

/// The rung's verdicts, carried to `phy_once`'s end line and to `tests wifi`.
pub(super) struct Verdict {
    pub reset_done: bool,
    pub ioctl_restored: bool,
    pub before_match: u32,
    pub after_match: u32,
    pub survived: u32,
    pub elems: u32,
}

/// # Safety
/// As [`super::w32`]: the window is mapped and proven to be the d11 core's.
unsafe fn w16(base: u64, off: u64, val: u16) {
    core::ptr::write_volatile((base + off) as *mut u16, val);
}

struct Phy<'a> {
    bar0: u64,
    pw: &'a mut PhyWrites,
    writes: u32,
}

impl Phy<'_> {
    /// Select a PHY register (one address-port write).
    fn select(&mut self, reg: u16) {
        unsafe { w16(self.bar0, D11_PHY_CONTROL, reg) };
        self.pw.core_regs += 1;
        self.posted();
    }
    fn read(&mut self, reg: u16) -> u16 {
        self.select(reg);
        unsafe { r16(self.bar0, D11_PHY_DATA) }
    }
    /// The ONE PHY data write this rung makes: the table cursor.
    fn set_cursor(&mut self, addr: u16) {
        self.select(PHY_HT_TABLE_ADDR);
        unsafe { w16(self.bar0, D11_PHY_DATA, addr) };
        self.pw.core_regs += 1;
        self.posted();
    }
    fn posted(&mut self) {
        self.writes += 1;
        if self.writes >= POSTING_EVERY {
            let _ = unsafe { r16(self.bar0, D11_PHY_VER) };
            self.writes = 0;
        }
    }
    /// One table element at `t.addr() + i`.
    fn table_elem(&mut self, t: &HtTab, i: usize) -> u32 {
        self.set_cursor(t.addr().wrapping_add(i as u16));
        match t.width {
            Width::W16 => self.read(PHY_HT_TABLE_DATALO) as u32,
            Width::W32 => {
                let hi = self.read(PHY_HT_TABLE_DATAHI) as u32;
                let lo = self.read(PHY_HT_TABLE_DATALO) as u32;
                (hi << 16) | lo
            }
        }
    }
    /// A radio register through the rev>=24 ports, core 1, read flag set.
    fn radio_read(&mut self, reg: u16) -> u16 {
        unsafe { w16(self.bar0, D11_RADIO_CONTROL24, reg | R2059_C1 | R2059_READ) };
        self.pw.radio_addr += 1;
        unsafe { r16(self.bar0, D11_RADIO_DATA24) }
    }
}

/// Read every init table back and compare with b43's contents. Returns (matched, elements,
/// 0x1a/0xc0 verdict: 0 neither, 1 early, 2 late) and prints one `[wifi8] httab` line per table.
fn tables(phy: &mut Phy<'_>, when: &str) -> (u32, u32, u8) {
    let (mut matched, mut elems) = (0u32, 0u32);
    let mut c0: u8 = 0;
    for t in B43_PHY_HT_TABLES_INIT.iter() {
        let (mut m, mut first) = (0u32, None::<(usize, u32, u32)>);
        let (mut m_late, late) = (0u32, t.id == 0x1a && t.offset == 0x0c0);
        for i in 0..t.len() {
            let got = phy.table_elem(t, i);
            if got == t.get(i) { m += 1; } else if first.is_none() { first = Some((i, got, t.get(i))); }
            if late && got == B43_HTTAB_1A_C0_LATE.get(i) { m_late += 1; }
        }
        matched += m;
        elems += t.len() as u32;
        if late {
            c0 = if m == t.len() as u32 { 1 } else if m_late == t.len() as u32 { 2 } else { 0 };
        }
        serial_println!(
            "[wifi8] httab when={} name={} id={:#04x} off={:#05x} width={} n={} match={}{}{}",
            when, t.name, t.id, t.offset, if matches!(t.width, Width::W16) { 16 } else { 32 }, t.len(), m,
            if late { alloc::format!(" late_match={}", m_late) } else { alloc::string::String::new() },
            first.map(|(i, g, w)| alloc::format!(" first_mismatch={} got={:#x} want={:#x}", i, g, w)).unwrap_or_default()
        );
    }
    (matched, elems, c0)
}

/// Read the 2059 init registers back; returns (matched, total).
fn radio_init(phy: &mut Phy<'_>, when: &str, phy_rev: u16) -> (u32, u32) {
    let Some(t) = init_table(phy_rev) else {
        serial_println!("[wifi8] r2059-init when={} phy_rev={} -> NO-TABLE (radio_2059.c:323-331 carries rev 1 only)", when, phy_rev);
        return (0, 0);
    };
    let mut m = 0u32;
    for [reg, val] in t.iter() {
        let got = phy.radio_read(*reg);
        let ok = (got & 0xFF) == (*val & 0xFF);
        if ok { m += 1; }
        serial_println!("[wifi8] r2059-init when={} reg={:#05x} got={:#06x} want={:#04x} {}", when, reg, got, val, if ok { "MATCH" } else { "DIFFERS" });
    }
    (m, t.len() as u32)
}

/// The rung. `gate_ok` is `phy_once`'s identity verdict (type 7, radio 0x2059, alive).
pub(super) fn run(bar0: u64, pw: &mut PhyWrites, gate_ok: bool, phy_rev: u16, radio_ver: u32) -> Verdict {
    let mut v = Verdict { reset_done: false, ioctl_restored: false, before_match: 0, after_match: 0, survived: 0, elems: 0 };
    if !gate_ok || phy_rev > 1 || radio_ver != 0x2059 {
        serial_println!(
            ":: wifi8: phy-reset REFUSED reason=gate gate_ok={} phy_rev={} want<=1 radio_ver={:#06x} want=0x2059 -> DECLINED(S5r) — main.c:4517-4520 and :4614-4616 are the two version gates b43 itself applies before it drives an HT-PHY ::",
            gate_ok as u8, phy_rev, radio_ver
        );
        status::set_phy_reset(3);
        return v;
    }
    let mut phy = Phy { bar0, pw, writes: 0 };
    // Pre-images of the two cursors this rung moves, restored at the end.
    let cursor_pre = phy.read(PHY_HT_TABLE_ADDR);
    let radio_addr_pre = unsafe { r16(bar0, D11_RADIO_CONTROL24) };

    // ── Read-back BEFORE: the platform firmware's PHY state against b43's init. ──
    let (b_m, elems, b_c0) = tables(&mut phy, "before");
    let (r_m, r_n) = radio_init(&mut phy, "before", phy_rev);
    serial_println!(
        ":: wifi8: htphy-tables when=before tables={} elems={} match={} table_1a_0xc0={} r2059_init={}/{} -> {} — the EFI's PHY against tables_phy_ht.c; a full MATCH means b43's init IS the firmware's and a re-init after reset is reproducible from the data in the tree ::",
        B43_PHY_HT_TABLES_INIT.len(), elems, b_m, ["neither", "early", "late"][b_c0 as usize], r_m, r_n,
        if b_m == elems { "MATCH" } else if b_m > 0 { "PARTIAL" } else { "DIFFERS" }
    );
    v.before_match = b_m;
    v.elems = elems;

    // ── The reset: b43_bcma_phy_reset + b43_phy_take_out_of_reset (BCMA arm). ──
    let pre = unsafe { r32(bar0, BAR0_WRAP_OFF + WRAP_IOCTL) };
    let w1 = pre | IOCTL_PHY_RESET | IOCTL_PHY_CLKEN;
    unsafe { w32(bar0, BAR0_WRAP_OFF + WRAP_IOCTL, w1) };
    phy.pw.wrapper += 1;
    let (s1, u1) = settle(2);
    let mid = unsafe { r32(bar0, BAR0_WRAP_OFF + WRAP_IOCTL) };
    let w2 = (mid & !IOCTL_PHY_RESET) | IOCTL_FGC;
    unsafe { w32(bar0, BAR0_WRAP_OFF + WRAP_IOCTL, w2) };
    phy.pw.wrapper += 1;
    let (s2, u2) = settle(1);
    let mid2 = unsafe { r32(bar0, BAR0_WRAP_OFF + WRAP_IOCTL) };
    let w3 = mid2 & !IOCTL_FGC;
    unsafe { w32(bar0, BAR0_WRAP_OFF + WRAP_IOCTL, w3) };
    phy.pw.wrapper += 1;
    let (s3, u3) = settle(1);
    let post = unsafe { r32(bar0, BAR0_WRAP_OFF + WRAP_IOCTL) };
    let in_reset_seen = (mid & IOCTL_PHY_RESET) != 0;
    let phy_ver = unsafe { r16(bar0, D11_PHY_VER) };
    let macctl = unsafe { r32(bar0, D11_MACCTL) };
    v.reset_done = in_reset_seen && (post & IOCTL_PHY_RESET) == 0;
    v.ioctl_restored = post == pre;
    serial_println!(
        ":: wifi8: phy-reset pre={:#010x} 1/3={:#010x} read={:#010x} in_reset={} settle={}{} 2/3={:#010x} settle={}{} 3/3={:#010x} settle={}{} post={:#010x} ioctl_restored={} phy_ver_after={:#06x} psm_run_after={} phy_state=reset-defaults(not-unwindable; re-init owed) -> {} — main.c:1233-1245 + phy_common.c:371-392, bits b43.h:496-504 ::",
        pre, w1, mid, in_reset_seen as u8, s1, u1, w2, s2, u2, w3, s3, u3, post,
        if v.ioctl_restored { "MATCH" } else { "DIFFERS" }, phy_ver, ((macctl & MACCTL_PSM_RUN) != 0) as u8,
        if v.reset_done && v.ioctl_restored { "DONE" } else { "FAILED" }
    );

    // ── Read-back AFTER: what the reset left. ──
    let (a_m, _, a_c0) = tables(&mut phy, "after");
    let (ra_m, ra_n) = radio_init(&mut phy, "after", phy_rev);
    v.after_match = a_m;
    v.survived = a_m;
    serial_println!(
        ":: wifi8: htphy-tables when=after tables={} elems={} match={} table_1a_0xc0={} r2059_init={}/{} survived={}/{} -> {} — the loss the re-init arc must repay; `survived` counts elements still equal to b43's init after the reset ::",
        B43_PHY_HT_TABLES_INIT.len(), elems, a_m, ["neither", "early", "late"][a_c0 as usize], ra_m, ra_n, a_m, b_m,
        if a_m == b_m { "UNCHANGED" } else { "CHANGED" }
    );

    // ── Restore the two cursors. ──
    phy.set_cursor(cursor_pre);
    unsafe { w16(bar0, D11_RADIO_CONTROL24, radio_addr_pre) };
    phy.pw.radio_addr += 1;
    let cur_post = phy.read(PHY_HT_TABLE_ADDR);
    let ra_post = unsafe { r16(bar0, D11_RADIO_CONTROL24) };
    serial_println!(
        ":: wifi8: restore table_addr pre={:#06x} post={:#06x} {} radio_addr24 pre={:#06x} post={:#06x} {} ::",
        cursor_pre, cur_post, if cur_post == cursor_pre { "MATCH" } else { "DIFFERS" },
        radio_addr_pre, ra_post, if ra_post == radio_addr_pre { "MATCH" } else { "DIFFERS" }
    );
    status::set_phy_reset(if v.reset_done && v.ioctl_restored { 1 } else { 2 });
    v
}
