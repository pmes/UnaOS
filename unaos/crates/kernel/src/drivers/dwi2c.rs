//! CHARTER: Kernel — driver (the AMD FCH DesignWare I2C master + an I2C-HID descriptor census, ZENBOOKTOUCHPAD zenbook-ledger Z14; no CODEX §2 handler owns a platform I2C bus, ec.rs sits in the same seam)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! DWI2C — the AMD FCH DesignWare I2C master, rung 1: a read-only bus census and the touchpad's I2C-HID descriptor
//! (ZENBOOKTOUCHPAD, zenbook-ledger Z14).
//!
//! The Zenbook's touchpad speaks HID over I2C on an AMD DesignWare controller. Measured at ae8272e02:
//! * the controller is NOT a PCI function (pci.ids has no AMD I2C row; 1022:15e4/164a is the Sensor Fusion Hub): it
//!   is ACPI `AMDI0010` (linux drivers/acpi/acpi_apd.c `wt_i2c_desc`, a fixed 150 MHz clock), its base the DSDT's
//!   `_CRS` — AML, which this tree cannot run (Z8). The base used here is the FCH's FIXED map (coreboot
//!   src/soc/amd/{phoenix,glinda}/include/soc/iomap.h: APU_I2C0..3_BASE), and each controller is IDENTIFIED by
//!   register (IC_COMP_TYPE == 0x44570140, "DW" + 0x0140) before anything is written; 0xffffffff (power-gated by
//!   the AOAC block, or absent) is `absent`, no write;
//! * STANDARD mode, 100 kHz: the firmware's SS_SCL counts are kept when both are set, else linux's formula at
//!   150 MHz (tHIGH 4000 ns, tLOW 4700 ns, tf 300 ns: 642 / 749). Fast mode needs the DSDT's FMCN (owed);
//! * the bus probe is a 1-byte READ (DesignWare has no quick write: every DATA_CMD carries a data phase); an
//!   address NACK is TX_ABRT with TX_ABRT_SOURCE bit 0;
//! * the I2C-HID descriptor register is the device's `_DSM` (AML again): the 30-byte descriptor is read ONLY at an
//!   acking address of the guess table `HID_GUESS` (ELAN 0x15/0x0001, Synaptics 0x2c/0x0020), so no register write
//!   ever reaches an unknown device (the CS35L41 amplifiers).
//!
//! Runs once at `pci.rs init` behind `hw::gate(Probe::I2c)` (the Zenbook only: the fixed map is AMD's); every
//! controller is left AS FOUND (CON, TAR, SS counts, INTR_MASK, ENABLE saved and restored) and a busy one is refused
//! untouched. Every wait is `drivers::block::wait::bounded` (TSC, right with interrupts masked inside pci::init).
//! The touchpad's reports are owed (ZENBOOK-TOUCHPAD-REPORTS). `tests dwi2c` runs the register-sequence machine
//! against a fake controller: `:: DWI2C: proto=ok cases=8/8 -> PASS ::`.

use core::cell::RefCell;

const IC_CON: usize = 0x00;
const IC_TAR: usize = 0x04;
const IC_DATA_CMD: usize = 0x10;
const IC_SS_SCL_HCNT: usize = 0x14;
const IC_SS_SCL_LCNT: usize = 0x18;
const IC_INTR_MASK: usize = 0x30;
const IC_RAW_INTR_STAT: usize = 0x34;
const IC_CLR_INTR: usize = 0x40;
const IC_CLR_TX_ABRT: usize = 0x54;
const IC_ENABLE: usize = 0x6c;
const IC_STATUS: usize = 0x70;
const IC_RXFLR: usize = 0x78;
const IC_TX_ABRT_SOURCE: usize = 0x80;
const IC_ENABLE_STATUS: usize = 0x9c;
const IC_COMP_PARAM_1: usize = 0xf4;
const IC_COMP_VERSION: usize = 0xf8;
const IC_COMP_TYPE: usize = 0xfc;

