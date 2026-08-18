# BRIEF — kepler-fence pull 19: PGRAPH power-on (single PMC write)

Lane: **kepler-fence** — `unaos/crates/kernel/src/gpu/kepler.rs` ONLY.
Read `docs/dev/GEMINI/README.md`, `video/INDEX.md`, and
`docs/dev/OS/08_VIDEO/KEPLER-METAL-LOG.md` sitting #21 first.

## The s21 ground truth this pull acts on

PMC_ENABLE = 0xE011216D — **bit 12 (PGRAPH) is CLEAR**; every Falcon/PGRAPH
register reads 0xBADF1200 (gated-engine pri error). The engine is off
because we never turned it on. Your init already sets PFIFO (bit 8) the
same way — this is the same class of write.

## This pull — one new write, then re-run your pull-18 recon

1. Print PMC_ENABLE pre: `:: kepler: pgraph-enable pre=XXXXXXXX ::`
2. **Write PMC_ENABLE |= (1<<12).** Readback:
   `:: kepler: pgraph-enable wrote=XXXXXXXX rb=XXXXXXXX ::`
   If rb doesn't have bit 12 set: `:: kepler: pgraph-enable REFUSED ::` and
   skip the re-dump (honest null).
3. Bounded settle (spin-loop idiom, ~100 ms equivalent).
4. Re-run the ENTIRE pull-18 recon dump unchanged (both passes, same
   markers) — the diff against s21's all-BADF1200 baseline is the
   deliverable. Expect cpuctl/bootvec/imemc/dmemc/pgraph-stat to become
   real values if the enable takes.
5. Leave PGRAPH enabled (document in the report; an enabled idle engine is
   the normal state — no restore, single-variable stands).

## Gates (DONE = all of these)

Exactly ONE new register write (PMC_ENABLE, bit-OR). Full-knob check both
arches + builder-path esp-x86 + `strings` proof of new markers in
kernel.elf + default QEMU green both. Commit ALL docs+code; delete scratch;
`git status` clean; no push (report "PUSH OWED: n").

Proposal first (`PROPOSAL-kepler-fence-pull19.md`, STATUS: PROPOSED).
