# SETTINGSREKEY (rmbp-ledger B483) — Settings' password change re-keys the ring, as `passwd` does

**Finding** (RINGLOGIN2 B479's owed list; read on c191d8d7): Settings' "Password" (General pane) and the Users
pane's own-row "Password" / other-row "Reset" all call `video/login.rs::open_set_password(name, false)`; the
form asks New + Retype only and its worker (`compute`, Job::SetPw) calls `users::set_password_checked` and
nothing else. No old password is asked, no REKEY door is posted, no `ring=kept … admin-reset` line is said:
a password changed in Settings leaves the ring under the old key and the next login reads
`[holocron] ring=refused at=login … bad-password`. FLIGHTS 24/25 carry no Settings password change (their
`[holocron]` lines are `tests holocron`'s `ring=none`), so this is read off the code, not the wire.

**Seam** (R79): unchanged — `holocron_core` is the one ring implementation, HOLOCRON.ELF the registered
fulfiller, the kernel the password change's half of the door (`keyring::ring_rekey`). Settings gets NO second
path: the change form's worker makes the SAME three calls `passwd` makes (`users::verify` the old password →
`users::set_password_checked` → `keyring::ring_rekey`), and an administrator's reset makes the same call
`passwd <name>` makes (`keyring::ring_kept_admin_reset`). The only new kernel state is the `at=` word.

**Milestones**
- M1 `keyring.rs`: `ring_rekey_at(name, old, new, at)` / `ring_kept_admin_reset_at(name, at)` (the old entry
  points pass `"passwd"`); the at word is kept for HOLOCRON.ELF's answer so `rekey_report` says
  `at=settings`; a per-boot count of Settings re-keys.
- M2 `video/login.rs`: `open_change_password(name)` — own row and not root: the change form (Old password ·
  New · Verify, "Change"); another user's row (an administrator's Reset) or root's own: the set form with the
  one line. Job::SetPw carries the mode and the old password; the worker (`login-submit`, never the render
  core) verifies the old password FIRST (a wrong one writes nothing: `reason=bad-old-password`, "Current
  password is wrong"), writes, then derives both Argon2id keys and posts the REKEY door — all on the worker.
  `video/settings.rs`: the three entries call `open_change_password`; the Users pane's Reset says
  `the user's secrets stay under the old password until they log in with it`.
- M3 `tests ringlogin` grows `settings_rekey=<ok|FAIL|no-session>`: the mode map (own → change, other →
  reset, root → reset) and the REAL worker body (`compute`) with a wrong old password on the session's row
  refuses `bad-old-password` and the typed new password does not verify after (nothing written).

**Witness** (no password, no key on the wire):
- open: `[login] change-password screen open user=<u> mode=<own|admin-reset> …`
- wrong old: `[login] set-password user=<u> NOT written reason=bad-old-password (the form stays)`
- own change: `[login] submit kdf_ms=<n> … on=worker … kind=setpw`, then
  `[holocron] door kdf=kernel mode=rekey … at=settings -> posted …` and on HOLOCRON.ELF's report
  `[holocron] ring=rekeyed at=settings user=<u> ms=<n>`
- admin reset: `[holocron] ring=kept at=settings user=<u> reason=admin-reset …` and
  `[settings] users op=reset name=<u> ok=1 reason=set-password-screen` with the pane's one line
- `tests ringlogin`: `:: RINGLOGIN: … settings_rekey=ok settings_rekeyed=<n> … -> PASS ::`

**Owed**: aarch64 has no worker core (`worker_core()` is None off x86), so there the change form's KDFs run
inline on the render task (as every login submit there does); HOLOCRON.ELF is x86-only (RINGLOGIN2's owed);
the set/change form has no Cancel (Esc does nothing on this window, R24) — a Settings-opened form is left only
by setting the password; metal unflown.
