# ANIMWEBP — animated WebP and APNG in pixel_core (LEDGER SR44)

Branch `exec-media-animwebp`, cut at 41ecb98e. VP8CORE (`exec-media-vp8` @ c10550d6) merged first; the
two conflicts (root `Cargo.toml` and kernel `Cargo.toml` member/dependency lists, kernel `video/facet.rs`
where QUARRY2's `anim::probe` + `open_base` met PIXELCORE's foreign-decode branch) were resolved by
keeping both sides.

## Finding

VP8CORE took the `image` crate off Aether's WebP path, and with it the one thing that fallback did that
pixel_core did not: an animated WebP (VP8X + ANIM/ANMF) was refused where it used to show its first
frame. PIXELCORE's PNG decoder showed an APNG's default image only. Both now decode, from the
specifications, into the shape GIF already fills: `Image::frames` (every composited canvas, with its
delay) + `Image::loop_count`, and `Image::rgba` = frame 0.

No new crate (the row says so; none was needed). No third-party crate does any of this work.

## Spec coverage

**Animated WebP — RFC 9649 §2.7 (extended format), §2.7.1.1 (Animation).** `src/webp.rs`

* VP8X: the Animation flag routes the rest of the file to `decode_animated`; canvas size from VP8X.
* ANIM: loop count (0 = forever). The background colour is read and ignored — the spec lets a decoder
  ignore it and libwebp's `anim_decode.c` and Blink's `WEBPImageDecoder` both do.
* ANMF: frame X/Y (×2), width/height (−1 coded), duration (24-bit ms), blending method, disposal method;
  frame data = optional `ALPH` + `VP8 `, or `VP8L`; unknown sub-chunks skipped.
* Two passes, like libwebp: a demux pass (a frame rectangle outside the canvas refuses the whole file, as
  `IsValidExtendedFormat` does; a chunk cut off by end of file ends the list), then decode + composite.
* Composite rules (libwebp `anim_decode.c` = Blink, checked against Chromium pixel for pixel):
  transparent initial canvas; a **key frame** (`IsKeyFrame`: the first; a full-canvas frame that has no
  alpha or does not blend; or one whose predecessor was disposed and was full-canvas or itself key)
  starts from transparent and is copied; dispose-to-background clears the previous rectangle; a blended
  frame is alpha-blended with the integer non-premultiplied src-over (`pixel_core::blend_nonpremult` =
  `BlendPixelNonPremult` = Blink `BlendSrcOverDstNonPremultiplied`) **except** inside the rectangle the
  previous frame disposed (copied: `FindBlendRangeAtRow`) and for alpha-255 pixels (copied: the integer
  formula would round them down by one).
* A frame whose bitstream fails ends the animation at the frames before it (Blink keeps showing those);
  a failing first frame refuses the file.
* Loop count → `loop_count` as Blink maps it: 0 → forever (`Some(0)`), 1 → play once (`None`),
  n → `Some(n − 1)` extra plays.
* vp8_core (shared with video): `BoolDecoder::eof` reproduces libwebp's `eof_` (set the first time a read
  needs a byte the partition does not have; at once for an empty partition), `Decoder::overran` reports
  it for partition 0 and every token partition, and `decode_key_frame_strict` refuses such a frame as
  `Truncated` — libwebp's "Premature end-of-file". pixel_core's lossy WebP (still and animated) uses the
  strict call; the video path (libvpx semantics) is unchanged. This is what makes Blink's
  `invalid-animated-webp2.webp` stop at frame 7 in both decoders.

**APNG — PNG 3rd edition §11.3.6 / APNG 1.0.** `src/png/decode.rs`

* `acTL` (num_frames > 0, num_plays), only before the first IDAT; absent → a plain PNG.
* `fcTL`: sequence number, width/height/x/y (must be non-empty and inside the canvas; the default-image
  frame must be the whole canvas at 0,0), delay `round(1000·num/den)` ms with den 0 read as 100,
  dispose_op 0–2, blend_op 0–1.
* `fdAT`: sequence number + data, split chunks concatenated.
* The default image is frame 0 only when an fcTL precedes the first IDAT; otherwise the animation is the
  fdAT frames alone (Chromium shows the first fdAT frame, not the IDAT — checked).
