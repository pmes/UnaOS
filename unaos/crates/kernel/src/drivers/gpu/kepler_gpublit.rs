//! GPUBLIT (B371) — the GK107 copy engine arms ITSELF at the display takeover and carries KCOMP's
//! window present.
//! CHARTER: Kernel — driver
//!
//! FINDING (flight 22): `:: KCOMP: blitter=cpu ... gpu=unavailable` — every desktop frame is composited
//! by the CPU, and no flight has ever printed a `[ce]` line: the copy engine never ran on the metal,
//! because the only code that drove it (KBLIT, `kepler_ce.rs`) sits behind the recon knob
//! `UNAOS_KEPLER_CE` that R80/QUIETBOOT turned off. Design and witness:
//! `docs/dev/evidence/rmbp-1005/GPUBLIT.md`.
//!
//! MECHANISM. ONE copy-engine channel, built once by [`arm`] right after `takeover_display` succeeds
//! (no knob: every build that takes the panel carries this module), proven by a 64x64 self-test blit
//! compared byte for byte with the CPU blit of the same source, and then offered to the compositor
//! through [`copy_wait`] — a raw, video-type-free API (`video/blitter.rs::GpuBlitter` does the geometry).
//! Every wait is bounded on the TSC; a stall is a CPU fallback, never a hang and never a dark frame.
//!
//! THE CHANNEL'S VM. PDE[0] is a small-page table identity-mapping the VRAM span from the takeover
//! framebuffer to the top of this module's window (GPU VA == VRAM offset == BAR1 offset, KF24). PDE[1..5]
//! is a 512 MiB SYSMEM window, mapped lazily per source buffer: VA is bump-allocated and NEVER reused,
//! so no stale GPU TLB entry can exist and no TLB-invalidate register is needed. A mapping is cached per
//! buffer and re-validated on every job by re-walking the CPU page tables; a moved frame takes fresh VA.
//!
//! CLEAN-ROOM. The instance-block words are `kepler.rs`'s RAMFC layout (CLEAN_ROOM_POLICY §5), exactly as
//! KBLIT reused them. GPUBLIT2 (B383) pinned the runlist, channel-table, PTOP, gpfifo, copy-class and
//! host-semaphore encodings as HARDWARE FACTS (LAWS §licence) with a pointer at each definition: NVIDIA's
//! published open-gpu-doc (`cla0b5.h`, `cla06f.h`, gv100 `dev_ram`/`dev_pbdma`) and nouveau v6.10 file:line
//! (read outside the repo; no text copied). The GMMU big-page span stays [EXT-UNPINNED]. The boot's
//! `selftest=` verdict is what proves them; nothing here is claimed validated on the metal.
//!
//! KBLIT DEFECTS NOT CARRIED OVER (read against its `tests_kblit`): the channel IS bound in the PFIFO
//! channel table; RUNLIST_SUBMIT is handed a RUNLIST page (not the instance block); LAUNCH_DMA carries
//! the multi-line bit; the page table is indexed from the PDE's VA origin.

use super::kepler::{mmio_read, mmio_write, VramAllocator};
// KEPLERGR (B421): the fifo-init walls, RAMFC, bind, commit, PTOP walk, PDE/PTE and the decode line live ONCE in
// `kepler_fifo::host`; this leg (chid 2) and the GR leg (chid 1, `kepler_fifo::kgr`) both call it.
use super::kepler_fifo::host as kf;
use kf::{USERD_BAR1, R2A04, SCHED_DISABLE, PB_13C, PB_STRIDE, PB_RUNM, PFIFO_CHAN};
use kf::{pb_hdr, M_HOST_SEM, SEM_RELEASE_WFI, FB_BIGPAGE};
use kf::{RUNLIST_BASE, RUNLIST_SUBMIT, RUNLIST_INFO, PMC_ENABLE, PMC_PBDMA_ENABLE, USERD_GP_GET, USERD_GP_PUT, pde, pte};
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering::*};

// ---- window layout (offsets from the window base, a VRAM offset) -------------------------------------

const W_INST: usize = 0x0000; // instance block (RAMFC + page-directory pointer)
const W_RUNL: usize = 0x1000; // runlist: one entry, our channel
const W_USERD: usize = 0x2000; // GPUBLIT3: unused (USERD moved into the BAR1 table W_UTAB below); still zeroed
const W_GPFIFO: usize = 0x3000; // 512 gpfifo entries x 8 B
const W_SEM: usize = 0x4000; // the host semaphore the fence reads
const W_PUSH: usize = 0x8000; // 16 pushbuffer slots x 2 KiB
const W_SCR_A: usize = 0x10000; // self-test: the CE's destination
const W_SCR_B: usize = 0x20000; // self-test: the CPU blit's destination (timing only)
const W_PD: usize = 0x30000; // page directory (64 KiB reserved, first 4 KiB written)
const W_PT0: usize = 0x40000; // small-page table under PDE[0] (32768 entries)
const W_PTS: usize = 0x80000; // four small-page tables under PDE[1..5] (the sysmem window)
/// GPUBLIT3 (B410): the BAR1-polled USERD table 0x2254 names — 4096 channels x 0x200, our channel's USERD at
/// `W_UTAB + CHID*0x200`. // nouveau gk104.c:116 (size 0x200), :804 (4096 chids), base.c:305-319, chan.c:463
const W_UTAB: usize = 0x180000;
const UTAB_SIZE: usize = 4096 * 0x200;
const W_SPAN: usize = W_UTAB + UTAB_SIZE;

