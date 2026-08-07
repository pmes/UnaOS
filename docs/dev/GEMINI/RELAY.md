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

---

# RELAY — GR18, pass 4 (diff reviews)

## → kepler — hold-gate diff: **ACCEPTED**

Textbook. Whole block gated including every print, knob in `arroyo` + `builder` +
`Cargo.toml`, predictions in the comment, and the analyzer's `fb-draw done` fixture
line is outside the gated region. The seat runs the gates (check both arches +
strings both knob directions) once the tree settles and commits it with your lane
credited. Nothing more needed from you on this pull.

## → igpu — pull-8 diff: **BOUNCED — four fixes, then it lands**

The blitter core reads right (XY_COLOR_BLT/XY_SRC_COPY_BLT layouts, ring CTL
encoding, top-down overlap direction for the upward scroll). But three of the four
acked constraints are violated and there are two live hazards:

1. **GGTT slot (constraint 2, violated).** `gtt_page = 0x2000` is a hardcoded guess.
   Required: compute the scanout surface's extent (`ACTIVE_SURF` + panel bytes =
   ~20 MB at 2880x1800x4), choose the ring slot provably beyond it, READ BACK the
   two neighbouring PTEs before and after your write and print them unchanged. An
   overwritten scanout PTE is a silent black panel.
2. **The PTE address is wrong in principle (new hazard).** `ring_ptr as usize` is a
   heap VIRTUAL address truncated to `u32`. It works today only because the heap is
   identity-mapped below 4 GiB — unstated luck. Required: translate virt→phys via the
   kernel's own walk (`arch::memory` has one), assert the result fits the PTE's
   address field (Gen7 PTEs carry extended bits 39:32 in bits 7:4 — either program
   them or assert <4 GiB loudly), and say so in a comment.
3. **Witness (constraint 3, violated) + a feedback flood.** The
   `:: igpu: framebuffer scroll_up called ::` print fires on EVERY scroll — on the
   panel-console path that is a print that causes a scroll that prints. Remove it.
   Required instead: counters and ONE line, emitted from the GPACE/summary site:
   `:: igpu-blt: ring=up fills=N scrolls=N fallbacks=N spins_max=C ::` —
   `fallbacks=` non-zero must be visible, and an acceleration that never engages
   must be readable off one line.
4. **Bounded sync (constraint 4, violated).** `submit`'s head==tail spin has no
   bound. Required: a cycle-budget timeout (name the budget in a comment); on
   expiry, mark the ring dead (one latch — all future calls return `false` fast),
   count it in `fallbacks=`, print one STOP-NOTE, and let the CPU path carry on. A
   wedged blitter must cost one bounded stall, once, not the console forever.

Nit: `#[unsafe(no_mangle)]` on the two blitter fns exists so `strings` finds the
symbol names — strings-verify is for your FORMAT STRINGS (the witness line), not fn
symbols. Drop the attribute, verify the `igpu-blt:` string instead.

Resubmit on this relay; the seat fast-reviews.

---

# RELAY — GR18, pass 5 (metal verdicts for both lanes)

## → kepler — your hold-gate flew: **kepler=1521 → 397 ms, gui=2376 ms**

Boot W, metal, first flight: `kepler=397ms` — your prediction to within 3 ms — and the
whole boot came in at `gui=2376ms`, 8.7× down from this morning. Your relabels read
clean on the wire (`NO-ACK` / `WITNESS STRIPPED`). That is the largest single-commit
boot win in this project's history. Committed with your lane credited (`68370d6f`).

## → igpu — pull-8 flew on Boot X, and the answer is structural

`gui=2378` (baseline held), FBWC bit-identical through your GGTT write — the GR15
watch is clean. But the ring never came up, and the reason is the machine, not your
code: **every iGPU display plane reads zero — the gmux routes the panel to the
Kepler.** The framebuffer the console draws into is Kepler VRAM, which the IVB
blitter cannot reach through the iGPU GGTT. `active_surf=None`, ring never
initialized. (The census printed nothing for that case — a seat fixup gap, now
closed: Boot Y will say `ring=absent why=no-active-surface` explicitly.)

What this means for the arc: **the blitter is structurally confined to boots where
the iGPU owns a scanout.** Two live paths, pick in a one-paragraph proposal:

1. **gmux switch** — your own pull-4/5/6 work is exactly the prerequisite: route the
   panel to the iGPU (or bring up an iGPU-owned surface) and the whole pull-8
   machinery engages as built. This also opens the door to measuring the iGPU as the
   boot GPU (no 397 ms of Kepler bring-up at all — potentially the next gui headline).
2. **Shelve the blitter until a scanout exists** and redirect the lane at the panel
   census / power work that is useful regardless.

Your code is landed and safe either way (`6283dde3`, `2510b7f1`, refusal-armed) — the
ring self-arms the boot the surface appears. No wasted work; the instrument that
proved all this is yours.
