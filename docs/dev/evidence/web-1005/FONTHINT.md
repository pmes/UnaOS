# FONTHINT — font_core hints glyphs the way Chromium on Linux does (LEDGER SR62)

Branch `exec-text-fonthint`, cut at `51b720ef`, joined with `exec-rmbp-merge14` (`097aee9a`).
Crate: `unaos/libs/text/font_core` (`no_std` + `alloc`, `forbid(unsafe_code)`, still zero dependencies).
Module: `src/hint/` (about 5,200 lines). Aether picks it up in `handlers/aether/src/fonts/`. The kernel keeps
`Hinting::None` for now; see the KERNELFONT2 section.

## Finding: under hintslight, Chromium uses the auto-hinter, not the TrueType interpreter

The row assumed that `hintslight` means the TrueType bytecode interpreter in light mode. **It does not.** This
host's fontconfig sets `hintstyle 1` (hintslight), `hinting true` and `autohint false`. Skia maps
`kSlight_FontHinting` to `FT_LOAD_TARGET_LIGHT`. FreeType 2.13's TrueType driver does not set
`FT_DRIVER_HINTS_LIGHTLY`, so `ft_glyphslot_...`/`FT_Load_Glyph` sends every **TrueType** face loaded with
`TARGET_LIGHT` to the **auto-hinter** (`af_loader_load_glyph`, light mode), whatever `autohint` says.
**CFF** faces go to the Adobe engine (`cf2`), because the CFF driver does hint lightly. Measured with
freetype-py on this host: `LIGHT` outlines are byte-identical to `LIGHT | FORCE_AUTOHINT` and differ from
`LIGHT | NO_AUTOHINT` (v40 interpreter). Chromium's screenshots match the auto-hinted bitmaps (mean |Δ| 2.2
levels) much better than the interpreter's (4.8). So, per the row's clause (2), the light auto-hinter IS
written, and the bytecode interpreter is **not**. The interpreter is owed: it is not on Chromium's path for any
face in this corpus under this configuration.

Three more Chromium render-parameter facts came out of matching the oracle page, each confirmed with CDP
`CSS.getPlatformFontsForNode` and screenshots:

* **fontconfig decides hinting per face.** `44-wqy-zenhei.conf` gives WenQuanYi Zen Hei `hintstyle hintnone`.
* **Platform-fallback faces are drawn unhinted.** Loma reached as the Thai fallback of `sans-serif` is
  unhinted. Loma named directly in `font-family` is hinted (hintslight). The same holds for WenQuanYi as the
  CJK fallback.
* **Synthetic oblique turns hinting off.** Skia drops hinting for skewed text.

## What is implemented (FreeType 2.13.2 source as the reference)

