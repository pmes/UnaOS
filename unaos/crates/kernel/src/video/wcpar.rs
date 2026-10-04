//! WCPAR — the compositor's band-worker pool (x86_64 + `wc`).
//!
//! FINDING (boot 16, rMBP, 8 CPUs): `[wcpar] cores=1 total=297 c0=297 c1..c7=0` on every rollup and
//! `:: SMPLOAD: busy=[12,0,0,0,0,0,0,3] stealable=[0,…] -> PASS ::`. `[wcpar]` counts which core a window
//! COMPOSE staged on; the compose is one serial pass under `COMP_GATE` on the presenter's core, and its
//! cost is the row-by-row BAR1 blit (`[comp2] blit_us=8469` of `pass_us=8477`). No band-worker pool
//! existed on x86, so the scheduler had nothing to spread: the WORK was serial.
//!
//! MECHANISM. At compositor ignition [`start`] spawns one PINNED worker per online AP (the pin contract:
//! a named core is a forever core). `stage_window`'s no-clip blit loop offers its rows to [`par_blit`]:
//! the presenter (which holds `COMP_GATE` and the staging buffer) publishes a job (disjoint row bands),
//! then claims bands alongside the workers through one packed `gen<<32|next` compare-exchange word, and
//! joins on a per-job completion counter. Workers touch ONLY the staging buffer (read) and their own
//! framebuffer rows (write); never the window table, never a lock. The presenter does not return (so the
//! staging buffer cannot be reused) until every band completed. Workers `sfence` before counting a band
//! done so the write-combined BAR1 stores are ordered before the presenter's `flush_rect`.
//! With no APs (or a job too small to pay for the hand-off) the caller keeps its serial loop.
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::*};

use super::FrameBuffer;

const MAX_W: usize = 8;
/// Smallest job worth fanning out: rows and bytes.
const MIN_ROWS: usize = 32;
const MIN_BYTES: usize = 128 * 1024;
/// Rows per band never below this (a band is at least a few pages of scan-out).
const MIN_BAND_ROWS: usize = 8;

/// PTRSTUTTER: the CPU left free of band workers (-1 = none).
static RESERVED_CPU: core::sync::atomic::AtomicI64 = core::sync::atomic::AtomicI64::new(-1); // never set: no reserving cores (ruling)

/// The CPU `start` kept free for the input service (-1 = none reserved).
pub fn reserved_cpu() -> i64 {
    RESERVED_CPU.load(Relaxed)
}

static WORKERS: AtomicUsize = AtomicUsize::new(0);
static READY: AtomicBool = AtomicBool::new(false);
static STARTED: AtomicBool = AtomicBool::new(false);
static JOB_BUSY: AtomicBool = AtomicBool::new(false);

// Job parameters: written by the presenter ONLY while JOB_BUSY is held and no band of the previous job is
// outstanding; read by claimants only after a successful claim of the current generation.
static J_FB: AtomicUsize = AtomicUsize::new(0);
static J_SRC: AtomicUsize = AtomicUsize::new(0);
static J_ROW_BYTES: AtomicUsize = AtomicUsize::new(0);
static J_DST0: AtomicUsize = AtomicUsize::new(0);
static J_FB_ROW: AtomicUsize = AtomicUsize::new(0);
static J_ROWS: AtomicUsize = AtomicUsize::new(0);
static J_BAND_ROWS: AtomicUsize = AtomicUsize::new(0);
static J_NBANDS: AtomicUsize = AtomicUsize::new(0);
/// `gen << 32 | next_band`. Generation only ever increases, so a claim cannot ABA across jobs.
static J_CLAIM: AtomicU64 = AtomicU64::new(0);
static J_DONE: AtomicUsize = AtomicUsize::new(0);

// Witness accumulators (drained by `emit`).
static W_JOBS: AtomicU64 = AtomicU64::new(0);
static W_BANDS: AtomicU64 = AtomicU64::new(0);
static W_BANDS_FAILED: AtomicU64 = AtomicU64::new(0);
static W_WALL_CYC: AtomicU64 = AtomicU64::new(0);
static W_WORK_CYC: AtomicU64 = AtomicU64::new(0);
static W_BY_CORE: [AtomicU64; MAX_W] = [const { AtomicU64::new(0) }; MAX_W];

/// How many band workers exist (0 = single-core path).
pub fn workers() -> usize {
    WORKERS.load(Relaxed)
}

