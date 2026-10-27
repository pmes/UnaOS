//! CHARTER: Kernel — driver (the MediaTek MT7925 Wi-Fi 7 radio's S1 probe, zenbook-ledger Z5, S2 driver-own and the chip id, Z11, and S3 the WFSYS reset and the WFDMA rings, Z15, and S4 the firmware upload, Z17, and S5 the NIC capability and MAC, Z23, and S6 the radio backend registered with the data rings armed, Z29; no CODEX §2 handler owns a NIC driver, the BCM4331 `wifi/**` sits in the same seam)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! MT7925 — S1, the probe (ZENBOOKMT7925, zenbook-ledger Z5). The ladder is
//! `docs/dev/OS/06_NETWORK_STACK/mt7925.md`; S2 (MT7925S2, Z11), S3 (MT7925S3, Z15), S4 (MT7925S4, Z17), S5 (MT7925S5, Z23) and S6 (MT7925S6, Z29) are the five file-tail blocks; S7..S10 are owed there by name.
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
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};

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
        S_S4 => s4(), // MT7925S4 (zenbook-ledger Z17): the pass after a clean S3 line (`s3_done`)
        S_S5 => s5(), // MT7925S5 (zenbook-ledger Z23): the pass after a kept S4 line (`s4_done`)
        S_S6 => s6(), // MT7925S6 (zenbook-ledger Z29): the pass after a clean S5 line (`s5` hands to it with the channel parked)
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
    let (fs, ents) = match fw_dir() {
        Ok(d) => d,
        Err(w) => {
            out.word = String::from(w);
            return out;
        }
    };
    let mut present = 0usize;
    for (i, (role, file)) in SET.iter().enumerate() {
        let Some((verdict, hex, size, _)) = fw_read(&fs, &ents, i) else { continue };
        present += 1;
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

/// The program-source FAT mounted and `FW/MT7925/` listed (MT7925S4, Z17, hoisted out of `fw_check` so S1 and S4 share
/// ONE reader). Err = the `fw=` word: `no-esp` (no mount or no root) or `absent` (no FW/MT7925 directory).
fn fw_dir() -> Result<(crate::fs::fat::FatFs, Vec<crate::fs::fat::DirEntry>), &'static str> {
    let fs = crate::fs::fat::mount_program_source().map_err(|_| "no-esp")?;
    let root = fs.read_root().map_err(|_| "no-esp")?;
    let d0 = sub(&fs, &root, DIR0).ok_or("absent")?;
    let ents = sub(&fs, &d0, DIR1).ok_or("absent")?;
    Ok((fs, ents))
}

/// Member `i` of SET read whole and digested against its pin (MT7925S4 hoisted it out of `fw_check`; S1 reads the
/// verdict, S4 uploads the bytes only when it is `ok`). None = not on the volume; else (verdict, sha256 hex or `-`,
/// directory size, the bytes read).
fn fw_read(fs: &crate::fs::fat::FatFs, ents: &[crate::fs::fat::DirEntry], i: usize) -> Option<(&'static str, String, usize, Vec<u8>)> {
    let (role, file) = SET[i];
    let de = ents.iter().find(|e| !e.is_dir && name_is(e, file))?;
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
    Some((verdict, hex, size, buf))
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
        rb &= arm(q, pa, 0); // CPU index 0: no TX posted, no RX buffer handed (MT7925S4 hoisted the body into `arm`)
    }
    let glo = dma_on(b);
    let rel = give_back(lp, took);
    serial_println!(
        "[mt7925] s3 wfdma tx={:#x} rx={:#x} cnt={} rb={} glo={:#010x} rel={} -> parked",
        tx, rx, RING_DEPTH, if rb { "ok" } else { "bad" }, glo, rel
    );
    s3_done(tx, rx, rb, glo, rel); // MT7925S4 (Z17): the rings recorded, S4 on the next pass only after a clean S3 line
}

