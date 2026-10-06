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
- **Sony ARW** (a TIFF/EXIF container) — RAWCORE (B444): read by `raw_core`
  (`unaos/libs/media/raw_core`, the `no_std` core the kernel links through
  pixel_core) and re-exported by `lux::parser`: the TIFF/IFD walk, the raw strip
  (uncompressed, or Sony cRAW — Compression 32767, 8 bits), a bilinear demosaic,
  normalized to linear.

`lux::decode` sniffs the container from its magic bytes and dispatches to the
right path; the per-format entry points (`decode_png`, `decode_jpeg`,
`parse_arw`) are also public.

Because PNG and JPEG store sRGB-encoded samples while `RgbBuffer` is defined as
linear, the common-format decoders convert every sample through the sRGB EOTF
(`lux::color`) so all three paths land in the same linear space.

The crate is `#![no_std]`-free host code: it relies on `std` (the ARW path is
raw_core's, single-threaded since RAWCORE), and declares [`memmap2`](https://crates.io/crates/memmap2) so callers can feed a
memory-mapped file directly as the input slice.

## Responsibilities

- **ARW** — `raw_core` owns the container, the strips, the preview, the EXIF
  facts and the demosaic (see its crate docs and
  `docs/dev/evidence/rmbp-1005/rawcore.md`); `lux::parse_arw` adapts its
  mosaic to the linear-f32 `RgbBuffer`.
- **PNG / JPEG** — the `png` and `zune-jpeg` crates, converted to linear
  through `lux::color` (chicken wire under R83 until pixel_core replaces them).

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

**PNG + JPEG: supported. ARW: through raw_core (RAWCORE, B444).**

- PNG and JPEG decode to linear `RgbBuffer` via `png` / `zune-jpeg`, with
  format sniffing and dispatch (`decode` / `sniff_format`).
- ARW: uncompressed and Sony cRAW (Compression 32767, 8 bits) strips, the
  bilinear demosaic, dimensions fenced (512 MP) before any allocation — all in
  raw_core, proven on a SYNTHETIC ARW until a real file is committed. Owed: the
  colour matrix, white balance, lossless-JPEG raw (Compression 7), ARW1.
