# WHITE BOARD — 2026-08-07 (GR21)

Two questions, both SDHC-4c planning — neither blocks anything now; decide any time
before the 4c arc starts.

## Q1 — When 4c lands, does the default x86 image ever ship with SD write (`sdw`) ON, or does it stay knob-gated?

Background: 4b mounted the internal card READ-ONLY with exactly one FAT writer allowed on
x86. The 4c design note (`~/unaos-bench/scratch/gr21/sdhc4c-writer-shape.md`) recommends
the flight-recorder shape — host-staged contiguous file, kernel writes only in place inside
a one-shot-armed LBA extent, no FAT-chain mutation ever — which keeps the single-writer
proof intact by construction. Even so, every write path to the only persistent internal
medium is brick surface. **Seat recommends: keep it knob-gated through 4c and revisit at
the one-volume collapse.** Only reason to ship it on: the flight recorder on the internal
card becomes useful on every boot without bench setup.

## Q2 — Will you spend one flight (and a spare >29 MiB card) proving the rMBP firmware can boot from the internal SDXC slot at all?

Background: the one-volume collapse (boot medium = internal card, USB reader retired) has
two hard gates before any code matters: the firmware must enumerate the internal slot as a
boot device, and the card must be bigger than the current 29 MiB v1.x Panasonic. Neither is
knowable from software we control — it is one experiment: ESP image on a spare SD card in
the internal slot, does the boot picker offer it. If the firmware refuses, the collapse
target changes (USB stays the boot medium; the internal card stays data), and 4c's design
is unaffected either way. No urgency — this only sequences when the collapse arc gets
scheduled.
