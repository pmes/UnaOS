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

## M3 — GIF, BMP, QOI

A second oracle mode lands for animation: `oracle/chromium-frames.cjs` runs Chromium's WebCodecs
`ImageDecoder` over the file, draws every (composited) frame to a canvas and reads it back with
`getImageData` — Chromium's own RGBA per frame, no screenshot; `pixel-check --compare-raw` diffs all four
channels of every frame (fully transparent pixels compare equal whatever their colour bytes).

**GIF** (GIF89a; 87a too): §17–18 header/LSD, §19/§21 colour tables, §20 image descriptor + Appendix E
interlace, §22/Appendix F LZW (code growth to 12 bits, Clear, EOI, deferred clear, KwKwK), §23 GCE (disposal
0–3, transparency, delay), NETSCAPE2.0/ANIMEXTS1.0 loop count. Composition as the browsers do it: canvas starts
transparent (background colour index ignored), disposal 2 clears the rectangle to transparent, disposal 3
restores the pre-frame canvas; a frame with short LZW data keeps the pixels it delivered.
Vectors: the pygif test-suite (disposal ×4, animation ×5, loop ×5, interlace, transparency, LZW edge cases
255/4095/large/max codes, no-clear, extra/missing pixels, plain-text, comment, 87a, LCT…) + image-rs samples
(1000×1000 two-frame animations, alpha) — 46 files.
**Chromium frame oracle: 40/40 files Chromium decodes → every frame exact (62 frames, max abs diff 0).**
Divergences, all by Chromium refusing: `invalid-code.gif` (pixel_core keeps the pixels before the bad code),
`max-codes.gif` (a legal deferred-clear stream; pixel_core decodes it), `image-zero-size.gif`; `no-data`,
`zero-width`, `zero-height` are refused by both.

