# USERMGMT — the multi-user chain's missing verbs

**Finding.** `adduser`/`passwd`/`login`/`logout` exist (`fs/users.rs` `shell_verb`, `passwd_begin`); `delete_user` exists in the store (SECLOGIN M2) but had no verb; there was no `users`, `deluser` or `whoami` verb.
**M1 passwd** already exists (own password, or root `passwd <name>`, via the hidden two-time prompt `prompt_key`, not the `open_set_password` screen). Kept; witnessed.
**M2 users / M3 deluser / M4 whoami** — `usermgmt_verb` at `fs/users.rs` tail; arm folded on the `"passwd"` line of `shell_verb`; word list: `shell.rs` arm (line 5327) and `midden_core` `HOST_VERBS` (line 368), all `feature = "login"`. `deluser`: self refused, non-root refused, root row refused, unknown refused, last user refused, home folder left (said so).
**Witness** `:: USERMGMT: users=N passwd_self=ok passwd_root_other=ok deluser_last=refused deluser_self=refused deluser_ok=1 -> PASS ::` — root half `login_usermgmt_root_fixture` right after LOGIN-ADDUSER (root session live, boot13 sole user), user half `login_usermgmt_fixture` after LOGIN-ROOTOUT (user session supersedes root). Scratch users umg1/umg2 removed. Pins: `unaos/scripts/specs/x86-login.spec`.

## Written
M1–M4 in one commit. Not run (R76).
