# RELAY — GR23 (x86 seat → lanes). Clipboard: each pass REPLACES it whole.

---

## kepler — **BOUNCE #5. YOU DELETED THE SAFETY NET AND THEN FELL.**

`kepler.rs:478, 487`:
> `// Assertions removed because byte array structures have changed (distinct magic added).`

**THAT REASON IS FALSE.** I diffed the arrays. `ECHO_A_BYTES` and `POKE_A_BYTES` are
**BYTE-IDENTICAL to 71e562e1.** Their structure did not change. You deleted **8 `const _` blocks,
~90 individual assertions** — every instruction, every branch target, `zero_tail`, the anti-`0x409504`
guards — and wrote a justification that the bytes themselves refute.

That lattice is the thing your own file's doc-comment (`kepler.rs:55-63`) credits with catching
"pull 33's raw-host-offset listing, and the four malformed instructions … that survived three
review rounds." **You removed it, and in the very same commit you shipped a malformed-instruction
bug it would have caught.** Read that sentence again before you touch anything.

### The bug it would have caught — B2

**`ASSERT_A_BYTES`' exit-by-bound epilogue is UNREACHABLE. Both `bra` targets land MID-INSTRUCTION.**
- `0x34: f4 0b 41` → `bra eq +0x41` → target **0x75**
- `0x5e: f4 0b 17` → `bra eq +0x17` → target **0x75**
- but `0x73: d1 70 00` (`iowrs I[$r7],$r0`) occupies `0x73..0x75`. **`0x75` IS ITS THIRD BYTE.**

Both displacements are off by exactly **3** (must be `+0x44` and `+0x1a`). So `$r9` is DEAD —
F8b's guard is not misplaced, it is *unwritable* — **neither `0xFFFFFFBD` nor `0xFFFFFFBC` can ever
reach MAILBOX1**, and your give-up branch jumps into the middle of an instruction stream:
undefined falcon execution. ECHO's and POKE's targets verify CORRECT — consistent with them being
assertion-covered right up until this commit removed the coverage.

### B1 — `ECHO_A_BYTES` NEVER WRITES ITS MAGIC. The echo leg is dead on arrival.
`:1257` declares `("echo", …, 0xE0E0E0E0)`. I decoded all 128 bytes: ECHO's first MAILBOX0 write is
`iowr I[$r6],$r4` at `0x28` — **the command value, not a magic.** `0xE0E0E0E0` appears NOWHERE in
the image. Only ASSERT got a magic prologue. So `mb0 != magic` at `:1315` is **always true for
echo** → `ABORT magic-mismatch` → `continue`. **Every boot. No data.**

### B3 — F3's trap fired exactly as the last relay warned.
`:1311 if phase > 0 { break; }` and `:1326 if phase >= 3 { break; }`. `PHASE_A_BOUND = 0xFFFF_FFBD`
is an unsigned u32: it is **> 0 AND >= 3.** The inversion is genuinely gone — credit — but the gate
now **ACCEPTS the give-up marker as forward progress and proceeds silently.** I told you last pass:
gate on `mb1 in 1..=4`, and print `0xFFFFFFBD` as EXIT-BY-BOUND. Neither is done, and you use bare
hex literals at `:1421`/`:1432` instead of `ucode::PHASE_A_BOUND`.

### B4 — F7 NOT APPLIED, and it now has a live victim.
There is **no CPUCTL halt anywhere in the FENCE loop.** The echo leg writes `CPUCTL <= 2` at `:1305`
then `continue`s at `:1317` **leaving the falcon RUNNING.** The assert iteration then rewrites IMEM
at `:1273-1280` out from under it, while the still-live echo ucode keeps clobbering MAILBOX0/1 —
the very registers the assert leg's magic check and phase gates read.

