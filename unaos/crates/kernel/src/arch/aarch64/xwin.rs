// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//! CHARTER: Kernel — driver (the aarch64 EL0 address space: the args page, the ELF window, SYS_SBRK)
//!
//! RING3ABI2 M2/M5 (rmbp-ledger B333) — the aarch64 twin of RING3WIN's ELF window and of the x86 args
//! page, in ONE module both slot backends (`boot.rs` on the Pi, `mmu_tegra_el0.rs` on the Orin and
//! QEMU virt) share through the `uslots` facade.
//!
//! # Where (the RING3WIN reasoning, applied to aarch64)
//! x86 placed its ELF window at `USER_BASE + 2 MiB`: a FIXED VA, so an image links at its run address
//! with no relocation; clear of the classic window and the FB hole; wired from static per-slot tables
//! with frames from the kernel heap. The aarch64 classic window has no fixed VA on the Pi (it is the
//! identity PA of a 2 MiB-aligned `.bss` anchor, and the rest of that 2 MiB block is live kernel `.bss`),
//! so the window cannot sit beside it. It lives in the slot's own **extension GiB** instead:
//! `L1[EXT_L1]` of every slot's TTBR0 table, VA `una_abi::USER_EXT_BASE_ARM` (481 GiB) — above every
//! identity window any aarch64 board maps and below the 512 GiB ceiling of the 39-bit VA. Inside it the
//! x86 offsets are reused verbatim:
//!
//! ```text
//!   ext + 0x1FF000           the args page (RO, never executable)        XL3[s][0] leaf 511
//!   ext + 0x200000 .. +64 MiB the ELF window (image, heap, guard, stack)   heap L3s, L2[1..=32]
//! ```
//!
//! One static L2 and one static L3 (the args page) per slot (`.bss`). WINDOW2 (B361): the window is
//! 64 MiB = 32 L3s, so those are heap frames wired on first touch of their 2 MiB and freed by `slot_free`
//! (static they would cost 8 x 32 x 4 KiB = 1 MiB of `.bss` on a Pi whose `.bss` has a hard ceiling at the
//! hand-placed 32 MiB heap). Window frames come from the kernel heap
//! (4 KiB-aligned `alloc_zeroed`, identity-mapped, the frames `linuxabi` and x86's XWIN use) and go back
//! at the slot's LAST teardown (`slot_free`, called by both backends' `teardown_user_slot` after the
//! ASID flush). The args page frame is static per slot.
//!
//! # TLB
//! Leaves are written while the slot is not yet live (loader) or by its own task (`SYS_SBRK`): an
//! invalid->valid change needs only `dsb ishst; isb` (an invalid entry is never cached); a valid->invalid
//! change (shrink, a loader unwind) is followed by `tlbi vae1is` for that VA under the slot's ASID. The
//! descriptors are the backends' own shapes (nG, AF, Inner-Shareable, Normal), restated here bit for
//! bit — both backends define them identically.

use alloc::alloc::{alloc_zeroed, dealloc, Layout};
use core::sync::atomic::{AtomicU64, Ordering};


/// The L1 index of the extension GiB (481).
pub const EXT_L1: usize = (una_abi::USER_EXT_BASE_ARM >> 30) as usize;
const EXT_BASE: u64 = una_abi::USER_EXT_BASE_ARM;
const ARGS_OFF: usize = una_abi::USER_ARGS_OFF as usize;
/// The ELF window's offset from the extension base and its size (una-abi's numbers — x86's too).
pub const XWIN_OFF: usize = una_abi::USER_XWIN_OFF as usize;
pub const XWIN_BYTES: usize = una_abi::USER_WINDOW_BYTES as usize;
const XWIN_PTS: usize = XWIN_BYTES / (512 * 4096);
/// The largest program image a launcher may hand the loader: the window, bounded by a quarter of the
/// kernel heap (WINDOW2: the launcher reads the whole file into the heap and the window's frames come from
/// it too, so on aarch64's 48 MiB heap the 64 MiB window's file cap is 12 MiB — the VA span stays 64 MiB).
pub const IMAGE_CAP: usize = if XWIN_BYTES < crate::allocator::HEAP_SIZE / 4 { XWIN_BYTES } else { crate::allocator::HEAP_SIZE / 4 };
const _: () = assert!(EXT_L1 < 512 && EXT_L1 != 480); // 480 is the Orin's classic window
const _: () = assert!(ARGS_OFF / (512 * 4096) == 0 && XWIN_OFF % (512 * 4096) == 0);
const _: () = assert!(XWIN_OFF / (512 * 4096) + XWIN_PTS < 512);

