# BRIEF — kepler-display pull 19: relocate the surface (the decisive test)

Lane: **kepler-display** — `unaos/crates/kernel/src/drivers/gpu/kepler_display.rs`
ONLY. Read `docs/dev/GEMINI/README.md`, `video/INDEX.md`, and
`docs/dev/OS/08_VIDEO/KEPLER-METAL-LOG.md` sitting #28 first — the whole
sitting is context for this pull.

## The s28 finding this pull acts on

The overlap detector fired: our scratch surface at VRAM 0x1600000 sits
INSIDE the firmware's GOP framebuffer (0x20000 … 0x1C40000, 1800 rows ×
16384 B), 1400 rows in. Every panel result since s17 is therefore
ambiguous — we may have been painting the firmware's own scanout surface,
with the EVO latch doing nothing. Your s28 photo confirmed the geometry
to the pixel: rows 0–399 visible, bottom 22% of the panel, 25 bands, and
a straight untruncated-slope diagonal proving a 1:1 unscaled row map.
Nothing about the mapping is mysterious any more. The one open question
is whether the LATCH works at all.

## This pull — one variable: where the surface lives

1. Move the scratch surface clear of the GOP window:
   `surf2_offset = 0x4000000` (64 MB). Headroom check, state it in the
   proposal: GOP ends at 0x1C40000 (29.6 MB); `VramAllocator` hands out
   from 32 MB and takes only a few 4 KiB pages; BAR1 visible = 256 MB;
   our surface is 0x1C20000 (29.6 MB), so 64 MB + 29.6 MB = 93.6 MB, well
   inside the window and clear of both.
2. Change NOTHING else — same pattern, same markers, same pre-latch
   control frame, same latch/restore, same holds. The overlap line must
   now print `gop-overlap=no`; if it does not, STOP and report.
3. Keep both photo points (A pre-latch, B post-latch) — with the surface
   out of the GOP window, A must show the console.

Verdict key, and both outcomes are valuable:
- **Pattern appears in B** ⇒ the EVO latch is real, s17 stands, and the
  display lane graduates to a real console next pull.
- **B shows the console (nothing of ours)** ⇒ the latch has never worked;
  everything on the panel since s17 was direct FB painting. That is NOT a
  dead end: it means UnaOS already has a working framebuffer on this
  machine by writing linear/pitch-16384 into the GOP FB. Next pull would
  make that a first-class path and the latch becomes a separate question.

## DONE (specialist side)

Implement exactly as approved, commit ALL docs+code, delete scratch,
`git status` clean, no push — report "PUSH OWED: n". The coordinator runs
all builds and gates and delivers the sitting ESP.

Proposal first (`PROPOSAL-kepler-display-pull19.md`, STATUS: PROPOSED) —
include the VRAM headroom arithmetic.
