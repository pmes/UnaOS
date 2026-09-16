# x86-usb2.spec — the TWO-STICK x86 leg: a second usb-storage on the same xHCI must be LISTED.
#
# RUN-BY: knobleg — UNAOS_USB2=sf UNAOS_WC=1 UNAOS_QEMU_FULL=1 ./arroyo test-fat sf 180 ; then
#          python3 scripts/mbench.py --replay target/serial.log --spec scripts/specs/x86-usb2.spec --platform x86
#
# WHY DECLARED AND NOT WIRED, stated first because the choice is the interesting part. The picker
# CAN see `UNAOS_USB2` — so the honest question is not "can it" but "what does wiring COST". This
# leg sets `UNAOS_FATIMG=sf`, so today `x86_pick_capture_spec` hands it **x86-fat.spec and its 32
# pins**. A spec is picked one at a time; wiring this file into the case would REPLACE that answer,
# trading 32 pinned lines for the five below. That is a strict loss of coverage on the leg with the
# most storage in it, bought for a wiring convenience — so the pins that hold with one stick OR two
# went into x86-fat.spec instead (SPECROWS, its tail block), where the gated leg scores them, and
# only the pins that REQUIRE a second stick live here, hand-run. GATE-SPECROOTS accepts this file on
# the `knobleg` declaration above and will red by name if the declaration ever rots.
#
# MINIMUM BUILD GENERATION — STORWAIT2 (`3caa48c7`, branch `exec-rmbp-storwait2` off `f8f8ce8c`).
# THIS FILE IS RED ON ANY OLDER IMAGE, BY DESIGN, AND THAT IS NOT A FLAW TO BE SOFTENED: it pins the
# fix, not the base. Replaying it against `f8f8ce8c` itself will come up short on the first three
# lines, because at `f8f8ce8c` the second stick IS NOT LISTED. Do not "fix" that by loosening a
# pattern; the shortfall is the correct answer for that image. (x86-fat.spec:146's FATVERB block
# carries the same idiom and the same warning.)
#
# THE DEFECT THESE FIVE LINES EXIST FOR, and why every one of them is a REQUIRE. STORSLOT
# (`265542b9`) made the driver bring up more than one usb-storage device, one record per main-loop
# pass, while enumeration DRAINS first (BOOTPACE M2, console-first). So on a two-stick machine the
# bus goes quiet and the one-shot `volid` census runs while the SECOND disk is still several passes
# away: it publishes, and it is never mounted under `/volumes/`. STORWAIT2's go-red capture
# (`~/unaos-bench/scratch/rmbp-0915/storwait2-logs/serial-gored.log`, the veto deleted) is the whole
# argument for the shape of this file —
#
#   green (serial-after.log, sha f8f8ce8c-d8ad19f)   mount /volumes/UNAOS · usb1=present · mounts=5
#   go-red (serial-gored.log, sha f8f8ce8c-dfc3b1b)  NO /volumes/UNAOS · NO usb1= line · mounts=4
#   and the go-red run's own exit code                                                      **rc=0**
#
# — a defect whose entire symptom is ABSENCE, riding a green verb. A FORBID cannot see it: there is
# no bad line to convict, only a good line missing. That is the case REQUIRE exists for, and it is
# why the FORBIDs at the tail here are the partners of the storage pins and not of these three.
#
# THE PREFIX IS PINNED WITH ITS `name=` FIELD, deliberately. `/volumes/UNAOS` is a PREFIX of
# `/volumes/UNAOS SDHC4`, the internal SD reader's mount, which is present on BOTH captures above —
# so `mount /volumes/UNAOS` alone would match the go-red and this file would pass the defect it was
# written for. Anchoring on ` name=UNAOS id=Some(<digits>)` separates them (a spec line's trailing
# whitespace is stripped at parse, so the trailing-space form cannot be written here at all).
REQUIRE :: volid: mount /volumes/UNAOS name=UNAOS id=Some\([0-9]+\) ::
# The resurvey's own census — the second half of the same fact, read from the VFS rather than from
# the mount table, so a mount that appeared without the disk behind it (or the reverse) is visible.
# `usb1=` is the second stick's slot in that census; the go-red capture carries no such line at all.
REQUIRE \[vfs\] resurvey n=[0-9]+ ms=[0-9]+ bound_on_pass=[0-9]+ disks=.*usb1=present
# `mounts=5` IS pinned as a number, against this file's own shape-not-numbers rule, and the
# exception is the point: 5 = root + /apps + /boot + the SD reader + THE SECOND STICK, and 4 is
# exactly what the go-red printed. The number is the fact here, not an incidental count. It does
# assume the SD reader is present, i.e. no `UNAOS_NOSDHCI` — which is why the RUN-BY line above
# names the command WITHOUT it (STORSLOT's own `UNAOS_NOSDHCI=1` control prints `mounts=3`).
REQUIRE :: X86BIND: root=global:/KERNEL\.ELF serial=0x[0-9a-f]+ by=[a-z]+ bootinfo=0x[0-9a-f]+ agrees=(yes|no) mounts=5 layout=true -> PASS ::
# And the two-stick facts x86-fat.spec deliberately matches loosely (`devices=[0-9]+`, `disks=[1-9]`)
# because that file must pass with ONE stick. Here the count IS the fixture, so it is fixed.
REQUIRE :: STORSLOT: claim slot=[0-9]+ ix=[0-9]+ devices=2 — mass-storage record taken; SCSI bring-up deferred to the main loop ::
REQUIRE :: USBREG: publish slot=[0-9]+ lun=[0-9]+ ix=[0-9]+ block_size=[0-9]+ num_blocks=[0-9]+ disks=2 ::
# The FORBID partners, both real emitted strings: STORSLOT's array-exhausted arm — which a
# two-stick leg is the only leg that can reach — and the settle deadline expiring.
FORBID :: STORSLOT: storage records FULL
FORBID :: \[fatverb\] storage settle: .* settled=ceiling

# ── CONTRACT (SPECRUN, 2026-09-15; this file joins it at birth) ─────────────────────────────────
# A PINNED LINE IN THIS FILE IS CHANGED TOGETHER WITH THE KERNEL LINE IT PINS, IN THE SAME COMMIT —
# re-pinned to the new wording (naming the arc that changed it), or dropped with the reason stated.
# It is never worked around by teaching the kernel a SECOND spelling of the same witness.
# TAIL-APPEND ONLY: pins get cited positionally, and a header insert moves every citation silently.
#
# NO `COMPLETE` MARKER, for the reason x86-fat.spec has none: this file describes a SEGMENT of a
# capture whose end-of-run marker was measured for x86-test.spec. A hand replay of this file cannot
# therefore tell a short capture from a regression on its own — check the capture settled first
# (`python3 scripts/qemu_await.py --settled --log target/serial.log --spec scripts/specs/x86-test.spec
# --label test`), which is exactly the order `./arroyo test` enforces for the gated specs.
