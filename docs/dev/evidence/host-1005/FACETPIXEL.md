# FACETPIXEL — Facet decodes through pixel_core (LEDGER SR43)

Branch `exec-host-facetpixel`, cut at `bdd6c550`, `exec-host-merge1` merged first (`32b4487b`).
Builds on FACET (SR29, `docs/dev/evidence/host-1004/FACET.md`) and PIXELCORE (SR25,
`docs/dev/evidence/host-1004/PIXELCORE.md`).

## Finding

Facet (The Canvas) decoded its pixels through the `image` crate behind a default
`chicken-wire-image` feature, while PIXELCORE already decoded PNG / JPEG / GIF / BMP / QOI /
lossless WebP from the specifications with zero dependencies, pixel-exact against Chromium. Both
were on `exec-host-merge1`; FACET had written its `ImageSource` seam to PIXELCORE's shapes, so the
seam closed in one impl. A second finding on the way: `tools/eyes` (the real harness) had landed on
merge1 and reads `suites/<name>/cases/*.toml`, but the facet suite still carried the FACET-era stub
shape (`[[case]]` in `suite.toml`), so `tools/eyes/run.sh facet` refused it — it was ported (M3a).

## Milestones

| M | What | Commit |
|---|---|---|
| M3a | Facet eyes suite on the real harness: `cases/facet.toml` (golden oracles, unchanged), `cases/chromium.toml` (the same 11 subjects against Chromium live, `<case>@chromium`), `baseline.json` accepted from the BEFORE state | `a91d4eed` |
| M1+M2 | `PixelCoreSource` is `default_source()`; refusals name the format; `chicken-wire-image`, `ImageCrateSource`, `NoSource` removed; `pixel_core` a path dependency; `image` a dev-dependency (test oracle) only | `34b2e183` |
| M3 | eyes rerun; photo goldens re-proven (identical to Chromium), `max_delta` 2 → 0 | `f736f482` |
| M4 | this doc, the Facet README | (this commit) |

## The seam

`handlers/facet/src/source.rs`:

- `PixelCoreSource` — `pixel_core::decode` → `Decoded` (field for field: width, height, rgba,
  frames, loop_count, orientation). Facet's `sniff` IS `pixel_core::sniff` (mapped to Facet's
  `Format`, which keeps its `name()`), so the two can never disagree on what a file is.
- Linked by path, not through `gneiss_pal::dsp::image` (the same crate re-exported): gneiss_pal's
  default `std` feature pulls reqwest/octocrab, which Facet has no use for.
- **Refusals, never a fallback.** `SourceError::Decode { format, reason }` for a container
  pixel_core recognises but does not decode (`webp: unsupported: webp lossy (VP8)` until VP8CORE,
  SR40; malformed/truncated streams), `SourceError::TooLarge { format }`, and
  `SourceError::Foreign(name)` for a well-known container no UnaOS decoder recognises yet —
  `source::foreign_format` names tiff, bigtiff, ico, cur, avif, heif (ISO BMFF `ftyp` brands,
  ISO/IEC 14496-12 §4.3), jpeg xl (codestream and container), jpeg 2000, psd, pnm, svg.
  `Facet::open_bytes` uses the same naming for bytes `sniff` refuses, so the bus `ImageError`
  reads e.g. `photo.tif: tiff: no UnaOS decoder reads tiff yet (refused, no fallback)`.
- **Orientation.** Facet's own from-spec reader (`meta.rs`) stays the authority it applies (it also
  reads PNG `eXIf` and WebP `EXIF`, which pixel_core does not); `Decoded::orientation` is what the
  decoder reported. For JPEG the two independent readers are asserted equal.

## Oracle method and numbers

1. **Chromium** (pre-installed, Playwright): each `pages/<case>.html` shows the fixture with the
   same view/edit in CSS. `facet diff <facet render output> <Chromium screenshot>` gives the exact
   max channel delta / PSNR / differing pixels. Shot two ways — the eyes harness's `ref.mjs`
   (the `@chromium` cases) and the suite's own `oracle.mjs` (`--force-color-profile=srgb`) — and
   both agree to the pixel.
2. **EYES gate** (`tools/eyes/run.sh facet`, SSIM / >40-level mismatch): exit 0, `gate: GREEN`,
   all 22 cases `ok` against the before baseline.
