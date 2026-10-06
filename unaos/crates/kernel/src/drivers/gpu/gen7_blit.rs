//! GEN7B (B422, R101) — the Ivy Bridge blitter (BCS) as KCOMP's second `Blitter`, beside the Kepler CE's.
//! CHARTER: Kernel — driver
//!
//! Design and ladder: `docs/dev/evidence/rmbp-1005/gen7b.md` (DRIVERS-METHOD §6); encodings are R7/R8's,
//! pinned in `docs/dev/OS/08_VIDEO/gen7.md` §2.7 and exercised on metal (flights 4, 10–19). The trait is
//! KCOMP's (`video/blitter.rs`); this file is the impl and the one-shot job that backs it.
//!
//! A child of `gen7` (declared by `#[path]` at the tail of gen7.rs) so it is built from the SAME helpers
//! flight 4 executed — `rd`/`wr`, `fw_acquire`/`fw_release`, `R6_CANDS`, `poll_cycles`, `clflush_range`,
//! the GGTT constants — not from a copy of them.
//!
//! THE JOB ([`bcs_job`]): every page of the source and destination spans mapped into its OWN GGTT window
//! (base slot 0x11000, above R5–R8's), the window proven unowned first (all-zero, or R4b's BDSM
//! scratch-fill on every slot, both neighbours and six far probes), `0x101008` written after the PTE update
//! (Vol1 Pt3 §1.2.21 p.191), the BCS ring armed under a forcewake hold, one XY_SRC_COPY_BLT per rect, one
//! MI_FLUSH_DW, one MI_STORE_DATA_IMM sentinel into a page of its own; then ring disabled, ring registers
//! restored to their entry images, forcewake released, PTEs restored to the fill image and re-read, the
//! flush written again. A ring that will not disable or drain leaves the PTEs claimed (a live engine must
//! not lose its pages) and says so. The panel is never written: the self-test's surfaces are heap scratch.
//!
//! THE ARMING RULE (B2): the BCS is offered to KCOMP only when its self-test passes, it is the fastest
//! passing candidate on the same fixture (cpu, the CE when armed, bcs), AND it can address the hot path's
//! destination — the present is `Ram -> Scanout`, and Scanout is the Kepler's BAR1 while the gmux reads
//! DIS (no page in-tree cites IGD DMA into another device's BAR). So on this machine today the line reads
//! `declined` by construction; the capture ([`crate::drivers::gpu::igpu::dpy_capture`]) is what decides it.
use super::*;
use crate::video::blitter::{self as kb, BlitErr, BlitJob, Blitter, Fence, Mem, Op, Rect, Surface};
use crate::video::FrameBuffer;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering as AO};

/// The GGTT inside BAR0. [ONE-SOURCE: capture — R4..R8 `ptes_landed=1` on metal]; the job's `landed=`
/// readback refutes it.
const GTT_BASE: usize = 0x200000;
const GGTT_SLOTS: usize = 524_288;
/// This job's window. GGTT address 0x11000000: RING_START bits[31:29] zero (Vol1 Pt3 §1.1.11.3 p.77), and
/// clear of R5..R8's windows (0x10000.. + R8's ≤ ~300 slots).
const BASE_SLOT: usize = 0x11000;
/// Ring page + sentinel page + the spans. 8192 slots = 32 MiB, a full 2880x1800x4 panel and a band.
const MAX_WIN: usize = 8192;
/// One ring page: 8 DW per rect + 4 flush + 4 store ≤ 1024 DW. KCOMP's present is one rect.
pub const MAX_RECTS: usize = 16;
const SENTINEL: u32 = 0x0B7C_B10B;
const SENTINEL_SEED: u32 = 0xDA7A_5EEB;
// R7/R8's encodings, unchanged (gen7.md §2.7 table; Vol1 Pt4 §1.9.14 pp.62–63, §2.2.5 pp.137–139).
const XY_SRC_COPY_BLT_DW0: u32 = (0x2 << 29) | (0x53 << 22) | (0x3 << 20) | 6; // 0x54F00006
const BR13_DEPTH_ROP: u32 = (0x3 << 24) | (0xCC << 16); // 32-bit colour | ROP CCh "S"
const MI_FLUSH_DW_DW0: u32 = (0x26 << 23) | (4 - 2); // 0x13000002, post-sync none, TLB-invalidate 0
/// The pinned write-any-value GTT flush (gen7.md §2.6, Vol1 Pt3 §1.2.21 p.191).
const GFX_FLSH: usize = g7regs::HYP_GFX_FLSH_CNTL;

