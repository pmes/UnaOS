# lux

Image decoding for UnaOS userspace, with a focus on camera RAW (Sony ARW).

## Overview

`lux` reads an image file from a byte slice and produces a fully decoded,
demosaiced RGB buffer. The current implementation targets the Sony ARW format
(a TIFF/EXIF container). It parses the TIFF header and image file directories
(IFDs), locates the raw sensor data, decodes it to a Bayer plane, demosaics that
plane to RGB, and normalizes the result to linear floating-point values.

The crate is `#![no_std]`-free host code: it relies on `std`, uses
[`rayon`](https://crates.io/crates/rayon) to parallelize the demosaic step, and
declares [`memmap2`](https://crates.io/crates/memmap2) so callers can feed a
memory-mapped file directly as the input slice.

## Responsibilities

- **Container parsing.** Read the TIFF header (`II`/`MM` endianness marker, magic
  `42`, first-IFD offset) and walk up to ten IFDs, extracting `ImageWidth` (256),
  `ImageLength` (257), `Compression` (259), `StripOffsets` (273), and
  `StripByteCounts` (279). IFD traversal is bounded and offsets are range-checked
  to reject corrupt files.
- **Raw decoding.** Two compression paths are handled:
  - *Uncompressed* (`Compression == 1`): the raw strip is wrapped zero-copy as a
    little-endian 16-bit-per-pixel slice.
  - *ARW2 lossless* (`Compression == 32769`): a baseline block decoder reads the
    Sony bit-stream (4-bit table index, 11-bit base value, 7-bit signed deltas).
- **Demosaic.** `demosaic_bilinear` reconstructs three color channels per pixel
  using bilinear interpolation, assuming an RGGB Bayer pattern. The pass is
  parallelized per row with Rayon.
- **Normalization.** Output samples are scaled by the 14-bit maximum (`16383.0`)
  into linear RGB `f32` in the range `0.0..=1.0`.

## Public API

- **`parse_arw(mmap: &[u8]) -> Result<RgbBuffer, LuxError>`** — the entry point.
  Takes the full file bytes and returns a decoded image.
- **`RgbBuffer`** — the decoded result: `width: u32`, `height: u32`, and
  `pixels: Vec<f32>`, a tightly packed linear-RGB buffer (`R, G, B, R, G, B…`).
- **`LuxError`** — the error enum (`Display` + `std::error::Error`):
  `BufferTooSmall`, `InvalidMagic`, `UnsupportedEndianness`, `MissingData`,
  `UnsupportedCompression(u16)`, `UnsupportedCFA`, `CorruptData`.

The module `parser::BayerData` (`Uncompressed` / `Lossless`) is the internal
representation of the single-channel sensor plane handed to the demosaic stage.

## Role in UnaOS

`lux` is a library crate under `libs/` — shared infrastructure, not a handler or
a vessel (see [`docs/dev/USERLAND/ARCHITECTURE.md`](../../docs/dev/USERLAND/ARCHITECTURE.md)).
It provides RAW decode for the imaging side of userspace; the natural consumer is
the **Facet** handler/vessel ("The Canvas", the raster/image surface), which would
take a `RgbBuffer` as the source texture for display and editing.

## Status

**Partial — early implementation.**

- TIFF/IFD parsing, the uncompressed path, the RGGB bilinear demosaic, and the
  `RgbBuffer`/`LuxError` API are implemented.
- **ARW2 lossless decoding is unstable.** It is a simplified baseline decoder: it
  assumes a fixed 16-pixel block layout and does not implement Sony's adaptive
  max/min delta tables, so the per-block delta bit-width is not honored. Output
  for genuinely compressed ARW2 files should be treated as approximate and may be
  visibly wrong; the uncompressed path is the reliable one today.
- The Bayer pattern is hard-coded to RGGB and the bit depth to 14-bit; other CFA
  patterns (`UnsupportedCFA`) and bit depths are not yet detected. No formats
  beyond Sony ARW (e.g. other vendors' RAW, or common consumer formats) are
  supported.