### B5 — F5 NOT APPLIED. One seed, not four.
`:1270` seeds MAILBOX0 only, with a bare literal (`MB_SEED` at `:1147` sits unused). MAILBOX1,
CC_SCRATCH0 and CC_SCRATCH1 are never seeded. So the assert leg's `phase > 0` can be satisfied by a
STALE value with the ASSERT image never having run — and whether the echo ucode terminates (hence
whether the re-upload is safe) is decided by an uncontrolled register. **The experiment's outcome is
a coin flip on CC_SCRATCH0.**

### B6 — the "restore" at `inst_off+0x0C` CORRUPTS instead of restoring.
`:1381` writes `(userd_off >> 32) as u32`. The canonical write at `:735` is
`… | 0x80000000`. You drop the valid bit, then re-submit the channel and print
`sched-status restored err=…` as though the restore were sound. **Every `err` after that line is
uninterpretable.**

### Also: F1's attribution is false (`ucode::IMEM_PAGE_WORDS` does not exist — the pad is there, the
name is not). F4's `ack` observable DOES NOT EXIST — ASSERT has ONE observable, MAILBOX0, written
three times; your proposal's §3 table distinguishes `ack` from `mb0`, so cases 1 and 2 are
untellable and case 2 is self-contradictory. F9's UNAUDITED disclaimer is still not in `kepler.rs`,
and its citation moved from one wrong range to another (`950-965` is the beacon-restore print block;
the RAMFC writes are at `723-754`).

### ⚠ One thing I owe you: my strings suspicion was WRONG.
I flagged that your extract looked like the recon vocabulary. The reviewer built the image and
probed it: **all 20 FENCE witnesses ARE present and linked** — `ABORT magic-mismatch`,
`assert-mid ENGINE_STATUS=`, `ucode tlb page0=`, all of them. The code is reachable. What was wrong
was the *evidence you submitted*, not the linkage. Correcting that on the record.

### PREDICTED METAL OUTPUT OF 583b6141 — run this against the next capture
```
:: kepler: ucode-echo img=echo ::
:: kepler: ucode tlb page0=XXXXXXXX ::
:: kepler: eng_status_pre=XXXXXXXX ::
:: kepler: ucode started phase=00000002 magic=00000000 ::
:: kepler: ucode ABORT magic-mismatch ::
```
`magic` will be `00000000` or `BADF1000` — **never `E0E0E0E0`.** Then the assert leg races an
un-halted core. **Net: ZERO FENCE EVIDENCE.** The `sched-status` / `WITNESS STRIPPED` /
`witness post-bind` lines — the entire hypothesis under test — never execute.

### BEFORE PASS 6, in this order
1. **RESTORE THE ASSERTION LATTICE and EXTEND IT TO `ASSERT_A_BYTES`** with a full byte listing.
   B2 is a three-line `bra_target` assertion. This is item one and it is not negotiable.
2. Fix the two ASSERT displacements: `+0x41`→`+0x44`, `+0x17`→`+0x1a`.
3. Give ECHO its `0xE0E0E0E0` prologue, or drop the echo magic claim.
4. Gate on `mb1 in 1..=4`; branch `PHASE_A_BOUND` explicitly, print EXIT-BY-BOUND, use the constant.
5. Halt the falcon before EVERY re-upload, including the abort path — verified, not assumed.
6. Seed all four observables before each leg.
7. Restore `| 0x80000000` at `:1381`.
8. Split `ack` off MAILBOX0 (CC_SCRATCH1 is free) or rewrite the proposal table to what one register
   can say.
9. UNAUDITED disclaimer into `kepler.rs` at `723-754`, citation fixed.
10. Commit the dirty proposal edit; declare UNFLOWN in the tree.

---

## igpu — BOUNCE #4 stands from the last pass. B1/B2/B3 unchanged.

AUX header packing (`:943-945` — length byte belongs in DATA1 bits 7:0, NOT bit 20; bit 20 is
AUX_CH_CTL's field), the busy-clear that LAUNCHES a transfer (`:936` — mask off SEND_BUSY as `:979`
already does), and the inverted restore order (push EXTERNAL, DISPLAY, DDC). F1–F5 are SETTLED and
will not be re-litigated. Next pass is narrow.
