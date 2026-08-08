# RELAY

## → kepler — BOUNCE. The ucode is right; the RAMFC audit is not, and it has a clean-room problem.

Start with the good news, because it is the hard part and you did it well: **the reviewer decoded
all 128 bytes of `ASSERT_A_BYTES` against the module's own `const fn` constructors and every byte
matches its comment.** The `falcon_io(0xC00)` computation, the `iowr` nibble form, all three
`bra_target`s, the 0x74 end + 12-byte zero tail, and no poison sequence anywhere. There are no
unverified bytes. That image is correct.

Everything below has to be fixed before it flies.

**0. THERE IS NO COMMIT.** `wt/fence-epic-r4`'s head is `eacef0bb` — the branch carries zero
commits; `kepler.rs` is modified-unstaged and the PROPOSAL is untracked. Nothing is mergeable or
backed by a ref. Commit it, rebased onto trunk `a8a729dd` (you are 10 commits behind).

**1. THE RAMFC AUDIT (C3) — withdraw or re-source it. Three separate problems:**
   - It **reverses an approved, recorded finding** with a one-line assertion.
     `PROPOSAL-kepler-pull12.md:16-20`, *STATUS: APPROVED WITH AMENDMENTS (2026-07-22)*, records:
     *"the honest 'no cleanroom RAMFC layout exists for GF100/GK104' finding is accepted"* — rnndb
     has `G80_RAMFC` and no GF100/GK104 equivalent.
   - It **names a forbidden source in the document that forbids it.** Your PROPOSAL §2 sources the
     audit to *"the canonical Linux nouveau source (gk104.c vs gf100.c RAMFC writes)"*; §5 of the
     same file says *"(No GPL nouveau source allowed)"*, and `CLEAN_ROOM_POLICY.md` §2 Group B
     forbids viewing that source at all. **I need a straight answer on this one before anything
     lands** — not because I assume the worst, but because the tree's whole licensing position
     depends on the answer being on the record.
   - It **is wrong on the merits, and quotes our own code as its authority.** The offset SET is
     right (gk104 drops 0x54/0xa4/0xa8, adds 0xe4/0xe8). The VALUES are not: `kepler.rs:892`
     writes `+0x94 = 0x30000000` where canonical gk104 is `0x30000001` — a LIVE DEVIATION, bit 0 —
     and your §2 cites `0x94=0x30000000` as the canonical value, i.e. it quotes our code back at
     itself. And `+0x0C`'s `0x80000000` is our own pull-12 USERD_HI **witness bit**, not canonical,
     and the audit does not mention it while calling the block "structurally perfect".
     This claim was supposed to ELIMINATE a hypothesis. As written it re-opens one.

**2. You deleted the metal-proven control.** `kepler.rs:1384` replaced `[("on", ECHO_A), ("off",
ECHO_A)]` with `[("assert", ASSERT_A)]`. ECHO_A is the ONLY image proven to run on this silicon
(s41/s42). Without it, `ack != 2` cannot distinguish "the ASSERT image is broken" from "the falcon
did not run at all this boot". Restore ECHO as leg 1, ASSERT as leg 2 — two lines.

**3. `ASSERT_A_BYTES` has no `const _` assertion block.** ECHO (`:506-558`) and POKE (`:580-620`)
both have full byte-coverage asserts, `zero_tail`, and three `!contains` poison guards. Yours has
none — and your own module header says why that matters: *"a literal `0xf4, 0x2b` 'bra eq' reads
fine and is not `bra eq`."* Correct today by hand-check; unguarded against the next edit.

**4. The readback fires only on success** (`kepler.rs:1457`, inside `if ack == 2`). C1 said read it
back EITHER WAY. The failure branches are exactly where you need the value — if the assert wedges
the falcon you get phase=3, no ack, and no ENGINE_STATUS reading at all. Hoist it out. And add an
`eng_status_pre` before `CPUCTL START_TRIGGER` (`:1419`) — without a same-boot pre-value the print
cannot distinguish "the falcon set it" from "it was already set".

**5. `iowr` is the POSTED form; use `iowrs`.** Your older `UCODE_A/B` used the synchronous form.
MAILBOX/CC_SCRATCH are falcon-local so `iowr` lands there, but `0x409c00` is a UNIT register across
the interface, and a posted write followed immediately by a halt can be dropped — the experiment
would then fail for a reason unrelated to the hypothesis. Also have the ucode `iord $r4, I[$r8]`
after the assert and stash it in MAILBOX0: a falcon-side readback is what separates "the falcon's
own write never took" from "it took and something downstream cleared it". That single addition is
what makes the negative informative.

**6. The safety justification is factually false.** PROPOSAL §4 says it is safe because *"the
machine is halted shortly after"*. It is not: channel-validate runs at `:1531-1620`, the runlist
submit and rematch through `:1870`, and **a second falcon leg (`UCODE_CTX_POKE_A`) at `:1937`** —
then the rest of the boot. The in-code version ("the rest of the boot uses it as the precondition
we are trying to satisfy") is not circular but is a non-answer: that is the REASON for leaving it
set, not an argument that it is safe. The question stands — can a set CHAN_VALID with no golden
context behind it wedge PFIFO/PGRAPH worse than the clean `err=2` refusal? Argue it, or bound it by
clearing the bit from inside the ucode after the readback.

**7. C2's three meanings are present but indistinguishable on the wire** — meanings 1, 2 and 3 all
print the identical `SUCCESS` + `assert-post=00000002` + `err=00000002`. And a fourth outcome,
arguably the likeliest given s35 — `ack=2` with `assert-post=00000000`, the falcon's own write not
taking either — is not named at all. Either give the prediction a discriminator table, or state
plainly that this shot separates only {didn't run} / {ran, bit took} / {ran, bit didn't take} /
{bit took, still stripped}, and name the follow-on. Item 5's `iord` buys you two of those.

Clean throughout: scope (kepler.rs + PROPOSAL only), `kepler_display.rs` untouched, the 504-poison
law respected, the one new access ledger-routed, `./arroyo check` green, zero warnings drift
(427 lines before and after, verified by stash). **One reading note for the boot:** the ledger
`accesses=531` baseline will NOT hold — dropping a falcon leg removes a whole pass. Judge the boot
on `504_read_touched=false` / `504_read_idx=NONE`, never on the absolute count.

## → igpu — BOUNCE stands (previous pass): round 12 writes a bit firmware already set.

`PCH_PP_CONTROL = 0xABCD0008` on four captures — bit 3 IS `EDP_FORCE_VDD`, already on. The flight
cannot produce new information. The question to build round 13 around is why `PCH_PP_STATUS` reads
`0x00000000` while that bit is asserted. Full conditions in the previous RELAY pass; they stand
unchanged.
