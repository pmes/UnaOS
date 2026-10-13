# DESIGNFIX — DESIGNAPPLY's four owed items outside its lane

*rmbp-ledger B649. Branch `exec-rmbp-designfix` from `8da376cb`. Source: DESIGNAPPLY (B639,
`docs/dev/evidence/designapply/DESIGNAPPLY.md` §9 "Owed by other lanes") and DESIGN-LANGUAGE §2.3's token table
(`docs/dev/OS/05_USER_EXPERIENCE/DESIGN-LANGUAGE.md`), §2.9 (the outside band).*

Measured here: compile-level only (the two kernel checks the arc names) and the host gates. No pixel seen on glass;
§5 is the wire the fold's run should show.

## 1. `tests winresize` leg 1 — the probes move into the outside band (`arch/x86_64/syscall.rs`)

B639 set `wm::BORDER()` to 0 and put the resize band OUTSIDE the box (`wm::rs_band_box`: the outer box grown by
`ui::px(RS_EDGE)` on every side; `wm::resize_band_hit` at `hit_test`'s tail; `rs_zone_of` classifies over that
grown box). The fixture's eight probes and every press point sat at `ox + 1` / `ox + ow - 2` / `oy + 1` /
`oy + oh - 2` — inside the frame until B639, inside the CONTENT after it (zone 0: `rs_zone_of` returns 0 for a point
in the content box), so `zones=0`, the hover leg saw no shape and every routed drag pressed on the app.

The fix (line-neutral, every edit in place): `e = ui::px(wm::RS_EDGE)` and the four mid-band coordinates
`bl = ox - 1 - e/2`, `bt = oy - 1 - e/2`, `br = ox + ow + e/2`, `bb = oy + oh + e/2` folded onto the `(ow, oh)`
line; the probe table reads them; `(brx, bry) = (br, bb)`; legs 3, 4 and 5 re-derive the BR grab from the live row
as `box right/bottom edge + e/2` (was `- 2`). Why those points: in the band box `(ox - e, …, ow + 2e, …)` the left
probe's `dl = e - 1 - e/2 < e` (`RS_EDGE` band) and `< RS_CORNER`'s corner only together with `dt`, so the mid-edge
probes classify L/R/T/B and the corner probes L|T, R|T, L|B, R|B; the `- 1` keeps `bl`/`bt` outside the box when
`e` rounds to 1. `strip_clear` (the title strip is the drag handle, zone 0) is unchanged. The cursor leg hovers the
same eight points (`wc_resize_hover_at` → `hit_test` → `resize_band_hit` → `resize_zone_at`).

Press points legs 3–5 are part of the same geometry (DESIGNAPPLY §9: "the drag legs press at the same points"); left
at `- 2` they press on the content and `drag` returns `None`, so they moved with leg 1. After the clamp leg the box's
right edge may sit AT the panel edge, so the band there is off-panel; the router takes the fixture's coordinates
unclamped (`wc_click_route_at(ev, x, y)` → `hit_test`, pure arithmetic), so the legs still reach the band.

## 2. Legacy role calls → semantic roles (DESIGN-LANGUAGE §2.3)

