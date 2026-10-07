# GEN7R9 (B504, R101) — rung R9: the first 3D-pipeline step on the RCS

Branch `exec-rmbp-gen7r9`, cut from 668cdd95 (merge19 fold + flight 27). Knob `UNAOS_IVB3D_R9=1` → feature
`gen7r9` (implies `gen7`), default OFF. Code: `unaos/crates/kernel/src/drivers/gpu/gen7_r9.rs`
(`//! CHARTER: Kernel — driver`), a `#[path]` child of `gen7.rs` like `gen7_blit.rs`.

## Finding (the wire, flight 27, `f27-boot1.log`)

- The RCS ring is ALREADY proven on this part: `gen7: r6 verdict=r6-sentinel-hit by=mt … any_ctl_enabled=1
  any_head_moved=1 any_sentinel=1` — R6 armed the RCS (0x2030..0x203C) in a boot-bank ring page under R3's held
  `mt` wake and executed MI_STORE_DATA_IMM. R7/R8 moved the same envelope to the BCS
  (`GEN7LADDER … r7=r7-blit-verified -> PASS`, `GEN7R8 … -> PASS`, `GEN7: ring=bcs r8=PASS blits=3`).
  So R9's ring half is R6's arm, not a new one.
- The 3D half has NO page in the tree. `PIPELINE_SELECT`, `STATE_BASE_ADDRESS` (gen7 10-DW form) and
  `PIPE_CONTROL` (post-sync write immediate, CS stall) are listed as owed in `gen7b.md` §4 (Vol1 Pt3, Vol2 Pt1)
  and `gen7.md` §5 says R9 is parked for want of them; no opcode, DW count or bit for any of the three is
  transcribed in `gen7.md`, `gpu_spec.md`, `SHUTOUT-REGISTER.md` or `gen7.rs` (searched). R83/R95 §2: no bit is
  guessed.

## The seam

Kernel driver (`gen7` ladder), no store, no second implementation: R9 is a child of `gen7.rs`, reads R6's
verdict through one note at R6's verdict line, and runs under `tests gen7` (R80) right after R1–R7, before the
`GEN7LADDER` line, which carries `r9=<word>` when built (byte-identical line when not).

## Shape

R9 holds a citation table — one row per command it needs (`PIPELINE_SELECT`, `STATE_BASE_ADDRESS`,
`PIPE_CONTROL`), each `src: Option<&str>` naming the PRM page once it is in the tree. The rung:
1. GATED unless R6 read `r6-sentinel-hit` this run (the RCS executes) — `-> GATED reason=r6-<verdict>`.
2. DECLINED at the first row with no source, by name — `-> DECLINED reason=page-PIPELINE_SELECT-uncited`.
3. Writes nothing on either path (`writes=0`). The executing path (R6's RCS arm + the three commands + a
   post-sync immediate into a pinned GGTT page, read back) is written WHEN the pages land — not before.

## Milestones

- M1 — design (this file).
- M2 — the knob (Cargo `gen7r9 = ["gen7"]`, arroyo `_feats` arm + aarch64 strip, builder arm, k8-reach NA row).
- M3 — `gen7_r9.rs`: the table, the gate, the decline, the witness; the R6 note and the ladder hook.
- M4 — the gen7.rs hooks made LINE-NEUTRAL (the card line carries `gen7`+`gen7r8`, so the gen7-on, R9-off image is
  the one that must not move): the R6 note, the `r9::run()` call and the two `GEN7LADDER` variants are same-line
  `#[cfg]` folds BEFORE each line's first `//` (gen7.rs:4577, 7368/7372, 7374/7378, 7416); the original ladder prints
  stay byte-for-byte as built under `#[cfg(not(feature = "gen7r9"))]` attached from the line above (no column
  moves); `mod r9` is a FILE-TAIL append (gen7.rs:7493-7498). `diff` against 668cdd95's gen7.rs: 6 lines changed in
  place, 6 appended at the tail, none inserted.

## Witness (the next metal boot, `UNAOS_IVB3D_R9=1`, type `tests gen7`)

    [gen7] r9 ring=rcs r6=r6-sentinel-hit select=uncited sba=uncited pipe_control=uncited sync_val=- read=- writes=0 -> DECLINED reason=page-PIPELINE_SELECT-uncited
    :: GEN7LADDER: rungs=R1-R7 ggtt=boot-bank wake=… r7=r7-blit-verified us=… replay=0 r9=declined -> PASS ::

On a run where R6 did not read `r6-sentinel-hit` (or never reached its verdict) the line reads
`… writes=0 -> GATED reason=r6-did-not-execute-on-the-rcs` and the ladder carries `r9=gated`. Built without the
knob, both lines are exactly flight 27's (no `[gen7] r9`, no `r9=` field).

The executing form, once the three rows carry a source:
`[gen7] r9 ring=rcs select=3d sba=ok pipe_control=posted sync_val=<hex> read=<hex> -> executed`.

## Owed

The three PRM pages (IVB 2012 PRM Vol1 Pt3 PIPE_CONTROL; Vol2 Pt1 PIPELINE_SELECT, STATE_BASE_ADDRESS) — or a
seat ruling that an MIT source (Linux i915 `intel_gpu_commands.h`, Mesa `genxml/gen7.xml`) is citable the way
R95 §2 allows nouveau. Then the executing path: R6's RCS arm, the stream
`PIPELINE_SELECT(3D); STATE_BASE_ADDRESS; PIPE_CONTROL(post-sync imm → pinned GGTT page); MI_BATCH_BUFFER_END/NOOP pad`,
the read-back, R6's teardown.
