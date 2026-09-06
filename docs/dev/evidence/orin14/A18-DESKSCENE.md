# A18 — DESKSCENE: the strip and its embedded pulse leave the cascaded scene, the pulse window returns

Executor A18, seat orin 14, track `hw-jetson`, base `6cc8de8c`. Ruling: `docs/dev/RULINGS.md` R17
(Peter, 2026-09-05): "it looks like you killed the wrong pulse. the windowed one is gone, the
embedded one is there and so is the old status bar at the bottom. both must go."

Files touched: `unaos/crates/kernel/src/main.rs` (tegra `orinrender` region only:
`orin_desk_scene_up`, `tegra_render_arm`, `orin_render_service`), `docs/dev/OS/orin-ledger.md`,
this file. No `video/` edit was needed — every symbol the fix reaches is existing `pub` API.

## M1 — why the strip was on the panel after the cascade

The predicate the strip retires on is `crate::video::desktop_scene_owns_backdrop()`
(`video/mod.rs:752`, a load of `DESKTOP_SCENE`, `video/mod.rs:715`). It has exactly ONE writer,
`retire_desktop_chrome(pw, ph)` (`video/mod.rs:728`, `swap(true)`), and that function has exactly
ONE call site in the tree: `main.rs:5576`, the Pi render pass's SHELLWIN-PI mint arm, folded onto the
`pal.clear_screen(DESKTOP_BG)` line — the Pi retires the legacy tenants in the same statement that
claims the backdrop, after `open_shell_window` returned a window.

    grep -rn 'retire_desktop_chrome\|DESKTOP_SCENE\b' unaos/crates/kernel/src
      video/mod.rs:715 (static), :728 (fn, the swap), :753 (load); main.rs:5576 (Pi call)

`video/desktop_firmware.rs::activate()` (:83) never calls it — its steps are desktop-clear,
`fbcon::panel_console_window_open` (:237), `menubar::set_enabled`, crystal, `pulsewin::arm()` (:373),
quarry. The Orin cascade seam `tegra_desk_cascade` (main.rs) calls `activate()` and reads back
`menubar::enabled()`; neither it nor `tegra_render_arm` / `orin_render_service` ever touched the latch.

So on the Orin `desktop_scene_owns_backdrop()` read FALSE for the whole boot and all three REALDESK
seams in `ui_status.rs` took their legacy arm: `draw` (:381) painted the LED band + status line into
the render pass's back buffer; `chrome_h` (:608) reserved 92 + 12 = 104 rows (render3b wire:
`[pstrip] armed cores=6 panel=(0,1096,1920x92) … strip_h=12 reserved=104`); `tick` (:1285) returned
`changed` unmasked, so every ~1 Hz change was a `pal.render()` (`presents=29`). The screenshot
`~/unaos-bench/scratch/orin13/render3b-SCREEN0.png` shows exactly that band at y 1096..1200 with the
dock sitting on its c1..c5 rows.

The pulse window: `activate()` step 6 armed it (`[pidesk] pulse-window ARMED`), but the only opener
is `pulsewin::service()`'s open arm (`video/pulsewin.rs:578–608`, `if ARMED && ncpu > 0 { open() }`)
and CASCADE a20839c6 removed that call from the pass. Armed, never serviced: 0 `[pulsewin] open`
lines (A10).

## M2 — the fix (all inside `#[cfg(all(target_arch = "aarch64", feature = "orinrender"))]`)

1. `orin_desk_scene_up()` — new helper above `tegra_render_arm`: `cfg!(feature = "deskcascade")
   && menubar::enabled() && fbcon::console_is_routed()` — the cascade's own CASCADED readback
   (`bar=1`) plus "the console is no longer the wallpaper" (`route=ROUTED`). Behind the `deskcascade`
   cfg so the partial seams (`orinfurn` + `orinconwin`) keep their strip — the Pi's rule is that the
   instrument stays wherever the shell still owns the backdrop.
2. `orin_render_service` head, right after the back-buffer seed `screen.fill_screen(DESKTOP_BG)`:
   `if scene { retire_desktop_chrome(pw, ph) } else { "[orinrender] strip=kept reason=no-scene …" }`.
   The cascade ran on the boot core before this task was spawned (`tegra_desk_cascade` precedes
   `tegra_render_arm` on the terminus line, main.rs:2717), so task start IS the Pi's "after the
   cascade, with the console in a window" instant. After it `draw` is a no-op, `chrome_h` shrinks to
   `dock_reserve_h()` = 64 (`[realdesk] … bottom_reserved=104->64` is the witness), `tick` never asks
   for a present. The sampler and `loads()` are untouched — `pulsewin` reads them.
3. `dirty |= ui_status::tick(&mut pal); #[cfg(feature = "desktop_firmware")] pulsewin::service();`
   — the Pi's fold (main.rs:5532), restored after `tick`, before the present. `loads()`
   (`ui_status.rs:946`) answers `ncpu` as soon as `tick`'s arming pass set `st.armed`, so the window
   OPENS on pass 1. `draw_panel_at(Some(rect))` (`ui_status.rs:956`) is not gated on the latch, so
   the window's lamps paint after the strip retired. `service()` returns `()` and presents through
   its own `wm` row.
4. Dirty: `dirty |= passes == 1` — the pass's own product on the cascaded scene is the backdrop seed,
   presented once (through the occluder walk); after pass 1 `tick` is the only source and on the
   cascaded scene it is never. At most one present per pass, unchanged. The `[u7stk]
   orin-render:pass1` probe stays inside `if dirty` and now reads the pass that opened the window.
5. Wire: `[orinrender] arm … cascade=1 scene=1`; `[orinrender] census … strip=retired pulsewin=<id>`.

