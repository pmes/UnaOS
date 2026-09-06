# A20 — ORINCLICK: `orinclick` composes with `deskcascade` as the code stands; render5 adds the knob

Executor ORINCLICK, seat orin 14, track `hw-jetson`, base `2a04fb4a`. Question (orin-ledger A20,
`FLIGHT-RESULT-render4.md` §Observed): on render4 Peter saw "keys ok but clicks not landing"; the
render arm line printed `click=0` because `UNAOS_ORINCLICK` was not in the flight's knob line. Before
the next flight adds it, does `orinclick` (`Cargo.toml:2034`, `orinclick = ["tegra_el0"]`;
`arroyo:909`, ORIN-CLICK rung 3) COMPOSE with `deskcascade` (`Cargo.toml:2367`,
`deskcascade = ["desktop_firmware", "tegra_el0"]`; `arroyo:1280`)?

**Verdict: it composes. No code change.** The three questions, each answered from the source with
the grep that could have found the opposite. All paths are under `unaos/crates/kernel/src/`.

## (a) `orinclick` mints no `wm` row before `tegra_desk_cascade` runs

The cascade refuses `table-not-empty` at `main.rs:8586-8590` (`wm::count() != 0`). `orinclick` can
only trip that floor if it creates a row first. It cannot:

1. **Every `wm::` call the `orinclick` block makes is a read.** The block is
   `arch/aarch64/display_tegra.rs:1081-1530` (`orin_click` at :1309, `orin_click_census` at :1441,
   the `CLK_*` statics between). Census of its `wm::` calls:

       awk 'NR>=1081 && NR<=1530 && /wm::/' src/arch/aarch64/display_tegra.rs | grep -o 'wm::[a-z_]*' | sort | uniq -c
         1 wm::close_box_hit   3 wm::compat_live   3 wm::count   2 wm::hit_test

   No `create`, `create_at`, `open`, `register`, `mint`. The one `wm::create`-shaped string in the
   block (:1105) is a doc comment.

