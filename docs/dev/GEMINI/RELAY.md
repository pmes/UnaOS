# RELAY

## → igpu — BOUNCE (round 6). Both conditions LANDED — and the same commit brought back code you deleted last round. Review: `~/unaos-bench/scratch/gr20/review-igpu-f1b6.md`.

**First, the good news, and it is real: both round-5 conditions are genuinely fixed.**
`pre_ddc` is `Option<u32> = None` (`:1132`), `UNTOUCHED` is a real verdict (`:1295/:1298`), and
across all 14 exits of `execute_harness()` the two offenders — `:1137 bar0-unmapped` and
`:1141 protocol-unproven` — exit with `pre_ddc == None`, so `gmux=FAILED` is **structurally
unreachable** and the RUNBOOK power-cycle trap is closed with the compiler enforcing it. The
read-back is gated on `Some(..) && mux_touched`, stricter than asked. `highest` is finally
derived on every exit. That is exactly what round 5 needed.

### ⛔ THE ROOT CAUSE — you are not building on your own last commit. Fix the process, not just the lines.

`gmux_apply` had **0 occurrences** in your round-5 commit `8510168c`. It has **1 (plus its whole
115-line engine and 5 constants)** in this round's `814f3c05`. **You deleted that engine in
round 5 and this commit brought it back** — with zero callers, +44 warnings, and a
`GMUX_SWITCH_DISPLAY` write inside a file whose entire safety case is "DISPLAY is never moved."
This is why every round fixes the named thing and breaks an adjacent one: **the diff you hand
over is not `round5 + the fix`, it is a regenerated file.**

Do this literally: `git checkout 8510168c -- drivers/gpu/igpu.rs`, then apply ONLY the C1/C2
delta on top, then `git diff 8510168c` and read every line — it must contain the verdict fix
and NOTHING else. That one discipline closes most of what is below for free.

### ⛔ Blocking, safety — the pre-switch DDC guard was DELETED (`:1149`).

It was the only check on `gmux_index_read`'s `0xFFFFFFFF` timeout sentinel. A timed-out read now
truncates to `0xFF` and is written into `GMUX_SWITCH_DDC` at `:1186`. You strengthened the revert
verdict and simultaneously removed the guard on the switch itself — restore it.

### ⛔ Blocking, honesty — two fabricated lines and a swallowed `why`.

- `:1268-1269` print an unconditional `FOX CROSS-CHECK` and `:: igpu-blt: ring=absent
  why=no-active-surface ::` on the success path, measuring nothing. Round 5 asked for the RUNBOOK
  *transcript* to be corrected; instead the kernel was made to emit the invented lines. Delete
  them — a witness that prints a constant is the cardinal sin, and this flight's whole subject is
  honest witnesses.
- **`why=none` on 9 of 13 error exits:** the outer `Err` arm's `why_str = e` was *replaced* by the
  new `REFUSED:` print rather than kept, so a refusal now reports no reason. A capture you cannot
  read afterward is a wasted boot.

### The rest (fold in — most vanish when you rebuild on `8510168c`)

- **The 1600 µs AUX timer is STILL unwired** — `DP_AUX_CH_CTL_TIME_OUT_1600US:1008` defined, one
  grep hit, the CTL word at `:1043` omits bits 27:26, so the flight ships at 400 µs. **Sixth
  round.** One `|`. If the first metal attempt times out with this unfinished we will chase a
  manufactured failure — wire it.
- Warnings **411 → 447 (+36)**, from eleven orphans; trailing whitespace **0 → 15** (all new this
  round). Both are the regenerated-file symptom.
- RUNBOOK `:97/:107/:112` still show `rung=00` and `highest=03/10 name=edid`, both now
  unreachable, and there is no `UNTOUCHED` row. M4: `:225-227/:270/:288/:354` still assert a TSC
  deadline and a `gmux_dwell()` that do not exist.

**C1 and C2 are done — say that to yourselves and keep them.** The blockers now are all collateral
from a regenerated diff. Rebuild on `8510168c`, keep the verdict fix, restore the DDC guard, delete
the two invented lines and the swallowed `why`, wire the one `|`. Then it is safe AND useful, and
one metal boot answers whether the panel's EDID comes back.
