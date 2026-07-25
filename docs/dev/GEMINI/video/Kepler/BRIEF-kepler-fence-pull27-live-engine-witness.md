# BRIEF — kepler-fence pull 27: witness the wall against a LIVE engine

Lane: **kepler-fence** — `unaos/crates/kernel/src/drivers/gpu/kepler.rs` ONLY.
Read `docs/dev/GEMINI/README.md`, `video/INDEX.md`, and
`docs/dev/OS/08_VIDEO/KEPLER-METAL-LOG.md` sitting #29. Cleanroom notice
binding for every instruction, as before.

## What s29 gave us

Your microcode ran. mailbox0 = F00DFACE, exact, twice, SENTINEL at
off=040, clean halt — first UnaOS-authored code executed on GPU silicon,
and the indexed IO scheme confirmed empirically. DMACTL REQUIRE_CTX was
the whole block.

## Why this pull exists

The fence wall (PFIFO strips VALID/POLL, err=2) has been tested against:
a powered-off engine (s21), a powered engine (s23), and a reset-pulsed
engine (s25). Never against a **running** one — we couldn't run code.
Now we can. If PFIFO's refusal is conditioned on the target engine being
live, this is the pull that finds out; if the strip is identical with
FECS executing, that is the strongest possible statement that the wall is
unrelated to engine liveness, and the arc turns to what the real FECS
context/init ucode must do.

## This pull — heartbeat ucode, then the existing witness

1. Author a second image (keep image A as landed and still run it first —
   it is now our known-good execution witness):
   **UCODE_HB** — a bounded counting loop that writes an incrementing
   value to MAILBOX1 (host +0x044, falcon I[0x1100] under the confirmed
   indexed scheme — derive and state it, don't copy blindly) and runs long
   enough to still be executing while the witness sequence runs later in
   init. Bound it: a finite iteration count, then `exit`. Do NOT write an
   unbounded loop — a Falcon spinning forever through the rest of boot is
   a wedge risk we are not taking. Full annotated listing with per-
   instruction citations, as for pull 25; approval depends on it.
2. Start UCODE_HB (same proven sequence: seed, page-padded upload,
   verify-gate, DMACTL clear, BOOTVEC=0, CPUCTL=2) but DO NOT poll to
   completion — print `:: kepler: hb start mb1=XXXXXXXX ::` and continue.
3. Immediately before the existing witness-rematch block:
   `:: kepler: hb pre-witness mb1=XXXXXXXX cpuctl=XXXXXXXX ::`
   Immediately after it:
   `:: kepler: hb post-witness mb1=XXXXXXXX cpuctl=XXXXXXXX ::`
   MAILBOX1 advancing across those two reads is the proof the engine was
   ALIVE during the witness — that is the pull's central evidence, and it
   must be checkable without trusting timing.
4. The witness sequence itself: unchanged, byte for byte.

Verdict key: witness passes (VALID sticks) ⇒ the wall was engine-liveness
all along and the fence lane is open. Witness still strips with mb1
provably advancing ⇒ refutation #8, the cleanest yet, and the next arc is
the real FECS init/context ucode (spec §3) rather than more PFIFO probing.

## DONE (specialist side)

Implement exactly as approved, commit ALL docs+code, delete scratch,
`git status` clean, no push — report "PUSH OWED: n". The coordinator runs
all builds and gates and delivers the sitting ESP.

Proposal first (`PROPOSAL-kepler-fence-pull27.md`, STATUS: PROPOSED) with
the full annotated listing and citations.
