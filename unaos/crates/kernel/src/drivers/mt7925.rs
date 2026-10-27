//! CHARTER: Kernel — driver (the MediaTek MT7925 Wi-Fi 7 radio's S1 probe, zenbook-ledger Z5, S2 driver-own and the chip id, Z11, and S3 the WFSYS reset and the WFDMA rings, Z15, and S4 the firmware upload, Z17, and S5 the NIC capability and MAC, Z23, and S6 the radio backend registered with the data rings armed, Z29, and S7 the scan, Z34, and S8 DEV_INFO/BSS_INFO, the channel and the open-system auth/assoc frames, Z38, and S9 the peer STA_REC and the data path, Z46, and S10 the WPA2-PSK EAPOL path and the radio's keys, Z55; no CODEX §2 handler owns a NIC driver, the BCM4331 `wifi/**` sits in the same seam)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! MT7925 — S1, the probe (ZENBOOKMT7925, zenbook-ledger Z5). The ladder is
//! `docs/dev/OS/06_NETWORK_STACK/mt7925.md`; S2 (MT7925S2, Z11), S3 (MT7925S3, Z15), S4 (MT7925S4, Z17), S5 (MT7925S5, Z23), S6 (MT7925S6, Z29), S7 (MT7925S7, Z34), S8 (MT7925S8, Z38), S9 (MT7925S9, Z46) and S10 (MT7925S10, Z55) are the nine file-tail blocks.
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
        _ => watch(), // S9WATCH (zenbook-ledger Z56): the parked arm watches the S9 link (one atomic load with no link)
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
// both by name (`why=s7-not-built`, retired by MT7925S9 Z46: the S9 link carries them). S6 re-takes own, runs S5's wake reset again through the ONE `wake` with linux's two
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

/// The MT7925 radio backend (S6): the kernel's one NIC trait (`net_phy::RawNic`) over the parked channel. MT7925S9 (Z46): the
/// S9 link (`S9_LINK`, filled by `join` after the association) carries the frames, 802.3 both ways; `drivers/e1000.rs`'s accessor
/// triple reaches it after the other links. With no link every operation but `mac` is refused by name, never a silent Ok.
pub struct MtRadio;

