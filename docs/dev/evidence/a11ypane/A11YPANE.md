# A11YPANE (rmbp-ledger B655) — the Accessibility pane, its four keys, and the glass that reads them

Branch `exec-rmbp-a11ypane`, cut from `8da376cb`. Design written before the code (R106); one compile (R107); the
QEMU and bench proof is the seat's at the fold. Typed by a first executor (two wip commits, stopped by the seat for
the model), finished, compiled and committed by an Opus executor; the wip commits' RESUME.md is folded into this file.

**Shared vocabulary touched:** the prefs schema (`unaos/libs/sys/prefs_core/src/schema.rs` `SCHEMA`, three new rows,
one widened) and `docs/dev/PREFS-SCHEMA.md`.

## The spec

* DESIGN-LANGUAGE.md (docs/dev/OS/05_USER_EXPERIENCE/) §2.6: "Settings → Accessibility → Reduce motion": toggles
  change in one frame, a slide becomes a cut. §2.3: high contrast — `text.secondary` -> `text.primary`, every
  `line.*` -> `line.control`. [S1]: text enlarged to at least 200 %.
* MOTIONKIT (B648, docs/dev/evidence/motionkit/MOTIONKIT.md): `reduced()` over the constant `REDUCE_MOTION`,
  owed "the Accessibility pane's arc adds the key and swaps that one body".
* PREFS-SCHEMA.md is the register; `tools/prefs-schema-check.py` (gate `prefs`) reds an undeclared key.

## The design

