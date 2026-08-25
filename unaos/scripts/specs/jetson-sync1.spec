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
#
# 2026-08-25 (orin 4): this file now also adjudicates BOOT 7e — the desktop / click /
# display-probe flight staged at
# `~/unaos-bench/flash/orin/boot7e-desk-click-jd1dcmodel-20260825T1927Z-24284e5/`
# (SRC.SHA commit 24284e50, branch hw-jetson). Four instrument families that image
# carries and this spec had never heard of — `[orinwm1]`, `[orinclick]`, `JD1-DC`
# (both verdict axes) and the always-on `[redzone]` guards — now have rows, all of
# them OPTIONAL or PENDING. See the BOOT 7e banner below the EL0/IRQEL/SMPMARK blocks.
#
# 2026-08-25 (later, exec-spec): the three defects this file used to RECORD under a
# "KNOWN DEFECTS … DELIBERATELY NOT FIXED" heading are fixed. That heading is gone; each
# fix is argued where its rows live.
#   1. `FORBID Serror` could never fire (the kernel emits `SError` / `SERROR`, the match
#      is case-sensitive). Replaced by two rows keyed on the two FATAL emitters — see
#      the regression block at the foot for why a `(?i)` fix was rejected as WORSE.
#   2. Zero `COMPLETE` markers meant a cut capture read FAIL instead of TRUNCATED, on
#      the one platform whose wire is known-lossy. One marker added; see the COMPLETE
#      block above the regression block.
#   3. The `[spin6]` unabsorbed-redzone task drop had no row and matched no default
#      FORBID, so a boot that LOST a task still scored clean. FORBID added, in the
#      REDZONE block beside the two OPTIONAL rows it must not be confused with.
# All three MOVE A COUNT, which is exactly why the previous pass left them: this one was
# scoped to move them. Required witnesses are unchanged at 13 (a `COMPLETE` is never
# counted into the tally — mbench.py:176); spec-declared FORBID rows go 5 -> 7, three
# arrive and the one that could never match is gone.
#
# 2026-08-25 (exec-spec, second pass): THE PENDING SWEEP. All 8 PENDING rows matched on
# boot7f, and "it matched" was NOT treated as sufficient — the test applied to each was
# whether promoting it adds a way for a HEALTHY boot to red. Three passed and were
# promoted (the EL0-EL1CORE trio); five were left PENDING, every one of them because its
# emitter is behind a `#[cfg(feature = …)]` that this spec does NOT already force, so a
# REQUIRE would fail on CONFIGURATION rather than on health — the SMPMARK argument, and
# the same shape as the TEGRA-SD defect fixed below:
#   `[orinwm1] win=`           `#[cfg(feature = "orindesk")]`, display_tegra.rs:376
#                              (arroyo:735, `UNAOS_ORINDESK=1`, default OFF)
#   `[orinclick] arm`          `#[cfg(feature = "orinclick")]`, display_tegra.rs:1265
#   `[orinclick] census`       same gate (arroyo:790, `UNAOS_ORINCLICK=1`, default OFF)
#   `JD1-DC VERDICT=`          `#[cfg(feature = "jd1dc")]`, display_tegra.rs:631
#                              (arroyo:756, `UNAOS_JD1DC=1`, "default OFF" in its own note)
# and one for a different and stronger reason:
#   `IRQEL-RT: first IRQ taken at EL1`  NOT knob-gated, but it is the PASS arm of a
#     THREE-WAY verdict whose FAIL arm real metal has actually printed (boot5c,
#     `capture/line-acm0/orin.log:8311`). This file already says a REQUIRE here "would
#     convert the instrument into a rubber stamp", and one capture of the good arm does
#     not turn a measurement into an invariant. Promotion needs a second flight at
#     minimum — the two-trial bar this file itself set for TEGRA-SD.
# THE PROMOTIONS COST ONE THING AND IT IS STATED RATHER THAN DISCOVERED LATER: boot5c
# (orin2-boot5c-gui.log) carries ZERO hits for all three promoted rows, because 4309446
# predates the EL0-EL1CORE arc. Replaying boot5c against this file now reads FAIL where it
# used to read PASS. That is correct and not a regression in the spec — a spec adjudicates
# the NEXT flight, the argument this file already makes for the IRQEL FAIL wording — but a
# reader reaching for boot5c as a green reference should know it is no longer one.

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

