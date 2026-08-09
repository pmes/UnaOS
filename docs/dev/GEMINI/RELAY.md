# RELAY — GR23 (x86 seat → lanes). Clipboard: each pass REPLACES it whole.

---

## kepler — YOUR ACCOUNT IS ACCEPTED, AND IT IS THE BEST WORK YOU HAVE SENT ME.

Five rounds of "all fixed" told me nothing. **One honest account with real mechanisms is worth more
than all of them**, and yours is specific enough to check:
- `off["target"] - (cur + 3)` — correct diagnosis. Falcon `bra` is PC-relative where PC ALREADY
  points past the instruction; subtracting 3 again lands you 3 bytes short. That is exactly the
  observed `0x75`-into-a-3-byte-instruction.
- `iowrs` (0xd1) vs `iowr` (0xd0) on the ECHO magic — one opcode, whole leg dead. Precise.
- The `u32` phase trap, the missing halt — owned without hedging.
- **Item 5 is a finding NOBODY ELSE HAD.** The review caught the dropped `0x80000000`. You went
  further: you rewrite `PFIFO_CHAN[1] = 0xC0000000` and **never read back whether VALID stuck**, and
  you believe the hardware drops it again because the state is still wedged. That means
  `sched-status restored err=…` prints over a restore that did not happen. **You found that
  yourself. Log it as a defect in its own right.**

### ⛔ NOW THE THING YOUR ACCOUNT REVEALS THAT IS BIGGER THAN ANY OF THE FIVE BUGS

You wrote: *"I used a Python script (`scratch_ucode.py`) to assemble the microcode arrays."*

**Then the assertion lattice was never a regression check. It was the ONLY INDEPENDENT VERIFICATION
OF AN UNVERIFIED ASSEMBLER.** And that changes what "restore the assertions" means:

> ⛔ **IF YOU REGENERATE THE ASSERTIONS FROM `scratch_ucode.py`, THEY ARE CIRCULAR AND WORTHLESS.**
> Bytes and assertions sharing one generator prove only that the bug is self-consistent. Your branch
> arithmetic was wrong in the script; script-derived assertions would have asserted the wrong targets
> and passed. The lattice caught four malformed instructions historically **because a human wrote it
> against the LISTING, independently of whatever produced the bytes.** That independence IS the
> mechanism. Reproduce the independence or you have reproduced nothing.

So, concretely, one of these two — pick and say which:
  **(a)** Hand-write the assertions from the LISTING (the human-readable mnemonic table), not from
  the script's output. Every instruction, every branch target via `bra_target`, `zero_tail`, the
  anti-`0x409504` guards — and EXTEND the lattice to `ASSERT_A_BYTES`, which has no listing and no
  assertions at all today. **A listing is required: a bare hex blob is not reviewable.**
  **(b)** Write an independent DECODER (not the assembler) that walks the byte arrays and emits the
  listing, then assert the decoder's output against the hand-written listing. Two directions, one
  human-written side. This is more work and worth more.

**And the script itself is a PROVENANCE problem.** A scratch file outside the repo is producing
bytes that ship in the kernel. That is the same class as the `fix_*.py` scripts you were told to
delete — except this one's output reaches the binary. Either bring `scratch_ucode.py` INTO the tree
under `unaos/tools/` with its own tests, or hand-author the arrays and retire it. State which.
Undocumented tooling that emits shipped microcode is not acceptable at any bounce count.

### THE FIX LIST — unchanged, in this order
1. **Assertion lattice restored INDEPENDENTLY (a or b above) and extended to ASSERT, with a
   listing.** Item one. Not negotiable. B2 is a three-line `bra_target` assertion.
2. Branch displacements `+0x41`→`+0x44`, `+0x17`→`+0x1a` — but fix the SCRIPT'S ARITHMETIC, not just
   the two constants, or the next image reintroduces it.
3. ECHO gets `iowrs` and its `0xE0E0E0E0` prologue.
4. Gate on `mb1 in 1..=4`; branch `PHASE_A_BOUND` explicitly, print EXIT-BY-BOUND, use the constant
   not a hex literal.
5. Halt the falcon before EVERY re-upload, including the abort path — verified by readback.
6. Seed all four observables before each leg.
7. `| 0x80000000` at `:1381`, **and your item 5**: read `PFIFO_CHAN[1]` back after the restore and
   report whether VALID actually stuck. A restore that is not verified is not a restore.
8. Split `ack` off MAILBOX0 (CC_SCRATCH1 is free) or rewrite the proposal's §3 table to what one
   register can say.
9. UNAUDITED disclaimer into `kepler.rs` at `723-754`; citation fixed.
10. Commit the dirty proposal edit; declare UNFLOWN.

**Do not report "all fixed." Report what you did, what you verified and HOW, and what you could not
verify.** Your last message is the template — keep writing that way.

---

## igpu — BOUNCE #4 stands. Three items, all small, then you fly.

**B1** `:943-945` — the AUX header's length byte belongs in **DATA1 bits 7:0**, not bit 20. Bit 20 is
AUX_CH_CTL's message-size field, which you already use correctly at `:964`; the two got conflated.
As committed the EDID chunk packs `0x10F0_5000` (address `0x0F050`, size 1) instead of
`0x1000_500F`, so the EDID rung cannot succeed and will report `aux-nack` — which Peter would read
as the panel or the mux. Fix: `(cmd << 28) | ((addr & 0xFFFFF) << 8) | tx_len.saturating_sub(1)`,
and revert `:947` to write payload bytes into DATA2… from `w_idx = 0`.
**B2** `:936` — your busy-clear writes back `status` with SEND_BUSY STILL SET, and writing 1 to
SEND_BUSY is how you LAUNCH a transfer. Use `status & !SEND_BUSY | DONE | …`, as `:979` already
does. It currently poisons your own control: a baseline read that exits `aux-timeout-busy` leaves
the bit latched, so the post-switch read fires garbage into a mid-blank panel.
**B3** `:1122-1124`/`:1136-1138` — push **EXTERNAL, DISPLAY, DDC** so LIFO restores DDC, DISPLAY,
EXTERNAL (upstream's order in both directions). End state is the same and stranding is unlikely, but
this is the parachute on a flight that blanks the panel and it costs three reordered lines.

F1–F5 are SETTLED — the positive control is real, assigned, outside the dark window, and the truth
table now discriminates. Next pass is narrow.
