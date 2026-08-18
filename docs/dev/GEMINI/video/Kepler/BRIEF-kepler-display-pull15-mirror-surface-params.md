# BRIEF — kepler-display pull 15: read the hw's own surface params (mirror decode)

Lane: **kepler-display** — `unaos/crates/kernel/src/drivers/gpu/kepler_display.rs`
ONLY. Read `docs/dev/GEMINI/README.md`, `video/INDEX.md`, and
`docs/dev/OS/08_VIDEO/KEPLER-METAL-LOG.md` sitting #24 first.

## The s24 verdict this pull acts on

Pitch×bw refuted: all four (bw,pg) combos still cluster-seamed. The
parameter ladder is exhausted — GOB 64B×8 is proven, but every
bh/bw/pg permutation leaves residual seams. The road changes: STOP
GUESSING, READ THE ANSWER. Our latch only ever swaps the surface pointer
(0x640460); the pitch / block-mode / size the hardware scans with are
still whatever firmware programmed for its own surface, and those live in
the same EVO core method mirror we already latch through. Decode them.

## This pull — read-only dense dump, three passes, zero writes

1. Pre-takeover (before any fill/latch this boot), dump the head-0 method
   window dense: 0x640400 through 0x6405FC inclusive, one row per reg:
   `:: kdisp: mirror-sp off=XXX val=XXXXXXXX ::` (off relative to 0x640400)
   Tag absent-pattern values (FFFFFFFF / BAD0xxxx) with ` ABSENT?` as in
   prior recon pulls.
2. Repeat the identical dump a second pass after a ~100 ms settle
   (volatility check — the s15/s18 lesson), marker `mirror-sp2`.
3. Known-value cross-check pass, printed as summary lines:
   - `:: kdisp: mirror-sp ptr-slot val=XXXXXXXX expect=00090000-ish (fw surface ptr>>8?) ::`
     (whatever 0x640460 currently holds, print it and note the shift
     convention you infer from it vs the GOP address 0x90020000)
   - flag every register in the window whose value is plausibly:
     (a) a pitch in bytes or GOBs for a 2880-wide surface (0x2D00, 0xB400,
     0x2D0, 0xB4, 180, 192, 256 or shifted forms), (b) a WxH pack
     (2880/1800 = 0xB40/0x708 in any halfword arrangement), (c) a small
     log2-ish field pack (block-mode candidates: values with only low
     nibbles set, e.g. 0xNN where N<8 per nibble).
   Print each flag as `:: kdisp: mirror-sp cand off=XXX val=XXXXXXXX kind=<pitch|wh|blockmode> ::`
4. NO fill, NO latch, NO writes anywhere this pull — the takeover routine
   runs in recon-only mode for this boot (keep the code path gated so the
   fill/latch machinery stays in place for pull 16).

Deliverable: the decoded (pitch, block-mode, size) the firmware scans
with. Pull 16 then re-runs ONE fill cycle matching those exact values —
if the mirror really is the scan config, that cycle is seam-free and the
mapping is over.

## Gates (DONE = all of these)

ZERO writes (read-only recon). Full-knob `UNAOS_IVB=1 UNAOS_KEPLER=1
UNAOS_KEPLER_TAKEOVER=1 UNAOS_KEPLER_FIFO=1 ./arroyo check` both arches;
default `./arroyo test` + `./arroyo test-arm` green; builder-path
`UNAOS_USBDEBUG=1 <same knobs> ./arroyo esp-x86`; strings-proof the new
`mirror-sp` markers in `target/x86_64_esp/kernel.elf`. Commit ALL
docs+code; delete scratch; `git status` clean; no push (report
"PUSH OWED: n").

Proposal first (`PROPOSAL-kepler-display-pull15.md`, STATUS: PROPOSED).
