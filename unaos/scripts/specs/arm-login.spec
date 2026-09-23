# arm-login.spec — THE LOGIN STORE, THE SESSION AND THE CREDENTIAL FILE ON THE aarch64 VIRT LANE.
# ARMUSERS (rmbp-ledger B188), 2026-09-23. The aarch64 twin of `x86-login.spec`, cut down to what the
# virt lane can run.
#
#   QEMU gate:  ./arroyo fat-img   (host side: the sandbox has no `sfdisk`)
#               env UNAOS_GICV3=1 UNAOS_VIRT_EL0=1 UNAOS_LOGIN=1 UNAOS_LOGINST=1 UNAOS_FATIMG=sf \
#                   ./arroyo test-arm 120 -> target/serial-arm.log
#               ./arroyo mbench --replay target/serial-arm.log --spec scripts/specs/arm-login.spec
#
# ── WHY THIS FILE EXISTS ─────────────────────────────────────────────────────────────────────────
# Until B188 `test-arm` under `login,loginst,virt_el0` booted to `:: CAPSTONE COMPLETE` with ZERO
# `[users]` lines (B169, orin-ledger A91): the GICv3 virt boot diverges into `run_capstone_boot_core`
# before any storage pass exists, so `fs::users::service()` had no reachable call and every aarch64
# half of SECLOGIN M1-M5 was type-checked and never run. `main.rs::virt_users_pass` is that pass. A
# line this file pins is the proof it ran; without the file, the next change that loses the pass would
# ship as green as the one that never had it, because `test-arm` replays no spec and its fault scan is
# negative-only (it cannot see a line that never printed).
#
# ── WHICH OF THE x86 FIXTURES RUN HERE ───────────────────────────────────────────────────────────
# The `loginst` chain in `fs::users::service` is ARCH-NEUTRAL at the call and dispatches per arch
# inside, so the same fixtures the x86 lane runs run here: LOGIN (the M1 store + session, with the
# aarch64 home ACL and SO37 epoch legs), LOGIN-HARD, LOGIN-IDENT, LOGIN-KOWN (a VERDICT on aarch64 —
# `open_locate` refuses — where x86 prints a measurement), LOGIN-RAND. What stays x86-only, and why:
#   LOGIN-END      `login_end_fixture`'s launch-and-end leg is `#[cfg(target_arch = "x86_64")]`; on
#                  aarch64 it prints `session-end fixture: x86 lane only`. The aarch64 walk
#                  (`session_end_processes`) runs on every Log Out and reads `ended=0` here because no
#                  program is launched under a session on this lane.
#   LOGIN-PRESS / LOGIN-CONTROL / LOGIN-SCREEN / LOGIN-LOGOUT / LOGIN-IGNITION
#                  need the screen, which is compiled under `any(all(x86_64, wc), all(aarch64,
#                  desktop_firmware))`; no desktop exists on the virt lane, so the SO43 seam prints
#                  `-> HELD` and none of these compile.
# Those absences are NOT pinned: a REQUIRE on a line that exists only because something is missing
# certifies the limitation (LAWS §5, "Require a PROPERTY, never a LIMITATION").
#
# NO NUMERICS are pinned that a fixture does not fold into its own verdict: `iters=`, `ms=`, uids,
# `next_uid=`, `passes=` and the volume serial vary per boot and per box.

# ── 0. THE PASS RAN (B188) ───────────────────────────────────────────────────────────────────────
# `serviced=true` is the property: the store loaded (or the service's own refusal bound spoke, which
# prints its own FAIL/SKIPPED verdict below). `block=up` says the stick registered at EL2.
REQUIRE \[armusers\] virt storage pass serviced=true passes=\d+ ms=\d+ block=up
FORBID \[armusers\] virt storage pass serviced=false

# ── 1. THE STORE AND THE SESSION (LOGIN M1, SECLOGIN M2) ─────────────────────────────────────────
# The fresh-store form and the loaded form both start `src=`; a stick that carried a store from an
# earlier boot reads `src=dat users=<n>`. A refused image is a defect on this medium.
REQUIRE \[users\] load volume=el0-fat\(rw\) src=(none|dat|new) users=\d+
FORBID \[users\] load .* REFUSED
REQUIRE \[users\] login ok user=una id=\d+ principal=user:una#\d+
REQUIRE \[users\] home=/home/una (created|exists) volume=[0-9a-f]{8}
# The aarch64 home ACL (LOGIN M2) — the leg B169 named "type-checked, not run".
REQUIRE \[users\] home-acl path=HOME/una/NOTES\.TXT created=true owned=true anon_refused=true owner_ok=true same_user_ok=true deleted=true
REQUIRE :: LOGIN-EPOCH: .* stale_refused=true .* -> PASS ::
FORBID :: LOGIN-EPOCH: .* -> FAIL
REQUIRE \[users\] logout epoch=\d+ ended=\d+ windows=\d+
# `linked=true` is the claim that the session principal machinery is IN this image (`aarch64_el0`,
# here through `virt_el0`); `linked=false` would mean the principal half compiled out.
REQUIRE :: LOGIN: users\+session create=(ok|exists) verify=ok wrong=refused login=ok principal=user:una#\d+ linked=true home=(created|exists) acl=ok epoch=ok logout=ok users=\d+ volume=el0-fat -> PASS ::
FORBID :: LOGIN: users\+session -> FAIL
# On this lane the stick is a FAT32 superfloppy (`UNAOS_FATIMG=sf`); a skip means it was lost.
FORBID :: LOGIN: users\+session -> SKIPPED

