# DRAGNOMOVE (rmbp-ledger B620) — the wire and the fixture

Peter, flight 34 (image 27b, boot 2, USB mouse plugged): "window dragging is not working. if i try it with lumen it changes window size like i double clicked it instead of dragging."

## 1. The wire

Source: `docs/dev/evidence/rmbp-0915/flight34/f34-boot2.log`, read with
`awk 'index($0,"[wm-act]") && (index($0,"drag-") || index($0,"zoom"))'` and the pointer census with
`awk 'index($0,"[hid]") && index($0,"mouse") || index($0,"MOUSE-1")'`.

```
[15:24:56Z] [wm-act] drag-begin win=4 owner=0xffffff07 at (1026,814) -> grabbed
[15:24:56Z] [wm-act] drag-cancel win=4 owner=0xffffff07 at (798,821) -> release-level
[15:25:00Z] [wm-act] drag-begin win=4 owner=0xffffff07 at (995,813) -> grabbed
[15:25:00Z] [wm-act] drag-cancel win=4 owner=0xffffff07 at (798,821) -> release-level
[15:25:00Z] [wm-act] drag-begin win=4 owner=0xffffff07 at (989,815) -> grabbed
[15:25:00Z] [wm-act] drag-cancel win=4 owner=0xffffff07 at (798,821) -> release-level
[15:25:00Z] [wm-act] drag-begin win=4 owner=0xffffff07 at (964,820) -> grabbed
[15:25:00Z] [wm-act] drag-cancel win=4 owner=0xffffff07 at (798,821) -> release-level
[15:25:00Z] [wm-act] drag-begin win=4 owner=0xffffff07 at (929,820) -> grabbed
[15:25:00Z] [wm-act] drag-cancel win=4 owner=0xffffff07 at (798,821) -> release-level
[15:25:00Z] [wm-act] drag-begin win=4 owner=0xffffff07 at (904,819) -> grabbed
[15:25:00Z] [wm-act] drag-cancel win=4 owner=0xffffff07 at (798,821) -> release-level
[15:25:00Z] [wm-act] drag-begin win=4 owner=0xffffff07 at (898,819) -> grabbed
[15:25:00Z] [wm-act] drag-cancel win=4 owner=0xffffff07 at (798,821) -> release-level
[15:25:00Z] [wm-act] drag-begin win=4 owner=0xffffff07 at (888,816) -> grabbed
[15:25:00Z] [wm-act] drag-cancel win=4 owner=0xffffff07 at (798,821) -> release-level
[15:25:00Z] [wm-act] drag-begin win=4 owner=0xffffff07 at (868,813) -> grabbed
[15:25:00Z] [wm-act] drag-end win=4 owner=0xffffff07 at (798,821) -> no-move
[15:25:00Z] [wm-act] drag-begin win=4 owner=0xffffff07 at (863,812) -> grabbed
[15:25:00Z] [wm-act] drag-end win=4 owner=0xffffff07 at (798,821) -> no-move
[15:25:00Z] [wm-act] drag-begin win=4 owner=0xffffff07 at (858,811) -> grabbed
[15:25:00Z] [wm-act] drag-cancel win=4 owner=0xffffff07 at (798,821) -> release-level
[15:25:02Z] [wm-act] drag-begin win=1 owner=0xffffff02 at (932,732) -> grabbed
[15:25:02Z] [wm-act] drag-cancel win=1 owner=0xffffff02 at (720,743) -> release-level
[15:25:05Z] [wm-act] drag-begin win=1 owner=0xffffff02 at (1184,733) -> grabbed
[15:25:05Z] [wm-act] drag-cancel win=1 owner=0xffffff02 at (720,743) -> release-level
[15:25:05Z] [wm-act] drag-begin win=1 owner=0xffffff02 at (1186,731) -> grabbed
[15:25:05Z] [wm-act] drag-cancel win=1 owner=0xffffff02 at (720,743) -> release-level
[15:25:05Z] [wm-act] drag-begin win=1 owner=0xffffff02 at (1205,716) -> grabbed
[15:25:05Z] [wm-act] drag-cancel win=1 owner=0xffffff02 at (720,743) -> release-level
[15:25:35Z] [wm-act] drag-begin win=1 owner=0xffffff02 at (1254,725) -> grabbed
[15:25:35Z] [wm-act] drag-end win=1 owner=0xffffff02 at (720,743) -> no-move
[15:25:36Z] [wm-act] drag-begin win=1 owner=0xffffff02 at (1302,728) -> grabbed
[15:25:36Z] [wm-act] drag-cancel win=1 owner=0xffffff02 at (720,743) -> release-level
[15:25:36Z] [wm-act] drag-begin win=1 owner=0xffffff02 at (1304,724) -> grabbed
[15:25:36Z] [wm-act] drag-cancel win=1 owner=0xffffff02 at (720,743) -> release-level
[15:25:39Z] [wm-act] drag-begin win=3 owner=0x2 at (366,66) -> grabbed
[15:25:39Z] [wm-act] drag-cancel win=3 owner=0x2 at (17,85) -> release-level
[15:25:39Z] [wm-act] zoom win=3 owner=0x2 at (372,71) -> zoomed
[15:25:39Z] [wm-act] action=zoom win=3 route=title-dbl -> zoomed
[15:25:46Z] [wm-act] drag-begin win=3 owner=0x2 at (1979,268) -> grabbed
[15:25:46Z] [wm-act] drag-cancel win=3 owner=0x2 at (864,283) -> release-level
[15:25:46Z] [wm-act] zoom win=3 owner=0x2 at (1980,264) -> restored
[15:25:46Z] [wm-act] action=zoom win=3 route=title-dbl -> restored
[15:25:48Z] [wm-act] drag-begin win=1 owner=0xffffff02 at (1314,721) -> grabbed
[15:25:48Z] [wm-act] drag-end win=1 owner=0xffffff02 at (720,743) -> no-move
[15:25:49Z] [wm-act] drag-begin win=1 owner=0xffffff02 at (1314,721) -> grabbed
[15:25:49Z] [wm-act] drag-end win=1 owner=0xffffff02 at (720,743) -> no-move
[15:25:49Z] [wm-act] drag-begin win=1 owner=0xffffff02 at (1314,721) -> grabbed
[15:25:49Z] [wm-act] drag-end win=1 owner=0xffffff02 at (720,743) -> no-move
[15:25:53Z] [wm-act] drag-begin win=3 owner=0x2 at (497,67) -> grabbed
[15:25:53Z] [wm-act] drag-end win=3 owner=0x2 at (17,85) -> no-move
[15:25:55Z] [wm-act] drag-begin win=3 owner=0x2 at (324,72) -> grabbed
[15:25:55Z] [wm-act] drag-cancel win=3 owner=0x2 at (17,85) -> release-level
[15:25:55Z] [wm-act] zoom win=3 owner=0x2 at (348,76) -> zoomed
[15:25:55Z] [wm-act] action=zoom win=3 route=title-dbl -> zoomed
[15:22:18Z] [hid] halted ctl=1 addr=2 ep=IN1 kind=boot-mouse reports=3 -> retired (HIDSTALL: not recovered)
[15:22:22Z] :: MOUSE-1: HID pointer detected vid:pid=1c4f:0034 proto=2 relative ep=0x81 mps=4 interval=10 == witness ::
[15:22:23Z] :: MOUSE-1: HID pointer detected vid:pid=1c4f:0034 proto=2 relative ep=0x81 mps=4 interval=10 == witness ::
[15:22:23Z] :: MOUSE-1: 1 reports, last dx=0 dy=0 buttons=0x00 == witness ::
[15:22:27Z] :: MOUSE-1: 32 reports, last dx=8 dy=-7 buttons=0x00 == witness ::
```