/// One ring's four-register arm (MT7925S4 hoisted it out of `s3`'s loop; S3 calls it with CPU index 0, S4 for rings 16, 15
/// and 0): base, count, CPU index, then base and count read back. True when both read back as written.
fn arm(q: u64, pa: u64, cidx: u32) -> bool {
    w32(q, pa as u32); // base
    w32(q + 4, RING_DEPTH); // count
    w32(q + 8, cidx); // CPU index
    r32(q) == pa as u32 && r32(q + 4) == RING_DEPTH
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

// ── MT7925S4 (zenbook-ledger Z17) — S4: the patch and the RAM code uploaded over the MCU, the N9 started ──────────────
// Once, on the storage-ready pass after a clean S3 line (`s3_done` hands to S_S4 only when S3's ring read-backs held, the
// ownership went back and WFDMA0 read back enabled). Linux's `mt7925e_mcu_init` / `mt792x_load_firmware`: take driver-own
// again, re-arm the rings (FWDL 16, the MCU command ring WM 15, the event ring 0 with buffers), enable the DMA, then
// patch (semaphore, per-section init_download + raw scatter, finish, release) and RAM code (per-region init_download +
// scatter, FW_START) and poll N9 ready. Commands ride ring 15 with the legacy 64-byte MCU TXD; the scatter chunks ride
// ring 16 raw and unacked (each is waited on by its TX completion); events are read from ring 0. PCI bus master goes ON
// (the ONE config write) and is put back unless FW_START was acked AND N9 reads ready. Hardware facts only; no code copied.

const S_S4: u8 = 6;
/// The ring-16 and ring-0 descriptor memory S3 allocated: written once by `s3_done`, read once by `s4`.
static S4_TX16: AtomicU64 = AtomicU64::new(0);
static S4_RX0: AtomicU64 = AtomicU64::new(0);

/// MT7925_TXQ_MCU_WM = 15 (mt7925.h:92).
const TXQ_WM: u64 = 15;
/// MT_PCIE_MAC_PM (BAR0+0x10194) and its L0S_DIS bit (pci_mcu.c:45).
const PCIE_MAC_PM: u64 = 0x1_0194;
const PM_L0S_DIS: u32 = 1 << 8;
/// MCU command ids (mt76_connac_mcu.h).
const CID_TARGET_ADDR: u8 = 0x01;
const CID_FW_START: u8 = 0x02;
const CID_PATCH_START: u8 = 0x05;
const CID_PATCH_FINISH: u8 = 0x07;
const CID_SEM: u8 = 0x10;
/// PATCH_SEM_CONTROL operations and answers.
const SEM_GET: u32 = 1;
const SEM_RELEASE: u32 = 0;
const SEM_IS_DL: u8 = 1;
const SEM_GOT: u8 = 2;
const SEM_RELEASED: u8 = 3;
/// Download mode bits (DL_MODE_*, DL_CONFIG_ENCRY_MODE_SEL).
const DL_ENCRYPT: u32 = 1 << 0;
const DL_KEY_IDX: u32 = 0x6;
const DL_RESET_SEC_IV: u32 = 1 << 3;
const DL_ENCRY_MODE_SEL: u32 = 1 << 6;
const DL_NEED_RSP: u32 = 1 << 31;
/// RAM region feature bits (FW_FEATURE_*).
const FEAT_ENCRYPT: u32 = 1 << 0;
const FEAT_ENCRY_MODE: u32 = 1 << 4;
const FEAT_OVERRIDE_ADDR: u32 = 1 << 5;
const FEAT_NON_DL: u32 = 1 << 6;
/// Patch image: 96-byte header (n_region BE at +0x2c), 64-byte sections; RAM image: 36-byte trailer, 40-byte regions.
const PATCH_HDR: usize = 96;
const PATCH_SEC: usize = 64;
const RAM_TRAILER: usize = 36;
const RAM_REGION: usize = 40;
const MAX_REGIONS: usize = 16;
/// The command TXD (connac2 mcu_txd): 64 bytes, payload after it.
const TXD_BYTES: usize = 64;
/// TX descriptor ctrl: LAST_SEC0 (bit 30); SD_LEN0 sits in bits 29:16.
const LAST_SEC0: u32 = 1 << 30;
/// Event ring: 31 buffers of 2048 bytes handed from one 64 KiB block of 32 slots; one descriptor always stays unhanded
/// (a full ring), and `recycle` (MT7925S5, Z23) re-hands the slot behind each consumed event, the 32nd slot first.
const RX_BUFS: u32 = 31;
const RX_BUF_BYTES: u64 = 2048;
const RX_BLOCK: usize = 65536;
/// FW_SCATTER chunk size; waits: an acked command's event, one chunk's TX completion, N9 ready after FW_START.
const CHUNK: usize = 4096;
const EVENT_MS: u64 = 3000;
const TXDONE_MS: u64 = 100;
const N9_MS: u64 = 2000;

/// The device-facing state of the MCU channel S4 opens; owned by `s4_run`'s frame, then (MT7925S5, Z23) handed to S5
/// through `MCU_LINK` only when FW_START was acked and N9 read ready.
struct Hw {
    q16: u64,
    q15: u64,
    /// Ring 0's register block (MT7925S5: `recycle` moves its CPU index).
    q0: u64,
    /// Ring 0's descriptor memory, the WM ring (MT7925S5: so S5 can re-arm ring 15), the command page, the event-buffer block.
    rx0: u64,
    wm: u64,
    page: u64,
    rxb: u64,
    /// CPU indices of rings 16 and 15, the next event descriptor, the last command sequence (1..15).
    fi: u32,
    wi: u32,
    ri: u32,
    seq: u8,
    /// The last matched event's bytes, rxd included (MT7925S5: copied before its buffer is re-handed).
    last: Vec<u8>,
}

/// What `s4` says at the end.
struct Rep {
    held: &'static str,
    patch: String,
    ram: usize,
    sent: bool,
    fw_start: &'static str,
    n9: &'static str,
    saved: Option<u16>,
}

/// S3 calls this at its end: record the two ring addresses and hand to S4 only on a clean S3 line.
fn s3_done(tx: u64, rx: u64, rb: bool, glo: u32, rel: &str) {
    if rb && rel != "timeout" && glo & (GLO_TX_EN | GLO_RX_EN) == (GLO_TX_EN | GLO_RX_EN) {
        S4_TX16.store(tx, Ordering::Relaxed);
        S4_RX0.store(rx, Ordering::Relaxed);
        STATE.store(S_S4, Ordering::Relaxed);
    }
}

fn be32(d: &[u8], o: usize) -> Option<u32> {
    let s = d.get(o..o.checked_add(4)?)?;
    Some(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

fn le32(d: &[u8], o: usize) -> Option<u32> {
    let s = d.get(o..o.checked_add(4)?)?;
    Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// True when every byte of `v` lies below 4 GiB (the identity-mapped heap address is the bus address).
fn low(v: &[u8]) -> bool {
    (v.as_ptr() as u64).checked_add(v.len() as u64).map_or(false, |e| e <= DMA_LIMIT)
}

/// `why` for a failed acked command: a full event ring keeps its own word, anything else the step's word.
fn why_of(e: &'static str, w: String) -> String {
    if e == "rx-full" { String::from(e) } else { w }
}

fn s4() {
    STATE.store(S_PARKED, Ordering::Relaxed);
    let held = *PROBE.lock();
    let Some(p) = held else { return };
    let b = p.bar0;
    let lp = b + CONN_HOST_CSR_TOP + LPCTL;
    let mapped = map_page(b + WFDMA0) && map_page(b + (DMASHDL_SW_CONTROL & !0xfff)) && map_page(b + (PCIE_MAC_PM & !0xfff));
    if !mapped {
        serial_println!("[mt7925] s4 -> NOT ATTEMPTED why=unmapped bm=off rel=found");
        return;
    }
    let took = r32(lp) & OWN_SYNC != 0;
    if took {
        let (ok, last, _) = own(lp, CLR_OWN, false);
        if !ok {
            let rel = if own(lp, SET_OWN, true).0 { "fw" } else { "timeout" };
            serial_println!("[mt7925] s4 -> NOT ATTEMPTED why=own lpctl={:#010x} bm=off rel={}", last, rel);
            return;
        }
    }
    // The firmware bytes live in this frame: dropped only after every descriptor completed or bus master is back off.
    let mut hold: Vec<u8> = Vec::new();
    let mut rep = Rep { held: "lost", patch: String::from("-"), ram: 0, sent: false, fw_start: "-", n9: "-", saved: None };
    let res = s4_run(&p, &mut hold, &mut rep);
    let keep = res.is_ok() && rep.sent && rep.fw_start == "ok" && rep.n9 == "ready";
    let bm = match rep.saved {
        None => "off",
        Some(_) if keep => "on",
        Some(saved) => {
            // SAFETY: the one restore write, the COMMAND word read before bus master went on, to the function the walk recorded.
            unsafe { crate::arch::pci::write_config_16(p.bus, p.dev, p.func, PCI_COMMAND, saved) };
            "restored"
        }
    };
    let rel = give_back(lp, took);
    if rep.sent {
        serial_println!(
            "[mt7925] s4 held={} patch={} ram={} fw_start={} n9={} bm={} rel={} -> parked",
            rep.held, rep.patch, rep.ram, rep.fw_start, rep.n9, bm, rel
        );
    } else {
        let why = match res {
            Err(w) => w,
            Ok(()) => String::from("?"),
        };
        serial_println!("[mt7925] s4 -> NOT ATTEMPTED why={} bm={} rel={}", why, bm, rel);
    }
    drop(hold);
}

/// Everything between driver-own and give-back. Err = the `why=` word; `rep` carries what was learned either way.
fn s4_run(p: &Probe, hold: &mut Vec<u8>, rep: &mut Rep) -> Result<(), String> {
    let b = p.bar0;
    let (tx16, rx0) = (S4_TX16.load(Ordering::Relaxed), S4_RX0.load(Ordering::Relaxed));
    let q16 = b + TX_RING_BASE + TXQ_FWDL * RING_REGS;
    let q15 = b + TX_RING_BASE + TXQ_WM * RING_REGS;
    let q0 = b + RX_EVENT_RING_BASE + RXQ_MCU * RING_REGS;
    rep.held = if r32(q16) as u64 == tx16 && r32(q0) as u64 == rx0 { "kept" } else { "lost" };
    let pm = b + PCIE_MAC_PM;
    w32(pm, r32(pm) | PM_L0S_DIS);
    let (fs, ents) = fw_dir().map_err(|_| String::from("no-fw"))?;
    let patch_bytes = match fw_read(&fs, &ents, 0) {
        Some(("ok", _, _, v)) => v,
        _ => return Err(String::from("no-fw")),
    };
    let wm = ring();
    let page = crate::arch::memory::dma::ident(4096, 4096);
    let rxb = crate::arch::memory::dma::ident(RX_BLOCK, 4096);
    if wm == 0 || page == 0 || rxb == 0 || tx16 == 0 || rx0 == 0 {
        return Err(String::from("no-memory"));
    }
    if wm + RING_BYTES as u64 > DMA_LIMIT || page + 4096 > DMA_LIMIT || rxb + RX_BLOCK as u64 > DMA_LIMIT || !low(&patch_bytes) {
        return Err(String::from("dma-high"));
    }
    *hold = patch_bytes;
    dma_off(b)?;
    hand_rx(rx0, rxb); // MT7925S5 (Z23) hoisted the 31-buffer hand into `hand_rx`, unchanged, so S4 and S5 share it
    let mut rb = arm(q16, tx16, 0);
    rb &= arm(q15, wm, 0);
    rb &= arm(q0, rx0, RX_BUFS);
    let glo = dma_on(b);
    if !rb || glo & (GLO_TX_EN | GLO_RX_EN) != (GLO_TX_EN | GLO_RX_EN) {
        return Err(String::from("rb"));
    }
    // SAFETY: a config-space read of the function the boot's walk recorded.
    let saved = unsafe { crate::arch::pci::read_config_16(p.bus, p.dev, p.func, PCI_COMMAND) };
    crate::drivers::pci::PciScanner::enable_bus_master(p.bus, p.dev, p.func); // the ONE config write
    rep.saved = Some(saved);
    let mut h = Hw { q16, q15, q0, rx0, wm, page, rxb, fi: 0, wi: 0, ri: 0, seq: 0, last: Vec::new() };
    let sem = cmd(&mut h, CID_SEM, &SEM_GET.to_le_bytes()).map_err(|e| why_of(e, String::from("sem")))?;
    match sem {
        SEM_IS_DL => rep.patch = String::from("resident"),
        SEM_GOT => {
            patch(&mut h, hold)?;
            rep.patch = alloc::format!("{}", hold.len());
        }
        _ => return Err(String::from("sem")),
    }
    let ram_bytes = match fw_read(&fs, &ents, 1) {
        Some(("ok", _, _, v)) => v,
        _ => return Err(String::from("no-fw")),
    };
    if !low(&ram_bytes) {
        return Err(String::from("dma-high"));
    }
    *hold = ram_bytes; // the patch's transfers are complete: its bytes go
    rep.ram = hold.len();
    let (option, addr) = ram(&mut h, hold)?;
    rep.sent = true;
    let mut pl = [0u8; 8];
    pl[0..4].copy_from_slice(&option.to_le_bytes());
    pl[4..8].copy_from_slice(&addr.to_le_bytes());
    rep.fw_start = if cmd(&mut h, CID_FW_START, &pl).is_ok() { "ok" } else { "timeout" };
    let misc = b + CONN_HOST_CSR_TOP + CONN_ON_MISC;
    let (ready, _) = spin_until(N9_MS, || r32(misc) & FW_N9_RDY == FW_N9_RDY);
    rep.n9 = if ready { "ready" } else { "timeout" };
    s4_done(h, rep); // MT7925S5 (Z23): the MCU channel handed to S5 only when FW_START was acked and N9 is ready
    Ok(())
}

/// One TX descriptor posted on a ring: buffer, length, LAST_SEC0, DMA_DONE clear, a fence, then the CPU index. Returns
/// the new CPU index. The ring's base is read back from its own base register.
fn post(q: u64, idx: u32, pa: u64, len: u32) -> u32 {
    let d = r32(q) as u64 + idx as u64 * DESC_BYTES;
    // SAFETY: descriptor `idx` (< RING_DEPTH) of a ring `arm` programmed and read back; the buffer is the caller's.
    unsafe {
        core::ptr::write_volatile(d as *mut u32, pa as u32);
        core::ptr::write_volatile((d + 8) as *mut u32, 0);
        core::ptr::write_volatile((d + 12) as *mut u32, 0);
        core::ptr::write_volatile((d + 4) as *mut u32, (len << 16) | LAST_SEC0);
    }
    core::sync::atomic::fence(Ordering::SeqCst);
    let next = (idx + 1) % RING_DEPTH;
    w32(q + 8, next);
    next
}

/// The ring's DMA index caught up with the CPU index: everything posted was fetched. Bounded TXDONE_MS.
fn tx_done(q: u64, next: u32) -> bool {
    spin_until(TXDONE_MS, || r32(q + 12) & 0xfff == next).0
}

/// One acked MCU command: the legacy connac2 TXD built in the command page, posted on ring 15, its TX completion awaited,
/// then the event with this sequence. Ok = the event's status byte (@40). MT7925S5 (Z23) hoisted the shared head into
/// `txd_head` and the post/wait into `send`, so `uni_cmd` rides the same path; the bytes written are unchanged.
fn cmd(h: &mut Hw, cid: u8, payload: &[u8]) -> Result<u8, &'static str> {
    let total = TXD_BYTES + payload.len();
    let seq = txd_head(h, total);
    let pg = h.page;
    // SAFETY: the 4 KiB command page (total <= 4096: payloads are at most 12 bytes), owned by this run, one command at a time.
    unsafe {
        core::ptr::write_volatile((pg + 32) as *mut u16, (total - 32) as u16);
        core::ptr::write_volatile((pg + 34) as *mut u16, 0x8000); // pq_id
        core::ptr::write_volatile((pg + 36) as *mut u8, cid);
        core::ptr::write_volatile((pg + 37) as *mut u8, 0xa0); // pkt_type
        core::ptr::write_volatile((pg + 38) as *mut u8, 3); // set_query: MCU_Q_NA
        core::ptr::write_volatile((pg + 39) as *mut u8, seq);
        for (i, v) in payload.iter().enumerate() {
            core::ptr::write_volatile((pg + TXD_BYTES as u64 + i as u64) as *mut u8, *v);
        }
    }
    send(h, total, seq)
}

/// The event ring, the ONE event consumer: descriptor `ri` polled for DMA_DONE (the whole wait bounded EVENT_MS), its
/// buffer's seq (@37) matched, status @40 returned; other events are skipped. MT7925S5 (Z23, S4-RECYCLE): a matched
/// event's bytes go to `h.last` first, then every consumed descriptor is followed by one `recycle`, so the ring never fills.
fn event(h: &mut Hw, seq: u8) -> Result<u8, &'static str> {
    let t0 = crate::arch::ms();
    loop {
        let left = EVENT_MS.saturating_sub(crate::arch::ms().saturating_sub(t0));
        if left == 0 {
            return Err("event");
        }
        let d = h.rx0 + h.ri as u64 * DESC_BYTES;
        // SAFETY: descriptor ri (< RING_DEPTH) of ring 0's block; a status read.
        let (done, _) = spin_until(left, || unsafe { core::ptr::read_volatile((d + 4) as *const u32) } & DMA_DONE != 0);
        if !done {
            return Err("event");
        }
        core::sync::atomic::fence(Ordering::SeqCst);
        let buf = h.rxb + h.ri as u64 * RX_BUF_BYTES;
        // SAFETY: descriptor ri's ctrl word and its 2048-byte buffer, which the device has finished writing (DMA_DONE).
        let (ctrl, got, status) = unsafe {
            (
                core::ptr::read_volatile((d + 4) as *const u32),
                core::ptr::read_volatile((buf + 37) as *const u8),
                core::ptr::read_volatile((buf + 40) as *const u8),
            )
        };
        if got == seq {
            // The reply's bytes (SD_LEN0, capped at the buffer) leave the buffer before it is re-handed; a reply split over
            // buffers (LAST_SEC0 clear) keeps nothing, so S5 reads it as `cap-parse`.
            h.last.clear();
            if ctrl & LAST_SEC0 != 0 {
                let n = (((ctrl & SD_LEN0) >> 16) as u64).min(RX_BUF_BYTES);
                for i in 0..n {
                    // SAFETY: inside this descriptor's 2048-byte buffer (i < n <= RX_BUF_BYTES), written by the device.
                    h.last.push(unsafe { core::ptr::read_volatile((buf + i) as *const u8) });
                }
            }
        }
        recycle(h);
        if got == seq {
            return Ok(status);
        }
    }
}

/// FW_SCATTER: `bytes` in chunks of at most CHUNK raw on ring 16, each chunk's TX completion awaited (linux gets no
/// event per chunk). Ok = chunks sent; Err = the index of the chunk that did not complete.
fn scatter(h: &mut Hw, bytes: &[u8]) -> Result<usize, usize> {
    let mut n = 0usize;
    for c in bytes.chunks(CHUNK) {
        let next = post(h.q16, h.fi, c.as_ptr() as u64, c.len() as u32);
        h.fi = next;
        if !tx_done(h.q16, next) {
            return Err(n);
        }
        n += 1;
    }
    Ok(n)
}

/// `mt76_connac_mcu_init_download`: {addr, len, mode} little-endian; the patch-start command for the three patch
/// addresses the 7925 uses, target-address-len otherwise. Acked.
fn init_download(h: &mut Hw, addr: u32, len: u32, mode: u32) -> Result<(), &'static str> {
    let c = if addr == 0x20_0000 || addr == 0x90_0000 || addr == 0xe000_2800 { CID_PATCH_START } else { CID_TARGET_ADDR };
    let mut pl = [0u8; 12];
    pl[0..4].copy_from_slice(&addr.to_le_bytes());
    pl[4..8].copy_from_slice(&len.to_le_bytes());
    pl[8..12].copy_from_slice(&mode.to_le_bytes());
    cmd(h, c, &pl).map(|_| ())
}

/// A patch section's download mode from its security word (`get_data_mode`).
fn patch_mode(info: u32) -> u32 {
    let mut m = DL_NEED_RSP;
    if info == 0xffff_ffff {
        return m;
    }
    match info >> 24 {
        1 => m |= DL_ENCRYPT | ((info & 0xff) << 1 & DL_KEY_IDX) | DL_RESET_SEC_IV,
        2 => m |= DL_ENCRYPT | DL_ENCRY_MODE_SEL | DL_RESET_SEC_IV,
        _ => {}
    }
    m
}

/// A RAM region's download mode from its feature word (`gen_dl_mode`).
fn ram_mode(f: u32) -> u32 {
    let mut m = DL_NEED_RSP | (f & DL_KEY_IDX);
    if f & FEAT_ENCRYPT != 0 {
        m |= DL_ENCRYPT | DL_RESET_SEC_IV;
    }
    if f & FEAT_ENCRY_MODE != 0 {
        m |= DL_ENCRY_MODE_SEL;
    }
    m
}

/// The patch: every section init_download + scatter, PATCH_FINISH (status 0), semaphore release (status 3).
fn patch(h: &mut Hw, fw: &[u8]) -> Result<(), String> {
    let hdr = || String::from("hdr");
    let n = be32(fw, 0x2c).ok_or_else(hdr)? as usize;
    if n == 0 || n > MAX_REGIONS {
        return Err(hdr());
    }
    for i in 0..n {
        let s = PATCH_HDR + PATCH_SEC * i;
        let (Some(ty), Some(offs), Some(addr), Some(len), Some(info)) =
            (be32(fw, s), be32(fw, s + 4), be32(fw, s + 12), be32(fw, s + 16), be32(fw, s + 20))
        else {
            return Err(hdr());
        };
        let (offs, len) = (offs as usize, len as usize);
        let end = offs.checked_add(len).filter(|e| *e <= fw.len()).ok_or_else(hdr)?;
        if ty & 0xffff != 2 {
            return Err(hdr());
        }
        init_download(h, addr, len as u32, patch_mode(info)).map_err(|e| why_of(e, alloc::format!("dl at={}", i)))?;
        scatter(h, &fw[offs..end]).map_err(|c| alloc::format!("scatter at={}", c))?;
    }
    match cmd(h, CID_PATCH_FINISH, &[0u8; 4]) {
        Ok(0) => {}
        Err(e) => return Err(why_of(e, String::from("finish"))),
        Ok(_) => return Err(String::from("finish")),
    }
    match cmd(h, CID_SEM, &SEM_RELEASE.to_le_bytes()) {
        Ok(SEM_RELEASED) => Ok(()),
        Err(e) => Err(why_of(e, String::from("sem"))),
        Ok(_) => Err(String::from("sem")),
    }
}

/// The RAM code: every downloadable region init_download + scatter (the data offset accumulates every region's length,
/// NON_DL ones included). Ok = FW_START's (option, override address).
fn ram(h: &mut Hw, fw: &[u8]) -> Result<(u32, u32), String> {
    let hdr = || String::from("hdr");
    let t = fw.len().checked_sub(RAM_TRAILER).ok_or_else(hdr)?;
    let n = *fw.get(t + 2).ok_or_else(hdr)? as usize;
    if n == 0 || n > MAX_REGIONS || n * RAM_REGION > t {
        return Err(hdr());
    }
    let data_end = t - n * RAM_REGION;
    let mut off = 0usize;
    let mut ov: Option<u32> = None;
    for i in 0..n {
        let r = t - RAM_REGION * (n - i);
        let (Some(addr), Some(len), Some(feat)) = (le32(fw, r + 16), le32(fw, r + 20), le32(fw, r + 24)) else {
            return Err(hdr());
        };
        let len = len as usize;
        let end = off.checked_add(len).filter(|e| *e <= data_end).ok_or_else(hdr)?;
        if feat & FEAT_NON_DL == 0 {
            if feat & FEAT_OVERRIDE_ADDR != 0 {
                ov = Some(addr);
            }
            init_download(h, addr, len as u32, ram_mode(feat)).map_err(|e| why_of(e, alloc::format!("dl at={}", i)))?;
            scatter(h, &fw[off..end]).map_err(|c| alloc::format!("scatter at={}", c))?;
        }
        off = end;
    }
    Ok((ov.is_some() as u32, ov.unwrap_or(0)))
}

// ── MT7925S5 (zenbook-ledger Z23) — S5: S4-RECYCLE, the NIC capability over a unified command, the MAC address ─────────
// Once, on the storage-ready pass after a kept S4 line (`s4_done` hands to S_S5 and parks the MCU channel in `MCU_LINK`
// only when FW_START was acked and N9 read ready). Linux's `mt7925_run_firmware`: firmware, then
// `mt7925_mcu_get_nic_capability` = the unified CHIP_CONFIG command (cid 0x0e) carrying the NIC_CAPA tag, its reply a count
// and TLVs (MAC address 7, phy caps 8, chip caps 0x20). S4 gave own back with the N9 running, so S5 re-takes it and does
// linux's wake reset (`mt792x_wpdma_reinit_cond`: DMA off, indices 0, RX refilled, DMA on) every time: the WFDMA_DUMMY_CR
// flag that would say whether it is needed is S3b's, owed. Bus master is left as S4 left it (on); no config write, no
// interrupt enable, nothing transmitted over the air. The efuse buffer mode and the CLC are owed (S5b, S5-CLC).
// Hardware facts only; no code copied.

const S_S5: u8 = 7;
/// The MCU channel S4 opened (rings, command page, event block, indices, seq): written by `s4_done`, taken by `s5`, put
/// back by `s5` on a clean line for the next rung. Never held across MMIO, a spin or a command.
static MCU_LINK: crate::sync::Mutex<Option<Hw>> = crate::sync::Mutex::new(None);
/// The radio's MAC address from the capability reply: written by `s5` on a clean line, read by `mac`.
static MAC: crate::sync::Mutex<Option<[u8; 6]>> = crate::sync::Mutex::new(None);

/// MCU_UNI_CMD_CHIP_CONFIG and its UNI_CHIP_CONFIG_NIC_CAPA tag (mt76_connac_mcu.h, mt7925/mcu.h).
const UNI_CHIP_CONFIG: u16 = 0x0e;
const NIC_CAPA: u16 = 3;
/// The unified command TXD (connac2 mcu_uni_txd): 48 bytes; option = ACK | UNI | SET (a set command wanting its reply).
const UNI_TXD_BYTES: usize = 48;
const UNI_OPT_EXT_ACK: u8 = 0x07;
/// The event header (mt7925_mcu_rxd) the reply's body follows, and the capability count header after it.
const RXD_BYTES: usize = 44;
const CAP_HDR: usize = 4;
/// Capability TLV tags (MT_NIC_CAP_*): MAC address, phy caps (13 bytes), chip caps (u64).
const TAG_MAC: u16 = 7;
const TAG_PHY: u16 = 8;
const TAG_CHIP_CAP: u16 = 0x20;
const PHY_CAP_BYTES: usize = 13;
/// RX descriptor ctrl: the received byte count (SD_LEN0, bits 29:16).
const SD_LEN0: u32 = 0x3fff << 16;

/// Ring 0's 31 buffers handed (S4's loop, hoisted unchanged by MT7925S5): descriptor i gets slot i of the event block, its
/// length, DMA_DONE clear. Descriptor 31 is left as it is; the CPU index (31) is the caller's `arm`.
fn hand_rx(rx0: u64, rxb: u64) {
    for i in 0..RX_BUFS as u64 {
        let d = rx0 + i * DESC_BYTES;
        // SAFETY: descriptor i of ring 0's block (RING_BYTES, 32 descriptors, i < 31); the buffer is inside the 64 KiB block.
        unsafe {
            core::ptr::write_volatile(d as *mut u32, (rxb + i * RX_BUF_BYTES) as u32);
            core::ptr::write_volatile((d + 4) as *mut u32, (RX_BUF_BYTES as u32) << 16); // DMA_DONE clear: handed to the device
        }
    }
}

/// The head every MCU command shares (hoisted out of `cmd`): the next sequence (1..15), the page's header zeroed, DW0
/// (byte count, PKT_FMT = CMD, Q_IDX = the MCU port's RX queue 0) and DW1 (HDR_FORMAT = CMD). Returns the sequence.
fn txd_head(h: &mut Hw, total: usize) -> u8 {
    h.seq = if h.seq >= 15 { 1 } else { h.seq + 1 };
    let pg = h.page;
    // SAFETY: the 4 KiB command page, owned by the MCU channel, one command at a time; 80 bytes zeroed.
    unsafe {
        core::ptr::write_bytes(pg as *mut u8, 0, TXD_BYTES + 16);
        core::ptr::write_volatile(pg as *mut u32, total as u32 | (2 << 23) | (0x20 << 25)); // TX_BYTES | PKT_FMT=CMD | Q_IDX
        core::ptr::write_volatile((pg + 4) as *mut u32, 1 << 14); // HDR_FORMAT=CMD
    }
    h.seq
}

/// The tail every MCU command shares (hoisted out of `cmd`): the page posted on ring 15, its TX completion awaited, then
/// the event with `seq`.
fn send(h: &mut Hw, total: usize, seq: u8) -> Result<u8, &'static str> {
    let next = post(h.q15, h.wi, h.page, total as u32);
    h.wi = next;
    if !tx_done(h.q15, next) {
        return Err("tx");
    }
    event(h, seq)
}

