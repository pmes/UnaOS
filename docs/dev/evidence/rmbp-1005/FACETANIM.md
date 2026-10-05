# FACETANIM — the kernel viewer plays animations, two frames at a time (rmbp-ledger B358)

Branch `exec-rmbp-facetanim`, cut at 78a8f6d6, `exec-rmbp-merge13` merged first (clean, no conflict).

## Finding

* QUARRY2 (B336) built `video/facet_anim.rs` — a stepper, a tick on Facet's service pass, a local
  `FrameDecoder` trait — whose only adapter (`PngStill`) answers "still" for everything. ANIMWEBP (SR44)
  then made `pixel_core` composite every frame of a GIF, animated WebP or APNG, and nothing steps them.
* Worse for Ring 0: the only way to get frames out of pixel_core is `decode`, which keeps EVERY
  composited canvas (`Image::frames`). The kernel shows frame 0 and pays for all of them (Blink's
  count-down-color-test: 74 canvases of 297 KB). QUARRY2's `Decoded` shape (`Vec<(delay, Vec<u32>)>`)
  had the same N-frame cost built in, resampled once per frame.

## The seam

* **pixel_core — shared-core** (`unaos/libs/media/pixel_core`, both rings link it; no new crate).
  Each animated format's compositor becomes a stepper that advances ONE canvas by one frame:
  `gif::Stepper`, `webp::Stepper`, `png::ApngStepper` (over a `png::Parsed` chunk walk). `decode` is
  rebuilt ON them (step + snapshot), so there is one composite per format and the streaming face and
  the full face cannot drift. New public API (`src/anim.rs`):
  * `Animation<B: AsRef<[u8]>>` — borrowed or owned bytes (the kernel keeps the file and its decoder
    in one value): `new`, `width/height`, `frame_count`, `loop_count`, `next_frame() ->
    Option<Result<FrameInfo{index, delay_ms}>>`, `canvas()`, `rewind()`, `buffers_held()`.
    It holds one canvas, plus one more canvas-sized buffer only while a dispose-to-previous frame is
    up (GIF disposal 3, APNG `DISPOSE_OP_PREVIOUS`).
  * `decode_first_frame(bytes)` — `decode(bytes).rgba` with nothing after frame 0 composited or kept.
* **Facet — kernel-by-ruling / fulfiller of the viewer** (`video/facet_anim.rs`, existing, CHARTER
  line kept `Facet — owed` → now `Facet — shared-core`: the decoder half IS the shared core). The
  `FrameDecoder` trait stays; `decoder()` returns the `PixelCore` adapter (sniff → `Animation` over
  the file's bytes). The viewer holds the decoder (one canvas) + the view's base image: two frames.
* **Wallpaper** (`video/facet.rs::decode_file`): a non-PNG file goes through `decode_first_frame`;
  the PNG path stays the streaming IDAT decoder (never a whole-file read).

## Milestones

* **M1 pixel_core.** Steppers, `Animation`, `decode_first_frame`; `tests/facetanim_kat.rs`: every frame
  of the stream byte-equal to `decode`'s `Image::frames` (delays, counts, loop counts, a second pass
  after `rewind`, `buffers_held() <= 2` at every frame) on the ANIMWEBP corpus and every fetched
  GIF/WebP/PNG vector; hand-built GIFs for disposal 2/3 and a broken tail; the three staged fixtures.
* **M2 viewer.** `decoder()` = pixel_core adapter; the viewer composites the next frame into the
  base image when its delay has run out (the existing service-pass tick), `p`/space pauses, loops per
  `loop_count` (forever / n extra plays / once), title `<name> - WxH - z% - frame i/n [paused]`.
  `tests quarry2`'s GIF leg now expects frames (`gif_frames=2`).
* **M3 `tests facetanim`.** Opens `/apps/ANIM3.GIF`, `/apps/ANIM3.WEBP`, `/apps/ANIM3.PNG` (8x8,
  3 frames, staged by the builder from `pixel_core/tests/fixtures/facetanim/`, 219 / 182 / 852 bytes)
  through `decoder()`, steps to frame 1 through the viewer's own base-image reducer and reads the
  pixel back (green at (3,3), red at (0,0)), runs to the end, counts. Wallpaper takes frame 0 only.
* **M4** this doc.

## Witness

    :: FACETANIM: gif=3 webp=3 apng=3 frames_held=2 -> PASS ::

`frames_held` = the view's base image + the most canvas-sized buffers the decoder held at once.
Viewer wire: `[facet] anim path= frames= loop= w= h= decoder=pixel_core k=` on open,
`[facet] anim step=<n> frame=<i>/<n>` every 64 steps, `[facet] anim end plays=<n>` when it stops,
`[facet] anim pause=<0|1>`.

## Owed

* Quarry has no thumbnail path in this tree (its list view draws no picture), so "thumbnails take
  frame 0" has nothing to bind to; whoever adds thumbnails calls `decode_first_frame`.
* The wallpaper's PNG path is the streaming IDAT decoder: for an APNG whose IDAT is NOT frame 0 (no
  fcTL before it) the wallpaper shows the IDAT, not the first fdAT frame. Frame 0 of every other case.

## Results

* `cargo test --release -p pixel_core`: every suite ok (anim_kat 7, facetanim_kat 6, the rest
  unchanged). `facetanim_kat`, all vectors fetched (`fetch-vectors.sh`, 301 files):

  | set | files | animated | frames byte-equal to `decode` |
  |---|---|---|---|
  | ANIMWEBP oracle corpus (`anim_digests.txt`) | 66 decoded (2 refused by both faces) | 66 | 389 |
  | `gif/*.gif` | 42 | 13 | 79 |
  | `apng/*.png` | 52 | 38 | 253 |
  | `anim/*.webp` | 14 | 14 | 136 |
  | `webp/*.webp`, `pngsuite/*.png` | 19 + 103 | 0 | 122 |

  Every frame of the stream equals `decode`'s frame, two passes (`rewind`), `buffers_held() <= 2`.
  `chromium_pinned_animations` (the 68 Chromium-pinned digests) still passes on the refactored
  compositors, and the refactored `decode` was diffed against the pre-refactor one (the merge tip,
  built side by side): 306 of 306 vector and fixture files identical (pixels, frames, delays, loop
  counts, refusal reasons).
* Hand-built GIFs: disposal 3 holds the second buffer only while that frame is up; disposal 2; a
  one-frame GIF is a still; a descriptor cut off after frame 0 — `decode` refuses the file (as before),
  the stream shows frame 0 and then reports `Truncated`, `decode_first_frame` answers frame 0.
* The three fixtures: generator output == committed bytes; frame 0 confirmed by the `image` crate.

## Merge note

`exec-rmbp-merge13` merged clean. One red in a file this arc does not own: `libs/gneiss_pal/src/dsp/mod.rs`
carried `pub mod image;` twice (the keep-both fold of host-merge1 + merge13) — `cargo check -p gneiss_pal`
failed E0428. Smallest fix: the second declaration (and its doc line) dropped.
