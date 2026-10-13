# PIWM — the window over the Pi probe strip, and who holds the keyboard (pi-ledger PI22; PI18/PI19/PI20)

Arc base `cb833ebb`, branch `exec-pi-piwm`. Source capture: `docs/dev/evidence/pikernel8test-1013/serial-pi.log`
(PIKERNEL8TEST2: `./arroyo kernel8-test` under `image-line-pi.env`, QEMU raspi4b 640x480, evidence base
`35af76a3e`). Every `NNN:` below is a line of that file, read with `LC_ALL=C awk 'index($0,"…")'`. Reading only:
no QEMU run was made for this record (R107); the wires in §5 are what the fold's `kernel8-test` should show.

## 1. The bench's reading, corrected

RUN2 and PI18 read one foreign window (owner `0xffffff61`, focus `0xc0c`) over the probe strip, holding the
keyboard. The capture carries three independent causes. `0xffffff61` is the smallest of them, and `0xc0c` is
not a window at all: it is clickroute leg 7's synthetic owner of nothing, left in `USER_INPUT_ACTIVE`.

| cause | surface | owner | opened by | fixed |
|---|---|---|---|---|
| A | splash hold, win=1, 640x480, modal-top | `0x0` | `video/desktop_firmware.rs` `activate` (SPLASHX86 M3, `5adaaff4`) | commit 1 of this arc |
| B | none: the shared pointer, seeded at (0,0) | — | `arch/aarch64/syscall.rs` `click_pointer_pos` on a refused panel read (since AETHERPTR2, `9a391fba`) | commit 2 of this arc |
| C | 3D window, win=5, 320x240, parked on the desktop | `0xffffff61` | `main.rs:1412` `rast_demo_maybe()` (RASTWIN park, `28bcd764`) | STOPPED: outside this arc's files (§4) |

All three commits named are ancestors of the evidence base (`git merge-base --is-ancestor <sha> 35af76a3e`).

## 2. Cause A: the splash hold covers the panel for the whole cascade

    600: [wm] alloc win=1 gen=1 owner=0x0 title="" from=declared
    607: [wc-a] composite windows=1 drawn=1
    609: [wc-f] twin -> DEFER (a live window overlaps the probe strip; retrying every composite until it clears)
    615: [splash] hold OPEN win=1 640x480 (until users::stage_resolve; 5000 ms bound)
    825: [chrome-truth] defers=33 budget=32 exhausted=true (contested passes skipped before this verdict)
    826: [chrome-truth] win=2 box=(35,0,570x220) foc=no pt=keyline_top at=(320,0) want=0xb4b4b9 got=0x060608 -> MISS
    832: [chrome-truth] win=3 box=(12,33,298x332) foc=no pt=keyline_top at=(161,33) want=0xb4b4b9 got=0x060608 -> MISS
   1025: [wc-d] verify win=6 surf=8x8 band=none scale=14x at (213,159) ... occluded=0 ... -> PASS
   1032: [wc-fv] focus-vis at (214,160) a=0xff2020 b=0x20ff20 stack=0x060608/false raise=0x060608/false shell=0x060608/false reraise=0x060608/false -> FAIL
   1191: [wcn] win=1 asid=0x0 live=yes above=yes ... z=38
   1663: :: DESK: desktop armed — the witness cascade released the panel, furniture may land ::

- `palette_core::SPLASH_BG` is `0x0006_0608` (`libs/sys/palette_core/src/lib.rs:348`). It is the value of every
  chrome-truth probe (826-837) and of all four focus-vis legs (1032), one line after `[wc-d]` verified fv-b on
  the glass at the same origin (1025).
- `wm::splash_open` (`video/wm.rs:29542`) mints a full-panel chromeless compat row; `set_modal_top` pins it;
  `reassert_modal_top` (`video/wm.rs:29220`) re-stamps its z AND `damage_all()`s it on every create and raise.
  So each fixture create/raise is followed by a composite that repaints `SPLASH_BG` over the whole panel.
