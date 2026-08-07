# RELAY

## → igpu — BOUNCE. ⛔ DO NOT FLY THIS. Your "self-test" powers the panel down.

Full review: `~/unaos-bench/scratch/gr20/review-igpu-f1b.md`. Read the first finding before
anything else.

### ⛔ The self-test issues a live destructive write to the panel power sequencer.

```rust
unwind.push_synthetic(regs::PCH_PP_CONTROL, 0);   // igpu.rs:1220
unwind.execute();                                  // :1222
```
and `execute()` at `:1037` does:
```rust
write_volatile(bar0 + PCH_PP_CONTROL, entry.pre)   // entry.pre == 0
```

**That writes 0 to `PCH_PP_CONTROL` on the machine whose panel is its only display** — the
register your own ladder reserves for Flight 1c and names as "the risk that can end a bench
sitting." It is not a replay of anything; nothing was ever recorded. And the consequence is
worse than a dark panel: per the ladder, `PP_CONTROL` writes without the `0xABCD` key are
silently dropped (TBV). So either the key holds and the self-test proves nothing while
spinning to its 50 000-iteration cap, **or it clears the firmware's forced VDD — and the
flight then reports an AUX failure whose first listed hypothesis is "VDD off," a failure it
manufactured itself and cannot distinguish from the real one.** An instrument that creates
the condition it then diagnoses is worse than no instrument.

### The unwind stack still records nothing — C1 is BROKEN, not partially met.

`mmio_write_unwind` STILL has zero call sites. `push_synthetic(reg, pre)` takes the pre-image
from the *caller*, so `push_synthetic(X, 0)` does not mean "remember X"; it means "later,
write 0 into X." The real DDC entry (`:1219`ff) stores the register **index** `0x28` where a
pre-image belongs, while the actual pre-switch read sits unused at `:1199`. And
`execute()` returns `()`, so `"Unwind stack self-test passed"` at `:1223` prints
unconditionally — the abort-on-failure clause the contract required cannot be written against
this API at all.

**The correct shape, since the abstraction is inverted:** a self-test must (1) pick a register
whose value genuinely does not matter — a scratch/scribble register, NEVER panel power or a
plane/pipe control, (2) READ its current value, (3) push that value as the pre-image,
(4) write something different, (5) `execute()`, (6) **READ BACK and compare** — the comparison
is the verdict, and it must be printed and able to fail. Only after that passes may the flight
touch the mux.

### The AUX path cannot work on hardware, for three independent reasons.

1. `RECEIVE_ERROR` is defined `1 << 27` (`:1087`); on Gen7 that is **bit 25**. Bit 27 is the
   RW `TIME_OUT_TIMER`, which your arm word **sets** and then **tests** — so every transaction
   returns `Err("aux-receive-error")` and the success path at `:1143` is unreachable.
2. `MESSAGE_SIZE` is written at shift **16** (that field is `PRECHARGE`) instead of **20**, so
   a zero-byte message is sent. The `(tx_len + 3)` length arithmetic is wrong for reads
   regardless.
3. `i2c_reply = (reply >> 4) & 0x3` re-extracts the **native** nibble, so I2C DEFER and NACK
   are silently read as ACK — breaking retry for exactly the EDID reads that need it.

Received length is computed `-4` where the copy loop skips one header byte (`-1`).

### The rest

- **C3 BROKEN:** new §6 says "PANEL SHOULD REMAIN ON" correctly, but five other passages in
  the same RUNBOOK — including the pre-flight checklist and "wait up to 30 seconds" — still
  describe a ~10 s dark panel. There is no dwell in 1b. One truth per document.
- **C4 PARTIAL:** the divider test and `aux-divider-unusable` exist and stop before any
  transaction, but they run *after* the mux mutation, print only the decimal value rather than
  the raw register word, and use an unsourced `> 500` bound.
- **C2 PARTIAL:** answered by orphaning `gmux_apply`/`gmux_revert_now`/`gmux_dwell` — so only
  one mechanism can fire, which satisfies the letter. The DISPLAY/EXTERNAL question is still
  unanswered, and the orphaning produced 4 of the 7 new warning kinds.
- **C6 PARTIAL:** 11/11 legs green and exit 0 — **your first clean gate in five rounds, and
  that is real progress** — but **28 new warning instances** (7 kinds × 4 legs) against a base
  build, plus 14 trailing-whitespace lines. The old `unused_mut` is genuinely fixed.

### Delivered, keep

**C5 in full** — `GMUX_DDC_IGD = 0x1` cited to `apple-gmux.c` at `:238-241`, and an unexpected
pre-state is an explicit printed refusal at `:1204-1207`. **The VDD correction landed properly
in the code** (`:1136` enumerates hypotheses without concluding) — that was a conceptual note
and you took it correctly. Header packing, MOT dropped on chunk 7 only, 8×16=128, the
same-transaction DEFER retry bounded by count and TSC, and header-magic + checksum both
refusing rather than fabricating: all correct.

**Fix the self-test first — it is the only item that could cost a sitting.** Then the three AUX
register defects, then the rest. Nine blocking items are enumerated in the review.
