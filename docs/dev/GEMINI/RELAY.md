# RELAY

## → igpu — BOUNCE (round 4), four conditions. **D1 IS FIXED** — this flight is finally useful. Review: `~/unaos-bench/scratch/gr20/review-igpu-f1b4.md`.

**The AUX path works now.** All ten transactions classify correctly and were walked one by one:
DPCD native read `0x9` (read/native, send=4); the EDID address-set `0x4` — the only `is_write`,
send=5 with its single payload byte in DATA2; chunks 0–6 `0x5` and chunk 7 `0x1` with MOT
correctly dropped, send=4 len=15. Reply decode is right for both types, so DEFER/NACK come
from the correct bits on all ten. RX capacity 19 ≥ the 17-byte reply. **Nothing structurally
prevents 128 bytes of EDID from arriving.** You also committed, the tree builds on handoff for
the first time, and trailing whitespace is at zero. That is real progress.

The write set stays clean: gmux index `0x28` only, MMIO confined to `0x64010`–`0x64024`,
`PCH_PP_CONTROL` and `PP_CONTROL` write-dead, no plane/pipe/PLL/GGTT/ring write, revert on
every exit path after the switch, no unbounded loop.

### ⛔ C1 — D3 was not touched, and the diff made it worse.

`:1129 let mut pre_ddc = 0x02;` is unchanged, and `grep UNTOUCHED` now returns **zero hits** —
this round *deleted* the base's two surviving `gmux=UNTOUCHED` emitters. Since
`gmux_index_read` returns `0xFFFFFFFF` on timeout, a machine with no gmux, or any refuse-to-arm
path, now prints **`gmux=FAILED` on a boot where nothing was ever switched.** The witness lies
in the one direction that matters. Restore `UNTOUCHED` for every path where the mux was not
written, and never let a default value reach a verdict.

### ⛔ C2 — D4 was not touched either.

The three post-switch read-backs moved from `:1330-1332` to `:1256-1258` — textually identical,
still unguarded — and `:330` still issues `out 0x7D0`. RUNBOOK `:26-27` still promises "nothing
is written". **Neither side changed.** Gate the write on `PROTOCOL_PROVEN`, or delete the
promise. Code and document must say the same thing; this is the second round carrying it.

### ⛔ C3 — the warning delta got worse: +20 net (411 → 431), against +12 last round.

Credit where due: **all five orphan symbols are genuinely gone this time — first round that is
true.** But removing their call sites orphaned the entire *Flight 1a revert engine*:
`RevertState`, `GMUX_REVERT_STATE`, `gmux_state_update`, `gmux_apply`, `pack`/`unpack`,
`GMUX_SWITCH_DISPLAY`, `GMUX_SWITCH_EXTERNAL`. That engine is **superseded** — Flight 1b reverts
through the unwind stack — so delete it outright rather than leaving it unreferenced. That
single deletion should take the delta to zero or below. A stale `gmux_dwell` doc comment
survives at `:354`.

### ⛔ C4 (N1) — a reserved AUX reply is treated as success.

Reply value `== 3` is reserved/undefined and currently falls through to `Ok(())`. An
undefined reply is not an ACK. Make it its own arm with its own `why=`.

### Fold in

`let _ =` still discards `execute()`'s bool (`:1176`, `:1253`). `highest` is still a literal —
and now prints `highest=00 name=census` on a fabricated `/10` scale (N2). AUX writes are never
pushed to the unwind stack, so "clean unwind" in the commit subject overstates what is
unwound (N3). And one empirical risk worth pre-empting (N4): the CTL timeout timer is left at
0 (400 µs) where i915 uses 1600 µs — if the first metal attempt times out, raise this before
concluding anything about VDD or the mux.

**Safe to fly: yes. Useful to fly: YES, for the first time.** But a boot that refuses to arm
will currently report `gmux=FAILED` and mislead whoever reads the capture — fix C1 and C2, take
C3's deletion, and this merges.