# --- EL0-EL1CORE: where an EL0 task was placed, and what happens when it cannot be -----
# The arc that motivated this block (sched.rs `EL0-EL1CORE`) established that on the
# smp_virt path only the BSP drops to EL1 — every PSCI-woken AP replays the BSP's EL2
# regime — so an EL0 task dispatched from an AP `eret`s at EL2, banks ELR_EL2/SPSR_EL2,
# and takes the board down with a RAS Uncorrectable. The fix filters EL2 cores out of the
# EL0 candidate set and REFUSES a placement it cannot satisfy.
#
# THE ANCHOR IS THE UNCONDITIONAL PLACEMENT LINE, AND THAT CHOICE IS THE POINT OF THIS
# BLOCK. `spawn_user_inner`'s witness fires on EVERY EL0 slot spawn — it is un-gated as of
# the same arc, so tegra emits it — and on this boot the spawn is `el0-hello`, pinned to
# core 0 by main.rs:6108 (`spawn_user("el0-hello", .., 0)`, a deliberate pin at the core
# the JM6 drop has just proven is at EL1). An explicit pin at an EL1 core passes the filter
# verbatim, so this line is emitted on every healthy tegra_el0 boot that reaches EL0 at all
# — which the `REQUIRE TEGRA-EL0 .. round-trip -> PASS` above already demands.
#
# THE REFUSAL LINE IS NOT AN ANCHOR AND MUST NOT BECOME ONE. Its presence is NOT an
# invariant: it fires only when the EL1-filtered candidate set is empty for the request in
# hand, which for a CPU_AUTO spawn depends on `ONLINE_MASK[0]`, and core 0's online bit is
# not fixed — `run_burst` and `simmer_start` both call `mark_online(driver_cpu)` and both
# are spawned from `run_capstone_boot_core` under `sched_demo`/`simmer_test`. With core 0
# online a CPU_AUTO EL0 spawn can legally succeed and the refusal never prints. Nothing on
# a default boot issues a CPU_AUTO EL0 spawn in the first place (the `bg` verb is operator-
# driven), so the honest strength is OPTIONAL: present in the table, never a gate.
# NOTE FOR ANYONE TEMPTED TO KEY ON `n=`: that counter (`EL0_REFUSALS`) is ONE GLOBAL shared
# by every refusing site, not a per-site sequence, so `n=1` says nothing about which lane
# refused first and it cannot be anchored per-`bg`. Read `el0refuse=` on `[el0core] rollup:` for
# the running total instead — NOT on `[spread4]`, which is LINK-TIME DEAD on a tegra build (every
# caller of `spread4_witness` is unreachable there, so the linker drops the string; sched.rs:3223
# records the `LC_ALL=C grep -a` over the linked `arm-tegra-el0` kernel that measured it). Pointing
# a reader at `[spread4]` on THIS board was the exact blindness commit 2f4cd179 existed to remove:
# the one platform that actually refuses could not read its own refusal counter. The per-event
# lines above stop for good at `EL0_REFUSE_LOG_MAX` (one cap-announce, then silence); the rollup is
# rate-limited to ~1/s but never goes permanently blind, because its guard is "the count CHANGED"
# — so the wire converges on the true total within a second of the last refusal.
# PROMOTED PENDING -> REQUIRE (exec-spec, 2026-08-25), on exactly the capture the old note
# asked for. It was `#[cfg(feature = "pi")]` for the whole tegra_el0 bring-up, so no Orin
# capture COULD carry it — boot5c (orin2-boot5c-gui.log) still has ZERO hits for `SCHED: task`,
# which is the blindness the arc removed. boot7f is the first capture that carries it:
#   `:: SCHED: task 'el0-hello' -> core 0 (policy: caller-pinned EL0 residents=1, no-migrate) ::`
#   — boot7f, `capture/line-acm0/orin.log:11395`
# THE TEST THE PROMOTION HAD TO PASS IS NOT "A CAPTURE HAS IT" BUT "IT ADDS NO NEW WAY FOR A
# HEALTHY BOOT TO RED", and it passes: `spawn_user_inner`'s witness (sched.rs:3800) is un-gated,
# and the only knob anywhere in the chain is `tegra_el0` — which `REQUIRE TEGRA-EL0 ..
# round-trip -> PASS` above ALREADY demands. Any image that can satisfy that row spawns this
# task and prints this line; any image that cannot was already going to red on that row. So
# this is NOT the knob-gated trap the SMPMARK block argues against. The pin to `core 0` is
# main.rs:6108's deliberate `spawn_user("el0-hello", .., 0)`.
REQUIRE SCHED: task 'el0-hello' -> core 0 \(policy: caller-pinned EL0 residents=[0-9]+, no-migrate\)
OPTIONAL SCHED: EL0 placement REFUSED \([a-z]+\) '[^']*' req=-?[0-9]+
# The LAST-INSTANT backstop in `user_task_trampoline`, and this one IS a fault signature —
# the only FORBID in this file that is not a hardware fault. It prints when an EL0 task
# reached the `eret` from a core that is not at EL1, i.e. when the placement filter has a
# hole. Unlike the IRQEL trio above (honest verdicts, deliberately not FORBID), this is not
# a measurement with a legitimate negative outcome: the filter is supposed to make it
# unreachable, so a hit means an invariant broke and the flight should be red. It is also
# the one line that proves the backstop is release-live — it replaced a `debug_assert` that
# no built configuration contained, since the kernel workspace has no `[profile]` section
# and every arroyo cargo call is `--release`.
FORBID SCHED: EL0 entry REFUSED
#
# THE `[el0core]` READERS — the counter's reader and the mask's stamp, on the platform that
# refuses. All three patterns are pure ASCII in the source (verified with `LC_ALL=C grep -a -n`,
# not assumed): unlike the cap-announce at sched.rs:3316, none of these lines carries an em-dash,
# so none needs truncating before one. `{:#x}` renders a lowercase `0x…`, hence `0x[0-9a-f]+`.
#
#   sched.rs:3362 THE ROLLUP. `el0_refusal_rollup` has TWO call sites in
#   `run_capstone_boot_core` — sched.rs:9831, an UNCONDITIONAL baseline BEFORE the drive loop is
#   entered, and sched.rs:9836, a poll inside the inner `while dispatch_next(cpu)` body. (The
#   obvious site, after the inner `while`, is RUNTIME DEAD on tegra: main.rs stages the infinite
#   `jd2_console_pump`, so that queue never drains and the inner loop never returns.) The baseline
#   is the falsifier for the whole instrument: `EL0_ROLLUP_LAST` starts at `u64::MAX`, so `0 != MAX`
#   and a boot that refuses NOTHING still prints one `el0refuse=0`.
#   PROMOTED PENDING -> REQUIRE (exec-spec, 2026-08-25). Evidence:
#     `:: SCHED: [el0core] rollup: el0refuse=0 el1cores=0x1 (EL0-EL1CORE) ::`
#     — boot7f, `capture/line-acm0/orin.log:11407`
#   AND THE BASELINE CALL IS WHAT MAKES THE PROMOTION SAFE: sched.rs:9831 runs it
#   unconditionally before the drive loop, inside `run_capstone_boot_core`, which
#   `REQUIRE CAPSTONE COMPLETE` above already demands. The function body is guarded by a `cfg!`
#   CONSTANT (not `#[cfg]`) on `any(baremetal, tegra_el0)` — sched.rs:3351 says so and sched.rs:3356
#   spells out that an unarmed build "reaches `run_capstone_boot_core` and prints NOTHING". On THIS
#   spec that costs nothing: `tegra_el0` is already forced by `REQUIRE TEGRA-EL0 .. -> PASS`, so
#   every image that can pass this file has the reader linked AND reached, and a boot that refuses
#   nothing still prints the `el0refuse=0` baseline. Absence now means not-linked or not-reached,
#   which is the regression this row was always meant to catch.
REQUIRE \[el0core\] rollup: el0refuse=[0-9]+ el1cores=0x[0-9a-f]+
#   sched.rs:3209 THE STAMP. `mark_el1_core` is called from main.rs:2677, inside `tegra_early_stop`
#   (`#[cfg(all(feature = "tegra", target_arch = "aarch64"))]`), with NO runtime knob and strictly
#   before `tegra_el0_start_maybe()` — an unstamped mask would refuse the pinned `el0-hello`, so the
#   `REQUIRE TEGRA-EL0 .. round-trip -> PASS` above cannot pass without this line having printed.
#   PROMOTED PENDING -> REQUIRE (exec-spec, 2026-08-25). Evidence:
#     `:: SCHED: [el0core] el1 core MEASURED: cpu=0 mask=0x1 (EL0-EL1CORE) ::`
#     — boot7f, `capture/line-acm0/orin.log:11384`
#   THE STRONGEST OF THE THREE PROMOTIONS: `mark_el1_core` carries no `#[cfg]` of its own and its
#   call site (main.rs:2677) carries none either — it sits bare on the statement before
#   `exceptions::install()` and `el1_oneshot_proof()`, and `REQUIRE IRQEL-RT: EL1 one-shot proof`
#   above already demands the line printed two statements later. Unconditional on a tegra image.
REQUIRE \[el0core\] el1 core MEASURED: cpu=[0-9]+ mask=0x[0-9a-f]+
#   sched.rs:3194 and :3202 THE TWO FAIL-CLOSED ARMS, sharing the prefix below (CurrentEL != 1, and
#   cpu_index out of range). OPTIONAL, and the kind is the decision, exactly as for the IRQEL trio:
#   this is a FAULT path. A boot that works correctly never prints it, so PENDING would advise
#   promoting a failure to REQUIRE, and REQUIRE would demand one. Present in the table, never a gate.
OPTIONAL \[el0core\] NOT stamped:

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
#       runtime knob, and main.rs:2590 calls it on the SAME statement as, and strictly
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