`MOUSE-1` is printed by `drivers/xhci/mod.rs:4621` and `:5096` and `[hid] halted` comes from `hidstall.rs:107`, so the
mouse is on xHCI. Every cancel lands at the same point, (798,821) for win=4 and (720,743) for win=1. That point is
`drag_settle_point()`'s `DRAG_DOWN_*` seed. The belt fired before any steering motion was recorded. On Lumen
(win=3), two grabs inside `DBL_MS` were read as a title double-click: `zoom … route=title-dbl -> zoomed`. That is
the "changes window size" Peter reported.

## 2. The mechanism (verified against d616bf98)

- Reader: the drag belt `if !crate::pal::cursor::button_down()` (`arch/x86_64/syscall.rs:6655`, and the second
  tail at `:6916`). `video/facet.rs:1670` (`drag_poll`, the viewer pan, shared with aarch64 `desktop_firmware`)
  also reads it. No file under `arch/aarch64/` reads the level. The aarch64 window drag ends on the edge only, so
  the Pi/Orin drag never had this bug; the facet pan on those boards did, and this fix covers it too.
- Writer before this arc: `ehci::note_buttons` (`drivers/ehci/mod.rs:490/509/511`), plus selftests and winsnap.
  The xHCI decode (`drivers/xhci/mod.rs:5048–5060`), BT HID (`drivers/ehci/bthid.rs:1776`) and the trackpad
  gesture taps and secondary clicks (`drivers/ehci/tpgest.rs:114–163`) queue `Event::Button` through
  `pal::push_pointer_report` and publish no level. The trackpad's PRIMARY click and three-finger hold come back
  as `out_btn` to the caller, which runs `note_buttons`, so the trackpad drag was not affected.