| Area | FreeType source | Module |
|---|---|---|
| Fixed-point helpers: `FT_MulFix`, `FT_MulDiv`, `FT_MulDiv_No_Round`, `FT_DivFix`, `FT_Hypot` approximation, `ft_corner_is_flat`, `FT_MSB` | `ftcalc.c` | `hint/fixed.rs` |
| Unscaled outlines with phantom points: glyf simple and composite (MulFix transforms, point matching, USE_MY_METRICS pp1); CFF charstrings → 26.6 | `ttgload.c` (NO_SCALE) | `glyf.rs::raw_outline`, `hint/cff_hint.rs` |
| Auto-hinter style selection: script coverage from Unicode ranges, then HarfBuzz GSUB closure, OpenType-feature styles (`c2sc`, `smcp`, `pcap`, `sups`, …) from feature lookups, fallback style `hani_dflt` (AF_CONFIG_OPTION_CJK) | `afglobal.c`, `afshaper.c`, `afranges.c` | `hint/autofit.rs`, `hint/coverage.rs`, `hint/autofit_tables.rs` (generated) |
| Latin writing system, light mode: standard widths, blue zones from `afblue.dat` (incl. LONG, overshoots, `sort_and_quantize`), x-height scale fitting, segments, links, edges, blue edges, stem hinting (light: widths unadjusted), strong, weak and IUP-like point alignment; vertical dimension only | `aflatin.c`, `afhints.c` | `hint/autofit.rs` |
| CJK writing system, light mode: both dimensions, CJK blues (fill/flat, delta clamp), `hint_normal_stem` light limits, serif and anchor logic | `afcjk.c` | `hint/autofit.rs` (`cjk_*`) |
| Adobe CFF engine as FreeType applies it (darkening off): blues init and capture (`blueScale`/`blueShift`/`blueFuzz`, family blues), hint init (ghost hints −20/−21), hint map build (capture pass per map, then the rest; initial map plus synthetic zero edge), two-pass `adjustHints`, hintmask/cntrmask (`setAll` mutating the live mask), delayed hint replacement, flex, subrs, builder `>>10` to 26.6 | `cf2blues.c`, `cf2hints.c`, `cf2intrp.c`, `cf2ft.c` | `hint/cff_hint.rs`, `cff.rs` (`HintPrivate`, `hint_source`) |
| `Hinting::{None, Slight}` on `rasterize_glyph_hinted` and in the glyph-cache key (`cache.rs`, `ui.rs` Engine) | — | `raster.rs`, `cache.rs`, `ui.rs` |
| Aether: installed faces hinted unless fontconfig says `hintnone`/`hinting=false` (fontconfig `<match target="font">` rules on family + pixelsize, and the pattern-level `hintstyle` default); fallback faces and synthetic oblique stay unhinted; a hinted outline is drawn with Skia AAA plus the A8 pre-blend | — | `handlers/aether/src/fonts/{raster,mod,fontconfig}.rs` |

FreeType quirks reproduced because the oracle demands them: `sort_and_quantize` uses `sum / j`. In
`af_glyph_hints_align_weak_points`, `first_touched > points` (a pointer comparison) is kept. The CJK blue
round-flag recompute loop is dead code in FreeType, so it is not reproduced. The CJK blue `delta2` takes the
`#if 0` path, which gives 0 below half a pixel and `pix_round` otherwise.

## Oracles and method

* **FreeType itself, per glyph:** `pip install freetype-py` (2.5.1, which bundles FreeType 2.13.2 built
  with HarfBuzz). `oracle/ft_hint_oracle.py` loads with `FT_LOAD_TARGET_LIGHT` and streams 26.6 points and
  gray bitmaps. `tests/hint_oracle.rs` compares font_core's hinted outline point for point, then compares the
  bitmaps. It SKIPs when freetype-py is absent. Options: `FONTHINT_FULL=1` (whole cmap),
  `FONTHINT_FAILS`, `FONTHINT_DUMP=face:gid:size` (segments, edges, CFF hint maps, points).
* **Frozen KATs:** `tests/hint_kat.rs` holds 199 FreeType outlines from `tests/data/hint_kat.tsv`
  (`oracle/gen_hint_kat.py`, 67 KB). Each face is keyed by the sha256 of its file. They cover latin, the
  feature styles, CJK, the hani fallback and CFF. A second test checks the cache key and the vertical-only
  snap. Unit KATs cover `fixed.rs`.
* **Chromium:** AETHERFONT's text oracle (`handlers/aether/tests/text_oracle.rs`, headless Chromium 1194,
  50 strings × 4 sizes), on both the system-font page and the web-font page. Then EYES.

## Results

FreeType per-glyph oracle, default corpus: 16 faces (DejaVu Sans/Bold/Serif/Serif-Bold/Mono, Liberation
Sans/Serif/Mono/Sans-Italic, FreeSans/Serif/Mono, WenQuanYi Zen Hei, Loma and Loma-Bold as CFF) × 12/16/24 px,
26,100 glyphs.

| measure | share |
|---|---:|
| hinted points identical to FreeType (26.6) | **99.98 %** (every face 100 % except DejaVu Sans Bold 99.8 %: 2 Arabic glyphs) |
| bitmap exact (font_core Exact raster vs FreeType gray raster) | 12.1 % |
| bitmap max |Δ| ≤ 8 | 71.7 % |
| bitmap mean |Δ| ≤ 8 | 100.0 % |

