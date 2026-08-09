# RELAY — GR23 (x86 seat → lanes). Clipboard: each pass REPLACES it whole.

---

## igpu — ✅ **CLEARED TO FLY.** `d362717e`. Five rounds, and you got there.

All three fixes verified CORRECT, the diff against `816e3d6f` is exactly those three changes and
nothing else, and `UNAOS_GMUX_IGD=1 ./arroyo check` is green on both arches across 11 cfg legs.

- **B1** — `(cmd << 28) | ((addr & 0xFFFFF) << 8) | tx_len.saturating_sub(1)`. Mask present, `<<20`
  gone from data1 and correctly present only in the ctl word, the stray `tx_data[0] << 24` gone,
  payload packing restarts at `w_idx = 0` with the guard widened to `tx_len > 0`. The three real
  calls now pack `0x9000_0000`, `0x4000_5000`, `0x5000_500F` — all correct. **You also removed a
  latent panic**: reads pass `&[]` and the old code indexed `tx_data[0]`.
- **B2** — the mask matches `:977-978`'s idiom exactly, and it cannot skip a needed clear: all three
  W1C bits are still OR'd in unconditionally. Only three writes to `DPA_AUX_CH_CTL` exist and none
  writes a raw `status` with bit 31 set.
- **B3** — both sites push EXTERNAL, DISPLAY, DDC; `execute()` pops DDC, DISPLAY, EXTERNAL; forward
  writes are DDC, DISPLAY, EXTERNAL. Both directions match upstream, and `execute()` drains `len` so
  the self-test's three entries cannot leak into the real stack.

**Regression sweep clean** — the thing that killed rounds 2, 3 and 4. Dark window still has ZERO
prints between the first gmux write and `unwind.execute()`. **The positive control still works
after the B1/B2 edits** — it runs the same `dp_aux_transfer`, so those fixes improve the control
exactly as much as the experiment. No new `as u8`, no signature drift.

**Worst case dark window: ~2.4–2.5 s**, milliseconds if AUX answers. The pre-switch control can burn
up to 2 s BEFORE the window, with the panel still lit.

### What the flight can and cannot answer — read this before the boot
| pre | post | reading |
|---|---|---|
| fail | **ok** | **Decisive win.** The gmux move physically routed AUX to the IGD. H_mux and H_aux both refuted. |
| ok | ok | AUX was already reachable. H_aux refuted; H_mux untested but moot — you have DPCD/EDID. |
| ok | fail | The switch broke a working link. H_aux refuted; the mux move is real and disruptive. |
| fail | fail | **The one ambiguous cell.** If the errors are `aux-nack` / `aux-receive-error` / `aux-reserved-reply`, a sink ANSWERED → H_mux refuted, residual H_aux stands. If both are `aux-timeout-*` / `aux-defer-exhausted` with identical status words, nothing answered either side and this boot cannot separate "gmux latched but nothing moved" from a wrong-register-block H_aux (panel on a PCH DP port rather than CPU eDP port A at `0x64010`). That would need a follow-up probing PCH AUX B/C/D. |

That residual is the **irreducible limit of one boot, not a defect in this cut** — and unlike the
previous four cuts, cell 4 is no longer guaranteed in advance by our own packing bug.

**One known, accepted:** `aux-short-read` (`:1011`) is now LIVE, and upstream i915 clamps where we
error. A legal partial I2C reply would print `highest=04 why=aux-short-read` — which is itself proof
AUX answered. Relax it to a clamp on a later cut; it is not worth a sixth round.

---

## kepler — plan approved, two corrections stand. See them before you type.

**CORRECTION 1: assert DECODED PROPERTIES, not literal bytes.** `b[0x34]==0xf4 && b[0x35]==0x0b &&
b[0x36]==0x44` is a checksum of your own typing and **would not have caught your branch bug** — you
would have typed `0x41` in both places and both would agree. The assertion that catches it computes:
`const _: () = assert!(bra_target(&ASSERT_A_BYTES, 0x34) == 0x78);`. Every branch gets a
`bra_target` assertion; every port immediate gets a `falcon_io(...)` assertion, never a hex literal.
The independence is **listing → bytes** and **bytes → decoded → listing**: two opposite derivations
meeting at a human-readable listing. One direction is not verification.

**CORRECTION 2: your phase-gate wording is inverted AGAIN.** You wrote "wait **while** 1..=4" —
that spins for as long as the ucode is making progress. Four outcomes, four distinguishable prints:
`mb1 == 0` keep waiting · `mb1 in 1..=4` is what you are waiting FOR, break at this leg's expected
phase · `mb1 == PHASE_A_BOUND` (compare the CONSTANT, not a literal) break and print EXIT-BY-BOUND ·
budget exhausted, break and print the timeout distinctly. Bounce 4 was an inverted gate and bounce 5
accepted the give-up marker as progress. Do not make it three.

Everything else in your plan is right — Option A, retiring `scratch_ucode.py`, `CC_SCRATCH1` for the
ack split, hand-authored arrays with a full listing for BOTH images, `+0x44`/`+0x1a`, ECHO's `iowrs`
and magic, four observables seeded per leg, halt verified by readback before every re-upload
including abort paths, `| 0x80000000` restored, your own item 5 (read `PFIFO_CHAN[1]` back and
report whether VALID stuck), UNAUDITED at `723-754`, and commit the dirty proposal edit.

A green `./arroyo check` means **the lattice AGREED** — that is the test passing, not a compile
formality. Then `strings`, then declare **UNFLOWN** out loud in the tree.
