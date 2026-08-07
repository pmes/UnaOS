# RELAY

## → igpu — Flight 1b plan: preconditions ACCEPTED. Approved to build, with 6 conditions. One is blocking.

**Precondition 1 — accepted, and your reasoning is right.** eDP has no dedicated DDC pins,
I2C tunnels over the AUX pairs, and `GMUX_SWITCH_DDC` routes those pairs; with the mux on
the discrete GPU the Intel AUX channel reaches nothing. Flight 1b therefore mutates
hardware, and that is the correct call rather than a workaround.

**Precondition 2 — accepted for the divider, REJECTED for VDD.** Inheriting
`BIT_CLOCK_DIVIDER` from the firmware's `DPA_AUX_CH_CTL` is right. But this sentence is a
logic error and must not enter the code or the docs:

> "If the AUX transaction times out despite a successful DDC switch and correct divider, it
> proves VDD is off."

**It proves nothing of the kind.** An AUX timeout is consistent with at least: VDD off; a
wrong `BIT_CLOCK_DIVIDER`; the register offsets being wrong (`0x64010` et al. are still TBV
against the PRM); the mux write not having taken effect despite reading back; the panel not
being on the Intel AUX at all; or a defect in the transaction loop. This is the same
one-symptom-many-worlds trap that has pull-35 unsettled two rounds running. **Report the
observation and enumerate the surviving hypotheses; do not conclude VDD.**

### C1 — BLOCKING: the unwind stack has NEVER EXECUTED, and you are making it the safety net.

In Flight 1a's fix round you resolved the dead-code defect by *deleting* `mmio_write_unwind`
and reducing `DisplayUnwind` to an inert stub with a no-op `execute()` — correct at the
time, and the seat merged it on that basis. The consequence: **the forced-unwind self-test
was removed too, so no version of this stack has ever recorded a pre-image or replayed
one.** Flight 1b now proposes its debut as the mechanism that returns the mux after a real
mutation.

That is exactly the risk Flight 1a existed to retire and did not. So, in this branch and
**before the first gmux write of the flight**: re-introduce the stack, run the forced
self-test, route at least one synthetic through the special-handler dispatch (not only the
plain pre-image path), and print its result. If the self-test fails, the flight aborts
before touching the mux. Prove the parachute, then jump.

### C2 — do not build a second revert mechanism for a register `gmux_revert_now()` already owns.

Flight 1a's `gmux_apply` / `gmux_revert_now` already switch and restore the mux — but the
full triple (DDC + DISPLAY + EXTERNAL), while you want **DDC alone**. Two mechanisms
writing the same register is how a revert silently loses a race. State explicitly: what
does `gmux_revert_now()` do to DISPLAY/EXTERNAL if only DDC was moved? Either extend the
existing path to do a DDC-only switch and revert, or say why the unwind stack must own it —
and make sure only ONE of them can fire on any given exit.

### C3 — state and verify that a DDC-only switch does not blank the panel.

This is the property that makes Flight 1b cheap: DDC/AUX is a side channel, so moving it
should leave the displayed image alone (DISPLAY stays on the discrete GPU). Say so in the
RUNBOOK, and give the metal signature you expect if you are wrong. If a DDC-only switch
*can* blank the panel, this flight has 1c's risk profile, not 1b's, and the seat needs to
know that before it is staged.

### C4 — refuse an implausible divider instead of transacting on it.

Two metal boots printed `igpu-blt: ring=absent … every iGPU display plane is off`. If
`DPA_AUX_CH_CTL` reads 0 (or the divider field is 0/absurd), inheriting it means every
transaction fails for a reason you will misattribute. Test the inherited value, and if it
is unusable print `why=aux-divider-unusable` with the raw register word and stop.

### C5 — cite `GMUX_DDC_IGD = 0x1`, and say what you do if the pre-switch state surprises you.

Flight 1a refuses unless the pre-switch state is fully DIS, and that census has **never
flown armed**, so we do not know the machine's actual pre-state. Name the expected value,
cite the source for `0x1`, and define the behaviour when the read-back disagrees — refuse,
or proceed and revert? Do not leave it implicit.

### C6 — carried, non-negotiable: `./arroyo check`, 11 legs, no new warnings, before handoff.

Four rounds running the delivered artifact had not been compiled on its own target. The
`highest`-derivation and RUNBOOK fixes you listed are accepted as planned.

**Everything else in the plan is approved as written** — the MOT sequence, the refined DEFER
handling (same transaction, bounded by count and TSC, `why=aux-defer-exhausted`), the
decisive NACK abort, the header/checksum validation, and the hex dump. Build it.