const PUSH_SLOT: usize = 0x800;
const PUSH_SLOTS: u32 = 16;
const GPFIFO_N: u32 = 512;
/// PDE span with small pages: 128 MiB with 128 KiB big pages (SPT 15 bits), 64 MiB with 64 KiB big pages (SPT 14 bits).
/// GPUBLIT3 reads which one the firmware left from 0x100c80 bit 0 at arm (`pde_span`).
/// // nouveau vmmgk104.c:41,55 (desc_17_12 / desc_16_12), fb/gf100.c:72-73 [ONE-SOURCE: nouveau; refuted by a PDE fault]
const PDE_SPAN: usize = 128 << 20;
// FB_BIGPAGE 0x100c80 and the span choice: `kepler_fifo::host::pde_span` (KEPLERGR; nouveau fb/gf100.c:72-73).
/// The sysmem window: 512 MiB of GPU VA starting at 128 MiB (PDE[1..5] at 128 MiB span, PDE[2..10] at 64 MiB); its small-page
/// entries are ONE flat array at W_PTS (131072 x 8 B), each PDE pointing at its slice.
const SYS_VA: usize = 128 << 20;
const SYS_BYTES: usize = 512 << 20;
const SYS_PAGES: usize = SYS_BYTES >> 12;
/// The channel id. Channel 1 is the fifo leg's (`kepler.rs`); 2 is free on every build. [TREE]
const CHID: u32 = 2;
const _: () = assert!(CHID == kf::CHID_CE, "KEPLERGR: the GR leg's guard reads the CE's bind by kf::CHID_CE");
// The copy engine's runlist, engine id and PMC reset bit are READ FROM PTOP at arm time (GPUBLIT2,
// B383) — `ptop_ce` below, nouveau top/gk104.c:45-81. B371's `CE_RUNLIST = 1` was a hypothesis.

// ---- encodings (GPUBLIT2: pinned against nouveau v6.10 + NVIDIA open-gpu-doc; GMMU still unpinned) ----

/// Copy class KEPLER_DMA_COPY_A. // open-gpu-doc cla0b5.h:33
const CLASS_COPY: u32 = 0x0000_A0B5;
/// Host SET_OBJECT (NVCLASS 15:0). // open-gpu-doc cla06f.h:69-70
const M_SET_OBJECT: u32 = 0x0000;
const M_LAUNCH_DMA: u32 = 0x0300; // open-gpu-doc cla0b5.h:66
const M_OFFSET_IN_UPPER: u32 = 0x0400; // cla0b5.h:123..129 IN_UPPER, IN_LOWER, OUT_UPPER, OUT_LOWER (0x400..0x40C)
const M_PITCH_IN: u32 = 0x0410; // cla0b5.h:131..137 PITCH_IN, PITCH_OUT, LINE_LENGTH_IN, LINE_COUNT (0x410..0x41C)
/// LAUNCH_DMA = DATA_TRANSFER_TYPE NON_PIPELINED (2, bits 1:0) | FLUSH_ENABLE (bit 2) | SRC_MEMORY_LAYOUT
/// PITCH (bit 7) | DST_MEMORY_LAYOUT PITCH (bit 8) | MULTI_LINE_ENABLE (bit 9). // open-gpu-doc
/// cla0b5.h:67-90. The bit KBLIT's `0x186` lacked is bit 9, so its 256-row copy would have moved one line.
const LAUNCH_PITCH_MULTI: u32 = 0x0000_0386;
// Host SEMAPHOREA..D (M_HOST_SEM 0x0010) and its RELEASE operation word (SEM_RELEASE_WFI 0x1002), and the pushbuffer
// header pb_hdr(): `kepler_fifo::host` (KEPLERGR — the GR leg pushes the same host methods; open-gpu-doc cla06f.h:77-97, :160-174).
// USERD GP_GET/GP_PUT, the channel table (PFIFO_CHAN + its hi-word fields), RUNLIST_BASE/SUBMIT/INFO/PENDING,
// PMC_ENABLE and PMC_PBDMA_ENABLE: `kepler_fifo::host` (KEPLERGR), each cited there (nouveau gk104.c:52,68,77,426,446-447,739;
// gf100.c:129-130).
/// PFIFO's PMC bit. // nouveau mc/gk104.c `gk104_mc_reset` { 0x00000100, NVKM_ENGINE_FIFO }
const PMC_PFIFO: u32 = 0x100;

// pde() / pte() / PTE_HI_SYSMEM (5): `kepler_fifo::host` (KEPLERGR — one source; nouveau vmmgf100.c:126-138, :263, :317-318, :328).

// ---- state ---------------------------------------------------------------------------------------------

static BAR0: AtomicUsize = AtomicUsize::new(0);
static BAR1: AtomicUsize = AtomicUsize::new(0);
static BAR1_SIZE: AtomicUsize = AtomicUsize::new(0);
static WIN: AtomicUsize = AtomicUsize::new(0);
static VLO: AtomicUsize = AtomicUsize::new(0);
static VHI: AtomicUsize = AtomicUsize::new(0);
static BUILT: AtomicBool = AtomicBool::new(false);
static ARMED: AtomicBool = AtomicBool::new(false);
static DEMOTED: AtomicBool = AtomicBool::new(false);
/// 0 never armed (no takeover on this boot), 1 ok, 2 mismatch, 3 timeout, 4 refused.
static VERDICT: AtomicU8 = AtomicU8::new(0);
static REFUSED: spin::Mutex<&'static str> = spin::Mutex::new("-");
static JOBS: AtomicU64 = AtomicU64::new(0);
static GPU_CYC: AtomicU64 = AtomicU64::new(0);
static BUSY: AtomicU64 = AtomicU64::new(0);
static TIMEOUTS: AtomicU64 = AtomicU64::new(0);
static UNMAPPABLE: AtomicU64 = AtomicU64::new(0);
static SYSMAPS: AtomicU64 = AtomicU64::new(0);
/// GPUBLIT2: the PTOP copy engine this channel runs on — CE index (type - 1), engine id, runlist, PMC
/// reset bit (`NONE` when PTOP gave none) — and the runlist commit's pending time.
const NONE: u32 = u32::MAX;
static CE_IDX: AtomicU32 = AtomicU32::new(NONE);
static CE_ENG: AtomicU32 = AtomicU32::new(NONE);
static CE_RL: AtomicU32 = AtomicU32::new(NONE);
static CE_RESET: AtomicU32 = AtomicU32::new(NONE);
static COMMIT_US: AtomicU32 = AtomicU32::new(NONE);
static PMC_PRE: AtomicU32 = AtomicU32::new(0);
/// GPUBLIT3: the PDE span this boot uses, and every pre/post image the walls line prints (DRIVERS-METHOD §6.6).
static SPAN: AtomicUsize = AtomicUsize::new(PDE_SPAN);
static FB_PAGE: AtomicU32 = AtomicU32::new(0);
static R2254_PRE: AtomicU32 = AtomicU32::new(0);
static R2A04_PRE: AtomicU32 = AtomicU32::new(0);
static R2630_PRE: AtomicU32 = AtomicU32::new(0);
static BIND_PRE: AtomicU32 = AtomicU32::new(0);
static INTR_PRE: AtomicU32 = AtomicU32::new(0);
static BIND_POST: AtomicU32 = AtomicU32::new(0);
static INTR_POST: AtomicU32 = AtomicU32::new(0);
static PB13C_PRE: [AtomicU32; 4] = [AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0)];

