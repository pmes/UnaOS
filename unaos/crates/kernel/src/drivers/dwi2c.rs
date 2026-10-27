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
//!   150 MHz (tHIGH 4000 ns, tLOW 4700 ns, tf 300 ns: 642 / 749). The held touchpad's 400 kHz from the DSDT's FMCN is DWI2CFAST (Z50, file tail);
//! * the bus probe is a 1-byte READ (DesignWare has no quick write: every DATA_CMD carries a data phase); an
//!   address NACK is TX_ABRT with TX_ABRT_SOURCE bit 0;
//! * the I2C-HID descriptor register is the device's `_DSM` (AML again): the 30-byte descriptor is read ONLY at an
//!   acking address of the guess table `HID_GUESS` (ELAN 0x15/0x0001, Synaptics 0x2c/0x0020), so no register write
//!   ever reaches an unknown device (the CS35L41 amplifiers).
//!
//! Runs once at `pci.rs init` behind `hw::gate(Probe::I2c)` (the Zenbook only: the fixed map is AMD's); every
//! controller is left AS FOUND (CON, TAR, SS counts, INTR_MASK, ENABLE saved and restored) and a busy one is refused
//! untouched. Every wait is `drivers::block::wait::bounded` (TSC, right with interrupts masked inside pci::init).
//! The touchpad's reports: the `touchpad` module at the tail (ZENBOOKTOUCHPADREPORTS, zenbook-ledger Z18; `ehcihid`
//! images, pumped by the one HID pass) — the controller that answered is HELD open for it only when armed. `tests dwi2c`
//! runs the register-sequence machine against a fake controller: `:: DWI2C: proto=ok cases=8/8 -> PASS ::`;
//! `tests dwi2ctp` the touchpad over the same fake: `:: DWI2C-TP: rdesc=ok reports=2 events=2 scroll2 dy=<n> palm=ok -> PASS ::`. DWI2CACPI (zenbook-ledger Z32): the DSDT byte scan `aml_scan` (file tail) cross-checks BASES / HID_GUESS (`[dwi2c] acpi ...`), `tests dwi2cacpi`. DWI2CGPIO (zenbook-ledger Z42): the touchpad's GpioInt and the AMDI0030 line (`[dwi2c] acpi gpio=...`) and the pin's IRQ waking the pump (`tpirq`, file tail). DWI2CFAST (zenbook-ledger Z50): the controller's FMCN/SSCN read by the same scan and the held touchpad switched to 400 kHz with a verify read and a 100 kHz fallback (`fast_attach`, file tail), `tests dwi2cacpi`'s third line.

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
    /// ZENBOOKTOUCHPADREPORTS (Z18): wReportDescRegister, wCommandRegister, wDataRegister.
    rdreg: u16,
    cmd: u16,
    data: u16,
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
    /// ZENBOOKTOUCHPADREPORTS (Z18): the address the descriptor answered at.
    hid_addr: u8,
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
    let budget = crate::drivers::block::wait::cycles_ms(XFER_MS + total as u64 / 8); // ZENBOOKTOUCHPADREPORTS (Z18): a report descriptor of ~700 B at 100 kHz is ~65 ms; a 1-byte scan read keeps 20 ms
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
    Some(HidDesc { rdesc: w(4), input: w(8), maxin: w(10), vid: w(20), pid: w(22), ver: w(24), rdreg: w(6), cmd: w(16), data: w(18) })
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
        match acpi_pick(i, kept).or_else(|| HID_GUESS.iter().find(|g| kept.contains(&g.0)).copied()) {
            None => {
                if say {
                    serial_println!("[dwi2c] ctrl={} hid -> NOT ATTEMPTED why=no-known-address", i);
                }
            }
            Some((addr, reg)) => match hid_desc(io, addr, reg, win) {
                Ok(Some(d)) => {
                    o.desc = Some(d);
                    o.hid_addr = addr;
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
    acpi_census(); for (i, &base) in BASES.iter().enumerate() { // DWI2CACPI (zenbook-ledger Z32): `acpi_census` (file tail) — the DSDT's AMDI0010 `_CRS` base and the touchpad's I2cSerialBus address cross-checked against BASES / HID_GUESS, `ACPI_PICK` set only on agree=yes; LINE-NEUTRAL, the call before this comment.
        crate::arch::memory::map_mmio_window(base, 0x1000);
        if crate::arch::memory::translate(base).is_none() {
            serial_println!("[dwi2c] ctrl={} -> SKIP reason=unmapped", i);
            continue;
        }
        let io = Real { base };
        let o = run(i, base, &io, true);
        #[cfg(feature = "ehcihid")]
        {
            if let (true, Some(d)) = (o.found, o.desc) {
                touchpad::attach(i, base, &io, o.hid_addr, d);
            }
        }
        let _ = &o;
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
    /// ZENBOOKTOUCHPADREPORTS (Z18): the report descriptor served at `rdreg`, the input reports a PLAIN read serves in
    /// turn, and the command writes (a STOP after more than 2 written bytes) counted.
    rdesc: [u8; 160],
    rdesc_n: usize,
    rdreg: u16,
    input: [[u8; 32]; 2],
    in_n: usize,
    in_i: usize,
    cmds: u32,
    /// DWI2CFAST (Z50): the fast-mode counts and SDA hold; `nack_fast` NACKs every address while IC_CON's speed field is fast.
    fs_hcnt: u32,
    fs_lcnt: u32,
    hold: u32,
    nack_fast: bool,
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
                rdesc: [0; 160],
                rdesc_n: 0,
                rdreg: 0,
                input: [[0; 32]; 2],
                in_n: 0,
                in_i: 0,
                cmds: 0,
                fs_hcnt: 0,
                fs_lcnt: 0,
                hold: 0,
                nack_fast: false,
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
            IC_FS_SCL_HCNT => st.fs_hcnt,
            IC_FS_SCL_LCNT => st.fs_lcnt,
            IC_SDA_HOLD => st.hold,
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
            IC_FS_SCL_HCNT => st.fs_hcnt = v,
            IC_FS_SCL_LCNT => st.fs_lcnt = v,
            IC_SDA_HOLD => st.hold = v,
            IC_INTR_MASK => st.mask = v,
            IC_DATA_CMD => {
                if st.en_status & 1 == 0 {
                    return;
                }
                if !st.in_txn {
                    if st.tar != st.ack as u32 || (st.nack_fast && st.con & CON_SPEED_MASK == CON_SPEED_FAST) {
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
                    let b = if st.wn == 2 && reg == 0x0001 {
                        st.desc.get(st.rpos).copied().unwrap_or(0xff)
                    } else if st.wn == 2 && st.rdesc_n > 0 && reg == st.rdreg {
                        if st.rpos < st.rdesc_n { st.rdesc[st.rpos] } else { 0xff }
                    } else if st.wn == 0 && st.in_i < st.in_n {
                        st.input[st.in_i].get(st.rpos).copied().unwrap_or(0)
                    } else if st.wn == 0 && st.in_n > 0 {
                        0
                    } else {
                        0xff
                    };
                    st.rpos += 1;
                    Fake::push_rx(&mut st, b);
                } else {
                    if st.wn < 2 {
                        let at = st.wn;
                        st.wbuf[at] = v as u8;
                    }
                    st.wn += 1;
                }
                if v & CMD_STOP != 0 {
                    st.raw |= INTR_STOP_DET;
                    st.in_txn = false;
                    if st.wn > 2 {
                        st.cmds += 1;
                    } else if st.wn == 0 && v & CMD_READ != 0 && st.in_i < st.in_n {
                        st.in_i += 1;
                    }
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

/// ZENBOOKTOUCHPADREPORTS (zenbook-ledger Z18) — the touchpad's I2C-HID reports into the pointer route. File tail.
#[cfg(feature = "ehcihid")]
pub mod touchpad {
    //! CHARTER: Kernel — driver (the I2C-HID touchpad's bring-up and its Precision Touchpad reports into
    //! `pal::push_pointer_report`, ZENBOOKTOUCHPADREPORTS zenbook-ledger Z18; no CODEX §2 handler owns a platform I2C bus).
    //!
    //! HID over I2C 1.0 at the address Z14's census found answering a guess-table descriptor register: SET_POWER(ON) and
    //! RESET over wCommandRegister, the report descriptor read once from wReportDescRegister, then — when the
    //! descriptor declares the Digitizer Input Mode feature (0x0D:0x52) — ONE SET_REPORT(Feature, Input Mode = 3)
    //! through wDataRegister, without which a Precision Touchpad stays in mouse mode and never sends its touch report.
    //! The input register is POLLED: one bounded plain read of
    //! wMaxInputLength bytes per [`POLL_MS`] (sooner when the GPIO interrupt fired: DWI2CGPIO Z42, `super::tpirq`), from the one HID pass (`ehci::service_ehci_hid`), under `EHCI_HID` then
    //! [`TP`] (`try_lock`, never blocking the pass). The parse is a MINIMAL PTP map ([`parse`]): the shared
    //! `ehci::parse_report_descriptor` keeps one report id and the last X it sees, so it cannot hold a Finger
    //! collection; the bit reads and the curve are EHCI's (`ehci::lend`), not copies. The first finger's absolute
    //! position becomes a relative delta by the Wellspring lane's rules (a lift, a contact id or contact count change,
    //! or a 250 ms silence re-baselines; the curve is `finemotion::step2`, the raw step pre-scaled to that lane's span);
    //! the Button-page bit is the click (the pad's physical press, as bcm5974's `ibt`); a touch is never a press.
    //!
    //! DWI2CTPGEST (zenbook-ledger Z26): the first TWO finger collections are read with their Confidence (0x47); a
    //! touching slot with Confidence 0 is a PALM, dropped before counting, and the first CONFIDENT contact is the
    //! pointer, its curve pixels through BP-input-12's gain (`prefs_core::trackpad::gain_step`, `tpgest`'s live speed).
    //! Two confident contacts and no click freeze the pointer and turn the centroid's curve dy into `Event::Wheel`
    //! detents by the rMBP lane's rule ([`scroll2`]: `tpgest::DETENT_PX`, `prefs_core::trackpad::wheel` with the live
    //! `natural`), pushed through `pal::push_event` as `tpgest::shape` does. Vertical only (`pal::Event` has no
    //! horizontal wheel).

    use super::{HidDesc, Mmio};
    use core::sync::atomic::{AtomicBool, Ordering};

    /// The longest report descriptor read.
    const RDESC_MAX: usize = 1024;
    /// The longest input report read.
    const MAXIN_CAP: usize = 64;
    /// One input-register poll per this many milliseconds (~3 ms of bus at 100 kHz for a 32-byte report).
    const POLL_MS: u64 = 8;
    /// After RESET, before the descriptor read (no interrupt to wait on).
    const SETTLE_MS: u64 = 100;
    const OP_RESET: u8 = 0x01;
    const OP_SET_REPORT: u8 = 0x03;
    const OP_SET_POWER: u8 = 0x08;
    const POWER_ON: u8 = 0x00;
    const TYPE_FEATURE: u8 = 0x03;
    /// Full usages (page << 16 | id).
    const U_TOUCH_PAD: u32 = 0x000d_0005;
    const U_FINGER: u32 = 0x000d_0022;
    const U_TIP: u32 = 0x000d_0042;
    const U_CONTACT_ID: u32 = 0x000d_0051;
    const U_INPUT_MODE: u32 = 0x000d_0052;
    const U_CONTACT_COUNT: u32 = 0x000d_0054;
    const U_X: u32 = 0x0001_0030;
    const U_Y: u32 = 0x0001_0031;
    const U_BUTTON1: u32 = 0x0009_0001;
    /// DWI2CTPGEST (Z26): a finger's Confidence (0 = a palm).
    const U_CONFIDENCE: u32 = 0x000d_0047;

    /// One report field: bit offset in the report body (after the id byte), size in bits (0 = absent), logical max.
    #[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
    struct Field {
        off: u16,
        size: u8,
        max: u32,
    }

    /// The minimal Precision Touchpad map.
    #[derive(Clone, Copy, Default, Debug)]
    struct Ptp {
        /// The touch report's id (0 = the descriptor declares none).
        rid: u8,
        tip: Field,
        cid: Field,
        x: Field,
        y: Field,
        count: Field,
        button: Field,
        /// Finger collections inside the Touch Pad application collection.
        contacts: u8,
        has_mode: bool,
        mode_rid: u8,
        mode: Field,
        /// The Input Mode feature report's body length in bytes.
        mode_len: u16,
        /// A Touch Pad collection with the first finger's X, Y and Tip found.
        ptp: bool,
        /// DWI2CTPGEST (Z26): the second finger's fields and both fingers' Confidence.
        more: More,
    }

    /// DWI2CTPGEST (Z26): the second Finger collection's fields and each finger's Confidence (size 0 = absent).
    #[derive(Clone, Copy, Default, Debug)]
    struct More {
        conf: [Field; 2],
        tip2: Field,
        cid2: Field,
        x2: Field,
        y2: Field,
    }

    /// The running bit offset of one report id (input or feature).
    fn slot(t: &mut [(u8, u32); 8], n: &mut usize, rid: u8) -> usize {
        if let Some(k) = t[..*n].iter().position(|e| e.0 == rid) {
            return k;
        }
        if *n < t.len() {
            t[*n] = (rid, 0);
            *n += 1;
            return *n - 1;
        }
        t.len() - 1
    }

    /// One walk of the report descriptor's short items (HID 1.11 §6.2.2) with a collection stack.
    fn parse(d: &[u8]) -> Ptp {
        let mut p = Ptp::default();
        let (mut page, mut rsize, mut rcount, mut lmax, mut rid) = (0u32, 0u32, 0u32, 0u32, 0u8);
        let mut usages = [0u32; 16];
        let mut nu = 0usize;
        let (mut umin, mut umax) = (0u32, 0u32);
        let mut depth = 0usize;
        let mut tp_depth = 0usize;
        let mut finger = 0u8;
        let mut finger_depth = 0usize;
        let mut seen_tp = false;
        let mut ins = [(0u8, 0u32); 8];
        let mut nin = 0usize;
        let mut fts = [(0u8, 0u32); 8];
        let mut nft = 0usize;
        let mut i = 0usize;
        while i < d.len() {
            let b = d[i];
            if b == 0xfe {
                break;
            }
            let size = match b & 3 {
                3 => 4,
                s => s as usize,
            };
            if i + 1 + size > d.len() {
                break;
            }
            let mut data = 0u32;
            for k in 0..size {
                data |= (d[i + 1 + k] as u32) << (8 * k);
            }
            let full = if size == 4 { data } else { (page << 16) | (data & 0xffff) };
            match b & 0xfc {
                0x04 => page = data & 0xffff,
                0x24 => lmax = data,
                0x74 => rsize = data,
                0x94 => rcount = data,
                0x84 => rid = data as u8,
                0x08 => {
                    if nu < usages.len() {
                        usages[nu] = full;
                        nu += 1;
                    }
                }
                0x18 => umin = full,
                0x28 => umax = full,
                0xa0 => {
                    let u = if nu > 0 { usages[0] } else { 0 };
                    depth += 1;
                    if u == U_TOUCH_PAD && tp_depth == 0 {
                        tp_depth = depth;
                        seen_tp = true;
                    }
                    if u == U_FINGER && tp_depth != 0 {
                        p.contacts = p.contacts.saturating_add(1);
                        finger = p.contacts;
                        finger_depth = depth;
                    }
                    nu = 0;
                    umin = 0;
                    umax = 0;
                }
                0xc0 => {
                    if depth > 0 {
                        if depth == finger_depth {
                            finger = 0;
                            finger_depth = 0;
                        }
                        if depth == tp_depth {
                            tp_depth = 0;
                        }
                        depth -= 1;
                    }
                }
                0x80 | 0xb0 => {
                    let input = b & 0xfc == 0x80;
                    let cnt = rcount.min(64);
                    let at = if input { slot(&mut ins, &mut nin, rid) } else { slot(&mut fts, &mut nft, rid) };
                    let base = if input { ins[at].1 } else { fts[at].1 };
                    if data & 1 == 0 {
                        for j in 0..cnt {
                            let u = if (j as usize) < nu {
                                usages[j as usize]
                            } else if nu > 0 {
                                usages[nu - 1]
                            } else if umax >= umin && umin != 0 {
                                umin.wrapping_add(j)
                            } else {
                                0
                            };
                            let f = Field { off: base.saturating_add(j.saturating_mul(rsize)).min(u16::MAX as u32) as u16, size: rsize.min(32) as u8, max: lmax };
                            if input && tp_depth != 0 {
                                match u {
                                    U_TIP if finger == 1 => {
                                        p.tip = f;
                                        p.rid = rid;
                                    }
                                    U_CONTACT_ID if finger == 1 => p.cid = f,
                                    U_X if finger == 1 => p.x = f,
                                    U_Y if finger == 1 => p.y = f,
                                    U_CONFIDENCE if finger == 1 => p.more.conf[0] = f,
                                    U_CONFIDENCE if finger == 2 => p.more.conf[1] = f,
                                    U_TIP if finger == 2 => p.more.tip2 = f,
                                    U_CONTACT_ID if finger == 2 => p.more.cid2 = f,
                                    U_X if finger == 2 => p.more.x2 = f,
                                    U_Y if finger == 2 => p.more.y2 = f,
                                    U_CONTACT_COUNT if p.count.size == 0 => p.count = f,
                                    U_BUTTON1 if p.button.size == 0 => p.button = f,
                                    _ => {}
                                }
                            } else if !input && u == U_INPUT_MODE && !p.has_mode {
                                p.has_mode = true;
                                p.mode_rid = rid;
                                p.mode = f;
                            }
                        }
                    }
                    let next = base.saturating_add(rsize.min(256) * cnt);
                    if input {
                        ins[at].1 = next;
                    } else {
                        fts[at].1 = next;
                    }
                    nu = 0;
                    umin = 0;
                    umax = 0;
                }
                0x90 => {
                    nu = 0;
                    umin = 0;
                    umax = 0;
                }
                _ => {}
            }
            i += 1 + size;
        }
        if p.has_mode {
            if let Some(e) = fts[..nft].iter().find(|e| e.0 == p.mode_rid) {
                p.mode_len = e.1.div_ceil(8).min(u16::MAX as u32) as u16;
            }
        }
        p.ptp = seen_tp && p.x.size > 0 && p.y.size > 0 && p.tip.size > 0;
        p
    }

    /// The first CONFIDENT contact of one input packet (`[len lo, len hi, id, body]`) and the second one's position;
    /// `None` = nothing pending or another report. DWI2CTPGEST (Z26): a touching slot with Confidence 0 is a palm,
    /// counted in `palms` and never in `n`; `count` stays the report's Contact Count.
    #[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
    struct Contact {
        x: i32,
        y: i32,
        tip: bool,
        cid: u32,
        count: u32,
        buttons: u8,
        /// Confident contacts among the first two slots.
        n: u8,
        x2: i32,
        y2: i32,
        palms: u8,
    }

    fn contact(p: &Ptp, pkt: &[u8]) -> Option<Contact> {
        if pkt.len() < 3 {
            return None;
        }
        let len = u16::from_le_bytes([pkt[0], pkt[1]]) as usize;
        if len <= 2 {
            return None;
        }
        let rep = &pkt[2..len.min(pkt.len())];
        let body = if p.rid != 0 {
            if rep.first().copied() != Some(p.rid) {
                return None;
            }
            &rep[1..]
        } else {
            rep
        };
        let rd = |f: Field| if f.size == 0 { 0 } else { crate::drivers::ehci::lend::bits(body, f.off, f.size) };
        let conf = |f: Field| f.size == 0 || rd(f) != 0;
        let m = p.more;
        let s1 = (rd(p.tip) != 0, conf(m.conf[0]), rd(p.x) as i32, rd(p.y) as i32, rd(p.cid));
        let s2 = (m.tip2.size != 0 && rd(m.tip2) != 0, conf(m.conf[1]), rd(m.x2) as i32, rd(m.y2) as i32, rd(m.cid2));
        let mut c = Contact { count: rd(p.count), buttons: (rd(p.button) & 1) as u8, ..Contact::default() };
        for (tip, sure, x, y, cid) in [s1, s2] {
            if !tip {
                continue;
            }
            if !sure {
                c.palms += 1;
                continue;
            }
            if c.n == 0 {
                c.x = x;
                c.y = y;
                c.tip = true;
                c.cid = cid;
            } else {
                c.x2 = x;
                c.y2 = y;
            }
            c.n += 1;
        }
        Some(c)
    }

    /// The absolute-to-relative state (the Wellspring lane's `mt_step` rules).
    #[derive(Default)]
    struct Track {
        prev: Option<(i32, i32)>,
        cid: u32,
        count: u32,
        ms: u64,
        res: (i32, i32),
        buttons: u8,
        /// DWI2CTPGEST (Z26): BP-input-12's gain remainder, and the two-finger scroll burst.
        gres: (i32, i32),
        sc: Scroll2,
    }

    /// What one packet comes to: the contact, the whole-pixel motion, a button edge, a wheel detent (DWI2CTPGEST).
    struct Out {
        c: Option<Contact>,
        motion: Option<(i32, i32)>,
        edge: bool,
        buttons: u8,
        wheel: Option<i8>,
    }

    impl Out {
        /// The pal events this packet pushes.
        fn events(&self) -> u32 {
            self.motion.is_some() as u32 + self.edge as u32 + self.wheel.is_some() as u32
        }
    }

    /// One packet, PURE but for `t`: a lift, a contact id or count change, or a silence of `lend::JUMP_GAP_MS`
    /// re-baselines; else the first finger's step through the lent curve. HID y is already screen-down.
    fn step(p: &Ptp, t: &mut Track, pkt: &[u8], now_ms: u64) -> Out {
        let mut o = Out { c: None, motion: None, edge: false, buttons: t.buttons, wheel: None };
        let Some(c) = contact(p, pkt) else { return o };
        o.c = Some(c);
        o.wheel = scroll2(p, t, &c, now_ms);
        if !c.tip || t.sc.on {
            t.prev = None;
            t.res = (0, 0);
            t.gres = (0, 0);
        } else {
            let gap = now_ms.wrapping_sub(t.ms);
            let rebase = c.cid != t.cid || c.count != t.count || gap >= crate::drivers::ehci::lend::JUMP_GAP_MS;
            match t.prev {
                Some((px, py)) if !rebase => {
                    let span = (p.x.max.min(i32::MAX as u32) as i32).max(1);
                    let (dx, dy) = crate::drivers::ehci::lend::curve((c.x - px, c.y - py), span, &mut t.res);
                    let speed = crate::drivers::ehci::tpgest::get().0;
                    let (dx, dy) = (prefs_core::trackpad::gain_step(dx, speed, &mut t.gres.0), prefs_core::trackpad::gain_step(dy, speed, &mut t.gres.1));
                    if dx != 0 || dy != 0 {
                        o.motion = Some((dx, dy));
                    }
                }
                _ => {
                    t.res = (0, 0);
                    t.gres = (0, 0);
                }
            }
            t.prev = Some((c.x, c.y));
            t.cid = c.cid;
            t.count = c.count;
            t.ms = now_ms;
        }
        if c.buttons != t.buttons {
            o.edge = true;
            t.buttons = c.buttons;
        }
        o.buttons = t.buttons;
        o
    }

    /// One command over wCommandRegister: `[reg lo, reg hi, b0, opcode]`.
    fn command<I: Mmio>(io: &I, addr: u8, d: &HidDesc, b0: u8, op: u8, win: usize) -> Result<(), super::Fail> {
        let r = d.cmd.to_le_bytes();
        super::xfer(io, addr, &[r[0], r[1], b0, op], &mut [0u8; 0], win)
    }

    /// SET_REPORT(Feature, Input Mode = 3): the command, then wDataRegister, the length (counting itself), the id,
    /// the feature body with the mode field set and every other byte 0.
    fn set_mode<I: Mmio>(io: &I, addr: u8, d: &HidDesc, p: &Ptp, win: usize) -> Result<(), super::Fail> {
        let n = (p.mode_len as usize).clamp(1, 8);
        let mut body = [0u8; 8];
        let at = p.mode.off as usize;
        if at / 8 < n {
            body[at / 8] |= 3u8 << (at % 8);
        }
        let (c, dr) = (d.cmd.to_le_bytes(), d.data.to_le_bytes());
        let mut w = [0u8; 24];
        let mut k = 0usize;
        let mut put = |b: u8| {
            w[k] = b;
            k += 1;
        };
        put(c[0]);
        put(c[1]);
        if p.mode_rid < 15 {
            put((TYPE_FEATURE << 4) | p.mode_rid);
            put(OP_SET_REPORT);
        } else {
            put((TYPE_FEATURE << 4) | 0x0f);
            put(OP_SET_REPORT);
            put(p.mode_rid);
        }
        put(dr[0]);
        put(dr[1]);
        let len = (2 + (p.mode_rid != 0) as usize + n) as u16;
        put(len as u8);
        put((len >> 8) as u8);
        if p.mode_rid != 0 {
            put(p.mode_rid);
        }
        for &b in &body[..n] {
            put(b);
        }
        super::xfer(io, addr, &w[..k], &mut [0u8; 0], win)
    }

    /// What the bring-up came to (the wire line's fields); `why` = None means armed.
    #[derive(Default)]
    struct Bring {
        power: bool,
        reset_len: u16,
        rdesc: u16,
        mode: &'static str,
        p: Ptp,
        why: Option<&'static str>,
    }

    /// SET_POWER(ON), RESET, a settle, one drain read, the report descriptor, the PTP map, the Input Mode feature.
    /// Generic over the register seam: the KAT drives this same fn.
    fn bring_up<I: Mmio>(io: &I, addr: u8, d: &HidDesc, win: usize, settle_ms: u64, rbuf: &mut [u8; RDESC_MAX]) -> Bring {
        let mut b = Bring { mode: "none", reset_len: 0xffff, rdesc: d.rdesc, ..Bring::default() };
        if d.cmd == 0 || d.rdreg == 0 {
            b.why = Some("no-registers");
            return b;
        }
        if (d.maxin as usize) > MAXIN_CAP {
            b.why = Some("maxin");
            return b;
        }
        b.power = command(io, addr, d, POWER_ON, OP_SET_POWER, win).is_ok();
        if !b.power {
            b.why = Some("power");
            return b;
        }
        if command(io, addr, d, 0x00, OP_RESET, win).is_err() {
            b.why = Some("reset");
            return b;
        }
        if settle_ms > 0 {
            let _ = crate::drivers::block::wait::bounded(settle_ms, || false);
        }
        let n = (d.maxin as usize).clamp(2, MAXIN_CAP);
        let mut drain = [0u8; MAXIN_CAP];
        if super::xfer(io, addr, &[], &mut drain[..n], win).is_ok() {
            b.reset_len = u16::from_le_bytes([drain[0], drain[1]]);
        }
        let len = d.rdesc as usize;
        if len == 0 || len > RDESC_MAX {
            b.why = Some("rdesc-len");
            return b;
        }
        if super::xfer(io, addr, &d.rdreg.to_le_bytes(), &mut rbuf[..len], win).is_err() {
            b.why = Some("rdesc-read");
            return b;
        }
        b.p = parse(&rbuf[..len]);
        if !b.p.ptp {
            b.why = Some("not-ptp");
            return b;
        }
        if b.p.has_mode {
            if set_mode(io, addr, d, &b.p, win).is_ok() {
                b.mode = "set";
            } else {
                b.mode = "fail";
                b.why = Some("mode");
            }
        }
        b
    }

    /// The held touchpad.
    struct Tp {
        base: u64,
        addr: u8,
        d: HidDesc,
        win: usize,
        p: Ptp,
        t: Track,
        last_ms: u64,
        first: bool,
        errs: u32,
        reports: u32,
    }

    /// Written once by [`attach`] at pci init; read by [`pump`] with `try_lock` under `EHCI_HID` (order EHCI_HID -> TP).
    static TP: crate::sync::Mutex<Option<Tp>> = crate::sync::Mutex::new(None);
    static ARMED: AtomicBool = AtomicBool::new(false);

    /// `probe`'s follow-on for a controller whose census found a descriptor: re-open it (Z14's save + configure),
    /// bring the touchpad up; armed = the controller is HELD open for [`pump`], else restored as Z14 left it.
    pub(super) fn attach(i: usize, base: u64, io: &super::Real, addr: u8, d: HidDesc) {
        if ARMED.load(Ordering::Acquire) {
            return;
        }
        let win = super::window(super::identify(io).param1);
        let (saved, _) = match super::open(io) {
            Ok(v) => v,
            Err(f) => {
                serial_println!("[dwi2c] ctrl={} hid addr={:#04x} -> NOT ARMED why=open step={} src={:#x}", i, addr, f.step, f.src);
                return;
            }
        };
        let mut rbuf = [0u8; RDESC_MAX];
        let b = bring_up(io, addr, &d, win, SETTLE_MS, &mut rbuf);
        let p = b.p;
        serial_println!(
            "[dwi2c] hid rdesc={} ptp={} contacts={} ctrl={} addr={:#04x} rid={} xmax={} ymax={} power={} reset={} mode={} -> {}{}",
            b.rdesc, if p.ptp { "yes" } else { "no" }, p.contacts, i, addr, p.rid, p.x.max, p.y.max,
            if b.power { "ok" } else { "fail" }, b.reset_len, b.mode,
            if b.why.is_none() { "armed" } else { "NOT ARMED why=" }, b.why.unwrap_or("")
        );
        match b.why {
            None => {
                super::fast_attach(i, io, addr, &d, win, &rbuf[..(d.rdesc as usize).clamp(1, 4)]);
                *TP.lock() = Some(Tp { base, addr, d, win, p, t: Track::default(), last_ms: 0, first: false, errs: 0, reports: 0 });
                ARMED.store(true, Ordering::Release);
                serial_println!("[dwi2c] ctrl={} held for touchpad", i);
                super::tpirq::arm();
            }
            Some(_) => {
                let back = super::close(io, &saved);
                if back {
                    serial_println!("[dwi2c] ctrl={} restored en={}", i, saved.en & 1);
                } else {
                    serial_println!("[dwi2c] ctrl={} restore -> stuck step=disable", i);
                }
            }
        }
    }

    /// The HID pass's touchpad step (`ehci::service_ehci_hid`, under `EHCI_HID`): at most one bounded input-register
    /// read per [`POLL_MS`], each decoded packet into ONE `pal::push_pointer_report`.
    pub fn pump() {
        if !ARMED.load(Ordering::Acquire) {
            return;
        }
        let Some(mut g) = TP.try_lock() else { return };
        let Some(tp) = g.as_mut() else { return };
        let now = crate::arch::ms();
        let irq = super::tpirq::take();
        if !irq && now.wrapping_sub(tp.last_ms) < POLL_MS {
            return;
        }
        tp.last_ms = now;
        let io = super::Real { base: tp.base };
        let n = (tp.d.maxin as usize).clamp(2, MAXIN_CAP);
        let mut buf = [0u8; MAXIN_CAP];
        let r = super::xfer(&io, tp.addr, &[], &mut buf[..n], tp.win);
        if irq {
            super::tpirq::rearm();
        }
        if r.is_err() {
            tp.errs = tp.errs.saturating_add(1);
            return;
        }
        let o = step(&tp.p, &mut tp.t, &buf[..n], now);
        let Some(c) = o.c else { return };
        tp.reports = tp.reports.saturating_add(1);
        super::tpirq::note(irq, tp.reports);
        if !tp.first {
            tp.first = true;
            serial_println!(
                "[dwi2c] touchpad first-report x={} y={} contacts={} tip={} buttons={:#04x}",
                c.x, c.y, c.count, c.tip as u8, c.buttons
            );
        }
        crate::pal::push_pointer_report(
            o.motion.map(|(x, y)| crate::pal::Event::Mouse { x, y }),
            if o.edge { Some(crate::pal::Event::Button(o.buttons)) } else { None },
        );
        if let Some(w) = o.wheel {
            crate::pal::push_event(crate::pal::Event::Wheel(w));
            if !tp.t.sc.said {
                tp.t.sc.said = true;
                if SCROLL_WIT.load(Ordering::Relaxed) < SCROLL_WIT_MAX {
                    SCROLL_WIT.fetch_add(1, Ordering::Relaxed);
                    serial_println!(
                        "[dwi2c] touchpad gest=scroll2 fingers=2 wheel={} natural={} palms={}",
                        w, crate::drivers::ehci::tpgest::get().2 as u8, c.palms
                    );
                }
            }
        }
    }

    /// The KAT's Precision Touchpad descriptor: Touch Pad (rid 4) with two Finger collections (Confidence, Tip,
    /// 6 pad bits, Contact ID 8, X 16 max 4095, Y 16 max 2047), Contact Count 8, Button 1, 7 pad bits, the Contact
    /// Count Maximum feature (rid 5); Device Configuration with the Input Mode feature (rid 3).
    const KAT_RDESC: &[u8] = &[
        0x05, 0x0d, 0x09, 0x05, 0xa1, 0x01, 0x85, 0x04, //
        0x09, 0x22, 0xa1, 0x02, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x01, 0x09, 0x47, 0x81, 0x02, 0x09, 0x42,
        0x81, 0x02, 0x95, 0x06, 0x81, 0x03, 0x75, 0x08, 0x95, 0x01, 0x25, 0x0f, 0x09, 0x51, 0x81, 0x02, 0x05, 0x01,
        0x26, 0xff, 0x0f, 0x75, 0x10, 0x95, 0x01, 0x09, 0x30, 0x81, 0x02, 0x26, 0xff, 0x07, 0x09, 0x31, 0x81, 0x02,
        0xc0, //
        0x05, 0x0d, 0x09, 0x22, 0xa1, 0x02, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x01, 0x09, 0x47, 0x81, 0x02,
        0x09, 0x42, 0x81, 0x02, 0x95, 0x06, 0x81, 0x03, 0x75, 0x08, 0x95, 0x01, 0x25, 0x0f, 0x09, 0x51, 0x81, 0x02,
        0x05, 0x01, 0x26, 0xff, 0x0f, 0x75, 0x10, 0x95, 0x01, 0x09, 0x30, 0x81, 0x02, 0x26, 0xff, 0x07, 0x09, 0x31,
        0x81, 0x02, 0xc0, //
        0x05, 0x0d, 0x15, 0x00, 0x25, 0x05, 0x75, 0x08, 0x95, 0x01, 0x09, 0x54, 0x81, 0x02, //
        0x05, 0x09, 0x09, 0x01, 0x25, 0x01, 0x75, 0x01, 0x95, 0x01, 0x81, 0x02, 0x95, 0x07, 0x81, 0x03, //
        0x05, 0x0d, 0x85, 0x05, 0x09, 0x55, 0x25, 0x05, 0x75, 0x08, 0x95, 0x01, 0xb1, 0x02, 0xc0, //
        0x05, 0x0d, 0x09, 0x0e, 0xa1, 0x01, 0x85, 0x03, 0x09, 0x22, 0xa1, 0x02, 0x09, 0x52, 0x15, 0x00, 0x25, 0x0a,
        0x75, 0x08, 0x95, 0x01, 0xb1, 0x02, 0xc0, 0xc0,
    ];

    /// One canned touch report for [`KAT_RDESC`]: `[17, 0, 0x04, body x14]`.
    fn kat_report(x: u16, y: u16, tip: bool, button: bool) -> [u8; 32] {
        let mut r = [0u8; 32];
        r[0] = 17;
        r[2] = 0x04;
        r[3] = 0x01 | if tip { 0x02 } else { 0x00 };
        r[4] = 1;
        r[5..7].copy_from_slice(&x.to_le_bytes());
        r[7..9].copy_from_slice(&y.to_le_bytes());
        r[15] = 1;
        r[16] = button as u8;
        r
    }

    /// `tests dwi2ctp`: the bring-up and two canned reports over the fake controller; the pal events are COUNTED from
    /// `step`, never pushed into the live queue.
    pub fn selftest() {
        let f = super::Fake::new();
        {
            let mut st = f.s.borrow_mut();
            let n = KAT_RDESC.len();
            st.desc[4..6].copy_from_slice(&(n as u16).to_le_bytes());
            st.desc[6..8].copy_from_slice(&0x0002u16.to_le_bytes());
            st.desc[16..18].copy_from_slice(&0x0005u16.to_le_bytes());
            st.desc[18..20].copy_from_slice(&0x0006u16.to_le_bytes());
            st.rdesc[..n].copy_from_slice(KAT_RDESC);
            st.rdesc_n = n;
            st.rdreg = 0x0002;
        }
        let raw = f.s.borrow().desc;
        let (mut rdesc_ok, mut reports, mut events, mut first_ok) = (false, 0u32, 0u32, false);
        let (mut scroll_ok, mut sdy, mut palm_ok) = (false, 0i32, false);
        if let (Some(d), Ok((saved, _))) = (super::parse_desc(&raw), super::open(&f)) {
            let win = super::window(super::identify(&f).param1);
            let mut rbuf = [0u8; RDESC_MAX];
            let b = bring_up(&f, 0x15, &d, win, 0, &mut rbuf);
            let p = b.p;
            let cmds = f.s.borrow().cmds;
            rdesc_ok = b.why.is_none() && p.ptp && p.rid == 4 && p.contacts == 2 && p.x.max == 4095 && p.y.max == 2047 && p.has_mode && p.mode_rid == 3 && b.mode == "set" && cmds == 3;
            rdesc_ok = rdesc_ok && p.more.x2.size == 16 && p.more.conf[0].size == 1 && p.more.conf[1].size == 1;
            {
                let mut st = f.s.borrow_mut();
                st.input[0] = kat_report(1000, 800, true, false);
                st.input[1] = kat_report(1100, 850, true, true);
                st.in_n = 2;
                st.in_i = 0;
            }
            let mut t = Track::default();
            for k in 0..2u64 {
                let mut buf = [0u8; 32];
                if super::xfer(&f, 0x15, &[], &mut buf, win).is_err() {
                    break;
                }
                let o = step(&p, &mut t, &buf, k * POLL_MS);
                if let Some(c) = o.c {
                    reports += 1;
                    if k == 0 {
                        first_ok = c.x == 1000 && c.y == 800 && c.tip && c.count == 1 && c.buttons == 0 && o.events() == 0;
                    }
                }
                events += o.events();
            }
            (scroll_ok, sdy, palm_ok) = kat_gest(&p);
            let _ = super::close(&f, &saved);
        }
        let pass = rdesc_ok && first_ok && reports == 2 && events == 2 && scroll_ok && palm_ok;
        serial_println!(
            ":: DWI2C-TP: rdesc={} reports={} events={} scroll2 dy={} palm={} -> {} ::",
            if rdesc_ok { "ok" } else { "fail" }, reports, events, sdy,
            if palm_ok { "ok" } else { "fail" }, if pass { "PASS" } else { "FAIL" }
        );
    }

    // DWI2CTPGEST (zenbook-ledger Z26) — two-finger scroll and the palm rule. Module tail.

    /// One two-finger scroll burst: on while two confident contacts are down and nothing is clicked.
    #[derive(Clone, Copy, Default)]
    struct Scroll2 {
        on: bool,
        /// The centroid at the last sample (sensor units).
        last: (i32, i32),
        /// The curve's Q8 remainder.
        res: (i32, i32),
        /// Curve pixels of dy not yet a detent.
        acc: i32,
        /// Detents this burst (signed, after `natural`).
        sum: i32,
        ms: u64,
        /// The burst's witness line spent.
        said: bool,
    }

    /// `[dwi2c] touchpad gest=scroll2` lines per boot.
    const SCROLL_WIT_MAX: u8 = 4;
    static SCROLL_WIT: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

    /// Two confident contacts and no click: the centroid's step through the lent curve, its dy as `Event::Wheel`
    /// detents by the rMBP lane's rule. The accumulate-and-detent below MIRRORS `tpgest::shape`'s two-finger arm
    /// (three lines over the same `DETENT_PX` and `prefs_core::trackpad::wheel`): that arm is inline over a `Wsp2Frame`
    /// and its private `ST`, not a fn over contacts (named in Z26). A burst's first sample, or one after
    /// `lend::JUMP_GAP_MS` of silence, baselines the centroid and makes nothing. dx is not used (no horizontal wheel).
    fn scroll2(p: &Ptp, t: &mut Track, c: &Contact, now_ms: u64) -> Option<i8> {
        if c.n < 2 || c.buttons != 0 {
            t.sc.on = false;
            return None;
        }
        let mid = ((c.x + c.x2) / 2, (c.y + c.y2) / 2);
        if !t.sc.on {
            t.sc = Scroll2 { on: true, last: mid, ms: now_ms, ..Scroll2::default() };
            return None;
        }
        let gap = now_ms.wrapping_sub(t.sc.ms);
        t.sc.ms = now_ms;
        if gap >= crate::drivers::ehci::lend::JUMP_GAP_MS {
            t.sc.last = mid;
            t.sc.res = (0, 0);
            return None;
        }
        let span = (p.x.max.min(i32::MAX as u32) as i32).max(1);
        let (_, dy) = crate::drivers::ehci::lend::curve((mid.0 - t.sc.last.0, mid.1 - t.sc.last.1), span, &mut t.sc.res);
        t.sc.last = mid;
        t.sc.acc += dy;
        let det = t.sc.acc / crate::drivers::ehci::tpgest::DETENT_PX;
        if det == 0 {
            return None;
        }
        t.sc.acc -= det * crate::drivers::ehci::tpgest::DETENT_PX;
        let w = prefs_core::trackpad::wheel(det, crate::drivers::ehci::tpgest::get().2).clamp(-127, 127);
        t.sc.sum += w;
        Some(w as i8)
    }

    /// One canned two-finger touch report for [`KAT_RDESC`]: each finger `(x, y, confidence, tip)`, contact ids 1 and 2.
    fn kat_report2(f1: (u16, u16, bool, bool), f2: (u16, u16, bool, bool), count: u8, button: bool) -> [u8; 32] {
        let mut r = [0u8; 32];
        r[0] = 17;
        r[2] = 0x04;
        for (k, f) in [f1, f2].iter().enumerate() {
            let at = 3 + 6 * k;
            r[at] = (f.2 as u8) | ((f.3 as u8) << 1);
            r[at + 1] = k as u8 + 1;
            r[at + 2..at + 4].copy_from_slice(&f.0.to_le_bytes());
            r[at + 4..at + 6].copy_from_slice(&f.1.to_le_bytes());
        }
        r[15] = count;
        r[16] = button as u8;
        r
    }

    /// `tests dwi2ctp`'s gesture legs, driving [`step`] on canned packets (nothing is pushed): a two-finger stroke
    /// down makes wheel detents and no pointer motion; a palm in slot 1 is dropped and slot 2's finger is the pointer.
    /// Returns `(scroll_ok, the burst's detents, palm_ok)`.
    fn kat_gest(p: &Ptp) -> (bool, i32, bool) {
        let want = prefs_core::trackpad::wheel(1, crate::drivers::ehci::tpgest::get().2).signum();
        let mut t = Track::default();
        let (mut moved, mut sum) = (false, 0i32);
        for k in 0..5u64 {
            let y = 800 + 200 * k as u16;
            let o = step(p, &mut t, &kat_report2((1000, y, true, true), (1400, y, true, true), 2, false), k * POLL_MS);
            moved |= o.motion.is_some();
            sum += o.wheel.map_or(0, |w| w as i32);
        }
        let scroll_ok = !moved && sum != 0 && sum.signum() == want && t.sc.sum == sum;
        let mut t = Track::default();
        let a = step(p, &mut t, &kat_report2((500, 500, false, true), (2000, 1000, true, true), 2, false), 0);
        let b = step(p, &mut t, &kat_report2((200, 500, false, true), (2300, 1000, true, true), 2, false), POLL_MS);
        let first = a.c.is_some_and(|c| c.n == 1 && c.palms == 1 && c.x == 2000 && c.tip);
        let palm_ok = first && a.wheel.is_none() && b.wheel.is_none() && b.motion.is_some_and(|(dx, _)| dx > 0);
        (scroll_ok, sum, palm_ok)
    }
}

// ── DWI2CACPI (zenbook-ledger Z32) — the DSDT's word on the controller base and the touchpad's address — FILE TAIL ────
// A BYTE SCAN, not an interpreter (BP-power-4: no AML runs): `aml_scan` finds the `_HID "AMDI0010"` Devices and the ONE
// `Memory32Fixed` (0x86) inside each Device's span (`5B 82` + PkgLength), the ONE PNP0C50 Device (the I2C-HID
// compatible id, String or EISAID) and the ONE `I2cSerialBus` (0x8E, type 1) inside its span, whose ResourceSource names
// the controller; the descriptor register only from the literal `_DSM` shape `If (Arg2 == One) Return (<const>)` after
// the HID GUID (linux i2c-hid-acpi), else `?`. Anything else refuses BY NAME. The result is a CROSS-CHECK of the survey
// (`BASES`, `HID_GUESS`): on agree=yes `ACPI_PICK` pins which guess pair `run` tries on which controller; on agree=no the
// survey path runs unchanged and the line records the mismatch (the first boot judges). `AMDI0019` (linux: the
// PSP-arbitrated bus, ARBITRATION_SEMAPHORE) is deliberately NOT accepted. `tests dwi2cacpi`: the live line, then the KAT
// over `ACPI_KAT` and three refusal mutants: `:: DWI2CACPI: base=ok tp=ok speed=400000 desc=ok refuse=3/3 -> PASS ::`.
// Owed: DWI2C-AOAC, DWI2C-ACPI-SSDT (the touchpad declared in an SSDT); DWI2C-GPIO BUILT by DWI2CGPIO (Z42, below); DWI2C-FAST BUILT by DWI2CFAST (Z50, file tail).

/// `Name (_HID, "AMDI0010")`: the DesignWare I2C master's ACPI id on AMD.
const ACPI_HID_PAT: &[u8] = b"\x08_HID\x0dAMDI0010\x00";
/// The I2C-HID compatible id after a `_HID`/`_CID` NameSeg: "PNP0C50", "ACPI0C50", EISAID(PNP0C50).
const ACPI_TP_IDS: [&[u8]; 3] = [b"\x0dPNP0C50\x00", b"\x0dACPI0C50\x00", b"\x0c\x41\xd0\x0c\x50"];
/// The HID-over-I2C `_DSM` GUID 3cdff6f7-4267-4555-ad05-b30a3d8938de in ToUUID byte order.
const HID_DSM_GUID: [u8; 16] = [0xf7, 0xf6, 0xdf, 0x3c, 0x67, 0x42, 0x55, 0x45, 0xad, 0x05, 0xb3, 0x0a, 0x3d, 0x89, 0x38, 0xde];
/// AMDI0010 Devices kept.
const ACPI_CTRL_MAX: usize = 8;

/// What the scan found.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct AcpiI2c {
    dev: [u8; 4],
    base: u32,
    tp: u16,
    tpdev: [u8; 4],
    speed: u32,
    desc: Option<u16>,
    /// DWI2CGPIO (Z42): the touchpad Device's ONE GpioInt (0x8C, connection type interrupt), else the refusal's name.
    gpio: Result<GpioInt, &'static str>,
    /// DWI2CFAST (Z50): the controller Device's FMCN and SSCN (`dw_cn` over its span).
    cn: Cn,
}

/// A refusal, by name; `what`/`n`/`at` qualify `ambiguous`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct AcpiNo {
    why: &'static str,
    what: &'static str,
    n: usize,
    at: [[u8; 4]; 2],
}

/// The pick `run` honours: (controller index into `BASES`, touchpad address, descriptor register). Written once by
/// `probe` (pci init, BSP) on agree=yes only.
static ACPI_PICK: spin::Once<(usize, u8, u16)> = spin::Once::new();

// AML-HELPERS-ONE (zenbook-ledger Z53): `find_from`/`pkg_len`/`last_seg`/`device_of` (and `DEV_BACK`) MOVED to acpi_power.rs's tail, the ONE home of the AML byte-scan helpers; re-exported here so this file's scans call them by the same names.
pub(crate) use crate::arch::acpi_power::{find_from, pkg_len, last_seg, device_of};

/// The descriptor register from the touchpad Device's `_DSM`, only in the literal shape
/// `If (Arg2 == One) Return (One | ByteConst | WordConst)` after the HID GUID; `None` otherwise.
fn dsm_desc(a: &[u8], s: usize, e: usize) -> Option<u16> {
    let mut k = s;
    while let Some(m) = find_from(a, &[0x14], k, e) {
        k = m + 1;
        let Some((len, w)) = pkg_len(a, m + 1) else { continue };
        if a.get(m + 1 + w..m + 5 + w) != Some(b"_DSM".as_slice()) {
            continue;
        }
        let me = (m + 1 + len).min(e);
        let g = find_from(a, &HID_DSM_GUID, m, me)?;
        let mut i = g + 16;
        while i + 4 < me {
            if a[i] == 0xa0 {
                if let Some((_, w)) = pkg_len(a, i + 1) {
                    let q = i + 1 + w;
                    if q + 4 < me && a[q..q + 4] == [0x93, 0x6a, 0x01, 0xa4] {
                        return match a[q + 4] {
                            0x01 => Some(1),
                            0x0a if q + 5 < me => Some(a[q + 5] as u16),
                            0x0b if q + 6 < me => Some(u16::from_le_bytes([a[q + 5], a[q + 6]])),
                            _ => None,
                        };
                    }
                }
            }
            i += 1;
        }
        return None;
    }
    None
}

/// The scan, bounded to `a` (the DSDT). Pure: the KAT runs it over `ACPI_KAT`.
fn aml_scan(a: &[u8]) -> Result<AcpiI2c, AcpiNo> {
    let no = |why: &'static str| AcpiNo { why, what: "-", n: 0, at: [*b"----"; 2] };
    // 1. the AMDI0010 Devices: (name, Memory32Fixed count, the first one's base, the Device span: DWI2CFAST Z50).
    let mut ctrls = [([0u8; 4], 0u8, 0u32, 0usize, 0usize); ACPI_CTRL_MAX];
    let mut nc = 0;
    let mut i = 0;
    while nc < ACPI_CTRL_MAX {
        let Some(h) = find_from(a, ACPI_HID_PAT, i, a.len()) else { break };
        i = h + 1;
        let Some((s, e, nm)) = device_of(a, h) else { continue };
        let (m, base, _) = mem32(a, s, e, None);
        ctrls[nc] = (nm, m, base, s, e);
        nc += 1;
    }
    if nc == 0 {
        return Err(no("no-hid"));
    }
    // 2. the ONE I2C-HID Device (a `_HID` and a `_CID` in one Device count once).
    let mut tps = [(0usize, 0usize, [0u8; 4]); 2];
    let mut nt = 0usize;
    for id in ACPI_TP_IDS {
        let mut i = 5;
        while let Some(h) = find_from(a, id, i, a.len()) {
            i = h + 1;
            let name = &a[h - 4..h];
            if a[h - 5] != 0x08 || (name != b"_HID" && name != b"_CID") {
                continue;
            }
            let Some(d) = device_of(a, h - 5) else { continue };
            if tps[..nt.min(2)].iter().any(|t| t.0 == d.0) {
                continue;
            }
            if nt < 2 {
                tps[nt] = d;
            }
            nt += 1;
        }
    }
    if nt == 0 {
        return Err(no("no-tp"));
    }
    if nt > 1 {
        return Err(AcpiNo { why: "ambiguous", what: "tp", n: nt, at: [tps[0].2, tps[1].2] });
    }
    let (s, e, tpdev) = tps[0];
    // 3. the ONE I2cSerialBus (type 1) inside it.
    let mut bus = None;
    let mut nb = 0usize;
    let mut k = s;
    while let Some(at) = find_from(a, &[0x8e], k, e) {
        k = at + 1;
        if at + 18 > e || a[at + 5] != 1 {
            continue;
        }
        let ln = u16::from_le_bytes([a[at + 1], a[at + 2]]) as usize;
        let tdl = u16::from_le_bytes([a[at + 10], a[at + 11]]) as usize;
        if at + 3 + ln > e || tdl < 6 || 12 + tdl > 3 + ln {
            continue;
        }
        nb += 1;
        if bus.is_none() {
            bus = Some((at, ln, tdl));
        }
    }
    let Some((at, ln, tdl)) = bus else { return Err(no("no-bus")) };
    if nb > 1 {
        return Err(AcpiNo { why: "ambiguous", what: "bus", n: nb, at: [tpdev, tpdev] });
    }
    let speed = u32::from_le_bytes([a[at + 12], a[at + 13], a[at + 14], a[at + 15]]);
    let tp = u16::from_le_bytes([a[at + 16], a[at + 17]]);
    // 4. its ResourceSource's last NameSeg is the controller.
    let src = &a[at + 12 + tdl..at + 3 + ln];
    let src = &src[..src.iter().position(|&c| c == 0).unwrap_or(src.len())];
    if src.len() < 4 {
        return Err(no("no-ctrl"));
    }
    let tail = &src[src.len() - 4..];
    let Some(c) = ctrls[..nc].iter().find(|c| &c.0[..] == tail) else { return Err(no("no-ctrl")) };
    match c.1 {
        0 => return Err(no("no-mem")),
        1 => {}
        m => return Err(AcpiNo { why: "ambiguous", what: "mem", n: m as usize, at: [c.0, c.0] }),
    }
    Ok(AcpiI2c { dev: c.0, base: c.2, tp, tpdev, speed, desc: dsm_desc(a, s, e), gpio: gpioint(a, s, e), cn: Cn { fmcn: dw_cn(a, c.3, c.4, b"FMCN"), sscn: dw_cn(a, c.3, c.4, b"SSCN") } })
}

/// The scan against the survey: (the controller index in `BASES`, the descriptor register to use, the first mismatch).
fn acpi_agree(f: &AcpiI2c) -> (Option<usize>, u16, Option<&'static str>) {
    let ctrl = BASES.iter().position(|&b| b == f.base as u64);
    let guess = HID_GUESS.iter().find(|g| g.0 as u16 == f.tp);
    let why = if ctrl.is_none() {
        Some("base")
    } else if guess.is_none() {
        Some("tp")
    } else if f.desc.is_some_and(|d| Some(d) != guess.map(|g| g.1)) {
        Some("desc")
    } else {
        None
    };
    (ctrl, f.desc.or(guess.map(|g| g.1)).unwrap_or(0), why)
}

/// `0x0001` or `?` / a number or `-`, for the wire.
struct OptHex(Option<u16>);
impl core::fmt::Display for OptHex {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        match self.0 {
            Some(v) => write!(f, "{:#06x}", v),
            None => f.write_str("?"),
        }
    }
}
struct OptNum(Option<usize>);
impl core::fmt::Display for OptNum {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        match self.0 {
            Some(v) => write!(f, "{}", v),
            None => f.write_str("-"),
        }
    }
}

fn seg_str(s: &[u8; 4]) -> &str {
    core::str::from_utf8(s).unwrap_or("????")
}

/// Print one scan's line; the pick when it agrees with the survey.
fn acpi_say(r: &Result<AcpiI2c, AcpiNo>) -> Option<(usize, u8, u16)> {
    match r {
        Err(n) if n.why == "ambiguous" => {
            serial_println!("[dwi2c] acpi -> REFUSED why=ambiguous what={} n={} at={},{}", n.what, n.n, seg_str(&n.at[0]), seg_str(&n.at[1]));
            None
        }
        Err(n) => {
            serial_println!("[dwi2c] acpi -> REFUSED why={}", n.why);
            None
        }
        Ok(f) => {
            let (ctrl, reg, why) = acpi_agree(f);
            match why {
                None => serial_println!(
                    "[dwi2c] acpi hid=AMDI0010 base={:#x} tp={:#04x} speed={} desc={} agree=yes dev={} tpdev={} ctrl={}",
                    f.base, f.tp, f.speed, OptHex(f.desc), seg_str(&f.dev), seg_str(&f.tpdev), OptNum(ctrl)
                ),
                Some(w) => serial_println!(
                    "[dwi2c] acpi hid=AMDI0010 base={:#x} tp={:#04x} speed={} desc={} agree=no why={} dev={} tpdev={} ctrl={}",
                    f.base, f.tp, f.speed, OptHex(f.desc), w, seg_str(&f.dev), seg_str(&f.tpdev), OptNum(ctrl)
                ),
            }
            match (why, ctrl) {
                (None, Some(k)) => Some((k, f.tp as u8, reg)),
                _ => None,
            }
        }
    }
}

/// `probe`'s once-per-boot scan of the live DSDT (read-only); sets `ACPI_PICK` on agree=yes.
fn acpi_census() {
    let Some(d) = crate::arch::acpi_power::dsdt() else {
        serial_println!("[dwi2c] acpi -> REFUSED why=no-dsdt");
        return;
    };
    let r = aml_scan(d);
    if let Some(p) = acpi_say(&r) {
        ACPI_PICK.call_once(|| p);
    }
    gpio_census(d, &r); timing_census(&r);
}

/// `run`'s guess pair from the scan: only on the controller the DSDT named, and only at an address that acked.
fn acpi_pick(i: usize, kept: &[u8]) -> Option<(u8, u16)> {
    ACPI_PICK.get().filter(|p| p.0 == i && kept.contains(&p.1)).map(|p| (p.1, p.2))
}

const ACPI_KAT_LEN: usize = 163;
/// The touchpad Device's bytes at the fragment's end (the `ambiguous` mutant appends them twice).
const ACPI_KAT_TP: usize = 112;
/// A hand-built DSDT body: `Device (I2CA) { _HID "AMDI0010", _UID 0, _CRS { Memory32Fixed (0xFEDC2000, 0x1000) } }` and
/// `Device (TPD0) { _HID "ELAN1201", _CID EISAID ("PNP0C50"), _CRS { I2cSerialBusV2 (0x15, 400000, "\_SB.I2CA") },
/// _DSM { If (Arg0 == ToUUID (HID)) { If (Arg2 == One) { Return (One) } } Return (Zero) } }`.
const ACPI_KAT: [u8; ACPI_KAT_LEN] = [
    0x5b, 0x82, 0x31, 0x49, 0x32, 0x43, 0x41, 0x08, 0x5f, 0x48, 0x49, 0x44, 0x0d, 0x41, 0x4d, 0x44, 0x49, 0x30,
    0x30, 0x31, 0x30, 0x00, 0x08, 0x5f, 0x55, 0x49, 0x44, 0x00, 0x08, 0x5f, 0x43, 0x52, 0x53, 0x11, 0x11, 0x0a,
    0x0e, 0x86, 0x09, 0x00, 0x01, 0x00, 0x20, 0xdc, 0xfe, 0x00, 0x10, 0x00, 0x00, 0x79, 0x00, 0x5b, 0x82, 0x4e,
    0x06, 0x54, 0x50, 0x44, 0x30, 0x08, 0x5f, 0x48, 0x49, 0x44, 0x0d, 0x45, 0x4c, 0x41, 0x4e, 0x31, 0x32, 0x30,
    0x31, 0x00, 0x08, 0x5f, 0x43, 0x49, 0x44, 0x0c, 0x41, 0xd0, 0x0c, 0x50, 0x08, 0x5f, 0x43, 0x52, 0x53, 0x11,
    0x21, 0x0a, 0x1e, 0x8e, 0x19, 0x00, 0x02, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x06, 0x00, 0x80, 0x1a, 0x06,
    0x00, 0x15, 0x00, 0x5c, 0x5f, 0x53, 0x42, 0x2e, 0x49, 0x32, 0x43, 0x41, 0x00, 0x79, 0x00, 0x14, 0x27, 0x5f,
    0x44, 0x53, 0x4d, 0x04, 0xa0, 0x1e, 0x93, 0x68, 0x11, 0x13, 0x0a, 0x10, 0xf7, 0xf6, 0xdf, 0x3c, 0x67, 0x42,
    0x55, 0x45, 0xad, 0x05, 0xb3, 0x0a, 0x3d, 0x89, 0x38, 0xde, 0xa0, 0x06, 0x93, 0x6a, 0x01, 0xa4, 0x01, 0xa4,
    0x00,
];

/// `tests dwi2cacpi`: the live DSDT's line (read-only), then the scan over `ACPI_KAT` and three refusal mutants.
pub fn acpi_selftest() {
    match crate::arch::acpi_power::dsdt() {
        Some(d) => {
            let r = aml_scan(d);
            let _ = acpi_say(&r);
            let _ = gpio_say(d, &r); let _ = timing_say(&r);
        }
        None => serial_println!("[dwi2c] acpi -> REFUSED why=no-dsdt"),
    }
    let good = aml_scan(&ACPI_KAT);
    let g = good.as_ref().ok();
    let base_ok = g.is_some_and(|f| f.base == 0xfedc_2000 && f.dev == *b"I2CA");
    let tp_ok = g.is_some_and(|f| f.tp == 0x15 && f.tpdev == *b"TPD0");
    let speed = g.map_or(0, |f| f.speed);
    let desc_ok = g.is_some_and(|f| f.desc == Some(1));
    let mut m = ACPI_KAT;
    m[20] = b'9'; // "AMDI0010" -> "AMDI0019"
    let nohid = ACPI_KAT[20] == b'0' && matches!(aml_scan(&m), Err(AcpiNo { why: "no-hid", .. }));
    let mut m = ACPI_KAT;
    m[119] = b'B'; // "\_SB.I2CA" -> "\_SB.I2CB"
    let noctrl = ACPI_KAT[119] == b'A' && matches!(aml_scan(&m), Err(AcpiNo { why: "no-ctrl", .. }));
    let mut two = [0u8; ACPI_KAT_LEN + ACPI_KAT_TP];
    two[..ACPI_KAT_LEN].copy_from_slice(&ACPI_KAT);
    two[ACPI_KAT_LEN..].copy_from_slice(&ACPI_KAT[ACPI_KAT_LEN - ACPI_KAT_TP..]);
    let amb = matches!(aml_scan(&two), Err(AcpiNo { why: "ambiguous", what: "tp", n: 2, .. }));
    let refuse = nohid as u32 + noctrl as u32 + amb as u32;
    let cases = [("base", base_ok), ("tp", tp_ok), ("speed", speed == 400_000), ("desc", desc_ok), ("no-hid", nohid), ("no-ctrl", noctrl), ("ambiguous", amb)];
    match cases.iter().find(|c| !c.1) {
        None => serial_println!(":: DWI2CACPI: base=ok tp=ok speed={} desc=ok refuse={}/3 -> PASS ::", speed, refuse),
        Some(c) => serial_println!(":: DWI2CACPI: case={} -> FAIL ::", c.0),
    }
    gpio_selftest(); timing_selftest();
}

// ── DWI2CGPIO (zenbook-ledger Z42) — the touchpad's GpioInt and the FCH GPIO controller's line — FILE TAIL ─────────────
// The same BYTE SCAN discipline as DWI2CACPI above (no AML runs, BP-power-4): `gpioint` reads the ONE GpioInt (0x8C,
// connection type 0 = interrupt; ACPI 6.x §6.4.3.8.1) inside the touchpad Device's span `aml_scan` already found (the
// `dsm_desc` shape): its first pin, Level/Edge, the polarity, Wake and the ResourceSource's last NameSeg. `gpio_ctrl`
// finds the ONE `_HID "AMDI0030"` Device (the AMD FCH GPIO controller, linux pinctrl-amd), its ONE Extended Interrupt
// (0x89, one GSI — an ACPI Interrupt number IS a GSI, §6.4.3.6, so `ioapic::route_gsi` takes it as is) and the
// Memory32Fixed at the FCH's fixed GPIO bank `GPIO_BASE` (coreboot ACPIMMIO_GPIO0_BASE; `mem32`, factored out of
// `aml_scan`). `GPIO_SCAN` is Ok ONLY when Z32 agreed (`ACPI_PICK` set) AND the base, the ResourceSource and the pin's
// bound agree; `tpirq::arm` (the module after this one) is the only reader. `tests dwi2cacpi` gains a second line over
// `ACPI_GPIO_KAT`: `:: DWI2CGPIO: pin=8 mode=level-low wake=1 ctrl=GPIO irq=7 line=level-low base=ok src=ok refuse=2/2
// -> PASS ::`. Owed: DWI2C-ACPI-SSDT (the touchpad and its GpioInt declared in an SSDT).

/// `Name (_HID, "AMDI0030")`: the AMD FCH GPIO controller's ACPI id.
const GPIO_HID_PAT: &[u8] = b"\x08_HID\x0dAMDI0030\x00";
/// The FCH GPIO bank 0 (coreboot ACPIMMIO_GPIO0_BASE): pin n's register at + 4n.
const GPIO_BASE: u32 = 0xfed8_1500;
/// Pin registers end here (0xfc is the master register, pin slot 63; 0x2f0.. the summary registers).
const GPIO_PIN_END: u32 = 0x2f0;
const GPIO_MASTER_PIN: u16 = 63;

/// The touchpad's GpioInt: (first pin, edge, polarity 0 high / 1 low / 2 both, wake, ResourceSource last NameSeg).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct GpioInt {
    pin: u16,
    edge: bool,
    pol: u8,
    wake: bool,
    src: [u8; 4],
}

/// The AMDI0030 Device: its name, its line (GSI, edge, active-low), its Memory32Fixed count and the GPIO bank's window.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct GpioCtl {
    dev: [u8; 4],
    gsi: u32,
    edge: bool,
    low: bool,
    mems: u8,
    base: u32,
    len: u32,
}

/// What `tpirq::arm` routes: the pin and its mode, the controller's line and window.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(not(all(feature = "ehcihid", feature = "ioapic")), allow(dead_code))]
struct TpGpio {
    pin: u16,
    edge: bool,
    pol: u8,
    gsi: u32,
    gsi_edge: bool,
    gsi_low: bool,
    base: u32,
    len: u32,
}

/// Written once by `acpi_census` (pci init, BSP); read by `tpirq::arm`.
#[cfg_attr(not(all(feature = "ehcihid", feature = "ioapic")), allow(dead_code))]
static GPIO_SCAN: spin::Once<Result<TpGpio, &'static str>> = spin::Once::new();

/// The Memory32Fixed descriptors (0x86) in `[s, e)`: (count, base, length) of the one whose base is `want`, else of
/// the first (0, 0 when none). `aml_scan`'s step 1 with `None`.
fn mem32(a: &[u8], s: usize, e: usize, want: Option<u32>) -> (u8, u32, u32) {
    let (mut m, mut first, mut hit, mut k) = (0u8, None, None, s);
    while let Some(at) = find_from(a, &[0x86, 0x09, 0x00], k, e) {
        k = at + 1;
        if at + 12 <= e {
            m = m.saturating_add(1);
            let b = u32::from_le_bytes([a[at + 4], a[at + 5], a[at + 6], a[at + 7]]);
            let l = u32::from_le_bytes([a[at + 8], a[at + 9], a[at + 10], a[at + 11]]);
            if first.is_none() {
                first = Some((b, l));
            }
            if hit.is_none() && want == Some(b) {
                hit = Some((b, l));
            }
        }
    }
    let (b, l) = hit.or(first).unwrap_or((0, 0));
    (m, b, l)
}

/// The ONE GpioInt in `[s, e)` (the touchpad Device's span). Pure: the KAT runs it.
fn gpioint(a: &[u8], s: usize, e: usize) -> Result<GpioInt, &'static str> {
    let (mut got, mut n, mut k) = (None, 0usize, s);
    while let Some(at) = find_from(a, &[0x8c], k, e) {
        k = at + 1;
        if at + 23 > e || a[at + 4] != 0 {
            continue;
        }
        let ln = u16::from_le_bytes([a[at + 1], a[at + 2]]) as usize;
        let end = at + 3 + ln;
        let pt = u16::from_le_bytes([a[at + 14], a[at + 15]]) as usize;
        let no = u16::from_le_bytes([a[at + 17], a[at + 18]]) as usize;
        if ln < 20 || end > e || pt < 23 || at + pt + 2 > end || no < 23 || at + no >= end {
            continue;
        }
        let name = &a[at + no..end];
        let name = &name[..name.iter().position(|&c| c == 0).unwrap_or(name.len())];
        if name.len() < 4 {
            continue;
        }
        let src = [name[name.len() - 4], name[name.len() - 3], name[name.len() - 2], name[name.len() - 1]];
        let fl = u16::from_le_bytes([a[at + 7], a[at + 8]]);
        n += 1;
        if got.is_none() {
            got = Some(GpioInt {
                pin: u16::from_le_bytes([a[at + pt], a[at + pt + 1]]),
                edge: fl & 1 != 0,
                pol: ((fl >> 1) & 3) as u8,
                wake: fl & 0x10 != 0,
                src,
            });
        }
    }
    match (got, n) {
        (None, _) => Err("no-gpioint"),
        (Some(g), 1) => Ok(g),
        _ => Err("ambiguous what=gpioint"),
    }
}

