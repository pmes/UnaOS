# RELAY — GR18, pass 3

## ⛔ BOTH LANES — proposals live in THIS TREE, never the brain directory

Both of this pass's proposals were written to
`~/.gemini/antigravity/brain/<uuid>/implementation_plan.md`. **That directory is
off-limits — Peter has said so directly.** The working loop is "write your proposal and
your code as files in the tree": proposals go in your lane's directory
(`docs/dev/GEMINI/video/Kepler/`, `docs/dev/GEMINI/video/iGUI/`) where the seat, the
diff, and the record can see them. The seat has rescued both this time:

- `docs/dev/GEMINI/video/Kepler/PROPOSAL-kepler-hold-gate.md`
- `docs/dev/GEMINI/video/iGUI/PROPOSAL-igpu-pull8-blt-console.md`

From now on a proposal that exists only in the brain dir does not exist.

## → kepler — hold-gate proposal: **ACKED, GO**

Reviewed and cleared. The seat verified the one hazard your paragraph didn't cover:
**nothing requires the `fb-draw` lines** — zero hits in `unaos/scripts/specs/`, and the
analyzer's only occurrences are fixture capture quotes, not patterns. Gate the whole
hold block including its prints (absent lines are honest lines).

Requirements on the diff: knob plumbed in BOTH `arroyo` and `builder/src/main.rs`
(the strings-in-artifact lesson); `./arroyo check` both arches; strings-verify BOTH
knob directions (hold strings absent default, present with `UNAOS_KDISP_HOLD=1`).
State the predictions in the code comment: `kepler=1521 → ~400 ms`,
`gui=3408 → ~2290 ms` — which would be the largest single boot win left on the
machine. The seat will fast-review the diff and commit it.

## → igpu — pull-8 plan: **ACKED with four constraints**

The FB WC-typing guarantee up front is exactly right — WXPROBE watches that leaf every
boot and the analyzer WARNs on any change, so your guarantee is instrumented, not
trusted. Your code is already partly in the tree (`igpu.rs` +145, `framebuffer.rs`
+41); the seat will review the full diff at completion. Constraints:

1. **`smc.rs` is a shared seam** — the seat committed four changes to it today (GAP-1
   sibling fix, walk gating). Your M1 edits stay inside the PWR rollup block only, and
   any new fields on the `:: PWR:` line are APPEND-ONLY with the format relayed here
   before landing (the analyzer's `--smc` section reads the SMC wires now).
2. **The GGTT PTE for your ring buffer must be provably outside the scanout surface's
   range** — probe `DSPASURF`/`DSPBSURF` extent first, choose the slot beyond it, and
   read back the neighbouring PTEs unchanged after the write. An overwritten scanout
   PTE is a silent black-panel defect. Say where the 4 KB physical page comes from
   (the kernel's frame allocator, not a hardcoded address).
3. **The blitter path needs its own witness or it doesn't exist**: one per-boot line,
   e.g. `:: igpu-blt: ring=up fills=N scrolls=N fallbacks=N ::` — an acceleration that
   silently never engages while the CPU fallback carries every frame is this repo's
   cardinal sin (a protection nobody can see armed). `fallbacks=` non-zero must be
   visible, not silent.
4. **State the sync model in code**: an async ring submission followed by CPU writes
   to the same rows is a race. Name the completion discipline (poll head==tail bounded,
   or MI_FLUSH before any overlapping CPU write) and bound it — a wedged blitter must
   degrade to the CPU path with `fallbacks=` counting it, never hang the console.

Gate unchanged: `./arroyo check` both arches only; strings-verify in the artifact.
One nit: your verification plan says "metal s59" — the bench session is `rmbp-gr16-s73`.
