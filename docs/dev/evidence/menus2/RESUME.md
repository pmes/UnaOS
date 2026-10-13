# MENUS2 (rmbp-ledger B652) — RESUME (stopped by the seat 2026-10-14: wrong model; resumes on Opus)

Worktree /home/user/exec/menus2, branch exec-rmbp-menus2, cut from 8da376cb. WIP commit c4b63fbd (dock.rs only).
NO compile was run. Last compile command: none (rc: n/a). No gates, no linters run.

## DONE (committed in c4b63fbd — unaos/crates/kernel/src/video/dock.rs only; NOT compiled)

* `menu_geo` (:3007): the published rect is the OUTER one — `mw = 18 cells + 2 PAD + sheet::BAND()`, `mh = nrows*TILE_H + BAND`;
  the hover test reads `sheet::body(rect)`.
* `menu_row` / `menu_box` (:3030/:3045): new `under: &crystal::sheet::Under` parameter; `menu_box` paints
  `sheet::row(out[bx..bx+mw], outer, j, under, square_top=false)` first, then `sheet::pill` on the hovered row,
  the label in `sheet::text_primary()` / `sheet::on_accent()` at `bx + INSET + PAD` (draw_row clipped to `bx+bw`),
  then `sheet::seal_row` over the menu span. The old face/keyline/full-width accent row is gone.
* `pub fn sheet_probe() -> bool` (after `menu_box`): the `dock_menu=` fixture leg over a synthetic one-row Trash
  `MenuGeo` and `Under::none()`: corner ≠ face, pill inside inset, band row darker than `theme::DESKTOP_BG`, white=0.
  (Uses `0x00FF_FFFF` as an `&` mask only — the appearance gate reads a mask operand as not-a-colour.)
* `paint` (:930): `under = if mb > 0 { Under::scan(rows) } else { Under::none() }`, passed into the closure.
* `side_menu_pass(l, mnu, rows)` (:3985): takes the model rows, builds `Under::scan(rows)`; both call sites in
  `compose` (:874, :920) pass `&rows[..n]`.
* `menu_press` (:3115): hit-tests `sheet::body(menu_rect())` — a press on the band dismisses.

## NOT DONE (the exact next steps)

1. quarry/ops.rs: `menu_w`/`menu_h` (:300) become OUTER sizes (`+ sheet::BAND()`; height `ITEMS.len()*row_h + 2*crystal::DROP_PADY() + BAND`).
   New `fn menu_paint(px, g, x, y, hover: Option<usize>)` called from `paint_overlay` (:338) and the probe; it paints IN Quarry's
   own surface through the kit (`draw::Surf::new(px, g.w)`): two `shadow_rrect` layers with `theme::shadow_soft()` (the toast ×'s
   `elev.2` ink, MENUSCRAFT §7 — `sheet::elev2` is private to crystal.rs, DESIGNFIX's lane), `fill_rrect(surface_overlay, RADIUS all)`,
   `stroke_rrect(line_subtle)`, `sheet::pill` per row for the hovered item, labels at `x + INSET + PAD, y + PADY + i*row_h + g.ts`
   in `text_primary`/`on_accent`, `sheet::seal_row` over each row of the outer rect (clipped to g.w). `sheet::row` is NOT usable
   here (it takes an `Under` in panel coords); the band blends over Quarry's TRUE pixels = the exactness (3) asks for, where the
   owner holds the pixels. Hover: read `panel_info_nonblocking()` + `pal::cursor::pos` → `to_source` (:353) at paint time
   (lock order checked: wm never calls into Quarry under its table, so `wm::info` under MODEL is safe); hover edges over the menu
   repaint through QUARRYHOVER's `hover_poll` list-row crossings (same `row_h` cadence); the exact hook (`ops::hover_tick()` in
   live.rs `hover_poll`) is out of lane — report it. `right_press` (:438) already clamps the outer rect. `menu_press` (:450/:484):
   body hit test `sx < mn.x + bw`, `sy >= mn.y + PADY`, `i = (sy - mn.y - PADY)/row_h`.
   `pub fn sheet_probe() -> (bool, bool)` (menu ok, band exact): `geometry(1280, 800)` (pure), paint at (0,0) hover 0 into
   `vec![theme::surface_content(); g.w*g.h]`, then again into `vec![theme::accent(); ..]`; body centre = surface_overlay; pill at
   `(bw - INSET - 2, PADY + row_h/2)` = accent_fill; band pixel `(bw + 1, bh/2)` darker than its fill in BOTH and different
   between the two → `band_exact`. Reachable from menubar as `super::quarry::live::ops::sheet_probe()` (`pub mod ops` at live.rs:136).
