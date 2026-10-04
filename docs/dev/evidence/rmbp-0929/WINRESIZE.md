# WINRESIZE (R75) — resize a window by its frame

## Design

**Finding.** Windows move (title-bar drag, `wm::drag_begin`/`drag_motion`, `wm.rs`) and zoom
(`wm::zoom`, WINTITLE double-click) but nothing resizes. Confirmed by grep: no `resize`/`set_geom` seam in
`video/wm.rs` (the only `resize` hits are `Vec::resize` on staging buffers); `ctrlgeom` in
`arch/x86_64/syscall.rs` is the control-cluster *geometry* leg of `[wm-act] direct`; `VUGRES` is the vug
*resume* witness. There is no una-abi resize event (events 1..8 in `una-abi/src/lib.rs`). A row's
`w`,`h` are fixed at `wm::create`; `stride`/`surf_len` bound what the owner's slot can hold.

**Mechanism.**
- Zones: `wm::resize_zone_at` / `rs_zone_of` — outer box minus content box (the app is never starved,
  the `chrome_hit` law), edges `RS_EDGE`=6, corners `RS_CORNER`=12 panel px. Effective side/bottom depth
  is the 5 px frame (clipped to chrome). The L/R bands stop at the title strip so the strip stays the
  drag handle (and the `[wm-act] direct` grab point is unchanged); only its corners resize.
- Press: `wc_resize_press` (syscall.rs tail), folded onto the `chrome_hit` line of `wc_click_route_at`,
  after `control_hit` (discs win). Begins via `wm::resize_begin`, which publishes the SAME `DRAG_WIN`
  state, so the release arm, level belt, `<TAB>` cancel, row-recycle guards all end it unchanged.
- Motion: `drag_motion` diverts to `resize_motion` while `RS_ZONE != 0`; `rs_solve` (pure) does zone
  deltas, min (`RS_MIN_W`=160 source px so the cluster fits, `RS_MIN_H`=64), cap (slot: `stride/4` x
  `surf_len/stride`), panel-edge clamp (right/bottom/work-top) and Shift aspect (`keymap::shift_held`,
  fed by `note_mods` from the xhci chord resolver). `resize_to_inner` sets `x,y,w,h`, damages and repaints
  with `move_to_inner`'s vacate/erase tail.
- Telling content: ring-3 gets `una_abi::INPUT_EV_WIN_RESIZE` (=9, payload w<<16|h, source px) via
  `user_input_push_owner`; kernel windows have no ring and re-layout from `wm::info` on next paint.
- Keys (M3): `Action::WinNudge*`/`WinSize*` rows, **Ctrl** and Ctrl+Shift + arrows, first in both tables
  (Cmd+arrows are already the terminal caret / selection, so Cmd is NOT used); routed in `wc_focus_key`
  to `wm::win_key` (16 px).
- Witness: `[wm-act] resize win= from=WxH to=WxH zone= owner=` per gesture end (`resize_finish`, from
  `drag_end` and `drag_cancel`); `:: WINRESIZE: zones=8 strip= min=WxH drags= clamped= ... -> PASS ::`
  from `tests winresize` (`winresize_selftest`).
- Spec: `x86-wc.spec` REQUIRE `:: WINRESIZE: zones=8 .* -> PASS ::`; `[wm-act] direct` row unchanged.
- No new UNAOS_* knob.

**Milestones.** M1 zones+drag+min+notify; M2 Shift aspect + panel clamp; M3 keys. Cursor shape: NOT done
(`cursor.rs` has one baked 8x8 `ARROW` sprite inside the hw-sprite plan machinery; a second glyph is not
cheap there).

## Written
Boot 17 (after `tests winresize`) should show
`:: WINRESIZE: zones=8 strip=true min=160x64 drags=3 clamped=N drag=true clamp=true minstop=true aspect=true keys=true -> PASS ::`
and `[wm-act] resize win=N from=200x120 to=... zone=br owner=0x3` lines (drags >= 3, clamped >= 1).
