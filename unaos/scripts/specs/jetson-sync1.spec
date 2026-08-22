# jetson-sync1.spec — boot 1 of the synced base (hw-jetson post trunk-merge ceaa32b8).
#   Metal: bridge capture per BENCH-PROCESS (never a /dev path; log-file replay/follow only).
#   The ONE question this boot answers: does the merged trunk (a month of desktop/
#   userspace/net work + the scheduler reconciliation) still bring the Orin up through
#   the full JD/JB chain on real silicon?
#
# Derived from jetson-jd5.spec's single-boot shape (that spec's power-cycle second half
# does not apply to boot 1). Serial-scope caveat carried over: panel verdicts render to
# the PANEL; serial carries the tegra `::` witnesses and keystroke echoes only.
#
# Expected noise, deliberately NOT forbidden: `xHCI: >>> COMMAND FAILED (Code 11) <<<`
# (hub-MSC intermittency, graceful fallthrough — unaos-jetson-resume).

# --- boot bring-up witnesses (the JD/JB chain — all previously metal-proven) --------
REQUIRE JD1.*scanout:.*sane=true
REQUIRE JD1.*panel LIVE
REQUIRE JB1b.*MRQ_PING.*-> PASS
REQUIRE JB0.*fan ON.*-> PASS
REQUIRE JB1c.*XUSB ALIVE.*-> PASS
REQUIRE JB2b.*keyboard ARMED.*-> PASS
REQUIRE JD3.*mass storage ready
REQUIRE JD2.*console pump live
REQUIRE JD4.*console OWNS the panel

# --- scheduler: the post-merge trunk scheduler on Orin metal ------------------------
REQUIRE CAPSTONE COMPLETE

# --- NEW this boot: witnesses shipped ahead of their bench (promote on capture) -----
# M1b: the first EL0 round-trip on Orin metal (tegra_el0 knob armed on the image).
REQUIRE TEGRA-EL0.*el0-hello round-trip -> PASS
# M2 step 1: the microSD becomes block-layer-visible (read-only backend).
# PROMOTED PENDING -> REQUIRE (orin 3, 2026-08-22) on capture, per the standing rule that
# nothing is promoted without one. Evidence: `capture/orin2-boot5c-gui.log:1051` carries it
# verbatim — `:: TEGRA-SD: block backend published - 62333952 sectors (read-only) ::` — and
# the board records it published on BOTH boot5c flights, so it is repeatable and not a
# single-trial call. It was THE BLOCKER for six boots; a silent regression here takes the
# installer, the native volume and `uls` with it, and every one of those reds would be
# misattributed downstream. The publish is also strictly upstream of ORIN-INSTALL-2, which
# takes its target from the same census.
REQUIRE TEGRA-SD.*block backend published

