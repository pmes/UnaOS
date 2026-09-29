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