// Descriptor bits (identical in `boot.rs` and `mmu_tegra_el0.rs`).
const DESC_TABLE: u64 = 0b11;
const DESC_PAGE: u64 = 0b11;
const DESC_AF: u64 = 1 << 10;
const SH_INNER: u64 = 0b11 << 8;
const ATTR_NORMAL: u64 = 0;
const AP_EL0: u64 = 1 << 6;
const AP_RO_ALL: u64 = 0b11 << 6;
const DESC_PXN: u64 = 1 << 53;
const DESC_UXN: u64 = 1 << 54;
const DESC_NG: u64 = 1 << 11;
const ADDR: u64 = 0x0000_FFFF_FFFF_F000;

const fn page_desc(pa: u64, w: bool, x: bool) -> u64 {
    let base = (pa & ADDR) | DESC_NG | DESC_PXN | DESC_AF | SH_INNER | ATTR_NORMAL | DESC_PAGE;
    if x {
        base | AP_RO_ALL // EL0-executable, read-only at both ELs
    } else if w {
        base | DESC_UXN | AP_EL0 // EL0+EL1 RW, never executable
    } else {
        base | DESC_UXN | AP_RO_ALL // read-only data
    }
}
const fn table_desc(pa: u64) -> u64 {
    (pa & ADDR) | DESC_TABLE
}

#[repr(C, align(4096))]
struct Table([u64; 512]);
#[repr(C, align(4096))]
struct Page([u8; 4096]);

// WINDOWCAP3 (B399, R90): a slot's extension L2, its L3 (ext+0 .. ext+2 MiB, the args page — the ELF window's
// L3s are heap frames, WINDOW2) and its args page are ONE heap record (identity-mapped, so its address is
// its PA), taken at the slot's first claim (`slot_tables_warm`) and recycled by index.
#[repr(C, align(4096))]
struct XRec {
    l2: Table,
    l3: Table,
    args: Page,
}
static XREC: crate::procslot::SlotVec<core::sync::atomic::AtomicPtr<XRec>> = crate::procslot::SlotVec::new(
    || core::sync::atomic::AtomicPtr::new(core::ptr::null_mut()),
    core::sync::atomic::AtomicPtr::new(core::ptr::null_mut()),
);
/// WINDOW2: window L3 frames (heap) wired across every slot right now.
static L3_LIVE: AtomicU64 = AtomicU64::new(0);

static PAGES: crate::procslot::SlotVec<AtomicU64> = crate::procslot::SlotVec::new(|| AtomicU64::new(0), AtomicU64::new(0));
static LIVE: AtomicU64 = AtomicU64::new(0);
static BRK: crate::procslot::SlotVec<AtomicU64> = crate::procslot::SlotVec::new(|| AtomicU64::new(0), AtomicU64::new(0));
static BRK_LO: crate::procslot::SlotVec<AtomicU64> = crate::procslot::SlotVec::new(|| AtomicU64::new(0), AtomicU64::new(0));
static BRK_MAX: crate::procslot::SlotVec<AtomicU64> = crate::procslot::SlotVec::new(|| AtomicU64::new(0), AtomicU64::new(0));
static PLACED: crate::procslot::SlotVec<AtomicU64> = crate::procslot::SlotVec::new(|| AtomicU64::new(0), AtomicU64::new(0)); // 1 when slot `s`'s extension GiB is installed in its L1.
static SBRK_LOCK: spin::Mutex<()> = spin::Mutex::new(());

