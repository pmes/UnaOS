# DIALOG2 (rmbp-ledger B404) — every notice sorted, the login Alert window deleted, the login power row, the bus dialog verb, the explicit spawn origin

Cut from 7811df99 on `exec-rmbp-dialog2`. Continues DIALOG (B395, `dialog.md`). MACPARITY rows 23/24/33.

## Finding (the tree at 7811df99, the wire of flights 24/25)
- The login screen's `State::Alert` still carries every session notice but the two DIALOG moved: `notice_show`/`notice_post`
  (`video/login.rs`) from Quarry (`ops.rs` refusals, the Trash two-step, Get Info), Screenshot saved (`prtscr.rs`), Low Battery
  (`powerui.rs`, at 10 % and 5 %), USB stick removed (`xhci`), Too many windows (`wincap.rs`), Storage read-only
  (`users::write_root_file`) and the refused Log Out (`login::refused_alert`). The wire still says `:: NOTICE-OPEN: … modal=none`.
- The login form has no power row (MACPARITY 33). Flight 25: "can't get past root set password" — the only way off that screen was
  the power button.
- No bus verb raises an app's own alert; `unsaved_declare` has no caller; TEXTEDIT's close "asks nothing: a dirty close is lost work".
- `toast::note_spawn` guesses "launched from the glass" by a 3 s window after the dock's verb drain.

## The seam
CHARTER Kernel — wm (no new file under `video/`; `src/origin.rs` is a crate-root kernel file, the spawn's own provenance).
- ONE router, `dialog::notice(title, text)` (queue-only, every old `notice_*` caller and `users::screen_notice`): the SORTING TABLE
  `dialog::SORTED` — ERRORS (`Quarry`, `Storage read-only`, `Log Out`) become the alert widget, app-modal to their owner (a Quarry
  error is a SHEET on the Quarry window); INFORMATION (`Screenshot saved`, `USB stick removed`, `Low Battery`, `Too many windows`, and
  any untabled title: the Trash two-step, Get Info) becomes a toast. `Program stopped` keeps DIALOG's glass/not rule, now read from
  the explicit origin. Low Battery fires at 20 % then 10 %.
- The login `Alert` state, `Ctl::AlertOk`, `ALERT_PREV`, the NOTICES queue and the CONSOLEFIX session-notice machinery are DELETED.
  The NOTICE and LOGOUT fixtures prove the same properties on the dialog/toast.
- The login form (Log in / Locked) carries a POWER ROW under the password field: Sleep · Restart · Shut Down. Sleep is instant
  (the crystal's `fire`); Restart / Shut Down go through DIALOG's 60 s confirm, raised OVER the screen (the dialog takes the modal
  ceiling while up and hands it back). Keyed: Tab walks Name → Password → Sleep → Restart → Shut Down; Return or Space presses.
- Bus verbs `BUS_VERB_DIALOG`/`SHEET`/`TOAST` (20/21/22, one codec in `una-abi`, one fulfiller `dialog::bus_fulfil`, both arches):
  a program posts message/info/buttons with a token; the answer reaches the poster's input ring as `INPUT_EV_DIALOG_ANSWER`
  (payload token<<8 | button). The door's `dialog <dialog|sheet|toast> <message>` drives the same fulfiller (GATE-VERBS row).
- TEXTEDIT is the first caller: `unsaved_declare` on every dirty flip; a dirty close asks a SHEET "Do you want to save the changes…"
  (Don't Save · Cancel · Save).
- The spawn ORIGIN is explicit: `origin::with(Origin::Glass|Door, …)` around the dock/Quarry launch and the typed shell line; a spawn
  with no scope is a service's (`system`). `[spawn] origin=<glass/door/system> slot=<n>`; the 3 s window is gone.

## Milestones
- M1 router + sorting table + every caller moved; Dlg owns its title/buttons; Low Battery 20/10.
- M2 the login Alert window deleted; NOTICE/LOGOUT fixtures on the dialog/toast.
- M3 the login power row (clicked and keyed).
- M4 the bus verbs + answer event + door verb; TEXTEDIT asks on close.
- M5 the explicit spawn origin.
- M6 `tests notice` -> `:: DIALOG2: … -> PASS ::`.

## Witness (metal)
- `[notice] route title=<t> kind=<error/info> -> <dialog/toast>`
- `[login] power row pick=<sleep/restart/shutdown> via=<click/key>`; `[dialog] open kind=power action=restart … screen=1`
- `[dialog] bus verb=<dialog/sheet/toast> owner=<n> token=<n> buttons=<n>`; `[dialog] answer … delivered=<0/1>`
- `[edit] close asks (dirty) -> sheet`; `[spawn] origin=<glass/door/system> slot=<n>`
- `tests notice`: `:: DIALOG2: alert_window=deleted errors_to_dialog=3 info_to_toast=4 login_power_row=1 bus_verbs=3 origin=explicit -> PASS ::`

## Owed
- Sleep is the crystal's stub (no S3 path) — the row's Sleep says `unimplemented` exactly as the crystal's does.
- Origin is x86 only on the wire (aarch64's spawn path does not stamp it); the scope is one global (a service spawning during a
  long door command reads `door`).
- No ring-3 library wrapper for the dialog verbs yet (the codec is in `una-abi`); Lumen is not a caller.
- Empty Trash is still the two-step (now a toast); a real Cancel/Empty confirm is the dialog's next caller.
