//! CHARTER: Kernel — driver (the MediaTek MT7925 Wi-Fi 7 radio's S1 probe, zenbook-ledger Z5, S2 driver-own and the chip id, Z11, and S3 the WFSYS reset and the WFDMA rings, Z15, and S4 the firmware upload, Z17, and S5 the NIC capability and MAC, Z23, and S6 the radio backend registered with the data rings armed, Z29, and S7 the scan, Z34, and S8 DEV_INFO/BSS_INFO, the channel and the open-system auth/assoc frames, Z38; no CODEX §2 handler owns a NIC driver, the BCM4331 `wifi/**` sits in the same seam)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! MT7925 — S1, the probe (ZENBOOKMT7925, zenbook-ledger Z5). The ladder is
//! `docs/dev/OS/06_NETWORK_STACK/mt7925.md`; S2 (MT7925S2, Z11), S3 (MT7925S3, Z15), S4 (MT7925S4, Z17), S5 (MT7925S5, Z23), S6 (MT7925S6, Z29), S7 (MT7925S7, Z34) and S8 (MT7925S8, Z38) are the seven file-tail blocks; S9..S10 are owed there by name.
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
    let took = match take_own(lp) { // MT7925S7 (Z34): the ONE take beside `give_back`, the writes unchanged and in order
        Ok(t) => t,
        Err((last, rel)) => {
            serial_println!("[mt7925] s3 -> NOT ATTEMPTED why=own lpctl={:#010x} rel={}", last, rel);
            return;
        }
    };
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

/// The ONE driver-own take (MT7925S7, Z34, hoisted out of s3..s6, whose four inline copies were byte-identical but for
/// the print; the writes are unchanged and in order): when the firmware owns (OWN_SYNC set), CLR_OWN through `own`; on a
/// failed take firmware-own is handed straight back. Ok(took); Err((the last LPCTL read, the `rel=` word)).
fn take_own(lp: u64) -> Result<bool, (u32, &'static str)> {
    let took = r32(lp) & OWN_SYNC != 0;
    if took {
        let (ok, last, _) = own(lp, CLR_OWN, false);
        if !ok {
            let rel = if own(lp, SET_OWN, true).0 { "fw" } else { "timeout" };
            return Err((last, rel));
        }
    }
    Ok(took)
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
    let took = match take_own(lp) { // MT7925S7 (Z34): the ONE take beside `give_back`, the writes unchanged and in order
        Ok(t) => t,
        Err((last, rel)) => {
            serial_println!("[mt7925] s4 -> NOT ATTEMPTED why=own lpctl={:#010x} bm=off rel={}", last, rel);
            return;
        }
    };
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
        if let Some(Some(status)) = event_take(h, &|_: u8, got: u8, _: u8| got == seq) {
            return Ok(status); // MT7925S7 (Z34): the consume step is `event_take`, the reads, h.last rule and recycle unchanged
        }
    }
}

