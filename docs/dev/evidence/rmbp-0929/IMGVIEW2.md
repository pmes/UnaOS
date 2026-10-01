# IMGVIEW2 — facet: fit, zoom, pan, browse, trash, info, Adam7, in-window refusals (R75)

## Finding
`video/facet.rs` decoded straight into a box-downscaled window surface (integer k, never upscale), held no image, and had no key, wheel or drag route: only `press_route` (close box + raise). A refused file printed `[facet] refuse` and showed nothing. Decoder breadth already covers depth 1/2/4/8/16, colour types 0/2/3/4/6 and all five filters; the one gap is Adam7 (`parse_ihdr` returned `Interlaced`).

## Mechanism
- Zoom and pan need pixels to re-sample, so the window keeps a BASE image (`View.px`, 0xFFRRGGBB): the file decoded by the existing `RowSink` at `fit(w,h,2048,1536)` (k=1 up to ~3.1 Mpx, 12 MB; larger files stay box-reduced and the title percent is relative to the source). The window surface (viewport) is a separate buffer re-rendered from the base on every change: nearest for zoom >= 100 %, area average below.
- Fit-on-open: viewport = base scaled by min(1200/bw, 800/bh, 100 %), floor 32x32; letterbox colour `BG`.
- Input: `facet::key_route` is chained from `quarry::live::key_route` (same seam as fileview). It only QUEUES a `Cmd` (router stack is 16 KiB, see `request_open`); `service()` (already drained from the render pass) applies it. Wheel is taken only when the pointer hit-tests onto the facet window; drag = press latches an anchor, `service()` polls `pal::cursor::button_down/pos`.
- Browse: `read_dir` of the shown file's folder, `.PNG` only (no JPEG decoder exists in the tree), sorted case-insensitively, wrapping. Nothing is preloaded. Delete = `fs::trash::trash`, then the next image (or close).
- Adam7: `Adam7Sink` (seven passes, per-pass unfilter, scatter into the k=1 base); refused by name over the 3.1 Mpx base cap.
- Refusal: `show_message` mints a small window carrying the file name and reason; `[facet] decode refused reason=` on the wire.

## Milestones
M1 base + viewport + fit + zoom/pan + title. M2 browse, Delete, `i` info. M3 Adam7, message window, `tests imgview`.

## Witness
`:: IMGVIEW: path= WxH= zoom= fit= browse_n= colour=rgb|rgba|pal|gray -> PASS ::` on every successful open and from `tests imgview` (hand-built 64x64 RGBA and 16x16 palette PNGs under `/home/<user>/`, open, zoom, browse, close, unlink). x86-wc lane has facet on (`UNAOS_FACET=1` in its gate header): REQUIRE/FORBID pinned.

## Written
M1-M3 in one commit (the milestones share the View state). Boot 17 should show, after a Quarry double-click on a PNG, `[facet] open ... -> DECODING`, `[facet] decoded ... inflate=OK`, `[facet] present`, `:: IMGVIEW: path=... WxH=... zoom=... fit=... browse_n=... colour=... -> PASS ::`; and from `tests imgview`: `:: IMGVIEW: path=/home/<user>/IVTESTA.PNG WxH=64x64 zoom=150 fit=1 browse_n=N colour=rgba pal=1 refusal-window=1 -> PASS ::` (plus the open-time line for each file it opens, and one deliberate `[facet] decode refused reason=not-png` for the junk file). Also `tests` FACETPNG now includes an Adam7 leg.
Deviations: title uses ASCII ` - ` not an em dash (the bitmap font has no U+2014); JPEG is not browsed (no decoder in the tree); zoom % is relative to the base image (equals the source up to ~3 Mpx).
