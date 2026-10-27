// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Amber Bytes (Disks) — seam: driver. The NVMe host controller (`UNAOS_NVME=1`, default OFF).
//!
//! # SCOPE — ZENBOOKNVME (zenbook-ledger Z2), rung 1
//!
//! **Does:** find the first NVMe controller by PCI class (0x01 mass storage / 0x08 NVM / progif 0x02),
//! claim it (memory decode + bus master), map BAR0 uncacheable, read `CAP`/`VS`, reset it (`CC.EN=0`,
//! wait `CSTS.RDY=0`), program ONE admin queue pair, enable it (wait `CSTS.RDY=1`, both waits bounded by
//! `CAP.TO`), IDENTIFY the controller (model, MDTS) and namespace 1 (size, LBA format), create ONE I/O
//! queue pair (qid 1, depth 32), and publish the namespace into `drivers::block`'s NVMe registry
//! (`block::register_nvme`) — the free-function registry shape AHCI's first rung (rmbp-ledger B89)
//! established. NVMe 1.4 base specification, cleanroom: no driver code from any other OS was consulted.
//!
//! **Polled, with one completion vector (NVMEMSIX, zenbook-ledger Z45):** a command's completion is polled on the CQ
//! phase bit against a TSC deadline ([`T_CMD_MS`]) — the phase bit is the ONLY authority. The I/O CQ is created with
//! `IEN=1, IV=0` and [`arm_irq`] points MSI-X table entry 0 (else single-message MSI) at a `vectors::alloc`'d vector
//! whose handler only sets a hint and counts (`irqs=` on the test line); the admin queue shares vector 0 and is idle
//! after probe, so it stays polled with no INTMS/INTMC write. NVMEIRQWAIT (zenbook-ledger Z49): an I/O command whose
//! caller came in UNMASKED, on the CPU the vector targets, sleeps between polls ([`cq_wait`]: `sti; hlt`, woken by the
//! vector or the tick); every other wait is the spin. A miss prints `[nvme] cmd timeout opcode=0x<op> ...` and MARKS the controller
//! (DONGLEBOOT's rule, rmbp-ledger B658): every later command is refused at once.
//!
//! **Rung 2 — NVMEBOOT (zenbook-ledger Z9):** the namespace is `BlockHandle::Nvme` / `fs::fat::BlockSource::Nvme`,
//! so the root walk sees it; the boot's NVMe root grant (one UnaFS extent, `block::hold_nvme_root_grant`)
//! admits writes through `block::write_blocks_nvme`; [`flush`] (NVM Flush) is the commit barrier.
//!
//! **Does NOT (owed rows, docs/dev/blueprint/storage.md §5):** a vector per queue (NVME-MSIX-PERQ, which also lets the
//! sleeping wait follow the caller's CPU); namespaces other than 1; TRIM; the
//! SMART/Health log page; an installer path onto the NVMe (NVMEINSTALL). NVME4KN (zenbook-ledger Z31): a 4096-byte-LBA
//! namespace is served through the block layer's 512 B sector view (`block::read_sectors_nvme`, block.rs tail).
//!
//! **The writes:** WRITE (0x01) is compiled, and [`write_at`] is `pub(crate)` with exactly TWO callers —
//! `block::write_blocks_nvme` (-EPERM outside the live root grant, each run read back with ONE `read_at` —
//! NVMEBATCH, zenbook-ledger Z22: a UnaFS block is one run, one WRITE per `max_xfer`) and the
//! `tests nvme` scratch leg ([`selftest`]), which runs it only on QEMU's emulated controller (model
//! string `QEMU NVMe Ctrl`), never inside the live root grant, and restores the scratch blocks it wrote.
//! `grep -rn 'nvme::write_at\|write_at(' unaos/crates/kernel/src/drivers` is the audit.
//!
//! # Lock
//!
//! [`CTRL`] holds the whole controller (rings, heads/tails, phases, the PRP list page, the bounce). It is
//! held ONLY through [`with_ctrl`]: interrupts masked (no preemption inside the hold) and acquired with a
//! bounded `try_lock` — DONGLEBOOT's discipline, minimal half (no render path reaches this driver in
//! rung 1; lifting `ahci::dongle::with_port` into one shared hold is owed row DISKHOLD). One command is
//! outstanding at a time, so one lock is a correctness requirement. The mask is the holder's NON-PREEMPTION (masked
//! callers, e.g. the UnaFS MOUNT hold, spin on `CTRL`: WEDGE-8 F3, block.rs). NVMEIRQWAIT (Z49): a holder that sleeps in
//! [`cq_wait`] has interrupts on for that window; NVMEPREEMPTOFF (Z52): it holds the scheduler's preempt-off count
//! (`arch::sched::preemptoff`), so it is never preempted and no contender runs on its CPU meanwhile.
//!
//! # Wire
//!
//! ```text
//! [nvme] bdf <b>:<s>.<f> <vid>:<did> bar0=0x<..> cap=0x<..> ver=<x.y.z> mqes=<n> dstrd=<n> to_ms=<n>   (bootlog)
//! [nvme] ctrl  (the ready line: mn="<model>" ns=1 lba=<bytes> blocks=<n> -> ready irq=<msix|msi|none> vector=<0x<v>|->; spelled once, at `probe`)
//! [nvme] test identify=ok sector0=<GPT|MBR|none> scratch_lba=<n> blocks=8 rw=<ok|skip|bad:<leg>> batch=<n> rbatch=<n> runs=<n> run_blocks=<n> rblocks=<n> rcmds=<n> lba=<bytes> shift=<9..12> rmw=<n> irqs=<n> waits=<n> wakes=<n> rw_us=<n> deferred=<n>
//! [nvme] irq wait off why=no-wakes waits=<n> — completions back on the spin   (once, NVMEIRQWAIT: a vector that never wakes)
//! :: NVME: ctrl=ok ns=1 rw=ok -> PASS ::
//! ```
//!
//! # Byte identity
//!
//! Declared `#[cfg(all(target_arch = "x86_64", feature = "nvme"))]` in `drivers/mod.rs`: knob off, this
//! file is never lexed, the block-layer hunk is cfg-erased at `block.rs`'s tail, and the `pci.rs` probe
//! call and the `tests.rs` registration are LINE-NEUTRAL folds that vanish.

use crate::sync::Mutex;
use core::sync::atomic::{fence, AtomicBool, AtomicU32, Ordering};

// ── NVMe 1.4 §3.1: controller registers ─────────────────────────────────────────────────────────
const REG_CAP: u64 = 0x00;
const REG_VS: u64 = 0x08;
const REG_CC: u64 = 0x14;
const REG_CSTS: u64 = 0x1C;
const REG_AQA: u64 = 0x24;
const REG_ASQ: u64 = 0x28;
const REG_ACQ: u64 = 0x30;
/// Doorbells start at 0x1000; queue y's SQ tail is at index 2y, its CQ head at 2y+1, each `4 << DSTRD` apart.
const DB_BASE: u64 = 0x1000;

