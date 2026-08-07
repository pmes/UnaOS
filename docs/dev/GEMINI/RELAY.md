# RELAY

Boot AA flew this morning (`gui=2217ms` third boot running, trunk media `37fc0f3c`,
capture `rmbp-gr16-s73`, `hz=2693846865`). M3a (CR0.WP all eight cores) metal-confirmed,
zero pace cost, mbench 28/28. Slice: `~/unaos-bench/scratch/gr20/bootAA-slice.log`.
⚠ Both lanes: work ONLY in your own worktree (`UnaOS-gemini-kepler` / `UnaOS-gemini-igpu`).
Last round both lanes wrote into the main tree and a restore destroyed a full
implementation, unrecoverable. Never build or edit in `~/src/github.com/pmes/UnaOS-gemini`.

## → kepler — your proposal is MERGED with amendments, and the metal already answered JOB 2

**Your branch is merged** (`20a00320`); cut the next one fresh from trunk (`37fc0f3c`+).

**JOB 1 — implement the falcon heartbeat, amended.** Full review with 13 numbered
amendments: `~/unaos-bench/scratch/gr20/review-kepler-heartbeat.md`. The ones that change
your design, not just your words:
1. **MAILBOX1 (`0x409044`) is INSIDE the severed unit** — it returned `BADF1000` on Boot Z
   like the rest of the window. Your separation works because it is **temporal** (read
   before the poisoned read arms the sever), not spatial. Say so in the doc, and read the
   heartbeat BEFORE any `0x409504`-adjacent access.
2. **Your `hb == 1` test convicts a healthy falcon.** `PHASE_A_PRELOOP=0x01` survives ~3
   falcon instructions before the loop overwrites it with `0x02` forever; a host read will
   essentially never see 1. Accept any non-`BADFxxxx` value as the heartbeat.
3. **Poll AFTER `cmd=1`, bounded.** Your pre-poll delays the poke; `ECHO_BOUND` is
   milliseconds of falcon patience, so an unbounded (or 100 ms) pre-poll converts every
   boot into EXIT-BY-BOUND with the poke never executed — an instrument-only violation.
4. **Controls:** CPUCTL proves nothing (same unit, non-host-controlled value). Use
   `CC_SCRATCH[0]` (`0x409800`, host-writes-1, your POKE image never writes it) as the
   known-readable in-unit control, and GPCCS `0x41A100` as the cross-unit control (Boot Z
   healthy: `00000010`).
5. **The third arm is undecidable and must say so:** "falcon completed the `iord` but the
   host cannot read the result" vs "falcon faulted at the `iord`" are byte-identical from
   the host. Do NOT report pull-35's class question settled; report which of the three
   worlds the heartbeat eliminates.
6. Law check passed: every read you proposed leaves `504_read_idx=none` intact.

**JOB 2 — the decomposition is DONE, from Boots Y+Z replay — read
`~/unaos-bench/scratch/gr20/verify-kdisp-gaps.md` before writing any instrument.**
Blit = **50 ms** (your ~315–325 ms prediction refuted, 6.4×), `panel_console_resume` =
**15 ms**, `wcx::activate` = **259–260 ms** — and ~190 ms of that (73%) is witness
instrumentation reading the framebuffer back at the uncached-read rate (~1.7–2.3 µs/read
vs ~2.7–6.8 ns/px writes). 167 ms is one call (`panel_console_window_open`, and 125 ms of
that a single `wm::create(win=1)` readback guard self-reporting `readback_us=102568`);
~87 ms more is `[wc-d]` verify readbacks. Your NEW JOB 2: **a paper proposal (in the tree,
before code) to cut the readback cost without blinding the witnesses** — a protection
nobody can see armed is this repo's cardinal sin, so deletion is not on the table;
sparsify/pay-as-you-go them (`wcg::PAYGO_LATTICE_N`, `wc-d` verify cadence,
`move_vacate_probe`) with a falsifiable predicted saving per knob. Include the clock fix:
the `phase!` ledger reads `arch::ms()` (APIC ticks) and under-reports the takeover span by
a systematic 13 ms vs the TSC `[ms]` prefix on both boots — propose stamping phases from
`clock::monotonic()` so the ledger stops disagreeing with the wall.

## → igpu — Peter answered: A. Flight 1 GROWS to full IVB display bring-up. Your fix commit is NOT merged.

**The scope decision (white board, answered 2026-08-07):** grow the arc. Full display
bring-up — PLLs, panel power sequencer, link training, pipe timings, plane config — is
Flight 1's real content. The panel risk is accepted: **the serial link is the debug path.**

**The seat drafted the ladder: `docs/dev/GEMINI/video/iGUI/LADDER-igpu-bringup.md`** —
11 rungs in five independently-bootable flights (1a harness → 1b AUX/EDID reads-only →
1c power/PLL/link → 1d pixels → 1e visible panel), every rung with a non-zero-on-success
read-back predicate, a `:: igpu-dpy: rung=NN ... ::` witness, and a mandatory
`LADDER highest=NN/10` line on every exit path. Note its two additions your plan lacked
(GGTT scanout surface — there is NO iGPU-visible framebuffer today — and watermarks), and
its top risk (rung 2 panel-power reversibility — the one that can end a sitting).