**Keys** (`schema.rs`, `system` namespace, sorted): `accessibility.cursor_size` (Float 1.0..=4.0, default 1.0),
`accessibility.increase_contrast` (Bool, false), `accessibility.text_size` (Enum default/large/larger/largest,
"default") — new; `accessibility.reduce_motion` already existed (AETHERA11Y SR119, operator-only) and is widened to
writers `settings, program` with the glass named as a reader. `accessibility.contrast` (the web's enum) stays; the
glass's switch is the bool `increase_contrast`. PREFS-SCHEMA.md mirrors each row in `render_markdown`'s format
(Rows: 57 -> 60; the reader/doc strings of the four rows compared equal to the schema's by a string comparison).
`prefs::key::A11Y_{REDUCE_MOTION,INCREASE_CONTRAST,CURSOR_SIZE,TEXT_SIZE}` folded onto the `QUARRY_FAVORITES` line
(prefs.rs is in every image; LINE-NEUTRAL, code before the `//`).

**The applied state lives in settings.rs** (desktop-gated), so the knob-off image carries no new code: four atomics
(`A11Y_REDUCE` default `motion::REDUCE_MOTION`, `A11Y_CONTRAST`, `A11Y_CURSOR_Q` in quarters 4..=16, `A11Y_TEXT`
step 0..=3), read with no lock and no bus call on a paint path: `pub fn reduce_motion()`, `increase_contrast()`,
`cursor_blocks(base) = ((base*q + 2)/4).clamp(1, ui::SCALE_MAX)`.

* Load: `a11y_load("login")` on the `mode_at_login()` line of `load_for_login`; `bus_changes_inner` marks
  `A11Y_DIRTY` on any `accessibility.*` PrefChanged (folded onto the appearance line) and `a11y_service()` (on the
  service pass after `bus_changes()`) re-reads (`via=prefchanged`). Unset keys answer the schema default.
* Store: `a11y_set(row, val, store)` latches the atomic, prints `[settings] accessibility.<key>=<v> applied=<0|1>`
  and sets a persist bit; `a11y_service` drains one PrefSet per owed key (never a bus write in the click router).
* Text size goes through the face seam that already exists: a step writes `display.font_size` (13 / 16 / 20 / 26 CSS
  px) which `video::text` polls (`prefs::int(FONT_SIZE_KEY, 9, 32)`) and sizes at CSS px x the panel's ppi / 96.
  Nothing is written at login (the store carries the px; a manual Display-tab size is not overridden).
* Motion: `motion::reduced()` returns `settings::reduce_motion()`; every `Glides::to` reads it, so the pill, the
  toggle knobs and the segmented thumbs (MOTIONKIT's three motions) are one-frame cuts while it is set.
* Cursor: `cursor.rs` `block_scale()` and `current_origin()`'s `shape_hot` scale, each edited in place with two cfg
  arms — the desktop gate takes `settings::cursor_blocks(Metrics::panel().scale)` / `block_scale(&fb)`, the knob-off
  arm is the line's old expression. The save-under is sized `(8+1) * ui::SCALE_MAX`, so the size is clamped there
  (x4 at 1:1, x2 at 2:1).
* The pane: `TABS` 9 -> 10, "Accessibility" last (`A11Y_TAB = TABS - 1`; `NP_TAB` becomes `TABS - 2`), a
  universal-access tab icon (ring, head, arms, legs; slate tile), `paint_a11y` on the kit (SETTINGSCRAFT's shape:
  groups (0,1),(2,3); toggle, toggle, slider, segmented; two help lines), `press_a11y` (a toggle row flips anywhere;
  the slider drags through PREFSUI's capture seam as `Drag.ctrl = A11Y_CURSOR_CTRL` (= `CONTROLS`), with an arm in
  `drag_value` / `drag_apply` / `drag_finish`; the drag witness names it by `NAMES.get(..).unwrap_or(..)`).
  `row_of_key` routes `accessibility.*` to the tab.
* FONTSCRAFT's owed: `Face::Chrome` -> `Face::Title` for the pane title band and the About card's name.

## `tests a11y`

Registered in tests.rs's settings block (LINE-NEUTRAL append). keys: the four `prefs::key::A11Y_*` found by
`schema::lookup`; pane: tab 9 painted offscreen records >= 6 hits, the name is "Accessibility", the search finds it,
`NP_TAB + 1 == A11Y_TAB`; snap: under the latch a moved glide returns its target with nothing running, and without
it returns less than the target with the tween running; contrast: the latch reads back both ways; cursor: x2
doubles 1 and 2 (held to `SCALE_MAX`), x4 holds `SCALE_MAX`, the store float rounds/clamps to quarters (NaN -> x1),
the slider's ends are x1 and x4; text: the steps start at `font_core::ui::DEFAULT_CSS_PX`, rise, reach 2x, sit inside
9..=32, and every step name is in the schema's enum. Every latch is restored; nothing is written to the store.

Wire: `:: A11Y: keys=4 declared=ok pane=ok reduce_motion=snap(ok) contrast=ok cursor=ok(x2) text=ok -> PASS ::`.
Login: `[settings] accessibility loaded reduce_motion=0 increase_contrast=0 cursor_size=x1.00 text_size=default
moved=<n> via=login`. `:: SETTINGS: controls=11 tabs=10 … ::`.

## The compile (the arc's one x86 line)

```
cd unaos/crates/kernel && CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/root/unaos-bench/scratch/cloud/wave34/target cargo +nightly check -Z build-std=core,compiler_builtins,alloc -Z build-std-features=compiler-builtins-mem -Z json-target-spec --target ../../x86_64-unknown-none.json --features "$(cat /root/unaos-bench/scratch/cloud/wave34/kfeats24.txt),stria,gen7r9,wifi9,usbnet,wifi10"
```

Before it, the one fix the resume note predicted: `4.min(max)` (an ambiguous numeric literal) -> `4usize.min(max)`
in `selftest_a11y`. Run 1: rc=0 (`Checking prefs_core`, `Checking unaos-kernel`). New warning from this arc: one
`unused_braces` at cursor.rs:3813 (the wrapping block that carries the two cfg arms in argument position; it is
needed for the attributes and is harmless). settings.rs's `row_of` / `sel` dead-code warnings predate the arc.
aarch64 not compiled (R107; the arc names the x86 line).

## Gates (`cd unaos && ./arroyo gates`)

`GATES: 17 run 16 green -> FAIL`: `prefs` `declared=63 referenced=59 undeclared=0 -> PASS`, `appearance`
`literals_outside_theme=0`, `charter`, `status` among the green. The one red is `deps` — `deps-audit --check`: tokio
1.53.3 published upstream, every lockfile resolves 1.53.2 (a live registry read; no Cargo file is in this diff and
none is this arc's). The schema_gate's byte-equality (`cargo test -p prefs_core --test schema_gate`) is a cargo
test (R107: the seat's); the four rows were compared to the schema strings, and the type/default renderings read off
`kind_text` / `default_text` / `write_float`.

## Owed (out of lane; the exact change, not made)

1. `video/theme.rs` (not this arc's file): when `#[cfg(<desktop gate>)] crate::video::settings::increase_contrast()`
   (OFF arm `false`; theme.rs is in every image), `ink_secondary()` returns `text_primary()` and the hairline /
   `line.*` roles return the control line. Until then the toggle sets and stores a latch nothing paints from.
2. `video/settingsfiles.rs` `domains_of(tab)`: no row for tab 9, so the pane has no footer file link
   (`prefs_core::files::domain_of` files the keys under `general`). A row is the seat's call.
3. `unaos/scripts/specs/x86-wc.spec:970` REQUIREs `:: SETTINGS: controls=[0-9]+ tabs=4 …` — already stale at 9 tabs;
   the wire is now `tabs=10` (use `tabs=[0-9]+`). No spec REQUIREs `tabs=9`.
4. `text.rs` does not call `dpi::face_ppi` (FONTSCRAFT's switch); text size reaches the faces through
   `display.font_size`.
5. The `deps` gate's tokio 1.53.2 -> 1.53.3 lock bump (repo-wide).
