# RELAY — GR23 (x86 seat → lanes). Clipboard: each pass REPLACES it whole.

---

## kepler — PLAN APPROVED. Option A is correct. Two corrections before you type.

You asked: *"Does this approach align with your expectations for the tree's health?"* **Yes.** Option A
plus retiring `scratch_ucode.py` plus hand-authoring the arrays is exactly right, and choosing
`CC_SCRATCH1` for `ack` is the clean split. Implement it.

### ⛔ CORRECTION 1 — ASSERT COMPUTED PROPERTIES, NOT LITERAL BYTES. This is the whole game.

If you hand-author the bytes and then hand-author assertions that restate those same bytes, you have
written a checksum of your own typing. `b[0x34]==0xf4 && b[0x35]==0x0b && b[0x36]==0x44` proves only
that you typed what you typed. **It would NOT have caught your branch bug** — you would have typed
`0x41` in both places and both would have agreed.

The assertion that catches it is the one that **DECODES**:
```rust
const _: () = assert!(bra_target(&ASSERT_A_BYTES, 0x34) == 0x78);
```
`bra_target` computes the destination FROM the bytes using the real PC-relative rule, and checks it
against the address you claim in the listing. That is why the existing lattice caught four malformed
instructions. **Every branch gets a `bra_target` assertion. Every port immediate gets a
`falcon_io(...)` assertion, never a typed hex literal** (`kepler.rs:36` says so and you violated it
last round). Where you can, assert a decoded FIELD (opcode nibble, register index, immediate) rather
than a raw byte.

The independence you are reproducing is: **listing → bytes** (one derivation) and
**bytes → decoded properties → listing** (a second, opposite derivation). Two directions meeting at
a human-readable listing. One direction is not verification.

### ⛔ CORRECTION 2 — your phase-gate wording is INVERTED, and this is the third time.

Your plan says: *"Update the phase gate on `MAILBOX1` to wait **while** 1..=4."* Waiting WHILE the
value is in 1..=4 means **spinning for as long as the ucode is making progress** — the exact
opposite. Bounce 4 was an inverted gate; bounce 5 was a gate that accepted the give-up marker.
**Do not make it three.**

The rule, stated so it cannot be misread:
- `mb1 == 0` → nothing has run yet → **KEEP WAITING**.
- `mb1` in `1..=4` → forward progress → **this is the value you are waiting FOR; break when it
  reaches the phase this leg expects.**
- `mb1 == PHASE_A_BOUND (0xFFFF_FFBD)` or the second marker → the ucode GAVE UP → **break AND print
  `EXIT-BY-BOUND`**, naming which marker. Never treat it as progress. Compare against
  `ucode::PHASE_A_BOUND`, not a hex literal.
- budget exhausted → **break and print the timeout distinctly** from both of the above.

Four outcomes, four distinguishable prints. If your code cannot tell all four apart on the wire, it
is not done.

### The rest of your plan is right — implement as written
Hand-authored arrays with a full mnemonic listing for BOTH images (ASSERT has none today and a bare
hex blob is not reviewable), displacements `+0x44`/`+0x1a`, ECHO gets `iowrs` (0xd1) and its
`0xE0E0E0E0` prologue, all four observables seeded per leg, halt verified BY READBACK before every
re-upload including abort paths, `| 0x80000000` restored, **and your own item 5** — read
`PFIFO_CHAN[1]` back after the restore and print whether VALID actually stuck. UNAUDITED disclaimer
at `723-754`. Commit the dirty proposal edit.

⚠ Your verification plan says `./arroyo check` "to ensure the extended assertion lattice compiles."
Say it precisely: **a green `check` means the lattice AGREED, which is the test passing** — that is
the one gate that carries real weight here, and it is not a compile formality. Then `strings` for
reachability, then declare **UNFLOWN**, out loud, in the tree.

**Report the way you reported your last message** — what you did, what you verified and HOW, what
you could not verify. That account was the best work you have sent me.

---

## igpu — `d362717e` is in review #5 NOW, and it is a NARROW pass. Hands off the branch.

You fixed B1/B2/B3 and I am reviewing exactly those three plus a regression sweep. F1–F5 are
SETTLED and will not be re-litigated — the positive control is real and the truth table
discriminates.

The sweep exists because **each of your last three rounds fixed something and broke something
else**: B1/B2 both touch `dp_aux_transfer`, which is the code the POSITIVE CONTROL also runs
through. If the packing or busy-clear change broke the control, the flight is pointless again — so
that is being checked, not assumed. Also being re-measured: the dark window and the zero-prints
invariant, because those edits sit inside it.

If it comes back clean you fly. Do not amend under the reviewer.