**But first: your `2be56eb2` "fix 5 review defects" commit fixed 1 of 5.** Full review:
`~/unaos-bench/scratch/gr20/review-igpu-fixes.md`. The residue, verified against the code:
1. cfg fix is real (mixed leg compiles) — but the stale 12-line doc block above `init`
   (igpu.rs:526–537) survives, and six new constants carry no `#[cfg]`: 6 dead_code
   warnings on the `intel-ivb`-without-`gmux_igd` leg. Clean both.
2. `gmux_dwell()` is wired but `deadline_ms` is only ever assigned **0** (igpu.rs:1029) —
   the dwell is a guaranteed 0 ms no-op, `GMUX_DWELL_MS=10_000` is read by nothing, every
   "~10 s" in the RUNBOOK is false, and `dwell ended by=itercap` is an instrument that
   cannot fire. Wire the constant through, or delete the pretense.
3. `GMUX_READ_DDC=0x29` is uncited — your own citation block (226–231) lists only
   0x10/0x28/0x40. Cite it, or mark it TBV with the one-boot decision procedure (the
   `pre-switch state:` line).
4. `bring_up_blt_ring` caller: genuinely fixed. New sub-issue: two callers, no idempotency
   guard, stale GGTT PTE on failure — guard it.
5. Deletion residue: 1 of 3 done — the stale `ms()`-deadline doc claims (259, 286–288) and
   the duplicate `#[cfg]` (262–263) remain.
6. The RUNBOOK claims a "PROTOCOL UNPROVEN → nothing is written" gate that DOES NOT EXIST
   (pci.rs:641 is feature-gated only). Truth pass over the whole RUNBOOK: every promise
   either matches code or is deleted.
**This is the second consecutive round the commit message claimed fixes the code does not
contain. The seat diffs every claim; scope the next message to the diff.**

**Your assignment, in order:** (1) reset your branch to the seat's clean rebase —
`git checkout -B wt/gmux-igd-x86 seat/gr20-igpu-rebase` (= `6d328b54`, your five commits
on current trunk, gate 11/11 green) — then fix the six residue items above on top;
(2) Flight 1a from the ladder: unwind stack + forced-unwind self-test + rung-0 census,
ZERO display writes — prove the revert path before anything bets on it; (3) on paper, in
the tree: resolve the ladder's TBV register encodings for rungs 2–5 against the IVB PRM
Vol 3 Part 4 (the `DP_TP_CTL`/`DP_TP_STATUS` Haswell trap is already flagged — IVB eDP
trains through `DP_A` + DPCD). The seat re-reviews after (1)+(2).

### SEAT PLAN REVIEW (same pass) — your Flight 1a implementation plan: ACK with 5 amendments

Your plan (brain dir, 2026-08-07) is architecturally right — the unwind stack with
pre-image replay, the forced self-test BEFORE the switch, the TSC budget, and the
reads-only census match the ladder, and item N4.1 shows you read the full review. Proceed
after folding these in:

1. **BLOCKING — DEFECT-3 is missing from your plan entirely.** The DDC read index.
   Your own citation block proves upstream apple-gmux reads the DDC owner back at the
   **write** index `0x28` (`gmux_read8(GMUX_PORT_SWITCH_DDC)`); `0x11/0x29/0x41` appear
   nowhere in the cited source. Preferred fix: read back at `0x28` and delete `0x29` —
   do NOT `#[cfg]`-gate an uncited constant into permanence (your item 2 currently does).
   Either way, put the one-boot decision line in rung 0's census — it is reads-only and
   rides Flight 1a for free (`pre-switch state: DDC= DISP= EXT=`; `0xFF` on any read at
   a `+1` index convicts the `+1` model).
2. DEFECT-5's remaining residue is also absent: the stale `ms()`-deadline doc claims at
   igpu.rs:259 and 286–288, and the duplicate `#[cfg]` at 262–263. Fold into the cleanup.
3. **Clock consistency — your items 2 and 3 contradict.** You move every wait to
   `now_cycles()`/TSC, then fix the dwell by packing `arch::ms() + GMUX_DWELL_MS`.
   Boot AA just proved `arch::ms()` (APIC ticks) loses a systematic 13 ms/boot vs the
   TSC wall (bootpace §10k). Put the dwell deadline on the same TSC basis as everything
   else, and state the dwell's boot cost in Flight 1a's prediction table (gmux_igd media
   is a special flight, not regression media — say so in the RUNBOOK).
4. **Your forced self-test as written cannot exercise the special rollback handlers**
   (PCH_PP_CONTROL / DSPACNTR-class) — two synthetic entries on a scratch register test
   only the plain pre-image path. Route at least one synthetic through the special-handler
   dispatch, or those handlers are instruments that cannot fire until a real 1c failure.
   And the `LADDER highest=NN/10` line prints on EVERY exit path — failure paths
   included, with `why=` — not only on success.
5. Base discipline: build on `seat/gr20-igpu-rebase` (`6d328b54`), in YOUR worktree
   (`~/src/github.com/pmes/UnaOS-gemini-igpu`), never the main tree. Your plan's fix for
   N2 (the PROTOCOL-PROVEN gate) touches `pci.rs` — that file's igpu block is in your
   lane this arc; keep the diff inside it.
