# SPLASH2 — the splash owns the glass from the first frame to the first real screen

Finding (boot 18): `:: SPLASH: arch=x86_64 WxH=2880x1800 ms=9 -> PASS` at 7.6 s (no video before the Kepler takeover) and
`[splash] held_ms=2384 released_by=store-loaded` at 10.1 s. Ruling: splash from the first frame the panel can show until the first
real screen is PAINTED.

Mechanism: `main.rs` (WRITER seed / early `boot_splash` site) -> `splash::gop_stage` (M1, also on bootlog/usbdebug/witness builds when `wc`);
`fbcon::panel_console_resume` -> `splash::takeover_blit` (M2: hold surface blitted instead of the BG clear; mirror held; `desktop_uefi::activate_on`
skips its DESKTOP_BG fill via `splash::glass_held`); `login::open_as` end + `users::stage_publish` (Desktop only) -> `hold_release("first-screen")` (M3). 5 s bound unchanged.

## Written
- M1 `:: SPLASH: stage=gop at_ms=N paint_ms=N WxH=.. ::` (measure paint_ms on metal; GOP is WC MMIO, the full ray march runs there).
- M2 `:: SPLASH: stage=takeover at_ms=N ::`.
- M3 `[splash] held_ms=N released_by=first-screen|timeout fade_ms=N`.
- Specs: x86-wc.spec / arm-login.spec updated. aarch64 gop/takeover stages not done (hold already opens at `desktop_firmware::activate`).