/// One acked unified MCU command on the same path as `cmd`: the 48-byte uni TXD (len, cid, pkt_type 0xa0, seq, host-to-WM,
/// option ACK|UNI|SET) and the TLV payload after it. Ok = the event's byte @40 (unused by CHIP_CONFIG); its bytes in `h.last`.
fn uni_cmd(h: &mut Hw, cid: u16, payload: &[u8]) -> Result<u8, &'static str> {
    let total = UNI_TXD_BYTES + payload.len();
    let seq = txd_head(h, total);
    let pg = h.page;
    // SAFETY: the 4 KiB command page (total <= 4096: S5's payload is 8 bytes), owned by the MCU channel, one command at a time.
    unsafe {
        core::ptr::write_volatile((pg + 32) as *mut u16, (total - 32) as u16); // len, not counting the 32-byte hardware TXD
        core::ptr::write_volatile((pg + 34) as *mut u16, cid);
        core::ptr::write_volatile((pg + 37) as *mut u8, 0xa0); // pkt_type
        core::ptr::write_volatile((pg + 39) as *mut u8, seq);
        core::ptr::write_volatile((pg + 42) as *mut u8, 0); // s2d: host to WM
        core::ptr::write_volatile((pg + 43) as *mut u8, UNI_OPT_EXT_ACK);
        for (i, v) in payload.iter().enumerate() {
            core::ptr::write_volatile((pg + UNI_TXD_BYTES as u64 + i as u64) as *mut u8, *v);
        }
    }
    send(h, total, seq)
}

