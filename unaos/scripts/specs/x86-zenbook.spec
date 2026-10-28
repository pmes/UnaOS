# x86-zenbook.spec — THE ZENBOOK IMAGE LINE, BOOTED UNDER QEMU AS THE ZENBOOK. ZENBOOKLANE (zenbook-ledger Z13), 2026-10-27.
#
#   QEMU gate:  ./arroyo test-zenbook 200 -> target/serial.log   (ZENBOOKLANETYPED Z16: 180 s; ZENBOOKSPECPINS Z54: 200 s, the default)
#               (= scripts/image-line-zenbook.env minus its [media] row, every inherited UNAOS_* unset first, plus
#                UNAOS_PLATFORM=zenbook_um5606w UNAOS_CENSUS=1 UNAOS_USBSERIAL=1, on a fresh card; tests-at-boot OFF)
#               ./arroyo mbench --replay target/serial.log --spec scripts/specs/x86-zenbook.spec --platform x86
#
# RUN-BY: verb:test-zenbook — `arroyo` names this file in code as `X86_ZENBOOK_SPEC`; `test_zenbook` hands it to
#          `test_x86_64` as X86_TEST_SPEC (its COMPLETE ends the wall, its REQUIREs are scored by `x86_test_completion`)
#          and `x86_pick_capture_spec` replays it (the ZENBOOKLANE arm, asked before the SELFHOST skip).
#          ZENBOOKLANETYPED (zenbook-ledger Z16): `zenbook_lane_typed` arms ONE typist (scripts/typist.sh, twoboot's) on the
#          lane's QMP port: the first-user form, then `tests platform|ec|nvme|i8042|dwi2c|hdacensus|gopshot|eclid|suspend|ecqnames|dwi2cacpi|wpa2|wifileave|unafs|bootlist`
#          (ZENBOOKFLIGHTLIST Z33 added hdacensus, gopshot, eclid; ZENBOOKSPECPINS Z54 suspend, ecqnames, dwi2cacpi), one line each (`tests`
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
# NVME4KN-FIXTURE (zenbook-ledger Z40): the 4Kn root lane — UNAFSLANEQEMU (Z102): the lane lays its own card over its own ESP, so it is `UNAOS_NVME_4K=1 ./arroyo test-zenbook` (an esp-x86 card is another build's kernel): the root flip a one-LBA read-modify-write
OPTIONAL \[nvme\] test identify=ok sector0=GPT .* rw=ok .* lba=4096 shift=12 rmw=[1-9]\d*
# NVMEMSIX (zenbook-ledger Z45): q35's nvme has MSI-X in BAR0 — the I/O CQ's completion vector fired during the rw leg
OPTIONAL \[nvme\] test identify=ok .* irqs=[1-9]\d*
# NVMEIRQWAIT (zenbook-ledger Z49): the rw leg's I/O waits slept on the vector and it woke them (the shell on the vector's CPU, unmasked); a vector that never wakes is the off-latch, never on QEMU
OPTIONAL \[nvme\] test identify=ok .* waits=[1-9]\d* wakes=[1-9]\d*
FORBID \[nvme\] irq wait off

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
# ZENBOOKSPECPINS (zenbook-ledger Z54): the three verbs typed after eclid; SUSPEND has no FAIL arm (its SKIP is the no-FADT arm, never on q35); dwi2cacpi prints three verdicts
REQUIRE :: SUSPEND: .* rung=none .* -> PASS ::
FORBID :: SUSPEND: -> SKIP reason=no-fadt ::
REQUIRE :: ECQNAMES: n=3 notify=LID0:0x80 fields=ok scope=ok refuse=3/3 -> PASS ::
OPTIONAL \[ec\] qnames -> REFUSED why=no-ec
REQUIRE :: DWI2CACPI: base=ok tp=ok speed=400000 desc=ok refuse=3/3 -> PASS ::
REQUIRE :: DWI2CGPIO: .* refuse=2/2 -> PASS ::
REQUIRE :: DWI2CFAST: fmcn=120/256/30 sscn=650/669/30 .* refuse=3/3 -> PASS ::
FORBID :: (SUSPEND|ECQNAMES|DWI2CACPI|DWI2CGPIO|DWI2CFAST): .*-> FAIL
FORBID :: (PLATFORM|EC|NVME|I8042|DWI2C|HDACENSUS|GOPSHOT|ECLID|BOOTLIST): .*-> FAIL
# MT7925S10 (zenbook-ledger Z55): `tests wpa2`, the WPA2-PSK known-answer legs (PBKDF2 Annex J.4, PRF Annex J.3 + a pinned PTK, the EAPOL-Key MIC, RFC 3394 unwrap, the M1->M2 / M3->M4 frames, the key-command bodies; S10REKEY Z63: rekey=, the group-key handshake); pure, so QEMU runs it with no radio
REQUIRE :: WPA2KAT: pbkdf2=ok prf=ok mic=ok unwrap=ok frame=ok keys=ok rekey=ok -> PASS ::
FORBID :: WPA2KAT: .*-> FAIL
# WIFILEAVE (zenbook-ledger Z68): `tests wifileave`, the operator-leave KAT (the kept plan dropped, nothing due after it, the PMK read back zero, the 26-octet deauthentication bytes); pure, so QEMU runs it with no radio (`radio=absent`); the typist line carries it in arroyo test-zenbook (the 4.5 s it costs against the 206 s budget is owed: ZENBOOKLANE-SECS)
REQUIRE :: WIFILEAVE: kat=ok .*-> PASS ::
FORBID :: WIFILEAVE: .*-> FAIL
# UNAFSLANEQEMU (zenbook-ledger Z102): the lane's NATIVE root — test_zenbook's zenbook_unafs_root lays the card over THIS lane's ESP (esp_x86 -> esp_x86_unafs_card) and the builder attaches it as the NVMe (UNAOS_NVME_DISK); the stick is INSTALLDEMO's blank scratch, so the NVMe is the one disk carrying this kernel. `tests unafs` typed before bootlist: the seam on the root and UNAFSBINDTRIGGER's bind_now KAT (B1061: its FAIL ends the verdict in FAIL); the skip arm is FORBIDden, so the lane cannot pass by skipping
REQUIRE :: UNAFSX86: root=unafs src=nvme blocks=\d+ gen=\d+ home=/home boot=\S+ -> PASS ::
REQUIRE \[unafs\] bind_now kat=ok r=ok bound=1 masked=0\b
REQUIRE :: UNAFSX86-T: root=unafs create=ok write=ok remount-read=ok owner=ok unlink=ok -> PASS ::
FORBID :: UNAFSX86-T: .*-> (SKIP|FAIL)

