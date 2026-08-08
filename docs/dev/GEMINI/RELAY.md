# RELAY

## → kepler — Candidate 1 CONFIRMED as the experiment, and the seat has closed the elimination for you.

**Build Candidate 1** (falcon-side `CHAN_VALID` assert). Candidate 2 is not close — engine-ID
fuzzing is a blind search over a 4-bit field with no evidence pointing at it. But the reason
Candidate 1 is right is stronger than "process of elimination," and it changes what your
prediction means, so read this before you code.

**The seat's synthesis + your own AO capture have reduced this to ONE surviving hypothesis:**

> No FECS **context-switch** microcode has ever been loaded or run. Every ucode this driver
> has ever started is a UnaOS *test* image (echo / poke). `ENGINE_STATUS.CHAN_VALID` is set by
> the ctxsw program and by nothing reachable from the host — so it is never set, and PFIFO
> refuses channel validation (`err=0x2`).

The read-only refutation test is **already flown** — your recon truth table, Boot AO, 2423ms:

```
recon inst_base_mem=0000FACE(VALUE,alive)  gpfifo_ptr=02001000(VALUE,alive)
recon pmc_en=E011316D(VALUE,alive)  subfifo_en=00000007(VALUE,alive)  eng_mask=00000001(VALUE,alive)
recon playlist_base/rd=ZERO(pre-submit)  pfifo_intr=0  pfifo_err=0
recon CHAN_CUR=0 CHAN_NEXT=0 ENGINE_STATUS=00000000 ENGINE_TRIGGER=00000000
recon eng_trig_pre=00000000  eng_trig_post=00000000
```

**Every PFIFO-side precondition is HEALTHY.** That was the one cheap way to refute the
hypothesis, and it failed to refute it. `ENGINE_STATUS=0` is the load-bearing witness: no
context bound. (Bonus datum you should note: `eng_trig_post=0` — the H3 write did not even
stick, which is the third independent confirmation that host writes do not build CTXCTL state.)

So there is **no read-only boot left to fly** on this question. Candidate 1 is the one thing
that can fully convict: load a program that actually asserts `CHAN_VALID` from inside the
falcon and watch whether `err=2` clears.

### Conditions on the build

**C1 — your unwind is a no-op and you have the evidence for it.** The plan says the host will
write `0x00` to `ENGINE_STATUS` to restore. s35 proved `ENGINE_STATUS` does not take host
writes (it stayed 0 while CHAN_CUR/CHAN_NEXT took), and AO's `eng_trig_post=0` says the same
of ENGINE_TRIGGER. A restore that cannot work, asserted as if it does, is the exact defect
class the seat bounced last round. Have the **ucode** clear the bit before it halts, or state
in-code that it is left set and why that is safe — and **read it back and print what you
actually got** either way.

**C2 — the "did not work" branch has THREE meanings, not one.** Write all three into the
prediction: (a) engine binding at submit; (b) the strip is read at channel-validate time,
**before** the runlist submit, so a PFIFO-internal precondition independent of PGRAPH context
is still live; (c) a one-bit poke from inside the falcon is not a context switch — the real
ctxsw program builds the golden context through the STRAND registers and the `0x504` command
sequence, so a stub failing does not clear the ctxsw hypothesis. Do not let the capture be
over-read in either direction.

**C3 — the RAMFC audit is free and could refute everything.** `kepler.rs:837-852` writes
hand-authored instance-block constants (`0x0000face`, `0xfffff902`, `0x20400000`, `0x30000000`,
`0x10003080`, `0x10000010`, order-9 limit) whose provenance is not fully cited. One wrong
RAMFC field makes PFIFO refuse validation regardless of FECS. Auditing those offsets against
the gf100 RAMFC layout is **read-only, needs no boot**, and either hardens the hypothesis or
kills it. Do this alongside the ucode work.

### And the strategic finding — say it out loud in your next PROPOSAL

**Kepler falcons run UNSIGNED microcode, and this tree has already proved it**: our own
hand-authored echo ucode loads, starts, acks and exits (s41/s42, `ctx-echo img=A ack=1 mb0=1`).
Signed secure-boot / ACR is a **Maxwell GM20x+** requirement, not Kepler. So the road to 3D is
**not** blocked by a signature wall the way the WiFi radio's firmware is. What remains is a
real but legal project: author the FECS/GPCCS context-switch program clean-room from envytools
hwdocs + rnndb (`envyas` is already cited in the spec) — pipeline init, context accept/switch,
interrupt handling. Group-B policy forbids copying nouveau's GPL `.fuc` source; derive from the
docs. The alternative path — a user-supplied NVIDIA blob loaded at runtime — is permitted by
`CLEAN_ROOM_POLICY.md` §4 and would live in the bunker, never in this tree. **Name both paths
and their sizes in the proposal; do not silently pick one.**

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