/// The event ring's ONE consume step (hoisted out of `event` by MT7925S7, Z34, so S7 can poll for SCAN_DONE between RX ring
/// 2 drains): descriptor `ri`'s DMA_DONE read; when set, its buffer's eid (@36), seq (@37), option (@38) and status (@40)
/// read, the reply's bytes copied to `h.last` when `want(eid, seq, option)` matches, then one `recycle`. None = descriptor
/// `ri` not done (nothing consumed); Some(Some(status)) = consumed and matched; Some(None) = consumed, another event.
fn event_take(h: &mut Hw, want: &dyn Fn(u8, u8, u8) -> bool) -> Option<Option<u8>> {
    let d = h.rx0 + h.ri as u64 * DESC_BYTES;
    // SAFETY: descriptor ri (< RING_DEPTH) of ring 0's block; a status read.
    if unsafe { core::ptr::read_volatile((d + 4) as *const u32) } & DMA_DONE == 0 {
        return None;
    }
    core::sync::atomic::fence(Ordering::SeqCst);
    let buf = h.rxb + h.ri as u64 * RX_BUF_BYTES;
    // SAFETY: descriptor ri's ctrl word and its 2048-byte buffer, which the device has finished writing (DMA_DONE).
    let (ctrl, eid, got, opt, status) = unsafe {
        (
            core::ptr::read_volatile((d + 4) as *const u32),
            core::ptr::read_volatile((buf + 36) as *const u8),
            core::ptr::read_volatile((buf + 37) as *const u8),
            core::ptr::read_volatile((buf + 38) as *const u8),
            core::ptr::read_volatile((buf + 40) as *const u8),
        )
    };
    let hit = want(eid, got, opt);
    if hit {
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
    Some(if hit { Some(status) } else { None })
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

/// S4-RECYCLE: ring 0's re-hand after each consumed event (MT7925S7, Z34: the body is the ONE ring-parameterised `rearm`,
/// ring 0's writes unchanged and in order).
fn recycle(h: &mut Hw) {
    h.ri = rearm(h.q0, h.rx0, h.rxb, h.ri);
}

/// The ONE RX re-hand for any ring (hoisted out of `recycle` by MT7925S7, Z34; ring 0's events and S7's RX ring 2 frames):
/// after descriptor `ri` of the ring whose registers are at `q` (descriptors at `desc`, 32 x 2 KiB buffers at `bufs`) is
/// consumed, the slot one behind it (`ri + 31`, mod 32) gets its own fixed buffer back (buf1 and info 0, the length in ctrl,
/// DMA_DONE clear), a fence, then the ring's CPU index moves to `ri`, so 31 buffers stay handed and one descriptor unhanded.
/// Returns the next `ri`, wrapping at 32.
fn rearm(q: u64, desc: u64, bufs: u64, ri: u32) -> u32 {
    let slot = (ri + RX_BUFS) % RING_DEPTH;
    let d = desc + slot as u64 * DESC_BYTES;
    // SAFETY: descriptor `slot` (< 32) of the ring's block; the buffer is slot `slot` of its 64 KiB block (32 x 2 KiB).
    unsafe {
        core::ptr::write_volatile(d as *mut u32, (bufs + slot as u64 * RX_BUF_BYTES) as u32);
        core::ptr::write_volatile((d + 8) as *mut u32, 0);
        core::ptr::write_volatile((d + 12) as *mut u32, 0);
        core::ptr::write_volatile((d + 4) as *mut u32, (RX_BUF_BYTES as u32) << 16); // DMA_DONE clear: handed to the device
    }
    core::sync::atomic::fence(Ordering::SeqCst);
    w32(q + 8, (slot + 1) % RING_DEPTH);
    (ri + 1) % RING_DEPTH
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
    let took = match take_own(lp) { // MT7925S7 (Z34): the ONE take beside `give_back`, the writes unchanged and in order
        Ok(t) => t,
        Err((last, rel)) => {
            serial_println!("[mt7925] s5 -> NOT ATTEMPTED why=own lpctl={:#010x} rel={}", last, rel);
            return;
        }
    };
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
            *MAC.lock() = Some(m); S7_PATH.store(c.phy.map_or(PATH_UNREAD, |v| v[10] as u16), Ordering::Relaxed); // MT7925S7 (Z34): the PHY TLV's hw_path byte (bit 0 = 2.4 GHz, bit 1 = 5 GHz) kept for S7's bands
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
    let took = match take_own(lp) { // MT7925S7 (Z34): the ONE take beside `give_back`, the writes unchanged and in order
        Ok(t) => t,
        Err((last, rel)) => {
            serial_println!("[mt7925] s6 -> NOT ATTEMPTED why=own lpctl={:#010x} held=- rel={}", last, rel);
            return;
        }
    };
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

// ── MT7925S7 (zenbook-ledger Z34) — S7: the scan ────────────────────────────────────────────────────────────────────────
// Only from the operator's `tests wifiscan` (wifi/verb.rs `wifiscan`), never at boot (BP-net-7), over the channel S6 parked
// in `MCU_LINK` and the data rings in `RADIO`. Linux's `mt7925_mcu_hw_scan`: ONE unified SCAN_REQ (cid 0x16) per band, its
// body a 4-byte header (scan seq, bss index) and six TLVs (request, SSID, BSSID, channel list, IEs, misc), acked; the end is
// the UNSOLICITED SCAN_DONE event (eid 0x0e) on ring 0; the results are ordinary beacons and probe responses on the data RX
// ring (ring 2) behind the connac3 RX descriptor. S7 re-takes own (`take_own`), runs S6's wake reset through the ONE `wake`
// with S6's two data rings, hands RX ring 2 its 31 buffers (`hand_rx`), then per band sends the request (`uni_cmd`) and,
// bounded 5 s, empties RX ring 2 (`drain`, one `rearm` per frame: the data-ring twin of the event recycle is the same fn)
// while it polls ring 0 for SCAN_DONE (`event_take`). Each frame, its RX descriptor stripped (`rx_frame`), goes raw to the
// station's closure; the driver links no wifi_core. 2.4 GHz channels 1-11 are scanned ACTIVE (the firmware sends the probe
// requests, wildcard SSID); the nine non-DFS 5 GHz channels PASSIVE (listen only) and only when S5's hw_path says 5 GHz; no
// DFS channel, no 12/13/14. bss index 0 with no DEV_INFO/BSS_INFO (the ladder's S8; S7-DEV owed should the firmware refuse:
// the wire's `ack=` / `why=no-done` says so). No config write, no interrupt enable, no host-built frame. Hardware facts only;
// no code copied.

/// MCU_UNI_CMD_SCAN_REQ and its TLV tags (mt7925/mcu.h UNI_SCAN_*).
const UNI_SCAN_REQ: u16 = 0x16;
const SCAN_TAG_REQ: u16 = 1;
const SCAN_TAG_SSID: u16 = 10;
const SCAN_TAG_BSSID: u16 = 11;
const SCAN_TAG_CHANNEL: u16 = 12;
const SCAN_TAG_IE: u16 = 13;
const SCAN_TAG_MISC: u16 = 14;
/// The fixed header and each TLV's length (counting its own 4-byte tag/len header): 4 + 20 + 152 + 16 + 136 + 608 + 12 = 948.
const SCAN_HDR: usize = 4;
const SCAN_REQ_LEN: usize = 20;
const SCAN_SSID_LEN: usize = 152;
const SCAN_BSSID_LEN: usize = 16;
const SCAN_CHAN_LEN: usize = 136;
const SCAN_IE_LEN: usize = 608;
const SCAN_MISC_LEN: usize = 12;
const SCAN_BODY: usize = SCAN_HDR + SCAN_REQ_LEN + SCAN_SSID_LEN + SCAN_BSSID_LEN + SCAN_CHAN_LEN + SCAN_IE_LEN + SCAN_MISC_LEN;
/// The channel TLV's capacity, the split-scan function bit, the wildcard SSID type, "listed channels", the band codes.
const SCAN_CHAN_MAX: usize = 64;
const SCAN_FUNC_SPLIT: u8 = 1 << 5;
const SSID_WILDCARD: u8 = 1 << 0;
const CHAN_LISTED: u8 = 4;
const BAND_2G: u8 = 1;
const BAND_5G: u8 = 2;
/// Per-channel dwell (linux's MT76_CONNAC_SCAN_CHANNEL_TIME), doubled for a passive band.
const DWELL_MS: u16 = 60;
/// The unsolicited SCAN_DONE event: its uni event id and the event header's unsolicited option bit.
const EVT_SCAN_DONE: u8 = 0x0e;
const OPT_UNSOLICITED: u8 = (1 << 1) | (1 << 2); // MT7925S8 (Z38, S7-UNSOL): linux dispatches unsolicited events on BIT(2); the eid tells SCAN_DONE / ROC from acks
/// How long one band waits for SCAN_DONE after the ack, and the drain after it (frames still in flight).
const SCAN_WAIT_MS: u64 = 5000;
const TAIL_MS: u64 = 100;
/// The channels: 2.4 GHz 1-11 (active); 5 GHz UNII-1 and UNII-3, the non-DFS nine (passive).
const CH_2G: [u8; 11] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
const CH_5G: [u8; 9] = [36, 40, 44, 48, 149, 153, 157, 161, 165];
/// The connac3 RX descriptor: 8 dwords; DW0 bits 31:27 the packet type (2 = a normal frame); DW1's group bits (G1 +4 dwords,
/// G2 +2, G3 +4, G4 +4, G5 +24 only under G3); DW2 bit 7 header-translated, bit 24 max-length error, bits 15:13 the pad.
const RXD_BASE: usize = 32;
const PKT_NORMAL: u32 = 2;
const RXD1_G1: u32 = 1 << 16;
const RXD1_G2: u32 = 1 << 17;
const RXD1_G3: u32 = 1 << 18;
const RXD1_G4: u32 = 1 << 19;
const RXD1_G5: u32 = 1 << 20;
const RXD2_HDR_TRANS: u32 = 1 << 7;
const RXD2_MAX_LEN_ERR: u32 = 1 << 24;
/// S5's hw_path byte: bit 0 = 2.4 GHz, bit 1 = 5 GHz; PATH_UNREAD = no PHY TLV (2.4 GHz scanned, 5 GHz not).
const PATH_UNREAD: u16 = 0x100;
const PATH_2G: u16 = 1 << 0;
const PATH_5G: u16 = 1 << 1;

/// S5's hw_path byte: written by `s5`'s clean line, read by `scan`.
static S7_PATH: core::sync::atomic::AtomicU16 = core::sync::atomic::AtomicU16::new(PATH_UNREAD);
/// RX ring 2's 64 KiB buffer block (32 x 2 KiB, below 4 GiB): allocated by the first scan, never freed (S3's rule).
static S7_RXB: AtomicU64 = AtomicU64::new(0);
/// The last scan sequence number (1..127).
static S7_SEQ: AtomicU8 = AtomicU8::new(0);

/// RX ring 2 as one scan drains it: registers, descriptors, buffer block, the next descriptor.
struct Rx2 {
    q: u64,
    desc: u64,
    bufs: u64,
    ri: u32,
}

/// One band's counts: frames handed to the station, descriptors skipped (not a normal frame, translated, split, length
/// error), and the BSSes the station listed new.
#[derive(Clone, Copy, Default)]
struct Counts {
    frames: u32,
    skip: u32,
    bss: u32,
}

/// What one band's request did.
struct BandRep {
    name: &'static str,
    kind: &'static str,
    req: usize,
    ack: Option<u8>,
    done: Option<u64>,
    n: Counts,
    why: Option<&'static str>,
}

/// One TLV's tag and length at `o`; returns the offset of its data.
fn put_tlv(v: &mut [u8], o: usize, tag: u16, len: usize) -> usize {
    v[o..o + 2].copy_from_slice(&tag.to_le_bytes());
    v[o + 2..o + 4].copy_from_slice(&(len as u16).to_le_bytes());
    o + 4
}

/// The SCAN_REQ body (pure): header {seq, bss 0}; REQ {active 1 / passive 0, probes 2 / 0, split scan, min dwell, dwell,
/// timeout = channels x dwell}; SSID {wildcard, none listed}; BSSID zero; CHANNEL {listed, n, (band, channel) each}; IE empty;
/// MISC zero. 948 bytes.
fn scan_req(seq: u8, band: u8, chans: &[u8], active: bool) -> Vec<u8> {
    let n = chans.len().min(SCAN_CHAN_MAX);
    let dwell = if active { DWELL_MS } else { DWELL_MS * 2 };
    let mut v = alloc::vec![0u8; SCAN_BODY];
    v[0] = seq & 0x7f; // band index 0 in bit 7
    v[1] = 0; // bss index 0 (no DEV_INFO: S7-DEV owed)
    let mut o = SCAN_HDR;
    let d = put_tlv(&mut v, o, SCAN_TAG_REQ, SCAN_REQ_LEN);
    v[d] = active as u8;
    v[d + 1] = if active { 2 } else { 0 };
    v[d + 2] = SCAN_FUNC_SPLIT;
    v[d + 4..d + 6].copy_from_slice(&dwell.to_le_bytes());
    v[d + 6..d + 8].copy_from_slice(&dwell.to_le_bytes());
    v[d + 8..d + 10].copy_from_slice(&(n as u16 * dwell).to_le_bytes());
    o += SCAN_REQ_LEN;
    let d = put_tlv(&mut v, o, SCAN_TAG_SSID, SCAN_SSID_LEN);
    v[d] = SSID_WILDCARD;
    o += SCAN_SSID_LEN;
    put_tlv(&mut v, o, SCAN_TAG_BSSID, SCAN_BSSID_LEN);
    o += SCAN_BSSID_LEN;
    let d = put_tlv(&mut v, o, SCAN_TAG_CHANNEL, SCAN_CHAN_LEN);
    v[d] = CHAN_LISTED;
    v[d + 1] = n as u8;
    for (i, c) in chans.iter().take(n).enumerate() {
        v[d + 4 + 2 * i] = band;
        v[d + 5 + 2 * i] = *c;
    }
    o += SCAN_CHAN_LEN;
    put_tlv(&mut v, o, SCAN_TAG_IE, SCAN_IE_LEN); // ies_len 0 (S7-IES owed)
    o += SCAN_IE_LEN;
    put_tlv(&mut v, o, SCAN_TAG_MISC, SCAN_MISC_LEN);
    v
}

/// The 802.11 frame's byte range inside one RX ring 2 buffer (pure): past the 8-dword descriptor, its groups and twice the
/// header pad. None = not a normal frame, header-translated, a length error, or too short for a MAC header.
fn rx_frame(buf: &[u8]) -> Option<core::ops::Range<usize>> {
    let (d0, d1, d2) = (le32(buf, 0)?, le32(buf, 4)?, le32(buf, 8)?);
    if d0 >> 27 != PKT_NORMAL || d2 & (RXD2_HDR_TRANS | RXD2_MAX_LEN_ERR) != 0 {
        return None;
    }
    let mut off = RXD_BASE;
    if d1 & RXD1_G4 != 0 {
        off += 16;
    }
    if d1 & RXD1_G1 != 0 {
        off += 16;
    }
    if d1 & RXD1_G2 != 0 {
        off += 8;
    }
    if d1 & RXD1_G3 != 0 {
        off += 16;
        if d1 & RXD1_G5 != 0 {
            off += 96;
        }
    }
    off += 2 * ((d2 >> 13) & 0x7) as usize;
    if off + 24 > buf.len() {
        return None;
    }
    Some(off..buf.len())
}

/// Every DMA_DONE descriptor of RX ring 2 in order, at most 32 per call: the bytes (SD_LEN0, capped at the buffer; a frame
/// split over buffers keeps nothing) copied out, the buffer re-handed (`rearm`), then the frame to the station's closure
/// (true = it listed a new BSS).
fn drain(r: &mut Rx2, n: &mut Counts, on_frame: &mut dyn FnMut(&[u8]) -> bool) {
    for _ in 0..RING_DEPTH {
        let d = r.desc + r.ri as u64 * DESC_BYTES;
        // SAFETY: descriptor ri (< RING_DEPTH) of RX ring 2's block (`ring()`, armed by `wake`); a status read.
        let ctrl = unsafe { core::ptr::read_volatile((d + 4) as *const u32) };
        if ctrl & DMA_DONE == 0 {
            return;
        }
        core::sync::atomic::fence(Ordering::SeqCst);
        let buf = r.bufs + r.ri as u64 * RX_BUF_BYTES;
        let mut v = Vec::new();
        if ctrl & LAST_SEC0 != 0 {
            let len = (((ctrl & SD_LEN0) >> 16) as u64).min(RX_BUF_BYTES);
            v.reserve(len as usize);
            for i in 0..len {
                // SAFETY: inside descriptor ri's 2048-byte buffer (i < len <= RX_BUF_BYTES), written by the device (DMA_DONE).
                v.push(unsafe { core::ptr::read_volatile((buf + i) as *const u8) });
            }
        }
        r.ri = rearm(r.q, r.desc, r.bufs, r.ri);
        match rx_frame(&v) {
            Some(span) => {
                n.frames += 1;
                if on_frame(&v[span]) {
                    n.bss += 1;
                }
            }
            None => n.skip += 1,
        }
    }
}

/// One band: the SCAN_REQ (`uni_cmd`; its TX completion or ack missing = `scan-timeout`), then up to 5 s of RX ring 2 drains
/// with ring 0 polled for SCAN_DONE between them (`no-done` without it), then a 100 ms tail drain; SCAN_DONE with no frame at
/// all = `rx-stall`.
fn band(h: &mut Hw, r: &mut Rx2, name: &'static str, code: u8, chans: &[u8], active: bool, on_frame: &mut dyn FnMut(&[u8]) -> bool) -> BandRep {
    let seq = S7_SEQ.load(Ordering::Relaxed) % 127 + 1;
    S7_SEQ.store(seq, Ordering::Relaxed);
    let mut rep = BandRep {
        name,
        kind: if active { "active" } else { "passive" },
        req: chans.len().min(SCAN_CHAN_MAX),
        ack: None,
        done: None,
        n: Counts::default(),
        why: None,
    };
    match uni_cmd(h, UNI_SCAN_REQ, &scan_req(seq, code, chans, active)) {
        Ok(s) => rep.ack = Some(s),
        Err(_) => {
            rep.why = Some("scan-timeout");
            return rep;
        }
    }
    let done_want = |eid: u8, _seq: u8, opt: u8| eid == EVT_SCAN_DONE && opt & OPT_UNSOLICITED != 0;
    let mut n = rep.n;
    let (got, waited) = spin_long(SCAN_WAIT_MS, || {
        drain(&mut *r, &mut n, &mut *on_frame);
        matches!(event_take(&mut *h, &done_want), Some(Some(_)))
    });
    let _ = spin_long(TAIL_MS, || {
        drain(&mut *r, &mut n, &mut *on_frame);
        false
    });
    rep.n = n;
    rep.done = if got { Some(waited) } else { None };
    if rep.done.is_none() {
        rep.why = Some("no-done");
    } else if rep.n.frames == 0 {
        rep.why = Some("rx-stall");
    }
    rep
}

/// `spin_until` in 1 ms slices for `ms`: S7's rounds read RAM descriptors only, so `spin_until`'s round cap alone could end a
/// long wait early; each slice is bounded by the tick or the cap, and at most `ms` slices run should the tick stall.
/// (done, ms waited).
fn spin_long(ms: u64, mut done: impl FnMut() -> bool) -> (bool, u64) {
    let t0 = crate::arch::ms();
    let mut slices = 0u64;
    while crate::arch::ms().saturating_sub(t0) < ms && slices < ms {
        if spin_until(1, &mut done).0 {
            return (true, crate::arch::ms().saturating_sub(t0));
        }
        slices += 1;
    }
    (false, crate::arch::ms().saturating_sub(t0))
}

/// S7: the scan, for the station's `tests wifiscan`. Each received beacon / probe response goes raw (RX descriptor stripped)
/// to `on_frame`, which answers true when it listed a new BSS. Ok = frames handed over (a band may still have refused: its
/// line says why); Err = the refusal word, said on the `[mt7925] s7` line. The channel goes back to `MCU_LINK` on every path.
pub fn scan(on_frame: &mut dyn FnMut(&[u8]) -> bool) -> Result<u32, &'static str> {
    let held = *PROBE.lock();
    let (Some(p), Some((tx0, rx2)), true) = (held, data_rings(), registered()) else {
        serial_println!("[mt7925] s7 -> NOT ATTEMPTED why=no-radio rel=found");
        return Err("no-radio");
    };
    let link = MCU_LINK.lock().take();
    let Some(mut h) = link else {
        serial_println!("[mt7925] s7 -> NOT ATTEMPTED why=no-link rel=found");
        return Err("no-link");
    };
    let res = scan_run(p.bar0, &mut h, tx0, rx2, on_frame);
    *MCU_LINK.lock() = Some(h); // put back on every path: the next scan re-wakes it
    res
}

/// Everything between the channel's take and its return: the pages, own, `held`, RX ring 2's buffer block, `wake` with the
/// data rings, RX ring 2's 31 buffers handed, the bands, own given back, the lines.
fn scan_run(b: u64, h: &mut Hw, tx0: u64, rx2: u64, on_frame: &mut dyn FnMut(&[u8]) -> bool) -> Result<u32, &'static str> {
    let Run { lp, took, kept, q2, rxb2, pre } = rx_open(b, h, tx0, rx2, "s7")?; // MT7925S8 (Z38): the prologue hoisted unchanged into `rx_open`
    let path = S7_PATH.load(Ordering::Relaxed);
    let mut reps: Vec<BandRep> = Vec::new();
    if pre.is_ok() {
        hand_rx(rx2, rxb2);
        w32(q2 + 8, RX_BUFS); dev_bss_once(h); // RX ring 2's 31 buffers handed (ring 0's convention: descriptor 31 unhanded)
        let mut r = Rx2 { q: q2, desc: rx2, bufs: rxb2, ri: 0 };
        if path == PATH_UNREAD || path & PATH_2G != 0 {
            reps.push(band(h, &mut r, "2g", BAND_2G, &CH_2G, true, &mut *on_frame));
        }
        if path != PATH_UNREAD && path & PATH_5G != 0 {
            reps.push(band(h, &mut r, "5g", BAND_5G, &CH_5G, false, &mut *on_frame));
        }
    }
    let rel = give_back(lp, took);
    if let Err(w) = pre {
        serial_println!("[mt7925] s7 -> NOT ATTEMPTED why={} rel={}", w, rel);
        return Err("pre");
    }
    for r in &reps {
        let ack = r.ack.map_or(String::from("-"), |a| alloc::format!("{:#04x}", a));
        match r.why {
            None => serial_println!(
                "[mt7925] s7 scan band={} type={} req={} ack={} done={} frames={} skip={} bss={} -> parked",
                r.name, r.kind, r.req, ack, r.done.unwrap_or(0), r.n.frames, r.n.skip, r.n.bss
            ),
            Some(w) => serial_println!(
                "[mt7925] s7 scan band={} type={} req={} -> REFUSED why={} ack={} frames={} skip={} bss={}",
                r.name, r.kind, r.req, w, ack, r.n.frames, r.n.skip, r.n.bss
            ),
        }
    }
    let ps = if path == PATH_UNREAD { String::from("-") } else { alloc::format!("{:#04x}", path) };
    serial_println!("[mt7925] s7 path={} held={} rel={}", ps, kept, rel);
    Ok(reps.iter().map(|r| r.n.frames).sum())
}

/// `tests wifiscan`'s known-answer rows (hand-assembled, no capture): the 2.4 GHz active request's length, TLV order and
/// lengths at their offsets, scan type, probe count, split bit, dwell 60 / timeout 660, wildcard SSID, 11 listed channels;
/// the 5 GHz passive request's dwell 120 / timeout 1080 and first channel 36; three RX descriptors: a normal frame with G4+G1
/// and pad 1 starts at 66, a translated one and a packet type 7 are refused.
pub fn scan_kat() -> bool {
    let v = scan_req(5, BAND_2G, &CH_2G, true);
    fn u(v: &[u8], o: usize) -> u16 {
        u16::from_le_bytes([v[o], v[o + 1]])
    }
    let mut ok = v.len() == 948 && v[0] == 5 && v[1] == 0;
    let tags = [
        (4, SCAN_TAG_REQ, SCAN_REQ_LEN),
        (24, SCAN_TAG_SSID, SCAN_SSID_LEN),
        (176, SCAN_TAG_BSSID, SCAN_BSSID_LEN),
        (192, SCAN_TAG_CHANNEL, SCAN_CHAN_LEN),
        (328, SCAN_TAG_IE, SCAN_IE_LEN),
        (936, SCAN_TAG_MISC, SCAN_MISC_LEN),
    ];
    for (o, t, l) in tags {
        ok &= u(&v, o) == t && u(&v, o + 2) as usize == l;
    }
    ok &= v[8] == 1 && v[9] == 2 && v[10] == SCAN_FUNC_SPLIT && u(&v, 12) == 60 && u(&v, 14) == 60 && u(&v, 16) == 660;
    ok &= v[28] == SSID_WILDCARD && v[29] == 0;
    ok &= v[196] == CHAN_LISTED && v[197] == 11 && v[200] == BAND_2G && v[201] == 1 && v[220] == BAND_2G && v[221] == 11;
    let p = scan_req(1, BAND_5G, &CH_5G, false);
    ok &= p[8] == 0 && p[9] == 0 && u(&p, 14) == 120 && u(&p, 16) == 1080 && p[197] == 9 && p[200] == BAND_5G && p[201] == 36;
    let mut f = alloc::vec![0u8; 90];
    f[0..4].copy_from_slice(&((PKT_NORMAL << 27) | 90).to_le_bytes());
    f[4..8].copy_from_slice(&(RXD1_G4 | RXD1_G1).to_le_bytes());
    f[8..12].copy_from_slice(&(1u32 << 13).to_le_bytes());
    ok &= rx_frame(&f) == Some(66..90);
    f[8..12].copy_from_slice(&((1u32 << 13) | RXD2_HDR_TRANS).to_le_bytes());
    ok &= rx_frame(&f).is_none();
    f[8..12].copy_from_slice(&(1u32 << 13).to_le_bytes());
    f[0..4].copy_from_slice(&((7u32 << 27) | 90).to_le_bytes());
    ok &= rx_frame(&f).is_none();
    ok
}

// ── MT7925S8 (zenbook-ledger Z38) — S8: DEV_INFO / BSS_INFO, the channel, the open-system auth and assoc frames ──────────
// Only from the operator's `tests wifijoin <ssid>` (wifi/verb.rs `wifijoin`), never at boot (BP-net-7), over the channel S6 parked in
// `MCU_LINK` and the data rings in `RADIO`; open BSSes only (WPA2 is S10). Linux's add_interface sends two acked unified commands before
// anything else: DEV_INFO_UPDATE (0x01: the own MAC, the device index) and BSS_INFO_UPDATE (0x02: a BASIC TLV, bss 0, connection type STA);
// `dev_bss_once` sends them once per boot (S7 calls it before its first scan: S7-DEV). A join then sends BSS_INFO with the target BSSID and,
// per frame, a ROC (0x27, JOIN, 1 s) whose GRANT is an unsolicited event on ring 0 (polled through the ONE consumer `event_take`; the grant
// carries the band index) and a ROC abort after. The host builds the frame (the station's `wifi_core` builders, handed in as closures: the
// driver links no codec) and posts it on TX ring 0 as linux does: ONE descriptor, one buffer = a 32-byte connac3 TXD + a 32-byte hw txp whose
// first pointer names the 802.11 frame (`post` + `tx_done`, S4's); wlan idx 19 = the interface's own wcid (WTBL_RESERVED), fixed rate by table
// index, queue AC03. The reply is whatever RX ring 2's `drain` hands the station's judge closure (S7's frame filter). No STA_REC (owed S8-STAREC),
// no key, no interrupt enable, no config write. Hardware facts only; no code copied.

const UNI_DEV_INFO: u16 = 0x01;
const UNI_BSS_INFO: u16 = 0x02;
const UNI_ROC: u16 = 0x27;
const EVT_ROC: u8 = 0x27;
const ROC_TAG_ACQUIRE: u16 = 0;
const ROC_TAG_ABORT: u16 = 1;
/// The interface's own (bmc) wlan index: MT792x_WTBL_RESERVED - 0.
const WCID_BMC: u16 = 19;
/// CONNECTION_INFRA_STA: STA_TYPE_STA | NETWORK_INFRA.
const CONN_INFRA_STA: u32 = 0x0001_0001;
const BAND_UNSET: u8 = 0xff;
const ROC_JOIN_MS: u32 = 1000;
const ROC_WAIT_MS: u64 = 1000;
const REPLY_MS: u64 = 1000;
/// The TX block: four 1 KiB slots, each a 64-byte TXD + txp at +0 and the frame at +128 (at most MGMT_MAX bytes).
const TX_BLOCK: usize = 4096;
const TX_SLOT: u64 = 1024;
const TX_SLOTS: u32 = 4;
const TX_FRAME_OFF: u64 = 128;
const MGMT_MAX: usize = 800;
const TXWI_BYTES: u32 = 64;
/// Management frames ride mac80211's VO queue: wmm 0 * 4 + (3 - 0).
const MGMT_Q_IDX: u32 = 3;
const HDR_802_11: u32 = 2;
/// The fixed-rate TABLE slots (MT792x_BASIC_RATES_TBL): the CCK 1M slot on 2.4 GHz, the OFDM 6M slot (+4) on 5 GHz.
const RATE_TBL_2G: u32 = 11;
const RATE_TBL_5G: u32 = 15;
const PHY_MODE_A: u8 = 1 << 0;
const PHY_MODE_BG: u8 = (1 << 1) | (1 << 2);
const NONHT_ERP: u16 = 1;
const NONHT_OFDM: u16 = 3;
/// The grant event: the TLV starts after the 44-byte rxd and 4 reserved bytes; status @54, dbdcband @62.
const GRANT_TLV: usize = RXD_BYTES + 4;

/// DEV_INFO and BSS_INFO acked once this boot.
static S8_DEV: AtomicBool = AtomicBool::new(false);
/// The 4 KiB TX block (below 4 GiB, `dma::ident` once, never freed: S3's rule).
static S8_TXB: AtomicU64 = AtomicU64::new(0);
/// The last ROC token (1..127).
static S8_TOK: AtomicU8 = AtomicU8::new(0);

/// The station's answer to one received frame: Ok(0) = authentication accepted / Ok(aid) = associated; Refused(code) = a status
/// code or a deauth / disassoc reason.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Ok(u16),
    Refused(u16),
}

