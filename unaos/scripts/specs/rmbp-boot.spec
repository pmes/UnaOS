# rmbp-boot.spec — minimal 2012 rMBP metal boot sanity (USBDEBUG esp build).
#   Metal:  ~/rmbp-serial.log (rmbp-bench-connect.sh bridge capture; the FTDI
#           console is TX-only — capture and assert, never inject)
#   Validated against the REAL 2026-07-10 STOR-1 bench capture
#   (~/unaos-bench/rmbp-serial-2026-07-10-172056.log, a knob-ON build).
#
# Minimal by design: boot came up, the FTDI console is live, storage enumerated,
# and the U-chain ran to its tail. The full chain assertion is x86-fat.spec;
# the full round-6 witness list is round6-rmbp.spec.

REQUIRE U2.5-0: DR7 cleared
REQUIRE U2.5: FTDI console up
REQUIRE U2.5: FTDI TX mirror -> PASS
REQUIRE U1a: user exited ok
REQUIRE U3.5: ring-3 preemption.*-> PASS
REQUIRE xHCI: Disk
REQUIRE U11m2:.*-> PASS
REQUIRE U6gx: UnaFS owner/grants.*-> PASS

# knob-ON (irqstorage) lines — present on the 2026-07-10 capture, but a default
# build must also pass this minimal spec
OPTIONAL bx-blockreq: PASS
OPTIONAL S3: synchronous write-through.*-> PASS

# The storage demo verdict. Pattern lesson: the USBDEBUG boot banner quotes the
# words 'MISSION SUCCESS', so a bare "MISSION SUCCESS" regex false-positives on
# the banner — always anchor on the real line's ">>>" frame. Absent from the
# 2026-07-10 capture (demo path not taken on that boot), hence OPTIONAL here.
OPTIONAL xHCI: >>> MISSION SUCCESS

# forbidden: any CPL-0 fault diagnostic (defaults -> FAIL / FAIL :: / PANIC always on)
FORBID EXCEPTION:
