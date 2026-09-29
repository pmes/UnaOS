# SCREENLOCK — lock the session without ending it

**Finding.** Peter can only leave a session by Log Out, which sweeps every window (LOGOUTDESK). No feature grep hit (`fn lock`, `LockScreen`) in `video/`.
**Mechanism.** `login::lock()` (login.rs tail) reuses `open_as(State::Open)` with a `LOCKED` flag: name pre-filled from `users::whoami`, read-only (consume_key / press arms), title "Locked", no roster; the LOGINZ modal pin is `open_as`'s own. Enter goes to `submit_unlock` = `users::verify` only (no `users::login`, no new epoch), then `take_down` + `State::Session`; `SWEPT`/`relaunch_furniture` is never reached (LOGOUTDESK's sweep never ran). Refused when no session, screen already up, or the row has no password.
**M1** lock()/unlock + `lock_fixture`. **M2** `Action::LockScreen` (keymap.rs, wire code 19), `cmd-l` + `ctrl-alt-l` (theme.rs CRISPY_ROWS 27, PC_ROWS 17), router arm beside WINCYCLE (x86 syscall.rs), crystal `Lock` verb + row after `Log Out`. **M3** `UNAOS_LOCK_ON_IDLE=1` (dimidle.rs const, arroyo comment): DIMIDLE wake calls `lock()`.
**Witness.** `:: SCREENLOCK: user=una locked=1 windows_kept=N wrong=refused unlock=ok furniture_reignited=0 -> PASS ::` (fixture chained in fs/users.rs beside `screen_fixture`). **Spec.** x86-login.spec REQUIRE + FORBID.

## Written
All three milestones. Not run (R76).
