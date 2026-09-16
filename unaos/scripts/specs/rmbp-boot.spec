# rmbp-boot.spec — minimal 2012 rMBP metal boot sanity (USBDEBUG esp build).
#   Metal:  ~/rmbp-serial.log (rmbp-bench-connect.sh bridge capture; the FTDI
#           console is TX-only — capture and assert, never inject)
#   Validated against the REAL 2026-07-10 STOR-1 bench capture
#   (~/unaos-bench/rmbp-serial-2026-07-10-172056.log, a knob-ON build).
#
# Minimal by design: boot came up, the FTDI console is live, storage enumerated,
# and the U-chain ran to its tail. The full chain assertion is x86-fat.spec;
# the full round-6 witness list is round6-rmbp.spec.

REQUIRE U2.5-0: DR7 cleared
REQUIRE U2.5: FTDI console up
REQUIRE U2.5: FTDI TX mirror -> PASS
REQUIRE U1a: user exited ok
REQUIRE U3.5: ring-3 preemption.*-> PASS
REQUIRE xHCI: Disk
REQUIRE U11m2:.*-> PASS
REQUIRE U6gx: UnaFS owner/grants.*-> PASS

# knob-ON (irqstorage) lines — present on the 2026-07-10 capture, but a default
# build must also pass this minimal spec
OPTIONAL bx-blockreq: PASS
OPTIONAL S3: synchronous write-through.*-> PASS

# The storage demo verdict. Pattern lesson: the USBDEBUG boot banner quotes the
# words 'MISSION SUCCESS', so a bare "MISSION SUCCESS" regex false-positives on
# the banner — always anchor on the real line's ">>>" frame. Absent from the
# 2026-07-10 capture (demo path not taken on that boot), hence OPTIONAL here.
OPTIONAL xHCI: >>> MISSION SUCCESS

# forbidden: any CPL-0 fault diagnostic (defaults -> FAIL / FAIL :: / PANIC always on)
FORBID EXCEPTION:

# --- GR18 WXN-x86 / WXAUDIT: the kernel map's own protection, asserted on the minimal boot -------
# Minimal by design still means "the machine came up correctly", and since `a0a2d163`/`32724cb4`
# that includes the first NX bits this kernel puts in its own map. These eight belong in the
# MINIMAL spec — not only the fat/witness ones — because they are the only GR18 wires that need no
# knob at all: `wxn_pdpt_sweep` and `wx_audit_report` are called unconditionally from `arch::init`
# (arch/x86_64/mod.rs:46, :50), and `wxn_nxe_report` (smp.rs:108) fires from `start_aps` on every
# exit path, uniprocessor ones included. A default build, a USBDEBUG build and a witness build all
# print all four lines, so a silently-dead sweep is exactly the kind of regression this file's
# eight-line sanity list is for.
#
# x86-witness.spec IS THE MASTER COPY of these patterns and of the argument for each (the WXN
# terminals are structurally exclusive with the SWEPT REQUIRE and are here for DIAGNOSIS; the
# WXAUDIT REQUIRE pins field ORDER and stops before the optional ` TRUNCATED` token, which is why
# that token needs its own FORBID; WXAUDIT-NXE deliberately gets no FORBID because its FAIL arm
# ends `-> FAIL ::` and the defaults above already catch it; `-> LEAF NX-ONLY` is a correct FBWC
# arm that this pattern does NOT accept, and widening waits for a capture that shows it).
# EDITS MUST BE PAIRED with that file. The regexes are byte-identical there, in x86-fat.spec and
# in round6-rmbp.spec; a drift between them reads as the metal and QEMU gates disagreeing about
# the map, which is the one failure this duplication exists to prevent.
#
# SCOPE — MINIMUM BUILD GENERATION `32724cb4` (2026-08-06). This is a second scope axis on this
# file, and it invalidates the header's validation claim for these eight lines specifically: the
# 2026-07-10 STOR-1 capture predates the sweep by a month and reds all four REQUIREs because its
# kernel could not print them. Verified green instead against the 2026-08-06 metal captures
# bootW/bootX (`nx_set=1022`, `kern_WX=1535`, `cores=8 nxe=8 -> PASS`, `fb=0x90020000 lvl=2 ->
# LEAF BIT-IDENTICAL`), and against an x86 QEMU `./arroyo test` capture on the dev host
# (`~/unaos-bench/scratch/qemu-x86-gr18.log`), which is the knob-independence claim above made
# good on a second configuration. Date a capture with `git log` before reading any red here.
REQUIRE :: WXN-x86: ehdr=0x[0-9A-F]+ img=\[0x[0-9A-F]+,0x[0-9A-F]+\) .*pdpt_seen=[0-9]+ nx_set=[0-9]+ .*residue_leaves=[0-9]+ .*wp=[0-9]+ -> SWEPT ::
FORBID :: WXN-x86: .*-> VACUOUS ::
FORBID :: WXN-x86: .*-> REFUSED
REQUIRE :: WXAUDIT x86: leaves=[0-9]+ user=[0-9]+ user_WX=[0-9]+ kern_WX=[0-9]+ \([0-9]+ MiB\) tables=[0-9]+ nxe=[0-9]+ walk=[0-9]+kcyc l1=[0-9]+ l2=[0-9]+ l3=[0-9]+
FORBID :: WXAUDIT x86: .* TRUNCATED ::
REQUIRE :: WXAUDIT-NXE: cores=[0-9]+ nxe=[0-9]+ nxe_mask=0x[0-9A-F]+ wp=[0-9]+ wp_mask=0x[0-9A-F]+ -> PASS ::
REQUIRE :: WXN-FBWC: fb=0x[0-9A-F]+ lvl=[0-9]+ e=0x[0-9A-F]{16} pat=[0-9]+ pcd=[0-9]+ pwt=[0-9]+ w=[0-9]+ fx=[0-9]+ -> LEAF BIT-IDENTICAL ::
FORBID :: WXN-FBWC: .*-> SKIPPED ::

