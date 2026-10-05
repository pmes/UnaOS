# AVCODEC2 (LEDGER SR24, second arc): AV1 inter decoding, written from the specification

Branch `exec-media-av1-inter`, cut from `exec-media-av1` at `791d570d`. The intra decoder (AVCODEC.md)
was complete there. Crate `unaos/libs/media/av1_core`: `#![no_std]` + `alloc`, `#![forbid(unsafe_code)]`,
**no dependencies**. It still builds for `aarch64-unknown-none-softfloat`. Source of truth: *AV1
Bitstream & Decoding Process Specification* v1.0.0 with Errata 1 (the AOMediaCodec/av1-spec Markdown
sources). Names follow the spec's pseudo-code.

## Finding

The crate now decodes AV1 video. That covers every frame type the public vectors contain (key,
inter, hidden frames shown with show_existing_frame, intra block copy), every inter coding tool,
reference scaling (spatial layers), film grain and resolution changes. On **30 libaom conformance
vectors (199 output frames), every frame is MD5-identical to libaom's own decode**. That is
frame-exact, not within a PSNR bound. Chromium agrees as well. Its `<video>` playback of
`bear-av1.mp4` matches our frames at 51.2–53.9 dB, Monochrome.avif (intra block copy) at 60.62 dB,
and Apple's layered AVIF at 51.10 dB.

Before this arc, every inter frame returned `Unsupported("inter frame")`. Intra block copy,
show_existing_frame, superres, film grain and layered AVIF were refused.

## Spec sections implemented in this arc

