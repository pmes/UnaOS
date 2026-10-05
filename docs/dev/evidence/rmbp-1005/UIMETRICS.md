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
(the crispy kit's `metrics.*` and Peter's 24-px disc, read as 96-ppi CSS px — R85 (11) "the theme scales by
DPI"; at 1.0 every length is the const it replaced, byte for byte). Every furniture
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

**Witness (metal).** On the first desktop service pass `[ui] metrics ppi=221 scale=2.5 s2=5 bar=85 title=85 frame=13
gap=30 disc=60 chrome=23x50 cell=18x40 glyph=2 win=1300x1120 consts=0 asserts=ok`; `tests metrics` as above.

**Stays owed (named before the build).** Ring-3 windows that would rather draw at scale 1 have no way to ask yet
(a `SYS_WIN_CREATE` flag); Lumen (ring 3) keeps the magnify path. Facet / beam / the rast demo keep their own
surfaces. The metal boot (R78).

## Results (compile legs; R78 — no QEMU, the metal boot is the seat's)

| milestone | commit | content |
|---|---|---|
| M1 | c6ec712b | `ui.rs` (`ui::base`, `Metrics::at/panel/for_scale/check`, `ui::px`, the compile-time proof over s2 2..=8; `for_height` / `SCALE_STEP` retired), `video/dpi.rs` (`metrics_scale`, `s2`, `latched`), `video/metrics.rs` (new, `//! CHARTER: Kernel — wm`: `ignite`, `consts_disagreeing`, `tests metrics`), the const -> fn conversion in theme / wm / menubar / dock / crystal / strip / winmenu / quarry (`SBW`) and every reader (44 files in all, every one line-neutral except `ui.rs`, `dpi.rs` and the file tails), the const assertion blocks -> `uimetrics_assert*` fns run by `ignite`, `text::chrome_cell` + the scaled-cell atlas fallback (`blowup`), the wm native-row plumbing (`create_at_native`, `spawn_geometry_native`, `NATIVE_SLOTS`, `uimetrics_census`) |
| M2 | a73df857 | native kernel windows (settings, activity, login + its fixture control row, quarry, fileview, textedit, instgui, pulsewin); the logical-canvas helpers in `video/metrics.rs`; `Face::Ui` on the dpi cell at `font_size` x ppi / 96; `text::grid_cell` x `font_size / 13`; `fbcon::regrid` (file tail) + `[kfont] regrid console` |
| M3 | b361c0ca | `tools/eyes/suites/metal/golden/README.md` pin 2 |
| M4 | (this) | this section |

### Numbers (2880x1800, EDID 221 ppi -> s2 = 5, scale 2.5; at s2 = 2 every value is the old const)

| what | 1.0 (was const) | 2.5 (bench) | 4.0 |
|---|---|---|---|
| title strip / menu bar | 34 | 85 | 136 |
| frame / bevel | 5 / 1 | 13 / 3 | 20 / 4 |
| gap (strip PAD) | 12 | 30 | 48 |
| control disc | 24 | 60 | 96 |
| chrome cell | 9x20 | 23x50 | 36x80 |
| console / window grid cell | 7x16 | 18x40 | 28x64 |
| dock tile height | 28 | 70 | 112 |
| cluster floor (`CLUSTER_MIN_SRC_W`) | 141 | 353 | 564 (fixture `FIX_W` 160 -> 576) |
| Settings / login / Activity surface | 520x448 / 440x240 / 560x480 (magnified 2x..3x) | 1300x1120 / 1100x600 / 1400x1200 at scale 1 | — |
| integer glyph scale (`Metrics::scale`, was `height/900`) | 1 | 2 (as 1800/900) | 4 |

### Gates

| leg | command | exit |
|---|---|---|
| charter | `bash /home/user/UnaOS/unaos/scripts/charter-check.sh <worktree>/unaos` | 0 |
| x86 metal shape | `cargo +nightly check --release --target ../../x86_64-unaos.json -Z build-std=core,compiler_builtins,alloc -Z build-std-features=compiler-builtins-mem -Z json-target-spec --features "wc,quarry,ftdirx,login,loginst,nvidia-kepler-vblank,smc,usbnet,hda,hda-tone,facet,beam,sdw,sdwrite,sdhcblk,selfhost,linuxabi,ahci,unafs,busreg,lumen,netring3,prefs_reset,census,installdemo,instgui,witness,selfdiag,ahciroot,btc"` (clean target) | 0 |
| aarch64 desktop | `… --target ../../aarch64-unaos.json … --features "login,loginst,virt_el0,lumen,desktop_firmware,quarry,facet"` (user_blob head `28 00 80 d2`) | 0 |
| aarch64 no desktop | `… --features "login,loginst,virt_el0"` | 0 |

### The wire a metal boot should print

On the first desktop service pass: `[ui] metrics ppi=221 scale=2.5 s2=5 bar=85 title=85 frame=13 gap=30 disc=60
chrome=23x50 cell=18x40 glyph=2 win=1300x1120 consts=0 asserts=ok`. `tests metrics`:
`[ui] metrics fixture latched=true check=ok native=<n> live=<n> chrome=23x50 win=1300x1120 disc=60 gap=30 frame=13` then
`:: UIMETRICS: ppi=221 scale=2.5 bar=85 title=85 cell=18x40 magnified=0 consts=0 -> PASS ::`. A Settings › Display
font size `+` -> `[kfont] restyle … font_size=14 …` and `[kfont] regrid console cell=19x44 grid=<c>x<r> was=<c>x<r>`.
`[settings] open`, `[quarry] open … face=dejavu-mono|dejavu-sans cell=…` with `box=` the surface plus 2x13 / 85+2x13.

### Stays owed

1. **Ring-3 windows cannot ask for scale 1** (a `SYS_WIN_CREATE` flag): Lumen and every ring-3 app keep the magnify
   path, which is the row's rule (the magnify path for ring-3 windows that ask — a small surface is the ask).
2. **Facet** (the image viewer, a kernel row) keeps the magnify path — its surface IS the image and the window scale
   is its zoom; `tests metrics` counts it in `magnified=` if a Facet window is open when it runs.
3. The console window (fbcon) and the shell window (main.rs) were already scale 1 (panel-sized surfaces) and are not
   flagged native (no line in fbcon's mint or main.rs moved); `fbcon::regrid` re-derives the console's grid on its
   existing surface.
4. Painter literals: the logical painters (settings, activity, login, instgui) scale every literal by construction;
   the physical ones (fileview, textedit, quarry, pulsewin) scale their named consts and the literals found in review
   (pads, caret, thumb); a stray 1-px keyline stays 1 device px by design.
5. The metal boot (R78): the goldens re-bless at pin 2; `FIX_W` 576 moves every fixture surface that used 160 (the
   control-cluster fixtures, WMD / winsnap / hit-test rows), whose QEMU-panel expectations were not re-run (R78).