- Fix: `pal::push_locked` (`pal.rs:1224`) is the one function both `push_event` and `push_pointer_report` go
  through. It now publishes `mask & 1` for every `Event::Button`, before `q.push`. That keeps the ARC D M1 order:
  the level changes first, then the edge is queued, and a synthesised release lowers the level first because its
  own `Button(0)` passes this line first. The write is in `push_locked` and not in `EventQueue::push`, so the
  re-circulation seam (`requeue_event`) never re-publishes an old level.

## 3. The fixture: `tests dragmouse` (`arch/x86_64/syscall.rs`, file tail)

Press, steer and release go through `pal::push_pointer_report` with no `set_button_level` call. The one
`set_button_level(false)` sets the starting level. The ring is held (PTRLEAK). Routing uses the router's own
`wc_route_event` + `wc_route_tail`.

Expected on the fixed tree:

```
[wm-act] drag-begin win=<n> owner=0x3 at (<px>,<py>) -> grabbed
[wm-act] drag-end win=<n> owner=0x3 at (<x>,<y>) -> placed
[wm-act] dragmouse press_level=true grabbed=true live=true release_level=true ended=true rest=true at=Some((<x>, <y>)) want=(<x>,<y>) discarded=<n> -> PASS
```

Expected on the pre-fix tree (the shape seen on the glass):

```
[wm-act] drag-cancel win=<n> owner=0x3 at (<grab>) -> release-level
[wm-act] dragmouse press_level=false grabbed=true live=false release_level=true ended=true rest=false … -> FAIL
```

## 4. R117: the WINRESIZE read

- Same failure: `resize_begin` (`video/wm.rs:29810`) publishes `DRAG_WIN`, and `wc_resize_press`
  (`arch/x86_64/syscall.rs:29610`) is reached from the click router (`:7624`). A frame resize with the USB mouse
  hit the same belt and was cancelled `release-level` on its first motion. The pal seam fixes it.
- Zone width: before this arc `rs_zone_of` used raw device px (`RS_EDGE` = 6, `RS_CORNER` = 12). The frame is
  `ui::px(5)`, which is 10 device px at the rMBP's 2x scale, so the edge band was 6 device px (3 logical) and the
  corner was 6 logical. Now both are scaled with `ui::px`: at 2x the edge band is the full 10 px frame (it is
  clipped to the chrome) and the corner is 24 px. At 1x (QEMU) nothing changes.
- `tests winresize` was never registered. Its `register` call sat after the `//` on the `wmdirect` registration
  line (LEDGER P7), so `x86-wc.spec`'s `REQUIRE :: WINRESIZE: zones=8 .* -> PASS ::` had nothing producing it.
  It is now registered before the comment.
- Resize cursors: `video/cursor.rs:5244/5251` has `CURSOR_RESIZE_NS`/`_EW` glyphs, but nothing chooses them when
  the pointer hovers a frame zone. `cursorshape` only applies a shape the focused program asks for
  (`SYS_CURSOR_SHAPE`). This is open as WINRESIZE2.
