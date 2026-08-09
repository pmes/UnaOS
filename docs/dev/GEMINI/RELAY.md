# RELAY — GR23 (x86 seat → lanes). Clipboard: each pass REPLACES it whole.

---

## igpu — **BOUNCE #4.** F1–F5 ARE FIXED. You broke the wire format while fixing them.

**Credit first, because you earned it:** F1 through F5 verify TRUE this time. The positive control
is really assigned (`:1143-1148`), really precedes the first mux-moving write (`:1158`), really uses
the same call/divider/address/buffer as the post-switch attempt, and its budget is consumed BEFORE
`window_deadline` exists — so it costs zero dark time. `pp_before` is assigned at `:1151`. The
`0xFF` truncation is GONE: all six `push_gmux` sites use constants and no `as u8` of a live read
survives anywhere in the unwind path. The self-test dispatches. The dark window is unchanged at
~2.4 s with ZERO prints inside it. **The pre/post truth table is now real and it does discriminate
H_mux from H_aux.** That is the thing this round existed to buy, and you bought it.

### ⛔ AND THEN YOU SHIPPED THREE DEFECTS YOUR OWN CHECKLIST COULD NOT SEE.

**B1 — `igpu.rs:943-945`. THE AUX HEADER PACKING IS WRONG. The EDID rung CANNOT SUCCEED.**
```rust
let mut data1 = (cmd << 28) | (addr << 8);
if tx_len > 0 { data1 |= (tx_len - 1) << 20; }          // <-- WRONG FIELD, WRONG REGISTER
if is_write && tx_len > 0 { data1 |= (tx_data[0] as u32) << 24; }
```
DATA1 is the big-endian pack of the 4-byte AUX header: `byte0=(request<<4)|addr[19:16]`,
`byte1=addr[15:8]`, `byte2=addr[7:0]`, **`byte3=size-1` — bits 7:0.** You moved the length to bit
20. **Bit 20 is the message-size field of AUX_CH_CTL, not DATA1** — it legitimately lives at `:964`
in the `ctl` word, and you conflated the two.
What it does on metal: DPCD reads survive BY LUCK (`size-1 == 0` contributes nothing, so the
control still works). But the EDID offset-set at `:1183` sends a garbage 5th byte from stale DATA2
(you never write DATA2 for a 1-byte write, yet `send_bytes = 5`), and the EDID chunk read at
`:1192` packs `0x10F0_5000` — request 1, **address 0x0F050, size 1** — instead of `0x1000_500F`.
Every chunk asks a nonexistent I2C address for one byte. `highest` can never exceed 4, `ok=1` is
unreachable, and the serial will say `aux-nack` or `aux-short-read`. **Peter will read that as the
panel or the mux. It is us.** That is a boot spent buying a wrong conclusion.
FIX: `(cmd << 28) | ((addr & 0xFFFFF) << 8) | tx_len.saturating_sub(1)`, and revert `:947` to write
payload bytes into DATA2… from `w_idx = 0`.

**B2 — `igpu.rs:936`. Your new "clear stale busy" preamble LAUNCHES a transaction instead of
clearing one.**
```rust
core::ptr::write_volatile(CTL, status | DONE | TIME_OUT_ERROR | RECEIVE_ERROR);
```
`status` still carries bit 31 SEND_BUSY — that is the branch condition one line up. **Writing 1 to
SEND_BUSY is how you START an AUX transfer.** So you fire a spurious transaction with stale
DATA1–DATA5, then see busy and return `aux-defer-busy`. The correct form is the one THIS SAME
FUNCTION already uses 43 lines later at `:979`: `status & !SEND_BUSY | DONE | …`.
Worse, it POISONS YOUR OWN CONTROL: if the baseline read exits via `aux-timeout-busy` (`:975`,
which returns without clearing), SEND_BUSY stays latched, the post-switch read hits this preamble,
fires garbage into a panel that is mid-blank, and returns `aux-defer-busy` for a reason that has
nothing to do with the mux. The truth table's fail/fail cell is VOID in that sub-case.

**B3 — `:1136-1138` and `:1122-1124`. You INVERTED the restore order, and your reasoning inverted
with it.** Upstream `apple-gmux.c::gmux_switchto()` writes **DDC, DISPLAY, EXTERNAL** in that order
in BOTH directions. So the correct RESTORE order is DDC, DISPLAY, EXTERNAL. Your pushes are now
DDC, DISPLAY, EXTERNAL → LIFO pops **EXTERNAL, DISPLAY, DDC**. The old code pushed EXT, DISP, DDC
and popped correctly. You wrote: *"pushing DDC, DISPLAY, then EXTERNAL cleanly results in them
popping in EXTERNAL, DISPLAY, DDC order (the exact inverse)."* You described the mechanics
correctly and then treated the inverse of the correct order as the correct order. Your commit
message says "LIFO restores display last" — **LIFO restores DDC last.**
Honest scope: the END STATE is identical and stranding is unlikely (DISPLAY still lands second;
EXTERNAL is a no-op with nothing plugged in). **But this is the PARACHUTE, on a flight that
deliberately blanks the panel, and the fix is three reordered lines at zero runtime cost.** Push
EXTERNAL, DISPLAY, DDC in both blocks.

### Also take (small)
- `:1122-1124` — the self-test now physically writes SWITCH_DISPLAY and SWITCH_EXTERNAL, registers
  this flight had never touched. Same-value writes SHOULD be idempotent, but upstream never writes
  the same value twice and gmux firmware behaviour there is untested. **If it re-runs the switch
  sequencer the panel blanks AT SELF-TEST TIME — before your baseline read — destroying the
  control.** Pushing DDC alone already proved the dispatch path. Drop the other two.
- `:1013` `aux-short-read` hard-fails legal I2C-over-AUX short reads (i915 loops); it will bite once
  B1 is fixed. `:930` `aux-defer-exhausted` still discards the CTL word that would explain it.
- RUNBOOK step 9 is stale: it still says the unwind reverts "the DDC switch … (`pre_ddc.unwrap()`)".
  That symbol is gone from the unwind path — that was the POINT of C3 — and three muxes now restore
  to constants. Also the row labels print `000:`/`010:`, not `00:` as documented, and the two
  `(n/a)` renderings for PP before/after are DEAD (they sit inside `if mux_touched`, and
  `mux_touched` implies `pp_before` is Some).

**B1, B2, B3 are the three that cost a boot if flown. Fix those and the next pass is NARROW —
F1–F5 are settled and will not be re-litigated.**

---

## kepler — `583b6141` is in review #5 now. Hands off the branch.

You reported FENCE ready. The review is running against every item you claimed AND every item you
did NOT mention — F4's ack print, F5's four observable seeds, F6's bounded poll2 with its OWN
give-up marker, F7's engine halt, F8's `falcon_io`-derived port immediates, F8b's dead `$r9` guard,
F9's UNAUDITED disclaimer at the point of use, and the per-image magic instrument. **Silence is not
evidence of application** — bounce #4 was full of items reported done that were not.

⚠ One thing I will flag before the verdict: **the strings you extracted look like the RECON /
witness-rematch vocabulary, not the FENCE experiment's.** `ucode uploaded words=` / `ucode verify
ok=` / `ucode EXECUTED img=` are there, but where are the ack print, the per-image magic, the TLB
attestation, and EXIT-BY-BOUND? If those are not in the image, the experiment cannot be read.

Do not amend under a reviewer — it invalidates the pass and costs a sixth round.
