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


## Results

### The callers (every bitmap text path before this arc, and where each went)

| surface | file : call | before | now |
|---|---|---|---|
| login screen (labels, fields, buttons, user rows, footer) | `video/login.rs` `text`, `field`, `button`, `user_row`, `repaint` | `font::draw_text` Body (mono grid) | `text::draw_text` **Ui** (DejaVu Sans, proportional); caret, button centring and row truncation by `text::advance` / `text::fit` |
| menu bar (caption, battery %, BRT, clock, date) | `video/menubar.rs` | `font::draw_row` Chrome | `text::draw_row` Chrome; clock and percent right-aligned by the shaped width |
| window captions | `video/wm.rs` `draw_title` | `font::glyph` per cell, `put_pixel` over the computed strip colour | `text::draw_with` (same write-only blend, glyphs end at `max_w`) |
| window menus, app menu, chords | `video/winmenu.rs` (5 sites) | `font::draw_row` | `text::draw_row`; `About <app>` suffix and chords placed by `text::advance` |
| crystal menu, power panel | `video/crystal.rs` (2) | `font::draw_row` | `text::draw_row` |
| dock tile captions, dock menu | `video/dock.rs` (2) | `font::draw_row` | `text::draw_row`; tile captions cut by `text::fit` to the tile's budget |
| shortcuts overlay | `video/shortcuts.rs` | `font::draw_row` | `text::draw_row` |
| Quarry columns | `video/quarry/live.rs` `text` | `font::draw_text` (Body / Chrome by panel width) | `text::draw_text` (same faces, `Face` type moved) |
| file viewer, editor | `video/fileview.rs` (2), `video/textedit.rs` | `font::draw_text` Body | `text::draw_text` Body (DejaVu Sans Mono on the 7 px grid — the styled-span arithmetic holds) |
| Settings | `video/settings.rs` (5) | `font::draw_text` Body | tabs, labels, buttons in **Ui**; value fields Body; Display tab: `Font` row + sample line |
| Activity | `video/activity.rs` (4) | `font::draw_text` Body | `text::draw_text` Body |
| Facet captions | `video/facet.rs` (3) | `font::draw_text` Body | `text::draw_text` Body |
| installer dialogs | `video/instgui.rs` | `font::draw_text` Body | `text::draw_text` Body |
| console (post-seam cells) | `video/fbcon.rs:168` | `font::draw_glyph_fb` | `text::draw_glyph_fb` (one char per 7x16 cell, pixels kept inside the cell) |
| boot step label | `splash.rs` `step_label` | `font::glyph` | `text::draw_with` (faces are not loaded that early: it draws the bitmap face through the seam) |

Not moved, on purpose: `selftest.rs`'s FONTAA fixture (it tests the bitmap atlas itself, which is now the
fallback face); `fbcon`'s pre-seam font8x8 path (before the volume mounts no face can be read); `pal.rs`'s
8x8 `draw_text` (aarch64 PAL status/pulsewin surfaces, not on the rMBP desktop). **The Lumen window is NOT
moved** — see Owed.

### Numbers

