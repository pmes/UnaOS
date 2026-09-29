# WALLPAPER — a desktop background image (rmbp-0929)

**Finding.** Every backdrop fill paints the flat `wm::DESKTOP_BG` (`video/wm.rs:2842`): `stage_fill`
(glass, `wm.rs` ~21407, drained from `drain_deferred`) and `Screen::fill_screen` / `paint_desktop_scene`
(desktop layer, `video/screen.rs:1166/1192`). No wallpaper exists (grep clean). **The brief's PNG
decoder is not in `video/png.rs` (encoder only, `PngEncoder`)**: the decoder is `video/facet.rs`
(`index_chunks`, `IdatSource`, `decode_into`, box downscale `fit`), so the wallpaper rides it and inherits
its gate (`feature = "facet"`; `UNAOS_FACET=1`, no new knob).

**Mechanism.** `facet::decode_file(path, cap=4MB, panel w, h)` (tail of facet.rs) = `open_inner`'s decode half
-> `Vec<u32>`. `video/wallpaper.rs` composes a panel-sized buffer (seeded `DESKTOP_BG` = letterbox colour,
picture centred; larger pictures are box-downscaled by facet's integer factor, never upscaled) and
serves it to (a) `stage_fill` per scanline (same-line folds: `wp` flag, `wp_row`, span source offset),
(b) `Screen::fill_screen(DESKTOP_BG)`/`paint_desktop_scene` (`wm::wp_paint`), (c) `Screen::flush` stale
repaint after a load/off (`wm::wp_take_stale`). Seam helpers are at the `wm.rs` tail; nothing shifts a line.

**M1** default probe `/home/<user>/Desktop/WALL.PNG` then `/WALL.PNG` via `vfs_mount_table()`; runs from
`Screen::flush` (`wallpaper::poll`, 2 s spacing, 5 tries, empty namespace counts) so a late-mounted volume
is tolerated; `login::close_into_session` calls `wallpaper::rearm()` for the session user.
**M2** verb `wallpaper <path>` reloads live (queues a whole-panel desktop erase via `wm::desktop_repaint`,
sets STALE for the desktop layer); `wallpaper off` restores the colour. Registered in `midden_core` HOST_VERBS.
**M3** witness `:: WALLPAPER: src=<path|none> WxH=<w>x<h> scaled=<w>x<h> letterbox=<0|1> ms=<decode> -> PASS ::`
(`-> FAIL` with `reason=` on a bad file). Spec pins in `unaos/scripts/specs/x86-wc.spec`: REQUIRE
`src=none ... -> PASS`, FORBID `-> FAIL`; gate line gains `UNAOS_FACET=1`.

## Written
M1 facet.rs tail, video/wallpaper.rs, wm.rs folds+tail, screen.rs folds, login.rs, video/mod.rs tail.
M2 shell.rs arm, midden_core verb. M3 spec + this doc. Not compiled or run (R76).
