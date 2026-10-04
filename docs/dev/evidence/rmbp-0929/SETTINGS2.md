# SETTINGS2 (R75) — tabs, Users, Display, About

## Design
Finding: `video/settings.rs` was one page of nine controls. Mechanism: `State` gains `strip` + `UsersUi`; `Values.tab` persists as `tab=` in `<home>/.settings`; tab strip painted at `settings.rs` `paint()`, rows below `TOP = 12 + TAB_H`.
- M1 TABS: General (brightness, volume, mute, pointer, wallpaper, password) · Users · Display · About. Left/Right on the strip (or on a non-slider control) or a click switches; Up from the first control focuses the strip.
- M2 Users: list via `users::name_at`/`password_unset`; root sees Add user (form: name + password twice, rules from `users::create_user_rules`, now shared with `installer_create_user`), Reset (opens `login::open_set_password(name,false)`; `submit_setpw` now writes through `users::set_password_checked` = unset row, root session, or own row), Delete (two-step Yes/No; runs the real `deluser` verb so refusals self/root-row/last-user/not-root and the `[users]` witness are the verb's own). A user sees the list and own Password. Every action prints `[settings] users op= name= ok= reason=`.
- M3 Display: idle-blank moved here; UI scale shown READ-ONLY (`wm::info(id).scale`; scale is fixed at takeover, not a runtime value); menubar clock skipped (CLOCKBAR has no runtime glyph switch, per TESTFIX2). About: `UNAOS_GIT_SHA` (else "dev build"; no selfhost provenance read), arch as board, ACPI CPU count (x86), kernel heap size, uptime.
- Witness: `:: SETTINGS: controls= tabs=4 loaded= saved= -> PASS ::` (x86-wc.spec pin updated) and `:: SETTINGS-USERS: listed= added= deleted= refused= -> PASS ::` from `tests settings` (`selftest_all`).

## Written
Boot 17 `tests settings` should show both lines. Under a non-root session the Users leg PASSes on refusals (added=0 deleted=0); under root, added=1 deleted=1.