const NCACHE: usize = 16;
const MAXP: usize = 1024; // 4 MiB per mapping; a larger source is a CPU job

#[derive(Clone, Copy)]
struct MapEnt {
    va: usize,
    pages: usize,
    gva: usize,
    lru: u32,
}

struct Chan {
    put: u32,
    seq: u32,
    sys_next: usize,
    tick: u32,
    ent: [MapEnt; NCACHE],
    pfn: [[u32; MAXP]; NCACHE],
}

static CHAN: spin::Mutex<Chan> = spin::Mutex::new(Chan {
    put: 0,
    seq: 0,
    sys_next: 0,
    tick: 0,
    ent: [MapEnt { va: 0, pages: 0, gva: 0, lru: 0 }; NCACHE],
    pfn: [[0; MAXP]; NCACHE],
});

/// Errors the compositor maps onto `BlitErr`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CeErr {
    /// Not armed, demoted, or the channel is busy with another core's band.
    Unavailable,
    /// An endpoint the channel's VM cannot address (a frame with no translation, a source past 4 MiB,
    /// the sysmem window exhausted, a VRAM span outside the identity map) or too many rects.
    Unsupported,
    /// The semaphore did not release inside the budget. The CE is demoted for the rest of the boot.
    Timeout,
}

/// One rect in BYTES, relative to each endpoint's base address.
#[derive(Clone, Copy, Debug)]
pub struct CeRect {
    pub src_off: usize,
    pub src_pitch: usize,
    pub dst_off: usize,
    pub dst_pitch: usize,
    pub row_bytes: usize,
    pub rows: usize,
}

pub const MAX_RECTS: usize = 32;

// ---- raw access ----------------------------------------------------------------------------------------

#[inline]
fn vw(off: usize, v: u32) {
    let b = BAR1.load(Relaxed);
    unsafe { core::ptr::write_volatile((b + off) as *mut u32, v) }
}
#[inline]
fn vr(off: usize) -> u32 {
    let b = BAR1.load(Relaxed);
    unsafe { core::ptr::read_volatile((b + off) as *const u32) }
}
#[inline]
fn win() -> usize {
    WIN.load(Relaxed)
}
/// Our channel's USERD inside the BAR1 table (window offset). // nouveau chan.c:463 (`chid * userd->size`)
#[inline]
fn userd_off() -> usize {
    W_UTAB + CHID as usize * 0x200
}
#[inline]
fn mw(off: usize, v: u32) {
    unsafe { mmio_write(BAR0.load(Relaxed), off, v) }
}
#[inline]
fn mr(off: usize) -> u32 {
    unsafe { mmio_read(BAR0.load(Relaxed), off) }
}

fn tsc_hz() -> u64 {
    let hz = crate::arch::apic::tsc_hz();
    if hz == 0 { 1_250_000_000 } else { hz }
}
fn cyc_us(c: u64) -> u64 {
    c.saturating_mul(1_000_000) / tsc_hz()
}

// ---- M1 — arm at takeover ------------------------------------------------------------------------------

/// Called ONCE from `kepler::init` right after `takeover_display` returned `Some(fb_offset)`. Builds the
/// channel, runs the 64x64 self-test, prints the one `:: GPUBLIT:` line, arms the compositor's GPU
/// blitter on `ok`. No knob (B371).
pub fn arm(bar0: usize, bar1: usize, bar1_size: usize, vram_size: usize, fb_offset: usize, vram: &mut VramAllocator) {
    BAR0.store(bar0, Release);
    BAR1.store(bar1, Release);
    BAR1_SIZE.store(bar1_size, Release);
    let refuse = |why: &'static str| {
        *REFUSED.lock() = why;
        VERDICT.store(4, Release);
        serial_println!(":: GPUBLIT: selftest=refused({}) ce_us=0 cpu_us=0 -> blitter=cpu ::", why);
    };
    if bar0 == 0 || bar1 == 0 {
        return refuse("no-bars");
    }
    let Some(w) = vram.alloc(W_SPAN) else { return refuse("no-vram-window") };
    let (vlo, vhi) = (fb_offset & !0xFFF, w + W_SPAN);
    // GPUBLIT3 (R9): the big-page size the firmware left decides the PDE span. // nouveau fb/gf100.c:72-73
    let fbp = mr(FB_BIGPAGE);
    FB_PAGE.store(fbp, Release);
    let span = kf::pde_span(fbp);
    SPAN.store(span, Release);
    if vhi > span || vlo > w {
        return refuse("window-beyond-pde0");
    }
    if vhi > core::cmp::min(bar1_size, vram_size) {
        return refuse("window-beyond-bar1");
    }
    // GPUBLIT2: the CE's runlist / engine / reset bit from PTOP, as nouveau reads them. No CE0/CE1 in the
    // table is a refusal: the channel cannot be placed on a runlist the spec does not name.
    let Some(ce) = ptop_ce() else { return refuse("ptop-no-ce") };
    CE_IDX.store(ce.idx, Release);
    CE_ENG.store(ce.engine, Release);
    CE_RL.store(ce.runlist, Release);
    CE_RESET.store(ce.reset, Release);
    WIN.store(w, Release);
    VLO.store(vlo, Release);
    VHI.store(vhi, Release);

    build(w, vlo, vhi);
    BUILT.store(true, Release);

    let st = {
        let mut ch = CHAN.lock();
        selftest(&mut ch)
    };
    let v = match st.verdict {
        St::Ok => 1,
        St::Mismatch(_) => 2,
        St::Timeout => 3,
        St::Refused(why) => {
            *REFUSED.lock() = why;
            4
        }
    };
    VERDICT.store(v, Release);
    ARMED.store(v == 1, Release);
    print_line(&st, if v == 1 { "gpu" } else { "cpu" });
    if v != 1 {
        detail(&st);
    }
}

