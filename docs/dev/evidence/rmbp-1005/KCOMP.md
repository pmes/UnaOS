# KCOMP — one blitter under the compositor (CPU today, the Kepler copy engine when KBLIT lands)

Ledger B321 · branch `exec-rmbp-kcomp` (cut from 776ffbe7) · seam `CHARTER: Kernel — wm` ·
Peter 2026-10-04: "what about the GPUs?"

## Design (short)

**Finding.** Every pixel on the rMBP glass is CPU-composited. The window → scan-out present is a
row loop of `copy_nonoverlapping` from the cached-RAM staging band into the write-combined BAR1
aperture, fanned over the APs by WCPAR. There was no interface under it: the copy was inlined in
`stage_window`, so a GPU copy engine had nowhere to plug in.

**Seam.** `video/blitter.rs` — `trait Blitter { blit(&BlitJob) -> Result<Fence, BlitErr>; wait(Fence,
timeout_us); name() }` over the compositor's own surface descriptor (`FrameBuffer` + where it lives:
`Mem::Ram | Mem::Scanout` (Scanout = BAR1 on the rMBP)). `CpuBlitter` IS the code that ran before (WCPAR's `par_blit` band fan-out
first, the per-row `blit_traced` loop when it declines). `GpuBlitter` is a stub returning
`BlitErr::Unavailable`; the one function KBLIT fills is named below. FALLBACK: any `BlitErr` from
the GPU blitter re-runs the job on the CPU blitter in the same frame and counts `gpu_fallback` —
the glass never goes dark because an engine stalled (the A1/BAR1WEDGE lesson: a wedged engine must
not take the panel with it).

**Knob.** `UNAOS_WC_BLITTER=gpu` → feature `wc_gpublit` (default OFF, x86 only). Off, or on with no
channel, the kernel selects cpu and prints once, at the first composite:
`[wc] blitter=cpu reason=<feature-off|kblit-channel-absent>`.

**Milestones.** M1 census (below) · M2 `video/blitter.rs` · M3 `stage_window`'s no-clip present
goes through `blitter::present_band`; `[wc-h] rollup` gains `blitter= blit_us= gpu_fallback=`
(inserted after `beamcross_ppk=`, the slot no spec keys an adjacency on) · M4 `tests blitter`.

**Witness** (`tests blitter`, never at boot — R80):
`:: KCOMP: blitter=cpu census=10 hot=window-present cpu_us=<n> gpu=unavailable fallback_ok=1 -> PASS ::`
preceded by a `[kcomp]` detail line (inline vs blitter checksums and timings).

**Owed.** The occluded-span present (`clip.n > 0`, per-row span lists) stays inline in M3 — it is
expressible as a rect list but a per-row list is the CE's worst shape; it moves when the GPU path
exists and is measured. The erase/wallpaper `stage_fill` and the dock/menubar strips are the next
callers (same primitive). Nothing here proves a GPU byte: that is KBLIT's `tests kblit`.

## M1 — census of every compositor pixel copy

Panel: 2880×1800, 4 B/px (BGRx/RGBx per `FrameBufferInfo::pixel_format`), pitch `stride*4`; a full
frame is ~20.7 MB. BAR1 is mapped write-combined (PAT PA4, `x86 fb-wc`; `bar1exp-uc` is the UC
arm), so CPU STORES stream but CPU READS of it are uncached — `[wc-g] prof … probes=52976
readback_us=90099` in flight 19 is ~1.7 us per probe. Measured numbers are flight 19
(`docs/dev/evidence/rmbp-0915/flight19/f19-boots.log`, four boots, image 12), aggregated with awk
over every rollup.

