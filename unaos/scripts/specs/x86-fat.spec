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
# --- (S8 = STOR-1 S8: a RW open of an arbitrary on-disk file overwrites live, overwrite-only — 2026-07-15.) --
OPTIONAL S8-write: a non-staged on-disk file
# --- (S9 = STOR-1 S9: a RW dynamic on-disk file GROWS past EOF live, bounded per-write/per-file — 2026-07-16.) --
OPTIONAL S9-grow: a dynamic on-disk file
FORBID S4-mf2 FAIL
FORBID S4-race FAIL
FORBID S6-witness FAIL
FORBID S7-openany FAIL
FORBID S8-write FAIL
FORBID S9-grow FAIL

# --- DMG-REFUSE: the SYS_WIN_PRESENT_ROWS(33) refusal arms (-EBADF / -EACCES / -EINVAL), 2026-08-04.
# --- REQUIRE, not OPTIONAL: this witness is headless-complete — two inline ring-3 blobs, no block
# --- device and no panel — so it runs on EVERY x86 QEMU boot, and its ABSENCE is a gate failure. That
# --- is deliberate: a FORBID alone cannot catch silence, and a witness that can vanish quietly is the
# --- exact defect that made the storage witnesses above untrustworthy. Uses the `— witness OK ::`
# --- idiom, so it does NOT add to the `COUNT 22 -> PASS` above.
# --- The third line is the loud-absence rule: every skip path in the launcher prints `NOT RUN`, and a
# --- skip must fail the gate rather than pass it silently.
# --- VERIFICATION PROVENANCE, stated because it is NOT complete: these three lines were proven against
# --- a real capture with `mbench --replay` (green log -> exit 0; a deliberately-reddened log -> exit 1,
# --- caught by the FORBID), but on the `./arroyo test` path, NOT `test-fat` — this host has no `mtools`,
# --- so `make-fat-img.sh` aborts at `mcopy` before QEMU ever starts. The REQUIRE is sound by
# --- construction (same kernel, same `witness` build; the launcher chains off `winx7_launcher` inside
# --- `u8x_launcher`, which does NOT gate on a block device, so the FAT path runs a SUPERSET of the
# --- chain) — but it has not been executed on this exact gate. First runner of `test-fat sf`: if this
# --- line is the only red, suspect this note before suspecting the kernel.
REQUIRE DMG-REFUSE:.*19/19 probes.*witness OK
FORBID DMG-REFUSE FAIL
FORBID DMG-REFUSE:.*NOT RUN