/// One plane of a job: where its bytes are (kernel VA), how many, and its pitch in bytes. 32 bpp only.
#[derive(Clone, Copy)]
pub struct Plane {
    pub va: usize,
    pub len: usize,
    pub pitch: usize,
}

/// What a job did, for the wire. Every field is a reading, none a verdict.
#[derive(Clone, Copy, Default)]
pub struct JobOut {
    pub cand: &'static str,
    pub win: usize,
    pub landed: bool,
    pub armed: bool,
    pub head: u32,
    pub tail: u32,
    pub sentinel: bool,
    pub idle: bool,
    pub regs_restored: bool,
    pub fw_restored: bool,
    pub ptes_restored: bool,
    pub setup_us: u64,
    pub bcs_us: u64,
}

impl JobOut {
    pub fn clean(&self) -> bool {
        self.landed && self.armed && self.sentinel && self.idle && self.regs_restored && self.fw_restored && self.ptes_restored
    }
}

/// The ring + sentinel pages (8 KiB, allocated once, held for the boot: they are GGTT-reachable and a
/// register with no read decode is never the sole reason a page goes back to the heap, gen7.md §2.6).
/// The lock is also the engine's: a second job while one runs is `busy`.
static PAGES: spin::Mutex<usize> = spin::Mutex::new(0);

fn span_pages(p: &Plane) -> (usize, usize) {
    let first = p.va & !0xFFF;
    let last = (p.va + p.len - 1) & !0xFFF;
    (first, (last - first) / 4096 + 1)
}

/// Rect bounds against a plane: the last byte the rect touches must be inside `len`.
fn rect_in(p: &Plane, x: usize, y: usize, w: usize, h: usize) -> bool {
    w != 0 && h != 0 && (y + h - 1).saturating_mul(p.pitch).saturating_add((x + w) * 4) <= p.len
}