/// Lay down the VM, the instance block and the runlist; bind the channel; submit the runlist.
fn build(w: usize, vlo: usize, vhi: usize) {
    // The USERD table's first page (chids 0..7, ours at +0x400) is zeroed with the rest: GP_GET/GP_PUT start at 0
    // (nouveau gf100.c:118-131 `gf100_chan_userd_clear`). The other channels' slots are never read for an unbound chid.
    for off in [W_INST, W_RUNL, W_USERD, W_GPFIFO, W_SEM, W_PD, W_UTAB] {
        for i in 0..0x400 {
            vw(w + off + i * 4, 0);
        }
    }
    // PDE[0] -> PT0 (VRAM identity); the sysmem window's PDEs -> slices of the flat table at W_PTS (entries lazy).
    let span = SPAN.load(Acquire);
    let (d0, d1) = pde(w + W_PT0);
    vw(w + W_PD, d0);
    vw(w + W_PD + 4, d1);
    for i in 0..SYS_BYTES / span {
        let (d0, d1) = pde(w + W_PTS + i * (span >> 12) * 8);
        let k = SYS_VA / span + i;
        vw(w + W_PD + k * 8, d0);
        vw(w + W_PD + k * 8 + 4, d1);
    }
    let mut p = vlo;
    while p < vhi {
        let (e0, e1) = pte(p as u64, false);
        let idx = p >> 12; // indexed from the PDE's VA origin (KBLIT indexed from its window)
        vw(w + W_PT0 + idx * 8, e0);
        vw(w + W_PT0 + idx * 8 + 4, e1);
        p += 0x1000;
    }
    // Instance block: RAMFC exactly as nouveau `gk104_chan_ramfc_write` (gk104.c:88-102, devm 0xfff / priv gk104.c:110-111)
    // — GPUBLIT3 (R4) applies the five words GPUBLIT2 read — plus the page directory (inst+0x200 base, +0x208 limit:
    // nouveau vmmgf100.c:342-360 `gf100_vmm_join_`, VRAM target 0). USERD is the BAR1 table's slot for our chid.
    let (inst, userd, gpf, pd) = (w + W_INST, w + userd_off(), w + W_GPFIFO, w + W_PD);
    // KEPLERGR: RAMFC (`gk104_chan_ramfc_write`, the 15 words GPUBLIT3 applied) and the PD pointer are `kepler_fifo::host`'s.
    kf::ramfc_write(&vw, inst, userd, gpf, 9, CHID); // limit2 = log2(512) entries
    kf::inst_pd(&vw, inst, pd);
    // Runlist: one entry, our channel: (chid, 0). // nouveau gk104.c:454-455
    vw(w + W_RUNL, CHID);
    vw(w + W_RUNL + 4, 0);
    // PMC + PBDMA enable, OR-only: PFIFO and the CE's PTOP reset bit (nouveau mc/base.c:57, BIT(reset)).
    let rl = CE_RL.load(Acquire);
    let reset = CE_RESET.load(Acquire);
    let ce_bit = if reset < 32 { 1u32 << reset } else { 0 };
    let pmc_pre = mr(PMC_ENABLE);
    PMC_PRE.store(pmc_pre, Release);
    mw(PMC_ENABLE, pmc_pre | PMC_PFIFO | ce_bit);
    mw(PMC_PBDMA_ENABLE, 0xFFFF_FFFF);
    fifo_init(w, rl);
    // GPUBLIT2 (B383) — THE CHANGE. Bind in nouveau's order: unbind; the channel's RUNLIST into the hi
    // word (B371 wrote the whole word 0x400, leaving runlist 0 = GR's: flight 23 read `11000001`);
    // VALID | inst page; commit the runlist; only then ENABLE_SET.
    // nouveau gk104.c:60 (unbind), :77 + :68 (gk104_chan_bind), :446-447 (commit), :52 (start).
    // KEPLERGR: the bind, its verdict, the runlist commit (2 ms bound) and ENABLE_SET are `kepler_fifo::host`'s —
    // the same writes in the same order (unbind; runlist into hi; VALID | inst; read 0x2100/0x252c; commit; start).
    let b0 = BAR0.load(Relaxed);
    let bo = kf::bind(b0, CHID, rl, inst);
    // GPUBLIT3 (R3c): OUR bind's verdict, read before the runlist commit. // nouveau gk104.c:612-613
    INTR_POST.store(bo.intr_post, Release);
    BIND_POST.store(bo.bind_post, Release);
    // Runlist submit: base page (VRAM target 0), then (runlist << 20) | entries; COMMIT_US stays NONE on "stuck".
    if let Some(us) = kf::runlist_commit(b0, rl, w + W_RUNL, 1) {
        COMMIT_US.store(us, Release);
    }
    kf::start(b0, CHID);
}

/// GPUBLIT3 (B410) M1 — the fifo-init state nouveau writes and we never did, before the bind (R3, R5, R6, R7).
/// KEPLERGR: the writes are `kepler_fifo::host::fifo_init` (one source; it also registers this window's table as THE
/// USERD table the GR leg's chid 1 uses); this keeps every pre-image for the walls line, as before.
fn fifo_init(w: usize, rl: u32) {
    let b0 = BAR0.load(Relaxed);
    let mut pre = kf::Pre::new();
    let Some(io) = kf::fifo_init(b0, w + W_UTAB, rl, &mut pre) else { return };
    R2A04_PRE.store(pre.get(R2A04).unwrap_or(0), Release);
    for i in 0..4usize {
        if let Some(v) = pre.get(PB_13C + i * PB_STRIDE) {
            PB13C_PRE[i].store(v, Release);
        }
    }
    R2254_PRE.store(pre.get(USERD_BAR1).unwrap_or(0), Release);
    INTR_PRE.store(io.intr_pre, Release);
    BIND_PRE.store(io.bind_pre, Release);
    R2630_PRE.store(pre.get(SCHED_DISABLE).unwrap_or(0), Release);
}

/// One PTOP device: the CE this channel runs on.
#[derive(Clone, Copy)]
struct CeTop {
    idx: u32,
    engine: u32,
    runlist: u32,
    reset: u32,
}

/// Walk the PTOP device-info table exactly as nouveau `gk104_top_parse` does (top/gk104.c:37-81) and
/// return CE0 (engine type 1), else CE1 (type 2). CE2 (type 3) is NOT taken: on Kepler it is the GR
/// copy engine and shares GR's runlist with the fifo leg's channel.
fn ptop_ce() -> Option<CeTop> {
    // KEPLERGR: the walk is `kepler_fifo::host::ptop_engine` (one source, top/gk104.c:37-81); types 1/2 = CE0/CE1
    // (top/gk104.c:79-80), the lower index wins as before.
    let b0 = BAR0.load(Relaxed);
    let t = kf::ptop_engine(b0, 1).or_else(|| kf::ptop_engine(b0, 2))?;
    Some(CeTop { idx: t.ty - 1, engine: t.engine, runlist: t.runlist, reset: t.reset })
}