const CC_EN: u32 = 1 << 0;
/// I/O submission queue entry size 2^6 = 64 bytes; completion entry 2^4 = 16 bytes. CSS=0 (NVM), MPS=0
/// (4 KiB pages), AMS=0 (round robin), SHN=0.
const CC_IOSQES: u32 = 6 << 16;
const CC_IOCQES: u32 = 4 << 20;
const CSTS_RDY: u32 = 1 << 0;
const CSTS_CFS: u32 = 1 << 1;

// ── Opcodes ────────────────────────────────────────────────────────────────────────────────────
const ADM_CREATE_IO_SQ: u32 = 0x01;
const ADM_CREATE_IO_CQ: u32 = 0x05;
const ADM_IDENTIFY: u32 = 0x06;
const IO_WRITE: u32 = 0x01;
const IO_READ: u32 = 0x02;

const ADMIN_DEPTH: u16 = 8;
const IO_DEPTH: u16 = 32;
const IO_QID: u16 = 1;
const PAGE: usize = 4096;
/// The data bounce: 8 pages = 32 KiB, `block::MAX_BLOCKS_PER_OP` (64) x 512-byte sectors.
const BOUNCE_PAGES: usize = 8;
const BOUNCE_BYTES: usize = BOUNCE_PAGES * PAGE;
const SQE_BYTES: u64 = 64;
const CQE_BYTES: u64 = 16;

/// A command's completion budget. IDENTIFY and a 32 KiB read are microsecond-scale on a healthy
/// device; a second is a deadline, not an expectation (AHCI's `T_CMD_MS`).
const T_CMD_MS: u64 = 1000;
/// How long a caller may wait for [`CTRL`] before refusing.
const T_HOLD_MS: u64 = T_CMD_MS + 500;
/// QEMU's emulated controller's default model number (`-device nvme`, `mn` unset).
const QEMU_MN: &[u8] = b"QEMU NVMe Ctrl";
/// The scratch leg: 8 blocks ending 64 blocks before the namespace's end (below a GPT backup array).
const SCRATCH_BLOCKS: u64 = 8;
const SCRATCH_FROM_END: u64 = 72;

// ── Bounded waits, in TSC units: the ONE shared wait (BOUNDEDWAIT, rmbp-ledger B981, at the tail of
// `drivers/block.rs`). `arch::ms()` is not usable: the probe runs inside `pci::init` with interrupts masked,
// where the APIC tick does not advance.
use crate::drivers::block::wait::{bounded as wait_ms, cycles_ms};

#[inline]
fn elapsed_ms(start: u64) -> u64 {
    crate::arch::now_cycles().wrapping_sub(start) / cycles_ms(1).max(1)
}

// ── MMIO: BAR0 is identity-mapped Uncacheable, so accesses are strongly ordered on x86.
#[inline]
fn r32(bar: u64, off: u64) -> u32 {
    unsafe { core::ptr::read_volatile((bar + off) as *const u32) }
}
#[inline]
fn w32(bar: u64, off: u64, val: u32) {
    unsafe { core::ptr::write_volatile((bar + off) as *mut u32, val) }
}
#[inline]
fn r64(bar: u64, off: u64) -> u64 {
    (r32(bar, off) as u64) | ((r32(bar, off + 4) as u64) << 32)
}
#[inline]
fn w64(bar: u64, off: u64, val: u64) {
    w32(bar, off, val as u32);
    w32(bar, off + 4, (val >> 32) as u32);
}
#[inline]
fn db_off(index: u16, dstrd: u32) -> u64 {
    DB_BASE + (index as u64) * (4u64 << dstrd)
}

/// Zeroed, aligned, DMA-visible memory; 0 on failure. Heap addresses ARE bus addresses (the identity
/// map, stated once in `arch::memory::dma::ident`, DMAALLOCONE B986). Never freed: the controller
/// owns these rings for as long as `CC.EN` is set, and this arc has no teardown.
fn dma_alloc(size: usize) -> u64 {
    crate::arch::memory::dma::ident(size, PAGE)
}

/// One queue pair: the SQ and CQ rings (one page each), the SQ tail we own, the CQ head we own, and
/// the phase tag the next valid completion will carry.
struct Queue {
    qid: u16,
    sq: u64,
    cq: u64,
    depth: u16,
    tail: u16,
    head: u16,
    phase: bool,
}

impl Queue {
    const fn new(qid: u16, sq: u64, cq: u64, depth: u16) -> Self {
        Queue { qid, sq, cq, depth, tail: 0, head: 0, phase: true }
    }
}

/// The brought-up controller.
struct Ctrl {
    bar0: u64,
    dstrd: u32,
    admin: Queue,
    io: Queue,
    cid: u16,
    prp_list: u64,
    bounce: u64,
    /// Bytes one command may move: min(the bounce, MDTS).
    max_xfer: usize,
    /// log2 of the namespace's LBA size (9 = 512 B .. 12 = 4 KiB; NVME4KN, zenbook-ledger Z31).
    lba_shift: u32,
    nsze: u64,
    mn: [u8; 40],
    /// NVMEIRQWAIT (Z49): the current hold's caller came in UNMASKED (set by [`with_ctrl`]), so an I/O wait may sleep.
    sleep_ok: bool,
}

/// THE controller. Held only through [`with_ctrl`].
static CTRL: Mutex<Option<Ctrl>> = Mutex::new(None);
/// A command missed its bound this boot: every later command is refused at once.
static DEAD: AtomicBool = AtomicBool::new(false);
static DEAD_SAID: AtomicBool = AtomicBool::new(false);
/// Acquires of [`CTRL`] that gave up at the bound (the first few print).
static BUSY_REFUSED: AtomicU32 = AtomicU32::new(0);

