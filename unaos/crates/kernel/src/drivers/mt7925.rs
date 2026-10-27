//! CHARTER: Kernel — driver (the MediaTek MT7925 Wi-Fi 7 radio's S1 probe, zenbook-ledger Z5, S2 driver-own and the chip id, Z11, and S3 the WFSYS reset and the WFDMA rings, Z15; no CODEX §2 handler owns a NIC driver, the BCM4331 `wifi/**` sits in the same seam)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! MT7925 — S1, the probe (ZENBOOKMT7925, zenbook-ledger Z5). The ladder is
//! `docs/dev/OS/06_NETWORK_STACK/mt7925.md`; S2 (MT7925S2, Z11) and S3 (MT7925S3, Z15) are the two file-tail blocks; S4..S10 are owed there by name.
//!
//! WHAT THIS RUNG DOES. Once per boot, from the three storage-ready passes in `main.rs` (beside `wifi::service()`,
//! the same forward-only polled shape), and only after `boot::services_gate("wifi")` answers (R86, BP-net-4):
//! * finds `14c3:7925` / `14c3:0717` in the boot's PCI registry (`arch::pci::functions`, HWREPORT B725) — no walk;
//! * reads config space (COMMAND, REVISION, BAR0);
//! * maps ONE page of BAR0 (BAR0+0x0e0000, the always-on `conn_host_csr_top` block, linux mt7925/pci.c fixed_map)
//!   and reads two registers in it: LPCTL (+0x10, the host/firmware ownership handshake; OWN_SYNC bit 2 set =
//!   firmware owns) and CONN_ON_MISC (+0xf0, bits 1:0 = FW_PWR_ON | FW_N9_ON). Linux polls LPCTL before it owns
//!   the chip (mt792x_core.c drv/fw pmctrl), so the block answers reads in either state;
//! * hashes the linux-firmware pair on the ESP at `FW/MT7925/` against `unaos/firmware/mt7925.pins` (arroyo's
//!   MT7925-FW line stages it from `UNAOS_MT7925_FW_PATH`), and says one line, then parks.
//!
//! S1 WRITES NOTHING: no config write, no MMIO write, nothing uploaded, nothing transmitted (BP-net-7, BP-net-8).
//! S2 (MT7925S2, zenbook-ledger Z11; the file tail) runs once on the next pass, only on a positively identified
//! Zenbook (`hw::platform()`), and its device writes are exactly: LPCTL CLR_OWN (driver-own, at most OWN_TRIES, none
//! when the driver already owns), HIF_REMAP_L1 twice (select the 0x7001 window for CHIPID 0x70010200 / REV
//! 0x70010204, then restore), and LPCTL SET_OWN (at most OWN_TRIES) only when S2 took own — the device left as found.
//! The WFSYS reset and the WFDMA are S3's (MT7925S3, Z15; the second tail block, the pass after a clean S2 line). A machine without the radio (QEMU, the rMBP) parks on the first pass: no
//! gate call, no line.
//!
//! Hardware facts are from linux v6.11 drivers/net/wireless/mediatek/mt76 (ISC); no code was copied.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicU8, Ordering};

/// MediaTek, and the two device ids linux's mt7925 table binds (mt7925/pci.c:13-19).
const VENDOR: u16 = 0x14c3;
const DEVICES: [u16; 2] = [0x7925, 0x0717];
/// BAR0 offset of the `conn_host_csr_top` block (0x7c060000 in the chip's map; pci.c fixed_map).
const CONN_HOST_CSR_TOP: u64 = 0x000e_0000;
/// MT_CONN_ON_LPCTL (0x7c060010) and its OWN_SYNC bit (mt792x_regs.h:469-472).
const LPCTL: u64 = 0x10;
const OWN_SYNC: u32 = 1 << 2;
/// MT_CONN_ON_MISC (0x7c0600f0) and MT_TOP_MISC2_FW_N9_RDY (mt792x_regs.h:474-477).
const CONN_ON_MISC: u64 = 0xf0;
const FW_N9_RDY: u32 = 0x3;
const PCI_COMMAND: u8 = 0x04;
const PCI_REVISION: u8 = 0x08;
const CMD_MEM: u16 = 1 << 1;
/// Where arroyo's MT7925-FW line (`fw_stage_pinned`) puts the pair on the ESP (R99).
const DIR0: &str = "FW";
const DIR1: &str = "MT7925";
/// (role in mt7925.pins, linux-firmware file name under mediatek/mt7925/).
const SET: [(&str, &str); 2] = [("patch", "WIFI_MT7925_PATCH_MCU_1_1_hdr.bin"), ("ram", "WIFI_RAM_CODE_MT7925_1_1.bin")];
/// The pinned RAM code is 1390712 bytes; a file past 4 MiB is refused before the heap is committed.
const MAX_FILE: usize = 4 * 1024 * 1024;
/// How long the probe waits for the program-source volume before it says `fw=no-volume`.
const WAIT_BOUND_MS: u64 = 30_000;

