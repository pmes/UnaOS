# pi4-desktop.spec — the ARMED Pi desktop battery (PIDESK-SPEC, rmbp-ledger B878).
#
#   QEMU gate:  ./arroyo kernel8-test --desk 300     (= UNAOS_PIDESK=1 ./arroyo kernel8-test 300)
#               → unaos/target/serial-pi.log, replayed against pi4-regression.spec FIRST and then this file.
#   Metal:      mbench.py --replay ~/pi-serial.log --spec <this file> --platform pi (a UNAOS_PIDESK image).
#
# ── WHY THIS IS A SECOND FILE (the DRAG-PI rule, written once) ─────────────────────────────────
# The default `kernel8-test` battery is KNOB-OFF: `desktop_firmware` arms only under UNAOS_PIDESK, and
# mbench has no knob-conditional REQUIRE. So every `desktop_firmware`-gated line in pi4-regression.spec is
# OPTIONAL + FORBID there (a REQUIRE would red the knob-off gate for doing what it should). Its POSITIVE form
# lives HERE, and `test_kernel8` replays this file ADDITIONALLY when UNAOS_PIDESK is armed
# (`k8_pick_capture_spec` in arroyo, the x86 picker's shape; SUITETYPE's second-spec pattern, as
# pi4-barename.spec). A new desktop_firmware line lands as OPTIONAL + FORBID in pi4-regression.spec AND as a
# REQUIRE here (or an OPTIONAL with its promotion condition written beside it).
#
# NO FORBID HERE: pi4-regression.spec runs on the armed battery too and already forbids every FAIL direction
# (dragperf, dragwedge, arm_skip, rescue, SHARD-PRESS, OVERLAYS, `win=shell … rows_ok=0`). This file carries
# only the positive half.
# NO `COMPLETE` HERE, deliberately (pi4-barename.spec's argument): truncation is the base spec's call — it owns
# the end-of-run markers and is the completion source — and a base TRUNCATED (3) or FLAKE (4) verdict is
# returned unchanged without this file being asserted (BP-gates-3).
# NO LOOK-AROUND: pi4-regression.spec's PORTABILITY RULE holds here (foreman's regex crate reads it too).
#
# MEASURED, not run (2026-10-27, at 3ead33dc7, code-only): each pin is the emitter's format string, cited. The
# one armed capture on record (docs/dev/evidence/pikernel8test-1013/serial-pi.log, QEMU raspi4b 640x480) shows
# the four REQUIRE lines' families; SHARD-PRESS and dragperf read FAIL there (pi-ledger PI19/PI20, open), the
# same lines pi4-regression.spec's FORBIDs already red. The bench's first `./arroyo kernel8-test --desk 300`
# is this file's first measure.

# --- REQUIRE: lines an untyped armed boot prints (an armed caller exists at the cut) -----------------------
# `desktop_firmware::activate` (video/desktop_firmware.rs:276): the desktop took the panel and routed the console.
REQUIRE \[deskfw\] activate panel=[0-9]+x[0-9]+ console_win=[0-9]+ routed=true
# `crystal::routed_selftest` (video/crystal.rs:1301), called at the activate seam under `witness`
# (desktop_firmware.rs:375): the crystal press reaches the glass and comes back down.
REQUIRE :: SHARD-PRESS: .* :: PASS ::
# DRAG-PI M4 `wm::dragperf_selftest` (video/wm.rs:22655) and DRAGWEDGE `wm::dragwedge_selftest`
# (video/wm.rs:22929): registered from arch/aarch64/syscall.rs:15486 in the aarch64 launcher cascade and run
# at registration under `tests-at-boot` (the kernel8-test lane exports it). Their FAIL forbids stay in
# pi4-regression.spec (:2311, :2339, :2349, :2360).
REQUIRE \[dragperf\] panel=[0-9]+x[0-9]+ .* -> PASS
REQUIRE \[dragwedge\] furniture aimed=.* -> PASS

# --- OPTIONAL: lines with NO trigger on an untyped aarch64 boot at the cut, each with its promotion ---------
# `desktopbuild::build`'s only aarch64 caller is fs/users.rs:3070 (stage-resolve, BootStage::Desktop); the armed
# capture has no stage-resolve and no build line. Promote with PIDESK-BUILT (whether the Pi desktop takes the
# DESKTOPBUILT object, R92/R93).
OPTIONAL \[desktop\] built at=
# SHELLFACE (B858), TEXTSCALE (BP-video-40), OVERLAYARM2 (B866): `tests textscale` and `tests overlays` are registered
# only in `tests::ensure_shellux` (tests.rs:175). PIDESK-LIST (B889) lists both `@pi4 @orin` in TESTS.LIST's HEAVY tier,
# run by `bootlist` from `boot::oldest_say` (a `login` build's first `[desktop] built`) on a `kernel8` card that staged the
# list. `kernel8-test --desk` has neither (no `login`, and `bootlist_stage` is media-only), so the four stay OPTIONAL here;
# the shell repaint moves only on the textscale epoch. Promote all four with PIDESK-QEMULIST (the desk lane reaches the list).
OPTIONAL \[kfont\] repaint shell epoch=[1-9][0-9]* console_rows=[1-9]
OPTIONAL :: OVERLAYS: launcher=ok appswitch=skip\(x86\) .* -> PASS ::
OPTIONAL :: TEXTSCALE: steps=[0-9]+ .* -> PASS ::
OPTIONAL \[textscale\] win=shell css=[0-9]+ grow=[0-9]+ rows_ok=1
# BOOTLIST: BOOTLISTARM (B874) runs the list on aarch64 too, but only from a `login` build's first build with a staged list (PIDESK-QEMULIST, B889).
OPTIONAL :: BOOTLIST: ran=[0-9]+ pass=[0-9]+
# KEYBOOT3 (B856) / KEYBOOT4's Pi worker: a keyed card only (fs/users.rs:4058, video/login.rs:3800).
OPTIONAL \[keyboot\] unlock form up kdf_pending=1
OPTIONAL \[keyboot\] unlock kdf_ms=[0-9]+ -> ok

# ── CONTRACT (SPECRUN, pi4-regression.spec's tail) ───────────────────────────────────────────────────
# A PINNED LINE IN THIS FILE IS CHANGED TOGETHER WITH THE KERNEL LINE IT PINS, IN THE SAME COMMIT. New rows
# go at the tail (rows elsewhere cite spec lines positionally).
# RUN-BY: verb:kernel8-test — `arroyo` names this file in code (`PI4_DESKTOP_SPEC`, picked by
#          `k8_pick_capture_spec` when UNAOS_PIDESK=1), and `k8_desk_replay` folds its rc into the verb's rc.
