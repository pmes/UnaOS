# x86-ahci.spec — the x86 boot when the BOOT VOLUME IS A SATA DISK, and the installer is armed.
#   QEMU gate:  UNAOS_WC=1 UNAOS_AHCI=1 UNAOS_INSTALLDEMO=1 UNAOS_QEMU_FULL=1 ./arroyo test 120
#               → unaos/target/serial.log, replayed by `x86_spec_replay` (arroyo names this file in
#               code as X86_AHCI_SPEC, so GATE-SPECROOTS resolves it GATED and it needs no RUN-BY).
#
# WHY A THIRD SPEC AND NOT THREE MORE LINES IN x86-test.spec (SPECROWS, rmbp seat, 2026-09-16).
# Until this file, `x86_pick_capture_spec` had exactly two answers — x86-test.spec for a default
# `./arroyo test` and x86-fat.spec for `UNAOS_FATIMG=sf` — so the leg above, which is where FIVE
# landed witnesses live, replayed x86-test.spec: ONE end-of-run marker and nothing else. SPECRUN's
# own STOP 2 said so. It could not be fixed by appending to that file, because this leg is a
# DIFFERENT MACHINE, not a longer boot:
#
#   * `UNAOS_AHCI=1` puts the boot ESP on a SATA port, so `/` is `ahci<port>:/kernel.elf` and the
#     bootinfo serial AGREES with the bound volume — the opposite of the USB legs, where the
#     firmware's ESP is not the volume the walk binds and `agrees=no` is the correct answer.
#   * `UNAOS_INSTALLDEMO=1` REPLACES the usb-storage backing with a fresh BLANK 128 MiB scratch
#     (`builder/src/main.rs:1093`, overriding UNAOS_FATIMG), so the default medium's MBR — which
#     x86-test.spec now pins — is NOT on this leg's wire at all. Pinning both files' facts in one
#     file would make every pin false on some leg, which is how a spec stops being read.
#
# SO THE SPLIT IS THE FIXTURE, and `x86_pick_capture_spec` states it in code rather than in prose:
# this file is named ONLY when AHCI, INSTALLDEMO and WC are all armed and no medium knob has
# replaced the disk. Any other AHCI combination gets NO SPEC and says why — the same "a gate nobody
# has watched go green is the defect" rule the picker already applied to the part/gpt/p16 layouts.
#
# NO `COMPLETE` MARKER HERE, and that is deliberate. `x86_test_completion` reads X86_TEST_SPEC —
# x86-test.spec — on EVERY x86 leg including this one, so end-of-run is already answered by the
# zeolite marker that was measured for it, and `x86_spec_replay` refuses to score any capture that
# marker calls short. A second marker here could only disagree with the first.
#
# ── WHAT IS PINNED, AND THE CAPTURE EACH SHAPE WAS MEASURED ON ──────────────────────────────────
# SELFGUARD-AHCI (`50aa543f`). The defect it fixed is worth restating because the FORBID below IS
# that defect: `selfguard::live()` built its candidate set from USB + the generic block registry
# only, so a SATA identity had no candidate to match, fell to the "do not invent a verdict" arm and
# came back **Eligible** — i.e. the installer was told the disk the OS is running from is a
# legitimate target. On the bench rMBP that disk is the one carrying Catalina. The REQUIRE asserts
# the boot disk is refused BY NAME and the FORBID convicts the old answer; a `-> FAIL` forbid could
# not have caught this, because the old behaviour was not a failure, it was a wrong PASS.
# Measured: `selfguard-logs/serial-after.log` (the SELFGUARD fold gate, this exact command).
REQUIRE :: SELFGUARD: census disks=[0-9]+ usb=[0-9]+ sata=[0-9]+ boot=ahci[0-9]+/slot[0-9]+ ::
REQUIRE :: SELFGUARD: classify ahci[0-9]+/slot[0-9]+ \([0-9]+ sectors, [0-9]+ FAT volume\(s\)\) -> INSTALL-SELF reason=carries-the-boot-volume-serial ::
FORBID :: SELFGUARD: classify ahci[0-9]+/slot[0-9]+ .* -> Eligible
# COUNT and not REQUIRE, and >=1 and not =2: the candidate set is the machine's disk count, which is
# 2 on this fixture and is not a fact about the guard. What IS a fact about the guard is that it
# classified SOMETHING — an empty census is `live()` regressing to the enumeration hole above, and
# it would leave both lines above satisfiable only by accident.
COUNT 1 :: SELFGUARD: classify
#
# X86BIND on its SATA leg. `agrees=yes` is fixed HERE and matched loosely in x86-fat.spec, because
# here it is the whole point: the volume the root walk bound IS the one the firmware booted, so the
# two independent answers can be compared instead of merely both existing. `root=global:` is the
# regression spelling — the walk falling back to the USB scratch — and it is forbidden by name;
# `root=-` is the no-volume answer (`reason=kernel-not-found-on-any-volume -> FAIL`).
# Measured: `selfguard-logs/serial-after.log` and `x86bind-logs/ahci2-serial.log`, two runs, same
# shape, `root=ahci5:/kernel.elf … agrees=yes mounts=4 layout=true`.
REQUIRE :: X86BIND: root=ahci[0-9]+:/kernel\.elf serial=0x[0-9a-f]+ by=[a-z]+ bootinfo=0x[0-9a-f]+ agrees=yes mounts=[0-9]+ layout=true -> PASS ::
FORBID :: X86BIND: root=global:
FORBID :: X86BIND: root=-
# And the registry's own refusals, which are silence-shaped on the wire above: a port that never
# registers produces no X86BIND line about itself, so the REQUIRE alone would go short without
# saying why. These two say why (`drivers/block.rs:2736`, `:2740`).
FORBID :: AHCI: REFUSED to register port=
#
# QUARRYDOCK (`85067fd5`) — the taskbar states its own pin set. This pin lives HERE and not in
# x86-test.spec for one measured reason: the census is emitted by the compositor's dock, so it is
# ABSENT from every boot without `UNAOS_WC`, and x86-test.spec is replayed by the knob-free default
# `./arroyo test`. That is the APPPIN trap x86-test.spec's own header names. This file's leg carries
# UNAOS_WC by construction (the picker refuses to name this file otherwise), so the pin is honest
# here and would have been a false red there.
# The FORBID is QUARRYDOCK's own finding, not an invented negation: `quarry=no` WITH
# `quarry_compiled=yes` is the second of the two defects behind "there is no Quarry" — the image
# carries the app and the dock did not pin it — and it is precisely the case the old `[dock] census`
# lines could not distinguish from the first. Both fields are matched loosely so the armed leg
# (`UNAOS_QUARRY=1`: `pins=3 quarry=yes … quarry_compiled=yes`) and the unarmed one (`pins=2
# quarry=no … quarry_compiled=no`) both pass; only the INCONSISTENT pair is forbidden.
# Measured: `quarrydock-logs/serial-unarmed.log` and `…/serial-armed.log`.
REQUIRE \[dock\] pins=[0-9]+ quarry=(yes|no) console=(yes|no) shell=(yes|no) pulse=(yes|no) tiles=[0-9]+ quarry_compiled=(yes|no) ::
FORBID \[dock\] pins=[0-9]+ quarry=no .* quarry_compiled=yes ::
#
# STORWAIT / STORSLOT, the same two shapes x86-fat.spec pins — the same pin in two specs, which is
# the ONE duplication this tree's spec contract permits, because it is one kernel line pinned by two
# legs rather than one witness taught two spellings. `handles=global=absent` is the measured answer
# on THIS leg (the installdemo scratch carries no FAT volume) where the sf leg reads `present`, so
# the field is matched, not fixed.
REQUIRE :: \[fatverb\] storage settle: waited=[0-9]+ms settled=found handles=global=(present|absent) sdhc=(present|absent|unbuilt) ::
REQUIRE :: USBREG: publish slot=[0-9]+ lun=[0-9]+ ix=[0-9]+ block_size=[0-9]+ num_blocks=[0-9]+ disks=[1-9][0-9]* ::
REQUIRE :: STORSLOT: claim slot=[0-9]+ ix=[0-9]+ devices=[0-9]+ — mass-storage record taken; SCSI bring-up deferred to the main loop ::
FORBID :: \[fatverb\] storage settle: .* settled=ceiling
FORBID :: STORSLOT: storage records FULL

# ── CONTRACT (SPECRUN, 2026-09-15; this file joins it at birth) ─────────────────────────────────
# A PINNED LINE IN THIS FILE IS CHANGED TOGETHER WITH THE KERNEL LINE IT PINS, IN THE SAME COMMIT —
# re-pinned to the new wording (naming the arc that changed it), or dropped with the reason stated.
# It is never worked around by teaching the kernel a SECOND spelling of the same witness.
#
# TAIL-APPEND ONLY, for the reason x86-fat.spec's copy of this block gives: pins get cited
# POSITIONALLY (`x86-fat.spec:156`) and a header insert moves every citation by the same amount,
# silently.
#
# GATE-SPECROOTS (`scripts/spec-roots.sh`, a leg of `./arroyo check`) reds by name on any spec under
# scripts/specs/ that is neither named in `arroyo`'s CODE nor carries a RUN-BY line. This file is
# named in code (X86_AHCI_SPEC, and again in `x86_pick_capture_spec`'s case), so it resolves GATED.
# If a future change unwires the picker's AHCI arm, this file becomes an ORPHAN and `check` says so
# by name — which is the whole point of that gate: the spec cannot quietly stop being run.