/// How one exchange ended.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Skipped,
    Timeout,
    Tx(&'static str),
    Ans(Answer),
}

/// The target the station chose from its scan.
pub struct JoinPlan<'a> {
    pub ssid: &'a str,
    pub bssid: [u8; 6],
    pub channel: u8,
}

/// What `join` did.
pub struct JoinRep {
    pub auth: Step,
    pub assoc: Step,
}

/// What `rx_open` leaves the caller: own, the channel's ring state, RX ring 2's registers and buffer block, `wake`'s verdict.
struct Run {
    lp: u64,
    took: bool,
    kept: &'static str,
    q2: u64,
    rxb2: u64,
    pre: Result<(), String>,
}

/// TX ring 0 as one join posts on it: registers, the next CPU index, the TX block, the band index the grant named, 5 GHz.
struct Tx {
    q: u64,
    wi: u32,
    blk: u64,
    band: u8,
    five: bool,
}

/// What one ROC did: the ack's status byte, the grant (status, band index, ms waited).
struct Roc {
    ack: Option<u8>,
    grant: Option<(u8, u8, u64)>,
}

/// S7's prologue, hoisted unchanged out of `scan_run` (MT7925S8, Z38; the writes and the s7 lines are the same, the tag is `who`):
/// the pages, own (`take_own`), `kept`, RX ring 2's 64 KiB buffer block, `wake` with the data rings.
fn rx_open(b: u64, h: &mut Hw, tx0: u64, rx2: u64, who: &'static str) -> Result<Run, &'static str> {
    let lp = b + CONN_HOST_CSR_TOP + LPCTL;
    if !(map_page(b + WFDMA0) && map_page(b + (DMASHDL_SW_CONTROL & !0xfff))) {
        serial_println!("[mt7925] {} -> NOT ATTEMPTED why=unmapped rel=found", who);
        return Err("unmapped");
    }
    let took = match take_own(lp) {
        Ok(t) => t,
        Err((last, rel)) => {
            serial_println!("[mt7925] {} -> NOT ATTEMPTED why=own lpctl={:#010x} rel={}", who, last, rel);
            return Err("own");
        }
    };
    let kept = if r32(h.q15) as u64 == h.wm && r32(h.q0) as u64 == h.rx0 { "kept" } else { "lost" };
    let mut rxb2 = S7_RXB.load(Ordering::Relaxed);
    if rxb2 == 0 {
        let v = crate::arch::memory::dma::ident(RX_BLOCK, 4096);
        if v != 0 && v + RX_BLOCK as u64 <= DMA_LIMIT {
            rxb2 = v;
            S7_RXB.store(v, Ordering::Relaxed);
        }
    }
    let q2 = b + RX_EVENT_RING_BASE + RXQ_DATA * RING_REGS;
    let pre = if rxb2 == 0 {
        Err(String::from("no-memory")) // nothing written to a ring register
    } else {
        let data = [(b + TX_RING_BASE + TXQ_DATA * RING_REGS, tx0), (q2, rx2)];
        wake(b, h, &data)
    };
    Ok(Run { lp, took, kept, q2, rxb2, pre })
}

/// DEV_INFO body (pure, 16 bytes): header {omac 0, band unset, 2 pad}, TLV {tag 0, len 12, active 1, link 0, the own MAC}.
fn dev_req(m: [u8; 6]) -> [u8; 16] {
    let mut v = [0u8; 16];
    v[1] = BAND_UNSET;
    v[6] = 12;
    v[8] = 1;
    v[10..16].copy_from_slice(&m);
    v
}

/// BSS_INFO body (pure, 36 bytes): header {bss 0, 3 pad}, the BASIC TLV (tag 0, len 32): active, omac 0, hw bss 0, `band`, STA/INFRA,
/// conn_state 1 (not connected), wmm 0, `bssid`, the bmc and sta wlan index 19, link 0. `phy` = Some(5 GHz?) is a join's: beacon
/// interval 100 (`parse_beacon` does not carry it: S8-BCNINT), the legacy phy mode and the non-HT basic phy of the band.
fn bss_req(bssid: [u8; 6], band: u8, phy: Option<bool>) -> [u8; 36] {
    let mut v = [0u8; 36];
    v[6] = 32;
    v[8] = 1;
    v[11] = band;
    v[12..16].copy_from_slice(&CONN_INFRA_STA.to_le_bytes());
    v[16] = 1;
    v[18..24].copy_from_slice(&bssid);
    v[24..26].copy_from_slice(&WCID_BMC.to_le_bytes());
    v[30..32].copy_from_slice(&WCID_BMC.to_le_bytes());
    if let Some(five) = phy {
        v[26..28].copy_from_slice(&100u16.to_le_bytes());
        v[29] = if five { PHY_MODE_A } else { PHY_MODE_BG };
        v[32..34].copy_from_slice(&(if five { NONHT_OFDM } else { NONHT_ERP }).to_le_bytes());
    }
    v
}

/// ROC ACQUIRE body (pure, 28 bytes): 4 reserved, TLV {tag 0, len 24, bss 0, token, control channel `ch`, sco 0, band 1 (2.4) / 2 (5),
/// width 0, center `ch`, center2 0, width/center from the AP likewise, request type JOIN, 1000 ms, band index unset}.
fn roc_req(tok: u8, ch: u8, five: bool) -> [u8; 28] {
    let mut v = [0u8; 28];
    v[4..6].copy_from_slice(&ROC_TAG_ACQUIRE.to_le_bytes());
    v[6] = 24;
    v[9] = tok;
    v[10] = ch;
    v[12] = if five { 2 } else { 1 };
    v[14] = ch;
    v[17] = ch;
    v[20..24].copy_from_slice(&ROC_JOIN_MS.to_le_bytes());
    v[24] = BAND_UNSET;
    v
}

/// ROC ABORT body (pure, 16 bytes): 4 reserved, TLV {tag 1, len 12, bss 0, token, band index unset, 5 reserved}.
fn roc_abort(tok: u8) -> [u8; 16] {
    let mut v = [0u8; 16];
    v[4..6].copy_from_slice(&ROC_TAG_ABORT.to_le_bytes());
    v[6] = 12;
    v[9] = tok;
    v[10] = BAND_UNSET;
    v
}

/// The grant event's (status, band index) (pure): the TLV after the rxd must be tag 0 and 64 bytes must be there.
fn roc_grant(ev: &[u8]) -> Option<(u8, u8)> {
    if ev.len() < GRANT_TLV + 16 || ev[GRANT_TLV] != 0 || ev[GRANT_TLV + 1] != 0 {
        return None;
    }
    Some((ev[GRANT_TLV + 6], ev[GRANT_TLV + 14]))
}

/// The 64-byte TXD + hw txp of one management frame (pure; `frame` >= 24 bytes at bus address `pa`, `tok` the msdu id): d0 length
/// + 32, CT, queue 3; d1 wlan 19, own MAC 0, band index (TGID, when not 0), 802.11 header format, header info 12, tid 0, fixed
/// rate; d2 subtype | type; d3 remaining tx count 15, BA disable (+ BCM for a group address); d6 DAS, DIS_MAT, one MSDU, the
/// fixed-rate table slot; d4, d5, d7 zero. txp: msdu id | valid, then pointer 0 = (`pa`, length | last).
fn mgmt_txwi(frame: &[u8], pa: u64, tok: u8, five: bool, band: u8) -> [u8; 64] {
    let fc = frame[0] as u32;
    let mut d1 = WCID_BMC as u32 | (HDR_802_11 << 14) | (12 << 16) | (1 << 31);
    if band != 0 {
        d1 |= (band as u32) << 12;
    }
    let mut d3 = (15u32 << 11) | (1 << 28);
    if frame[4] & 1 != 0 {
        d3 |= 1 << 4;
    }
    let d = [
        (frame.len() as u32 + 32) | (MGMT_Q_IDX << 25),
        d1,
        (fc >> 4) | (((fc >> 2) & 3) << 4),
        d3,
        0,
        0,
        (1 << 2) | (1 << 3) | (1 << 4) | ((if five { RATE_TBL_5G } else { RATE_TBL_2G }) << 16),
        0,
    ];
    let mut v = [0u8; 64];
    for (i, w) in d.iter().enumerate() {
        v[4 * i..4 * i + 4].copy_from_slice(&w.to_le_bytes());
    }
    v[32..34].copy_from_slice(&(tok as u16 | 0x8000).to_le_bytes());
    v[40..44].copy_from_slice(&(pa as u32).to_le_bytes());
    v[44..46].copy_from_slice(&(frame.len() as u16 | 0x8000).to_le_bytes());
    v
}

/// An ack's `0x..` text, `-` when the command did not complete.
fn ack_text(r: &Result<u8, &'static str>) -> String {
    match r {
        Ok(a) => alloc::format!("{:#04x}", a),
        Err(_) => String::from("-"),
    }
}

/// S7-DEV: linux's add_interface pair, once per boot, from the first scan or join: DEV_INFO then BSS_INFO (no BSSID), each acked.
/// Bounded by `uni_cmd`'s own waits; a refusal is said by name and does not stop the caller (it retries at the next call).
fn dev_bss_once(h: &mut Hw) {
    if S8_DEV.load(Ordering::Relaxed) {
        return;
    }
    let Some(m) = mac() else {
        serial_println!("[mt7925] s8 dev -> REFUSED why=no-mac");
        return;
    };
    let d = uni_cmd(h, UNI_DEV_INFO, &dev_req(m));
    let bs = if d.is_ok() { uni_cmd(h, UNI_BSS_INFO, &bss_req([0u8; 6], BAND_UNSET, None)) } else { Err("skipped") };
    let ok = d == Ok(0) && bs == Ok(0);
    if ok {
        S8_DEV.store(true, Ordering::Relaxed);
    }
    let why = match (&d, &bs) {
        (Err(e), _) | (_, Err(e)) => *e,
        _ => "ack",
    };
    serial_println!(
        "[mt7925] s8 dev mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} omac=0 wcid={} dev={} bss={} -> {}",
        m[0], m[1], m[2], m[3], m[4], m[5], WCID_BMC, ack_text(&d), ack_text(&bs),
        if ok { String::from("parked") } else { alloc::format!("REFUSED why={}", why) }
    );
}

/// One management frame out on TX ring 0: the frame into the slot's +128, `mgmt_txwi` at its head, `post` (S4's one-buffer
/// descriptor, the CPU index last), then `tx_done` (the DMA index caught up). Err = `frame-len` or `tx0`.
fn tx_mgmt(t: &mut Tx, frame: &[u8], tok: u8) -> Result<(), &'static str> {
    if frame.len() < 24 || frame.len() > MGMT_MAX {
        return Err("frame-len");
    }
    let slot = t.blk + (t.wi % TX_SLOTS) as u64 * TX_SLOT;
    let pa = slot + TX_FRAME_OFF;
    for (i, v) in frame.iter().enumerate() {
        // SAFETY: inside slot `wi % 4` of the 4 KiB block `join_run` allocated (frame.len() <= MGMT_MAX, offset 128).
        unsafe { core::ptr::write_volatile((pa + i as u64) as *mut u8, *v) };
    }
    let w = mgmt_txwi(frame, pa, tok, t.five, t.band);
    for (i, v) in w.iter().enumerate() {
        // SAFETY: the slot's first 64 bytes, same block.
        unsafe { core::ptr::write_volatile((slot + i as u64) as *mut u8, *v) };
    }
    t.wi = post(t.q, t.wi, slot, TXWI_BYTES);
    if tx_done(t.q, t.wi) { Ok(()) } else { Err("tx0") }
}