2. strip.rs `seal` (:327, keep it one line): `pub fn seal(h) -> u64 { seal_with(h, super::theme::epoch(), wall_gen()) }`;
   `pub fn seal_with(h, epoch: u32, gen: u64) -> u64 { let h = fnv1a_u64(h ^ ((epoch as u64) << 40), gen); if h == 0 { 1 } else { h } }`;
   `#[cfg(feature = "facet")] pub fn wall_gen() -> u64 { super::wallpaper::back().map_or(0, |b| (b.px.as_ptr() as u64) ^ ((b.w as u64) << 48) ^ ((b.h as u64) << 32)) }`
   + `#[cfg(not(feature = "facet"))] pub fn wall_gen() -> u64 { 0 }`. Why a pointer and not §6's counter: wallpaper.rs is NOT in the
   FILES list; every `wallpaper::install` allocates a fresh `Vec` while the previous `Wall` is still held in `RETIRED`
   (wallpaper.rs:65-85), so consecutive installs never share an address — an install generation by construction. `Back` fields are pub
   (desktop.rs:164). wallpaper's module gate is `facet` AND the furniture gate (mod.rs:1157); strip is the furniture gate only.
3. menubar.rs `menuscraft_selftest` (:3644), fixture fields only: `dock_ok = super::dock::sheet_probe()`,
   `(quarry_ok, band_exact) = super::quarry::live::ops::sheet_probe()`,
   `wall_ok = strip::seal(1) == strip::seal_with(1, theme::epoch(), strip::wall_gen()) && strip::seal_with(1, e, g) != strip::seal_with(1, e, g ^ 1)`;
   print `dock_menu=ok quarry_menu=ok wallpaper_gen=ok band_exact=ok` before `-> PASS/FAIL` and fold all four into `ok`.
4. (3) the compositor's scene-under-overlay for the PANEL sheets (crystal, winmenu, the dock): NOT buildable in this lane — `sheet::Under`
   (crystal.rs:1689, DESIGNFIX) has only `none()`/`scan()` and private fields; an exact band needs an `Under::exact(reader)` seam there,
   and the compositor side needs screen.rs to publish its `back` store (the desktop layer) the way `wallpaper::SNAP` does plus the windows'
   content via wm's surface read. STOP-and-report item (HEAD rule 3). The dock keeps the §4 estimate like crystal/winmenu.
5. Ledger row B652 in docs/dev/OS/rmbp-ledger.md (append after the last B row, line ~680; 7 cells: id, item, owner
   `shared (video/dock.rs, video/quarry/ops.rs, video/strip.rs, the compositor)`, flies-on `rmbp`, status `fixed-unflown — MENUS2 on
   exec-rmbp-menus2 @the commit carrying this row (parent 8da376cb): compiled (<command> rc=0), proof owed to the fold` + one sentence,
   evidence `docs/dev/evidence/menus2/MENUS2.md; witness x86 QEMU (tests menuscraft: …), then the rmbp bench`, closed-by `—`; no `|` in a cell,
   no claim word). Evidence doc MENUS2.md (the design text is in this file's §DESIGN below and in
   /root/unaos-bench/scratch/cloud/wave34/menus2/design.md).
6. Compile ONCE with the arc's x86 line (wave34 shared target dir, never rm), then (coordinator's correction, 2026-10-14) run from unaos/:
   `./arroyo gates > gates.log 2>&1; echo rc=$?` and paste `GATES: 17 run 17 green -> PASS`; the three linters are inside it
   (charter-check.sh, status-check.py, appearance-check.py). Then squash/replace the wip commit with the real one
   (`video: MENUS2 — …`, body = the design, Co-Authored-By of the model that did the work).

## Learned (not in the arc file)

