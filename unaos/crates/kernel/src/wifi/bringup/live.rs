// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! WIFI7 (rmbp-ledger B505) — flight 27's next rungs, after the first upload the card medium carried
//! (`f27-boot1.log`: `:: WIFI5: … uploaded_this_boot=1 -> READY ::`). Design and witness:
//! `docs/dev/evidence/rmbp-1005/wifi7.md`; the ladder: `bcm4331.md` §S5–S6 and §8 (S5-0, S5r, S5c, S5d).
//!
//! CHARTER: Kernel — driver (the d11 is the kernel's device; no CODEX §2 handler owns a radio
//! register). A child of `bringup` on its covenanted accessors. The ONE register this module writes
//! is `SHM_CONTROL`, the shared-memory READ path's address selector ([SPEC-V3 SHM] fact 2, routing
//! 0x0001 — the flown shm-probe's class); its pre-image is read first and put back last. `SHM_DATA` is
//! only READ. No MACCTL, no wrapper register, no radio or PHY port.
//!
//! * **S5u — the ucode's own revision, read back from SHM** ([SPEC-V3 SHM] fact 5: shared 0x0000/0x0002
//!   = "uCode Revision High/Low 16 bits"). C2 read `shared[+0x00]` BEFORE the upload (the EFI's image:
//!   0x0288 on f13–f27). The upload streams through routing 0x0300 (microcode memory), not shared
//!   memory; the host's only shared-routing WRITES are the initvals' own `SHM_CONTROL` selects, whose
//!   lowest dword index is computed below from the staged records (f27's `s5i-delta` rows: index 3,
//!   byte 0x000c) and the window only auto-increments upward from a select. So a word 0x0000 that moved
//!   from the EFI's 0x0288 to our image's revision with `psm-run=1` was written by OUR microcode: the
//!   first direct proof the uploaded image EXECUTES rather than merely verifying in ucode memory.
//! * **S5d on the EFI's channel — the 5 s SHM watch** (`tests wifi`, R80: nothing tests at boot). No
//!   page in [SPEC-V3]/[SPEC-V4] the tree carries pins an SHM offset for an RX-frame counter, so the
//!   count is NOT decoded (R83): the shared segment is read at t0 and t0+5 s (TSF-timed) and every
//!   mover is printed by offset; the boot snapshot gives the since-boot movers. MAC-enable (MACCTL bit
//!   0, [SPEC-V4 802.11/Registers]) is printed beside it — the discriminator for an all-quiet answer.
//! * **S5r / S5c** are declined on wifi4's own lines (`phy_once`), with [`s5r_advisory`]'s decode and
//!   [`S5C_OWED`]'s table names folded onto them — said once, not twice.

use super::{
    r16, r32, w32, Writes, CFG_BAR0_WIN, D11_MACCTL, D11_SHM_CONTROL, D11_SHM_DATA, D11_SHM_DATA_UNALIGNED,
    D11_TSF_HIGH, D11_TSF_LOW, IOCTL_CLK, MACCTL_PSM_RUN, SHM_ROUTE_SHARED, UPLOAD_PROVEN,
};
use crate::arch::pci::read_config_32;
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// The shared segment watched: 1024 dwords = bytes 0x0000..0x1000 (C2's extent).
const SHARED_DWORDS: usize = 1024;
/// The watch window, in TSF microseconds (the TSF pair advanced at 1 MHz on our metal — `D11_TSF_*`).
const WATCH_US: u64 = 5_000_000;
/// Mover rows printed per watch.
const MAX_ROWS: usize = 16;
/// `[SPEC-V4 802.11/Registers]` MACCTL 0x1 "MAC Enabled" (fact 2): the ucode receives only with it set.
const MACCTL_ENABLED: u32 = 0x0000_0001;
/// `[SPEC-V4 802.11/CoreFlags]`, SSB TM State Low: PHY Clock Enable / PHY Reset / MAC PHY Clock
/// Control Enable (the values wifi4's REFUSED line already quotes).
const TMSLOW_PHY_CLKEN: u32 = 0x0004_0000;
const TMSLOW_PHY_RESET: u32 = 0x0008_0000;
const TMSLOW_MACPHYCLK: u32 = 0x0010_0000;
/// The same page's generic Clock Enable, bit 16. The AI IOCTL carries CLK at bit 0 (`IOCTL_CLK`), so
/// the core-flag field sits 16 lower — an ADVISORY offset, coherence and never a pin.
const TMSLOW_CLK: u32 = 0x0001_0000;
/// S5c's owed tables, by name: none is in [SPEC-V3] or [SPEC-V4] (wifi4's checked-page list).
pub(super) const S5C_OWED: &str =
    "radio2059-init,radio2059-chan-2g,radio2059-chan-5g,htphy-init,htphy-tableram-ports,htphy-chan-bb";

