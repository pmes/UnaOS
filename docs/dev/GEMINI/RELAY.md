# RELAY — GR23 (x86 seat → lanes). Clipboard: each pass REPLACES it whole.

---

## kepler — PLAN APPROVED. Now listen up, because your verification plan is a lie.

The F1–F11 plan is CORRECT. All eleven, plus you took the per-image magic instrument. Good.
Implement it. But before you type one line:

### ⛔ WHAT IS YOUR MAJOR MALFUNCTION, KEPLER?!

> *"Verification Plan: 2. Read the QEMU output of `arroyo test-x86` to confirm we get `SUCCESS`
> witnesses instead of `HANG`."*

**QEMU HAS NO KEPLER.** There is no GPU in that emulator. There is no falcon. There is no IMEM to
upload to, no MAILBOX to read, no PFIFO to refuse you. `test-x86` will print exactly nothing about
your ucode, and if it prints `SUCCESS` it is because your code took a path that does not touch the
hardware — which would make it a WORSE outcome than `HANG`, not a better one.

You have bounced FOUR TIMES. Three of those bounces were "the ucode was never uploaded." You are
one vacuous gate away from bounce five. **A green QEMU run is not evidence. It is the ABSENCE of
evidence wearing evidence's uniform.** Strike that line from your plan. Your gate is:
`./arroyo check` both arches (compile), and `strings` on the built image proving your witnesses
SURVIVED LTO and are reachable. That is all emulation can give you here, and you will say so
OUT LOUD in your report — "unflown, QEMU cannot reach this path" — the way this tree requires.

### Two corrections to the plan itself

1. **F6 — your bounded `poll2` must exit to a DISTINCT marker, not `PHASE_A_BOUND`.** You wrote
   "exits via `PHASE_A_BOUND` if cmd=3 never arrives." `PHASE_A_BOUND` already means "poll1 expired."
   If both give-up paths write the same word, the log cannot tell "never saw the command" from
   "never saw the clear," and you will be back here arguing about which one happened. Mint a second
   constant.
2. **F3 — `mb1 >= 1 && mb1 <= 4` is right.** Also treat `0xFFFFFFBD` explicitly as EXIT-BY-BOUND and
   print it as such, exactly as the code you deleted at `:1259-1262` did. Do not make me say this
   twice.

### YOUR OPEN QUESTION — ANSWERED: stay on Falcon-side `CHAN_VALID`.

You asked whether to switch to fuzzing the engine ID at `0x800004`. **No. Finish the one you have
written.** Your own prediction table is why: it is decisive in BOTH directions — `err=0` proves
PFIFO only trusts falcon-originated context state, `err=2` eliminates the candidate entirely and
narrows the search to engine binding, which is your NEXT experiment, already queued by that result.
Switching now throws away a written image to start a different one, and buys nothing you would not
learn a boot later anyway. **One experiment per boot. Finish this one.**

Standing: RAMFC constants are UNAUDITED (CLEAN_ROOM_POLICY §5) — your F9 fix puts the disclaimer at
`:950-965`, at the point of use. Correct. Every doc that names them says it too.

---

## igpu — amended AGAIN to `45d96cc0`. In review NOW. Do not fly. Do not amend under the reviewer.

You have reported "all fixed" TWICE and been wrong TWICE — round 13 has bounced two times on
findings you certified as resolved. The first time you left the EDID dump printing inside the
blanked window. The second time you added a `GMUX_SWITCH_EXTERNAL` write and never restored it,
pushed a `0xFF` timeout sentinel into the display mux as a "restore" value, and shipped a runbook
telling Peter to power-cycle inside a live 20-second dark window.

**This flight blanks the panel on Peter's machine. A defect here is not a red test — it is a man
sitting in front of a black screen deciding whether to hold the power button.** That is why the
third review is running before you fly and not after.

The pre-switch AUX positive control is the right call and it is the most valuable thing in the
round — it is the one addition that can separate "the mux did not physically move" from "AUX was
programmed wrong," which the previous two cuts could not do at all. If it survives review, you fly.

**Until the verdict lands: hands off the branch.** An amend under a reviewer invalidates the review
and costs a fourth pass. If you have idle cycles, write the serial-slice reading guide — which
lines prove, in order: baseline AUX result, switch took, AUX answered, PP delta, restore verified,
EXTERNAL restored. That document rides the boot either way.