# =====================================================================================
# BOOT 7e — THE DESKTOP / CLICK / DISPLAY-PROBE FLIGHT. Everything from here to the
# regression block below is UNFLOWN: every pattern is a PREDICTION about a line that
# has NEVER printed. Measured, not assumed — all 29 patterns take ZERO hits across all
# nine Orin/Jetson captures in the bench tree (85,599 lines), scanned with `awk`.
# Every string keyed on below was read out of the STAGED ARTIFACT
# (`flash/orin/boot7e-desk-click-jd1dcmodel-20260825T1927Z-24284e5/kernel.elf`, SRC.SHA
# commit 24284e50) with `LC_ALL=C grep -a -o` — never `strings`, whose default -n 4
# silently drops short marks and whose runs break at em dashes and middle dots.
#
# THE IMAGE'S FEATURE SET, AND ONE CORRECTION TO THE OBVIOUS READING OF IT:
#   tegra, tegrasmp, smpmark, orindesk, orinclick, jd1dc  (+ always-on redzone guards)
# `tegradesk` and the installer are OUT. `tegra_el0` is IN — NOT because it was passed,
# but because Cargo.toml:1747 declares `orinclick = ["tegra_el0"]`. So the EL0-EL1CORE
# block above is ARMED on this flight and adjudicates normally; its lines are NOT
# expected absent. Confirmed in the artifact rather than inferred from the feature
# list: `TEGRA-EL0` x10, `[el0core]` x4, `el1 core MEASURED` x1, `SCHED: task '` x1.
# (SMPMARK likewise already has its three rows above and needs nothing added here.)
#
# NOTHING BELOW IS A REQUIRE AND NOTHING BELOW IS A FORBID. That is the whole point:
# this file promotes nothing without a capture, and not one of these lines has one.
# The required/forbidden counts are unchanged by this block, by construction.
# =====================================================================================