/// ROC ACQUIRE through `uni_cmd`, then up to ROC_WAIT_MS of ring-0 events for the unsolicited GRANT (`event_take`, the ONE consumer;
/// the matched event's bytes are in `h.last`). No grant is not fatal here: the caller says so and goes on.
fn roc(h: &mut Hw, tok: u8, ch: u8, five: bool) -> Roc {
    let a = uni_cmd(h, UNI_ROC, &roc_req(tok, ch, five));
    let mut r = Roc { ack: a.ok(), grant: None };
    if a.is_ok() {
        let want = |eid: u8, _seq: u8, opt: u8| eid == EVT_ROC && opt & OPT_UNSOLICITED != 0;
        let (got, ms) = spin_long(ROC_WAIT_MS, || matches!(event_take(&mut *h, &want), Some(Some(_))));
        if got {
            r.grant = roc_grant(&h.last).map(|(s, b)| (s, b, ms));
        }
    }
    r
}

/// One request/response exchange: ROC (its line), `build(phase)`, `tx_mgmt`, then up to REPLY_MS of RX ring 2 `drain`s with `judge`
/// as the frame filter (the first answer wins; ring 0 is drained of strays each round), then ROC ABORT.
fn exchange(
    h: &mut Hw, r: &mut Rx2, t: &mut Tx, phase: u8, ch: u8,
    build: &mut dyn FnMut(u8) -> Vec<u8>, judge: &mut dyn FnMut(u8, &[u8]) -> Option<Answer>,
) -> Step {
    let tok = S8_TOK.load(Ordering::Relaxed) % 127 + 1;
    S8_TOK.store(tok, Ordering::Relaxed);
    let rc = roc(h, tok, ch, t.five);
    let grant = rc.grant.map_or(String::from("none"), |(s, b, ms)| alloc::format!("status={} band={} ms={}", s, b, ms));
    serial_println!(
        "[mt7925] s8 roc phase={} ch={} band={} ack={} grant={}",
        if phase == 0 { "auth" } else { "assoc" }, ch, if t.five { "5g" } else { "2g" },
        rc.ack.map_or(String::from("-"), |a| alloc::format!("{:#04x}", a)), grant
    );
    if let Some((_, band, _)) = rc.grant {
        if band < 2 {
            t.band = band;
        }
    }
    let frame = build(phase);
    if let Err(w) = tx_mgmt(t, &frame, tok) {
        let _ = uni_cmd(h, UNI_ROC, &roc_abort(tok));
        return Step::Tx(w);
    }
    let mut ans: Option<Answer> = None;
    let mut n = Counts::default();
    let _ = spin_long(REPLY_MS, || {
        drain(&mut *r, &mut n, &mut |f: &[u8]| {
            if ans.is_none() {
                ans = judge(phase, f);
            }
            false
        });
        let _ = event_take(&mut *h, &|_: u8, _: u8, _: u8| false);
        ans.is_some()
    });
    let _ = uni_cmd(h, UNI_ROC, &roc_abort(tok));
    match ans {
        Some(a) => Step::Ans(a),
        None => Step::Timeout,
    }
}

