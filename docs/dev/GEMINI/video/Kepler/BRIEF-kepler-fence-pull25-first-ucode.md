# BRIEF — kepler-fence pull 25: K-GPU-4 milestone 2 — first from-scratch microcode

Lane: **kepler-fence** — `unaos/crates/kernel/src/drivers/gpu/kepler.rs` ONLY.
Read `docs/dev/GEMINI/README.md`, `video/INDEX.md`,
`docs/dev/OS/08_VIDEO/KEPLER-METAL-LOG.md` sitting #27, and the cleanroom
spec. **CLEANROOM notice binding: every byte of this program is authored
in this repo; instruction encodings cited from envytools Falcon ISA
documentation (allowed source) — no blob, no disassembly of vendor
firmware.**

## The s27 ground truth this pull acts on

Upload path proven: all sixteen sentinels returned at both Falcons, imem
and dmem, control readbacks real. The Falcons are halted (cpuctl=0x10)
with working memory ports. Milestone 2 is execution: run the first
UnaOS-authored instructions on the GPU.

## This pull — smallest possible program, FECS ONLY

Target: FECS (0x409000) only. GPCCS waits until FECS behaves.

1. Author a minimal Falcon program (aim ≤16 words) that:
   - writes a magic constant 0xF00DFACE to FALCON_MAILBOX0 (host-visible
     at base+0x040) via the Falcon's iowr path, and
   - cleanly halts (EXIT).
   Every instruction encoding cited in the proposal (envytools falcon
   ISA docs; include doc/section per instruction). The program bytes live
   in a Rust const in kepler.rs with a comment block showing the assembly.
2. Host sequence (markers):
   - Pre-state: `:: kepler: ucode pre mailbox0=XXXXXXXX cpuctl=XXXXXXXX ::`
   - Upload: IMEMC = 0 | (1<<24); write the program words via IMEMD;
     per-256B-block tag via IMEMT (base+0x188) per the envytools upload
     discipline (cite it). `:: kepler: ucode uploaded words=N ::`
   - Readback-verify the words (AINCR) before execution:
     `:: kepler: ucode verify ok=Y/N w0=XXXXXXXX ::` — if verify fails,
     STOP (no CPUCTL write), honest-null marker.
   - BOOTVEC (base+0x104) = 0. CPUCTL (base+0x100) = 2 (STARTCPU).
     `:: kepler: ucode start cpuctl-wr=00000002 ::`
   - Bounded poll (standard spin idiom, ~100 ms budget) on CPUCTL for the
     halt bit; then read MAILBOX0:
     `:: kepler: ucode end cpuctl=XXXXXXXX mailbox0=XXXXXXXX ::`
3. Verdict key: mailbox0=F00DFACE = FIRST UNAOS CODE EXECUTED ON THE GPU
   (milestone 2 complete; milestone 3 = the real init/readiness program).
   cpuctl never halts or mailbox unchanged = record exact end-state,
   engine left as-is (no restore; PMC pulse next boot resets it), honest
   null — no retries, no improvisation.
4. Everything landed stays as baseline (pulse, fal-base verdicts,
   fal-port probe, witness, late-recap). Gated blocks stay gated.

## DONE (specialist side)

Implement exactly as approved, commit ALL docs+code, delete scratch,
`git status` clean, no push — report "PUSH OWED: n". The coordinator runs
all builds and gates at land-review and delivers the sitting ESP.

Proposal first (`PROPOSAL-kepler-fence-pull25.md`, STATUS: PROPOSED) —
the proposal MUST include the full annotated assembly listing and its
citations; approval is contingent on the encodings being verifiable.
