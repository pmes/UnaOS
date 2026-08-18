# BRIEF — kepler-fence pull 28: what does FECS want? (context/init ucode study + minimal probe)

Lane: **kepler-fence** — `unaos/crates/kernel/src/drivers/gpu/kepler.rs` ONLY.
Read `docs/dev/GEMINI/README.md`, `video/INDEX.md`,
`docs/dev/OS/08_VIDEO/KEPLER-METAL-LOG.md` sitting #30, and
`docs/dev/OS/08_VIDEO/falcon_microcode_spec.md` §3. Cleanroom notice
binding for every instruction and every register claim.

## What s30 gave us

Refutation #8, the cleanest of the series. Your heartbeat ran across the
entire witness: MAILBOX1 advanced 0x5750 → 0x5AA5 straight through the
strip and restore, cpuctl=00000000 throughout — and PFIFO stripped the
channel anyway, with the err=2 / stat=5 / valid=00002000 signature
byte-identical to the s25 baseline. **The wall is not engine liveness.**
The refutation ledger now has eight entries; do not re-propose any of them.

## Why this pull exists

Every host-side variable is exhausted. The one hint the chip volunteered
is DMACTL bit 0: **REQUIRE_CTX**. The chip wants a *context*, not a live
core. Real FECS boot on this family runs a context-switch microcode that
initializes PGRAPH state and manages channel context load/save — and
PFIFO's channel-validation path plausibly gates on that machinery being
up. We have never given it any of that. This pull turns the arc from
"poke the wall" to "learn what the wall guards."

## This pull — TWO deliverables, study first

1. **STUDY (the bulk of the pull): a cleanroom analysis document**
   `docs/dev/GEMINI/video/Kepler/STUDY-fecs-ctx-init.md` answering, with
   per-claim citations (envytools/nouveau docs by name+section, never code
   verbatim):
   - What does the real FECS context-switch ucode do at init on Kepler
     (GK107)? Enumerate the phases (self-init, PGRAPH strand/state init,
     host-interface mailbox protocol, ctx load/save loop).
   - What is the FECS↔host handshake surface: which mailbox/method
     registers does the host use to command it, and what does "a context
     exists" mean concretely (ctx buffer in VRAM? instance-block fields?
     PGRAPH_CTXCTL registers?).
   - What is the MINIMAL subset that could plausibly flip PFIFO's channel
     validation — smallest hypothesis first, with the register-level test
     for each.
2. **PROBE (small, read-only, zero new execution): recon of the
   PGRAPH_CTXCTL / FECS host-interface registers** your study names as the
   handshake surface — dump their reset values under the existing
   fal_base-style print discipline (budget the FTDI ring; a handful of
   registers, one line each, no dense sweeps). This is the ground truth
   the study's hypotheses will be tested against in later pulls.

No new ucode execution in this pull. Keep image A and UCODE_HB as landed.

## Ground rules (unchanged)

As approved, commit ALL docs+code, delete scratch, no push, report
"PUSH OWED: n". No build or gate commands — the coordinator runs every
gate at land-review.
