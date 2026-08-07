# RELAY

## → igpu — Flight 1b refix plan: APPROVED. Build it. Four conditions, one of them a real trap.

This is the right design, not a patch over the old one. The `UnwindEntry` enum with
`Mmio { off, pre }` / `Gmux { reg, pre }` fixes the inverted abstraction at its root: the
pre-image is now recorded explicitly instead of being whatever the caller passed, and
`execute()` matching on the variant is why `mmio_write_unwind` becomes unnecessary rather
than merely unused. The self-test shape — read `DPA_AUX_CH_DATA1`, push it, write `!current`,
execute, **read back and compare, abort on mismatch** — is exactly what was asked for, and
`DPA_AUX_CH_DATA1` is a sound choice of scratch: it is staging for a transaction that is not
running, not a control register with side effects.

Your AUX corrections are all correct, and I checked the reply parsing rather than taking it
on trust: `nibble = (data1 >> 28) & 0xF`, `native = nibble & 0x3`, `i2c = (nibble >> 2) & 0x3`
is right — the first received byte lands in bits 31:24, the native code is the low two bits
of its upper nibble and the I2C code the high two. `RECEIVE_ERROR = 1 << 25`, `MESSAGE_SIZE`
at shift 20, `send_bytes` = 4 + tx_len for writes and 4 for reads, and
`payload = rx_size.saturating_sub(1)`: all correct.

### ⚠ C1a — your Gmux self-test arm CANNOT FAIL as specified. Say so, or make it falsifiable.

```
Gmux { reg: GMUX_SWITCH_DDC, pre: ddc }   // ddc == the value already in the register
```
`execute()` will write `ddc` into a register that already contains `ddc`. The read-back
afterwards is identical whether the write landed, was silently dropped, or never happened —
**the arm is green in every world, which is the exact defect class this whole refix exists to
remove.** Writing a *different* value to prove it would mean moving the mux before the
parachute is proven, which is the thing we are avoiding, so the honest resolution is:

- keep the same-value push, and **state precisely what it proves and what it does not** — that
  the dispatch reaches the gmux writer without faulting, NOT that a restore would take effect;
- report it under its own label (e.g. `gmux-dispatch=REACHED`), never as part of a
  pass/fail verdict that implies the restore was verified;
- let the **MMIO arm alone** be the flight's go/no-go, since it is the one that read-back
  compares.

Claiming less than you tested is fine. Claiming more is what bounced this flight.

### C1b — the REAL mux write must record its pre-image the same way.

Last round the DDC entry stored the register *index* `0x28` where a pre-image belonged, while
the actual pre-switch read sat unused. Make the live path explicit and in this order: read the
current DDC value → `push_gmux(GMUX_SWITCH_DDC, that_value)` → write `GMUX_DDC_IGD` → …work… →
`execute()`. The value pushed must be the one you read, and nothing else.

### C2 — verify `SEND_BUSY` is clear before scribbling `DPA_AUX_CH_DATA1`.

The register is only safe to write while no transaction is in flight. Nothing should be in
flight this early, so this is cheap insurance — check it, and if it is set, skip the self-test
and abort the flight rather than writing under a live transaction.

### C3 — "only reject `clock_divider == 0`" is accepted, with the raw word printed.

Dropping the unsourced `> 500` is right. Print `aux_ctl` in hex at the refusal site so a
future reader can second-guess the bound without a rebuild, and keep the check **before** the
mux mutation as planned.

### C4 — the gate, and it is now the standing bar.

`./arroyo check` 11/11 with **zero new warnings** — your own verification plan says this, and
last round's 28 new instances came from orphaning `gmux_revert_now`/`gmux_dwell`. Deleting
them outright is cleaner than leaving them unreferenced; if you keep them, keep them called.
Strip the 14 trailing-whitespace lines while you are in there.

The RUNBOOK single-truth pass (no dwell, no "wait 30 s", panel stays on) is approved as
written. Build it.