const PINS: &str = include_str!("../../../../firmware/mt7925.pins");

const S_START: u8 = 0;
const S_GATE: u8 = 1;
const S_WAIT_VOL: u8 = 2;
const S_PARKED: u8 = 3;
static STATE: AtomicU8 = AtomicU8::new(S_START);
static SINCE_MS: AtomicU64 = AtomicU64::new(0);

/// What the probe read; owned by this file, taken only by `service`, never held across MMIO or the FAT read.
#[derive(Clone, Copy)]
struct Probe {
    bus: u8,
    dev: u8,
    func: u8,
    device: u16,
    rev: u8,
    bar0: u64,
    mem: bool,
    /// (LPCTL, CONN_ON_MISC) when BAR0 was mapped and read.
    regs: Option<(u32, u32)>,
}

static PROBE: crate::sync::Mutex<Option<Probe>> = crate::sync::Mutex::new(None);

/// The forward-only state machine; called from every storage-ready pass, it speaks at most once per boot and then
/// costs one relaxed load.
pub fn service() {
    match STATE.load(Ordering::Relaxed) {
        S_START => start(),
        S_GATE => gate(),
        S_WAIT_VOL => wait_vol(),
        S_S2 => s2(),
        S_S3 => s3(),
        _ => {}
    }
}

/// The registry check comes BEFORE any gate call, so a machine without the radio leaves no trace.
fn start() {
    let found = crate::arch::pci::functions().into_iter().find(|f| f.vendor == VENDOR && DEVICES.contains(&f.device));
    let Some(f) = found else {
        STATE.store(S_PARKED, Ordering::Relaxed);
        return;
    };
    *PROBE.lock() = Some(Probe { bus: f.bus, dev: f.dev, func: f.func, device: f.device, rev: 0, bar0: 0, mem: false, regs: None });
    STATE.store(S_GATE, Ordering::Relaxed);
}

fn gate() {
    if !crate::boot::services_gate("wifi") {
        return; // R86: no radio probe under the setter or the login screen
    }
    let held = *PROBE.lock();
    let Some(mut p) = held else {
        STATE.store(S_PARKED, Ordering::Relaxed);
        return;
    };
    // SAFETY: config-space READS of a function the boot's walk recorded; no side effect.
    let cmd = unsafe { crate::arch::pci::read_config_16(p.bus, p.dev, p.func, PCI_COMMAND) };
    // SAFETY: as above.
    p.rev = (unsafe { crate::arch::pci::read_config_32(p.bus, p.dev, p.func, PCI_REVISION) } & 0xff) as u8;
    p.mem = cmd & CMD_MEM != 0;
    p.bar0 = crate::drivers::pci::PciScanner::get_bar_address(p.bus, p.dev, p.func);
    if p.mem && p.bar0 != 0 {
        p.regs = read_csr_top(p.bar0);
    }
    *PROBE.lock() = Some(p);
    SINCE_MS.store(crate::arch::ms(), Ordering::Relaxed);
    STATE.store(S_WAIT_VOL, Ordering::Relaxed);
}

/// Map the one page of `conn_host_csr_top` (a page-table edit; the device sees nothing) and read LPCTL and
/// CONN_ON_MISC. `None` when the page is not present after the map (reading it would fault).
fn read_csr_top(bar0: u64) -> Option<(u32, u32)> {
    let page = bar0 + CONN_HOST_CSR_TOP;
    crate::arch::memory::map_mmio_window(page, 0x1000);
    crate::arch::memory::translate(page)?;
    // SAFETY: the page is identity-mapped uncacheable (map_mmio_window) and proved present (translate); both
    // registers are status reads in the always-on block that linux polls before it owns the chip.
    let lpctl = unsafe { core::ptr::read_volatile((page + LPCTL) as *const u32) };
    // SAFETY: as above.
    let misc = unsafe { core::ptr::read_volatile((page + CONN_ON_MISC) as *const u32) };
    Some((lpctl, misc))
}

fn wait_vol() {
    let waited = crate::arch::ms().saturating_sub(SINCE_MS.load(Ordering::Relaxed));
    let have = crate::drivers::block::program_source().is_some();
    if !have && waited < WAIT_BOUND_MS {
        return;
    }
    STATE.store(S_S2, Ordering::Relaxed); // MT7925S2 (Z11): S2 runs on the next pass, after this S1 line
    let held = *PROBE.lock();
    let Some(p) = held else { return };
    let fw = if have { fw_check() } else { Fw { word: String::from("no-volume"), sha: ["-", "-"] } };
    say(&p, &fw);
}

