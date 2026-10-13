# ARMRESIZE (orin-ledger A96): window resize by edge and corner drag on aarch64

R117 (RULINGS.md:148): "WINDOW RESIZE BY EDGE AND CORNER DRAG, every window, live, the standard cursor shapes at the
edges". Peter ruled it at the rMBP, but the ruling covers every window, and the Orin's and the Pi's desktops use the
same `video::wm`. x86 got the gesture from WINRESIZE (R75), DRAGNOMOVE (B620) and WINRESIZE2 (B635). Before this
arc, aarch64 had none of it. The chrome arm of `wc_click_route` (`arch/aarch64/syscall.rs:14386`, DRAG-PI M3) only
called `wm::drag_begin`, which grabs on the title strip alone. A press on a border raised the window and was
consumed (WINRESIZE2.md §2).

CHARTER: Kernel, wm (the compositor seam). No new kernel file.

## 1. The mechanism

The window-system half was already shared and arch-neutral, so nothing in `video/wm.rs` changes:

- `wm::resize_zone_at` classifies a point into one of eight zones.
- `wm::resize_begin` publishes `DRAG_WIN`/`DRAG_OWNER` with `RS_ZONE` set.
- `wm::drag_motion` hands off to `resize_motion` while `RS_ZONE` is nonzero.
- `drag_end`/`drag_cancel` call `resize_finish`, which prints `[wm-act] resize win= from=WxH to=WxH zone=`.

The arch file supplies the rest:

| piece | where | how |
|---|---|---|
| press | `arch/aarch64/syscall.rs:14386` (chrome arm) | `arm_resize_press(win, owner, cur, x, y)` is folded ahead of the title-strip grab. The order matches x86: the controls, then the zones, then the chrome. It declines owner 0 and `KERNEL_OWNER_DESKTOP`, the same pair `wc_resize_press` declines. Otherwise it focuses on the chrome arm's rule, calls `wm::resize_begin` and prints `[clickroute] press chrome … -> resize`. |
| motion | unchanged | The Pi steers through `wm::drag_route_tail` (`main.rs` focused-app drain and `render_service`), the Orin through `display_tegra::orin_drag_steer`. Both enter `wm::drag_motion_paced` → `drag_motion` → `resize_motion`, so the resize is paced exactly like the move. |
| release | unchanged | The release arm's `wm::drag_end()` ends it. This is the button edge alone; aarch64 has no level belt (DRAGNOMOVE §2). A `<TAB>` focus switch's `drag_cancel("focus-key")` ends it too. |
| keys | `arch/aarch64/syscall.rs:13341` (`wc_focus_key`) | `Action::win_key()` → `wm::win_key` is the x86 `wc_route_event` arm in the same shape. It is reached from both the focused-app and the shell key paths. |
| hover | `arch/aarch64/syscall.rs:13209` (`user_input_enqueue`) | `arm_resize_hover(ev)` is the x86 `wc_resize_hover_at` rule. It finds the topmost window, skips a control disc and owner 0 or the desktop row, then calls `wm::resize_zone_at`. A live resize keeps its own zone. The update is edge-only, through `ARZ_HOVER`. A zone entry prints one `[cursor] zone=<n\|s\|e\|w\|ne\|nw\|se\|sw> shape=<name> win=N` line. |
| shapes | `video/cursor.rs` tail | The `ZONE_SHAPE` word, the two system rows (resize-nwse, resize-nesw), `glyph`, `zone_over`, `zone_shape_name`, `set_zone_shape`, `zone_shape` and `shape_count` are widened from x86 `wc` to `any(x86_64+wc, aarch64+desktop_firmware)`. On aarch64 `shape_count()` is now 10. |
| fixture | `arch/aarch64/syscall.rs:15488` (registration) and file tail | `tests winresize` (`arm_winresize_selftest`), registered under `all(witness, desktop_firmware)`. |

All four arch-file edits are same-line folds placed before each line's first `//`, so `arch/aarch64/syscall.rs` keeps
its line count up to the file tail (`git diff -U0` shows four `-N +N` single-line hunks and one tail append). The
`cursor.rs` edits are `#[cfg]` lines rewritten in place. With the knob off, every new statement is removed by its
`#[cfg(feature = "desktop_firmware")]`.

## 2. The other two hover call sites (second commit, scope widened by the seat)

`user_input_enqueue` covers the hover only while a ring-3 app has focus. The two pointer paths that never call it
now each carry one folded statement, placed before the line's first `//` so `main.rs` keeps its line count:

