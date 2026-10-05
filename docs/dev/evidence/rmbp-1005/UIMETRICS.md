# UIMETRICS — the furniture follows the panel's ppi; kernel windows drawn at scale 1 (rmbp-ledger B372, R85 item 11)

Branch `exec-rmbp-uimetrics`, cut from `exec-rmbp-merge14` @2d4e1b12 (carries KERNELFONT2, B363). No merge.
R78: no QEMU — compile legs; the metal boot is the seat's.

## Design

**Finding.** KERNELFONT2 (B363) put the console's cell on `video::dpi` (7x16 x 2.5 = 18x40 on the 221-ppi bench) and
named what it left: the furniture is `const` device px with `const` assertions — `theme::TITLE_HEIGHT` 34 (the
title strip AND the menu bar), `FRAME` 5, `GAP` 12, `CONTROL_BOX` 24, the chrome text cell 9x20, the dock tile, the
crystal menu — derived through `wm.rs`, `menubar.rs`, `dock.rs`, `crystal.rs`, `strip.rs`, `winmenu.rs`; the text
metrics of `ui::Metrics` follow `height / 900`; and the kernel windows (Settings, Activity, login, Quarry, the
viewers) lay out on the 7x16 cell at a few hundred px and are blown up by the compositor's integer window scale, so
`Face::Ui` stays capped at 13.5 px inside them. On 2880x1800 the desktop is drawn small, then magnified.

**The seam.** `Kernel — wm` (the window manager's own geometry; no handler owns it). ONE runtime table,
`ui::Metrics`, is a pure function of `video::dpi`'s latched scale (x2, 1.0..=4.0) and the theme's base numbers
(the crispy kit's `metrics.*`, read as 96-ppi CSS px — R85 (11) "the theme scales by DPI"). Every furniture
length is `dpi::px_at(base, s2)`. The former `const`s keep their names as zero-argument `fn`s returning the
Metrics field (same line, same token plus `()`: every fold stays line-neutral); the `const` assertions become
(a) a compile-time proof over EVERY scale 1.0..=4.0 (`Metrics::at` is a `const fn`, the theme's relations are
checked for all seven scales) and (b) a runtime assert at the compositor's ignition on the latched one, which prints
`[ui] metrics …`. The `height / 900` rule (`ui::SCALE_STEP`, `Metrics::for_height`) is retired: the integer glyph
scale is `floor(s2 / 2)` (221 ppi: 2, as 1800 / 900 was; no EDID: 1, as every QEMU panel was); fixtures that
wanted "scale 1 whatever the panel" call `Metrics::for_scale(1)`.

New file (CHARTER line): `unaos/crates/kernel/src/video/metrics.rs` — `//! CHARTER: Kernel — wm` — the ignition
check, the `[ui] metrics` witness, the native-window helpers, `tests metrics`.

**Milestones.**
- **M1 `ui::Metrics` from dpi.** Fields: `s2 ppi scale cell_w cell_h line_h margin` (text) and `frame bevel title_h
  bar_h corner widget_r well_r scroll_w button_h button_pad gap ctrl_box ctrl_r chrome_cw chrome_ch dock_tile
  win_w win_h ui_px` (furniture). theme / wm / menubar / dock / crystal / strip / winmenu geometry `const`s ->
  Metrics-backed `fn`s; the chrome text cell (`wm::TITLE_CELL_W/H`) is the 9x20 atlas cell x scale and
  `text::Face::Chrome` sizes its face to it; before the faces load the atlas glyph is drawn at the scale's integer
  part centred in the cell (KERNELFONT2's `Face::Grid` fallback, generalised). Ignition check + witness.
- **M2 kernel windows at scale 1.** A `native` row flag: the tiler pins its scale at 1 and its surface is drawn at
  its real pixel size; Settings, Activity, login, Quarry, the file viewer, the editor and the installer GUI create
  native windows sized `px(logical)` and paint through Metrics (`ui::px`), with `Face::Ui` sized
  `font_size x ppi / 96` capped by the scaled cell (no more 13.5-px cap). The magnify path (integer window scale)
  stays only for ring-3 surfaces, whose small surface IS the ask. The console regrids on a `font_size` restyle
  (the cell becomes the face's own advance x line box at the new size, `fbcon` re-arms; B363 seat: yes).
- **M3 GLASSEYES re-pin + `tests metrics`.** golden README pin 2 (every state moves: the furniture and the kernel
  windows change size at 2880x1800/221 ppi); `tests metrics` ->
  `:: UIMETRICS: ppi=221 scale=2.5 bar=<px> title=<px> cell=18x40 magnified=0 consts=0 -> PASS ::`, where
  `magnified` counts live kernel-owned rows drawn at window scale > 1 and `consts` counts furniture readers that
  disagree with the latched Metrics.
- **M4 doc** (this file's results).

**Witness (metal).** At ignition `[ui] metrics ppi=221 scale=2.5 s2=5 bar=85 title=85 frame=13 gap=30 disc=30
chrome=23x50 cell=18x40 glyph=2 asserts=ok`; `tests metrics` as above.

**Stays owed (named before the build).** Ring-3 windows that would rather draw at scale 1 have no way to ask yet
(a `SYS_WIN_CREATE` flag); Lumen (ring 3) keeps the magnify path. Facet / beam / the rast demo keep their own
surfaces. The metal boot (R78).
