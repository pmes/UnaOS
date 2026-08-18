# BRIEF — kepler-fence pull 23: find the real Falcon — FECS/GPCCS base recon

Lane: **kepler-fence** — `unaos/crates/kernel/src/drivers/gpu/kepler.rs` ONLY.
Read `docs/dev/GEMINI/README.md`, `video/INDEX.md`,
`docs/dev/OS/08_VIDEO/KEPLER-METAL-LOG.md` sitting #25, and the cleanroom
spec (notice binding).

## The s25 verdict this pull acts on

The reset pulse was electrically clean (bit 12 reads clear when cleared,
set when set) and changed NOTHING: the 0x400180/0x4001C0 ports still
return BADF1000 on every access, control readbacks included. New read of
that signature: BADF1000 on every access is what a NONEXISTENT pri
register looks like — the spec's §2 base address is likely wrong for
GK107. On this hardware family the PGRAPH context-switch Falcons live at
**0x409000 (FECS)** and **0x41A000 (GPCCS)**; the Falcon register file
(CPUCTL 0x100, BOOTVEC 0x104, IMEMC 0x180, IMEMD 0x184, DMEMC 0x1C0,
DMEMD 0x1C4) is an offset block from the unit base. The spec doc has been
annotated; verify against envytools hwdocs (allowed source) and cite in
your proposal.

## This pull — read-only recon of both candidate bases, zero writes

After the existing pulse + settle (unchanged; keep everything landed):

1. For each base in {0x409000, 0x41A000}, dump dense, both passes
   (~100 ms apart, the standard two-pass discipline):
   - the falcon core block: base+0x000 through base+0x1FC step 4
   - markers:
     `:: kepler: fal-base b=409000 off=XXX val=XXXXXXXX ::` (` ABSENT?`
     tag for FFFFFFFF/BAD0xxxx as usual; second pass `fal-base2`)
2. Summary line per base:
   `:: kepler: fal-base b=XXXXXX verdict cpuctl=XXXXXXXX imemc=XXXXXXXX dmemc=XXXXXXXX ::`
   (cpuctl = base+0x100, imemc = base+0x180, dmemc = base+0x1C0)
3. ZERO writes anywhere new — no port probes this pull; if a base shows
   real registers, the sentinel probe moves there in pull 24.
4. Witness rematch + existing probes stay as landed (baseline).

Verdict key: real values (not BADF1000) at either base = the Falcon is
found; pull 24 = sentinel probe at that base, then the first ucode. Both
bases dead too = recon widens (0x408000–0x41FFFF coarse scan proposal).

## Gates (DONE = all of these)

ZERO new writes (read-only recon). Full-knob check both arches; default
`./arroyo test` + `./arroyo test-arm` green; builder-path esp-x86 full-knob;
strings-proof the new `fal-base` markers in `target/x86_64_esp/kernel.elf`.
Commit ALL docs+code; delete scratch; `git status` clean; no push
(report "PUSH OWED: n").

Proposal first (`PROPOSAL-kepler-fence-pull23.md`, STATUS: PROPOSED).
