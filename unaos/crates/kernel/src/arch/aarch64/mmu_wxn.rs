//! CHARTER: Kernel — kernel-by-ruling (B685 WXN64: the aarch64 EL1&0 W^X map — the kernel image's RX carve, SCTLR_EL1.WXN, `tests wxaudit`)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// WXN64 (rmbp-ledger B685) — aarch64 W^X, enforced. The twin of the x86 WXAUDIT/M3b split.
//
// WHAT WAS THERE. Every aarch64 EL1&0 identity map (`boot_virt::build_l1`, `mmu_tegra::fill_table` for
// `L1_EL1`, the Pi's `boot::build_l1`) maps RAM as ONE attribute set: AP[2:1]=0b00 (EL1 read-write),
// PXN=0, UXN=0 — so the kernel's own `.text` and its heap/stack/BSS share "writable AND executable", and
// `SCTLR_EL1.WXN` could not be set (it would make the RAM the kernel executes from PXN and the next fetch
// would abort — `docs/SECURITY.md` "aarch64 W^X map audit", M6c).
//
// THE CARVE (`carve`). Called once per boot on the EL1 root, AFTER the root's RAM/Device/carveout entries
// are written and BEFORE that root becomes live (virt: `boot_virt::build_l1`, at EL2, regime dormant;
// tegra: `mmu_tegra::init`, after `install_carveout_holes`, on `L1_EL1` — or on `L1` itself at the EL1
// fallback, before `enable_el1` switches to it). So no live translation is edited and no break-before-
// make or TLB maintenance is owed; the tables are cleaned to PoC like their callers clean theirs.
//   1. The image's executable span comes from its OWN program headers through `__ehdr_start` (the x86
//      M3b mechanism, `arch/x86_64/memory.rs` `wxn_image_bounds`: the UEFI loader copies the first
//      PT_LOAD — ELF header and phdrs included — so the headers are in RAM). [lo, hi) = the page-rounded
//      union of the PF_X PT_LOADs. Refused (nothing written, WXN stays off, the audit says why) if the
//      header is not ELF64, no PF_X segment exists, any PF_W segment's pages touch [lo, hi), or this very
//      function is not inside [lo, hi) / a static of this module is.
//   2. Each 1 GiB RAM block the span touches is demoted to an L2 of 2 MiB blocks with the SAME
//      attributes (`KIMG_L2`); a GiB the caller already split (tegra's carveout `L2_POOL_EL1`) is used as
//      is. Each 2 MiB block the span touches is demoted to an L3 of 4 KiB pages (`KIMG_L3`): pages inside
//      [lo, hi) become AP[2:1]=0b10 (EL1 read-only, no EL0), PXN=0, UXN=1 — the only EL1-executable
//      memory in the system; every other page keeps the block's attributes.
//   3. `harden` walks the whole root once: every valid leaf writable at any EL (AP[2]=0) gains PXN|UXN;
//      every read-only leaf EL0 cannot access (AP[1]=0) gains UXN (an AP=0b00/0b10 leaf with UXN=0 is
//      EL0-execute-only memory — the EL0 maps copy this root). Leaves EL0 may execute (the user code
//      pages, AP=0b11 UXN=0) are untouched; the EL0 data pages already carry UXN|PXN.
//   4. `ARMED` latches. From then on `sctlr_wxn()` adds SCTLR_EL1.WXN (bit 19) to every EL1 SCTLR write
//      (`boot_virt::enable_el1_regime`, `boot_tegra::enable_el1_regime` — the BSP and the EL1-seat APs —
//      and `mmu_tegra::enable_el1`), and `ram_xn()` makes every LATER EL1 RAM leaf `mmu_tegra` writes
//      (`map_fb_region`, `repair_fb_l2`) execute-never too. So "the page tables are final" when WXN is
//      set: every leaf is already W^X by its own bits; WXN is the hardware's second statement of it.
//
// THE AUDIT (`wxaudit_fixture`, `tests wxaudit`). Walks the live boot root and every user slot root
// (`uslots`), folds APTable/PXNTable/UXNTable down the path, and counts pages that are writable at any EL
// AND executable at any EL — at DESCRIPTOR level, not trusting WXN to cover them. The verdict is
// `:: WXAUDIT: wxn=<0|1> wx_pages=<n> -> PASS|FAIL ::`; PASS needs wxn=1, wx_pages=0, the auditor's own
// code page EL1-RX and a static of this module RW-XN, and the classifier's negative control. At EL2 (the
// GICv2 `virt` run, which never drops) the EL1&0 regime is not the running one: SKIP, with that reason.
//
// THE PI (`baremetal`). `kernel8.img` is a raw objcopy (no ELF header in RAM) and `pi-baremetal.ld` has
// a SECTIONS layout (LLD synthesises no `_etext` for it), so step 1 reads the script's own bounds instead:
// `[_start, __rx_end)`, where `__rx_end` is the page-aligned end of `.text` + `.rodata` the script places
// before `.data`. Refused if `__rx_end` is unaligned or a `.data` probe static lies below it. The carve
// runs at the end of `boot::build_l1` (MMU off, BSP, before any core walks the table) and
// `boot::enable_mmu` adds WXN on EVERY core — the secondaries read `ARMED` with their MMU off, from DRAM,
// where the BSP's MMU-off store already sits.

