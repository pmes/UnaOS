# BRIEF — kepler-fence pull 18: PGRAPH Falcon recon (read-only) — K-GPU-4 arc opens

Lane: **kepler-fence** — `unaos/crates/kernel/src/gpu/kepler.rs` ONLY.
Read `docs/dev/GEMINI/README.md`, `video/INDEX.md`,
`docs/dev/OS/08_VIDEO/KEPLER-METAL-LOG.md` sitting #20, and — the arc's
spec of record — `docs/dev/OS/08_VIDEO/falcon_microcode_spec.md` (CLEANROOM
POLICY at its top is binding: no proprietary blobs, ever).

## Why the lane pivots (Peter ruling, 2026-07-24)

The PFIFO fallback ladder is exhausted (six refutations, s8–s20). Campaign
frame says the wall's other side is PGRAPH: it refuses PFIFO work until its
Falcon boots and signals ready — and OUR channel may be rejected at
validate precisely because no engine ever reports ready. The pivot: bring
the PGRAPH Falcon up with from-scratch open microcode. This pull is the
ground-truth recon before anything is uploaded.

## This pull — read-only Falcon/PGRAPH state dump

Registers (bases per the spec; all reads, sentinel discipline, absence-
honesty labels on every one — these are our own cleanroom citations, still
unproven on this silicon):
1. PMC_ENABLE bit state for PGRAPH (already read at init — reprint labeled).
2. Falcon core block 0x400100–0x40011C dense (CPUCTL 0x400100, BOOTVEC
   0x400104, + neighbors) — is the Falcon halted/running/scrubbing?
3. IMEMC/DMEMC control words as-found: 0x400180, 0x4001C0 (READ only — no
   port writes this pull).
4. PGRAPH status neighborhood 0x400000–0x40007C dense (engine status/intr).
5. Two passes with bounded delay (does anything tick on its own?).

Markers (verbatim):
- `:: kepler: falcon core off=XXX val=XXXXXXXX ::` (+ `done rows=N`)
- `:: kepler: falcon mem imemc=XXXXXXXX dmemc=XXXXXXXX ::`
- `:: kepler: pgraph stat off=XXX val=XXXXXXXX ::` (+ `done rows=N`)
- `:: kepler: falcon pass<P> cpuctl=XXXXXXXX bootvec=XXXXXXXX ::`
- absence-honesty: any 0xFFFFFFFF/sentinel-shaped read gets `ABSENT?` in
  its row.

## Gates (DONE = all of these)

Read-only (zero new writes). Full-knob check both arches + builder-path
esp-x86 + `strings` proof of new markers in kernel.elf + default QEMU green
both. Commit ALL docs+code; delete scratch; `git status` clean; no push
(report "PUSH OWED: n").

Proposal first (`PROPOSAL-kepler-fence-pull18.md`, STATUS: PROPOSED).
