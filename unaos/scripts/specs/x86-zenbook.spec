# x86-zenbook.spec — THE ZENBOOK IMAGE LINE, BOOTED UNDER QEMU AS THE ZENBOOK. ZENBOOKLANE (zenbook-ledger Z13), 2026-10-27.
#
#   QEMU gate:  ./arroyo test-zenbook 90 -> target/serial.log
#               (= scripts/image-line-zenbook.env minus its [media] row, every inherited UNAOS_* unset first, plus
#                UNAOS_PLATFORM=zenbook_um5606w UNAOS_CENSUS=1 UNAOS_USBSERIAL=1, on a fresh card; tests-at-boot OFF)
#               ./arroyo mbench --replay target/serial.log --spec scripts/specs/x86-zenbook.spec --platform x86
#
# RUN-BY: verb:test-zenbook — `arroyo` names this file in code as `X86_ZENBOOK_SPEC`; `test_zenbook` hands it to
#          `test_x86_64` as X86_TEST_SPEC (its COMPLETE ends the wall, its REQUIREs are scored by `x86_test_completion`)
#          and `x86_pick_capture_spec` replays it (the ZENBOOKLANE arm, asked before the SELFHOST skip).
#
# WHY. The Zenbook-only kernel arms answer `hw::platform()`, and q35's SMBIOS resolves `qemu`, so no QEMU leg reached them
# (GOPIGNITE-FIXTURE, Z10). `UNAOS_PLATFORM` (hw.rs `platform_knob`, witness builds only) writes the platform's one
# resolution before `platform_say` prints it; the DMI strings stay QEMU's own. NOTHING here is the metal (ZENBOOK-FIRSTBOOT).
#
# MEASURED AT THE CUT (ae8272e02), and why some expected lines are NOT pinned:
#   * `[hw] skip smc|gmux|hda` — ZERO lines: the SMC (feature smc), the gmux (gmux_igd + intel-ivb) and the HDA codec
#     drive (hda) are not on the Zenbook line, so no gate call is linked. A future line that adds one prints its skip here.
#   * `[mt7925] s2` — SILENT: with no 14c3 device on the bus `mt7925::s2` returns before its platform test.
#   * the typed fixtures (`:: PLATFORM: … src=knob … -> PASS ::`, `:: EC:`, `:: NVME:`, `:: I8042:`) register only from the
#     `tests` verb, which needs a session on this `login` line (the first-user form typed): ZENBOOK-LANETYPED, owed.

# ── 1. THE OVERRIDE: the platform's one resolution says the Zenbook, by the knob ──────────────────────────────────────
REQUIRE \[hw\] platform=zenbook_um5606w dmi=".*" vendor=".*" src=knob

# ── 2. Z8: the EC census runs on a non-Apple platform; q35 carries no ECDT, so it stops before any port I/O ───────────
REQUIRE \[ec\] -> SKIP reason=no-ecdt

# ── 3. Z2: the NVMe controller (UNAOS_NVME on the line; the builder attaches the QEMU nvme device) ─────────────────────
REQUIRE \[nvme\] ctrl mn=".*" ns=1 lba=\d+ blocks=\d+ -> ready

# ── 4. Z10: the compositor ignites on the GOP framebuffer (no Kepler takeover) — the GopBoot arm's first QEMU run ──────
REQUIRE \[wc-x\] gop-ignite platform=zenbook_um5606w fb=\d+x\d+

# ── 5. Z7: the xHCI census (one controller on q35) ─────────────────────────────────────────────────────────────────────
REQUIRE \[xhci\] controllers=\d+ picked=\d+ bdf=\d+:\d+\.\d+

# ── 6. Z3: the 8042 keyboard controller (q35 has one; only the MacBookPro10,1 skips it) ───────────────────────────────
REQUIRE \[i8042\] keyboard id=[0-9a-f]{4} set=1xlat irq=(routed|polled) -> ready

# ── END OF RUN: a fresh card has no users, so the boot stops at the first-user form (BP-boot-7) ────────────────────────
COMPLETE \[login\] installer: first-user form open