/// Submit one command on the admin (`admin`) or the I/O queue and poll its completion. Returns DW0.
fn submit(c: &mut Ctrl, admin: bool, cmd: &mut [u32; 16]) -> Result<u32, ()> {
    let opc = cmd[0] & 0xFF;
    if DEAD.load(Ordering::Acquire) {
        if !DEAD_SAID.swap(true, Ordering::AcqRel) {
            serial_println!("[nvme] ctrl marked — opcode={:#04x} refused at once (it missed its bound earlier this boot)", opc);
        }
        return Err(());
    }
    let cid = c.cid;
    c.cid = c.cid.wrapping_add(1);
    cmd[0] = (cmd[0] & 0xFF) | ((cid as u32) << 16);
    let bar0 = c.bar0;
    let dstrd = c.dstrd;
    let may_sleep = !admin && c.sleep_ok; // NVMEIRQWAIT (Z49): the admin queue stays polled
    let q = if admin { &mut c.admin } else { &mut c.io };

    let slot = q.sq + (q.tail as u64) * SQE_BYTES;
    for (i, dw) in cmd.iter().enumerate() {
        unsafe { core::ptr::write_volatile((slot + (i as u64) * 4) as *mut u32, *dw) };
    }
    q.tail = (q.tail + 1) % q.depth;
    fence(Ordering::SeqCst);
    w32(bar0, db_off(q.qid * 2, dstrd), q.tail as u32);

    let ent = q.cq + (q.head as u64) * CQE_BYTES;
    let start = crate::arch::now_cycles();
    let budget = cycles_ms(T_CMD_MS);
    loop {
        // NVMEMSIX (Z45): the vector's hint is consumed first, then the phase bit decides — a hint never completes a
        // command (the idle admin CQ shares the vector). Under `with_ctrl`'s masked hold the message latches in the
        // LAPIC IRR and is delivered at the hold's release, so the hint is seen here only when another CPU took it.
        let _hint = CQ_PENDING.swap(false, Ordering::AcqRel);
        let dw3 = unsafe { core::ptr::read_volatile((ent + 12) as *const u32) };
        if ((dw3 >> 16) & 1 == 1) == q.phase {
            break;
        }
        if crate::arch::now_cycles().wrapping_sub(start) >= budget {
            DEAD.store(true, Ordering::Release);
            serial_println!(
                "[nvme] cmd timeout opcode={:#04x} qid={} cid={} ms={} bound_ms={} csts={:#010x} -> ctrl marked",
                opc, q.qid, cid, elapsed_ms(start), T_CMD_MS, r32(bar0, REG_CSTS)
            );
            return Err(());
        }
        cq_wait(may_sleep); // NVMEIRQWAIT (Z49): sleep on the vector when the caller is unmasked on its CPU, else the spin
    }
    fence(Ordering::SeqCst);
    let dw0 = unsafe { core::ptr::read_volatile(ent as *const u32) };
    let dw3 = unsafe { core::ptr::read_volatile((ent + 12) as *const u32) };
    q.head = (q.head + 1) % q.depth;
    if q.head == 0 {
        q.phase = !q.phase;
    }
    w32(bar0, db_off(q.qid * 2 + 1, dstrd), q.head as u32);

    let sc = (dw3 >> 17) & 0xFF;
    let sct = (dw3 >> 25) & 0x7;
    if sc != 0 || sct != 0 {
        serial_println!("[nvme] cmd error opcode={:#04x} qid={} sct={} sc={:#04x}", opc, q.qid, sct, sc);
        return Err(());
    }
    if (dw3 & 0xFFFF) as u16 != cid {
        serial_println!("[nvme] cmd cid mismatch opcode={:#04x} qid={} want={} got={}", opc, q.qid, cid, dw3 & 0xFFFF);
        return Err(());
    }
    Ok(dw0)
}

/// The ONE way [`CTRL`] is held: interrupts masked, a bounded try, never an unbounded spin.
fn with_ctrl<R>(lba: u64, f: impl FnOnce(&mut Ctrl) -> Result<R, ()>) -> Result<R, ()> {
    // NVMEIRQWAIT (Z49): the caller's own mask, read before the hold masks (an unmasked caller's holds may sleep).
    // NVMEPREEMPTOFF (Z52): a holder asleep in `cq_wait` holds the preempt-off count, so no contender ever runs on its
    // CPU while it holds `CTRL`: the yield and the at-once refusal are retired; the busy line's bounded path stays.
    let open = !crate::arch::irqs_masked();
    // if open { irqwait_yield(); }   (NVMEIRQWAIT Z49; retired by NVMEPREEMPTOFF Z52)
    let at_once = false; // was `!open && sleeper_here()` (NVMEIRQWAIT Z49; retired by NVMEPREEMPTOFF Z52)
    crate::drivers::block::hold::with(
        T_HOLD_MS,
        at_once,
        || CTRL.try_lock(),
        |spun| {
            let n = BUSY_REFUSED.fetch_add(1, Ordering::Relaxed) + 1;
            if n <= 4 {
                serial_println!("[nvme] busy refused lba={} waited_ms={} bound_ms={} n={}", lba, spun / cycles_ms(1).max(1), T_HOLD_MS, n);
            }
            Err(())
        },
        |mut guard| match &mut *guard {
            Some(c) => {
                c.sleep_ok = open;
                f(c)
            }
            None => Err(()),
        },
    )
}

/// One READ/WRITE of `bytes` (whole blocks, <= `max_xfer`) between the namespace and the bounce.
fn io_cmd(c: &mut Ctrl, opc: u32, lba: u64, bytes: usize) -> Result<(), ()> {
    let pages = (bytes + PAGE - 1) / PAGE;
    let prp1 = c.bounce;
    let prp2 = if pages <= 1 {
        0
    } else if pages == 2 {
        c.bounce + PAGE as u64
    } else {
        for i in 1..pages {
            unsafe {
                core::ptr::write_volatile((c.prp_list + ((i - 1) as u64) * 8) as *mut u64, c.bounce + (i * PAGE) as u64)
            };
        }
        c.prp_list
    };
    let nlb = (bytes >> c.lba_shift) as u32;
    if nlb == 0 {
        return Err(());
    }
    let mut cmd = [0u32; 16];
    cmd[0] = opc;
    cmd[1] = 1; // NSID 1
    cmd[6] = prp1 as u32;
    cmd[7] = (prp1 >> 32) as u32;
    cmd[8] = prp2 as u32;
    cmd[9] = (prp2 >> 32) as u32;
    cmd[10] = lba as u32;
    cmd[11] = (lba >> 32) as u32;
    cmd[12] = nlb - 1; // 0-based
    if opc == IO_WRITE {
        IO_WRITE_CMDS.fetch_add(1, Ordering::Relaxed); // NVMEBATCH (Z22): what `tests nvme`'s `batch=` counts
    } else if opc == IO_READ { IO_READ_CMDS.fetch_add(1, Ordering::Relaxed); IO_READ_BLOCKS.fetch_add(nlb as u64, Ordering::Relaxed); } // NVMEBATCHREAD (Z24): what `tests nvme`'s `rbatch=` / `rblocks=` / `rcmds=` count
    submit(c, false, &mut cmd).map(|_| ())
}

/// Move `len` bytes (whole blocks) at `lba`, chunked by `max_xfer`, through the bounce.
fn transfer(lba: u64, ptr: *mut u8, len: usize, write: bool) -> Result<(), ()> {
    with_ctrl(lba, |c| {
        let bs = 1usize << c.lba_shift;
        if len == 0 || len % bs != 0 {
            return Err(());
        }
        let blocks = (len / bs) as u64;
        match lba.checked_add(blocks) {
            Some(end) if end <= c.nsze => {}
            _ => return Err(()),
        }
        let per = (c.max_xfer / bs).max(1) * bs;
        let mut done = 0usize;
        while done < len {
            let n = core::cmp::min(per, len - done);
            let at = lba + (done / bs) as u64;
            if write {
                unsafe { core::ptr::copy_nonoverlapping(ptr.add(done) as *const u8, c.bounce as *mut u8, n) };
                io_cmd(c, IO_WRITE, at, n)?;
            } else {
                io_cmd(c, IO_READ, at, n)?;
                unsafe { core::ptr::copy_nonoverlapping(c.bounce as *const u8, ptr.add(done), n) };
            }
            done += n;
        }
        Ok(())
    })
}

