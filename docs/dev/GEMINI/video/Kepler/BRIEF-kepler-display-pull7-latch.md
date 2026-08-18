# BRIEF — kepler-display pull 7: assembly write + UPDATE latch

Lane: **kepler-display** — `unaos/crates/kernel/src/gpu/kepler_display.rs`
ONLY. Read `docs/dev/GEMINI/README.md`, `video/INDEX.md`, and
`docs/dev/OS/08_VIDEO/KEPLER-METAL-LOG.md` sitting #16 first.

## Facts this pull stands on (all capture-verified, s16)

- 0x640460 = 0x00000200 — the assembly-side surface slot (EVO core method
  mirror: head 0 methods at +0x400, OFFSET slot +0x60), in a coherent record
  with 0x640420 = 0x07380BAF (proven raster totals) and the geometry cluster
  at 0x640468–0x6404C8.
- 0x6101E0 (s15: read-only armed readout) and 0x61D1E0 both hold the armed
  0x200; 0x61D014 holds 0x00020000.
- Method layout puts UPDATE at slot +0x80 → 0x640080 is the latch trigger
  candidate.

## The experiment — arm assembly, latch, watch, restore

Two new writable registers this pull: **0x640460** and **0x640080**. Nothing
else. Sequence:

1. Prepare surf2: green fill at VRAM +0x1600000 (pull-5 code shape), marker
   `:: kdisp: surf2 prep off=01600000 bytes=NNNNNNNN fill=FF00FF00 ::`.
2. Pre-state: read 0x640460, 0x6101E0, 0x61D1E0 —
   `:: kdisp: latch pre asm=XXXXXXXX armed=XXXXXXXX shadow=XXXXXXXX ::`.
3. **Write 0x640460 = 0x00016000.** Readback:
   `:: kdisp: latch asm-wrote=00016000 rb=XXXXXXXX ::`.
4. Hold ~2 s polling 0x6101E0 once/s (does assembly alone self-latch on
   vblank?): `:: kdisp: latch selfcheck t=<n>s armed=XXXXXXXX ::`.
5. **Write 0x640080 = 0x00000000 (UPDATE).** Then ~5 s hold, once/s:
   `:: kdisp: latch update-wrote rb0080=XXXXXXXX ::`
   `:: kdisp: latch hold t=<n>s armed=XXXXXXXX stat vert=XXXXXXXX ::`
   PANEL IS THE VERDICT during this hold (green?).
6. Restore: write 0x640460 = original, write 0x640080 = 0 again, ~2 s hold,
   read all three: `:: kdisp: latch restored asm=XXXXXXXX armed=XXXXXXXX shadow=XXXXXXXX ::`.
7. Verdict marker (code states only readables):
   `:: kdisp: latch verdict asm-stuck=<y|n> armed-followed=<y|n> ::`.

Honesty rules: if the 0x640460 write snaps back at step 3, SKIP the update
write (nothing armed — print the skip: `:: kdisp: latch skip — asm rb
unchanged ::`) and go to restore. No other writes, no retries, no improvised
registers.

## Gates (DONE = all of these)

Writes limited to 0x640460 + 0x640080 (+ VRAM fill via BAR1). Full-knob check
both arches + builder-path esp-x86 + `strings` proof of new markers in
kernel.elf + default QEMU green both. Commit ALL docs+code; delete scratch;
`git status` clean; no push (report "PUSH OWED: n").

Proposal first (`PROPOSAL-kepler-display-pull7.md`, STATUS: PROPOSED).
