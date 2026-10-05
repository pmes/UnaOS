# Facet

**The Canvas — the Images handler for UnaOS** (CODEX §2: *Facet · Images · Photoshop,
Preview · Raster/Vector engine*). Facet is the domain service that owns pictures on the host:
it opens and decodes an image file, reads what the file declares about itself, keeps a
non-destructive edit list per open picture, renders any view of it into a viewport, and
exports the baked result. Every other surface that meets an image — Matrix's Finder, Aether,
the `facet-view` vessel — goes through Facet's bus verbs instead of decoding on its own.

## What it does today

| Capability | How |
| --- | --- |
| Open + decode | PNG, JPEG, GIF (animated: every frame composited), BMP, QOI, WebP — through the `ImageSource` seam (below). Files over 512 MiB, and claimed sizes over 65 536 px a side or 2^26 px, are refused before allocating. |
| Metadata | Facet's own readers, from the specifications (`src/meta.rs`): stored size, bits per sample, alpha, the colour space the file declares (PNG `cICP`/`iCCP`/`sRGB`/`gAMA`, JPEG APP2 ICC profile description, WebP `ICCP`, BMP `CSType`, QOI colorspace), animation loop count, and the **EXIF orientation** (TIFF IFD0 tag 0x0112 in JPEG APP1, PNG `eXIf`, WebP `EXIF`). |
| Orientation | **Applied** on open (EXIF 2.3 §4.6.4 A; the same mapping as CSS `image-orientation: from-image`). `FacetImageInfo` carries both the stored and the displayed size. |
| Edit list | Non-destructive, in order: crop, rotate (quarter turns), flip, resize, brightness/contrast; whole-state undo / redo / reset (reset is undoable). The decoded original is never modified. |
| Resize filter | **Triangle**, documented in `Raster::resized`: separable tent `max(0, 1-|t|)` stretched by `max(1, src/dst)`, sample `i` centred at `(i+0.5)·src/dst`, edge weights renormalised, filtered alpha-premultiplied in f32, rounded once. (Pillow `BILINEAR`; the `image` crate's `Triangle`.) |
| Brightness/contrast | CSS Filter Effects 1 semantics, `brightness(b) contrast(c)` in that order, on straight sRGB values, each step clamped. Pixel-exact with Chromium's `filter:`. |
| View model | zoom (`Fit` = shrink-never-enlarge, `Actual`, `Percent(1..=6400)`), pan (clamped so no gap opens on an overflowing axis; a fitting axis stays centred), view rotate/flip — display only, never baked. Minification uses the triangle filter, magnification nearest-neighbour (`image-rendering: pixelated`); composited onto Moonstone `#2D2B55` with Skia's rounding. Rules in `src/view.rs`. |
| Export | PNG (8-bit RGBA, `sRGB`-tagged) by Facet's own writer (`src/png.rs`: filters with the §12.8 heuristic, DEFLATE fixed-Huffman over LZ77, zlib, CRC-32). Atomic (temp + rename); an existing file is refused unless `overwrite`. JPEG export is **owed** (answered with an `ImageError` naming the gap). |

## Bus verbs

All carried on `SMessage::Facet(FacetCommand)` (`libs/bandy/src/signals.rs`). Every request
carries a caller-chosen `receipt_id` that its answer echoes; every refused request is answered
by `ImageError` with the same receipt, never silence. Answers are inert as input.

| Direction | Message | Behaviour |
| --- | --- | --- |
| In | `ImageOpen { receipt_id, principal, path }` | Read, sniff, read metadata, decode, apply orientation. |
| Out | `ImageOpened { receipt_id, handle, info }` | The handle every later verb names, and `FacetImageInfo`. |
| In | `ImageInfo { receipt_id, handle }` | Ask for a handle's metadata. |
| Out | `ImageInfoIs { receipt_id, handle, info }` | The answer (also the answer to every `ImageEdit`). |
| In | `ImageEdit { receipt_id, handle, edit }` | `Crop`/`Rotate`/`Flip`/`Resize`/`Adjust`, or `Undo`/`Redo`/`Reset`. Each op is checked against the size the list has produced so far. |
| In | `ImageRender { receipt_id, handle, width, height, view }` | Render the edited picture through `FacetView` into a viewport (≤ 4096 a side). |
| Out | `ImageRendered { receipt_id, handle, width, height, rgba }` | Opaque 8-bit RGBA, row-major, top row first. |
| In | `ImageExport { receipt_id, handle, path, format, overwrite }` | Bake the list and write the file. |
| Out | `ImageExported { receipt_id, handle, path, bytes }` | What was written. |
| In | `ImageClose { handle }` | Release the handle (no answer). |
| Out | `ImageError { receipt_id, handle, message }` | A refused request, by receipt. |

### The "open image" association

`FacetCommand::image_mime_for(path)` is the host twin of the kernel type database's
`image/png → facet` row (`unaos/crates/kernel/src/fs/assoc.rs`): `png`, `jpg/jpeg/jpe/jfif`,
`gif`, `bmp/dib`, `qoi`, `webp`. A surface that meets such a path delegates:

- **Matrix** — a successful Finder `FsVerb::Open` of an image fires `ImageOpen` for the same
  principal with the absolute path (`matrix::finder::facet_delegation`; receipts tagged `'M'`
  in the top byte).
- **Aether** — a navigation to a local `file://` image is handed to Facet instead of being
  fetched as a page (`FacetCommand::open_for_url`).
- **Quarry** (the kernel's Finder) already opens `image/png` with its `facet` opener on the metal;
  the kernel viewer `video/facet.rs` is Facet's metal twin.

### Running it

`facet::ignite(synapse)` subscribes and serves the loop; `facet::serve(synapse, rx, facet)`
takes a receiver subscribed before the spawn so nothing in between is missed.
`Facet::dispatch(&FacetCommand)` is the pure seam the loop runs and the tests drive.

## The `facet` command

```
facet info   <file>
facet render <file> --out shot.png [--width W --height H] [--zoom fit|actual|N]
             [--rotate 0|90|180|270] [--flip-h] [--flip-v] [--pan X,Y] [EDITS]
facet export <file> --out edited.png [--overwrite] [EDITS]
facet diff   <a.png> <b.png> [--max-delta N] [--min-psnr DB]
EDITS: --crop X,Y,W,H  --turn N  --mirror h|v  --resize WxH  --adjust B,C
```

`render` is the EYES subject: `tools/eyes/run.sh facet` renders every case of
`tools/eyes/suites/facet/suite.toml` and scores it against the checked-in goldens.

## The decoder seam — `ImageSource`

`src/source.rs` defines the one trait Facet reads pixels through, written to the shape of
PIXELCORE's `gneiss_pal::dsp::image` (`unaos/libs/media/pixel_core`, SR25): `sniff`,
`decode → Decoded { width, height, rgba, frames, loop_count, orientation }`. Today the
`image` crate stands behind it — **chicken wire** (R83): a third-party crate doing decoding
UnaOS claims. It is confined to `source.rs`, behind the default `chicken-wire-image` feature,
and decodes pixels only; orientation and every other declaration come from Facet's own readers.
When PIXELCORE is on trunk, a `PixelCoreSource` replaces it and the feature goes.

## Proof

`cargo test -p facet`: known-answer tests for every orientation, turn, flip, crop, the resize
filter, the CSS adjust, the view rules, CRC-32/Adler-32/zlib/Paeth and the PNG layout; the PNG
writer round-tripped through an independent decoder; the triangle filter against the `image`
crate's; EXIF fixtures; the bus round trip; and the eyes goldens — themselves proven against
Chromium (`tools/eyes/suites/facet/oracle.mjs`): 9 of 11 cases pixel-exact, the two JPEG cases
within 2 levels (decoder IDCT), the resize case recorded at 30 dB (Chromium uses another filter).
Design doc and numbers: `docs/dev/evidence/host-1004/FACET.md`.

## Status

**Live on the host as a library + CLI, served over the bus; `vessels/facet-view` is its window.**
Owed: `PixelCoreSource` (retires the chicken wire), JPEG export, colour management (declared
spaces are reported, not converted), vector (SVG) — CODEX's "Vector" half of the charter — and
GPU texture editing.

## See also

- `docs/CODEX.md` §2 — the handler manifest (Facet: The Canvas).
- `libs/bandy/src/signals.rs` — `FacetCommand` and friends.
- `vessels/facet-view` — the window over these verbs.
- `unaos/crates/kernel/src/video/facet.rs` — the kernel viewer, Facet's metal twin.
