//! KCOMP — the compositor's one blitter interface: the CPU today, the Kepler copy engine when KBLIT lands.
//! CHARTER: Kernel — wm
//!
//! FINDING (Peter 2026-10-04, "what about the GPUs?"): every pixel on the rMBP glass is CPU-composited.
//! The window -> scan-out present was a row loop of `copy_nonoverlapping` inlined in
//! `wm::stage_window` (cached-RAM staging band -> write-combined BAR1), fanned over the APs by WCPAR,
//! with no interface under it — so a GPU copy engine had nowhere to plug in. Census and measurements:
//! `docs/dev/evidence/rmbp-1005/KCOMP.md` (flight 19: present_us mean 3355 per pass, the CE-LADDER R0
//! stop line is 300).
//!
//! MECHANISM. [`Blitter`] over the compositor's OWN surface descriptor ([`FrameBuffer`] plus where the
//! bytes live, [`Mem`]) — no new pixel type. [`CpuBlitter`] IS the code the compositor ran before this
//! arc, moved and not rewritten: WCPAR's `par_blit` band fan-out first (x86 + `wc`), the per-row
//! `wm::blit_traced` loop when it declines — so the CPU output is byte-identical and the work is the
//! same work. [`GpuBlitter`] is the stub KBLIT fills (see its `blit`). [`run_on`] holds the FALLBACK
//! RULE: any [`BlitErr`] from a non-CPU blitter re-runs the whole job on the CPU blitter in the same
//! frame and counts `gpu_fallback` — the glass never goes dark because an engine stalled (A1 /
//! BAR1WEDGE: a wedged engine must not take the panel with it).
//!
//! SELECTION. `UNAOS_WC_BLITTER=gpu` arms feature `wc_gpublit` (default OFF, x86 only). [`ignite`] runs
//! once, at the first composite, asks [`GpuBlitter::probe`], and prints
//! `[wc] blitter=<cpu|gpu> reason=<…>` once. With the feature off, or on with no KBLIT channel, the
//! answer is cpu.
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering::*};

use super::FrameBuffer;

/// Where a surface's bytes live — what decides whether an engine can address it and what a CPU read
/// of it costs. `Scanout` is the panel memory: on the rMBP the BAR1 aperture into GK107 VRAM, mapped
/// write-combined (CPU stores stream, CPU READS are uncached, ~1.7 us a probe in flight 19); on the Pi
/// and the Orin it is DRAM the display controller scans.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mem {
    Ram,
    Scanout,
}

/// A blit endpoint: the compositor's existing surface descriptor plus its memory class.
#[derive(Clone, Copy)]
pub struct Surface {
    pub fb: FrameBuffer,
    pub mem: Mem,
}

impl Surface {
    pub fn ram(fb: FrameBuffer) -> Self {
        Self { fb, mem: Mem::Ram }
    }
    pub fn scanout(fb: FrameBuffer) -> Self {
        Self { fb, mem: Mem::Scanout }
    }
}

/// One rectangle: `w x h` pixels from `(sx, sy)` in the source to `(dx, dy)` in the destination.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rect {
    pub sx: usize,
    pub sy: usize,
    pub dx: usize,
    pub dy: usize,
    pub w: usize,
    pub h: usize,
}

/// `Copy` is a straight move (the compositor's present). `CopyAlpha` is src-over with the 4th byte of
/// a 4-byte pixel as alpha; it is not on any hot path and a GPU implementation of it must be
/// all-or-nothing, because the fallback re-runs the WHOLE job and a blend is not idempotent.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Op {
    Copy,
    CopyAlpha,
}

pub struct BlitJob<'a> {
    pub src: Surface,
    pub dst: Surface,
    pub rects: &'a [Rect],
    pub op: Op,
}

/// Completion token. The CPU blitter is synchronous (WCPAR joins every band before `par_blit`
/// returns), so it always hands back [`Fence::DONE`]; a GPU fence is the CE's semaphore sequence.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Fence(pub u64);