- x86 opens the same hold at the Kepler takeover (`video/desktop_uefi.rs:449/458`), after the cascade. The Pi
  reaches `desktop_firmware::activate` at the GUI handoff (`main.rs:1412`) while u7-launch runs on an AP. That
  breaks the standing rule at `video/desktop_firmware.rs:559-561`, which predates the hold.
- aarch64 has no 5 s bound on the hold (`hold_service` is x86-only, `desktop_firmware.rs:169`), and the capture
  has no `[users] stage-resolve`, `:: FIRSTBOOT:` or `[desktop] built` line in 6500 lines, so nothing released
  it during the cascade. Why the stage never published on this 60 s wall is NOT established here (§6).
- **PI19's budget question.** `CHROME_TRUTH_DEFER_BUDGET` (`video/wcf.rs:656`) is a plain 32, not a metal-tuned
  constant, and `exhausted` is printed as `defers > budget` (`video/wcf.rs:913-917`): every exhausted run reads
  `defers=33 budget=32`. It is not "over by one"; all 33 contested passes read the splash.
- `[wc-f] twin`: the first composite that drew (607) held only win=1, the hold, which overlaps the clearance
  strip (`wcf::clearance`, y 400..464 at 640x480) edge to edge.

**Fix (commit 1).** The cascade claims the panel as `u7_launcher`'s first statement
(`desktop_firmware::cascade_claim`, same-line `#[cfg(feature = "desktop_firmware")]` fold). `activate` asks
`cascade_owns_panel()` once at the `hold_prepare` line and carries the answer to the `hold_open` line: declined
prints `[splash] hold SKIP reason=cascade-owns-panel` and renders nothing. A hold already up when the cascade
starts is released by name (`[splash] hold RELEASE reason=cascade-owns-panel`). The Orin reaches `activate`
from `tegra_desk_cascade` past the `-> !` terminus, never runs u7_launcher, and keeps the hold unchanged.

## 3. Cause B: the pointer is seeded at (0,0), so router legs press the crystal corner

    122: [uvug8] focus asid=1 — discarded 1 pre-launch event(s) from EVENT_QUEUE
   1211: :: SHARD-MENU: crystal_press=open via=corner-zone menu=242x247+0+34 items=8 ::
   1219: [wc-fv] focus raise asid=0xc0c windows=0 top_win=0 z=0 shell_z=37
   1221: :: SHARD-MENU: crystal_press=dismiss reason=outside ::
   1222: [clickroute] hit-test at (215,161) inside=true topmost=true raise=true outside=true miss=(2,2) hidden=true shell=false bare=false hit=skip deliver=skip wake=skip corner=true close=skip closereal=skip -> FAIL hidden_owner=0xffffff61
   1457: [dragperf] router -> SKIP (title bar not placeable under cursor)
   1645: [dragwedge] furniture aimed=false chrome=false grab=false release=false | ... -> FAIL
   1647: [dragwedge] furniture -> SKIP (title bar not placeable under cursor)
   1660: :: TABRING: TAB rotation — an app holds the keyboard (focus=0xc0c), SKIP ::

- `pal::cursor::pos(w, h)` seeds the shared pointer lazily at `(w/2, h/2)` from its FIRST caller
  (`pal.rs:527-528`). `click_pointer_pos` (`arch/aarch64/syscall.rs:14145`) passed `(0, 0)` whenever LOCKFIX's
  non-blocking panel read was refused. Since AETHERPTR2 (`9a391fba`) it runs on every focus arrival
  (`user_input_set_active` -> `ptr_abs_arrival`, `:13468`); the first is the cascade's own `[uvug8]` focus (122),
  while every core is printing through the panel. QEMU raspi4b delivers no HID, so nothing moves it afterwards.
  Every other `pos()` caller reads a ready geometry or returns first (`flightring.rs:71`, `video/screen.rs:2640`,
  `video/wm.rs:17380`).
- In 1222, `hidden=true` is the PASSING leg (a kernel-band owner is accepted, `video/wm.rs:25139`). The red legs
  are `shell`/`bare`. Both check `hit_test(cursor)` is None before pressing (`video/wm.rs:24567`, `:24642`). At
  panel centre the 3D window would answer and both would SKIP. They ran, so the pointer was over no row. The
  router then gave the press to the crystal's corner cell (`video/crystal.rs:784-792`; `via=corner-zone`, not the
  glyph), and leg 7's press was spent dismissing that menu (1221).
