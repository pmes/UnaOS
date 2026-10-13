# MENUS2 (rmbp-ledger B652) — the dock's and Quarry's context menus on the sheet, the wallpaper generation in the seal, the exact band where the owner holds the pixels

Branch `exec-rmbp-menus2`, cut from 8da376cb. MENUSCRAFT (B642) §9 named three owed items; this arc builds (1) and
(2), builds (3) where the owner holds the pixels (Quarry), and names the exact change for the rest (§3 below).
Where §9's text and the tip disagreed, the tip won: §9 said "`sheet::row` for each row composer"; Quarry's menu is
painted in a WINDOW surface, not a panel strip, so it does not use `sheet::row` (§2).

CHARTER: the domain is the window manager's furniture (`wm` seam, docs/CODEX.md §2) and Quarry's own window
(Quarry's handler); no new kernel file, so no `CHARTER:` header is owed.

## 1. The dock's tile menu (video/dock.rs)

* `menu_geo` (:3008) publishes the OUTER rect: `mw = 18 cells + 2 PAD + sheet::BAND()`, `mh = nrows * TILE_H + BAND`;
  the hover test reads `sheet::body(rect)` (the band lights nothing).
* `menu_box` (:3046) paints `crystal::sheet::row(out[bx..], outer, j, under, square_top=false)` first (all four
  corners round: a context menu floats), then the hovered row's `sheet::pill`, the label in `sheet::text_primary()`
  (`on_accent()` on the pill) at `bx + INSET + PAD` clipped to the body, then `sheet::seal_row` over the menu's span
  (MENU-UNDER: no pure white). The old raised face, keyline and full-width accent row are gone.
* The `Under` is `sheet::Under::scan(rows)` — the model `compose` already scanned — built once per paint in `paint`
  (:935, only while a menu is open) and in `side_menu_pass` (:4009, now given the rows by both `compose` call sites).
* `menu_press` (:3137) hit-tests `sheet::body(menu_rect())`: a press on the band is a press outside (dismiss),
  MENUSCRAFT §2's rule. `menu_rect()` has no consumer outside dock.rs, so the outer rect moves nothing in the
  occluder registry.
* `sheet_probe()` (:3069): the `dock_menu=` leg over a synthetic one-row (Trash) geometry and `Under::none()`.

## 2. Quarry's file-ops menu (video/quarry/ops.rs)

* `menu_w`/`menu_h` (:304/:308) are the OUTER sizes: `22 cells + 2 PAD + 2 INSET + BAND` (the label sits at
  `INSET + PAD` inside the pill's inset, so the widest label `Empty Trash (NN items)` still fits whole) and
  `12 * row_h + 2 * crystal::DROP_PADY() + BAND`. `right_press` already clamps the outer rect inside the content,
  so the band always fits. `menu_body` (:312) is `(bw, bh, pady)`.
* `menu_paint` (:341) paints IN Quarry's own surface through the kit (`draw::Surf::new(px, g.w)`): the `elev.2` lift
  as two `shadow_rrect` layers in `theme::shadow_soft()` (MENUSCRAFT §7's mapped `elev.2` ink for an overlay outside
  crystal; the tight layer at two thirds of it, elev.2's own tight:wide ratio — `sheet::elev2` is private to
  crystal.rs), the `surface.overlay` body at `radius.overlay`, the `line.subtle` hairline, `sheet::pill` per surface
  row of the hovered item, labels in `text.primary`/`on_accent`, `sheet::seal_row` over every row of the outer rect.
  `sheet::row` is NOT used: its `Under` is a scene ESTIMATE in panel coordinates, while the pixels beneath Quarry's
  band are Quarry's own and already in `px` — the shadow blends over the TRUE scene. That is MENUSCRAFT §4's
  exactness, where the owner holds the pixels.
* Hover (`menu_hover`, :320): the system cursor through `panel_info_nonblocking` (LOCKFIX's door) and `to_source`
  (wm's `hit_test`/`info`), against the body less its padding, at paint time. `paint_overlay` (:445) copies the
  menu origin out of `MENU` before the read, so no Quarry UI lock is held across wm's calls (MODEL is held, as for
  every overlay; wm never calls into Quarry under its table — its only calls are `press_route`).
* `menu_press` (:584-588) tests the body less `DROP_PADY`: `i = (sy - mn.y - pady) / row_h`; a press on the band or
  the padding picks nothing and dismisses.
* `sheet_probe()` (:384) → `(quarry_menu, band_exact)`; reachable from menubar as `super::quarry::live::ops`
  (`pub mod live` under `quarry`, `pub mod ops` at live.rs:136).

### Owed outside this lane (hover repaint)

A hover edge over Quarry's menu repaints only when QUARRYHOVER's `hover_poll` (quarry/live.rs :3006) repaints for a
list/tree crossing underneath (same `row_h` cadence, so in practice most item edges coincide). The exact hook: in
`live.rs` `hover_poll`, after `hover_route(x, y)`, call a new `ops::hover_tick(x, y)` that compares `menu_hover`
with the last painted item and calls `repaint()` on a change. live.rs is QUARRYHOVER's landed file and not in this
lane: reported, not built.

## 2b. The wallpaper generation in the strip seal (video/strip.rs)

* `seal` (:327-329) is still one statement line (LINE-NEUTRAL: 1 line out, 1 line in):
  `seal_with(h, super::theme::epoch(), wall_gen())`.
* `seal_with(h, epoch, wall)` (:2033, the file's tail, so no line above it moves): the epoch at bit 40 as before
  (B408), then `fnv1a_u64(.., wall)`; never 0.
* `wall_gen()` (:2043, `facet`; :2048 `0` without it): `wallpaper::back()`'s `(px.as_ptr(), w, h)` folded. Why a
  pointer and not §6's counter: wallpaper.rs is not in this lane; every `wallpaper::install` builds its picture in a
  fresh `Vec` while the previous one is still alive in `WALL` and the one before in `RETIRED` (wallpaper.rs
  `install`), so two consecutive installs never share an address. Residual: three installs inside ONE strip pass can
  reuse the address freed two installs back; a one-line `GEN` counter in wallpaper.rs `install` would close it.
* (Compile note: the parameter was first spelled `gen`, a reserved keyword in this edition; renamed `wall`.)

## 3. The compositor's scene under the PANEL sheets — NOT BUILT (HEAD rule 3), the exact change

The panel sheets (the SHARD menu and its panel mode in crystal.rs, the window menus in winmenu.rs, the dock's
tile menu above) blend their band over `sheet::Under`, an ESTIMATE (visible window boxes as `surface.content` /
`chrome.face`, else the wallpaper). An exact band needs:

1. crystal.rs (DESIGNFIX's lane) `sheet::Under` (:1689): a third constructor `Under::exact(reader)` whose `at(x, y)`
   reads the composed scene, beside `none()`/`scan()` (its fields are private, so no caller can add it).
2. the compositor side: `screen.rs` publishing its `back` store (the desktop layer: wallpaper + the shell's drawing)
   the way `wallpaper::SNAP` publishes the picture, plus the windows' content through `wm::draw_window`'s surface
   read, for the reader in (1).

Both are outside this lane; the panel sheets keep MENUSCRAFT §4's estimate. Quarry's band (§2) is exact because
Quarry holds its own pixels.

## 4. The fixture (video/menubar.rs `menuscraft_selftest`, leg 3 at :3735)

* `dock_menu` = `dock::sheet_probe()`: body corner ≠ face (rounded), the hovered row's pill inside the inset and
  not outside it, the band row below the body darker than `DESKTOP_BG`, no pure white.
* `quarry_menu`, `band_exact` = `quarry::live::ops::sheet_probe()`: `geometry(1280, 800)`, item 0 hovered, painted
  into a `surface.content` surface and an accent surface. Menu: body centre = `surface.overlay`, pill at the body's
  right inner edge = `accent.fill` and not at `INSET/2`, no pure white in the outer rect. Band exact: the band pixel
  `(bw + 1, bh / 2)` darker than its own fill in BOTH and different between the two. `n/a` (not a failure) in an
  image without `quarry`.
* `wallpaper_gen` = `seal(1) == seal_with(1, epoch, wall_gen())` and `seal_with(1, e, g) != seal_with(1, e, g ^ 1)`.
* All four fold into the verdict. Wire shape:
  `:: MENUSCRAFT: sheet=<w>x<h> band=<b> corners=ok face=ok hairline=ok shadow=ok(…) pill=ok white=0 glyphs=3/3 inside=ok gap=ok(<g>) dock_menu=ok quarry_menu=ok wallpaper_gen=ok band_exact=ok -> PASS ::`

## 5. Palette

Every colour through `crystal::sheet`'s token fns and `theme::*` (`shadow_soft`, `surface_content`, `accent`,
`pure_white`); the white rule through `sheet::seal_row` / `theme::pure_white()`; `0x00FF_FFFF` appears only as an
`&` mask. No colour literal.

## 6. Build

One compile of the x86 leg (wave34 head), from `unaos/crates/kernel`:
`CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/root/unaos-bench/scratch/cloud/wave34/target cargo +nightly check -Z build-std=core,compiler_builtins,alloc -Z build-std-features=compiler-builtins-mem -Z json-target-spec --target ../../x86_64-unknown-none.json --features "$(cat /root/unaos-bench/scratch/cloud/wave34/kfeats24.txt),stria,gen7r9,wifi9,usbnet,wifi10"`.
First run rc=101 (`gen` reserved keyword, strip.rs); after the rename the second run printed no `Checking` line (a
replay of a stale fingerprint, the head's named case), so the four touched files were touched and it ran once
more: `Checking unaos-kernel` → rc=0, no warning in the new code. No QEMU, no suite (R107).
