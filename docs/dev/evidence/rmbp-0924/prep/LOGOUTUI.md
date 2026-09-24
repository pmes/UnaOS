# LOGOUTUI — prep

## The finding

Flight 15 read six silent `[users] logout REFUSED session=root reason=no-users` refusals — root's
Log Out died on the wire with nothing on the glass. Peter (R70, rmbp session 2026-09-24): "there is
no status bar for that so an alert it is." Ruling: a refused Log Out says why in an alert of the
set-password screen's shape — `reason=no-users` reads "add a user first (adduser <name>)",
`reason=storage-not-up` names the store.

## Mechanism

- `users.rs:2473-2486` `root_logout_refused()` — root-session-only check; computes `reason`
  (`"storage-not-up"` if `!load_once()`, else `"no-users"` if `user_count()==0`), prints the serial
  refusal, returns `bool`. The reason string is computed then thrown away.
- Two callers, both silent today: `users.rs:2503` (`log_out_to_screen`, the shell's `logout`), and
  `video/login.rs:636-641` `reopen_after_logout()` (crystal's Log Out row via `crystal.rs:696` and
  `strip.rs:1675`). Both just `return` on `true` — no screen action.
- `video/login.rs:522-547` `open_set_password(name, login_after)` is the shape to copy: flips
  `FORM.state` in place if a window is already open, else calls `open_as(state)`; sets `f.message`.
- `video/login.rs:155-163` `enum State { Closed, Open, Session, SetPw }` needs a fifth variant.
- `video/login.rs:241-260` `enum Ctl` / `:293-306` `ctl_rect(c, setpw)` — the ONE place control rects
  live, read by `repaint()` (`:434+`) and `ctl_at` (`:313-326`). One more rect variant, not a new table.
- `video/login.rs:801-855` `consume_key` — Esc does nothing today (`b'\x1b' => {}`); Alert state must
  special-case Esc/Enter to close.
- `video/login.rs:739-746` press dispatch needs `Some(Ctl::AlertOk) => alert_ok()`.

## Plan

- **M1** — `users.rs`: split the reason out. Add `pub fn root_logout_reason() -> Option<&'static str>`
  holding today's `root_logout_refused` body (same serial line), and make `root_logout_refused()` a
  one-line wrapper (`root_logout_reason().is_some()`) so `log_out_to_screen` (`users.rs:2503`) needs
  no edit. Witness: none new. Go-red: none of its own — folds into M3's.
- **M2** — `video/login.rs`: add `State::Alert`, reuse `f.message` for body text, add `Ctl::AlertOk`,
  extend `ctl_rect` with an `(Ctl::AlertOk, _)` rect (reuse the set-password `Button` position),
  extend `repaint()` with an `Alert` branch ("Log Out" title, one line of `f.message`, one
  `button(px, Ctl::AlertOk, b"OK", true, false)`), extend `ctl_at` to check `AlertOk` when
  `state == Alert`, extend press dispatch with `Some(Ctl::AlertOk) => alert_ok()`, extend
  `consume_key` so Enter/Esc in `Alert` state call `alert_ok()` instead of `submit()`/no-op.
  Witness: none yet (wired in M3).
- **M3** — `video/login.rs`: add `pub fn open_alert(reason: &'static str)` (mirrors
  `open_set_password`'s in-place-vs-fresh-window branch) mapping `reason` to the two R70 strings and
  setting `State::Alert`; add `fn alert_ok()` closing back to `State::Closed` then `open()` (same
  tail `reopen_after_logout` uses today). Rewrite `reopen_after_logout` (`login.rs:637-641`):
  ```
  pub fn reopen_after_logout() {
      if let Some(reason) = users::root_logout_reason() {
          open_alert(reason);
          serial_println!(":: LOGOUTUI: reason={} alert=open -> PASS ::", reason);
          return;
      }
      users::logout();
      FORM.lock().state = State::Closed;
      serial_println!("[login] logged out — screen returns");
      open();
  }
  ```
  Witness: `:: LOGOUTUI: reason=<r> alert=open -> PASS ::` (one line, printed once per refused Log
  Out, `<r>` is `no-users` or `storage-not-up`). Go-red: delete the `open_alert` call and keep the
  early `return` (today's behaviour) — witness line never prints, refusal stays silent -> FAIL by
  absence.
- **M4** — `loginst` fixture (`users.rs` `login_rootout_fixture`, `:2534+`, `empty_refused` leg already
  empties the store, expects `reason=no-users`): also assert `alert=open` and that `alert_ok()` (Enter
  or a click on the OK rect) closes the alert back to the login screen, store still empty. Witness:
  M3's line plus `:: LOGOUTUI: close=ok -> PASS ::` after the OK press. Go-red: `consume_key`'s Alert
  branch missing — Enter falls through to old no-op, alert never closes, `close=ok` never prints.

## Draft code (unbuilt)

```rust
// unaos/crates/kernel/src/fs/users.rs — after fn root_logout_refused (line ~2486)
pub fn root_logout_reason() -> Option<&'static str> {
    if !root_session() { return None; }
    let reason = if !load_once() { "storage-not-up" }
        else if user_count() == 0 { "no-users" }
        else { return None; };
    serial_println!("[users] logout REFUSED session=root reason={} (R63)", reason);
    Some(reason)
}
pub fn root_logout_refused() -> bool { root_logout_reason().is_some() }
```

```rust
// unaos/crates/kernel/src/video/login.rs — after fn open_set_password (line ~547)
pub fn open_alert(reason: &'static str) {
    let text: &'static str = match reason {
        "no-users" => "Add a user first (adduser <name>)",
        "storage-not-up" => "Storage is not up yet",
        _ => "Log Out was refused",
    };
    let switched = {
        let mut f = FORM.lock();
        f.message = text;
        if matches!(f.state, State::Open | State::SetPw) { f.state = State::Alert; true } else { false }
    };
    if !switched { open_as(State::Alert); }
    repaint();
}
fn alert_ok() { FORM.lock().state = State::Closed; open(); }
```

## Spec pins

`unaos/scripts/specs/x86-login.spec` (alongside the existing `LOGIN-LOGOUT`/`LOGIN-SCREEN` blocks):

```
REQUIRE :: LOGOUTUI: reason=(no-users|storage-not-up) alert=open -> PASS ::
FORBID :: LOGOUTUI: .* -> FAIL
FORBID \[users\] logout REFUSED session=root reason=no-users$
```

(Last FORBID pins M3's go-red: a bare refusal line with nothing after it — alert never opened — must
not appear; every refusal must be followed by the PASS witness in the same boot log. Three
independent line patterns, no look-around.)

## Open questions

- Exact copy for the two reason strings — R70 gives the sense, not byte-exact text; a person should
  bless the final wording before M3 lands.
- New `State::Alert` variant vs. folding into `State::SetPw`'s slot — plan above assumes a new
  variant (matches R70's "the set-password screen's shape" as a sibling, not a repurposing).

## Next-session start

1. `grep -n "root_logout_refused\|reopen_after_logout" unaos/crates/kernel/src/fs/users.rs unaos/crates/kernel/src/video/login.rs` to re-anchor lines (this doc is from HEAD 4c1c1d75).
2. Land M1 (`root_logout_reason` split) in `users.rs`, `arroyo check`.
3. Land M2+M3 (`State::Alert`, `open_alert`, `alert_ok`, `reopen_after_logout` rewrite) in `login.rs`, then the spec pins above.
