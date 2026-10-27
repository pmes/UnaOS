# x86-zenbook.spec — THE ZENBOOK IMAGE LINE, BOOTED UNDER QEMU AS THE ZENBOOK. ZENBOOKLANE (zenbook-ledger Z13), 2026-10-27.
#
#   QEMU gate:  ./arroyo test-zenbook 180 -> target/serial.log   (ZENBOOKLANETYPED Z16: 180 s, the default)
#               (= scripts/image-line-zenbook.env minus its [media] row, every inherited UNAOS_* unset first, plus
#                UNAOS_PLATFORM=zenbook_um5606w UNAOS_CENSUS=1 UNAOS_USBSERIAL=1, on a fresh card; tests-at-boot OFF)
#               ./arroyo mbench --replay target/serial.log --spec scripts/specs/x86-zenbook.spec --platform x86
#
# RUN-BY: verb:test-zenbook — `arroyo` names this file in code as `X86_ZENBOOK_SPEC`; `test_zenbook` hands it to
#          `test_x86_64` as X86_TEST_SPEC (its COMPLETE ends the wall, its REQUIREs are scored by `x86_test_completion`)
#          and `x86_pick_capture_spec` replays it (the ZENBOOKLANE arm, asked before the SELFHOST skip).
#          ZENBOOKLANETYPED (zenbook-ledger Z16): `zenbook_lane_typed` arms ONE typist (scripts/typist.sh, twoboot's) on the
#          lane's QMP port: the first-user form, then `tests platform|ec|nvme|i8042|dwi2c|hdacensus|gopshot|eclid|bootlist`
#          (ZENBOOKFLIGHTLIST Z33 added hdacensus, gopshot, eclid), one line each (`tests`
#          runs ONE fixture per line), each held on the verdict before it; transcript target/zenbook-typist.log (beside).
#
# WHY. The Zenbook-only kernel arms answer `hw::platform()`, and q35's SMBIOS resolves `qemu`, so no QEMU leg reached them
# (GOPIGNITE-FIXTURE, Z10). `UNAOS_PLATFORM` (hw.rs `platform_knob`, witness builds only) writes the platform's one
# resolution before `platform_say` prints it; the DMI strings stay QEMU's own. NOTHING here is the metal (ZENBOOK-FIRSTBOOT).
#
# MEASURED AT THE CUT (ae8272e02), and why some expected lines are NOT pinned:
#   * `[hw] skip smc|gmux` — ZERO lines: the SMC (feature smc) and the gmux (gmux_igd + intel-ivb) are not on the Zenbook
#     line, so no gate call is linked. ZENBOOKACP (zenbook-ledger Z25) put `hda` on the line: `[hw] skip hda` now prints
#     and the gate's non-Apple arm runs the read-only codec census (section 10).
#   * `[mt7925] s2` — SILENT: with no 14c3 device on the bus `mt7925::s2` returns before its platform test.
#   * the typed fixtures register only from the `tests` verb, which needs a session on this `login` line: section 7 below
#     (ZENBOOKLANETYPED, Z16, measured at d7ad4440e).
#   * ZENBOOKLANELIST (zenbook-ledger Z19): the builder re-creates target/x86_64_esp every run (builder/src/main.rs:1252),
#     so `test_zenbook` stages the LANE list (unaos/fixtures/lanes/zenbook/TESTS.LIST: `platform @zenbook`, `gen7 @rmbp12`)
#     through the ONE stager `bootlist_stage` and the builder carries it onto the ESP (UNAOS_BOOTLIST_STAGE,
#     `stage_bootlist_carry`); the typist's first typed verb is held on the list's light summary. Section 8 pins the list.
#   * the pixel witness (GOPIGNITEPIXEL, zenbook-ledger Z28): gop_ignite paints five named squares and reads them back before
#     activate_on (`[gop] shot`, section 4); no QMP screendump is scored (GOPSHOT-QMP owed: the typist holds the one socket).

# ── 1. THE OVERRIDE: the platform's one resolution says the Zenbook, by the knob ──────────────────────────────────────
REQUIRE \[hw\] platform=zenbook_um5606w dmi=".*" vendor=".*" src=knob

# ── 2. Z8: the EC census runs on a non-Apple platform; q35 carries no ECDT, so it stops before any port I/O ───────────
REQUIRE \[ec\] -> SKIP reason=no-ecdt
OPTIONAL :: ECLID: -> SKIP reason=no-ecdt ::

