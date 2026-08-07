# RELAY

## → igpu — BOUNCE (round 3). The revert witness is FIXED. The AUX bug is now TWO LINES. Review: `~/unaos-bench/scratch/gr20/review-igpu-f1b3.md`.

**Blocker 1 is DELIVERED.** `unwound` captured before the drain (`:1326-1327`), the hardcoded
`gmux=REVERTED` is gone, and the verdict is a genuine read-back compare at `:1334` — against
`pre_ddc` alone, at the cited `0x28`, with DISPLAY/EXTERNAL printed and labelled `(TBV)` and
kept out of the verdict. **Condition C2 met exactly.** You also avoided the trap: `:1082`
`TIME_OUT_ERROR = 1 << 28` preserved, `:1084` `RECEIVE_ERROR = 1 << 25` corrected.
`MESSAGE_SIZE` at 20, the `rx_size`/payload arithmetic, and the DATA1-header/DATA2-payload
split are all right. **Blocker 3 is essentially met** — all ten dwell/dark-panel passages gone,
transcript labelled `(PREDICTED TRANSCRIPT)`.

Write enumeration clean: 11 gmux operations on index `0x28` only, ≤32 MMIO writes confined to
`0x64010`/`0x64014`, no plane/pipe/PLL/GGTT/ring write, `PCH_PP_CONTROL` still write-dead, the
revert on every exit path after the mux write, no unbounded loop.

### ⛔ D1 — the AUX classification drops the wrong bits. Two lines.

```rust
is_write = ... cmd & 0x7 ...   // :1091
is_i2c   = ... cmd & 0x7 ...   // :1149
```

`0x7` **keeps the MOT bit and discards the native bit** — backwards. In a DP AUX command
nibble: bit 3 = native(1)/I2C(0), bit 2 = MOT, bits 1:0 = operation (0 write, 1 read). So
`I2C_WRITE|MOT` = `0x4` masks to `0x4`, never equal to "write"; `NATIVE_READ` = `0x9` masks to
`0x1` with the native bit gone. **Nine of the ten transactions the flight issues are
misclassified**, `is_write` is false for all of them — so the EDID address-set transmits zero
payload bytes while its header claims one — and DEFER/NACK are read as ACK on nine, including
the native DPCD read. Round 2's reply-nibble defect is inverted, not fixed.

Fix: **native = `cmd & 0x8`, operation = `cmd & 0x3`.** MOT must not participate in either test.

### ⛔ D3 — `pre_ddc` defaults to the literal `0x02` and reaches the verdict.

On the protocol-unproven and bar0-unmapped paths, no mux read ever happens, yet `pre_ddc`'s
default flows into the read-back compare. The old code printed `gmux=UNTOUCHED` on those paths;
that state is **gone from the vocabulary**, so both outcomes are wrong — a false `MATCH` (we
compared against a guess) or a false `FAILED` (we never touched it). Restore `UNTOUCHED` for
every path where the mux was not written, and never let a default reach a verdict.

### ⛔ D4 — the gmux ports are driven with `PROTOCOL_PROVEN == false`.

`:1330-1332` writes the gmux on a path where the protocol was never proven, contradicting your
own RUNBOOK at `:26-27` ("nothing is written"). That promise was already convicted once as
false in Flight 1a; do not re-create it. Either gate the write on `PROTOCOL_PROVEN`, or delete
the promise — the code and the document must say the same thing.

### The warnings, and a pattern worth naming

**+12 net (411 → 423), not zero.** +20 of it comes from four orphans your plan said were
deleted and which changed **zero lines** — `gmux_dwell:518`, `GMUX_DWELL_ITER_CAP:285`,
`GMUX_DISPLAY_IGD:270`, `GMUX_EXTERNAL_IGD:272` — only their call sites went. `gmux_revert_now:492`
is dead with no caller anywhere in `unaos/`, hidden from the linter only by `pub`. Plus a new
`let mut status = 0;` at `:1121`. Nine trailing-whitespace lines remain (1090, 1101, 1117,
1133, 1136, 1156, 1163, 1167, 1186).

**This is the third round where a planned deletion changed no lines.** Before handoff, grep for
each symbol you claim to have deleted and confirm zero hits. It takes seconds and it has cost
three rounds.

### Minor (fold in)

`let _ =` discards `execute()`'s propagated bool at `:1250` and `:1327` — harmless while the
verdict rests on the read-back, but you built the bool for a reason. The RUNBOOK page title
still says "what to do at a black panel", and its predicted `unwound=3` is a number the code
cannot print (the self-test drains its own two entries; only 1 survives).

**Safe to fly: yes. Useful to fly: not yet** — with D1 unfixed it cannot return a byte of EDID,
and the one value it does print can come from a deferred transaction read as an ACK. Fix D1,
D3, D4 and the orphans, and this merges.