# --- ORINWM1 (rung 0): the first composited window on Orin silicon -------------------
# ARMED BY `orindesk`. `orin_wm1` (display_tegra.rs:377) is called UNCONDITIONALLY from
# main.rs:2185 — appended to the `:: KERNEL HEAP ALLOCATED ::` statement, no runtime
# knob. It emits EXACTLY ONE line per boot: one of six early-return DECLINEs
# (display_tegra.rs:394/417/421/430/473/491) or the terminal `win=` line (:516).
#
# WHY THE `win=` LINE IS PENDING AND NOT OPTIONAL. It is what a healthy boot of THIS
# image prints, and the only DECLINE a CONFIGURATION rather than a fault can cause is
# `no-panel` — which this spec has already excluded: `orin_wm1` runs at main.rs:2185,
# strictly AFTER JD1's `panel LIVE` at main.rs:2028, and `REQUIRE JD1.*panel LIVE`
# above demands that line. On any capture this spec passes, `WRITER` is seeded and the
# no-panel arm cannot fire. Absence of `win=` on such a boot is therefore a regression,
# and PENDING is the directive that says "promote on the first capture that carries
# it". NOT REQUIRE: no capture carries it yet, and this file promotes nothing without.
#
# THE `\[orinwm1\]` TAG IS LOAD-BEARING, and that is measured rather than stylistic.
# The field shape ` win=<n> panel=<w>x<h> surf=<w>x<h>` is NOT unique to this rung —
# dropping the tag takes two false hits in the existing corpus, both in
# `capture/orin1-boot2/boot2-recovered.log`:
#   `[wc-x] console-window win=1 panel=1920x1200 surf=1295x736 box=1305x780 at (307,158) ...`
#   `[pulsewin] open win=3 panel=1920x1200 surf=1280x120 box=1290x164 at (10,922) ...`
# An untagged pattern would report the x86 compositor's window as the Orin's.
# NO EM-DASH TRUNCATION IS NEEDED HERE: display_tegra.rs:516's format string is pure
# ASCII end to end. The one `[orinwm1]` line that DOES carry an em dash is the
# `no-panel` DECLINE (`(headless boot — no JD1 scanout)`), and the DECLINE pattern
# below stops at `reason=`, well before it.
PENDING \[orinwm1\] win=[0-9]+ panel=[0-9]+x[0-9]+ surf=[0-9]+x[0-9]+
# THE TWO VERDICT ARMS OF THAT LINE, and the kind IS the decision. `present=` is the
# question rung 0 exists to ask — did pixels reach glass, or did the present pass run
# and get suppressed — so BOTH answers are legitimate outcomes of a first flight and
# NEITHER may red it. OPTIONAL, on exactly the IRQEL-trio argument above.
# MEASURED CAVEAT, stated rather than glossed: `wm::Presented::Coalesced` also maps to
# `-> COMPOSITED` (display_tegra.rs:511), but the literal `Coalesced` has ZERO
# occurrences in the staged kernel.elf while `Composited`, `Suppressed` and `NoRow`
# each have one — that arm is unreachable from `present_outcome` on this build and the
# compiler dropped the string. So on THIS image `-> COMPOSITED` implies
# `present=Composited`, and a future build could change that without warning.
OPTIONAL \[orinwm1\] win=.* -> COMPOSITED
OPTIONAL \[orinwm1\] win=.* -> PRESENT-DECLINED
# The six refusal arms under their shared prefix — the `[el0core] NOT stamped:`
# precedent. FAULT PATHS: a correct boot never prints one, so PENDING would advise
# promoting a failure to REQUIRE and REQUIRE would demand one. OPTIONAL: in the table,
# never a gate. The reason is on the line; the table says only that one of them fired.
OPTIONAL \[orinwm1\] DECLINE reason=

