# A19 — the top-left text band is the shell painting the panel AFTER the cascade (case b), and the fix is the shell's own window

Executor A19FIX, seat orin 14, track `hw-jetson`, base `2a04fb4a`. Finding: orin-ledger A19
(render4 `SCREEN0.PNG`, `FLIGHT-RESULT-render4.md`): four lines of `UnaOS — Jetson Orin Nano …
architect@unaos:~$` at the panel's top-left under the 34 px menu bar on the cascaded desktop.

Files touched: `unaos/crates/kernel/src/main.rs` (tegra region only: the jd2 spawn line, six
line-neutral folds in `jd2_console_pump` phase 2, and the REALDESK-SHELLWIN tail block),
`docs/dev/OS/orin-ledger.md` (A19 tick), `PROGRESS.md`, this file. `video/` is untouched — every
symbol the fix reaches is existing `pub` API, so no `A19-video.patch` was needed.

## M0 — the band, measured again

Decoder: `docs/dev/evidence/orin14/A19-pngband.py` (stdlib only: zlib + the five PNG filters;
non-background = any channel further than 8/255 from `wm::DESKTOP_BG` 0x2D2B55).

    python3 docs/dev/evidence/orin14/A19-pngband.py ~/unaos-bench/scratch/orin14/render4-card-harvest/SCREEN0.PNG
    band x0-700 y34-120: non-bg=2826/60200 (4.7%)
      rows with non-bg: y 46..89 (32 rows); x extent 12..495
      row groups (text lines): [(46, 53), (58, 65), (70, 77), (82, 89)]
    ctrl-right x700-1400 y34-120: non-bg=0/60200 (0.0%)
    ctrl-below x0-300 y120-220: non-bg=0/30000 (0.0%)

Four 8-px glyph rows on a 12-px pitch starting at y=46, x from 12. That geometry is a signature:
`Console::top_y` (console.rs:193) = `ui_status::top_chrome_h` (34, the bar) + `m.margin` (12) = 46,
and the text starts at `m.margin` = 12. It is the shell `Console`'s layout, on the panel, laid out
UNDER the bar — i.e. drawn while the bar was already enabled. (The seat's 5.7%/855 was a subsampled
count of the same band; the full-pixel count is 2826/60200.)

## M1 — mechanism: case (b), proven from the wire and the code

The A19 brief asked (a) never cleared / (b) cleared then repainted by the console / (c) other.
It is (b).

**Order on the wire** (`render4-boot1.log`, line numbers):

    417  [pidesk] desktop-clear panel=1920x1200 bg=002D2B55        ← activate's clear (whole panel)
    479  [deskcascade] -> CASCADED windows=1 bar=1 … route=ROUTED  ← cascade done, boot core
    497  :: tegra: JD2 — EL1 console pump live …                   ← jd2 task FIRST dispatched
    522  [u7stk] at=orin-render:pass1 …                            ← render pass 1 presented the seed
    591  :: tegra: JD4 — console OWNS the panel … screen-on-boot (no key, ~8 s)   ← phase 2 PAINTS
    681  :: tegra: JD2 — KEY 's' ::                                ← every key repaints
    952  :: PRTSCR: SCREEN0.PNG … -> capturing                     ← the screenshot

`jd2_console_pump` is spawned pre-terminus (main.rs, the JB2b attach block) but is a cooperative
task: it first runs when `run_capstone_boot_core(0)` starts dispatching, which the terminus line
(main.rs:2717) reaches only AFTER `tegra_desk_cascade()` and `tegra_render_arm()`. Its phase 1 waits
~8 s for a key; its phase 2 then:

- builds `Screen::new(front_fb)` over the PANEL (`*video::WRITER.lock()`, main.rs:2874) — the fbcon
  route (`fbcon::console_is_routed`) re-homes only the boot-log MIRROR (`FbCon::draw_fb` hands back
  the window surface); the shell `Console` is a separate painter with no route at all;
- `Console::draw` (console.rs:243) does `pal.clear_screen(Self::BG)` then draws the history and the
  prompt. `Console::BG` (console.rs:162) is 0x2D2B55 — the same number as `wm::DESKTOP_BG`
  (wm.rs:2400; screen.rs:847 names the coincidence). So the clear is invisible on the desktop;
- `pal.render()` → `Screen::flush`, whose WC-I occluder walk (screen.rs:1583 `wm::occluders`)
  withholds every window box, the bar and the dock. Everything else — the text — reaches the glass.
  That is why the controls read 0% and only the four text rows show;
- every keystroke repeats it (`handle_key` → `draw_input_line` → `pal.render()`, main.rs:3008).