* Each frame decodes at its own size with the IHDR's colour type, depth, interlace, PLTE and tRNS
  (`decode_pixels`, now shared with the still path).
* Composite: transparent canvas; SOURCE copies the rectangle; OVER blends with
  `pixel_core::blend_srcover_f32`; dispose NONE / BACKGROUND (clear to transparent black) / PREVIOUS
  (restore the canvas from before the frame; on frame 0 it acts as BACKGROUND, per the spec).
* A sequence-number break, an invalid fcTL, or a frame that fails to inflate ends the animation at the
  frames before it (a broken first fdAT frame falls back to the default image).

**Two blends, on purpose.** Chromium 141 composites APNG through Skia's `SkPngRustCodec` and
`SkRasterPipeline` in single precision — not through Blink's integer blend that WebP uses. The first
APNG run with the WebP blend missed by up to 26 levels on translucent-over-translucent pixels (apng26)
and by 1 on opaque results (wpt-021/033/035/038). `blend_srcover_f32` reproduces the pipeline operation
for operation: bytes load as `c·(1/255)`, both pixels premultiplied, `src + dst·(1 − src_a)`,
unpremultiplied by multiplying with `1/out_a`, stored as `v·255` rounded half-to-even
(`_mm_cvtps_epi32`). Six rounding variants were scored against all 14,452 distinct (source, destination)
pairs Chromium blended in the corpus; this one (and one sibling) gets every pair right. The plain
`trunc(v + 0.5)` variant misses exactly one pair (an exact tie at 110.5 / 144.5).

## Oracle method

`pixel_core/oracle/chromium-anim-raw.cjs` (Playwright + the pre-installed Chromium 141.0.7390.37,
`--force-color-profile=srgb`): WebCodecs `ImageDecoder` (`premultiplyAlpha: 'none'`,
`colorSpaceConversion: 'none'`) decodes every frame, Blink- and Skia-composited, and each `VideoFrame` is
read with `copyTo` — the raw straight RGBA, no canvas (so no premultiplied round trip), no screenshot. A
frame Blink fails ends its list (`failed_at=K`). `tools/pixel-check --compare-raw` scores every frame:
exact frames, max abs diff split by Chromium's pixel being opaque or translucent. Frame counts, loop
counts (`repetitionCount`) and per-frame durations were compared against `pixel-check --anim-digest`.
`oracle/run-anim-oracle.sh <outdir> <files…>` drives both.

## Results

| corpus | files | frames | result |
|---|---|---|---|
| Blink `web_tests/images/resources` animated WebP (webp-animated*, semitransparent1–4, no-blend, opaque, large, icc-xmp, count-down-color-test, invalid-animated-webp2/4) | 12 | 129 | every frame max_abs_diff = 0 (opaque 0, translucent 0) |
| image-rs/image-webp `tests/images/animated` (random_lossless, random_lossy) | 2 | 7 | max_abs_diff = 0 |
| Blink invalid-animated-webp, -webp3 (frame outside canvas) | 2 | — | Chromium refuses; pixel_core refuses |
| Blink APNG (animated.png, apng00/01/18/24/26, apng-test-suite-dispose-op-none-basic, png-animated-*) | 10 | 45 | max_abs_diff = 0 |
| The APNG test suite as ported to WPT (`png/apng/support/001…062` + the 8 reference stills) | 42 | 208 | max_abs_diff = 0 |

So: 68 files (66 decoded, 2 refused by both), 389 frames, pixel-exact on opaque AND translucent pixels (the row's bar was exact on
opaque, within 1 on translucent); every frame count, loop count and delay equal to Chromium's (e.g.
`32767/65534` → 500 ms, `50/0` → 500 ms, `0/0` → 0, loop 31999 on icc-xmp).

## KATs

`tests/anim_kat.rs` (7 tests) + `tests/anim_digests.txt` (68 pinned lines; vectors fetched from
`tests/vectors.txt` by `fetch-vectors.sh`, nothing committed):

* `blend_matches_chromium_readout` — the integer blend on values read back from Chromium and two
  hand-worked cases.