fn xrec(s: usize) -> Option<*mut XRec> {
    let p = XREC.peek(s)?.load(Ordering::Acquire);
    if p.is_null() { None } else { Some(p) }
}
/// WINDOWCAP3: slot `s` has its extension record (the range check that was `s < USER_SLOTS`).
fn known(s: usize) -> bool {
    s < crate::procslot::SLOT_ID_MAX && xrec(s).is_some()
}
fn xl2(s: usize) -> *mut u64 {
    xrec(s).map_or(core::ptr::null_mut(), |p| unsafe { (&raw mut (*p).l2).cast::<u64>() })
}
/// The args-page L3 of slot `s`.
fn xl3(s: usize) -> *mut u64 {
    xrec(s).map_or(core::ptr::null_mut(), |p| unsafe { (&raw mut (*p).l3).cast::<u64>() })
}
/// WINDOW2: the window L3 behind L2 index `i` of slot `s` (a heap frame, identity-addressed), or null.
fn wl3(s: usize, i: usize) -> *mut u64 {
    let e = unsafe { xl2(s).add(i).read_volatile() };
    if e & 1 == 0 { core::ptr::null_mut() } else { (e & ADDR) as *mut u64 }
}
/// WINDOW2: window L3 frames held (all slots).
pub fn l3_live() -> u64 {
    L3_LIVE.load(Ordering::Acquire)
}
fn args_ptr(s: usize) -> *mut u8 {
    xrec(s).map_or(core::ptr::null_mut(), |p| unsafe { (&raw mut (*p).args).cast::<u8>() })
}
/// Slot `s`'s own L1 table (the TTBR0 base, ASID bits masked off).
fn slot_l1(s: usize) -> *mut u64 {
    (super::uslots::slot_ttbr0(s) & ADDR) as *mut u64
}
fn asid_of(s: usize) -> u64 {
    super::uslots::slot_ttbr0(s) >> 48
}
fn publish() {
    unsafe { core::arch::asm!("dsb ishst", "isb", options(nostack, preserves_flags)) };
}
fn flush_va(s: usize, va: u64) {
    unsafe {
        core::arch::asm!(
            "dsb ishst",
            "tlbi vae1is, {op}",
            "dsb ish",
            "isb",
            op = in(reg) ((va >> 12) & 0xFFF_FFFF_FFFF) | (asid_of(s) << 48),
            options(nostack, preserves_flags),
        )
    };
}
/// The slot the CALLER runs in (from `TTBR0_EL1`'s ASID), `None` on the boot/shared root.
fn current_slot() -> Option<usize> {
    let t: u64;
    unsafe { core::arch::asm!("mrs {}, TTBR0_EL1", out(reg) t, options(nomem, nostack, preserves_flags)) };
    let asid = (t >> 48) as usize;
    if asid >= 1 && known(asid - 1) { Some(asid - 1) } else { None }
}

/// RING3ABI2: install slot `s`'s extension GiB (args page + empty ELF window), write the empty args header
/// (argc 0, `window_base` = the classic window base) and arm the heap over the whole window (a fixed-model
/// program's heap, as on x86). Called by the loader for every image it places, before the task exists.
/// `false` (nothing installed, a wire line) if the slot's `L1[EXT_L1]` is already in use.
pub fn slot_placed(s: usize, window_base: u64) -> bool {
    if !known(s) {
        return false;
    }
    let l1 = slot_l1(s);
    let mine = table_desc(xl2(s) as u64);
    let cur = unsafe { l1.add(EXT_L1).read_volatile() };
    if cur != 0 && cur != mine {
        serial_println!(":: RING3ABI2: slot={} L1[{}] already mapped ({:#x}) — extension GiB NOT installed ::", s, EXT_L1, cur);
        return false;
    }
    unsafe {
        slot_free(s); // a recycled slot holds nothing; this is the defensive reset
        core::ptr::write_bytes(xl2(s), 0, 512);
        core::ptr::write_bytes(xl3(s), 0, 512);
        xl3(s).add(ARGS_OFF >> 12).write_volatile(page_desc(args_ptr(s) as u64, false, false));
        xl2(s).add(0).write_volatile(table_desc(xl3(s) as u64));
        publish();
        l1.add(EXT_L1).write_volatile(mine);
    }
    publish();
    PLACED[s].store(1, Ordering::Release);
    WINDOW_BASE[s].store(window_base, Ordering::Release);
    let _ = args_write(s, &[]);
    set_heap(s, XWIN_OFF as u64, (XWIN_OFF + XWIN_BYTES) as u64);
    true
}
static WINDOW_BASE: crate::procslot::SlotVec<AtomicU64> = crate::procslot::SlotVec::new(|| AtomicU64::new(0), AtomicU64::new(0));

/// RING3ABI2 M2: lay `words` out in slot `s`'s args page (`una_abi::args_build`). `false` when they do not
/// fit (the page keeps argc 0) or the slot was not placed.
pub fn args_write(s: usize, words: &[&str]) -> bool {
    if !known(s) || PLACED[s].load(Ordering::Acquire) == 0 {
        return false;
    }
    let page = unsafe { core::slice::from_raw_parts_mut(args_ptr(s), 4096) };
    let base = WINDOW_BASE[s].load(Ordering::Acquire);
    if una_abi::args_build(una_abi::USER_ARGS_VA_ARM, base, words, page).is_some() {
        return true;
    }
    let _ = una_abi::args_build(una_abi::USER_ARGS_VA_ARM, base, &[], page);
    false
}

