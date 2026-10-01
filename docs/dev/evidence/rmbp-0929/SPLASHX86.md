# SPLASHX86 — the splash holds the glass until the stage is known

## Design
- **Finding.** FIRSTBOOT (boot 17): `activate_on` mints bar+console ~430 ms before the users store loads; a fresh card flashes
  furniture then `[login] installer: furniture swept n=2`. The old pre-GUI splash (`main.rs:233`) is retired by the takeover's own clear.
  The prep (`rmbp-0924/prep/SPLASHX86.md`) found `splash.rs` x86-only by an inner `#![cfg]`; R74 makes it cross-arch.
- **Mechanism.** `splash::paint` (was `boot_splash`'s body; `arm=false` = no animation, no `SPLASH_UP`) renders into a panel-sized
  RAM surface (`splash.rs hold_prepare`, before the desktop-clear). `hold_open` (after the clear, before the console mint)
  parks it as `wm::splash_open`: a chromeless compat row (never hit-tested, spared by `close_all_furniture_except`,
  not `COMPAT_WIN`), pinned with `wm::set_modal_top` (LOGINZ ceiling). `users::stage_publish` (every path: store-loaded,
  no-store, advances) ends with `hold_release` = `clear_modal_top` + `wm::close` (immediate clear, fade_ms=0 <= 300).
  `hold_service` (from `desktop_uefi::desktop_app_service`, ~1 kHz) releases at 5000 ms (`timeout`).
- **Sites.** x86 `video/desktop_uefi.rs activate_on`; aarch64 `video/desktop_firmware.rs activate` (M3, same calls).
- **Arch-neutral.** inner `#![cfg]` dropped; animation tail (ANIM/advance/retire/SPLASH_FB) now `target_arch="x86_64"`-gated per item.
  fc2.registry `splash` exception removed.
- **Witness.** `:: SPLASH: arch=<a> WxH=WxH ms=N -> PASS ::`, `[splash] held_ms=N released_by=store-loaded|timeout fade_ms=0`.
- **Pins.** `x86-wc.spec`, `arm-login.spec`. No knob.

## Written
Boot 17 should show: `:: SPLASH: arch=x86_64 WxH=2880x1800 ms=<n> -> PASS ::` right after `[wc-x] desktop-clear`, `[splash] hold OPEN`,
then (fresh card) `[login] installer: furniture swept` beneath it and `[splash] held_ms=~430+ released_by=store-loaded`.
Not done: aarch64 5 s timeout poll (relies on `desktop_allowed`); no real fade (instant clear).