/// IC_COMP_TYPE of every DesignWare APB I2C ("DW" + 0x0140).
const COMP_TYPE_DW: u32 = 0x4457_0140;
const CON_MASTER: u32 = 1 << 0;
const CON_SPEED_STD: u32 = 1 << 1;
const CON_RESTART_EN: u32 = 1 << 5;
const CON_SLAVE_DISABLE: u32 = 1 << 6;
const CMD_READ: u32 = 1 << 8;
const CMD_STOP: u32 = 1 << 9;
const CMD_RESTART: u32 = 1 << 10;
const INTR_TX_ABRT: u32 = 1 << 6;
const INTR_STOP_DET: u32 = 1 << 9;
const ST_ACTIVITY: u32 = 1 << 0;
const ST_TFNF: u32 = 1 << 1;
const ABRT_7B_ADDR_NOACK: u32 = 1 << 0;
/// Standard-mode counts at the 150 MHz AMDI0010 clock (linux i2c_dw_scl_hcnt / i2c_dw_scl_lcnt).
const SS_HCNT_150: u32 = 642;
const SS_LCNT_150: u32 = 749;
/// One enable/disable handshake.
const WAIT_EN_MS: u64 = 10;
/// One whole transfer (100 kHz x 32 bytes is about 3 ms).
const XFER_MS: u64 = 20;
/// The FCH's four I2C masters (coreboot APU_I2C0..3_BASE).
const BASES: [u64; 4] = [0xfedc_2000, 0xfedc_3000, 0xfedc_4000, 0xfedc_5000];
/// (address, HID descriptor register): the only places a register-pointer write is made.
const HID_GUESS: [(u8, u16); 2] = [(0x15, 0x0001), (0x2c, 0x0020)];
/// Acking addresses kept for the wire.
const ACKS_MAX: usize = 16;

/// The register seam the KAT fakes.
trait Mmio {
    fn rd(&self, off: usize) -> u32;
    fn wr(&self, off: usize, v: u32);
}

/// One mapped controller page.
struct Real {
    base: u64,
}

impl Mmio for Real {
    fn rd(&self, off: usize) -> u32 {
        // SAFETY: `base` is mapped uncacheable (map_mmio_window) and proved present (translate) by `probe`; `off` is
        // one of this file's register offsets, inside the 4 KiB page.
        unsafe { core::ptr::read_volatile((self.base + off as u64) as *const u32) }
    }
    fn wr(&self, off: usize, v: u32) {
        // SAFETY: as `rd`; writes are made only after IC_COMP_TYPE identified a DesignWare controller.
        unsafe { core::ptr::write_volatile((self.base + off as u64) as *mut u32, v) }
    }
}

/// A step that did not settle, and the register it last read.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Fail {
    step: &'static str,
    src: u32,
}

/// The read-only identification.
struct Ident {
    comp: u32,
    ver: u32,
    param1: u32,
    con: u32,
    en: u32,
}

/// The registers this rung writes, as found.
struct Saved {
    con: u32,
    tar: u32,
    hcnt: u32,
    lcnt: u32,
    mask: u32,
    en: u32,
}

/// The validated I2C-HID descriptor's fields the wire names.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct HidDesc {
    vid: u16,
    pid: u16,
    ver: u16,
    rdesc: u16,
    input: u16,
    maxin: u16,
}

/// One bus scan.
struct Scan {
    acks: [u8; ACKS_MAX],
    n: usize,
    nacks: u32,
    stuck: Option<(u8, Fail)>,
}

/// What one controller's run came to (the KAT reads it).
#[derive(Default)]
struct Outcome {
    found: bool,
    refused: bool,
    stuck: bool,
    acks: usize,
    first_ack: u8,
    nacks: u32,
    desc: Option<HidDesc>,
    restored: bool,
}

fn identify<I: Mmio>(io: &I) -> Ident {
    Ident {
        comp: io.rd(IC_COMP_TYPE),
        ver: io.rd(IC_COMP_VERSION),
        param1: io.rd(IC_COMP_PARAM_1),
        con: io.rd(IC_CON),
        en: io.rd(IC_ENABLE),
    }
}