- Leg 7 leaves `USER_INPUT_ACTIVE = 0xc0c`; the teardown restores `FOCUS_ASID` only (`video/wm.rs:25239`). TABRING
  declines on any non-zero entry focus (`arch/aarch64/syscall.rs:13730`). `[wc-c] focus tab-cycle` is emitted only
  by `wc_focus_key` (`:13421`), which TABRING drives, so it is absent.
- `close=skip closereal=skip`: `closebox_leg` returns None for a pointer within 96 px of an edge
  (`video/wm.rs:24840`). This is why spec `:1821` and `:1834` miss.
- dragperf and dragwedge solve a window position putting the title strip under the pointer and require
  `ny >= TITLE_H + BORDER` (`video/wm.rs:22755`); under (0,0) that fails. So PI20's dragwedge red is this cause.

**Fix (commit 2).** `click_pointer_pos` keeps the last geometry its door read and answers a refusal with it. If
no geometry has ever been read, it returns (0,0) for that one event without calling `pos()`, so the shared pointer
is never seeded with a zero panel. Line-neutral (4 body lines out, 4 in).

## 4. Cause C (STOPPED): the 3D window, owner 0xffffff61

    926: :: RAST: BCM2711 mailbox panel 640x480 (live firmware geometry, inherited scanout) — software rasterizer cube, the Pi's first 3D pixels ::
    931: [rastwin] spin open win=5 owner=0xffffff61 surf=320x240 box=330x284 scale=1 at (155,98) panel=640x480 title=3D seat=Some((95, 150, 320, 240)) ...
   1153: [wc-iso] close-iso ... forced=0x101018/false ... -> FAIL owner_sibling_up=true
   1300: [rastwin] spin park win=5 -> ON-DESKTOP (spin ended, window stays; surface retained, closable by the ordinary route)
   2000: [wcn] win=5 asid=0xffffff61 live=yes above=yes ... z=85

- RASTWIN (`rast_demo.rs`, `RW_OWNER = KERNEL_OWNER_BASE + 0x61`) opens from `rast_demo_maybe()` on the handoff
  line `main.rs:1412` (UNAOS_PIRAST=1). Its 90 paced frames (931-1300) interleave with the cascade's window
  fixtures (1004-1222). Since `28bcd764` the row is parked on the desktop for the life of the boot.
- As kernel-band furniture it comes forward on every shell raise. wc-iso's only false leg is `forced`: after its
  deliberate `focus_changed(0)` the ordinary row's origin reads `0x101018`, which is `rast_demo::RW_CLEAR`.
  `owner_sibling_up=true` is a diagnostic field, not the failing leg.
- `main.rs:7227-7230` says this call runs "BEFORE any service task exists, so it can neither race bring-up nor
  fight the compositor". The capture shows the cascade compositing concurrently.
- **Exact change (main.rs, not made).** Drop ` rast_demo_maybe();` from the Pi handoff line `main.rs:1412`. Run it
  at the Pi's armed seam: once, at the first render pass where `video::desktop_firmware::armed()` reads true (the
  SHELLWIN-PI mint arm, `main.rs:5593-5597`). Run it on a spawned kernel task, not inline, because its 90 paced
  frames (~3 s) would otherwise stall the render pass. The tegra call on `main.rs:2742` stays where it is.
- After cause A, focus-vis is not expected to need this: its shell leg accepts a kernel-owned hit
  (`video/wm.rs:21932`), and its raise legs re-stamp the fixture rows above the 3D row.

## 5. Expected wires on the fold's `./arroyo kernel8-test` (with commits 1 and 2; cause C still open)

