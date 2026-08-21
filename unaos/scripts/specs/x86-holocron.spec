# x86-holocron.spec — the BT-BOND M1 store, FULLY ARMED (store + selftests).
#   QEMU gate:  UNAOS_HOLOCRON=1 UNAOS_HCRONST=1 ./arroyo test-fat sf 150   → unaos/target/serial.log
#   Assert:     python3 scripts/mbench.py --replay unaos/target/serial.log \
#                   --spec scripts/specs/x86-holocron.spec --platform x86 --quiet
#
# BOTH KNOBS ARE NEEDED TO SATISFY THIS FILE (HCR1, 2026-08-21). `holocron` arms the STORE; `hcronst`
# (which implies `holocron`) arms the two selftests that WRITE the boot medium at boot — the
# `store round-trip` pair in §4 and §5 below. The split follows the repo's standing convention for a
# destructive write, the same one that gives `sdw` a knob apart from `sdhcblk`: arming a MECHANISM
# must not be the same act as arming a TEST that writes, or M2's real consumer could not have the
# store without the selftests' writes to the user's medium. A `UNAOS_HOLOCRON=1`-only capture
# satisfies §1, §2, §3 and §6 and is SHORT on §4/§5 — the honest outcome, not a regression.
#
# SCOPE, and it is narrow on purpose: this file asserts an ARMED boot over a writable FAT image AND
# NOTHING ELSE. It is the companion to the OPTIONAL/FORBID block `x86-fat.spec` carries for the same
# witnesses. That split exists because `x86-fat.spec`'s own contract is that BOTH the default and the
# knob-on builds pass it, and `holocron` is DEFAULT OFF — so a REQUIRE there would red the default
# gate on every run. The BT-BOND design asks for a REQUIRE; this file is where the REQUIRE can
# actually live without breaking a gate that predates it. See usb_xhci.md §28.7.
#
# A knob-OFF capture CANNOT satisfy this file, and that is the point: silence is the failure mode a
# FORBID cannot catch, and a store whose fixtures can quietly vanish is exactly the defect the
# §10h/DMG-REFUSE notes in `x86-fat.spec` were written about.
#
# MINIMUM BUILD GENERATION — the HCR1 commit (2026-08-21). A BT-BOND M1 build cannot print §3.

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

# --- 3. THE DEFERRAL BOUND (HCR1). The guard that keeps the store's write out of the EHCI service
# --- pass reads `EHCI_HID.is_locked()`, which is a GLOBAL predicate: it reports that the lock is
# --- taken, never by whom. It therefore cannot tell a flush issued from INSIDE `service_ehci_hid()`
# --- from an ordinary interleaving in which the `input` task simply held the lock while `usb-pump`
# --- sampled it. The guard is read in the one direction it is sound in ("free" PROVES this stack is
# --- outside the pass) and "held" is treated as a DEFERRAL, and the whole hazard then becomes the
# --- witness: the first version of this guard returned before `flush_fails`/`gave_up`, so a dirty
# --- store over a contended lock printed once per main-loop pass without bound.
# --- This fixture is the proof that it no longer can. It takes EHCI_HID FOR REAL, drives
# --- `flush_if_dirty` past the escalation point (HCRON_DEFER_STUCK + 64 passes), and checks three
# --- things: every driven pass reached and took the deferral return, ZERO writes were issued (seq
# --- unmoved, the dirty flag it set still set), and the witness emitted no more than
# --- HCRON_DEFER_NOTES lines. It writes nothing, so it rides `holocron` and not `hcronst`.
REQUIRE :: \[hcron\] deferral bound: EHCI_HID HELD, flush_if_dirty driven [0-9]+ times .* every pass deferred, ZERO writes issued .* the witness emitted [0-9]+ line\(s\) against a cap of [0-9]+ -> PASS ::

# --- 4. The store round-trip through REAL FAT, and the CRC refusal driven end to end through the
# --- real block layer: write, read back byte-identical, then flip ONE byte ON THE MEDIUM and the
# --- load must refuse it. `scratch removed=true` gates the self-clean — a selftest that leaves
# --- litter on the volume is a selftest that changes what the next boot sees. Needs `hcronst`.
REQUIRE :: \[hcron\] store round-trip: wrote [0-9]+ bytes to /HCRON/HCRNTEST\.DAT, read back byte-identical, parsed seq=41 n=1; then flipped ONE on-disk body byte and the load REFUSED it \(bad record crc\); scratch removed=true -> PASS ::

# --- 5. A BOND through the real table, the real deferred flush and the real file — staged with no
# --- driver lock held, flushed, looked up by BOTH identity forms, then evicted and re-flushed.
# --- Needs `hcronst`. The `flush -> ... ok` line is also what proves the publish SWAP completed: the
# --- flush stages into /HCRON/BTBOND.NEW, reads it back and parses it, and only then drops the live
# --- leaf and renames the staged one over it, so `ok` means the swap landed and not merely a write.
REQUIRE :: \[btbond\] stored addr=aa:bb:cc:dd:ee:ff type=0x04 le=88:c6:26:cc:2d:3c -> staged; flush is deferred past the service pass == witness ::
REQUIRE :: \[hcron\] flush -> /HCRON/BTBOND\.DAT n=[0-9]+ seq=[0-9]+ bytes=[0-9]+ ok == witness ::
REQUIRE :: \[btbond\] store round-trip: staged addr=aa:bb:cc:dd:ee:ff .* looked it up by BOTH identity forms, then evicted \+ re-flushed; store back to n=[0-9]+ -> PASS ::

# --- 6. FORBIDs. The first five catch a red leg (the fixtures use `-> FAIL ::`, which the default
# --- forbids already cover, but naming them keeps the failure legible in the table).
FORBID \[hcron\] framing fixture: .*-> FAIL ::
FORBID \[btbond\] codec fixture: .*-> FAIL ::
FORBID \[hcron\] deferral bound: .*-> FAIL ::
FORBID \[hcron\] store round-trip: .*-> FAIL ::
FORBID \[btbond\] store round-trip: .*-> FAIL ::

# --- WHAT REPLACED THE OLD INVARIANT FORBID (HCR1). This file used to carry
# --- `FORBID \[hcron\] flush REFUSED`, on the reading that the guard firing at all meant a call site
# --- had issued the flush from inside the EHCI service pass. That reading was not available from the
# --- evidence: `is_locked()` names no holder, so the witness's claim that "this call site is inside
# --- it" was false whenever the lock was merely contended — and `usb-pump` (which calls the store's
# --- service) and `input` (which reaches `service_ehci_hid` at ~1 kHz) are two preemptible tasks
# --- main.rs spawns on the same svc_cpu, so contention is an ordinary outcome on a CORRECT build.
# --- The FORBID could therefore red on scheduler luck. It is gone, the witness no longer accuses,
# --- and what gates instead is §3: a fixture that MAKES the deferral path run thousands of times and
# --- proves it writes nothing and cannot print without bound. That is reachable on every armed run,
# --- rather than only on the schedule that happens to contend the lock.

# --- A fail-closed load is legitimate (a torn write on a previous boot), but a load that reports a
# --- refusal on a gate run means the previous run left a bad image, which is a finding about the
# --- flush and not about the medium. (When it does fire, the refused bytes are renamed to
# --- /HCRON/BTBOND.BAD rather than overwritten — see usb_xhci.md §28.3.)
FORBID \[hcron\] load: .* -> store starts EMPTY, fail-closed

# --- A flush that gave up means eight consecutive write failures against a volume the gate believes
# --- is writable.
FORBID \[hcron\] flush -> .* GIVING UP