# ── CONTRACT (SPECRUN, 2026-09-15) ──────────────────────────────────────────────────────────────
# A PINNED LINE IN THIS FILE IS CHANGED TOGETHER WITH THE KERNEL LINE IT PINS, IN THE SAME COMMIT —
# re-pinned to the new wording (naming the arc that changed it), or dropped with the reason stated.
# It is never worked around by teaching the kernel a SECOND spelling of the same witness. That is
# what an unrun spec cost this tree once: STORWAIT added a second `storage settle:` line rather than
# edit the `[fatverb] storage witness` REQUIRE this file pins verbatim — a pin no command was
# reading, and still expensive.
#
# WHY THIS BLOCK IS AT THE TAIL AND NOT THE HEAD. Ledger rows, queue rows and kernel comments across
# this tree cite pinned lines POSITIONALLY (`x86-fat.spec:238`, `pi4-regression.spec:1549`,
# `jetson-sync1.spec:1839`, `crates/kernel/src/shell.rs:4933` -> `x86-fat.spec:156`). A header insert
# moves every one of them by the same amount, silently — rmbp-ledger PI5 names tail-append as this
# repo's safe form for exactly that reason. The contract is ENFORCED, not merely written: see below.
#
# WHO RUNS THIS FILE, and the gate that makes the answer mandatory:
# RUN-BY: bench — ./arroyo mbench --follow ~/rmbp-serial.log --spec scripts/specs/rmbp-boot.spec --platform x86 --timeout 300
#   No verb replays it: the capture is the rMBP FTDI bridge log, which no QEMU verb can produce.
#
# GATE-SPECROOTS (`scripts/spec-roots.sh`, a leg of `./arroyo check`) reds by name on any spec under
# scripts/specs/ that is neither named in `arroyo`'s CODE nor carries a RUN-BY line above — and a
# `RUN-BY: verb:` claim is cross-checked against `arroyo`, so this file cannot claim a runner it
# does not have. "A replay spec no gate command runs is a silent landmine."
