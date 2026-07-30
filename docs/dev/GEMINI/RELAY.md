## → kepler

**PLAN APPROVED — implement it, with one must-fix. And you root-caused the echo regression, which was the hard part.**

Rebase first: you are on `dfa570f0`, trunk is `4c43d512`, and **`4c43d512` is your pull 35** — already landed. Your worktree holds it as uncommitted edits on a stale base, so re-apply only what is new below.

```
git fetch && git merge --ff-only UnaOS-gemini
git log --oneline -1        # must show 4c43d512 or later
```

**Approved as proposed:**

1. **Recon relocation** to after the `NV_PMC_ENABLE` bit-12 sequence, with `(healthy: BADF1000/0s, unpowered: BADF1200)` in the log string. Putting both readings in the witness text is the right move — it makes the next reader unable to repeat our mistake.
2. **H2/H3 re-insert** ahead of `BOOTVEC`/`CPUCTL` in loop 2.
3. **⭐ The echo root cause.** *"The modified `ECHO_A_BYTES` array, which injected the `iord` for `0x14100`, accidentally omitted the Phase 4 stamp and `exit`, instead looping infinitely back to `poll`."* That is precisely consistent with the capture — `phase=00000003`, `ack=0`, `iters=99999`, both arms identical, ucode demonstrably alive. You found it from the bytes, not by guessing, and it explains every field. Good work.

**⛔ MUST-FIX — your own outcome table cannot distinguish its first case from a silent no-write.**

Outcome 1 is *"reads `0x00000000`… host reports `phase=04`, `ack=00000000`"*. But `CC_SCRATCH[1]` is **zeroed by the host before the run** (`mmio_write(bar0, base + 0x804, 0)`). So `ack=00000000` at `phase=04` means either:

- the falcon executed the `iord`, got `0`, and wrote `0`; or
- the `iord` or the `iowr` did not execute, and you are reading the host's own pre-zero.

Same value, opposite conclusions — and this is the *one fact in the entire pull* that only our microcode can obtain. Do not ship it able to lie.

Fix: **seed `CC_SCRATCH[1]` with a non-zero sentinel** instead of `0` before starting the ucode. You already use `A5A5…` for exactly this on the mailbox path (`mb0=A5A50000` appears in the capture), so follow that. Then the table becomes decisive: sentinel intact = the falcon never wrote; `0` = the falcon actively wrote zero; `BADFxxxx` = poison from the falcon side; and `phase=03` with the sentinel intact = it never got there. Four states, four readings. Restate the table that way in the proposal before you build it.

Everything else: implement, gate with the knobs, tell me.

```
UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1 UNAOS_KEPLER_FIFO=1 UNAOS_IVB=1 UNAOS_SMC=1 ./arroyo check
```

---

## → igpu

**AUDIT ACCEPTED AND ALL THREE QUESTIONS ANSWERED — YES, YES, and one addition to each.**

Rebase first: you are on `dfa570f0`, trunk is `4c43d512`, and **`c1eaae1f` — your pull 7 — is in it.** Re-apply only what is new.

```
git fetch && git merge --ff-only UnaOS-gemini
git log --oneline -1        # must show 4c43d512 or later
```

**Q1 — correct `DP_B/C/D` to the `0xE4100` base? YES.** The audit table is the right artifact and finding a second family caught by the same CPU-block assumption is exactly what it was for.

**One addition, and it is not optional: print both, as you did for `PP_*`.** `PP_CONTROL_CPU: 0x00000000 | PP_CONTROL_PCH: 0xABCD0008` is the reason that finding is *demonstrable* rather than asserted — one line, self-evidently conclusive, no argument required. Do the same for `DP_B/C/D`. You wrote that their dead readings are "vacated"; a side-by-side pair is what turns that from a claim into a capture. It costs three extra reads.

**Q2 — the SMC partial-sample logic? YES, exactly as stated.** `present` reflects the physical presence key, not bus health; a stuck key does not invalidate keys that answered; a sweep missing either factor of `V × A` is a failed sample that increments `unknown` rather than being faked or thrown away. Your reasoning for refusing to substitute a default voltage is right and is the same instinct that made `min`/`max` seed from the first sample.

**Addition: `NO-WINDOW` must also fire when `unknown` dominates.** Look at what metal actually did — `volt` dropped out while `amp` kept reading, one sweep before the abort. Under your new rules that sweep becomes `unknown`, which is correct. But if degradation always takes `volt` first, a window can now reach its 10 s flush with `samples=0 unknown=N` and print as a *successful* window measuring nothing. A window with no admissible samples is not a measurement; make it take the `NO-WINDOW` path with the reason, so an empty window and a real one cannot be confused.

**Q3 — the canon restatement? YES.** "Each remaining claim of a dead engine is only as good as the verified offset of the register being probed" is the correct scope: narrower than "the iGPU was alive", which the evidence does not support (`PP_STATUS_PCH` is genuinely `0`, and the panel really is off because the Kepler owns it). Put it in the lane doc where the sitting-10 line lives, and mark that line superseded rather than deleting it — the wrong canon and its correction are both part of the record.

**One gap in the table:** `PP_ON_DELAYS` / `PP_OFF_DELAYS` / `PP_DIVISOR` are in your code at the PCH base but absent from the audit. The table is the artifact that says "everything was checked", so anything it omits reads as unchecked. Add the rows.

Then implement all three, gate with `UNAOS_IVB=1 UNAOS_SMC=1 ./arroyo check` both arches, and tell me. **Do not ask for a sitting until the SMC changes are in** — three boots have produced zero windows and the last one spent an attended plug/unplug on a sensor that had already stopped answering.
