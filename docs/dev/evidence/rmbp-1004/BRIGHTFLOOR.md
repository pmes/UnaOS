# BRIGHTFLOOR — brightness can never leave the panel dark (rmbp-ledger B312)

Branch `exec-rmbp-brightfloor`, cut from 3160b02a. Answers FLIGHT 19 §2 BRIGHTFLOOR (SERIOUS).

## The finding — which register, and what the dark state was

Read from `docs/dev/evidence/rmbp-0915/flight19/f19-boots.log` (four boots: lines 1..11929 boot 1,
11930..15370 boot 2, 15371..33254 boot 3 = the rewrite, 33255.. boot 4).

* **The writer.** The Settings Brightness slider (`video/settings.rs` control 0, on the GENERAL tab, not
  Display — Display carries only "Blank screen") calls `brightkeys::set_level`, which pends a step; the
  desktop pass calls `brightkeys::apply` → `igpu::gmux_set_brightness(raw_for(level))`: the **Apple gmux
  backlight register, `GMUX_PORT_BRIGHTNESS` (index 0x74)**, over the indexed gmux port window. No Intel
  `BLC_PWM`, no `pp_control` write: the Kepler drives the panel (`ext_state=kepler-owned`) and the gmux
  owns the backlight on this machine. Boot 1, lines 10251..10601: `[settings] brightness=4 applied=1`,
  `:: BRIGHTKEYS: key=down level=4/16 gmux_written=1`, then 7, then the keys 8..11, each
  `gmux_written=1`, each persisted (`[prefs] set system.display.brightness=11 ok=1` is the last).
  (The boot-time `gmux_written=0` lines are the BRIGHTKEYS fixture, which never writes by design.)
* **Why every level was OFF — two bugs in `gmux_set_brightness`:**
  1. **Wrong data port.** It writes the four value bytes to `0x7C0..=0x7C3`; the gmux value register is
     `GMUX_PORT_VALUE` = `0x7C2..=0x7C5` (every other helper in igpu.rs, and upstream
     `gmux_index_write32`, use 0x7C2). So the register received bytes 2 and 3 of a 16-bit value — always
     zero — in its low half, plus whatever stale bytes sat at 0x7C4/0x7C5 (zero after the boot's own
     reads). The gmux was written **0 = backlight off**, for EVERY level 1..16, not only 0.
  2. **Wrong scale.** `raw_for` maps 16 to 0xFFFF; the gmux's own `MAX_BRIGHTNESS` (index 0x70, read at
     boot: `:: igpu: MAX_BRIGHTNESS | 0x000003FF`) is 1023. Even on the right port, levels 1..16 would all
     have been above the panel's range.
  And there was no readback: `gmux_written=1` meant "the transaction completed", not "the panel is lit".
* **The persistence.** `system.display.brightness=11` was saved; at boot 2's login `load_for_login`
  applied it — the same broken write — so the session came up dark. Boot 2's capture ends on the login
  screen (15:31:22), before the session; the dark login is Peter's report. Boot 3 = the rewrite; its
  `[settings] tab=Display` at 11:40:31 is a tab switch only (no brightness line follows: the slider is
  not on that tab). Boot 4 loaded `n=1` (the tab) — clean.

## The seam

* Backlight path — `CHARTER: Kernel — driver`. New `video/backlight.rs`: ONE writer
  `backlight::set_level(l)` (clamp to the floor, scale to the gmux's measured max, write, READ BACK,
  print). Both the slider and the keys go through it. `igpu.rs`: the port fixed, `gmux_get_brightness`
  and `gmux_max_brightness` added (32-bit index reads at 0x7C2).
* Preference half — `CHARTER: Principia — shared-core` (the floor rule lives in `prefs_core`, which the
  kernel and `handlers/principia` both link, so the host cannot persist a dark value the kernel would
  refuse): `prefs_core::display::{BRIGHTNESS_MIN, BRIGHTNESS_MAX, BRIGHTNESS_DEFAULT, clamp_brightness}`.
  Owed B287: Principia's own `PrefSet` does not call it yet.

## Milestones

* **M1 — the floor.** Levels are 1..=16; 0 is never written by the slider or the keys (OFF stays
  DIMIDLE's, which blanks the surface and never touches the backlight). `raw = max * level / 16` with
  `max` read from gmux index 0x70 (fallback 1023 when the read times out or is out of range); level 1 =
  1/16 of max. `[backlight] level=<l> reg=<raw> readback=<rb> max=<m> on=<0|1> via=<slider|keys|login|
  fixture>`. The keys' `gmux_written` is now the readback verdict (`readback == reg`).
* **M2 — the persistence guard.** `display.brightness` is clamped on LOAD (a stored 0 loads as 1 and is
  re-saved clamped: `[settings] brightness stored=<s> clamped=<l>`) and on SAVE. The loaded value is
  applied on the first desktop pass AFTER the session opened (the login screen is always at the boot
  level). SAFE MODE: Shift held at the session's open (Shift+Enter / Shift-click on Log In, read from the
  HID modifier byte, EHCI and xHCI) or the knob `UNAOS_PREFS_RESET=1` (feature `prefs_reset`) resets
  `system.display.*` to defaults: `[prefs] display reset=1 reason=<key|knob>`. Documented in the Settings
  About tab and `docs/env-knobs.md`.
* **M3 — the slider.** A press on the track maps to 1..16; Left at 1 stays at 1; the label reads
  `<l>/16 <pct>%`; the apply is immediate with readback (`applied=` is the readback's `on`).
* **M4 — fixture `tests brightfloor`** (registered, never at boot — R80; the BRIGHTKEYS boot fixture is
  moved behind it too): set 0 → floor, on=1; set 16 → full; a stored 0 reloads clamped; the reset path
  (simulated) → defaults. On a board with the gmux the sets are real and the prior level is restored; on
  any other board a simulated register with the gmux's semantics (0 = off) stands in.

Witness: `:: BRIGHTFLOOR: floor=1 set0_on=1 load_clamped=1 reset_ok=1 hw=<gmux|sim> -> PASS ::`

The metal wire (a Settings slider press to the far left): `[backlight] level=1 reg=63 readback=63
max=1023 on=1 via=slider` then `[settings] brightness=1 applied=1`, then `[prefs] set
system.display.brightness=1 ok=1`.

## Owed

* Principia's host `PrefSet` applying `prefs_core::display::clamp_brightness` (B287).
* Whether the 1/16 floor is comfortably visible on the bench panel — the metal boot says; the floor is
  one constant (`backlight::FLOOR_NUM`).
* The aarch64 desktops have no backlight driver: `set_level` keeps the level and prints `driver=none`.