/// A step's wire word; `assoc` shows an Ok as the AID.
fn step_text(s: &Step, assoc: bool) -> String {
    match s {
        Step::Skipped => String::from("skipped"),
        Step::Timeout => String::from("timeout"),
        Step::Tx(w) => alloc::format!("tx:{}", w),
        Step::Ans(Answer::Ok(v)) => if assoc { alloc::format!("aid={}", v) } else { String::from("ok") },
        Step::Ans(Answer::Refused(c)) => alloc::format!("refused code={}", c),
    }
}

/// S8: the open-system join for the station's `tests wifijoin`. `build(0)` is the authentication request, `build(1)` the association
/// request; `judge(phase, frame)` answers a received frame (None = not the answer). Err = the refusal word, said on the `[mt7925] s8`
/// line. The channel goes back to `MCU_LINK` on every path.
pub fn join(
    plan: &JoinPlan, build: &mut dyn FnMut(u8) -> Vec<u8>, judge: &mut dyn FnMut(u8, &[u8]) -> Option<Answer>,
) -> Result<JoinRep, &'static str> {
    let held = *PROBE.lock();
    let (Some(p), Some((tx0, rx2)), true) = (held, data_rings(), registered()) else {
        serial_println!("[mt7925] s8 -> NOT ATTEMPTED why=no-radio rel=found");
        return Err("no-radio");
    };
    let link = MCU_LINK.lock().take();
    let Some(mut h) = link else {
        serial_println!("[mt7925] s8 -> NOT ATTEMPTED why=no-link rel=found");
        return Err("no-link");
    };
    let res = join_run(p.bar0, &mut h, tx0, rx2, plan, build, judge);
    *MCU_LINK.lock() = Some(h); // put back on every path
    res
}

