# x86-aether.spec — AETHERLANE (rmbp-ledger B1004): `tests aether` on a QEMU wire — the SR109 drive and the PDF4KERNEL
# (B999) fixture PDF, scored by a verb for the first time.
#
#   QEMU gate:  UNAOS_WC=1 UNAOS_AETHER=1 UNAOS_QEMU_FULL=1 ./arroyo test 300 -> target/serial.log
#               (`x86_pick_capture_spec`'s AETHERLANE arm replays this file: UNAOS_AETHER + UNAOS_WC, no medium or
#                lane knob; x86-test.spec's COMPLETE stays the completion source, as on x86-wc.)
#
# WHAT RUNS. No typist: `tests aether` (crates/kernel/src/aether.rs `selftest` -> `drive` -> `chrome`) is driven by the
# KERNEL — it pushes the operator's keys and pointer into AETHER.ELF's ring and reads each step's verdict tail off the
# serial seam. Under tests-at-boot (the `./arroyo test` head case) `tests::register` runs it at registration. Tail 13
# types Ctrl+L /system/doc/fixture.pdf (the builder's `stage_welcome` stages crates/user-aether/doc/*.pdf) and
# `pdf_verdict` prints `:: AETHERPDF: tail=Some("PASS pages=2 …") -> PASS ::` beside SR109's `:: AETHERAPP: … ::`.
#
# WHY ITS OWN LANE AND NOT x86-wc. Arming aether on x86-wc moved that lane's knob set (`aether` implies `selfdiag`;
# TEXTSCALE2's aether leg spawns instead of `skip(no-aether-knob)`) under ~1000 lines measured without it, and until
# B1004 every page load with no icon printed `:: AETHER: favicon -> FAIL reason=none ::`, which mbench's default
# `-> FAIL` forbid reds in every spec (user-aether main.rs `favicon` now says SKIP for "no icon declared").
#
# THE WALL. 300 = x86-wc's 240 + the drive's worst case by the code, rounded: WAIT_MS 20 s for the page report +
# 13 steps x STEP_MS 3 s = 59 s (plus <= 500 ms per push into a full ring). Measured on metal (f36-boot3) the whole
# drive took ~5 s; NO TCG capture of it exists yet, hence UNAOS_QEMU_FULL=1 (AETHERLANE-TIMING owed: restate the
# first TCG capture's report_ms and step tails here, then drop QEMU_FULL).
#
# NAMED RISKS on the first capture, each red through a named line, never silent: (i) under TCG a 3 s STEP_MS can miss
# a layout-heavy step (url / find) -> AETHERAPP FAIL; (ii) `:: AETHER: net … -> FAIL` if the https step's fetch fails
# inside the ~50 ms before `bg_kill` (metal f36: the kill won; AETHERLANE-NET owed); (iii) TEXTSCALE2 `aether=bad` on
# this wc lane; (iv) selfdiag's fixtures, which this lane is the first QEMU wc lane to compile.
#
# RUN-BY: knobleg — UNAOS_WC=1 UNAOS_AETHER=1 UNAOS_QEMU_FULL=1 ./arroyo test 300 (arroyo names this file in code: X86_AETHER_SPEC)

# --- the kernel PDF face (B999): the program opened the fixture and page 1 drew -----------------------------------
REQUIRE :: AETHER: pdf -> PASS pages=2 page=1/2 dpi=
REQUIRE :: AETHERPDF: tail=.* -> PASS ::
FORBID :: AETHERPDF: .* -> (FAIL|SKIP) ::

# --- SR109 restated: the drive's verdict (M3 + the chrome) -------------------------------------------------------
REQUIRE :: AETHERAPP: image=/apps/AETHER\.ELF window=elf .* tab=1 url=1 back=1 find=[1-9]\d* -> PASS ::
FORBID :: AETHERAPP: .* -> (FAIL|SKIP) ::