/// RING3ABI2 M2: does `argv` fit one args page? Asked before a slot is claimed.
pub fn argv_fits(argv: &[&str]) -> bool {
    let mut scratch = alloc::vec![0u8; una_abi::USER_ARGS_BYTES];
    una_abi::args_build(una_abi::USER_ARGS_VA_ARM, 0, argv, &mut scratch).is_some()
}

/// The argc slot `s`'s args page carries.
pub fn args_argc(s: usize) -> usize {
    if !known(s) {
        return 0;
    }
    let page = unsafe { core::slice::from_raw_parts(args_ptr(s), 4096) };
    una_abi::Args::parse(page, una_abi::USER_ARGS_VA_ARM).map(|a| a.argc()).unwrap_or(0)
}

fn set_heap(s: usize, lo: u64, max: u64) {
    BRK_LO[s].store(lo, Ordering::Release);
    BRK_MAX[s].store(max, Ordering::Release);
    BRK[s].store(lo, Ordering::Release);
}

/// The leaf for ext offset `off` (inside the ELF window), wiring its L2 entry to a fresh zeroed heap L3 on
/// first use (WINDOW2). Null = the heap has no frame for the table.
unsafe fn leaf(s: usize, off: usize) -> *mut u64 {
    let i = off / (512 * 4096); // L2 index (1..=XWIN_PTS for the window)
    unsafe {
        let l2e = xl2(s).add(i);
        if l2e.read_volatile() == 0 {
            let t = alloc_zeroed(Layout::from_size_align_unchecked(4096, 4096));
            if t.is_null() {
                return core::ptr::null_mut();
            }
            L3_LIVE.fetch_add(1, Ordering::AcqRel);
            publish(); // the zeroed table is visible to the walker before the entry that names it
            l2e.write_volatile(table_desc(t as u64));
        }
        wl3(s, i).add((off >> 12) & 0x1FF)
    }
}
fn in_window(off: usize) -> bool {
    off >= XWIN_OFF && off < XWIN_OFF + XWIN_BYTES
}

/// Map (or re-permission) the page at ext offset `off` of slot `s`; a shared page takes the UNION of the
/// rights, refused if that union is W+X. `Err(())` = out of heap frames or W+X.
unsafe fn map_page(s: usize, off: usize, w: bool, x: bool) -> Result<(), ()> {
    if !in_window(off) || off % 4096 != 0 || PLACED[s].load(Ordering::Acquire) == 0 {
        return Err(());
    }
    unsafe {
        let l = leaf(s, off);
        if l.is_null() {
            return Err(()); // WINDOW2: no heap frame for the L3
        }
        let old = l.read_volatile();
        let (w, x, pa) = if old & 1 != 0 {
            let old_x = old & DESC_UXN == 0;
            let old_w = old & AP_RO_ALL == AP_EL0;
            (w || old_w, x || old_x, old & ADDR)
        } else {
            let p = alloc_zeroed(Layout::from_size_align_unchecked(4096, 4096));
            if p.is_null() {
                return Err(());
            }
            PAGES[s].fetch_add(1, Ordering::AcqRel);
            LIVE.fetch_add(1, Ordering::AcqRel);
            (w, x, p as u64)
        };
        if w && x {
            if old & 1 == 0 {
                l.write_volatile(page_desc(pa, false, false)); // keep it accounted; freed at teardown
            }
            return Err(());
        }
        l.write_volatile(page_desc(pa, w, x));
    }
    Ok(())
}

fn frame_ptr(s: usize, off: usize) -> Option<*mut u8> {
    if !in_window(off) {
        return None;
    }
    let t = wl3(s, off / (512 * 4096));
    if t.is_null() {
        return None;
    }
    let l = unsafe { t.add((off >> 12) & 0x1FF).read_volatile() };
    if l & 1 == 0 { None } else { Some((l & ADDR) as *mut u8) }
}

fn copy_in(s: usize, off: usize, src: &[u8]) -> bool {
    let mut done = 0usize;
    while done < src.len() {
        let o = off + done;
        let Some(f) = frame_ptr(s, o & !0xFFF) else { return false };
        let n = (4096 - (o & 0xFFF)).min(src.len() - done);
        unsafe { core::ptr::copy_nonoverlapping(src.as_ptr().add(done), f.add(o & 0xFFF), n) };
        done += n;
    }
    true
}