* `webp_offsets_blend_dispose`, `webp_key_frames_and_disposed_rect_copy`, `webp_structural_refusals` —
  hand-built animations (a ten-line VP8L encoder writes solid-colour frames with five one-symbol prefix
  codes, so each pixel's answer is known): offsets, blend/no-blend, dispose, the key-frame copy, the
  disposed-rectangle copy, loop mapping, frame-outside-canvas refusal, single frame = still, a cut file
  keeps its complete frames, ANMF without the VP8X flag refused.
* `apng_dispose_blend_offsets`, `apng_delays_plays_and_refusals` — hand-built APNGs (stored-deflate
  frames): OVER/SOURCE, NONE/BACKGROUND/PREVIOUS, IDAT outside the animation, delay rounding, plays,
  frame outside the canvas, sequence break, no acTL = still.
* `chromium_pinned_animations` — every oracle file pinned as `WxH frames loop delays crc32(all frames)`
  (or `REFUSED`).

## The faces (M3) — confirmed, not rewritten

* **Kernel facet.rs** — a non-PNG file goes through `decode_foreign` → `pixel_core::decode`, which shows
  `Image::rgba` = frame 0: an animated WebP now opens at its first composited frame where it was refused.
  A PNG keeps the streaming path, so an APNG shows its IDAT; that IS frame 0 whenever the IDAT is part of
  the animation (the common case: frame 0 over a transparent canvas is the IDAT's pixels), and the
  spec's non-APNG rendering when it is not. Kernel code unchanged by this arc (only the merge).
* **Aether** — `decode_raster` takes `Image::rgba` = frame 0; animated WebP no longer lands in the
  `img-webp-refused` ledger miss. Doc comment updated.
* **Stepping** — what steps frames in this tree is the kernel's QUARRY2 `facet_anim.rs`, behind its local
  `FrameDecoder` seam, whose only adapter is still `PngStill` (`frames: None`): GIF, animated WebP and
  APNG all show frame 0 and nothing steps them yet. The host Facet handler (SR29/SR43) is not in this
  tree. The fold is the one adapter + one line `facet_anim.rs` already describes
  (`impl FrameDecoder` over `pixel_core::decode`, `Image::frames` → `(delay, 0xAARRGGBB)`), and it now
  serves all three formats at once — OWED (below), not done here because the row says confirm, and the
  `tests quarry2` leg's expectations change with it.

Gates: `cargo test --release -p pixel_core -p gneiss_pal` green; `cargo check -p aether -p facet` green;
the x86 metal-shape kernel check from `unaos/crates/kernel` (`cargo +nightly check --release --target
../../x86_64-unaos.json -Z build-std=core,compiler_builtins,alloc -Z build-std-features=compiler-builtins-mem
-Z json-target-spec --features "wc,quarry,ftdirx,login,loginst,nvidia-kepler-vblank,smc,usbnet,hda,
hda-tone,facet,beam,sdw,selfhost,linuxabi,ahci,unafs,busreg"`) rc=0 on the merge and on the tip.

## Honest ceiling

* Every frame is composited and kept (`width·height·4` bytes per frame, all `try_reserve`d — a kernel
  heap answers `OutOfMemory`, no panic). The kernel shows only frame 0 but pays for all of them
  (count-down-color-test: 74 × 297 KB); a frame-0-only entry point is owed for Ring 0.
* No ICC / colour management (as before); the ANIM background colour is ignored (as Blink does).
* APNG error tolerance is modelled on the spec and the corpus; Chromium's Rust PNG decoder may refuse some
  malformed streams this decoder accepts (or keep frames this one drops). No malformed-APNG corpus was
  run beyond the WPT/Blink files.
* The float APNG blend matches Chromium 141 (Skia, x86 SSE rounding); a future Chromium that changes its
  PNG compositor would show up as a moved pin, not silently.

## Owed

1. QUARRY2's `facet_anim.rs` fold: the pixel_core `FrameDecoder` adapter + the one `decoder()` line, so
   kernel Facet steps GIF / animated WebP / APNG; then `tests quarry2` expects frames from the GIF leg.
2. A frame-0-only decode for the kernel (`decode_foreign`), so a still view of a long animation does not
   hold every frame.
3. The host Facet handler (SR43 `PixelCoreSource`) gets the frames for free at the fold; its viewer
   stepping is that arc's.
