# K2 metal bench — cross-reboot enforcement on real Pi 4 silicon

Attended (Peter physical + this executor session drives the software). Two parts: **A** one-boot sanity
(the mechanism on silicon), **B** the money-shot (a REAL two-boot power-cycle survival via the
`UNAOS_K2_LEAVE` knob, K2 M(e)).

## Prereqs (all parts)
- **16 GB `UNAOS` card only** — the Pi 4 EEPROM refuses the 31 MB SD-1.0 `UNAOSRW` (zero output).
- Raspberry Pi Debug Probe wired; `scripts/pi-serial-bridge.py` running (`run_in_background`), dated log
  under `~/unaos-bench/`, `~/pi-serial.log` symlink.
- **Verify the bridge captures a FULL boot first** — the dated log must visibly grow *past the kernel
  banner* before any witness matters. A log stuck at a few KB mid-bench = probe re-enumeration → re-run
  `pi-bench-connect.sh` (that IS the recovery) and note the gap. Use `stat -Lf %z` for symlink sizes.
- **Pristine card:** delete stale `UNAFS.ATR` + `K2PRIV.BIN`; for a clean Part-A/boot-1 battery also
  restore pristine `GROW.BIN` (512×0xC1) + `SCRATCH.BIN` (1024×0xEE) and delete `OWNED/FRESH/DELME/B11.BIN`.
- **Unmount `UNAOS` before every `kernel8` build.**
- Watch-item (both parts): **any `EC=0` heal line on the A72 Pi is NEWS** (the A72 is NOT in the
  a78ae-1941500 erratum range) — capture + report. A crash-leftover `K2PRIV.BIN` early in a recovery boot
  is EXPECTED (probe_once may cat it), not a failure.

## Part A — one-boot sanity (NORMAL build; the mechanism on silicon)
1. `./arroyo kernel8` (normal — no knob). Flash `target/UnaOS-pi4-baremetal.img` (Raspberry Pi Imager →
   custom → microSD). Boot.
2. `./arroyo mbench --follow ~/pi-serial.log --spec scripts/specs/pi4-regression.spec --timeout 120`
   (let the timeout run for the panic-path/late-FORBID stretch; --follow early-exits on completion).
3. **PASS =** 23 PASS + CAPSTONE 6/6 + `:: K2-liveenf: … rebuild+enforce PASS [w=0x7f] ::` +
   `K1-persist … PASS` + `K1-corrupt … PASS` + F2/F3 witnesses locked 240000/240000, and the spec's
   FORBID (R1 error / programming-busy timeout / AARCH64 EXCEPTION) all clear.

## Part B — the money-shot (LEAVE build; a genuine two-boot power-cycle survival)
The `k2_leave` build swaps the same-boot `k2_liveenf` for a two-boot proof: boot-1 LEAVES a persisted
owned file; boot-2 verifies the LIVE boot rebuild reinstalled it across the power-cycle. **One image,
flashed once, booted twice — do NOT re-flash between boots.**

1. `UNAOS_K2_LEAVE=1 ./arroyo kernel8`. Flash the LEAVE image onto a **pristine** card. (`arroyo` prints
   `• UNAOS_K2_LEAVE: k2_liveenf → two-boot money-shot mode`.)
2. **Boot-1:** boot; verify — with a pristine card the full battery runs (23 PASS + CAPSTONE 6/6) plus:
   `:: K2-metal: BOOT-1 left K2PRIV.BIN persisted (owner prog:K2OWN.BIN, fc=0x..) on disk — POWER-CYCLE
   NOW; boot-2 verifies the live boot rebuild survived the reboot ::`.
   ⚠ If instead you see `:: K2-metal: BOOT-1 leave incomplete … (do NOT power-cycle; re-prep the card) ::`
   → the create didn't fully land; re-prep and retry boot-1.
3. **POWER-CUT the Pi.** You own the timing, but there is **no rush** — the fixture has already written
   the row + file to disk before it prints the BOOT-1 line, so cut power any time after you see it.
4. **Boot-2** (same card, NO re-flash): boot; verify:
   `:: K2-metal: BOOT-2 cross-reboot SURVIVED a real power-cycle — owner prog:K2OWN.BIN re-admitted BY
   NAME against the LIVE-boot-rebuilt UNAFS.ATR row, impostor prog:K2IMP.BIN refused (-EACCES);
   self-cleaned rebuild+enforce PASS [w=0x07] ::`.
   ⚠ **Boot-2's battery is STATEFUL-DEGRADED** — boot-1 mutated the card, so U9/U10/U6-grants/U11
   self-skip (`already present pre-demo — skipped`) or print `sector_changed=false` / `size_grew=false`
   false-FAILs. These are NOT regressions (the arroyo `-> FAIL|FAIL ::` detector correctly ignores them);
   **the money-shot verdict is the single `K2-metal … SURVIVED … PASS [w=0x07]` line**, not the battery
   count. Boot-2 self-cleans (`K2PRIV.BIN` deleted) → the card ends pristine.

Verification for Part B is by the distinctive `K2-metal` line (the standard `pi4-regression.spec` REQUIREs
`K2-liveenf`, which the LEAVE build does not print). Eyeball it, or add a one-line `pi4-k2-metal.spec` that
`REQUIRE`s `K2-metal: BOOT-. … PASS`.

## QEMU pre-proof (already done, for reference)
The two-boot logic is QEMU-proven via same-image `if=sd` write-back (a genuine reboot for the FAT):
boot-1 → `BOOT-1 left … fc=0x14`; boot-2 → `BOOT-2 … SURVIVED … PASS [w=0x07]`, self-cleaned, 0 FAIL,
CAPSTONE 6/6. On metal the reboot is a real power-cycle.

## Close
Metal-verdict docs commit on `hw-pi4` (the `a82eacc` pattern), landing report to the seat (ccd), seat
merges the K2 M(e) knob + closes the round-6 Pi metal item.
