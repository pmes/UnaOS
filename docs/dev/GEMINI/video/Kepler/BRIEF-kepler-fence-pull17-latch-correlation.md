# BRIEF — kepler-fence pull 17: window-vs-latch correlation (read-only)

Lane: **kepler-fence** — `unaos/crates/kernel/src/gpu/kepler.rs` ONLY.
Read `docs/dev/GEMINI/README.md`, `video/INDEX.md`, and
`docs/dev/OS/08_VIDEO/KEPLER-METAL-LOG.md` sitting #19 first.

## Facts this pull stands on (s19, capture-verified)

Your beacon test came back NONE-SEEN and pass1→pass2 changed ZERO words:
the 0x640000 window is (a) not a mirror of our USERD/pb/runlist, (b) STABLE
within a boot once we're watching. But s18→s19 contents differ across
boots, and the display lane's latch (0x640460+0x640080) demonstrably drives
this engine. Open question: does the UPDATE latch perturb the window?
If yes → the window is core-channel processing state and the write path
into it is the display lane's proven mechanism; that's the road to any
USERD/channel-control slot for PFIFO.

## This pull — sequencing only, zero new probes

kepler.rs owns both call sites. Add ONE dump call: run the existing
mirror-hdr dense dump (256 rows) **BEFORE** the `takeover_display(...)`
call (marker `pass=pre`), and keep your existing post-takeover dumps
(pass0/1/2 — beacons stay, they cost nothing and re-confirm none-seen).
In-code comparison after pass0: print every word that differs pre→pass0:
- `:: kepler: mirror-hdr pre off=XXX val=XXXXXXXX ::` (+ done rows=256)
- `:: kepler: latch-delta off=XXX pre=XXXXXXXX post=XXXXXXXX ::` (each diff)
- `:: kepler: latch-delta none ::` if identical (honest null — then the
  window doesn't track latch activity and the aperture idea needs the
  cross-boot variable instead).

The display lane's pull-10 latch runs in the same boot between your pre and
pass0 dumps — that's the experiment; no coordination needed beyond build
order already in the tree.

## Gates (DONE = all of these)

Read-only MMIO (the pre-dump + existing dumps; beacon BAR1 writes
unchanged). Full-knob check both arches + builder-path esp-x86 + `strings`
proof of the new markers in kernel.elf + default QEMU green both. Commit
ALL docs+code; delete scratch; `git status` clean; no push
(report "PUSH OWED: n").

Proposal first (`PROPOSAL-kepler-fence-pull17.md`, STATUS: PROPOSED).