/// The ONE AMDI0030 Device, its ONE one-GSI Extended Interrupt, the Memory32Fixed at `GPIO_BASE`. Pure.
fn gpio_ctrl(a: &[u8]) -> Result<GpioCtl, &'static str> {
    let (mut hit, mut nh, mut i) = (None, 0usize, 0usize);
    while let Some(h) = find_from(a, GPIO_HID_PAT, i, a.len()) {
        i = h + 1;
        if let Some(d) = device_of(a, h) {
            nh += 1;
            if hit.is_none() {
                hit = Some(d);
            }
        }
    }
    let Some((s, e, dev)) = hit else { return Err("no-gpio") };
    if nh > 1 {
        return Err("ambiguous what=gpio");
    }
    let (mut irq, mut ni, mut k) = (None, 0usize, s);
    while let Some(at) = find_from(a, &[0x89, 0x06, 0x00], k, e) {
        k = at + 1;
        if at + 9 > e || a[at + 4] != 1 {
            continue;
        }
        ni += 1;
        if irq.is_none() {
            irq = Some((u32::from_le_bytes([a[at + 5], a[at + 6], a[at + 7], a[at + 8]]), a[at + 3]));
        }
    }
    let Some((gsi, fl)) = irq else { return Err("no-irq") };
    if ni > 1 {
        return Err("ambiguous what=irq");
    }
    let (mems, base, len) = mem32(a, s, e, Some(GPIO_BASE));
    if mems == 0 {
        return Err("no-mem");
    }
    Ok(GpioCtl { dev, gsi, edge: fl & 2 != 0, low: fl & 4 != 0, mems, base, len })
}