/// One BCS job: `rects` (pixels, 32 bpp) from `src` to `dst`, synchronous within the engine budgets.
/// `Err(why)` before any write; `Ok(out)` once the window was claimed (read `out` for what happened).
///
/// # Safety
/// `bar0`/`bar0_size` are the IGD BAR0 mapping `igpu::init` published (the ladder's bank). `src` and `dst`
/// are mapped kernel memory of at least `len` bytes that outlive the call and that nothing else writes
/// during it. Writes: the window's PTEs (restored), `0x101008`, one forcewake request register at a time
/// (restored), the four BCS ring registers (restored). No display register.
pub unsafe fn bcs_job(bar0: usize, bar0_size: usize, src: Plane, dst: Plane, rects: &[Rect], tag: &str) -> Result<JobOut, &'static str> {
    if rects.is_empty() || rects.len() > MAX_RECTS {
        return Err("rect-count");
    }
    if src.pitch == 0 || dst.pitch == 0 || src.pitch > 0xFFFF || dst.pitch > 0xFFFF || src.pitch % 4 != 0 || dst.pitch % 4 != 0 {
        return Err("pitch-not-br13"); // BR13/BR11 pitch is a 16-bit byte field (Vol1 Pt4 §1.10.7 p.91, §1.10.9 pp.92–93)
    }
    for r in rects {
        if !rect_in(&src, r.sx, r.sy, r.w, r.h) || !rect_in(&dst, r.dx, r.dy, r.w, r.h) {
            return Err("bounds");
        }
        if r.sx + r.w > 0xFFFF || r.sy + r.h > 0xFFFF || r.dx + r.w > 0xFFFF || r.dy + r.h > 0xFFFF {
            return Err("coord-not-16bit"); // DW2/DW3/DW5 carry 16-bit X and Y (§1.9.14 p.63)
        }
    }
    let Some(mut pages) = PAGES.try_lock() else { return Err("busy") };
    if *pages == 0 {
        let p = alloc_zeroed(Layout::from_size_align(8192, 4096).unwrap());
        if p.is_null() {
            return Err("alloc-ring");
        }
        *pages = p as usize; // held for the boot (see PAGES)
    }
    let ring_va = *pages;
    let sent_va = ring_va + 4096;
    let (s0, ns) = span_pages(&src);
    let (d0, nd) = span_pages(&dst);
    let win = 2 + ns + nd;
    if win > MAX_WIN || BASE_SLOT + win + 1 >= GGTT_SLOTS || bar0_size < GTT_BASE + (BASE_SLOT + win + 2) * 4 || bar0_size < GFX_FLSH + 4 {
        return Err("window-too-large");
    }
    let t_setup = crate::arch::now_cycles();

    // ---- census: the whole window and both neighbours, read before any write (R4b's legs) ----------
    let slot = |k: usize| GTT_BASE + (BASE_SLOT + k) * 4;
    let prev_off = GTT_BASE + (BASE_SLOT - 1) * 4;
    let next_off = slot(win);
    let fill = rd(bar0, slot(0));
    let (prev, next) = (rd(bar0, prev_off), rd(bar0, next_off));
    let mut uniform = prev == fill && next == fill;
    for k in 0..win {
        if rd(bar0, slot(k)) != fill {
            uniform = false;
            break;
        }
    }
    if !uniform {
        return Err("window-owned");
    }
    if fill != 0 {
        let bdsm = crate::arch::pci::read_config_32(0, 0, 0, 0xB0) & 0xFFF0_0000;
        let wellformed = (fill & 1) != 0 && (fill & 0xFFFF_F000) != 0 && (fill & 0xF0) == 0;
        let is_bdsm = (fill >> 12) == (bdsm >> 12);
        let far = R5_FAR_PROBE.iter().all(|&s| GTT_BASE + s * 4 + 4 <= bar0_size && rd(bar0, GTT_BASE + s * 4) == fill);
        if !(wellformed && is_bdsm && far) {
            return Err("fill-not-scratch");
        }
    }
    let base_img = fill;

    // ---- the PTEs: ring, sentinel, source span, destination span ----------------------------------
    let mut ptes: Vec<u32> = Vec::new();
    if ptes.try_reserve_exact(win).is_err() {
        return Err("alloc-ptes");
    }
    for k in 0..win {
        let va = if k == 0 {
            ring_va
        } else if k == 1 {
            sent_va
        } else if k < 2 + ns {
            s0 + (k - 2) * 4096
        } else {
            d0 + (k - 2 - ns) * 4096
        };
        let Some(pa) = crate::arch::memory::translate(va as u64) else { return Err("va-unmapped") };
        if pa >= 0x1_0000_0000 {
            return Err("phys-above-4g"); // the 32-bit PTE form R4..R8 flew; bits 11:4 (addr 39:32) never written
        }
        let pte = (pa as u32 & 0xFFFF_F000) | GGTT_PTE_VALID;
        if pte == base_img {
            return Err("pte-indistinct");
        }
        ptes.push(pte);
    }
    let gtt = |k: usize| ((BASE_SLOT + k) * 4096) as u32;
    let src_gtt = gtt(2) + (src.va & 0xFFF) as u32;
    let dst_gtt = gtt(2 + ns) + (dst.va & 0xFFF) as u32;
    let sent_gtt = gtt(1);

    // ---- BCS entry images; a live ring is never armed under ----------------------------------------
    let start_pre = rd(bar0, g7regs::BCS_RING_START);
    let ctl_pre = rd(bar0, g7regs::BCS_RING_CTL);
    let head_pre = rd(bar0, g7regs::BCS_RING_HEAD);
    let tail_pre = rd(bar0, g7regs::BCS_RING_TAIL);
    if ctl_pre & 1 != 0 {
        return Err("bcs-ring-live");
    }

    // ---- the ring: per rect XY_SRC_COPY_BLT (8 DW), then MI_FLUSH_DW (4), MI_STORE_DATA_IMM (4) ------
    let ring = ring_va as *mut u32;
    for i in 0..1024 {
        core::ptr::write_volatile(ring.add(i), MI_NOOP);
    }
    let mut d = 0usize;
    let mut put = |v: u32| {
        core::ptr::write_volatile(ring.add(d), v);
        d += 1;
    };
    for r in rects {
        put(XY_SRC_COPY_BLT_DW0);
        put(BR13_DEPTH_ROP | dst.pitch as u32); // BR13: depth, ROP, dst pitch in bytes
        put(((r.dy as u32) << 16) | r.dx as u32); // DW2 dst Y1|X1
        put((((r.dy + r.h) as u32) << 16) | (r.dx + r.w) as u32); // DW3 dst Y2|X2 (exclusive: R8 spill=0)
        put(dst_gtt); // DW4 dst base
        put(((r.sy as u32) << 16) | r.sx as u32); // DW5 src Y1|X1
        put(src.pitch as u32); // DW6 src pitch (BR11, bytes)
        put(src_gtt); // DW7 src base
    }
    put(MI_FLUSH_DW_DW0);
    put(0);
    put(0);
    put(0);
    put(MI_STORE_DATA_IMM_DW0);
    put(0);
    put(sent_gtt);
    put(SENTINEL);
    let tail = (d * 4) as u32; // 8n+8 DW: always QWord aligned (§1.1.11.1 p.75)
    let sent = sent_va as *mut u32;
    core::ptr::write_volatile(sent, SENTINEL_SEED);
    clflush_range(ring_va, 8192);
    clflush_range(src.va, src.len);
    clflush_range(dst.va, dst.len);

    let mut out = JobOut { cand: "none", win, tail, ..JobOut::default() };

    // ---- claim: PTEs, landed check, the pinned flush ------------------------------------------------
    for k in 0..win {
        wr(bar0, slot(k), ptes[k]);
    }
    out.landed = (0..win).all(|k| rd(bar0, slot(k)) == ptes[k]) && rd(bar0, prev_off) == prev && rd(bar0, next_off) == next;
    wr(bar0, GFX_FLSH, 1); // write-any-value: invalidate after a CPU-side GTT update (§1.2.21 p.191)

    let mut all_disabled = true;
    let mut cyc = 0u64;
    if out.landed {
        for &(cand, class_pin, req_off, ack_off, ack_mask, srcp, mask_form) in R6_CANDS.iter() {
            let hold = fw_acquire(bar0, tag, cand, class_pin, req_off, ack_off, ack_mask, srcp, mask_form, FW_ACK_BUDGET_CYC);
            wr(bar0, g7regs::BCS_RING_CTL, 0);
            wr(bar0, g7regs::BCS_RING_START, gtt(0));
            wr(bar0, g7regs::BCS_RING_HEAD, 0);
            wr(bar0, g7regs::BCS_RING_TAIL, 0);
            wr(bar0, g7regs::BCS_RING_CTL, RCS_RING_CTL_1PAGE_EN);
            let armed = rd(bar0, g7regs::BCS_RING_CTL) & 1 == 1;
            let mut idle = true;
            if armed {
                if out.setup_us == 0 {
                    out.setup_us = kb::cycles_to_us(crate::arch::now_cycles().saturating_sub(t_setup));
                }
                core::sync::atomic::compiler_fence(AO::SeqCst);
                let t0 = crate::arch::now_cycles();
                wr(bar0, g7regs::BCS_RING_TAIL, tail);
                let (_hit, _it, _cy) = poll_cycles(EXEC_BUDGET_CYC, || {
                    clflush_range(sent_va, 4);
                    core::ptr::read_volatile(sent) == SENTINEL
                });
                let (i, _it2, _cy2) = poll_cycles(DRAIN_BUDGET_CYC, || (rd(bar0, g7regs::BCS_RING_HEAD) & RING_HEAD_OFF_MASK) == tail);
                idle = i;
                cyc = crate::arch::now_cycles().saturating_sub(t0);
                clflush_range(sent_va, 4);
                out.sentinel = core::ptr::read_volatile(sent) == SENTINEL;
                out.head = rd(bar0, g7regs::BCS_RING_HEAD) & RING_HEAD_OFF_MASK;
                out.idle = idle;
            }
            wr(bar0, g7regs::BCS_RING_CTL, 0);
            let disabled = rd(bar0, g7regs::BCS_RING_CTL) & 1 == 0;
            if disabled {
                wr(bar0, g7regs::BCS_RING_TAIL, tail_pre);
                wr(bar0, g7regs::BCS_RING_HEAD, head_pre);
                wr(bar0, g7regs::BCS_RING_START, start_pre);
                wr(bar0, g7regs::BCS_RING_CTL, ctl_pre);
                out.regs_restored = rd(bar0, g7regs::BCS_RING_START) == start_pre
                    && rd(bar0, g7regs::BCS_RING_CTL) == ctl_pre
                    && (rd(bar0, g7regs::BCS_RING_HEAD) & RING_HEAD_OFF_MASK) == (head_pre & RING_HEAD_OFF_MASK)
                    && rd(bar0, g7regs::BCS_RING_TAIL) == tail_pre;
            }
            let (fw_ok, _ev) = fw_release(bar0, tag, &hold);
            out.fw_restored = fw_ok;
            all_disabled &= disabled;
            if armed {
                out.armed = true;
                out.cand = cand;
                all_disabled &= idle; // a ring that did not drain may still be writing: keep the PTEs
                break;
            }
            if !disabled {
                break;
            }
        }
    }
    out.bcs_us = kb::cycles_to_us(cyc);

    // ---- unwind the window: only behind a quiesced engine -------------------------------------------
    if all_disabled {
        for k in 0..win {
            wr(bar0, slot(k), base_img);
        }
        out.ptes_restored = (0..win).all(|k| rd(bar0, slot(k)) == base_img) && rd(bar0, prev_off) == prev && rd(bar0, next_off) == next;
        wr(bar0, GFX_FLSH, 1);
    }
    clflush_range(dst.va, dst.len); // the CPU reads what the engine wrote, not a stale line
    Ok(out)
}