/// `shared[+0x00]` as C2 read it BEFORE the upload (u32::MAX = never read this boot).
static PRE_WORD: AtomicU32 = AtomicU32::new(u32::MAX);
/// The mapped BAR0 VA, latched only on a boot whose ucode was seen executing (0 = the watch refuses).
static BAR0: AtomicU64 = AtomicU64::new(0);
/// `bus<<16 | dev<<8 | func` and the d11 base, for the watch's live `cfg:0x80` check.
static BDF: AtomicU32 = AtomicU32::new(0);
static D11_BASE: AtomicU32 = AtomicU32::new(0);
/// The revision our microcode published (0 = unread / not executing).
static REV: AtomicU32 = AtomicU32::new(0);
/// The boot snapshot of the shared segment and its TSF.
static BOOT_SNAP: Mutex<Vec<u32>> = Mutex::new(Vec::new());
static BOOT_TSF: AtomicU64 = AtomicU64::new(0);

/// C2's pre-upload `shared[+0x00]` (capture.rs, before `upload BEGIN`).
pub(super) fn note_pre(word: u32) {
    PRE_WORD.store(word & 0xFFFF, Ordering::Relaxed);
}

/// S5r's advisory decode of the measured AI IOCTL word, for wifi4's phy-reset line.
pub(super) fn s5r_advisory(ioctl: u32) -> String {
    let sh = TMSLOW_CLK.trailing_zeros() - IOCTL_CLK.trailing_zeros();
    let bit = |m: u32| ((ioctl & (m >> sh)) != 0) as u8;
    alloc::format!(
        "also=s5-0-efi-tune-would-be-destroyed ioctl={:#010x} advisory(CoreFlags>>{}) phyclk={} phyreset={} macphyclk={}",
        ioctl, sh, bit(TMSLOW_PHY_CLKEN), bit(TMSLOW_PHY_RESET), bit(TMSLOW_MACPHYCLK)
    )
}

unsafe fn tsf(bar0: u64) -> u64 {
    let lo = r32(bar0, D11_TSF_LOW);
    let hi = r32(bar0, D11_TSF_HIGH);
    ((hi as u64) << 32) | lo as u64
}

/// One shared-memory halfword through routing 0x0001 (the handshake's own read path). One select.
unsafe fn shm16(bar0: u64, off: u16) -> u16 {
    w32(bar0, D11_SHM_CONTROL, (SHM_ROUTE_SHARED << 16) | ((off as u32) >> 2));
    if off & 2 == 0 { r16(bar0, D11_SHM_DATA) } else { r16(bar0, D11_SHM_DATA_UNALIGNED) }
}

/// The whole shared segment as an auto-incrementing dword stream (C2's read path). One select.
unsafe fn snapshot(bar0: u64) -> Vec<u32> {
    w32(bar0, D11_SHM_CONTROL, SHM_ROUTE_SHARED << 16);
    let mut v = Vec::with_capacity(SHARED_DWORDS);
    for _ in 0..SHARED_DWORDS {
        v.push(r32(bar0, D11_SHM_DATA));
    }
    v
}

/// Put `SHM_CONTROL` back to the word read before this module's first select; `true` = read back equal.
unsafe fn restore_ctl(bar0: u64, pre: u32) -> bool {
    w32(bar0, D11_SHM_CONTROL, pre);
    r32(bar0, D11_SHM_CONTROL) == pre
}

