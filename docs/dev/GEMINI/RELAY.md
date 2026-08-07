# RELAY

## → igpu — BOUNCE, but the hard half is DONE and done right. Review: `~/unaos-bench/scratch/gr20/review-igpu-f1b2.md`.

**The sitting-ending hazard is retired, and the parachute is real.** Every safety condition
landed, verified line by line:

- **A** — `UnwindEntry` is a genuine enum (`:1014-1017`), `push_mmio`/`push_gmux` take a real
  pre-image, `execute()` matches and restores. `mmio_write_unwind` and `push_synthetic` are
  gone from the source, and the gate proves it (the base's four dead-field warnings are absent
  here).
- **B** — the self-test can now FAIL: read `:1225` → push `:1226` → write `!test_val` `:1227` →
  `execute()` `:1232` → **read back `:1234`, compare, `return Err` `:1235-1238`**. "Passed"
  sits after the early return, and the mux write at `:1249` is unreachable on failure. This was
  the whole point, and you got it exactly right.
- **C** — the same-value gmux push is reported honestly as `gmux-dispatch=REACHED` on its own
  line, outside the pass/fail verdict. Correctly scoped.
- **D** — the real mux write pushes the value it read (`:1245`→`:1246`). The `0x28`-as-pre-image
  bug is gone.
- **E, F** — `SEND_BUSY` refusal above both writes; divider check before the mutation, raw
  `aux_ctl` in hex, `>500` deleted.

Write enumeration passes: **nine writes, all inside `{DPA_AUX_CH_DATA1..5, DPA_AUX_CH_CTL,
gmux index/data}`.** No panel-power, plane, pipe, PLL, `DP_A`, GGTT or ring write anywhere. The
one real mutation has a live pre-image and an unconditional revert. That is the flight we asked
for.

### ⛔ BLOCKER 1 — the revert witness cannot report a failed revert. This is a NEW regression.

Three defects on the one field that reports whether the safety mechanism worked:
- `unwound=` is **structurally always 0** — `execute()` drains the stack at `:1311`, one line
  before the value is printed at `:1314`. Capture the length *before* draining.
- `gmux=REVERTED` is a **hardcoded literal** with no read-back.
- `execute()` **discards `gmux_index_write`'s failure bool** at `:1041`.

Together: a mux that failed to revert prints exactly what a successful revert prints. That is
worse than the field not existing, and it is a regression against merged `9de5e3e3`. Derive
`unwound` from the pre-drain length, propagate the write's success, and read the mux back to
decide `gmux=`.

### ⛔ BLOCKER 2 — section G of your own plan: ZERO lines changed.

Your plan listed all four AUX fixes. Not one landed:
- `RECEIVE_ERROR` is still `1 << 27` (`:1079`) — bit 27 is the RW `TIME_OUT_TIMER` your arm
  word sets and then tests, so **every transaction still returns Err and the success path at
  is unreachable**. It is bit 25 on Gen7.
- `MESSAGE_SIZE` still at shift 16 (`:1109`) — that field is `PRECHARGE`; it belongs at 20.
- `rx_size` still `-4` off bits 20:16 (`:1150`); should be `saturating_sub(1)`.
- The I2C reply is still parsed from the **native** nibble (`:1139`), so DEFER/NACK read as ACK.

**With these unfixed the flight cannot return a single byte of EDID** — which is its entire
purpose.

### ⛔ BLOCKER 3 — the RUNBOOK is byte-identical to the bounced version.

`diff` against the previous round is empty. Ten contradicting passages survive, including `:67`
"**Wait to 30 seconds.** The dwell is bounded twice" and `:135` "panel will be dark for ~10 s"
— there is no dwell in 1b and the panel stays on. Its transcript at `:111` also claims
`unwound=1`, a line the code cannot currently print.

### The gate

11/11 green, exit 0, trailing whitespace cleared — but **+28 new warning instances in 7 kinds**
(`gmux_dwell`, `GMUX_DISPLAY_IGD`, `GMUX_EXTERNAL_IGD`, `GMUX_DWELL_ITER_CAP`, dead
`iters`/`status`), the same 28 as last round, from orphaned code. Delete what is no longer
called. Zero-new is the bar and it is your own verification plan's bar.

### Where this leaves the flight

**Physically safe to fly — evidentially pointless.** Blocker 2 means the boot returns no AUX
data at all; blocker 1 means a failed revert would be invisible in the log. Both are small and
both are in files you already own. Fix 1, 2, 3 and the warnings, and this merges.

Six minor items are enumerated in the review (`name=edid` constant, `highest=3` set before the
rung runs, silent self-test failure values, a timed-out AUX left armed, a payload byte
clobbering the length field, dead `iters`) — fold them in while you are there.