/// S4-RECYCLE: after descriptor `ri` is consumed, the slot one behind it (`ri + 31`, mod 32) gets its own fixed buffer back
/// (buf1 and info 0, the length in ctrl, DMA_DONE clear), a fence, then ring 0's CPU index moves to `ri`, so 31 buffers stay
/// handed and one descriptor unhanded; `ri` advances, wrapping at 32.
fn recycle(h: &mut Hw) {
    let slot = (h.ri + RX_BUFS) % RING_DEPTH;
    let d = h.rx0 + slot as u64 * DESC_BYTES;
    // SAFETY: descriptor `slot` (< 32) of ring 0's block; the buffer is slot `slot` of the 64 KiB event block (32 x 2 KiB).
    unsafe {
        core::ptr::write_volatile(d as *mut u32, (h.rxb + slot as u64 * RX_BUF_BYTES) as u32);
        core::ptr::write_volatile((d + 8) as *mut u32, 0);
        core::ptr::write_volatile((d + 12) as *mut u32, 0);
        core::ptr::write_volatile((d + 4) as *mut u32, (RX_BUF_BYTES as u32) << 16); // DMA_DONE clear: handed to the device
    }
    core::sync::atomic::fence(Ordering::SeqCst);
    w32(h.q0 + 8, (slot + 1) % RING_DEPTH);
    h.ri = (h.ri + 1) % RING_DEPTH;
}

