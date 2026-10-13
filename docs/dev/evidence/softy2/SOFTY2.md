# SOFTY2 — Softy finished (rmbp-ledger B641, R126)

Branch `exec-rmbp-local-softy2`, cut from `6f13e5e92` (seat rmbp-0915/local3). Parent arc: SOFTY (B629).

## The one compile (R107 / R113)

The x86 kernel leg under the bench's line, ONCE:

```
cd unaos && export UNAOS_WIFI_FW_PATH=/home/pmes/src/github.com/pmes/UnaOS-bunker/drivers/broadcom-b43
set -a; . ./scripts/image-line.env; set +a; export UNAOS_AETHER=1 UNAOS_MIDDEN=1 UNAOS_UNA=1 UNAOS_SONDE=1
cd crates/kernel && cargo +nightly check --release --target ../../x86_64-unaos.json \
  -Z build-std=core,compiler_builtins,alloc -Z build-std-features=compiler-builtins-mem -Z json-target-spec \
  --features "witness,selfhost,wcg-paygo,wcdvalve,logts,ehcihid,kbdwit,smc,sdw,sdhcblk,bt,btc,login,installdemo,smolnet,wifi,wifi2,wifi3,wifi4,wifi9,nvidia-kepler,nvidia-kepler-takeover,noaspm,nvidia-kepler-ce,wc,nvidia-kepler-vblank,deadman,linuxabi,busreg,lumen,midden,una,sonde,aether,stria,netring3,selfdiag,lidsleep,videoplayer,intel-ivb,unaos_ivb,gen7,gen7r8,gen7blit,gen7r9,gmux_igd,beam,quarry,facet,sdwrite,ftdirx,usbnet,ahci,ahci-write,unafs,ahciroot,hda,hda-tone,uvc,ioapic"
```