/// Everything between the channel's take and its return: the TX block, `rx_open`, RX ring 2's buffers, S7-DEV, BSS_INFO with the
/// target, the two exchanges, own given back, the line.
fn join_run(
    b: u64, h: &mut Hw, tx0: u64, rx2: u64, plan: &JoinPlan,
    build: &mut dyn FnMut(u8) -> Vec<u8>, judge: &mut dyn FnMut(u8, &[u8]) -> Option<Answer>,
) -> Result<JoinRep, &'static str> {
    let mut txb = S8_TXB.load(Ordering::Relaxed);
    if txb == 0 {
        let v = crate::arch::memory::dma::ident(TX_BLOCK, 4096);
        if v != 0 && v + TX_BLOCK as u64 <= DMA_LIMIT {
            txb = v;
            S8_TXB.store(v, Ordering::Relaxed);
        }
    }
    if txb == 0 {
        serial_println!("[mt7925] s8 -> NOT ATTEMPTED why=no-txmem rel=found");
        return Err("no-txmem");
    }
    let Run { lp, took, kept, q2, rxb2, pre } = rx_open(b, h, tx0, rx2, "s8")?;
    let five = plan.channel >= 36;
    let mut rep = JoinRep { auth: Step::Skipped, assoc: Step::Skipped };
    let mut bss: Result<u8, &'static str> = Err("skipped");
    let mut posted = 0u32;
    if pre.is_ok() {
        hand_rx(rx2, rxb2);
        w32(q2 + 8, RX_BUFS); // RX ring 2's 31 buffers handed (S7's convention)
        dev_bss_once(h);
        bss = uni_cmd(h, UNI_BSS_INFO, &bss_req(plan.bssid, BAND_UNSET, Some(five)));
        let mut r = Rx2 { q: q2, desc: rx2, bufs: rxb2, ri: 0 };
        let mut t = Tx { q: b + TX_RING_BASE + TXQ_DATA * RING_REGS, wi: 0, blk: txb, band: 0, five };
        rep.auth = exchange(h, &mut r, &mut t, 0, plan.channel, &mut *build, &mut *judge);
        if matches!(rep.auth, Step::Ans(Answer::Ok(_))) {
            rep.assoc = exchange(h, &mut r, &mut t, 1, plan.channel, &mut *build, &mut *judge);
        }
        posted = t.wi;
    }
    let rel = give_back(lp, took);
    if let Err(w) = pre {
        serial_println!("[mt7925] s8 -> NOT ATTEMPTED why={} rel={}", w, rel);
        return Err("pre");
    }
    let m = plan.bssid;
    serial_println!(
        "[mt7925] s8 join ssid=\"{}\" bssid={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} ch={} tx0={} bss={} auth={} assoc={} held={} rel={} -> parked",
        plan.ssid, m[0], m[1], m[2], m[3], m[4], m[5], plan.channel, posted, ack_text(&bss),
        step_text(&rep.auth, false), step_text(&rep.assoc, true), kept, rel
    );
    Ok(rep)
}