/// S4 calls this at its end: the MCU channel parked for S5, and S5 on the next pass, only when FW_START was acked and N9 is
/// ready (`s4`'s `keep` condition: s4_run reached its end, so the run is Ok and the RAM code was sent).
fn s4_done(h: Hw, rep: &Rep) {
    if rep.fw_start == "ok" && rep.n9 == "ready" {
        *MCU_LINK.lock() = Some(h);
        STATE.store(S_S5, Ordering::Relaxed);
    }
}

/// What the capability reply carried.
struct Caps {
    n: u16,
    mac: [u8; 6],
    chip: Option<u64>,
    phy: Option<[u8; PHY_CAP_BYTES]>,
}

/// S5's failures: before the command (`NOT ATTEMPTED`) or the command's own refusals (`REFUSED`, with the TLV count when read).
enum S5Why {
    Pre(String),
    Refused(&'static str, Option<u16>),
}

fn s5() {
    STATE.store(S_PARKED, Ordering::Relaxed);
    let held = *PROBE.lock();
    let Some(p) = held else { return };
    let link = MCU_LINK.lock().take();
    let Some(mut h) = link else { return };
    let b = p.bar0;
    let lp = b + CONN_HOST_CSR_TOP + LPCTL;
    if !(map_page(b + WFDMA0) && map_page(b + (DMASHDL_SW_CONTROL & !0xfff))) {
        serial_println!("[mt7925] s5 -> NOT ATTEMPTED why=unmapped rel=found");
        return;
    }
    let took = r32(lp) & OWN_SYNC != 0;
    if took {
        let (ok, last, _) = own(lp, CLR_OWN, false);
        if !ok {
            let rel = if own(lp, SET_OWN, true).0 { "fw" } else { "timeout" };
            serial_println!("[mt7925] s5 -> NOT ATTEMPTED why=own lpctl={:#010x} rel={}", last, rel);
            return;
        }
    }
    let mut kept = "lost";
    let res = s5_run(b, &mut h, &mut kept);
    let rel = give_back(lp, took);
    match res {
        Ok(c) => {
            let m = c.mac;
            let chip = c.chip.map_or(String::from("-"), |v| alloc::format!("{:#018x}", v));
            let phy = c.phy.map_or(String::from("-"), |v| {
                let mut t = String::with_capacity(2 * PHY_CAP_BYTES);
                for x in v {
                    let _ = core::fmt::Write::write_fmt(&mut t, format_args!("{:02x}", x));
                }
                t
            });
            *MAC.lock() = Some(m);
            serial_println!(
                "[mt7925] s5 mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} caps={} chip_cap={} phy={} held={} rel={} -> parked",
                m[0], m[1], m[2], m[3], m[4], m[5], c.n, chip, phy, kept, rel
            );
            *MCU_LINK.lock() = Some(h); STATE.store(S_S6, Ordering::Relaxed); // the channel kept for the next rung (S6; MT7925S6, Z29, hands to S_S6 here)
        }
        Err(S5Why::Pre(w)) => {
            serial_println!("[mt7925] s5 -> NOT ATTEMPTED why={} rel={}", w, rel);
        }
        Err(S5Why::Refused(w, n)) => {
            let caps = n.map_or(String::from("-"), |v| alloc::format!("{}", v));
            serial_println!("[mt7925] s5 -> REFUSED why={} caps={} held={} rel={}", w, caps, kept, rel);
        }
    }
}

/// Everything between driver-own and give-back: `held`, linux's wake reset over S4's memory (ring 16's CPU index back to 0
/// so S4's firmware chunks cannot be fetched again), then the capability query and its parse.
fn s5_run(b: u64, h: &mut Hw, held: &mut &'static str) -> Result<Caps, S5Why> {
    *held = if r32(h.q15) as u64 == h.wm && r32(h.q0) as u64 == h.rx0 { "kept" } else { "lost" };
    wake(b, h, &[]).map_err(S5Why::Pre)?; // MT7925S6 (Z29) hoisted the wake reset into `wake`, S5's writes unchanged and in order
    let mut pl = [0u8; 8]; // 4 reserved bytes, then the NIC_CAPA tag and its length (4)
    pl[4..6].copy_from_slice(&NIC_CAPA.to_le_bytes());
    pl[6..8].copy_from_slice(&4u16.to_le_bytes());
    uni_cmd(h, UNI_CHIP_CONFIG, &pl).map_err(|_| S5Why::Refused("cap-timeout", None))?;
    caps(&h.last)
}

/// The capability reply: past the 44-byte event header, a TLV count and the TLVs (each length counting its own 4-byte
/// header); the walk stops at a short or overlong TLV. The MAC is required and must pass `mac_ok`. The chip caps are the
/// TLV's 8 data bytes (linux copies 8 bytes from the TLV's header instead).
fn caps(ev: &[u8]) -> Result<Caps, S5Why> {
    if ev.len() < RXD_BYTES + CAP_HDR {
        return Err(S5Why::Refused("cap-parse", None));
    }
    let n = u16::from_le_bytes([ev[RXD_BYTES], ev[RXD_BYTES + 1]]);
    let (mut mac, mut chip, mut phy) = (None, None, None);
    let mut o = RXD_BYTES + CAP_HDR;
    for _ in 0..n {
        if ev.len() - o < 4 {
            break;
        }
        let tag = u16::from_le_bytes([ev[o], ev[o + 1]]);
        let len = u16::from_le_bytes([ev[o + 2], ev[o + 3]]) as usize;
        if len < 4 || len > ev.len() - o {
            break;
        }
        let d = &ev[o + 4..o + len];
        match tag {
            TAG_MAC if d.len() >= 6 => {
                let mut m = [0u8; 6];
                m.copy_from_slice(&d[..6]);
                mac = Some(m);
            }
            TAG_PHY if d.len() >= PHY_CAP_BYTES => {
                let mut c = [0u8; PHY_CAP_BYTES];
                c.copy_from_slice(&d[..PHY_CAP_BYTES]);
                phy = Some(c);
            }
            TAG_CHIP_CAP if d.len() >= 8 => {
                let mut c = [0u8; 8];
                c.copy_from_slice(&d[..8]);
                chip = Some(u64::from_le_bytes(c));
            }
            _ => {}
        }
        o += len;
    }
    match mac {
        Some(m) if mac_ok(&m) => Ok(Caps { n, mac: m, chip, phy }),
        _ => Err(S5Why::Refused("no-mac", Some(n))),
    }
}

/// The station-MAC rule `wifi_core::sprom::decode` applies inline (not zero, not all-ones, unicast); wifi_core is not on
/// the always-on MT7925 path, so the one-line rule is restated here (MACOK-SHARED owed: one wifi_core fn for both).
fn mac_ok(m: &[u8; 6]) -> bool {
    *m != [0; 6] && *m != [0xff; 6] && m[0] & 1 == 0
}

/// The MT7925's MAC address once S5 read a clean capability reply, else None; RawNic::mac's shape, for S6's backend.
pub fn mac() -> Option<[u8; 6]> {
    *MAC.lock()
}

// ── MT7925S6 (zenbook-ledger Z29) — S6: the radio backend registered over the parked channel, the data rings armed ──────
// Once, on the storage-ready pass after a clean S5 line (`s5` hands to S_S6 with the MCU channel back in `MCU_LINK` and the
// MAC behind `mac()`). No radio trait exists in the tree: the Broadcom station drives its own internals, and its link joins
// the stack through `wifi/data.rs`'s accessor triple (BP-net-16). The one NIC trait is `net_phy::RawNic` (BP-net-1), so the
// backend is `MtRadio` implementing it: the MAC from S5, transmit refused (`tx_ready` false) and receive answering nothing,
// both by name (`why=s7-not-built`). S6 re-takes own, runs S5's wake reset again through the ONE `wake` with linux's two
// data rings added (TX ring 0, band-0 data; RX ring 2, data: programmed, CPU index 0, every descriptor DMA_DONE, nothing
// posted, no buffer handed), gives own back and registers the radio; `wifi::service`'s parked arm names it
// (`[wifi] radio=mt7925`, wifi/mod.rs `which`). The stack's link chain (drivers/e1000.rs's accessor fold) is S9's, so smolnet
// never binds a link that cannot send. No config write (bus master as S4 left it), no interrupt enable, no MCU command,
// nothing transmitted. Hardware facts only; no code copied.

const S_S6: u8 = 8;
/// Linux's band-0 data TX ring and data RX ring (mt7925.h: TX band0 = ring 0, RX data = ring 2; mt7925.md §1).
const TXQ_DATA: u64 = 0;
const RXQ_DATA: u64 = 2;

/// The two data rings S6 armed (descriptor memory; heap address = bus address, below 4 GiB): written once by `s6`.
struct Data {
    tx0: u64,
    rx2: u64,
}
static RADIO: crate::sync::Mutex<Option<Data>> = crate::sync::Mutex::new(None);
/// Set by `s6` on its clean line, after `RADIO`; read by `registered` (wifi's `which`, `MtRadio::mac`).
static REGISTERED: AtomicBool = AtomicBool::new(false);

/// Linux's wake reset over the parked channel (hoisted out of `s5_run` by MT7925S6, S5's writes unchanged and in order): DMA
/// off, ring 0's 31 buffers handed, rings 16 (CPU index 0), 15 and 0 (31) re-armed, then each `(ring registers, descriptor
/// memory)` in `data` armed with CPU index 0 (S6's data rings; empty for S5), DMA on. Err = `dma_off`'s word, or `rb` when a
/// read-back or the enable did not hold. The channel's ring indices restart at 0.
fn wake(b: u64, h: &mut Hw, data: &[(u64, u64)]) -> Result<(), String> {
    dma_off(b)?;
    hand_rx(h.rx0, h.rxb);
    let mut rb = arm(h.q16, S4_TX16.load(Ordering::Relaxed), 0);
    rb &= arm(h.q15, h.wm, 0);
    rb &= arm(h.q0, h.rx0, RX_BUFS);
    for &(q, pa) in data {
        rb &= arm(q, pa, 0); // CPU index 0: nothing posted, no buffer handed (S3's convention)
    }
    let glo = dma_on(b);
    if !rb || glo & (GLO_TX_EN | GLO_RX_EN) != (GLO_TX_EN | GLO_RX_EN) {
        return Err(String::from("rb"));
    }
    h.wi = 0;
    h.ri = 0;
    Ok(())
}

fn s6() {
    STATE.store(S_PARKED, Ordering::Relaxed);
    let held = *PROBE.lock();
    let Some(p) = held else { return };
    let link = MCU_LINK.lock().take();
    let Some(mut h) = link else { return };
    let Some(m) = mac() else {
        serial_println!("[mt7925] s6 -> NOT ATTEMPTED why=no-mac held=- rel=found");
        return;
    };
    let b = p.bar0;
    let lp = b + CONN_HOST_CSR_TOP + LPCTL;
    if !(map_page(b + WFDMA0) && map_page(b + (DMASHDL_SW_CONTROL & !0xfff))) {
        serial_println!("[mt7925] s6 -> NOT ATTEMPTED why=unmapped held=- rel=found");
        return;
    }
    let took = r32(lp) & OWN_SYNC != 0;
    if took {
        let (ok, last, _) = own(lp, CLR_OWN, false);
        if !ok {
            let rel = if own(lp, SET_OWN, true).0 { "fw" } else { "timeout" };
            serial_println!("[mt7925] s6 -> NOT ATTEMPTED why=own lpctl={:#010x} held=- rel={}", last, rel);
            return;
        }
    }
    let kept = if r32(h.q15) as u64 == h.wm && r32(h.q0) as u64 == h.rx0 { "kept" } else { "lost" };
    let (tx0, rx2) = (ring(), ring());
    let res = if tx0 == 0 || rx2 == 0 {
        Err(String::from("no-memory")) // nothing written to a ring register; an allocated ring is never freed (S3's rule)
    } else {
        let data = [(b + TX_RING_BASE + TXQ_DATA * RING_REGS, tx0), (b + RX_EVENT_RING_BASE + RXQ_DATA * RING_REGS, rx2)];
        wake(b, &mut h, &data)
    };
    let rel = give_back(lp, took);
    match res {
        Ok(()) => {
            *RADIO.lock() = Some(Data { tx0, rx2 });
            REGISTERED.store(true, Ordering::Release);
            *MCU_LINK.lock() = Some(h); // the channel kept for the next rung (S7)
            serial_println!(
                "[mt7925] s6 radio=registered mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} rings=tx0,rx2 tx0={:#x} rx2={:#x} cnt={} held={} rel={} -> parked",
                m[0], m[1], m[2], m[3], m[4], m[5], tx0, rx2, RING_DEPTH, kept, rel
            );
        }
        Err(w) => {
            serial_println!("[mt7925] s6 -> NOT ATTEMPTED why={} held={} rel={}", w, kept, rel);
        }
    }
}

/// True once S6 registered the radio (one load); wifi's `which` and `MtRadio::mac` read it.
pub fn registered() -> bool {
    REGISTERED.load(Ordering::Acquire)
}

/// The data rings' descriptor memory (TX ring 0, RX ring 2) once S6 registered the radio, else None; S7 posts on them.
pub fn data_rings() -> Option<(u64, u64)> {
    RADIO.lock().as_ref().map(|d| (d.tx0, d.rx2))
}

/// The MT7925 radio backend (S6): the kernel's one NIC trait (`net_phy::RawNic`) over the parked channel. Nothing binds it
/// to the stack yet (S9 owed); every operation but `mac` is refused by name, never a silent Ok.
pub struct MtRadio;

#[cfg(feature = "smolnet")]
impl crate::net_phy::RawNic for MtRadio {
    /// Refused: RX ring 2 is armed with no buffer handed; the frame path is S7's.
    fn rx_frame_raw(_out: &mut [u8]) -> Option<usize> {
        static SAID: AtomicBool = AtomicBool::new(false);
        if !SAID.swap(true, Ordering::Relaxed) {
            serial_println!("[mt7925] s6 rx -> REFUSED why=s7-not-built");
        }
        None
    }
    /// Refused: TX ring 0 is armed, nothing is posted (the TXWI/connac TXD is S7's); the frame is dropped, said once.
    fn transmit(_frame: &[u8]) {
        static SAID: AtomicBool = AtomicBool::new(false);
        if !SAID.swap(true, Ordering::Relaxed) {
            serial_println!("[mt7925] s6 tx -> REFUSED why=s7-not-built");
        }
    }
    /// S5's capability-reply MAC, only once S6 registered the radio.
    fn mac() -> Option<[u8; 6]> {
        if registered() { self::mac() } else { None }
    }
    /// The explicit refusal: no room ever, so smoltcp's back-pressure never calls `transmit`.
    fn tx_ready() -> bool {
        false
    }
}