/// The host's own shared-routing writes this boot: the staged initvals' `SHM_CONTROL` selects whose
/// routing's low byte is 0x01 (0x0001 Shared, and — conservatively — 0x0301, which [SPEC-V3 SHM]
/// lists with a "?"). Returns `(selects, lowest byte offset)`; `None` = a record touches the
/// control word in a shape this scan does not decode (a 16-bit write), so no claim is made.
fn host_shared_floor() -> Option<(u32, u32)> {
    let (mut n, mut low) = (0u32, u32::MAX);
    for role in ["initvals", "bsinitvals"] {
        let recs = super::super::firmware::with_staged(role, |d| wifi_core::fw::records(d))?.ok()?;
        for r in &recs {
            let off = r.offset as u64;
            if off < D11_SHM_CONTROL || off >= D11_SHM_CONTROL + 4 {
                continue;
            }
            if !r.wide || off != D11_SHM_CONTROL {
                return None;
            }
            if (r.value >> 16) & 0xFF == SHM_ROUTE_SHARED {
                n += 1;
                low = low.min((r.value & 0xFFFF) << 2);
            }
        }
    }
    Some((n, low))
}

/// After `phy_once`, on the same window (still on the d11 core: R8's restore runs after this returns).
/// Every select is counted in `w.core_regs`, so wifi2's audited end line includes them.
pub(super) fn boot(bar0: u64, bus: u8, dev: u8, func: u8, d11_base: u64, w: &mut Writes) {
    if !UPLOAD_PROVEN.load(Ordering::Relaxed) {
        serial_println!("[wifi] ucode rev REFUSED reason=wifi3-upload-not-proven — no SHM read, nothing written; `tests wifi` will refuse its watch");
        return;
    }
    // ── S5u ──
    let ctl_pre = unsafe { r32(bar0, D11_SHM_CONTROL) };
    let (rev, patch) = unsafe { (shm16(bar0, 0x0000), shm16(bar0, 0x0002)) };
    w.core_regs += 2;
    let macctl = unsafe { r32(bar0, D11_MACCTL) };
    let psm = (macctl & MACCTL_PSM_RUN) != 0;
    let pre = PRE_WORD.load(Ordering::Relaxed);
    let changed = pre != u32::MAX && pre != rev as u32;
    let floor = host_shared_floor();
    // The host cannot have written byte 0x0000 when its lowest shared select is above dword 0.
    let host_clear = matches!(floor, Some((_, low)) if low >= 4);
    let live = changed && psm && host_clear && rev != 0 && rev != 0xFFFF;
    let snap = if live { Some(unsafe { snapshot(bar0) }) } else { None };
    if snap.is_some() {
        w.core_regs += 1;
    }
    let t_snap = unsafe { tsf(bar0) };
    let restored = unsafe { restore_ctl(bar0, ctl_pre) };
    w.core_regs += 1;
    serial_println!(
        "[wifi] ucode rev={} patch={} from SHM pre={} changed={} host-shared-selects={} host-shared-low={} psm-run={} macctl={:#010x} shm-ctl-restored={} -> {} — [SPEC-V3 SHM] fact 5's revision words, read AFTER the initvals; the upload went to routing 0x0300 and the host's shared writes start at host-shared-low, so a moved word 0x0000 is the microcode's own",
        rev, patch,
        if pre == u32::MAX { String::from("unread") } else { alloc::format!("{:#06x}", pre) },
        changed as u8,
        floor.map(|(n, _)| alloc::format!("{}", n)).unwrap_or_else(|| String::from("undecoded")),
        match floor {
            Some((0, _)) => String::from("none"),
            Some((_, l)) => alloc::format!("{:#06x}", l),
            None => String::from("undecoded"),
        },
        psm as u8, macctl,
        if restored { "MATCH" } else { "FAILED" },
        if live { "EXECUTING" } else if pre == u32::MAX { "UNPROVEN(no-pre-image)" } else if !host_clear { "UNPROVEN(host-may-have-written)" } else { "NOT-EXECUTING" }
    );
    if live && restored {
        REV.store(rev as u32, Ordering::Relaxed);
        if let Some(s) = snap {
            *BOOT_SNAP.lock() = s;
        }
        BOOT_TSF.store(t_snap, Ordering::Relaxed);
        BDF.store(((bus as u32) << 16) | ((dev as u32) << 8) | func as u32, Ordering::Relaxed);
        D11_BASE.store(d11_base as u32, Ordering::Relaxed);
        BAR0.store(bar0, Ordering::Relaxed);
    }
}

