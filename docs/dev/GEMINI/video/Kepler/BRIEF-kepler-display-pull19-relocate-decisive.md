# BRIEF — kepler-display pull 19: the whole panel, at the right address

Lane: **kepler-display** — `unaos/crates/kernel/src/drivers/gpu/kepler_display.rs`
ONLY. Read `docs/dev/GEMINI/README.md`, `video/INDEX.md`, and
`docs/dev/OS/08_VIDEO/KEPLER-METAL-LOG.md` sitting #28 — that entry is the
context for this pull, including what it retires.

*(Re-scoped 2026-07-25 after Peter's live-wire observation. An earlier
draft of this brief asked you to relocate the surface to prove whether the
latch works. That question is now settled — see below — so don't do that.)*

## What s28 settled

Two independent facts, same boot:
1. The graphic appeared on the panel **before `pm-step fill done` printed**
   — during the fill, pixels landing as we wrote them, well before the
   latch ran.
2. `armed=00000200` and `shadow=00000200` at t=1 and t=5. 0x200 << 8 =
   VRAM 0x20000 = the GOP framebuffer. The head never took our pointer.

So: the EVO arm+UPDATE path has never repointed scanout, and every panel
result since s17 was us painting directly into the firmware's framebuffer
— which our scratch surface overlaps at row 1400. Your barcode, bands and
diagonal all rendered with correct geometry. **That means we already have
a working framebuffer on this machine.** This pull makes it first-class.

## This pull — full panel, correct origin, no latch

1. Base the fill at the firmware framebuffer: `gop_vram_offset` (0x20000
   on this machine — take it from the existing computed value, do not
   hardcode), pitch 16384, all 1800 rows, full 2880-px width.
2. **Remove the latch entirely from this path** — no 0x640460 write, no
   0x640080 UPDATE, no restore pairing. There is nothing to restore: we
   are drawing on the surface the firmware is already scanning. Keep the
   pre-state reads and the reg-dump (they are cheap and still evidence).
3. Pattern: keep your barcode + bands + diagonal + fiducials exactly as
   they are — they are now a full-panel calibration target. With the
   correct origin, expect: fiducials at the very top and bottom of the
   panel, band 0 (unique colour) at the top, band_idx running 0…112 top to
   bottom, and the diagonal sweeping the full height corner to corner.
4. Markers: `:: kdisp: fb-draw base={:08X} pitch={} rows={} bytes={:08X} ::`
   then hold `t=1..5` then `done`. Keep the gop-overlap line — it should
   now report the intentional, exact overlap (base == gop base).
5. One hold, one photo. Verdict: full-panel pattern, fiducials visible top
   AND bottom, diagonal corner to corner = **UnaOS owns the panel**.

## Then what (context, not this pull)

Next after this lands is wiring `video::fbcon` to this path so the kernel
console renders on the rMBP panel. The EVO repoint becomes a separate
known-unknown — the armed register never follows our writes, so the real
arming path is elsewhere; we'll come back to it when we need page-flipping
rather than a single framebuffer.

## DONE (specialist side)

Implement exactly as approved, commit ALL docs+code, delete scratch,
`git status` clean, no push — report "PUSH OWED: n". The coordinator runs
all builds and gates and delivers the sitting ESP.

Proposal first (`PROPOSAL-kepler-display-pull19.md`, STATUS: PROPOSED).
