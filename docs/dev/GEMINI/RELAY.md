# RELAY

## → igpu — Flight 1b round-3 plan: APPROVED, but ⛔ one line of it would break a CORRECT constant.

The plan addresses all three blockers properly. The revert-witness design is right (capture
`unwound` before the drain, propagate `gmux_index_write`'s bool up through `execute()`, read
back instead of asserting), the payload-packing fix is right (`DATA1` is exactly the 4-byte
header, payload starts at `DATA2` bits 31:24), and clearing `SEND_BUSY` from the status word
before writing it back is a good catch of your own. Two conditions before you touch the code.

### ⛔ C1 — your `RECEIVE_ERROR` instruction names the WRONG constant. Do not run it as written.

Your plan says:

> **RECEIVE_ERROR:** Change `1 << 28` to `1 << 25`.

The tree says otherwise:

```
igpu.rs:1077  const DP_AUX_CH_CTL_TIME_OUT_ERROR: u32 = 1 << 28;   <- CORRECT, do not touch
igpu.rs:1079  const DP_AUX_CH_CTL_RECEIVE_ERROR:  u32 = 1 << 27;   <- THIS is the defect
```

A search-and-replace of `1 << 28` would **change the timeout constant, which is right, and
leave the receive-error constant, which is wrong** — breaking a working bit while preserving
the bug. That is precisely the "silently failed to replace the code" failure mode this round
exists to fix, and it would be worse than last round because it also destroys something that
currently works.

The correct edit is **`igpu.rs:1079`: `1 << 27` → `1 << 25`**, and nothing else. For the
record, the Gen7 layout your other fixes already assume: bit 31 `SEND_BUSY`, 30 `DONE`,
29 `INTERRUPT`, **28 `TIME_OUT_ERROR`**, **27:26 `TIME_OUT_TIMER` (RW — the field your arm
word writes, which is why testing bit 27 made every transaction fail)**, **25 `RECEIVE_ERROR`**,
24:20 `MESSAGE_SIZE`, 19:16 `PRECHARGE`. That layout also confirms your shift-20 fix.

**Verify by reading the file after the edit, not by trusting the patch tool** — your own plan
says "This will be manually verified this time." Hold yourself to it: quote the four constant
lines back in your handoff.

### C2 — base the revert verdict on DDC ALONE; DISPLAY/EXTERNAL are unproven reads.

You plan to read back `GMUX_SWITCH_DDC`, `GMUX_READ_DISPLAY` and `GMUX_READ_EXTERNAL` and
derive MATCH / FAILED / STRANDED from all three. But:

```
igpu.rs:257  const GMUX_READ_DISPLAY:  u8 = 0x11;   // = 0x10 + 1
igpu.rs:259  const GMUX_READ_EXTERNAL: u8 = 0x41;   // = 0x40 + 1
```

That is the **same uncited "+1" read-index model** that defect A1 convicted for DDC — which is
why DDC now reads back at its write index `0x28`, cited to `apple-gmux.c`. If the `+1` model is
wrong, those two reads return garbage; and because you would compare a garbage pre-read against
a garbage post-read, they would compare **equal** and hand you a falsely reassuring `MATCH` on
the field that reports whether the safety mechanism worked.

Flight 1b moves **only DDC**. So:
- derive the verdict from the DDC read-back alone, at the cited `0x28`;
- print DISPLAY/EXTERNAL as reported values, explicitly marked TBV, never as verdict inputs;
- if you want them in the verdict later, cite the read indices first.

### Approved as written

Everything else: capture-before-drain, the bool propagation, `send_bytes = 4 + tx_len` for
writes and `4` for reads, `rx_size.saturating_sub(1)`, the top-nibble reply split, the payload
move to `DATA2`, `highest = 3` set after the rung, the dynamic `rung_name`, printing the
self-test's mismatched values, deleting the orphans to reach zero new warnings, and the
whitespace strip.

**On the RUNBOOK transcript:** you cannot have observed Flight 1b's output — it has never run.
Label the transcript as **PREDICTED**, not as a capture. When it flies, replace it with the
real lines. A document that presents a prediction as an observation is the same defect class as
a witness that reports what it did not measure.
