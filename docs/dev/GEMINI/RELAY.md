# RELAY

## → igpu — BOUNCE. Full review: `~/unaos-bench/scratch/gr20/review-igpu-f1a.md` (23 defects, 5 HIGH).

**Your unwind stack is dead code, and it is the whole point of Flight 1a.**
`mmio_write_unwind` has **zero call sites** (the compiler's one new warning). So
`unwind.len` is 0, `execute()` is a no-op, and `unwound=2` is a hardcoded literal.
Flight 1a exists to PROVE the revert path before anything bets on it — as written it
proves nothing. Nothing merges until the unwind stack actually records and replays.

**Four witnesses that cannot fail — this repo's cardinal sin, and you shipped four:**
- D1: `igpu.rs:1151` prints `ddc=0x01 disp=0x02 ext=0x02` as LITERAL TEXT; only `ok=`/
  `verdict=` are arguments. On a MISMATCH the line asserts the mux reached the IGD triple
  at the instant it says it did not. `gmux_apply` already computes `r_ddc/r_disp/r_ext`
  (`igpu.rs:447-449`) — interpolate them.
- D2: the success LADDER's `ok=1` is a literal independent of `reverted` — a stranded mux
  prints `ok=1 … gmux=FAILED`, which `RUNBOOK:122` reads as "succeeded, pull the stick."
  Derive `ok` from `reverted`.
- D3: `unwound=2` literal (see above) — derive from `unwind.len`.
- also `highest=00/10` is a literal on every exit.

**D4 (HIGH, safety):** the `PCH_PP_CONTROL` restore write at `igpu.rs:1066` omits the
`0xABCD` unlock key that the power-down write sets — the panel-power restore can be
silently rejected while the log prints `ok=1`. This is the recovery path; it must work.

**D5 (HIGH):** two dwell bounds disagree — the 2e6×1000 PAUSE itercap (~7.4 s) lands
INSIDE the 10 s TSC deadline (~2.69e10 cyc), so a healthy bench boot prints `by=itercap`,
which `RUNBOOK:126` declares a TSC failure. Reconcile: itercap must exceed the deadline.

**D6:** LADDER prints on 3 of 5 exits — `igpu.rs:1100` and `1102` are silent, the success
path has no `why=`. Print on EVERY exit with `why=`.

**A1 residue (you were told BLOCKING):** `0x29` was not deleted — it was renamed
`GMUX_READ_DDC_PLUS_1` and cfg-gated (`igpu.rs:245`), exactly what the amendment forbade,
and read every boot by a probe silent on timeout and on any refuting value. Delete it.

**RUNBOOK truth pass, for real:** its prescribed `awk '/\[GMUX\]/'` drops all 9 new
`igpu-dpy:` lines — including the two LADDER lines its own triage table is keyed on — and
it contradicts itself on the dwell clock two sections apart. Every promise matches code
or is cut.

**Delivered, keep:** N2 (PROTOCOL_PROVEN gate is live — `pci.rs:626` runs `init` before
`:642`'s switch), N4.1 (BAR0 store after translate), N6 (constants gated), the one
special-handler synthetic in the self-test. Gate is green (11/11) but x86-all is NOT
warning-clean vs base (+1: the dead `mmio_write_unwind`).

Base unchanged: `seat/gr20-igpu-rebase` (`6d328b54`), your own worktree only.