2. **Both call sites are inside the pump's phase-2 drain loop, which runs after the cascade.**
   The two `#[cfg(feature = "orinclick")]` statements in `main.rs` are :2979 (`orin_click(mask)`,
   the pump's `Event::Button` arm) and :3014 (`orin_click_census(sweep_tick)`, the ~250 ms sweep);
   their `supstate` twins (:7644, :7658) are inside `jd2_supstate_phase2`, and `supstate` is not in
   render5's knob line. The terminus line `main.rs:2717` carries no `orinclick` statement at all —
   its order is `tegra_el0_start_maybe` → `[tegradesk]` → `[orinconwin]` → `[orintenant]` →
   `[orinladder]` → `[orinfurn]` → **`[deskcascade] tegra_desk_cascade()`** →
   `[orinrender] tegra_render_arm()` → `tegra_rast_demo_maybe` → `run_capstone_boot_core(0)`.
   The pump is spawned pinned to core 0 (`main.rs:2495`,
   `sched::spawn("jd2-console", jd2_console_pump, 0, 0)`) and cannot run until the boot core enters
   that run loop, i.e. after the cascade returned. The render4 wire confirms the order:
   `[deskcascade] -> CASCADED windows=1` → `[pulsewin] open win=2` → `:: tegra: JD20 — pointer live`
   (`render4-boot1.log`).

Consequence for the wire: the first census call prints `[orinclick] arm … rows=2 compat=0 … -> ARMED`
(`display_tegra.rs:1473-1477`; rows = console window 1 + pulse window 2), not the `DECLINE
reason=no-target` of the default armed boot.

## (b) the click router delivers to `wm::hit_test` — the cascade's console window, the menubar's
## crystal, the dock and the pulse window are all reachable

`orin_click` (`display_tegra.rs:1309`) asks `wm::hit_test(x, y)` itself (:1326, for the verdict) and
then calls `sc::wc_click_route(Event::Button(mask))` (:1328) — `arch/aarch64/syscall.rs:14290`. On a
press edge (:14296) the router:

| order | arm | cfg | file:line | wire |
|---|---|---|---|---|
| 1 | `strip::press_route(x, y)` = `crystal::press_at` \|\| `dock::press_at` | `desktop_firmware` | `syscall.rs:14299`; `strip.rs:744`; `crystal.rs:634`; `dock.rs:948` | crystal/dock own lines |
| 2 | `pulsewin::press_route` \|\| `quarry::press_route` | `desktop_firmware` | `syscall.rs:14299`; `pulsewin.rs:821` | pulsewin's own lines |
| 3 | `wm::hit_test(x, y)` — close / minimise / zoom discs | unconditional | `syscall.rs:14300-14399`; `wm.rs:2566` | `[clickroute] close=…` / `[wm-act] minimise …` / `[wm-act] zoom …` |
| 4 | chrome (title strip / border) — focus + drag | `desktop_firmware` | `syscall.rs:14400` | `[clickroute] press chrome win=N owner=… at (x,y) -> chrome\|drag` |
| 5 | kernel-furniture content — raise, keyboard to shell | `desktop_firmware` | `syscall.rs:14401` | `[clickroute] press hit furniture asid=… win=N (was …) consume -> shell focus` |
| 6 | `owner != cur` — focus grant | unconditional | `syscall.rs:14402` | `[clickroute] …` |

`deskcascade = ["desktop_firmware", "tegra_el0"]` (`Cargo.toml:2367`) and `arroyo:1280` folds
`desktop_firmware` into the feature list, so every `desktop_firmware` arm above is COMPILED IN on
render5. (This is the "what `desktop_firmware` brings into the router" paragraph of
`display_tegra.rs:2492-2500`, stated there for `orinconwin`; it holds identically for `deskcascade`.)

**The console window is a hittable `wm` row.** `desktop_firmware::activate()`
(`video/desktop_firmware.rs:83`) opens it at :237, `fbcon::panel_console_window_open()`
(`video/fbcon.rs:1892`), which mints it at :1979 as `wm::create_at(wm::KERNEL_OWNER_CONSOLE, …)`
(`wm.rs:1023`). `wm::hit_test` (`wm.rs:2566`) skips only `!used`, `compat`, `owner_asid == 0` and
rows below the shell — and `above_shell` (`wm.rs:3039`) admits every kernel-owned row that is not
parked, so a kernel row with no shell window on the panel (render4: `[orinrender] DECLINE
reason=console-already-windowed`, no shell row minted) is found. A press on its text takes arm 5, a
press on its title strip arm 4, its discs arm 3. The pulse window (`pulsewin::arm()` at
`desktop_firmware.rs:373`, opened by the render pass as `win=2`) takes arm 2 for its own discs/menu
and arms 3-5 otherwise. The crystal and dock take arm 1; the plain menubar band is no row, so a
press there falls to the desktop-miss arm and `orin_click` reports `MISS-IDLE`/`MISS-SHELL`.

**The coordinates are the pump's cursor.** Both position reads — `clk_pointer_pos`
(`display_tegra.rs:1251`) and the router's `click_pointer_pos` (`syscall.rs:14159`) — return
`pal::cursor::pos(w, h)`, the cursor the same drain loop advances on every pointer report
(`main.rs:2990` `cursor::move_rel`, `:2994` `cursor::set_abs`). Nothing legacy-only: no
`orinconwin`/`orindesk` row is consulted anywhere on the path.

## (c) no ordering-rule decline fires with `orinconwin` absent

§6.1's rule ("Rung 4 may not ship a console window on an image where `orinclick` is off",
`docs/dev/OS/08_VIDEO/orin-desktop.md:1287`) is enforced in exactly two places, both compiled out
of render5:

- `orin_conwin` — `display_tegra.rs:2631`, `[orinconwin] DECLINE reason=ordering-rule`; its only
  caller is terminus item 5, `#[cfg(feature = "orinconwin")]` (`main.rs:2717`).