| # | Path (file :: fn) | Src → dst | Format / pitch | Src memory | Size per frame | Measured (flight 19) | GPU verdict |
| :- | :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| 1 | **window present** `video/wm.rs::stage_window` (no-clip arm → `wcpar::par_blit` / `blit_traced`) | staging band → panel | panel fmt, src pitch `bw*4`, dst pitch `stride*4` | cached heap (per-core stage) | box bytes: BUFFERED mean 1.67 MB, up to 3.6 MB (1242×732) per window present | `[comp2]` 294 rollups / 28257 passes: present_us mean **3355** per pass (compose 3103, blit_us 13170, residual ≈ beam waits); BUFFERED mean present_us 1221 for 1.67 MB (~1.37 GB/s with bands); `:: WCPAR:` 293 rollups, 618861 bands, pass_us 32.49 s vs serial_us 121.59 s (3.7× over 7 workers), 292 PASS 1 FAIL (TESTFIX3's light-band case) | **THE HOT PATH.** Rectangular, no scaling, no format conversion: exactly a copy-engine job. 3355 us ≫ CE-LADDER R0's 300 us stop line, so the ladder continues. Through `Blitter` now. |
| 2 | occluded-span present, same fn (`clip.n > 0` arm) | staging → panel, per-row spans | as #1 | cached heap | ≤ #1 (withheld spans) | inside #1's present_us | CE-able as a rect list; stays inline (owed, see Design). |
| 3 | window compose `wm.rs::paint_window` (+ the integer-upscale row replication `dst.blit(...)`) | window surface → staging band | surface fmt = panel fmt, `scale=2` nearest-neighbour | cached heap (window `surf`) | box bytes | `[comp2]` compose_us mean 3103 per pass | NOT a copy-engine job (a scaler plus chrome drawing). A 2D-engine scaled blit could take the upscale later; out of KCOMP. Stays CPU. |
| 4 | cursor `video/cursor.rs` (`compose_into` into the layer; direct sprite paint with save-under `fb.read_pixel`) | sprite → layer / panel; panel → save-under | 4 B/px | sprite cached; **save-under reads BAR1** | sprite box (tens of px²) | `[comp2]` sprite_us mean 86 per pass | Stays CPU. The right GPU answer is the Kepler head's hardware cursor plane (an overlay, no blit at all), a separate arc. |
| 5 | dock + menubar furniture `video/strip.rs` (row of `u32` → `fb.blit`) | strip row buffer → panel | 4 B/px, dst pitch `stride*4` | cached (stack/static row) | strip height × width per repaint | `[dock] paint=` (not aggregated here) | CE-able (same primitive); next caller after #1. |
| 6 | erase / wallpaper fill `wm.rs::stage_fill` (drained by `drain_deferred`) | staging row(s) → panel | panel fmt | cached heap | up to a full panel (20.7 MB) on a full erase | priced in `[comp2] sprite_us`, `[wc-k]` lines (38 in flight 19) | CE-able; the largest single present. Next caller. |
| 7 | splash takeover `splash.rs::takeover_blit` | hold surface → panel, whole frame once | 4 B/px, pitch `stride*4` | cached heap | 20.7 MB, once per boot | `:: SPLASH: stage=takeover at_ms=7275` | Stays CPU: it runs at the takeover, before any channel could exist. |
| 8 | console store `video/fbcon.rs` (`fb.blit(off, &store[..])`) | console back store → panel | 4 B/px | cached heap | dirty rows | — | Stays CPU (pre-desktop and fallback path). |
| 9 | screenshot readback `video/prtscr.rs` (`panel.read_pixel`), `shotsel.rs` | **panel (BAR1) → PNG encoder** | 4 B/px → RGB | **BAR1, uncached reads** | the captured region (up to 20.7 MB) | `[prtscr]` chord 2× in flight 19 | GPU-relievable in principle (a VRAM→sysmem DMA is the cheap way to read BAR1), but it needs a sysmem target in the channel's VM; stays CPU until KBLIT proves a VRAM→RAM copy. |
| 10 | coherence witnesses `video/wcg.rs` (`[wc-g]` lattice), `[wc-d]` verify, `menubar.rs` probe | **panel (BAR1) → checksum** | 4 B/px | **BAR1, uncached reads** | lattice16 probes / box | `[wc-g] prof … readback_us=90099` | MUST stay CPU: they verify what the scan-out holds; letting the engine under test read it back would verify the GPU against itself. |

Also in the tree but not a panel path: `wm.rs` PACE_MIRROR (window surface → mirror, cached → cached;
the shadow copy for paced presents) and `screen.rs`'s banded back→front damage flush (the
`Screen` path, aarch64 desktop and pre-WC x86).

**census=10** (the numbered rows). **hot=window-present** (row 1).

### The seam KBLIT fills

`video/blitter.rs::GpuBlitter::blit` calls, once KBLIT's channel exists:

    crate::drivers::gpu::kepler_ce::blit(job: &BlitJob) -> Result<Fence, BlitErr>   // brief name: kepler::ce::blit(job) -> Fence

