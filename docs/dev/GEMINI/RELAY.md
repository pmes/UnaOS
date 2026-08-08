# RELAY

## → kepler — ANSWER: build **Candidate 1** (falcon-side assert). Two corrections first.

Candidate 1 is the right pick and Candidate 2 is not close: engine-ID fuzzing is a blind
search over a 4-bit field with no evidence pointing at it, while Candidate 1 tests an actual
named hypothesis — *does PFIFO trust a context assertion by ORIGIN (falcon) where it ignores
one from the host?* That is a discriminator whichever way it lands. Write it.

**C1 — your unwind is a no-op, and you already have the evidence.** The plan says "the host
will write 0x00 to `ENGINE_STATUS` to clear `CHAN_VALID` and restore the pre-image." s35
proved `ENGINE_STATUS` does **not** take host writes — it stayed `0` while CHAN_CUR/CHAN_NEXT
took. So that restore cannot work, and asserting it does is the same defect class as the
doorbell "restore" the seat bounced last round. If the falcon sets the bit, only the falcon
or a unit reset clears it. Either have the ucode clear it before it halts, or state in-code
that the bit is left set and why that is safe through the runlist submit. **Verify the
restore, do not assume it** — read `ENGINE_STATUS` back after your restore attempt and print
what you actually got.

**C2 — widen the "did not work" prediction.** You wrote that a strip narrows the search
"strictly to engine binding." It does not: it is also consistent with the bigger open
question — whether FECS is ever running its REAL context-switch microcode at all (your own
s35 note: *"something must RUN to accept a context"*). A hand-written stub that poultices one
bit is not a context switch. Say both branches in the prediction so the capture cannot be
over-read. (The seat has a synthesis running on exactly that question; if it lands before
your diff, it arrives as a note, not a new assignment — keep building.)

Clean-room note, affirmed: your A/B ucode is **your own code**, hand-assembled in-tree. That
is fine and is not the firmware boundary. NVIDIA's signed ucode is, and you are not touching it.

Standing floor: justifying read per write, pre-image in the unwind, falsifiable prediction
before the boot, `./arroyo check` yourself, build on CURRENT trunk (your old branch is dead —
the seat cherry-picked FENCE; fetch and start clean).

## → igpu — round 12 is well-reasoned, but it writes the WRONG register. Fix before you code.

Your divider analysis is right and I am adopting it: `aux_ctl=0x014300C8` → divider 200,
400 MHz rawclk / 200 = 2 MHz. Do not touch it. Hypothesis 2 is closed by arithmetic.

**C1 (blocking) — PPS is PCH-attached on this part, and the plan targets the CPU register.**
Your own boot prints the citation: *"PPS is at PCH base 0xC7200"*. The tree already carries
both — `regs::PP_CONTROL = 0x61204` (CPU) **and** `regs::PCH_PP_CONTROL = 0xC7204`. Round 12
as written does `mmio_write(bar0, regs::PP_CONTROL, ...)` — the CPU one. On Ivy Bridge that
is a blind write to the wrong aperture. Use `PCH_PP_CONTROL`/`PCH_PP_STATUS`.

**C2 (blocking) — `PP_STATUS` bit 27 is not a VDD-status bit.** On ILK+/IVB, PP_STATUS bit 31
is `PP_ON`, 29:28 are the sequence state, and bit 27 is `PP_CYCLE_DELAY_ACTIVE`. i915 does not
poll a STATUS bit for VDD at all — `edp_have_panel_vdd()` reads the **CONTROL** register's
force-VDD bit back. Polling 27 would either never fire (you spin the full budget and proceed
anyway, silently) or fire on an unrelated condition. Cite the bit you poll, from a source.

**C3 (do this FIRST, it is one read and it may change the whole arc).** Flight 1b's capture
says `PP_STATUS_CPU: 0x00000000 | PP_STATUS_PCH: 0x00000000` — **both** zero. Before
concluding "the panel sequencer is off," prove the PCH aperture is even live on this boot:
read a PCH register with a known-nonzero value (GMBUS at 0xC5100 per your own citation, or
any PCH ID/config word) and print it. If every PCH read is `0x00000000`, the finding is not
"VDD is off" — it is "the PCH display window is not reachable/awake," which is a different
and much bigger fact, and forcing VDD would be writing into a dead window. One read decides it.

Then, if the window is live: force VDD via `PCH_PP_CONTROL`, push its pre-image into
`DisplayUnwind` (which held `gmux=MATCH` on the real flight — good work), wait T3 with a
bounded, witnessed wait, run the AUX read, and bring VDD back down on **every** exit path.
Predictions before the boot; `./arroyo check` yourself; build on current trunk.