impl Fence {
    pub const DONE: Fence = Fence(0);
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BlitErr {
    /// No engine (the stub, or KBLIT's channel not bound).
    Unavailable,
    /// The job asks for something this blitter cannot do (format mismatch, an unaddressable source).
    Unsupported,
    /// A rectangle falls outside a surface.
    Bounds,
    /// The fence did not signal within `timeout_us` — an engine stall; the frame falls back.
    Timeout,
}

impl BlitErr {
    pub fn name(self) -> &'static str {
        match self {
            BlitErr::Unavailable => "unavailable",
            BlitErr::Unsupported => "unsupported",
            BlitErr::Bounds => "bounds",
            BlitErr::Timeout => "timeout",
        }
    }
}

pub trait Blitter: Sync {
    fn blit(&self, job: &BlitJob) -> Result<Fence, BlitErr>;
    fn wait(&self, f: Fence, timeout_us: u32) -> Result<(), BlitErr>;
    fn name(&self) -> &'static str;
}

/// One frame's budget at 60 Hz: the longest a present may wait on an engine before it falls back.
pub const WAIT_US: u32 = 16_667;

// ---- the CPU blitter -------------------------------------------------------------------------------

pub struct CpuBlitter;

/// Validated geometry of one rect against both surfaces: (src byte offset, src pitch, dst byte offset,
/// dst pitch, row bytes, src span length).
fn geom(job: &BlitJob, r: &Rect, bpp: usize) -> Result<(usize, usize, usize, usize, usize, usize), BlitErr> {
    let (si, di) = (job.src.fb.info(), job.dst.fb.info());
    if r.w == 0 || r.h == 0 {
        return Ok((0, 0, 0, 0, 0, 0));
    }
    if r.sx + r.w > si.width || r.sy + r.h > si.height || r.dx + r.w > di.width || r.dy + r.h > di.height {
        return Err(BlitErr::Bounds);
    }
    let (spitch, dpitch, rb) = (si.stride * bpp, di.stride * bpp, r.w * bpp);
    let s0 = (r.sy * si.stride + r.sx) * bpp;
    let span = (r.h - 1) * spitch + rb;
    if s0 + span > job.src.fb.len() || job.src.fb.base_addr() == 0 {
        return Err(BlitErr::Bounds);
    }
    let d0 = (r.dy * di.stride + r.dx) * bpp;
    Ok((s0, spitch, d0, dpitch, rb, span))
}

impl Blitter for CpuBlitter {
    fn blit(&self, job: &BlitJob) -> Result<Fence, BlitErr> {
        let (si, di) = (job.src.fb.info(), job.dst.fb.info());
        let bpp = di.bytes_per_pixel;
        if bpp == 0 || si.bytes_per_pixel != bpp || si.pixel_format != di.pixel_format {
            return Err(BlitErr::Unsupported);
        }
        if job.op == Op::CopyAlpha && bpp != 4 {
            return Err(BlitErr::Unsupported);
        }
        for r in job.rects {
            let (s0, spitch, d0, dpitch, rb, span) = geom(job, r, bpp)?;
            if rb == 0 {
                continue;
            }
            // SAFETY: `geom` bounded `s0 + span` by the source surface's mapped length; the surface
            // outlives the job (the caller holds it across `blit` + `wait`).
            let src = unsafe { core::slice::from_raw_parts((job.src.fb.base_addr() + s0) as *const u8, span) };
            if job.op == Op::CopyAlpha {
                copy_alpha(&job.dst.fb, src, spitch, d0, dpitch, r.w, r.h);
                continue;
            }
            // WCPAR — the band fan-out is this blitter's internal strategy, exactly where the inline
            // present called it (contiguous source rows only: `par_blit` takes one row pitch).
            #[cfg(all(target_arch = "x86_64", feature = "wc"))]
            if spitch == rb && super::wcpar::par_blit(&job.dst.fb, src, rb, d0, dpitch, r.h) {
                continue;
            }
            for y in 0..r.h {
                super::wm::kcomp_row(&job.dst.fb, d0 + y * dpitch, &src[y * spitch..y * spitch + rb], r.dy + y);
            }
        }
        Ok(Fence::DONE)
    }

    fn wait(&self, _f: Fence, _timeout_us: u32) -> Result<(), BlitErr> {
        Ok(())
    }

    fn name(&self) -> &'static str {
        "cpu"
    }
}