/// Read commands in flight at most: the RX FIFO depth from IC_COMP_PARAM_1[15:8] + 1.
fn window(param1: u32) -> usize {
    ((param1 >> 8) & 0xff) as usize + 1
}

/// Write IC_ENABLE and wait (bounded) for IC_ENABLE_STATUS bit 0 to follow.
fn set_enable<I: Mmio>(io: &I, on: bool, step: &'static str) -> Result<(), Fail> {
    let want = on as u32;
    io.wr(IC_ENABLE, want);
    if crate::drivers::block::wait::bounded(WAIT_EN_MS, || io.rd(IC_ENABLE_STATUS) & 1 == want) {
        Ok(())
    } else {
        Err(Fail { step, src: io.rd(IC_ENABLE_STATUS) })
    }
}

/// Refuse a busy controller (nothing written), save what this rung writes, disable, mask, master + standard mode.
/// (saved, firmware timing kept).
fn open<I: Mmio>(io: &I) -> Result<(Saved, bool), Fail> {
    let st = io.rd(IC_STATUS);
    if st & ST_ACTIVITY != 0 {
        return Err(Fail { step: "busy", src: st });
    }
    let s = Saved {
        con: io.rd(IC_CON),
        tar: io.rd(IC_TAR),
        hcnt: io.rd(IC_SS_SCL_HCNT),
        lcnt: io.rd(IC_SS_SCL_LCNT),
        mask: io.rd(IC_INTR_MASK),
        en: io.rd(IC_ENABLE),
    };
    if let Err(f) = set_enable(io, false, "disable") {
        io.wr(IC_ENABLE, s.en & 1);
        return Err(f);
    }
    io.wr(IC_INTR_MASK, 0);
    io.wr(IC_CON, CON_MASTER | CON_SPEED_STD | CON_RESTART_EN | CON_SLAVE_DISABLE);
    let fw = s.hcnt != 0 && s.lcnt != 0;
    if !fw {
        io.wr(IC_SS_SCL_HCNT, SS_HCNT_150);
        io.wr(IC_SS_SCL_LCNT, SS_LCNT_150);
    }
    Ok((s, fw))
}

/// One transfer to `addr`: the `wr` bytes, then `rd.len()` reads (RESTART on the first read after a write, STOP on
/// the last command). Bounded by `XFER_MS`; a TX abort returns its source.
fn xfer<I: Mmio>(io: &I, addr: u8, wr: &[u8], rd: &mut [u8], win: usize) -> Result<(), Fail> {
    set_enable(io, false, "disable")?;
    io.wr(IC_TAR, (addr & 0x7f) as u32);
    set_enable(io, true, "enable")?;
    let _ = io.rd(IC_CLR_INTR);
    let total = wr.len() + rd.len();
    let mut sent = 0usize;
    let mut got = 0usize;
    let start = crate::arch::now_cycles();
    let budget = crate::drivers::block::wait::cycles_ms(XFER_MS);
    loop {
        let raw = io.rd(IC_RAW_INTR_STAT);
        if raw & INTR_TX_ABRT != 0 {
            let src = io.rd(IC_TX_ABRT_SOURCE);
            let _ = io.rd(IC_CLR_TX_ABRT);
            let _ = io.rd(IC_CLR_INTR);
            return Err(Fail { step: "abort", src });
        }
        while got < rd.len() && io.rd(IC_RXFLR) > 0 {
            rd[got] = io.rd(IC_DATA_CMD) as u8;
            got += 1;
        }
        if sent < total && io.rd(IC_STATUS) & ST_TFNF != 0 {
            let reading = sent >= wr.len();
            if !reading || (sent - wr.len()) - got < win {
                let mut cmd = if reading { CMD_READ } else { wr[sent] as u32 };
                if reading && sent == wr.len() && !wr.is_empty() {
                    cmd |= CMD_RESTART;
                }
                if sent + 1 == total {
                    cmd |= CMD_STOP;
                }
                io.wr(IC_DATA_CMD, cmd);
                sent += 1;
                continue;
            }
        }
        if sent == total && got == rd.len() && raw & INTR_STOP_DET != 0 {
            let _ = io.rd(IC_CLR_INTR);
            return Ok(());
        }
        if crate::arch::now_cycles().wrapping_sub(start) >= budget {
            return Err(Fail { step: "xfer", src: raw });
        }
        core::hint::spin_loop();
    }
}

