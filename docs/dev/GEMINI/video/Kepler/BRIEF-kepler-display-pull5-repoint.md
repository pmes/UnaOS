# BRIEF — kepler-display pull 5: repoint-the-surface (FIRST display write — Peter-approved)

Lane: **kepler-display** — `unaos/crates/kernel/src/gpu/kepler_display.rs`
ONLY. Read `docs/dev/GEMINI/README.md`, `video/INDEX.md`, and
`docs/dev/OS/08_VIDEO/KEPLER-METAL-LOG.md` sitting #14 first.

## The find this pull tests

s14's known-value scan produced exactly one hit in 16 KB:
**0x6101E0 = 0x00000200** — the GOP surface address >>8 (fb at VRAM
+0x20000). Hypothesis: 0x6101E0 is the armed scanout surface pointer in
256-byte units. Proof: repoint it and the panel shows the new surface.

## The experiment — ONE register write, fully reversible

Gated behind `nvidia-kepler-takeover` (the write knob). Sequence:

1. **Prepare a second surface** in VRAM via BAR1 at offset **0x1600000**
   (past fb end 0x20000+0x13C6800; 256-aligned). Fill with a solid,
   unmistakable color (e.g. 0xFF00FF00 green, full fb-size worth of rows —
   bounded fill, report bytes). Marker with offset+bytes.
2. Read HEAD_STAT + 0x6101E0, print (`pre`).
3. **Write 0x6101E0 = 0x00016000** (0x1600000>>8). Read back, print.
4. **Bounded panel window**: hold ~5 s (bounded spin like pull 3/4) so the
   bench can SEE the panel. Print HEAD_STAT during the hold (does the
   raster keep ticking?).
5. **Restore 0x6101E0 = original**, read back, print. Second ~2 s hold —
   panel should return.
6. Honesty branch: if the readback in (3) does not stick, or panel/HEAD_STAT
   show nothing, that is an HONEST refutation of "bare pointer, live-armed" —
   print it and DO NOT improvise extra writes (no update/commit doorbells,
   no other registers — those are pull 6 material if this refutes).

## Exact serial markers (verbatim)

- `:: kdisp: surf2 prep off=01600000 bytes=NNNNNNNN fill=FF00FF00 ::`
- `:: kdisp: repoint pre 6101E0=XXXXXXXX stat vert=XXXXXXXX horz=XXXXXXXX ::`
- `:: kdisp: repoint wrote=00016000 rb=XXXXXXXX ::`
- `:: kdisp: repoint hold t=<n>s stat vert=XXXXXXXX horz=XXXXXXXX ::` (per second)
- `:: kdisp: repoint restored rb=XXXXXXXX ::`
- `:: kdisp: repoint verdict rb-stuck=<yes|no> ::` (code states only what it
  can read; the panel verdict is the bench's)

## Gates (DONE = all of these)

The ONLY new register write is 0x6101E0 (two writes: repoint + restore).
Full-knob check both arches + builder-path esp-x86 + `strings` proof of the
new markers in kernel.elf + default QEMU green both. Commit ALL docs+code;
delete scratch; `git status` clean; no push (report "PUSH OWED: n").

Proposal first (`PROPOSAL-kepler-display-pull5.md`, STATUS: PROPOSED).
