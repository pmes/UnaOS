//! CHARTER: Kernel — driver (the MediaTek MT7925 Wi-Fi 7 radio's S1 probe, zenbook-ledger Z5, and S2 driver-own and the chip id, Z11; no CODEX §2 handler owns a NIC driver, the BCM4331 `wifi/**` sits in the same seam)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! MT7925 — S1, the probe (ZENBOOKMT7925, zenbook-ledger Z5). The ladder is
//! `docs/dev/OS/06_NETWORK_STACK/mt7925.md`; S2 (MT7925S2, Z11) is the file tail; S3..S10 are owed there by name.
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
//! The WFSYS reset and the WFDMA are S3's. A machine without the radio (QEMU, the rMBP) parks on the first pass: no
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

/// CHIPID and REV through the L1 remap: the selector saved, the 0x7001 window selected (read back to push it), the two
/// reads, the selector restored and read back. (chip, rev, selector restored).
fn read_chip(bar0: u64) -> (u32, u32, bool) {
    let sel = bar0 + HIF_REMAP_L1;
    let win = bar0 + REMAP_BASE_L1;
    // SAFETY: both pages are mapped uncacheable and proved present (`map_page`); the driver owns the chip (the
    // handshake above, or found so); the selector is put back before this block ends.
    unsafe {
        let backup = core::ptr::read_volatile(sel as *const u32);
        core::ptr::write_volatile(sel as *mut u32, (backup & 0xffff) | (HW_CHIPID & 0xffff_0000));
        let _ = core::ptr::read_volatile(sel as *const u32); // push the selector write (linux reads it back)
        let chip = core::ptr::read_volatile((win + (HW_CHIPID & 0xffff) as u64) as *const u32);
        let rev = core::ptr::read_volatile((win + (HW_REV & 0xffff) as u64) as *const u32);
        core::ptr::write_volatile(sel as *mut u32, backup);
        let restored = core::ptr::read_volatile(sel as *const u32) == backup;
        (chip, rev, restored)
    }
}
