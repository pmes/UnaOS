# AVCODEC (LEDGER SR24): an AV1 decoder written from the specification, intra first (AVIF)

Branch `exec-media-av1`, cut from `0686cc1b`. Crate `unaos/libs/media/av1_core`: `#![no_std]` + `alloc`,
`#![forbid(unsafe_code)]`, **no dependencies**. A ROOT workspace member. Source of truth: *AV1 Bitstream &
Decoding Process Specification* v1.0.0 with Errata 1. Function and variable names follow the spec's
pseudo-code.

## Finding

Before this arc the tree had no video decoder and decoded AVIF nowhere. Today a key frame (or
intra-only frame) of any AV1 profile (0/1/2), at 8, 10 or 12 bits, in 4:0:0/4:2:0/4:2:2/4:4:4, decodes
with every intra coding tool and all three post filters, from an AVIF file or from a raw OBU stream
or temporal units. Against Chromium's own rendering, every 8-bit vector Chromium displays matches
within **max abs diff 2** in RGB (the difference is only Y'CbCr→RGB rounding). This means the decoded
planes agree with libdav1d/libaom except for rounding.

## Spec sections covered

| module | spec |
|---|---|
| `bits` | §4.10 f(n) su(n) ns(n) le(n) leb128() uvlc() |
| `obu` | §5.3 OBU framing; §5.5 sequence header (all fields); §5.9 uncompressed header for KEY / INTRA_ONLY (frame size, superres params, tile info, quantization, segmentation, delta q/lf, loop filter, CDEF, LR, tx mode, film grain params) |
| `avif` | ISOBMFF/HEIF ftyp/meta/hdlr/pitm/iinf/iloc/iprp/ipco/ipma; ispe pixi av1C colr irot imir clap; `Av1Config::parse` |
| `symbol` | §8.2 init_symbol, read_symbol with CDF adaptation, read_bool, read_literal, NS, exit_symbol padding check |
| `cdf` + `tables` | §6.8.2 init_non_coeff_cdfs / init_coeff_cdfs; the default CDFs, scans, quantizer, filter taps all **generated** by `tools/gen_tables.py` into one file `src/tables.rs` (213 tables, 422 constants; one erratum handled, a missing comma in Split_Tx_Size) |
| `decode` | §5.9.11–§5.11 decode_tile, partition, block, intra_frame_mode_info (segment id, skip, cdef idx, delta q/lf, y/uv modes, angle deltas, CfL alphas, palette with cache + color map, filter intra), tx size / var-tx, residual, transform_type, coeffs; §8.3.2 every context derivation; §5.11.57 LR unit coefficients |
| `predict` | §7.11.2 intra: DC/V/H/Paeth/Smooth(V/H)/directional with edge filter + upsampling, filter intra, §7.11.4 palette, §7.11.5 CfL |
| `transform` | §7.12.3 dequant + reconstruct; §7.13 DCT4..64, ADST4/8/16, flip, identity4..32, WHT (lossless) |
| `loopfilter` | §7.14 deblocking (4/6/8/14-tap, levels, sharpness, delta lf) |
| `cdef` | §7.15 CDEF (direction search, primary/secondary filters) |
| `restoration` | §7.17 loop restoration (Wiener, self-guided) |
| `image` | §7.4 decode_frame_wrapup (intra) and §7.18 output; `decode_avif(&[u8]) -> Image` (RGBA, nclx/sequence matrix BT.601/709/2020, limited/full range); `StreamDecoder` for temporal units |

Each post filter can be switched off (`Filters`, `av1-check --no-deblock|--no-cdef|--no-lr`).

## Oracle method

`tools/av1-check` (no crates) decodes and writes a PNG. `tools/av1-check/oracle/run.sh` has Chromium
(Playwright, `/opt/pw-browsers`) render the same `.avif` 1:1 in an 800x600 page (`--force-color-profile=srgb`)
and takes a screenshot, then `compare.cjs` (node zlib only) reports RGB PSNR and max abs diff over the
shared window (the top-left 800x600 at most).

| vector (tests/vectors.txt) | geometry | PSNR dB | max abs diff |
|---|---|---|---|
| white_1x1 | 1x1 4:4:4 q255 | inf | 0 |
| extended_pixi | 4x4 4:2:0 | 53.80 | 1 |
| colors_sdr_srgb (lossless, palette) | 200x200 4:4:4 | 55.63 | 1 |
| colors_text_sdr_srgb (lossless, palette) | 200x200 4:4:4 | 55.85 | 1 |
| kodim23_yuv420_8bpc | 768x512 | **54.10** | 2 |
| kodim03_yuv420_8bpc | 768x512 | 56.54 | 2 |
| Chimera_8bit_cropped_480x256 (4 tiles) | 480x270 | 54.83 | 1 |
| kids_720p | 1280x720 | 55.08 | 2 |
| Irvine_CA | 480x640 | 54.93 | 2 |
| sdr_cosmos 4:2:0 limited q160 (LR on all planes) | 2048x858 | 54.10 | 2 |
| sdr_cosmos 4:4:4 full q160 | 2048x858 | 60.64 | 1 |
| fox 8-bit mono | 1204x800 | inf | 0 |
| fox 8-bit 4:4:4 odd size (BT.2020 matrix) | 1203x799 | 47.86 | 5 |
| fox 10-bit 4:2:0 (BT.2020 matrix) | 1204x800 | 44.35 | 6 |
| fox 12-bit 4:2:2 | 1204x800 | no oracle: Chromium renders it black | — |

The BT.2020-matrix/high-depth rows differ by 5–6 in the blue channel only, which fits Chromium's
reduced-precision conversion for that matrix. Every tile of those streams still passes exit_symbol,
so the entropy decode is in sync.

Filter ablation, each filter switched off in turn against the same oracle, kodim23: deblock off
50.2 dB / max 9, CDEF off 50.5 / 8 (LR is not used in this stream). sdr_cosmos 4:2:0: deblock off
50.6 / 10, CDEF off 49.9 / 6, LR off 51.3 / 10. Every filter is active, and it is correct only with
all of them on.

## KATs (`cargo test -p av1_core --release`: 14 unit + 8 M1 + 4 M3 + sha256 KAT, all pass)

- unit: bits (4), symbol adaptation rule + equiprobable bool, predict (DC/V/H/Paeth/Smooth values,
  45° directional shift, filter-intra unit gain), transform (IDCT and ADST4 against float, WHT DC),
  neg_deinterleave, inverse_recenter.
- `tests/m1_headers.rs`: hand-built sequence/frame headers field by field, OBU framing, AVIF container on
  the public vectors.
- `tests/m3_decode.rs`: for 15 public AVIF vectors, (1) the §8.2.4 exit_symbol padding check passes on
  every tile (18 tiles), and (2) the FNV-1a 64 of the decoded Y/U/V planes equals the pinned value. The
  values were recorded from the decodes above that match the oracle. Also: honest refusals (intra
  block copy, layered AVIF) and StreamDecoder agreeing with the still path.
  Vectors are fetched at test time with sha256 checks and skipped when offline. Nothing over 200 KB is committed.

## Intra tools the vectors actually exercised (ToolStats via av1-check)

All 13 y modes and all 14 uv modes (including CfL: 42–892 blocks per stream), non-zero angle deltas,
filter intra (176–3879 blocks), palette Y and UV (only the screen-content lossless `colors_*`),
lossless/WHT (`colors_*`), transform sizes 4x4…64x64 including rectangular 4:1 (TX_16X64/64X16
only in sdr_cosmos), intra tx types DCT_DCT/ADST_DCT/DCT_ADST/ADST_ADST/IDTX/V_DCT/H_DCT,
multiple tiles (Chimera, 4x1), 64 and 128 superblocks, mono, 4:2:2, 4:4:4, odd sizes, 8/10/12-bit,
Wiener and self-guided LR units. **Not exercised by any vector:** segmentation, delta q / delta lf,
quantizer matrices (`using_qmatrix`), skip = 1 blocks, reduced_tx_set, FLIPADST types (intra sets
never use them), 128x128 LR units mixed with superres. The code for these exists from the spec but
has no oracle proof yet.

## Honest ceiling: what is NOT decoded

- **Intra block copy** (`allow_intrabc`, screen content key frames; Microsoft `Monochrome.avif`) is
  refused with `Unsupported("intra block copy")`. It needs the MV-stack machinery shared with inter.
- **Inter frames** (all of video after the first key frame): `Unsupported("inter frame")`.
- Superres upscaling (§7.16), film grain synthesis (§7.18.3), show_existing_frame, large-scale tile.
- AVIF: grid items, alpha auxiliary images, layered/progressive (`a1lx`/`lsel`, Xiph
  `fruits_2layer_thumbsize` refused), irot/imir/clap parsed but not applied, ICC ignored.

## INTER is owed: the spec sections

§5.9.2 the inter half of uncompressed_header (refresh/ref_order_hint, frame_refs_short_signaling with
§7.8 set_frame_refs, frame_size_with_refs, interpolation_filter, is_motion_mode_switchable,
use_ref_frame_mvs, skip_mode_params §5.9.22, global_motion_params §5.9.24/§5.9.25, film grain
update/load); §5.11.18–§5.11.33 inter_frame_mode_info (inter_segment_id, read_is_inter, intra_block_mode_info,
inter_block_mode_info, read_ref_frames, read_motion_mode, read_inter_intra, read_compound_type, assign_mv /
read_mv, §5.11.23 intrabc); §7.9 motion field estimation; §7.10 motion vector prediction (the MV stack,
scan row/col/point, temporal, extra search, sorting, global MV); §7.11.3 inter prediction (rounding
variables, motion vector scaling, block inter prediction with the 8-tap filters, warp estimation and
§7.11.3.6 setup shear, block warp, OBMC, wedge / difference-weight / inter-intra masks, mask blend,
distance weights); §7.11.3.2 intrabc rounding; §7.20 the reference frame update process (saving CDFs,
loop filter deltas, segmentation, film grain, MVs per reference); §7.21 reference frame loading;
§7.4 the CDF averaging at frame end (disable_frame_end_update_cdf = 0, context_update_tile_id); the inter-only
CDF tables (already generated into tables.rs, not yet in `CdfContext`); segmentation map prediction from
the previous frame; §7.16 superres; §7.18.3 film grain.

## Consumers

- **Stria via PLAYBACK** (`gneiss_pal::dsp::video::av1`): `libs/gneiss_pal/src/dsp/video/av1.rs` is written
  against PLAYBACK's `VideoDecoder` seam (branch exec-media-playback, which already declares
  `#[cfg(feature = "av1")] pub mod av1;` and the registry arm). It compiles there, and an end-to-end test
  passed in a scratch overlay of that branch: a built MP4 with three AV1 key-frame samples went through
  demux_core → `decoder_for` → `Av1Decoder`, and each frame's RGBA equalled `decode_avif`'s. Fold step:
  `av1 = ["dep:av1_core"]` and
  `av1_core = { path = "../../unaos/libs/media/av1_core", optional = true, features = ["std"] }` in gneiss_pal's Cargo.toml.
  On this branch the file is an orphan until PLAYBACK's `dsp/video.rs` lands.