**BMP**: core/info/V2–V5/OS2 headers; 1/4/8-bit palettes; 16-bit X1R5G5B5 and BI_BITFIELDS (565); 24-bit;
32-bit BI_RGB (opaque) and BI_BITFIELDS/BI_ALPHABITFIELDS with alpha; bottom-up and top-down; RLE8/RLE4
(skipped pixels transparent); BI_PNG/BI_JPEG delegated to our own decoders. Narrow mask channels widen by
`round(v·255/(2ⁿ−1))` (Blink's table — bit replication was ±1 off on rgb16, the oracle caught it).
Vectors: 21 files (bmpsuite subset + the Core/Info/V4/V5 set). **Chromium: 21/21 exact**, plus four
generated top-down variants (rgb24, rgb32bf, pal8v5, rgb16-565) **4/4 exact**; `cargo test` re-derives a
top-down variant of every uncompressed vector and requires an identical decode. Image crate: 20/20 identical.

**QOI** (spec 1.0, all six ops, index hash, run, end marker): exact by construction. qoiformat.org's test
zip is not reachable from the build proxy, and Chromium does not decode QOI, so the oracle is transitive:
all 103 PngSuite images re-encoded by an independent QOI writer (the `image` crate's `qoi`) decode
byte-identical to their PNG source — whose decode is itself Chromium-exact.

Pinned digests now cover 89 Chromium-exact files (28 JPEG, 40 GIF incl. every frame, 21 BMP).

Not decoded: GIF Plain Text rendering (no browser renders it either); BMP 2/64-bit, OS/2 Huffman/RLE24, V5 ICC.

## M4 — WebP lossless (VP8L) and `tools/pixel-check`

RFC 9649: §2 RIFF (simple `VP8L` and extended `VP8X` files), §3.2 header, §4 all four transforms (predictor:
14 modes + border + rightmost-TR rule; colour: signed 3.5 deltas; subtract-green; colour indexing with
palette delta coding and 1/2/4-bit bundling), §5.2.2 LZ77 with the 120-entry distance map and prefix-coded
lengths/distances, §5.2.3 colour cache, §6.2.1 simple + normal prefix codes (code-length code, `max_symbol`),
§6.2.2 meta prefix codes / entropy image.

KATs: 9 VP8L files from image-rs/image-webp (libwebp gallery2 lossless+alpha ×5, palette 1/2/4-bit ×3,
`color_index`) — **9/9 byte-identical to image-webp** (lossless has exactly one right answer); `lossy` VP8 is
refused by name. Chromium: the 3 opaque palette files exact; on the 6 translucent ones every OPAQUE pixel is
exact and translucent pixels differ by ≤ 1 after compositing (Chromium decodes WebP to premultiplied
storage with libwebp's fast multiply; the frame oracle's canvas readback shows the same rounding).
Plus 4 lossless files written by Chromium's own encoder (`toDataURL('image/webp', 1.0)`) from opaque
sources — **4/4 exact against Chromium's decode**; three (< 200 KB each) are committed under
`tests/fixtures/webp/` and pinned. The one inflater also gets its own KAT: CPython gzip/zlib level-9 streams
inflate to the exact text, a flipped trailer is refused (`TrailerMismatch`/`AdlerMismatch`).

`tools/pixel-check` (pixel_core only, no other crate): `pixel-check <in> [out.png]` decodes anything to an
RGBA PNG for eyes; `--frames` writes each composited frame; `--compare` / `--compare-raw` score the two
Chromium oracles; `--digest` prints the pinned KAT digest.

Not decoded: lossy VP8 (RFC 6386 intra decoder + ALPH) — OWED; animation (ANIM/ANMF) — OWED.

## M5 — the host face (`gneiss_pal::dsp::image`) and Aether behind it

* `libs/gneiss_pal/src/dsp/{mod,image}.rs`: Gneiss's DSP module (CODEX §2: "Audio and Video codecs") gets
  its still-image face — `gneiss_pal::dsp::image` re-exports `pixel_core` whole (`decode`, `sniff`, the
  per-format `decode_*`, `Image`/`Frame`/`Format`/`Error`, `MAX_DIM`/`MAX_PIXELS`). No wrapper types, no
  second copy: the host and the kernel run the same code. `gneiss_pal` depends on `pixel_core` by path.
* Aether (`handlers/aether`): every raster decode — page `<img>`s in `net::fetch_page`, `data:` URIs, the
  favicon — goes through ONE function, `images::decode_raster`, which calls `gneiss_pal::dsp::image::decode`
  and applies the EXIF orientation (CSS `image-orientation: from-image`, the browser default). The
  third-party crates sit behind it, each behind a feature (all three on by default):
  * `pixel-core` — UnaOS's decoders first. Off = the `image` crate in front for everything (reversible).
  * `image-fallback` — enables the `image` crate's format decoders (`image/default-formats`), consulted ONLY
    for what pixel_core refuses: lossy WebP, ICO (favicons), TIFF, AVIF, the formats pixel_core has not
    reached, or a file it calls malformed. Without it the `image` crate is compiled with only its `png`
    codec: the `RgbaImage` container the renderer holds and the PNG writer `headless` screenshots use.
  * `svg` — `resvg` (SVG is vector, not pixel_core's domain). Without it `decode_svg` is a miss.
* R83 bumps: `image` pinned to `0.25.10` (the latest stable on crates.io, 2026-10-04; it was `"0.25"`,
  default features — now `default-features = false`, `rayon` dropped); `resvg` `0.47` → `0.48.1` (latest
  stable).
* Kernel: Facet's viewer already calls `pixel_core::decode` (M1, `video/facet.rs` `decode_foreign`; an
  animated GIF shows frame 0) — the one call site, unchanged here. The PNG path stays the streaming
  scale-while-decoding row sink (48 MiB heap: a PNG is never materialised), on the same inflater. EXIF
  orientation is NOT applied in the kernel viewer yet: `Image::apply_orientation` allocates a second full
  RGBA buffer infallibly, which the kernel heap must not risk; a fallible/in-place rotate is owed.
* Sibling arc: `handlers/facet` (FACET, ledger SR29, branch `exec-host-facet`) is the host Images handler
  CODEX §2 charters; it builds against THIS `gneiss_pal::dsp::image` shape (the `pixel_core` re-export, not
  a wrapper), so a change to that surface is a change to its contract.
* Fold note: `libs/gneiss_pal/src/dsp/mod.rs` and the `pub mod dsp;` line in `gneiss_pal/src/lib.rs` are
  also created by the sibling media arcs (PLAYBACK: `avsync`, `video`, `demux`; the audio arc: `audio`).
  The merge is a union of `pub mod` lines; this arc owns only `pub mod image;` and `dsp/image.rs`.
* Proof: `cargo test --release -p pixel_core -p aether` — rc=0, 98 passed / 0 failed (aether 84 incl.
  the engine image tests that encode PNG/JPEG fixtures and decode them through `decode_raster`; pixel_core
  14 across the PNG/JPEG/M3/WebP/inflate KATs and the 89 pinned Chromium digests).
* Kernel metal leg (x86, `cargo +nightly check --release --target ../../x86_64-unaos.json -Z build-std=… -Z
  json-target-spec --features "wc,quarry,ftdirx,login,loginst,nvidia-kepler-vblank,smc,usbnet,hda,hda-tone,
  facet,beam,sdw,selfhost,linuxabi,ahci,unafs,busreg"` from `unaos/crates/kernel`, at 333bb155): **rc=0**.
  (The brief's `kepler_vblank` is spelled `nvidia-kepler-vblank` in the kernel's Cargo.toml.) Targets deleted.