# --- ORINCLICK (rung 3): does a press actually reach a window? ----------------------
# ARMED BY `orinclick`. THREE LINES, THREE KINDS, because they have three different
# reachabilities — and flattening them is how this instrument would go quiet unnoticed.
#
#   1. THE ARM LINE (display_tegra.rs:1269) — ONCE, from the first call of
#      `orin_click_census` (display_tegra.rs:1236), which main.rs:2887 appends to
#      `jd2_console_pump`'s phase-2 sweep with no runtime knob. `REQUIRE JD2.*console
#      pump live` above already demands that pump, so on any capture this spec passes
#      the arm line should have printed. PENDING.
#   2. THE CENSUS (display_tegra.rs:1313) — every ~10 s from inside that same drain
#      loop, UNCONDITIONALLY, including and especially when nothing has happened. It is
#      the routing task printing on its own core off its own counter, so its absence is
#      a DEAD PUMP — the regression worth advising a promotion for. PENDING.
#      STATED HOLE, NARROWED BUT NOT CLOSED BY THE `COMPLETE` MARKER ADDED AT THE FOOT
#      (2026-08-25): a capture cut inside the first census period carries the arm line
#      and no census, and this file still cannot tell that apart from a dead pump. The
#      marker does not reach it — it is anchored at `main.rs:2846`'s shell banner, which
#      is UPSTREAM of the arm line, so a capture that stops between the two reads
#      "complete" and the missing census reads as a dead pump. Anchoring the marker on
#      the census instead was considered and REJECTED: the census is `orinclick`-gated,
#      and a marker that depends on CONFIGURATION would make every unarmed boot report
#      TRUNCATED — the failure mode the SMPMARK block above argues in full, in the other
#      verdict. PENDING never fails a run, so the row is safe either way; the reader
#      checks the arm line by eye, exactly as for the IRQEL window above.
#   3. THE EDGE LINE (display_tegra.rs:1179) — one per Button event, routed from
#      main.rs:2852. An UNATTENDED boot prints none, and that is not a fault. OPTIONAL.
#      Absence here means UNRUN, never failed — the census's `IDLE-NO-CLICKS` is the
#      line that says which.
PENDING \[orinclick\] arm panel=[0-9]+x[0-9]+ rows=[0-9]+
PENDING \[orinclick\] census seq=[0-9]+ t=[0-9]+ up=[0-9]+s btn=[0-9]+
OPTIONAL \[orinclick\] edge=(press|release|none) btn=0x[0-9a-f]+
# THE CENSUS VERDICTS THAT ARE NOT FAILURES. `ROUTING` is the success answer and it is
# deliberately NOT a REQUIRE: it can only be reached if a human actually pressed the
# button, so a REQUIRE would red an unattended boot on CONFIGURATION rather than on
# health — the argument the SMPMARK block above makes in full. `IDLE-NO-CLICKS` is the
# UNRUN answer and is equally legitimate. Both OPTIONAL; between them they tell the
# reader whether the click test was performed at all, which is the first thing anyone
# adjudicating this flight needs to know.
OPTIONAL \[orinclick\] census .* -> ROUTING
OPTIONAL \[orinclick\] census .* -> IDLE-NO-CLICKS
# The DECLINE arms of all three lines, keyed on the `-> ` that precedes every verdict:
# `no-geometry` (edge); `panel-locked` / `no-panel` / `no-target` (arm); and
# `geometry-refused` / `no-target` / `release-only` / `all-miss` (census). REFUSALS,
# not failures — `DECLINE reason=no-target` is precisely what a correct `orinclick`
# boot prints when no window exists to click. OPTIONAL. The pattern stops at `reason=`
# on purpose: the ARM line's no-target text continues `(wm table empty — arm
# UNAOS_ORINDESK=1 for a row to click)` and carries an em dash a lossy UART can replace.
OPTIONAL \[orinclick\] .* -> DECLINE reason=
# NO ROW IS WRITTEN FOR THIS RUNG'S THREE `FAIL` VERDICTS, AND THAT IS DELIBERATE.
# `FAIL reason=no-raise` and `FAIL reason=miss-unhandled` (the edge line) and
# `FAIL reason=stuck-focus` (the census) all render on the wire as `-> FAIL reason=…`,
# which mbench's ALWAYS-ON default FORBID `-> FAIL` (mbench.py:135) already matches.
# They red the flight with no help from this file, and that IS the correct outcome:
# unlike the JD1-DC and IRQEL verdicts, a press that hit a window and did not move the
# focus is an invariant break, not a measurement with a legitimate negative arm. A
# second FORBID here would only double-report it.
# READ THIS BEFORE THE FLIGHT: it means an attended click that finds `stuck-focus`
# turns the WHOLE replay FAIL. That is the intended reading, not a spec defect.