/// `level-low` and kin, for the wire.
fn mode_str(edge: bool, pol: u8) -> &'static str {
    match (edge, pol) {
        (false, 0) => "level-high",
        (false, 1) => "level-low",
        (false, _) => "level-both",
        (true, 0) => "edge-high",
        (true, 1) => "edge-low",
        (true, _) => "edge-both",
    }
}

/// Print the gpio line for one DSDT and the I2C scan's result; the verdict `GPIO_SCAN` keeps.
fn gpio_say(d: &[u8], r: &Result<AcpiI2c, AcpiNo>) -> Result<TpGpio, &'static str> {
    let c = match gpio_ctrl(d) {
        Ok(c) => c,
        Err(w) => {
            serial_println!("[dwi2c] acpi gpio -> REFUSED why={}", w);
            return Err(w);
        }
    };
    let Ok(f) = r else {
        serial_println!("[dwi2c] acpi gpio -> REFUSED why=no-tp");
        return Err("no-tp");
    };
    let g = match f.gpio {
        Ok(g) => g,
        Err(w) => {
            serial_println!("[dwi2c] acpi gpio -> REFUSED why={}", w);
            return Err(w);
        }
    };
    let why = if ACPI_PICK.get().is_none() {
        Some("i2c")
    } else if c.base != GPIO_BASE {
        Some("base")
    } else if g.src != c.dev {
        Some("src")
    } else if g.pin == GPIO_MASTER_PIN || (g.pin as u32) * 4 + 4 > c.len.min(GPIO_PIN_END) {
        Some("pin")
    } else {
        None
    };
    serial_println!(
        "[dwi2c] acpi gpio={} {} irq={} line={} base={:#x} src={} wake={} agree={}{}",
        g.pin, mode_str(g.edge, g.pol), c.gsi, mode_str(c.edge, c.low as u8), c.base, seg_str(&g.src), g.wake as u8,
        if why.is_none() { "yes" } else { "no why=" }, why.unwrap_or("")
    );
    match why {
        Some(w) => Err(w),
        None => Ok(TpGpio {
            pin: g.pin,
            edge: g.edge,
            pol: g.pol,
            gsi: c.gsi,
            gsi_edge: c.edge,
            gsi_low: c.low,
            base: c.base,
            len: c.len,
        }),
    }
}