use core::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};

const PAGE: u64 = 0x1000;
const ADDR: u64 = 0x0000_FFFF_FFFF_F000; // next-level table / output address, bits [47:12]
const TYPE: u64 = 0b11;
const BLOCK: u64 = 0b01; // L1/L2 block
const TABLE: u64 = 0b11; // L1/L2 table, and the L3 PAGE encoding
const AP_RO: u64 = 1 << 7; // AP[2]: read-only
const AP_EL0: u64 = 1 << 6; // AP[1]: EL0 access
const CONTIG: u64 = 1 << 52;
const PXN: u64 = 1 << 53;
const UXN: u64 = 1 << 54;
const T_PXN: u64 = 1 << 59; // PXNTable
const T_UXN: u64 = 1 << 60; // UXNTable
const T_AP0: u64 = 1 << 61; // APTable[0]: no EL0 access below
const T_AP1: u64 = 1 << 62; // APTable[1]: no write below
const SCTLR_WXN: u64 = 1 << 19;

/// Fresh L2s for 1 GiB RAM blocks the image span touches (an image straddles at most one GiB boundary).
const L2_N: usize = 2;
/// Fresh L3s, one per 2 MiB block the image span touches: 32 MiB of executable image, ample.
const L3_N: usize = 16;

#[repr(C, align(4096))]
struct Tbl([u64; 512]);
static mut KIMG_L2: [Tbl; L2_N] = [const { Tbl([0; 512]) }; L2_N];
static mut KIMG_L3: [Tbl; L3_N] = [const { Tbl([0; 512]) }; L3_N];

static ARMED: AtomicBool = AtomicBool::new(false);
/// Carve outcome, an index into `CARVE_NAMES`.
static CARVE: AtomicU8 = AtomicU8::new(0);
static RX_LO: AtomicU64 = AtomicU64::new(0);
static RX_HI: AtomicU64 = AtomicU64::new(0);
const CARVE_NAMES: [&str; 7] = ["none", "ok", "no-ehdr", "no-x", "w-overlap", "self", "layout"];

/// The carve completed on the EL1 root: every leaf is W^X by its own bits.
#[inline]
pub fn armed() -> bool {
    ARMED.load(Ordering::Relaxed)
}
/// `SCTLR_EL1.WXN` when the carve completed, else 0 — OR'd into every EL1 SCTLR write.
#[inline]
pub fn sctlr_wxn() -> u64 {
    if armed() { SCTLR_WXN } else { 0 }
}
/// PXN|UXN when the carve completed, else 0 — OR'd into every EL1 RAM leaf written after it.
#[inline]
pub fn ram_xn() -> u64 {
    if armed() { PXN | UXN } else { 0 }
}

#[cfg(not(feature = "baremetal"))]
unsafe extern "C" {
    // LLD synthesises `__ehdr_start` at the loaded image base for a link that references it (the x86
    // kernel's `memory.rs` is the precedent on the same loader). Strong extern: a link without it fails
    // at build time rather than carving nothing at run time.
    static __ehdr_start: u8;
}

#[cfg(not(feature = "baremetal"))]
#[inline]
fn rd16(a: u64) -> u16 {
    unsafe { core::ptr::read_unaligned(a as *const u16) }
}
#[cfg(not(feature = "baremetal"))]
#[inline]
fn rd32(a: u64) -> u32 {
    unsafe { core::ptr::read_unaligned(a as *const u32) }
}
#[cfg(not(feature = "baremetal"))]
#[inline]
fn rd64(a: u64) -> u64 {
    unsafe { core::ptr::read_unaligned(a as *const u64) }
}