(the feature list is arroyo's own `⚡ kernel features:` banner under that environment, the `_cfg_slot_worker` x86
row's `cargo +nightly check --release --target … $NIGHTLY_FLAGS --features …` shape) — **rc=0**, no new warning in a
touched file. Not run, by R107/R113/R78: the `./arroyo check` matrix, the aarch64 leg, knoboff, QEMU, `tests softy`.

## The key table (R126: "mac option is windows win key and mac command is windows alt")

Boot Camp maps the keys BY POSITION (a Mac's `Ctrl Option Command` sits where a PC's `Ctrl Win Alt` does). A
MIRRORED table reads the GUI and ALT keys swapped; the look and the keyboard (the STATUSTRAY input item's PC flag)
pick one of four tables at `keymap::active()` (keymap.rs:362), and no row is re-typed:

| look  | keyboard | table       | rows               | Command role | Option role (`option_role`) | the Windows key (Start) |
|-------|----------|-------------|--------------------|--------------|-----------------------------|-------------------------|
| Una   | Mac      | `crispy`    | `theme::CRISPY_ROWS` | Command (GUI) | Option (ALT)              | —                       |
| Una   | PC       | `una-pc`    | `theme::CRISPY_ROWS` mirrored | Alt (ALT) | Win (GUI)            | —                       |
| Softy | Mac      | `softy-mac` | `theme::PC_ROWS` mirrored | Command (GUI) | Command (GUI): Ctrl+Command | Option (ALT), a tap |
| Softy | PC       | `pc`        | `theme::PC_ROWS`   | Alt (ALT)    | Alt (ALT): Ctrl+Alt         | Win (GUI), a tap        |

So on the rMBP under Softy: Command+C copies, Command+Tab cycles (Windows' Alt+Tab), Ctrl+Command+L locks
(Windows' Ctrl+Alt+L), Ctrl+Command+arrows snap, and a bare tap of Option toggles Start. On a PC keyboard under Una:
Alt+C copies (⌘C), Alt+Win+← snaps left (⌘⌥←), Win is Option.

Mechanism: `Table::option_role` (keymap.rs, tail) — the ALT role's physical key is `HID_MOD_GUI` on a mirrored
table (`core::ptr::eq` against `SOFTY_MAC_BINDINGS` / `UNA_PC_BINDINGS`), read by `Table::held` (keymap.rs:281) and
`option_held` (keymap.rs:515). The Windows key is a bare TAP of `keymap::start_role()`: `tap_track` (from
`note_mods`, keymap.rs:485, every HID report on both decoders) arms on a lone down-edge, counts on the up-edge;
`note_keys` (from `resolve_edge`, keymap.rs:345) spoils an armed tap on any key edge (Win+E is a chord) and counts
Esc edges. The look's service pass consumes both (`softy::startmenu::key_service`): an odd tap count toggles Start
(`by=key`), an Esc edge closes an open flyout. Nothing opens a window from the decoder.

Wire: `[softy] start key role=<option|win> taps=<n> -> open|close`, and when the live table moves (a look switch, the
PC flag) `[keymap] table=<name> cmd_role=<gui|alt> option=<gui|alt> start=<option|win|none> via=look`;
STATUSTRAY's own `[keymap] table=… via=statustray` now names `una-pc` under Una.

## The rest of the finish

* **Settings > Appearance > Look: UnaOS | Softy** — settings.rs:2495 (the paint row, row 7, the segmented control on
  the kit) and :2506 (the press arm: `look::choose(i)`, stored and applied by `look::service`, off the click router;
  `[settings] system_look=<una|softy>`).
* **The clock press opens the notification calendar** — the calendar's head is now notify's summary (`notify::unread`,
  `notify::dnd`: "3 new notifications" with an accent dot / "No new notifications" / "Do not disturb is on") over the
  long date and the month (startmenu.rs `paint_notes`); `[softy] calendar open by=button … shadow=<m> notify_unread=<n>`.
  Opening notify's OWN Center from the clock needs one exported call notify does not have (see "Owed").
* **`softy/paint.rs` repointed at `video::draw`** — every facade op is the kit's (`fill`, `fill_rrect` with per-corner
  radii for `rrect_top`, `stroke_rrect` width 1, `hairline_h/v`, `fill_disc`, `line_aa`, `blend`); `Surf` maps scene
  coordinates through an origin (whole / one strip row / FRAMED). New: `paint::shadow` (DESIGN-LANGUAGE §2.4 `elev.2`:
  `0 0 2` + `0 8 24`, light 12 %/18 %, dark 30 %/45 %, two `draw::shadow_rrect` falloffs held inside the band) and
  `paint::under` (the wallpaper beneath, `wm::desktop::live_back().sharp`). Every flyout is now a framed surface: the
  body inside a `FLY_GAP` band (startmenu.rs:713) — real rounded corners over the wallpaper and a soft shadow.
  `cut_top_corners` (the window head's desktop-coloured corners) keeps its own coverage loop: the kit has no
  "outside" fill and a window row has no under-sample.
* **Dark palette from the OKLCH table** — `softy::palette`'s colours are FUNCTIONS (`SURFACE()`, `TEXT()`, …) resolved
  per paint from `theme::is_dark()`: light = Softy's own table (palette_core::softy, unchanged); dark = DESIGN-LANGUAGE
  §2.3's tokens BY NAME through `palette::Sem` / `palette::dark()` (palette.rs:93, the one seam). The taskbar repaints
  on `theme::epoch()`, the flyouts on the next service pass.
* **Quick settings (sound, Wi-Fi, battery)** — the tray press opens a third flyout (`[softy] quick open by=button …`):
  the network tile (accent when a link is up; "Connected a.b.c.d" / "Not connected" / "No network adapter"; a press
  opens Settings), the sound row (mute button + level slider through `status::set_volume`, the Settings slider's own
  door: `[softy] quick volume level=<n>/16 muted=<0|1> amp_written=<0|1> by=<mute|slider>`; "No sound output" with no
  sink), the battery (`status::reading`) and a gear that opens Settings.
* **Pressed states** — the caption buttons (chrome.rs: the latch carries a pressed bit from `pal::cursor::button_down`;
  min/max at twice the hover wash, close a step darker red) and the taskbar's tiles, tray and clock (the pressed state
  was drawn but never latched; taskbar.rs:486).
* **`#0067C0` replaced by the accent table's fill** — `palette::ACCENT()` = `theme::accent()` (the chosen accent's
  fill, `system.appearance.accent`); the focused window's border and the running pip use `ACCENT_INK()` (the fill in
  light, `accent.ink` in dark). The `palette_core::softy::ACCENT` constant is no longer read by Softy (DESIGNAPPLY's
  file; left in place).

## Palette names SOFTY2 depends on (DESIGN-LANGUAGE §2.3)

`surface.window`, `surface.sidebar`, `surface.content`, `surface.raised`, `surface.overlay`, `surface.sunken`,
`line.subtle`, `line.strong`, `line.control`, `text.primary`, `text.secondary`, `text.tertiary`, `text.disabled`,
`accent.fill`, `accent.hover`, `accent.pressed`, `accent.ink`, `status.critical`, `rim.dark` (19, `palette::SEMS`).
Softy reads them through `palette::dark(Sem::…)` ONLY. Until DESIGNAPPLY (B639) lands them in `palette_core`, each arm
resolves through the palette API that exists today: `theme::DARK[Tok::ContentFill | WindowBg | BevelShadow | FieldBg |
TitleInactiveTop | TitleActiveBottom | FrameLine | TitleTextInactive | ButtonText | ButtonFacePressed]` and
`theme::accent()`. **At DESIGNAPPLY's fold the seat repoints the 19 arms of `softy/palette.rs::dark` to the tokens of
the same names** (DESIGNAPPLY removes the material `Tok` names, so this function is where a red lands — and the only
place in Softy).

## The expected wire (`tests softy`, the next metal boot; the seat's)

```
:: SOFTY: switch=ok bar=0,<y>,<pw>x<h> tiles=<n> start=open,close tile=<app> buttons=right(min,max,close) keys=softy-mac:ok,una-pc:ok,tap:ok,spoil:ok quick=open,close calendar=open,close shadow=<m> dark=19/19 accent=table pressed=ok back=menubar,dock -> PASS ::
```

`quick=notray` is a pass when the tray has no item yet; `pressed=nowindow` / `buttons=nowindow` pass with no focused
window. `shadow=` is `ui::px(FLY_GAP)` (12 logical: 24 at scale 2). Around it on the wire: `[look] switch to=softy …
via=tests`, `[softy] start open by=button …`, `[softy] quick open by=button … shadow=<m>`, `[softy] calendar open
by=button … notify_unread=<n>`, and their `close by=button` rows.

By hand on the glass (the screenshot): Settings > Appearance > Look → Softy; tap Option → Start opens
(`[softy] start key role=option taps=1 -> open`), Esc closes it; the clock → the notification calendar; the tray →
quick settings, a click on the slider moves `level=`; Appearance → Dark repaints the band and flyouts dark.

## Owed (outside this arc's files — the exact change)

1. **`video/statusmenu.rs:399`** — `tests statustray`'s input leg asserts `keymap::active().name == "pc"` after the
   toggle; under Una the PC keyboard's table is now `una-pc` (R126). Change `"pc"` to `"una-pc"` (or assert
   `keymap::pc_selected()`), or that leg reads FAIL.