/// Read whole blocks at `lba` into `buf`. What `block::read_blocks_nvme` calls.
pub(crate) fn read_at(lba: u64, buf: &mut [u8]) -> Result<(), ()> {
    transfer(lba, buf.as_mut_ptr(), buf.len(), false)
}

/// Write whole blocks at `lba` from `buf`. TWO callers: `block::write_blocks_nvme` (grant-checked, NVMEBOOT
/// Z9) and [`selftest`]'s scratch leg. Never call it from anywhere else.
pub(crate) fn write_at(lba: u64, buf: &[u8]) -> Result<(), ()> {
    transfer(lba, buf.as_ptr() as *mut u8, buf.len(), true)
}

fn trimmed(s: &[u8]) -> &str {
    let end = s.iter().rposition(|&b| b != b' ' && b != 0).map_or(0, |i| i + 1);
    core::str::from_utf8(&s[..end]).unwrap_or("?")
}

/// Walk PCI for the first function reporting class 0x01 / subclass 0x08 / progif 0x02 (NVM Express).
/// Class-based, never bdf-based (LAWS §3); the same 256x32 multi-function-gated sweep as `ahci`'s.
fn find_controller() -> Option<(u8, u8, u8, u16, u16)> {
    for bus in 0u16..256 {
        for slot in 0u8..32 {
            let v0 = unsafe { crate::arch::pci::read_config_16(bus as u8, slot, 0, 0x00) };
            if v0 == 0xFFFF {
                continue;
            }
            let hdr0 = unsafe { crate::arch::pci::read_config_32(bus as u8, slot, 0, 0x0C) };
            let max_func: u8 = if ((hdr0 >> 16) & 0x80) != 0 { 7 } else { 0 };
            for func in 0..=max_func {
                let vend = unsafe { crate::arch::pci::read_config_16(bus as u8, slot, func, 0x00) };
                if vend == 0xFFFF {
                    continue;
                }
                let class_reg = unsafe { crate::arch::pci::read_config_32(bus as u8, slot, func, 0x08) };
                let class = ((class_reg >> 24) & 0xFF) as u8;
                let sub = ((class_reg >> 16) & 0xFF) as u8;
                let progif = ((class_reg >> 8) & 0xFF) as u8;
                if class == 0x01 && sub == 0x08 && progif == 0x02 {
                    let devid = unsafe { crate::arch::pci::read_config_16(bus as u8, slot, func, 0x02) };
                    return Some((bus as u8, slot, func, vend, devid));
                }
            }
        }
    }
    None
}

/// Claim the function (memory decode + bus master: NVMe has no PIO data path) and decode BAR0, which
/// is a 64-bit memory BAR on every controller (NVMe 1.4 §2.1.10/§2.1.11: MLBAR + MUBAR).
fn take(bus: u8, slot: u8, func: u8, vend: u16, devid: u16) -> Option<u64> {
    let lo = unsafe { crate::arch::pci::read_config_32(bus, slot, func, 0x10) };
    let command = unsafe { crate::arch::pci::read_config_16(bus, slot, func, 0x04) };
    if lo & 0x1 != 0 {
        serial_println!("[nvme] bdf {}:{}.{} {:04x}:{:04x} BAR0 is an I/O BAR — not an NVMe register block, refused", bus, slot, func, vend, devid);
        return None;
    }
    let bar0 = if lo & 0x06 == 0x04 {
        let hi = unsafe { crate::arch::pci::read_config_32(bus, slot, func, 0x14) };
        ((lo & 0xFFFF_F000) as u64) | ((hi as u64) << 32)
    } else {
        (lo & 0xFFFF_F000) as u64
    };
    if bar0 == 0 {
        serial_println!("[nvme] bdf {}:{}.{} {:04x}:{:04x} BAR0 unassigned by firmware — refused", bus, slot, func, vend, devid);
        return None;
    }
    if command & 0x0006 != 0x0006 {
        unsafe { crate::arch::pci::write_config_16(bus, slot, func, 0x04, command | 0x0006) };
        let after = unsafe { crate::arch::pci::read_config_16(bus, slot, func, 0x04) };
        if after & 0x0006 != 0x0006 {
            serial_println!("[nvme] bdf {}:{}.{} decode/bus-master did not stick (cmd={:#06x}) — refused", bus, slot, func, after);
            return None;
        }
    }
    // Registers + the admin doorbells at DSTRD 0; `bring_up` widens the window once CAP names the stride.
    crate::arch::memory::map_mmio_window(bar0, 0x2000);
    Some(bar0)
}

