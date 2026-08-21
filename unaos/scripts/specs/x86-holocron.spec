# x86-holocron.spec — the BT-BOND M1 store, ARMED.
#   QEMU gate:  UNAOS_HOLOCRON=1 ./arroyo test-fat sf 150   → unaos/target/serial.log
#   Assert:     python3 scripts/mbench.py --replay unaos/target/serial.log \
#                   --spec scripts/specs/x86-holocron.spec --platform x86 --quiet
#
# SCOPE, and it is narrow on purpose: this file asserts a `holocron`-ARMED boot over a writable FAT
# image AND NOTHING ELSE. It is the companion to the OPTIONAL/FORBID block `x86-fat.spec` carries for
# the same four witnesses. That split exists because `x86-fat.spec`'s own contract is that BOTH the
# default and the knob-on builds pass it, and `holocron` is DEFAULT OFF — so a REQUIRE there would
# red the default gate on every run. The BT-BOND design asks for a REQUIRE; this file is where the
# REQUIRE can actually live without breaking a gate that predates it. See usb_xhci.md §28.7.
#
# A knob-OFF capture CANNOT satisfy this file, and that is the point: silence is the failure mode a
# FORBID cannot catch, and a store whose fixtures can quietly vanish is exactly the defect the
# §10h/DMG-REFUSE notes in `x86-fat.spec` were written about.
#
# MINIMUM BUILD GENERATION — the BT-BOND M1 commit (2026-08-21).

# --- 1. The pure fixtures. No block device, no filesystem, no radio — they run on the first
# --- main-loop pass, so their ABSENCE means the module was compiled out (or the knob never reached
# --- the kernel binary, the s42/INSTGUI failure this arc maps in BOTH arroyo and builder/src/main.rs).
# --- The framing fixture is the CRC REFUSAL PROOF: it does not merely parse a good image, it makes
# --- every refusal fire — body crc, header crc, truncation, trailing bytes, magic, version — and
# --- then re-parses the untouched copy, so the refusals cannot be a parser that refuses everything.
REQUIRE :: \[hcron\] framing fixture: [0-9]+/[0-9]+ legs — clean round-trip, and every refusal \(body crc, header crc, truncation, trailing bytes, magic, version\) FIRED -> PASS ::
REQUIRE :: \[btbond\] codec fixture: [0-9]+/[0-9]+ legs — event parse, 37-byte record round-trip, short/version refusals, key-span agreement, either-form lookup, and the framing CRC refusal all held -> PASS ::

# --- 2. The load reaches a decision and says which one. Either verdict is legitimate on a fresh FAT
# --- image (no store file yet) or on a re-run (a store file the previous run left behind), so the
# --- alternation is the honest REQUIRE — what must never happen is silence, i.e. a load that never
# --- ran because storage never came up.
REQUIRE :: \[hcron\] (load: no store at /HCRON/BTBOND.DAT yet -> store starts EMPTY|loaded n=[0-9]+ from /HCRON/BTBOND.DAT \(seq=[0-9]+\))

# --- 3. The store round-trip through REAL FAT, and the CRC refusal driven end to end through the
# --- real block layer: write, read back byte-identical, then flip ONE byte ON THE MEDIUM and the
# --- load must refuse it. `scratch removed=true` gates the self-clean — a selftest that leaves
# --- litter on the volume is a selftest that changes what the next boot sees.
REQUIRE :: \[hcron\] store round-trip: wrote [0-9]+ bytes to /HCRON/HCRNTEST\.DAT, read back byte-identical, parsed seq=41 n=1; then flipped ONE on-disk body byte and the load REFUSED it \(bad record crc\); scratch removed=true -> PASS ::

# --- 4. A BOND through the real table, the real deferred flush and the real file — staged with no
# --- driver lock held, flushed, looked up by BOTH identity forms, then evicted and re-flushed.
REQUIRE :: \[btbond\] stored addr=aa:bb:cc:dd:ee:ff type=0x04 le=88:c6:26:cc:2d:3c -> staged; flush is deferred past the service pass == witness ::
REQUIRE :: \[hcron\] flush -> /HCRON/BTBOND\.DAT n=[0-9]+ seq=[0-9]+ bytes=[0-9]+ ok == witness ::
REQUIRE :: \[btbond\] store round-trip: staged addr=aa:bb:cc:dd:ee:ff .* looked it up by BOTH identity forms, then evicted \+ re-flushed; store back to n=[0-9]+ -> PASS ::

# --- 5. FORBIDs. The first four catch a red leg (the fixtures use `-> FAIL ::`, which the default
# --- forbids already cover, but naming them keeps the failure legible in the table).
FORBID \[hcron\] framing fixture: .*-> FAIL ::
FORBID \[btbond\] codec fixture: .*-> FAIL ::
FORBID \[hcron\] store round-trip: .*-> FAIL ::
FORBID \[btbond\] store round-trip: .*-> FAIL ::

# --- THE INVARIANT FORBID, and it is the one this whole arc exists for. The store's write is
# --- deferred past the `EHCI_HID` lock BY CONSTRUCTION: the Bluetooth chain that produces a bond
# --- runs inside `service_ehci_hid()` holding that mutex, and the writable FAT volume rides USB mass
# --- storage serviced from the same pass. `flush_if_dirty` CHECKS that (it consults
# --- `EHCI_HID.is_locked()`) instead of trusting its call site, and prints `flush REFUSED` if a
# --- caller ever gets it wrong. That line appearing means the deferral was broken — the exact bug
# --- the design was written to avoid — so it fails the gate rather than being read as the guard
# --- working. A guard that fires is not a guard doing its job here; it is a call site that should
# --- not exist.
FORBID \[hcron\] flush REFUSED

# --- A fail-closed load is legitimate (a torn write on a previous boot), but a load that reports a
# --- refusal on a gate run means the previous run left a bad image, which is a finding about the
# --- flush and not about the medium.
FORBID \[hcron\] load: .* -> store starts EMPTY, fail-closed

# --- A flush that gave up means eight consecutive write failures against a volume the gate believes
# --- is writable.
FORBID \[hcron\] flush -> .* GIVING UP
