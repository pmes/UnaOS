# x86-twoboot-2.spec — TWOBOOT (rmbp-ledger B526), BOOT 2 of the two-boot fold leg: the SAME image on the SAME volume
# boot 1 wrote its first user to — no rebuild, no reformat — must resolve the session and put the login screen up.
#
#   QEMU gate:  ./arroyo twoboot   (arroyo names this file in its `twoboot)` case, so GATE-SPECROOTS resolves it GATED)
#               → unaos/target/twoboot-2.log; the builder's `qemu_test_wall` stops reading at this file's COMPLETE
#               marker (FASTTEST) and `scripts/twoboot.sh` judges the capture.
#
# WHY. Flight 29's boots 2 and 3 (FLIGHT29.md §2, RESOLVEHANG B524): with una in the store the splash stopped at
# "Resolving the session" — `users::stage_resolve` on the login-screen path — before the console, and nothing reached
# the wire. Boot 1 of a fresh card passes that step (users=0); only a SECOND boot of a volume with a user takes the
# path. This file is that path's end. Lines read off fs/users.rs (`stage_witness`, `stage_publish`, the `stage-resolve`
# boot step) and video/login.rs (`open_as`); the login window is the `State::Open` form, so its open line says
# `power_row=1` (the create-user form of boot 1 says `power_row=0`).

# The store boot 1 wrote, adopted.
REQUIRE \[users\] load volume=el0-fat\(rw\) via=\S+ src=dat users=[1-9]
# RESOLVEHANG's step, begun AND ended.
REQUIRE \[boot\] step=stage-resolve end
REQUIRE :: FIRSTBOOT: stage=login-screen root=locked users=[1-9]\d* desktop_ignited=false why=store-has-users -> coherent ::
REQUIRE \[login\] installer: stage=login-screen
FORBID :: FIRSTBOOT: .* -> incoherent
FORBID \[login\] installer: first-user form open
# The login screen on the glass — the console's first prompt on boot 2 is this window (R86: the login dialog only).
COMPLETE \[login\] screen open window=\d+ box=\d+x\d+ at \(\d+,\d+\) modal=true power_row=1