/// Reset, program the admin queue, enable, identify, create the I/O pair. Every refusal names itself.
fn bring_up(bus: u8, slot: u8, func: u8, vend: u16, devid: u16, bar0: u64) -> Option<Ctrl> {
    let cap = r64(bar0, REG_CAP);
    let vs = r32(bar0, REG_VS);
    let mqes = (cap & 0xFFFF) as u32 + 1;
    let to_ms = (((cap >> 24) & 0xFF) * 500).max(500);
    let dstrd = ((cap >> 32) & 0xF) as u32;
    let css_nvm = (cap >> 37) & 1;
    let mpsmin = (cap >> 48) & 0xF;
    crate::bootlog_println!(
        "[nvme] bdf {}:{}.{} {:04x}:{:04x} bar0={:#x} cap={:#018x} ver={}.{}.{} mqes={} dstrd={} to_ms={}",
        bus, slot, func, vend, devid, bar0, cap, vs >> 16, (vs >> 8) & 0xFF, vs & 0xFF, mqes, dstrd, to_ms
    );
    if dstrd != 0 {
        crate::arch::memory::map_mmio_window(bar0, (DB_BASE + 4 * (4u64 << dstrd)) as usize);
    }
    if mpsmin != 0 {
        serial_println!("[nvme] refused mpsmin={} (4 KiB pages only)", mpsmin);
        return None;
    }
    if css_nvm == 0 {
        serial_println!("[nvme] refused cap.css={:#04x} (no NVM command set)", (cap >> 37) & 0xFF);
        return None;
    }
    if mqes < 2 {
        serial_println!("[nvme] refused mqes={}", mqes);
        return None;
    }
    let io_depth = core::cmp::min(IO_DEPTH as u32, mqes) as u16;

    // Reset: the firmware's NVMe driver usually leaves the controller enabled.
    let cc = r32(bar0, REG_CC);
    if cc & CC_EN != 0 {
        w32(bar0, REG_CC, cc & !CC_EN);
    }
    if !wait_ms(to_ms, || r32(bar0, REG_CSTS) & CSTS_RDY == 0) {
        serial_println!("[nvme] disable timeout csts={:#010x} to_ms={}", r32(bar0, REG_CSTS), to_ms);
        return None;
    }

    let asq = dma_alloc(PAGE);
    let acq = dma_alloc(PAGE);
    let iosq = dma_alloc(PAGE);
    let iocq = dma_alloc(PAGE);
    let prp_list = dma_alloc(PAGE);
    let bounce = dma_alloc(BOUNCE_BYTES);
    if asq == 0 || acq == 0 || iosq == 0 || iocq == 0 || prp_list == 0 || bounce == 0 {
        serial_println!("[nvme] DMA allocation failed — controller not brought up");
        return None;
    }

    let aq = (ADMIN_DEPTH - 1) as u32;
    w32(bar0, REG_AQA, (aq << 16) | aq);
    w64(bar0, REG_ASQ, asq);
    w64(bar0, REG_ACQ, acq);
    w32(bar0, REG_CC, CC_IOCQES | CC_IOSQES);
    w32(bar0, REG_CC, CC_IOCQES | CC_IOSQES | CC_EN);
    let ready = wait_ms(to_ms, || r32(bar0, REG_CSTS) & (CSTS_RDY | CSTS_CFS) != 0);
    let csts = r32(bar0, REG_CSTS);
    if !ready || csts & CSTS_CFS != 0 || csts & CSTS_RDY == 0 {
        serial_println!("[nvme] enable timeout csts={:#010x} to_ms={}", csts, to_ms);
        return None;
    }

    let mut c = Ctrl {
        bar0,
        dstrd,
        admin: Queue::new(0, asq, acq, ADMIN_DEPTH),
        io: Queue::new(IO_QID, iosq, iocq, io_depth),
        cid: 0,
        prp_list,
        bounce,
        max_xfer: BOUNCE_BYTES,
        lba_shift: 9,
        nsze: 0,
        mn: [b' '; 40],
        sleep_ok: false,
    };

    // IDENTIFY controller (CNS 1).
    let mut cmd = [0u32; 16];
    cmd[0] = ADM_IDENTIFY;
    cmd[6] = bounce as u32;
    cmd[7] = (bounce >> 32) as u32;
    cmd[10] = 1;
    submit(&mut c, true, &mut cmd).ok()?;
    let id = unsafe { core::slice::from_raw_parts(bounce as *const u8, PAGE) };
    c.mn.copy_from_slice(&id[24..64]);
    let mdts = id[77] as usize;
    c.max_xfer = if mdts == 0 || mdts >= 4 { BOUNCE_BYTES } else { PAGE << mdts };

    // IDENTIFY namespace 1 (CNS 0).
    let mut cmd = [0u32; 16];
    cmd[0] = ADM_IDENTIFY;
    cmd[1] = 1;
    cmd[6] = bounce as u32;
    cmd[7] = (bounce >> 32) as u32;
    submit(&mut c, true, &mut cmd).ok()?;
    let ns = unsafe { core::slice::from_raw_parts(bounce as *const u8, PAGE) };
    let mut nsze_b = [0u8; 8];
    nsze_b.copy_from_slice(&ns[0..8]);
    let nsze = u64::from_le_bytes(nsze_b);
    let flbas = (ns[26] & 0x0F) as usize;
    let lbaf_at = 128 + 4 * flbas;
    let lbaf = u32::from_le_bytes([ns[lbaf_at], ns[lbaf_at + 1], ns[lbaf_at + 2], ns[lbaf_at + 3]]);
    let lbads = (lbaf >> 16) & 0xFF;
    if nsze == 0 {
        serial_println!("[nvme] refused ns=1 nsze=0 (namespace 1 inactive)");
        return None;
    }
    if !(9..=12).contains(&lbads) {
        serial_println!("[nvme] refused ns=1 lbads={} (512..4096-byte blocks only)", lbads);
        return None;
    }
    c.nsze = nsze;
    c.lba_shift = lbads;

    // CREATE I/O CQ (qid 1, physically contiguous, interrupts on at vector 0 — NVMEMSIX, Z45), then CREATE I/O SQ on it.
    let qsz = ((io_depth - 1) as u32) << 16;
    let mut cmd = [0u32; 16];
    cmd[0] = ADM_CREATE_IO_CQ;
    cmd[6] = iocq as u32;
    cmd[7] = (iocq >> 32) as u32;
    cmd[10] = qsz | IO_QID as u32;
    cmd[11] = 0b11; // PC=1, IEN=1, IV=0 (NVMEMSIX, Z45: MSI-X entry 0 / the one MSI message, shared with the idle admin CQ)
    submit(&mut c, true, &mut cmd).ok()?;
    let mut cmd = [0u32; 16];
    cmd[0] = ADM_CREATE_IO_SQ;
    cmd[6] = iosq as u32;
    cmd[7] = (iosq >> 32) as u32;
    cmd[10] = qsz | IO_QID as u32;
    cmd[11] = ((IO_QID as u32) << 16) | 1; // CQID 1, PC=1
    submit(&mut c, true, &mut cmd).ok()?;
    Some(c)
}

/// The one entry point. Called once from `arch::x86_64::pci::init`, behind the `nvme` feature.
pub fn probe() {
    let (bus, slot, func, vend, devid) = match find_controller() {
        Some(t) => t,
        None => {
            crate::bootlog_println!("[nvme] no NVMe controller (class 0x01/0x08 progif 0x02)");
            return;
        }
    };
    let bar0 = match take(bus, slot, func, vend, devid) {
        Some(b) => b,
        None => return,
    };
    let c = match bring_up(bus, slot, func, vend, devid, bar0) {
        Some(c) => c,
        None => {
            serial_println!("[nvme] bdf {}:{}.{} not brought up — no namespace published", bus, slot, func);
            return;
        }
    };
    let (mn, lba, blocks) = (c.mn, 1u32 << c.lba_shift, c.nsze); XFER_SECTORS.store((c.max_xfer / 512) as u32, Ordering::Relaxed); // NVMEBATCHREAD (Z24): the read-ahead window `fs::unafs::ra_window` gives the NVMe, set before `c` moves; NVME4KN (Z31): in 512 B view sectors on EVERY namespace (the one unit)
    let (irq, vector) = arm_irq(bus, slot, func, bar0); // NVMEMSIX (Z45): one completion vector, after every admin command
    if irq != "none" {
        IRQ_APIC.store(crate::arch::apic::apic_id() as u32, Ordering::Release); // NVMEIRQWAIT (Z49): the CPU `arm_irq`'s message names
    }
    crate::arch::without_interrupts(|| *CTRL.lock() = Some(c));
    if crate::drivers::block::register_nvme(blocks, lba, &mn) {
        if irq == "none" {
            serial_println!("[nvme] ctrl mn=\"{}\" ns=1 lba={} blocks={} -> ready irq=none vector=-", trimmed(&mn), lba, blocks);
        } else {
            serial_println!("[nvme] ctrl mn=\"{}\" ns=1 lba={} blocks={} -> ready irq={} vector={:#04x}", trimmed(&mn), lba, blocks, irq, vector);
        }
    }
}

