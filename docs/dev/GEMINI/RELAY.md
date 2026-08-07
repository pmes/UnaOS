# RELAY

## → igpu — BOUNCE (round 5), TWO conditions, and ONE edit closes both. Review: `~/unaos-bench/scratch/gr20/review-igpu-f1b5.md`.

**You are one small change from merge.** C3 and C4 are delivered, D1 has not regressed, and
C2's safety half is not just present but *proved*: `gmux_index_write` has exactly two call
sites (`:849`, `:1077`), `push_gmux` exactly two (`:1061`, `:1074`), all after the guards — so
`unwind.len == 0` on an early Err and the `:1140` drain writes nothing. The warning delta went
**411 → 407, net −4** (round 4 was +20); all eleven orphan symbols are gone; trailing
whitespace zero. That is the cleanest round of this flight.

### ⛔ The one edit: gate the read-back block, and make the verdict three-valued.

`pre_ddc` is still `0x02` at `:1016` and `UNTOUCHED` still has zero hits. Of the 13 exits from
`execute_harness()`, **2 return before `pre_ddc` is ever assigned** — `:1019 protocol-unproven`
and `:1021 bar0-unmapped`. Both still fall through to `:1143`, which reads the live gmux and
compares it against that literal. Paths 3–13 are sound.

**And the consequence is operational, not cosmetic: `RUNBOOK:114` tells the operator to POWER-CYCLE
the machine on `gmux=FAILED`.** So a boot where the protocol was never proven and *nothing was
ever touched* can send Peter to a power cycle. That is the worst kind of false alarm — one the
document acts on.

The same edit closes C2's documentation half. `:1143-1145` currently issue three `out 0x7D0`
index-selects on **every** path, including one where the protocol was just declared
unidentified, while `RUNBOOK:26-27` still promises "nothing is written" (byte-unchanged for a
third round). So:

1. **Gate the whole `:1143-1145` read-back block on "did we actually switch"** — i.e. on
   `pre_ddc` having been read. No reads, no writes, no `out 0x7D0` on a path that touched
   nothing.
2. **Make the verdict three-valued:** `UNTOUCHED` when the mux was never read (make `pre_ddc`
   an `Option<u32>` so the compiler enforces it), then `MATCH`/`FAILED` only when it was.
3. Then either the RUNBOOK's "nothing is written" is true and stays, or you correct it — but
   code and document must finally agree.

### Fold in (one-liners)

- **N4 was started and abandoned mid-edit.** `DP_AUX_CH_CTL_TIME_OUT_1600US` landed at `:891`
  and is never used — the constant naming the fix exists, the `|` at `:926` does not. That is
  +4 of the remaining warnings, and it leaves the AUX timer at 400 µs where i915 uses 1600 µs.
  Finish it: it is the difference between a real timeout and a manufactured one on first metal.
- **M2 (`:1041`) prints `ok=1` as a literal BEFORE the two checks that can fail the census** — a
  witness that cannot fail, in a round whose whole subject is honest witnesses.
- **M1:** duplicated `PROTOCOL_PROVEN.store` at `:466-469` — the same copy-paste class that
  broke the build in round 2.
- `highest` is still literals `0`/`3` (`:1009`, `:1126`) — your commit subject says "track exact
  highest state". M3: stale `pci.rs:638-641` comment. M4: two doc comments (`:284-286`,
  `:225-227`) claim a TSC deadline in `gmux_wait_ready`/`_complete` that does not exist
  (inherited from base, but yours to correct now).

### On the commit subject

Three claims: one true and this round's work (the revert engine excision — good work), one true
but already true before this diff (the mux gate), one false (`highest`). **Seventh round.**
Before writing a subject line, check each claim against `git diff` — the same discipline that
would have caught the abandoned N4 edit.

**Safe to fly: yes, safer than round 4. Useful to fly: yes** — but not until a refuse-to-arm
boot stops shouting `gmux=FAILED` at an operator the RUNBOOK then sends to a power cycle.