* `quarry::ops` is reachable from menubar as `super::quarry::live::ops` (quarry.rs:26 `pub mod live`, live.rs:136 `pub mod ops`).
* Quarry's `repaint()` (live.rs:1850) holds MODEL + SURF, not wm's table; wm's only calls into Quarry are via `press_route` (wm.rs:26256 table).
* `theme::shadow_soft()` is the mapped `elev.2` ink for an overlay outside crystal (MENUSCRAFT §7's toast × row).
* The dock's `menu_rect()` has no consumer outside dock.rs (menu_press and the DOCK2 fixture at :3171 only), so the outer rect
  changes nothing in the occluder registry; the bottom dock's band rides `ext_rect` as before.
* Disk at start: 2.7 GB free on /. kfeats24.txt read fine.

## DESIGN (as written before the code)
video: MENUS2 — the dock's and Quarry's context menus on the sheet, the wallpaper generation in strip::seal, the exact band where the owner holds the pixels

DESIGN (R106, written before the code; MENUSCRAFT §9's three items, the tip read first).

(1) The dock's tile menu (video/dock.rs). `menu_geo` (:3007) publishes the OUTER rect: `mw = 18 cells + 2 PAD + sheet::BAND()`,
    `mh = nrows * TILE_H + BAND`; the hover test and `menu_press` (:3100) read `sheet::body(rect)` — a press on the band is a press
    outside the menu (dismiss), MENUSCRAFT §2's rule. `menu_box` (:3037) paints `crystal::sheet::row(out[bx..], outer, j, under,
    square_top=false)` first (all four corners round: a context menu floats), then the hovered row's `sheet::pill`, the label in
    `sheet::text_primary()` (or `on_accent()` on the pill) at `INSET + PAD`, and `sheet::seal_row` over the menu's span (no pure
    white: MENU-UNDER). The `Under` is `sheet::Under::scan(rows)` — the same model `compose` already scanned — built once per paint
    in `paint` (:930) and in `side_menu_pass` (:3971, now given the rows), only while the menu is open. The band above a bottom dock
    (`menu_row`) still fills the rest of the row with `DESKTOP_BG` as before. The fixture read: `dock::sheet_probe()` paints a
    one-row Trash geometry with `Under::none()` and measures corner, pill, band-darker-than-scene, white=0.
(2) Quarry's file-ops menu (video/quarry/ops.rs). `menu_w`/`menu_h` (:300) are the outer sizes (`+ BAND`; the height carries
    `crystal::DROP_PADY()` above and below the items, the sheet's container padding). `right_press` (:438) clamps the OUTER rect
    inside the content, so the band always fits. `menu_press` (:450) hit-tests the body less the padding. The painter
    (`menu_paint`, new; `paint_overlay` calls it) works IN Quarry's own surface (`px`, source px at the window's scale) through the
    kit (`draw::Surf::new(px, g.w)`): the sheet's `elev.2` lift as two `shadow_rrect` layers (`theme::shadow_soft()`, the toast ×'s
    `elev.2` ink per MENUSCRAFT §7; the sheet's own peak numbers are `crystal::sheet`'s private `elev2`, DESIGNFIX's lane), the
    `surface_overlay` body at `radius.overlay`, the `line.subtle` hairline, `sheet::pill` per row for the hovered item (read from
    `pal::cursor::pos` through `to_source` at paint time; a hover edge over the menu repaints through QUARRYHOVER's `hover_poll`,
    whose list-row crossings beneath the menu are the same `row_h` cadence), labels in `text_primary`/`on_accent`, and
    `sheet::seal_row` over every row of the rect. `sheet::row` is NOT used here: it takes an `Under` (panel coordinates, a scene
    estimate), and Quarry's menu lives in a window surface whose pixels beneath the band Quarry itself holds — so the band
    blends over the TRUE pixels. That is (3)'s exactness where the owner has the pixels; `sheet_probe()` proves it by painting
    over two different fills and reading two different band pixels, each darker than its fill.
(3) The compositor's scene-under-overlay (the panel sheets: crystal, winmenu, the dock). NOT DONE HERE, the exact change:
    `sheet::Under` (crystal.rs :1689, DESIGNFIX's lane) has two constructors, `none()` and `scan(rows)`, and private fields; an
    exact band needs a third source — `Under::exact(reader)` reading the compositor's composed scene — and the compositor side
    needs `screen.rs` to publish its `back` store (the desktop layer: wallpaper + the shell's drawing) the way `wallpaper::SNAP`
    publishes the picture, plus the windows' content through `wm::draw_window`'s surface read. Both files are outside this lane
    (crystal.rs by name, wm.rs/screen.rs by the FILES list's "the compositor" having no in-lane consumer without the `Under`
    seam). Reported, not built; the panel sheets keep MENUSCRAFT §4's estimate.
(2b) The strip seal (video/strip.rs :327): `seal(h)` folds `wall_gen()` beside `theme::epoch`, through `seal_with(h, epoch, gen)`
    (pure, the fixture's read). `wall_gen` is the installed picture's identity from `wallpaper::back()` (`px.as_ptr(), w, h`):
    every `wallpaper::install` allocates a fresh `Vec` while the previous `Wall` is still held in `RETIRED`, so consecutive
    installs never share an address — an install generation with no counter and no edit to wallpaper.rs (not in the FILES list;
    §6's one-line counter would be the same fact). `None` (off / not loaded) folds 0. `facet`-gated like the module; 0 without it.
Fixture (video/menubar.rs `menuscraft_selftest` :3644): `dock_menu=` from `dock::sheet_probe`, `quarry_menu=`/`band_exact=` from
    `quarry::live::ops::sheet_probe`, `wallpaper_gen=` = `seal(1) == seal_with(1, epoch, wall_gen())` and a different generation
    changes it; all four fold into the verdict.
Palette: every colour through `crystal::sheet`'s token fns and `theme::*`; the white rule through `sheet::seal_row` /
    `theme::pure_white()`; no literal (appearance gate).