/// `tests nvme` (ZENBOOKNVME M3): identify (the registered namespace), read sector 0 through the block
/// registry, and — on QEMU's controller only — a write + read-back of 8 scratch blocks that are restored.
pub fn selftest() {
    let dev = match crate::drivers::block::nvme_info() {
        Some(d) => d,
        None => {
            serial_println!(":: NVME: ctrl=none ns=0 rw=- -> SKIP reason=no-nvme ::");
            return;
        }
    };
    let bs = dev.block_size as usize;
    let held = with_ctrl(0, |c| Ok(c.mn));
    let mn = held.unwrap_or([b' '; 40]);
    let identify = held.is_ok() && dev.num_blocks > 0 && !trimmed(&mn).is_empty();

    let mut sec = alloc::vec![0u8; bs];
    let sector0 = match crate::drivers::block::read_blocks_nvme(0, &mut sec) {
        Ok(_) => match amber_core::gpt::classify_sector0(&sec[..512]) {
            amber_core::gpt::Sector0::Protective => "GPT",
            amber_core::gpt::Sector0::Mbr => "MBR",
            amber_core::gpt::Sector0::None => "none",
        },
        Err(_) => "unreadable",
    };

    let scratch = dev.num_blocks.saturating_sub(SCRATCH_FROM_END);
    let in_root = crate::drivers::block::nvme_root_grant().is_some_and(|(a, b)| scratch <= b && scratch + SCRATCH_BLOCKS > a); // NVMEBOOT (Z9): the live volume's blocks are never the test's scratch
    let rw: &str = if !mn.starts_with(QEMU_MN) {
        "skip"
    } else if in_root {
        "in-root"
    } else if dev.num_blocks < SCRATCH_FROM_END * 2 {
        "bad:small"
    } else {
        let t0 = crate::arch::now_cycles(); let d0 = crate::arch::sched::preemptoff::deferred_total(); // NVMEPREEMPTOFF (Z52): deferred preempts before the leg
        let r = scratch_rw(scratch, bs);
        RW_US.store(crate::arch::now_cycles().wrapping_sub(t0).saturating_mul(1000) / cycles_ms(1).max(1), Ordering::Relaxed); RW_DEFERRED.store(crate::arch::sched::preemptoff::deferred_total().wrapping_sub(d0), Ordering::Relaxed); // NVMEIRQWAIT (Z49): the rw leg's wall time; NVMEPREEMPTOFF (Z52): the preempts deferred during it
        r
    };
    let (runs, run_blocks) = crate::drivers::block::nvme_run_census(); // NVMEBATCH (Z22)
    serial_println!(
        "[nvme] test identify={} sector0={} scratch_lba={} blocks={} rw={} batch={} rbatch={} runs={} run_blocks={} rblocks={} rcmds={} lba={} shift={} rmw={} irqs={} waits={} wakes={} rw_us={} deferred={}",
        if identify { "ok" } else { "bad" }, sector0, scratch, SCRATCH_BLOCKS, rw, LAST_BATCH.load(Ordering::Relaxed), LAST_RBATCH.load(Ordering::Relaxed), runs, run_blocks, IO_READ_BLOCKS.load(Ordering::Relaxed), IO_READ_CMDS.load(Ordering::Relaxed), dev.block_size, dev.block_size.trailing_zeros(), crate::drivers::block::nvme_rmw_count(), IRQS.load(Ordering::Relaxed), IRQ_WAITS.load(Ordering::Relaxed), IRQ_WAKES.load(Ordering::Relaxed), RW_US.load(Ordering::Relaxed), RW_DEFERRED.load(Ordering::Relaxed) // NVMEPREEMPTOFF (Z52): preempts deferred while a holder slept in the rw leg; NVMEIRQWAIT (Z49): sleeping waits, vector-woken sleeps, the rw leg's microseconds; NVMEMSIX (Z45): `irqs=` the completion interrupts fired this boot; NVME4KN (Z31): the namespace's LBA size, its shift, and the sub-LBA writes served by read-modify-write this boot
    );
    let ctrl = if identify { "ok" } else { "bad" };
    let root = root_leg();
    if !identify || sector0 == "unreadable" {
        serial_println!(":: NVME: ctrl={} ns=1 rw={} -> FAIL reason={} :: root={} ::", ctrl, rw, if identify { "read" } else { "identify" }, root);
    } else if rw.starts_with("bad") {
        serial_println!(":: NVME: ctrl=ok ns=1 rw={} -> FAIL reason=rw :: root={} ::", rw, root);
    } else if root.starts_with("bad") {
        serial_println!(":: NVME: ctrl=ok ns=1 rw={} -> FAIL reason=root :: root={} ::", rw, root);
    } else if rw == "ok" || rw == "in-root" {
        serial_println!(":: NVME: ctrl=ok ns=1 rw={} -> PASS :: root={} ::", rw, root);
    } else {
        serial_println!(":: NVME: ctrl=ok ns=1 rw=skip -> SKIP reason=rw-not-qemu :: root={} ::", root);
    }
}

/// The scratch leg: read the original, write an lba-salted pattern, read it back, restore, verify.
fn scratch_rw(lba: u64, bs: usize) -> &'static str {
    let len = SCRATCH_BLOCKS as usize * bs;
    let mut orig = alloc::vec![0u8; len];
    if crate::drivers::block::read_blocks_nvme(lba, &mut orig).is_err() {
        return "bad:read-orig";
    }
    let mut pat = alloc::vec![0u8; len];
    for (i, b) in pat.iter_mut().enumerate() {
        *b = (i as u64 ^ lba.wrapping_mul(0x9E37_79B9) ^ (i as u64 >> 9)) as u8;
    }
    let expect = match with_ctrl(lba, |c| Ok(len.div_ceil((c.max_xfer / bs).max(1) * bs) as u32)) {
        Ok(n) => n,
        Err(()) => return "bad:busy",
    };
    let w0 = IO_WRITE_CMDS.load(Ordering::Relaxed);
    if write_at(lba, &pat).is_err() {
        return "bad:write";
    }
    let batch = IO_WRITE_CMDS.load(Ordering::Relaxed).wrapping_sub(w0); // NVMEBATCH (Z22): the 8 blocks' WRITE commands
    LAST_BATCH.store(batch, Ordering::Relaxed);
    let mut back = alloc::vec![0u8; len];
    if crate::drivers::block::read_blocks_nvme(lba, &mut back).is_err() {
        return "bad:read-back";
    }
    let same = back == pat;
    if write_at(lba, &orig).is_err() {
        return "bad:restore";
    }
    if crate::drivers::block::read_blocks_nvme(lba, &mut back).is_err() || back != orig {
        return "bad:restore-verify";
    }
    if !same {
        "bad:mismatch"
    } else if batch != expect {
        "bad:batch"
    } else {
        read_legs(lba, &orig, expect) // NVMEBATCHREAD (Z24): the 8-block read is ONE command, on the block layer and through the mount device
    }
}