/// The firmware verdict: `fw=` word and the per-role `*_sha=` tokens.
struct Fw {
    word: String,
    sha: [&'static str; 2],
}

fn name_is(e: &crate::fs::fat::DirEntry, want: &str) -> bool {
    e.name().trim_end_matches('.').eq_ignore_ascii_case(want)
}

fn sub(fs: &crate::fs::fat::FatFs, ents: &[crate::fs::fat::DirEntry], name: &str) -> Option<Vec<crate::fs::fat::DirEntry>> {
    let c = ents.iter().find(|e| e.is_dir && name_is(e, name))?.first_cluster();
    if c == 0 {
        return None;
    }
    fs.read_dir(c).ok()
}

fn hex32(d: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for b in d {
        let _ = core::fmt::Write::write_fmt(&mut s, format_args!("{:02x}", b));
    }
    s
}

/// Read `FW/MT7925/<file>` x2 from the program-source FAT, digest each against its pin. A member that is not `ok`
/// gets its own line (`[mt7925] fw <file> role= bytes= sha256= -> <why>`), so a newer linux-firmware drop reads
/// as a digest to pin, not a silence.
fn fw_check() -> Fw {
    let mut out = Fw { word: String::from("absent"), sha: ["-", "-"] };
    let Ok(fs) = crate::fs::fat::mount_program_source() else {
        out.word = String::from("no-esp");
        return out;
    };
    let Ok(root) = fs.read_root() else {
        out.word = String::from("no-esp");
        return out;
    };
    let Some(d0) = sub(&fs, &root, DIR0) else { return out };
    let Some(ents) = sub(&fs, &d0, DIR1) else { return out };
    let mut present = 0usize;
    for (i, (role, file)) in SET.iter().enumerate() {
        let Some(de) = ents.iter().find(|e| !e.is_dir && name_is(e, file)) else { continue };
        present += 1;
        let size = de.size as usize;
        let mut buf = Vec::new();
        let (verdict, hex): (&'static str, String) = if size > MAX_FILE {
            ("too-large", String::from("-"))
        } else if fs.read_file(de, &mut buf, MAX_FILE).is_err() || buf.len() != size {
            ("unreadable", String::from("-"))
        } else {
            let sha = crypto_core::sha2::sha256(&buf);
            let hex = hex32(&sha);
            match crate::hash::pins::pin_for(PINS, role) { // FWSTAGEONE (rmbp-ledger B984): the one pins reader
                None => ("unpinned", hex),
                Some(p) if p == sha => ("ok", hex),
                Some(_) => ("mismatch", hex),
            }
        };
        out.sha[i] = match verdict {
            "ok" | "mismatch" | "unpinned" => verdict,
            _ => "-",
        };
        if verdict != "ok" {
            let why = if verdict == "mismatch" { "pin-mismatch" } else { verdict };
            serial_println!("[mt7925] fw {} role={} bytes={} sha256={} -> {}", file, role, size, hex, why);
        }
    }
    if present > 0 {
        out.word = alloc::format!("present:{}", present);
    }
    out
}

fn say(p: &Probe, fw: &Fw) {
    let bar0 = if p.bar0 == 0 { String::from("unassigned") } else { alloc::format!("{:#x}", p.bar0) };
    let (lpctl, own, fwstate) = match p.regs {
        Some((l, m)) if l != u32::MAX && m != u32::MAX => (
            alloc::format!("{:#010x}", l),
            if l & OWN_SYNC != 0 { "fw" } else { "drv" },
            alloc::format!("{}", m & FW_N9_RDY),
        ),
        Some((l, _)) => (alloc::format!("{:#010x}", l), "-", String::from("-")),
        None => (String::from("-"), "-", String::from("-")),
    };
    serial_println!(
        "[mt7925] probe at={}:{}.{} id={:04x}:{:04x} rev={:#04x} bar0={} mem={} lpctl={} own={} fwstate={} fw={} patch_sha={} ram_sha={} -> parked",
        p.bus, p.dev, p.func, VENDOR, p.device, p.rev, bar0, if p.mem { "on" } else { "off" }, lpctl, own, fwstate, fw.word, fw.sha[0], fw.sha[1]
    );
}

// ── MT7925S2 (zenbook-ledger Z11) — S2: driver-own and the chip id through the L1 remap ─────────────────────────────
// Once, on the storage-ready pass after the S1 line (`wait_vol` hands to S_S2), only on a positively identified Zenbook
// (`hw::platform()`). Writes, all bounded: LPCTL CLR_OWN (<= OWN_TRIES; none when the driver already owns), HIF_REMAP_L1
// twice (select the 0x7001 window, restore), and LPCTL SET_OWN (<= OWN_TRIES) only when this rung took own. Linux
// reads CHIPID/REV right after driver-own and before the WFSYS reset (mt7925/pci.c probe), so the reset stays S3's.

/// MT_CONN_ON_LPCTL's SET_OWN (firmware takes own) and CLR_OWN (driver takes own) bits (mt792x_regs.h:470-471).
const SET_OWN: u32 = 1 << 0;
const CLR_OWN: u32 = 1 << 1;
/// MT_HIF_REMAP_L1 (bits 31:16 select a 64 KiB window of the chip's map) and the BAR0 offset that window answers at
/// (mt7925/regs.h:70-74).
const HIF_REMAP_L1: u64 = 0x0015_5024;
const REMAP_BASE_L1: u64 = 0x0013_0000;
/// MT_HW_CHIPID / MT_HW_REV (mt792x_regs.h:394-395); both in the 0x7001 window, so one selector write serves both.
const HW_CHIPID: u32 = 0x7001_0200;
const HW_REV: u32 = 0x7001_0204;
/// Per try: one LPCTL write, then OWN_SYNC polled up to OWN_TRY_MS; OWN_TRIES tries = the 200 ms bound. The read cap
/// ends a try should the tick stall.
const OWN_TRY_MS: u64 = 50;
const OWN_TRIES: u32 = 4;
const OWN_READ_CAP: u32 = 1_000_000;
const S_S2: u8 = 4;

fn s2() {
    STATE.store(S_PARKED, Ordering::Relaxed);
    let held = *PROBE.lock();
    let Some(p) = held else { return };
    let plat = crate::hw::platform();
    if plat != crate::hw::Platform::ZenbookUm5606w {
        serial_println!("[mt7925] s2 -> NOT ATTEMPTED why=platform={}", plat.token());
        return;
    }
    let why = if !p.mem {
        Some("mem-off")
    } else if p.bar0 == 0 {
        Some("unassigned")
    } else {
        match p.regs {
            None => Some("unmapped"),
            Some((l, _)) if l == u32::MAX => Some("dead"),
            Some(_) => None,
        }
    };
    if let Some(w) = why {
        serial_println!("[mt7925] s2 -> NOT ATTEMPTED why={}", w);
        return;
    }
    if !map_page(p.bar0 + (HIF_REMAP_L1 & !0xfff)) || !map_page(p.bar0 + REMAP_BASE_L1) {
        serial_println!("[mt7925] s2 -> NOT ATTEMPTED why=unmapped");
        return;
    }
    let lp = p.bar0 + CONN_HOST_CSR_TOP + LPCTL;
    // SAFETY: LPCTL sits in the conn_host_csr_top page S1's `read_csr_top` mapped and proved present (regs is Some).
    let now = unsafe { core::ptr::read_volatile(lp as *const u32) };
    let took = now & OWN_SYNC != 0;
    if took {
        let (ok, last, waited) = own(lp, CLR_OWN, false);
        if !ok {
            let rel = if own(lp, SET_OWN, true).0 { "fw" } else { "timeout" };
            serial_println!("[mt7925] own -> REFUSED lpctl={:#010x} waited={} rel={}", last, waited, rel);
            return;
        }
    }
    let (chip, rev, restored) = read_chip(p.bar0);
    let rel = if !took {
        "found"
    } else if own(lp, SET_OWN, true).0 {
        "fw"
    } else {
        "timeout"
    };
    serial_println!(
        "[mt7925] s2 own={} chip={:#x} rev={:#x} remap={} rel={} -> parked",
        if took { "ok" } else { "found" }, chip, rev, if restored { "restored" } else { "stuck" }, rel
    );
    if restored && rel != "timeout" && chip != u32::MAX {
        STATE.store(S_S3, Ordering::Relaxed); // MT7925S3 (Z15): S3 runs on the next pass, only after this clean S2 line
    }
}

/// One LPCTL handshake (linux's drv/fw pmctrl shape): per try write `bit`, then poll OWN_SYNC until it reads
/// `want_fw` (set = firmware owns) for up to OWN_TRY_MS; at most OWN_TRIES writes. (reached, last LPCTL, ms waited).
fn own(lp: u64, bit: u32, want_fw: bool) -> (bool, u32, u64) {
    let t0 = crate::arch::ms();
    let mut last = 0u32;
    for _ in 0..OWN_TRIES {
        // SAFETY: LPCTL in the mapped, present conn_host_csr_top page; the ownership handshake write linux issues.
        unsafe { core::ptr::write_volatile(lp as *mut u32, bit) };
        let t = crate::arch::ms();
        let mut reads = 0u32;
        while crate::arch::ms().saturating_sub(t) < OWN_TRY_MS && reads < OWN_READ_CAP {
            // SAFETY: as above; a status read.
            last = unsafe { core::ptr::read_volatile(lp as *const u32) };
            if (last & OWN_SYNC != 0) == want_fw {
                return (true, last, crate::arch::ms().saturating_sub(t0));
            }
            reads += 1;
            core::hint::spin_loop();
        }
    }
    (false, last, crate::arch::ms().saturating_sub(t0))
}

/// `read_csr_top`'s map shape for one more BAR0 page: a page-table edit, then present proved before any access.
fn map_page(page: u64) -> bool {
    crate::arch::memory::map_mmio_window(page, 0x1000);
    crate::arch::memory::translate(page).is_some()
}

/// CHIPID and REV through the L1 remap (`l1`): the 0x7001 window, the two reads. (chip, rev, selector restored).
fn read_chip(bar0: u64) -> (u32, u32, bool) {
    let ((chip, rev), restored) = l1(bar0, HW_CHIPID, |win| {
        // SAFETY: the window page is mapped uncacheable and proved present (`map_page`); the driver owns the chip (the
        // handshake above, or found so); two status reads.
        unsafe {
            let chip = core::ptr::read_volatile((win + (HW_CHIPID & 0xffff) as u64) as *const u32);
            let rev = core::ptr::read_volatile((win + (HW_REV & 0xffff) as u64) as *const u32);
            (chip, rev)
        }
    });
    (chip, rev, restored)
}

/// The ONE L1 remap access (MT7925S3 hoisted it out of `read_chip`; callers `read_chip` and S3's EMI write): the selector
/// saved, `chip_addr`'s 64 KiB window selected (read back to push it), `f` run on the window's BAR0 base, the selector
/// restored and read back. (f's result, selector restored).
fn l1<T>(bar0: u64, chip_addr: u32, f: impl FnOnce(u64) -> T) -> (T, bool) {
    let sel = bar0 + HIF_REMAP_L1;
    let win = bar0 + REMAP_BASE_L1;
    // SAFETY: the selector page is mapped uncacheable and proved present (`map_page`); the driver owns the chip; the
    // selector is put back before this block ends.
    unsafe {
        let backup = core::ptr::read_volatile(sel as *const u32);
        core::ptr::write_volatile(sel as *mut u32, (backup & 0xffff) | (chip_addr & 0xffff_0000));
        let _ = core::ptr::read_volatile(sel as *const u32); // push the selector write (linux reads it back)
        let out = f(win);
        core::ptr::write_volatile(sel as *mut u32, backup);
        let restored = core::ptr::read_volatile(sel as *const u32) == backup;
        (out, restored)
    }
}

// ── MT7925S3 (zenbook-ledger Z15) — S3: EMI SLPPROT, the WFSYS reset, WFDMA0 to idle, two rings, WFDMA0 enabled ───────
// Once, on the storage-ready pass after a clean S2 line (`s2` hands to S_S3 only when the selector was restored, own was
// given back and CHIPID read live). Linux's probe order after the chip id (mt7925/pci.c:392-407): EMI_CTL SLPPROT_EN,
// `mt792x_wfsys_reset`, then `mt7925_dma_init` = `mt792x_dma_disable(force)`, the queues, `mt792x_dma_enable`. S3 arms
// ONE TX ring (FWDL, ring 16) and ONE RX ring (MCU events, ring 0), depth 32, every descriptor DMA_DONE, no RX buffer
// handed (CPU index = DMA index = 0) and no TX descriptor posted, so the engine has nothing to fetch; PCI bus master
// stays OFF (no config write), so no DMA can happen at all. No interrupt enable, no firmware, no packet: S4's.

const S_S3: u8 = 5;
/// MT_HW_EMI_CTL and SLPPROT_EN (mt792x_regs.h:397-398): not in the fixed map, so through the L1 remap (`l1`).
const HW_EMI_CTL: u32 = 0x1801_1100;
const EMI_SLPPROT_EN: u32 = 1 << 1;
/// MT_WFSYS_SW_RST_B 0x7c000140 -> BAR0+0xf0140 (fixed map, CONN_INFRA); RST_B bit 0, INIT_DONE bit 4 (mt792x_regs.h:484-485).
const WFSYS_RST: u64 = 0x000f_0140;
const WFSYS_SW_RST_B: u32 = 1 << 0;
const WFSYS_SW_INIT_DONE: u32 = 1 << 4;
/// mt792x_dma.c:361-366: RST_B held low 50 ms, INIT_DONE polled up to 500 ms.
const WFSYS_HOLD_MS: u64 = 50;
const WFSYS_DONE_MS: u64 = 500;
/// WFDMA0 at BAR0+0xd4000 (mt792x_regs.h:260-358); every register S3 touches but DMASHDL sits in this one page.
const WFDMA0: u64 = 0x000d_4000;
const WFDMA_RST: u64 = WFDMA0 + 0x100;
const RST_LOGIC: u32 = 1 << 4;
const RST_DMASHDL_ALL: u32 = 1 << 5;
const GLO_CFG: u64 = WFDMA0 + 0x208;
const GLO_TX_EN: u32 = 1 << 0;
const GLO_TX_BUSY: u32 = 1 << 1;
const GLO_RX_EN: u32 = 1 << 2;
const GLO_RX_BUSY: u32 = 1 << 3;
const GLO_CHAIN_EN: u32 = 1 << 15;
const GLO_OMIT_RX_PFET2: u32 = 1 << 21;
const GLO_OMIT_RX: u32 = 1 << 27;
const GLO_OMIT_TX: u32 = 1 << 28;
/// `mt792x_dma_enable`'s feature set (dma.c:139-148): TX_WB_DDONE b6, FIFO_LITTLE_ENDIAN b12, CLK_GAT_DIS b30,
/// OMIT_TX_INFO b28, DMA_SIZE=3 (b5:4), FIFO_DIS_CHECK b11, RX_WB_DDONE b13, CSR_DISP_BASE_PTR_CHAIN_EN b15,
/// OMIT_RX_INFO_PFET2 b21.
const GLO_FEATURES: u32 =
    (1 << 6) | (1 << 12) | (1 << 30) | GLO_OMIT_TX | (3 << 4) | (1 << 11) | (1 << 13) | GLO_CHAIN_EN | GLO_OMIT_RX_PFET2;
const RST_DTX_PTR: u64 = WFDMA0 + 0x20c;
const RST_DRX_PTR: u64 = WFDMA0 + 0x280;
const GLO_EXT0: u64 = WFDMA0 + 0x2b0;
const EXT0_TX_DMASHDL_EN: u32 = 1 << 6;
/// MT_UWFDMA0_GLO_CFG_EXT1 0x7c0242b4 -> BAR0+0xd42b4 (fixed map, CONN_INFRA wfdma); bit 28 set on the 7925 (dma.c:153-154).
const GLO_EXT1: u64 = WFDMA0 + 0x2b4;
const EXT1_B28: u32 = 1 << 28;
const PRI_DLY_INT_CFG0: u64 = WFDMA0 + 0x2f0;
/// MT_DMASHDL_SW_CONTROL 0x7c026004 -> BAR0+0xd6004 (fixed map) and its BYPASS bit (mt792x_regs.h:406-408).
const DMASHDL_SW_CONTROL: u64 = 0x000d_6004;
const DMASHDL_BYPASS: u32 = 1 << 28;
/// Ring registers, 16 bytes per ring {base, count, CPU index, DMA index} (dma.c:188-218): TX at +0x300, RX events at +0x500.
const TX_RING_BASE: u64 = WFDMA0 + 0x300;
const RX_EVENT_RING_BASE: u64 = WFDMA0 + 0x500;
const RING_REGS: u64 = 0x10;
/// MT7925_TXQ_FWDL = 16, MT7925_RXQ_MCU_WM = 0 (mt7925.h:90-100).
const TXQ_FWDL: u64 = 16;
const RXQ_MCU: u64 = 0;
/// S3's depth (linux: FWDL 128, events 512); 16-byte descriptors {buf0, ctrl, buf1, info}, ctrl = DMA_DONE at reset.
const RING_DEPTH: u32 = 32;
const DESC_BYTES: u64 = 16;
const RING_BYTES: usize = RING_DEPTH as usize * DESC_BYTES as usize;
const DMA_DONE: u32 = 1 << 31;
/// `mt792x_dma_disable`'s busy poll bound (dma.c:263-266).
const DMA_IDLE_MS: u64 = 100;
/// The ring's single 32-bit base register and linux's DMA_BIT_MASK(32) (pci.c:331): a ring at or past 4 GiB is refused.
const DMA_LIMIT: u64 = 0x1_0000_0000;
const fn pf(base: u32, depth: u32) -> u32 {
    (base << 16) | depth
}
/// The 7925 prefetch table, linux's values and order (`mt792x_dma_prefetch`, dma.c:93-105): RX 0-3, TX 0-3, 15, 16.
const PREFETCH: [(u64, u32); 10] = [
    (WFDMA0 + 0x680, pf(0x0000, 0x4)),
    (WFDMA0 + 0x684, pf(0x0040, 0x4)),
    (WFDMA0 + 0x688, pf(0x0080, 0x4)),
    (WFDMA0 + 0x68c, pf(0x00c0, 0x4)),
    (WFDMA0 + 0x600, pf(0x0100, 0x10)),
    (WFDMA0 + 0x604, pf(0x0200, 0x10)),
    (WFDMA0 + 0x608, pf(0x0300, 0x10)),
    (WFDMA0 + 0x60c, pf(0x0400, 0x10)),
    (WFDMA0 + 0x63c, pf(0x0500, 0x4)),
    (WFDMA0 + 0x640, pf(0x0540, 0x4)),
];

fn s3() {
    STATE.store(S_PARKED, Ordering::Relaxed);
    let held = *PROBE.lock();
    let Some(p) = held else { return };
    let b = p.bar0;
    // S2 mapped the selector page and the window's first page; EMI_CTL (+0x1100) sits in the window's second page.
    let mapped = map_page(b + WFDMA0)
        && map_page(b + (DMASHDL_SW_CONTROL & !0xfff))
        && map_page(b + (WFSYS_RST & !0xfff))
        && map_page(b + REMAP_BASE_L1 + (HW_EMI_CTL & 0xf000) as u64);
    if !mapped {
        serial_println!("[mt7925] s3 -> NOT ATTEMPTED why=unmapped");
        return;
    }
    let lp = b + CONN_HOST_CSR_TOP + LPCTL;
    let took = r32(lp) & OWN_SYNC != 0;
    if took {
        let (ok, last, _) = own(lp, CLR_OWN, false);
        if !ok {
            let rel = if own(lp, SET_OWN, true).0 { "fw" } else { "timeout" };
            serial_println!("[mt7925] s3 -> NOT ATTEMPTED why=own lpctl={:#010x} rel={}", last, rel);
            return;
        }
    }
    let (emi, restored) = l1(b, HW_EMI_CTL, |win| {
        let a = win + (HW_EMI_CTL & 0xffff) as u64;
        w32(a, r32(a) | EMI_SLPPROT_EN); // linux's rmw_field(SLPPROT_EN, 1)
        r32(a)
    });
    let (reset_ok, waited, rst) = wfsys(b);
    serial_println!(
        "[mt7925] s3 emi={:#010x} remap={} wfsys reset={} waited={} rst={:#010x}",
        emi, if restored { "restored" } else { "stuck" }, if reset_ok { "ok" } else { "timeout" }, waited, rst
    );
    let why = if !reset_ok { Some(String::from("wfsys-timeout")) } else { dma_off(b).err() };
    if let Some(w) = why {
        let rel = give_back(lp, took);
        serial_println!("[mt7925] s3 wfdma -> NOT ATTEMPTED why={} rel={}", w, rel);
        return;
    }
    let (tx, rx) = (ring(), ring());
    let refuse = if tx == 0 || rx == 0 {
        Some("no-memory")
    } else if tx + RING_BYTES as u64 > DMA_LIMIT || rx + RING_BYTES as u64 > DMA_LIMIT {
        Some("dma-high")
    } else {
        None
    };
    if let Some(w) = refuse {
        let rel = give_back(lp, took);
        serial_println!("[mt7925] s3 wfdma -> NOT ATTEMPTED why={} rel={}", w, rel);
        return; // nothing written to a ring register; the allocations are never freed (the BCM4331 rings' rule)
    }
    let mut rb = true;
    for (q, pa) in [(b + TX_RING_BASE + TXQ_FWDL * RING_REGS, tx), (b + RX_EVENT_RING_BASE + RXQ_MCU * RING_REGS, rx)] {
        w32(q, pa as u32); // base
        w32(q + 4, RING_DEPTH); // count
        w32(q + 8, 0); // CPU index: no TX posted, no RX buffer handed
        rb &= r32(q) == pa as u32 && r32(q + 4) == RING_DEPTH;
    }
    let glo = dma_on(b);
    let rel = give_back(lp, took);
    serial_println!(
        "[mt7925] s3 wfdma tx={:#x} rx={:#x} cnt={} rb={} glo={:#010x} rel={} -> parked",
        tx, rx, RING_DEPTH, if rb { "ok" } else { "bad" }, glo, rel
    );
}

/// A BAR0 register read; only on pages `map_page` proved present this boot (S2's, S3's).
fn r32(a: u64) -> u32 {
    // SAFETY: callers pass addresses in BAR0 pages mapped uncacheable and proved present (`map_page`).
    unsafe { core::ptr::read_volatile(a as *const u32) }
}

/// A BAR0 register write; as `r32`, and only with the driver owning the chip.
fn w32(a: u64, v: u32) {
    // SAFETY: as `r32`; every write is one of S3's named, bounded writes.
    unsafe { core::ptr::write_volatile(a as *mut u32, v) }
}

/// Spin until `done` or `ms` elapse, at most OWN_READ_CAP rounds should the tick stall; `done` reads a register on every
/// round, so the cap is a time bound too. (done, ms waited).
fn spin_until(ms: u64, mut done: impl FnMut() -> bool) -> (bool, u64) {
    let t = crate::arch::ms();
    let mut n = 0u32;
    while crate::arch::ms().saturating_sub(t) < ms && n < OWN_READ_CAP {
        if done() {
            return (true, crate::arch::ms().saturating_sub(t));
        }
        n += 1;
        core::hint::spin_loop();
    }
    (done(), crate::arch::ms().saturating_sub(t))
}

/// `mt792x_wfsys_reset` (dma.c:357-370): RST_B low, 50 ms, RST_B high, INIT_DONE polled up to 500 ms.
/// (INIT_DONE seen, ms waited for it, the register's last value).
fn wfsys(b: u64) -> (bool, u64, u32) {
    let a = b + WFSYS_RST;
    w32(a, r32(a) & !WFSYS_SW_RST_B);
    let _ = spin_until(WFSYS_HOLD_MS, || {
        let _ = r32(a); // the hold reads the register each round, so the read cap cannot cut it short
        false
    });
    w32(a, r32(a) | WFSYS_SW_RST_B);
    let (ok, waited) = spin_until(WFSYS_DONE_MS, || r32(a) & WFSYS_SW_INIT_DONE != 0);
    (ok, waited, r32(a))
}

/// `mt792x_dma_disable(force)` (dma.c:253-285): the EN / chain / omit bits cleared, both BUSY bits polled clear up to
/// 100 ms, DMASHDL off and bypassed, the LOGIC and DMASHDL_ALL resets pulsed. Err = the refusal word.
fn dma_off(b: u64) -> Result<(), String> {
    let g = b + GLO_CFG;
    w32(g, r32(g) & !(GLO_TX_EN | GLO_RX_EN | GLO_CHAIN_EN | GLO_OMIT_TX | GLO_OMIT_RX | GLO_OMIT_RX_PFET2));
    let (idle, _) = spin_until(DMA_IDLE_MS, || r32(g) & (GLO_TX_BUSY | GLO_RX_BUSY) == 0);
    if !idle {
        return Err(alloc::format!("dma-busy glo={:#010x}", r32(g)));
    }
    let e0 = b + GLO_EXT0;
    w32(e0, r32(e0) & !EXT0_TX_DMASHDL_EN);
    let sc = b + DMASHDL_SW_CONTROL;
    w32(sc, r32(sc) | DMASHDL_BYPASS);
    let rs = b + WFDMA_RST;
    w32(rs, r32(rs) & !(RST_LOGIC | RST_DMASHDL_ALL));
    w32(rs, r32(rs) | RST_LOGIC | RST_DMASHDL_ALL);
    Ok(())
}

/// One ring's descriptors: zeroed, page-aligned, below 4 GiB, heap address = bus address, every ctrl word DMA_DONE
/// (linux's queue reset, dma.c:209-211). 0 on failure, a ring past 4 GiB included (DMAALLOCXHCI B988: the allocator
/// refuses it, so S3 says `no-memory` and its `dma-high` arm is unreachable). Never freed (DMAALLOCONE B986).
fn ring() -> u64 {
    let p = crate::arch::memory::dma::alloc(RING_BYTES, 4096, true).filter(|b| b.pa == b.va).map_or(0, |b| b.va);
    if p != 0 {
        for i in 0..RING_DEPTH as u64 {
            // SAFETY: inside the RING_BYTES block just allocated; descriptor i's ctrl word at +4.
            unsafe { core::ptr::write_volatile((p + i * DESC_BYTES + 4) as *mut u32, DMA_DONE) };
        }
    }
    p
}

/// `mt792x_dma_enable` (dma.c:126-158) up to the interrupt enables, which S3 does not do: the prefetch table, the DMA
/// indices reset, delay-int 0, the feature bits, then TX/RX_DMA_EN, then EXT1 bit 28. Returns GLO_CFG read back.
fn dma_on(b: u64) -> u32 {
    for (o, v) in PREFETCH {
        w32(b + o, v);
    }
    w32(b + RST_DTX_PTR, !0);
    w32(b + RST_DRX_PTR, !0);
    w32(b + PRI_DLY_INT_CFG0, 0);
    let g = b + GLO_CFG;
    w32(g, r32(g) | GLO_FEATURES);
    w32(g, r32(g) | GLO_TX_EN | GLO_RX_EN);
    let e1 = b + GLO_EXT1;
    w32(e1, r32(e1) | EXT1_B28);
    r32(g)
}

/// Firmware-own given back only when this rung took it (S2's release shape). The `rel=` word.
fn give_back(lp: u64, took: bool) -> &'static str {
    if !took {
        "found"
    } else if own(lp, SET_OWN, true).0 {
        "fw"
    } else {
        "timeout"
    }
}