#[cfg(not(feature = "baremetal"))]
/// The runtime page-rounded `[lo, hi)` of the image's PF_X PT_LOADs, or the `CARVE_NAMES` index of why not.
fn image_rx() -> Result<(u64, u64), u8> {
    const PT_LOAD: u32 = 1;
    const PF_X: u32 = 1;
    const PF_W: u32 = 2;
    let eh = &raw const __ehdr_start as u64;
    // "\x7fELF", ELFCLASS64, e_phentsize 56 — what makes the phdr reads below safe to perform at all.
    if rd32(eh) != 0x464C_457F || unsafe { *(eh as *const u8).add(4) } != 2 || rd16(eh + 54) != 56 {
        return Err(2);
    }
    let phoff = rd64(eh + 32);
    let phnum = (rd16(eh + 56) as u64).min(64);
    let ph = |i: u64| eh + phoff + i * 56;
    // The link-time vaddr of the header: the PT_LOAD at file offset 0 (it carries the header).
    let mut base_v = None;
    for i in 0..phnum {
        if rd32(ph(i)) == PT_LOAD && rd64(ph(i) + 8) == 0 {
            base_v = Some(rd64(ph(i) + 16));
            break;
        }
    }
    let Some(bv) = base_v else { return Err(2) };
    let rt = |v: u64| eh.wrapping_add(v.wrapping_sub(bv));
    let (mut lo, mut hi) = (u64::MAX, 0u64);
    for i in 0..phnum {
        let (ty, fl, va, mem) = (rd32(ph(i)), rd32(ph(i) + 4), rd64(ph(i) + 16), rd64(ph(i) + 40));
        if ty == PT_LOAD && fl & PF_X != 0 && mem != 0 {
            lo = lo.min(rt(va) & !(PAGE - 1));
            hi = hi.max((rt(va + mem) + PAGE - 1) & !(PAGE - 1));
        }
    }
    if hi <= lo {
        return Err(3);
    }
    for i in 0..phnum {
        let (ty, fl, va, mem) = (rd32(ph(i)), rd32(ph(i) + 4), rd64(ph(i) + 16), rd64(ph(i) + 40));
        if ty == PT_LOAD && fl & PF_W != 0 && mem != 0 {
            let (wl, wh) = (rt(va) & !(PAGE - 1), (rt(va + mem) + PAGE - 1) & !(PAGE - 1));
            if wl < hi && lo < wh {
                return Err(4);
            }
        }
    }
    let me = image_rx as *const () as usize as u64;
    let data = &raw const RX_LO as u64;
    if !(lo <= me && me < hi) || (lo <= data && data < hi) {
        return Err(5);
    }
    Ok((lo, hi))
}

#[cfg(feature = "baremetal")]
unsafe extern "C" {
    // `pi-baremetal.ld`: `_start` is the first byte of `.text.boot` at the load address, `__rx_end` the
    // page-aligned end of `.text` + `.rodata` (everything from `.data` up is writable).
    static _start: u8;
    static __rx_end: u8;
}

/// A `.data` static (non-zero initialiser, interior mutability): the carve refuses if the linker ever
/// places writable data below `__rx_end`.
#[cfg(feature = "baremetal")]
static DATA_PROBE: AtomicU64 = AtomicU64::new(0x5758_4E36_3400_0001);

/// The Pi's executable span `[_start, __rx_end)` — `kernel8.img` is a raw objcopy with no ELF header in
/// RAM, so the bounds are the linker script's — or the `CARVE_NAMES` index of why not.
#[cfg(feature = "baremetal")]
fn image_rx() -> Result<(u64, u64), u8> {
    let lo = (&raw const _start as u64) & !(PAGE - 1);
    let hi = &raw const __rx_end as u64;
    if hi & (PAGE - 1) != 0 || hi <= lo {
        return Err(3);
    }
    let data = &raw const DATA_PROBE as u64;
    if data < hi && DATA_PROBE.load(Ordering::Relaxed) != 0 {
        return Err(4);
    }
    let me = image_rx as *const () as usize as u64;
    let bss = &raw const RX_LO as u64;
    if !(lo <= me && me < hi) || (lo <= bss && bss < hi) {
        return Err(5);
    }
    Ok((lo, hi))
}