/// `tests wifijoin`'s known-answer rows (hand-computed from the field table, no capture): the TXD + txp of a 30-byte authentication
/// frame on 2.4 GHz and 5 GHz and a 52-byte association request, the DEV_INFO, BSS_INFO (idle and a join's on both bands), ROC
/// ACQUIRE and ABORT bodies, and `roc_grant` over a canned event.
pub fn join_kat() -> bool {
    fn u32_at(v: &[u8], o: usize) -> u32 {
        u32::from_le_bytes([v[o], v[o + 1], v[o + 2], v[o + 3]])
    }
    fn u16_at(v: &[u8], o: usize) -> u16 {
        u16::from_le_bytes([v[o], v[o + 1]])
    }
    let mut auth = alloc::vec![0u8; 30];
    auth[0] = 0xb0;
    let a = mgmt_txwi(&auth, 0x1080, 1, false, 0);
    let mut ok = a.len() == 64
        && u32_at(&a, 0) == 0x0600_003e && u32_at(&a, 4) == 0x800c_8013 && u32_at(&a, 8) == 0x0b
        && u32_at(&a, 12) == 0x1000_7800 && u32_at(&a, 16) == 0 && u32_at(&a, 20) == 0
        && u32_at(&a, 24) == 0x000b_001c && u32_at(&a, 28) == 0
        && u32_at(&a, 32) == 0x8001 && u32_at(&a, 40) == 0x1080 && u32_at(&a, 44) == 0x801e;
    let f = mgmt_txwi(&auth, 0x1080, 1, true, 0);
    ok &= u32_at(&f, 24) == 0x000f_001c && u32_at(&f, 4) == 0x800c_8013;
    let mut req = alloc::vec![0u8; 52];
    req[0] = 0x00;
    let c = mgmt_txwi(&req, 0x1480, 2, false, 0);
    ok &= u32_at(&c, 0) == 0x0600_0054 && u32_at(&c, 8) == 0 && u32_at(&c, 32) == 0x8002
        && u32_at(&c, 40) == 0x1480 && u32_at(&c, 44) == 0x8034;
    let g = mgmt_txwi(&auth, 0x1080, 1, false, 1);
    ok &= u32_at(&g, 4) == 0x800c_9013;
    let mac = [0x02, 0x55, 0x4e, 0x41, 0x4f, 0x53];
    let d = dev_req(mac);
    ok &= d[..10] == [0, 0xff, 0, 0, 0, 0, 12, 0, 1, 0] && d[10..] == mac;
    let bssid = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
    let i = bss_req(bssid, BAND_UNSET, None);
    ok &= i.len() == 36 && i[6] == 32 && i[8] == 1 && i[11] == 0xff && u32_at(&i, 12) == 0x0001_0001 && i[16] == 1
        && i[18..24] == bssid && u16_at(&i, 24) == 19 && u16_at(&i, 30) == 19 && i[29] == 0 && u16_at(&i, 26) == 0;
    let j = bss_req(bssid, BAND_UNSET, Some(false));
    ok &= u16_at(&j, 26) == 100 && j[29] == 6 && u16_at(&j, 32) == 1;
    let k = bss_req(bssid, BAND_UNSET, Some(true));
    ok &= k[29] == 1 && u16_at(&k, 32) == 3;
    let r = roc_req(7, 6, false);
    ok &= r.len() == 28 && u16_at(&r, 4) == 0 && r[6] == 24 && r[9] == 7 && r[10] == 6 && r[12] == 1 && r[14] == 6
        && r[17] == 6 && r[19] == 0 && u32_at(&r, 20) == 1000 && r[24] == 0xff && roc_req(7, 36, true)[12] == 2;
    let x = roc_abort(7);
    ok &= x.len() == 16 && u16_at(&x, 4) == 1 && x[6] == 12 && x[9] == 7 && x[10] == 0xff;
    let mut ev = alloc::vec![0u8; 68];
    ev[GRANT_TLV + 6] = 0;
    ev[GRANT_TLV + 14] = 0;
    ok &= roc_grant(&ev) == Some((0, 0));
    ev[GRANT_TLV + 6] = 1;
    ev[GRANT_TLV + 14] = 1;
    ok &= roc_grant(&ev) == Some((1, 1)) && roc_grant(&ev[..60]).is_none();
    ev[GRANT_TLV] = 4;
    ok &= roc_grant(&ev).is_none();
    ok
}