/// 0x08..=0x77, one 1-byte read each; an address NACK is counted, any other failure stops the scan.
fn scan<I: Mmio>(io: &I, win: usize) -> Scan {
    let mut s = Scan { acks: [0; ACKS_MAX], n: 0, nacks: 0, stuck: None };
    for addr in 0x08u8..=0x77 {
        let mut b = [0u8; 1];
        match xfer(io, addr, &[], &mut b, win) {
            Ok(()) => {
                if s.n < ACKS_MAX {
                    s.acks[s.n] = addr;
                }
                s.n += 1;
            }
            Err(f) if f.step == "abort" && f.src & ABRT_7B_ADDR_NOACK != 0 => s.nacks += 1,
            Err(f) => {
                s.stuck = Some((addr, f));
                break;
            }
        }
    }
    s
}

/// HID over I2C 1.0 §5.1: wHIDDescLength 30 and bcdVersion 0x0100, else not a descriptor.
fn parse_desc(d: &[u8; 30]) -> Option<HidDesc> {
    let w = |i: usize| u16::from_le_bytes([d[i], d[i + 1]]);
    if w(0) != 30 || w(2) != 0x0100 {
        return None;
    }
    Some(HidDesc { rdesc: w(4), input: w(8), maxin: w(10), vid: w(20), pid: w(22), ver: w(24) })
}

/// The descriptor register written (little-endian), then 30 bytes read after a RESTART.
fn hid_desc<I: Mmio>(io: &I, addr: u8, reg: u16, win: usize) -> Result<Option<HidDesc>, Fail> {
    let mut d = [0u8; 30];
    xfer(io, addr, &reg.to_le_bytes(), &mut d, win)?;
    Ok(parse_desc(&d))
}

/// Disable, write back what `open` saved, re-enable only if it was. false = the disable did not settle (the
/// write-backs may not have taken).
fn close<I: Mmio>(io: &I, s: &Saved) -> bool {
    let off = set_enable(io, false, "disable").is_ok();
    io.wr(IC_CON, s.con);
    io.wr(IC_TAR, s.tar);
    io.wr(IC_SS_SCL_HCNT, s.hcnt);
    io.wr(IC_SS_SCL_LCNT, s.lcnt);
    io.wr(IC_INTR_MASK, s.mask);
    io.wr(IC_ENABLE, s.en & 1);
    off
}