# --- JD1-DC: does the CCPLEX decode nvdisplay, and through WHICH register map? ------
# ARMED BY `jd1dc`. `jd1_dc_probe` (display_tegra.rs:632) is called from main.rs:2127
# with no runtime knob, inside the block guarded by JB1b's resolved DTB geometry —
# which `REQUIRE JB1b.*MRQ_PING.*-> PASS` above already demands.
#
# TWO ORTHOGONAL AXES, AND CONFLATING THEM IS THE DEFECT THE `MODEL-VERDICT=` AXIS WAS
# ADDED TO REMOVE. `VERDICT=` answers "does the aperture decode, and did a window hold
# the inherited scanout base". `MODEL-VERDICT=` answers "and through WHICH register map
# were we reading". `DECODES-NOMATCH` alone cannot separate a wrong-window sweep from a
# wrong-chip register map; the two lines together can.
#
# AXIS 1 — `JD1-DC VERDICT=`. EXACTLY ONE prints on every path that reaches an
# nvdisplay read or refuses to (display_tegra.rs:616 states the invariant; the six
# REFUSED arms are the early returns at :640 / :649 / :656 / :676 / :686 / :723 and the
# three read verdicts are at :845 / :857 / :866). Unconditional on this image, so the
# UMBRELLA is PENDING: absence on the next capture means the rung is not linked or not
# reached, which is a real regression and should advise the promotion.
PENDING JD1-DC VERDICT=
# THE FOUR ARMS ARE OPTIONAL AND NONE OF THEM MAY RED THE FLIGHT. This is a PROBE and
# its polarity IS the finding: `NOT-DECODING` is not a failure of UnaOS, it is an answer
# about Tegra234 silicon that nobody in this tree has ever had. A FORBID on any arm
# would convert a measurement into a rubber stamp; a REQUIRE on `REACHABLE` would demand
# an answer the hardware may simply not give. Every pattern stops before the em dash
# that follows the verdict word on all nine of these lines — they are long and
# prose-heavy and they cross a UART shared with the SPE's TCU.
OPTIONAL JD1-DC VERDICT=REACHABLE
OPTIONAL JD1-DC VERDICT=DECODES-NOMATCH
OPTIONAL JD1-DC VERDICT=NOT-DECODING
OPTIONAL JD1-DC VERDICT=REFUSED reason=[a-z-]+
# THE HANG-FORENSICS PAIR (display_tegra.rs:732 and :738). The first announces a read
# into an MMIO class this CCPLEX has never touched; the second says it came back. If
# the board dies inside an EL3-fatal read, FIRST TOUCH is the LAST line on the wire and
# the SURVIVED line never comes — that asymmetry IS the instrument. OPTIONAL, both:
# they are reachable only once the BPMP power guard has passed, and a boot that refuses
# at the guard legitimately prints neither.
# THE PATTERNS DELIBERATELY START AFTER THE EM DASH. The wire text is
# `:: tegra: JD1-DC — FIRST TOUCH …`; keying on the `JD1-DC` prefix would put a
# multi-byte character inside the match on the exact line whose job is to survive a
# board that is about to stop transmitting. Both fragments below are contiguous ASCII
# in the staged kernel.elf and were read out of it.
OPTIONAL FIRST TOUCH of a new MMIO class: about to read
OPTIONAL FIRST READ SURVIVED:
# AXIS 2 — `MODEL-VERDICT=` (`jd1_dc_model`, display_tegra.rs:1490, called from :741).
# SEVEN ARMS, AND EVERY ONE OF THEM IS A LEGITIMATE RESULT. There is no failure arm on
# this axis: the rung's entire purpose is to report which of seven states this silicon
# is in, and a spec that let one of them red the flight would be asking the hardware to
# BE something instead of measuring what it IS. So: ALL SEVEN OPTIONAL — and, unlike
# `VERDICT=` above, NO PENDING UMBRELLA EITHER. `jd1_dc_model` is called downstream of
# both the BPMP guard and the aperture-size check at :723, so a boot that prints
# `VERDICT=REFUSED reason=aperture-too-small` prints no MODEL-VERDICT line at all and
# is CORRECT to. A PENDING here would advise promoting a line that can legitimately be
# absent — the same error as a REQUIRE, in the other direction.
# THE TWO CLASS ARMS ARE DISJOINT BY CONSTRUCTION: `{:04X}` (display_tegra.rs:1669,
# :1675) renders UPPERCASE hex, and `UNKNOWN` cannot match `[0-9A-F]{4}` because `U` is
# not a hex digit — so the specific arm can never swallow the unknown one.
OPTIONAL JD1-DC-MODEL MODEL-VERDICT=NVDISPLAY-CLASS-[0-9A-F]{4}
OPTIONAL JD1-DC-MODEL MODEL-VERDICT=NVDISPLAY-CLASS-UNKNOWN-[0-9A-F]{4}
OPTIONAL JD1-DC-MODEL MODEL-VERDICT=DECODES-NOT-NVDISPLAY
OPTIONAL JD1-DC-MODEL MODEL-VERDICT=NOT-DECODING
OPTIONAL JD1-DC-MODEL MODEL-VERDICT=DISCRIMINATOR-TRIVIAL
OPTIONAL JD1-DC-MODEL MODEL-VERDICT=UNDETERMINED reason=discriminator-not-read
OPTIONAL JD1-DC-MODEL MODEL-VERDICT=REFUSED reason=no-reads
# THE SUPPORTING DUMPS. `JD1-DC-REG` is the DTB `reg`/`reg-names` decode, `JD1-DC-IDS`
# the clocks/resets/power-domains resolution the BPMP guard is built out of, and
# `CAP CROSS-CHECK` the +0x30000-vs-+0x00060 comparison that says whether the two
# capability mirrors agree. DIAGNOSTICS ATTACHED TO A VERDICT, not verdicts — nothing
# should ever be REQUIRED to print a state dump, exactly as argued for `[irqel2a]`.
# All three tags are followed by an em dash on the wire; all three patterns stop first.
OPTIONAL JD1-DC-REG
OPTIONAL JD1-DC-IDS
OPTIONAL CAP CROSS-CHECK

