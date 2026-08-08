# WHITE BOARD — 2026-08-07 (GR21)

## Q1 — Park the Kepler FENCE lane in favour of a GEN7 3D lane?

Background: the 3D assessment (`~/unaos-bench/scratch/gr21/x86-3d-assessment.md`) is
decisive on paper: Kepler 3D needs a GPU MMU that doesn't exist, a pushbuffer that has
never been written, and FECS context-switch microcode against undocumented PGRAPH
registers (nouveau needed years); the HD 4000 (GEN7) needs **no firmware blob**, has
public PRMs, and UnaOS already owns BAR0/GGTT/ring-submit plumbing. Proposed shape:
new `gen7.rs` lane, render-offload only — GEN7 draws into GGTT system RAM, the Kepler
keeps the panel and delivers (no gmux switch; flagged deliberately). First arc is one
boot, ~150 lines, decisive either way: forcewake ack or no-ack — a `no-ack` kills an
8–12 arc plan for the price of one. The kepler lane's DISPLAY work stays regardless —
it is the delivery path. **Seat recommends: yes, park FENCE, keep DISPLAY.**
(If FENCE is kept instead, a licensing read becomes urgent: NVIDIA's blob is not
GPL-compatible, and nouveau's blob-free ctxsw microcode is GPL-2.0-only —
incompatible with our GPL-3.0-or-later in the copy direction, file-by-file at best.)

## Q2 — When SDHC-4c lands, does SD write ever ship ON by default, or stay knob-gated?

Background: 4b mounted the internal card READ-ONLY, one FAT writer allowed on x86. The
4c note (`~/unaos-bench/scratch/gr21/sdhc4c-writer-shape.md`) recommends the
flight-recorder shape — host-staged contiguous file, in-place writes inside a one-shot
LBA extent, zero FAT-chain mutation — keeping the single-writer proof by construction.
Still brick surface on the only persistent internal medium. **Seat recommends:
knob-gated through 4c; revisit at the one-volume collapse.**

## Q3 — Spend one flight (and a spare >29 MiB card) proving the firmware can boot the internal SDXC slot?

Background: the one-volume collapse (boot medium = internal card) has two gates no
software we control can answer: firmware willingness to boot that slot, and a card
bigger than the 29 MiB Panasonic. One experiment answers both: an ESP image on a spare
card in the internal slot — does the boot picker offer it. If refused, the collapse
target changes (USB stays boot; internal stays data) and 4c is unaffected either way.
No urgency until the collapse arc is scheduled.