fn movers(a: &[u32], b: &[u32]) -> (u32, u32) {
    let (mut n, mut up) = (0u32, 0u32);
    for (x, y) in a.iter().zip(b.iter()) {
        if x != y {
            n += 1;
            if y.wrapping_sub(*x) < 0x8000_0000 {
                up += 1;
            }
        }
    }
    (n, up)
}

/// `tests wifi`: the 5 s SHM watch. Returns `(ucode_rev, movers_5s)`; `None` = not this boot.
pub(in crate::wifi) fn watch() -> (Option<u32>, Option<u32>) {
    let rev = match REV.load(Ordering::Relaxed) {
        0 => None,
        r => Some(r),
    };
    let bar0 = BAR0.load(Ordering::Relaxed);
    if bar0 == 0 {
        serial_println!("[wifi] rx-watch REFUSED reason=no-executing-ucode-this-boot — nothing read, nothing written");
        return (rev, None);
    }
    // The window must still be on the d11 core: R8 restored cfg:0x80 to firmware's pre-image, which
    // is the d11 on this machine (f13–f27) but is not guaranteed — a moved window is another core.
    let bdf = BDF.load(Ordering::Relaxed);
    let (bus, dev, func) = ((bdf >> 16) as u8, (bdf >> 8) as u8, bdf as u8);
    let win = unsafe { read_config_32(bus, dev, func, CFG_BAR0_WIN) };
    let base = D11_BASE.load(Ordering::Relaxed);
    if win != base {
        serial_println!(
            "[wifi] rx-watch REFUSED reason=window-not-on-d11 cfg80={:#010x} d11={:#010x} — BAR0+0 is another core's register file; nothing read, nothing written",
            win, base
        );
        return (rev, None);
    }
    let ctl_pre = unsafe { r32(bar0, D11_SHM_CONTROL) };
    let macctl = unsafe { r32(bar0, D11_MACCTL) };
    let t0 = unsafe { tsf(bar0) };
    let a = unsafe { snapshot(bar0) };
    // TSF-timed; a TSC cap (6 s) so a stopped TSF cannot hang the shell. Inside a task the wait
    // sleeps (the CPU is not held); outside one `sleep_ms` is a no-op and the loop spins.
    let hz = crate::bootpace::origin_hz();
    let cap = if hz >= 1000 { hz * 6 } else { 0 };
    let c0 = crate::arch::now_cycles();
    let mut t1 = t0;
    while t1.wrapping_sub(t0) < WATCH_US && crate::arch::now_cycles().wrapping_sub(c0) < cap {
        crate::arch::sched::sleep_ms(20);
        t1 = unsafe { tsf(bar0) };
    }
    let b = unsafe { snapshot(bar0) };
    let restored = unsafe { restore_ctl(bar0, ctl_pre) };
    let (n, up) = movers(&a, &b);
    let dt = t1.wrapping_sub(t0);
    serial_println!(
        "[wifi] rx-watch window={}ms tsf-delta={}us full={} mac-enabled={} psm-run={} movers={} up={} selects=3 shm-ctl-restored={} — no [SPEC-V3]/[SPEC-V4] page in the tree pins an SHM RX-frame counter, so no count is decoded: the counter is the mover that tracks the air (antenna live vs shielded)",
        WATCH_US / 1000, dt, (dt >= WATCH_US) as u8, (macctl & MACCTL_ENABLED) as u8,
        ((macctl & MACCTL_PSM_RUN) != 0) as u8, n, up,
        if restored { "MATCH" } else { "FAILED" }
    );
    let mut rows = 0;
    for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
        if x != y && rows < MAX_ROWS {
            serial_println!("[wifi] rx-watch mover shm+{:#06x} t0={:#010x} t5={:#010x} d={}", i * 4, x, y, y.wrapping_sub(*x) as i32);
            rows += 1;
        }
    }
    let boot = BOOT_SNAP.lock();
    if boot.len() == b.len() {
        let (bn, bup) = movers(&boot, &b);
        serial_println!(
            "[wifi] rx-watch since-boot tsf-delta={}us movers={} up={}",
            t1.wrapping_sub(BOOT_TSF.load(Ordering::Relaxed)), bn, bup
        );
    }
    (rev, Some(n))
}
