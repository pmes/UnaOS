# x86-twoboot-4.spec — KEYBOOT4 (rmbp-ledger B864), BOOT 4 of `./arroyo twoboot --card`: the SAME card boot 3 keyed —
# the boot holds at the unlock form, the typist types the password, the worker runs the KDF, the render task binds the
# root, the boot goes on to the login screen, the typist logs una in, the desktop builds.
#
#   QEMU gate:  ./arroyo twoboot --card   (arroyo names this file in its `twoboot)` case, so GATE-SPECROOTS resolves it
#               GATED) → unaos/target/twoboot-4.log; `scripts/twoboot.sh` judges the capture.
#
# WHY. KEYBOOTSPEC (KEYBOOT3 B856, security.md §5): the KEYBOOT3 lines bracket the KDF of a keyed root's unlock — the
# form up before it, `kdf_ms= -> ok` after it on the task that ran it — and no fold leg had a keyed boot to see them.
# Lines read off fs/users.rs (`keyboot_hold`, `keyboot_form_up`), video/login.rs (`kb_dispatch`, `kb_kdf_said`,
# `kb_wire`, `kb_apply`, KEYBOOT4's `kb_drained`), fs/unafs.rs (the `root unlocked` line) and the boot-2 login screen.
# x86 registers no render wake (the hook is the Pi's), so the unlock drains on the render pulse or ahead of a key.

# The hold, the form, and the KDF's wire before it runs.
REQUIRE \[keyboot\] hold OPEN root=keyed
REQUIRE \[login\] unlock form open
REQUIRE \[keyboot\] unlock form up kdf_pending=1 on=worker
REQUIRE \[login\] unlock submit mode=password on=worker
# The KDF returned (KEYBOOT3's line, on the worker), then the bind on the render task (KEYBOOT2's line).
REQUIRE \[keyboot\] unlock kdf_ms=\d+ -> ok
REQUIRE \[keyboot\] unlock kdf_ms=\d+ read_ms=\d+ bind_ms=\d+ .* -> ok
REQUIRE \[login\] unlock drained on=(tick|key)
REQUIRE \[keyboot\] root unlocked handle=\S+ via=password
REQUIRE \[login\] unlock form closed kdf_ms=\d+
REQUIRE \[keyboot\] hold released held_ms=\d+
# The store boot 3 wrote, on the opened root; the login screen; una logged in; the desktop.
REQUIRE :: FIRSTBOOT: stage=login-screen .* why=store-has-users -> coherent ::
REQUIRE \[login\] screen open window=\d+ box=\d+x\d+ at \(\d+,\d+\) modal=true power_row=1
REQUIRE \[login\] session open user=una
REQUIRE \[desktop\] built at=
FORBID \[keyboot\] unlock kdf_ms=\d+ -> bad why=
FORBID \[keyboot\] unlock .* -> refused why=
FORBID \[login\] submit on=worker NO OUTCOME after
FORBID :: FIRSTBOOT: .* -> incoherent
COMPLETE \[boot\] services up first=