# --- REDZONE: the kernel-stack guard bands, and they are ALWAYS ON ------------------
# NOT knob-gated and NOT feature-gated: sched.rs:41 sizes a 1024 B low absorber
# (`STACK_REDZONE`) and a 512 B high guard (`STACK_HIGHGUARD`) on every task stack in
# every build, and both readers sit on the dispatch path. UNLIKE the SMPMARK block
# above, therefore, absence here is NOT a configuration artifact — it is the good news.
#
#   sched.rs:5298 LOW-REDZONE — read AFTER the task returns from `switch_context`.
#     `entered` = this task's own SP crossed into its absorber; `TRAVERSED` = the
#     absorber is EXHAUSTED and the slab below it may already hold a smashed frame.
#   sched.rs:5225 HIGH-GUARD — read BEFORE the switch. A NEIGHBOUR's overrun came into
#     this stack's slab from ABOVE; the guard absorbed it and this task IS resumed.
#
# BOTH ARE FAULT PATHS, SO BOTH ARE OPTIONAL — the rule the `[el0core] NOT stamped:`
# and `[orinwm1] DECLINE` rows follow. A correct boot never prints either, so PENDING
# would advise promoting a stack overflow to REQUIRE. And NOT FORBID, which is the one
# judgement call in this block and is made deliberately: these lines report a guard
# that WORKED. The overrun was absorbed, the parked frame is intact, the task is
# resumed, and both emitters are rate-limited to 16 reports. Redding a flight on an
# absorbed overrun would suppress the one signal telling the next arc which stack to
# grow.
# EM-DASH TRUNCATION: both lines carry an em dash immediately after `task={id}:{name}`,
# so both patterns stop at `task=` and neither ever contains a multi-byte character.
OPTIONAL \[redzone\] cpu=[0-9]+ LOW-REDZONE (entered|TRAVERSED) task=
OPTIONAL \[redzone\] cpu=[0-9]+ HIGH-GUARD entered task=
# THE UNABSORBED CASE — FORBID, and the kind is the whole point of the row (2026-08-25:
# this was a STATED GAP in this file until now; a boot that lost a task this way scored
# clean, checked and not assumed). When the saved SP is outside the task's own stack, or
# the high guard was TRAVERSED rather than merely entered, `dispatch_next` refuses the
# switch-in at sched.rs:5228 and DROPS the task. That is categorically NOT the two rows
# above: those report a guard that WORKED and a task that IS resumed; this one reports a
# guard that was overrun and a task that no longer exists. It is an invariant break, and
# it must red the flight.
# WHY IT NEEDED ITS OWN ROW rather than a default FORBID. mbench's always-on set is
# `-> FAIL` / `FAIL ::` / `PANIC` (mbench.py:135) and this line carries none of the
# three — it is `[spin6] cpu=… REFUSING corrupt switch-in: …`. Measured against the
# whole bench tree, not assumed.
# THE PATTERN STOPS AT `task=`, WHICH IS DELIBERATE AND BUYS TWO THINGS. (1) Everything
# after `task={id}:{name}` is an em dash and then prose, and this file never puts a
# multi-byte character inside a pattern that crosses UARTC. (2) The wording after that
# point CHANGED — the corpus carries the pre-2026-08 text (`ctx_sp=… outside its stack
# […] — the parked frame was OVERWRITTEN (neighboring stack overflow?)`) while the
# current emitter writes `ctx_sp=… vs its stack […) higuard=N — …`. The prefix is
# identical in both, so this row adjudicates old captures and new ones alike.
# ARMED, AND THAT IS MEASURED: `[spin6] cpu=` and `REFUSING corrupt switch-in: task=`
# are each present once in the staged boot7f kernel.elf
# (`flash/orin/boot7f-nowinsweep-20260825T2034Z-04d46aa/kernel.elf`, read with
# `LC_ALL=C grep -a -o -F`), so the emitter is linked on a tegra image — `sched.rs:41`
# sizes the bands in every build and both readers sit on the dispatch path.
# ZERO FALSE HITS ON THIS BOARD: the pattern takes 0 hits across every Orin/Jetson
# capture in the bench tree and REAL hits on other platforms — pi4 (`pi3-boot11/
# boot11.log:7424`, `pi4-pi1-b1/ttyACM0.log:1372`, `pi4-pi0-b1/ttyACM0.log:1324`) and
# the pi bridge line (`line-acm0/pi.log:6971`, `:10241`, `:21160`). Those hits are what
# prove the row can fire; the Orin zeroes are what prove arming it costs this lane
# nothing on a healthy boot.
FORBID \[spin6\] cpu=[0-9]+ REFUSING corrupt switch-in: task=

# --- COMPLETE: the END-OF-RUN MARKER this file spent its whole life without ----------
# Until 2026-08-25 this spec declared ZERO `COMPLETE` markers, so mbench's TRUNCATED
# verdict (rule 2 of `run_verdict`) could never fire for it and a capture cut short read
# FAIL — a regression — rather than the honest INCONCLUSIVE. That mattered more here
# than on any other platform: this medium is KNOWN-LOSSY (UARTC is shared with the SPE's
# TCU), so a short capture is the ordinary case, not the exotic one.
#
# THE MARKER IS THE LAST STRUCTURAL LINE, NEVER A VERDICT — the rule
# `pi4-regression.spec:96` states and the reason there is only one row here. `main.rs:2846`
# prints the shell banner through the JD11 serial sink UNCONDITIONALLY, and only THEN
# does `match first_key` choose between the two console-ownership arms:
#   `JD2 … console OWNS the panel …; first key 0x..`   (main.rs:2852 — attended boot)
#   `JD4 … console OWNS the panel …; screen-on-boot`   (main.rs:2860 — quiescent boot)
# So the banner is the latest point common to BOTH legitimate exits, and it sits exactly
# ONE line ahead of `REQUIRE JD4.*console OWNS the panel`, the last REQUIRE in this file
# in boot order. Every one of the 13 required witnesses is upstream of it.
#
# ANCHORING ON JD4 ITSELF WOULD HAVE BEEN THE DEFECT, not the fix: JD4 is a REQUIRE, and
# a marker pinned to a witness lets a genuine JD4 regression re-badge itself as a short
# log. The bias runs the other way on purpose — a capture severed in the ONE-line window
# between the banner and JD4 reports FAIL rather than TRUNCATED, and both are red.
#
# MEASURED, NOT ASSUMED. The literal is present once in the staged boot7f kernel.elf and
# lands verbatim on the wire in all three modern Orin captures, at the same position in
# the same order (CAPSTONE COMPLETE -> UI1 -> banner -> JD4):
#   boot7f `capture/line-acm0/orin.log:11422`
#   boot5c `capture/orin2-boot5c-gui.log:1551`
#   boot4f `capture/orin2-boot4f.log:1483`
# The pattern deliberately starts AFTER the em dash in `:: tegra: JD2 — OUT | …`: the
# fragment below is contiguous ASCII, so a DARKWIN-dropped byte cannot lossy-replace a
# multi-byte character inside the one line the truncation verdict hangs on.
COMPLETE JD2: interactive shell on the inherited scanout

