# FACET — The Canvas lands on the host (ledger SR29)

Branch `exec-host-facet`, cut from `676ca0e9` (descendant of `0686cc1b`). Charter: `docs/CODEX.md` §2,
*Facet · Images · Photoshop, Preview · The Canvas. Raster/Vector engine.*

## Finding

SR29: Facet was chartered and absent on the host. The tree had the kernel viewer
(`unaos/crates/kernel/src/video/facet.rs`, a PNG window) and a macOS-only viewer vessel
`vessels/facet` that decoded through `libs/lux` (itself the `png` and `zune-jpeg` crates — chicken
wire) and drew with its own zoom/pan. No domain owner, no bus surface, no edits, no export, and
every surface that met an image decoded it (or did not) on its own.

## What landed

| Milestone | What | Where |
|---|---|---|
| M1 | `handlers/facet` (package `facet`): open → decode through `ImageSource` → Facet's own metadata reader (size, depth, alpha, declared colour space, **EXIF orientation, applied**) → non-destructive edit list (crop, rotate, flip, resize, brightness/contrast; whole-state undo/redo/reset) → view model (zoom fit/actual/percent, pan, view rotate/flip) → PNG export by Facet's own writer. README in the house shape. | `handlers/facet/` |
| M2 | Bandy surface `SMessage::Facet(FacetCommand)`: `ImageOpen/ImageOpened`, `ImageInfo/ImageInfoIs`, `ImageEdit`, `ImageRender/ImageRendered`, `ImageExport/ImageExported`, `ImageClose`, `ImageError` (every request receipted, every failure answered), wire shapes frozen by KATs. The association: `FacetCommand::image_mime_for` (host twin of the kernel `image/png → facet` row) and `FacetCommand::local_image_path` (navigations). **Matrix** delegates a Finder `Open` of an image; **Aether**'s shell delegates a `file://` image navigation (Facet opens, renders at the viewport, the shell blits); **Quarry** already opens `image/png` with the `facet` opener on the metal (kernel untouched). | `libs/bandy/src/signals.rs`, `handlers/matrix`, `vessels/aether-shell` |
| M3 | `vessels/facet-view` (the old `vessels/facet`, renamed so the handler owns the name): a CLIENT of Facet — the `Viewer` controller turns window input into Facet requests and Facet's frames into `SurfaceBlit`s; on macOS the window is quartzite's image surface; everywhere a headless witness drives the same controller over the same bus from a key script. Shortcuts for zoom/pan/view-rotate/flip and every edit op. | `vessels/facet-view/` |
| M4 | The eyes: `facet render <file> --out shot.png [--zoom --rotate --flip-h --pan ...]` (+ `info`, `export`, `diff`), and `tools/eyes/suites/facet/` (TOML cases + golden PNGs + the Chromium pages that proved them); `tools/eyes/run.sh` is a runner STUB in the AETHERSEE harness's documented shape (`run.sh <suite> [--accept]`, cmd subjects vs golden oracles), to be replaced when `tools/eyes` lands. | `handlers/facet/src/main.rs`, `tools/eyes/` |

## The decoder seam — `ImageSource` (PIXELCORE's shape)

```rust
pub enum Format { Png, Jpeg, Gif, Bmp, Qoi, WebP }
pub struct Frame { pub delay_ms: u32, pub rgba: Vec<u8> }
pub struct Decoded {
    pub width: u32, pub height: u32,
    pub rgba: Vec<u8>,                 // straight RGBA, row-major, top-down
    pub frames: Option<Vec<Frame>>,    // composited animation canvases
    pub loop_count: Option<u16>,
    pub orientation: u8,               // EXIF 1..=8, reported not applied
}
pub enum SourceError { UnknownFormat, Decode(String), TooLarge }
pub trait ImageSource: Send + Sync {
    fn name(&self) -> &'static str;
    fn sniff(&self, bytes: &[u8]) -> Option<Format> { sniff(bytes) }
    fn decode(&self, bytes: &[u8]) -> Result<Decoded, SourceError>;
}
```

