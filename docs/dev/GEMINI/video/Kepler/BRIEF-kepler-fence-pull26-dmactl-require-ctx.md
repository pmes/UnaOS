# BRIEF — kepler-fence pull 26: clear DMACTL REQUIRE_CTX, then re-run image A

Lane: **kepler-fence** — `unaos/crates/kernel/src/drivers/gpu/kepler.rs` ONLY.
Read `docs/dev/GEMINI/README.md`, `video/INDEX.md`,
`docs/dev/OS/08_VIDEO/KEPLER-METAL-LOG.md` sitting #28, and the cleanroom
spec (notice binding).

## The s28 ground truth this pull acts on

Everything up to execution now works and is proven on metal:
- both images verified byte-exact in IMEM after upload;
- `tlb page0=01000000` — the page-pad made page 0 **usable**;
- CPUCTL bit 6 clear, so writing 0x100 directly is correct.
And yet: `cpuctl 00000010 → 00000012`, mailbox0 never left its
A5A50000 seed, halt-iters=0, no SENTINEL anywhere in the 128-row sweep.
Per rnndb CPUCTL (bit1 START_TRIGGER, bit4 STOPPED): **the trigger
latched and the core stayed stopped.**

The post-sweep already names the blocker: **DMACTL (base+0x10C) =
0x00000001 → REQUIRE_CTX is SET.** The Falcon is demanding a bound
context before it will run. The scrub bits (1, 2) are clear, which is
consistent with our IMEM writes having landed. Nouveau clears exactly
this bit on the no-context path before starting a falcon.

## This pull — one new write

Keep everything landed (pulse, fal-base verdicts, fal-port probe, the
image-A/B loop, the post-sweep) unchanged, and insert before the start:

1. `:: kepler: dmactl pre=XXXXXXXX ::` (read base+0x10C)
2. Mask-clear bit 0: write `dmactl_pre & !1`. Readback:
   `:: kepler: dmactl post=XXXXXXXX ::`
   If bit 0 is still set: `:: kepler: dmactl REFUSED ::` and skip the
   start (honest null — do not improvise a second approach).
3. Re-run image A exactly as landed (0x1000 / F00DFACE), including the
   mailbox seed, the page-padded upload, the verify gate, BOOTVEC=0,
   CPUCTL=2, the halt-iters poll and the post-sweep.
4. Image B stays as the conditional fallback it already is.

Verdict key: mailbox0 leaving the seed = **first UnaOS code executed on
GPU silicon** (milestone 2 complete). Still stopped with DMACTL clear =
the next suspect is the engine-level reset/context registers
(base+0x3C0 FALCON_ENGINE and the 0x048/0x054/0x480 context aperture
group) — propose a READ-ONLY recon of those, not blind writes.

## DONE (specialist side)

Implement exactly as approved, commit ALL docs+code, delete scratch,
`git status` clean, no push — report "PUSH OWED: n". The coordinator runs
all builds and gates and delivers the sitting ESP.

Proposal first (`PROPOSAL-kepler-fence-pull26.md`, STATUS: PROPOSED).