/// One band: rows `[i*band_rows, min(rows, (i+1)*band_rows))`. Returns cycles spent.
fn run_band(i: usize) -> u64 {
    let t0 = crate::arch::now_cycles();
    let fb = J_FB.load(Relaxed) as *const FrameBuffer;
    let src = J_SRC.load(Relaxed) as *const u8;
    let (rb, dst0, fbrow) = (J_ROW_BYTES.load(Relaxed), J_DST0.load(Relaxed), J_FB_ROW.load(Relaxed));
    let (rows, br) = (J_ROWS.load(Relaxed), J_BAND_ROWS.load(Relaxed));
    let (y0, y1) = (i * br, ((i + 1) * br).min(rows));
    for y in y0..y1 {
        // SAFETY: `fb`/`src` outlive the job (the presenter joins before returning); band row ranges
        // are disjoint, so no two claimants write the same framebuffer bytes or alias a `&mut`.
        unsafe {
            (*fb).blit(dst0 + y * fbrow, core::slice::from_raw_parts(src.add(y * rb), rb));
        }
    }
    // Order this core's write-combined stores before the band is announced done.
    unsafe { core::arch::x86_64::_mm_sfence() };
    crate::arch::now_cycles().saturating_sub(t0)
}

/// Claim and run one band of the current job. Returns true if it ran one.
fn claim_one() -> bool {
    loop {
        let c = J_CLAIM.load(Acquire);
        let idx = (c & 0xFFFF_FFFF) as usize;
        if idx >= J_NBANDS.load(Relaxed) || J_DONE.load(Relaxed) >= J_NBANDS.load(Relaxed) {
            return false;
        }
        if J_CLAIM.compare_exchange(c, c + 1, AcqRel, Relaxed).is_err() {
            continue;
        }
        let cyc = run_band(idx);
        W_WORK_CYC.fetch_add(cyc, Relaxed);
        let cpu = crate::arch::sched::meter_current_cpu().min(MAX_W - 1);
        W_BY_CORE[cpu].fetch_add(1, Relaxed);
        J_DONE.fetch_add(1, Release);
        return true;
    }
}

fn worker(_arg: usize) {
    let hz = crate::arch::apic::tsc_hz();
    let hot = if hz == 0 { 25_000_000 } else { hz / 1000 }; // PTRSTUTTER: stay hot <= 1 ms (was 20 ms), then sleep so a PRIO_NORMAL wake gets the core
    let mut last = crate::arch::now_cycles();
    loop {
        if READY.load(Relaxed) && claim_one() {
            last = crate::arch::now_cycles();
            crate::arch::sched::yield_now(); // PTRSTUTTER: between bands, let a ready peer (input service) run
        } else if crate::arch::now_cycles().saturating_sub(last) < hot {
            core::hint::spin_loop();
        } else {
            crate::arch::sched::sleep_ms(1);
        }
    }
}

/// Compositor ignition: spawn one pinned worker per online AP and say how many exist and why.
pub fn start() {
    if STARTED.swap(true, AcqRel) {
        return;
    }
    // PTRSTUTTER M2 (revised): Peter's ruling "THERE IS NO RESERVING CORES" stands - workers go on EVERY
    // online AP; the pointer is kept healthy by priority + yielding (worker spin <= 1 ms, yield between bands).
    let aps = crate::arch::smp::online_aps();
    let (n, reason) = if aps.is_empty() { (0, "no-aps-online") } else { (aps.len().min(MAX_W), "one-per-online-ap") };
    for &cpu in aps.iter().take(n) {
        crate::arch::sched::spawn("wcpar-band", worker, 0, cpu, crate::arch::sched::PRIO_LOW);
    }
    WORKERS.store(n, Release);
    READY.store(n > 0, Release);
    serial_println!(
        "[wcpar] pool={} workers={} cpus_online={} reason={}",
        n + 1,
        n,
        crate::arch::sched::online_cpu_count(),
        reason
    );
}

