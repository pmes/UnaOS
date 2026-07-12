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

# --- STOR-1 S5 shared-backing witness (knob-ON only — the default sf run is knob-OFF, so
# --- OPTIONAL here; a dedicated knob-on spec is a ledgered tooling candidate. Knob-on FAT
# --- ideal PASS count = 25 incl. S5; the S5 FAIL text evades arroyo's detector, so witness
# --- PRESENCE is the real gate on knob-on runs — seat fold at the S5 merge, 2026-07-11) ----
OPTIONAL S5: cross-process read serves LIVE shared backing

# --- STOR-1 knob-ON uncounted witnesses (S4-mf2 / S4-race / S6-witness / S7-openany): they use
# --- the `— witness OK ::` idiom (NOT `-> PASS`), so they do NOT add to the COUNT above and the
# --- default FORBIDs (`-> FAIL`, `FAIL ::`, `PANIC`) do NOT catch their `FAIL — …` failure text.
# --- So on knob-on runs, PRESENCE (OPTIONAL) + an explicit per-witness FORBID on the FAIL variant
# --- is the gate. Knob-off the sf run omits them entirely, so all stay silent — both builds pass.
# --- (S7 = STOR-1 S7: an open of an arbitrary on-disk file, off the pre-stage set — 2026-07-12.) --
OPTIONAL S4-mf2: RW open of staged code
OPTIONAL S4-race
OPTIONAL S6-witness: NAMESPACE cross-core RMW
OPTIONAL S7-openany: a non-staged on-disk file
FORBID S4-mf2 FAIL
FORBID S4-race FAIL
FORBID S6-witness FAIL
FORBID S7-openany FAIL
