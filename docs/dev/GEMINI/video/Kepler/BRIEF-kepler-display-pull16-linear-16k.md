# BRIEF — kepler-display pull 16: ONE linear fill at the hw's own pitch (0x4000)

Lane: **kepler-display** — `unaos/crates/kernel/src/drivers/gpu/kepler_display.rs`
ONLY. Read `docs/dev/GEMINI/README.md`, `video/INDEX.md`, and
`docs/dev/OS/08_VIDEO/KEPLER-METAL-LOG.md` sitting #25 first.

## The s25 verdict this pull acts on

Your mirror recon delivered the answer: the head-0 ISO surface cluster
reads size=07080B40 (1800×2880), storage=01004000, format=0000CF00,
offset=0x200. Storage decodes as LAYOUT=PITCH (bit 24) with pitch>>8 =
0x40 → **the scanout is LINEAR with a 16384-byte row stride.** No blocks,
no GOBs. Every artifact since s19 was aliasing against wrong layouts.

## This pull — one cycle, linear, single variable

1. Fill the scratch surface at offset 0x1600000 LINEAR:
   for y in 0..1800: row base = y * 16384; write 2880 ruler pixels
   (identical ruler pattern to prior pulls: 256px white column, 8px black,
   then 64-row color bands with black separator rows); fill bytes
   2880*4..16384 of each row BLACK (real padding bytes).
2. Markers:
   `:: kdisp: lin-step pitch=4000 fill done bytes=NNNNNNNN ::`
   (bytes = 1800*16384 = 01C20000)
   `:: kdisp: lin-step pitch=4000 hold t=<n>s ::` (t=1..5)
   `:: kdisp: lin-step pitch=4000 done ::`
3. Latch/restore exactly as always (0x640460 → 0x640080, restore-paired).
   ONE cycle only. Remove/gate the pull-15 recon dumps and the old matrix
   loop (keep the recon code compiled behind a bool, as you did for the
   takeover in pull 15).
4. Do NOT touch 0x468/0x46C/0x470 — we scan through firmware's config by
   design this pull; matching it is the test.

Verdict key: seam-free ruler with solid white left column = MAPPING
SOLVED (linear, pitch 16384). Photos: one per hold as usual.

## Gates (DONE = all of these)

Writes remain exactly 0x640460 + 0x640080 (one cycle, restore-paired).
Full-knob `UNAOS_IVB=1 UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1
UNAOS_KEPLER_FIFO=1 ./arroyo check` both arches; default `./arroyo test` +
`./arroyo test-arm` green; builder-path `UNAOS_USBDEBUG=1 <same knobs>
./arroyo esp-x86`; strings-proof the new `lin-step` markers in
`target/x86_64_esp/kernel.elf`. Commit ALL docs+code; delete scratch;
`git status` clean; no push (report "PUSH OWED: n").

Proposal first (`PROPOSAL-kepler-display-pull16.md`, STATUS: PROPOSED).
