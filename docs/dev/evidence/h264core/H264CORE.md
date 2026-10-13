# H264CORE (LEDGER SR133) — evidence

`unaos/libs/media/h264_core`: a `no_std` + `alloc`, zero-dependency H.264 / AVC decoder written
from ITU-T Rec. H.264, linked by Stria through `libs/gneiss_pal/src/dsp/video/h264.rs`
(`decoder_for`: `Codec::Avc` → `h264 (h264_core)`). Branch `exec-host-h264core`, cut from
`ef3fe22d`, eight commits (one per stage; each commit body carries that stage's design with clause
citations, the first carries the module plan and the entropy / DPB mechanisms).

## Status of the measurements

| what | status |
|---|---|
| `cargo check -p h264_core -p gneiss_pal -p stria --tests` | rc=0 at every commit of the series; the last run is [check.log](check.log) |
| `cargo test -p h264_core` | **not run here** (R107: the seat runs every test once, at the fold) |
| JVT conformance bit-exact count | **0 of 38 measured** — owed to the fold; the vectors could not be fetched at this seat (below) |
| vector pins (sha256) | **owed**: `www.itu.int` answers 403 at this seat's egress proxy (`curl: (56) CONNECT tunnel failed, response 403`, 2026-10-13), so `tests/vectors.txt` rows carry `-`; the harness prints `UNPINNED <path> <sha256>` on first fetch for the seat to pin |
| table transcription | `tools/gen_tables.py` holds the Recommendation's tables as printed and checks sizes, prefix-freeness / completeness of the VLC codes and monotonicity of rangeTabLPS; the PDF itself (same host) could not be opened to re-check the transcription, so the CABAC (m, n) columns (Tables 9-12 .. 9-24) are checked only by the fold's CABAC KATs |

## Feature table

| feature | clause | state |
|---|---|---|
| NAL units, emulation prevention | 7.3.1, 7.4.1 | decoded |
| Annex B byte stream | B.1, B.2 | decoded |
| avcC + length-prefixed samples (MP4 avc1/avc3, MKV V_MPEG4/ISO/AVC) | ISO/IEC 14496-15 5.3 | decoded |
| SPS (incl. High branch, VUI bitstream_restriction), PPS, scaling lists + Table 7-2 fall-back | 7.3.2, E.1 | decoded |
| slice header, ref list modification, pred weight table, dec_ref_pic_marking | 7.3.3 | decoded |
| CAVLC residuals | 9.2 | decoded |
| CABAC (all frame-coding contexts, I_PCM restart) | 9.3 | decoded |
| I / P / B slices | 7.4.3 | decoded |
| Intra_4x4, Intra_8x8 (reference filtering), Intra_16x16, chroma, I_PCM | 8.3 | decoded |
| constrained_intra_pred | 8.3.1.2 | decoded |
| P partitions 16x16 .. 4x4, P_8x8ref0, P_Skip | 8.4.1.1, 8.4.1.3 | decoded |
| B partitions, B_Skip / B_Direct_16x16 / B_Direct_8x8, spatial and temporal direct, direct_8x8_inference | 8.4.1.2 | decoded |
| quarter-sample luma / eighth-sample chroma interpolation | 8.4.2.2 | decoded |
| weighted prediction: explicit (P, B) and implicit (B) | 8.4.2.3 | decoded |
| multiple reference frames, long-term references | 8.2.4, 8.2.5 | decoded |
| 4x4 / 8x8 transforms, luma / chroma DC, scaling matrices | 8.5 | decoded |
| deblocking (bS 0-4, alpha / beta / tC0, disable_deblocking_filter_idc 0/1/2) | 8.7 | decoded |
| POC types 0 / 1 / 2 | 8.2.1 | decoded |
| sliding window, MMCO 1-6, frame_num gaps | 8.2.5 | decoded |
| DPB output order (bumping), max_num_reorder_frames | C.4 | decoded |
| frame cropping | 7.4.2.1.1 | decoded |
| arbitrary slice order, redundant slices (skipped) | 7.4.3 | decoded |
| field pictures, MBAFF | 7.3.3, 7.3.4 | refused by name (`interlace (field pictures)`, `interlace (MBAFF)`) |
| FMO (slice groups) | 8.2.2 | refused by name (`FMO (slice groups)`) |
| data partitioning, SP / SI slices (Extended) | 7.3.2.9, 8.6 | refused by name |
| 4:0:0, 4:2:2, 4:4:4 | 6.2 | refused by name |
| bit depth > 8, transform bypass | 7.4.2.1.1 | refused by name |
| error concealment of missing slices | — | not attempted |
| SVC / MVC extensions | Annex G / H | NAL types ignored; the base layer decodes |

## The vectors (tests/vectors.txt, tests/conformance.rs)

Each JVT package (ITU-T H.264.1 conformance suite) is a zip with the bitstream and the reference
decoder's decoded YUV; every output frame must equal the YUV's frame byte for byte.

- Baseline (17): BA1_Sony_D, BA2_Sony_F, BA_MW_D, BANM_MW_D, BA1_FT_C, NL1_Sony_D, NL2_Sony_H,
  SVA_BA1_B, SVA_NL1_B, MIDR_MW_D, NRF_MW_E, MPS_MW_A, CI_MW_D, LS_SVA_D, CVPCMNL1_SVA_C,
  MR1_BT_A, BAMQ1_JVC_C.
- Main (14): CABA1_SVA_B, CABA2_Sony_E, CABA3_SVA_B, CABA3_TOSHIBA_E, CACQP3_Sony_D,
  CAPCM1_Sand_E, CABAST3_Sony_E, CVBS3_Sony_C, CVWP1_TOSHIBA_E, CVWP2_TOSHIBA_E, CAWP1_TOSHIBA_E,
  MR2_TANDBERG_E, CVSE2_Sony_B, CVFC1_Sony_C.
- High (7, FRExt/): FRExt1_Panasonic_D, FRExt2_Panasonic_C, HCAFR1_HHI, HPCV_BRCM_A,
  HPCVNL_BRCM_A, HPCADQ_BRCM_B, HPCALQ_BRCM_B.
- Refused by name (4): CAMA1_Sony_C (MBAFF), CVFI1_Sony_D (field pictures), FM1_BT_B (FMO),
  FRExt/Hi422FR1_SONY_A (4:2:2).

The package file names follow the suite's naming but were not listed at this seat (403); a name
that 404s is reported as SKIP by the harness and is the seat's to correct.

## What the fold's `cargo test -p h264_core` should print

- `tests/structure.rs`: 7 tests, all `ok` — `intra16x16_dc_frame_decodes_flat`,
  `pcm_frame_round_trips_and_p_skip_copies_it`, `frames_leave_in_poc_order`,
  `cabac_i_pcm_macroblock_round_trips`, `avcc_samples_decode_like_annexb`,
  `chroma_422_and_field_pictures_are_refused_by_name`, `exp_golomb_and_framing_helpers`.
- `tests/conformance.rs`: 42 tests. Offline (or with the ITU host blocked) every one prints
  `SKIP <path>` and passes. Online, each decode test prints `UNPINNED <path> <sha256>` once and
  then `<path>: N frames bit-exact`, or fails naming the first differing frame, plane and sample;
  the refusal tests print `<path>: refused by name (...)`.
- `libs/gneiss_pal` (`cargo test -p gneiss_pal`): `h264_track_gets_h264_core` and the renamed
  stand-in test (`hvc1`) pass; `handlers/stria/tests/media.rs`
  `codec_without_decoder_plays_the_labelled_stand_in` now uses `hvc1` / `hvcC`.
