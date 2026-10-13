# WINEDGE — a window has a visible edge; the shadow check reads the cast (rmbp-ledger B659)

Branch `exec-rmbp-winedge`, cut from `45e03964`. Files: `unaos/crates/kernel/src/video/desktop.rs`,
`unaos/crates/kernel/src/video/wm.rs` (`paint_window` only, two LINE-NEUTRAL folds),
`docs/dev/OS/05_USER_EXPERIENCE/DESIGN-LANGUAGE.md` (§2.9 amendment, §3.1 Outline row), `docs/dev/OS/rmbp-ledger.md`.

## 1. What Peter said, and what the wire said

Flight 35 (`docs/dev/evidence/rmbp-0915/flight35/FLIGHT35.md`, "Peter, verbatim"): *"new prefs panel looking very
nice. new borders not so much."* DESIGNAPPLY (B639) removed the 5 pt frame and bevel and left the window's edge to its
shadow outside the box; to him that does not read as an edge. Boot 1 (`f35-boot1.log`, `tests desktopcraft`):

```
:: DESKTOPCRAFT: corners=ok shadow=FAIL hairline=ok black=ok windows=3 surface=2880x1800 -> FAIL ::
:: DESKTOPCRAFT: cost clock=tsc frame_us=15179 ... shadow_us=1790 tiles_us=2124 tiles_kib=34 margin=48/80/112 idle_margin=24/40/56 frame=0 track_fills=4 black_runs=0 -> UNDER ::
```

## 2. The shadow FAIL: the check, not the cast

The clause DESIGNAPPLY wrote (`desktop.rs`, `selftest`, the `shadow:` block at `45e03964`) began `let mut prev =
i64::MAX;` and failed a row on `dk > prev + 3`. Every kernel image is built `--release` (`unaos/arroyo`'s note at the
`--release` legs) and no profile turns overflow checks on, so on the FIRST row `i64::MAX + 3` wraps to `i64::MIN + 2`,
`dk > i64::MIN + 2` is true, and `shadow=false` — at every scale, in both themes, whatever the tiles cast.

The cast itself is right. `edgecheck.rs.txt` is DESIGNAPPLY's host copy of the tile functions (`tilecheck.rs.txt`,
verbatim) plus `wall_gen`, `keep_rgb`/`keep_of` and the fixture's three boxes; built with `-C overflow-checks=off` (the
release semantics) it runs the old clause literally and the new one (`edgecheck.out`):

```
s2=2 dark=0 bot=56 old_clause=FAIL (first=76 last=0) new_clause=ok ramp_a=59/0 ramp_dark=76/0 mono=true
s2=2 dark=1 bot=56 old_clause=FAIL (first=165 last=0) new_clause=ok ramp_a=128/0 ramp_dark=165/0 mono=true
s2=4 dark=0 bot=112 old_clause=FAIL (first=81 last=0) new_clause=ok ramp_a=62/0 ramp_dark=81/0 mono=true
s2=4 dark=1 bot=112 old_clause=FAIL (first=172 last=0) new_clause=ok ramp_a=133/0 ramp_dark=172/0 mono=true
```

At 2.0 (s2=4) the bottom ramp below the focused box falls 62 -> 0 with no step up (`mono=true`), so the cast is not
changed. (The Dark rows reuse the Light wallpaper; the clause under test is the ramp, not the colour.)