3. **Second opinion** (`tests/oracles.rs`): the `image` crate decodes the same files.

Facet render vs Chromium, per case (max delta / PSNR / differing px):

| case | before (image crate) | after (pixel_core) |
|---|---|---|
| card-actual | 0 / inf / 0 | 0 / inf / 0 |
| card-zoom400 | 0 / inf / 0 | 0 / inf / 0 |
| card-rotate90 | 0 / inf / 0 | 0 / inf / 0 |
| card-fliph | 0 / inf / 0 | 0 / inf / 0 |
| **photo-o6** (EXIF 6, JPEG) | 2 / 70.33 dB / 52 | **0 / inf / 0** |
| **photo-o5** (EXIF 5, JPEG) | 2 / 70.33 dB / 52 | **0 / inf / 0** |
| card-crop | 0 / inf / 0 | 0 / inf / 0 |
| card-adjust | 0 / inf / 0 | 0 / inf / 0 |
| card-resize | 35 / 30.03 dB / 285 | 35 / 30.03 dB / 285 (Chromium smooth-scales with another filter; recorded, not gated) |
| card-fit | 0 / inf / 0 | 0 / inf / 0 |
| card-pan | 0 / inf / 0 | 0 / inf / 0 |

9 of 11 exact before → 10 of 11 exact after; the remaining case is a resampling-filter difference,
not a decoder one. EYES SSIM: every case 1.000 before and after except `card-resize@chromium`
0.995 → 0.995; mismatch 0.0 % everywhere.

## KATs / tests

`cargo test --release -p facet -p facet-view`: facet lib 22 (4 new in `source.rs`: 17 foreign /
unknown magic vectors named, lossy WebP refused by name, per-format decode errors, oversized QOI
refused before allocating), bus 1, oracles 8 (3 new: pixel_core vs the image crate — card.png
bit-exact, the EXIF JPEGs ≤ 3 levels / > 50 dB; the EXIF agreement test — Facet's reader vs
pixel_core's on both fixtures and orientations 0..=10 in both TIFF byte orders, out-of-range → 1
for both, pixels unchanged by the Exif segment; refusals through `Facet::open_bytes`), facet-view 3.
All green. `bus.rs` and `oracles.rs` no longer hide behind a feature.

## Dependencies (R83)

`cargo tree -p facet -e normal`: `bandy`, `log`, `pixel_core`, `tokio` — **no `image`**.

| crate | version | role | verdict |
|---|---|---|---|
| `pixel_core` | in-tree | the decoders | UnaOS's own |
| `image` | 0.25.10 (latest) | dev-dependency: the second-opinion oracle in tests | test oracle, not linked |
| `toml` | 1.1.6 (latest) | dev: tests read the eyes cases | utility |
| `tempfile` | 3.27.0 (latest) | dev | utility |
| `tokio`, `log` | 1.53 / 0.4 | runtime, logging | utility |

## Honest ceiling

Facet decodes exactly what pixel_core decodes: PNG (all colour types/depths, interlace), JPEG
(baseline + progressive Huffman, 8-bit), GIF (animated, composited), BMP (BITMAPINFOHEADER family),
QOI, WebP lossless (VP8L). Refused by name: lossy and animated WebP, arithmetic-coded / 12-bit /
lossless JPEG, TIFF, ICO/CUR, AVIF/HEIF, JPEG XL, JPEG 2000, PSD, PNM, SVG.

## Owed

1. Lossy WebP — VP8CORE (SR40); animated WebP (ANIM/ANMF) after it.
2. pixel_core reading PNG `eXIf` and WebP `EXIF` orientation (Facet's reader covers them today; the
   agreement test can then widen beyond JPEG).
3. ICO / TIFF / AVIF decoders (AVIF rides AVCODEC's av1_core intra path).
4. The Aether `image-fallback` feature is its own owner's arc (untouched here).

## Continue

`cargo test --release -p facet -p facet-view`; `tools/eyes/run.sh facet` (gate vs `baseline.json`;
`--accept` to rewrite). Exact per-case numbers: `target/release/facet diff
tools/eyes/out/facet/<case>.subject.png tools/eyes/out/facet/<case>.ref.png`. A new decoder lands in
pixel_core and shows up in Facet with no Facet change; a new foreign name goes in
`source::foreign_format` with a vector in its test.
