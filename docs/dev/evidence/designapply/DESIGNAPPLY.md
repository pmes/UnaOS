# DESIGNAPPLY — the design language's first three decisions, applied

*rmbp-ledger B639. Rulings R120 (scale 2.0), R121 (OKLCH tokens, `crispy` kept), R122 (window shadows, the frame
goes). Source: `docs/dev/OS/05_USER_EXPERIENCE/DESIGN-LANGUAGE.md` §2.1, §2.3, §2.4, §2.5, §3.1. Branch
`exec-rmbp-local-designapply` from `6f13e5e92`. Two commits: `palette: DESIGNAPPLY — …` (the tokens and the
painters) and `wm: DESIGNAPPLY — …` (the scale rule, the frame, the shadow).*

What was measured is compile-level and host-level only (the seat's rule for this wave: one x86 kernel check, no
QEMU). No pixel of this has been seen on glass. §7 is the wire the next boot should show.

---

## 1. The token table as applied (R121, §2.3)

`unaos/libs/sys/palette_core/src/lib.rs`. `Tok` is named by job; `TOKENS` 32 -> 27; `TOKEN_NAMES` are §2.3's
spellings. Every value is §2.3's hex, unchanged.

| Token | Light | Dark |
|---|---|---|
| `surface.window` | `#F5F7FB` | `#1D1E21` |
| `surface.sidebar` | `#EBEDF1` | `#252629` |
| `surface.content` | `#FFFFFF` | `#16171A` |
| `surface.raised` | `#FFFFFF` | `#292A2D` |
| `surface.overlay` | `#FAFCFF` | `#2F3033` |
| `surface.sunken` | `#EEF0F4` | `#121417` |
| `line.subtle` | `#DCDEE2` | `#343539` |
| `line.strong` | `#BCBEC2` | `#4B4D50` |
| `line.control` | `#84868A` | `#7E8084` |
| `text.primary` | `#191B1D` | `#F0F2F6` |
| `text.secondary` | `#595B5E` | `#AFB1B5` |
| `text.tertiary` | `#67696C` | `#96989C` |
| `text.disabled` | `#A3A5A8` | `#5C5E61` |
| `ctrl.close` / `.min` / `.zoom` | `#F0584B` `#F4BF3F` `#47C86E` | same |
| `status.positive` / `.warning` / `.critical` / `.info` (fills) | `#32851B` `#A26708` `#BC4945` `#1577C8` | same |
| `ink.*` (status as text) | = the fill | `#89CC7B` `#E8AA4E` `#FF958D` `#73BDFF` |
| `syntax.code` / `.num` / `.lit` (content inks, not in §2.3; unchanged) | `#208040` `#B05000` `#8030A0` | `#6CC88A` `#E39A55` `#C38BE0` |
| `scrim` (alpha of black) | 77 (30 %) | 128 (50 %) |
| `rim.dark` (alpha of white) | — | 20 (8 %) |

The accents (`ACCENTS` = fill, `ACCENT_HOVER`, `ACCENT_PRESSED`, `ACCENT_INK_LIGHT`, `ACCENT_INK_DARK`), §2.3's
table row for row. Index 0 keeps the name **`crispy`** (R121: "crispy stays") and takes §2.3's default periwinkle;
`prefs_core::appearance::ACCENTS` is unchanged, so the stored choice still reads.

| Accent | fill | hover | pressed | ink Light | ink Dark |
|---|---|---|---|---|---|
| **crispy** | `#4E6DD3` | `#4361C5` | `#3955B8` | `#3D5ABD` | `#A2BCFF` |
| teal | `#138282` | `#0A7575` | `#016869` | `#017272` | `#10D8D8` |
| moss | `#32851B` | `#247905` | `#1D6C00` | `#227702` | `#83D670` |
| amber | `#A26708` | `#925D0B` | `#845300` | `#895709` | `#FEA92F` |
| clay | `#C44F21` | `#B6420F` | `#A63801` | `#A83903` | `#FDA485` |
| rose | `#C4486D` | `#B63C62` | `#A82E57` | `#A82E57` | `#FF9CB3` |
| violet | `#8A5FC9` | `#7E53BC` | `#7347AF` | `#7347AF` | `#C9ADFD` |
| slate | `#657689` | `#5A6B7D` | `#4F5F71` | `#546577` | `#ADC0D4` |