/// Src-over, 4-byte pixels, alpha in byte 3. Reads the destination (on a `Scanout` surface that is an
/// uncached BAR1 read on the rMBP — the reason this op is not on the hot path).
fn copy_alpha(dst: &FrameBuffer, src: &[u8], spitch: usize, d0: usize, dpitch: usize, w: usize, h: usize) {
    let (base, len) = (dst.base_addr(), dst.len());
    if base == 0 || d0 + (h - 1) * dpitch + w * 4 > len {
        return;
    }
    for y in 0..h {
        for x in 0..w {
            let s = &src[y * spitch + x * 4..y * spitch + x * 4 + 4];
            let a = s[3] as u32;
            if a == 0 {
                continue;
            }
            let p = (base + d0 + y * dpitch + x * 4) as *mut u8;
            for c in 0..3 {
                // SAFETY: bounded above against the destination's mapped length.
                unsafe {
                    let d = core::ptr::read_volatile(p.add(c)) as u32;
                    let v = (s[c] as u32 * a + d * (255 - a)) / 255;
                    core::ptr::write_volatile(p.add(c), v as u8);
                }
            }
        }
    }
}

// ---- the GPU blitter (the KBLIT seam) ----------------------------------------------------------------

pub struct GpuBlitter;

impl GpuBlitter {
    /// `Ok` only when KBLIT's copy-engine channel is bound (its `tests kblit` PASS state). Until then the
    /// reason the compositor selects the CPU, as printed on `[wc] blitter=cpu reason=`.
    pub fn probe(&self) -> Result<(), &'static str> {
        #[cfg(all(target_arch = "x86_64", feature = "wc_gpublit"))]
        {
            // KBLIT fills: `crate::drivers::gpu::kepler_ce::channel_ready()` -> Ok(()).
            Err("kblit-channel-absent")
        }
        #[cfg(not(all(target_arch = "x86_64", feature = "wc_gpublit")))]
        {
            Err("feature-off")
        }
    }
}

impl Blitter for GpuBlitter {
    fn blit(&self, job: &BlitJob) -> Result<Fence, BlitErr> {
        // THE SEAM KBLIT FILLS — once `tests kblit` proves a CE copy with checksum, this body becomes
        //     crate::drivers::gpu::kepler_ce::blit(job)        // brief: `kepler::ce::blit(job) -> Fence`
        // which builds the CE method stream for each rect (src/dst GPU VA, pitches, w*bpp x h) and
        // returns the semaphore sequence as the Fence. A source the channel's VM cannot address
        // (`Mem::Ram` with no sysmem mapping of the staging band) is `BlitErr::Unsupported` — a
        // fallback, never a dark frame.
        let _ = job;
        Err(BlitErr::Unavailable)
    }

    fn wait(&self, f: Fence, timeout_us: u32) -> Result<(), BlitErr> {
        // KBLIT fills: poll the CE semaphore for `f.0` up to `timeout_us`; expiry -> `BlitErr::Timeout`.
        let _ = (f, timeout_us);
        Err(BlitErr::Unavailable)
    }

    fn name(&self) -> &'static str {
        "gpu"
    }
}

pub static CPU: CpuBlitter = CpuBlitter;
pub static GPU: GpuBlitter = GpuBlitter;

static IGNITED: AtomicBool = AtomicBool::new(false);
static SEL_GPU: AtomicBool = AtomicBool::new(false);
static GPU_FALLBACK: AtomicU64 = AtomicU64::new(0);
static BLITS: AtomicU64 = AtomicU64::new(0);
static BLIT_CYC: AtomicU64 = AtomicU64::new(0);

/// Compositor ignition (lazily, at the first composite): pick the blitter and say why, once.
pub fn ignite() {
    if IGNITED.swap(true, AcqRel) {
        return;
    }
    let reason = match GPU.probe() {
        Ok(()) => {
            SEL_GPU.store(true, Release);
            "kblit-channel-bound"
        }
        Err(r) => r,
    };
    serial_println!("[wc] blitter={} reason={}", selected().name(), reason);
}

pub fn selected() -> &'static dyn Blitter {
    if SEL_GPU.load(Acquire) {
        &GPU
    } else {
        &CPU
    }
}

/// Fallbacks taken since boot (the CPU re-ran a job a non-CPU blitter refused or stalled on).
pub fn gpu_fallbacks() -> u64 {
    GPU_FALLBACK.load(Relaxed)
}