The outlines match. The bitmap columns measure rasterizers: FreeType's `gray` flattens and accumulates
differently from font_core's, and Chromium draws with Skia anyway, which font_core matches in `SkiaAaa` mode.
The CFF faces' low exact share comes from cubic flattening. Per face and size, this is printed by
`cargo test --release -p font_core --test hint_oracle -- --nocapture`. The Noto faces are not on this host;
only DejaVu, Liberation, Free*, WenQuanYi and Loma are.

AETHERFONT text oracle, share of glyphs within 8 levels of Chromium:

| page | 12 px | 16 px | 24 px | 48 px |
|---|---:|---:|---:|---:|
| system fonts, before (SR61) | 25.8 % | 23.5 % | 49.8 % | 65.3 % |
| **system fonts, FONTHINT** | **99.5 %** | **99.7 %** | **99.9 %** | 99.9 % |
| web fonts (unhinted, unchanged) | 99.4 % | 99.8 % | 99.7 % | 99.7 % |

Widths: 400/400 within 0.5 px (worst 0.023 px). **EYES aether: 0.991 → 0.993** (mismatch 0.4 %), gate
GREEN, no case worse by more than 0.001. 01-blog 0.994 → 0.999, 04-sticky 0.993 → 0.999, 08 0.995 → 0.998,
11-non-latin 0.990 → 0.993, 17 0.993 → 0.997, 22-fonts 0.992 → 0.997. 07-images-svg moved 0.940 → 0.939
(svg_core text, not this path).

## KERNELFONT2: the kernel's mode

`unaos/crates/kernel/src/video/text.rs` draws through `font_core::ui::Engine`. `Engine.hinting` now exists,
defaults to `Hinting::None` (KERNELFONT's B359 rendering unchanged) and is part of the engine's glyph key.
The kernel faces are DejaVu Sans/Mono/Serif (TrueType), so Linux's hintslight rendering of them is exactly
the light auto-hinter proven above (100 % point-identical for those faces). KERNELFONT2 sets
`engine.hinting = Hinting::Slight` and re-measures the desktop against a Chromium **system-font** page, not
the web-font page B359 used. That change is not made here because the row scopes this arc to font_core.

## Ceiling and owed

* **The TrueType bytecode interpreter (v40) is not written.** Under this host's hintslight it is not on
  Chromium's path for any face. It is owed for `hintfull`/`hintmedium` configurations (`Hinting::Full`).
* DejaVu Sans Bold Arabic: HarfBuzz shapes the Arabic blue characters with `isol`/joining forms; font_core
  uses plain cmap. That leaves 2 glyphs off.
* Feature styles take GSUB single substitutions only. GPOS y offsets and contextual/ligature lookups in style
  coverage are not modelled (no corpus face needs them).
* CFF: `seac` accented glyphs come back unhinted (falls back), CFF2/variable fonts, stem darkening (off in
  Chromium anyway), and a FontMatrix other than 1/upem are not handled.
* fontconfig: only `<match>` rules whose tests are `family`/`pixelsize` and whose edits are
  `hinting`/`hintstyle`/`autohint` are evaluated. `hintmedium`/`hintfull` hint as slight (no interpreter).
  `autohint=true` with `hintfull` (normal auto-hinting, both axes) is not modelled.
* The FreeType gray rasterizer is not reproduced (bitmap exact 12 %). Skia's AAA is the target and is matched.

## How to reproduce

```
pip install freetype-py                      # 2.5.1, bundles FreeType 2.13.2
cargo test --release -p font_core            # hint_kat (frozen), hint_oracle (live FreeType), unit KATs
cargo test --release -p font_core --test hint_oracle -- --nocapture   # per face/size table
FONTHINT_FULL=1 cargo test --release -p font_core --test hint_oracle -- --nocapture  # whole cmaps (slow)
AETHER_TEXT_ORACLE_ROWS=1 cargo test --release -p aether --test text_oracle -- --nocapture
tools/eyes/run.sh aether
```

No crate was added. freetype-py/FreeType is a test-time oracle only, and nothing links it.
`oracle/vectors.txt` records the FreeType source crate URL and sha256.