/// One controller: identify, open, scan, the guess-table descriptor, close. `say` = print the wire (off in the KAT).
fn run<I: Mmio>(i: usize, base: u64, io: &I, say: bool) -> Outcome {
    let mut o = Outcome::default();
    let id = identify(io);
    let present = id.comp == COMP_TYPE_DW;
    if say {
        serial_println!(
            "[dwi2c] ctrl={} base={:#x} comp={:#010x} ver={:#010x} param1={:#010x} con={:#x} en={} -> {}",
            i, base, id.comp, id.ver, id.param1, id.con, id.en & 1, if present { "found" } else { "absent" }
        );
    }
    if !present {
        return o;
    }
    o.found = true;
    let (saved, fw) = match open(io) {
        Ok(v) => v,
        Err(f) if f.step == "busy" => {
            if say {
                serial_println!("[dwi2c] ctrl={} -> REFUSED why=busy status={:#x}", i, f.src);
            }
            o.refused = true;
            return o;
        }
        Err(f) => {
            if say {
                serial_println!("[dwi2c] ctrl={} scan -> stuck at=- step={} src={:#x}", i, f.step, f.src);
            }
            o.stuck = true;
            return o;
        }
    };
    let win = window(id.param1);
    let t0 = crate::arch::now_cycles();
    let s = scan(io, win);
    let ms = crate::arch::now_cycles().wrapping_sub(t0) / crate::drivers::block::wait::cycles_ms(1).max(1);
    let kept = &s.acks[..s.n.min(ACKS_MAX)];
    o.acks = s.n;
    o.nacks = s.nacks;
    o.first_ack = kept.first().copied().unwrap_or(0);
    if say {
        for &a in kept {
            serial_println!("[dwi2c] ctrl={} scan addr={:#04x} ack", i, a);
        }
    }
    let (hc, lc) = (io.rd(IC_SS_SCL_HCNT), io.rd(IC_SS_SCL_LCNT));
    let timing = if fw { "fw" } else { "calc" };
    match s.stuck {
        None => {
            if say {
                serial_println!("[dwi2c] ctrl={} scan acks={} nacks={} ms={} hcnt={} lcnt={} timing={} -> ok", i, s.n, s.nacks, ms, hc, lc, timing);
            }
        }
        Some((at, f)) => {
            o.stuck = true;
            if say {
                serial_println!(
                    "[dwi2c] ctrl={} scan acks={} nacks={} ms={} hcnt={} lcnt={} timing={} -> stuck at={:#04x} step={} src={:#x}",
                    i, s.n, s.nacks, ms, hc, lc, timing, at, f.step, f.src
                );
            }
        }
    }
    if s.stuck.is_none() {
        match HID_GUESS.iter().find(|g| kept.contains(&g.0)) {
            None => {
                if say {
                    serial_println!("[dwi2c] ctrl={} hid -> NOT ATTEMPTED why=no-known-address", i);
                }
            }
            Some(&(addr, reg)) => match hid_desc(io, addr, reg, win) {
                Ok(Some(d)) => {
                    o.desc = Some(d);
                    if say {
                        serial_println!(
                            "[dwi2c] ctrl={} hid addr={:#04x} reg={:#06x} vid={:#06x} pid={:#06x} ver={:#06x} rdesc={} input={:#06x} maxin={} -> desc",
                            i, addr, reg, d.vid, d.pid, d.ver, d.rdesc, d.input, d.maxin
                        );
                    }
                }
                Ok(None) => {
                    if say {
                        serial_println!("[dwi2c] ctrl={} hid addr={:#04x} reg={:#06x} -> invalid", i, addr, reg);
                    }
                }
                Err(f) => {
                    if say {
                        serial_println!("[dwi2c] ctrl={} hid addr={:#04x} reg={:#06x} -> stuck step={} src={:#x}", i, addr, reg, f.step, f.src);
                    }
                }
            },
        }
    }
    o.restored = close(io, &saved);
    if say {
        if o.restored {
            serial_println!("[dwi2c] ctrl={} restored en={}", i, saved.en & 1);
        } else {
            serial_println!("[dwi2c] ctrl={} restore -> stuck step=disable", i);
        }
    }
    o
}

/// The once-per-boot census (pci.rs init, after `ec::probe`). Silent unless `hw::gate(Probe::I2c)` (the Zenbook).
pub fn probe() {
    if !crate::hw::gate(crate::hw::Probe::I2c) {
        return;
    }
    for (i, &base) in BASES.iter().enumerate() {
        crate::arch::memory::map_mmio_window(base, 0x1000);
        if crate::arch::memory::translate(base).is_none() {
            serial_println!("[dwi2c] ctrl={} -> SKIP reason=unmapped", i);
            continue;
        }
        let io = Real { base };
        let _ = run(i, base, &io, true);
    }
}

/// The KAT's controller: one device acks at `ack`, answering the descriptor register 0x0001 with `desc`.
struct FakeState {
    comp: u32,
    busy: bool,
    en_reg: u32,
    en_status: u32,
    en_stuck: bool,
    con: u32,
    tar: u32,
    hcnt: u32,
    lcnt: u32,
    mask: u32,
    raw: u32,
    abrt_src: u32,
    rx: [u8; 64],
    rh: usize,
    rn: usize,
    ack: u8,
    desc: [u8; 30],
    in_txn: bool,
    wbuf: [u8; 2],
    wn: usize,
    rpos: usize,
    writes: u32,
}

