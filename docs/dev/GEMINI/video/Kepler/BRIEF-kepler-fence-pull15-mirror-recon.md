# BRIEF — kepler-fence pull 15: method-mirror header recon (read-only)

Lane: **kepler-fence** — `unaos/crates/kernel/src/gpu/kepler.rs` ONLY.
Read `docs/dev/GEMINI/README.md`, `video/INDEX.md`, and
`docs/dev/OS/08_VIDEO/KEPLER-METAL-LOG.md` sittings #16–#17 first.

## Why the lane wakes up

s17 PROVED the EVO method-mirror write + UPDATE path live (display lane put
pixels on the panel through it). Your last standing fallback — disp-era USERD
enablement — now has a working mechanism to ride. Before any write: map the
territory. The method-mirror header region **0x640000–0x6403FC has never
been dumped** (s16 scanned it for known values only; the sole hits were
0x640080-adjacent state at +0x420…+0x4C8 which are head-0 method slots).

## This pull — read-only, one probe

Dense dump of 0x640000–0x6403FC (256 words), two passes, bounded delay
between (the pull-3/4 idiom), zeros printed as zeros. This is the core
channel's non-head method/control region — channel-control, USERD-linkage,
and interrupt/notify slots live below +0x400 if the method-mirror layout
holds. Your kepler.rs already runs in boot 2 with all knobs; place the dump
in your existing disp-recon position (after the CTRL_ADDR audit remains
removed/quiet — do not re-run refuted ladders).

Markers (verbatim):
- `:: kepler: mirror-hdr pass<P> off=XXX val=XXXXXXXX ::` (off relative to 0x640000)
- `:: kepler: mirror-hdr pass<P> done rows=256 ::`

## Gates (DONE = all of these)

Read-only (zero new writes in kepler.rs this pull). Full-knob check both
arches + builder-path esp-x86 + `strings` proof of the new markers in
kernel.elf + default QEMU green both. Commit ALL docs+code; delete scratch;
`git status` clean; no push (report "PUSH OWED: n").

Proposal first (`PROPOSAL-kepler-fence-pull15.md`, STATUS: PROPOSED).