/// Clean one 4 KiB table to the Point of Coherency (`dc cvac` per line), the callers' own discipline.
unsafe fn clean_page(base: u64) {
    let mut off = 0u64;
    while off < PAGE {
        unsafe { core::arch::asm!("dc cvac, {}", in(reg) base + off, options(nostack, preserves_flags)) };
        off += 64;
    }
}

/// Step 3 of the carve: make every leaf below `table` W^X by its own bits (see the module header).
unsafe fn harden(table: u64, level: u32) {
    let t = table as *mut u64;
    for i in 0..512 {
        let d = unsafe { t.add(i).read_volatile() };
        if d & 1 == 0 {
            continue;
        }
        if level < 3 && d & TYPE == TABLE {
            if d & ADDR != 0 {
                unsafe { harden(d & ADDR, level + 1) };
            }
            continue;
        }
        if level == 3 && d & TYPE != TABLE {
            continue; // 0b01 at L3 is reserved — not a leaf
        }
        let ap = (d >> 6) & 0b11;
        let nd = if ap & 0b10 == 0 { d | PXN | UXN } else if ap & 0b01 == 0 { d | UXN } else { d };
        if nd != d {
            unsafe { t.add(i).write_volatile(nd) };
        }
    }
}

/// Carve the kernel image's executable span out of the EL1&0 root `l1` and make every other leaf
/// execute-never (module header, steps 1-4). Runs on a root that is NOT live; silent (tegra calls it
/// before UARTC is mapped) and allocation-free. `true` = armed; `false` = nothing was written.
///
/// # Safety
/// `l1` is the identity-mapped PA of a 512-entry EL1&0 L1 table (4 KiB granule, T0SZ=25) that no core
/// is translating through, whose table descriptors point at identity-mapped tables.
pub unsafe fn carve(l1: *mut u64) -> bool {
    let r = unsafe { carve_inner(l1) };
    match r {
        Ok((lo, hi)) => {
            RX_LO.store(lo, Ordering::Relaxed);
            RX_HI.store(hi, Ordering::Relaxed);
            CARVE.store(1, Ordering::Relaxed);
            ARMED.store(true, Ordering::Release);
            true
        }
        Err(why) => {
            CARVE.store(why, Ordering::Relaxed);
            false
        }
    }
}

unsafe fn carve_inner(l1: *mut u64) -> Result<(u64, u64), u8> {
    let (lo, hi) = image_rx()?;
    let (c_lo, c_hi) = (lo >> 21, (hi - 1) >> 21); // the 2 MiB blocks the span touches
    if c_hi - c_lo + 1 > L3_N as u64 {
        return Err(6);
    }
    // Pre-check every entry before writing any: a refusal leaves the root exactly as the caller built it.
    let mut fresh = [u64::MAX; L2_N];
    let mut nf = 0usize;
    for c in c_lo..=c_hi {
        let g = c >> 9;
        if g >= 512 {
            return Err(6);
        }
        let d1 = unsafe { l1.add(g as usize).read_volatile() };
        match d1 & TYPE {
            BLOCK => {
                if !fresh[..nf].contains(&g) {
                    if nf == L2_N {
                        return Err(6);
                    }
                    fresh[nf] = g;
                    nf += 1;
                }
            }
            TABLE => {
                let d2 = unsafe { ((d1 & ADDR) as *const u64).add((c & 511) as usize).read_volatile() };
                if d2 & TYPE != BLOCK {
                    return Err(6); // punched, or already split: not a plain 2 MiB block to demote
                }
            }
            _ => return Err(6),
        }
    }
    unsafe {
        // GiB blocks -> L2 of 2 MiB blocks, same attributes.
        for (k, &g) in fresh[..nf].iter().enumerate() {
            let attrs = l1.add(g as usize).read_volatile() & !ADDR & !TYPE & !CONTIG;
            let l2 = (&raw mut KIMG_L2 as *mut Tbl).add(k) as *mut u64;
            for i in 0..512u64 {
                l2.add(i as usize).write_volatile((g << 30) | (i << 21) | attrs | BLOCK);
            }
            l1.add(g as usize).write_volatile((l2 as u64 & ADDR) | TABLE);
        }
        // 2 MiB blocks -> L3 of 4 KiB pages; [lo, hi) EL1-RO + EL1-executable, never EL0-executable.
        for (j, c) in (c_lo..=c_hi).enumerate() {
            let l2 = (l1.add((c >> 9) as usize).read_volatile() & ADDR) as *mut u64;
            let i2 = (c & 511) as usize;
            let attrs = l2.add(i2).read_volatile() & !ADDR & !TYPE & !CONTIG;
            let l3 = (&raw mut KIMG_L3 as *mut Tbl).add(j) as *mut u64;
            for p in 0..512u64 {
                let pa = (c << 21) | (p << 12);
                let d = if pa >= lo && pa < hi {
                    (attrs & !(0b11 << 6) & !PXN) | AP_RO | UXN | pa | TABLE
                } else {
                    attrs | pa | TABLE
                };
                l3.add(p as usize).write_volatile(d);
            }
            l2.add(i2).write_volatile((l3 as u64 & ADDR) | TABLE);
        }
        harden(l1 as u64, 1);
        clean_page(l1 as u64);
        for k in 0..nf {
            clean_page((&raw const KIMG_L2 as *const Tbl).add(k) as u64);
        }
        for j in 0..=(c_hi - c_lo) as usize {
            clean_page((&raw const KIMG_L3 as *const Tbl).add(j) as u64);
        }
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }
    Ok((lo, hi))
}