| file:line | legacy call | semantic role | token fn | note |
|---|---|---|---|---|
| `video/winmenu.rs:1686` | `theme::bevel_light()` | the open title's label on the accent pill (`theme::accent()` fill, :1673) | `theme::ink_on_accent()` | §2.3: white text on the accent fill. The forwarder gave `surface.raised`, a SURFACE token used as ink: value-identical in Light (`ON_FILL` = #FFFFFF), dark grey on the accent in Dark — that Dark caption is the bug the alias carried. The bar's own app-menu caption (`menubar.rs:1974`, MENUS2's, outside this arc's FILES) still reads `surface_raised()` and owes the same one-token change so the two lit titles agree in Dark |
| `video/winmenu.rs:1686` | `theme::title_text_active()` | the bar's label ink | `theme::text_primary()` | `text.primary` |
| `video/crystal.rs:1550` | `theme::title_text_active()` (`sheet::text_primary`) | an item's label | `theme::text_primary()` | `text.primary` |
| `video/crystal.rs:1560` | `theme::title_text_inactive()` (`sheet::text_disabled`) | a disabled item | `theme::text_disabled()` | the wrapper's own doc names `text.disabled` (§2.3: disabled labels, exempt from 1.4.3); the forwarder gave `text.secondary` — value moves #595B5E→#A3A5A8 Light, #AFB1B5→#5C5E61 Dark. `tests menuscraft` paints no labels (its white census is the sheet's rows and the pill) |
| `video/crystal.rs:1748` | `theme::chrome_face()` | the head band under a sheet's shadow estimate | `theme::surface_window()` | the head is flat `surface.window` (`desktop::chrome_row`) |
| `arch/aarch64/display_tegra.rs:603-606`, `:3916-3919` | `theme::frame_line()` ×8 | keyline probe `want` | `theme::line_subtle()` | value-identical (the forwarder's target). The `kl_*`/`bev_*` probes still test the RETIRED frame (B639 §9: the Orin track's) — role calls only here, positions untouched |
| `display_tegra.rs:607`, `:3920` | `theme::bevel_light()` ×2 | bevel-light probe `want` | `theme::surface_raised()` | as above |
| `display_tegra.rs:608`, `:3921` | `theme::bevel_shadow()` ×2 | bevel-shadow probe `want` | `theme::line_strong()` | as above |
| `crates/user-aether/src/theme.rs:80-101` | `Tok::TitleActiveBottom` | status band, inactive tab | `Tok::SurfaceWindow` | the alias targets (palette_core:128-135), value-identical, user-una's spelling |
| | `Tok::ErrorText` | the alert band's tint | `Tok::InkCritical` | |
| | `Tok::FrameLine` ×2 | status rule, chrome rule | `Tok::LineSubtle` | |
| | `Tok::ContentText` ×3 | status / chrome / field ink | `Tok::TextPrimary` | |
| | `Tok::WindowBg` ×2 | chrome ground, hover wash | `Tok::SurfaceWindow` | |
| | `Tok::ContentFill` | the active tab | `Tok::SurfaceContent` | |
| | `Tok::TitleTextInactive` | dimmed chrome ink | `Tok::TextSecondary` | |
| | `Tok::FieldBg` ×2 | the address field | `Tok::SurfaceContent` | |

**Use count after the change** (`grep -rn -E "theme::(chrome_face|bevel_light|bevel_shadow|frame_line|title_text_active|title_text_inactive|button_face|content_text)\(" unaos/crates/kernel/src`, `theme.rs` excluded; `grep -rn -E "Tok::(WindowBg|TitleActiveBottom|FrameLine|ContentFill|ContentText|TitleTextInactive|FieldBg|ErrorText)\b" --include=*.rs unaos libs handlers`, `palette_core` excluded):