// ══════════ NVMEBOOT (zenbook-ledger Z9) — the commit barrier and the root leg ══════════

/// NVM Flush (opcode 0x00) on namespace 1, through the one hold — what `block::flush_nvme` forwards, the
/// K8a pre-flip barrier of a UnaFS root on the NVMe.
pub(crate) fn flush() -> Result<(), ()> {
    with_ctrl(0, |c| {
        let mut cmd = [0u32; 16];
        cmd[0] = 0x00; // NVM Flush
        cmd[1] = 1; // NSID 1
        submit(c, false, &mut cmd).map(|_| ())
    })
}

/// `tests nvme`'s root leg: `skip` with no NVMe root grant; else a write of LBA 0's OWN bytes (outside every
/// grant: `hold_nvme_root_grant` refuses a range from 0) must come back -EPERM, and the shared UnaFS mount
/// must ride the NVMe handle.
fn root_leg() -> &'static str {
    let Some((first, _last)) = crate::drivers::block::nvme_root_grant() else { return "skip" };
    if first == 0 {
        return "bad:grant-covers-0";
    }
    let Some(dev) = crate::drivers::block::nvme_info() else { return "bad:no-namespace" };
    let mut sec = alloc::vec![0u8; dev.block_size as usize];
    if crate::drivers::block::read_blocks_nvme(0, &mut sec).is_err() {
        return "bad:read-lba0";
    }
    if !matches!(crate::drivers::block::write_blocks_nvme(0, &sec), Err(crate::drivers::block::BlockError::Denied)) {
        return "bad:lba0-admitted";
    }
    #[cfg(feature = "unafs")]
    {
        if crate::fs::unafs::mount_bound_handle() != Some(crate::drivers::block::BlockHandle::Nvme) {
            return "bad:unbound";
        }
    }
    "ok"
}

// ══════════ NVMEBATCH (zenbook-ledger Z22) — the write-command census `tests nvme` reads ══════════

/// NVMEBATCH: I/O WRITE commands submitted this boot (bumped in [`io_cmd`]).
static IO_WRITE_CMDS: AtomicU32 = AtomicU32::new(0);
/// NVMEBATCH: the WRITE commands the scratch leg's 8-block `write_at` issued (0 = the leg did not run); it must
/// equal `ceil(len / max_xfer)` — 1 on every controller for 8 x 512 B — or the leg answers `bad:batch`.
static LAST_BATCH: AtomicU32 = AtomicU32::new(0);

// ══════════ NVMEBATCHREAD (zenbook-ledger Z24) — the READ census and the rw leg's read checks ══════════

/// NVMEBATCHREAD: I/O READ commands submitted this boot (bumped in [`io_cmd`], the twin of [`IO_WRITE_CMDS`]).
static IO_READ_CMDS: AtomicU32 = AtomicU32::new(0);
/// NVMEBATCHREAD: logical blocks those READ commands moved (`rblocks=`; `rblocks / rcmds` is the batch a read rides).
static IO_READ_BLOCKS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// NVMEBATCHREAD: the READ commands the scratch leg's 8-block `block::read_blocks_nvme` cost (0 = the leg did not
/// run); it must equal the write leg's `expect` (`ceil(len / max_xfer)`, 1 for 8 x 512 B) or the leg answers `bad:rbatch`.
static LAST_RBATCH: AtomicU32 = AtomicU32::new(0);
/// NVMEBATCHREAD: 512 B sectors one command may move (`max_xfer / 512`), set once at [`probe`]; 0 = not probed. NVME4KN
/// (Z31): set on every namespace — the window is in the 512 B view sectors `fs::unafs::SdSectorDevice` addresses.
static XFER_SECTORS: AtomicU32 = AtomicU32::new(0);

/// NVMEBATCHREAD: the read-ahead window `fs::unafs::ra_window` gives the NVMe handle (it clamps to the block layer's
/// per-op cap), so one window fill is one READ command. Lock-free: no controller hold.
pub(crate) fn xfer_sectors() -> u32 {
    XFER_SECTORS.load(Ordering::Relaxed)
}

/// NVMEBATCHREAD: the rw leg's read checks over the restored scratch `orig` at `lba`. (i) `block::read_blocks_nvme` of
/// the 8 blocks costs `expect` READ commands and returns `orig`, else `bad:rbatch`; (ii) with `unafs`, the same blocks
/// through the mount device's read path (`fs::unafs::nvme_read_leg`, a fresh device: one window fill) cost exactly ONE
/// READ and return `orig`, else `bad:rpath`. The commands are counted globally, so each leg keeps the FEWEST of three
/// attempts: a live root's concurrent READ cannot fail it, a per-sector path (8) always does.
fn read_legs(lba: u64, orig: &[u8], expect: u32) -> &'static str {
    let mut back = alloc::vec![0u8; orig.len()];
    let mut rbatch = u32::MAX;
    for _ in 0..3 {
        let r0 = IO_READ_CMDS.load(Ordering::Relaxed);
        if crate::drivers::block::read_blocks_nvme(lba, &mut back).is_err() || back != orig {
            return "bad:rbatch";
        }
        rbatch = rbatch.min(IO_READ_CMDS.load(Ordering::Relaxed).wrapping_sub(r0));
    }
    LAST_RBATCH.store(rbatch, Ordering::Relaxed);
    if rbatch != expect {
        return "bad:rbatch";
    }
    #[cfg(feature = "unafs")]
    {
        let mut fewest = u32::MAX;
        for _ in 0..3 {
            back.iter_mut().for_each(|b| *b = 0);
            let r0 = IO_READ_CMDS.load(Ordering::Relaxed);
            if !crate::fs::unafs::nvme_read_leg(lba, &mut back) || back != orig {
                return "bad:rpath";
            }
            fewest = fewest.min(IO_READ_CMDS.load(Ordering::Relaxed).wrapping_sub(r0));
        }
        if fewest != 1 {
            return "bad:rpath";
        }
    }
    "ok"
}

// ══════════ NVMEMSIX (zenbook-ledger Z45) — one completion vector for the I/O CQ ══════════