- `tegra_desk_arm` — `main.rs:7404-7410`, `[deskseam] REFUSE reason=…` on
  `!(TEGRADESK_CLICK_ROUTED && TEGRADESK_CASCADE_OK)`; the fn is
  `#[cfg(all(target_arch = "aarch64", feature = "tegradesk"))]` (`main.rs:7304`), terminus item 4.

Neither `orinconwin` nor `tegradesk` is in render5's knob line, and neither is implied:
`deskcascade = ["desktop_firmware", "tegra_el0"]`, `orinclick = ["tegra_el0"]`,
`desktop_firmware = []` (`Cargo.toml:1758`), `arroyo:1280`/`:909` add nothing else. The cascade's
own floors (`main.rs:8556-8590`: `already-armed`, `no-panel`, `stage-unreserved`,
`table-not-empty`) and `activate`'s (`dock-cannot-host-full-strip`, `desktop_firmware.rs:223`)
read no click knob. The one place the cascade image READS `orinclick` is the render arm line,
`main.rs:8142-8149`, `click={}` = `cfg!(feature = "orinclick")` — so render5 prints `click=1`.

Type-check of the composed configuration is already a standing matrix leg: `arm-tegra-render`
(`arroyo:3106`) carries `desktop_firmware,…,orinconwin,orindesk,orinclick,orinrender,deskcascade,holocron`
— a superset of render5's features. The knob-off image is untouched: this arc changes no source.

## Recipe and expected wire (render5)

    UNAOS_TEGRA=1 UNAOS_TEGRA_EL0=1 UNAOS_WITNESS=1 UNAOS_ORINRENDER=1 UNAOS_DESKCASCADE=1 \
    UNAOS_ORINRX=1 UNAOS_HOLOCRON=1 UNAOS_ORINCLICK=1 ./arroyo esp-jetson

(never with `UNAOS_ORINCONWIN`/`ORINDESK`/`ORINTENANT` — the cascade refuses a non-empty table.)

1. `[deskcascade] -> CASCADED windows=1 bar=1 … route=ROUTED` (unchanged from render4)
2. `[orinrender] arm conwin=0 furn=0 tenant=0 click=1 desk=0 cascade=1 scene=1 …`
3. `[pulsewin] open win=2 …`
4. `[orinclick] arm panel=1920x1200 rows=2 compat=0 focus=0x0 pidesk=1 t=… -> ARMED`
   (first sweep of the pump's phase 2)
5. on a press:
   `:: tegra: JD20 — pointer BUTTON 0x01 (down) ::` then, in this order, the router's line naming
   the window — `[clickroute] press hit furniture asid=… win=1 (was 0x0) consume -> shell focus`
   (console text), `[clickroute] press chrome win=1 … -> chrome|drag` (its title strip), or the
   crystal/dock/pulsewin line — then
   `[orinclick] edge=press btn=0x01 at (x,y) geom=yes hit=yes win=1 owner=0x… focus 0x0->0x0 consumed=1 -> CONSUMED`
6. every ~10 s: `[orinclick] census seq=N … press=P … consumed=C … rows=2 … -> ROUTING`
   (`IDLE-NO-CLICKS` until the first press; `FAIL reason=stuck-focus` is the failing verdict)

Scorer: `awk '/\[orinclick\] (arm|edge=|census)|\[clickroute\]|\[wm-act\]/' <log>`. A20 PASS
predicate: the arm line reads `-> ARMED`, at least one `edge=press … hit=yes … -> CONSUMED|RAISED|HIT-SAME`,
and the last census is `-> ROUTING`.

## In-artifact gate and image identity

See the seat's `build-render5.log` (`~/unaos-bench/scratch/orin14/orinclick/`) for the
`aarch64 effective features` line, the `grep -a -o -F -e` census of the witness strings and the
`readelf -l -W` max LOAD vaddr; the seat stages and writes the card from this worktree's
`target/aarch64_esp/`.
