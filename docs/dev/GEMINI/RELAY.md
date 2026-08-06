# RELAY — session-opening text for the new kepler and igpu sessions (GR18)

## → kepler (new session, read this first)

We are on the same Linux box. The working loop, unchanged from last round but restated
because this is a fresh session:

- **Read your brief directly from the working tree.** It is a file; open it. The
  authority for this pull is
  `docs/dev/GEMINI/video/Kepler/BRIEF-kepler-fence-pull35-poison-order-and-access-ledger.md`
  (your own PROPOSAL-kepler-fence-pull35.md sits beside it — start from where they
  disagree, if they do).
- **No per-pull commits.** Write your proposal and your code as files in the tree and
  leave them there; one code commit at round end. Everything else stands:
  proposal-first, full listing with citations, bound every loop, cleanroom (facts with
  attribution, no GPL code bodies).
- **The gate is `./arroyo check`, both arches, only.** Do NOT run `./arroyo test` /
  `test-fat` — QEMU has no gmux, no SMC, no panel, no Falcon; it cannot reach your
  code. Metal is the verdict.
- **Verify your feature's symbols are IN the artifact** (`strings` on
  `target/x86_64_esp/kernel.elf`), not merely that the build was green. A knob added
  only to arroyo is invisible to `builder/`; the feature ships disabled while every
  check passes. This has cost us twice.

Where the bench stands, from this seat's metal captures (boots T/T2/U/V, all today):

- **Your block is now the boot's biggest.** `kepler=1521/1521/1522/1521 ms` at n=4,
  identical to the millisecond, ~45% of a `gui=3408 ms` boot. Everything else is
  attributed and mostly at silicon/spec floors — the next boot-time headline anyone
  can buy comes from this lane.
- **Standing item 1 — the `kdisp: fb-draw hold` constant.** Measured on metal: your
  per-"second" tick spans 225 ms of real time (~4.4× fast), so the "5 s" hold is
  ~1.12 s. Calibrate against `cycles_to_us` and decide which is lying — the constant
  or the label. Either answer shrinks or truthfully names most of the 1.5 s block.
- **Standing item 2 — your failure prints reproduce at n≥4 and read as failures.**
  `kepler: ucode-echo FAILURE h2h3=on` / `h2h3=off` and `WITNESS FAILED - bits
  stripped. Restoring inst_off+0x0C` print on every boot in the s73 capture. If they
  are expected diagnostics, relabel them (they trip human FAIL-sweeps every round);
  if they are real, they are reproducing on demand and are chaseable.
- **Anchors, still binding:** `Initializing Kepler` and `GPACE: span` are load-bearing
  for `tools/serial-analyzer.py --gaps/--wcg`. Rename either only with a paired
  analyzer change, relayed here first.

## → igpu (new session, read this first)

Same working loop as kepler above — brief from the tree, no per-pull commits, gate is
`./arroyo check` both arches only, no QEMU suites (no gmux, no panel there), and
strings-verify your symbols are in `target/x86_64_esp/kernel.elf` before calling a
feature shipped.

Your standing brief on disk is
`docs/dev/GEMINI/video/iGUI/BRIEF-igpu-pull7-window-truth-and-panel-census.md`
(your PROPOSAL-igpu-pull7 sits beside it). Finish or fold that per its own terms —
and then this seat is handing the lane its next arc, with the metal numbers to
justify it:

**Get the compositor's present path off the CPU and onto the IVB (HD 4000) blitter.**

- The problem, measured on today's boots: the panel is `2880x1800 stride=4096px
  pitch=16384B bpp=4` — ~29.5 MB per full frame — and every present is CPU stores
  into a write-combined framebuffer. WC store throughput is the entire per-frame
  budget; it is why the witness battery went pay-as-you-go and why full verify passes
  are deferred. A BLT-ring blit takes that cost off the CPU. Console fill/scroll
  acceleration is a legitimate smaller opening milestone if the full present path is
  too big for one pull.
- Your bring-up budget today is `igpu=1ms` in GPACE next to kepler's 1521 ms — there
  is room to spend real initialization and still be invisible.
- **Hard constraints from this seat:** (1) the framebuffer's **WC typing is sacred** —
  GR15 proved un-typing it costs 8.7–9.1× and the defect is silent. As of Boot V the
  fb leaf is instrumented: `WXPROBE map: at=fb … pat=1 pcd=0 pwt=0` prints every boot
  and the analyzer WARNs if it ever changes — your GTT/ring setup must keep that line
  bit-identical. (2) New serial lines: stable `key=value` witness formats, relayed
  here before renaming anything. (3) Your lane's code stays behind `UNAOS_IVB=1`.
- Do not re-derive on our dime: the gmux protocol facts are in your pull-4/5 briefs
  and the power-on-battery facts in pull-6 — they cost real boots.
