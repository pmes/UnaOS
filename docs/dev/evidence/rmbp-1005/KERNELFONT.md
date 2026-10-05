# KERNELFONT — the desktop draws real typefaces (rmbp-ledger B359)

Branch `exec-rmbp-kernelfont`, cut at `c16370c5`. Merged first: `exec-rmbp-merge13` (boot-23 integration:
GLASSEYES, KCOMP, KBLIT, SETTINGSBUS, PREFSKERNEL, pixel_core) then `exec-text-fontbidi` @3e25aa10 (font_core
with bidi, complex shaping, `shape_fallback`). Both merges were clean.

## Design

**Finding.** Every string the desktop draws comes from `video/font.rs`: Noto Sans Mono pre-rasterized at
build time into fixed 7x16 / 9x20 atlases (`noto-sans-mono-bitmap`), ASCII only, one advance for every
glyph. `font_core` (SR48 + SR56) parses OpenType, shapes every script and rasterizes exactly, `no_std`,
zero dependencies, and the kernel never linked it.

**The seam (shared-core).** The engine that turns a string into pixels is written ONCE in `font_core::ui`
(`no_std`, no `unsafe`, zero deps): face roles (sans / serif / mono + script fallbacks), sizing from the
cell the caller lays out on, a byte-bounded glyph cache with eviction counts, a shaped-run cache, subpixel
x positioning (quarters, Skia's), `RenderMode::SkiaAaa` coverage through Skia's A8 pre-blend (contrast 0.2,
gamma 1.2 — the FONTCORE oracle fit), source-over blending into 0RGB `u32` surfaces, and the EDID ppi
reading. The kernel's `video::text` is the thin fulfiller: it reads the faces off `/system/fonts/` once the
volume is up, holds the engine behind a lock, and falls back to `video::font`'s bitmap atlases (saying so
on the wire) when a face is missing. The host proof drives the same `font_core::ui` code, so what is
measured on the host is the kernel's paint path.

**Milestones.**
- M1: `font_core` linked into the kernel; the builder stages DejaVu Sans / Sans Bold / Sans Mono / Sans
  Mono Bold / Serif (+ Noto Sans Arabic / Hebrew / Devanagari / Thai when the host has them) as DATA under
  `system/fonts/` with their licences; `video::text::service` loads them once the volume is mounted
  (`[kfont] load faces=<n> missing=<list> fallback=<none|bitmap|names> cache_kib=<n> heap_kib=<n>`); the
  glyph cache is bounded at HEAP_SIZE/128 clamped to 512 KiB..2 MiB.
- M2: `video::text` (same call shapes as `video::font`: `draw_text`, `draw_row`, `draw_glyph_fb`, plus
  `measure`) and every caller moved to it.
- M3: `system.display.font` (enum sans/serif/mono) and `system.display.font_size` (CSS px 9..=32, default
  13) as Principia schema rows; device size = font_size x ppi/96 from the panel's EDID, capped by the cell.
  The Settings window's Display tab shows the sample line.
- M4: host harness `font_core/tests/kernelfont.rs`: the login-screen strings through `font_core::ui` into an
  RGBA canvas vs Chromium rendering the same strings in the same DejaVu face and size (within-8 share);
  `tests font` on the metal.

**Witness.** `:: KERNELFONT: faces=<n> cache_kib=<n> evictions=<n> fallback=none glyphs_drawn=<n>
ms_per_1000=<n> -> PASS ::`.

**Stays owed** (see the end of this file for the final list).