# --- IRQEL-RT2: the EL1 one-shot proof adjudicates THREE ways; do not flatten it -----
# `timer::el1_oneshot_proof` (tegra-gated, and UNCONDITIONAL on a tegra image — main.rs
# calls it right after the post-drop `exceptions::install`) arms ONE CNTP tick inside a
# ~100 ms IRQ-unmask window ON THE ARMING CORE ONLY. IRQEL-CORE: the window is keyed to
# `EL1_PROOF_CORE` = that core's `cpu_index`. It used to be a machine-global AtomicBool,
# and on a 6-core Orin the five ORIN-SMP-3 APs each arm their own 250 Hz PPI 30 and stay
# at EL2, so an AP consumed the window with probability ~1 — that, and not a routing bug,
# is what boot 5c's "taken at EL2" verdict actually reported. Exactly ONE of the three
# lines below prints per boot:
#
#   PASS  `first IRQ taken at EL1 on cpu <n>`             the banked EL1 vector path is live
#   FAIL  `taken at EL<x> on cpu <n> (the ARMING core)`   the ARMING core's own IRQ went up
#   MISS  `proof INCONCLUSIVE`                            nothing arrived; not a verdict
#
# THE KINDS ENCODE THAT AND NOTHING MORE. PASS is PENDING — code ahead of its bench, never
# yet captured (boot 4f: "IRQEL-RT EL1 live-arm: still unproven on metal") — so it reads ⏳
# until a capture carries it and mbench then advises the promotion. FAIL and MISS are
# OPTIONAL: NOT PENDING, because a matched PENDING advises "consider promoting to REQUIRE",
# which is exactly wrong for a fault path (x86-witness.spec's `[wc-d]` teardown block makes
# the identical argument — nobody should ever REQUIRE a failure); and NOT FORBID, because
# an honest negative result must not red the flight. The proof's ability to FAIL without
# being suppressed is its most valuable property, and a REQUIRE on the PASS line would
# convert the instrument into a rubber stamp.
#
# THE HOLE, STATED RATHER THAN GLOSSED: since none of the three fails the run, a boot where
# the proof ARMS and no verdict prints scores clean. mbench has no conditional REQUIRE
# (`WHEN <guard> REQUIRE <rx>` — the grammar hole x86-witness.spec already writes up), so
# the arm line is the guard a reader checks BY EYE: arm line present + all three verdicts ◦
# = the window was entered and never closed. Patterns are kept ASCII on purpose: these lines
# carry em-dashes, and a DARKWIN-dropped byte mid-sequence would lossy-replace them.
# CAPTURE STATUS, measured against the record rather than assumed: the FAIL branch is the
# ONLY one metal has ever printed (boot5c, `capture/line-acm0/orin.log:8311`, in its
# PRE-IRQEL-RT2 wording `taken at EL2 — NOT the EL1 proof (investigate)`); `first IRQ taken
# at EL1`, `proof INCONCLUSIVE` and `[irqel2a]` have ZERO hits anywhere in the bench tree.
# The FAIL pattern below deliberately matches the NEW wording only, so replaying boot5c
# against this spec reads it as ◦ — that is correct, not a miss: boot5c's verdict was the
# machine-global-flag artifact IRQEL-RT2 removed, and a spec adjudicates the NEXT flight.
PENDING IRQEL-RT: first IRQ taken at EL1 on cpu [0-9]+
OPTIONAL IRQEL-RT: one-shot proof IRQ taken at EL[0-9]+ on cpu [0-9]+ \(the ARMING core\)
OPTIONAL IRQEL-RT: EL1 one-shot NOT delivered in ~100 ms
# The arm line is REQUIRE, and it is the one promotion this block makes. Justification, in
# full, because a REQUIRE that cannot match is the defect this file guards against:
#   (1) CAPTURED — boot5c `orin.log:8310` carries it verbatim; the pattern is the prefix the
#       IRQEL-RT2 rewording left untouched, so it matches the old and the new text alike.
#   (2) UNCONDITIONAL — `el1_oneshot_proof` is `#[cfg(feature = "tegra")]` only, with no
#       runtime knob, and main.rs:2581 calls it on the SAME statement as, and strictly
#       BEFORE, `run_capstone_boot_core(0)`. So every boot that satisfies the
#       `REQUIRE CAPSTONE COMPLETE` above must already have printed this line — the
#       promotion adds NO new way for a healthy boot to red.
#   (3) IT BUYS COVERAGE — if the emitter is deleted or the proof stops being reached while
#       CAPSTONE still prints, a PENDING would silently drop to ⏳ and mbench would still
#       exit 0. This REQUIRE is what makes that loud.
REQUIRE IRQEL-RT: EL1 one-shot proof
# The adjudicating latch. `[irqel2a]` carries HCR_EL2/CNTHCTL_EL2/ICC_SRE_* read back from
# the JM6 drop's RAM latch (an `mrs` of an EL2 register from EL1 is UNDEFINED, so they
# cannot be read live at EL1); IMO=0 is the bit that makes an IRQ taken at EL1 target EL1.
# `[irqel2b]` SKIPPED means the latch said ICC_SRE_EL1.SRE=0, i.e. the GIC view was NOT read
# because any ICC_*_EL1 access at EL1 would itself have been UNDEFINED. Both are new this
# arc and uncaptured, but they are DIAGNOSTICS attached to the verdict above rather than
# verdicts, so OPTIONAL: nothing should ever be required to print a state dump.
OPTIONAL \[irqel2a\] \S+ cpu=[0-9]+ CurrentEL=[0-9]+
OPTIONAL \[irqel2b\] \S+ SKIPPED