#[cfg(feature = "smolnet")]
impl crate::net_phy::RawNic for MtRadio {
    /// The next header-translated (Ethernet) frame from RX ring 2 (`link_rx`); None before the stack bound the link.
    fn rx_frame_raw(out: &mut [u8]) -> Option<usize> {
        if !bound() {
            return None;
        }
        link_rx(out)
    }
    /// One Ethernet frame posted on TX ring 0 behind the 802.3 TXD (`link_tx`); dropped and counted when there is no room.
    fn transmit(frame: &[u8]) {
        link_tx(frame)
    }
    /// S5's capability-reply MAC, only once S6 registered the radio.
    fn mac() -> Option<[u8; 6]> {
        if registered() { self::mac() } else { None }
    }
    /// Room on TX ring 0 for one more frame (`tx_room`); false with no link, so smoltcp keeps the packet.
    fn tx_ready() -> bool {
        tx_room()
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
    match rx_span(buf) { // MT7925S9 (Z46): the descriptor walk is the ONE `rx_span` (S9's tail); S7 keeps untranslated frames of >= 24 bytes
        Some((r, false)) if r.len() >= 24 => Some(r),
        _ => None,
    }
}

/// Every DMA_DONE descriptor of RX ring 2 in order, at most 32 per call: the bytes (SD_LEN0, capped at the buffer; a frame
/// split over buffers keeps nothing) copied out, the buffer re-handed (`rearm`), then the frame to the station's closure
/// (true = it listed a new BSS).
fn drain(r: &mut Rx2, n: &mut Counts, on_frame: &mut dyn FnMut(&[u8]) -> bool) {
    for _ in 0..RING_DEPTH {
        let Some(v) = rx_take(r) else { return }; // MT7925S9 (Z46): the per-descriptor read + `rearm` is the ONE `rx_take` (S9's tail), unchanged and in order
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
// index, queue AC03. The reply is whatever RX ring 2's `drain` hands the station's judge closure (S7's frame filter). The AP's STA_REC is S9's
// (MT7925S9, Z46: S8-STAREC built in `join_run`), no key, no interrupt enable, no config write. Hardware facts only; no code copied.

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
    /// S9WATCH (zenbook-ledger Z56): the station's deauth / disassoc judge (`wifi/assoc.rs::refusal`), handed in like S8's judges
    /// and kept in the S9 link, so the driver links no wifi_core.
    pub watch: Judge,
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

/// TX ring 0 as one join posts on it: registers, the next CPU index, the TX block, the band index the grant named, 5 GHz, and
/// (MT7925S9, Z46) the wlan index the frames ride: the AP peer's 1 once its STA_REC was acked, else the interface's own 19.
struct Tx {
    q: u64,
    wi: u32,
    blk: u64,
    band: u8,
    five: bool,
    wcid: u16,
}

/// What one ROC did: the ack's status byte, the grant (status, band index, ms waited).
struct Roc {
    ack: Option<u8>,
    grant: Option<(u8, u8, u64)>,
}

/// S7's prologue, hoisted unchanged out of `scan_run` (MT7925S8, Z38; the writes and the s7 lines are the same, the tag is `who`):
/// the pages, own (`take_own`), `kept`, RX ring 2's 64 KiB buffer block, `wake` with the data rings.
fn rx_open(b: u64, h: &mut Hw, tx0: u64, rx2: u64, who: &'static str) -> Result<Run, &'static str> {
    let was = link_down(who, Some(&mut *h)); // S9REJOIN (zenbook-ledger Z59, S9-UNLINK): the channel in hand, so the firmware's records of the old link are unwound too. MT7925S9 (Z46): a scan or join re-wakes the rings, so the S9 link ends first (its line); S9WATCH (Z56): through the ONE link end
    let lp = b + CONN_HOST_CSR_TOP + LPCTL;
    if !(map_page(b + WFDMA0) && map_page(b + (DMASHDL_SW_CONTROL & !0xfff))) {
        serial_println!("[mt7925] {} -> NOT ATTEMPTED why=unmapped rel=found", who);
        return Err("unmapped");
    }
    let took = match take_own(lp) {
        Ok(t) => t || was, // MT7925S9 (Z46): own the ended link kept is given back by this run
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
/// conn_state 1 (not connected), wmm 0, `bssid`, the bmc wlan index 19, link 0. `phy` = Some(5 GHz?) is a join's: beacon
/// interval 100 (`parse_beacon` does not carry it: S8-BCNINT), the legacy phy mode and the non-HT basic phy of the band.
/// MT7925S9 (Z46): `sta` is the sta wlan index (19 idle, the AP peer's 1 for a join) and `connected` writes conn_state 0.
fn bss_req(bssid: [u8; 6], band: u8, phy: Option<bool>, sta: u16, connected: bool) -> [u8; 36] {
    let mut v = [0u8; 36];
    v[6] = 32;
    v[8] = 1;
    v[11] = band;
    v[12..16].copy_from_slice(&CONN_INFRA_STA.to_le_bytes());
    v[16] = if connected { 0 } else { 1 };
    v[18..24].copy_from_slice(&bssid);
    v[24..26].copy_from_slice(&WCID_BMC.to_le_bytes());
    v[30..32].copy_from_slice(&sta.to_le_bytes());
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
/// + 32, CT, queue 3; d1 wlan `wcid` (MT7925S9: 1 = the AP peer, 19 = the interface), own MAC 0, band index (TGID, when not 0),
/// 802.11 header format, header info 12, tid 0, fixed rate; d2 subtype | type; d3 remaining tx count 15, BA disable (+ BCM for a
/// group address); d6 DAS, DIS_MAT, one MSDU, the fixed-rate table slot; d4, d5, d7 zero. txp (`txw_pack`): msdu id | valid, then
/// pointer 0 = (`pa`, length | last).
fn mgmt_txwi(frame: &[u8], pa: u64, tok: u8, five: bool, band: u8, wcid: u16) -> [u8; 64] {
    let fc = frame[0] as u32;
    let mut d1 = wcid as u32 | (HDR_802_11 << 14) | (12 << 16) | (1 << 31);
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
    txw_pack(&d, pa, frame.len(), tok as u16) // MT7925S9 (Z46): the packing is the ONE `txw_pack` (S9's tail), the bytes unchanged
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
    let bs = if d.is_ok() { uni_cmd(h, UNI_BSS_INFO, &bss_req([0u8; 6], BAND_UNSET, None, WCID_BMC, false)) } else { Err("skipped") };
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
    let slot = t.blk + (t.wi % TX_SLOTS) as u64 * TX_SLOT; // slot `wi % 4` of the 4 KiB block `join_run` allocated (frame.len() <= MGMT_MAX)
    let w = mgmt_txwi(frame, slot + TX_FRAME_OFF, tok, t.five, t.band, t.wcid);
    t.wi = tx_put(t.q, t.wi, slot, frame, &w); // MT7925S9 (Z46): the copy and post are the ONE `tx_put` (S9's tail), writes unchanged and in order
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
    let (mut sta, mut wcid, mut linked): (Result<u8, &'static str>, u16, bool) = (Err("skipped"), WCID_BMC, false); // MT7925S9 (Z46)
    if pre.is_ok() {
        hand_rx(rx2, rxb2);
        w32(q2 + 8, RX_BUFS); // RX ring 2's 31 buffers handed (S7's convention)
        dev_bss_once(h);
        bss = uni_cmd(h, UNI_BSS_INFO, &bss_req(plan.bssid, BAND_UNSET, Some(five), WCID_PEER, false)); // MT7925S9: sta idx = the AP peer's
        sta = uni_cmd(h, UNI_STA_REC, &sta_req(plan.bssid, 0, STA_NONE, five)); // MT7925S9 (Z46, S8-STAREC): the AP's STA_REC before auth
        if sta == Ok(0) {
            wcid = WCID_PEER; // the frames ride the peer's wlan index as linux's do once the station exists; else the interface's 19
        }
        let mut r = Rx2 { q: q2, desc: rx2, bufs: rxb2, ri: 0 };
        let mut t = Tx { q: b + TX_RING_BASE + TXQ_DATA * RING_REGS, wi: 0, blk: txb, band: 0, five, wcid };
        rep.auth = exchange(h, &mut r, &mut t, 0, plan.channel, &mut *build, &mut *judge);
        if matches!(rep.auth, Step::Ans(Answer::Ok(_))) {
            rep.assoc = exchange(h, &mut r, &mut t, 1, plan.channel, &mut *build, &mut *judge);
        }
        posted = t.wi;
        if let Step::Ans(Answer::Ok(aid)) = rep.assoc {
            linked = link_up(h, plan, aid, &t, r, took); if linked { watch_arm(h); } // S9WATCH (zenbook-ledger Z56): the firmware's beacon monitor armed after the link (LINE-NEUTRAL). MT7925S9 (Z46): the connected BSS_INFO, STA_REC ASSOC, the S9 link
        }
    }
    let rel = if linked { "kept-s9" } else { give_back(lp, took) }; // MT7925S9: a linked run keeps driver-own for the data path
    if let Err(w) = pre {
        serial_println!("[mt7925] s8 -> NOT ATTEMPTED why={} rel={}", w, rel);
        return Err("pre");
    }
    let m = plan.bssid;
    serial_println!(
        "[mt7925] s8 join ssid=\"{}\" bssid={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} ch={} tx0={} bss={} sta={} wcid={} auth={} assoc={} held={} rel={} -> parked",
        plan.ssid, m[0], m[1], m[2], m[3], m[4], m[5], plan.channel, posted, ack_text(&bss), ack_text(&sta), wcid,
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
    let a = mgmt_txwi(&auth, 0x1080, 1, false, 0, WCID_BMC);
    let mut ok = a.len() == 64
        && u32_at(&a, 0) == 0x0600_003e && u32_at(&a, 4) == 0x800c_8013 && u32_at(&a, 8) == 0x0b
        && u32_at(&a, 12) == 0x1000_7800 && u32_at(&a, 16) == 0 && u32_at(&a, 20) == 0
        && u32_at(&a, 24) == 0x000b_001c && u32_at(&a, 28) == 0
        && u32_at(&a, 32) == 0x8001 && u32_at(&a, 40) == 0x1080 && u32_at(&a, 44) == 0x801e;
    let f = mgmt_txwi(&auth, 0x1080, 1, true, 0, WCID_BMC);
    ok &= u32_at(&f, 24) == 0x000f_001c && u32_at(&f, 4) == 0x800c_8013;
    let mut req = alloc::vec![0u8; 52];
    req[0] = 0x00;
    let c = mgmt_txwi(&req, 0x1480, 2, false, 0, WCID_BMC);
    ok &= u32_at(&c, 0) == 0x0600_0054 && u32_at(&c, 8) == 0 && u32_at(&c, 32) == 0x8002
        && u32_at(&c, 40) == 0x1480 && u32_at(&c, 44) == 0x8034;
    let g = mgmt_txwi(&auth, 0x1080, 1, false, 1, WCID_BMC);
    ok &= u32_at(&g, 4) == 0x800c_9013;
    let mac = [0x02, 0x55, 0x4e, 0x41, 0x4f, 0x53];
    let d = dev_req(mac);
    ok &= d[..10] == [0, 0xff, 0, 0, 0, 0, 12, 0, 1, 0] && d[10..] == mac;
    let bssid = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
    let i = bss_req(bssid, BAND_UNSET, None, WCID_BMC, false);
    ok &= i.len() == 36 && i[6] == 32 && i[8] == 1 && i[11] == 0xff && u32_at(&i, 12) == 0x0001_0001 && i[16] == 1
        && i[18..24] == bssid && u16_at(&i, 24) == 19 && u16_at(&i, 30) == 19 && i[29] == 0 && u16_at(&i, 26) == 0;
    let j = bss_req(bssid, BAND_UNSET, Some(false), WCID_BMC, false);
    ok &= u16_at(&j, 26) == 100 && j[29] == 6 && u16_at(&j, 32) == 1;
    let k = bss_req(bssid, BAND_UNSET, Some(true), WCID_BMC, false);
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
    ok && link_kat() // MT7925S9 (Z46): the S9 rows (data TXD, STA_REC, connected BSS_INFO, rx_span)
}

// ── MT7925S9 (zenbook-ledger Z46) — S9: the AP peer's STA_REC and the data path ─────────────────────────────────────────
// Linux's station flow after S8's frames: the AP is a station record before authentication (`mt7925_mac_link_sta_add`: BSS_INFO
// with the peer's wlan index, then STA_REC_UPDATE 0x03 state NONE; the peer's wlan index is 1, wcid 0 being the global entry), so
// the authentication and association frames ride wcid 1; after the association (`mt7925_mac_link_sta_assoc`) BSS_INFO connected
// with the RLM TLV (the operating channel once the ROC is aborted) and STA_REC state ASSOC with the AID. The STA_REC carries BASIC,
// PHY, RA, STATE and HDR_TRANS (to_ds, RX translation on); HT/VHT/HE are S8-HT's. The firmware translates headers both ways
// (mt792x advertises TX encap and RX decap offload): a data frame goes on TX ring 0 as the Ethernet frame behind an 802.3 TXD
// (`data_txwi`), and RX ring 2 hands it back header-translated at the offset S7's walk computes (`rx_span`). No 802.11 data frame
// is built here and `wifi/data.rs` is the Broadcom's. The link lives in `S9_LINK` (filled by `join_run`, ended by `rx_open`);
// `drivers/e1000.rs`'s accessor triple reaches it through `MtRadio` and `hw_addr` (cfg wifi), smolnet's `tx_ready` / `rx_ready`
// through `tx_room` / `rx_pending`. Lock order: smolnet's STACK, then S9_LINK; nothing here calls the stack or sleeps under the
// lock. Driver-own is kept while linked (the data path's MMIO). Fixed rate on data (S9-RATES owed); no TX-free decode: at most
// TX_INFLIGHT frames un-fetched and a slot rewritten only 32 posts later (S9-TXFREE owed). Hardware facts only; no code copied.

/// The AP peer's wlan index (linux: wcid 0 is the global entry, the first station record takes 1).
const WCID_PEER: u16 = 1;
/// STA_REC_UPDATE and its TLV tags (mt76_connac_mcu.h STA_REC_*).
const UNI_STA_REC: u16 = 0x03;
const STA_TAG_BASIC: u16 = 0;
const STA_TAG_RA: u16 = 1;
const STA_TAG_STATE: u16 = 7;
const STA_TAG_PHY: u16 = 0x15;
const STA_TAG_HDR_TRANS: u16 = 0x2b;
/// CONNECTION_INFRA_AP (STA_TYPE_AP | NETWORK_INFRA): the record of a station's AP; CONN_STATE_PORT_SECURE.
const CONN_INFRA_AP: u32 = 0x0001_0002;
const CONN_PORT_SECURE: u8 = 2;
/// MT76_STA_INFO_STATE_NONE / _ASSOC; BASIC's extra_info VER and NEW (NEW while not associated).
const STA_NONE: u8 = 0;
const STA_ASSOC: u8 = 2;
const EXTRA_VER: u16 = 1;
const EXTRA_NEW: u16 = 2;
/// The STA_REC body: 8-byte header + BASIC 20 + PHY 12 + RA 16 + STATE 16 + HDR_TRANS 8.
const STA_REQ_LEN: usize = 80;
/// BSS_INFO's RLM TLV tag, the streams (the 2x2 radio) and the connected body (36 + 16).
const BSS_TAG_RLM: u16 = 2;
const STREAMS: u8 = 2;
const BSS_CONN_LEN: usize = 52;
/// The data TXD: queue = wmm 0 * 4 + (3 - AC_BE 2); HDR_FORMAT 802.3; ETH_802_3 (an EtherType frame); FRAME_TYPE data.
const Q_IDX_BE: u32 = 1;
const HDR_802_3: u32 = 0;
const TXD1_ETH_802_3: u32 = 1 << 20;
const FRAME_TYPE_DATA: u32 = 2;
const ETHERTYPE_MIN: u16 = 0x0600;
/// RXD2 bit 25: the header translation failed (a 2-byte length inserted): dropped.
const RXD2_TRANS_ERR: u32 = 1 << 25;
/// The data TX block: 32 slots of 2 KiB (slot i = TX ring 0 descriptor i: TXD + txp at +0, the frame at +128).
const DATA_BLOCK: usize = 65536;
const DATA_SLOT: u64 = 2048;
/// An Ethernet header, the largest frame the link takes (smolnet's FRAME_CAP), the frames left un-fetched at most.
const ETH_HDR: usize = 14;
const ETH_MAX: usize = 1536;
const TX_INFLIGHT: u32 = 8;
/// The data frames' msdu ids: 0x100 + the slot (the management frames use the ROC tokens 1..127).
const DATA_TOK: u16 = 0x100;

/// The S9 link: own, the association, TX ring 0 (registers, the next CPU index continuing S8's, the data block), RX ring 2
/// (continuing the join's drain), the counts.
struct Link9 {
    took: bool,
    aid: u16,
    five: bool,
    band: u8,
    q0: u64,
    wi: u32,
    txb: u64,
    rx: Rx2,
    tx: u32,
    rx_n: u32,
    skip: u32,
    full: u32,
    /// S9WATCH (zenbook-ledger Z56): the station's MAC, the AP's BSSID, the station's judge, a refusal heard (noted by `link_rx`,
    /// ended by `watch` on the main loop, never under smolnet's STACK).
    me: [u8; 6],
    bssid: [u8; 6],
    judge: Judge,
    lost: Option<(&'static str, u16)>,
}

/// The link, Some from `link_up` until `rx_open` ends it.
static S9_LINK: crate::sync::Mutex<Option<Link9>> = crate::sync::Mutex::new(None);
/// The stack was built on the link (`hw_addr` answered).
static S9_BOUND: AtomicBool = AtomicBool::new(false);
/// The 64 KiB data TX block (below 4 GiB, `dma::ident` once, never freed: S3's rule).
static S9_TXB: AtomicU64 = AtomicU64::new(0);
/// A refusal said once per link loss.
static S9_RX_SAID: AtomicBool = AtomicBool::new(false);
static S9_TX_SAID: AtomicBool = AtomicBool::new(false);

/// STA_REC_UPDATE body (pure, 80 bytes) for the AP `peer`: header {bss 0, wlan lo 1, 5 TLVs, append, muar 0, wlan hi 0}; BASIC
/// {INFRA_AP, PORT_SECURE, qos 0, `aid`, the peer, VER (+ NEW unless ASSOC)}; PHY {basic rates, phy type: 1/2/5.5/11 and HR_DSSS|ERP
/// on 2.4 GHz, 6/12/24 and OFDM on 5 GHz}; RA {legacy mask: CCK 3:0 + OFDM 13:6 / OFDM}; STATE {`state`}; HDR_TRANS {to_ds 1, RX
/// translation on}.
fn sta_req(peer: [u8; 6], aid: u16, state: u8, five: bool) -> [u8; STA_REQ_LEN] {
    let mut v = [0u8; STA_REQ_LEN];
    v[1] = (WCID_PEER & 0xff) as u8;
    v[2..4].copy_from_slice(&5u16.to_le_bytes());
    v[4] = 1;
    v[6] = (WCID_PEER >> 8) as u8;
    let d = put_tlv(&mut v, 8, STA_TAG_BASIC, 20);
    v[d..d + 4].copy_from_slice(&CONN_INFRA_AP.to_le_bytes());
    v[d + 4] = CONN_PORT_SECURE;
    v[d + 6..d + 8].copy_from_slice(&aid.to_le_bytes());
    v[d + 8..d + 14].copy_from_slice(&peer);
    let extra = if state == STA_ASSOC { EXTRA_VER } else { EXTRA_VER | EXTRA_NEW };
    v[d + 14..d + 16].copy_from_slice(&extra.to_le_bytes());
    let d = put_tlv(&mut v, 28, STA_TAG_PHY, 12);
    v[d..d + 2].copy_from_slice(&(if five { 0x0015u16 } else { 0x000f }).to_le_bytes());
    v[d + 2] = if five { 0x08 } else { 0x03 };
    let d = put_tlv(&mut v, 40, STA_TAG_RA, 16);
    v[d..d + 2].copy_from_slice(&(if five { 0x3fc0u16 } else { 0x3fcf }).to_le_bytes());
    let d = put_tlv(&mut v, 56, STA_TAG_STATE, 16);
    v[d] = state;
    let d = put_tlv(&mut v, 72, STA_TAG_HDR_TRANS, 8);
    v[d + 1] = 1; // to_ds; from_ds 0, dis_rx_hdr_tran 0
    v
}

/// The connected BSS_INFO body (pure, 52 bytes): `bss_req` with the join's phy, the peer's wlan index and conn_state 0, then RLM
/// {control `ch`, center `ch`, center2 0, bw 20 (0), 2 TX / 2 RX streams, ht_op 0, sco 0, band 0 (2.4) / 1 (5)}.
fn bss_conn_req(bssid: [u8; 6], band: u8, ch: u8, five: bool) -> [u8; BSS_CONN_LEN] {
    let mut v = [0u8; BSS_CONN_LEN];
    v[..36].copy_from_slice(&bss_req(bssid, band, Some(five), WCID_PEER, true));
    let d = put_tlv(&mut v, 36, BSS_TAG_RLM, 16);
    v[d] = ch;
    v[d + 1] = ch;
    v[d + 4] = STREAMS;
    v[d + 5] = STREAMS;
    v[d + 8] = five as u8;
    v
}

/// The ONE TXD + hw txp packer (hoisted out of `mgmt_txwi`): the eight dwords little-endian, then msdu id | valid, then pointer 0
/// = (`pa`, `len` | last).
fn txw_pack(d: &[u32; 8], pa: u64, len: usize, tok: u16) -> [u8; 64] {
    let mut v = [0u8; 64];
    for (i, w) in d.iter().enumerate() {
        v[4 * i..4 * i + 4].copy_from_slice(&w.to_le_bytes());
    }
    v[32..34].copy_from_slice(&(tok | 0x8000).to_le_bytes());
    v[40..44].copy_from_slice(&(pa as u32).to_le_bytes());
    v[44..46].copy_from_slice(&(len as u16 | 0x8000).to_le_bytes());
    v
}

/// The 64-byte TXD + txp of one Ethernet frame (pure; `len` bytes at bus address `pa`, `ety` its EtherType): d0 length + 32, CT,
/// queue 1 (BE); d1 wlan 1, band index (TGID, when not 0), 802.3 header format, ETH_802_3 for an EtherType frame, tid 0, fixed
/// rate; d2 type data, subtype 0; d3 remaining tx count 15, BA disable; d6 DAS, DIS_MAT, one MSDU, the fixed-rate table slot.
fn data_txwi(len: usize, ety: u16, pa: u64, tok: u16, five: bool, band: u8) -> [u8; 64] {
    let mut d1 = WCID_PEER as u32 | (HDR_802_3 << 14) | (1 << 31);
    if ety >= ETHERTYPE_MIN {
        d1 |= TXD1_ETH_802_3;
    }
    if band != 0 {
        d1 |= (band as u32) << 12;
    }
    let d = [
        (len as u32 + 32) | (Q_IDX_BE << 25),
        d1,
        FRAME_TYPE_DATA << 4,
        (15u32 << 11) | (1 << 28),
        0,
        0,
        (1 << 2) | (1 << 3) | (1 << 4) | ((if five { RATE_TBL_5G } else { RATE_TBL_2G }) << 16),
        0,
    ];
    txw_pack(&d, pa, len, tok)
}

/// The ONE TX put (hoisted out of `tx_mgmt`): the frame into `slot` + 128, the TXD + txp at `slot`, `post` (CPU index last).
/// Returns the next CPU index. The slot is the caller's: `tx_mgmt` a 1 KiB slot (<= MGMT_MAX), `link_tx` a 2 KiB one (<= ETH_MAX).
fn tx_put(q: u64, wi: u32, slot: u64, frame: &[u8], w: &[u8; 64]) -> u32 {
    let pa = slot + TX_FRAME_OFF;
    for (i, v) in frame.iter().enumerate() {
        // SAFETY: inside the caller's slot of its DMA block (offset 128, frame.len() bounded by the caller as above).
        unsafe { core::ptr::write_volatile((pa + i as u64) as *mut u8, *v) };
    }
    for (i, v) in w.iter().enumerate() {
        // SAFETY: the slot's first 64 bytes, same block.
        unsafe { core::ptr::write_volatile((slot + i as u64) as *mut u8, *v) };
    }
    post(q, wi, slot, TXWI_BYTES)
}

/// The frame's byte range inside one RX ring 2 buffer and whether the firmware header-translated it (pure; the walk hoisted out of
/// S7's `rx_frame`): past the 8-dword descriptor, its groups and twice the header pad. None = not a normal frame, a length error, a
/// failed translation, or shorter than an Ethernet header.
fn rx_span(buf: &[u8]) -> Option<(core::ops::Range<usize>, bool)> {
    let (d0, d1, d2) = (le32(buf, 0)?, le32(buf, 4)?, le32(buf, 8)?);
    let trans = d2 & RXD2_HDR_TRANS != 0;
    if d0 >> 27 != PKT_NORMAL || d2 & RXD2_MAX_LEN_ERR != 0 || (trans && d2 & RXD2_TRANS_ERR != 0) {
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
    if off + ETH_HDR > buf.len() {
        return None;
    }
    Some((off..buf.len(), trans))
}

/// The ONE RX ring 2 take (hoisted out of S7's `drain`): descriptor `ri`'s DMA_DONE read; when set, its bytes (SD_LEN0, capped at
/// the buffer; a frame split over buffers keeps nothing) copied out, then the buffer re-handed (`rearm`). None = not done.
fn rx_take(r: &mut Rx2) -> Option<Vec<u8>> {
    let d = r.desc + r.ri as u64 * DESC_BYTES;
    // SAFETY: descriptor ri (< RING_DEPTH) of RX ring 2's block (`ring()`, armed by `wake`); a status read.
    let ctrl = unsafe { core::ptr::read_volatile((d + 4) as *const u32) };
    if ctrl & DMA_DONE == 0 {
        return None;
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
    Some(v)
}

/// After an association: the 64 KiB data block, the connected BSS_INFO and STA_REC ASSOC (each acked 0), then the link filled and
/// said. False = refused (said by name); the caller then gives own back. Only when the AP's STA_REC NONE was acked (`t.wcid`).
fn link_up(h: &mut Hw, plan: &JoinPlan, aid: u16, t: &Tx, r: Rx2, took: bool) -> bool {
    if t.wcid != WCID_PEER {
        serial_println!("[mt7925] s9 link aid={} -> REFUSED why=sta-none", aid);
        return false;
    }
    let mut txb = S9_TXB.load(Ordering::Relaxed);
    if txb == 0 {
        let v = crate::arch::memory::dma::ident(DATA_BLOCK, 4096);
        if v != 0 && v + DATA_BLOCK as u64 <= DMA_LIMIT {
            txb = v;
            S9_TXB.store(v, Ordering::Relaxed);
        }
    }
    if txb == 0 {
        serial_println!("[mt7925] s9 link aid={} -> REFUSED why=no-txmem", aid);
        return false;
    }
    let bs = uni_cmd(h, UNI_BSS_INFO, &bss_conn_req(plan.bssid, t.band, plan.channel, t.five));
    let st = if bs == Ok(0) { uni_cmd(h, UNI_STA_REC, &sta_req(plan.bssid, aid, STA_ASSOC, t.five)) } else { Err("skipped") };
    if bs != Ok(0) || st != Ok(0) {
        serial_println!(
            "[mt7925] s9 link aid={} bss={} sta={} -> REFUSED why={}",
            aid, ack_text(&bs), ack_text(&st), if bs != Ok(0) { "bss" } else { "sta" }
        );
        return false;
    }
    *S9_LINK.lock() = Some(Link9 { took, aid, five: t.five, band: t.band, q0: t.q, wi: t.wi, txb, rx: r, tx: 0, rx_n: 0, skip: 0, full: 0, me: mac().unwrap_or([0; 6]), bssid: plan.bssid, judge: plan.watch, lost: None }); // S9WATCH (Z56): the watch's four fields
    S9_RX_SAID.store(false, Ordering::Relaxed);
    S9_TX_SAID.store(false, Ordering::Relaxed);
    serial_println!(
        "[mt7925] s9 link up aid={} wcid={} tx=0 rx=0 skip=0 full=0 bss={} sta={} rel=kept-s9 -> linked",
        aid, WCID_PEER, ack_text(&bs), ack_text(&st)
    );
    true
}

/// S9WATCH (zenbook-ledger Z56): THE ONE link end (hoisted out of `rx_open`, which calls it when a scan or join re-wakes the
/// rings; `watch` calls it on a deauth, a disassoc or the firmware's beacon loss): the link taken out of `S9_LINK`, one line, the
/// watch disarmed, smolnet's flag raised when the stack was bound (`down_take`; the driver never calls the stack). Returns whether
/// the link's run took own (the caller hands it back: `rx_open` with its run, `watch` at once). False with no link. Call it with
/// no lock held (it takes `S9_LINK`). S9REJOIN (zenbook-ledger Z59, S9-UNLINK): `h` is the MCU channel owned by value by the
/// caller (`rx_open`'s run, or `watch` having taken it out of `MCU_LINK`; never under a lock); with it the firmware's records of
/// the link are unwound in linux's disconnect order (`unlink`), their acks on the line (`fw=busy` with no channel). Not `pub`:
/// `Hw` is private and every caller is in this file. S10-KEYZERO (owed): the key teardown folds AFTER the S9-UNLINK hunk, same `h`.
fn link_down(why: &str, h: Option<&mut Hw>) -> bool {
    let Some(l) = S9_LINK.lock().take() else { return false };
    S9_WATCH.store(false, Ordering::Relaxed);
    if bound() {
        S9_DOWN.store(true, Ordering::Relaxed);
    }
    let fw = match h { Some(h) => unlink(h, &l), None => String::from("busy") }; // S9-UNLINK (zenbook-ledger Z59): STA_REC remove, BSS_INFO not connected, BCNFT off
    let rekeys = rekey_clear(); // S10REKEY (zenbook-ledger Z63): the group-rekey record (the KCK/KEK) dropped and zeroized, the EAPOL mailbox emptied; its count on the line
    serial_println!(
        "[mt7925] s9 link down why={} aid={} tx={} rx={} skip={} full={} fw={} rekeys={}",
        why, l.aid, l.tx, l.rx_n, l.skip, l.full, fw, rekeys
    );
    l.took
}

/// The link's state now (`say_link`, for the station's verb after its DHCP wait).
pub fn say_link() {
    match S9_LINK.lock().as_ref() {
        Some(l) => serial_println!(
            "[mt7925] s9 link up aid={} wcid={} tx={} rx={} skip={} full={}",
            l.aid, WCID_PEER, l.tx, l.rx_n, l.skip, l.full
        ),
        None => serial_println!("[mt7925] s9 link -> down"),
    }
}

/// A refusal with no link, said once per loss.
fn not_linked(said: &AtomicBool, what: &str) {
    if !said.swap(true, Ordering::Relaxed) {
        serial_println!("[mt7925] s9 {} -> REFUSED why=not-linked", what);
    }
}

/// The next header-translated frame from RX ring 2 into `out` (at most 32 descriptors per call; untranslated frames, other
/// packets, failed translations and frames longer than `out` are dropped and counted `skip`).
pub fn link_rx(out: &mut [u8]) -> Option<usize> {
    let mut g = S9_LINK.lock();
    let Some(l) = g.as_mut() else {
        not_linked(&S9_RX_SAID, "rx");
        return None;
    };
    for _ in 0..RING_DEPTH {
        let v = rx_take(&mut l.rx)?;
        match rx_span(&v) {
            Some((span, true)) if span.len() <= out.len() && !s10_eapol(&v[span.clone()]) => {
                let n = span.len();
                out[..n].copy_from_slice(&v[span]);
                l.rx_n += 1;
                return Some(n);
            }
            Some((s, false)) => { l.skip += 1; if l.lost.is_none() { l.lost = (l.judge)(&v[s], l.me, l.bssid); } } // S9WATCH (zenbook-ledger Z56): an untranslated frame (management) judged by the station's refusal: noted, ended by `watch`
            _ => l.skip += 1,
        }
    }
    None
}

/// Room for one more frame: fewer than TX_INFLIGHT posted frames not yet fetched (TX ring 0's DMA index).
fn room(l: &Link9) -> bool {
    let didx = r32(l.q0 + 12) & 0xfff;
    (l.wi + RING_DEPTH - didx % RING_DEPTH) % RING_DEPTH < TX_INFLIGHT
}

/// One Ethernet frame out: into slot `wi` of the data block behind `data_txwi`, `tx_put`. No room, shorter than an Ethernet
/// header or longer than ETH_MAX: dropped and counted `full`.
pub fn link_tx(frame: &[u8]) {
    let mut g = S9_LINK.lock();
    let Some(l) = g.as_mut() else {
        not_linked(&S9_TX_SAID, "tx");
        return;
    };
    if frame.len() < ETH_HDR || frame.len() > ETH_MAX || !room(l) {
        l.full += 1;
        return;
    }
    let slot = l.txb + l.wi as u64 * DATA_SLOT;
    let ety = u16::from_be_bytes([frame[12], frame[13]]);
    let w = data_txwi(frame.len(), ety, slot + TX_FRAME_OFF, DATA_TOK + l.wi as u16, l.five, l.band);
    l.wi = tx_put(l.q0, l.wi, slot, frame, &w);
    l.tx += 1;
}

/// smolnet's `tx_ready` for the link: room on TX ring 0 now; false with no link.
pub fn tx_room() -> bool {
    S9_LINK.lock().as_ref().map_or(false, room)
}

/// smolnet's poll gate: RX ring 2's next descriptor is done.
pub fn rx_pending() -> bool {
    S9_LINK.lock().as_ref().map_or(false, |l| {
        let d = l.rx.desc + l.rx.ri as u64 * DESC_BYTES;
        // SAFETY: descriptor ri (< RING_DEPTH) of RX ring 2's block; a status read.
        (unsafe { core::ptr::read_volatile((d + 4) as *const u32) } & DMA_DONE) != 0
    })
}

/// The S9 link is up (the station's verb asks before its DHCP; no side effect, unlike `hw_addr`).
pub fn linked() -> bool {
    S9_LINK.lock().is_some()
}

/// The stack was built on the link.
pub fn bound() -> bool {
    S9_BOUND.load(Ordering::Relaxed)
}

/// `e1000::hw_addr`'s last fallback: (S5's MAC, 0.0.0.0 before a lease, link up) once linked; link down after `rx_open` ended a
/// link the stack was bound to; None otherwise.
pub fn hw_addr() -> Option<([u8; 6], [u8; 4], bool)> {
    let up = linked();
    if !up && !bound() {
        return None;
    }
    let m = mac()?;
    S9_BOUND.store(true, Ordering::Relaxed);
    Some((m, [0; 4], up))
}

/// `tests wifijoin`'s S9 rows (hand-computed from the field tables, no capture): the data TXD + txp of a 60-byte Ethernet II frame
/// on 2.4 GHz and 5 GHz (band 1) and of an 802.2 frame, STA_REC NONE on 2.4 GHz and ASSOC (aid 5) on 5 GHz, the join's and the
/// connected BSS_INFO with RLM, and `rx_span` over a translated, an untranslated and a translation-failed canned descriptor.
fn link_kat() -> bool {
    fn w(v: &[u8], o: usize) -> u32 {
        u32::from_le_bytes([v[o], v[o + 1], v[o + 2], v[o + 3]])
    }
    fn h(v: &[u8], o: usize) -> u16 {
        u16::from_le_bytes([v[o], v[o + 1]])
    }
    let a = data_txwi(60, 0x0800, 0x2080, 0x102, false, 0);
    let mut ok = w(&a, 0) == 0x0200_005c && w(&a, 4) == 0x8010_0001 && w(&a, 8) == 0x20 && w(&a, 12) == 0x1000_7800
        && w(&a, 16) == 0 && w(&a, 20) == 0 && w(&a, 24) == 0x000b_001c && w(&a, 28) == 0
        && w(&a, 32) == 0x8102 && w(&a, 40) == 0x2080 && w(&a, 44) == 0x803c;
    let f = data_txwi(60, 0x0800, 0x2080, 0x102, true, 1);
    ok &= w(&f, 4) == 0x8010_1001 && w(&f, 24) == 0x000f_001c;
    ok &= w(&data_txwi(60, 0x0040, 0x2080, 0x102, false, 0), 4) == 0x8000_0001;
    let peer = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
    let s = sta_req(peer, 0, STA_NONE, false);
    ok &= s[..8] == [0, 1, 5, 0, 1, 0, 0, 0]
        && h(&s, 8) == 0 && h(&s, 10) == 20 && w(&s, 12) == 0x0001_0002 && s[16] == 2 && s[17] == 0 && h(&s, 18) == 0
        && s[20..26] == peer && h(&s, 26) == 3
        && h(&s, 28) == 0x15 && h(&s, 30) == 12 && h(&s, 32) == 0x000f && s[34] == 3
        && h(&s, 40) == 1 && h(&s, 42) == 16 && h(&s, 44) == 0x3fcf
        && h(&s, 56) == 7 && h(&s, 58) == 16 && s[60] == 0
        && h(&s, 72) == 0x2b && h(&s, 74) == 8 && s[76] == 0 && s[77] == 1 && s[78] == 0;
    let t = sta_req(peer, 5, STA_ASSOC, true);
    ok &= h(&t, 18) == 5 && h(&t, 26) == 1 && t[60] == 2 && h(&t, 32) == 0x0015 && t[34] == 8 && h(&t, 44) == 0x3fc0;
    let j = bss_req(peer, BAND_UNSET, Some(false), WCID_PEER, false);
    ok &= j[16] == 1 && h(&j, 30) == 1 && h(&j, 24) == 19;
    let c = bss_conn_req(peer, 0, 6, false);
    ok &= c[16] == 0 && h(&c, 30) == 1 && h(&c, 24) == 19 && c[29] == PHY_MODE_BG && h(&c, 36) == 2 && h(&c, 38) == 16
        && c[40] == 6 && c[41] == 6 && c[42] == 0 && c[43] == 0 && c[44] == 2 && c[45] == 2 && c[46] == 0 && c[47] == 0 && c[48] == 0
        && bss_conn_req(peer, 0, 36, true)[48] == 1;
    let mut x = alloc::vec![0u8; 90];
    x[0..4].copy_from_slice(&((PKT_NORMAL << 27) | 90).to_le_bytes());
    x[4..8].copy_from_slice(&(RXD1_G4 | RXD1_G1).to_le_bytes());
    x[8..12].copy_from_slice(&((1u32 << 13) | RXD2_HDR_TRANS).to_le_bytes());
    ok &= rx_span(&x) == Some((66..90, true)) && rx_frame(&x).is_none();
    x[8..12].copy_from_slice(&((1u32 << 13) | RXD2_HDR_TRANS | RXD2_TRANS_ERR).to_le_bytes());
    ok &= rx_span(&x).is_none();
    x[8..12].copy_from_slice(&(1u32 << 13).to_le_bytes());
    ok &= rx_span(&x) == Some((66..90, false)) && rx_frame(&x) == Some(66..90);
    ok && watch_rows() && unlink_rows() // S9WATCH (zenbook-ledger Z56): the BCNFT body and the beacon-loss event decode; S9REJOIN (Z59): the three unlink bodies
}

// ── S9WATCH (zenbook-ledger Z56) — the S9 link's watch: deauth / disassoc from the AP, the firmware's beacon loss ──────────
// The S9 link (`S9_LINK`) ended only by a later scan or join (`rx_open`); a deauthentication or disassociation from the AP, or
// the AP gone silent, left it up while nothing flowed. Linux (mt7925, facts only): the station's beacon monitor is the
// firmware's — at the association `mt7925_mcu_set_beacon_filter` sends ONE BSS_INFO_UPDATE with the BCNFT TLV (tag 22: the beacon
// interval, the DTIM period) and the firmware reports the loss as the unsolicited uni event BSS_BEACON_LOSS (0x0c: hdr {bss
// index, 3 pad}, TLV {tag, len, reason, nr_btolink, 2 pad}); a deauthentication / disassociation reaches the host as an ordinary
// received management frame (RX ring 2, not header-translated). Here: `watch_arm` sends the BCNFT once after `link_up` (from
// `join_run`, inside `tests wifijoin`: nothing at boot); `link_rx`'s untranslated arm hands each management frame to the
// station's judge (`JoinPlan.watch` = `wifi/assoc.rs::refusal`, kept in the link: the driver links no wifi_core) and only NOTES a
// refusal (under smolnet's STACK -> S9_LINK); `watch` (from `service()`'s parked arm, the main loop, no lock held) ends the link
// on a noted refusal or on the beacon-loss event read through the ONE event consumer `event_take` (ring 0 under
// `MCU_LINK.try_lock`, never nested with S9_LINK; other events consumed and dropped), through the ONE link end `link_down`, then
// hands driver-own back (`give_back`). smolnet is told through its existing `dhcp_reset` on its own side (`down_take`, the one
// line in `service_poll`). No beacon timer of our own. Owed: S9-REJOIN, S9-ROAM, S9-UNLINK (the firmware's records unwound on
// link_down), S9-BCNDROP (linux's DROP_OTHER_BEACON rx filter), S9-BCNINT (the beacon's own interval / DTIM), S10-KEYZERO.
// Hardware facts only; no code copied.

/// The station's deauth / disassoc judge: (frame, the station's MAC, the BSSID) -> (`deauth` / `disassoc`, the reason code).
pub type Judge = fn(&[u8], [u8; 6], [u8; 6]) -> Option<(&'static str, u16)>;
/// MCU_UNI_EVENT_BSS_BEACON_LOSS (mt76_connac_mcu.h).
const EVT_BCN_LOSS: u8 = 0x0c;
/// The beacon-loss event's TLV: after the 44-byte rxd and the 4-byte header {bss index, 3 pad}; reason at +4.
const BCN_LOSS_TLV: usize = RXD_BYTES + 4;
/// UNI_BSS_INFO_BCNFT (mt76_connac_mcu.h); its TLV is 12 bytes {interval le16, dtim, bmc delivered ac, bmc triggered ac, 3 pad}.
const BSS_TAG_BCNFT: u16 = 22;
/// The beacon interval (TU) and DTIM period the BCNFT names: unread from the beacon (S9-BCNINT, with S8-BCNINT; `bss_req`'s 100).
const BCN_TU: u16 = 100;
const BCN_DTIM: u8 = 1;

/// The watch is armed (`watch_arm`) until `link_down`: `watch` returns on this one load with no link.
static S9_WATCH: AtomicBool = AtomicBool::new(false);
/// `link_down` ended a link the stack was bound to; taken by smolnet's `service_poll` (`down_take`).
static S9_DOWN: AtomicBool = AtomicBool::new(false);

/// BSS_INFO_UPDATE body with the BCNFT TLV (pure, 16 bytes): header {bss 0, 3 pad}, TLV {tag 22, len 12, `tu`, `dtim`, the two
/// ac bytes 0, 3 pad}.
fn bcnft_req(tu: u16, dtim: u8) -> [u8; 16] {
    let mut v = [0u8; 16];
    let d = put_tlv(&mut v, 4, BSS_TAG_BCNFT, 12);
    v[d..d + 2].copy_from_slice(&tu.to_le_bytes());
    v[d + 2] = dtim;
    v
}

/// The beacon-loss event's reason (pure; `ev` = the event's bytes, rxd included): None when shorter than the TLV or for
/// another BSS index than the station's 0 (linux checks only the bss index).
fn bcn_loss(ev: &[u8]) -> Option<u8> {
    if ev.len() < BCN_LOSS_TLV + 8 || ev[RXD_BYTES] != 0 {
        return None;
    }
    Some(ev[BCN_LOSS_TLV + 4])
}

/// After `link_up`, inside `tests wifijoin`: ONE BSS_INFO BCNFT (the firmware's beacon monitor), the watch armed either way (a
/// refused BCNFT leaves the deauth / disassoc arm: `deauth-only`), one line.
fn watch_arm(h: &mut Hw) {
    let a = uni_cmd(h, UNI_BSS_INFO, &bcnft_req(BCN_TU, BCN_DTIM));
    S9_WATCH.store(true, Ordering::Relaxed);
    serial_println!(
        "[mt7925] s9 watch armed bcnft={} tu={} dtim={} -> {}",
        ack_text(&a), BCN_TU, BCN_DTIM, if a == Ok(0) { "armed" } else { "deauth-only" }
    );
}

/// From `service()`'s parked arm (the main loop, no lock held): a refusal `link_rx` noted, else up to RING_DEPTH ring-0 events
/// for the unsolicited beacon loss (`MCU_LINK.try_lock`: None while a scan or join holds the channel, whose `rx_open` ends the
/// link itself); on a loss the ONE link end `link_down`, driver-own handed back, one line.
pub fn watch() {
    if !S9_WATCH.load(Ordering::Relaxed) {
        return;
    }
    let Some(noted) = S9_LINK.lock().as_ref().map(|l| l.lost) else { return };
    let lost = noted.or_else(|| {
        let mut g = MCU_LINK.try_lock()?;
        let h = g.as_mut()?;
        let want = |eid: u8, _seq: u8, opt: u8| eid == EVT_BCN_LOSS && opt & OPT_UNSOLICITED != 0;
        for _ in 0..RING_DEPTH {
            if event_take(h, &want)?.is_some() {
                if let Some(r) = bcn_loss(&h.last) {
                    return Some(("beacon-loss", r as u16));
                }
            }
        }
        None
    });
    let Some((what, code)) = lost else { return };
    let why = if what == "beacon-loss" { String::from(what) } else { alloc::format!("{}(reason={})", what, code) };
    let stack = if bound() { "reset" } else { "unbound" };
    let rel = link_end(&why); // WIFILEAVE (zenbook-ledger Z68): the ONE link end (take the channel, `link_down`, put it back, hand driver-own back), hoisted verbatim to the file tail so `leave` runs the same path
    let rejoin = if rejoin_lose(what) { "armed" } else { "off" }; // S9REJOIN (Z59): the kept plan's backoff armed (REJOIN, a leaf lock); the attempts run on the mt7925-rejoin task
    serial_println!("[mt7925] s9 watch -> lost why={} code={} rel={} stack={} rejoin={}", what, code, rel, stack, rejoin);
}

/// smolnet's `service_poll`: a link the stack was bound to ended since the last call (taken once).
pub fn down_take() -> bool {
    S9_DOWN.swap(false, Ordering::Relaxed)
}

/// `tests wifijoin`'s S9-WATCH rows (hand-assembled, no capture): canned RX ring 2 buffers (`link_kat`'s descriptor: groups 4 and
/// 1, pad 1, frame at 66) through `rx_span` and `judge` as `link_rx`'s untranslated arm does — a deauthentication reason 3 and a
/// disassociation reason 8 from the BSSID to the station, a deauthentication to another addr1 (not ours), the same frame marked
/// translated (not judged) — then `watch_rows`.
pub fn watch_kat(judge: Judge) -> bool {
    let me = [0x02, 0x55, 0x4e, 0x41, 0x4f, 0x53];
    let ap = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
    let buf = |fc0: u8, a1: [u8; 6], reason: u16, trans: bool| -> Vec<u8> {
        let mut x = alloc::vec![0u8; 66];
        x.extend_from_slice(&[fc0, 0, 0, 0]);
        x.extend_from_slice(&a1);
        x.extend_from_slice(&ap);
        x.extend_from_slice(&ap);
        x.extend_from_slice(&[0, 0]);
        x.extend_from_slice(&reason.to_le_bytes());
        let n = x.len() as u32;
        x[0..4].copy_from_slice(&((PKT_NORMAL << 27) | n).to_le_bytes());
        x[4..8].copy_from_slice(&(RXD1_G4 | RXD1_G1).to_le_bytes());
        x[8..12].copy_from_slice(&((1u32 << 13) | if trans { RXD2_HDR_TRANS } else { 0 }).to_le_bytes());
        x
    };
    let heard = |x: &[u8]| match rx_span(x) {
        Some((s, false)) => judge(&x[s], me, ap),
        _ => None,
    };
    let other = [0x02, 0, 0, 0, 0, 1];
    heard(&buf(0xc0, me, 3, false)[..]) == Some(("deauth", 3))
        && heard(&buf(0xa0, me, 8, false)[..]) == Some(("disassoc", 8))
        && heard(&buf(0xc0, other, 3, false)[..]).is_none()
        && heard(&buf(0xc0, me, 3, true)[..]).is_none()
        && watch_rows()
}

/// The pure rows (no judge; also folded into `link_kat`): the BCNFT body, a canned beacon-loss event (reason 1 on bss 0), the
/// same on bss 1, and one byte short.
fn watch_rows() -> bool {
    let mut ok = bcnft_req(BCN_TU, BCN_DTIM) == [0, 0, 0, 0, 22, 0, 12, 0, 100, 0, 1, 0, 0, 0, 0, 0];
    let mut ev = alloc::vec![0u8; BCN_LOSS_TLV + 8];
    ev[36] = EVT_BCN_LOSS;
    ev[38] = 1 << 2;
    ev[BCN_LOSS_TLV + 2] = 8;
    ev[BCN_LOSS_TLV + 4] = 1;
    ok &= bcn_loss(&ev) == Some(1) && bcn_loss(&ev[..BCN_LOSS_TLV + 7]).is_none();
    ev[RXD_BYTES] = 1;
    ok &= bcn_loss(&ev).is_none();
    ok
}

// ── MT7925S10 (zenbook-ledger Z55) — S10: the WPA2-PSK handshake's EAPOL path and the radio's keys ─────────────────────────────
// The handshake itself is `wifi::assoc` (pure) driven by `wifi::verb::s10_wpa2`; this block is its two doors into the radio.
// EAPOL rides the ONE data path: an inbound 802.1X frame (EtherType 0x888e) is lifted out of `link_rx`'s arm into the one-slot
// mailbox `S10_EAPOL` (so smolnet never sees it; counted here, not on the link's `rx`), and `eapol_rx` hands it to the station; an
// outbound one is `link_tx`'s Ethernet frame like any other, its fetch awaited (`eapol_tx`) so the next command cannot overtake it.
// Lock order: smolnet's STACK, then S9_LINK, then S10_EAPOL; nothing sleeps under any of them. The radio does CCMP: the keys go to
// it as linux's `mt7925_set_link_key` does (measured, linux v6.11): the first key makes the BSS secure (BSS_INFO re-sent with
// the SEC TLV: mode WPA2-PSK, status 6, cipher CCMP), then STA_REC_UPDATE with ONE STA_REC_KEY_V3 TLV per key — the pairwise key on
// the AP peer's wlan index (tx_key 1, key_type 1, muar 0), the group key on the interface's own (wcid 19, tx_key 0, key_type 0,
// muar 0x0e), peer address = the AP, cipher AES-CCMP (4), key id from the KDE, key length 16, mgmt_prot 1, key RSC 0. No port-secure
// command: the peer's BASIC TLV carries CONN_STATE_PORT_SECURE since S9 (linux sends none after the handshake). Key bytes live in
// stack arrays wiped after the command and in the command page, wiped after the last one. Hardware facts only; no code copied.

/// STA_REC's key TLV (STA_REC_KEY_V3), BSS_INFO's security TLV, the CONNAC3 CCMP cipher id, the BSS security mode (MODE_WPA2_PSK) and
/// status (CCMP), the muar index of the interface's own wlan index, the EAPOL EtherType.
const STA_TAG_KEY_V3: u16 = 0x27;
const BSS_TAG_SEC: u16 = 16;
const CIPHER_CCMP: u8 = 4;
const MODE_WPA2_PSK: u8 = 7;
const SEC_STATUS_CCMP: u8 = 6;
const MUAR_BMC: u8 = 0x0e;
const ETHERTYPE_EAPOL: u16 = 0x888e;
/// The secured BSS_INFO body (BASIC 36 + SEC 8 + RLM 16) and the key STA_REC body (8-byte header + the 68-byte key TLV).
const BSS_SEC_LEN: usize = 60;
const KEY_REQ_LEN: usize = 76;

/// The newest inbound EAPOL frame (the whole Ethernet frame), written by `link_rx`'s fold, taken by `eapol_rx`.
static S10_EAPOL: crate::sync::Mutex<Option<Vec<u8>>> = crate::sync::Mutex::new(None);
/// EAPOL frames lifted out of `link_rx`, and non-EAPOL frames `eapol_rx` dropped (counts since boot; the station reads deltas).
static S10_N_EAPOL: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static S10_N_OTHER: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// An Ethernet frame of EtherType 802.1X with at least a 4-octet EAPOL header after the Ethernet header (pure).
fn s10_is_eapol(f: &[u8]) -> bool {
    f.len() >= ETH_HDR + 4 && u16::from_be_bytes([f[12], f[13]]) == ETHERTYPE_EAPOL
}

/// `link_rx`'s fold (called under S9_LINK): an EAPOL frame is kept in the mailbox (the newest replaces an untaken one) and counted;
/// true = lifted, so the arm does not hand it to the caller. Any other frame is false and untouched.
fn s10_eapol(f: &[u8]) -> bool {
    if !s10_is_eapol(f) {
        return false;
    }
    let old = S10_EAPOL.lock().replace(f.to_vec());
    drop(old);
    S10_N_EAPOL.fetch_add(1, Ordering::Relaxed);
    true
}

/// The next EAPOL frame from the AP (the whole Ethernet frame), or None: one `link_rx` drive into a local buffer (it lifts EAPOL
/// into the mailbox itself; a frame it returns is not EAPOL and is dropped, counted), then the mailbox taken. No sleep here: the
/// station polls.
pub fn eapol_rx() -> Option<Vec<u8>> {
    let mut buf = alloc::vec![0u8; ETH_MAX];
    if link_rx(&mut buf).is_some() {
        S10_N_OTHER.fetch_add(1, Ordering::Relaxed);
    }
    S10_EAPOL.lock().take()
}

/// (EAPOL frames lifted, other frames dropped by `eapol_rx`), since boot.
pub fn eapol_counts() -> (u32, u32) {
    (S10_N_EAPOL.load(Ordering::Relaxed), S10_N_OTHER.load(Ordering::Relaxed))
}

/// One Ethernet frame (802.1X) out through `link_tx`, then its fetch awaited: (ring, next index, posted count) are read under the
/// lock, the lock released, `tx_done` polled. False = no link, not posted (no room, too long) or the fetch not seen.
pub fn eapol_tx(frame: &[u8]) -> bool {
    let before = S9_LINK.lock().as_ref().map(|l| l.tx);
    let Some(before) = before else { return false };
    link_tx(frame);
    let now = S9_LINK.lock().as_ref().map(|l| (l.q0, l.wi, l.tx));
    match now {
        Some((q0, wi, tx)) if tx != before => tx_done(q0, wi),
        _ => false,
    }
}

/// The secured BSS_INFO body (pure, 60 bytes): `bss_conn_req`'s BASIC (36), the SEC TLV {tag 16, len 8, mode WPA2-PSK, status 6,
/// cipher CCMP, 1 pad} and its RLM (16) copied from it.
fn bss_sec_req(bssid: [u8; 6], band: u8, ch: u8, five: bool) -> [u8; BSS_SEC_LEN] {
    let c = bss_conn_req(bssid, band, ch, five);
    let mut v = [0u8; BSS_SEC_LEN];
    v[..36].copy_from_slice(&c[..36]);
    let d = put_tlv(&mut v, 36, BSS_TAG_SEC, 8);
    v[d] = MODE_WPA2_PSK;
    v[d + 1] = SEC_STATUS_CCMP;
    v[d + 2] = CIPHER_CCMP;
    v[44..60].copy_from_slice(&c[36..52]);
    v
}

/// The key STA_REC_UPDATE body (pure, 76 bytes) for wlan index `wcid`: header {bss 0, wlan lo, 1 TLV, append, muar (0 for the
/// pairwise peer, 0x0e for the interface's own index), wlan hi}; TLV {tag 0x27, len 68, add 1, tx_key and key_type 1 for the
/// pairwise key (0 for the group key), is_authenticator 0, peer `peer`, bss 0, cipher CCMP, `key_id`, key length 16, wlan lo,
/// mgmt_prot 1, key (16 of 32), key RSC 0}.
fn key_req(peer: [u8; 6], wcid: u16, pairwise: bool, key_id: u8, key: &[u8; 16]) -> [u8; KEY_REQ_LEN] {
    let mut v = [0u8; KEY_REQ_LEN];
    v[1] = (wcid & 0xff) as u8;
    v[2..4].copy_from_slice(&1u16.to_le_bytes());
    v[4] = 1;
    v[5] = if pairwise { 0 } else { MUAR_BMC };
    v[6] = (wcid >> 8) as u8;
    let d = put_tlv(&mut v, 8, STA_TAG_KEY_V3, 68);
    v[d] = 1;
    v[d + 1] = pairwise as u8;
    v[d + 2] = pairwise as u8;
    v[d + 4..d + 10].copy_from_slice(&peer);
    v[d + 11] = CIPHER_CCMP;
    v[d + 12] = key_id;
    v[d + 13] = 16;
    v[d + 14] = (wcid & 0xff) as u8;
    v[d + 15] = 1;
    v[d + 16..d + 32].copy_from_slice(key);
    v
}

/// Hand the radio the keys of a finished handshake (after message 4 went out): BSS_INFO with the SEC TLV, the pairwise key on the AP
/// peer (wlan 1), the group key on the interface (wlan 19), each command acked 0 (a refusal stops the rest and is named `bss`, `ptk`
/// or `gtk`). `ch` is the BSS's channel (the RLM TLV); the band and the link come from the S9 link. `MCU_LINK` is taken and put back
/// on every path; the key arrays and the command page are wiped. One `[mt7925] s10 keys` line, no key byte in it. S10REKEY
/// (zenbook-ledger Z63): `tk` None = the GROUP ARM (a rekey): no BSS_INFO, no pairwise key, the one group KEY_V3 on wlan 19 (linux
/// sends the BSS SEC TLV only with the first key), same channel take/put and wipe, no `s10 keys` line (`rekey_tick`'s line speaks).
pub fn set_keys(bssid: [u8; 6], ch: u8, tk: Option<&[u8; 16]>, gtk: &[u8; 16], gtk_id: u8) -> Result<(), String> {
    use crypto_core::ct::Zeroize;
    let lk = S9_LINK.lock().as_ref().map(|l| (l.five, l.band));
    let Some((five, band)) = lk else { return Err(String::from("no-link")) };
    let link = MCU_LINK.lock().take();
    let Some(mut h) = link else { return Err(String::from("no-mcu")) };
    let rc = |r: &Result<u8, &'static str>| match r {
        Ok(a) => alloc::format!("{:#04x}", a),
        Err(e) => String::from(*e),
    };
    let full = tk.is_some();
    let b = if full { uni_cmd(&mut h, UNI_BSS_INFO, &bss_sec_req(bssid, band, ch, five)) } else { Ok(0) };
    let (mut p, mut g): (Result<u8, &'static str>, Result<u8, &'static str>) = (Err("skipped"), Err("skipped"));
    if b == Ok(0) {
        p = match tk {
            Some(tk) => {
                let mut k = key_req(bssid, WCID_PEER, true, 0, tk);
                let r = uni_cmd(&mut h, UNI_STA_REC, &k);
                k.zeroize();
                r
            }
            None => Ok(0),
        };
        if p == Ok(0) {
            let mut k = key_req(bssid, WCID_BMC, false, gtk_id, gtk);
            g = uni_cmd(&mut h, UNI_STA_REC, &k);
            k.zeroize();
        }
    }
    for i in 0..(UNI_TXD_BYTES + KEY_REQ_LEN) as u64 {
        // SAFETY: the 4 KiB command page, owned by this run (MCU_LINK taken), no command in flight: the last command's key bytes are wiped.
        unsafe { core::ptr::write_volatile((h.page + i) as *mut u8, 0) };
    }
    core::sync::atomic::fence(Ordering::SeqCst);
    *MCU_LINK.lock() = Some(h);
    if b == Ok(0) && p == Ok(0) && g == Ok(0) {
        if !full {
            return Ok(());
        }
        serial_println!(
            "[mt7925] s10 keys bss={} ptk={} gtk={} wcid={}/{} gtk_id={} -> installed",
            rc(&b), rc(&p), rc(&g), WCID_PEER, WCID_BMC, gtk_id
        );
        return Ok(());
    }
    let (which, r) = if b != Ok(0) { ("bss", &b) } else if p != Ok(0) { ("ptk", &p) } else { ("gtk", &g) };
    if full {
        serial_println!("[mt7925] s10 keys bss={} ptk={} gtk={} -> REFUSED why={}", rc(&b), rc(&p), rc(&g), which);
    }
    Err(alloc::format!("{}:{}", which, rc(r)))
}

/// `tests wpa2`'s `keys` leg (hand-computed from the field tables, no capture): the 60-byte secured BSS_INFO on 2.4 GHz ch 6 (and
/// the RLM band and channel on 5 GHz ch 36), the pairwise key body (wlan 1, key id 0) and the group key body (wlan 19, muar 0x0e,
/// key id 2) as byte literals, and the EAPOL judge over a canned EAPOL frame, a canned IPv4 frame and truncations.
pub fn s10_kat() -> bool {
    let peer = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
    let key: [u8; 16] = core::array::from_fn(|i| i as u8);
    let want_bss: [u8; BSS_SEC_LEN] = [
        0, 0, 0, 0, 0, 0, 32, 0, 1, 0, 0, 0, 1, 0, 1, 0, 0, 0, 0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 19, 0, 100, 0, 0, 6, 1, 0, 1, 0, 0, 0,
        16, 0, 8, 0, 7, 6, 4, 0,
        2, 0, 16, 0, 6, 6, 0, 0, 2, 2, 0, 0, 0, 0, 0, 0,
    ];
    let mut ok = bss_sec_req(peer, 0, 6, false) == want_bss;
    let five = bss_sec_req(peer, 1, 36, true);
    ok &= five[11] == 1 && five[29] == PHY_MODE_A && five[36..44] == want_bss[36..44] && five[44..48] == [2, 0, 16, 0] && five[48] == 36 && five[49] == 36 && five[56] == 1
        && five[52] == 2 && five[53] == 2;
    let mut want = [0u8; KEY_REQ_LEN];
    want[..28].copy_from_slice(&[0, 1, 1, 0, 1, 0, 0, 0, 0x27, 0, 68, 0, 1, 1, 1, 0, 0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0, 4, 0, 16, 1, 1]);
    want[28..44].copy_from_slice(&key);
    ok &= key_req(peer, WCID_PEER, true, 0, &key) == want;
    let mut want = [0u8; KEY_REQ_LEN];
    want[..28].copy_from_slice(&[0, 19, 1, 0, 1, 0x0e, 0, 0, 0x27, 0, 68, 0, 1, 0, 0, 0, 0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0, 4, 2, 16, 19, 1]);
    want[28..44].copy_from_slice(&key);
    ok &= key_req(peer, WCID_BMC, false, 2, &key) == want;
    let mut eapol = alloc::vec![0u8; ETH_HDR + 4];
    eapol[12] = 0x88;
    eapol[13] = 0x8e;
    let mut ip = alloc::vec![0u8; 60];
    ip[12] = 0x08;
    ok &= s10_is_eapol(&eapol) && !s10_is_eapol(&ip) && !s10_is_eapol(&eapol[..ETH_HDR + 3]) && !s10_is_eapol(&[]);
    ok
}

// ── S10REKEY (zenbook-ledger Z63) — the WPA2 group-key rekey on the secured link (IEEE Std 802.11-2020 §12.7.7) ───────────────────
// After the 4-way handshake the AP replaces the group key (typically hourly) with group message 1 (the new GTK wrapped under the
// KEK); a station that does not answer loses broadcast and multicast and is deauthenticated by many APs. `link_rx`'s divert keeps
// lifting EAPOL into `S10_EAPOL` for the link's whole life; the station hands its pure judge here once SECURED (`rekey_arm`: a
// closure owning `wifi::assoc::Rekey`, the KCK/KEK, so the driver names no wifi type), and `rekey_tick` — on the `mt7925-rejoin`
// task's poll, never the render loop (`set_keys` waits on the MCU up to EVENT_MS per command) — takes the mailbox, judges, installs
// the GTK through `set_keys`' group arm, then answers group message 2 through `eapol_tx` (wpa_supplicant's and mt76's order: install,
// then message 2, which is unicast under the PTK). `rekey_clear` (from `link_down`) drops the record (its Drop zeroizes the keys).
// S10_REKEY is a LEAF lock: the judge (pure, no sleep) runs under it; `set_keys` and `eapol_tx` run with it released; never held
// with S9_LINK, MCU_LINK or S10_EAPOL. Owed: S10-MFP (the IGTK KDE), S10-REKEY-EVENTRACE. Facts from the standard; no code copied.

/// The station's group-message-1 judge: the whole Ethernet frame in -> (group message 2 as an Ethernet frame, the GTK's key id, the
/// GTK, the replay counter), or the refusal's word. It owns the KCK/KEK; dropping it zeroizes them.
pub type GroupJudge = alloc::boxed::Box<dyn FnMut(&[u8]) -> Result<(Vec<u8>, u8, [u8; 16], u64), &'static str> + Send>;

/// One secured link's rekey record: the judge, the BSS and its channel (for `set_keys`), the rekeys installed.
struct Rekey10 {
    judge: GroupJudge,
    bssid: [u8; 6],
    ch: u8,
    n: u32,
}

/// Some from a SECURED `tests wifijoin` until `link_down`. A LEAF lock (see the block head).
static S10_REKEY: crate::sync::Mutex<Option<Rekey10>> = crate::sync::Mutex::new(None);

/// The station's arm after SECURED: the record replaced (an old one dropped, its keys zeroized), the mailbox emptied, and the
/// `mt7925-rejoin` task started if it is not yet (its poll runs `rekey_tick`).
pub fn rekey_arm(bssid: [u8; 6], ch: u8, judge: GroupJudge) {
    let old = S10_REKEY.lock().replace(Rekey10 { judge, bssid, ch, n: 0 });
    drop(old);
    let stale = S10_EAPOL.lock().take();
    drop(stale);
    #[cfg(feature = "wifi")]
    {
        rejoin_spawn();
    }
}

/// From `link_down`: the record dropped (the KCK/KEK zeroized by its Drop), the mailbox emptied; the rekeys it installed (0 with none).
fn rekey_clear() -> u32 {
    let old = S10_REKEY.lock().take();
    let n = old.as_ref().map_or(0, |r| r.n);
    drop(old);
    let stale = S10_EAPOL.lock().take();
    drop(stale);
    n
}

/// One check from the `mt7925-rejoin` poll: nothing with no record; else the newest EAPOL frame (the mailbox when smolnet drains
/// the link, else one `eapol_rx` drive), judged under S10_REKEY, then with the lock released the GTK into the radio (`set_keys`'
/// group arm, the GTK wiped after) and group message 2 out (`eapol_tx`). One line per frame: installed or REFUSED by name.
pub fn rekey_tick() {
    if S10_REKEY.lock().is_none() {
        return;
    }
    let f = if bound() { S10_EAPOL.lock().take() } else { eapol_rx() };
    let Some(f) = f else { return };
    let r = S10_REKEY.lock().as_mut().map(|k| ((k.judge)(&f), k.bssid, k.ch));
    let Some((res, bssid, ch)) = r else { return };
    let refuse = |why: &str| {
        serial_println!("[mt7925] s10 rekey -> REFUSED why={}", why);
    };
    let (m2, gtk_id, mut gtk, replay) = match res {
        Ok(v) => v,
        Err(w) => return refuse(w),
    };
    let set = set_keys(bssid, ch, None, &gtk, gtk_id);
    crypto_core::ct::Zeroize::zeroize(&mut gtk[..]);
    if let Err(e) = set {
        return refuse(&alloc::format!("set-key({})", e));
    }
    if !eapol_tx(&m2) {
        return refuse("tx");
    }
    let n = S10_REKEY.lock().as_mut().map_or(0, |k| {
        k.n += 1;
        k.n
    });
    serial_println!("[mt7925] s10 rekey n={} gtk_id={} replay={} -> installed", n, gtk_id, replay);
}

// ── S9REJOIN (zenbook-ledger Z59) — S9-UNLINK (the firmware's records unwound on link_down) and S9-REJOIN (the kept plan, re-joined) ──
// `link_down` left the firmware holding the dead link: the AP peer's STA_REC (wcid 1, ASSOC), the BSS_INFO connected, the BCNFT
// beacon monitor armed; and after a loss the station stayed down until the operator typed `tests wifijoin` again. Linux (v6.17
// mt76 + mac80211, facts only): `ieee80211_set_disassoc` flushes the AP station first — mt7925's link-station removal sends
// STA_REC_UPDATE with enable off (the header, a REMOVE TLV 0x25 {action 0} and a bare MLD_OFF TLV 0x23), clears the WTBL's ADM
// count (an MMIO write, not built: S9-WTBLCLR), then BSS_INFO_UPDATE with enable off (BASIC conn_state 1, which is our join's own
// `bss_req(.., WCID_PEER, false)` byte for byte) — and only then the ASSOC change turns the beacon filter off: ONE BSS_INFO_UPDATE
// with the PM_DISABLE TLV (27; not a BCNFT TLV) plus an rx-filter clear of DROP_OTHER_BEACON we never set (S9-BCNDROP). Here:
// `unlink` sends those three in that order from `link_down`, each acked (`fw=` on its line). The rejoin: the station keeps its
// successful OPEN join's plan (`rejoin_keep`, wifi/verb.rs; cleared by every operator `tests wifijoin`); `watch` arms STATIONREJOIN's
// ONE backoff (`wifi::assoc::Rejoin`, B891's: 1 s after the loss, doubling, capped 30 s, no attempt cap, moved out of the wifi10-only
// data.rs for this) on a loss; ONE `mt7925-rejoin` task (B891's `rejoin_spawn` shape, spawned once by the first kept plan; never
// the render loop's parked arm: a join takes seconds and the DHCP drive up to 12 s) runs each due attempt through the station's
// handed-in `again` (the operator's ONE join path, `join_frames` -> `join`), then the station's `s9_dhcp`. REJOIN is a LEAF lock:
// never held while taking MCU_LINK, S9_LINK or STACK, never across an attempt. Owed: S9-ROAM, S10-PMK-KEEP (a WPA2 rejoin needs the
// PMK kept; only open plans are kept), WIFILEAVE, S9-BCNINT, S9-BCNDROP, S9-WTBLCLR, REJOIN-TASK-ONE, S10-KEYZERO (after S9-UNLINK).
// Hardware facts only; no code copied.

/// STA_REC_REMOVE and STA_REC_MLD_OFF (mt76_connac_mcu.h); UNI_BSS_INFO_PM_DISABLE; the remove body's length.
const STA_TAG_REMOVE: u16 = 0x25;
const STA_TAG_MLD_OFF: u16 = 0x23;
const BSS_TAG_PM_DISABLE: u16 = 27;
const STA_REMOVE_LEN: usize = 20;

/// STA_REC_UPDATE remove body (pure, 20 bytes) for the AP peer: header {bss 0, wlan lo 1, 2 TLVs, append, muar 0, wlan hi 0, rsv},
/// REMOVE {tag 0x25, len 8, action 0, 3 pad}, MLD_OFF {tag 0x23, len 4}.
fn sta_remove_req() -> [u8; STA_REMOVE_LEN] {
    let mut v = [0u8; STA_REMOVE_LEN];
    v[1] = WCID_PEER as u8;
    v[2..4].copy_from_slice(&2u16.to_le_bytes());
    v[4] = 1;
    let _ = put_tlv(&mut v, 8, STA_TAG_REMOVE, 8);
    let _ = put_tlv(&mut v, 16, STA_TAG_MLD_OFF, 4);
    v
}

/// BSS_INFO_UPDATE beacon-monitor off (pure, 8 bytes): header {bss 0, 3 pad}, PM_DISABLE {tag 27, len 4}.
fn pm_disable_req() -> [u8; 8] {
    let mut v = [0u8; 8];
    let _ = put_tlv(&mut v, 4, BSS_TAG_PM_DISABLE, 4);
    v
}

/// S9-UNLINK, from `link_down` with the channel owned by value: STA_REC remove, BSS_INFO not connected (the link's BSSID, band and
/// PHY band), beacon monitor off — each sent whatever the one before answered (linux ignores the errors too). The `fw=` text.
fn unlink(h: &mut Hw, l: &Link9) -> String {
    let s = uni_cmd(h, UNI_STA_REC, &sta_remove_req());
    let b = uni_cmd(h, UNI_BSS_INFO, &bss_req(l.bssid, l.band, Some(l.five), WCID_PEER, false));
    let c = uni_cmd(h, UNI_BSS_INFO, &pm_disable_req());
    alloc::format!("sta:{},bss:{},bcnft:{}", ack_text(&s), ack_text(&b), ack_text(&c))
}

/// The unlink rows (hand-computed from the field tables, no capture; folded into `link_kat`): the remove body, the PM_DISABLE body,
/// and the not-connected BSS_INFO on band 0 (active 1, band 0, conn_state 1, sta index 1).
fn unlink_rows() -> bool {
    let b = bss_req([0x00, 0x11, 0x22, 0x33, 0x44, 0x55], 0, Some(false), WCID_PEER, false);
    sta_remove_req() == [0, 1, 2, 0, 1, 0, 0, 0, 0x25, 0, 8, 0, 0, 0, 0, 0, 0x23, 0, 4, 0]
        && pm_disable_req() == [0, 0, 0, 0, 27, 0, 4, 0]
        && b[8] == 1 && b[11] == 0 && b[16] == 1 && u16::from_le_bytes([b[30], b[31]]) == WCID_PEER
}

/// The station's ONE rejoin attempt (`wifi/verb.rs::rejoin_again`, the operator's join path): Ok = the S9 link is up again; Err =
/// the word on the rejoin line (`auth`, `assoc`, `link`, or the join's own refusal).
#[cfg(feature = "wifi")]
pub type Again = fn(&RejoinPlan) -> Result<(), &'static str>;

/// What a successful open join leaves for a rejoin: the SSID bytes, the BSS, its channel and capability, the station's judge, the
/// attempt and the post-link DHCP drive (both the station's: the driver links no wifi_core).
#[cfg(feature = "wifi")]
#[derive(Clone, Copy)]
pub struct RejoinPlan {
    pub ssid: [u8; 32],
    pub ssid_len: u8,
    pub bssid: [u8; 6],
    pub channel: u8,
    pub cap: u16,
    pub watch: Judge,
    pub again: Again,
    pub dhcp: fn(),
    /// MT7925S10PMKKEEP (zenbook-ledger Z64): a WPA2 plan's derived PMK (zeroized by `pmk_forget`, on every drop of the `Kept`).
    pub pmk: [u8; 32],
    /// The AP's beacon RSN element the handshake's message 3 is checked against; `rsn_len` 0 = an OPEN plan (the valid flag).
    pub rsn: [u8; 128],
    pub rsn_len: u8,
}

/// The kept plan and its backoff (STATIONREJOIN's ONE ladder).
#[cfg(feature = "wifi")]
struct Kept {
    plan: RejoinPlan,
    r: crate::wifi::assoc::Rejoin,
}

/// MT7925S10PMKKEEP (zenbook-ledger Z64): the kept PMK dies with the plan — replaced by a new keep, forgotten by the operator's
/// next join, or dropped (the KAT's local one).
#[cfg(feature = "wifi")]
impl Drop for Kept {
    fn drop(&mut self) {
        pmk_forget(self);
    }
}

/// Some from a successful open `tests wifijoin` until the next operator `tests wifijoin`. A LEAF lock (see the block head).
#[cfg(feature = "wifi")]
static REJOIN: crate::sync::Mutex<Option<Kept>> = crate::sync::Mutex::new(None);
/// The `mt7925-rejoin` task was spawned (once per boot).
#[cfg(feature = "wifi")]
static REJOIN_SPAWNED: AtomicBool = AtomicBool::new(false);
/// The task's check cadence (B891's).
#[cfg(feature = "wifi")]
const REJOIN_POLL_MS: u64 = 250;

/// The station's keep (Some, after a successful open join, the link up) or forget (None, at every operator `tests wifijoin`); the
/// first Some spawns the task (B891's CPU choice: an xHCI worker, else the service CPU).
#[cfg(feature = "wifi")]
pub fn rejoin_keep(p: Option<RejoinPlan>) {
    let on = p.is_some();
    *REJOIN.lock() = p.map(|plan| Kept { plan, r: crate::wifi::assoc::Rejoin::new() });
    if on {
        rejoin_spawn();
    }
}

/// The `mt7925-rejoin` task, spawned once per boot (hoisted verbatim out of `rejoin_keep` by S10REKEY, zenbook-ledger Z63, so a
/// secured join's `rekey_arm` starts the same ONE task: its poll also runs `rekey_tick`).
#[cfg(feature = "wifi")]
fn rejoin_spawn() {
    if !REJOIN_SPAWNED.swap(true, Ordering::AcqRel) {
        let cpu = crate::arch::smp::xhci_worker_cpu(0).or(crate::arch::smp::service_cpu()).unwrap_or(0);
        crate::arch::sched::spawn("mt7925-rejoin", rejoin_task, 0, cpu, crate::arch::sched::PRIO_NORMAL);
    }
}

/// From `watch` after a loss (no other lock held): the kept plan's backoff armed (first attempt 1 s later). False with no plan.
#[cfg(feature = "wifi")]
fn rejoin_lose(why: &'static str) -> bool {
    match REJOIN.lock().as_mut() {
        Some(k) => {
            k.r.lose(crate::arch::ms(), why);
            true
        }
        None => false,
    }
}

/// No station without `wifi`: nothing is ever kept.
#[cfg(not(feature = "wifi"))]
fn rejoin_lose(_: &'static str) -> bool {
    false
}

/// The rejoin task: every `REJOIN_POLL_MS`, a due attempt (`rejoin_tick`); idle otherwise.
#[cfg(feature = "wifi")]
fn rejoin_task(_: usize) {
    loop {
        crate::arch::sched::sleep_ms(REJOIN_POLL_MS);
        rejoin_tick(crate::arch::ms()); rekey_tick(); // S10REKEY (zenbook-ledger Z63): the group-key rekey rides the same poll (never the render loop: its key command waits on the MCU)
    }
}

/// Under REJOIN (pure over the Kept): a due attempt starts — (the plan, its number since the loss, ms since the loss, why).
#[cfg(feature = "wifi")]
fn rejoin_begin(k: &mut Kept, now: u64) -> Option<(RejoinPlan, u32, u64, &'static str)> {
    if !k.r.due(now) {
        return None;
    }
    let (n, after) = k.r.attempt(now);
    Some((k.plan, n, after, k.r.why))
}

/// Under REJOIN (pure over the Kept): the attempt's verdict recorded — linked resets the ladder, a failure doubles the wait. The
/// re-joins that linked.
#[cfg(feature = "wifi")]
fn rejoin_end(k: &mut Kept, ok: bool, now: u64) -> u32 {
    if ok {
        k.r.linked();
    } else {
        k.r.failed(now);
    }
    k.r.rejoins
}

/// One check: nothing while a scan or join holds the channel (no attempt counted); else a due attempt — the plan copied out under
/// REJOIN, the lock released, the station's `again`, the verdict recorded under REJOIN, one line, and on linked the station's DHCP.
#[cfg(feature = "wifi")]
fn rejoin_tick(now: u64) {
    if MCU_LINK.try_lock().map_or(true, |g| g.is_none()) {
        return;
    }
    let go = REJOIN.lock().as_mut().and_then(|k| rejoin_begin(k, now));
    let Some((mut plan, n, after, why)) = go else { return };
    let res = (plan.again)(&plan);
    let secured = (res.is_ok() && plan.rsn_len > 0) as u8; // MT7925S10PMKKEEP (Z64): the rejoin ran the handshake from the kept PMK
    crate::wifi::assoc::zeroize_pmk(&mut plan.pmk); // the stack copy of the kept PMK wiped as soon as the attempt returns
    let rejoins = REJOIN.lock().as_mut().map_or(0, |k| rejoin_end(k, res.is_ok(), crate::arch::ms()));
    let verdict = match res {
        Ok(()) => String::from("linked"),
        Err(w) => alloc::format!("failed:{}", w),
    };
    serial_println!("[mt7925] s9 rejoin n={} after={} why={} -> {} secured={} rejoins={}", n, after, why, verdict, secured, rejoins);
    if res.is_ok() {
        (plan.dhcp)();
    }
}

/// The KAT's stub attempt: refused twice, then linked (counted by `REJOIN_KAT_N`, reset by `rejoin_kat`).
#[cfg(feature = "wifi")]
static REJOIN_KAT_N: AtomicU8 = AtomicU8::new(0);
#[cfg(feature = "wifi")]
fn rejoin_kat_again(_: &RejoinPlan) -> Result<(), &'static str> {
    if REJOIN_KAT_N.fetch_add(1, Ordering::Relaxed) < 2 { Err("auth") } else { Ok(()) }
}
#[cfg(feature = "wifi")]
fn rejoin_kat_judge(_: &[u8], _: [u8; 6], _: [u8; 6]) -> Option<(&'static str, u16)> {
    None
}
#[cfg(feature = "wifi")]
fn rejoin_kat_dhcp() {}

/// `tests wifijoin`'s S9-REJOIN rows (no device; the stub seam is the plan's `again`, as the judge is `watch_kat`'s): on a local
/// Kept, a beacon loss at 1000 ms — nothing due at 1999; attempt 1 at 2000 (1000 ms after) refused; nothing at 3999; attempt 2 at
/// 4000 refused; nothing at 7999; attempt 3 at 8000 linked (rejoins 1); never due again; a deauth at 200000 resets the ladder
/// (nothing at 200999, attempt 1 at 201000, linked, rejoins 2) — then `unlink_rows`.
#[cfg(feature = "wifi")]
pub fn rejoin_kat() -> bool {
    fn step(k: &mut Kept, now: u64) -> Option<(u32, u64, bool, u32)> {
        let (p, n, after, _) = rejoin_begin(k, now)?;
        let ok = (p.again)(&p).is_ok();
        Some((n, after, ok, rejoin_end(k, ok, now)))
    }
    REJOIN_KAT_N.store(0, Ordering::Relaxed);
    let plan = RejoinPlan {
        ssid: [0; 32], ssid_len: 0, bssid: [0x00, 0x11, 0x22, 0x33, 0x44, 0x55], channel: 6, cap: 0x0001,
        watch: rejoin_kat_judge, again: rejoin_kat_again, dhcp: rejoin_kat_dhcp, pmk: [0; 32], rsn: [0; 128], rsn_len: 0,
    };
    let mut k = Kept { plan, r: crate::wifi::assoc::Rejoin::new() };
    k.r.lose(1000, "beacon-loss");
    let mut ok = step(&mut k, 1999).is_none()
        && step(&mut k, 2000) == Some((1, 1000, false, 0))
        && step(&mut k, 3999).is_none()
        && step(&mut k, 4000) == Some((2, 3000, false, 0))
        && step(&mut k, 7999).is_none()
        && step(&mut k, 8000) == Some((3, 7000, true, 1))
        && step(&mut k, 1_000_000).is_none()
        && k.r.why == "beacon-loss";
    k.r.lose(200_000, "deauth");
    ok &= step(&mut k, 200_999).is_none() && step(&mut k, 201_000) == Some((1, 1000, true, 2)) && k.r.why == "deauth";
    ok && unlink_rows()
}

// ── MT7925S10PMKKEEP (zenbook-ledger Z64) — the kept PMK of a WPA2 plan: zeroize and the KAT's read-back ─────────────────────────
/// Wipe a kept plan's PMK in place (the `Drop` of `Kept`; the beacon RSN element is not a secret). Not in `link_down`: the PMK is
/// per network and survives a link loss, it dies with the plan.
#[cfg(feature = "wifi")]
fn pmk_forget(k: &mut Kept) {
    crate::wifi::assoc::zeroize_pmk(&mut k.plan.pmk);
}

/// `tests wifijoin`'s S10PMKKEEP rows (no device): a local WPA2 Kept (the pattern PMK = Annex J.4 test 2's PSK, a 22-octet RSN
/// element) — `rejoin_begin` copies the plan out with the PMK and the element intact; `pmk_forget` zeroizes it in place; an open
/// plan carries `rsn_len` 0. Returns the PMK read back from the copy (for `assoc::pmk_kat`), None if any row failed.
#[cfg(feature = "wifi")]
pub fn pmk_rows() -> Option<[u8; 32]> {
    const P: [u8; 32] = [
        0x0d, 0xc0, 0xd6, 0xeb, 0x90, 0x55, 0x5e, 0xd6, 0x41, 0x97, 0x56, 0xb9, 0xa1, 0x5e, 0xc3, 0xe3,
        0x20, 0x9b, 0x63, 0xdf, 0x70, 0x7d, 0xd5, 0x08, 0xd1, 0x45, 0x81, 0xf8, 0x98, 0x27, 0x21, 0xaf,
    ];
    let mut rsn = [0u8; 128];
    rsn[..22].copy_from_slice(&[0x30u8; 22]);
    let wpa = RejoinPlan {
        ssid: [0; 32], ssid_len: 0, bssid: [0x00, 0x11, 0x22, 0x33, 0x44, 0x55], channel: 6, cap: 0x0001,
        watch: rejoin_kat_judge, again: rejoin_kat_again, dhcp: rejoin_kat_dhcp, pmk: P, rsn, rsn_len: 22,
    };
    let open = RejoinPlan { pmk: [0; 32], rsn: [0; 128], rsn_len: 0, ..wpa };
    let mut k = Kept { plan: wpa, r: crate::wifi::assoc::Rejoin::new() };
    k.r.lose(1000, "beacon-loss");
    let (p, _, _, _) = rejoin_begin(&mut k, 2000)?;
    let got = p.pmk;
    let kept = got == P && p.rsn_len == 22 && p.rsn[..22] == [0x30u8; 22] && open.rsn_len == 0;
    pmk_forget(&mut k);
    if kept && k.plan.pmk == [0u8; 32] { Some(got) } else { None }
}

// ── WIFILEAVE (zenbook-ledger Z68) — the operator leaves the network: the plan dropped, the station's leaving notice, the ONE teardown ──
// A leave is final: `rejoin_keep(None)`'s drop path (the kept plan's `Drop` zeroizes the PMK; the `mt7925-rejoin` task idles, nothing
// respawns it until the next operator join), `rekey_clear`, the station's own deauthentication (reason 3, leaving) posted on the
// link's TX ring BEFORE `link_down` (whose STA_REC remove frees the peer's WTBL entry), the ONE teardown `link_end`. The MCU has no
// disconnect command of its own (S7/S8 built none; `unlink` only informs the firmware), so the notice is a raw management frame; a
// silent station is aged out by the AP's inactivity timer. Radio keys: the PTK goes with the STA_REC remove; the GTK slot on wlan 19
// stays until a rejoin overwrites it (`set_keys` only installs; S10-KEYZERO owed). Owed: S9-ROAM, S10-PMKSA, S10-KEYSTORE,
// S10-KEYZERO, WIFILEAVE-BUILD, WIFILEAVE-DEAUTH-PMF. Hardware facts only; no code copied.

/// The ONE link end, hoisted verbatim out of `watch` (and now also `leave`'s): the MCU channel taken out of `MCU_LINK` (None while a scan
/// or join holds it), `link_down`, the channel put back, driver-own handed back when the link's run took it. The `rel=` word.
fn link_end(why: &str) -> &'static str {
    let mut hw = MCU_LINK.try_lock().and_then(|mut g| g.take()); // S9REJOIN (Z59, S9-UNLINK): the channel taken out (lock released) for link_down's commands, put back after; None while a scan/join holds it
    let took = link_down(why, hw.as_mut());
    if let Some(hw) = hw {
        *MCU_LINK.lock() = Some(hw);
    }
    let held = *PROBE.lock();
    match held {
        Some(p) => give_back(p.bar0 + CONN_HOST_CSR_TOP + LPCTL, took),
        None => "found",
    }
}

/// The kept plan out of its slot: the PMK wiped in place (`pmk_forget`) and read back (all zero when it worked); None = no plan.
#[cfg(feature = "wifi")]
fn plan_drop_of(k: &mut Option<Kept>) -> Option<[u8; 32]> {
    let mut old = k.take()?;
    pmk_forget(&mut old);
    Some(old.plan.pmk)
}

/// The operator's leave (`tests wifileave`, no lock held): the plan dropped and its PMK wiped, the group-rekey record cleared, the
/// station's deauthentication (`deauth(me, bssid)`, the station's frame) posted on the link's ring and awaited, the ONE teardown, ONE line.
#[cfg(feature = "wifi")]
pub fn leave(deauth: &dyn Fn([u8; 6], [u8; 6]) -> Vec<u8>) {
    let pmk = match plan_drop_of(&mut *REJOIN.lock()) {
        Some(z) if z == [0u8; 32] => "zeroized",
        Some(_) => "NOT-ZEROED",
        None => "none",
    };
    let keys = rekey_clear();
    let mut notice = "skip(no-link)";
    let mut wait = None;
    {
        let mut g = S9_LINK.lock();
        if let Some(l) = g.as_mut() {
            let f = deauth(l.me, l.bssid);
            if f.len() < 24 || f.len() > MGMT_MAX {
                notice = "skip(frame-len)";
            } else if !room(l) {
                notice = "skip(no-room)";
            } else {
                let slot = l.txb + l.wi as u64 * DATA_SLOT;
                let w = mgmt_txwi(&f, slot + TX_FRAME_OFF, LEAVE_TOK, l.five, l.band, WCID_PEER);
                l.wi = tx_put(l.q0, l.wi, slot, &f, &w);
                l.tx += 1;
                notice = "sent";
                wait = Some((l.q0, l.wi));
            }
        }
    }
    if let Some((q, wi)) = wait {
        if !tx_done(q, wi) {
            notice = "sent(unfetched)";
        }
    }
    let rel = link_end("leave");
    serial_println!(
        "[mt7925] s9 left reason=operator keys=cleared pmk={} deauth={} rejoins=0 rekeys={} radiokeys=ptk-sta-remove,gtk-owed(S10-KEYZERO) rel={}",
        pmk, notice, keys, rel
    );
}

/// The management token of the leaving notice (the TXD's msdu id; a u8 here, below the data frames' 0x100 range).
const LEAVE_TOK: u8 = 3;

/// `tests wifileave`'s leg (no device): a local kept plan carrying a PMK, its backoff armed and one attempt due; the drop leaves no
/// plan (nothing can come due), the PMK reads back all zero, a second drop and a drop with no plan are no-ops.
#[cfg(feature = "wifi")]
pub fn leave_kat() -> bool {
    let plan = RejoinPlan {
        ssid: [0; 32], ssid_len: 0, bssid: [0x00, 0x11, 0x22, 0x33, 0x44, 0x55], channel: 6, cap: 0x0001,
        watch: rejoin_kat_judge, again: rejoin_kat_again, dhcp: rejoin_kat_dhcp, pmk: [0x5a; 32], rsn: [0; 128], rsn_len: 0,
    };
    let mut k = Some(Kept { plan, r: crate::wifi::assoc::Rejoin::new() });
    if let Some(x) = k.as_mut() {
        x.r.lose(1000, "beacon-loss");
    }
    let due = k.as_mut().and_then(|x| rejoin_begin(x, 2000)).is_some();
    let z = plan_drop_of(&mut k);
    let gone = k.is_none() && k.as_mut().and_then(|x| rejoin_begin(x, 1_000_000)).is_none();
    due && gone && z == Some([0u8; 32]) && plan_drop_of(&mut k).is_none()
}
