# lux

Image decoding for UnaOS userspace: common consumer formats (PNG, JPEG) plus
camera RAW (Sony ARW).

## Overview

`lux` reads an image file from a byte slice and produces a fully decoded RGB
buffer in **linear** floating-point. Three container paths are supported:

- **PNG** and **JPEG** — the common consumer formats — via the established
  decoder crates [`png`](https://crates.io/crates/png) and
  [`zune-jpeg`](https://crates.io/crates/zune-jpeg). Hand-rolling these codecs is
  explicitly not this crate's value; lux wraps them and normalizes their output
  into the shared `RgbBuffer` contract.
- **Sony ARW** (a TIFF/EXIF container) — parsed in-crate: it walks the TIFF
  header and image file directories (IFDs), locates the raw sensor data, decodes
  it to a Bayer plane, demosaics that plane to RGB, and normalizes to linear.

`lux::decode` sniffs the container from its magic bytes and dispatches to the
right path; the per-format entry points (`decode_png`, `decode_jpeg`,
`parse_arw`) are also public.

Because PNG and JPEG store sRGB-encoded samples while `RgbBuffer` is defined as
linear, the common-format decoders convert every sample through the sRGB EOTF
(`lux::color`) so all three paths land in the same linear space.

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

- **`decode(bytes: &[u8]) -> Result<RgbBuffer, LuxError>`** — the format-sniffing
  entry point; dispatches on magic bytes to the PNG, JPEG, or ARW path.
- **`sniff_format(bytes: &[u8]) -> Option<Format>`** — the magic-byte detector
  (`Format::Png` / `Jpeg` / `Arw`).
- **`decode_png(bytes: &[u8]) -> Result<RgbBuffer, LuxError>`** — PNG path.
  Palette/grayscale/low-bit-depth inputs are expanded to 8-bit, 16-bit is
  stripped to 8-bit, alpha is dropped.
- **`decode_jpeg(bytes: &[u8]) -> Result<RgbBuffer, LuxError>`** — JPEG path;
  output forced to interleaved RGB.
- **`parse_arw(mmap: &[u8]) -> Result<RgbBuffer, LuxError>`** — the Sony ARW path.
  Takes the full file bytes and returns a decoded image.
- **`RgbBuffer`** — the decoded result: `width: u32`, `height: u32`, and
  `pixels: Vec<f32>`, a tightly packed linear-RGB buffer (`R, G, B, R, G, B…`).
- **`LuxError`** — the error enum (`Display` + `std::error::Error`):
  `BufferTooSmall`, `InvalidMagic`, `UnsupportedEndianness`, `MissingData`,
  `UnsupportedCompression(u16)`, `UnsupportedCFA`, `CorruptData`,
  `UnknownFormat`, `Decode(String)`.

The module `parser::BayerData` (`Uncompressed` / `Lossless`) is the internal
representation of the single-channel sensor plane handed to the demosaic stage.

## Role in UnaOS

`lux` is a library crate under `libs/` — shared infrastructure, not a handler or
a vessel (see [`docs/dev/USERLAND/ARCHITECTURE.md`](../../docs/dev/USERLAND/ARCHITECTURE.md)).
It provides RAW decode for the imaging side of userspace; the natural consumer is
the **Facet** handler/vessel ("The Canvas", the raster/image surface), which would
take a `RgbBuffer` as the source texture for display and editing.

## Testing

`tests/decode.rs` exercises the decoders end to end against tiny committed
fixtures (`tests/fixtures/`, a few bytes each): a 2×2 RGB PNG (exact linear
round-trip of pure red/green/blue/white), a 2×1 grayscale PNG (expansion to RGB),
and an 8×8 solid-red JPEG (lossy, so channel-dominance rather than exact match).
Additional tests assert every path **fails closed** — returns an error rather
than panicking or reading out of bounds — on empty, truncated, and garbage input,
including an ARW whose header names implausibly large dimensions.

## Status

**PNG + JPEG: supported. ARW: partial.**

- PNG and JPEG decode to linear `RgbBuffer` via `png` / `zune-jpeg`, with
  format sniffing and dispatch (`decode` / `sniff_format`).
- TIFF/IFD parsing, the uncompressed ARW path, the RGGB bilinear demosaic, and
  the `RgbBuffer`/`LuxError` API are implemented. Image dimensions read from tags
  are now fenced (`≤ 512 MP`, non-zero) before any allocation keyed on them, so a
  malformed ARW can no longer drive a multi-gigabyte allocation.
- **ARW2 lossless decoding is unstable.** It is a simplified baseline decoder: it
  assumes a fixed 16-pixel block layout and does not implement Sony's adaptive
  max/min delta tables, so the per-block delta bit-width is not honored. Output
  for genuinely compressed ARW2 files should be treated as approximate and may be
  visibly wrong; the uncompressed path is the reliable one today.
- The Bayer pattern is hard-coded to RGGB and the bit depth to 14-bit; other CFA
  patterns (`UnsupportedCFA`) and bit depths are not yet detected. No RAW formats
  beyond Sony ARW are supported.