| where | before | after |
|---|---|---|
| this arc's files (winmenu, crystal, wcf, display_tegra, user-aether) | 4 + 12 + 13 Tok | **0** |
| `video/notify.rs` (SOFTY3's) | 16 | 16 |
| `video/menubar.rs` (MENUS2's) | 3 | 3 |
| `video/quarry/toolbar.rs` (MENUS2's) | 1 | 1 |
| `video/softy/palette.rs` (Tok aliases; SOFTY's) | 6 | 6 |

So the aliases cannot go yet: 20 legacy fn calls and 6 Tok alias reads remain in files outside this arc's FILES line
(the arc names them NOT to touch). The seat removes the LEGACY block and the Tok aliases once those lanes land.

## 3. `wcf.rs` CHROME-TRUTH — re-pointed at the frameless chrome

Until now the five probes read the retired frame: `keyline_top` at the box's top row (now the blended outline),
`bevel_light` one `theme::BEVEL` in (now the strip), `title_top`/`title_bot` as `ceramic::shade(title_row_color)`
(the Una head is painted flat OVER the stock strip), `face_left` at `bx + BORDER - 2` = `bx - 2` (OUTSIDE the box).

The new five, derived from `desktop::chrome_row` (every expectation an exact token: `fill_rrect` writes the colour
exactly at full coverage, `hairline_h` is an opaque fill; the outline is `stroke_rrect` width 1 at `x = 0`,
`x = bw - 1` and row 0 only, never probed):

| probe | at | want |
|---|---|---|
| `edge_left` | `(bx + 1, by + TITLE_H/2)` | `surface_window` — the 0 px border: one px inside the edge is the strip |
| `edge_right` | `(bx + bw - 2, by + TITLE_H/2)` | `surface_window` |
| `strip_top` | `(bx + bw - TITLE_H - 2, by + 1)` | `surface_window` (index 2 → `title_grad`) |
| `strip_bot` | `(bx + bw - TITLE_H - 2, by + TITLE_H - 2)` | `surface_window` (index 3) |
| `title_rule` | `(bx + bw/2, by + TITLE_H - 1)` | `line_subtle` |

Clearances: `head_radius <= TITLE_H` and `TITLE_H/2 >= radius` (12 pt vs 34 pt at every scale), so the columns and
mid-row are past the arcs; the title is centred inside a reserve at least the disc cluster wide (> `TITLE_H + 2`),
and rows 1 / `TITLE_H - 2` are outside its cell. The skip guard became `bw < TITLE_H + 4`. A seventh reading,
`pt=band` at `(bx - 1 - ui::px(RS_EDGE)/2, by + TITLE_H/2)`, reports the outside band (`-> BAND`, not judged: the
desktop under the window's own shadow). `title_grad` now reads the strip's flatness (0 expected). `wcf.rs` registers
no `tests` name: `chrome_truth` runs from the composite (`wm.rs:6510`, x86 `wc` / aarch64 `desktop_firmware`); the
reader is `scripts/specs/pi4-regression.spec:903-905` (`REQUIRE \[chrome-truth\] verdict … -> PASS`). Under the
Softy look the head is Softy's painter and these Una expectations MISS — as the old ones did.

## 4. Not done here, named

* `display_tegra.rs`'s frame probes (`kl_*`, `bev_*`, `theme::BEVEL()`, the strip read at `ox + ow - BORDER - 2`)
  still measure the retired frame — the Orin track's (B639 §9); this arc was role calls only.
* `menubar.rs:1974`'s open-caption ink: `theme::surface_raised()` → `theme::ink_on_accent()` (MENUS2's file; until it lands, in Dark the app-menu caption is grey on the accent while the other lit titles are white).
* The remaining 26 legacy reads (§2's table) and then the LEGACY block / Tok aliases.

## 5. Commands and the wire

Compile (the arc's two lines, each once): x86 `cargo +nightly check … x86_64-unknown-none.json` rc=0 and aarch64
`… aarch64-base.json` rc=0 (logs `~/unaos-bench/scratch/cloud/wave34/designfix/{x86,arm}.log`; no warning in a touched
line). `./arroyo gates`: `GATES: 17 run 16 green -> FAIL` — the one red is `deps` (tokio 1.53.3 published upstream,
every lock resolves 1.53.2; a `Cargo.lock` bump, no file of this arc). `user-aether` is not compiled by either (it is a ring-3 crate; the
rename is eight identifier swaps against `palette_core`'s enum, `cargo +nightly check` of that crate is owed to the
fold).

Wire, x86 QEMU (`x86-wc.spec:44`): `:: WINRESIZE: zones=8 strip=true min=160x64 drags=<n>=3+ clamped=<n> drag=true
clamp=true minstop=true aspect=true keys=true cursor=ok cursor_shapes=<n> -> PASS ::`. Chrome-truth on an armed
desktop: five `HIT` lines per window (`pt=edge_left … pt=title_rule`), `pt=content … -> APP`, `pt=band … -> BAND`,
then `[chrome-truth] verdict wins=<n> hits=<5n>/<5n> title_grad=0 … -> PASS`.