- **Facet via pixel_core** (PIXELCORE, branch exec-media-pixel): behind an `avif` feature, pixel_core maps
  `av1_core::decode_avif(&[u8]) -> av1_core::Image { w, h, rgba }` onto its `Image { width, height, rgba,
  frames: None, .. }`. That is a three-line arm, owed at the fold.
- No new handler is needed: Stria owns A/V and Facet owns images (CODEX Amendment II, SR29).

## Third-party crates

None in `av1_core` and none in `tools/av1-check` (PNG writer, CRC, Adler, SHA-256 and FNV are written here).
The oracle uses the pre-installed Playwright + Chromium and node's built-in zlib. None of them is in the build.

## How to continue

1. `cargo build --release -p av1-check`, fetch vectors (`tests/vectors.txt`), then
   `tools/av1-check/oracle/run.sh <f.avif> target/ora` for any new vector.
2. Next milestone: intrabc (§5.11.23 + the §7.10 MV stack subset + §7.11.3.2), proved on `Monochrome.avif`.
   Then inter, in this order: reference update/load (§7.20/§7.21) → MV prediction → translational block
   inter prediction → OBMC/warp/compound. Use a short AV1 WebM, with Chromium `<video>` frame-N screenshots as the oracle.
3. Find vectors that exercise segmentation, delta q/lf and qmatrix (e.g. libaom `--deltaq-mode`,
   `--enable-qm` encodes in public AVIF test sets) to prove those paths.