/// NVMEMSIX: set by [`nvme_msi_handler`], consumed (swapped false) on each spin of [`submit`]'s poll. A hint only.
static CQ_PENDING: AtomicBool = AtomicBool::new(false);
/// NVMEMSIX: completion interrupts delivered this boot (`irqs=` on the test line; > 0 on QEMU = the vector fires).
static IRQS: AtomicU32 = AtomicU32::new(0);

/// NVMEMSIX: the vector's handler — the xhci1 shape (`drivers/xhci/mod.rs` `xhci1_msi_handler`): lock-free, bounded, no
/// MMIO (MSI/MSI-X are messages: nothing to ack on the device), no `CTRL`, no print; EOI last.
extern "x86-interrupt" fn nvme_msi_handler(_f: x86_64::structures::idt::InterruptStackFrame) {
    CQ_PENDING.store(true, Ordering::Release);
    IRQS.fetch_add(1, Ordering::Relaxed);
    crate::arch::apic::eoi();
}

/// NVMEMSIX: arm the one completion vector, after `bring_up`'s last admin command. MSI-X entry 0 through the existing
/// `PciScanner::enable_msix` once `msix_map_table` has mapped the table; else single-message MSI (`enable_msi`); else
/// polled. Returns `("msix"|"msi"|"none", vector)`; every non-MSI-X answer prints its refusal by name. No INTMS/INTMC
/// write (NVMe 1.4 §3.1.3/§3.1.4: not under MSI-X; under one MSI message the admin and I/O CQs share vector 0, and the
/// admin queue is idle after probe — NVME-INTMS owed if that changes).
fn arm_irq(bus: u8, slot: u8, func: u8, bar0: u64) -> (&'static str, u8) {
    let table = crate::drivers::pci::PciScanner::msix_map_table(bus, slot, func, bar0);
    let Some(v) = crate::arch::interrupts::vectors::alloc("nvme", nvme_msi_handler) else {
        serial_println!("[nvme] irq polled why=no-vector — completions stay on the phase-bit poll");
        return ("none", 0);
    };
    let msg_addr = 0xFEE0_0000u32 | ((crate::arch::apic::apic_id() as u32) << 12);
    let msix_why = match table {
        Ok(()) => {
            if crate::drivers::pci::PciScanner::enable_msix(bus, slot, func, bar0, msg_addr, v as u32) {
                return ("msix", v);
            }
            "msix-refused"
        }
        Err(why) => why,
    };
    if crate::drivers::pci::PciScanner::enable_msi(bus, slot, func, msg_addr, v as u32) {
        serial_println!("[nvme] irq msi why={} — one MSI message, vector={:#04x}", msix_why, v);
        return ("msi", v);
    }
    serial_println!("[nvme] irq polled why={},no-msi — completions stay on the phase-bit poll", msix_why);
    ("none", 0)
}

// ══════════ NVMEIRQWAIT (zenbook-ledger Z49) — the I/O wait sleeps on the vector ══════════

/// NVMEIRQWAIT: the APIC id `arm_irq`'s message targets (set at [`probe`] for msix|msi); `u32::MAX` = no vector.
static IRQ_APIC: AtomicU32 = AtomicU32::new(u32::MAX);
// NVMEIRQWAIT: `static SLEEP_CPU` (the CPU of a `CTRL` holder asleep in `cq_wait`) — retired by NVMEPREEMPTOFF (Z52): the
// sleeping holder is never preempted, so no contender on its CPU needs to know it sleeps.
/// NVMEIRQWAIT: sleeping waits this boot (`waits=`), and those the vector ended (`wakes=`; the rest the tick ended).
static IRQ_WAITS: AtomicU32 = AtomicU32::new(0);
static IRQ_WAKES: AtomicU32 = AtomicU32::new(0);
/// NVMEIRQWAIT: the off-latch — [`WAIT_OFF_AFTER`] sleeps with no wake: the vector never wakes, back on the spin.
static WAIT_OFF: AtomicBool = AtomicBool::new(false);
const WAIT_OFF_AFTER: u32 = 16;
/// NVMEIRQWAIT: the scratch leg's wall time in microseconds (`rw_us=`; 0 = the leg did not run).
static RW_US: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// NVMEPREEMPTOFF (Z52): preempts deferred (`sched::preemptoff`) during the scratch leg (`deferred=`; 0 is legal).
static RW_DEFERRED: AtomicU32 = AtomicU32::new(0);

// NVMEIRQWAIT: `fn this_cpu()` (SLEEP_CPU's index) — retired with it by NVMEPREEMPTOFF (Z52).

// NVMEIRQWAIT: `fn sleeper_here()` (a sleeping holder on THIS CPU: a masked caller refused at once) — retired by
// NVMEPREEMPTOFF (Z52): the sleeping holder is never preempted.
// NVMEIRQWAIT: `fn irqwait_yield()` (an unmasked caller yields to a sleeping holder on its CPU, bounded `T_HOLD_MS`) —
// retired by NVMEPREEMPTOFF (Z52): the sleeping holder is never preempted.

/// NVMEIRQWAIT: THE wait between two polls of [`submit`]'s loop (the loop keeps the phase bit, the deadline and the
/// timeout line). It sleeps only when `may_sleep` (an I/O command, an unmasked caller), the vector targets THIS CPU and
/// the off-latch is clear: `sti; hlt` as one pair (a message latched in the IRR during the masked submit fires right
/// after `sti` and ends the `hlt`), then masked again; a wake with `CQ_PENDING` set is the vector's. Every other wait is
/// the old spin. Runs inside `with_ctrl`'s hold (IF=0 on entry and on return). NVMEPREEMPTOFF (Z52): the sleep holds the
/// scheduler's preempt-off count, so the holder is never switched out while its interrupts are on.
fn cq_wait(may_sleep: bool) {
    if !may_sleep || WAIT_OFF.load(Ordering::Relaxed) || IRQ_APIC.load(Ordering::Acquire) != crate::arch::apic::apic_id() as u32 {
        core::hint::spin_loop();
        return;
    }
    crate::arch::sched::preemptoff::disable(); // NVMEPREEMPTOFF (Z52): the holder sleeps with interrupts ON but is never switched out
    let waits = IRQ_WAITS.fetch_add(1, Ordering::Relaxed) + 1;
    x86_64::instructions::interrupts::enable_and_hlt();
    x86_64::instructions::interrupts::disable();
    crate::arch::sched::preemptoff::enable(); // NVMEPREEMPTOFF (Z52): masked again; a preempt deferred during the sleep fires after the hold
    if CQ_PENDING.load(Ordering::Acquire) {
        IRQ_WAKES.fetch_add(1, Ordering::Relaxed);
    } else if waits >= WAIT_OFF_AFTER && IRQ_WAKES.load(Ordering::Relaxed) == 0 && !WAIT_OFF.swap(true, Ordering::AcqRel) {
        serial_println!("[nvme] irq wait off why=no-wakes waits={} — completions back on the spin", waits);
    }
}