unsafe fn unmap_page(s: usize, off: usize, live: bool) {
    let t = wl3(s, off / (512 * 4096));
    if t.is_null() {
        return;
    }
    let l = unsafe { t.add((off >> 12) & 0x1FF) };
    let e = unsafe { l.read_volatile() };
    if e & 1 == 0 {
        return;
    }
    unsafe { l.write_volatile(0) };
    if live {
        flush_va(s, EXT_BASE + off as u64);
    }
    unsafe { dealloc((e & ADDR) as *mut u8, Layout::from_size_align_unchecked(4096, 4096)) };
    PAGES[s].fetch_sub(1, Ordering::AcqRel);
    LIVE.fetch_sub(1, Ordering::AcqRel);
}

/// RING3ABI2 M5: return every window frame slot `s` holds and uninstall its extension GiB. Called by both
/// backends' `teardown_user_slot` on the FINAL release, after the ASID flush (no core can walk the root),
/// and by `slot_placed` as a reset. Idempotent; a slot never placed holds nothing.
pub unsafe fn slot_free(s: usize) {
    if !known(s) {
        return;
    }
    for i in XWIN_OFF / (512 * 4096)..XWIN_OFF / (512 * 4096) + XWIN_PTS {
        if unsafe { xl2(s).add(i).read_volatile() } == 0 {
            continue;
        }
        for l in 0..512 {
            unsafe { unmap_page(s, i * 512 * 4096 + l * 4096, false) };
        }
        let t = wl3(s, i);
        unsafe { xl2(s).add(i).write_volatile(0) };
        publish();
        unsafe { dealloc(t as *mut u8, Layout::from_size_align_unchecked(4096, 4096)) }; // WINDOW2: the L3 is a heap frame
        L3_LIVE.fetch_sub(1, Ordering::AcqRel);
    }
    if PLACED[s].swap(0, Ordering::AcqRel) != 0 {
        let l1 = slot_l1(s);
        if unsafe { l1.add(EXT_L1).read_volatile() } == table_desc(xl2(s) as u64) {
            unsafe { l1.add(EXT_L1).write_volatile(0) };
        }
    }
    publish();
    set_heap(s, 0, 0);
}

/// Frames held in ELF windows across every slot.
pub fn live_pages() -> u64 {
    LIVE.load(Ordering::Acquire)
}

/// RING3ABI2 M5: `SYS_SBRK(delta)` on aarch64 — the x86 semantics: the OLD break VA, or `-ENOMEM` (past the
/// cap / the guard / out of frames) / `-EINVAL` (shrink below the start). Growth maps zeroed RW pages.
pub fn sys_sbrk(delta: i64) -> i64 {
    let Some(s) = current_slot() else { return una_abi::ENOMEM };
    let _g = SBRK_LOCK.lock();
    let (lo, max, cur) = (BRK_LO[s].load(Ordering::Acquire), BRK_MAX[s].load(Ordering::Acquire), BRK[s].load(Ordering::Acquire));
    if max == 0 {
        return una_abi::ENOMEM;
    }
    let new = (cur as i64).wrapping_add(delta);
    if delta < 0 && new < lo as i64 {
        return una_abi::EINVAL;
    }
    if delta > 0 && (new < cur as i64 || new as u64 > max) {
        serial_println!(":: RING3ABI2: sbrk refused slot={} brk={:#x} delta={} cap={:#x} -ENOMEM ::", s, EXT_BASE + cur, delta, EXT_BASE + max);
        return una_abi::ENOMEM;
    }
    let new = new as u64;
    let up = |v: u64| (v + 0xFFF) & !0xFFF;
    if new > cur {
        let mut p = up(cur);
        while p < up(new) {
            if unsafe { map_page(s, p as usize, true, false) }.is_err() {
                let mut q = up(cur);
                while q < p {
                    unsafe { unmap_page(s, q as usize, true) };
                    q += 4096;
                }
                return una_abi::ENOMEM;
            }
            p += 4096;
        }
        publish();
    } else if new < cur {
        let mut p = up(new);
        while p < up(cur) {
            unsafe { unmap_page(s, p as usize, true) };
            p += 4096;
        }
    }
    BRK[s].store(new, Ordering::Release);
    (EXT_BASE + cur) as i64
}

