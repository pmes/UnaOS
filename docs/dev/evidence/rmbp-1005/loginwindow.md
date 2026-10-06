# LOGINWINDOW (rmbp-ledger B430, MACPARITY row 33) — design

**Finding.** The login window already carries the user tiles (login.rs `user_row`, one tile per user, the picked one
highlighted, the password field under the roster), the generic avatar (loginwindow.rs `avatar`, FIRSTUSER B409) and
DIALOG2's power row (Sleep / Restart / Shut Down, Tab-walked; Restart and Shut Down through
`dialog::power_confirm_on_screen`, the session's own confirm). Missing: (a) the avatar is the same figure for
everyone — no per-user picture, no initials; (b) Sleep calls the crystal's `Verb::Sleep`, which only prints
"unimplemented" — it is not greyed and names no lane; (c) no arrow-key picking, no Esc back; (d) no `tests
loginwindow`, no `[login] window` glass line.

**Seam.** No new store (R79). The avatar's source is the user's HOME DIRECTORY's `una:icon` attribute (the same
key ASSOC writes on type objects), read through the mount table the shell builds; its value is an APPRES icon key
(a built-in or sighted program's key) drawn by APPRES's own decoder (`appres::blit_icon_known`, no generic
fallback). No attribute, or a key APPRES does not know, gives the initials on a disc drawn from the name (theme
tokens only). The sources are resolved once per opening of the log-in form (never per repaint) into a small table
in loginwindow.rs. Sleep is LIDSLEEP's (B431): `crate::power::sleep_request()` / `sleep_armed()` are declared
here as stubs (`unarmed` / false) for LIDSLEEP's fold to replace; the button greys while `sleep_armed()` is false
and a press says `-> unarmed`.

**Milestones.** M1 avatars (attr or initials) + Sleep greyed through `power::sleep_request`. M2 keyboard: Left/Right
(0x1D/0x1C) pick the previous/next tile, Return on an arrow-picked tile opens the password field, Esc from the
field goes back to picking (clears the password); glass line on every pick. M3 `tests loginwindow` + MACPARITY
row 33 folded.

**Witness.** Glass (on every pick, click or key): `[login] window users=<n> picked=<name> avatar=<attr:KEY or initials>`.
Test: `:: LOGINWINDOW: users=<n> avatars=<n> power_row=<ok or FAIL> keys=<ok or FAIL> sleep=<armed or unarmed> -> PASS ::`.

**Owed.** Sleep itself (LIDSLEEP B431 — the stubs go at their fold); a picture file as an avatar (a path value) and
a Settings > Users control to set `una:icon` (today `setfattr <home> una:icon=<key>`); the Mac's
password-field-under-the-picked-tile-only layout (the field stays under the roster, the tile row is the pick);
fast user switching.