/// `acpi_census`'s gpio half (after the I2C pick): print, and keep the verdict once.
fn gpio_census(d: &[u8], r: &Result<AcpiI2c, AcpiNo>) {
    let v = gpio_say(d, r);
    GPIO_SCAN.call_once(|| v);
}

const ACPI_GPIO_KAT_LEN: usize = 89;
/// Where the raw GpioInt starts (after the GPIO Device).
const ACPI_GPIO_KAT_DEV: usize = 54;
/// `Device (GPIO) { _HID "AMDI0030", _CRS { Interrupt (ResourceConsumer, Level, ActiveLow, Shared) {7},
/// Memory32Fixed (ReadWrite, 0xFED81500, 0x400) } }`, then one raw `GpioInt (Level, ActiveLow, ExclusiveAndWake, PullUp,
/// 0, "\_SB.GPIO") {8}` (35 bytes: the 23-byte header, pin table at 0x17, the name at 0x19).
const ACPI_GPIO_KAT: [u8; ACPI_GPIO_KAT_LEN] = [
    0x5b, 0x82, 0x34, 0x47, 0x50, 0x49, 0x4f, 0x08, 0x5f, 0x48, 0x49, 0x44, 0x0d, 0x41, 0x4d, 0x44, 0x49, 0x30,
    0x30, 0x33, 0x30, 0x00, 0x08, 0x5f, 0x43, 0x52, 0x53, 0x11, 0x1a, 0x0a, 0x17, 0x89, 0x06, 0x00, 0x0d, 0x01,
    0x07, 0x00, 0x00, 0x00, 0x86, 0x09, 0x00, 0x01, 0x00, 0x15, 0xd8, 0xfe, 0x00, 0x04, 0x00, 0x00, 0x79, 0x00,
    0x8c, 0x20, 0x00, 0x01, 0x00, 0x01, 0x00, 0x12, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x17, 0x00, 0x00, 0x19,
    0x00, 0x23, 0x00, 0x00, 0x00, 0x08, 0x00, 0x5c, 0x5f, 0x53, 0x42, 0x2e, 0x47, 0x50, 0x49, 0x4f, 0x00,
];