Field for field PIXELCORE's `pixel_core::{Format, Frame, Image}` (read from
`/home/user/exec/media-pixel`, `gneiss_pal::dsp::image`, commit `a8a264ff`); `sniff` is byte for byte
the same. The replacement is one `impl ImageSource for PixelCoreSource` and deleting the feature.

## Spec sections Facet implements itself (no crate)

- EXIF 2.3 §4.6.4 A (Orientation, tag 0x0112) in a TIFF 6.0 IFD0, both byte orders, from JPEG APP1
  (§4.5.4), PNG `eXIf`, WebP `EXIF`; the 8-way display mapping (identical to PIXELCORE's and CSS
  `image-orientation: from-image`).
- PNG 3rd ed. §11 (IHDR, tRNS, sRGB, iCCP, cICP, gAMA, eXIf, acTL); T.81 Annex B marker walk (SOFn);
  ICC.1 B.4 (APP2 ICC_PROFILE chunks) and §9.2.41 `desc` (v2 `desc`, v4 `mluc`); GIF89a §18/§23/§26 +
  NETSCAPE2.0; BMP V4/V5 `CSType`; QOI header; RFC 9649 §2.5–2.7 + RFC 6386 §9.1 headers.
- The PNG writer: PNG §5, §9 filters with the §12.8 heuristic, RFC 1951 fixed-Huffman DEFLATE over a
  greedy LZ77 hash chain, RFC 1950 zlib + Adler-32, PNG Annex D CRC-32.
- CSS Filter Effects 1 `brightness()` / `contrast()`.
- The triangle resize filter (Pillow `BILINEAR` definition; documented in `Raster::resized`).

## Oracle method

1. **Chromium** (pre-installed, Playwright): `tools/eyes/suites/facet/pages/<case>.html` shows the
   fixture with the CSS that means the same thing (`image-rendering: pixelated` for nearest zoom,
   `transform: rotate()/scaleX(-1)`, an overflow box for crop, `filter: brightness() contrast()`, the
   default `image-orientation: from-image` for EXIF), screenshotted by `oracle.mjs` at its viewport;
   `facet diff` scores Facet's render against it. The goldens were checked in only after that.