# ── 2. THE KDF (SECLOGIN M1) ─────────────────────────────────────────────────────────────────────
REQUIRE \[users\] kdf calibrated iters=\d+ ms=\d+
FORBID \[users\] kdf REFUSED
REQUIRE \[users\] rehash user=hard1 v1->v2 iters=\d+ ms=\d+
REQUIRE :: LOGIN-HARD: kat=ok iters=\d+ ms=\d+ v2_rows=\d+ migrated=1 legacy_verify=ok migrated_verify=ok wrong=refused unknown=refused floor=10000 -> PASS ::
FORBID :: LOGIN-HARD: .* -> FAIL

# ── 3. THE IDENTITY RULE (SECLOGIN M2) — the aarch64 ACL half, run for the first time ──────────────
REQUIRE \[users\] ident path=HOME/IDENT\.TXT owner=identa#\d+ same_slot=identb#\d+ refused=true reason=recycled-id same_name=identa#\d+ refused=true reason=recycled-id
REQUIRE :: LOGIN-IDENT: a_uid=\d+ b_uid=\d+ a2_uid=\d+ slot_reused=true owner_ok=true same_slot_refused=true same_name_refused=true reason=recycled-id -> PASS ::
FORBID :: LOGIN-IDENT: .* -> FAIL
REQUIRE \[users\] delete user=identa uid=\d+ \(slot \d+ freed; uid never reissued

# ── 4. THE CREDENTIAL FILE IS KERNEL-OWNED (SECLOGIN M4) — a verdict on this arch ──────────────────
REQUIRE :: LOGIN-KOWN: pred=ok resolver=refused errno=-\d+,-\d+ reason=kernel-owned -> PASS ::
FORBID :: LOGIN-KOWN: .* -> FAIL
FORBID :: LOGIN-KOWN: .* resolver=OPENED

# ── 5. ENTROPY (SECLOGIN M5) ─────────────────────────────────────────────────────────────────────
# QEMU's cortex-a72 has no FEAT_RNG, so this lane reads `jitter`; `rndr` is what an ARMv8.5 core
# says. `rdrand` cannot occur on aarch64 and is not in the alternation.
REQUIRE \[rand\] source=(rndr|jitter) probe=\S+ bits=256
REQUIRE :: LOGIN-RAND: source=(rndr|jitter) distinct=true nonzero=true same_source=true salts_differ=true draws=\d+ epoch_bits=64 -> PASS ::
FORBID :: LOGIN-RAND: .* -> FAIL

# ── 6. THE IGNITION (SO43) ───────────────────────────────────────────────────────────────────────
# The seam was reached. The screen opens iff a desktop exists, never on a console route; the FORBID
# is the SO43 inversion (a screen opened with no desktop up).
REQUIRE \[login\] ignition desktop_up=(true|false) console_routed=(true|false) -> (OPEN|HELD)
FORBID \[login\] ignition desktop_up=false console_routed=(true|false) -> OPEN

# ── 7. THE END OF THE RUN ────────────────────────────────────────────────────────────────────────
# The virt GICv3 boot's last line (B188 M3 declares the same marker as `test-arm`'s LADDER-TAIL row). A capture
# without it is TRUNCATED on replay, never scored.
COMPLETE :: CAPSTONE COMPLETE .* sync primitives verified in one boot ::

# ── CONTRACT (SPECRUN, 2026-09-15) ──────────────────────────────────────────────────────────────
# A PINNED LINE IN THIS FILE IS CHANGED TOGETHER WITH THE KERNEL LINE IT PINS, IN THE SAME COMMIT —
# re-pinned to the new wording (naming the arc that changed it), or dropped with the reason stated.
#
# WHO RUNS THIS FILE:
# RUN-BY: knobleg — ./arroyo fat-img (host), then env UNAOS_GICV3=1 UNAOS_VIRT_EL0=1 UNAOS_LOGIN=1 UNAOS_LOGINST=1 UNAOS_FATIMG=sf ./arroyo test-arm 120, then
#          ./arroyo mbench --replay target/serial-arm.log --spec scripts/specs/arm-login.spec
#   EVERY KNOB IS LOAD-BEARING. `UNAOS_GICV3=1` is the regime `virt_el0` needs (the EL0 window is
#   installed on the JC3 drop, which only the GICv3 branch takes) and the branch that carries
#   `virt_users_pass`; `UNAOS_VIRT_EL0=1` links the session principal (`linked=true`, the home ACL,
#   the epoch, the ident ACL, the `open_locate` refusal); `UNAOS_LOGIN=1` compiles the store and the
#   pass; `UNAOS_LOGINST=1` compiles the fixtures (a BOOT-TIME WRITE of user `una` to the stick);
#   `UNAOS_FATIMG=sf` attaches a FAT volume (the default `usb.img` has none and the service would
#   print SKIPPED). NOT on any default gate.
#
# GATE-SPECROOTS (`scripts/spec-roots.sh`, a leg of `./arroyo check`) reds by name on any spec under
# scripts/specs/ that is neither named in `arroyo`'s CODE nor carries a RUN-BY line above.
