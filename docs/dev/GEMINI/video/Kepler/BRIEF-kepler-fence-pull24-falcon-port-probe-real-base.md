# BRIEF — kepler-fence pull 24: sentinel port probe at the REAL Falcon bases

Lane: **kepler-fence** — `unaos/crates/kernel/src/drivers/gpu/kepler.rs` ONLY.
Read `docs/dev/GEMINI/README.md`, `video/INDEX.md`,
`docs/dev/OS/08_VIDEO/KEPLER-METAL-LOG.md` sitting #26, and the cleanroom
spec (notice binding).

## The s26 verdict this pull acts on

⭐ THE FALCONS ARE FOUND. FECS (0x409000) and GPCCS (0x41A000) both read
REAL: cpuctl=00000010 at both bases (a live state bit — likely HALTED),
imemc/dmemc true zeros. The spec's old 0x400180 base is formally dead.

## This pull — pull-21's sentinel probe, retargeted, zero execution

For each base in {0x409000, 0x41A000}:
1. IMEMC = base+0x180, IMEMD = base+0x184, DMEMC = base+0x1C0,
   DMEMD = base+0x1C4.
2. Write IMEMC = (1<<24) (AINCW, offset 0); readback →
   `:: kepler: fal-port b=XXXXXX imemc wr=01000000 rb=XXXXXXXX ::`
3. Write 4 sentinels to IMEMD: DEADBEEF, CAFEF00D, 12345678, A5A55A5A.
4. Write IMEMC = (1<<25) (AINCR — reads auto-increment on bit 25, NOT
   bit 24; the s24 lesson, now in the spec); read IMEMD 4× →
   `:: kepler: fal-port b=XXXXXX imem rb w0=.. w1=.. w2=.. w3=.. ::`
5. Same shape for DMEM (dmemc wr/rb line + dmem rb line).
6. NO CPUCTL write, NO BOOTVEC write — zero execution. No restore
   (scratch words in an idle, halted Falcon's local memory).
7. Keep everything landed (pulse, old-base probes, fal-base recon,
   witness) as the per-boot baseline, unchanged.

Verdict key: sentinels back at either base = upload path proven; pull 25
is the first from-scratch microcode (spec §3 minimal init + readiness
signal — CLEANROOM notice binding, everything authored in-repo).
Sentinels not back = read the rb values against the spec's IMEMC field
layout and propose the corrected control-word write, still zero
execution.

## DONE (specialist side)

Implement exactly as approved, commit ALL docs+code, delete scratch,
`git status` clean, no push — report "PUSH OWED: n". The coordinator runs
all builds and gates at land-review.

Proposal first (`PROPOSAL-kepler-fence-pull24.md`, STATUS: PROPOSED).