- **Pi, shell focus.** In `main.rs:5482` and `:5492` (`render_service`'s `Mouse`/`MouseAbsolute` arms), after
  `drag_route_tail(ev);`: `#[cfg(feature = "desktop_firmware")] unaos_kernel::arch::aarch64::syscall::arm_resize_hover(ev);`.
- **Orin.** In `main.rs:3021` (`jd2_console_pump`'s per-frame pointer block), after `orin_drag_steer(..)` and before
  `cursor::draw_over`, so the draw latches the new shape: `#[cfg(all(feature = "orinclick", feature =
  "desktop_firmware"))] { let (hx, hy) = cursor::pos(..); …::arm_resize_hover_at(hx, hy); }`. `orinclick` does not
  imply `desktop_firmware` in `Cargo.toml`, so both are named.

The same commit adds `DRAG_LAST_MOVE_MS.store(0, Relaxed)` to `resize_begin` (`video/wm.rs:29832`, under
`target_arch = "aarch64"`). `drag_begin` already makes that reset (`:17169`). Without it a resize inherited the
previous gesture's admission floor (DRAG-ADMIT, capped at 250 ms).

The tegra-image feature set the compile used has no `orinclick`, so the Orin statement was not type-checked. The
fold's orinclick leg owes that check.

## 3. The fixture: `tests winresize` on aarch64

It runs the same legs as the x86 fixture and prints the same verdict line, so one spec row scores both arches. The
legs run in this order: cursor, zones and strip, drag, clamp, minstop, aspect, keys. The window is 200x120 over a
320x200 slot, owned by `0xD41`. That owner is outside the private-slot range, following dragperf's `0xD40` idiom.

This router reads the pointer position, so it cannot be handed a point the way x86's `wc_click_route_at` is. The
fixture follows dragperf's CLICK-PLAIN rule: it moves the window under the pointer, putting the BR probe on the
cursor. The cursor leg's press and the plain drag's press then go through `wc_click_route` itself. The clamp, min and
aspect presses land where the pointer is not, after the frame has moved. They call `arm_resize_press`, the function
the chrome arm calls. The keys leg sends the keymap actions through `wc_focus_key`, which exercises the new fold.

Expected on QEMU raspi4b (640x480), QEMU virt, the Orin and the bench Pi (any panel of at least 480x360):

```
[cursor] zone=w shape=resize-ew win=<n>
[cursor] zone=e shape=resize-ew win=<n>
[cursor] zone=n shape=resize-ns win=<n>
[cursor] zone=s shape=resize-ns win=<n>
[cursor] zone=nw shape=resize-nwse win=<n>
[cursor] zone=ne shape=resize-nesw win=<n>
[cursor] zone=sw shape=resize-nesw win=<n>
[cursor] zone=se shape=resize-nwse win=<n>
[cursor] zone=se shape=resize-nwse win=<n>
[clickroute] press chrome win=<n> owner=3393 at (<cx>,<cy>) -> resize
[wm-act] resize-begin win=<n> owner=0xd41 at (<cx>,<cy>) -> br
[wm-act] resize win=<n> from=200x120 to=200x120 zone=br owner=0xd41
…
[cursor] winresize press routed=2/2 pointer=(<cx>,<cy>) aimed=true
:: WINRESIZE: zones=8 strip=true min=160x64 drags=<n> clamped=<n> drag=true clamp=true minstop=true aspect=true keys=true cursor=ok cursor_shapes=10 -> PASS ::
```

`routed=2/2` is expected on QEMU, where the pointer is seeded at the panel centre. A pointer parked within one window
of the panel's top-left reads `routed=0/2 aimed=false`. The legs still score through the chrome arm's own function.
On failure, `[cursor] winresize leg seen=<k>/8 restored=<b> held=<b> released=<b> live=<b>` prints before the
verdict. Below 480x360 the fixture reports `:: WINRESIZE: -> SKIP (panel WxH too small) ::`.

Registration rides `wcb_launcher`, the u7 chain that the `hittest`/`dragperf` registrations also use, and needs
`witness` + `desktop_firmware`.

## 4. Proof owed to the fold

The only proof so far is one compile (R107): `cargo +nightly check … --target ../../aarch64-base.json --features
"$(cat …/leg-tegra-image.txt)"`, rc=0. The new code produced no warnings. The seat's fold owes:

- the QEMU raspi4b / virt run of `tests winresize`;
- the Pi `kernel8.img` knob-off byte-identity check;
- the bench: hover each edge and corner and watch the shape, then drag each one.
