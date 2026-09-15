# PI — WHAT IS IN FLIGHT AND WHAT IS OWED (the Pi 4 track queue, R45)

One file, fixed path, not per-round. Kept current AS WORK HAPPENS, never at close. Only jobs that need
the Pi's metal or the Pi's own files live here (`drivers/emmc2.rs`, `arch/aarch64` Pi arms,
`pi4-regression.spec`, `flash-pi4.sh`, `kernel8*` verbs); everything else is a row in the trunk queue
`docs/dev/QUEUE.md` on `main`. Ledger: `docs/dev/OS/pi-ledger.md` (PI<n>) and `docs/dev/LEDGER.md` (SP<n>).
`✓` = verified in this tree · `·` = inherited, not re-checked. Re-derive every sha before acting.
Created 2026-09-12T00:35Z (2026-09-11 local; dates here are UTC) by orin 27 (all lanes, R46) from pi-ledger PI1-PI8, the SP rows and pi 11's archived
focus queue (`docs/dev/evidence/pi11/FOCUS-QUEUE.md`, 2830 lines — a SUMMARY ARTIFACT, re-source every number).

## STATE — 2026-09-12T01:4xZ
✓ origin/hw-pi4 153c78dd; local hw-pi4 = this commit, clean. `git rev-list --left-right --count main...hw-pi4` with main at 4a03404f: main +6 (rmbp landing, R45-R48, QUEUE.md) / pi +48 (docs + the DRAG/STORM/V3D video and sched work + this file). Landing now (review panel ACK 2026-09-12).
· Pi bench legs (R39): `kernel8-test`, `arm virt v2`, `arm virt v3 (CAPSTONE)`, the arm usb-write witness — pi's, unconditionally.

## METAL — needs the Pi 4 on the bench
· PI6  the spec's CAPSTONE caveat (`pi4-regression.spec:36-40`) pre-excuses a 3-of-4-core boot; conditional on a power-cycle measurement
· PI8  CMD2 ALL_SEND_CID is latched and thrown away — the block registry discriminates cards by SIZE only; consumer question open
· SP17  no Pi card-write path checks the TARGET's identity (`flash-pi4.sh` verifies the image, never the card) — the Orin's card became a Pi card this way
· SP15  the in-kernel installer keeps destruction-time consent in a build-time env var
· render-class boot on the bench Pi: the 46 owed commits have never executed on metal (DRAG-ADMIT, DRAGWIDE, CHROMEBAND, REDZONE, FATGROW, STACKPOOL, V3D rungs)

## PI'S OWN FILES — QEMU raspi4b gates them
· NEW 2026-09-12: the Pi card image is formatted by the same `make-pi-img.sh` line FATCLUST changed (`-s 1`); on dosfstools 4.2 the Pi's 55 MiB FAT already got 1 sector/cluster (110,874 clusters) so the geometry is unchanged here, but no Pi flight has run on a FATCLUST-built image and the GPU boot ROM's view of 512-byte clusters is unverified — flash one and boot it before trusting it (trunk queue §5 has the guard objections).
· PI1  `drivers/emmc2.rs` header claims NO writes; `drivers/block.rs` routes SD writes to it (doc lie, one edit + a test that reads the header)
· PI2  the registry-full FORBID for `pi4-regression.spec` — one line, precondition measured, unwritten
· PI4  `/volumes` and `/usb` documented-and-ungated on pi — a REQUIRE naming the `/usb` write posture
· PI5  `[pstrip]` FORBID depends on four adjacent emitter fields with no contract at the emitter (the `[wc-g]`/`[wc-h]` family has one)
· PI7  `:359 OPTIONAL K1-atr` is silently unfailable — promote to REQUIRE (was "grant #9"; no grants since R46)
· PI3  `[wc-d] moved=` emitted, never consulted — BLOCKED on purpose (tightening first manufactures a red); revisit after PI5
· SP9  `pi4-regression.spec:262 COUNT 26` is a fleet-wide population wearing a local shape
· S18 / S28  `pidesk` phantom knob sites and stale prose in arch-neutral `main.rs`/`video/menubar`
· S19 / S20  Pi serial: two producers, one drain; `read_byte` returns None for both open-bus and no-data
· S22 / S23  parked patches in `plans/unaos/wip/` (u11 measure twin; pi4-owed-tail-reland) — apply or archive, never leave loose
· SO7 / B26  `kernel8-test` flaky under host load — the pi half is the quiet-box run that measures the rate

## SHARED, ROUTED TO THE TRUNK QUEUE (listed so the cut is visible)
· SP2 / SP4 / SP5 / SP6 / SP11 / SP18 are process records, not jobs — they stay in LEDGER.md
· the virt-leg ownership question is settled (LAWS §3, R39 amendment): virt legs are pi's; any aarch64 seat may run them for its own landing
