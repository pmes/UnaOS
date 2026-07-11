# x86-fat.spec — the x86 U-chain over the sf FAT image.
#   QEMU gate:  UNAOS_FATIMG=sf ./arroyo test 150   → unaos/target/serial.log
#   (the battery's "x86 test-fat sf 300" step asserts the same chain)
#
# Default (irqstorage knob-OFF) build: 22 `-> PASS` verdicts, no MISSION line
# (the sf fixture path skips the storage demo banner). The knob-ON build adds
# the STOR-1 lines — OPTIONAL below so both builds pass this spec.

# --- the aggregate: 22 fixture verdicts --------------------------------------------
COUNT 22 -> PASS

# --- ring-3 bring-up + fault plumbing ----------------------------------------------
REQUIRE U2-0c: self-NMI taken on IST -> PASS
REQUIRE U2-0c: canonical-rcx guard refuses
REQUIRE U1a: user exited ok
REQUIRE U1b: fault isolation.*-> PASS
REQUIRE U2-0a: TF\+SYSCALL survived -> PASS
REQUIRE U3: per-process CR3 isolation
REQUIRE U3.5: ring-3 preemption.*-> PASS
REQUIRE U2: loaded program exited ok -> PASS

# --- the capability chain ----------------------------------------------------------
REQUIRE U4x: x86 process model.*-> PASS
REQUIRE U5x: x86 capabilities.*-> PASS
REQUIRE U6x: x86 general object table.*-> PASS
REQUIRE U6bx: x86 real File handles.*-> PASS
REQUIRE U7x: cross-process transfer.*-> PASS
REQUIRE U8x: revocation trees.*-> PASS

# --- the FAT write/lifecycle chain -------------------------------------------------
REQUIRE U9x: real File writes.*-> PASS
REQUIRE U10: file growth.*-> PASS
REQUIRE U10c: file create.*-> PASS
REQUIRE U10d: file delete.*-> PASS
REQUIRE U11x: open-file lifecycle.*-> PASS
REQUIRE U11m2:.*-> PASS
REQUIRE U6gx: UnaFS owner/grants.*-> PASS

# --- STOR-1 knob-ON (irqstorage) lines: present only on that build ------------------
OPTIONAL bx-blockreq: PASS
OPTIONAL S3: synchronous write-through.*-> PASS

# --- forbidden: any CPL-0 fault diagnostic (defaults -> FAIL / FAIL :: / PANIC are
# --- always on; ring-3 intentional faults print nothing — verified on a clean run) --
FORBID EXCEPTION:
