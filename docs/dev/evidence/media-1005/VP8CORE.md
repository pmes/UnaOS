# VP8CORE — one codec, two ceilings closed (ledger SR40)

Branch `exec-media-vp8`, cut at `e2ab59be`; `exec-media-pixel` (PIXELCORE, SR25) and `exec-media-playback`
(PLAYBACK, SR26) merged in at the start (`dsp/mod.rs` hand-joined, every `pub mod` kept). New crate
`unaos/libs/media/vp8_core` (`no_std` + `alloc`, `#![forbid(unsafe_code)]`, **zero dependencies**), a root
workspace member; the kernel reaches it through `pixel_core` (its path dependency).

## Finding

PIXELCORE refused lossy WebP by name and Aether fell back to the `image` crate for it; PLAYBACK showed the
labelled test-pattern stand-in for WebM `V_VP8`. Both are the same missing piece: a VP8 decoder (lossy WebP is
one VP8 key frame, RFC 9649 §2.5). VP8CORE is that decoder, written from RFC 6386, and both faces now use it.
No third-party crate decodes anything on either path.

## What the core decodes (RFC 6386 sections)

* §7 boolean entropy decoder (64-bit window; round-trip KAT against the §7 reference *encoder*, 20 000 bits).
* §9.1/§19.1 frame tag, start code, dimensions + scale bits; §9.2–§9.11/§19.2 the whole frame header:
  segmentation (map + tree probs, per-segment quantiser and filter level, absolute/delta), filter type /
  level / sharpness, reference and mode loop-filter deltas, 1/2/4/8 token partitions, the six quantiser
  indices, golden/alt-ref refresh + copy (1 = last, 2 = the other) + sign bias, `refresh_entropy_probs`
  save/restore, `refresh_last`, token-probability updates (§13.4), the skip flag, intra/last/golden probs,
  updatable inter-frame Y/UV mode probabilities, motion-vector probability updates (§17.2).
* §11/§16/§19.3 per-macroblock modes: key-frame contextual sub-block modes, inter-frame intra modes, reference
  selection, the near-MV search with sign-bias inversion and the clamp (§16.3), NEAREST/NEAR/ZERO/NEW,
  SPLITMV with the 16×8 / 8×16 / 8×8 / 4×4 partitions and LEFT/ABOVE/ZERO/NEW sub-vectors (§16.4), the
  long/short MV component coding (§17).
* §13 DCT tokens (four block types, bands, contexts, DCT_CAT1..6 extra bits), §14.1 dequantisation (Y2 DC ×2,
  Y2 AC ×155/100 ≥ 8, UV DC ≤ 132), §14.3 inverse WHT, §14.4 inverse DCT — with the spec's 16-bit
  intermediates (values wrap to `i16` between passes exactly as its `short` arrays do).
* §12 intra prediction: 16×16 and chroma DC/V/H/TM with the §12.2 edges (127 above, 129 left, corner 127 on
  the top row else 129; DC uses only the available edges), the ten 4×4 modes (§12.3) with the above-right rule
  (right-column sub-blocks reuse the row above the macroblock; the last column repeats its last pixel).
* §18 inter prediction from last/golden/alt-ref: six-tap (version 0), bilinear (1, 2), full-pixel chroma (3);
  2-D filtering in the spec's two rounded+clamped passes; chroma vectors per §17.4 (whole-MB vector halved;
  per 2×2 luma group averaged, rounding away from zero). Reads outside the reference are edge-clamped — the
  reference decoder's 32-pixel border made infinite, equivalent because every vector reaching past the
  border is clamped to one that sees only replicated samples.
* §15 loop filters: normal (MB-edge and sub-block variants, hev thresholds by frame type) and simple, the
  per-macroblock level (segment override, ref/mode deltas), inner edges skipped for coefficient-less
  non-B_PRED/non-SPLITMV macroblocks. Intra prediction uses the unfiltered frame; the filter runs after.
* §9.7 reference updates in the reference decoder's order: alt-ref copy, golden copy (so "golden ← alt-ref"
  sees an alt-ref copied in the same frame), then the refreshes. The RFC prose is silent on that order; the
  reference decoder's MD5s settle it.