// ── The audit ────────────────────────────────────────────────────────────────────────────────────────

/// `(writable at any EL, executable at EL1, executable at EL0)` of a leaf under the folded table
/// attributes `fold`, from its own bits — WXN deliberately NOT applied, so the census is about the map.
/// EL0-writable (AP=0b01) forces PXN architecturally (DDI 0487, stage 1 instruction permissions).
fn classify(leaf: u64, fold: u64) -> (bool, bool, bool) {
    let mut ap = (leaf >> 6) & 0b11;
    if fold & T_AP1 != 0 {
        ap |= 0b10;
    }
    if fold & T_AP0 != 0 {
        ap &= !0b01;
    }
    let w = ap & 0b10 == 0;
    let x1 = leaf & PXN == 0 && fold & T_PXN == 0 && ap != 0b01;
    let x0 = leaf & UXN == 0 && fold & T_UXN == 0;
    (w, x1, x0)
}

/// The negative control: the classifier fires on W+X at either EL, clears RO-X, honours the forced PXN
/// of an EL0-writable leaf and the table-level PXN/AP folds. An audit that only ever prints 0 proves
/// nothing (the x86 WXAUDIT-0 lesson).
fn classifier_ok() -> bool {
    let rw = BLOCK; // AP=0b00
    classify(rw, 0) == (true, true, true)
        && classify(rw | PXN | UXN, 0) == (true, false, false)
        && classify(rw | AP_RO | UXN, 0) == (false, true, false)
        && classify(rw | AP_EL0 | UXN, 0) == (true, false, false)
        && classify(rw | AP_EL0, 0) == (true, false, true)
        && classify(rw | UXN, T_PXN) == (true, false, false)
        && classify(rw | UXN, T_AP1) == (false, true, false)
}

#[derive(Default)]
struct Tally {
    leaves: u64,
    pages: u64,
    wx: u64,
    x1: u64,
    x0: u64,
}

unsafe fn walk(table: u64, level: u32, fold: u64, t: &mut Tally) {
    let p = table as *const u64;
    for i in 0..512 {
        let d = unsafe { p.add(i).read_volatile() };
        if d & 1 == 0 {
            continue;
        }
        if level < 3 && d & TYPE == TABLE {
            if d & ADDR != 0 {
                unsafe { walk(d & ADDR, level + 1, fold | (d & (T_PXN | T_UXN | T_AP0 | T_AP1)), t) };
            }
            continue;
        }
        if level == 3 && d & TYPE != TABLE {
            continue;
        }
        let pages = 1u64 << (9 * (3 - level));
        let (w, x1, x0) = classify(d, fold);
        t.leaves += 1;
        t.pages += pages;
        if w && (x1 || x0) {
            t.wx += pages;
        }
        if x1 {
            t.x1 += pages;
        }
        if x0 {
            t.x0 += pages;
        }
    }
}