# --- SMPMARK: knob-gated, so OPTIONAL — and the kind IS the decision -----------------
# The three marks are `#[cfg(feature = "smpmark")]`, armed by `UNAOS_SMPMARK=1`. A REQUIRE
# would go red on every UNARMED boot — a directive that fails on CONFIGURATION rather than
# on health. PENDING is wrong for the same reason: promoting a knob-gated line reds every
# default boot. x86-witness.spec's PCI-CENSUS / bcma block is the in-tree precedent and
# argues it in full. So OPTIONAL — visible in the table, never a gate. THE COST IS REAL AND
# IS STATED: an OPTIONAL proves NOTHING; on an unarmed image all three read ◦ and mbench
# still exits 0. PRESENCE is what these carry, never absence.
# The marks are emitted with `serial_print!` (NO newline — they append to the neighbouring
# line by construction, which is what keeps the disarmed image byte-identical), so they are
# matched as substrings anywhere in a line, not as lines of their own.
#
# READING A PARKED CAPTURE (full mechanism: arch_arm64.md §ORIN-SMP-3-PARK; smp_virt.rs tail):
#   `ORIN-SMP-3 enumerated core 5` then RAS  the BSP publication block died; no CPU_ON was
#                                           ever issued. REFUTES the Device-fetch hypothesis.
#   `:P:` then RAS                          publication survived; the FIRST CPU_ON SMC did
#                                           not return. Fault in PSCI/ATF on the BSP. REFUTES.
#   `:P::R1:` then RAS, with NO `:A:`       the SMC returned and the AP never crossed
#                                           `enable_mmu_virt`. CONVICTS the MMU-off
#                                           Device-nGnRnE window, ~40 instructions of text.
#   `:P::R1::A:` then RAS                   the AP survived the Device window and died after.
#                                           Hypothesis REFUTED — look at `exceptions::install`,
#                                           `percpu::init`, `gic::init_secondary_v3`.
# A clean flight reads `:P::R1::A::R2::A:…`. CAVEAT, load-bearing: the interleaving order of
# `:R<n>:` against an earlier core's `:A:` is a two-cores-one-UART race and carries NO
# meaning. Only PRESENCE and the LAST tag before a park are evidence.
# `^:P:` IS ANCHORED AND THE OTHER TWO ARE NOT — measured, not stylistic. Scanned over every
# jetson capture in the bench tree, a bare `:P:` takes exactly one FALSE hit:
#   jetson-serial-2026-07-15-smp3bench.log:2709
#   `:: KERNEL HEAP ALLOCATED :P: released cores 1-3 via spin-table (0xE0/0xE8/0xF0) ::`
# An unarmed boot showing ✅ beside `:P:` would corrupt the very first row of the reading
# table below, so it is anchored. The mark can only ever START a line — the statement before
# it is the enumeration `serial_println!`, which ends in a newline — so anchoring costs no
# real hit. `:R[0-9]+:` and `:A:` take ZERO false hits across the same corpus and are left
# unanchored, which they must be: an AP's `:A:` races the BSP's output for the UART lock and
# can legitimately land mid-line.
OPTIONAL ^:P:
OPTIONAL :R[0-9]+:
OPTIONAL :A:

# --- regressions that would convict the merge, not the hardware ---------------------
FORBID PANIC
FORBID Serror
FORBID X200 FLAG
# The ORIN-SMP-3 park, NAMED rather than inferred. Without it a parked flight reds only as a
# pile of missing REQUIREs and the operator diagnoses the cause by eye; with it the table
# says WHICH fault fired, and the SMPMARK reading table above says which side of the PSCI
# wake it was on. FORBID and not REQUIRE-shaped anything: it is a fault signature.
# Byte-identical across all four metal instances (2026-07-15 smp3bench.log:1267; 2026-07-17
# r21b-sitting.log:1284; boot5b capture/line-acm0/orin.log:6186; boot5c flight #1
# orin.log:6780) — Status/SERR/IERR and both ADDR fields identical every time, only
# MISC0/MISC1 (timestamp-like) vary. `syndrome=0x82000010` decodes as EC=0x20 (INSTRUCTION
# abort from a LOWER EL), IL=1, IFSC=0x10 (synchronous external abort, NOT on a
# translation-table walk) — an instruction-FETCH external abort.
# DELIBERATELY NARROW, and this is the point: the `bg`-verb fault on this same board is a
# DIFFERENT RAS (Status 0xec000612, IERR SNOC Write Error 0xd, ADDR 0x8000000000000000) and
# carries NO `Exception reason=` line at all. Conflating the two has already cost this lane
# a wrong claim to a peer seat, so this keys on the syndrome and never on `RAS` or on an
# `ADDR` value. See arch_arm64.md §ORIN-RAS-ADDR for why an ADDR must never be the key.
FORBID Exception reason=1 syndrome=0x82000010
