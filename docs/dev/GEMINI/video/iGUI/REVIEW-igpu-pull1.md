# REVIEW — igpu pull 1 (2026-07-22) — APPROVED WITH AMENDMENTS

The probe plan, register offsets, scope fence, and honesty lines are all good.
Device ID 0x0166 (IVB GT2 mobile), BAR0 GTTMMADR 2MB MMIO + 2MB GTT, BAR2
GMADR aperture, and the PIPExCONF/DSPxCNTR/DSPxSURF offsets all match the
published Ivy Bridge PRM. Cleanroom posture accepted.

Two binding amendments before implementation:

## A1 — DSPxSURF is a GGTT offset, not a CPU physical address (correctness)

The proposal expects `DSPASURF` to read back `0x90020000`. It will not. On
this hardware the plane surface base is an offset into graphics memory
resolved through the **GGTT** — `0x90020000` is the CPU-visible side (GMADR
aperture + 0x20000, which is why the metal record shows the fb there).
Expected readback is a small GGTT offset (plausibly `0x20000` or 0).

Consequences:
- Milestone 1 (read-only) is unaffected in its actions — but record the raw
  value without asserting it equals the CPU address, and dump enough to
  correlate: read the GGTT base entries via the BAR0+2MB GTT window for the
  region around the DSPxSURF value, so metal tells us how GOP mapped the fb.
- The repoint milestone cannot be "write our fb's physical address". Options,
  to be chosen from metal evidence: (a) write pixels through the existing
  mapping (render into the GOP fb region — zero register writes, best first
  pixel), or (b) install GGTT entries for our own buffer, then repoint
  DSPxSURF at that offset. Plan (a) first; (b) is its own approved step.

## A2 — milestone 1 must also dump geometry/layout registers

Repointing (or writing in place) requires knowing the layout GOP programmed.
Add to the read-only pass, for the active pipe/plane:
- `DSPxSTRIDE` (0x70188/0x71188/0x72188) — pitch;
- `DSPxCNTR` full decode, not just bit 31 — pixel format bits and tiling bit;
- `PIPExSRC` (0x6001C/0x6101C/0x6201C) — source width/height;
- `DSPxLINOFF`/`DSPxTILEOFF` — panning offsets.

## Notes (non-binding)

- eDP on this machine is CPU DP port A; a readback of `DP_A` / port-active
  state would confirm the panel path but is optional for pull 1.
- The persistent black-panel-on-every-boot open question (KEPLER-METAL-LOG
  sitting #5) sits exactly in this lane's blast radius: milestone 1's dumps
  (is the pipe still enabled? plane still pointing at the GOP fb?) are also
  the cross-check that question has been waiting for. Call that out in the
  instrumentation output so Fox can read it at sitting #6.