* `yuv`: I420 → RGBA as libwebp presents a lossy WebP (BT.601 limited range in 14-bit fixed point, "fancy"
  9-3-3-1 upsampling in libwebp's exact integer steps). Not av1_core's float H.273 converter: the shapes
  differ (u8 planes vs u16, nclx vs WebP's fixed BT.601), and the WebP oracle demands libwebp's rounding.

Tables: `tools/gen_tables.py` generates `src/tables.rs` (default and update token probabilities, key-frame
sub-block mode probabilities, mode/MV probabilities, quantiser lookups, split tables). rfc-editor.org and
ietf.org are refused by the build proxy, so the numbers are read from the libvpx source files RFC 6386 §20
ships as its reference decoder — the same tables. Only data is taken; the decoder is written from the prose,
and the bit-exact KATs prove the numbers.

## Oracles and KATs

1. **Reference-decoder MD5s (bit-exact).** `tests/conformance.rs`: every shown frame's cropped I420 MD5 equals
   the `.ivf.md5` the reference decoder wrote. **62 vectors, 2 142 frames, all exact**: the comprehensive set
   vp80-00-comprehensive-001..018 (1 082 frames, one test each) plus the rest of libvpx's VP8 set (intra,
   inter, segmentation ×22, partitions, sharpness ×10, smallsize: 44 vectors, 1 060 frames). Go-red: changing
   one MB-filter constant (27 → 26) turns 13 of the 18 comprehensive vectors red. Vectors are fetched at test
   time with `curl` from `tests/vectors.txt` (URL + sha256, cache `$VP8_VECTORS` or the target tmpdir),
   skipped offline. Never committed.
2. **Chromium, lossy WebP, decoder planes.** `oracle/webp-raw.cjs` decodes each file with WebCodecs
   `ImageDecoder` (premultiplyAlpha/colourSpaceConversion `none`) and copies the frame out with
   `VideoFrame.copyTo`. For opaque lossy WebP Chromium returns **I420 — libwebp's planes themselves**;
   `pixel-check --compare-i420` scores vp8_core's planes: the 5 lossy gallery files (550×368 … 1280×720),
   **Y/U/V max_abs_diff 0, 0 differing samples**. Their RGB screenshots (`pixel_core/oracle/run-oracle.sh`,
   white background) also score **max_abs_diff 0, PSNR ∞**.
3. **Chromium, lossy + alpha.** For the 5 gallery2 `*_webp_a.webp` files (VP8X + ALPH, VP8L-compressed
   alpha) Chromium returns unpremultiplied BGRA: pixel_core's RGBA is **byte-identical, transparent pixels
   included** (`--compare-raw` max_abs_diff 0; CRC-32 of the whole buffer equal). The public files only use
   VP8L-compressed, unfiltered alpha, so `oracle/alph-variants.py` re-encodes one with **raw** alpha under
   filtering methods 0–3: Chromium decodes all four back to the original alpha and pixel_core matches it
   exactly on each (100.000 %). `pixel_core/tests/webp_kat.rs` repeats that round trip offline.
4. **Chromium, VP8 WebM, frame by frame.** `oracle/video-frames.cjs` plays the WPT
   `test-av-384k-44100Hz-1ch-320x240-30fps-10kfr.webm` (V_VP8 + Vorbis) at 0.25×; on every
   `requestVideoFrameCallback` it wraps the presented frame as a `VideoFrame` and dumps its I420.
   `play-check <file> --i420-oracle <dir>` plays the same file through Stria's player (decoder now
   `vp8 (vp8_core)`, `real_video: true`): **60/60 frames compared, 60 exact, max_abs_diff 0**, and the existing
   mediaTime oracle still holds (within one frame, count 60 = 60). Go-red: one corrupted byte in one oracle
   frame → `exact 59, max_abs_diff 73, ok false`.

Pinned offline answers: `pixel_core/tests/oracle_digests.txt` (10 lossy CRC-32s, each pinned only after the
Chromium score above was exact) and `gneiss_pal::dsp::video` test `vp8_webm_decodes_to_chromiums_frames`
(FNV-1a over the 60 frames = the FNV of Chromium's own 60 dumped frames, `1cf9f393b689319c`).

## Faces

* `pixel_core::webp` decodes `VP8 ` (through `vp8_core::decode_key_frame` + `vp8_core::yuv`), `VP8X` canvas
  check, `ALPH` (raw or VP8L image-stream with implicit dimensions — `decode_vp8l_stream`, split out of the
  VP8L decoder — and the three unfilters). `vp8_core` is pixel_core's only dependency (UnaOS's own).
* Aether `images::decode_raster`: a WebP never falls back to the `image` crate any more (a refused WebP —
  animation — is a ledgered `img-webp-refused` miss). The fallback stays for ICO/TIFF/AVIF only.
* `gneiss_pal::dsp::video`: `Codec::Vp8 => vp8::Vp8Decoder` (WebM `V_VP8`, MP4 `vp08` accepted), frames as
  `Pixels::I420`, alt-ref (hidden) frames return `Ok(None)`, `reset` drops the references.
* `tools/pixel-check --compare-i420`, `tools/play-check --i420-oracle`.

## Honest ceiling (NOT decoded / not proven)

* Animated WebP (`ANIM`/`ANMF`) is still refused — and, with the fallback gone, Aether no longer shows its
  first frame via the `image` crate (it did before). Owed to pixel_core.
* The scale bits are reported, not applied (display hint). `color_space` = 1 is decoded as 0. No error
  concealment: a truncated partition decodes on zeros (as the reference does); a corrupt stream is decoded
  as far as it parses.
* Versions 1–3 (bilinear / full-pixel) are implemented from the spec but no public KAT exercises them (all 62
  vectors are version 0). The simple loop filter IS exercised (comprehensive set).
* Speed: correctness-first scalar code (a generic edge-clamped window per prediction block); fine for the
  test vectors and 320×240 video, not measured at HD frame rates. No SIMD.
* `dsp::video::Frame::to_rgba` (PLAYBACK's nearest-chroma BT.601) is what Stria presents; the WebP-exact
  converter lives in `vp8_core::yuv`. The video oracle compares decoder planes, not presented RGB.

## Third-party crates

None in `vp8_core`, `pixel_core` (library), `pixel-check`, or the VP8 path of `gneiss_pal`. The `image`
crate remains a pixel_core *dev*-dependency (lossless second opinion; it no longer judges lossy) and Aether's
fallback for non-WebP formats (chicken wire, named in Aether's Cargo.toml).

## Owed

Animated WebP (ANIM/ANMF compositing) in pixel_core; a version 1–3 KAT; a performance pass (SIMD-free row
filters, fewer window copies) measured on an HD WebM; the CODEX/ledger fold by the seat.