// ---- submission ----------------------------------------------------------------------------------------

/// Write `words` into the next pushbuffer slot, queue it on the gpfifo, ring GP_PUT, return the seq the
/// trailing host semaphore will release. The caller holds the channel.
fn submit(ch: &mut Chan, build: impl FnOnce(&mut dyn FnMut(u32)) -> ()) -> u32 {
    let w = win();
    ch.seq = ch.seq.wrapping_add(1);
    if ch.seq == 0 {
        ch.seq = 1;
    }
    let seq = ch.seq;
    let slot = w + W_PUSH + (ch.put % PUSH_SLOTS) as usize * PUSH_SLOT;
    let mut n = 0usize;
    {
        let mut push = |v: u32| {
            if n < PUSH_SLOT / 4 {
                vw(slot + n * 4, v);
            }
            n += 1;
        };
        push(pb_hdr(0, M_SET_OBJECT, 1));
        push(CLASS_COPY);
        build(&mut push);
        let sem = w + W_SEM;
        push(pb_hdr(0, M_HOST_SEM, 4));
        push((sem >> 32) as u32);
        push(sem as u32);
        push(seq);
        push(SEM_RELEASE_WFI);
    }
    let len = core::cmp::min(n, PUSH_SLOT / 4) as u32;
    // GP entry: ENTRY0 GET 31:2 (address), ENTRY1 GET_HI 7:0 | LENGTH 30:10 (dwords). // cla06f.h:139-148
    let e = w + W_GPFIFO + (ch.put % GPFIFO_N) as usize * 8;
    vw(e, (slot as u32) & 0xFFFF_FFFC);
    vw(e + 4, ((slot >> 32) as u32) | (len << 10));
    ch.put = (ch.put + 1) % GPFIFO_N;
    // Every byte the CE will read (the staging band in WB RAM, the pushbuffer in UC VRAM) must be
    // globally visible before the doorbell.
    core::sync::atomic::fence(SeqCst);
    unsafe { core::arch::x86_64::_mm_mfence() };
    vw(w + userd_off() + USERD_GP_PUT, ch.put);
    seq
}

/// Poll the host semaphore for `seq`, bounded by `budget_us` on the TSC. Returns (ok, cycles).
fn wait_seq(seq: u32, budget_us: u64) -> (bool, u64) {
    let sem = win() + W_SEM;
    let budget = tsc_hz().saturating_mul(budget_us) / 1_000_000;
    let t0 = crate::arch::now_cycles();
    loop {
        let v = vr(sem);
        if v != 0 && (v.wrapping_sub(seq) as i32) >= 0 {
            return (true, crate::arch::now_cycles().saturating_sub(t0));
        }
        let dt = crate::arch::now_cycles().saturating_sub(t0);
        if dt >= budget {
            return (false, dt);
        }
        core::hint::spin_loop();
    }
}

/// Resolve `[va + lo, va + hi)` to a GPU VA: identity VRAM inside BAR1, else the sysmem window.
fn resolve(ch: &mut Chan, va: usize, lo: usize, hi: usize) -> Result<usize, CeErr> {
    let (b1, b1s) = (BAR1.load(Relaxed), BAR1_SIZE.load(Relaxed));
    let (a, z) = (va + lo, va + hi);
    if a >= b1 && a < b1 + b1s {
        let (o, oz) = (a - b1, z - b1);
        if o < VLO.load(Relaxed) || oz > VHI.load(Relaxed) {
            return Err(CeErr::Unsupported);
        }
        return Ok(o);
    }
    map_sys(ch, a, z - a)
}

fn pfn_of(va: usize) -> Option<u32> {
    crate::arch::memory::translate(va as u64).map(|pa| (pa >> 12) as u32)
}

/// Map `[a, a + len)` of kernel RAM into the sysmem window, from the cache when every frame still
/// translates to the same page; else fresh, never-reused VA.
fn map_sys(ch: &mut Chan, a: usize, len: usize) -> Result<usize, CeErr> {
    let p0 = a & !0xFFF;
    let np = ((a + len + 0xFFF) & !0xFFF).saturating_sub(p0) >> 12;
    if np == 0 || np > MAXP {
        return Err(CeErr::Unsupported);
    }
    ch.tick = ch.tick.wrapping_add(1);
    let tick = ch.tick;
    for i in 0..NCACHE {
        let e = ch.ent[i];
        if e.pages >= np && e.va == p0 {
            let mut same = true;
            for k in 0..np {
                if pfn_of(p0 + (k << 12)) != Some(ch.pfn[i][k]) {
                    same = false;
                    break;
                }
            }
            if same {
                ch.ent[i].lru = tick;
                return Ok(e.gva + (a & 0xFFF));
            }
        }
    }
    if ch.sys_next + np > SYS_PAGES {
        return Err(CeErr::Unsupported);
    }
    let mut pf = [0u32; MAXP];
    for (k, slot) in pf.iter_mut().enumerate().take(np) {
        *slot = pfn_of(p0 + (k << 12)).ok_or(CeErr::Unsupported)?;
    }
    let first = ch.sys_next;
    ch.sys_next += np;
    let w = win();
    for (k, &f) in pf.iter().enumerate().take(np) {
        let (e0, e1) = pte((f as u64) << 12, true);
        let at = w + W_PTS + (first + k) * 8;
        vw(at, e0);
        vw(at + 4, e1);
    }
    // Victim: an empty entry, else the least recently used. Its VA is retired, never handed out again.
    let victim = (0..NCACHE).max_by_key(|&i| if ch.ent[i].pages == 0 { u32::MAX } else { tick.wrapping_sub(ch.ent[i].lru) }).unwrap_or(0);
    ch.ent[victim] = MapEnt { va: p0, pages: np, gva: SYS_VA + (first << 12), lru: tick };
    ch.pfn[victim][..np].copy_from_slice(&pf[..np]);
    SYSMAPS.fetch_add(1, Relaxed);
    Ok(SYS_VA + (first << 12) + (a & 0xFFF))
}

