# SUITES 2026-10-13 — EYES on 441d7e45d, the rMBP bench (Fedora 44)

The brief named `unaos/scripts/eyes`; no such path exists at 441d7e45d. The renderer-eyes suite lives at
`tools/eyes` (entry `tools/eyes/run.sh <suite>`, documented in its header). Run as documented:
`tools/eyes/run.sh aether` (the renderer, Aether vs Chromium) and `tools/eyes/run.sh facet` (golden + Chromium).
`metal` needs a card pull (`--from DIR`, R78/no card here) and `xvfb-smoke` needs Xvfb + GTK + Chromium: not run.

## CANNOT RUN HERE — the oracle half

This host has **no `node`, no Playwright, no Chromium** (`command -v node` → nothing; `/opt/pw-browsers` absent;
no `Xvfb`). Every `chromium` oracle case therefore has no reference frame. Exact error (after aa4f18675 made eyes
print it; at 441d7e45d it printed only `eyes: chromium oracle reported failures (above)` with nothing above):

    eyes: chromium oracle could not start `node`: No such file or directory (os error 2) (no reference frames this run)

**This is NOT a pass and NOT a renderer regression**: the gate is RED because nothing was scored.

## Summary lines

- `aether`: 28 cases rendered by `aether render` (subjects + ledgers written), **0 scored** (all 28 `[no reference
  frame]`); `gate: RED — 25 case(s) regressed past …/suites/aether/baseline.json (ssim_drop 0.02, mismatch_rise 2)`,
  rc=1. The 25 "regressions" are ssim baseline → 0.000 for want of a reference; the other three cases have no
  baseline row.
- `facet`: 22 cases; **11 golden cases scored, 11/11 ok** (ssim 1.000, mismatch 0.0%: card-actual, card-adjust,
  card-crop, card-fit, card-fliph, card-pan, card-resize, card-rotate90, card-zoom400, photo-o5, photo-o6);
  11 `@chromium` cases unscored (`[no reference frame]`); `gate: RED — 11 case(s) regressed`, rc=1 — all 11 are the
  unscored `@chromium` twins.

## Red list

None attributable to the code under test. The red gate on both suites is the missing oracle (above). The one defect
found — eyes swallowing the spawn error so the red read as a regression — is fixed in aa4f18675 (ledger SR118).

Raw logs: `eyes-aether.raw.txt`, `eyes-facet.raw.txt`. Out dirs (PNGs, SCORE.md) moved to
`~/unaos-bench/scratch/rmbp-0915/local1/suites/eyes-out/`.