The new clause (`desktop.rs` `selftest`, the `shadow (WINEDGE B659)` block): over the bottom margin's `bot` rows at
the focused box's mid-width, (a) the cast's own alpha, read through `Cast::for_each_alpha` (the reader the fill uses),
is > 0 on the first row, never rises row to row, and is <= 2 on the last; (b) each panel pixel there is EXACTLY the
wallpaper shaded by every cast (`shade_words` on that one pixel, as `corners` reads its pixel). The darkness is no
longer asserted (the wallpaper is ordered-dithered, so its row-to-row step is the dither's); its first and last row
are printed as `ramp_dark=` beside `ramp_a=`.

## 3. The edge

* `desktop::edge_ink()` — `line.strong` on Light, `line.subtle` on Dark (§2.3 roles; no literal).
* The head's stretch: `chrome_row`'s `stroke_rrect` now strokes with `Ink::solid(edge_ink())` (was `line.strong` @ 60 %
  on Light, `rim.dark` on Dark); the title rule runs from column 1 to `bw - 2` so the edge column stays one line.
* The sides and the foot: `desktop::edge_spans(bw, bh, j)` (box-local spans: the two outer columns on every row
  below the head, the whole last row) painted by `desktop::edge_paint` from `wm::paint_window`'s TAIL, after the
  content upscale. With `BORDER()` = 0 the content reaches the box's sides and foot, so anything laid before it — the
  keyline's old seat at `PW_FRAME`, and DESIGNAPPLY's head-only outline — is painted over on every present; that is
  why the edge rides the tail. It stamps `PW_FRAME` again so a wedge in it is named. Inside the box: the shadow still
  starts one pixel past it, the resize band and the occluder extents do not move.
* `wm.rs` is touched on two lines and keeps 30575 lines: the tail call folded onto `comp_mark(r.id, mk + PW_EXIT);`
  under the desktop arm's cfg (`all(x86_64, wc)` / `all(aarch64, desktop_firmware)`), code before the line's first
  `//`; and a pointer comment appended to `let kl = super::theme::line_strong();`.
* Una only. Softy's painter (`softy::chrome::compose_row`) strokes its own 1 px border before the content, so below the
  head the content paints over it too: owed to that painter's lane (not this arc's files).
* `tests desktopcraft` paints the same spans after its content and asserts `edge=ok(1px)`: the focused box's outer
  column at mid-content (both sides) and its last row at mid-width are `edge_ink`; one pixel further in is the content;
  the head's side one pixel below the corner arc is `edge_ink` and the pixel inside it is not; one pixel outside on
  the left and the first row below are the shaded desktop.

## 4. The capture Peter judges

The deliverable is the screenshot; **his eyes decide**, not the fixture. Take the STAGED desktop: `UNAOS_SHOTSTAGE=1`
(DESIGN-LANGUAGE §4's arrangement — Aether focused over Settings and Lumen — staged at login), then Cmd-Shift-3
(`PRTSCR … SCREEN0.PNG 2880x1800`). SHOTSTAGE is queued (`docs/dev/OS/rmbp-queue.md` item 7, B643) and is not in
this tree; until it lands, open Aether, Settings and Lumen by hand, focus Aether, and capture. Flight 35's boot 3 lost
its glass 2 s after a Cmd-Shift-3 (GATESTEAL, the `prtscr` pump step): take the capture after GATESTEAL is folded, or
photograph the panel. Look at: the edge on all four sides of each window, one device px, continuous through the
rounded head; the shadow beyond it; the title rule meeting the edge without a notch; Light and Dark.

## 5. Legs

| Leg | Command | rc |
|---|---|---|
| x86 kernel | `cd unaos/crates/kernel && CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/root/unaos-bench/scratch/cloud/wave35/target cargo +nightly check -Z build-std=core,compiler_builtins,alloc -Z build-std-features=compiler-builtins-mem -Z json-target-spec --target ../../x86_64-unknown-none.json --features "$(cat /root/unaos-bench/scratch/cloud/wave35/kfeats24.txt),stria,gen7r9,wifi9,usbnet,wifi10"` | 0 (no warning from `desktop.rs` or `wm.rs`'s two lines) |
| host copy | `rustc -O -C overflow-checks=off --edition 2021 --crate-name edgecheck edgecheck.rs.txt -o edgecheck && ./edgecheck` | 0 (`edgecheck.out`) |
| pre-commit | `cd unaos && ./arroyo gates` | see the commit's report |

Not run (the seat's rule): QEMU, the aarch64 leg (`desktop.rs` and `paint_window` are also in the Pi's
`desktop_firmware` image), knoboff (the fold measures it; the wm.rs change is two same-line folds, the call under the
desktop arm's cfg).

## 6. The wire the next boot should show

`tests desktopcraft` on the rMBP at 2.0, Light:

```
:: DESKTOPCRAFT: corners=ok shadow=ok edge=ok(1px) hairline=ok black=ok windows=3 surface=2880x1800 -> PASS ::
:: DESKTOPCRAFT: cost clock=tsc frame_us=<n> budget_us=16667 ... margin=48/80/112 idle_margin=24/40/56 ramp_a=62/0 ramp_dark=81/0 frame=0 track_fills=<n> black_runs=0 -> UNDER ::
```

QEMU (scale 1.0): `margin=24/40/56 idle_margin=12/20/28 ramp_a=59/0 ramp_dark=76/0` (the host copy's s2=2 Light row).