2. **Independent implementations** (`handlers/facet/tests/oracles.rs`): the PNG writer round-tripped
   bit-exact through another decoder (the `image` crate's) at 5 sizes up to 300x200 and 1024x3; the
   triangle filter against the `image` crate's `FilterType::Triangle` at 5 target sizes
   (PSNR > 45 dB, max delta ≤ 3).

### Numbers (Facet golden vs Chromium screenshot)

| case | what | max delta | PSNR |
|---|---|---|---|
| card-actual | 100 %, half-alpha band over the field | 0 | ∞ (pixel-exact) |
| card-zoom400 | 400 % nearest | 0 | ∞ |
| card-rotate90 | view turn + 200 % | 0 | ∞ |
| card-fliph | view flip + 200 % | 0 | ∞ |
| card-crop | edit crop + 200 % | 0 | ∞ |
| card-adjust | edit `brightness(1.3) contrast(0.7)` | 0 | ∞ |
| card-fit | fit in 100x100, centred, not enlarged | 0 | ∞ |
| card-pan | 400 % + clamped pan in 100x80 | 0 | ∞ |
| photo-o6 | EXIF 6 applied (JPEG) | 2 | 70.33 dB |
| photo-o5 | EXIF 5 applied (JPEG) | 2 | 70.33 dB |
| card-resize | edit resize 48x32 → 30x20 | 35 | 30.03 dB — Chromium smooth-scales with another filter; recorded, not gated |

9 of 11 pixel-exact. The JPEG delta is decoder IDCT/upsampling (the chicken-wire decoder vs
libjpeg-turbo); the orientation itself is exact (a wrong orientation is a gross failure, not 2 levels).
Getting the alpha band exact needed Skia's rounding: each source-over term rounded on its own
(`round(c·a/255) + round(f·(255−a)/255)`), measured and adopted (view.rs rule 5).

## KATs

`cargo test -p facet`: 18 unit KATs (all 8 orientations on a 3x2 index picture; turns/flips/crop; the
resize filter's downscale/upscale/identity/premultiplied cases; CSS adjust incl. clamping; Fit/200 %/
pan-clamp/turn-flip order/alpha composite; CRC-32 "123456789", Adler-32 "Wikipedia", the empty zlib
stream, Paeth, PNG layout; EXIF both byte orders + prefix + garbage; truncation-totality of all six
metadata readers; ICC v2 `desc`; the edit list's undo/redo/reset and op checks) + 5 oracle/integration
tests (the 11 eyes goldens included) + 1 bus round trip. Bandy: 15 Facet wire KATs + completeness
guards + the association. Matrix: the Finder→Facet delegation over a live Synapse. facet-view: the
controller over a live Facet (see its tests).

## Honest ceiling

- **Decoding is chicken wire**: the `image` crate decodes every pixel until PIXELCORE lands
  (`PixelCoreSource`). Facet's own code decides orientation, metadata, edits, views and export.
- APNG / animated WebP: only the first frame is decoded (GIF animations decode fully; Facet renders
  frame 0 and reports the count).
- Colour: declared spaces are REPORTED, not converted (no ICC transform; everything renders as sRGB).
- Export: PNG only; JPEG export answers `ImageError` (no UnaOS JPEG encoder yet).
- Vector (the charter's "Raster/**Vector**") and GPU texture editing: not started.
- facet-view's window is macOS-only (quartzite's AppKit image surface); this container cannot build
  it, so the window path is code-reviewed, not run here — the headless witness runs the same
  controller and bus. The old FACET-GPU textured-quad presentation is not carried into facet-view
  (it presented a static frame with view-side zoom; Facet now renders every view) — a GPU blit for
  `SurfaceBlit` is owed. Drag-pan is not delivered by the surface (arrows pan).
- Aether: only `file://` images delegate; a remote image URL stays the page's business. A page
  animation tick can repaint over a delegated image.
- Quarry/kernel untouched: the metal twin opens PNG only until the kernel links PIXELCORE.

## Chicken-wire table (third-party crates this arc leans on)

| crate | version | role | class |
|---|---|---|---|
| `image` | 0.25.10 (latest stable at add) | decodes PNG/JPEG/GIF/BMP/QOI/WebP behind `ImageSource` | **chicken wire** (until PIXELCORE) |
| `tokio` | 1.53 (tree's pin) | the bus loop | utility |
| `log` | 0.4 | logging | utility |
| `tempfile` | 3.27.0 (dev, latest) | test dirs | utility |
| `toml` | 1.1.6 (dev, latest) | tests read the eyes suite | utility |

The tests also use the `image` crate as an independent ORACLE (decoder, triangle filter) and the
fixture generator uses its JPEG ENCODER; neither is a Facet code path.

## Owed

1. `PixelCoreSource` once PIXELCORE (SR25) is on trunk; drop `chicken-wire-image`.
2. JPEG export (an UnaOS JPEG encoder — PIXELCORE's family).
3. ICC transforms (declared → sRGB) — a colour core.
4. facet-view: run the window on macOS (eye-witness), GPU blit, drag-pan in surface mode.
5. Animated stills: frame stepping in the view (APNG/WebP frames from PIXELCORE).
6. The CODEX §2 Facet entry gains "host: handlers/facet" at the fold (the seat's job).
7. Replace `tools/eyes/run.sh` with the AETHERSEE harness and add the Chromium-oracle variants of
   these cases there (`kind = "chromium"`, `url = file://{page}`).

## How to continue

`cargo test --release -p facet -p facet-view`; `tools/eyes/run.sh facet`; to re-prove goldens
against Chromium: `node tools/eyes/suites/facet/oracle.mjs <dir>` then `facet diff <golden>
<dir>/<case>.chromium.png`. Fixtures regenerate deterministically:
`cargo run --release -p facet --example make_fixtures`.