# ── 3. Z2: the NVMe controller (UNAOS_NVME on the line; the builder attaches the QEMU nvme device) ─────────────────────
REQUIRE \[nvme\] ctrl mn=".*" ns=1 lba=\d+ blocks=\d+ -> ready
# NVME4KN-FIXTURE (zenbook-ledger Z40): the 4Kn root lane — `UNAOS_UNAFS=1 UNAOS_NVME_4K=1 ./arroyo esp-x86`, then `UNAOS_NVME_4K=1 UNAOS_NVME_DISK=target/unaos-x86-card.img ./arroyo test-zenbook`: the root flip a one-LBA read-modify-write
OPTIONAL \[nvme\] test identify=ok sector0=GPT .* rw=ok .* lba=4096 shift=12 rmw=[1-9]\d*
# NVMEMSIX (zenbook-ledger Z45): q35's nvme has MSI-X in BAR0 — the I/O CQ's completion vector fired during the rw leg
OPTIONAL \[nvme\] test identify=ok .* irqs=[1-9]\d*

# ── 4. Z10: the compositor ignites on the GOP framebuffer (no Kepler takeover) — the GopBoot arm's first QEMU run ──────
REQUIRE \[wc-x\] gop-ignite platform=zenbook_um5606w fb=\d+x\d+
REQUIRE \[gop\] shot centre=ff8000 tl=ff0000 tr=00ff00 bl=0000ff br=ffffff -> PASS
REQUIRE :: GOPSHOT: ignite=pass centre=ff8000 tl=ff0000 tr=00ff00 bl=0000ff br=ffffff -> PASS ::

# ── 5. Z7: the xHCI census (one controller on q35) ─────────────────────────────────────────────────────────────────────
REQUIRE \[xhci\] controllers=\d+ picked=\d+ bdf=\d+:\d+\.\d+

# ── 6. Z3: the 8042 keyboard controller (q35 has one; only the MacBookPro10,1 skips it) ───────────────────────────────
REQUIRE \[i8042\] keyboard id=[0-9a-f]{4} set=1xlat irq=(routed|polled) -> ready

# ── 7. ZENBOOKLANETYPED (Z16): the first-user form (BP-boot-7), typed; the session's desktop; the board by platform ─────
REQUIRE \[login\] installer: first-user form open
REQUIRE \[login\] installer: create-user user=una\b
REQUIRE \[desktop\] built at=
REQUIRE :: OLDEST: board=zenbook year=2024 image=\S+ net=(absent|present) -> BOOTS ::

# ── 8. ZENBOOKLANELIST (Z19): the lane list read as the Zenbook: the @zenbook line runs, the @rmbp12 line is counted ──
REQUIRE \[bootlist\] list src=/boot/TESTS\.LIST bytes=\d+ light=1 heavy=0 heavy_mode=after boot=\S+ cpu=\d+ board=zenbook other=1\b
REQUIRE :: BOOTLIST: ran=1 pass=1 fail=0 skip=0 heavy=none ms=\d+ ::
FORBID \[bootlist\] list=absent
FORBID \[bootlist\] list src=.* board=(rmbp12|pi4|orin)\b

# ── 9. the six typed verdicts, one `tests <verb>` line each, in the typist's order ───────────────────────────────────
REQUIRE :: PLATFORM: platform=zenbook_um5606w src=knob kat=\d+/\d+ gate=smc:skip,gmux:skip,hda:skip -> PASS ::
REQUIRE :: EC: proto=ok cases=5/5 -> PASS ::
REQUIRE :: NVME: ctrl=ok ns=1 rw=(ok|in-root) -> PASS :: root=(ok|skip) ::
REQUIRE :: I8042: init=ready irq=(routed|polled) transport=ps2 table=ok .* -> PASS ::
REQUIRE :: DWI2C: proto=ok cases=8/8 -> PASS ::
REQUIRE :: BOOTLIST: probe entries=\d+ heavy=\d+ budget_skips=\d+ summary=ok -> PASS ::
FORBID :: (PLATFORM|EC|NVME|I8042|DWI2C|HDACENSUS|GOPSHOT|ECLID|BOOTLIST): .*-> FAIL

# ── 10. ZENBOOKACP (Z25): the HDA census on q35's intel-hda + hda-duplex (UNAOS_HDA), the drive path skipped ───────────
REQUIRE \[hw\] skip hda platform=zenbook_um5606w
REQUIRE \[hda\] census codec=0 vid=1af4:0022 subsys=[0-9a-f]{4}:[0-9a-f]{4} afg=0x[0-9a-f]{2} nodes=\d+ ctrl=[0-9a-f]{4}:[0-9a-f]{4} pcissid=[0-9a-f]{4}:[0-9a-f]{4} -> parked
REQUIRE \[hda\] audit stage=census .*verbs-set=0\b
REQUIRE :: HDACENSUS: codecs=1 vid=10ec:0294 subsys=1043:1df3 afg=0x01 nodes=\d+ sets=0 cases=4/4 -> PASS ::
FORBID \[hda\] (gpio|tone|path)\b

# ── END OF RUN: the last typed verdict (mbench's completion also needs every REQUIRE above) ───────────────────────────
COMPLETE :: BOOTLIST: probe .* -> PASS ::
