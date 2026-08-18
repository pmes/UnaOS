# BRIEF — kepler-display pull 3: candidate decode (read-only, multi-pass)

Lane: **kepler-display** — `unaos/crates/kernel/src/gpu/kepler_display.rs`
ONLY. New session? Read `docs/dev/GEMINI/README.md` first, then
`video/INDEX.md`, then `docs/dev/OS/08_VIDEO/KEPLER-METAL-LOG.md` sitting #12.

## What pull 2 bought (do not re-derive)

The s12 head0↔head1 differential landed clean (49/40 live rows, uncapped).
Verdict: **no offset in the 0x616000 head block holds the GOP surface address
in any obvious shift** (nothing 0x200/0x20000/0x90020000-shaped) — the surface
pointer is elsewhere or encoded. The diff sorted head-0-only rows into:
- **frame-varying** (live-scan telemetry): 0x118/0x314/0x53C (one shared value,
  0x0E35→0x0D9E across passes), 0x340/0x344 (track HEAD_STAT raster);
- **stable config-shaped**: 0x310=0x008959E6 (the one address-shaped
  candidate), 0x520=0x00000600, 0x604=0x00780000, 0x614=0x00022500;
- rich DIFF rows vs head-1 near-reset defaults: 0x30C, 0x348/0x34C
  (timing-shaped), 0x3F4/0x3F8, 0x538, 0x600, 0x610.

## This pull — classify the candidates without writing anything

Read-only. Goal: pin down which stable candidate is surface/pitch/format
config vs mode timing, so write-a-pixel (pull 4, Peter-gated) has a proven
target.

**Milestone 1 — dense window reads.** Full sequential dumps (live-values +
sentinels, no skip-zeros filtering this time) of three narrow windows on BOTH
head 0 and head 1: 0x300–0x35F, 0x3F0–0x40F, 0x5F0–0x61F. Zeros print as
zeros — in a dense window a zero row is data.

**Milestone 2 — time-separated passes.** Three passes of the same windows
separated by bounded delays (use the existing bounded-poll idiom, ~2 raster
frames apart minimum). Anything that moves is telemetry, not config; the
brief expects 0x310/0x520/0x604/0x614 to hold still.

**Milestone 3 — arithmetic cross-check (in-kernel, printed).** For each
stable head-0 candidate, print derived interpretations against known truth
(GOP fb 0x90020000, vram_off 0x20000; panel timing from HEAD_STAT):
value<<8, value<<12, value as pitch (bytes and /4), value vs hsync/vsync
totals. Marker per candidate. This is printing arithmetic, not guessing
semantics — no nouveau-derived meanings.

## Exact serial markers (verbatim)

- `:: kdisp: window head<H> pass<P> off=XXX val=XXXXXXXX ::` (dense rows)
- `:: kdisp: window head<H> pass<P> done rows=N ::`
- `:: kdisp: cand off=XXX stable=<yes|no> v0=XXXXXXXX v1=XXXXXXXX v2=XXXXXXXX ::`
- `:: kdisp: cand off=XXX shl8=XXXXXXXX shl12=XXXXXXXX pitch4=DDDD ::`
- keep the existing begin-trace/caps/stat header markers unchanged

## Gates (DONE = all of these)

Read-only: no register writes anywhere in this pull. Full-knob check
(`UNAOS_IVB UNAOS_KEPLER UNAOS_KEPLER_TAKEOVER UNAOS_KEPLER_FIFO ./arroyo
check`, both arches) + builder-path esp-x86 build + `strings` proof of the
new markers in kernel.elf AND BOOTX64.EFI + default QEMU regression green.
Bounded delays/polls; commit ALL docs+code; delete scratch files;
`git status` clean.

Proposal first (`PROPOSAL-kepler-display-pull3.md`, STATUS: PROPOSED) — no
implementation until approved.
