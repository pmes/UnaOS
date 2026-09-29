# FIRSTBOOT — R77: the first boot is an installer (M1 + M2)

## Design

**Finding (boot 16, f16.log).** `[login] boot session=root desktop=true screen=closed` at 7969 ms, `[users] load ... users=0 (fresh store)` at 7973, `[login] root password unset row=created -> set-password screen` at 8034, `[login] screen open window=3 ... modal=true`, and beneath the setter the whole desktop ignited (STAT.ELF armed at 7548, press batteries `-> FAIL`, fixtures, `GLASSFIX2 ... win3-over-win5`). Peter could do nothing on the glass.

**Mechanism.** One predicate at `fs/users.rs` tail: `BootStage {Installer, CreateUser, Desktop}`, `boot_stage()`, `desktop_allowed()` (Desktop only; exact name for the M3 arc), `furniture_held()`, `stage_resolve(why)`. The store is known ~430 ms AFTER `desktop_uefi::activate` (7546 vs 7973 ms), so the stage cannot gate the takeover itself; it gates every TENANT, which waits (bounded 30 s, then "no store" = Desktop):
- `video/desktop_uefi.rs` `desktop_app_service`: STAT.ELF / DESKTOP_APP launch held.
- `arch/x86_64/syscall.rs` `u4x_launcher` (head of the ring-3 witness chain u4x -> ... -> winx -> press batteries): waits for the stage, returns unless Desktop; `winx_launcher` re-checks.
- `video/strip.rs` `compose_all`: bar/dock/menus/crystal do not paint while held; `stage_publish` also turns the menu bar off (`menubar::set_enabled(false)`) and back on at the Desktop advance.
- `drivers/hda.rs` `probe_after_root`: no HDA bring-up (tone) before Desktop.
- Resolution: end of `service()`'s load arm (AFTER the loginst chain, so the QEMU lane's seeded store is a Desktop-stage store); mount-refusal bound -> `stage_no_store()`.

## M1 — installer: the setter alone

Root password unset -> `Installer`. The setter (`login::open_set_password`, unchanged) is the only window that opens. When it WRITES root's password (`login::submit_setpw`, `!login_after`, name root) it calls `users::installer_root_password_set()`, which re-derives the stage: users==0 -> CreateUser (M2 form), else Desktop (furniture released, tenants run). No-op unless the published stage is Installer (the loginst chain drives the same setter mid-chain).

## M2 — create-user

`login::State::CreateUser` (`open_create_user`, `submit_create`, `cu_rect`): Name, Password, Retype, Create (Tab cycles, Enter/press submits, the set-password form's pattern). Submit -> `users::installer_create_user` = `name_ok` + not root + `adduser_commit` (row + `/home/<name>`) + `set_first_password`; then `users::login` (`[login] session open user=<name>`), `close_into_session`, `users::installer_desktop_ignite()` -> stage Desktop. Store persists, so boot 2 has root set + a user row -> Desktop stage with the login row per R64/R65. A root-password-set + zero-user store on boot resolves to CreateUser directly (form deferred through `CREATE_PENDING` to `boot_session` if the glass is not up).

## Witness / pins

`:: FIRSTBOOT: stage=installer|create-user|desktop root_set=.. users=.. desktop_ignited=.. why=.. -> PASS ::` at boot resolution and at each advance; `[login] installer: ...` lines. Pins in `x86-login.spec` (REQUIRE desktop stage on the seeded lane; FORBID `-> FAIL`, FORBID `witness chain HELD`). No new knob.

Go-red: a fresh store showing any window but the setter -> ring-3 chain would run (drop the `u4x_launcher` gate); users==0 after root password showing a desktop -> `stage_of_store` returning Desktop.

## Not finished / residual
- The console window minted by `activate_on` (`console_win`) exists before the store is known; it is not closed at Installer (bar and dock/strip paint are held). A later arc may defer the window.
- Non-loginst QEMU lanes with a fresh disk resolve to Installer and skip the witness chain (R77 intent); lanes needing it must seed the store or use `loginst`.

## Written
Boot 17 on a fresh card should show, in order: `:: FIRSTBOOT: stage=installer root_set=false users=0 desktop_ignited=false why=store-loaded -> PASS ::`, only the set-password window, then after setting root: `:: FIRSTBOOT: stage=create-user ...`, the create form, then `[login] session open user=<name>` and `:: FIRSTBOOT: stage=desktop root_set=true users=1 desktop_ignited=true why=user-created -> PASS ::`.
