# BRIEF — kepler-fence pull 22: PGRAPH reset pulse, then re-probe the ports

Lane: **kepler-fence** — `unaos/crates/kernel/src/drivers/gpu/kepler.rs` ONLY.
Read `docs/dev/GEMINI/README.md`, `video/INDEX.md`,
`docs/dev/OS/08_VIDEO/KEPLER-METAL-LOG.md` sitting #24, and the cleanroom
spec (CLEANROOM notice binding, as for the whole arc).

## The s24 ground truth this pull acts on

With PMC bit 12 SET, the Falcon memory ports are still dead: IMEMC/IMEMD/
DMEMC/DMEMD all return BADF1000 on every access, control readbacks
included. Setting the enable bit on an engine that was disabled at
power-on evidently leaves the Falcon sub-block behind a second gate.
Standard init discipline for this hardware class is reset-THEN-enable,
not enable-alone: pulse the engine's PMC bit so the Falcon fabric
interface re-initializes from a clean state.

## This pull — one reset pulse, then the identical probe

Replace the plain enable in the pgraph-enable block with a pulse (this is
a resequence of writes to a register we already own — no new register):

1. `:: kepler: pgraph-pulse pre=XXXXXXXX ::` (read PMC_ENABLE)
2. Write PMC_ENABLE with bit 12 CLEARED. Readback:
   `:: kepler: pgraph-pulse off rb=XXXXXXXX ::`
3. Settle (standard ~100 ms spin idiom).
4. Write PMC_ENABLE with bit 12 SET. Readback:
   `:: kepler: pgraph-pulse on rb=XXXXXXXX ::`
5. Settle again.
6. Re-run, UNCHANGED: the pull-18 falcon core recon (cpuctl/bootvec/core
   rows/imemc/dmemc/pgraph stat, both passes) AND the pull-21 IMEM/DMEM
   sentinel probe (same markers). The diff against s24's all-BADF1000
   port baseline is the deliverable.
7. Witness rematch stays as landed (runs after, engine left enabled).

Verdict key: sentinels back post-pulse = the gate was reset-latch; M2 is
the first real ucode. Ports still BADF1000 = the second gate is elsewhere
(engine-level clock/reset registers inside PGRAPH space) → next pull is a
read-only recon of the PGRAPH 0x400100–0x400200 control neighborhood
under both states; do NOT propose blind writes beyond this pulse.

## Gates (DONE = all of these)

Writes remain confined to PMC_ENABLE (pulse) + the four Falcon memory
ports (existing probe). No protection weakened. Full-knob
`UNAOS_IVB=1 UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1 UNAOS_KEPLER_FIFO=1
./arroyo check` both arches; default `./arroyo test` + `./arroyo test-arm`
green; builder-path `UNAOS_USBDEBUG=1 <same knobs> ./arroyo esp-x86`;
strings-proof the new `pgraph-pulse` markers in
`target/x86_64_esp/kernel.elf`. Commit ALL docs+code; delete scratch;
`git status` clean; no push (report "PUSH OWED: n").

Proposal first (`PROPOSAL-kepler-fence-pull22.md`, STATUS: PROPOSED).