struct Fake {
    s: RefCell<FakeState>,
}

impl Fake {
    fn new() -> Fake {
        let mut desc = [0u8; 30];
        let put = |d: &mut [u8; 30], i: usize, v: u16| d[i..i + 2].copy_from_slice(&v.to_le_bytes());
        put(&mut desc, 0, 30);
        put(&mut desc, 2, 0x0100);
        put(&mut desc, 4, 0x02c0);
        put(&mut desc, 8, 0x0003);
        put(&mut desc, 10, 0x0020);
        put(&mut desc, 20, 0x04f3);
        put(&mut desc, 22, 0x3242);
        put(&mut desc, 24, 0x0001);
        Fake {
            s: RefCell::new(FakeState {
                comp: COMP_TYPE_DW,
                busy: false,
                en_reg: 0,
                en_status: 0,
                en_stuck: false,
                con: 0x63,
                tar: 0x55,
                hcnt: 0,
                lcnt: 0,
                mask: 0x8ff,
                raw: 0,
                abrt_src: 0,
                rx: [0; 64],
                rh: 0,
                rn: 0,
                ack: 0x15,
                desc,
                in_txn: false,
                wbuf: [0; 2],
                wn: 0,
                rpos: 0,
                writes: 0,
            }),
        }
    }
    fn push_rx(st: &mut FakeState, b: u8) {
        let at = (st.rh + st.rn) % st.rx.len();
        st.rx[at] = b;
        st.rn += 1;
    }
}

impl Mmio for Fake {
    fn rd(&self, off: usize) -> u32 {
        let mut st = self.s.borrow_mut();
        match off {
            IC_COMP_TYPE => st.comp,
            IC_COMP_VERSION => 0x3230_312a,
            IC_COMP_PARAM_1 => (15 << 16) | (15 << 8),
            IC_CON => st.con,
            IC_TAR => st.tar,
            IC_SS_SCL_HCNT => st.hcnt,
            IC_SS_SCL_LCNT => st.lcnt,
            IC_INTR_MASK => st.mask,
            IC_ENABLE => st.en_reg,
            IC_ENABLE_STATUS => st.en_status,
            IC_STATUS => (st.busy as u32) | ST_TFNF,
            IC_RAW_INTR_STAT => st.raw,
            IC_CLR_INTR => {
                st.raw = 0;
                0
            }
            IC_CLR_TX_ABRT => {
                st.raw &= !INTR_TX_ABRT;
                st.abrt_src = 0;
                0
            }
            IC_TX_ABRT_SOURCE => st.abrt_src,
            IC_RXFLR => st.rn as u32,
            IC_DATA_CMD => {
                if st.rn == 0 {
                    return 0;
                }
                let b = st.rx[st.rh];
                st.rh = (st.rh + 1) % st.rx.len();
                st.rn -= 1;
                b as u32
            }
            _ => 0,
        }
    }
    fn wr(&self, off: usize, v: u32) {
        let mut st = self.s.borrow_mut();
        st.writes += 1;
        match off {
            IC_ENABLE => {
                st.en_reg = v & 1;
                if !st.en_stuck {
                    st.en_status = v & 1;
                }
            }
            IC_CON => st.con = v,
            IC_TAR => st.tar = v,
            IC_SS_SCL_HCNT => st.hcnt = v,
            IC_SS_SCL_LCNT => st.lcnt = v,
            IC_INTR_MASK => st.mask = v,
            IC_DATA_CMD => {
                if st.en_status & 1 == 0 {
                    return;
                }
                if !st.in_txn {
                    if st.tar != st.ack as u32 {
                        st.raw |= INTR_TX_ABRT;
                        st.abrt_src = ABRT_7B_ADDR_NOACK;
                        return;
                    }
                    st.in_txn = true;
                    st.wn = 0;
                    st.rpos = 0;
                }
                if v & CMD_READ != 0 {
                    let reg = u16::from_le_bytes(st.wbuf);
                    let b = if st.wn == 2 && reg == 0x0001 { st.desc.get(st.rpos).copied().unwrap_or(0xff) } else { 0xff };
                    st.rpos += 1;
                    Fake::push_rx(&mut st, b);
                } else if st.wn < 2 {
                    let at = st.wn;
                    st.wbuf[at] = v as u8;
                    st.wn += 1;
                }
                if v & CMD_STOP != 0 {
                    st.raw |= INTR_STOP_DET;
                    st.in_txn = false;
                }
            }
            _ => {}
        }
    }
}