// =====================================================================================================
// The Blitter impl — KCOMP's trait, the BCS's answers.
// =====================================================================================================

/// The arming decision's result (B2): set only by [`test`] when every leg of the rule holds.
static ARMED: AtomicBool = AtomicBool::new(false);
/// The capture said the panel is the IGD's (gmux READ_DISPLAY = IGD): a Scanout surface is then
/// IGD-addressable memory. Never true on a boot whose gmux reads DIS.
static PANEL_IGD: AtomicBool = AtomicBool::new(false);

pub struct BcsBlitter;
pub static BCS: BcsBlitter = BcsBlitter;

fn plane(s: &Surface) -> Plane {
    let i = s.fb.info();
    Plane { va: s.fb.base_addr(), len: s.fb.len(), pitch: i.stride * i.bytes_per_pixel }
}

impl Blitter for BcsBlitter {
    fn blit(&self, job: &BlitJob) -> Result<Fence, BlitErr> {
        if !ARMED.load(AO::Acquire) {
            return Err(BlitErr::Unavailable);
        }
        let (si, di) = (job.src.fb.info(), job.dst.fb.info());
        if job.op != Op::Copy || di.bytes_per_pixel != 4 || si.bytes_per_pixel != 4 || si.pixel_format != di.pixel_format {
            return Err(BlitErr::Unsupported); // straight 32-bit copies only (BR13 depth 11b); a blend is not idempotent
        }
        if job.dst.mem == Mem::Scanout && !PANEL_IGD.load(AO::Acquire) {
            return Err(BlitErr::Unsupported); // the Kepler's BAR1 is not IGD-addressable (B2)
        }
        for r in job.rects {
            if r.sx + r.w > si.width || r.sy + r.h > si.height || r.dx + r.w > di.width || r.dy + r.h > di.height {
                return Err(BlitErr::Bounds);
            }
        }
        let Some(b) = *super::ladder::BANK.lock() else { return Err(BlitErr::Unavailable) };
        // SAFETY: the bank's BAR0 is igpu::init's live mapping; the job's surfaces outlive `blit` (the
        // caller holds them across blit + wait, the trait's contract).
        match unsafe { bcs_job(b.bar0, b.bar0_size, plane(&job.src), plane(&job.dst), job.rects, "g7b") } {
            Ok(o) if o.clean() => Ok(Fence::DONE),
            Ok(o) if o.armed && !o.idle => {
                ARMED.store(false, AO::Release);
                kb::demote_bcs(); // a stall demotes, as the CE's Timeout does
                Err(BlitErr::Timeout)
            }
            Ok(_) => Err(BlitErr::Unavailable),
            Err("busy") => Err(BlitErr::Unavailable),
            Err("bounds") => Err(BlitErr::Bounds),
            Err(_) => Err(BlitErr::Unsupported),
        }
    }