/// `tests dwi2cacpi`'s second line: the two scans over `ACPI_GPIO_KAT` and two refusal mutants.
fn gpio_selftest() {
    let (ck, gk) = (gpio_ctrl(&ACPI_GPIO_KAT).ok(), gpioint(&ACPI_GPIO_KAT, ACPI_GPIO_KAT_DEV, ACPI_GPIO_KAT_LEN).ok());
    let pin = gk.map_or(0xffff, |g| g.pin);
    let mode = gk.map_or("-", |g| mode_str(g.edge, g.pol));
    let wake = gk.is_some_and(|g| g.wake);
    let ctrl_ok = ck.is_some_and(|c| c.dev == *b"GPIO");
    let irq = ck.map_or(u32::MAX, |c| c.gsi);
    let line = ck.map_or("-", |c| mode_str(c.edge, c.low as u8));
    let base_ok = ck.is_some_and(|c| c.base == GPIO_BASE && c.len == 0x400 && c.mems == 1);
    let src_ok = matches!((gk, ck), (Some(g), Some(c)) if g.src == c.dev);
    let mut m = ACPI_GPIO_KAT;
    m[ACPI_GPIO_KAT_DEV + 4] = 1; // connection type interrupt -> IO
    let noint = ACPI_GPIO_KAT[ACPI_GPIO_KAT_DEV + 4] == 0
        && matches!(gpioint(&m, ACPI_GPIO_KAT_DEV, ACPI_GPIO_KAT_LEN), Err("no-gpioint"));
    let mut m = ACPI_GPIO_KAT;
    m[20] = b'9'; // "AMDI0030" -> "AMDI0039"
    let nogpio = ACPI_GPIO_KAT[20] == b'0' && matches!(gpio_ctrl(&m), Err("no-gpio"));
    let refuse = noint as u32 + nogpio as u32;
    let cases = [
        ("pin", pin == 8), ("mode", mode == "level-low"), ("wake", wake), ("ctrl", ctrl_ok), ("irq", irq == 7),
        ("line", line == "level-low"), ("base", base_ok), ("src", src_ok), ("no-gpioint", noint), ("no-gpio", nogpio),
    ];
    match cases.iter().find(|c| !c.1) {
        None => serial_println!(
            ":: DWI2CGPIO: pin={} mode={} wake=1 ctrl=GPIO irq={} line={} base=ok src=ok refuse={}/2 -> PASS ::",
            pin, mode, irq, line, refuse
        ),
        Some(c) => serial_println!(":: DWI2CGPIO: case={} -> FAIL ::", c.0),
    }
}