/// RING3ABI2 M5: is `[va, va+len)` a legal syscall buffer in the CALLER's extension GiB? Every page must be
/// a live leaf (the guard page and an ungrown heap are not); `write` additionally needs EL0-RW. The args
/// page is a legal READ source and never a write target. (The aarch64 copy seam has no live-leaf walk for
/// the classic window — that window is fully mapped — so the walk lives here, for the pages that may not
/// be.)
pub fn range_ok(va: u64, len: usize, write: bool) -> bool {
    let Some(s) = current_slot() else { return false };
    if PLACED[s].load(Ordering::Acquire) == 0 || len == 0 {
        return false;
    }
    let Some(end) = va.checked_add(len as u64) else { return false };
    let args_lo = una_abi::USER_ARGS_VA_ARM;
    if va >= args_lo && end <= args_lo + 4096 {
        return !write;
    }
    let lo = EXT_BASE + XWIN_OFF as u64;
    if va < lo || end > lo + XWIN_BYTES as u64 {
        return false;
    }
    let mut p = va & !0xFFF;
    while p < end {
        let off = (p - EXT_BASE) as usize;
        let t = wl3(s, off / (512 * 4096));
        if t.is_null() {
            return false;
        }
        let e = unsafe { t.add((off >> 12) & 0x1FF).read_volatile() };
        if e & 1 == 0 || (write && e & AP_RO_ALL != AP_EL0) {
            return false;
        }
        p += 4096;
    }
    true
}

// ── The elf model (the aarch64 twin of x86 `elf.rs::validate_elf_model` / `map_elf_model`) ─────────────
// OWED (flagged since WINX-2): one `crate::elf` validator for x86, aarch64 and linuxabi. This is the third
// copy of the PT_LOAD walk; it accepts exactly what the x86 elf model accepts, with EM_AARCH64.

const PT_LOAD: u32 = 1;
const PT_GNU_STACK: u32 = 0x6474_E551;
const PF_X: u32 = 1;
const PF_W: u32 = 2;
const EM_AARCH64: u16 = 183;
const MAX_SEGS: usize = 8;

#[derive(Clone, Copy)]
struct Seg {
    off: usize,
    vaddr: u64, // ext offset after validation
    filesz: usize,
    memsz: usize,
    flags: u32,
}

fn rd(b: &[u8], o: usize, n: usize) -> Option<u64> {
    let s = b.get(o..o.checked_add(n)?)?;
    let mut v = 0u64;
    for (i, &c) in s.iter().enumerate() {
        v |= (c as u64) << (8 * i);
    }
    Some(v)
}

/// The lowest PT_LOAD p_vaddr, or `None` for a malformed header.
fn min_vaddr(b: &[u8]) -> Option<u64> {
    let (phoff, phent, phnum) = (rd(b, 32, 8)? as usize, rd(b, 54, 2)? as usize, rd(b, 56, 2)? as usize);
    let mut lo = u64::MAX;
    for i in 0..phnum {
        let ph = phoff.checked_add(i.checked_mul(phent)?)?;
        if rd(b, ph, 4)? as u32 == PT_LOAD {
            lo = lo.min(rd(b, ph + 16, 8)?);
        }
    }
    if lo == u64::MAX { None } else { Some(lo) }
}

/// RING3ABI2 M5: place an elf-model aarch64 image. `None` = the image is not elf-model (its lowest PT_LOAD
/// is below `una_abi::USER_XWIN_VA_ARM`): the caller takes the classic path. `Some(Ok((slot, entry, sp,
/// nsegs)))` = placed in a fresh slot (args page argc 0 — the launcher writes the words); `Some(Err(why))` =
/// refused, nothing leaked.
pub fn place_elf_model(b: &[u8]) -> Option<Result<(usize, u64, u64, u32), &'static str>> {
    if b.len() < 64 || b[0..4] != [0x7F, b'E', b'L', b'F'] {
        return None;
    }
    if min_vaddr(b)? < una_abi::USER_XWIN_VA_ARM {
        return None;
    }
    Some(place(b))
}