/// Emit the copy methods for `rects` (GPU VAs already resolved per endpoint base).
fn emit(push: &mut dyn FnMut(u32), sbase: usize, dbase: usize, rects: &[CeRect], slo: usize, dlo: usize) {
    for r in rects {
        let (s, d) = (sbase + (r.src_off - slo), dbase + (r.dst_off - dlo));
        push(pb_hdr(0, M_OFFSET_IN_UPPER, 4));
        push((s >> 32) as u32);
        push(s as u32);
        push((d >> 32) as u32);
        push(d as u32);
        push(pb_hdr(0, M_PITCH_IN, 4));
        push(r.src_pitch as u32);
        push(r.dst_pitch as u32);
        push(r.row_bytes as u32);
        push(r.rows as u32);
        push(pb_hdr(0, M_LAUNCH_DMA, 1));
        push(LAUNCH_PITCH_MULTI);
    }
}

/// The byte span `[lo, hi)` the rects touch on one side.
fn span(rects: &[CeRect], src: bool) -> (usize, usize) {
    let (mut lo, mut hi) = (usize::MAX, 0usize);
    for r in rects {
        let (o, p) = if src { (r.src_off, r.src_pitch) } else { (r.dst_off, r.dst_pitch) };
        lo = lo.min(o);
        hi = hi.max(o + (r.rows - 1) * p + r.row_bytes);
    }
    (lo, hi)
}

fn run(ch: &mut Chan, src: usize, dst: usize, rects: &[CeRect], budget_us: u64) -> Result<u64, CeErr> {
    let mut buf = [CeRect { src_off: 0, src_pitch: 0, dst_off: 0, dst_pitch: 0, row_bytes: 0, rows: 0 }; MAX_RECTS];
    let mut n = 0;
    for r in rects.iter().filter(|r| r.rows != 0 && r.row_bytes != 0) {
        if n == MAX_RECTS {
            return Err(CeErr::Unsupported);
        }
        buf[n] = *r;
        n += 1;
    }
    if n == 0 {
        return Ok(0);
    }
    let rects = &buf[..n];
    let (slo, shi) = span(rects, true);
    let (dlo, dhi) = span(rects, false);
    let sg = resolve(ch, src, slo, shi)?;
    let dg = resolve(ch, dst, dlo, dhi)?;
    let seq = submit(ch, |push| emit(push, sg, dg, rects, slo, dlo));
    let (ok, cyc) = wait_seq(seq, budget_us);
    if ok { Ok(cyc) } else { Err(CeErr::Timeout) }
}

// ---- M2 — the compositor's entry --------------------------------------------------------------------

/// The compositor's per-job budget: about one CPU present on flight 19 (present_us mean 3355).
pub const BUDGET_US: u64 = 4_000;

/// `true` while the boot self-test passed and no job has timed out.
pub fn armed() -> bool {
    ARMED.load(Acquire) && !DEMOTED.load(Acquire)
}

/// Why the compositor runs on the CPU (or `selftest-ok`), for `[wc] blitter=… reason=…`.
pub fn reason() -> &'static str {
    if DEMOTED.load(Acquire) {
        return "gpublit-demoted";
    }
    match VERDICT.load(Acquire) {
        0 => "no-takeover",
        1 => "selftest-ok",
        2 => "selftest-mismatch",
        3 => "selftest-timeout",
        _ => "selftest-refused",
    }
}

/// Copy `rects` from the surface at CPU address `src` to the surface at CPU address `dst` on the copy
/// engine, in ONE pushbuffer with ONE semaphore release, and wait for it (bounded by `budget_us`).
/// Endpoints inside BAR1 are VRAM (identity GPU VA); anything else is kernel RAM, mapped into the
/// sysmem window. Busy (another core holds the channel) is `Unavailable` at once — the caller's CPU
/// blitter takes the job this frame. A timeout demotes the CE for the rest of the boot.
pub fn copy_wait(src: usize, dst: usize, rects: &[CeRect], budget_us: u64) -> Result<u64, CeErr> {
    if !armed() {
        return Err(CeErr::Unavailable);
    }
    let Some(mut ch) = CHAN.try_lock() else {
        BUSY.fetch_add(1, Relaxed);
        return Err(CeErr::Unavailable);
    };
    match run(&mut ch, src, dst, rects, budget_us) {
        Ok(cyc) => {
            JOBS.fetch_add(1, Relaxed);
            GPU_CYC.fetch_add(cyc, Relaxed);
            Ok(cyc)
        }
        Err(CeErr::Timeout) => {
            TIMEOUTS.fetch_add(1, Relaxed);
            if !DEMOTED.swap(true, AcqRel) {
                serial_println!(
                    ":: GPUBLIT: demoted reason=timeout job={} budget_us={} gp_get={} gp_put={} sem={:08X} want={:08X} -> blitter=cpu ::",
                    JOBS.load(Relaxed) + 1, budget_us, vr(win() + userd_off() + USERD_GP_GET), ch.put, vr(win() + W_SEM), ch.seq
                );
            }
            Err(CeErr::Timeout)
        }
        Err(e) => {
            UNMAPPABLE.fetch_add(1, Relaxed);
            Err(e)
        }
    }
}

// ---- the self-test (boot + `tests gpublit`) ---------------------------------------------------------

