# RAWCORE (rmbp-ledger B444) — the Sony raw handler, one decoder for both rings

**Finding (tip 82319dd6).** `libs/lux` parses ARW on the host only (std: rayon, memmap2) and nothing links it.
Its "ARW2 lossless 4/11/7" decoder is not Sony's format: Sony's compressed ARW is `Compression 32767` with
`BitsPerSample 8` — the cRAW block of dcraw's `sony_arw2_load_raw` (16 bytes carry 16 same-colour pixels of a
32-column span: 11-bit max, 11-bit min, two 4-bit positions, fourteen 7-bit deltas shifted by the span, then the
tone curve of tag 28688). `Compression 32769` is not a Sony code at all; `Compression 7` (lossless JPEG, the
"Lossless Compressed RAW" bodies) is. The kernel's filetype knows no raw type; pixel_core cannot open a TIFF;
Facet refuses it; Quick Look shows a card. The card's exFAT label already names the volume (EXFAT B392).

**The seam (R79, R83).** `unaos/libs/media/raw_core` — `no_std` + alloc, no dependencies, forbid(unsafe):
the TIFF container (both byte orders, bounded IFD walk with a visited set: IFD0 chain, SubIFDs, EXIF IFD), the
raw strip (uncompressed 16-bit containers; cRAW 32767/8 with the curve; packed 12-bit 32767/12 is owed), the embedded
JPEG preview (513/514 in any IFD, the largest that starts `FF D8`), the EXIF facts, a bilinear demosaic and a
streaming binned developer (for a window smaller than the sensor: one row of u16 and one accumulator row,
never the whole mosaic) to pixel_core's surface (straight RGBA8, sRGB through the EOTF of lux's color.rs,
inverted by a table — no libm). pixel_core gains the `raw` module (decode / decode_preview / mime / facts) that
calls it; the preview goes through pixel_core's own JPEG decoder. lux's `parse_arw` becomes a re-export
adapter (API unchanged, rayon dropped). The kernel reaches raw_core only through pixel_core.

**Milestones.**
- M1 — raw_core: container, IFDs, strips (1, 32767/8 cRAW, 32767/12 packed), preview, EXIF facts, demosaic,
  developer; host KATs on a SYNTHETIC ARW the test writes (no real Sony file on this machine).
- M2 — pixel_core `raw` route (FAST = embedded preview, FULL = demosaic), `mime_of`/`facts_of` know TIFF/ARW;
  lux re-exports the core.
- M3 — kernel: `fs::filetype` types `image/x-sony-arw` / `image/tiff` (sniff through pixel_core, `.arw`
  `.tif` `.tiff` in the table), assoc rows → facet, Facet's app.res declares both; ATTRCOLUMNS writes
  `media:camera media:lens media:exposure media:iso media:focal_mm media:taken` beside width/height; Facet
  opens a raw by streaming the strip through the developer (witness below), Quick Look/wallpaper take the
  preview; `tests rawcore`.

**Witness.** Open: `[facet] raw path=<p> w=<n> h=<n> compression=<n> preview=<ok|none> demosaic_ms=<n>`.
Test: `:: RAWCORE: tiff=ok ifds=<n> preview=<ok> demosaic=<ok> facts=<n> -> PASS ::`. With the card in:
`[volumes] mounted … fs=exfat` (EXFAT's), `[quarry] open … type=image/x-sony-arw -> facet`.

**Owed.** Every number above is from the synthetic file until Peter's first real ARW: the colour matrix
(camera RGB to sRGB per model), white balance (maker note / 0x7303 WB_GRBGLevels), Sony's lossless JPEG
(`Compression 7`), the A100's ARW1 (32767 with 12-bit Huffman) — bilinear with no matrix and no WB is a FLAT,
greenish first image. A FAT/exFAT card takes no attributes, so the `media:*` facts land only on UnaFS copies.
Icon-view thumbnails (no icon view draws any picture yet): `pixel_core::raw::decode_preview` is the call.

**Built (e320b590).** Host: `cargo test -p raw_core -p lux -p pixel_core` exit 0 (raw_core 6 KATs, pixel_core
raw_kat 2, lux unchanged and passing). Kernel: x86 metal shape exit 0; aarch64 `login,loginst,virt_el0,lumen,desktop_firmware,quarry,facet,usbnet`
exit 0; aarch64 `tegra,…` exit 101 = No space left on device (shared disk), twice. No knob: the route rides `facet`.
**Bytes wanted from Peter's card:** the first 64 KiB of one ARW (IFD0, the raw SubIFD, the EXIF IFD, the curve/
black/white tags, the preview's start) for the container KATs, and one whole ARW (the smallest — a compressed
one, ~25 MB) for the decoder and demosaic KATs; its body model names the colour matrix owed.
