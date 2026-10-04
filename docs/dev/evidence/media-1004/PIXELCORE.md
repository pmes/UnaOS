# PIXELCORE — the still-image decoders UnaOS owns (ledger SR25)

Branch `exec-media-pixel`, cut from `0686cc1b`. Crate `unaos/libs/media/pixel_core` (`no_std` + `alloc`,
`#![forbid(unsafe_code)]`, **zero dependencies**), a root-workspace member and a kernel path dependency.

## Finding (M1)

The brief called `unaos/crates/kernel/src/video/png.rs` "the kernel's from-scratch decoder". It is not: it
is SHOTZIP, the screenshot **encoder** (fixed-Huffman deflate + its own streaming self-check inflater). The
kernel's PNG **decoder** is the decode half of `video/facet.rs` (a streaming, scale-while-decoding row
sink built for a 48 MiB heap), and its inflater is `selfhost/inflate.rs`. So M1 did this:

* `selfhost/inflate.rs` (RFC 1950/1951/1952, 664 lines) MOVED verbatim to `pixel_core/src/inflate.rs`; the one
  edit is its CRC-32 source (`crate::hash::Crc32` → `pixel_core::crc::Crc32`, same table, same API, KAT'd
  against the catalogue check value `0xCBF43926`). The kernel file is now `pub use pixel_core::inflate::*;`.
  ONE inflater: SELFHOST-2's tar walk, Facet, and pixel_core's PNG decoder all run it.
