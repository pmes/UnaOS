# LOGINFLOW2 — boot 2: the login screen is the boot session

## Design

**Finding.** Boot 17 flew R77's first boot only. On the second boot `stage_of_store()` (`fs/users.rs`) returns `Desktop` for a store with root set and users >= 1, and `stage_publish(Desktop)` ignites root's desktop: `boot_session` (`users.rs`, `boot_ignition` is a no-op) opens nothing. R64/R65/R77 ("root is not the assumed login") say the LOGIN SCREEN comes up. `login::submit` also refused a typed `root` ("Root logs in by booting").

**Mechanism.**
- `users::stage_resolve("store-loaded")` with `Desktop` and `user_count() > 0` is boot 2: it publishes `why=store-has-users` (witness prints `stage=login-screen`, `desktop_ignited=false`), drops `ROOT_LIVE` (no assumed session), and calls `screen_boot2()` -> `login::open_boot2()` = `open()` then `installer_sweep()` (desktop empty, `SWEPT` owed). Deferred through `BOOT2_PENDING` to `boot_session` if the glass is not up. `installer_release` is skipped for boot 2; `furniture_held()` holds the furniture while `stage_name() == "login-screen"`.
- A login re-mints through `close_into_session` -> `dock::relaunch_furniture` (the FIRSTBOOT/LOGOUTDESK path) and prints the REIGNITE witness.
- Root types `root` + its password: `users::login_root` (verify, `ROOT_LIVE` back on, no user session).
- `whoami` / `users` carry `stage=` (`login-screen` until a session opens); `users::stage_name()`.

**Milestones.** M1 boot-2 path + root login; M2 "logout" fixture (LOGOUTUI M4); M3 LOGOUTDESK-REIGNITE witness; M4 whoami/users stage. The menubar has NO user-name item in tree (grep `menubar.rs`: none) — painting one is owed, see Not finished.

**Witnesses.** `:: FIRSTBOOT: stage=login-screen root_set=true users=N desktop_ignited=false why=store-has-users -> PASS ::`; `:: FIRSTBOOT-LOGIN: ... -> PASS ::` (fixture "boot2-login"); `:: LOGOUTUI: close=ok -> PASS ::` + `:: LOGOUT: alert_open= alert_ok_closes= session= screen_back_empty= relogin= furniture_back= -> PASS ::` (fixture "logout"); `:: LOGOUTDESK: windows_closed= reignited= console= shell= -> PASS ::`. Pins: `x86-login.spec` (FIRSTBOOT pin changed from `stage=desktop ... store-loaded` to `stage=login-screen ... store-has-users`).

## Written

Boot 17 (second boot of the card): `:: FIRSTBOOT: stage=login-screen root_set=true users=1 desktop_ignited=false why=store-has-users -> PASS ::`, the login screen over an empty desktop, then after typing the user's password `[login] session open user=<name>` and `:: LOGOUTDESK: windows_closed=N reignited=2 console=posted shell=posted -> PASS ::`; `whoami` prints `<name> uid=N stage=desktop`.

Lane (QEMU loginst): `tests-at-boot` runs "boot2-login" then "logout" right after `stage_resolve`, before the chain's registered pass (they need the screen up). Chain itself unchanged.

## Not finished / notes
- Menubar user-name item: does not exist; owed (painter in `menubar.rs` reading `users::whoami`).
- Pre-existing: `service()` runs the chain inline AND `register("login-chain", loginst_chain)` runs it again under `tests-at-boot` (root-session fixtures cannot hold twice). Not touched.
- Root's typed-correct-password leg has no fixture (the lane's root password is not known to the fixture); the wrong-password leg is pinned.