P7 position proof (statement precedes the line's first `//`): `python3` scan of main.rs —
line 8358 `stmt_at 116 first_//_at 137 OK` (the Pi's twin at 5532: `120 / 141 OK`).
Every hunk of the diff lies in 8109–8408 (`git diff -U0 | grep '^@@'`), inside the two
`orinrender`-gated fns and the new helper; nothing below 8115 in main.rs is compiled into a knob-off
image (every item from 8115 to EOF carries `orinrender` or `deskcascade`), so no Pi panic `Location`
moves.

## M3 — the pulse window's box vs the console window, 1920x1200, from code

Inputs (`video/pulsewin.rs:236–262`, `ui.rs:72–81`, `video/wm.rs:831`, `theme.rs`):
`Metrics::for_height(1)` → scale 1, cell 8, `line_h` 12; `ncpu` = min(8, 6) = 6;
`cw` = min(1920·2/3, 1910) = **1280**; `ch` = 12 + 2·6 + 6·24 = **168**; `BORDER` 5, `TITLE_H` 34 →
outer **1290x212** (scale 1 — matches the render2 wire `[pulsewin] open … surf=1280x168 box=1290x212`).
`ox` = 2·BORDER = 10; `oy` = 1200 − `chrome_h(1200)` − 10 − 212.

| state | `chrome_h` | pulse box (x, y, w×h) | vs console box y 158..938 |
|---|---|---|---|
| before retire (render2 wire) | 104 = 92 + 12 | (10, 874) 1290×212 → y 874..1086 | **64 px** overlap (S15's Orin number) |
| after retire (this arc) | 64 = `strip::PAD` 12 + `dock::STRIP_H` 52 (28 + 2·12) | (10, **914**) 1290×212 → y 914..1126 | **24 px** of the box (914..938), **19 px** of the text area |

Console window (render3b wire, placed by the cascade with the pre-retire reservation, pinned):
`[wc-x] console-window win=1 … surf=1295x736 box=1305x780 at (307,158) cell=7x16 cols=185 rows=46`
→ box x 307..1612, y 158..938; text origin (312, 197), row r at y 197+16r: row 44 = 901..917,
row 45 (the prompt row) = 917..933.

Overlap after retire: x 307..1300 (993 px) × y 914..938 (24 px) = 993×24 px of the console BOX,
of which 19 px (914..933) is text: **row 45 (the prompt row) fully, plus 3 px of row 44** — 2 rows
touched, 1 fully, down from 4 (S15/B1). What covers the console is the pulse window's frame + TITLE BAR
(y 914..953 = BORDER 5 + TITLE_H 34); its content (y 953..1121) starts 15 px below the console box. The pulse box bottom
(1126) clears the dock (`ph − PAD − STRIP_H` = 1136) by 10 px.

Not applied: `docs/dev/evidence/orin13/pulseoccl-fbcon.patch` (rmbp's, `video/`). The residual
1-row overlap is its problem statement, now smaller.

## Gate (EXECUTOR-BRIEF item 6), 2026-09-05, logs in `~/unaos-bench/scratch/orin14/a18/`

- `cd unaos && ./arroyo check` → **exit 0** (61 `Finished` legs, 0 `error` lines; `check1.log`).
- `./arroyo test-arm 60` → **exit 0**; `target/serial-arm.log` **478 lines**; positive witnesses
  (`awk '/== witness ::|-> HONOURED|-> LIVE|-> OK|PASS$| PASS /'`) **57**; the fault scanner's one
  `FAIL` substring is BOT prose ("behind a FAILED set-deq"), not a verdict (`testarm.log`).
- `UNAOS_TEGRA=1 UNAOS_TEGRA_EL0=1 UNAOS_WITNESS=1 UNAOS_ORINRENDER=1 UNAOS_DESKCASCADE=1
  UNAOS_ORINRX=1 UNAOS_HOLOCRON=1 ./arroyo esp-jetson` → **exit 0** (`espjetson.log`);
  `strings target/aarch64_esp/kernel.elf | grep -c 'pulsewin'` → **27** (reachability);
  `… | grep -c '\[orinrender\] strip=kept'` → 1; `… | grep -c 'scene=1 is the cascade'` → 1.

## Expected wire on the next flight (same knob line, card written by the seat)

    [deskcascade] -> CASCADED windows=1 bar=1 owns_pixels=1 route=ROUTED activate=false …   (unchanged)
    [u7stk] at=boot-core:post-cascade …                                                     (unchanged)
    [orinrender] arm conwin=0 furn=0 tenant=0 click=0 desk=0 cascade=1 scene=1 (…)
    [orinrender] spawned tid=… cpu=0 pinned=1 …
    [realdesk] backdrop=desktop-scene retired=pulse-band,status-line bottom_reserved=104->64 panel=1920x1200 menubar=Some((0, 0, 1920, 34)) dock_h=64 was=false == witness ::
    [orinrender] DECLINE reason=console-already-windowed …
    [pstrip] armed cores=6 … reserved=64 …
    [pulsewin] open win=2 panel=1920x1200 surf=1280x168 box=1290x212 at (10,914) view=Pi LED lamps (…)
    [u7stk] at=orin-render:pass1 …
    [orinrender] census passes=1 presents=1 win=0 declined=1 strip=retired pulsewin=2 -> RENDER-LIVE
    … census … presents=1 (steady — no strip repaints) strip=retired pulsewin=2 …

On the panel: no LED band and no status line at the bottom; the dock alone in the bottom rows; the
pulse window bottom-left with the six lamp rows; the console window unchanged at (307,158). Esc on
the pulse window's menu is A10's own question and is NOT verified by this arc.
