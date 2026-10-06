# FIRSTUSER (rmbp-ledger B409) — R100: the Mac's first boot

**Finding.** The tree is R77's: `root_credential_ignition` makes a `root` row with no credential and opens the
set-password screen for it (`[login] set-password user=root …`); `stage_of_store` says Installer until root's
password is set, then CreateUser, then Desktop. No row carries a role; every privileged path asks
`root_session()` (no user session + the boot's root latch). Flight 25's `[login] set-password user=root NOT
written reason=volume` is the root step R100 removes.

**Seam.** `fs/users.rs` is the store (fs-core): the ROLE is a byte of the v2 row (offset 115, inside the CRC
span; 0 = standard, 1 = admin — every row written before this arc reads standard). Root is a PRINCIPAL, not
a row: no row is ever made for it, `set_password`/`create_row` refuse the name, `login_root` refuses. The
administrator's authority is ONE function, `users::admin_authority(action)`, which every privileged path
calls and which prints `[auth] admin=<name> for=<action>`. The login window's users list and the generic
avatar are a new file, `video/loginwindow.rs` (CHARTER Kernel — wm), hooked from `login.rs` by one call in
`user_row` and one in `repaint`'s lock arm.

**Milestones.**
- M1 — store: the role byte, `Role`, `role_of`/`admin_name`/`admin_session`/`privileged`, root locked
  (no row, no password), the R77->R100 migration at the store's load
  (`[users] migrated R77->R100 admin=<name> root=locked persisted=<0/1>`).
- M2 — the flow: no user rows -> Installer, whose one screen is the first-user form (name, password,
  retype; R86's bare setter, unchanged phase gate); the user it makes is the administrator; FIRSTBOOT's
  predicate reads `root=locked` instead of `root_set`; `shot setter` opens the first-user form; LOGIN-ROOTPW
  proves root locked.
- M3 — authority: `admin_authority` on `install ssd --write`, Settings > Users add (with an Admin toggle:
  admin or standard), and the verbs that were root-only (`adduser`, `deluser`, `passwd <other>`) accept an
  administrator's session; `[serialdoor] principal=<system or name>` on each principal change.
- M4 — the login window: the generic avatar on every user tile, the session user's tile on the lock screen,
  the power-row hook DIALOG2 sets, and the witness.

**Witness.** First boot, at the first user's session:
`:: FIRSTUSER: flow=mac first_user=<name> role=admin root=locked prompts=1 login_window=users+power -> PASS ::`;
later boots at the login window `prompts=0`; `tests firstuser` re-prints it.

**Owed.** The updater and R99's boot-FAT unlock do not exist in this tree: they call `admin_authority`
when they land (one line each). The power row is DIALOG2's (B404): its row sets `loginwindow::POWER_ROW`
at the fold, until then the witness reads `login_window=users -> FAIL — power=absent`. APPRES avatars.
Fast user switching.