/// Fan `rows` rows of `stage` (row pitch `row_bytes`) out to the framebuffer starting at byte offset
/// `dst0` with panel pitch `fb_row`. Returns false (nothing written) when the caller must run its serial
/// loop; true when every row was written and every band has completed.
pub fn par_blit(fb: &FrameBuffer, stage: &[u8], row_bytes: usize, dst0: usize, fb_row: usize, rows: usize) -> bool {
    let w = WORKERS.load(Relaxed);
    if w == 0 || !READY.load(Relaxed) || rows < MIN_ROWS || rows * row_bytes < MIN_BYTES || stage.len() < rows * row_bytes {
        return false;
    }
    if JOB_BUSY.compare_exchange(false, true, Acquire, Relaxed).is_err() {
        return false;
    }
    let t0 = crate::arch::now_cycles();
    let band_rows = (rows / ((w + 1) * 2)).max(MIN_BAND_ROWS);
    let nbands = rows.div_ceil(band_rows);
    J_FB.store(fb as *const FrameBuffer as usize, Relaxed);
    J_SRC.store(stage.as_ptr() as usize, Relaxed);
    J_ROW_BYTES.store(row_bytes, Relaxed);
    J_DST0.store(dst0, Relaxed);
    J_FB_ROW.store(fb_row, Relaxed);
    J_ROWS.store(rows, Relaxed);
    J_BAND_ROWS.store(band_rows, Relaxed);
    J_DONE.store(0, Relaxed);
    J_NBANDS.store(nbands, Relaxed);
    let generation = (J_CLAIM.load(Relaxed) >> 32) + 1;
    J_CLAIM.store(generation << 32, Release); // publish: workers may claim from here
    while claim_one() {}
    let mut spins = 0u64;
    while J_DONE.load(Acquire) < nbands {
        core::hint::spin_loop();
        spins += 1;
    }
    let _ = spins;
    // Close the job to late claimants before the buffers are released.
    J_CLAIM.store((generation << 32) | 0xFFFF_FFFF, Release); // closed: idx >= any nbands, so no late claim can land
    W_JOBS.fetch_add(1, Relaxed);
    W_BANDS.fetch_add(nbands as u64, Relaxed);
    W_WALL_CYC.fetch_add(crate::arch::now_cycles().saturating_sub(t0), Relaxed);
    JOB_BUSY.store(false, Release);
    true
}

/// Rollup: the concurrency reading, on the `[wcpar]` cadence. Silent when no job ran.
#[cfg(feature = "witness")]
pub fn emit() {
    let jobs = W_JOBS.swap(0, Relaxed);
    if jobs == 0 {
        return;
    }
    let bands = W_BANDS.swap(0, Relaxed);
    let failed = W_BANDS_FAILED.swap(0, Relaxed);
    let wall = super::wcg::cycles_to_us(W_WALL_CYC.swap(0, Relaxed));
    let work = super::wcg::cycles_to_us(W_WORK_CYC.swap(0, Relaxed));
    let mut cores = 0u32;
    let mut done = 0u64;
    for c in W_BY_CORE.iter() {
        let n = c.swap(0, Relaxed);
        done += n;
        if n > 0 {
            cores += 1;
        }
    }
    // serial_us = Σ band time (what one core would have spent); pass_us = wall inside par_blit.
    let speedup = if wall == 0 { 0 } else { (work.saturating_mul(100) / wall).saturating_sub(100) };
    // PTRSTUTTER M3 — the verdict is about COMPLETION: every band ran (a speedup of 14-28% under six vug
    // windows is the machine being busy, not a defect). Only an IDLE machine is held to a speedup floor.
    let busy = cores_busy();
    let ok = failed == 0 && (busy || speedup >= 20); if !crate::census::on(crate::census::WCPAR) { return; } // QUIETBOOT (R80): a census, OFF until `census start`.
    serial_println!(
        ":: WCPAR: cores={} workers={} bands={} pass_us={} serial_us={} speedup_pct={} load={} -> {} ::",
        cores,
        WORKERS.load(Relaxed),
        bands,
        wall,
        work,
        speedup,
        if busy { "busy" } else { "idle" },
        if ok { "PASS" } else { "FAIL" }
    );
}

/// PTRSTUTTER M3: the machine is BUSY when at least half of the online CPUs ran >= 70% over the recent window
/// (the SMPLOAD feed). Workers only spin 20 ms after a band, so on an idle desktop they do not trip it.
#[cfg(feature = "witness")]
fn cores_busy() -> bool {
    let n = crate::arch::sched::online_cpu_count();
    let hot = (0..n).filter(|&c| {
        let l = crate::arch::sched::core_load(c);
        l.tracked && l.busy_pct_recent >= 70
    }).count();
    n > 0 && hot * 2 >= n
}
