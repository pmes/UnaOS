# WINRESIZE2 (rmbp-ledger B635): the resize cursor over a window frame

R117 (RULINGS.md:148): "WINDOW RESIZE BY EDGE AND CORNER DRAG, every window, live, the standard cursor shapes at the
edges (CURSORSHAPE)". DRAGNOMOVE (B620, `docs/dev/evidence/dragnomove/DRAGNOMOVE.md` §4) fixed the drag and left
one item open: no cursor changes over a frame zone. The NS/EW glyphs existed (`video/cursor.rs`, `SHAPES` rows 5
and 6), but only `SYS_CURSOR_SHAPE` from the focused program ever selected a row, and there was no diagonal glyph.

## 1. The mechanism

- **Classification site (one, x86).** `wc_route_tail`'s motion arm (`arch/x86_64/syscall.rs:7349`,
  `Event::Mouse`/`MouseAbsolute`) now calls `wc_resize_hover()` immediately after `wc_drag_motion()`. It is a
  same-line fold placed before the line's first `//`, under `#[cfg(feature = "wc")]`. The body is at the file tail
  (`wc_resize_hover_at`) and asks the press arm's own question: `wm::hit_test` for the topmost window, not a
  control disc (`wm::control_hit`, which the press judges first), not owner 0 or `KERNEL_OWNER_DESKTOP` (which
  `wc_resize_press` declines), then `wm::resize_zone_at`. A resize cursor therefore appears exactly where a press
  would start a resize.
- **A live gesture wins.** While `wm::drag_active()` names a window, the zone is `wm::resize_zone_now()`. A resize
  keeps the shape it was pressed with until the release, even if the pointer outruns the edge. A title-bar move has
  zone 0 and shows the program's shape.
- **Edge-only.** `(win << 8) | zone` is swapped into `WRZ_HOVER`. If the pair is unchanged, the function returns
  before any store or print. On entry to a zone it publishes `video::cursor::set_zone_shape(Some(row))` and prints
  `[cursor] zone=<n|s|e|w|ne|nw|se|sw> shape=<name> win=N` once (hard ceiling 4096). On exit it calls
  `set_zone_shape(None)`.
- **Precedence, not save/restore.** `video/cursor.rs` gains a third word, `ZONE_SHAPE`, beside `REQ_SHAPE` (the
  program's ask) and `LIVE_SHAPE` (the latched row). `shape_effective` is SHOTREGION's crosshair, then the zone,
  then the ask (`zone_over`). The program's shape comes back the moment the zone clears because its word was never
  overwritten. `set_zone_shape` calls `shape_owe` exactly as `set_shape` does, so the latch rule is unchanged: the
  row is latched only inside `draw_locked`, with the sprite down.
- **Sprites.** Two SYSTEM rows (`ZONE_SHAPES`) sit past the ABI table, ids `CURSOR_SHAPES` (resize-nwse, the
  tl/br corners) and `CURSOR_SHAPES + 1` (resize-nesw, the tr/bl corners). Each is an 8x8 block mask, MSB leftmost,
  with the hotspot on the shaft pixel nearest the centre (the NS/EW rule) and ink `POINTER_SHAPE_INK[5]`.
  `SYS_CURSOR_SHAPE` still refuses anything `>= CURSOR_SHAPES`, so a program cannot ask for them, and neither
  `una_abi` nor `theme.rs` changes. `CURSOR_MOVE` already existed as row 4. `sprite_color` and `shape_hot` read
  rows through `glyph(i)`, and `shape_name` falls through to `zone_shape_name`. All three are in-place, same-line
  edits. Off x86 `wc`, each helper is the expression it replaced.

| zone | token | row |
|---|---|---|
| T / B | n / s | `CURSOR_RESIZE_NS` (5) |
| L / R | w / e | `CURSOR_RESIZE_EW` (6) |
| TL / BR | nw / se | `SHAPE_RESIZE_NWSE` (8) |
| TR / BL | ne / sw | `SHAPE_RESIZE_NESW` (9) |

## 2. aarch64

`arch/aarch64/syscall.rs` has no resize zones. Its click router's chrome arm (`:14386`, `desktop_firmware`) raises
the window and calls `wm::drag_begin`, which grabs only on the title strip, so a press on a border raises the window
and is consumed without starting a resize. `wc_resize_press` and `resize_begin` have no call site under
`arch/aarch64/`. With no zone to classify there is no shape to show, and this arc touches no aarch64 file. When
aarch64 gains the press arm, the hover goes on its drag-route motion path (`wm::drag_route_tail` callers), the same
shape as the x86 fold. The ZONE word and the system rows are cfg'd to x86 `wc` today, so that arc widens the cfg in
`video/cursor.rs`.

## 3. The fixture: `tests winresize` (`winresize_cursor_leg`, file tail)

The fixture runs before the drag legs move the frame. It sets the program's ask to HAND, so the restore is checked
against a real program shape rather than the arrow. It then classifies at each of the fixture's eight zone probes
(L R T B TL TR BL BR) and checks the shape against a table written in the leg (EW EW NS NS NWSE NESW NESW NWSE), not
against `wrz_zone_cursor`. Back over content, HAND must return. A real BR press through `wc_click_route_at` follows:
over content mid-gesture the shape must still be NWSE, and after the release HAND must be back. The probe point is a
parameter (`wc_resize_hover_at`), the same idiom as `wc_click_route_at`, because QEMU delivers no pointer.

Expected on the fixed tree (x86 QEMU, `UNAOS_WC=1` witness build):

```
[cursor] zone=w shape=resize-ew win=<n>
[cursor] zone=e shape=resize-ew win=<n>
[cursor] zone=n shape=resize-ns win=<n>
[cursor] zone=s shape=resize-ns win=<n>
[cursor] zone=nw shape=resize-nwse win=<n>
[cursor] zone=ne shape=resize-nesw win=<n>
[cursor] zone=sw shape=resize-nesw win=<n>
[cursor] zone=se shape=resize-nwse win=<n>
[cursor] zone=se shape=resize-nwse win=<n>
:: WINRESIZE: zones=8 strip=true min=160x64 drags=<n> clamped=<n> drag=true clamp=true minstop=true aspect=true keys=true cursor=ok cursor_shapes=10 -> PASS ::
```

The ninth line is the press leg re-entering BR after the restore. On a failure, one line comes first:
`[cursor] winresize leg seen=<k>/8 restored=<b> held=<b> released=<b> live=<b>`, and the verdict reads
`cursor=fail … -> FAIL`. On a tree without the arc, the verdict line has no `cursor=` token, so a spec row keyed
on `cursor=ok cursor_shapes=10` is red.

Proof owed to the fold: compile only (R107). The QEMU run and the bench (hover each edge and corner, watch the
shape) belong to the seat.