fn kat_absent() -> bool {
    let f = Fake::new();
    f.s.borrow_mut().comp = 0xffff_ffff;
    let o = run(0, 0, &f, false);
    let w = f.s.borrow().writes;
    !o.found && w == 0
}

fn kat_identify() -> bool {
    let f = Fake::new();
    let id = identify(&f);
    id.comp == COMP_TYPE_DW && window(id.param1) == 16 && f.s.borrow().writes == 0
}

fn kat_restore() -> bool {
    let f = Fake::new();
    {
        let mut st = f.s.borrow_mut();
        st.con = 0x65;
        st.tar = 0x2c;
        st.hcnt = 0x200;
        st.lcnt = 0x300;
        st.mask = 0x8ff;
        st.en_reg = 1;
        st.en_status = 1;
    }
    let o = run(0, 0, &f, false);
    let st = f.s.borrow();
    o.restored && st.con == 0x65 && st.tar == 0x2c && st.hcnt == 0x200 && st.lcnt == 0x300 && st.mask == 0x8ff && st.en_reg == 1 && st.en_status == 1
}

fn kat_busy() -> bool {
    let f = Fake::new();
    f.s.borrow_mut().busy = true;
    let o = run(0, 0, &f, false);
    let w = f.s.borrow().writes;
    o.found && o.refused && w == 0
}

fn kat_scan() -> bool {
    let f = Fake::new();
    let o = run(0, 0, &f, false);
    o.acks == 1 && o.first_ack == 0x15 && o.nacks == (0x77 - 0x08 + 1) - 1 && !o.stuck
}

fn kat_desc() -> bool {
    let f = Fake::new();
    let o = run(0, 0, &f, false);
    matches!(o.desc, Some(d) if d.vid == 0x04f3 && d.pid == 0x3242 && d.rdesc == 0x02c0 && d.input == 0x0003 && d.maxin == 0x0020)
}

fn kat_nack() -> bool {
    let f = Fake::new();
    let Ok((s, _)) = open(&f) else { return false };
    let mut b = [0u8; 1];
    let r = xfer(&f, 0x50, &[], &mut b, 16);
    let back = close(&f, &s);
    matches!(r, Err(Fail { step: "abort", src }) if src & ABRT_7B_ADDR_NOACK != 0) && back
}

fn kat_wedge() -> bool {
    let f = Fake::new();
    {
        let mut st = f.s.borrow_mut();
        st.en_reg = 1;
        st.en_status = 1;
        st.en_stuck = true;
    }
    matches!(open(&f), Err(Fail { step: "disable", .. })) && f.s.borrow().en_reg == 1
}

/// `tests dwi2c`: the register-sequence machine against the fake controller; no real register is touched.
pub fn selftest() {
    let cases: [(&str, fn() -> bool); 8] = [
        ("absent", kat_absent),
        ("identify", kat_identify),
        ("restore", kat_restore),
        ("busy", kat_busy),
        ("scan", kat_scan),
        ("desc", kat_desc),
        ("nack", kat_nack),
        ("wedge", kat_wedge),
    ];
    let mut ok = 0;
    let mut first: Option<&str> = None;
    for (name, f) in cases {
        let r = f();
        serial_println!("[dwi2c] kat case={} {}", name, if r { "ok" } else { "FAIL" });
        if r {
            ok += 1;
        } else if first.is_none() {
            first = Some(name);
        }
    }
    match first {
        None => serial_println!(":: DWI2C: proto=ok cases={}/{} -> PASS ::", ok, cases.len()),
        Some(name) => serial_println!(":: DWI2C: proto=fail case={} -> FAIL ::", name),
    }
}
