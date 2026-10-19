# BP-video — panel, wm, composite, furniture, tokens, looks, wallpaper (read at 1719fe54)

## 1. What it is
- `FrameBuffer` is the one pixel-format-aware surface; `Screen` double-buffers; `fbcon` is the boot/panic console; `wm` is the window table and compositor (`unaos/crates/kernel/src/video/mod.rs:17-36`).
- The desktop family compiles under x86 `wc` OR aarch64 `desktop_firmware` (BP-shape-6). On x86 the compositor ignites at the Kepler takeover (`unaos/crates/kernel/src/video/fbcon.rs:1791`).
- Furniture = the menu bar, dock (Una) / taskbar + Start (Softy), crystal and menus, notify/toasts, the login card. Windows = rows in `wm`'s table owned by a ring-3 slot or a kernel app.
- Colour comes from ONE table, `unaos/libs/sys/palette_core` (semantic tokens, OKLCH, Light/Dark pairs); `unaos/crates/kernel/src/video/theme.rs` re-exports it; painters ask `theme::tok(Tok)`.
- Two looks over one window system: `una` (default) and `softy`, chosen by Principia key `system.look`, switched live (`unaos/crates/kernel/src/video/look.rs:4-20`).

## 2. Invariants
| id | statement | anchor | held by |
|---|---|---|---|
| BP-video-1 | A colour literal lives only in `palette_core`; zero literals in `video/**`, the listed painters and the four ring-3 `unaos/crates/kernel/src/video/theme.rs` files. | `unaos/libs/sys/palette_core/src/lib.rs:4-20`; `unaos/crates/kernel/src/video/theme.rs:600-606` (`AUDIT_LITERALS_OUTSIDE = 0`) | `gates` row `appearance` (`unaos/scripts/appearance-check.py`) |
| BP-video-2 | Painters read semantic tokens by job (`Tok`, `theme::tok`); Softy maps its own roles (`Sem`) over the same table. | `unaos/libs/sys/palette_core/src/lib.rs:113`; `unaos/crates/kernel/src/video/theme.rs:658-717`; `unaos/crates/kernel/src/video/softy/palette.rs:30` | `tests softy`, `tests appearance` |
| BP-video-3 | The look is a Principia preference (`system.look`), never a dotfile, never a build knob; both looks are in every desktop image; `look::is_softy()` is asked first at every hook. | `unaos/crates/kernel/src/video/look.rs:4-20` `unaos/crates/kernel/src/video/look.rs:62` `unaos/crates/kernel/src/video/look.rs:67`; `docs/dev/OS/05_USER_EXPERIENCE/DESKTOPCRAFT.md:16-19` | `gates` row `prefs`; `[look] switch` line |
| BP-video-4 | Theme metrics are device px = base x ONE dpi scale; scale = ppi/110 rounded to the half (rMBP 2.0 → 1440x900 logical), R120. | `unaos/crates/kernel/src/video/dpi.rs:17-30`; `unaos/crates/kernel/src/video/theme.rs:175-257` | unverified (no gate read) |
| BP-video-5 | One modal row may be pinned topmost (`set_modal_top`); every create/raise re-asserts it (LOGINZ). Nothing passes the login screen or the splash hold. | `unaos/crates/kernel/src/video/wm.rs:29195-29230` `unaos/crates/kernel/src/video/wm.rs:3357` `unaos/crates/kernel/src/video/wm.rs:23611` | `:: LOGINZ: … -> PASS ::` |
| BP-video-6 | The shell window has a z FLOOR (`SHELL_Z`); the modal is the same idiom pointed up. | `unaos/crates/kernel/src/video/wm.rs:473` `unaos/crates/kernel/src/video/wm.rs:29200-29202` | unheld |
| BP-video-7 | No hard-coded window or process cap: the limit is derived from free heap (`windows = mem`, `procs = min(mem, windows)`); `wm::create_inner` and `sys_spawn` ASK `wincap`. | `unaos/crates/kernel/src/video/wincap.rs:1-25`; R90 | `[wm] limit windows= procs= from=` line |
| BP-video-8 | A ring-3 window surface is at most 288x288 on x86 (`FB_WIN_MAX_W/H`, one 0x51000 slot). A resize never goes below 160x64. | `unaos/crates/kernel/src/arch/x86_64/memory.rs:532-539`; `unaos/crates/kernel/src/video/wm.rs:29647-29648` | compile (const) |
| BP-video-9 | Before a session the bar and dock own no pixels; `strip::compose_all` asks `desktopbuild::built()` on every composite. | `unaos/crates/kernel/src/video/strip.rs:702`; `unaos/crates/kernel/src/video/desktopbuild.rs:37` | `:: DESKTOPBUILT:` (BP-boot-6) |
| BP-video-10 | The panel has ONE owner regime at a time (`PanelOwner`), published at every handover; a writer outside the owner refuses (`panel_refuse_term`). | `unaos/crates/kernel/src/video/mod.rs:31-36`; `unaos/crates/kernel/src/video/wm.rs:4816` | witness-build handover lines |
| BP-video-11 | The wallpaper/STARFIELD picture is generated once per panel size off the frame path; per frame only a sparse shimmer runs, through the erase drain. | `unaos/crates/kernel/src/video/desktop.rs:2091`; commit `81048c5c`; `unaos/crates/kernel/src/video/wallpaper.rs:1-18` | `tests starfield`; DESKTOPCRAFT cost line `shimmer_us=` |
| BP-video-12 | One full repaint is budgeted at 16667 us and measured on the running machine. | `docs/dev/OS/05_USER_EXPERIENCE/DESKTOPCRAFT.md:119-124` | `:: DESKTOPCRAFT: cost … -> UNDER|OVER ::` |
| BP-video-13 | A screen capture has an owner (the session user's Desktop); with no session it is refused; root takes no screenshots (R67). | `unaos/crates/kernel/src/video/prtscr.rs:83` `unaos/crates/kernel/src/video/prtscr.rs:175`; R67 | `tests prtscr` |
| BP-video-14 | A ring-3 present returns 0 for every success on both arches; no pacing status exists (renderers run unpaced). | `unaos/crates/una-abi/src/lib.rs:233-252` | compile |
| BP-video-15 | DESKTOPCRAFT is paint-only: no hit box, strip rect or router changes from the look's paint. | `docs/dev/OS/05_USER_EXPERIENCE/DESKTOPCRAFT.md:16-19` | `tests desktopcraft` |
| BP-video-16 | The accessibility tree is READ from the window table, never kept beside it (BP-bar-22): `wm::a11y` walks the rows once read-only (`table()` dereferenced immutably, no allocation under the hold, no WCSER) front to back — role = the owner's (ring-3 window or kernel furniture), name = the row's title else the armed program name, HIDDEN by `above_shell`, FOCUSED = `wl_focused`'s row — and the furniture's own tables (`winmenu::bar_boxes`, `dock::DP_PINS`, `settings::TAB_NAMES`); every kit control and the Users row draw the §2.7 ring (`DIMS.focus_ring`, device-scaled) at a Tab stop. | `unaos/crates/kernel/src/video/a11y.rs:106` `unaos/crates/kernel/src/video/a11y.rs:142`; `unaos/crates/kernel/src/video/wm.rs:30582`; `unaos/crates/kernel/src/video/settings.rs:1170` `unaos/crates/kernel/src/video/settings.rs:3208` | `tests a11ytree` (`:: A11YTREE: windows= nodes= unnamed=0 focus=ok ring_px= -> PASS ::`) |

## 3. Seams
- `wm` may be called by: kernel apps (`facet`, `quarry`, `settings`, `fileview`, `login`, …), ring-3 window syscalls (ring3.md), the input router (input.md), `splash`, `desktopbuild`.
- `wm` calls: `theme`/`palette_core`, `wallpaper`/`desktop` (backdrop rows), `blitter` (GPU or CPU copy), `wincap`, `boot::phase` (`unaos/crates/kernel/src/boot.rs:17-18`), `lag` (pass meter).
- Painters may not write the panel directly outside the owner regime (BP-video-10). The compositor gate rules are cores.md BP-cores-7..10.
- A kernel app file declares `CHARTER: Kernel — wm` or its handler (BP-shape-8).

## 4. Wire
`[wc-x] desktop-clear panel=<w>x<h> …` · `[desktop] built …` · `[look] switch to= from= ms= windows_rechromed=` · `:: WALLPAPER: src= … -> PASS ::` ·
`:: DESKTOPCRAFT: cost … frame_us= budget_us=16667 … -> UNDER|OVER ::` · `:: LOGINZ: … ::` · `[wm] limit windows= procs=` · `:: GPUBLIT: … blitter=gpu` (Kepler takeover).

## 5. Ruled, unbuilt
- R121: the Una look's full refinement waits on 3D acceleration (Kepler GR, gen7 R10+); R101 keeps one executor on each GPU.
- R127: GK107 ctxsw firmware cut into the bunker, digests in `unaos/firmware/kepler.pins`; GR golden context generated (fold `269cc521`); 3D composite on the Kepler is `unverified` at HEAD.
- R128 game tier (TEMPLE): `unaos/libs/media/temple_core` measured, bar NOT reached in fixed point; stopped as ruled (commit `d92a36b7`). See ring3.md (FPU).
- WCPAR (cores.md §5).

## 6. Read further
- `docs/dev/OS/05_USER_EXPERIENCE/DESIGN-LANGUAGE.md` §2 (unit, type, colour, depth) and §3 (component catalogue).
- `docs/dev/OS/05_USER_EXPERIENCE/DESKTOPCRAFT.md`, `docs/dev/OS/05_USER_EXPERIENCE/SOFTY.md`, `docs/dev/OS/05_USER_EXPERIENCE/STARFIELD.md`, `docs/dev/OS/05_USER_EXPERIENCE/GLASSKIT.md` (same directory).
- `docs/dev/OS/08_VIDEO/PARITY.md` (per-chip desktop gates), `framebuffer.md`, `composite_blit_bracket.md`, `engine.md`.