/// DWI2CGPIO (zenbook-ledger Z42) — the touchpad's GPIO pin as an interrupt that wakes the HID pass's read. File tail.
#[cfg(feature = "ehcihid")]
pub mod tpirq {
    //! CHARTER: Kernel — driver (the touchpad's FCH GPIO pin routed through the IOAPIC to the HID pass's one read, DWI2CGPIO zenbook-ledger Z42; no CODEX §2 handler owns the FCH GPIO block).
    //!
    //! `arm` (once, from `touchpad::attach`'s armed branch; BSP, pci init, interrupts masked) takes `GPIO_SCAN`'s Ok
    //! only, reads the pin register and every other pin register of the bank (read-only census, `others=` = pins
    //! firmware left enabled AND delivered on the same shared line, which nothing here services: any -> refused
    //! `why=shared-pins` before a write), then i8042's order (`route_irq1` / `unmask_irq1`): `vectors::alloc`,
    //! `ioapic::route_gsi` MASKED with the AMDI0030 line's mode, ONE pin write (trigger and active level from the
    //! GpioInt as linux `amd_gpio_irq_set_type` does, INTERRUPT_ENABLE bit 11, INTERRUPT_MASK bit 12 = delivered,
    //! INTERRUPT_STS bit 28 W1C; debounce and wake kept as found, WAKE_STS bit 29 never written 1), read back, then
    //! `ioapic::set_mask(gsi, false)`. The ISR takes NO lock (`sync::Mutex` does not mask interrupts): on its pin's
    //! status it MASKS the pin (bit 12 clear: an I2C-HID line stays asserted until the report is read) and W1Cs the
    //! status, sets `PENDING`, EOIs the GPIO unit (master 0xfc bit 29) and the local APIC. `touchpad::pump` reads at
    //! once on `PENDING` (else the 8 ms poll, kept as the safety net, DWI2C-PUMP owed) and `rearm` unmasks the pin
    //! after the read. The pin register's two writers never overlap: the ISR writes it only while delivered, the
    //! pump only after a PENDING read.
    use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

    /// linux pinctrl-amd.h: the pin register's fields.
    #[cfg(feature = "ioapic")]
    const LEVEL_TRIG: u32 = 1 << 8;
    #[cfg(feature = "ioapic")]
    const ACTIVE_SHIFT: u32 = 9;
    #[cfg(feature = "ioapic")]
    const ACTIVE_MASK: u32 = 3 << 9;
    #[cfg(feature = "ioapic")]
    const INT_EN: u32 = 1 << 11;
    const INT_UNMASK: u32 = 1 << 12;
    const INT_STS: u32 = 1 << 28;
    const WAKE_STS: u32 = 1 << 29;
    /// WAKE_INT_MASTER_REG and its EOI bit.
    #[cfg(feature = "ioapic")]
    const MASTER: u64 = 0xfc;
    #[cfg(feature = "ioapic")]
    const MASTER_EOI: u32 = 1 << 29;
    /// The census line prints at these report counts (bounded: four lines).
    const WIT: [u32; 4] = [1, 100, 1000, 10000];