    fn wait(&self, _f: Fence, _timeout_us: u32) -> Result<(), BlitErr> {
        Ok(()) // `blit` waited on the sentinel inside the engine budget
    }

    fn name(&self) -> &'static str {
        "bcs"
    }
}

// =====================================================================================================
// `tests gen7` tail — the self-test, the capture, the arming decision (R80: nothing at boot).
// =====================================================================================================

const FW: usize = 512; // fixture source: 512x128, two 512x64 rects in ONE ring (the multi-rect encoding)
const FH: usize = 128;
const DW: usize = 640; // destination 640x136, rects at (8,4) and (8,68)
const DH: usize = 136;
const DX: usize = 8;
const DY: usize = 4;

fn src_seed(i: usize) -> u32 {
    0x5B7B_0000u32 ^ (i as u32).wrapping_mul(0x9E37_79B9)
}
fn dst_seed(i: usize) -> u32 {
    0xDB7B_0000u32 ^ (i as u32).wrapping_mul(0x85EB_CA6B)
}

/// The first run's three lines, replayed on a second `tests gen7` (the pages are held, gen7.md §2.6).
static DONE: spin::Mutex<Option<[alloc::string::String; 3]>> = spin::Mutex::new(None);

fn mk_fb(base: usize, len: usize, w: usize, h: usize) -> FrameBuffer {
    let mut f = FrameBuffer::new();
    f.init(base, len, unaos_boot_info::FrameBufferInfo { width: w, height: h, stride: w, bytes_per_pixel: 4, pixel_format: unaos_boot_info::PixelFormat::Bgr });
    f
}

