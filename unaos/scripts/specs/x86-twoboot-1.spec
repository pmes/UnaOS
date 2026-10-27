# x86-twoboot-1.spec — TWOBOOT (rmbp-ledger B526), BOOT 1 of the two-boot fold leg: a FRESH volume, no user, the
# first-user form typed by the leg's QMP typist, through `[login] desktop bare` to the desktop and its services.
#
#   QEMU gate:  ./arroyo twoboot   (arroyo names this file in its `twoboot)` case, so GATE-SPECROOTS resolves it GATED)
#               → unaos/target/twoboot-1.log; the builder's `qemu_test_wall` stops reading at this file's COMPLETE
#               marker (FASTTEST) and `scripts/twoboot.sh` judges the capture.
#
# WHY. Flight 29's boot 1 (FLIGHT29.md §2, SESSIONWEDGE B525): the first user was created, `[boot] phase=desktop
# from=setter why=session` printed, `[login] desktop bare: furniture=none (R88) why=sessio` was CUT MID-WORD and the
# kernel printed nothing more — no `[desktop] built`, no `[boot] services up`. `./arroyo test` boots once with no user
# and never reaches this path; this file is the end of the path that wedged. Every line below was read off the kernel's
# own print sites on this tree (fs/users.rs, video/login.rs, loginfurn.rs, video/desktopbuild.rs, boot.rs) and off
# f29-boot1.log, never invented.
#
# The leg's typist types `una` / `correct-horse` twice into the form (the store's first user, the administrator: R100).
# KEYBOOT3 (rmbp-ledger B856): no `[keyboot] unlock form up` / `unlock kdf_ms= ->` REQUIRE here — this volume is FRESH, so its
# hold reads `[keyboot] root=none hold=none` and never opens the unlock form. KEYBOOT4 (B864): the keyed QEMU boot is
# `./arroyo twoboot --card`'s boots 3-4 (x86-twoboot-3.spec keys a native card, x86-twoboot-4.spec unlocks it); this file is unchanged.

# The Installer stage on the fresh volume, and the form the typist types into.
REQUIRE :: FIRSTBOOT: stage=installer root=locked users=0 desktop_ignited=false why=store-loaded -> coherent ::
REQUIRE \[login\] installer: first-user form open
REQUIRE \[login\] key taken by the screen
# The first user, written to the volume — boot 2 stands on this row.
REQUIRE \[login\] installer: create-user user=una id=\d+ home_created=true
REQUIRE \[login\] session open user=una role=admin
REQUIRE \[boot\] phase=desktop from=setter why=session
REQUIRE \[login\] desktop bare: furniture=none \(R88\) why=session
# SESSIONWEDGE's missing lines: the desktop object built, then the services.
REQUIRE \[desktop\] built at=
# OLDEST (rmbp-ledger B682, BP-bar-1, BP-boot-15): the line's oldest machine, printed by `boot::oldest_say` at the boot's first build.
REQUIRE :: OLDEST: board=rmbp12 year=2012 image=[^ ]+ net=(absent|present) -> BOOTS ::
# TRUSTSTAGE (rmbp-ledger B836, net.md §5): the trust bundle the https clients open, read once by `boot::trust_say` beside
# OLDEST. The bench stages it first (tools/trust-bundle, then the builder's stage_trust); a bench without it reds this row.
REQUIRE \[trust\] roots\.pem bytes=\d+ pin=[0-9a-f]{8} -> ok
FORBID :: LOGIN: users\+session -> FAIL
FORBID :: FIRSTBOOT: .* -> incoherent
COMPLETE \[boot\] services up first=
