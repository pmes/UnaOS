# DIALOG (rmbp-ledger B395) — one alert widget, power confirms, the start-banner toast

Cut from a8a5df91 on `exec-rmbp-dialog`. MACPARITY rows 23 and 34 (cloud review §15, rank 8); row 24 (NOTIFY) grows from `video/toast.rs`.

## Finding (the wire, flights 24/25)
- Every notice is the login screen's `State::Alert` row (`[notice] … NOTICE-OPEN: title=Program stopped … modal=none`, f24 12:43:43/12:44:04, f25 08:26:58/08:27:17): a fault in a program nobody launched from the glass (the `linuxabi` fixture) opens a window with an OK button, and the holocron client's start line (`holocron <verb>: <status>`, `BUS_VERB_NOTICE`) is the same window — a banner made a dialog.
- Shut Down / Restart are POWERMENU's "click again" arming (`:: POWER-UI: armed verb=… window_ms=…`): the second click acts at once; Log Out acts on the first click. No countdown, no ask about unsaved state.

## The seam
CHARTER Kernel — wm, both files: the dialog and the toast are drawn by the WM as kernel rows, like the login screen and Settings.
- `video/dialog.rs` — THE alert widget: icon, bold message, informative text (up to 3 lines), 1–3 buttons, the DEFAULT rightmost and accent-coloured, Return = default, Esc = cancel. A free-standing dialog (centred, upper third) or a SHEET (`sheet_owner_win`: placed under the owner window's title bar and slid down in 4 steps). APP-MODAL: while up, a press on any window of its owner app is swallowed and raises the dialog; the owner's keys (focus held by the owner) go to the dialog; every other window and the console are untouched. It TAKES focus only when the person caused it (a crystal pick) or the owner app held focus — otherwise focus stays where the typing is (`focus_theft` counted, 0 by construction). Posting is queue-only (fault/bus safe); the open runs in `login::notice_service` (the storage pass).
- `video/toast.rs` — the transient: a chromeless compat row (`wm::overlay_open`, owner 0 — never hit-tested, never focused) at the top right under the bar, one title + one line, 3 s, then gone. Queue of 4, queue-only post. aarch64: wire-only (overlay_open is x86 `wc`).
- Power: the crystal's Restart / Shut Down / Log Out pick opens a power dialog (`crystal::power_ask`), counting down 60 s on the glass; OK or expiry fires the verb through crystal's one `fire`, Cancel/Esc closes. Log Out lists apps that declared unsaved state (`dialog::unsaved_declare(owner, name, dirty)` — the hook TEXTEDIT calls; nobody does today).
- Routing: `BUS_VERB_NOTICE` (holocron's start/answer line) -> toast. `Program stopped` -> the dialog ONLY when the dead program was launched from the glass (the dock/Quarry verb drain `dock::take_verb_launch` arms `toast::note_glass`, the x86 bg spawn consumes it into a per-slot bit, the fault's `notice_post` reads `memory::current_slot`), else a toast. The other notices keep the CONSOLEFIX session-notice surface (owed below).

## Milestones
- M1 `video/dialog.rs` — model, paint, app-modal press/key routes (hooked in `users::screen_press`/`screen_key` ahead of the login screen's session-notice arm), sheet slide, the service.
- M2 power confirm — crystal Restart/Shut Down/Log Out through the dialog, 60 s countdown, the unsaved hook.
- M3 `video/toast.rs` + routing (bus notice, Program stopped glass/not).
- M4 `tests notice` extends with `:: DIALOG: … -> PASS ::`.

## Witness (metal)
- `[dialog] open kind=power action=shutdown owner=none sheet=0 focus=taken countdown_s=60 win=<n>`
- `[power] confirm action=<restart/shutdown/logout> countdown_s=60 answer=<ok/cancel/expired>`
- `[toast] show title=<t> ms=3000 focus=kept win=<n>` / `[toast] closed by=timeout title=<t>`
- `[dialog] program-stopped glass=<0/1> -> <dialog/toast>`
- `tests notice`: `:: DIALOG: anatomy=ok default=right esc=cancel modal=app focus_theft=0 confirm=ok toast=ok -> PASS ::`

## Owed
- The remaining session notices (Quarry/Trash errors, Screenshot saved, Low Battery, USB stick removed, Too many windows, Storage read-only, the refused Log Out) still ride the login screen's Alert row (CONSOLEFIX); errors move to `dialog`, information to `toast`/NOTIFY — the NOTICE/LOGOUT fixtures are written against that row and move with it.
- A login-screen power row (Restart / Shut Down on the login form) does not exist yet; the shell's `shutdown`/`reboot` verbs stay immediate (the serial door's control).
- An app-raised dialog verb on the bus (an app asks for an alert/sheet on its own window) — the widget takes `owner`/`sheet_owner_win`; the verb is not cut.
- Toast on aarch64 is wire-only.