    /// The pin register's address (0 = no IRQ: the poll alone).
    static PIN_VA: AtomicU64 = AtomicU64::new(0);
    #[cfg(feature = "ioapic")]
    static MASTER_VA: AtomicU64 = AtomicU64::new(0);
    static PENDING: AtomicBool = AtomicBool::new(false);
    static ISR_N: AtomicU32 = AtomicU32::new(0);
    static FOREIGN: AtomicU32 = AtomicU32::new(0);
    static IRQ_REPORTS: AtomicU32 = AtomicU32::new(0);
    static POLL_REPORTS: AtomicU32 = AtomicU32::new(0);

    fn rd(va: u64) -> u32 {
        // SAFETY: `va` is inside the GPIO bank window `arm` mapped and translated.
        unsafe { core::ptr::read_volatile(va as *const u32) }
    }

    fn wr(va: u64, v: u32) {
        // SAFETY: as `rd`.
        unsafe { core::ptr::write_volatile(va as *mut u32, v) }
    }

    #[cfg(feature = "ioapic")]
    pub(super) fn arm() {
        use crate::arch::ioapic::{self, Polarity, Trigger};
        let g = match super::GPIO_SCAN.get() {
            Some(Ok(g)) => *g,
            Some(Err(w)) => {
                serial_println!("[dwi2c] touchpad irq -> REFUSED why={}", w);
                return;
            }
            None => {
                serial_println!("[dwi2c] touchpad irq -> REFUSED why=no-scan");
                return;
            }
        };
        let base = g.base as u64;
        let page = base & !0xfff;
        let size = ((base - page + g.len as u64 + 0xfff) & !0xfff) as usize;
        crate::arch::memory::map_mmio_window(page, size);
        if crate::arch::memory::translate(base).is_none() || crate::arch::memory::translate(base + MASTER).is_none() {
            serial_println!("[dwi2c] touchpad irq -> REFUSED why=unmapped base={:#x}", base);
            return;
        }
        let pin_va = base + 4 * g.pin as u64;
        let v = rd(pin_va);
        let mut others = 0u32;
        let pins = (g.len.min(super::GPIO_PIN_END) / 4) as u64;
        for k in 0..pins {
            if k == g.pin as u64 || k == super::GPIO_MASTER_PIN as u64 {
                continue;
            }
            let o = rd(base + 4 * k);
            if o != 0xffff_ffff && o & (INT_EN | INT_UNMASK) == INT_EN | INT_UNMASK {
                others += 1;
            }
        }
        serial_println!(
            "[dwi2c] gpio pin={} reg={:#010x} sts={} en={} unmask={} level={} act={} wake={} others={}",
            g.pin, v, (v >> 28) & 1, (v >> 11) & 1, (v >> 12) & 1, (v >> 8) & 1, (v >> 9) & 3, (v >> 13) & 7, others
        );
        if v == 0xffff_ffff {
            serial_println!("[dwi2c] touchpad irq -> REFUSED why=pin-absent");
            return;
        }
        if others > 0 {
            serial_println!("[dwi2c] touchpad irq -> REFUSED why=shared-pins n={}", others);
            return;
        }
        let Some(vector) = crate::arch::interrupts::vectors::alloc("dwi2c-tp", isr) else {
            serial_println!("[dwi2c] touchpad irq -> REFUSED why=no-vector");
            return;
        };
        let pol = if g.gsi_low { Polarity::ActiveLow } else { Polarity::ActiveHigh };
        let trig = if g.gsi_edge { Trigger::Edge } else { Trigger::Level };
        let entry = match ioapic::route_gsi(g.gsi, vector, pol, trig, crate::arch::apic::apic_id()) {
            Ok(e) => e,
            Err(e) => {
                serial_println!("[dwi2c] touchpad irq -> REFUSED why=route-{} gsi={}", e.as_str(), g.gsi);
                return;
            }
        };
        let level = if g.edge { 0 } else { LEVEL_TRIG };
        let act = ((g.pol as u32) & 3) << ACTIVE_SHIFT;
        let want = (v & !(LEVEL_TRIG | ACTIVE_MASK | WAKE_STS)) | level | act | INT_EN | INT_UNMASK | INT_STS;
        wr(pin_va, want);
        let back = rd(pin_va);
        let fields = LEVEL_TRIG | ACTIVE_MASK | INT_EN | INT_UNMASK;
        if back & fields != want & fields {
            wr(pin_va, v & !(INT_STS | WAKE_STS));
            serial_println!("[dwi2c] touchpad irq -> REFUSED why=pin-readback want={:#010x} back={:#010x}", want, back);
            return;
        }
        MASTER_VA.store(base + MASTER, Ordering::Release);
        PIN_VA.store(pin_va, Ordering::Release);
        if let Err(e) = ioapic::set_mask(g.gsi, false) {
            PIN_VA.store(0, Ordering::Release);
            wr(pin_va, v & !(INT_STS | WAKE_STS));
            serial_println!("[dwi2c] touchpad irq -> REFUSED why=unmask-{} gsi={}", e.as_str(), g.gsi);
            return;
        }
        serial_println!(
            "[dwi2c] touchpad irq=armed gsi={} vector={:#04x} pin={} mode={} line={} entry={:#x} -> routed",
            g.gsi, vector, g.pin, super::mode_str(g.edge, g.pol), super::mode_str(g.gsi_edge, g.gsi_low as u8), entry
        );
    }

    #[cfg(not(feature = "ioapic"))]
    pub(super) fn arm() {
        serial_println!("[dwi2c] touchpad irq -> REFUSED why=no-ioapic-in-kernel");
    }

    /// The pin's ISR: MMIO and atomics only, no lock.
    #[cfg(feature = "ioapic")]
    extern "x86-interrupt" fn isr(_f: x86_64::structures::idt::InterruptStackFrame) {
        let pin_va = PIN_VA.load(Ordering::Acquire);
        if pin_va != 0 {
            let v = rd(pin_va);
            if v & INT_STS != 0 {
                wr(pin_va, (v & !(INT_UNMASK | WAKE_STS)) | INT_STS);
                PENDING.store(true, Ordering::Release);
                ISR_N.fetch_add(1, Ordering::Relaxed);
            } else {
                FOREIGN.fetch_add(1, Ordering::Relaxed);
            }
            let m = MASTER_VA.load(Ordering::Acquire);
            if m != 0 {
                wr(m, rd(m) | MASTER_EOI);
            }
        }
        crate::arch::apic::eoi();
    }

    /// The pump's question: did the pin fire since the last read? (Clears the flag.)
    pub(super) fn take() -> bool {
        PENDING.swap(false, Ordering::AcqRel)
    }

    /// After a PENDING read: deliver the pin again (bit 12), writing no W1C bit.
    pub(super) fn rearm() {
        let va = PIN_VA.load(Ordering::Acquire);
        if va == 0 {
            return;
        }
        let v = rd(va);
        wr(va, (v & !(INT_STS | WAKE_STS)) | INT_UNMASK);
    }

    /// One decoded report: counted by what woke its read; the census line at `WIT`'s counts.
    pub(super) fn note(irq: bool, reports: u32) {
        if irq {
            IRQ_REPORTS.fetch_add(1, Ordering::Relaxed);
        } else {
            POLL_REPORTS.fetch_add(1, Ordering::Relaxed);
        }
        if WIT.contains(&reports) {
            serial_println!(
                "[dwi2c] touchpad irq={} polled={} isr={} foreign={}",
                IRQ_REPORTS.load(Ordering::Relaxed), POLL_REPORTS.load(Ordering::Relaxed),
                ISR_N.load(Ordering::Relaxed), FOREIGN.load(Ordering::Relaxed)
            );
        }
    }
}

// ── DWI2CFAST (zenbook-ledger Z50) — the controller's FMCN/SSCN and the held touchpad at 400 kHz — FILE TAIL ──────────────
// The same BYTE SCAN discipline (no AML runs, BP-power-4). linux `i2c_dw_acpi_params` evaluates FMCN/SSCN as NAMED OBJECTS
// under the controller Device (not `_DSD` properties): a Package of 3 Integers (SCL hcnt, SCL lcnt, SDA hold). `dw_cn` reads
// the two literal shapes `Name (FMCN, Package (3) {..})` and `Method (FMCN) { Return (Package (3) {..}) }` inside the AMDI0010
// Device span `aml_scan` found, refusing by name (absent, ambiguous, shape). `FAST` is Ok ONLY when Z32 agreed (`ACPI_PICK`),
// the touchpad's I2cSerialBus ConnectionSpeed is >= 400000 and the counts are sane at AMDI0010's fixed 150 MHz (`fast_ok`);
// SSCN is printed, not applied (standard mode stays as `open` sets it). `fast_attach` (from `touchpad::attach`'s armed arm,
// before TP/ARMED, so the pump cannot share the bus) runs `fast_switch`, THE ONE program fn: disable, IC_CON speed = fast,
// FS_SCL_HCNT/LCNT, SDA_HOLD (IC_COMP_VERSION >= 1.11a only), read back; the FIRST 400 kHz transfer is a verify against
// `bring_up`'s 100 kHz read of the report descriptor, and any failure restores standard mode with ONE 100 kHz retry. The
// census and the bring-up never run at 400 kHz. `tests dwi2cacpi`'s third line: `:: DWI2CFAST: fmcn=120/256/30
// sscn=650/669/30 program=ok verify=ok fallback=ok refuse=3/3 -> PASS ::`. Owed: DWI2C-FAST-CALC (linux's fast-mode formula
// at 150 MHz when the DSDT carries no FMCN), DWI2C-ACPI-SSDT (FMCN/SSCN declared in an SSDT).

const IC_FS_SCL_HCNT: usize = 0x1c;
const IC_FS_SCL_LCNT: usize = 0x20;
const IC_SDA_HOLD: usize = 0x7c;
/// IC_CON's speed field [2:1]: 1 standard, 2 fast.
const CON_SPEED_MASK: u32 = 3 << 1;
const CON_SPEED_FAST: u32 = 2 << 1;
/// IC_SDA_HOLD exists from IC_COMP_VERSION 1.11a (linux DW_IC_SDA_HOLD_MIN_VERS).
const SDA_HOLD_MIN_VER: u32 = 0x3131_312a;
/// IC_SDA_HOLD's RX hold field (linux forces it to 1 when the count leaves it 0).
const SDA_HOLD_RX_MASK: u32 = 0xff << 16;
/// The I2cSerialBus ConnectionSpeed that allows fast mode.
const FAST_MIN: u32 = 400_000;

/// The controller Device's FMCN and SSCN (hcnt, lcnt, sda hold), else the refusal's name.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Cn {
    fmcn: Result<[u32; 3], &'static str>,
    sscn: Result<[u32; 3], &'static str>,
}

/// What `fast_attach` programs: the controller index into `BASES`, the touchpad address, the FMCN counts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(not(feature = "ehcihid"), allow(dead_code))]
struct Fast {
    ctrl: usize,
    addr: u8,
    hcnt: u32,
    lcnt: u32,
    hold: u32,
}

/// What `fast_switch` came to: the read-back counts and hold (None = SDA_HOLD kept) or why nothing stayed programmed,
/// the read-back (con, hcnt, lcnt), the verify's word and the 100 kHz retry after a fallback.
#[cfg_attr(not(feature = "ehcihid"), allow(dead_code))]
struct FastOut {
    prog: Result<(u32, u32, Option<u32>), &'static str>,
    back: (u32, u32, u32),
    verify: &'static str,
    retry: Option<bool>,
}

/// Written once by `acpi_census` (pci init, BSP); read once by `touchpad::attach` before ARMED.
#[cfg_attr(not(feature = "ehcihid"), allow(dead_code))]
static FAST: spin::Once<Result<Fast, &'static str>> = spin::Once::new();

/// A Package (0x12) of exactly 3 Integers (Zero, One, ByteConst, WordConst, DWordConst) at `p`, inside its own bound and `e`.
fn pkg3(a: &[u8], p: usize, e: usize) -> Option<[u32; 3]> {
    if *a.get(p)? != 0x12 {
        return None;
    }
    let (len, w) = pkg_len(a, p + 1)?;
    let end = (p + 1 + len).min(e).min(a.len());
    if *a.get(p + 1 + w)? != 3 {
        return None;
    }
    let mut i = p + 2 + w;
    let mut v = [0u32; 3];
    for slot in v.iter_mut() {
        let (x, n) = match *a.get(i)? {
            0x00 => (0u32, 1usize),
            0x01 => (1, 1),
            0x0a => (*a.get(i + 1)? as u32, 2),
            0x0b => (u16::from_le_bytes(a.get(i + 1..i + 3)?.try_into().ok()?) as u32, 3),
            0x0c => (u32::from_le_bytes(a.get(i + 1..i + 5)?.try_into().ok()?), 5),
            _ => return None,
        };
        if i + n > end {
            return None;
        }
        *slot = x;
        i += n;
    }
    Some(v)
}

/// The ONE definition of `name` in the controller span `[s, e)`: `Name (name, Package)` (NameOp 0x08 before it) or
/// `Method (name) { Return (Package) }` (MethodOp 0x14 + a PkgLength of the width it encodes before it, ReturnOp first).
fn dw_cn(a: &[u8], s: usize, e: usize, name: &[u8; 4]) -> Result<[u32; 3], &'static str> {
    let (mut hit, mut n, mut k) = (None, 0usize, s);
    while let Some(at) = find_from(a, name, k, e) {
        k = at + 1;
        let named = at >= 1 && a[at - 1] == 0x08;
        let method = !named
            && (1..=4usize).any(|w| at > w && a[at - 1 - w] == 0x14 && pkg_len(a, at - w).is_some_and(|(_, ww)| ww == w))
            && a.get(at + 5) == Some(&0xa4);
        let p = if named {
            at + 4
        } else if method {
            at + 6
        } else {
            continue;
        };
        n += 1;
        if hit.is_none() {
            hit = Some(p);
        }
    }
    let Some(p) = hit else { return Err("absent") };
    if n > 1 {
        return Err("ambiguous");
    }
    pkg3(a, p, e).ok_or("shape")
}

