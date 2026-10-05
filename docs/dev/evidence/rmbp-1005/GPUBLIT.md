# GPUBLIT — the copy engine arms itself at takeover and carries KCOMP's window present (ledger B371)

Branch `exec-rmbp-gpublit` (cut from 2d4e1b12, the merge14 tip). Seam: `CHARTER: Kernel — driver` for
the new `drivers/gpu/kepler_gpublit.rs` (a device's driver; no handler owns a GPU copy engine) and
`Kernel — wm` for the existing `video/blitter.rs`. No new dotfile, no new shell verb (`tests gpublit` is
a fixture name), NO NEW KNOB.

## Finding (evidence first)

- Flight 22 (`rmbp-0915/flight22/f22-boots.log`): `:: KCOMP: blitter=cpu census=10 hot=window-present
  cpu_us=122 gpu=unavailable fallback_ok=1 -> PASS ::`, `[kcomp] ... gpu_direct=unavailable`. Zero `[ce]`
  lines, zero `:: KBLIT`/`:: CE:` lines, no `CE context banked` line: the flown line did not carry
  `UNAOS_KEPLER_CE` (`nvidia-kepler-ce`), so `tests ce`/`tests kblit` were not registered (`ran=0`).
- No flight (15, 19, 20, 21, 22) has EVER printed a `[ce]` or `:: KBLIT` line. The copy engine has never
  run on the metal. The only GPU-command evidence is the PFIFO/PGRAPH channel leg (`nvidia-kepler-fifo`),
  which on flight 22 still ends `FENCE VOID ... took=N` and `sched-status post-submit err=00000002`: the
  graphics channel has never validated. GPUTESTS (B334) moved fixtures and Cargo gates; it proved no
  PFIFO semaphore on the metal. "The fence GPUTESTS proved" in the row is therefore read as "the
  semaphore-release shape KBLIT authored", which is [EXT-UNPINNED] until a metal `selftest=ok`.
- Today's gate for the CE path: the Cargo feature `nvidia-kepler-ce` (knob `UNAOS_KEPLER_CE`) for the
  recon ladder + KBLIT, and `wc_gpublit` (`UNAOS_WC_BLITTER=gpu`) for `GpuBlitter::probe`, whose GPU arm
  was a stub returning `kblit-channel-absent` unconditionally.
- Reading KBLIT's channel code against the row, four defects would keep it from ever passing even if
  the engine fetched: (a) the channel is never bound in the PFIFO channel table (`0x800000 + chid*8`);
  (b) RUNLIST_SUBMIT is handed the INSTANCE BLOCK page as the runlist base; (c) LAUNCH_DMA `0x186` has
  no multi-line bit, so a 256-row pitch copy moves one line; (d) the identity page table indexes from
  the window's first page, not the PDE's VA origin (GPUTESTS already owed this). GPUBLIT builds its own
  channel with all four corrected; KBLIT's code is left as it is (recon, under its knob).

## Design

**The seam.** `drivers/gpu/kepler_gpublit.rs` (x86_64 + `nvidia-kepler` + `nvidia-kepler-takeover`, i.e.
every build that takes the panel; NOT behind `nvidia-kepler-ce`) owns ONE copy-engine channel and
exposes a raw, video-type-free API: `copy(src, dst, rects) -> seq` / `wait(seq, budget_us)` over
"endpoints" that are either VRAM (identity GPU VA) or kernel RAM (mapped into a sysmem window of the
channel's VM). `video/blitter.rs::GpuBlitter` does the geometry and calls it; `probe` answers from the
boot self-test. No second blitter, no second compositor path: `present_band` is unchanged.

**The channel's VM** ([EXT-UNPINNED] GMMU formats, marked at their definitions): PDE[0] = a small-page
table identity-mapping the VRAM span the takeover framebuffer and the GPUBLIT window occupy (GPU VA ==
VRAM offset, KF24); PDE[1..5] = a 512 MiB sysmem window, mapped LAZILY per source buffer, VA bump-
allocated and NEVER reused (so no stale TLB entry can exist and no TLB-invalidate register is needed).
A mapping is cached per buffer and re-validated every job by re-walking the CPU page tables
(`memory::translate`); a changed frame takes a fresh VA. Exhausting the window is a CPU fallback.

**Milestones.**
- **M1 — self-arming at takeover, no knob.** `kepler::init`, right after `takeover_display` returns
  `Some(fb_offset)`, calls `kepler_gpublit::arm(...)`: reserve the window from the boot `VramAllocator`,
  build PD/PT/instance block/runlist, bind the channel, submit one pushbuffer copying a 64x64 pattern
  from a heap buffer (sysmem window: the same path the hot path uses) into VRAM scratch, wait on the host
  semaphore with a TSC-bounded 20 ms budget, read the scratch back and compare byte for byte with the CPU
  blit of the same source. One line, always:
  `:: GPUBLIT: selftest=<ok|mismatch@n|timeout|refused(<why>)> ce_us=<n> cpu_us=<n> -> blitter=<gpu|cpu> ::`
  and, only when it is not `ok`, one `[gpublit]` line with the readbacks (GP_GET/GP_PUT, semaphore,
  channel words, PMC). Never a hang: every wait is bounded by `now_cycles` against `tsc_hz`.
- **M2 — KCOMP's hot path through the CE.** `GpuBlitter::blit` → one pushbuffer per job (all rects of the
  job, one semaphore release), `wait` bounded by a 4 ms budget (about one CPU present on flight 19). The
  channel is behind a try-lock: busy (another core's band in flight) is `Unavailable` → the CPU blitter
  runs the job that frame (the per-frame fallback). A timeout DEMOTES the CE for the rest of the boot
  (`:: GPUBLIT: demoted reason=timeout ... ::`, once) because a late CE write could land over a staging
  band the compositor has already reused. `tests blitter` (KCOMP) gains `gpu_us=` beside `cpu_us=`.
- **M3 — `tests gpublit`.** Re-runs the 64x64 self-test live on the bound channel (it never re-arms a
  CPU-selected boot) and prints the M1 line, then
  `[gpublit] boot=<reason> jobs=<n> gpu_us_mean=<n> busy=<n> timeouts=<n> unmappable=<n> demoted=<0|1> sysmaps=<n> sys_used_kb=<n> put=<n> seq=<n>`, then the `GPUBLIT-TEST` verdict.
- **M4 — this doc + the ledger status text.**

**Why not one pushbuffer per FRAME.** `stage_window` composes each band into a per-core staging buffer
and presents it under `beam::hold` before composing the next band into the SAME buffer, so a band's copy
must finish before the next compose. Batching a frame needs double-buffered staging; it stays owed and
the per-job pushbuffer is the unit (one window band, every rect in it, one fence).

**The vblank** stays B369's (KVBLANK10). Presents keep the pace timer.

## Witness (metal)

Boot (both outcomes are a correct build; which one is the metal's answer):
`:: GPUBLIT: selftest=ok ce_us=<n> cpu_us=<n> -> blitter=gpu ::` then `[wc] blitter=gpu reason=selftest-ok`
— or — `:: GPUBLIT: selftest=timeout ce_us=20000 cpu_us=<n> -> blitter=cpu ::`, `[gpublit] ...`, then
`[wc] blitter=cpu reason=selftest-timeout`. Expected on boot 24, honestly: `timeout` (the PFIFO wall).
`tests blitter`: `:: KCOMP: blitter=<cpu|gpu> census=10 hot=window-present cpu_us=<n> gpu_us=<n|-> gpu=<available|unavailable> fallback_ok=1 -> PASS ::`.
`tests gpublit`: the GPUBLIT line again plus the `[gpublit] boot=` counters line; `-> PASS` is not
printed by design on the boot line (it is a selection, not a test); `tests gpublit` closes with
`:: GPUBLIT-TEST: rerun=<verdict> boot=<reason> -> PASS|FAIL ::` (PASS iff the boot armed the CE and the
rerun matched byte for byte; a CPU-selected boot is a FAIL — the metal test is "the GPU blits").

## Owed

- Every GMMU / RAMFC / runlist / copy-class / host-semaphore encoding is [EXT-UNPINNED]; the metal's
  `selftest=` is what pins or kills them. The CE runlist id is a hypothesis (runlist 1).
- One pushbuffer per frame (needs double-buffered staging). The occluded-span present (`clip.n > 0`) and
  `stage_fill` stay CPU.
- `wc_gpublit` (`UNAOS_WC_BLITTER=gpu`) is now inert (the GPU path keys on the takeover build, no knob):
  the seat retires the knob (Cargo, arroyo, builder, the k8-reach row) at the fold.
- KBLIT's `tests kblit` keeps its four defects (recon under `UNAOS_KEPLER_CE`); fold it onto this
  channel or retire it.
