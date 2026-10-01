# SHOTREGION (R75) — region and window capture

**Finding.** `prtscr.rs` captures only the whole panel (`:: PRTSCR: SCREEN0.PNG 2880x1800 … chunks= ::`); `Action::ScreenshotRegion`
(`⌘⇧4`, `keymap.rs`) existed but `is_capture()` made the decoders arm the same whole-panel capture. No `⌘⇧5`, no selection, no overlay.

**Mechanism.**
- `video/shotsel.rs` (new): modes REGION/WINDOW, `route`/`route_at` (router seam), `motion`/`motion_at`, `commit` -> `prtscr::request_rect`.
- Overlay = the SPLASHX86 pattern: `wm::splash_open` chromeless row + `set_modal_top`; surface is a dimmed snapshot with the clear rect (white border) cut back; drag repaints only changed pixels.
- Router: `wc_route_event` asks `shotsel::route` first; `wc_route_tail` calls `shotsel::motion` (`arch/x86_64/syscall.rs`).
- Keys: `Action::ScreenshotWindow` (`⌘⇧5`, usage 0x22) added in `keymap.rs`/`theme.rs`/`clipboard.rs`; `is_capture()` no longer claims the region chord on x86 `wc` (still does elsewhere, so the Pi/Orin are unchanged).
- Crosshair: `cursor::set_crosshair` switches `sprite_color`'s mask.
- Capture: `prtscr` gains `RECT_*` statics, `request_rect`/`set_rect`/`take_rect`/`disarm`; `Job` carries `kind,rx,ry` and reads `read_pixel(rx+x, ry+y)` into the same streaming encoder; `Shot` carries kind/rect.
- Name (M3): `shot_name` — `SHOT-HHMMSS.PNG` via `clock::try_unix_now`+`civil_from_unix` when free, else `SHOT<n>.PNG`; directory is the unchanged PRTSCR-DIR-FIX plan (`/home/<user>/Desktop`); `finish` posts `crystal::login::notice_show` for kind != panel.
- `wm::frame_at` (tail of `wm.rs`): topmost window frame under a point, kernel rows included, overlay skipped.
- Verb: `shot region|window` (x86 `wc`, folded onto the `"screenshot"` arm line in `shell.rs`).

**Milestones.** M1 region (chord, verb, crosshair, overlay, release capture, Esc). M2 window. M3 name + notice.

**Witness.** `:: PRTSCR: … bytes -> OK :: … chunks=N kind=panel|region|window rect=x,y,WxH ::` (appended to the existing second segment; first segment and `bytes -> OK ::` untouched).
`:: SHOTREGION: region_ok=1 window_ok=1 cancel_ok=1 region_file= window_file= panel=WxH -> PASS ::` from `tests shotregion`. Also `:: SHOTSEL: begin|overlay open|commit|cancel … ::`.

**Pins.** `x86-wc.spec`: REQUIRE `:: SHOTREGION: region_ok=1 window_ok=1 cancel_ok=1 .* -> PASS ::`, FORBID `-> FAIL`. PRTSCR-DIR-FIX / PRTSCR-REFUSE rows untouched (their lines come from fixtures not changed).

## Written
All three milestones, uncompiled (R76). Boot 17: `tests shotregion` must show the PASS line above; by hand, `⌘⇧4` drag then `ls ~/Desktop` shows `SHOT-HHMMSS.PNG` (or `SHOTn.PNG` while the clock is unanchored) and the NOTICE names it. Cost to watch: the overlay's panel snapshot (`snap_ms=` on `:: SHOTSEL: overlay open`) reads 2880x1800 pixels through `read_pixel`.