| module | spec |
|---|---|
| `obu` | §5.9.2 the whole uncompressed header: show_existing_frame (with load_grain_params), RefValid / RefOrderHint handling, mark_ref_frames, frame_refs_short_signaling + **§7.8 set_frame_refs**, frame_size_with_refs, interpolation filter, use_ref_frame_mvs, OrderHints / RefFrameSignBias, setup_past_independence / load_previous (loop-filter deltas, segmentation features, PrevGmParams), segmentation_update_map / temporal_update, §5.9.22 skip_mode_params, frame_reference_mode, allow_warped_motion, **§5.9.24/25 global motion params** (subexp-coded vs PrevGmParams), film grain update / load |
| `refs` | §7.20 the reference frame store (Arc-shared slots): frame, sizes, MiRows/Cols, order hints, SavedOrderHints, MfRefFrames/MfMvs, gm params, segment ids, CDFs, film grain, loop-filter deltas, segmentation features |
| `cdf` | §6.8.2 **every** CDF array (all inter CDFs added: interp filter, motion mode, new/zero/ref MV, compound mode, DRL, is_inter, comp mode/ref/bwd ref/ref type/uni comp ref, single ref, skip mode, MV joint/class/bits/fr/hp for both MV contexts, inter tx sets 1–3, OBMC, inter-intra, wedge, compound idx / group / type); load_cdfs with counter reset; tile copy; §8.2.4 saved CDFs of context_update_tile_id; §7.4 frame_end_update_cdf (AV1 copies the saved tile CDFs: there is no averaging) |
| `modeinfo` (new) | §5.11.18–§5.11.33 inter_frame_mode_info: inter_segment_id with the predicted map and seg_id_predicted contexts, read_skip_mode, read_is_inter, intra_block_mode_info, inter_block_mode_info (compound_mode, new/zero/ref MV, DRL), read_ref_frames (single, bidirectional, unidirectional compound), assign_mv (incl. the intrabc default vector), read_mv / read_mv_component, read_interintra_mode, read_motion_mode + is_scaled, read_compound_type, get_mode, needs_interp_filter. Every §8.3.2 context (count_refs, comp_ref_type, comp_group_idx, compound_idx, interp_filter, ...) |
| `mvpred` (new) | **§7.9** motion field estimation (projection, get_mv_projection, get_block_position); **§7.10.2** find_mv_stack: setup global MV, scan row / col / point with weights, temporal scan + sample, add_ref_mv_candidate, search stack, compound search stack, lower_mv_precision, sorting, extra search + add extra MV candidate, context and clamping; §7.10.3 has_overlappable_candidates; §7.10.4 find_warp_samples / add sample |
| `inter` (new) | **§7.11.3**: rounding variables, MV scaling (any reference size), block inter prediction with Subpel_Filters (regular / smooth / sharp / bilinear, 4-tap variants for small blocks, dual filter), block warp + setup shear + resolve divisor, warp estimation (least squares, i128 where the spec's products exceed i64), OBMC + overlap blending, wedge masks (codebook + master masks), difference-weight mask, inter-intra (intra mode variant) mask, mask blend, distance weights, intra block copy (refIdx −1, uncropped) |
| `decode` | decode_block for inter frames (stores RefFrames, Mvs, InterpFilters, CompGroupIdxs, CompoundIdxs, IsInters, SkipModes), §5.11.33 compute_prediction (sub-8x8 chroma from neighbouring MVs, inter-intra), §5.11.16/17 read_block_tx_size + **read_var_tx_size** (txfm_split contexts), §5.11.36 transform_tree, inter tx sets / types / compute_tx_type, inter-aware tx-size contexts |
| `loopfilter` | §7.14.4/5 reference and mode deltas, isIntra from RefFrames |
| `predict` | is_smooth for inter neighbours (§7.11.2.8) |
| `superres` (new) | §7.16 upscaling between CDEF and loop restoration (LR takes the upscaled CurrFrame) |
| `filmgrain` (new) | **§7.18.3** film grain synthesis: random number process, AR luma/chroma grain with luma correlation, scaling LUT (incl. 10/12-bit interpolation), noise stripes with overlap, restricted-range clip. Applied to output frames only (references keep LrFrame) |
| `image` | `Decoder`: the §7.2 general decoding loop with the reference store, show_existing_frame (§7.21 load for key frames), §7.19 motion-field MV storage, segmentation map carry-over, §7.20 update. `StreamDecoder` keeps references across packets (`decode_temporal_unit_all` returns every shown frame; `reset()` drops references). Layered AVIF presents the top layer |

## Oracle method

1. **libaom per-frame MD5 (frame-exact).** Every AOM conformance vector comes with the MD5 of
   each frame libaom decodes. The hashed bytes are the Y, U and V rows: 8-bit samples as bytes, deeper
   samples as 16-bit LE, and 4:0:0 with mid-grey 4:2:0 chroma. `tools/av1-check <v.ivf|v.mkv> --md5
   <v.md5>` prints MATCH/DIFF per frame. `tests/m5_inter.rs` asserts identity for every frame.
2. **Chromium `<video>`.** `tools/av1-check/oracle/vrun.sh <clip.mp4> <dir> <fps> <frames>`
   works in three steps. First av1-check demuxes the MP4 (fragmented too) or Matroska/WebM itself and
   writes frame N as a PNG. Then `vshot.cjs` has Chromium (Playwright, `/opt/pw-browsers`) play
   the same file 1:1, seek to the middle of frame N's display interval, wait for
   `requestVideoFrameCallback` and screenshot the video rectangle. The script prints the presented
   `mediaTime`, and it was verified equal to frame N for every frame shot. Last, `compare.cjs`
   reports RGB PSNR.
3. **Chromium `<img>`** (AVCODEC's `run.sh`) for AVIF. Display-P3 images get the same P3→sRGB
   conversion Chromium applies (oracle-side only).

## Results

### Frame-exact vs libaom (every frame MD5-identical)

| vector | frames | what it exercises (ToolStats) |
|---|---|---|
| av1-1-b8-01-size-16x16, 34x34, 66x66, 196x196, 226x226 | 5 × 2 | odd sizes, OBMC, local warp, inter-intra, dual filter |
| av1-1-b8-00-quantizer-00, 10, 20, 32, 45, 63 | 6 × 2 | lossless inter (q 0), OBMC, local + global warp, wedge inter-intra, var-tx, smooth/sharp filters |
| av1-1-b10-00-quantizer-00, 32, 63 | 3 × 2 | the same at 10 bits |
| av1-1-b8-02-allintra | 39 | intra regression through the new header path |
| av1-1-b8-03-sizeup / sizedown (Matroska) | 2 × 20 | resolution changes with new sequence headers, compound, skip mode |
| av1-1-b8-04-cdfupdate | 2 | frame-end CDF update, load_cdfs |
| av1-1-b8-05-mv | 4 | compound wedge / diff-weighted / distance, skip mode, global warp, extreme MVs, show_existing_frame |
| av1-1-b8-06-mfmv | 4 | motion field projection (temporal MVs), everything above |
| av1-1-b8-16-intra_only-intrabc-extreme-dv | 2 | 17,050 intra-block-copy blocks, 1920x1080 |
| av1-1-b8-22-svc-L1T2, L2T1, L2T2, L2T1-2, L2T2-2 | 5 × 8 | temporal + spatial layers; L2T1 alone has 14,334 scaled-reference blocks |
| av1-1-b8-23-film_grain-50, av1-1-b10-23-film_grain-50 | 2 × 10 | film grain synthesis 8/10-bit, delta q (30 superblocks) |
| av1-1-b8-24-monochrome, av1-1-b10-24-monochrome | 2 × 10 | 4:0:0 inter, hidden frames, show_existing_frame |

That is 30 vectors and 199 frames, 199 identical. The first inter frame decoded matched libaom on the first build.

### Chromium

| input | measured |
|---|---|
| bear-av1.mp4 (Chromium's own clip, 320x240, 82 frames: compound, OBMC, local + global warp, temporal MVs), frames 0,1,2,3,10,30,45,60,75,81 | RGB PSNR 51.19, 51.19, 51.23, 51.35, 52.05, 53.24, 53.92, 53.26, 53.55, 53.19 dB; max abs diff 6–12 (blue channel; R/G 55–58 dB) |
| Monochrome.avif (intra block copy, 1280x720) | 60.62 dB, max abs diff 1 |
| Apple animals_00 multilayer a1lx / a1op / lsel (2048x1536, the top layer predicted from a half-size base) | 51.10 dB (the single-layer encode of the same image: 50.99 dB) |
| Xiph fruits_2layer_thumbsize (segmentation, scaled refs) | 43.75 dB RGB, max abs diff 8. Averaged over 2x2 in Y'CbCr: Y 56.3, Cr 59.0, Cb 45.6 dB (saturated chroma clipped in RGB) |

The PSNR stays flat over 82 inter frames (frame 0, a key frame: 51.2 dB; frame 81: 53.2 dB), so
nothing drifts. The remaining difference is the Y'CbCr→RGB conversion and chroma upsampling (blue).
The MD5 identity above is the stronger proof of the decode itself.

### Robustness

libaom's invalid-stream corpus (23 oss-fuzz / bug-report files) is fed temporal unit by temporal
unit, errors included. It returns errors and never panics, in a debug build with overflow checks. This
run found two bugs, now fixed: a usize underflow in the coeff_base_eob context (an intra-arc bug that
release builds hid), and an unbounded eob. The eob is now clamped to segEob.

## KATs

`cargo test -p av1_core --release`: **20 unit + 8 (m1) + 3 (m3) + 11 (m5_inter), all pass.**

- unit (new): `cdf::reset_counts_zeroes_every_counter`; `inter::subpel_filters_have_unit_gain` (all 6
  Subpel_Filters + 193 Warped_Filters sum to 128), `identity_warp_is_valid_and_unsheared`,
  `resolve_divisor_approximates_reciprocal`, `wedge_masks_are_complementary`;
  `filmgrain::lfsr_matches_the_spec_recurrence`.
- `tests/m5_inter.rs`: intra_block_copy_monochrome_avif (pinned planes), intrabc extreme-dv, inter
  sizes, quantizers (with tool-coverage assertions), reference management (cdfupdate, mv, mfmv,
  monochrome ×2, asserting compound wedge/diffwtd/distance + skip mode + temporal MVs), scalable
  streams (asserting scaled references), film grain, resolution changes (Matroska), the invalid corpus,
  and layered AVIF (pinned planes). All vectors are fetched at test time from `tests/vectors.txt`
  (URL + sha256 + size) and skipped when offline. Nothing is committed.
- m3's 15 intra AVIF pins are unchanged, so the intra decode moved no pixel.

## Honest ceiling: what is NOT proven or NOT decoded

- **Superres (§7.16)** is implemented from the spec and wired in, but **no public vector uses
  it**. The AOM bucket (listed in full) and all 172 av1-avif test files were checked. Owed: a proof.
- **Segmentation** is parsed and applied (inter_segment_id, the predicted map, feature data,
  update_map = 0 carry-over). Only fruits_2layer uses it, and no vector uses the temporal prediction
  (seg_id_predicted = 0 everywhere). It has the Chromium proof above, not a frame-exact one.
  **delta_lf** is never read by any vector. **delta_q** is frame-exact (film grain vectors).
- Not seen in any vector: the bilinear filter on inter blocks (it only appears via intrabc, with
  integer MVs), switch frames, intra-only frames as output, frame_id_numbers, error-resilient inter
  frames, 12-bit inter, and 4:2:2 / 4:4:4 inter. The code paths exist from the spec, with no oracle yet.
- Large-scale tile (Annex D), and AVIF grid items, alpha, image sequences (`avis` moov tracks),
  irot / imir / clap application.
- Performance is not tuned. It runs ~50–110 ms per 352x288 inter frame and ~0.5 s per 1280x720
  frame (release, one core). The structure is spec-literal (per-sample closures, per-call
  allocations). SIMD-free optimisation is owed before 1080p real-time playback.

## Consumers / fold

- `libs/gneiss_pal/src/dsp/video/av1.rs` (PLAYBACK's `VideoDecoder` seam) is unchanged in shape. Its
  `StreamDecoder` now carries references across packets and `reset()` drops them on seek. The docs
  and name are updated. The fold wiring is unchanged from AVCODEC.md (`av1 = ["dep:av1_core"]`).
- No new handler: Stria owns A/V and Facet owns images (CODEX Amendment II).

## Third-party crates

None. `av1_core` has no dependencies. `tools/av1-check` has none: MD5, SHA-256, PNG, IVF, MP4
(incl. fragmented) and Matroska demux are all written in the tool. The oracle uses the pre-installed
Playwright + Chromium and node's built-in zlib, none of which is in any build.

## How to continue

1. `cargo build --release -p av1-check`, then for any IVF/MKV vector:
   `target/release/av1-check v.ivf --md5 v.ivf.md5 -v` (per-frame MATCH + tool counts). For a clip:
   `tools/av1-check/oracle/vrun.sh clip.mp4 work 29.97 0,10,20`.
2. Owed proofs: a superres stream (e.g. libaom `--superres-mode=1` encode with its decoder MD5s
   once an encoder is available, or the Argon conformance suite); delta_lf and segmentation with
   temporal update (libaom `--deltaq-mode`, `--aq-mode=1` encodes); switch frames.
3. Owed features: AVIF grid / alpha / `avis` sequences / irot-imir-clap; large-scale tile;
   performance (row-based prediction buffers, no per-block allocation, then SIMD).