/// The FMCN counts' sanity at AMDI0010's fixed 150 MHz (linux acpi_apd): hcnt 6..=0xffff, lcnt 8..=0xffff, hcnt + lcnt >=
/// 300 (at most ~500 kHz nominal), the SDA hold under the low count. None = sane, else the refusal's name.
fn fast_ok(hcnt: u32, lcnt: u32, hold: u32) -> Option<&'static str> {
    if !(6..=0xffff).contains(&hcnt) || !(8..=0xffff).contains(&lcnt) || hcnt + lcnt < 300 {
        Some("counts")
    } else if hold & 0xffff >= lcnt {
        Some("hold")
    } else {
        None
    }
}

/// `120/256/30` or the refusal's name, for the wire.
struct CnStr(Result<[u32; 3], &'static str>);
impl core::fmt::Display for CnStr {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        match self.0 {
            Ok(v) => write!(f, "{}/{}/{}", v[0], v[1], v[2]),
            Err(w) => f.write_str(w),
        }
    }
}

/// Print the timing line for one scan; the verdict `FAST` keeps (Ok only on Z32's agreement, speed and sane counts).
fn timing_say(r: &Result<AcpiI2c, AcpiNo>) -> Result<Fast, &'static str> {
    let Ok(f) = r else {
        serial_println!("[dwi2c] acpi timing -> REFUSED why=no-scan sscn=-");
        return Err("no-scan");
    };
    let [h, l, hold] = match f.cn.fmcn {
        Ok(v) => v,
        Err(w) => {
            let why = match w {
                "absent" => "no-fmcn",
                "ambiguous" => "fmcn-ambiguous",
                _ => "fmcn-shape",
            };
            serial_println!("[dwi2c] acpi timing -> REFUSED why={} sscn={}", why, CnStr(f.cn.sscn));
            return Err(why);
        }
    };
    let pick = ACPI_PICK.get();
    let why = if pick.is_none() {
        Some("i2c")
    } else if f.speed < FAST_MIN {
        Some("speed")
    } else {
        fast_ok(h, l, hold)
    };
    serial_println!(
        "[dwi2c] acpi timing fmcn={}/{}/{} sscn={} speed={} -> {}{}",
        h, l, hold, CnStr(f.cn.sscn), f.speed, if why.is_none() { "fast" } else { "standard why=" }, why.unwrap_or("")
    );
    match (why, pick) {
        (None, Some(p)) => Ok(Fast { ctrl: p.0, addr: p.1, hcnt: h, lcnt: l, hold }),
        (Some(w), _) => Err(w),
        (None, None) => Err("i2c"),
    }
}

/// `acpi_census`'s timing half (after the I2C pick): print, and keep the verdict once.
fn timing_census(r: &Result<AcpiI2c, AcpiNo>) {
    let v = timing_say(r);
    FAST.call_once(|| v);
}

/// Write back what `fast_switch` found (the controller already disabled).
fn fast_restore<I: Mmio>(io: &I, found: &(u32, u32, u32, u32), hold_ok: bool) {
    io.wr(IC_CON, found.0);
    io.wr(IC_FS_SCL_HCNT, found.1);
    io.wr(IC_FS_SCL_LCNT, found.2);
    if hold_ok {
        io.wr(IC_SDA_HOLD, found.3);
    }
}

/// THE ONE fast-mode program fn (the KAT drives it over the fake): read the found CON / FS counts / SDA_HOLD, disable,
/// IC_CON speed field = fast, FS_SCL_HCNT/LCNT, SDA_HOLD (>= 1.11a only, RX hold forced to 1 when 0), read back (a mismatch
/// restores the found four); then the first 400 kHz transfer reads `want.len()` (<= 4) bytes of `rdreg` and compares them
/// with `want` (read at 100 kHz); any failure restores the found four (`open`'s standard mode) and retries ONCE at 100 kHz.
fn fast_switch<I: Mmio>(io: &I, f: &Fast, rdreg: u16, want: &[u8], win: usize) -> FastOut {
    let found = (io.rd(IC_CON), io.rd(IC_FS_SCL_HCNT), io.rd(IC_FS_SCL_LCNT), io.rd(IC_SDA_HOLD));
    let mut o = FastOut { prog: Err("disable"), back: (found.0, found.1, found.2), verify: "-", retry: None };
    if set_enable(io, false, "disable").is_err() {
        return o;
    }
    let hold_ok = io.rd(IC_COMP_VERSION) >= SDA_HOLD_MIN_VER;
    let hold = if f.hold & SDA_HOLD_RX_MASK == 0 { f.hold | (1 << 16) } else { f.hold };
    io.wr(IC_CON, (found.0 & !CON_SPEED_MASK) | CON_SPEED_FAST);
    io.wr(IC_FS_SCL_HCNT, f.hcnt);
    io.wr(IC_FS_SCL_LCNT, f.lcnt);
    if hold_ok {
        io.wr(IC_SDA_HOLD, hold);
    }
    let (con, hc, lc, hd) = (io.rd(IC_CON), io.rd(IC_FS_SCL_HCNT), io.rd(IC_FS_SCL_LCNT), io.rd(IC_SDA_HOLD));
    o.back = (con, hc, lc);
    if con & CON_SPEED_MASK != CON_SPEED_FAST || hc & 0xffff != f.hcnt || lc & 0xffff != f.lcnt || (hold_ok && hd & 0x00ff_ffff != hold) {
        fast_restore(io, &found, hold_ok);
        o.prog = Err("readback");
        return o;
    }
    o.prog = Ok((hc & 0xffff, lc & 0xffff, if hold_ok { Some(hd & 0x00ff_ffff) } else { None }));
    let n = want.len().min(4);
    let reg = rdreg.to_le_bytes();
    let mut chk = [0u8; 4];
    o.verify = match xfer(io, f.addr, &reg, &mut chk[..n], win) {
        Ok(()) if chk[..n] == want[..n] => "ok",
        Ok(()) => "mismatch",
        Err(e) if e.step == "abort" && e.src & ABRT_7B_ADDR_NOACK != 0 => "nack",
        Err(e) if e.step == "abort" => "abort",
        Err(_) => "stuck",
    };
    if o.verify != "ok" {
        let _ = set_enable(io, false, "disable");
        fast_restore(io, &found, hold_ok);
        let mut again = [0u8; 4];
        o.retry = Some(xfer(io, f.addr, &reg, &mut again[..n], win).is_ok() && again[..n] == want[..n]);
    }
    o
}

/// `touchpad::attach`'s armed arm, before TP/ARMED: the switch on the controller and address `FAST` names, else standard
/// mode kept by name. `want` = the report descriptor's first bytes `bring_up` read at 100 kHz.
#[cfg_attr(not(feature = "ehcihid"), allow(dead_code))]
fn fast_attach(i: usize, io: &Real, addr: u8, d: &HidDesc, win: usize, want: &[u8]) {
    let f = match FAST.get() {
        Some(Ok(f)) if f.ctrl == i && f.addr == addr => *f,
        Some(Ok(_)) => {
            serial_println!("[dwi2c] ctrl={} speed=100k -> KEPT why=other-ctrl", i);
            return;
        }
        Some(Err(w)) => {
            serial_println!("[dwi2c] ctrl={} speed=100k -> KEPT why={}", i, w);
            return;
        }
        None => {
            serial_println!("[dwi2c] ctrl={} speed=100k -> KEPT why=no-scan", i);
            return;
        }
    };
    let o = fast_switch(io, &f, d.rdreg, want, win);
    match o.prog {
        Err("readback") => {
            serial_println!(
                "[dwi2c] ctrl={} speed=100k -> KEPT why=readback con={:#x} hcnt={} lcnt={}",
                i, o.back.0, o.back.1, o.back.2
            );
            return;
        }
        Err(w) => {
            serial_println!("[dwi2c] ctrl={} speed=100k -> KEPT why={}", i, w);
            return;
        }
        Ok((h, l, Some(hd))) => serial_println!("[dwi2c] ctrl={} speed=400k hcnt={} lcnt={} hold={:#x} -> programmed", i, h, l, hd),
        Ok((h, l, None)) => serial_println!("[dwi2c] ctrl={} speed=400k hcnt={} lcnt={} hold=kept -> programmed", i, h, l),
    }
    if o.verify == "ok" {
        serial_println!("[dwi2c] ctrl={} speed=400k verify=ok -> fast", i);
    } else {
        serial_println!(
            "[dwi2c] ctrl={} speed=400k verify={} -> FALLBACK speed=100k retry={}",
            i, o.verify, if o.retry == Some(true) { "ok" } else { "fail" }
        );
    }
}

const TIMING_KAT_LEN: usize = 34;
/// `Name (FMCN, Package (3) { 0x78, 0x0100, 0x1E })` (ByteConst, WordConst, ByteConst), then
/// `Method (SSCN) { Return (Package (3) { 0x028A, 0x029D, 0x1E }) }`.
const TIMING_KAT: [u8; TIMING_KAT_LEN] = [
    0x08, 0x46, 0x4d, 0x43, 0x4e, 0x12, 0x09, 0x03, 0x0a, 0x78, 0x0b, 0x00, 0x01, 0x0a, 0x1e, //
    0x14, 0x12, 0x53, 0x53, 0x43, 0x4e, 0x00, 0xa4, 0x12, 0x0a, 0x03, 0x0b, 0x8a, 0x02, 0x0b, 0x9d, 0x02, 0x0a, 0x1e,
];
/// The KAT's switch: FMCN's counts on the fake's acking address.
const FAST_KAT: Fast = Fast { ctrl: 0, addr: 0x15, hcnt: 120, lcnt: 256, hold: 30 };

/// The fake opened as `attach` opens it, the report descriptor's first four bytes served at register 0x0002.
fn kat_fast_fake(nack_fast: bool) -> Fake {
    let f = Fake::new();
    {
        let mut st = f.s.borrow_mut();
        st.rdesc[..4].copy_from_slice(&[0x05, 0x0d, 0x09, 0x05]);
        st.rdesc_n = 4;
        st.rdreg = 0x0002;
        st.nack_fast = nack_fast;
    }
    f
}

/// (programmed and read back, verified at 400 kHz).
fn kat_fast_program() -> (bool, bool) {
    let f = kat_fast_fake(false);
    if open(&f).is_err() {
        return (false, false);
    }
    let o = fast_switch(&f, &FAST_KAT, 0x0002, &[0x05, 0x0d, 0x09, 0x05], 16);
    let st = f.s.borrow();
    let prog = o.prog == Ok((120, 256, Some(30 | (1 << 16))))
        && st.con & CON_SPEED_MASK == CON_SPEED_FAST
        && st.fs_hcnt == 120
        && st.fs_lcnt == 256;
    (prog, o.verify == "ok" && o.retry.is_none())
}

/// A NACK at 400 kHz: standard mode and the found FS counts back, the one 100 kHz retry answers.
fn kat_fast_fallback() -> bool {
    let f = kat_fast_fake(true);
    if open(&f).is_err() {
        return false;
    }
    let o = fast_switch(&f, &FAST_KAT, 0x0002, &[0x05, 0x0d, 0x09, 0x05], 16);
    let st = f.s.borrow();
    o.prog.is_ok()
        && o.verify == "nack"
        && o.retry == Some(true)
        && st.con & CON_SPEED_MASK == CON_SPEED_STD
        && st.fs_hcnt == 0
        && st.fs_lcnt == 0
        && st.hold == 0
}

/// `tests dwi2cacpi`'s third line: `dw_cn` over `TIMING_KAT` and two mutants, the counts' sanity, the switch and its
/// fallback over the fake controller.
fn timing_selftest() {
    let fm = dw_cn(&TIMING_KAT, 0, TIMING_KAT_LEN, b"FMCN");
    let ss = dw_cn(&TIMING_KAT, 0, TIMING_KAT_LEN, b"SSCN");
    let mut m = TIMING_KAT;
    m[4] = b'X'; // "FMCN" -> "FMCX"
    let absent = TIMING_KAT[4] == b'N' && dw_cn(&m, 0, TIMING_KAT_LEN, b"FMCN") == Err("absent");
    let mut m = TIMING_KAT;
    m[7] = 2; // NumElements 3 -> 2
    let shape = TIMING_KAT[7] == 3 && dw_cn(&m, 0, TIMING_KAT_LEN, b"FMCN") == Err("shape");
    let counts = fast_ok(120, 5, 30) == Some("counts") && fast_ok(120, 256, 30).is_none();
    let refuse = absent as u32 + shape as u32 + counts as u32;
    let (prog, verify) = kat_fast_program();
    let fallback = kat_fast_fallback();
    let cases = [
        ("fmcn", fm == Ok([120, 256, 30])), ("sscn", ss == Ok([650, 669, 30])), ("absent", absent), ("shape", shape),
        ("counts", counts), ("program", prog), ("verify", verify), ("fallback", fallback),
    ];
    match cases.iter().find(|c| !c.1) {
        None => serial_println!(
            ":: DWI2CFAST: fmcn={} sscn={} program=ok verify=ok fallback=ok refuse={}/3 -> PASS ::",
            CnStr(fm), CnStr(ss), refuse
        ),
        Some(c) => serial_println!(":: DWI2CFAST: case={} -> FAIL ::", c.0),
    }
}