#[derive(Clone, Copy)]
enum St {
    Ok,
    Mismatch(usize),
    Timeout,
    Refused(&'static str),
}

struct StOut {
    verdict: St,
    ce_us: u64,
    cpu_us: u64,
}

const T_W: usize = 64;
const T_H: usize = 64;
const T_DW: usize = 80; // destination row in pixels (a real 2D blit: pitches differ)
const T_DH: usize = 72;
const T_DX: usize = 8;
const T_DY: usize = 4;
const SELFTEST_BUDGET_US: u64 = 20_000;

/// 64x64 pattern from a HEAP buffer (the sysmem window — the hot path's source class) into VRAM scratch
/// at (8,4) of an 80x72 surface; the CPU blits the same source into a heap reference and into VRAM
/// scratch B (timing); the CE's destination is read back and compared byte for byte with the reference.
fn selftest(ch: &mut Chan) -> StOut {
    let refused = |why| StOut { verdict: St::Refused(why), ce_us: 0, cpu_us: 0 };
    let mut src = alloc::vec::Vec::new();
    let mut want = alloc::vec::Vec::new();
    if src.try_reserve_exact(T_W * T_H).is_err() || want.try_reserve_exact(T_DW * T_DH).is_err() {
        return refused("heap");
    }
    src.resize(T_W * T_H, 0u32);
    want.resize(T_DW * T_DH, 0u32);
    for y in 0..T_H {
        for x in 0..T_W {
            src[y * T_W + x] = 0xA500_0000 ^ ((x as u32) << 16) ^ ((y as u32) << 8) ^ ((x * 7 + y * 13) as u32 & 0xFF);
        }
    }
    let (dst_a, dst_b) = (win() + W_SCR_A, win() + W_SCR_B);
    // CPU blit #1 — the reference, in heap.
    for y in 0..T_H {
        want[(T_DY + y) * T_DW + T_DX..(T_DY + y) * T_DW + T_DX + T_W].copy_from_slice(&src[y * T_W..(y + 1) * T_W]);
    }
    // CPU blit #2 — the same copy into VRAM (BAR1), timed: the cost the CE replaces.
    let t0 = crate::arch::now_cycles();
    for y in 0..T_H {
        for x in 0..T_W {
            vw(dst_b + ((T_DY + y) * T_DW + T_DX + x) * 4, src[y * T_W + x]);
        }
    }
    let cpu_us = cyc_us(crate::arch::now_cycles().saturating_sub(t0));
    // Zero the CE's destination, then the CE blit.
    for i in 0..T_DW * T_DH {
        vw(dst_a + i * 4, 0);
    }
    let rect = [CeRect {
        src_off: 0,
        src_pitch: T_W * 4,
        dst_off: (T_DY * T_DW + T_DX) * 4,
        dst_pitch: T_DW * 4,
        row_bytes: T_W * 4,
        rows: T_H,
    }];
    let b1 = BAR1.load(Relaxed);
    let r = run(ch, src.as_ptr() as usize, b1 + dst_a, &rect, SELFTEST_BUDGET_US);
    let ce_us = match r {
        Ok(c) => cyc_us(c),
        Err(CeErr::Timeout) => SELFTEST_BUDGET_US,
        Err(_) => return StOut { verdict: St::Refused("unmappable"), ce_us: 0, cpu_us },
    };
    if r.is_err() {
        return StOut { verdict: St::Timeout, ce_us, cpu_us };
    }
    for i in 0..T_DW * T_DH {
        let got = vr(dst_a + i * 4);
        if got != want[i] {
            let x = (got ^ want[i]).trailing_zeros() as usize / 8;
            return StOut { verdict: St::Mismatch(i * 4 + x), ce_us, cpu_us };
        }
    }
    StOut { verdict: St::Ok, ce_us, cpu_us }
}

fn verdict_word(v: St) -> alloc::string::String {
    use alloc::format;
    match v {
        St::Ok => "ok".into(),
        St::Mismatch(n) => format!("mismatch@{}", n),
        St::Timeout => "timeout".into(),
        St::Refused(why) => format!("refused({})", why),
    }
}

fn print_line(st: &StOut, blitter: &str) {
    serial_println!(
        ":: GPUBLIT: selftest={} ce_us={} cpu_us={} -> blitter={} ::",
        verdict_word(st.verdict), st.ce_us, st.cpu_us, blitter
    );
}

/// The readbacks behind a non-ok verdict: did the PBDMA fetch (GP_GET), did the channel stay bound.
fn detail(st: &StOut) {
    let _ = st;
    let (w, c) = (win(), PFIFO_CHAN + CHID as usize * 8);
    serial_println!(
        "[gpublit] chid={} runlist={} win={:#x} vram_id={:#x}..{:#x} gp_get={} gp_put={} sem={:08X} chan={:08X}/{:08X} pmc={:08X} pbdma={:08X} rl={:08X}/{:08X}",
        CHID, CE_RL.load(Relaxed), w, VLO.load(Relaxed), VHI.load(Relaxed),
        vr(w + userd_off() + USERD_GP_GET), vr(w + userd_off() + USERD_GP_PUT), vr(w + W_SEM),
        mr(c), mr(c + 4), mr(PMC_ENABLE), mr(PMC_PBDMA_ENABLE), mr(RUNLIST_BASE), mr(RUNLIST_SUBMIT)
    );
    detail_host();
    detail_walls();
    detail_decode();
}

// ---- M3 — the PBDMA / channel path read again from the top, every word decoded (DRIVERS-METHOD §7) --------------

/// GPUBLIT3 M3, lifted by KEPLERGR (B421) into `kepler_fifo::host::pfifo_decode` — one decoder for both legs; the line
/// is now `[kfifo] decode chid=2 ...` (was `[gpublit] decode`). The CE's own MMU fault unit is CE0 0x15 + idx.
fn detail_decode() {
    let (eng, rl, idx) = (CE_ENG.load(Relaxed), CE_RL.load(Relaxed), CE_IDX.load(Relaxed));
    let ce_unit = if idx < 2 { kf::FAULT_UNIT_CE0 + idx } else { kf::FAULT_UNIT_CE0 };
    kf::pfifo_decode(BAR0.load(Relaxed), CHID, eng, rl, ce_unit);
}

/// GPUBLIT3 M1: every wall this boot applied, pre-image -> readback, so flight 26 tells R3..R9 apart (gpublit3.md §3).
/// `pte_hi` is the first sysmem PTE's dw1 (the self-test's source page); `ramfc_*` read the instance block back.
fn detail_walls() {
    let (w, inst) = (win(), win() + W_INST);
    let mut pb = alloc::string::String::new();
    let rl = CE_RL.load(Relaxed);
    for i in 0..4usize {
        if rl < 32 && mr(PB_RUNM + i * 4) & (1 << rl) != 0 {
            let _ = core::fmt::Write::write_fmt(
                &mut pb,
                format_args!(" pb{}_13c={:08X}->{:08X}", i, PB13C_PRE[i].load(Relaxed), mr(PB_13C + i * PB_STRIDE)),
            );
        }
    }
    serial_println!(
        "[gpublit] walls bind_pre={:02X} intr_pre={:08X} bind_post={:02X} intr_post={:08X} r2254={:08X}->{:08X} r2a04={:08X}->{:08X} r2630={:08X}->{:08X} pte_hi={} ramfc_0c={:08X} ramfc_94={:08X} ramfc_e4={:08X} ramfc_f8={:08X} ramfc_fc={:08X} fb_page={} pde_span_mb={} utab={:#x}{}",
        BIND_PRE.load(Relaxed) & 0xff, INTR_PRE.load(Relaxed), BIND_POST.load(Relaxed) & 0xff, INTR_POST.load(Relaxed),
        R2254_PRE.load(Relaxed), mr(USERD_BAR1), R2A04_PRE.load(Relaxed), mr(R2A04), R2630_PRE.load(Relaxed), mr(SCHED_DISABLE),
        vr(w + W_PTS + 4), vr(inst + 0x0C), vr(inst + 0x94), vr(inst + 0xE4), vr(inst + 0xF8), vr(inst + 0xFC),
        if FB_PAGE.load(Relaxed) & 1 != 0 { 16 } else { 17 }, SPAN.load(Relaxed) >> 20, w + W_UTAB, pb
    );
}

/// GPUBLIT2 M2: whether the host fetched at all, and who refused it. Reads only.
/// PFIFO_INTR 0x2100 (nouveau gk104.c:658), SCHED_ERROR 0x256c (:624), runlist event 0x2a00 (:644),
/// 0x2a04 (:740) and the USERD BAR1 base 0x2254 (:749) — both written by nouveau at fifo init, by us
/// never —, the CE's engine status 0x2640 + eng*8 (:206), the runlist's pending word 0x2284 + rl*8 (:426);
/// the channel's RAMFC GP_PUT/GP_GET/GP_FETCH at inst +0x00/+0x14/+0x50 (open-gpu-doc gv100
/// dev_ram.ref.txt:448/453/468, the same layout as gk104's +0x08 USERD and +0x48 GP_BASE); per PBDMA i
/// (0x204's bits): its runlist mask 0x2390 + i*4 (gk104.c:392), CHANNEL 0x040120, INTR_0 0x040108
/// (gf100.c:315-318) and live GP_GET 0x040014 / GP_PUT 0x040000 (gv100 dev_pbdma.ref.txt:425/468),
/// stride 0x2000.
fn detail_host() {
    let opt = |v: u32| -> alloc::string::String {
        if v == NONE { "-".into() } else { alloc::format!("{}", v) }
    };
    let (idx, eng, rl, rst) = (CE_IDX.load(Relaxed), CE_ENG.load(Relaxed), CE_RL.load(Relaxed), CE_RESET.load(Relaxed));
    let commit = COMMIT_US.load(Relaxed);
    let inst = win() + W_INST;
    let eng_stat = if eng < 32 { alloc::format!("{:08X}", mr(0x2640 + eng as usize * 8)) } else { "-".into() };
    let rl_pend = if rl < 16 { alloc::format!("{:08X}", mr(RUNLIST_INFO + rl as usize * 8)) } else { "-".into() };
    let mut pb = alloc::string::String::new();
    let en = mr(PMC_PBDMA_ENABLE);
    for i in 0..4usize {
        if en & (1 << i) == 0 {
            continue;
        }
        let b = 0x04_0000 + i * 0x2000;
        let _ = core::fmt::Write::write_fmt(
            &mut pb,
            format_args!(
                " pb{}={:08X}/{:08X}/{:08X}/{}/{}",
                i, mr(0x2390 + i * 4), mr(b + 0x120), mr(b + 0x108), mr(b + 0x14), mr(b)
            ),
        );
    }
    serial_println!(
        "[gpublit] host ce=ce{} eng={} rl={} reset={} pmc_pre={:08X} rl_pend={} commit_us={} pfifo_intr={:08X} sched={:08X} rl_ev={:08X} r2a04={:08X} userd_bar1={:08X} eng_stat={} ramfc_put={} ramfc_get={} ramfc_fetch={}{} (pbN=runm/chan/intr0/get/put)",
        opt(idx), opt(eng), opt(rl), opt(rst), PMC_PRE.load(Relaxed), rl_pend,
        if commit == NONE { "stuck".into() } else { opt(commit) },
        mr(0x2100), mr(0x256c), mr(0x2a00), mr(0x2a04), mr(0x2254), eng_stat,
        vr(inst), vr(inst + 0x14), vr(inst + 0x50), pb
    );
}

// ---- M3 — `tests gpublit` ---------------------------------------------------------------------------

/// Re-run the 64x64 self-test on the bound channel (never re-arms a CPU-selected boot), print the M1
/// line, the counters, and the verdict: PASS iff the boot armed the CE and the rerun matched.
pub fn tests_gpublit() {
    let boot = reason();
    if !BUILT.load(Acquire) {
        let why = if VERDICT.load(Acquire) == 0 { "no-takeover" } else { *REFUSED.lock() };
        serial_println!(":: GPUBLIT: selftest=refused({}) ce_us=0 cpu_us=0 -> blitter=cpu ::", why);
        serial_println!(":: GPUBLIT-TEST: rerun=refused boot={} -> FAIL ::", boot);
        return;
    }
    let t0 = crate::arch::now_cycles();
    let lim = tsc_hz() / 20; // 50 ms for a present to release the channel
    let mut g = None;
    while g.is_none() && crate::arch::now_cycles().saturating_sub(t0) < lim {
        g = CHAN.try_lock();
        core::hint::spin_loop();
    }
    let Some(mut ch) = g else {
        serial_println!(":: GPUBLIT-TEST: rerun=busy boot={} -> FAIL ::", boot);
        return;
    };
    let st = selftest(&mut ch);
    let (put, seq) = (ch.put, ch.seq);
    let sys_used_kb = ch.sys_next * 4;
    drop(ch);
    print_line(&st, if armed() { "gpu" } else { "cpu" });
    if !matches!(st.verdict, St::Ok) {
        detail(&st);
    }
    let jobs = JOBS.load(Relaxed);
    serial_println!(
        "[gpublit] boot={} jobs={} gpu_us_mean={} busy={} timeouts={} unmappable={} demoted={} sysmaps={} sys_used_kb={} put={} seq={}",
        boot, jobs, if jobs == 0 { 0 } else { cyc_us(GPU_CYC.load(Relaxed)) / jobs }, BUSY.load(Relaxed),
        TIMEOUTS.load(Relaxed), UNMAPPABLE.load(Relaxed), DEMOTED.load(Relaxed) as u8, SYSMAPS.load(Relaxed), sys_used_kb, put, seq
    );
    let pass = ARMED.load(Acquire) && matches!(st.verdict, St::Ok);
    serial_println!(":: GPUBLIT-TEST: rerun={} boot={} -> {} ::", verdict_word(st.verdict), boot, if pass { "PASS" } else { "FAIL" });
}
