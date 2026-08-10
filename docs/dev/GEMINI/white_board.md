# WHITE BOARD — 2026-08-09 (GR24, first pass)

Questions for Peter, each with the background to answer it. Nothing else lives here.

**Answered and off the board:** the control hues (the loud macOS set shipped with knurl — your
size-and-hue ruling) · MEGABOOM matching (by ADDRESS, your 2026-08-08 ruling — merged with zero
new transmit in the match itself) · lane shutdown (done; the seat adopted all seven branches).

---

## Q1 — igpu round 13 was amended AFTER your clearance. Veto or confirm before it flies.

The adoption review found the round's answer gated on registers never read back on this machine:
the post-switch check demanded an echo from `SWITCH_DISPLAY`/`SWITCH_EXTERNAL` (write-side ports
with no proven echo — a switch that WORKED could have failed the comparison, blanked the panel,
and aborted before the first AUX transaction), and the pre-switch gate's `SWITCH_DISPLAY==DIS`
term conflated that register with `READ_DISPLAY`. Both are now advisory; only metal-proven
registers abort or vote (DDC's echo, the two `READ_*` ports). The RUNBOOK's predicted transcript
also contradicted itself — its own success line would have printed `gmux=FAILED` and sent you to
a power cycle on a correct boot; fixed. **The flight artifact is therefore no longer byte-for-byte
the one you cleared.** Every change makes it strictly less likely to refuse or lie, none touches
the parachute, and the armed build type-checks green — but post-clearance amendment is yours to
veto. Diff: `git log --oneline 509b9706..50969ab8`.

## Q2 — Windows 122–158 px wide now silently lose their whole control cluster. Which way?

KNURL's 24 px discs moved the cluster's width floor from 122 to 158 px of box width. A live ring-3
window whose box lands in that band draws NO close/minimise/zoom — no witness, no skip line — and
its app has no way to know. The fixtures were all re-sized to clear the floor, so no gate sees it.
Options: (a) accept and witness it (one `[wm]` line when a cluster is declined for width — cheap,
honest, ships today); (b) let narrow windows keep a smaller disc set (breaks "the size a Mac draws
them"); (c) set a minimum window width ≥ 158 so the band cannot exist. I lean (a)+(c) together —
say the word and it lands this round.

## Q3 — The discs sit 5 px from the frame; a Mac gives them ~16. Raise TITLE_HEIGHT?

`(TITLE_HEIGHT − CONTROL_BOX) / 2 = (34 − 24) / 2 = 5 px` clearance. KNURL flagged it in-tree and
deliberately did not take the question — chrome proportions are the taste gate. Raising
TITLE_HEIGHT to ~40–56 gives macOS-like air around the discs but costs every window that much
content height. Veto by eye on the next boot, or name a number.

## Q4 — `ls`/`cat`/`stat` are still dead on an internal-reader boot. Which volume should they mean?

`vug` launches now (the resolver, re-resolve, and loader all bind `mount_program_source()`), but
~23 other `fat::mount()` sites in shell.rs — the whole FAT verb family — still ask the default
handle, which does not exist on a machine booted from the internal SD reader. Boot AR shows you
typing `ls` twice before `vug`. The read verbs could move to the program source mechanically, but
the card mounts READ-ONLY there, so the write verbs (`write`, `rm`, `mkdir`...) cannot follow —
they would need either a loud per-verb refusal ("this volume is read-only") or a split where reads
follow the program source and writes stay on the (absent) default and refuse with the census line.
One ruling wanted: **reads follow the program source everywhere, writes refuse loudly on a
read-only source** — say yes and it is a mechanical arc; say different and I brief it your way.

## Q5 — Boot planning: four things want glass, two of them are flights.

Never flown: the drag with sliver-erase (the flicker fix) · the 24 px knurled controls in the
macOS hues · the dock + close-isolation already on the card from GR23 · the MEGABOOM by-address
selection (its first boot decides the connect). Flights, both gated green and both needing your
go: **igpu round 13** (pending Q1 — blanks the panel ~2.4–2.5 s) and **kepler FENCE** (first
flight ever — QEMU has no Kepler, so metal is its only test; the verdict now requires both the
falcon-side AND host-side ENGINE_STATUS to carry the bit, re-checks the hold at the stimulus, and
force-clears host-side if the falcon's clear loses the race). The desktop work wants you clicking;
the flights blank or perturb the panel — same card or separate ones, your call. Media stages
whenever you say.
