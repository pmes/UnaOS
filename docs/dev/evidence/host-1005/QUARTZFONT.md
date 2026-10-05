# QUARTZFONT — quartzite's text on UnaOS's own font_core

Ledger row SR64. Branch `exec-host-quartzfont`, cut at `2d4e1b12` (exec-rmbp-merge14, which carries AETHERFONT).
Lane: `libs/quartzite` (the vessels' GUI toolkit; aether-shell's chrome) and a new shared crate, `libs/text_host`.

## The finding

The row assumed quartzite rasterized its chrome text with `ab_glyph`. It did not. `quartzite::text`, the only
`ab_glyph` user, was **dead code**: an embedded Hack-Regular.ttf, glyphs thresholded at coverage > 0.5 (no
anti-aliasing), and no caller anywhere in the tree. The chrome's real text came from the platform toolkit:
**Pango** on the GTK face (`Button::with_label`, `Entry`) and CoreText on the macOS face. So "ab_glyph out of
the lock" meant removing dead code. "Quartzite's text through font_core" meant quartzite drawing its own text
instead of handing strings to Pango. This arc does both, for every widget aether-shell's chrome is made of.

## What it does now

| | spec / reference | code |
|---|---|---|
| **`libs/text_host`** (new crate; two users justify it). Aether's fontconfig reader, face database (Chromium's Linux family resolution, css-fonts-4 §5.2 selection, Blink synthesis, the fontconfig `sans-serif` fallback order), loaded-face store and Skia-mode rasterizer (SkiaAaa, quarter-pixel phases, Skia A8 pre-blend, FreeType embolden, Blink skew), moved with `git mv`. | fonts-conf(5), css-fonts-4, Skia, FreeType | `libs/text_host/src/{fontconfig,db,raster,lib}.rs` |
| Aether changed **by import paths only**: `fonts/mod.rs` re-exports `text_host::{db, fontconfig, raster, Face, Metrics, load_face, load_face_synth, next_face_id, line_metrics}` at the old paths, and the moved bodies were deleted. No Aether logic changed. AETHERJS's js lane is untouched. | — | `handlers/aether/src/fonts/mod.rs`, `Cargo.toml` |
| `text_host::line`: one line in a family list. Installed faces, then fontconfig `sans`, then per-character platform fallback. Shaped by `font_core::shape_fallback` (UAX #9, GSUB/GPOS, kerning, ligatures), placed, painted source-over into a premultiplied RGBA/BGRA buffer. Caret x and hit-testing over UAX #29 grapheme boundaries (ligatures split by grapheme). | UAX #9, #29, OpenType Layout | `libs/text_host/src/line.rs` |
| `quartzite::text`: rewritten on `text_host`. Covers the desktop UI font from a Pango description (`gtk-font-name` at `gtk-xft-dpi`: family list, style words, pt/px size), word wrap (spaces, then graphemes for an over-long word), and `draw_text` / `measure_text_height` into 32-bit buffers. `ab_glyph` and the embedded Hack font are gone. | Pango `FontDescription` syntax | `libs/quartzite/src/text.rs` |
| GTK face: **`GlyphLabel`**, a Widget subclass with CSS node `label`. It is the Tetra `Button`'s child, and GTK still draws the frame (`text-button` class kept, so the geometry is identical). | GTK 4 | `platforms/gtk/glyph.rs`, `button.rs` |
| GTK face: **`GlyphEntry`**, a Widget subclass with CSS node `entry`, so the theme's frame, background and focus ring apply. It is the Tetra `TextField`. It provides: an `IMMulticontext` (every committed string, so IMEs work); grapheme caret moves; click/shift-click/drag selection; Home/End; Backspace/Delete; Ctrl+A/C/X/V through the display clipboard; horizontal scrolling that keeps the caret visible; a placeholder at half the CSS colour; and `activate` on Enter. Ink is the widget's CSS `color`, and text is rendered at the surface scale factor. | GTK 4 | `platforms/gtk/glyph.rs`, `text_field.rs` |

**Lock:** `ab_glyph`, `ab_glyph_rasterizer`, `owned_ttf_parser` and `ttf-parser` left `Cargo.lock`. `cargo tree
-p aether-shell -e normal` (with and without `--features gtk`) shows none of them, and no FreeType or fontconfig
library. The one `fontconfig-parser` 0.5.8 left in the tree is resvg/usvg's (PIXELCORE's chicken wire, behind
Aether's `svg` feature), as AETHERFONT recorded.

## Oracle

**`tools/eyes/suites/xvfb-smoke/quartzfont.py`** (python3 stdlib, plus Chromium through `tools/eyes/ref.mjs`)
runs these steps:

1. It runs the real vessel (`aether-shell --features gtk`) on a private 640×400 Xvfb screen, and grabs the
   framebuffer from `-fbdir` exactly as the EYES `xvfb` subject does.
2. With `QUARTZFONT_TRACE=1`, every painted string reports its face file, size, pen origin in window px, ink,
   ascent/descent and per-glyph pens.
3. It builds a Chromium page with, for each string:
   - the frame's own background around it;
   - the same string set in `@font-face { src: url(file://<that face>) }` at the same size, colour and pen
     origin. A web font is unhinted in Chromium, the like-for-like path from AETHERFONT.
4. It screenshots that page with the EYES flags.
5. It scores each inked glyph box by its mean |luma difference|, after the best ±2 px registration per string.

**Result: 15/15 glyphs within 8 levels (100 %).** Every string registered at shift (0, 0). The worst glyph
mean |d| is 2.68, on `C`.

| string | face | size | pen (x, baseline) | glyphs ≤ 8 | worst mean \|d\| |
|---|---|---|---|---|---|
| `<` | NotoSans-Regular | 13.333 px | (21, 22) | 1/1 | 1.38 |
| `>` | NotoSans-Regular | 13.333 px | (71, 22) | 1/1 | 1.37 |
| `C` | NotoSans-Regular | 13.333 px | (120, 22) | 1/1 | 2.68 |
| `Enter URL...` (placeholder, 50 % ink) | NotoSans-Regular | 13.333 px | (159, 22) | 12/12 | 1.58 |

Frames: [`quartzfont-chrome-subject.png`](quartzfont-chrome-subject.png) (the vessel) and
[`quartzfont-chrome-ref.png`](quartzfont-chrome-ref.png) (Chromium). Scores:
[`quartzfont-score.json`](quartzfont-score.json).

**Golden re-pinned:** `tools/eyes/suites/xvfb-smoke/golden/aether-shell.png` is now the font_core-drawn
frame. Before the re-pin, the Pango golden scored ssim 0.996 / mismatch 0.2 % against it. The button and entry
geometry is unchanged (50/50/50 px buttons, entry at x = 150). The glyphs are now Noto Sans, which is this
host's fontconfig `sans` (`fc-match sans`), where the old Pango golden showed a DejaVu-like face. The whole
suite gate is **GREEN**: aether-shell 1.000, chromium-window 0.986, png-vs-golden 1.000.

The raster path is the same code Aether's text oracle gates. Its numbers are unchanged after the lift:
**5487/5504 web-font glyphs within 8 (99.7 %)** and **400/400 widths within 0.5 px** (worst 0.0234 px).

## Known-answer tests

- **`cargo test --release -p text_host`: 11 tests.**
  - 8 moved with the code: `match_style_kat`, `style_of_dejavu`, `resolution_and_fallback_vs_fontconfig`
    (`fc-match -s` oracle), `xml_and_alias_kat`, `preblend_matches_fontcore_dark_table`,
    `quarter_pixel_split`, `embolden_and_skew_kat`, `synthetic_bold_inks_more`.
  - 3 new for `line`:
    - `shaped_widths_and_kerning`: DejaVu `A` = 1401/2048 em, `AV` kerned, 13 px metrics 12/3/0;
    - `caret_and_hit`: boundaries, hit-testing, Hebrew fallback glyphs;
    - `paint_inks_premultiplied`.
- **`cargo test --release -p quartzite`: 12 tests**, including `pango_font_names_kat` (6 descriptions) and
  `wrap_and_draw`.
- **`cargo test --release -p aether-shell`**: green (0 tests in the crate).
- **`cargo test --release -p aether`**: green. That is 143 lib tests (151 at AETHERFONT minus the 8 that moved),
  plus `dom_oracle`, `style_oracle`, `style_time` and `text_oracle` (2).
- **`cargo check -p quartzite --features gtk`**: clean (GTK 4.14.5, libadwaita 1.5, gtksourceview 5.12 and
  libspelling present). `cargo build --release -p aether-shell --features gtk` builds.

## Third-party crates

None added. `text_host` depends only on `font_core`. The GTK widgets use the gtk4/glib 0.11/0.22 bindings
quartzite already had: the platform toolkit, not text. Chromium is a test oracle only.

## Honest ceiling

- **Only aether-shell's chrome is quartzite's own text.** That chrome is the Tetra `Button` and `TextField`
  on GTK. The rest of the GTK face still sets its text through Pango, and the macOS face (AppKit/CoreText,
  untestable here) and the Qt face are untouched. That rest of the GTK face is:
  - lumen's workspace: sidebar, comms, chat manager, console view;
  - `ScrollableText` (a GtkTextView);
  - the GNOME mega bar.

  The row's "tab strip, status, menus, dialogs" do not exist in aether-shell's chrome today, so there was
  nothing of them to move.
- **No hinting.** System faces are rendered unhinted, so Chromium's `hintslight` rendering of an installed
  family differs (FONTHINT, SR62, owed). The oracle uses the face as a web font, like AETHERFONT's gate.
- `GlyphEntry` is a one-line editor. It has no undo, no word-wise Ctrl+arrows, no double-click word select,
  no context menu, no primary-selection paste, no preedit (IME composition) rendering (commits work), and no
  accessibility text interface beyond the generic widget role. Caret geometry in right-to-left runs uses the
  visual-order heuristic in `Line::rtl_at`.
- The live-frame oracle has 15 glyphs, which is all the chrome paints at rest. Typed URLs and the focused
  caret are not exercised under Xvfb, because there is no input injector in the container.
- The URL field still does not follow navigation (`BrowserUrlChanged`). That is a parity choice: the GTK
  `Entry` did not follow it either.

## Owed

1. Move the rest of the GTK face onto `GlyphLabel` and a multi-line `GlyphText`: lumen's workspace widgets,
   the console, and `ScrollableText`.
2. macOS face: `text_field`, `window_title` and `meter` through `text_host` (needs a Mac to prove).
3. FONTHINT (SR62). The hinter lands in `text_host::raster::rasterize`, so Aether and quartzite gain it
   together.
4. Preedit rendering and accessible text for `GlyphEntry`. An Xvfb input injector (XTEST from the harness)
   so the oracle can type.

## How a future executor continues

Run these:

- `cargo test --release -p text_host -p quartzite -p aether`.
- `cargo build --release -p aether-shell --features gtk && python3 tools/eyes/suites/xvfb-smoke/quartzfont.py`
  (writes `tools/eyes/out/quartzfont/{subject,ref}.png`, `score.json`; exit 1 below 90 %).
- `tools/eyes/run.sh xvfb-smoke`.

The seams are:

- `text_host::line::TextStyle::shape` → `Line::{paint, caret_x, hit}` for any toolkit text.
- `quartzite::platforms::gtk::glyph::{GlyphLabel, GlyphEntry, style_for}` for GTK widgets.