So `desktop_firmware::activate`'s desktop-clear (line 417) and `orin_render_service`'s
`DESKTOP_BG`-seeded pass-1 present (STACKSEED, main.rs:8265; line 522) both ran BEFORE the paint.
Neither could have cleared it, and no occluder registration is involved: the old console's box is
not an occluder — the text is simply painted last. A "one-time full-panel fill + `mark_full`" on the
first post-cascade pass (the brief's candidate fix) would have changed nothing.

## M2 — the fix (main.rs, tegra region; REALDESK-SHELLWIN)

The shell needs a surface that is not the panel. The render service's `no-painter` DECLINE arm
already states the constraint exactly: a shell window needs the task that owns the drain. That task
is `jd2_console_pump`, so it mints the window — the Pi's SHELLWIN-PI shape, port-for-port:

- `tegra_shell_window_open(&mut screen)` (tail): on the cascaded scene (`menubar::enabled() &&
  fbcon::console_is_routed()`, runtime reads) it seeds the PANEL `Screen` with `DESKTOP_BG` (its
  back buffer is zero-filled with full damage, and the console no longer paints into it — left
  alone the first flush would publish black), allocates a `pw/2 × 2/5·workh` `Bgr` surface with
  `try_reserve_exact`, `wm::create_at(KERNEL_OWNER_DESKTOP, …, b"shell", …)` centred +40/+40 (the
  classic cascade over the boot-log window, clear of the pulse window at the bottom-left and of the
  top-left band), and returns the store + `Screen::direct(fb)` + id. Every decline is named on the
  wire (`[realdesk] shell=panel reason=…`) and leaves the pre-A19 path.
- Phase 2 folds (all line-neutral): `console.mark_in_window()` when windowed; `console.draw` and
  `handle_key` draw through `tegra_shell_pick(&mut spal, &mut pal)` — the window pal when there is
  one, the panel pal otherwise; `tegra_shell_present(…)` does the panel `pal.render()` verbatim (the
  cursor's damage; the first call is the seeded band clear) and, only on a shell repaint, the
  window's flush + `wm::present_outcome_owned(id, KERNEL_OWNER_DESKTOP)` (never per pointer frame —
  the JD20 coalescing rule). The cursor bracket stays on the panel pal, untouched.
- The jd2 spawn is `spawn_stack(…, TEGRA_JD2_STACK_SIZE = 32 KiB)` on the `deskcascade` cfg
  (`create_at` and the present composite from this task's stack; orin-render measured `hw=23232`
  on that chain); knob-off the statement is the original `spawn`.
- Knob-off (`tegra` without `deskcascade`) every helper is an `#[inline(always)]` no-op twin.

Not changed: `orin_render_service` (its DECLINE stays true — IT mints no window);
`jd2_supstate_phase2` (main.rs:7524, the `supstate` twin — not in the flight's knob line; it still
paints the panel and would show the same band on a `supstate` + `deskcascade` image — noted, not
in this brief).

## M3 — gate

Chain: `~/unaos-bench/scratch/orin14/a19/gate.sh` (logs beside it: `test-arm.log`, `esp-jetson.log`, `check2.log`).

- `./arroyo check`: exit 0 (both arches; `GATE-KNOB: OK — 154 features declared, 153 named by a cfg, 0 phantom`,
  `GATE-LEDGER: OK — 72 rows`). A first run exited 1 on GATE-LEDGER only — the A19 row cited the scorer in
  `~/unaos-bench/scratch`; the scorer now lives in git as `A19-pngband.py` and the row cites that.
- `./arroyo test-arm 60`: exit 0 (`✅ aarch64 test complete`).
- `UNAOS_TEGRA=1 UNAOS_TEGRA_EL0=1 UNAOS_WITNESS=1 UNAOS_ORINRENDER=1 UNAOS_DESKCASCADE=1 UNAOS_ORINRX=1 UNAOS_HOLOCRON=1 ./arroyo esp-jetson`:
  exit 0; `target/aarch64_esp/kernel.elf` sha256 `9debc8760ef45dd35fbfa569955c17a7df09ef9e1c7a76b92dd9fe1d565c94da`;
  `grep -a -c` in it: `band-cleared` = 1, `shell-present` = 2 (the witness line + the `[u7stk]` probe name),
  `jd2-console:shell-present` = 1, `shell=panel reason=` = 4 (the four named declines).
- kernel8 knob-off byte identity (`~/unaos-bench/scratch/orin14/a19/k8.sh`: `./arroyo kernel8` with this tree's
  `main.rs`, then with `main.rs` at base `2a04fb4a`, same worktree): sha256 before
  `d73a8981d65bd24e254567934f0f2d21b3307b4a761408618d576623e2669fb0` / after
  `d73a8981d65bd24e254567934f0f2d21b3307b4a761408618d576623e2669fb0` — IDENTICAL (both exit 0; `git diff`
  of `main.rs` against the base read 0 lines for the before build and 0 against the WIP commit after the restore).

## render5 — expected wire and the scorer

Wire, in order, after `[deskcascade] -> CASCADED` and `[orinrender] … pass1` (~8 s later, or on
the first key):

    [realdesk] band-cleared x=0 y=34 w=1920 h=1166 bg=2d2b55 shell=win=3 surf=960x466 box=…x… at (…,…) (…)
    [realdesk] shell-present win=3 outcome=Composited (…)
    [u7stk] at=jd2-console:shell-present task=…:jd2-console … len=32768 used=… hw=… headroom=…
    :: tegra: JD4 — console OWNS the panel (Screen back buffer live); path=jd2-console-pump; …

(`band-cleared` prints from `tegra_shell_window_open` at the head of phase 2, before the banner
draw; `shell-present` + the gauge print from the first present, which precedes the `JD4/JD2 —
console OWNS` line by construction.) A `[realdesk] shell=panel reason=…` line instead means the window declined and
the band WILL be back; the reason names why.

Scorer (§C-style, verbatim):

    python3 docs/dev/evidence/orin14/A19-pngband.py <harvest>/SCREEN0.PNG
    # PASS iff:  band x0-700 y34-120: non-bg=0/60200 (0.0%)   and   A19 scorer verdict … PASS
    # controls stay 0/60200 and 0/30000; the shell window's chrome must NOT intrude into the band
    # (its box starts at y >= 34 + (1166 - oh)/2 + 40, far below y=120).

Also at the bench: a third window titled `shell` cascaded over the boot-log window, the prompt
`architect@unaos:~$` inside it, and typed keys echoing THERE (not at the panel's top-left).
`[u7stk] jd2-console:shell-present` must read `headroom>0` — the 32 KiB is a sized claim, and this
line is its measurement.