2. **`video/notify.rs`** — the clock's press opens notify's own Center when notify exports
   `pub fn request_center(open: bool) { REQ_CENTER.store(if open { 1 } else { 2 }, Ordering::Release); }`
   and Softy's `HOT_CLOCK` arm (taskbar.rs) adds `crate::video::notify::request_center(true)`. The Center's rect sits
   under the menu bar (`center_rect`: `y = TITLE_H + 4`); under Softy it wants the taskbar's top instead (MENUSCRAFT).
3. **An `Action::Start`** — not added: `Action` has three exhaustive matches outside this lane (`smallfix3.rs`
   `ACTIONS` + `listed`, `video/clipboard.rs` `terminal_action` + `action_code`, the last a ring-3 ABI number). The tap
   reaches Start through `keymap::take_start_taps` instead; the Win-chords (Win+E, Win+D, Win+arrows) need a WIN role
   bit and Softy rows in `theme.rs`.
4. **`docs/dev/OS/05_USER_EXPERIENCE/SOFTY.md`** "Not yet Windows-like" — the Win key, the Settings control, flyout
   shadows, pressed states, quick settings and dark Softy are done; the doc still lists them owed.
5. **`look::service`** does not repaint Settings after a Look choice lands: the Look segment shows the new look on the
   pane's next repaint (a pointer move), not on the click (look.rs is not this arc's file).