fn place(b: &[u8]) -> Result<(usize, u64, u64, u32), &'static str> {
    if b[4] != 2 || b[5] != 1 || rd(b, 16, 2) != Some(2) {
        return Err("not a little-endian ELF64 ET_EXEC");
    }
    if rd(b, 18, 2) != Some(EM_AARCH64 as u64) {
        return Err("not EM_AARCH64");
    }
    let entry = rd(b, 24, 8).ok_or("bad e_entry")?;
    let (phoff, phent, phnum) = (
        rd(b, 32, 8).ok_or("bad e_phoff")? as usize,
        rd(b, 54, 2).ok_or("bad e_phentsize")? as usize,
        rd(b, 56, 2).ok_or("bad e_phnum")? as usize,
    );
    if phent != 56 || phnum == 0 {
        return Err("bad program-header table");
    }
    let mut segs = [Seg { off: 0, vaddr: 0, filesz: 0, memsz: 0, flags: 0 }; MAX_SEGS];
    let (mut n, mut stack_req) = (0usize, 0usize);
    for i in 0..phnum {
        let ph = phoff.checked_add(i * 56).ok_or("phdr overflow")?;
        let t = rd(b, ph, 4).ok_or("program-header table out of image")? as u32;
        if t == PT_GNU_STACK {
            stack_req = rd(b, ph + 40, 8).ok_or("bad p_memsz")? as usize;
            continue;
        }
        if t != PT_LOAD {
            continue;
        }
        if n >= MAX_SEGS {
            return Err("too many PT_LOAD segments");
        }
        let flags = rd(b, ph + 4, 4).ok_or("bad p_flags")? as u32;
        let off = rd(b, ph + 8, 8).ok_or("bad p_offset")? as usize;
        let va = rd(b, ph + 16, 8).ok_or("bad p_vaddr")?;
        let filesz = rd(b, ph + 32, 8).ok_or("bad p_filesz")? as usize;
        let memsz = rd(b, ph + 40, 8).ok_or("bad p_memsz")? as usize;
        if filesz > memsz || off.checked_add(filesz).map_or(true, |e| e > b.len()) {
            return Err("segment file range out of image");
        }
        if flags & PF_W != 0 && flags & PF_X != 0 {
            return Err("W^X: segment both writable and executable");
        }
        let vaddr = va.checked_sub(EXT_BASE).ok_or("segment below the extension GiB")?;
        segs[n] = Seg { off, vaddr, filesz, memsz, flags };
        n += 1;
    }
    let entry = entry.checked_sub(EXT_BASE).ok_or("entry below the window")?;
    let top = (XWIN_OFF + XWIN_BYTES) as u64;
    let stack = if stack_req == 0 { una_abi::USER_STACK_DEFAULT as usize } else { (stack_req + 0xFFF) & !0xFFF };
    if stack > una_abi::USER_STACK_MAX as usize {
        serial_println!(":: RING3ABI2: refused stack={} max={} -ENOMEM ::", stack, una_abi::USER_STACK_MAX);
        return Err("declared stack exceeds the 1 MiB cap (-ENOMEM)");
    }
    let limit = top - stack as u64 - 4096;
    let (mut max_end, mut min_va, mut entry_ok) = (0u64, u64::MAX, false);
    for sg in &segs[..n] {
        let end = sg.vaddr.checked_add(sg.memsz as u64).ok_or("segment span overflow")?;
        max_end = max_end.max(end);
        min_va = min_va.min(sg.vaddr);
        if sg.flags & PF_X != 0 && entry >= sg.vaddr && entry < end {
            entry_ok = true;
        }
    }
    if min_va < XWIN_OFF as u64 || max_end > limit {
        serial_println!(":: RING3ABI2: refused image span={} stack={} cap={} -ENOMEM ::", max_end.saturating_sub(min_va), stack, XWIN_BYTES);
        return Err("image + stack exceed the ELF window (USER_WINDOW_BYTES, -ENOMEM)");
    }
    if !entry_ok {
        return Err("entry not in an executable segment");
    }
    // STORMFAULT (B351): the shared segment plan — the same `elf_core::plan` the x86 loader runs: overlapping
    // segments, a segment or stack over the args page, a bss into the stack guard, each refused by NAME.
    {
        let mut v = [elf_core::Seg::default(); MAX_SEGS];
        for (d, sg) in v.iter_mut().zip(&segs[..n]) {
            *d = elf_core::Seg { vaddr: sg.vaddr, filesz: sg.filesz as u64, memsz: sg.memsz as u64, flags: sg.flags };
        }
        let w = elf_core::Window {
            lo: XWIN_OFF as u64,
            hi: top,
            args: elf_core::Range { lo: una_abi::USER_ARGS_OFF, hi: una_abi::USER_ARGS_OFF + una_abi::USER_ARGS_BYTES as u64 },
            elf: true,
            refuse_overlap: true,
            stack_default: una_abi::USER_STACK_DEFAULT,
            stack_max: una_abi::USER_STACK_MAX,
        };
        if let Err(e) = elf_core::plan(&v[..n], stack_req as u64, &w) {
            serial_println!(":: STORMFAULT: refused model=elf arch=aarch64 reason={} ::", e.word());
            return Err(e.as_str());
        }
    }
    // Validated: claim the slot LAST, and unwind through the ordinary teardown on any failure.
    let s = super::uslots::alloc_user_slot().ok_or("no free address-space slot")?;
    let fail = |why: &'static str| -> Result<(usize, u64, u64, u32), &'static str> {
        serial_println!(":: RING3ABI2: refused slot={} reason={} ::", s, why);
        unsafe { super::uslots::teardown_user_slot(asid_of(s)) };
        Err(why)
    };
    if !slot_placed(s, super::uslots::user_region().0) {
        return fail("the extension GiB is in use on this board");
    }
    for sg in &segs[..n] {
        let (w, x) = (sg.flags & PF_W != 0, sg.flags & PF_X != 0);
        let mut p = sg.vaddr & !0xFFF;
        while p < (sg.vaddr + sg.memsz as u64 + 0xFFF) & !0xFFF {
            if unsafe { map_page(s, p as usize, w, x) }.is_err() {
                return fail("out of frames, or segments share a page with conflicting W/X");
            }
            p += 4096;
        }
    }
    for sg in &segs[..n] {
        if !copy_in(s, sg.vaddr as usize, &b[sg.off..sg.off + sg.filesz]) {
            return fail("segment copy hit an unmapped page");
        }
        if sg.flags & PF_X != 0 {
            let mut p = sg.vaddr & !0xFFF;
            while p < sg.vaddr + sg.memsz as u64 {
                if let Some(f) = frame_ptr(s, p as usize) {
                    super::cache::icache_sync_range(f as usize, 4096);
                }
                p += 4096;
            }
        }
    }
    let stack_lo = top - stack as u64;
    let mut p = stack_lo;
    while p < top {
        if unsafe { map_page(s, p as usize, true, false) }.is_err() {
            return fail("out of frames mapping the stack");
        }
        p += 4096;
    }
    publish();
    let heap_lo = (max_end + 0xFFF) & !0xFFF;
    set_heap(s, heap_lo, stack_lo - 4096);
    serial_println!(
        ":: RING3ABI2: model=elf arch=aarch64 slot={} segs={} span={} stack={} heap=[{:#x},{:#x}) frames={} ::",
        s, n, max_end - min_va, stack, EXT_BASE + heap_lo, EXT_BASE + stack_lo - 4096, live_pages()
    );
    Ok((s, EXT_BASE + entry, (EXT_BASE + top) & !0xF, n as u32))
}