* `video/png.rs` (SHOTZIP encoder) MOVED byte-for-byte to `pixel_core/src/png/encode.rs`; the kernel file is
  `pub use pixel_core::png::encode::*;` — `prtscr` and `facet` callers did not move. (Its private
  fixed-Huffman `Verify` is the encoder's self-check, not a decoder; it stays inside the encoder.)
* A NEW full PNG decoder `pixel_core/src/png/decode.rs`, written from ISO/IEC 15948, using that inflater.
* Facet (charter correction: Images are Facet's domain): `open_inner` now sends any file that is not a PNG
  through `pixel_core::decode` (read whole, ≤ 16 MiB, box-reduced into the same base image, alpha dropped as
  the PNG path drops it; an animation shows frame 0). The PNG path is unchanged (still streaming).

Licence note: the two moved files keep their `GPL-3.0-or-later` headers; the crate's new files are
`LGPL-3.0-or-later` like every other shared core. Same copyright holder; Peter's call whether to relicense
the two moved files.

Kernel metal leg (x86, the brief's feature list) — `cargo +nightly check --release …` from
`unaos/crates/kernel`: rc=0.

## Oracle method

`pixel_core/oracle/chromium-oracle.cjs` (Playwright + the pre-installed Chromium, `--force-color-profile=srgb`)
renders each file as `<img>` at 1:1, `margin:0`, on a known background (`rgb(0,128,255)`), and screenshots
exactly the image rectangle. PNG inputs are first copied with gAMA/cHRM/sRGB/iCCP removed (pixel_core does not
colour-manage; documented). `tools/pixel-check --compare` composites pixel_core's RGBA over the same
background with Skia's rounding (premultiply `c·a/255` rounded, plus `bg·(255−a)/255` rounded) and reports
max abs diff, % exact channel bytes, and PSNR (RGB). `oracle/run-oracle.sh` drives both.

Second opinion in `cargo test`: the `image` crate (dev-dependency only, never linked into the library) decodes
the same KAT files and the tests compare byte-for-byte.

## M1 — PNG

Spec sections: §5.2 signature; §5.3 chunk layout + CRC (critical chunk CRC error = refuse; ancillary = drop the
chunk); §11.2.2 IHDR, all 15 legal colour-type/bit-depth pairs; §11.2.3 PLTE; §11.3.2.1 tRNS (types 0, 2, 3);
§10 + RFC 1950/1951 IDAT; §9 all five filters, Paeth tie order as written; §8.2 Adam7. 16→8 bit by the
correctly rounded `v/257` (§13.12); sub-byte grey by exact scaling.

KATs: PngSuite (117 files: basic `basn*`, interlaced `basi*`, sizes `s01..s40`, filters `f00..f04,f99`,
transparency `tb*/tp*/tm3`, zlib levels `z00..z09`, IDAT split `oi1..oi9`, and the 14 corrupt `x*`) —
**103/103 valid files byte-identical to the `image` crate; 14/14 corrupt files refused.** Plus an encoder →
decoder round trip (SHOTZIP output decodes to its input).

Chromium oracle, the same 103 files: **86 exact (max abs diff 0, 100 % bytes)**; the 17 16-bit files differ
by at most 1 (49.7–96.7 % exact, PSNR ≥ 51 dB) — proven to be Chromium reducing 16→8 by truncation (`v>>8`)
where the spec rounds: rebuilding with `v>>8` made those 17 exact too (103/103). pixel_core keeps the spec's
rounding.

Not decoded: gAMA/cHRM/sRGB/iCCP colour management (samples returned as stored); sBIT (advisory); APNG
(`acTL/fcTL/fdAT`) — an APNG decodes as its default image. Inflate is the audited bit-at-a-time "puff"
decoder — correct, not fast; a table-driven fast path is owed.

## M2 — JPEG

Spec sections (ITU-T T.81): B.1 marker syntax (fill bytes, stuffing); B.2.4.1 DQT 8/16-bit; B.2.4.2 DHT +
Annex C code generation; B.2.2 SOF0 baseline, SOF1 extended Huffman (8-bit), SOF2 progressive; B.2.3 SOS with
A.2 MCU/block order (interleaved and non-interleaved); B.2.4.4 DRI + F.1.2.3 RSTn; F.2.2 sequential Huffman;
G.1.2 progressive — DC first/refine, AC first/refine with EOB runs (spectral selection AND successive
approximation); sampling factors 1..4 with 4:4:4, 4:2:2, 4:2:0 proven. Annex K.3 tables pre-installed in
slots 0/1 (Motion-JPEG frames carry no DHT). JFIF/T.871 YCbCr; Adobe APP14 RGB/CMYK/YCCK; EXIF 2.3 IFD0
orientation read into `Image::orientation`, applied by `Image::apply_orientation` (all eight, unit-tested).

Reconstruction, chosen to match the IJG reference Chromium builds on (libjpeg-turbo):
IDCT = IJG **islow** (integer LLM, 13-bit constants, PASS1_BITS=2, rounding descale, clamp) — the IEEE 1180
accuracy class, bit-exact with libjpeg-turbo's C/SIMD islow; chroma = libjpeg **fancy** upsampling (h2v1,
h2v2 triangle filters, h1v2 with biases 1/2; other ratios replicated); colour = libjpeg's 16-bit fixed-point
YCbCr→RGB; inverted-Adobe CMYK → RGB as `C·K/255` truncated (Blink's formula).

KATs (30 files: libjpeg-turbo `testorig/testimgint/testimgari`, EXIF `Landscape_6`/`Portrait_8`, image-rs
progressive `cat/3/test`, jpeg-decoder reftests `restarts` (DRI 5), `mjpeg` (4:2:2, DRI, no DHT), `rgb`
(Adobe RGB), `ycck`, `16bit-qtables`, `extraneous-data`, Mozilla's `jpg-*` (gray, CMYK ×2, progressive, ICC,
sizes 1..33), `grumpycat` (4:4:4)):

* **Chromium oracle: 28/28 decodable non-ICC files EXACT — max abs diff 0, 100 % bytes, PSNR ∞** — including
  the photos (Landscape_6 1800×1200 rotated, Portrait_8, progressive3 650×470, cat, rgb, grumpycat, mjpeg
  960×720), far past the brief's ≥ 45 dB bar. These 28 are pinned as CRC-32 digests in
  `tests/oracle_digests.txt`, so `cargo test` holds pixel_core to Chromium's answer offline.
* `ycck.jpg` decodes structurally right but scores 20.4 dB: it carries a CMYK ICC profile (18 APP2 chunks)
  that Chromium applies and pixel_core does not. ICC is the ceiling, not a decode bug.
* `testimgari.jpg` (arithmetic) is refused by name; a truncated file is refused.
* Second opinion (`image` crate / zune-jpeg, a different IDCT and upsampler): 26 files, PSNR 44.9–71 dB.

Not decoded: arithmetic coding, lossless, hierarchical, 12-bit, DNL; ICC profiles not applied; a truncated
progressive file is refused rather than shown at its partial quality.