/// Run `job` on `b` and wait for it. THE FALLBACK RULE: any error from a blitter that is not the CPU
/// re-runs the whole job on the CPU blitter and counts `gpu_fallback`. Returns the final result and
/// whether the fallback was taken.
pub fn run_on(b: &dyn Blitter, job: &BlitJob) -> (Result<(), BlitErr>, bool) {
    match b.blit(job).and_then(|f| b.wait(f, WAIT_US)) {
        Ok(()) => (Ok(()), false),
        Err(_) if b.name() != CPU.name() => {
            GPU_FALLBACK.fetch_add(1, Relaxed);
            (CPU.blit(job).and_then(|f| CPU.wait(f, WAIT_US)), true)
        }
        Err(e) => (Err(e), false),
    }
}

/// `now_cycles` rate: the calibrated TSC on x86, CNTFRQ on aarch64 (the `strip::cycles_to_us` rule,
/// restated because `strip` is not compiled on every build that composites).
pub fn cycles_to_us(dt: u64) -> u64 {
    #[cfg(target_arch = "x86_64")]
    let hz = crate::arch::apic::tsc_hz();
    #[cfg(target_arch = "aarch64")]
    let hz = crate::arch::timer::cntfrq();
    let hz = if hz == 0 { 1_250_000_000 } else { hz };
    dt.saturating_mul(1_000_000) / hz
}

/// M3 — `stage_window`'s no-clip present: band `layer` (cached RAM, `w x h`, pitch `w`) to the panel at
/// `(dx, dy)`, through the selected blitter. True when every row reached the panel; false leaves the
/// caller's inline row loop to do it (the second safety net under the fallback rule).
#[allow(clippy::too_many_arguments)]
pub fn present_band(win: u32, fb: &FrameBuffer, layer: &FrameBuffer, dx: usize, dy: usize, w: usize, h: usize) -> bool {
    ignite();
    let rect = [Rect { sx: 0, sy: 0, dx, dy, w, h }];
    let job = BlitJob { src: Surface::ram(*layer), dst: Surface::scanout(*fb), rects: &rect, op: Op::Copy };
    let t0 = crate::arch::now_cycles();
    let (r, fell) = run_on(selected(), &job);
    let cyc = crate::arch::now_cycles().saturating_sub(t0);
    BLITS.fetch_add(1, Relaxed);
    BLIT_CYC.fetch_add(cyc, Relaxed);
    #[cfg(feature = "witness")]
    super::wcg::blit_note(win, cyc, fell);
    let _ = (win, fell);
    r.is_ok()
}

// ---- M4 — `tests blitter` (never at boot, R80) -----------------------------------------------------

/// The census row count and the hot path, as `docs/dev/evidence/rmbp-1005/KCOMP.md` §M1 names them.
pub const CENSUS_PATHS: usize = 10;
pub const HOT_PATH: &str = "window-present";

/// `tests blitter` registration, once.
pub fn ensure_tests() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, AcqRel) {
        crate::tests::register("blitter", selftest);
    }
}