// ── `tests ring3abi` probes ───────────────────────────────────────────────────────────────────────────

/// M5 probe: claim a slot, place its extension GiB, map one window page, write and read it back through
/// the frame, free it, release the slot; the live-frame count must return. `None` = no slot free.
pub fn selftest_probe() -> Option<bool> {
    let s = super::uslots::alloc_user_slot()?;
    let live0 = live_pages();
    let l3_0 = l3_live();
    let placed = slot_placed(s, super::uslots::user_region().0);
    let off = XWIN_OFF + XWIN_BYTES / 2;
    let mapped = placed && unsafe { map_page(s, off, true, false) }.is_ok();
    let rw = mapped && copy_in(s, off, b"RING3ABI2") && frame_ptr(s, off).is_some_and(|f| unsafe { core::slice::from_raw_parts(f, 9) } == b"RING3ABI2");
    unsafe { super::uslots::teardown_user_slot(asid_of(s)) };
    let freed = live_pages() == live0 && l3_live() == l3_0; // WINDOW2: the heap L3 goes back too
    serial_println!("[ring3abi] arm ext_l1={} placed={} mapped={} rw={} freed={} l3_live={}", EXT_L1, placed, mapped, rw, freed, l3_live());
    Some(placed && mapped && rw && freed)
}

/// M2 probe: the launchers' args writer into a placed slot, read back as ring 3 reads it.
pub fn args_probe(words: &[&str]) -> Option<usize> {
    let s = super::uslots::alloc_user_slot()?;
    let ok = slot_placed(s, super::uslots::user_region().0) && args_write(s, words);
    let n = if ok { args_argc(s) } else { 0 };
    unsafe { super::uslots::teardown_user_slot(asid_of(s)) };
    Some(n)
}

/// WINDOWCAP3 (rmbp-ledger B399): give slot `s` its extension record and every per-slot row of this file at
/// its claim (process context — both backends' `alloc_user_slot`). `false` = the heap said no.
pub fn slot_tables_warm(s: usize) -> bool {
    if xrec(s).is_none() {
        // SAFETY: non-zero size, power-of-two alignment.
        let p = unsafe { alloc_zeroed(Layout::new::<XRec>()) } as *mut XRec;
        if p.is_null() {
            return false;
        }
        XREC[s].store(p, Ordering::Release); // only the claimant of `s` reaches here
    }
    for t in [&PAGES, &BRK, &BRK_LO, &BRK_MAX, &PLACED, &WINDOW_BASE] {
        t.warm(s);
    }
    true
}