| what | value |
|---|---|
| x86 metal shape `.text` (release, `size -A`) | 6,252,696 B at the merged base `bca2e663` → 6,088,808 B at M4 (**−163,888 B**) |
| `.rodata` | 666,100 → 834,956 (**+168,856 B**: font_core's UCD / shaping tables) |
| `.data.rel.ro` / `.data` | 248,432 → 260,888 (+12,456) / 53,800 → 54,116 (+316) |
| kernel ELF file | 8,920,776 → 9,009,512 (+88,736 B); loadable sections net **+17,740 B** |
| faces staged (this host) | DejaVuSans 759,720 · Sans-Bold 708,920 · SansMono 343,140 · SansMono-Bold 334,268 · Serif 380,660 · Serif-Bold 356,668 = **2,883,376 B**; licence bitstream-vera (DejaVu changes public domain), from `fonts-dejavu-core` / `fonts-dejavu-mono` copyright files. Noto Sans Arabic/Hebrew/Devanagari/Thai: **not installed on this host** (staged when the builder host has `fonts-noto-core`, or via `UNAOS_FONT_DIRS`; OFL 1.1) |
| heap | the faces' bytes once per boot (2,815 KiB, never freed) + the glyph cache bound HEAP_SIZE/128 clamped 512 KiB..2 MiB = **2 MiB on x86** (256 MiB heap), 512 KiB on aarch64 |
| cache behaviour (host) | 16 KiB cap over 5 sizes of an 80-glyph line: 251 misses, 2 flushes, 199 evictions, never over the cap; 4 MiB cap: 0 evictions, 46,178 B used |
| sizes (kernel rule, host-computed) | Body DejaVu Sans Mono **11.5 px** (7x16 cell, baseline 12); Chrome DejaVu Sans Bold **14 px** (9x20 cell, baseline 15); Ui **13 px** at 96 ppi; on the rMBP (EDID 227 ppi 13-inch / 221 ppi 15-inch) `font_size` 13 CSS px wants 30.7 / 29.9 device px and is **capped at 13.5 px** by the 16 px cell |
| Chromium oracle (login strings, 52 jobs, 888 glyphs) | within-8 **99.2 %**: Ui 13 px 98.6 % (mean 3.64), Ui 13.5 px 99.1 % (5.11), Chrome 14 px bold 100.0 % (2.95), Body 11.5 px mono 99.1 % (3.63); 0 colour-fringed pixels |
| widths | 52/52 within 0.5 px of `canvas.measureText`, worst 0.0000 px (frozen in `tests/data/kernelfont_chrome.tsv`) |

The `.text` drop is measured, not explained; the likeliest reading is codegen (the ~40 call sites no longer each
inline `font::draw_row`/`draw_text`'s loops beside their own code), and it is offered as a hypothesis only.

Evidence (`kernelfont/`): `ui13.png`, `ui135.png`, `chrome14.png`, `body115.png` — per string, Chromium, the
kernel engine, |diff| x 4, at 2x.

### Gates

| leg | command | exit |
|---|---|---|
| charter | `bash /home/user/UnaOS/unaos/scripts/charter-check.sh <worktree>/unaos` | 0 |
| x86 metal shape | `cargo +nightly check --release --target ../../x86_64-unaos.json … --features "wc,quarry,ftdirx,login,loginst,nvidia-kepler-vblank,smc,usbnet,hda,hda-tone,facet,beam,sdw,sdwrite,sdhcblk,selfhost,linuxabi,ahci,unafs,busreg,lumen,netring3,prefs_reset,census,installdemo,instgui,witness,selfdiag,ahciroot,btc"` | 0 |
| aarch64 desktop | `… --target ../../aarch64-unaos.json … --features "login,loginst,virt_el0,desktop_firmware,quarry,facet"` (user_blob head `28 00 80 d2`) | 0 |
| aarch64 no desktop | `… --features "login,loginst,virt_el0"` (no font_core linked) | 0 |
| builder | `cargo test -p unaos-builder kernelfont` (unaos/) | 0 |
| font_core | `cargo test --release -p font_core` (13 suites incl. `kernelfont`, 9 tests) | 0 |
| prefs | `cargo test -p prefs_core -p principia`; `python3 tools/prefs-schema-check.py` → `declared=31 referenced=31 undeclared=0 -> PASS` | 0 |

### The wire a metal boot should print

Once the volume is up (the desktop service pass, no boot line before it):
`[kfont] load dir=/system/fonts faces=6/10 fallback=none scripts_missing=noto-arabic+noto-hebrew+noto-devanagari+noto-thai font_kib=2815 cache_kib=2048 ppi=227 font=sans font_size=13 body=dejavu-mono-11.50 chrome=dejavu-sans-14.00 ui=dejavu-sans-13.50 ms=<n>`
(`faces=10/10 … scripts_missing=none` when the builder host has the Noto packages; `dir=/boot/system/fonts`
when the SSD is the UnaFS root). Then `tests font`:
`[kfont] fixture font_kib=2815 cache_bytes=<n> hits=<n> misses=<n> flushes=0 runs_shaped=<n> contended=0 aa_mid=1 ink=1 body=dejavu-mono-11.50 chrome=dejavu-sans-14.00 ui=dejavu-sans-13.50`
`:: KERNELFONT: faces=6 cache_kib=2048 evictions=0 fallback=none glyphs_drawn=<n> ms_per_1000=<n> -> PASS ::`.
No face on the volume: `[kfont] load faces=0/10 fallback=bitmap reason=absent path=/system/fonts/DejaVuSans.ttf tries=15`
and the fixture reads `fallback=bitmap … -> FAIL`. A `pref set system display.font_size 11` prints
`[kfont] restyle font=sans font_size=11 ppi=227 chrome=dejavu-sans-14.00 ui=dejavu-sans-13.50` (the cap) — see Owed.

## Owed

1. **The Lumen window.** LUMEN.ELF draws its own font8x8 inside its 4 MiB ring-3 window. Linking `font_core::ui`
   into it was built and measured (+53,456 B of code with LTO) and reverted: after the image the window has
   589,424 B of heap left, which cannot hold DejaVu Sans Mono (343 KB) + a glyph cache beside the TLS heap and the
   parsed trust store. It needs a kernel text verb (the kernel as the fulfiller, drawing into the window's
   surface) or a larger `USER_WINDOW_BYTES`; a seat decision.
2. **DPI does not reach the furniture.** The theme's cells (7x16 body, 9x20 bar, 34 px title strip) are device px and
   are not scaled by the panel's ppi, so on the rMBP `font_size` is capped at 13.5 px (Ui) and the bar keeps 14 px.
   The ppi is read and printed; scaling the theme by ppi/96 is the follow-on that lets `font_size` take effect.
3. Windows keep the pixels they painted before the faces load (the faces load on the first desktop service pass
   that finds the volume, then damage the screen); a surface repaints in DejaVu on its next own repaint.
4. Script fallbacks need the Noto packages on the builder host; italics have no staged face; the console's
   pre-seam path and `pal.rs` stay font8x8.
5. The Settings `Font` row is a read-out; the family and size are set with `pref set` (Operator) — a picker is owed.