fn fnv(b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &x in b {
        h = (h ^ x as u64).wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn heap(n: usize) -> Option<alloc::vec::Vec<u8>> {
    let mut v = alloc::vec::Vec::new();
    v.try_reserve_exact(n).ok()?;
    v.resize(n, 0);
    Some(v)
}

/// A 512x512 composite into a 640x520 destination at (8, 4): the OLD inline present (WCPAR's
/// `par_blit`, else one `blit` per row — the code `stage_window` ran before M3) against the
/// [`CpuBlitter`], then the [`GpuBlitter`] through [`run_on`] — `Unavailable`, the fallback taken and
/// counted, the output equal. Heap buffers on both sides, so the panel is never touched.
pub fn selftest() {
    const W: usize = 512;
    const H: usize = 512;
    const DW: usize = 640;
    const DH: usize = 520;
    const BPP: usize = 4;
    const DX: usize = 8;
    const DY: usize = 4;
    let (Some(mut src), Some(d_inline), Some(d_cpu), Some(d_gpu)) = (heap(W * H * BPP), heap(DW * DH * BPP), heap(DW * DH * BPP), heap(DW * DH * BPP)) else {
        serial_println!(":: KCOMP: heap short for the 512x512 fixture -> SKIP ::");
        return;
    };
    for y in 0..H {
        for x in 0..W {
            let o = (y * W + x) * BPP;
            src[o] = (x * 7 + y) as u8;
            src[o + 1] = (y * 13 ^ x) as u8;
            src[o + 2] = (x ^ y) as u8;
            src[o + 3] = 0xFF;
        }
    }
    let fmt = unaos_boot_info::PixelFormat::Bgr;
    let mk = |base: usize, len: usize, w: usize, h: usize| {
        let mut f = FrameBuffer::new();
        f.init(base, len, unaos_boot_info::FrameBufferInfo { width: w, height: h, stride: w, bytes_per_pixel: BPP, pixel_format: fmt });
        f
    };
    let sfb = mk(src.as_ptr() as usize, src.len(), W, H);
    let (fi, fc, fg) = (
        mk(d_inline.as_ptr() as usize, d_inline.len(), DW, DH),
        mk(d_cpu.as_ptr() as usize, d_cpu.len(), DW, DH),
        mk(d_gpu.as_ptr() as usize, d_gpu.len(), DW, DH),
    );
    let blank = fnv(&d_inline);

    // The OLD inline path, verbatim in shape: par_blit over the contiguous band, else the row loop.
    let (rb, drow, d0) = (W * BPP, DW * BPP, (DY * DW + DX) * BPP);
    let t0 = crate::arch::now_cycles();
    #[cfg(all(target_arch = "x86_64", feature = "wc"))]
    let par = super::wcpar::par_blit(&fi, &src, rb, d0, drow, H);
    #[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
    let par = false;
    if !par {
        for y in 0..H {
            fi.blit(d0 + y * drow, &src[y * rb..(y + 1) * rb]);
        }
    }
    let inline_us = cycles_to_us(crate::arch::now_cycles().saturating_sub(t0));

    let rect = [Rect { sx: 0, sy: 0, dx: DX, dy: DY, w: W, h: H }];
    let job_c = BlitJob { src: Surface::ram(sfb), dst: Surface::ram(fc), rects: &rect, op: Op::Copy };
    let t1 = crate::arch::now_cycles();
    let (rc, fell_c) = run_on(&CPU, &job_c);
    let cpu_us = cycles_to_us(crate::arch::now_cycles().saturating_sub(t1));

    let fb0 = gpu_fallbacks();
    let gpu_direct = GPU.blit(&job_c);
    let job_g = BlitJob { src: Surface::ram(sfb), dst: Surface::ram(fg), rects: &rect, op: Op::Copy };
    let t2 = crate::arch::now_cycles();
    let (rg, fell_g) = run_on(&GPU, &job_g);
    let gpu_path_us = cycles_to_us(crate::arch::now_cycles().saturating_sub(t2));
    let counted = gpu_fallbacks().wrapping_sub(fb0);

    let (ci, cc, cg) = (fnv(&d_inline), fnv(&d_cpu), fnv(&d_gpu));
    let gpu_word = match gpu_direct {
        Err(BlitErr::Unavailable) => "unavailable",
        Err(e) => e.name(),
        Ok(_) => "available",
    };
    // Bounds refusal: a rect one row past the source must be refused, not clipped silently.
    let bad = [Rect { sx: 0, sy: 1, dx: 0, dy: 0, w: W, h: H }];
    let bounds_ok = CPU.blit(&BlitJob { src: Surface::ram(sfb), dst: Surface::ram(fc), rects: &bad, op: Op::Copy }) == Err(BlitErr::Bounds);
    let cpu_eq = rc.is_ok() && !fell_c && cc == ci && ci != blank;
    let fallback_ok = rg.is_ok() && fell_g && counted == 1 && cg == ci;
    serial_println!(
        "[kcomp] w={} h={} inline_us={} cpu_us={} gpu_path_us={} cks_inline={:#018x} cks_cpu={:#018x} cks_gpu={:#018x} blank={:#018x} par={} gpu_direct={} fallback_counted={} bounds_refused={}",
        W, H, inline_us, cpu_us, gpu_path_us, ci, cc, cg, blank, par as u8, gpu_word, counted, bounds_ok as u8
    );
    let ok = cpu_eq && fallback_ok && bounds_ok && gpu_word == "unavailable";
    serial_println!(
        ":: KCOMP: blitter={} census={} hot={} cpu_us={} gpu={} fallback_ok={} -> {} ::",
        selected().name(),
        CENSUS_PATHS,
        HOT_PATH,
        cpu_us,
        gpu_word,
        fallback_ok as u8,
        if ok { "PASS" } else { "FAIL" }
    );
}