# ── 10. ZENBOOKACP (Z25): the HDA census on q35's intel-hda + hda-duplex (UNAOS_HDA), the drive path skipped ───────────
REQUIRE \[hw\] skip hda platform=zenbook_um5606w
REQUIRE \[hda\] census codec=0 vid=1af4:0022 subsys=[0-9a-f]{4}:[0-9a-f]{4} afg=0x[0-9a-f]{2} nodes=\d+ ctrl=[0-9a-f]{4}:[0-9a-f]{4} pcissid=[0-9a-f]{4}:[0-9a-f]{4} -> parked
REQUIRE \[hda\] audit stage=census .*verbs-set=0\b
REQUIRE :: HDACENSUS: codecs=1 vid=10ec:0294 subsys=1043:1df3 afg=0x01 nodes=\d+ sets=0 cases=4/4 -> PASS ::
REQUIRE :: HDAPATH: d0=ok path=ok sets=6 cases=3/3 -> PASS ::
# ZENBOOKSPECPINS (zenbook-ledger Z54): ALCPLAY-SPEC — ZENBOOKACPPLAY's (Z47) decision KAT, printed after HDAPATH by the same `tests hdacensus`
REQUIRE \[hda\] alcplay kat keep=ok arm=ok tone=ok play=ok held=ok -> ok
FORBID \[hda\] alcplay kat .*-> FAIL
# ZENBOOKACPDEPOP (zenbook-ledger Z60): the alc294_hp_init depop KAT (verb order, the 0x6f / 0x58 masks, the 20 x 50 ms poll bound, a silent door), printed after the alcplay kat by the same `tests hdacensus`
REQUIRE \[hda\] depop kat verbs=20 order=ok rmw=ok poll=ok nack=ok -> ok
FORBID \[hda\] (gpio|tone|path)\b

# ── END OF RUN: the last typed verdict (mbench's completion also needs every REQUIRE above) ───────────────────────────
COMPLETE :: BOOTLIST: probe .* -> PASS ::