fn heap(n: usize) -> Option<Vec<u8>> {
    let mut v = Vec::new();
    v.try_reserve_exact(n).ok()?;
    v.resize(n, 0);
    Some(v)
}

/// B2 as a pure function: (winner among passing candidates, Ok when the BCS arms, Err(why) otherwise).
pub fn decide(bcs_ok: bool, bcs_us: u64, cpu_us: u64, gpu_us: Option<u64>, owner: &str) -> (&'static str, Result<(), &'static str>) {
    let mut win = ("cpu", cpu_us);
    if let Some(g) = gpu_us {
        if g < win.1 {
            win = ("gpu", g);
        }
    }
    if bcs_ok && bcs_us < win.1 {
        win = ("bcs", bcs_us);
    }
    let why = if !bcs_ok {
        Err("selftest-failed")
    } else if win.0 == "cpu" {
        Err("slower-than-cpu")
    } else if win.0 == "gpu" {
        Err("slower-than-gpu")
    } else if owner != "igd" {
        Err(if owner == "kepler" { "hot-dst-on-kepler" } else { "hot-dst-owner-unknown" })
    } else {
        Ok(())
    };
    (win.0, why)
}

pub fn test() {
    if let Some(l) = DONE.lock().as_ref() {
        serial_println!(":: gen7: blit2 replay=1 note=the-job-ran-once-this-boot-its-pages-are-held ::");
        for s in l.iter() {
            serial_println!("{}", s);
        }
        return;
    }
    let owner = crate::drivers::gpu::igpu::dpy_capture();
    let gate = match (*super::ladder::BANK.lock(), super::ladder::r7_verdict()) {
        (None, _) => Err("gated-no-bank"),
        (Some(_), _) if crate::drivers::gpu::igpu::blt_ring_live() => Err("gated-igpu-blt-ring-live"),
        (Some(_), Some("r7-blit-verified")) => Ok(()),
        (Some(_), _) => Err("gated-r7"), // R19: the BCS must have blitted THIS boot (R7) before a job is asked
    };
    let Some(b) = *super::ladder::BANK.lock() else {
        serial_println!(":: GEN7BLIT2: impl=bcs selftest=gated-no-bank us=0 -> declined ::");
        return;
    };
    let (sb, db) = (FW * FH * 4, DW * DH * 4);
    let lay_s = Layout::from_size_align(sb, 4096).unwrap();
    let lay_d = Layout::from_size_align(db, 4096).unwrap();
    // SAFETY: page-aligned heap scratch, held for the boot once the BCS may have mapped it (gen7.md §2.6).
    let (sp, dp) = unsafe { (alloc_zeroed(lay_s), alloc_zeroed(lay_d)) };
    let (Some(mut dcpu), Some(mut dgpu)) = (heap(db), heap(db)) else {
        serial_println!(":: GEN7BLIT2: impl=bcs selftest=heap-short us=0 -> declined ::");
        return;
    };
    if sp.is_null() || dp.is_null() {
        serial_println!(":: GEN7BLIT2: impl=bcs selftest=heap-short us=0 -> declined ::");
        return;
    }
    let (s32, d32) = (sp as *mut u32, dp as *mut u32);
    for i in 0..sb / 4 {
        unsafe { core::ptr::write_volatile(s32.add(i), src_seed(i)) };
    }
    for i in 0..db / 4 {
        let v = dst_seed(i).to_le_bytes();
        unsafe { core::ptr::write_volatile(d32.add(i), dst_seed(i)) };
        dcpu[i * 4..i * 4 + 4].copy_from_slice(&v);
        dgpu[i * 4..i * 4 + 4].copy_from_slice(&v);
    }
    let rects = [
        Rect { sx: 0, sy: 0, dx: DX, dy: DY, w: FW, h: FH / 2 },
        Rect { sx: 0, sy: FH / 2, dx: DX, dy: DY + FH / 2, w: FW, h: FH / 2 },
    ];
    let sfb = mk_fb(sp as usize, sb, FW, FH);

    // The CPU leg: KCOMP's own CpuBlitter on the same bytes (the reference image and cpu_us).
    let jc = BlitJob { src: Surface::ram(sfb), dst: Surface::ram(mk_fb(dcpu.as_ptr() as usize, db, DW, DH)), rects: &rects, op: Op::Copy };
    let t = crate::arch::now_cycles();
    let (rc, _) = kb::run_on(&kb::CPU, &jc);
    let cpu_us = kb::cycles_to_us(crate::arch::now_cycles().saturating_sub(t));
    // The CE leg, only when GPUBLIT armed it: same fixture, direct (its fallback is not a measurement).
    let gpu_us = if kb::GPU.probe().is_ok() {
        let jg = BlitJob { src: Surface::ram(sfb), dst: Surface::ram(mk_fb(dgpu.as_ptr() as usize, db, DW, DH)), rects: &rects, op: Op::Copy };
        let t = crate::arch::now_cycles();
        let r = kb::GPU.blit(&jg);
        let us = kb::cycles_to_us(crate::arch::now_cycles().saturating_sub(t));
        if r.is_ok() && dgpu == dcpu { Some(us) } else { None }
    } else {
        None
    };

    // The BCS leg.
    let (job, why) = match gate {
        Err(w) => (None, w),
        Ok(()) => {
            // SAFETY: the bank's BAR0; heap scratch sp/dp outlive the job and are held after it.
            match unsafe { bcs_job(b.bar0, b.bar0_size, Plane { va: sp as usize, len: sb, pitch: FW * 4 }, Plane { va: dp as usize, len: db, pitch: DW * 4 }, &rects, "g7b") } {
                Ok(o) => (Some(o), ""),
                Err(w) => (None, w),
            }
        }
    };
    // Score against the CPU's image: dwords inside the rects that match, dwords outside that moved.
    let (mut matched, mut spill) = (0u32, 0u32);
    let total = (FW * FH) as u32;
    if job.is_some() {
        for y in 0..DH {
            for x in 0..DW {
                let i = y * DW + x;
                let got = unsafe { core::ptr::read_volatile(d32.add(i)) };
                let inside = x >= DX && x < DX + FW && y >= DY && y < DY + FH;
                if inside {
                    let want = u32::from_le_bytes([dcpu[i * 4], dcpu[i * 4 + 1], dcpu[i * 4 + 2], dcpu[i * 4 + 3]]);
                    matched += (got == want) as u32;
                } else if got != dst_seed(i) {
                    spill += 1;
                }
            }
        }
    }
    let o = job.unwrap_or_default();
    let ok = rc.is_ok() && job.is_some() && o.clean() && matched == total && spill == 0;
    let selftest: &str = if ok {
        "ok"
    } else if job.is_none() {
        why
    } else if !o.landed {
        "ptes-not-landed"
    } else if !o.armed {
        "enable-void"
    } else if !o.idle {
        "drain-timeout"
    } else if !o.sentinel {
        "sentinel-miss"
    } else if matched != total {
        "copy-partial"
    } else if spill != 0 {
        "spill"
    } else {
        "unwind-not-clean"
    };
    let gpu_tok = match gpu_us {
        Some(g) => alloc::format!("{}", g),
        None => alloc::string::String::from("-"),
    };
    let l1 = alloc::format!(
        "[gen7blit] job rects=2 fixture={}x{}->{}x{} win={} cand={} landed={} armed={} head={:08X} tail={:08X} sentinel={} idle={} regs_restored={} fw_restored={} ptes_restored={} match={}/{} spill={} setup_us={} bcs_us={} cpu_us={} gpu_us={} reclaim=held",
        FW, FH, DW, DH, o.win, o.cand, o.landed as u8, o.armed as u8, o.head, o.tail, o.sentinel as u8, o.idle as u8,
        o.regs_restored as u8, o.fw_restored as u8, o.ptes_restored as u8, matched, total, spill, o.setup_us, o.bcs_us, cpu_us, gpu_tok
    );
    let (winner, arm) = decide(ok, o.bcs_us, cpu_us, gpu_us, owner);
    PANEL_IGD.store(owner == "igd", AO::Release);
    if arm.is_ok() {
        ARMED.store(true, AO::Release);
        kb::arm_bcs();
    }
    let l2 = alloc::format!(
        ":: GEN7BLIT2: impl=bcs selftest={} us={} -> {} ::",
        selftest, o.bcs_us, if arm.is_ok() { "armed" } else { "declined" }
    );
    let l3 = alloc::format!(
        ":: KCOMP: blitter={} cand=cpu:{},gpu:{},bcs:{} fastest={} hot_dst=scanout panel={} -> bcs={}{}{} ::",
        kb::selected().name(), cpu_us, gpu_tok,
        if ok { alloc::format!("{}", o.bcs_us) } else { alloc::string::String::from(selftest) },
        winner, owner,
        if arm.is_ok() { "armed" } else { "declined(" }, arm.err().unwrap_or(""), if arm.is_ok() { "" } else { ")" }
    );
    for s in [&l1, &l2, &l3] {
        serial_println!("{}", s);
    }
    *DONE.lock() = Some([l1, l2, l3]);
    // sp/dp are deliberately not freed (reclaim=held); dcpu/dgpu were never GGTT-mapped and drop here.
}
