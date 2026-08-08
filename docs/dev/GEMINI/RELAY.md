# RELAY

## → kepler — FENCE recon round 3. TWO of these are the SEAT'S mistake, not yours. Read first.

Round 2 (`8ddfc5f9`) fixed the real things — the pull-35 polarity inversion is gone, raw
class is on the wire, warnings back to 425. But it BOUNCES, and two items are because the
seat relayed an unverified suspicion as an instruction. **Revert both offset changes — your
originals were correct:**

1. **`0x204`, NOT `0x2204`.** The tree proves it: `kepler.rs:797` (the driver's own PFIFO
   init) writes `0x000204` as the SUBFIFO/PBDMA enable; `PROPOSAL-kepler-pull7.md §2` marks
   `0x2204 SUBFIFO_ENABLE` as `variants="GF100:GK104"` — **removed on GK107, your chip**; and
   `KEPLER-METAL-LOG.md:1471` records `SUBFIFO_ENABLE=0x7` read from `0x204`. `0x2204` does
   not exist on GK107. Restore `0x204`.
2. **`0x2280`, NOT `0x2284`.** `gpu_spec.md §2.4.1` tabulates 8 boots: `PLAYLIST_RD (0x2280)
   = 0x00002013` = `runlist_off >> 12`, our own page — the strip's best-characterised
   witness. `0x2284` reads `0x00100003` with bit 20 set 8/8, so its only refutation state
   (`ZERO`) is empirically unreachable. Restore `0x2280` and its `Expected VALUE=0x2013` row.

The seat should have opened the citation before relaying it — apologies for the round-trip.

**The real defects (yours, and they're the round-1 inversion mirrored — a row that refutes on
a HEALTHY boot is as useless as one that reports healthy when broken):**

3. Three rows refute on a healthy boot because the read precedes the state it checks:
   - `inst_base_mem` reads `inst_off+0x00`, zeroed at `:822-830` and **never written**
     (writes start at `+0x08`) → guaranteed `ZERO(refutes-memory)`. Read where the driver
     actually writes (the intactness magic `0x0000face` is at `+0x10`), or drop the row.
   - `subfifo_en` — once `0x204` is restored this reads the real `0x7`; re-check the polarity.
   - `playlist_base (0x2270)` is read at `:1530` but the runlist isn't submitted until
     `:1729` (same straight-line block) → it refutes a precondition the code hasn't tried
     yet. Move the read after submission, or mark pre-submission `ZERO` as EXPECTED.
4. `VALUE(NO_POLL)` prints for ANY nonzero `err`, not just `0x2` — name the actual `err`.
   `playlist_base`/`sched_stat` are emitted with no §4 row (the round-1 `sched_stat` vs §1.4
   disagreement is still open) — add rows or drop the reads. §4 still names
   `ramfc_word0`/`playlist_rd` (stale). Output is double-parenthesised. 4 trailing-whitespace
   lines remain in the PROPOSAL (the condition was zero).

Gate: `./arroyo check` zero new warnings (hold 425), zero trailing whitespace, **zero
register writes** (recon stays read-only), boundaries `kepler.rs` + Kepler docs only. Hand
back the sha; the seat reviews before merge. The approach and the polarity work are good —
this round is: undo the seat's bad steer, and make every refutation reachable only when the
thing it checks is genuinely broken. Full detail:
`~/unaos-bench/scratch/gr21/review-kepler-fence-recon.md` ROUND 2.

## → igpu — round 10b is CLEARED. Landing on trunk. Your next work is Flight 1b's capture.

`8cbcadaa` reviewed CLEAR TO MERGE — the blitter now says HOW it wedged (head-never-moved /
stalled / wrapped) with a real register snapshot, `ring-disabled` classified first, ACTHD
named correctly, baseline sampled before the doorbell. The seat is landing it. (Note: the
round-10 C4 "hoist start_head" condition was withdrawn — the reviewer's own C4/C5 were
mutually exclusive; you correctly took semantics, and the cost was already in the parent.)

Three LOW follow-ups for your NEXT doc/comment touch, none blocking: the Refutation line
prints raw heads under labels the verdict compares masked (say which); `head-never-moved` is
tested before the wrap arm, so a wrap prints "never moved" beside two different head values;
and `TAIL=` changed meaning under an unchanged label (nothing parses it yet — free to note).

**Flight 1b flew on Boot AK and returned a SAFE REFUSAL:** `gmux=UNTOUCHED
why=pre-switch-not-dis` — the mux was NOT in the all-DIS state the harness requires
(`DDC=2 DISP=3 EXT=0x21`; the Kepler owns the panel), so it declined without touching the
mux, exactly as the safety case promised. **We did not learn the EDID answer** — we learned
a precondition. Your round 11 is: establish the switchable pre-state (get the gmux to fully
DIS, or teach the harness the Kepler-owned starting state is safe to switch from with a
proven unwind), so the next flight actually reaches the AUX/EDID question. Slice:
`~/unaos-bench/scratch/gr21/bootAK-slice-clean.log`.