| spec row | before | expected | cause |
|---|---|---|---|
| — | `[splash] hold OPEN win=1` (615) | `[splash] hold SKIP reason=cascade-owns-panel` or `hold RELEASE reason=cascade-owns-panel`; no `hold OPEN` before `:: DESK: desktop armed` | A |
| :903 / :904 | `defers=33 budget=32 exhausted=true`; verdict `hits=0/10 -> FAIL` | `exhausted=false`; `[chrome-truth] verdict … -> PASS`; no `got=0x060608` | A |
| :675 | `[wc-fv] focus-vis … 0x060608 … -> FAIL` | `stack=0x20ff20/true raise=0xff2020/true … -> PASS` | A |
| :861 | `[wc-f] twin -> DEFER` only | `[wc-f] twin … -> PASS` (in the console-only window before the 0xffffff05 row at 691 and quarry at 895) | A |
| :1821 / :1834 | `shell=false bare=false close=skip closereal=skip -> FAIL` | `close=true closereal=true -> PASS`; `shell`/`bare` read `skip` while the 3D row sits at panel centre; `[clickroute] close=win… settle=noproc-selftest` present | B |
| :724 / :725 | TABRING SKIP `focus=0xc0c`; tab-cycle absent | `:: TABRING: … witness=0xff … -> PASS ::` and `[wc-c] focus tab-cycle N -> M (ring of …)` | B |
| FORBID dragwedge | `furniture aimed=false … -> FAIL` | `furniture aimed=true chrome=true grab=true release=true …` | B |
| — | `[dragperf] router -> SKIP` | `[dragperf] router press=chrome grabbed=true released=true -> PASS` | B |
| FORBID `-> FAIL` | `[wc-iso] … forced=0x101018/false -> FAIL` | unchanged until §4's move lands | C |

## 6. Not explained by A, B or C (left open, named)

- **SHARD-PRESS `painted=false` (683, PI19).** Under `login`, `strip::compose_all` paints no furniture until
  `desktopbuild::built()` (`video/strip.rs:711`, DESKTOPBUILT B387). The fixture runs inside `activate`
  (`video/desktop_firmware.rs:375`), before any session, and this capture never builds the desktop (no
  `[desktop] built` line). Re-siting it to the build edge would make it silent on a boot that never builds.
  Its spec row is FORBID-only, so it would pass with no witness at all. Not moved; this needs a decision.
- **The stage never published.** No `[users] stage-resolve` and no `:: FIRSTBOOT:` line appear in 6500 lines. The
  no-store fallback publishes at `STAGE_WAIT_MS = 30_000` (`fs/users.rs:2961`) through `boot::services_gate`. This
  is `fs/users.rs`/`main.rs` territory, outside this arc.
- **`[dragperf] … coalesced=0` (1503, PI20).** 19 admitted moves in the 320 ms half (1495) means each paced report
  took >= 16 ms, so none fell inside a pace interval to fold. Timing-shaped; cause B does not touch this half.
- **`[wc-g] … -> BLIT` x8 (PI19).** Quarry `fbbad=12852/108864` (909, 913) is 378x34 rows. The 3D window has
  `fbbad=8000/76800` (955-962), which is 320x25. win=7 has `fbbad=10/64` with `occluded=54` (1136). What
  overwrote those bands is not established; re-read on the fold with cause A gone.
- **`[spinhunt] SYS_EXIT … orphan-reap armed` absent (spec :1664).** It comes from a separate emitter
  (`arch/aarch64/syscall.rs:6929`) that does not depend on window state. The SPINHUNT verdict line is present and
  PASS (1858). Not explained here.
- **PI15/PI16/PI17** (BGRUN-ST capacity text, knurl `box=12`, WMCTRL `floor=115`) are emitter/spec constants.
  None of A, B or C explains them. They are PISPEC's.
- **`pi4-regression.spec`** is unchanged: no line is shown stale by this arc.

## 7. Also reported, not made (outside this arc's files)

- **pal.rs (the bottom layer of cause B).** `pal::cursor::pos` should seed only from a real geometry: take
  `get_or_insert((w/2, h/2))` only when `w > 0 && h > 0`, and otherwise return the stored point or `(0, 0)` without
  storing it. That also covers x86's `click_pointer_pos` (`arch/x86_64/syscall.rs:6366-6372`, same refused-door
  shape). Commit 2 closes the one aarch64 caller that structurally passes a zero geometry.