Derived helpers (`video/theme.rs`): `focus_ring()` = `accent.fill` (Light) / `accent.ink` (Dark) at alpha 255
(§2.7: >= 3:1, so not faded; was the accent at 120); `control_edge()` = `line.control` opaque; `group_fill()` =
`surface.raised`; `ink_on_accent()` = white (every fill is solved for >= 4.6:1 white); `ink_secondary()` =
`text.secondary`; `knob_face()` = `surface.raised` / `text.primary`; `control_track()` = `surface.sunken` a third
of the way to `line.strong`.

Compile-time shape checks (palette_core's const block): Dark elevation by lightness (window < raised < overlay),
the text ramps ordered in both sets, every accent fill > hover > pressed and ink-dark > fill in lightness, the
discs three values, the aliases real. Kernel side (theme.rs): every pair packed, Light != Dark per token, the
jobs that must differ differing.

## 2. The rename (old role -> token), one commit

Applied by one script over every in-lane kernel painter (36 files, 449 call sites, `theme::<old>()` ->
`theme::<new>()`):

| Old role function (kit material) | Token | Why that token |
|---|---|---|
| `chrome_face` | `surface_window` | window body / title bar |
| `bevel_light`, `gloss_highlight` | `surface_raised` | the lifted surface (white on Light) |
| `bevel_shadow`, `scroll_thumb` | `line_strong` | the firm line |
| `frame_line` | `line_subtle` | separators and card borders (its uses were rules, keylines, overlay borders) |
| `title_active_top/bottom`, `title_inactive_top/bottom` | `surface_window` | §3.1: flat title bar, no gradient |
| `title_text_active`, `button_text`, `content_text` | `text_primary` | |
| `title_text_inactive` | `text_secondary` | dim ink (the clock, captions); the window title itself is `text_tertiary` |
| `button_face` | `surface_raised` | a resting button |
| `button_face_pressed`, `scroll_track` | `surface_sunken` | |
| `content_fill`, `field_bg` | `surface_content` | |
| `control_close` / `control_mid` / `control_zoom` (the crystal mark's blue ramp) | `accent_pressed` / `accent` / `accent_tint` | the mark now follows the chosen accent |
| `error_text`, `meter_high` | `ink_critical` | |
| `meter_mid` / `meter_low` | `ink_warning` / `ink_positive` | |

Files: activity appearance appswitch ceramic desktop dialog dnd dock draw facet fileview install3 instgui launcher
login loginwindow menubar paper player quarry/{attrcols,columns,getinfo,iconview,live,millercols,openwith,ops,
quicklook,sidebar,toolbar} settings shortcuts statusmenu textedit wcf wm (all under `crates/kernel/src/video/`).
By hand: `knurl.rs` leg 4 -> `material::KNURL_IN`, `ceramic.rs` leg 4 -> `material::CERAMIC`, `paper.rs` base and
tripwire -> `material::PAPER` (the retiring engines' own inputs, so their golden legs keep passing — not tokens);
`appearance.rs` selftest -> `Tok::SurfaceWindow`; `settings.rs` `sidebar_bg()` -> `surface.sidebar`; `wm.rs`
`title_row_color` -> flat `surface.window` (the gloss band retired) and the `[crispy] theme=` witness.
Ring 3: `user-una`, `user-midden`, `user-lumen` `theme.rs` (Una's sidebar = `surface.sidebar`, its console =
`surface.sunken`; Lumen's code blocks = `surface.sunken`).

Kept as forwarders, for consumers outside this lane (§9): eight legacy functions in `theme.rs` (`chrome_face`,
`bevel_light`, `bevel_shadow`, `frame_line`, `title_text_active`, `title_text_inactive`, `button_face`,
`content_text`) and eight `Tok` associated-const aliases in palette_core (`WindowBg`, `TitleActiveBottom`,
`FrameLine`, `ContentFill`, `ContentText`, `TitleTextInactive`, `FieldBg`, `ErrorText`).

## 3. The scale rule (R120, §2.1)

`video/dpi.rs`: `s2_for(ppi) = round(ppi * 2 / 110)` clamped to 1.0 ..= 4.0 (x2), `DENSITY_PPI = 110`.
221 ppi (rMBP 15") -> 4 = **2.0** (1440x900 logical), 227 (13") -> 2.0; 192 -> 1.5; 300 -> 2.5. A const block pins
those. Settings > Display (`prefs_core::modes`) already lists every half step from 1.0 to the clamp with the
panel's own as default, so 1.5 and 2.5 stay one click away; the language's quarter steps (1.75, 2.25) need a x4
scale (owed, §9). `dpi::face_ppi()` (= scale x 96) is the seam for the type lane: `text.rs` still sizes faces from
the physical 221 ppi, i.e. text 15 % larger than the 2.0 chrome until FONTSCRAFT switches it.

## 4. The frame (R122, §2.4, §3.1)

`wm::BORDER()` is **0** (was `theme::FRAME()`, 5 pt = 12 px at 2.5, 10 px at 2.0). The box is the head and the
content. The stock chrome's four-edge keyline and two bevel hairlines are deleted from `paint_window` (the lines are
held, wm.rs keeps 30465 lines up to its tail). `ui::base::FRAME`/`BEVEL` and `theme::FRAME()`/`BEVEL()` are left
for the metrics census and the aarch64 probes that still name them.

The frame was also the resize handle. With no frame the band lies OUTSIDE the box (§2.9: an invisible 6 pt band plus
12 pt corner squares): `hit_test` falls through to `resize_band_hit` (a point outside every box but inside a
resizable window's band hits that window when it is higher than what lies under it; not the shell's row, owner 0,
or a zoomed window), and `resize_zone_at` classifies over `rs_band_box` (the box grown by `RS_EDGE`). The title
strip stays the drag handle.

## 5. The shadow (R122, §2.4) — margin math, tiles, the compositor

**Levels as applied** (`desktop::CRAFT.elev`, pt / alpha of 255): elev.1 `0 1 2 @36 + 0 0 1 @26` (Dark
`0 1 2 @102`); elev.2 `0 0 2 @31 + 0 8 24 @46` (Dark `@77 / @115`); **elev.3** (focused window) `0 0 2 @31 + 0 16 40
@66` (Dark `@102 / @140`); **elev.3i** (unfocused) `0 0 2 @20 + 0 8 20 @36` (Dark `@77 / @89`); elev.4 = elev.3 +
scrim. Windows use 3 / 3i; 1 and 2 are in the table for the knob and menu lanes.

**The margin** (device px outside the box) per level: `side = max blur`, `top = max(blur - dy)`, `bottom = max(blur
+ dy)` over the layers, at `px = pt x scale`:

| scale | elev.3 top/side/bottom | elev.3i | radius | tiles (elev.3 + 3i) |
|---|---|---|---|---|
| 1.0 (QEMU) | 24 / 40 / 56 | 12 / 20 / 28 | 12 | 6.3 + 2.4 KiB |
| 2.0 (rMBP) | **48 / 80 / 112** | **24 / 40 / 56** | 24 | **24.7 + 9.2 KiB** |
| 2.5 | 60 / 100 / 140 | 30 / 50 / 70 | 30 | 38.5 + 14.3 KiB |

A 900x600 pt focused window at 2.0 has a 518 896-px ring (§2.4 estimated "about 0.5 M"); unfocused 259 056.

**The tiles.** Per (radius, level, scale, theme), once, kept (24 slots): a top and a bottom corner tile (left; the
right mirrors), a top, a bottom and a side ramp. Pixel value `1 - prod(1 - A_k F(sd_k / blur_k))`, `sd_k` the signed
distance in quarter px to the rounded box shifted down `dy_k`, `F` GLASSKIT's smoothstep over `[-blur, +blur]` (half
the peak on the edge). Integer only. Host check (`tilecheck.rs.txt` -> `tilecheck.out`, every scale x level x theme):
zero outside the margin, monotone below the box, the margin's last row 0, no seam jump above the contact layer's own
slope, and the kernel's span walk (`Cast::for_each_alpha`) equal to a per-pixel lookup on 6 763 480 pixels
including windows narrower than two corner tiles.

**The compositor.** A shadow can never be blended over what `wm` already put on the panel (it would darken on every
present). It is part of the DESKTOP instead: the background fill (`stage_fill`, the erase drain that writes the
wallpaper wherever no window box or furniture is) rewrites each row whole and multiplies every pixel by every
window's shadow there — a pixel is the same however often it is painted. `desktop::track` runs at the head of every
composite pass (folded onto `composite_inner`'s `let mut deferred = false;` line): it snapshots the casting rows
(used, above the shell, not compat, not zoomed; topmost 24), publishes them, and for every window whose box, level
(focus), theme or scale changed — or that appeared or went — queues the union of its old and new extent on the
erase queue. The same pass drains it (clipped by every box, `damage_intersecting` repainting what the extent
crosses), so a moved window's shadow lands with the move. `wm::occluders` reports the extents, so the desktop
layer's own presents never paint over a shadow; a reloaded picture marks every cast stale (`wp_take_stale`).
A shadow falls on the desktop, not on a lower window (the fill is clipped by every box): the outline (`line.strong`
@ 60 % Light, `rim.dark` Dark) and the shadow's edge on the desktop carry the depth between overlapping windows.

**The head** (`desktop::chrome_row`): flat `surface.window`, rounded top corners `radius.window` (Una 12, Softy 8;
square when zoomed), the corners outside the arc the desktop under every shadow, the 1-px outline, the
`line.subtle` title rule, discs Ø `CONTROL_BOX` filled `ctrl.*` (unfocused: all three `line.strong`), hover glyphs
black @ 50 % (×, −, two diagonal arrowheads), the title bold and centred in `text.primary` / `text.tertiary`. The
VEIL tint (bar, dock) is `surface.window` @ 72 % / 78 % (§2.4; was 92 % bar / 72 % dock).

## 6. Commands and exit codes

Seat line: `cd unaos && export UNAOS_WIFI_FW_PATH=…/UnaOS-bunker/drivers/broadcom-b43; set -a; . ./scripts/image-line.env;
set +a; export UNAOS_AETHER=1 UNAOS_MIDDEN=1 UNAOS_UNA=1 UNAOS_SONDE=1`; features as arroyo derives them (62:
`witness,selfhost,wcg-paygo,…,wc,…,facet,…,ioapic`).

| Leg | Command | rc |
|---|---|---|
| x86 kernel (both commits) | `cd unaos/crates/kernel && cargo +nightly check --release --target ../../x86_64-unaos.json -Z build-std=core,compiler_builtins,alloc -Z build-std-features=compiler-builtins-mem -Z json-target-spec --features "<the 62>"` | 0 (104 warnings, as at `6f13e5e92`; none from the touched files) |
| palette_core | `cargo check -p palette_core` | 0 |
| ring 3 (palette commit) | `cargo +nightly check --release -Z json-target-spec --target unaos/x86_64-unknown-none.json` in `user-una`, `user-midden`, `user-lumen` (`--features cryptocore`), `user-aether` (untouched; the aliases) | 0, 0, 0, 0 |
| appearance gate | `python3 unaos/scripts/appearance-check.py unaos` (the `./arroyo gates` row) | 0 — `literals_outside_theme=0 … ring3=4 core_literals=267` |
| tile math (host) | `rustc -O --edition 2021 --crate-name tilecheck tilecheck.rs.txt && ./tilecheck` | `equivalence: px=6763480 diff=0` |

Not run (the seat's rule): the aarch64 leg (`wm.rs`, `desktop.rs` and the renamed painters are also in the Pi's
`desktop_firmware` image), the `./arroyo check` matrix, QEMU, knoboff.

## 7. The wire the next boot should show

`tests desktopcraft` (registered, `tests.rs:191`), on the rMBP at 2.0, Light:

```
:: DESKTOPCRAFT: corners=ok shadow=ok hairline=ok black=ok windows=3 surface=2880x1800 -> PASS ::
:: DESKTOPCRAFT: cost clock=tsc frame_us=<n> budget_us=16667 backdrop_us=<n> chrome_us=<n> bar_us=<n> dock_us=<n> wall_ms=<n> veil_ms=<n> shadow_us=<n> tiles_us=<n> tiles_kib=34 margin=48/80/112 idle_margin=24/40/56 frame=0 track_fills=<n> black_runs=0 -> UNDER ::
```

The fields up to `veil_ms` keep B631's names and order; `shadow_us` (the desktop shaded over the three windows'
rings, the fill's clip emulated), `tiles_us`, `tiles_kib`, `margin`, `idle_margin`, `frame`, `track_fills` are new.
`shadow=` now asserts the shadow OUTSIDE the box (positive on the first row below, non-increasing, ~0 at the
margin's last row); `corners=` asserts the corner pixel is exactly the fill's shaded desktop. QEMU (scale 1.0):
`margin=24/40/56 idle_margin=12/20/28 tiles_kib=9`.

`[crispy] theme=designlang@B639 frame=0 title_h=… surfaces=0xf5f7fb/0xebedf1/0xffffff/0xffffff/0xfafcff lines=… discs=0xf0584b/0xf4bf3f/0x47c86e accent=0x4e6dd3/0x4361c5/0x3955b8 desktop=0x2d2b55` (Light, crispy).
`[kfont] … scale=2.0` and `:: APPEARANCE: … tokens=29 …` (27 + 2). `tests glasskit` and `tests settings` read live
tokens; no expected line of theirs moves.

## 8. What a QEMU or bench reading should check first

1. Drag a window: the shadow follows with no trail and no double-darkening (`track_fills` grows; the extent fill is
   one rect per moved window).
2. Focus change: the shadow grows on the focused window (elev.3) and shrinks on the other (elev.3i).
3. Appearance switch: every shadow re-fills (the tiles key on the theme).
4. A window over another: the lower window is NOT darkened under the upper one's margin (by design, §5).

## 9. Owed by other lanes (not changed here)

* **`arch/x86_64/syscall.rs` `tests winresize` (leg 1)**: its eight probes sit at `ox + 1` / `ox + ow - 2` / `oy + 1`
  / `oy + oh - 2`, which were inside the frame and are now inside the content (zone 0). They must move into the
  outside band (`ox - crate::ui::px(wm::RS_EDGE) / 2`, etc.); the drag legs press at the same points.
* **MENUSCRAFT** (`winmenu.rs`, `crystal.rs`, `notify.rs`): repoint to the tokens (`chrome_face` ->
  `surface_overlay` for menus/toasts per §2.3, `frame_line` -> `line_subtle`, `title_text_*` -> `text_primary` /
  `text_secondary`, `button_face` -> `surface_raised`, `content_text` -> `text_primary`, `bevel_light` ->
  `surface_raised`), shadow menus with `desktop::CRAFT.elev[ELEV_OVERLAY]`, then delete the LEGACY block in
  `theme.rs`.
* **The Orin track** (`arch/aarch64/display_tegra.rs`): `bevel_light`/`bevel_shadow`/`frame_line` and
  `theme::BEVEL()`; its frame probes (`bev_sh`, the strip read at `ox + ow - BORDER - 2`) test the retired frame.
* **`video/wcf.rs` CHROME-TRUTH** (aarch64 witness): `keyline_top`, `bevel_light`, `face_left` probe the retired
  frame; re-derive against `desktop::chrome_row`.
* **Witnesses that recognise an erase by `== DESKTOP_BG`**: `vacated_points` is updated here (`bg_expect`: the
  backdrop under the published shadows); `[wc-j]`'s `parked_px == DESKTOP_BG` fallback, WC-K3/WCK4's DESKTOP_BG
  pixel counts and `wcg` see shaded pixels inside a shadow extent.
* **The cloud** (`crates/user-aether/src/theme.rs`): repoint to the tokens and delete palette_core's `Tok`
  aliases. `handlers/tabula/src/highlight.rs` `token_rgb` carries copies of the OLD values (#7A7A7E, #4A73AA …).
* **syntax_core** (`libs/text/syntax_core/src/theme.rs`): its vocabulary still spells `ContentText`,
  `TitleTextInactive`, `ErrorText`; user-una maps them.
* **FONTSCRAFT**: size the faces from `dpi::face_ppi()` (scale x 96) so text follows the 2.0 chrome.
* **prefs_core::modes + dpi**: the quarter steps 1.75 / 2.25 (a x4 scale).
* **`ui.rs` metrics** (not this lane): `TITLE_HEIGHT` 34 vs §3.1's 32; disc spacing `GAP` 12 vs §3.1's 8.
* `docs/dev/OS/05_USER_EXPERIENCE/DESIGN-LANGUAGE.md` §2.3 still says the default accent is renamed `una`; R121
  overrules it (`crispy` kept) — the doc edit is the language owner's.