and `GpuBlitter::wait` polls that fence (a semaphore release the CE writes after the copy) with
`timeout_us`; a timeout is `BlitErr::Timeout` and therefore a fallback, never a dark frame.
`GpuBlitter::probe` returns `Ok` only when the channel is bound (KBLIT's `tests kblit` PASS state).
What the GPU path needs that the CPU path does not: the SOURCE in GPU-visible memory. The staging
band is kernel heap; the CE reads it only through a sysmem mapping in the channel's VM (or windows
compose straight into a VRAM band). That mapping is KBLIT's to establish; `Surface::mem` carries
`Mem::Ram` vs `Mem::Scanout` so the GPU blitter can refuse (`BlitErr::Unsupported` → fallback) a
source it cannot address.

## M2–M4 — what was built

- **M2** `unaos/crates/kernel/src/video/blitter.rs` (CHARTER: Kernel — wm). `Blitter`, `BlitJob { src,
  dst: Surface, rects: &[Rect], op: Copy|CopyAlpha }`, `Surface { fb: FrameBuffer, mem: Ram|Scanout }`,
  `Fence`, `BlitErr { Unavailable, Unsupported, Bounds, Timeout }`. `CpuBlitter::blit` is the moved
  present: `wcpar::par_blit` over contiguous source rows (x86 + `wc`), else `wm::kcomp_row` per row
  (= `comp_mark_row` + `blit_traced`, the exact pair the inline loop ran). `GpuBlitter` returns
  `Unavailable`; `probe` answers `feature-off` / `kblit-channel-absent`. `run_on` is the fallback rule.
  Knob `UNAOS_WC_BLITTER=gpu` → `wc_gpublit` (Cargo.toml, arroyo `_feats`, builder, k8-reach NA row).
- **M3** `wm::stage_window`: the no-clip arm's `par_blit` + serial copy is now
  `blitter::present_band(r.id, fb, &layer, bx, by + band, bw, rows)`; if it returns false the inline
  row loop still copies (a second net under the fallback). The occluded-span arm is unchanged.
  `[wc-h] rollup` gains `blitter=<cpu|gpu> blit_us=<cumulative per window> gpu_fallback=<n>` directly
  after `beamcross_ppk=` (arity 43 → 46); every spec rule on that line (`pi4-regression.spec:1028–1030,
  1139–1140`, `x86-witness.spec:1245`) keys `.*` wildcards across that slot, so none moves.
  Byte-identity argument: the destination offsets are the same expressions
  (`(dy*stride + dx)*bpp + y*stride*bpp` = `py*fb_row + bx*bpp`), the primitive is the same
  (`FrameBuffer::blit` via `blit_traced`, or `par_blit` with the same arguments), and the order of the
  witness accounting is unchanged (it never read the panel). The added cost per band is one `dyn` call,
  one rect validation and two `now_cycles` reads.
- **M4** `tests blitter` (registered from `tests::shell_verb`, never at boot): 512×512 into a 640×520
  heap destination at (8,4) — the old inline path vs `CpuBlitter` (checksums equal, not blank), then
  `GpuBlitter` via `run_on` (direct blit = `Unavailable`, fallback taken, counted exactly once, output
  equal), plus a bounds refusal. Lines:
  `[kcomp] w=512 h=512 inline_us=… cpu_us=… gpu_path_us=… cks_inline=… cks_cpu=… cks_gpu=… blank=… par=… gpu_direct=unavailable fallback_counted=1 bounds_refused=1`
  `:: KCOMP: blitter=cpu census=10 hot=window-present cpu_us=<n> gpu=unavailable fallback_ok=1 -> PASS ::`

**On metal, expect** `[wc] blitter=cpu reason=feature-off` once at the first composite (or
`reason=kblit-channel-absent` with `UNAOS_WC_BLITTER=gpu`), and every `[wc-h] rollup` carrying
`blitter=cpu blit_us=<n> gpu_fallback=0`. The unchanged-ness proof on metal is the existing lines:
`:: WCPAR: … -> PASS`, `[wc-g] … -> CLEAN`, `[wc-h] rollup … -> TEAR-FREE` at flight 19's rates, and the
screenshot readback (`prtscr`) as before. `blit_us` should sit at or just above the rollup's present
time (it is the present plus the dispatch).

**Proved here (R78, no QEMU):** compile legs and the static gates — see the report in the ledger row's
source. NOT proved here: any runtime number; the first `tests blitter` run is the bench's.
