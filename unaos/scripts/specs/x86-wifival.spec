# x86-wifival.spec — WVAL-REPLAY: the set-validation dry-run, asserted from QEMU.
#
#   QEMU gate:  ./arroyo fat-img
#               UNAOS_FATIMG=sf UNAOS_WIFIVAL=1 ./arroyo test 150   -> unaos/target/serial.log
#               ./arroyo mbench --replay target/serial.log \
#                        --spec scripts/specs/x86-wifival.spec --platform x86
#
# WHAT THIS FILE IS FOR. Everything the wifi loader does before a device is touched is byte-level
# work on files — the FAT search, the bounds checks, `classify_header`'s container verdict, and arc
# 2's `validate_set()` dry-run over the whole staged set. None of it needs a radio. But QEMU models
# no BCM4331, so until WVAL-REPLAY the census refused at `S_START`, the module parked, and not one
# line of that work ran anywhere but on the bench rMBP. WCLS-RECORD (2b2a125e) then taught the
# classifier the metal-pinned container truth — the 8-byte header, `size`=bytes for the ucode
# word-stream and `size`=RECORD COUNT for the initvals record-streams — and that new knowledge had
# exactly one gate: a bench round. This spec is its second, and the fast one.
#
# SCOPE — this file asserts a `UNAOS_WIFIVAL=1` boot over the `sf` FAT image AND NOTHING ELSE. The
# knob is default-OFF, so the standing `UNAOS_FATIMG=sf ./arroyo test` run does NOT satisfy this
# spec and is not meant to: x86-fat.spec is that run's gate and is unaffected by this one.
#
# MEDIA PRECONDITION, and it is a real one. The three b43 files are USER-SUPPLIED (UnaOS ships no
# firmware — CLEAN_ROOM_POLICY.md §4): `make-fat-img.sh` copies them into B43/ on the image from
# UNAOS_B43_DIR (default /home/pmes/Downloads/bcout/b43) if that directory exists, and skips
# silently if it does not. On a checkout without the extraction the STAGED and set-validate lines
# below go red — correctly, because the leg genuinely did not run. If those are the ONLY reds,
# suspect the media before suspecting the classifier: check that `./arroyo fat-img` printed its
# `added B43/ (3/3 ...)` line.
#
# MINIMUM BUILD GENERATION — the WVAL-REPLAY commit. No image built before it can print any line in
# this file, because `wifival` did not exist as a feature.

# --- the leg is ARMED, and says so ------------------------------------------------------------
# The census still prints its own honest refusal; THIS line is the one that says what happens
# instead of the park, so a replay boot can never be mistaken for a metal boot in a capture.
REQUIRE :: wifi: census=ABSENT .* wifival REPLAY armed: staging \+ set-validation dry-run proceed, NO device rung will run ::

# --- the set was found on the media and CLASSIFIED --------------------------------------------
# One line per role, from the loader's own `classify_header`. The `hdr=` field is the load-bearing
# half: `words` for the microcode word-stream, `records` for BOTH initvals record-streams. That
# per-role split IS the WCLS-RECORD finding — the earlier cross-file "layout uniformity" rule is
# exactly the reading the 178-byte bsinitvals file falsified — so a regression that collapses the
# two back into one verdict reds a line here rather than waiting for the bench.
# `stream=ok` is `classify_header`'s shape check: whole be32 words, or a record walk that consumed
# the payload in exactly the declared number of records.
REQUIRE :: wifi: ucode STAGED .* hdr=words .* stream=ok ::
REQUIRE :: wifi: initvals STAGED .* hdr=records .* stream=ok ::
REQUIRE :: wifi: bsinitvals STAGED .* hdr=records .* stream=ok ::
REQUIRE :: wifi: firmware set COMPLETE 3/3 staged

# --- the dry-run itself: per-role verdicts, then the set verdict -------------------------------
# `want=` is the REQUIRED layout for that role and `=> VALID` means the classified layout matched it
# AND the shape rule held. Naming the role in each pattern is deliberate: a single `=> VALID`
# COUNT 3 would also be satisfied by one role validating three times, which is precisely the failure
# a per-role rule exists to catch.
REQUIRE :: wifi2: set-validate ucode .* hdr=words want=words .* stream-ok=1 => VALID
REQUIRE :: wifi2: set-validate initvals .* hdr=records want=records .* stream-ok=1 => VALID
REQUIRE :: wifi2: set-validate bsinitvals .* hdr=records want=records .* stream-ok=1 => VALID
REQUIRE :: wifi2: set-validation verdict=VALID

# --- the terminal park, and the shape of the leg ----------------------------------------------
REQUIRE :: wifi: wifival REPLAY begin .* the radio is ABSENT
REQUIRE :: wifi: wifival REPLAY park verdict=VALID staged=3/3

# --- FORBIDDEN: the other verdicts. A spec that only REQUIREs the green line passes a boot that
# --- printed BOTH (it cannot happen today — the park has one print site per branch — but the
# --- FORBID is what keeps that true if the branches are ever split).
FORBID :: wifi: wifival REPLAY park verdict=INVALID
FORBID :: wifi: wifival REPLAY park verdict=INCOMPLETE
FORBID :: wifi: wifival REPLAY set-validation SKIPPED
FORBID :: wifi2: set-validate .* => INVALID
FORBID :: wifi2: set-validation verdict=INVALID
FORBID :: wifi2: set-validate FAILED
FORBID :: wifi: .* REJECTED .* reason=

# --- FORBIDDEN: EVERY DEVICE RUNG. This block is the whole safety claim of the replay leg, stated
# --- from the capture side. `validate_replay()` has no PCI config access, no `map_mmio_window`, no
# --- MMIO read and no window-selector write anywhere in its call graph — but "I read the function
# --- and it looked clean" is not an instrument. Each line below is a witness that arc 2's
# --- `bringup_once`/`explore` prints at a specific rung, so a single one of them appearing in a
# --- replay capture means the ABSENT branch reached the device ladder, and the run fails.
# ---
# --- Ordered as arc 2 would execute them: the banner, the live config re-read, the BAR0 map, the
# --- cfg:0x80 pre-image and its unwind self-test, the two window moves, ChipCommon's raw read, and
# --- the restore. (`wifi2:` lines that are NOT device rungs — the set-validate family above — are
# --- deliberately not swept by a blanket `wifi2:` FORBID, which would forbid the very rung this
# --- spec exists to require.)
FORBID :: wifi2: begin
FORBID :: wifi2: precond [0-9a-f]{2}:[0-9a-f]{2}\.
FORBID :: wifi2: map bar0=
FORBID :: wifi2: pre-image cfg:0x80=
FORBID :: wifi2: unwind-selftest
FORBID :: wifi2: WROTE cfg:0x80
FORBID :: wifi2: cc-raw chipid=
FORBID :: wifi2: RESTORE cfg:0x
FORBID :: wifi2: upload
FORBID :: wifi2: end
