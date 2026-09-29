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

## Furniture sweep (call-back)
At Installer/CreateUser, `stage_publish` -> `login::installer_sweep()` closes every row but the screen's own window (`wm::close_all_furniture_except`, new at wm.rs tail) and sets the LOGOUTDESK `SWEPT` latch; `[login] installer: furniture swept n=.. re-minted=0`. At the Desktop advance `login::installer_release()` (or `close_into_session`, whichever swaps `SWEPT` first) posts `dock::relaunch_furniture()` (console+shell), the Log-In-after-Log-Out re-mint; `... re-minted=2`.

## Not finished / residual
- SUPERSEDED by the sweep below. `activate_on` cannot hold its mint: it is a one-shot takeover from PCI enumeration, the store is read ~430 ms later by a different pass, and there is no re-entry point, so the bar/console flash before the store loads remains on the metal (the sweep removes it once the stage is known).
- Non-loginst QEMU lanes with a fresh disk resolve to Installer and skip the witness chain (R77 intent); lanes needing it must seed the store or use `loginst`.

## Written
Boot 17 on a fresh card should show, in order: `:: FIRSTBOOT: stage=installer root_set=false users=0 desktop_ignited=false why=store-loaded -> PASS ::`, only the set-password window, then after setting root: `:: FIRSTBOOT: stage=create-user ...`, the create form, then `[login] session open user=<name>` and `:: FIRSTBOOT: stage=desktop root_set=true users=1 desktop_ignited=true why=user-created -> PASS ::`.
## M3 — the tests verb

**Finding (boot 16, `f16.log`).** The whole desktop battery ran beneath the modal set-password screen: `[clickroute] battery held 0ms for the loginst chain settled=true`, then every press fixture red (`[wm-act] … -> FAIL`, `[clickroute] … deflect=true -> FAIL`, `:: TERMSEL2: … -> FAIL`), and GLASSFIX2 counted the modal (`cascade overlaps=3 worst=win3-over-win5`). R77: the suite is a command.

**Mechanism.**
- `unaos/crates/kernel/src/tests.rs` (new): `register(name, fn())` into a 48-entry static table, `run(Option<&str>)`, `deferred_count()`, `source_done(bit)`, `tally(pass)` (called from `selftest::capture`, so pass/fail is exactly the `-> PASS|FAIL` lines the fixtures printed) and `shell_verb`. The verb refuses until `crate::fs::users::desktop_allowed()` (defined by M1/M2, called here).
- Verb: `tests` (all) · `tests <name>` · `tests list`; `("tests", Avail::Always)` in midden_core `HOST_VERBS`, arm beside `"tste"` in `shell.rs`. Prints `:: TESTS: ran=<n> pass=<n> fail=<n> ::`.
- Boot line: `:: TESTS: deferred=<n> fire=tests at_boot=<n> ::`, once, when the desktop battery source (x86 witness) and the loginst chain (if compiled) have both registered.
- Knob `UNAOS_TESTS_AT_BOOT=1` / feature `tests-at-boot`: `register` runs the fixture on the spot, at the old call site, so lane order is unchanged and the line reads `deferred=0`. Three-place wire: `arroyo` (`_feats`), `builder/src/main.rs`, `banner-cert.sh` row. The `test`/`test-arm`/`kernel8-test`/… case at the top of arroyo exports it by default, so `x86-login`, `x86-wc`, `x86-witness`, `x86-default`, `arm-login` are unchanged; `esp-*` metal media do not carry it. `:: TESTS: deferred=` pinned in `x86-wc.spec`.

**Moved (registered).**
- `users::service` loginst chain -> `login-chain` (`loginst_chain`, `fs/users.rs` tail: rootpw/bootroot/adduser/usermgmt/rootout/login/hard/ident/end/kown/rand fixtures, `login_press_fixture`, `screen_fixture`, `lock_fixture`). `LOGINST_LIVE` is raised by the chain itself when deferred, so `loginst_settled()` is true all boot. `root_credential_ignition()` (the installer's set-password screen) stays.
- `arch/x86_64/syscall.rs`: `winx` (WINX-1 window demo, which fans out to the battery when it runs), `winx-stat` (WINX-2), `winx-threads` (WINX-7), `winx-vug` (WINX-8), `winx-pulse`; inside the winx battery `hittest`, `clickroute` (+ termsel pointer/termwrap), `dock` (-> DOCKID, dockrun), `crystal`, `clickband` (+ menudrop, serialdoor), `ptrdead` (+ lockfix-b1), `wmdirect`, `dmgovlp` (-> GLASSFIX2), `vugres` (+ apppin, vugprobe, loginz, wci_rollup). `demo_cpu` travels in `TESTS_DEMO_CPU`.
- `main.rs` (aarch64 baremetal): `typematic` (+ keyrepeat), `inwedge`.
- Fanout note: when deferred, `tests` runs `winx`, which registers the battery entries while running; `run` re-reads the table, so they run in the same pass in the old order. `deferred=<n>` therefore counts the top-level entries; `tests list` shows the fan-out once `winx` has run.

**Left at boot, and why.** Hardware/structural witnesses, not glass tests: USERSMOUNT, PORTROUTE, KVBLANK4, SMPLOAD, EHCI-HID, the HDA codec walk and HDA-TONE (it is `run_tone` inside the walk on live DMA rings, no re-entry point), `input_router_selftest` and `serial_focus_selftest` (main.rs, before `start_aps`: their whole claim is owning the input focus before any user slot exists, so they cannot run later), `ptrlag_selftest` (a boot-race hang witness scored on its own core before the first report), `canonical_guard`/u3 fixtures/`bot_park` (kernel guards, not desktop), `winx3` (headless ELF loader). `aarch64/syscall.rs`'s own `hittest_selftest` ladder is not moved (x86 desktop was the R77 target).

**GLASSFIX2 side-fix.** `wm.rs` census skips `r.id == MODAL_WIN`, so the set-password screen no longer counts in `cascade overlaps=`.

## Written

Boot 17 (metal, no knob): `:: TESTS: deferred=<n> fire=tests at_boot=0 ::` and no fixture lines. After `tests` from the shell: `:: TESTS: ran=<n> pass=<n> fail=<n> ::`. QEMU lanes: `:: TESTS: deferred=0 fire=tests at_boot=<n> ::` and the old fixtures.
Not compiled here (R76). For the compiler executor: `crate::fs::users::desktop_allowed()` is M1/M2's (guess: `pub fn desktop_allowed() -> bool`); the `tests` verb arm uses `&args` (`Vec<&str>` deref to `&[&str]`) and `console`; `crate::tests` name may collide with nothing but check `cfg(test)`; closures passed to `register` must coerce to `fn()` (non-capturing) — they read `TESTS_DEMO_CPU`.