# --- regressions that would convict the merge, not the hardware ---------------------
FORBID PANIC
# THE TWO SError ROWS THAT REPLACED `FORBID Serror` (2026-08-25). The old row COULD NEVER
# FIRE and this file said so for a week without arming it: the kernel emits `SError` and
# `SERROR`, the staged kernel.elf carries `SError` x2 and `SERROR` x6 and the literal
# `Serror` ZERO times, and mbench compiles every pattern with a bare `re.compile`
# (mbench.py:157) — no `re.IGNORECASE`, and the spec grammar has no flag directive.
# Dates from 18813ed1 (2026-08-18), this file's first commit.
#
# A CASE-INSENSITIVE FIX WOULD HAVE BEEN WORSE THAN THE DEFECT, AND THAT IS THE WHOLE
# REASON THESE ARE TWO NARROW ROWS INSTEAD OF ONE `(?i)serror`. Python DOES accept a
# leading inline `(?i)`, so the loose fix was available and was REJECTED: of the eight
# `SError`/`SERROR` literals in the staged kernel.elf, exactly TWO are fault text and
# the others are BENIGN — and one of the benign ones already has an OPTIONAL row in this
# very file. Measured with `LC_ALL=C grep -a -o -P` over
# `flash/orin/boot7f-nowinsweep-20260825T2034Z-04d46aa/kernel.elf`:
#   display_tegra.rs:676 `VERDICT=REFUSED reason=domain-not-on — … (JX1: SError ESR
#     0xbe000011 …)` — a REFUSAL this spec deliberately scores ◦, not ❌
#   exceptions.rs:530  `:: SERROR-DRAIN: consumed N latent async abort(s) … — machine
#     clean ::` — the drain witness, printed when the guard WORKED
#   sdmmc_tegra.rs:2678 `FWALL: vendor block DISABLED — metal conviction: SError with
#     clocks proven on …` — a configuration witness
#   + four `SERROR_DRAIN_*` symbol names, which are not wire text at all
# `(?i)serror` would red a healthy boot on all three. So the rows key on the FAULT
# emitters and nothing else.
#
# ROW 1 — `aarch64_fault_handler` (exceptions.rs:614), reached from `__vec_serror`
# (exceptions.rs:371) ONLY after `aarch64_serror_drain_check` declines to consume the
# abort, i.e. outside an armed drain window. It prints `=== AARCH64 EXCEPTION: {} ===`
# with `what` = `"SERROR"` (exceptions.rs:687) and then `hlt_loop()`s. Fatal by
# construction. LINKED ON A TEGRA IMAGE: `main.rs` calls `exceptions::install` on the
# post-drop path — `REQUIRE IRQEL-RT: EL1 one-shot proof` above already depends on it —
# and both halves of the line are in the staged kernel.elf (`=== AARCH64 EXCEPTION: `
# x1, the merged `SYNCHRONOUSFIQSERROREL0-SYNC (no current task` blob x1). The assembled
# wire form is not a prediction: it appears verbatim, seven times, in three captures
# already in the bench tree — `capture/pi-r22s2/cu.usbmodem143402.log:96,197,281`,
# `capture/pi4-p40/cu.usbmodem142402.log:678,1379,2641` and
# `capture/rmbp-r23s12/cu.usbmodem142402.log:486` (an aarch64 session under a bridge
# directory named for the other bench; the text is what matters, not the folder).
# ZERO hits on any Orin/Jetson capture, so arming it costs this lane nothing.
# The leading `=== ` is left off the pattern on purpose — three punctuation bytes are the
# cheapest thing on this wire to lose, and the text after them is already unique.
FORBID AARCH64 EXCEPTION: SERROR
# ROW 2 — `tegra_fault_handler` (mmu_tegra.rs:1484), the EARLY-boot vectors that own the
# machine before `exceptions::install` runs. Different printer, different wording, same
# verdict: it prints `:: tegra: FAULT — entry {idx} ({kind}) ESR=… ::` with `kind` =
# `"SError"` (mmu_tegra.rs:1491) and then spins forever. A tegra boot can take a fatal
# SError on EITHER handler depending on WHEN, so one row cannot cover both — that is why
# there are two and not one alternation.
# THE PATTERN STARTS AFTER THE EM DASH in `:: tegra: FAULT — entry`, so `entry [0-9]+
# \(SError\) ESR=` is contiguous ASCII. Both literals are in the staged kernel.elf
# (`:: tegra: FAULT ` x1 and the merged `…syncIRQFIQSError` blob x1), so this row is
# ARMED on the image; the `(SError)` and `) ESR=` halves are assembled at runtime, which
# is why neither the whole line nor a `(SError) ESR=` fragment appears in the .elf.
FORBID entry [0-9]+ \(SError\) ESR=
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