/// The leaf mapping `va` under `root` and the table attributes folded down to it, or `None`.
fn translate(root: u64, va: u64) -> Option<(u64, u64)> {
    let mut table = root;
    let mut fold = 0u64;
    for level in 1..=3u32 {
        let idx = (va >> (39 - 9 * level)) & 511;
        let d = unsafe { (table as *const u64).add(idx as usize).read_volatile() };
        if d & 1 == 0 {
            return None;
        }
        if level < 3 && d & TYPE == TABLE {
            fold |= d & (T_PXN | T_UXN | T_AP0 | T_AP1);
            table = d & ADDR;
            continue;
        }
        if level == 3 && d & TYPE != TABLE {
            return None;
        }
        return Some((d, fold));
    }
    None
}

#[inline]
fn current_el() -> u64 {
    let v: u64;
    unsafe { core::arch::asm!("mrs {}, CurrentEL", out(reg) v, options(nomem, nostack, preserves_flags)) };
    (v >> 2) & 0b11
}

/// `tests wxaudit` — the aarch64 W^X census of the live EL1&0 maps (module header, THE AUDIT). Called at
/// boot from `mmu_tegra_el0::install` (the EL0 window is the last boot-time map) in the QUIETBOOT shape,
/// and registered by `tests.rs` for every aarch64 build (the Pi has no `install` call site).
pub fn wxaudit_fixture() {
    if crate::tests::defer("wxaudit", wxaudit_fixture) {
        return;
    }
    let el = current_el();
    if el != 1 {
        serial_println!(":: WXAUDIT: wxn=- wx_pages=- -> SKIP reason=el{}-regime ::", el);
        return;
    }
    let (sctlr, ttbr0): (u64, u64);
    unsafe {
        core::arch::asm!("mrs {}, SCTLR_EL1", out(reg) sctlr, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mrs {}, TTBR0_EL1", out(reg) ttbr0, options(nomem, nostack, preserves_flags));
    }
    let wxn = (sctlr & SCTLR_WXN != 0) as u8;
    #[allow(unused_mut)]
    let mut boot = ttbr0 & ADDR;
    #[cfg(feature = "aarch64_el0")]
    {
        let b = crate::arch::uslots::boot_ttbr0() & ADDR;
        if b != 0 {
            boot = b;
        }
    }
    let mut t = Tally::default();
    let mut roots = 1u32;
    unsafe { walk(boot, 1, 0, &mut t) };
    #[cfg(feature = "aarch64_el0")]
    for s in 0..crate::arch::uslots::user_slots() {
        if !crate::arch::uslots::slot_known(s) {
            continue;
        }
        let r = crate::arch::uslots::slot_ttbr0(s) & ADDR;
        if r != 0 && r != boot {
            unsafe { walk(r, 1, 0, &mut t) };
            roots += 1;
        }
    }
    let code = wxaudit_fixture as *const () as usize as u64;
    let data = &raw const CARVE as u64;
    let text_rx = translate(boot, code).is_some_and(|(d, f)| {
        let (w, x1, _) = classify(d, f);
        x1 && !w
    });
    let data_xn = translate(boot, data).is_some_and(|(d, f)| classify(d, f) == (true, false, false));
    let kclass = classifier_ok();
    let carve = CARVE_NAMES[(CARVE.load(Ordering::Relaxed) as usize).min(CARVE_NAMES.len() - 1)];
    let yn = |b: bool| if b { "y" } else { "n" };
    serial_println!(
        "[wxaudit] arm64 el=1 sctlr={:#x} roots={} leaves={} pages={} el1x_pages={} el0x_pages={} rx=[{:#x},{:#x}) carve={} text_rx={} data_xn={} classifier={}",
        sctlr,
        roots,
        t.leaves,
        t.pages,
        t.x1,
        t.x0,
        RX_LO.load(Ordering::Relaxed),
        RX_HI.load(Ordering::Relaxed),
        carve,
        yn(text_rx),
        yn(data_xn),
        if kclass { "ok" } else { "FAIL" },
    );
    let pass = wxn == 1 && t.wx == 0 && text_rx && data_xn && kclass;
    serial_println!(":: WXAUDIT: wxn={} wx_pages={} -> {} ::", wxn, t.wx, if pass { "PASS" } else { "FAIL" });
}

/// Register `tests wxaudit` once (every aarch64 build; the Pi's boot path has no `install` call site).
pub fn ensure() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, Ordering::AcqRel) {
        crate::tests::register("wxaudit", wxaudit_fixture);
    }
}
