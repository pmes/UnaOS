# BRIEF — kepler-fence pull 16: mirror-window backing-store beacon test

Lane: **kepler-fence** — `unaos/crates/kernel/src/gpu/kepler.rs` ONLY.
Read `docs/dev/GEMINI/README.md`, `video/INDEX.md`, and
`docs/dev/OS/08_VIDEO/KEPLER-METAL-LOG.md` sitting #18 first.

## Facts this pull stands on (s18, capture-verified)

Your mirror-hdr window (0x640000–0x6403FC) is VOLATILE: the 0xFF114D95 fill
grew 62→158 non-zero rows between two passes seconds apart; five
high-entropy words at 0x16C–0x17C; lone 0x08C=0x2CB23507; 0x240=0x00000801.
Logged hypothesis: the window is an aperture onto live MEMORY (pushbuffer/
USERD territory), not a config register file. This pull tests that.

## The experiment — beacons in VRAM, watch the window (no new MMIO writes)

All writes are BAR1 VRAM writes into allocations WE own — zero MMIO
register writes this pull.

1. Baseline: dump the window once (`mirror-hdr` markers, pass0 form).
2. **Plant beacons via BAR1**: write the 8-word pattern
   {0xBEAC0001..0xBEAC0008} at each of: our USERD block (inst area,
   userd_off), our pushbuffer base (pb_off region), and our runlist base —
   the three channel structures we already own. Marker per plant:
   `:: kepler: beacon planted at=<userd|pb|runlist> off=XXXXXXXX ::`
3. Re-dump the window (pass1 form). Then scan the dump comparison IN CODE:
   any window word in {0xBEAC0001..0xBEAC0008} →
   `:: kepler: beacon SEEN off=XXX val=XXXXXXXX ::` ; none →
   `:: kepler: beacon none-seen ::` (honest null).
4. Third dump after a bounded ~2 s delay (volatility re-check with beacons
   in place).

Verdict space this design separates: window mirrors one of OUR channel
structures (beacon appears → which one, by value) vs window is
engine-private memory (no beacon, still volatile) vs window is stable this
boot (volatility was boot-phase-transient).

## Gates (DONE = all of these)

Zero MMIO register writes; BAR1 writes only to our own allocations.
Full-knob check both arches + builder-path esp-x86 + `strings` proof of the
new markers in kernel.elf + default QEMU green both. Commit ALL docs+code;
delete scratch; `git status` clean; no push (report "PUSH OWED: n").

Proposal first (`PROPOSAL-kepler-fence-pull16.md`, STATUS: PROPOSED).
