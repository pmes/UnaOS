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
# ============================================================================
# 2026-08-31 (orin 11, SPECSCORE) — HOW TO SCORE A CAPTURE AGAINST THIS FILE, and the one
# thing mbench cannot tell you.
#
#   scripts/orin-specscore.py <capture> --spec scripts/specs/jetson-sync1.spec \
#       --image ~/unaos-bench/flash/orin/<staged-dir>/
#
# `mbench.py --replay` remains the semantics of record and orin-specscore imports it rather
# than re-implementing it, so the verdict is the same verdict. What it ADDS is the question
# this file had no way to ask: COULD THIS RULE HAVE FIRED ON THE IMAGE THAT PRODUCED THIS
# CAPTURE? A rule whose emitter is `#[cfg]`-erased from the build prints `✅ 0 hits` and is
# indistinguishable, in mbench's table, from a rule that passed — and this file carries
# SIX such rows against the 2026-09-01 staged pair (the two sdmmc rows, the `[wedge4]`
# tripwire, the two stale-image takeover rows, and the firmware RAS row). Each is argued
# where it lives; none is deleted, because each is correct for an image that builds it.
# The tool exits 4 (PASS-BUT-VACUOUS) when a capture passes while a FAILABLE row could not
# have fired, and `--accept-dead <spec lines>` records the reviewed exemptions ON THE
# COMMAND LINE rather than in this file — see the GRAMMAR LIMIT note below.
#
# ⚠ GRAMMAR LIMIT, restated here because three separate blocks below run into it: mbench
# has no `WHEN <guard> REQUIRE/FORBID <rx>`, so this file CANNOT say "this row applies to
# the conwin image only". Every per-image asymmetry is therefore handled by making the row
# a FORBID or a PENDING (neither can red an unarmed boot) and arguing the reading in prose.
# That works for FORBIDs and it is why the `orinconwin` / `orinstkdepth` / `supstate` rows
# are shaped the way they are; it does NOT give the file a way to REQUIRE something of one
# image and not the other, and nothing here should be read as if it did. orin-specscore
# does not invent that syntax either — it MEASURES which rows a given image can fire and
# reports the answer, which is the part that was missing.
#
# ⚠ AND: a rule keyed on RUNTIME-COMPOSED text cannot be validated against an artifact at
# all. `live=FROZEN` is the worked example (see its block); the contiguous string is never
# in any `.rodata` because `live={}` and `"FROZEN"` are separate literals. Before grepping
# a staged `kernel.elf` to "check" a row, confirm the row's text is a CONTIGUOUS LITERAL in
# the source. If it spans a `{}`, the grep can only ever return zero and means nothing.
#
# A synthetic green reference lives at `scripts/specs/jetson-sync1-green.capture` — 17/17,
# zero forbidden hits. It is SYNTHETIC and is not a metal green reference; its job is to
# prove this file CAN go green, since every real capture predating the folds now reds.
# ============================================================================
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
#
# 2026-08-25 (exec-spec, third pass): DEFECT 4 — `REQUIRE TEGRA-SD.*block backend published`
# failed on CONFIGURATION, not on health, and had been reading a healthy boot7f 12/13 for it.
# Replaced by `PENDING` + two `FORBID`s on the armed-path non-publish outcomes; the argument
# and the six-way enumeration are at the row itself. THE EXPECTED COUNTS ARE NOW A FUNCTION
# OF THE KNOB SET, and stating that plainly is part of the fix:
#   15 REQUIREs total (13 before this arc, +3 promoted, -1 demoted here), and ALL FIFTEEN
#     are forced by `tegra` + `tegra_el0` alone — no other knob moves the required tally.
#   `UNAOS_TEGRA_EL0=1` (or `UNAOS_ORINCLICK=1` / `UNAOS_TEGRADESK=1`, which imply it) is
#     therefore the minimum this file adjudicates at all.
#   WITHOUT `UNAOS_SDMMC=1`: a healthy flight is 15/15 PASS and the microSD publish reads ⏳.
#     boot7f is this case, and it now reads PASS instead of the 12/13 FAIL it had earned
#     only by not being asked for a card.
#   WITH `UNAOS_SDMMC=1`: still 15/15 PASS, and the publish row reads ✅ with mbench advising
#     a promotion it must NOT be given — the row is PENDING on purpose, permanently. A card
#     that does not publish reds through the two FORBIDs, naming the rung that stopped.
#   The knob-gated instruments (`orindesk` / `orinclick` / `jd1dc`) never move the count in
#     either direction; that is what their PENDING/OPTIONAL kinds are for.
#
# 2026-08-25 (exec-tailfold, boot7h fold): THREE NEW WITNESS FAMILIES flown on boot7h
# (media boot7h-conwin-net4-20260825T2208Z-68c4758, SRC.SHA 68c47585; scored slice
# capture/line-acm0/orin.log lines 13159-16290) get rows, and ONE moves the count:
#   REQUIREs go 15 -> 16: `SCHED: load` (SMPINSTR, a50358f0) — un-gated, emitted from
#     `run_capstone_boot_core` strictly before the drive loop, the exact reachability
#     argument the `[el0core] rollup:` REQUIRE already carries. The row and its full
#     justification live in the SMPINSTR block below the EL0-EL1CORE block.
#   ORINCONWIN (rung 4, `orinconwin`, `UNAOS_ORINCONWIN=1`, default OFF) and ORIN-NET-4's
#     NET-4F/NET-4V instruments (`net4`, `UNAOS_NET4=1`, default OFF) are BOTH knob-gated, so
#     their healthy-terminus rows are PENDING and their verdict arms OPTIONAL — the orinwm1 /
#     orinclick / jd1dc idiom exactly, and for the same SMPMARK reason: a REQUIRE would red
#     every unarmed boot on CONFIGURATION. Their PENDINGs matched on boot7h and mbench will
#     advise promotion; the advice must NOT be taken, permanently, same as TEGRA-SD.
#   GREEN-REFERENCE CHANGE, stated rather than discovered later (the boot5c precedent above):
#     boot7f (capture lines 10055-11541) and boot7g (11542-13151) carry ZERO `SCHED: load`
#     hits — a50358f0 postdates both images — so replaying either against this file now reads
#     FAIL on that one row. Correct, not a regression: a spec adjudicates the NEXT flight.
#     boot7h (13159-) is the green reference now: 16/16, 0 forbidden.
#
# 2026-08-25 (exec-smallfix): IRQEL-RT PASS-arm promotion — the second flight the PENDING
# SWEEP above demanded has arrived. `IRQEL-RT: first IRQ taken at EL1` is captured on two
# CONSECUTIVE metal flights (boot7g `orin.log:12967`, boot7h `:14822`), both post-IRQEL-RT2,
# so the row goes PENDING -> REQUIRE (argument in full at the row, including the one cost the
# promotion buys). REQUIREs 16 -> 17; spec-declared PENDINGs 10 -> 9. Supersedes the tallies
# in the exec-tailfold note above: boot7h replays 17/17, 0 forbidden, pending 8/9 (TEGRA-SD
# the one unmatched) and remains the green reference. Go-red proven both directions this
# session: the boot7h slice PASSes 17/17 with the promoted row ✅; the same slice with the
# one IRQEL-RT PASS line removed reds on exactly this row (16/17).
#
# 2026-08-25 (exec-boot7iprep): BOOT 7i ARMED AHEAD OF ITS CUT — five witness families
# from the forming metal batch (ORIN-REBOOT's verb+watchdog halves, ORIN-SHUTOFF,
# ORIN-SELFUP's S0..S6 ladder, ORIN-BSPTICK, NET-4G) get rows in a BOOT 7i banner block
# below the BOOT 7h one. NO COUNT MOVES: the boot7i image is uncut and not one of the
# nineteen new patterns has ever printed on this bench (zero hits across the whole
# capture tree AND the boot7h slice — the per-block banner carries the numbers), so
# everything arms as PENDING (the lines an armed boot prints unconditionally) or
# OPTIONAL (operator-driven verbs, payload-conditional ladder stages, self-gating
# NET-4G arms), promote-on-evidence. boot7h remains the green reference: 17/17, 0
# forbidden, pending 8/15 matched after this fold (TEGRA-SD unmatched as before; the
# six new PENDINGs read ⏳ until boot7i flies).
#
# 2026-08-25 (exec-o7-boot7jspec): BOOT 7j ARMED AHEAD OF ITS CUT — the FOCUSED flight for
# the two arcs that each put a real new risk on this board's interactive surface, given
# their own banner block below the BOOT 7i one: ORIN-BSPRUN (the first preemption ever on
# this surface — terminus swap to `run_bsp_tegra`, the first tegra `SCHED_ACTIVE` setter,
# and the post-EOI `timer_preempt` arm that was ABSENT from `handle_irq_v3`) and
# ORIN-SUPSTATE (the first restructure of the one working pump — the console surface lifted
# to a module-owned handle and the pump split into three named roles). SMPMARK is armed as
# always and needs no new row (checked, not assumed — its three marks are pre-terminus).
# ONE ROW IS WIDENED AND ONE FORBID IS ADDED; NOTHING ELSE MOVES A COUNT:
#   `REQUIRE CAPSTONE COMPLETE` -> `REQUIRE (CAPSTONE COMPLETE|[orinbsprun] boot core N joins
#     run())`. IT HAD TO MOVE: ORIN-BSPRUN replaces `run_capstone_boot_core` at the terminus,
#     so an armed boot spawns NO capstone and the old row failed on CONFIGURATION — the
#     TEGRA-SD defect, on this file's oldest scheduler invariant. Unlike TEGRA-SD an
#     alternation IS available, because the armed image prints a marker on the statement that
#     replaces the one that would have printed the old witness. Argued at the row; the widened
#     pattern takes the SAME 217 hits tree-wide as the bare one, so it costs no strength.
#   Spec-declared FORBIDs 9 -> 10: `[wedge4] preempt-in-section`, the WEDGE-4 tripwire that
#     lives inside `timer_preempt` and that ORIN-BSPRUN makes reachable on tegra for the first
#     time. Zero hits across the whole tree; it can only ever speak about a bsprun flight.
#   REQUIREs stay 17. PENDINGs 15 -> 20, OPTIONALs 60 -> 68.
# boot7h REMAINS THE GREEN REFERENCE and this fold does not move it: 17/17, 0 forbidden,
# pending 8/20 (the five new PENDINGs ⏳ until boot7j flies). Every direction proven before
# commit: the widened REQUIRE reds on a boot7h slice with its `CAPSTONE COMPLETE` removed
# (16/17) and greens again when that line is replaced by the `[orinbsprun]` banner (17/17);
# an armed-shape synthetic flips all five new PENDINGs (14/20, PASS); an all-rows synthetic
# proves every one of the fourteen new rows matchable and reds on the new FORBID.
#
# 2026-08-28 (exec-orin10-specgate): BOOT 7k — THE TERMINUS INSTRUMENTS GET ROWS, and the
# reason is a measured gap rather than a missing feature. The TERMINUS fold (405b21f6)
# repaired four tegra instruments and then proved that NO CHECK LEG IN THIS TREE CAN
# DISTINGUISH TWO OF THE FOUR FIXES: it re-introduced the defects deliberately and both
# `arm-tegra-furn` and `arm-tegra-supstate` still exited 0. Tegra legs are `cargo check`
# only and no QEMU regression compiles `tegra`, so the compile matrix scores tegra
# COMPILATION and cannot score tegra BEHAVIOUR at all. This file is now the only thing
# that scores those four instruments on anything, and the block is at the foot, above the
# COMPLETE marker.
#   Spec-declared FORBIDs 10 -> 15, PENDINGs 20 -> 23, OPTIONALs 68 -> 74. REQUIREs stay 17
#   — three of the four families are knob-gated and default OFF, so every failable rule is
#   spelled as a FORBID on a FAILURE LITERAL: a token that exists only in an image that
#   compiled the instrument, so an unarmed boot cannot trip it and its silence is never
#   read as a pass. The one pair whose reachability is GUARANTEED (by this file's own
#   `REQUIRE JD4.*console OWNS the panel`) is the takeover-path pair.
# ⚠ GREEN-REFERENCE CHANGE, THE LARGEST THIS FILE HAS TAKEN. The takeover-path FORBIDs
#   match the PRE-fold shape of a line every Orin capture in the tree carries (13 hits in
#   capture/line-acm0/orin.log), so boot7f/7g/7h — the standing green reference included —
#   now replay FAIL on exactly those two rows. Correct, and the point: those captures came
#   from images that could not say which of the two takeover sites had printed. THERE IS NO
#   GREEN REFERENCE FOR THIS FILE UNTIL A POST-405b21f6 IMAGE FLIES. Do not weaken the rows
#   to recover a green; fly the fold.
# Every direction proven before commit against `capture/line-acm0/orin.log` (mbench
# --replay): the un-folded capture reds on the two takeover rows and ONLY those two; a
# synthetic post-fold takeover line greens them; a synthetic carrying `live=FROZEN`,
# `DEPTH-UNAVAILABLE` and `RAST-PAINTED-OVERWRITTEN` reds on each of those three rows
# individually. The three go-red proofs are the reason those rows are not decoration.

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
# WIDENED 2026-08-25 (exec-o7-boot7jspec, the boot7j fold) — AND THE WIDENING IS THE ONLY
# WAY THIS ROW SURVIVES ORIN-BSPRUN. `CAPSTONE COMPLETE` is printed by `capstone_body`
# (sched.rs:8185), which runs only if `run_capstone_boot_core` SPAWNED it. ORIN-BSPRUN
# (`bsprun`, UNAOS_BSPRUN=1) replaces that whole function at the terminus with
# `run_bsp_tegra(0)` (main.rs:2679's cfg-selected statement), which spawns NO capstone —
# the arc's own commit says so in as many words ("`run_bsp_tegra` bypasses
# `run_capstone_boot_core`, so no CAPSTONE is spawned"). So the un-widened row FAILS ON
# CONFIGURATION on a perfectly healthy bsprun flight: the TEGRA-SD defect, in a new place,
# and this time on the row that is this file's oldest scheduler invariant.
#
# WHY AN ALTERNATION IS AVAILABLE HERE AND WAS NOT AVAILABLE FOR TEGRA-SD. The TEGRA-SD
# note above rejects exactly this shape and gives the reason: "an unarmed image prints NO
# sdmmc line at all, so there is no marker to key the second branch on". The bsprun case is
# the mirror — the armed image prints a marker, `[orinbsprun]`, at the terminus, on the
# statement that REPLACES the one that would have printed `CAPSTONE COMPLETE`. The two
# branches are the two termini, exactly one of which runs on any boot, so the alternation IS
# the conditional the grammar cannot otherwise spell.
#
# IT COSTS NOTHING ON AN UNARMED FLIGHT, and that is measured rather than argued: `bsprun`
# is `#[cfg]`-gated end to end and the string `[orinbsprun]` is not in a knob-off image at
# all, so on every image this file has ever adjudicated the second branch is unsatisfiable
# and the row is byte-for-byte as strong as the one it replaces. Across the whole bench
# capture tree (313 files, 2,383,287 lines) the widened pattern takes exactly the same 217
# hits as the bare `CAPSTONE COMPLETE` — the second branch contributes ZERO. Go-red proven
# both directions on the boot7h slice: 17/17 unchanged; the same slice with its one
# `CAPSTONE COMPLETE` line removed reds on exactly this row (16/17); the same slice with
# that line REPLACED by the `[orinbsprun]` banner reads 17/17 again.
#
# THE ONE THING THE WIDENING GIVES UP, STATED. Three rows below (`SCHED: load`,
# `[el0core] rollup:`, `IRQEL-RT: EL1 one-shot proof`) justify their REQUIRE by pointing at
# this one: "CAPSTONE COMPLETE prints AFTER the baseline emits, so any image that passes
# this file has reached them". `[orinbsprun]` prints BEFORE them, not after. The argument is
# not lost, it changes shape: on the armed terminus the banner, `el0_refusal_rollup()` and
# `load_witness_emit()` are three CONSECUTIVE statements of `run_bsp_tegra` with nothing
# between them that can legitimately decline, so a boot that prints the banner and not the
# other two has faulted in between — a genuine regression that SHOULD red. No new way for a
# healthy boot to red, in either polarity.
REQUIRE (CAPSTONE COMPLETE|\[orinbsprun\] boot core [0-9]+ joins run\(\))

# --- NEW this boot: witnesses shipped ahead of their bench (promote on capture) -----
# M1b: the first EL0 round-trip on Orin metal (tegra_el0 knob armed on the image).
REQUIRE TEGRA-EL0.*el0-hello round-trip -> PASS
# M2 step 1: the microSD becomes block-layer-visible (read-only backend).
# DEFECT 4, FIXED HERE (exec-spec, 2026-08-25). This was `REQUIRE`, promoted on capture at
# orin 3 (2026-08-22) on `capture/orin2-boot5c-gui.log:1051` — `:: TEGRA-SD: block backend
# published — 62333952 sectors (read-only) ::` — and the promotion was right about the
# EVIDENCE and wrong about the KIND. `sdmmc_tegra.rs` is `#[cfg(feature = "sdmmc")]` end to
# end and the knob is `UNAOS_SDMMC=1` (arroyo:1015), default OFF. boot5c was flown
# `knobs=TEGRA+TEGRA_EL0+RAST+SDMMC+NOJB11` (capture/line-acm0/marks.txt); boot7f was not.
# So a REQUIRE here fails on CONFIGURATION, not on health — the exact error the SMPMARK
# block below argues against — and boot7f, a completely healthy flight, has been reading
# 12/13 for it. `capture/orin2-boot4f.log` is the same story: marks.txt calls that flight
# the `nosdmmc-control`.
#
# THE FIX HAS TO KEEP BOTH HALVES, and a plain demotion keeps only one: an sdmmc-ARMED boot
# that fails to publish must still red. It was THE BLOCKER for six boots, a silent
# regression takes the installer, the native volume and `uls` with it, and the publish is
# strictly upstream of ORIN-INSTALL-2, which takes its target from the same census.
#
# mbench HAS NO CONDITIONAL REQUIRE — no `WHEN <guard> REQUIRE <rx>` (the grammar hole
# x86-witness.spec writes up), and matching is per-line, so "armed implies published" cannot
# be written as one row: the arming witness and the publish are different LINES. An
# alternation `published|<not-armed marker>` was the other candidate and is impossible for
# the same reason in reverse — an unarmed image prints NO sdmmc line at all, so there is no
# marker to key the second branch on, and tegra prints no `kernel features:` banner
# (measured: zero hits in every Orin capture) to run a lookahead against.
#
# SO THE CONDITIONAL IS BUILT OUT OF THE TWO KINDS THAT DO EXIST, and it is exact:
#   PENDING on the publish  — an unarmed image reads ⏳ and never fails; an armed image that
#                             publishes reads ✅ and mbench advises the promotion, which is
#                             the honest state of a row whose arming is an operator choice
#   FORBID on every armed-path NON-publish outcome (below) — those lines exist ONLY in the
#                             `sdmmc` build, so they cannot fire on an unarmed boot at all,
#                             and on an armed boot they name WHICH rung stopped instead of
#                             reporting an anonymous missing witness. Strictly more
#                             informative than the REQUIRE it replaces.
PENDING TEGRA-SD.*block backend published
# THE ARMED HALF. Every path an `sdmmc` build can take that reaches the recon and does NOT
# reach `publish_block_backend` prints exactly one of these, and there are six of them —
# enumerated from the source, not guessed, by walking every `return` between the entry
# banner (sdmmc_tegra.rs:2563) and the publish call (:2714):
#   :2569  `recon SKIPPED (no resolvable microSD-slot SDMMC controller)`
#   :2584  `M1: controller window … is outside the already-mapped GiB windows … — recon
#           SKIPPED (no unmapped deref)`
#   :2598  `M1: CAPABILITIES[…] = … — POISON … — recon REFUSED (no reset, no writes)`
#   :2687  `ORIN-SDMMC-1 recon done at M2 (no identified card / honest stop)`
#   :2694  `ORIN-SDMMC-1 recon STOPPED at M3 (sector-0 read failed)`
#   block.rs:1684 `TEGRA-SD: REFUSED to publish the microSD block backend — num_blocks=0`
# The first row below covers the five SDMMC-side stops, the second the block layer's refusal.
# THE SEVENTH ARM IS DELIBERATELY NOT FORBIDDEN: sdmmc_tegra.rs:85's `no Tegra234 SDMMC on
# this build (QEMU virt) — recon is metal-only` is the honest not-on-metal answer and carries
# none of the tokens below, so it cannot fire either row. Checked, not assumed.
# `recon done at M2 (no identified card)` IS FORBIDDEN AND THAT IS THE DELIBERATE PART:
# an empty slot on an armed flight is a configuration state, but it is the OPERATOR'S
# configuration on a flight that asked for the card, and the old REQUIRE red it too. This
# preserves that verdict exactly; the only behaviour that changes is the UNARMED image's.
# EM DASHES: two of the five stops put their verdict after one, so both patterns key on the
# contiguous-ASCII verdict token itself and never span the dash.
# MEASURED AGAINST THE WHOLE CORPUS, both directions. `recon (SKIPPED|REFUSED|STOPPED at
# M3|done at M2)` takes FIVE real hits and ZERO false ones — `capture/line-acm0/orin.log:114`
# and `:5772` (two armed-sdmmc flights that stopped with no card seated),
# `capture/line-acm0/raw.log` x2, `capture/orin1-boot2/boot2-recovered.log` x2,
# `capture/orin1-boot3/boot3-banked-0258.log:…` and `capture/pi4-pi1-b1/ttyACM0.log` x2 —
# all of them genuine armed stops. The bare token `recon` appears 862 times across the tree
# (`disp-userd-recon`, `recon-pre`, `recon-post`, `reconfig`, …), which is exactly why the
# verdict word is part of the pattern and not just the tag.
# ⚠ BOTH ROWS ARE UNFIREABLE ON THE 2026-09-01 STAGED PAIR, and the corpus counts above are
# the reason that is easy to miss. Measured 2026-08-31 (orin 11, SPECSCORE): every emitter
# behind them is `#[cfg(…, feature = "sdmmc")]` — the recon stops at sdmmc_tegra.rs:2569+,
# the publish/refuse pair at block.rs:1653+ — and NEITHER staged image carries `sdmmc`
# (conwin1 and supstate1 both build `…,sdhcblk,…` and no `sdmmc`). The strings `recon `
# and `TEGRA-SD` take ZERO hits in both `kernel.elf`s; the five real corpus hits cited above
# are all from ARMED-SDMMC flights, which these two are not. So a `✅ 0 hits` on either
# staged boot says nothing about the card path — it says the card path was not built. The
# rows are KEPT, not deleted: they are correct and proven for an `sdmmc` image, and this is
# a configuration gap, not a bad rule. The PENDING `TEGRA-SD.*block backend published` row
# at the head of this file is unfireable on the same pair for the same reason, so its ⏳ can
# never promote from these two flights either.
FORBID recon (SKIPPED|REFUSED|STOPPED at M3|done at M2)
FORBID TEGRA-SD: REFUSED to publish

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

# --- SMPINSTR: the load witness finally has an emission path on this board (a50358f0) --
# Before SMPINSTR the `:: SCHED: load ::` line had NEVER printed on the Orin: its only
# trigger was `timer_preempt`, which no-ops unless `SCHED_ACTIVE`, whose only aarch64
# setter is inside `main.rs`'s `baremetal` block — and `baremetal` implies `pi`, which is a
# hard `compile_error!` with `tegra`. `load_witness_poll` is the fix: a CNTPCT-rate-limited
# (~1 s) poll inside `run_capstone_boot_core`'s inner `while dispatch_next(cpu)`, beside
# `el0_refusal_rollup`, PLUS one unconditional baseline emit before the loop.
#
# THE REQUIRE STANDS ON THE BASELINE AND ON NOTHING ELSE, and it is the el0core-rollup
# argument verbatim: the baseline call is un-gated (no `#[cfg]`, no `cfg!`, no runtime
# knob — sched.rs's own doc block says "NOT FEATURE-GATED, deliberately"), it runs inside
# `run_capstone_boot_core` strictly before the drive loop, and `REQUIRE CAPSTONE COMPLETE`
# above already demands a line that prints AFTER that point (boot7h: baseline at capture
# line 14845, CAPSTONE COMPLETE at 14860). So every image that can pass this file has the
# emitter linked AND reached, and the promotion adds NO new way for a healthy boot to red.
# CAPTURED: boot7h, capture/line-acm0/orin.log:14845 —
#   `:: SCHED: load c0=--/f=never-folded c1=0%/f=3ms … c7=--/f=never-folded (ctx +0/win nofold=0x0) ::`
# EXPECT FEW LINES, NOT MANY, and read their stopping as the design: the poll lives inside
# the drive loop, `jd2_console_pump` is infinite, so after the pump is dispatched the loop
# never returns and the lines stop — boot7h carries exactly two (baseline + one poll before
# the pump). "The poll is the loop": if the drive loop dies the lines stop, and their
# stopping is the measurement. A REQUIRE >= 1 is therefore the right strength; a COUNT
# would encode the pump's dispatch timing, which is not an invariant.
# THE PATTERN KEYS ON THE `/f=` FOLD-AGE TELL, which is SMPINSTR's own wire shape: the
# pre-SMPINSTR pi-format line (`SCHED: load c0=0% c1=54% …`, no `/f=`) cannot match, so
# this row cannot be satisfied by the format the arc replaced. Both first-column forms are
# covered: `c0=--/f=` (untracked) and `c0=<n>%/f=` (tracked). Pure ASCII end to end; the
# lowercase `nofold=0x0` on the same line is a different token than the uppercase `!NOFOLD`
# tell and neither is in this pattern.
REQUIRE SCHED: load c0=(--|[0-9]+%)/f=
# THE TWO DIAGNOSTICS BESIDE IT, OPTIONAL — nothing should ever be required to print a
# state dump (the [irqel2a] rule). `[pulse5]` is the live-span line emitted with the load
# window (boot7h 14846); `!NOFOLD` is the staleness TELL — it NAMES the condition "tracked
# but not folded past the bound" and deliberately cannot distinguish a wedged core from a
# genuinely compute-bound one (sched.rs says so on the emitter), so it is a lead for a
# reader, never a verdict, and must not gate or red a flight in either direction.
OPTIONAL \[pulse5\] live c0=[0-9]+ms
OPTIONAL !NOFOLD

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
# THE KINDS: PASS is REQUIRE — promoted 2026-08-25 (exec-smallfix) at the two-trial bar
# this file itself set for TEGRA-SD: two CONSECUTIVE metal flights carry the line verbatim
# (boot7g `capture/line-acm0/orin.log:12967`, boot7h `:14822`, cpu 0 both), both flown
# AFTER IRQEL-RT2 removed the machine-global-flag artifact that produced the only FAIL
# metal ever printed (boot5c). The old objection — a REQUIRE on the PASS line "would
# convert the instrument into a rubber stamp" — was an objection to requiring the good arm
# while the FAIL arm was a live outcome of a HEALTHY boot; post-IRQEL-RT2 a FAIL means the
# ARMING core's own IRQ went up at EL2, i.e. the banked EL1 vector path is NOT live — a
# genuine regression that SHOULD red. The emitter is un-gated (`tegra` only, no runtime
# knob) on the same path the `REQUIRE IRQEL-RT: EL1 one-shot proof` arm line below already
# proves reached. THE COST, STATED: a boot printing MISS (`proof INCONCLUSIVE` — designed
# as "not a verdict") now reds via this missing REQUIRE. Accepted deliberately: on both
# flights the one-shot arrived well inside the ~100 ms window, and a metal boot where it
# does not is a flight a human adjudicates, not one a spec waves through. FAIL and MISS
# stay OPTIONAL: NOT FORBID (an honest negative reds ONCE, as this missing REQUIRE, never
# double-counted), and nobody should ever REQUIRE a failure (x86-witness.spec's `[wc-d]`
# argument, unchanged).
#
# THE HOLE, NOW CLOSED BY THE PROMOTION: pre-promotion, since none of the three failed the
# run, a boot where the proof ARMS and no verdict prints scored clean. With the PASS arm
# REQUIRE, a no-verdict boot reds on that row — the by-eye guard (arm line present + all
# three verdicts ◦) is no longer load-bearing, though mbench still has no conditional
# REQUIRE (`WHEN <guard> REQUIRE <rx>` — the grammar hole x86-witness.spec writes up).
# Patterns are kept ASCII on purpose: these lines carry em-dashes, and a DARKWIN-dropped
# byte mid-sequence would lossy-replace them.
# CAPTURE STATUS, measured against the record rather than assumed: `first IRQ taken at
# EL1` is captured TWICE (boot7g `orin.log:12967`, boot7h `:14822` — consecutive flights);
# the FAIL branch's only metal print remains boot5c (`orin.log:8311`, in its
# PRE-IRQEL-RT2 wording `taken at EL2 — NOT the EL1 proof (investigate)`, the removed
# artifact); `proof INCONCLUSIVE` and `[irqel2a]` still have ZERO hits anywhere in the
# bench tree. The FAIL pattern below deliberately matches the NEW wording only, so
# replaying boot5c against this spec reads it as ◦ — that is correct, not a miss: boot5c's
# verdict was the machine-global-flag artifact IRQEL-RT2 removed, and a spec adjudicates
# the NEXT flight.
REQUIRE IRQEL-RT: first IRQ taken at EL1 on cpu [0-9]+
OPTIONAL IRQEL-RT: one-shot proof IRQ taken at EL[0-9]+ on cpu [0-9]+ \(the ARMING core\)
OPTIONAL IRQEL-RT: EL1 one-shot NOT delivered in ~100 ms
# The arm line is REQUIRE — the block's first promotion (the PASS arm above, 2026-08-25,
# is its second). Justification, in
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

# =====================================================================================
# BOOT 7h — THE CONSOLE-WINDOW / NET FLIGHT (2026-08-25). Unlike the BOOT 7e banner
# above, everything here IS captured: boot7h (capture/line-acm0/orin.log lines
# 13159-16290, media boot7h-conwin-net4-20260825T2208Z-68c4758, SRC.SHA 68c47585) flew
# both families below and every PENDING in this block matched on it. They stay PENDING
# anyway, permanently, because both families are KNOB-GATED and default OFF —
# `orinconwin` behind `UNAOS_ORINCONWIN=1` (arroyo:922), `net4` behind `UNAOS_NET4=1`
# (arroyo:1045) — so a REQUIRE would red every unarmed boot on CONFIGURATION rather
# than on health: the SMPMARK/TEGRA-SD argument, permanently binding here. mbench's
# promotion advice on these rows is to be ignored on purpose.
# =====================================================================================

# --- ORINCONWIN (rung 4): the console as a compositor window --------------------------
# ARMED BY `orinconwin`. `orin_conwin` (display_tegra.rs) is called on `tegra_early_stop`'s
# terminus line with no runtime knob beyond the feature. TWO unconditional lines on an
# armed image, so both are PENDING (the orinwm1 shape):
#   1. THE GATE — printed above every early return, so an armed boot that prints nothing
#      else still prints it. boot7h: `[orinconwin] gate panel=1920x1200x4 stage=4194304
#      table=1 dock=GRANTED route=UNROUTED orindesk=1 orinclick=1 rows=12` (capture 14828).
#   2. THE TERMINUS — prints whenever the window opened, with the verdict derived from
#      present-outcome CROSSED with the route read back. boot7h: `[orinconwin] win=2
#      panel=1920x1200 cell=7x16 stage=4194304 table=2 present=Composited route=true
#      live=LIVE -> ROUTED` (capture 14833).
# Patterns are pure ASCII and stop before any prose. The `\[orinconwin\]` tag is
# load-bearing for the same measured reason as `\[orinwm1\]`'s: the shared fbcon path
# prints `[wc-x] console-window win=… panel=…` on the SAME boot (boot7h 14830), and an
# untagged pattern would credit rung 4 with the x86 compositor's line.
PENDING \[orinconwin\] gate panel=[0-9]+x[0-9]+x[0-9]+
PENDING \[orinconwin\] win=[0-9]+ panel=[0-9]+x[0-9]+ cell=[0-9]+x[0-9]+
# THE TWO VERDICT ARMS of the terminus, OPTIONAL on exactly the orinwm1 argument: ROUTED
# is derived (`ok && routed`), PRESENT-DECLINED is its honest negative, and neither may
# red a first flight.
OPTIONAL \[orinconwin\] win=.* -> ROUTED
OPTIONAL \[orinconwin\] win=.* -> PRESENT-DECLINED
# THE REFUSAL ARMS under their shared token — `already-armed`, `no-panel`, `ordering-rule`
# (§6.1 as a branch), `dock-cannot-host-full-strip`, `console-not-ready` (console-face),
# `open-declined`. REFUSALS, not failures: `ordering-rule` is precisely what a correct
# boot prints when `orindesk`/`orinclick` are absent from the conjunction. The pattern
# tolerates the `console-face ` infix and stops at `reason=` — several of these lines
# carry em dashes and a `§` later in their prose, and this file never puts a multi-byte
# character inside a pattern that crosses UARTC.
OPTIONAL \[orinconwin\].* DECLINE reason=

# --- ORIN-NET-4 (NET-4F / NET-4V): the RTL8168 ring discriminator and the DHCP verdict --
# ARMED BY `net4`. Two unconditional-at-window-close lines on an armed metal boot, so both
# PENDING; every verdict arm is a MEASUREMENT about the NIC/RC or the wire, so every arm
# is OPTIONAL — the JD1-DC rule: a spec that reds an honest negative answer is asking the
# hardware to BE something instead of measuring what it IS. In particular the boot7h
# conviction (`single-address latch`, buffer 17, capture 14524) is the instrument WORKING,
# and the no-lease `NO-OFFER` verdict (capture 14529) is an answer about the wire/server,
# not a regression in UnaOS. No FORBID in this block, deliberately: mbench's default
# `-> FAIL` scan already covers any future arm that chooses to spell failure.
#   THE BRING-UP TERMINUS. Prints whether the lease arrived or the static fallback bound —
#   boot7h took the fallback and still printed it (capture 14541). Pattern stops before
#   the em dash on the wire (`ORIN-NET-4 DONE — RTL8168 driver up …`).
PENDING ORIN-NET-4 DONE
#   THE RING-PASS ORACLE at window close, three mutually exclusive readings (wraps>0 |
#   RDU-clears>0 | both zero); boot7h read `wraps=0 RDU-clears=0` — the un-serviced-latch
#   theory REFUTED on the wire (capture 14528).
PENDING \[net4F\] RX ring pass verdict:
#   THE TAG-DISCRIMINATOR'S CONVICTION ARM — fires only when consecutive completions all
#   land in ONE named wrong buffer (boot7h: buffer 17, capture 14524). A healthy NIC
#   never prints it; a PENDING would advise promoting a defect signature to REQUIRE.
OPTIONAL \[net4F\] VERDICT tag-proven single-address latch
#   THE CHIP-ID CROSS-CHECK (MATCH/MISMATCH read by eye — boot7h: xid=0x541 MATCH,
#   capture 14502). Pattern stops at the tag; the verdict brackets sit before an em dash.
OPTIONAL \[net4F\] MAC chip id:
#   THE ONE-LINE NO-LEASE VERDICT (NET-4V), emitted ONLY when the DHCP window closes
#   without a lease — a leased boot legitimately never prints it, so no PENDING umbrella
#   (the MODEL-VERDICT argument above, in the other direction).
OPTIONAL \[net4V no-lease verdict\]

# =====================================================================================
# BOOT 7i — THE METAL BATCH, ARMED AHEAD OF ITS CUT (2026-08-25, exec-boot7iprep).
# Unlike the BOOT 7h banner above, NOTHING here is captured: every pattern is a
# PREDICTION about a line that has NEVER printed on this bench. Measured, not assumed —
# all nineteen patterns below take ZERO hits across the whole bench capture tree (277
# log/txt files, ~2.5M lines, scanned with the compiled patterns themselves), and the
# boot7h green-reference slice (orin.log 13159-16290) carries none of the five family
# tags, so replaying boot7h against this file is UNCHANGED by this block: 17/17, 0
# forbidden, the new rows all ⏳/◦. That is the negative control, run before commit.
# Every string below was read out of the SOURCE (power.rs, wdt_tegra.rs,
# selfup_tegra.rs, the timer.rs tail, rtl8168_tegra.rs) — the boot7i image is not yet
# cut, so there is no staged artifact to grep; the artifact-side check (each family's
# token in the staged kernel.elf, `LC_ALL=C grep -a`) is owed at cut time with the
# MANIFEST. Rows arm as PENDING/OPTIONAL now and promote on the boot7i capture, per
# promote-on-evidence.
# REQUIRED/FORBIDDEN COUNTS ARE UNCHANGED BY THIS BLOCK, BY CONSTRUCTION. Four of the
# five families are knob-gated, default OFF — `orinwdt` (UNAOS_ORINWDT=1, arroyo:838),
# `selfup` (UNAOS_SELFUP=1, arroyo:963), `bsptick` (UNAOS_BSPTICK=1, arroyo:735), and
# [net4G] rides `net4` (UNAOS_NET4=1, arroyo:1045) — so a REQUIRE on any of them would
# red every unarmed boot on CONFIGURATION rather than on health: the SMPMARK/TEGRA-SD
# argument, permanently binding. mbench still has no conditional REQUIRE (`WHEN <guard>
# REQUIRE <rx>`, the x86-witness.spec grammar hole), so "REQUIRE on an armed boot" is
# spelled the only way the grammar can spell it: PENDING on the lines an armed boot
# prints unconditionally — the TEGRA-SD idiom exactly. The fifth family (the power
# VERBS) is OPERATOR-DRIVEN: an unattended boot legitimately prints none of it — the
# [orinclick]-edge argument, so OPTIONAL end to end.
# TOKEN LENGTHS, per the >8-byte immediate-encoding trap the timer.rs tail writes up:
# `[pwrreboot]` 11 B, `[pwrshutoff]` 12 B, `[orinwdt]` 9 B, `[orinselfup]` 12 B, `[orinbsptick]` 14 B —
# all artifact-grep-able as bare tags. `[net4G]` is EXACTLY 8 bytes, at the LLVM bound:
# grep the staged artifact for the longer fragment `latch-site status` (17 B, same
# format literal) rather than the bare tag when doing the cut-time check.
# =====================================================================================

# --- ORIN-REBOOT (watchdog half): the TKE boot watchdog, ARMED/DISARMED pair ---------
# ARMED BY `orinwdt`. Both lines are UNCONDITIONAL on an armed boot that completes:
# `boot_arm()` fires at main.rs:2064 (right after `exceptions::install`, EL2, MMU device
# window live) and prints `wdt ARMED` with read-backs; `boot_ok_disarm()` fires on the
# EL1 terminus line (main.rs:2679, strictly BEFORE `run_capstone_boot_core`) and prints
# `wdt DISARMED`. So on an armed image, any boot that satisfies `REQUIRE CAPSTONE
# COMPLETE` has printed BOTH — the orinconwin gate/terminus shape, PENDING both.
# THE PAIR IS THE MEASUREMENT: ARMED without DISARMED on a capture that then goes dark
# is the watchdog ABOUT TO FIRE — a wedged boot self-resetting, which is the instrument
# WORKING, and it reds through the missing REQUIREs downstream, never through these
# rows. No FORBID is writable for it: the signature is an absence, and mbench forbids
# lines, not absences.
# EM DASHES: both lines put one immediately after the verdict token (`ARMED — POR reset
# in …` / `DISARMED — boot reached …`), so both patterns stop at the token and never
# span the dash. `wdt ARMED` cannot false-hit the DISARMED line (`DISARMED` does not
# contain ` ARMED` after `wdt `), and both carry the 9-byte `[orinwdt]` family tag (its own family since PWRNAME — the watchdog is a POR mechanism, not the reboot verb).
PENDING \[orinwdt\] wdt ARMED
PENDING \[orinwdt\] wdt DISARMED

# --- ORIN-REBOOT / ORIN-SHUTOFF (verb half): the power verbs at the shell ------------
# NOT knob-gated (power.rs compiles on every build; the aarch64 non-pi arm is the PSCI
# one) but OPERATOR-DRIVEN: the only callers are the shell verbs `reboot` and
# `shutdown`/`off` (shell.rs:4041) and selfup's S6 hook below. An unattended boot prints
# none of these, so every row is OPTIONAL — the [orinclick]-edge argument verbatim:
# absence means UNRUN, never failed.
# THE SUCCESS SIGNATURE IS SILENCE, and that must be read from the wire shape, not from
# a row: on success the PSCI dispatch line (`… via SMC — firmware owns the machine from
# here`) is the LAST line this kernel ever prints — SYSTEM_RESET warm-resets into
# firmware chatter, SYSTEM_OFF goes DARK and stays dark. A dark board after the
# `[pwrshutoff]` PSCI line is the shutdown verb PASSING (Peter's cold-boot ruling:
# the dark board IS the cold-boot-ready signal), not a hang. No spec row can assert
# "nothing printed after"; the playbook carries that reading.
# THE `RETURNED` ARMS ARE HONEST REFUSALS, NOT FAULTS: a returning PSCI call is the
# firmware declining (negative return per DEN0022) and the kernel parking in hlt with
# the machine's refusal stated — a measurement about ATF, not a UnaOS invariant break,
# so OPTIONAL and deliberately not FORBID (the JD1-DC rule). Patterns stop before each
# line's em dash; `{:#010x}` renders lowercase `0x…`, hence `0x[0-9a-f]+`.
OPTIONAL \[pwrreboot\] reboot verb invoked
OPTIONAL \[pwrreboot\] PSCI SYSTEM_RESET \(0x[0-9a-f]+\) via SMC
OPTIONAL \[pwrreboot\] PSCI SYSTEM_RESET RETURNED
OPTIONAL \[pwrshutoff\] shutdown verb invoked
OPTIONAL \[pwrshutoff\] PSCI SYSTEM_OFF \(0x[0-9a-f]+\) via SMC
OPTIONAL \[pwrshutoff\] PSCI SYSTEM_OFF RETURNED

# --- ORIN-SELFUP: the staged-payload self-update ladder (S0..S6) ---------------------
# ARMED BY `selfup` (main.rs:2479 — after ORIN-INSTALL-2's slot, before the JM6 drop).
# S0 IS THE ONLY UNCONDITIONAL LINE: `selfup_service` opens with the S0 scan banner on
# EVERY armed boot — payload or no payload, mountable volume or not, every arm of the
# S0 match prints an `S0 scan` line (selfup_tegra.rs:108/114/122/131/137/143). So S0 is
# PENDING (the gate-line shape) and EVERYTHING ELSE IS PAYLOAD-CONDITIONAL: a normal
# armed boot with no UPDATE.PAK prints S0 alone, and that is the healthy common case —
# S1..S6 on such a boot would be a defect, which is why none of them can be PENDING
# (promoting a payload-conditional line reds every payload-less armed boot).
PENDING \[orinselfup\] S0 scan
# THE LADDER, one row per stage so the table names how far an update got — read TOP-DOWN
# on an update boot; the deepest ✅ is the stage the ladder reached. Wire order on a
# successful update: S0 (staged) -> S1 verify -> S2 parse -> S3 write (once PER FILE) ->
# S4 flip (window OPEN / per-pair `is live` / window CLOSED) -> S5 clean ->
# UPDATE APPLIED -> S6 reboot -> the [pwrreboot] verb pair above -> RESET (silence,
# then firmware chatter — the boot that follows is the UPDATED kernel). S6's line is
# NOT the last before reset: the [pwrreboot] PSCI SYSTEM_RESET dispatch line is.
# Every stage token is followed by an em dash on the wire; every pattern stops at the
# stage word. The REFUSED line (`UPDATE REFUSED — S2 parse — …`) cannot false-hit the
# ladder rows: the rows anchor the stage token directly after the family tag.
OPTIONAL \[orinselfup\] S1 verify
OPTIONAL \[orinselfup\] S2 parse
OPTIONAL \[orinselfup\] S3 write
OPTIONAL \[orinselfup\] S4 flip
OPTIONAL \[orinselfup\] S5 clean
OPTIONAL \[orinselfup\] S6 reboot
# THE TWO TERMINAL ARMS. APPLIED is the success verdict; REFUSED is the fail-closed core
# WORKING (bad sha, bad magic, short read, missing pair — live boot set untouched, the
# machine boots its old self normally). An honest refusal must not red the flight — the
# JD1-DC rule — and no FORBID is added: none of selfup's refusal text carries ` FAIL`
# (checked against the module, zero hits), so mbench's default FORBIDs stay silent too.
# On a payload boot the reader adjudicates by eye: APPLIED ✅ + REFUSED ◦ is the round
# trip; REFUSED ✅ is the finding to quote whole, reason and all.
OPTIONAL \[orinselfup\] UPDATE APPLIED
OPTIONAL \[orinselfup\] UPDATE REFUSED

# --- ORIN-BSPTICK: the standing periodic EL1 tick across the terminus ----------------
# ARMED BY `bsptick`. Both lines are UNCONDITIONAL on an armed boot: `el1_bsptick_start`
# is called on the terminus line (main.rs:2679, right after `el1_oneshot_proof`) and
# prints the arming banner BEFORE the unmask; the per-tick witness prints tick 1 (the
# arm-delivered proof) and then every TICK_HZ-th tick (~1/s) FOREVER — unlike SCHED:
# load's poll, this emitter lives in IRQ context off the timer itself, so the lines do
# NOT stop when the drive loop dispatches the pump. PENDING both (the wdt-pair shape).
# THE COUNT ADVANCING IS THE MEASUREMENT — periodic, not one-shot — and the EL is
# re-measured per emission (an EL2 reading would mean HCR_EL2.IMO regressed, printed
# rather than hidden; the pattern deliberately matches ANY EL digit so a regressed line
# still lands in the table where a reader sees the digit). The banner's em dash sits
# after `(… Hz, PPI…)`; both patterns stop well before their line's first non-ASCII.
PENDING \[orinbsptick\] arming PERIODIC CNTP at EL[0-9]+ on cpu [0-9]+
PENDING \[orinbsptick\] tick [0-9]+ taken at EL[0-9]+ on cpu [0-9]+

# --- NET-4G: the latch-SITE discriminator (rides `net4`, self-gating) ----------------
# THE TAG IS CASE-SENSITIVE AND THAT IS LOAD-BEARING, measured: lowercase `[net4g]` is
# the OLDER NET-4g RX-descriptor dump (rtl8168_tegra.rs:1976), which the boot7h slice
# already carries 9 times. mbench compiles patterns with a bare `re.compile` (no
# IGNORECASE), so the uppercase rows below cannot credit the dump — do not "fix" the
# case. The experiment itself SELF-GATES on a conviction: `net4g_arm` runs only when the
# [net4F] tag-discriminator has just proven a single-address latch (the boot7h
# conviction, buffer 17), so on a healthy-NIC armed boot the DECOY never arms and only
# the status line prints.
# THE STATUS LINE IS THE UNCONDITIONAL ONE (rtl8168_tegra.rs:2298): it prints at EVERY
# armed window close — concluded, aborted, armed-but-unresolved, or never-armed are its
# four arms, all legitimate — so the window close is never silent about the experiment.
# PENDING (the ORIN-NET-4 DONE shape above, and the same permanent no-promote rule).
# Pattern stops at the colon: the arm texts carry em dashes.
PENDING \[net4G\] latch-site status:
# THE CONVICTED-BOOT LINES, all OPTIONAL — each fires only downstream of a [net4F]
# latch conviction, which a healthy NIC never produces (the boot7h-block argument for
# `VERDICT tag-proven single-address latch`, inherited whole):
#   DECOY ARMED  the victim-slot rewrite, with the full verdict vocabulary pre-stated
#                on the line itself; prints once per boot at most.
#   VERDICT      the victim completed and exactly one of seven arms names the site —
#                RC-SIDE | NIC-SIDE | RC-PAGE | PREFETCH-DEPTH | UNDECIDED-CLEARED |
#                UNDECIDED-CONTAMINATED | UNDECIDED-NO-LANDING. Every arm is a
#                MEASUREMENT about NIC/RC silicon (the JD1-DC rule): no arm may red the
#                flight, no arm is a REQUIRE, and the UNDECIDED arms are honest re-fly
#                verdicts, not failures. `[A-Z-]+` covers all seven; the meaning prose
#                after the final em dash is never part of the pattern.
#   arm ABORTED  no NIC-owned victim in reach at conviction time — UNRESOLVED, re-fly.
#   interim pop  the between-arm-and-verdict attribution bookkeeping, one per pop.
OPTIONAL \[net4G\] DECOY ARMED: victim slot [0-9]+
OPTIONAL \[net4G\] VERDICT latch-site=[A-Z-]+
OPTIONAL \[net4G\] arm ABORTED:
OPTIONAL \[net4G\] interim pop slot=[0-9]+

# =====================================================================================
# BOOT 7j — THE FOCUSED SCHEDULER/PUMP FLIGHT, ARMED AHEAD OF ITS CUT
# (2026-08-25, exec-o7-boot7jspec). boot7j exists because two arcs are each a REAL new
# risk on this board's INTERACTIVE surface, and flying them inside the boot7i batch
# would muddy every verdict in it:
#   ORIN-BSPRUN  (`bsprun`, UNAOS_BSPRUN=1 — ARMS `bsptick` TOO, arroyo folds it in and a
#                `compile_error!` at sched.rs's tail refuses the split) — the boot core
#                joins `run()`. The FIRST PREEMPTION EVER on this board's interactive
#                surface: terminus swap `run_capstone_boot_core(0)` -> `run_bsp_tegra(0)`,
#                the first tegra `SCHED_ACTIVE` setter, and a post-EOI `timer_preempt` arm
#                in `gic::handle_irq_v3` — which was ABSENT there, so the setter alone
#                would have been inert.
#   ORIN-SUPSTATE (`supstate`, UNAOS_SUPSTATE=1, implies `tegra`) — the FIRST RESTRUCTURE
#                of the one working pump: the console surface lifts out of
#                `jd2_console_pump`'s stack into `display_tegra::SUP_SURFACE`, and the pump
#                splits into jd2-console (input) / jd2-dispatch / jd2-present, all still
#                pinned to core 0.
#   SMPMARK      armed as always. IT NEEDS NO NEW ROW: its three marks already have rows
#                in the SMPMARK block above, they are emitted at PSCI wake time (strictly
#                pre-terminus), and neither arc touches that path. Checked, not assumed.
#
# EVERY PATTERN IN THIS BLOCK WAS READ OUT OF THE EMITTING SOURCE at hw-jetson cd568275,
# not out of the commit prose — `sched.rs` tail (`run_bsp_tegra`, sched.rs:10378), the
# `timer.rs` `cfg!` truth-split (:714/:716), `display_tegra.rs` (`sup_install`, :3071),
# `main.rs` (`jd2_supstate_phase2`'s roles line, :7088), and the load train's own emitters
# (`load_witness_emit` :8441, `pulse5_witness` :8527, `spread4_witness` :9009,
# `el0live_witness` :8696, `prio_witness` :8589, the WEDGE-4 tripwires :5085 / :1494).
# BOTH ARCS ARE ALREADY IN THE TREE (SUPSTATE at b9e59a54, BSPRUN at ea182855 — the exec
# branches' shas were rebased on landing), so the strings below are the tree's, not a
# branch's. The boot7j image is not yet cut; the artifact-side check (each family's token
# in the staged kernel.elf, `LC_ALL=C grep -a` — never `strings`, whose default -n 4 drops
# short marks) is owed at cut time with the MANIFEST.
#
# TOKEN LENGTHS, per the >8-byte immediate-encoding trap: `[orinbsprun]` 12 B,
# `[supstate]` 11 B, `[spread4]` 10 B, `[el0live]` 10 B, `[wedge4]` 9 B, `[spin1]` 8 B and
# `[prio]` 7 B — the last two are AT OR UNDER the LLVM immediate-encode floor, so an
# artifact grep for either must use the longer fragment from its own format literal
# (`one task has owned this core the whole span` for `[spin1]`, `defer= agedin=`'s
# neighbourhood for `[prio]`), never the bare tag. This is the `[net4G]` trap, restated
# because it has already bitten this file once.
#
# WHAT MOVES A COUNT, STATED UP FRONT RATHER THAN DISCOVERED IN THE TABLE:
#   REQUIREs stay at 17. One row is WIDENED (`CAPSTONE COMPLETE` -> the terminus
#     alternation, argued in full at the row, in the scheduler block above); no REQUIRE is
#     added, because every line in this block is behind a knob this file does not force —
#     the SMPMARK/TEGRA-SD argument, permanently binding.
#   Spec-declared FORBIDs go 9 -> 10: `[wedge4] preempt-in-section`, argued at the row. It
#     is the ONE addition here that can red a flight, it takes ZERO hits across the whole
#     bench tree, and ORIN-BSPRUN is the arc that makes it reachable on tegra for the FIRST
#     TIME (it fires only from inside `timer_preempt`).
#   PENDINGs go 15 -> 20, OPTIONALs 60 -> 68. (Row census, measured on the file itself:
#     REQUIRE 17, PENDING 20, OPTIONAL 68, FORBID 10 spec-declared + 3 mbench defaults,
#     COMPLETE 1.)
# boot7h REMAINS THE GREEN REFERENCE and this block does not move it: replayed against this
# file it reads 17/17, 0 forbidden, pending 8/20 — the five new PENDINGs all ⏳, two of the
# eight new OPTIONALs ✅ (the cooperative-terminus tell and the JD2 key echo, both of which
# an unarmed attended flight legitimately prints), six ◦. That is the negative control, run
# before commit.
#
# READ THIS BEFORE FLYING BOTH KNOBS TOGETHER — it is a FINDING, not a row. ORIN-BSPRUN's
# soundness derivation for video/WM locks is CONDITIONAL, and it names its own condition:
# "the pump can now be preempted while holding one, but on this terminus every other core-0
# task touches no video state, so there is no contender to deadlock with today; the arc that
# adds a second video-touching task to core 0 must re-derive this". ORIN-SUPSTATE ADDS
# EXACTLY TWO SUCH TASKS: `jd2-present` and `jd2-dispatch` both enter `sup_with_surface` and,
# through the PAL, reach `video::WRITER` at a BARE `lock()` (pal.rs:565). A combined
# boot7j image is therefore OUTSIDE the derivation BSPRUN's own commit wrote, and the
# re-derivation that commit demands has not been written anywhere in the tree. The
# mitigating half, also stated: with preemption armed a bare-lock spin is broken by the
# quantum, so the shape is a spin-storm and not the hard livelock the SUPSTATE header
# feared on a non-preemptive core — but "we think it degrades gracefully" is not a
# derivation, and no row in this file can adjudicate it. Fly the knobs SEPARATELY first if
# the flight can afford two boots.
#
# WHAT THIS BLOCK COULD NOT WRITE A ROW FOR — the honest list, because an unadjudicable
# change is worth more stated than papered over with a plausible row:
#   1. `jd2-present` HAS NO WIRE VOICE AT ALL. The presenter (main.rs:7218) contains not one
#      `serial_println!`: it drains the frame board, owns the save-under cursor composite and
#      every `pal.render()`, and says nothing. A DEAD PRESENTER IS FROZEN GLASS AND A
#      PERFECTLY HEALTHY-LOOKING SERIAL LOG. This spec cannot see it; only the panel can.
#      It is the single largest blind spot ORIN-SUPSTATE introduces.
#   2. THE ROLE SPAWNS THEMSELVES ARE INVISIBLE. `sched::spawn`'s `:: SCHED: task '<name>' ->
#      core <n> … ::` witness is `#[cfg(feature = "pi")]` (sched.rs:3681), so on tegra the
#      `jd2-present` / `jd2-dispatch` spawns print nothing. The `[supstate] roles` row below
#      proves `spawn` RETURNED (it is printed after both calls) and nothing more — it is not
#      evidence that either task was ever DISPATCHED.
#   3. THE 64-DEEP KEY SEAM HAS NO DROP WITNESS. The input source checks `sup_key_full()`
#      BEFORE each pop, so a backed-up dispatcher leaves keys in the PAL ring rather than
#      dropping them — but if the PAL ring then overflows the loss is silent on this wire.
#   4. A `SUP_SURFACE` LOCK-ORDER INVERSION IS A LIVELOCK, NOT A DEADLOCK, and livelocks are
#      absences. Every acquisition in the module is `try_lock` + `yield_now`, so an inverted
#      order does not hang the core, it spins the roles against each other forever. Its
#      signature is "the load train keeps printing while the `:: tegra: JD2 —` transcript
#      stops" — mbench forbids LINES, not absences, so no FORBID is writable. `[spin1]` does
#      not catch it either: a yielding task accumulates no live span.
#   5. PREEMPTION HAS NO COUNTER ON THE WIRE. `(ctx +N/win …)` on the `SCHED: load` line
#      aggregates cooperative yields and quantum preemptions into one number and cannot
#      separate them, so "how many times did the pump actually get preempted" is not a
#      question this spec can put to a capture. `[spread4]`'s presence answers only the
#      binary "at least once".
#   6. THE ARC'S OWN HEADLINE CLAIM IS A PANEL READING. ORIN-BSPRUN's commit says the point
#      is "a keystroke and a click surviving quantum expiry"; a keystroke's SURVIVAL shows on
#      serial (the KEY-echo row below), a click's does not, and neither says whether the
#      glass kept up. The playbook carries that half.
# =====================================================================================

# --- ORIN-BSPRUN, half 1: WHICH TERMINUS DID THIS BOOT ACTUALLY TAKE? ----------------
# THE PAIR IS THE MEASUREMENT, and it is the cleanest binary in this whole file: exactly
# ONE of the next two lines prints on any tegra boot, because main.rs:2679 selects between
# them with `#[cfg(not(feature = "bsprun"))]` / `#[cfg(feature = "bsprun")]` on the same
# statement. The banner is UNCONDITIONAL on an armed boot that reaches the terminus and is
# printed BEFORE the `SCHED_ACTIVE` store (deliberately — so it cannot itself be the first
# preempted print), which is why it is a PENDING of the gate-line shape and not an OPTIONAL.
# THE BROKEN SHAPES THIS PAIR NAMES:
#   banner ✅, cooperative tell ◦   the armed terminus ran. Read half 2 next.
#   banner ◦, cooperative tell ✅   THE KNOB DID NOT TAKE. Either the image was not built
#                                  with `bsprun` (check the MANIFEST — this is the common
#                                  and boring cause) or `run_bsp_tegra` was not reached.
#                                  Everything in half 2 is then meaningless and reads ⏳,
#                                  which is the correct verdict, not a failure.
#   BOTH ◦                         the boot never reached the terminus at all. The missing
#                                  REQUIREs upstream say where it stopped; this pair adds
#                                  nothing and must not be read as a scheduler fault.
#   BOTH ✅                         IMPOSSIBLE on one boot from one image — a merged or
#                                  multi-flight capture (the `line-acm0/raw.log` hazard the
#                                  half-2 note explains). Re-slice and re-run.
# The pattern stops at `run()` — the wire text continues `— SCHED_ACTIVE=true, …` past an
# em dash — and `run\(\)` is contiguous ASCII with the 12-byte family tag in front of it.
# ZERO hits across the whole bench capture tree (313 files, 2,383,287 lines) and zero on
# the boot7h slice; the cooperative tell takes 6 hits on `capture/line-acm0/orin.log` (one
# per Orin flight on that wire) and 1 on the boot7h slice, which is exactly what it should.
# ⚠ AND IT CANNOT PROMOTE FROM THE 2026-09-01 STAGED PAIR. `run_bsp_tegra` is
# `#[cfg(all(feature = "tegra", feature = "bsprun"))]` (sched.rs:10447) and NEITHER staged
# image carries `bsprun`; the token `orinbsprun` takes ZERO hits in both `kernel.elf`s
# (measured 2026-08-31, orin 11). So a ⏳ here on boot A or boot B is CONFIGURATION, never
# evidence, and the paired reading above must not be entered at all for those two flights.
# The `REQUIRE` at the head of this file that names this token is unaffected — it is an
# alternation whose OTHER arm, `CAPSTONE COMPLETE`, is present in both images and is what
# will satisfy it.
PENDING \[orinbsprun\] boot core [0-9]+ joins run\(\)
OPTIONAL running the full M4 CAPSTONE cooperatively
# THE SECOND, INDEPENDENT REGIME TELL — and it is worth having BOTH because it is emitted
# from a different file by a different arc. ORIN-BSPTICK's arming banner (timer.rs:705, in
# `el1_bsptick_start` at :693) had its regime clause `cfg!`-split by ORIN-BSPRUN precisely
# so it would stop claiming "no
# preemption" on a bsprun image, so the two arms below are a second binary that cannot lie:
#   `dispatch is on_tick + post-EOI timer_preempt`  the v3 dispatch CARRIES the arm
#   `dispatch is on_tick ONLY (no timer_preempt …)` arc 1 alone — tick, no preemption
# Both arms live INSIDE the `all(tegra, bsptick)` banner, so a bsptick-less image prints
# NEITHER and both read ◦ — which is why the second is OPTIONAL and not the PENDING its
# unconditionality would otherwise earn: on the boot7i/boot7j knob set `bsprun` implies
# `bsptick`, so an armed boot7j boot prints the FIRST arm unconditionally (PENDING) while
# the second arm belongs to a configuration boot7j is not flying (OPTIONAL).
# CROSS-CHECK AGAINST HALF 1, and this is the row's real value: banner ✅ with `on_tick
# ONLY` ✅ would mean `run_bsp_tegra` ran while the gic.rs arm was compiled out — a
# feature-list split that the `compile_error!` is supposed to make unbuildable. If that
# combination ever appears on a wire, the backstop has a hole and it is a finding, not a
# flight result. Both patterns are contiguous ASCII well before their line's first em dash.
# ZERO hits tree-wide for both.
PENDING dispatch is on_tick \+ post-EOI timer_preempt
OPTIONAL dispatch is on_tick ONLY \(no timer_preempt arm

# --- ORIN-BSPRUN, half 2: DID PREEMPTION ACTUALLY DELIVER, OR IS THE FLAG INERT? -----
# THE FAILURE MODE THIS HALF EXISTS FOR. `SCHED_ACTIVE` true with no timer arm, or a timer
# arm that never fires, gives a flight that LOOKS armed — banner ✅, half 1 green — and has
# taken exactly zero preemptions. Nothing in half 1 can tell that apart from a working one,
# and "the image says preemptive" is not "the board preempted".
#
# THE DISCRIMINATOR IS `[spread4]`, AND THE REASON IS A LINK-TIME ONE, MEASURED IN-TREE.
# `spread4_witness` has exactly one steady-state caller, `load_witness_tick` (sched.rs:8346),
# whose only caller is `timer_preempt`, which returns immediately unless `SCHED_ACTIVE`.
# Before ORIN-BSPRUN nothing on a tegra image reached it at all: sched.rs:3401 records the
# `LC_ALL=C grep -a` over the LINKED `arm-tegra-el0` kernel that found no `[spread4] live`
# in the image — "the string is in the rlib and the linker drops it, because every caller of
# `spread4_witness` is unreachable on that configuration". ORIN-BSPRUN's post-EOI arm is what
# makes that caller reachable. So `[spread4] live` on an Orin wire is not merely evidence
# that preemption fired — before this arc it could not have been PRINTED, and the same
# reachability change is what turns `[el0live]` and `[prio]` live on this board.
# CONSEQUENCE WORTH STATING SEPARATELY: the sched.rs:3401 and `EL0_REFUSALS` notes both say
# `[spread4]` is link-time dead on tegra and that `[el0core] rollup:` exists because of it.
# THAT CLAIM IS TRUE ONLY KNOB-OFF. On a bsprun image `[spread4]` carries `el0refuse=` again,
# and the two readers will then disagree only if one of them is broken.
#
# THE FALSE-POSITIVE HAZARD IS REAL AND IS MEASURED, because these three are CROSS-PLATFORM
# strings and the Pi prints them constantly. Across the bench tree: `[spread4] live c0=`
# 32,337 hits, `[el0live] verdict=` 11,837, `[prio] svc=` 45,113 — overwhelmingly Pi. Scoped
# to genuinely-Orin captures the picture is the one the rows need: `capture/line-acm0/orin.log`
# — the Orin scoring wire, 211 `tegra: JD1` lines — carries ZERO of all three, as does every
# other single-board Orin capture and the boot7h slice. The 2,846 / 5,466 / 3,990 "Orin-ish"
# hits are ALL in files that are not one Orin boot: `line-acm0/pi.log` and `line-acm0/unknown.log`
# (the bridge directory is SHARED between benches), `line-acm0/raw.log` (both boards merged), and
# `orin1-boot2/boot2-recovered.log` (7 tegra lines against a Pi session — `[spin1] … task=99:input`
# on `cpu=3` is the Pi's task on the Pi's 4-core geometry). NEVER REPLAY THIS FILE AGAINST
# `raw.log`: it would read another board's preemption as this board's. Score a per-flight LINE
# RANGE of `orin.log`, which is this file's standing convention anyway.
# PENDING, AND PERMANENTLY NO-PROMOTE — the TEGRA-SD rule. It is knob-gated by `bsprun`; a
# REQUIRE would red every unarmed boot on CONFIGURATION. mbench will advise the promotion on
# the first armed capture; the advice must not be taken, ever.
PENDING \[spread4\] live c0=[0-9]+/[0-9]+
# THE TWO COMPANIONS THE SAME TICK CHAINS. `[el0live]` is chained UNCONDITIONALLY from
# `load_witness_tick` (before the change-suppression, deliberately — sched.rs:8342 says why);
# `[prio]` is chained only when the load line actually printed. Neither is ever a gate: they
# are state dumps, and nothing should be REQUIRED to print one (the `[irqel2a]` rule). They
# are OPTIONAL rather than PENDING for a second reason too — both are change-suppressed, so a
# genuinely quiet armed board can legitimately print `[spread4]` and not these, and a PENDING
# would advise promoting a line whose absence is legal.
# WHAT THEY BUY THE READER: `[el0live] verdict=` names whether the EL0 fleet is NONE / LIVE /
# STARVED / EXTINCT under preemption, which is the first thing to check if `el0-hello` stops
# round-tripping; `[prio] svc=`'s per-window deltas say who WON the dispatches the new quantum
# started handing out. Every arm of both is a MEASUREMENT, never a failure (the JD1-DC rule).
OPTIONAL \[el0live\] verdict=[A-Z]+ el0 runnable/parked/committed=
OPTIONAL \[prio\] svc=[0-9]+ el0=[0-9]+ defer=[0-9]+ agedin=[0-9]+ /win
# THE WEDGE NAMER, AND THE ONE ROW THAT ADJUDICATES "PREEMPTION FIRED AND KILLED THE BOARD".
# `[spin1]` fires from `pulse5_witness` when a core has been inside ONE task for >10 s while
# the witness still runs. Knob-off it is unreachable in practice — the cooperative terminus
# emits the load train exactly TWICE (baseline + one poll before the infinite pump is
# dispatched; the boot7h slice is the proof, and the SMPINSTR block above says so), and two
# passes cannot observe a 10 s span. With `bsprun` the train is IRQ-driven and runs FOREVER,
# so `[spin1]` becomes a live instrument on this board for the first time.
# IT IS A LEAD, NEVER A VERDICT — the `!NOFOLD` rule verbatim: sched.rs says on the emitter
# that a genuinely compute-bound core holding one task for 12 s prints it too, and from
# outside the core those two states are not distinguishable. So OPTIONAL, and no FORBID.
# HOW TO READ IT ON THIS FLIGHT: `task=<id>:jd2-console` / `:jd2-dispatch` / `:jd2-present`
# beside a JD2 transcript that has stopped is the interactive-surface wedge boot7j exists to
# catch. `sched phase=` and `passes=` on the same line say whether the CORE's own loop is
# still turning (frozen = the dispatch/resume path; advancing = `current` is a lie).
OPTIONAL \[spin1\] cpu=[0-9]+ span=[0-9]+ms task=[0-9]+:
# WEDGE-4 W4-B, the rq-acquisition stall namer (sched.rs:1494, emitted ONCE per stalled
# acquisition at `RQ_STALL_SPINS`; the caller then keeps spinning, so behaviour is unchanged
# and this only makes a silent wedge legible). ORIN-BSPRUN is what makes rq contention real
# on this board: core 0 joins `ONLINE_MASK` and becomes a legal steal target for the first
# time. OPTIONAL and deliberately not FORBID — a recovered stall is a lead about contention,
# not an invariant break; its FORBID-worthy sibling is the next row and they must not be
# confused. Written through `w4_str` (raw port writes, `\r\n`-delimited), which mbench splits
# on; contiguous ASCII.
OPTIONAL \[wedge4\] RQ STALL core=[0-9]+
# WEDGE-4 W4-A — THE FIX'S OWN TRIPWIRE, AND THE ONE COUNT THIS BLOCK MOVES (spec-declared
# FORBIDs 7 -> 8). It prints from INSIDE `timer_preempt` (sched.rs:5085) when a timer IRQ
# landed while this core was inside a run-queue section. Every rq section runs IRQ-masked
# (the WEDGE-4 `rq` law), so the word it reads MUST be zero there; sched.rs states the
# invariant on the emitter — "a line from it means the discipline has been breached again,
# i.e. some acquisition reached the lock without masking". That is an invariant break, not a
# measurement, so FORBID is the right kind and not the `!NOFOLD`/`[spin1]` treatment above.
# IT CANNOT FIRE ON ANY BOOT THIS FILE HAS EVER ADJUDICATED. `timer_preempt` returns at its
# first line unless `SCHED_ACTIVE`, whose only other aarch64 setter is the Pi-only
# `start_aps`; ORIN-BSPRUN is the first tegra setter this flag has ever had. Measured: ZERO
# hits across the bench tree (313 files, 2,383,287 lines) and zero on the boot7h slice, so
# arming it costs every existing capture nothing and it can only ever speak about a bsprun
# flight. Rate-limited at `W4A_PRINT_MAX`, so a breached boot reds on the first few and then
# goes quiet — the count is not the evidence, the presence is.
# ⚠ CORRECTION, measured 2026-08-31 (orin 11, SPECSCORE). The paragraph above says this row
# cannot fire because `timer_preempt` returns at its first line without `SCHED_ACTIVE`. The
# RUNTIME argument is right but it is not the operative one, and the difference matters to
# anyone who tries to check this row against an artifact: the string `preempt-in-section`
# is NOT IN EITHER STAGED IMAGE'S `.rodata` AT ALL — zero hits in both `conwin1-…-93825ea`
# and `supstate1-…-93825ea` — while its sibling `[wedge4] RQ STALL core=` (the OPTIONAL row
# above) is present once in each. The emitter is not merely un-driven, it is OPTIMISED AWAY:
# with no tegra `SCHED_ACTIVE` setter compiled in — `run_bsp_tegra` is
# `#[cfg(all(tegra, bsprun))]` and neither staged image carries `bsprun`, and the only other
# aarch64 setter, `start_aps`, is the Pi's — the whole guarded block is unreachable and LLVM
# drops it. CONSEQUENCE FOR THE READER: a `✅ 0 hits` here on either staged boot is VACUOUS,
# not a passed invariant, and no amount of behaviour on those flights can change it. The row
# stays (it is correct and cheap, and a `bsprun` image will arm it), but it is not coverage
# of the two boots this file is about. `scripts/orin-specscore.py` scores it DEAD on both
# and refuses to let it count toward a pass.
FORBID \[wedge4\] preempt-in-section core=[0-9]+

# --- ORIN-SUPSTATE: the lift, the split, and the half-restructure between them -------
# ARMED BY `supstate` (UNAOS_SUPSTATE=1, implies `tegra`; default OFF, knob-off image
# byte-identical). BOTH LINES BELOW ARE UNCONDITIONAL ON AN ARMED BOOT THAT REACHES PHASE 2,
# and phase 2 is a place this file already demands: `REQUIRE JD4.*console OWNS the panel`
# and `REQUIRE JD2.*console pump live` above are printed on the same path, the JD4/JD2 line
# LITERALLY three statements before `sup_install`. So they are PENDINGs of the gate-line
# shape — the ORINCONWIN/TEGRA-SD idiom — and permanently no-promote for the standing reason:
# a REQUIRE would red every unarmed boot on CONFIGURATION.
#
# THE ORDER IS THE INSTRUMENT, AND THE GAP BETWEEN THEM IS THE DANGEROUS SHAPE. `lift`
# prints from `sup_install` (display_tegra.rs:3071) the instant the surface leaves the task
# stack; `roles` prints from `jd2_supstate_phase2` (main.rs:7088) after BOTH role spawns.
# Between them there is nothing but two `sched::spawn` calls.
#   lift ✅ + roles ✅   the restructure completed. Read the liveness rows below next.
#   lift ✅ + roles ⏳   THE HALF-RESTRUCTURE. The surface is module-owned and NO ROLE OWNS
#                       IT — the console's state outlived its holder and nothing inherited
#                       it, which is the exact condition this arc exists to remove, reached
#                       by the arc itself. Quote the last five lines of the capture whole.
#   lift ⏳ + roles ⏳   phase 2 was not reached, or the image is unarmed. The JD2/JD4
#                       REQUIREs above already say which; this pair adds nothing.
#   lift ⏳ + roles ✅   IMPOSSIBLE from one image — `sup_install` is the statement before
#                       the spawns. A merged capture; re-slice.
# NO FORBID IS WRITABLE FOR THE HALF-RESTRUCTURE and that is a grammar limit, not an
# oversight: its signature is an ABSENCE, and mbench forbids lines, not absences. The
# reading above is the instrument.
# Both patterns are pure ASCII end to end — these two lines carry no em dash, which is why
# they can key on their full leading fragment rather than stopping at a tag. `screen\+console`
# escapes the `+`. ZERO hits tree-wide for both.
PENDING \[supstate\] lift gen=[0-9]+ screen\+console module-owned
PENDING \[supstate\] roles input=jd2-console presenter=jd2-present dispatcher=jd2-dispatch core=0
# THE ADOPTION COUNTER, AND WHY `gen=1` IS THE ONLY HEALTHY VALUE TODAY. `sup_install` bumps
# `generation` on every install and arc 1 calls it EXACTLY ONCE, from phase 2; the counter
# exists for arc 2's supervisor, which does not exist yet ("Adoption counter for arc 2's
# supervisor: bumped by `sup_install`, read back by nothing in arc 1"). So on boot7j a
# `gen=2` or higher is a SECOND install with no supervisor to have ordered it — a finding to
# quote, not a pass. OPTIONAL rather than FORBID because the honest reading is "something
# re-installed the surface" and this file does not yet know what a legitimate arc-2
# re-adoption will look like; converting an unknown into a red is the JD1-DC error.
OPTIONAL \[supstate\] lift gen=([2-9]|[1-9][0-9]+) screen\+console
# THE ONLY WIRE SIGNAL EITHER NEW ROLE HAS, and its scarcity is itself a finding (see the
# unadjudicable list in the banner). Under `supstate` the `:: tegra: JD2 — KEY … ::` echo is
# printed BY THE DISPATCHER (main.rs:7190/7192, inside `sup_with_surface`), not by the
# monolithic pump — so on an ARMED, ATTENDED flight this line present means `jd2-dispatch`
# is alive, took the SURFACE lock, and got to `handle_key`. Absent on an attended armed
# flight whose `[orinclick]` census (above) keeps ticking is the split's characteristic
# half-death: the input source lives, the dispatcher does not.
# OPTIONAL, NEVER PENDING: an UNATTENDED boot prints none of these and is not faulty — the
# `[orinclick]`-edge argument verbatim. Absence means UNRUN unless a keystroke is known to
# have been sent.
# THE PATTERN STARTS AFTER THE EM DASH. The wire text is `:: tegra: JD2 — KEY …`; keying on
# the `JD2` prefix would put a multi-byte character inside the match on a line that arrives
# 16+ times per attended flight over a UART shared with the SPE's TCU. Both arms are covered:
# `'c'` for printable ASCII (main.rs:7190) and `0x0a` for everything else (:7192). MEASURED
# ACROSS THE WHOLE TREE, and every single hit is a genuine `:: tegra: JD2 — KEY … ::`: 146 on
# the Orin scoring wire (`capture/line-acm0/orin.log`), 141 across the other single-board Orin
# captures, 146 in the mixed bridge files (the same lines again, via `raw.log`), 176 under
# pi4-named bridge directories that are in fact carrying this board's traffic — and 16 on the
# boot7h slice. ZERO false hits, which the trailing ` ::` is what buys: `JD2` is tegra-only
# text and no other line in the corpus ends `KEY <quoted-char-or-byte> ::`.
OPTIONAL KEY ('.'|0x[0-9a-f]{2}) ::

# =====================================================================================
# BOOT 7k — THE TERMINUS INSTRUMENTS, AND THE ONE THING THE COMPILE MATRIX CANNOT SCORE
# (2026-08-28, exec-orin10-specgate). The TERMINUS fold (405b21f6, on top of 0ed6fee2)
# repaired four tegra instruments that had each been reporting something other than the
# state their author intended. Its own arc then proved the more useful half: it
# RE-INTRODUCED two of the four defects deliberately and `arm-tegra-furn` and
# `arm-tegra-supstate` BOTH STILL EXITED 0. The tegra legs are `cargo check` only and no
# QEMU regression in this tree compiles `tegra` (QEMU models no Tegra234), so the compile
# matrix can score tegra COMPILATION and cannot score tegra BEHAVIOUR at all. The fold's
# fallback was an `llvm-objdump` artifact proof, which is right for a commit and does not
# carry forward: nothing re-checks it on the next build and nothing scores the next metal
# capture. THIS BLOCK IS THE THING THAT SCORES THE NEXT CAPTURE. The instruments exist;
# what was missing was anything that REQUIRES them.
#
# WHAT MOVES: spec-declared FORBIDs 10 -> 15, PENDINGs 20 -> 23, OPTIONALs 68 -> 74.
# REQUIREs stay 17 and that is deliberate, not timidity — see the arming note below.
#
# THE ARMING NOTE, and it is the reason there is no new REQUIRE in this block but one.
# Three of the four families are knob-gated and default OFF (`orinconwin`,
# `orinfurn`, `rast`), and the fourth's discriminator points BOTH WAYS depending on
# `supstate`. A REQUIRE on any of them reds an unarmed boot on CONFIGURATION rather than
# on health — the SMPMARK/TEGRA-SD argument, permanently binding in this file. So every
# rule below that CAN fail is spelled as a FORBID on a FAILURE LITERAL, which is the one
# shape that is unconditionally safe: the token exists only in an image that compiled the
# instrument, so an unarmed boot cannot trip it and its silence is never taken as a pass.
# The healthy lines get PENDING and the honest negatives get OPTIONAL, exactly as the
# ORINCONWIN / TEGRA-SD / net4 blocks above spell the same idea.
# THE ONE EXCEPTION IS THE TAKEOVER-PATH PAIR, and it is the strongest position available
# anywhere in this file: its FORBIDs sit on a line that `REQUIRE JD4.*console OWNS the
# panel` ALREADY DEMANDS, so their precondition is guaranteed by another row of this same
# spec. That is a FORBID whose reachability is not argued — it is required.
#
# THE FRESHNESS TRAP, checked on every pattern below rather than assumed. A witness that
# also existed in the PREVIOUS image proves nothing about THIS build. Measured across the
# whole bench capture tree (`~/unaos-bench/capture/`), with `[orinrast]` at 76 hits as the
# control proving the scan reached:
#   `live=FROZEN`                 0 hits   NEW — see the ORINCONWIN row for why `live=LIVE`
#                                          is the trap and `FROZEN` is the fresh half
#   `path=jd2-console-pump`       0 hits   NEW with the fold
#   `path=jd2-supstate-phase2`    0 hits   NEW with the fold
#   `[orinfurn] arm`              0 hits   never flown on this bench
#   `[orinstkdepth]` (both arms)  0 hits   NEW with the fold, never flown
#   `RAST-PAINTED-OVERWRITTEN`    0 hits   NOT NEW — the verdict string predates the fold
#                                          (display_tegra.rs); what the fold changed is
#                                          that a supstate boot can now reach the CORRECT
#                                          arm instead of this one. Stated so no reader
#                                          takes this row as evidence about the fold.
#   `[orinrast] census`          38 hits   NOT NEW — armed on every recent flight
#
# ⚠ GREEN-REFERENCE CHANGE, AND IT IS THE LARGEST THIS FILE HAS EVER TAKEN — stated here
# rather than discovered on the next replay, per the boot5c / boot7f precedents above.
# The two takeover-path FORBIDs match the PRE-FOLD shape of a line every Orin capture in
# the tree carries. Replaying boot7f / boot7g / boot7h — including the standing green
# reference — against this file now reads FAIL on exactly those two rows, 13 hits in
# `capture/line-acm0/orin.log`. That is correct and is the whole point: those captures were
# cut from images that could not say which of the two takeover sites had printed, and this
# file adjudicates the NEXT flight. THERE IS NO GREEN REFERENCE FOR THIS FILE UNTIL A
# POST-405b21f6 IMAGE FLIES. Do not "fix" the red by weakening the rows; fly the fold.
# =====================================================================================

# --- INSTRUMENT 1: ORINCONWIN's `live=` — a read-back that used to be a literal --------
# ARMED BY `orinconwin` (UNAOS_ORINCONWIN=1, default OFF, and it still DECLINES unless
# UNAOS_ORINDESK=1 UNAOS_ORINCLICK=1 ride with it — §6.1, the ordering rule the block
# above already scores). The terminus line already has its PENDING and its two verdict
# arms up in the BOOT 7h block; this row adds the field that block never had.
#
# WHAT CHANGED, AND WHY THE ROW KEYS ON `FROZEN` AND NEVER ON `LIVE`. Before the fold the
# `live=` field was the compile-time string `"LIVE"` — asserted from the build, on a line
# whose own comment forbids exactly that of every other field on it ("DERIVED from the
# outcome CROSSED with the route read back, never asserted"). It is now a READ-BACK of
# `fbcon::console_is_routed()` taken at print time, AFTER `present_outcome` and
# `composite` have run, so a route dropped by the present pass can no longer print `LIVE`.
# THE TRAP: boot7h's `live=LIVE` (capture/line-acm0/orin.log:14833) is the OLD literal. A
# row keyed on `live=LIVE` would be satisfied by every pre-fold capture in the tree and
# would prove nothing about any build — decoration, and the exact failure mode this
# project cares most about. `FROZEN` is the half that could not print before the fold.
#
# ⚠ THE ROW CLAIMS EXACTLY WHAT THE INSTRUMENT CLAIMS AND NOT ONE WORD MORE. The
# instrument's own site documents its limit: `fbcon::detach()` sets GUI_ACTIVE and does
# NOT clear CONSOLE_WIN, so after a detach `console_is_routed()` still answers true while
# no further glyph reaches the window. The sample is taken before any terminus detach can
# have run. So this row scores "the route was installed at this instant" and says NOTHING
# about a later detach freezing the window — that second half is owned by the guards on
# the two detach sites, not by this field, and no spec row here pretends otherwise.
#
# FAILS WHEN: an `orinconwin` image opens the console window, reaches the terminus line,
# and the route is NOT installed at print time — i.e. the present pass dropped a route the
# `route=` field (sampled earlier, and already on the wire) had reported as held.
# CANNOT FALSE-RED AN UNARMED BOOT: the `[orinconwin]` tag is absent from every image that
# did not compile the rung. Its silence is not read as a pass either — the PENDING
# terminus row in the BOOT 7h block is what says whether the instrument fired at all, and
# the two rows are read as a pair:
#   PENDING terminus ⏳ + this row 0 hits  the rung was unarmed or declined. NOT SCORED.
#   PENDING terminus ✅ + this row 0 hits  armed, window open, route installed. The pass.
#   PENDING terminus ✅ + this row HIT     armed, window open, route dropped by the
#                                          present pass. Quote the whole line.
# ⚠ DO NOT TRY TO CONFIRM THIS ROW WITH `strings` / `grep -a` ON THE ARTIFACT, AND DO NOT
# READ A ZERO-HIT RESULT AS A BROKEN IMAGE. Measured 2026-08-31 (orin 11, SPECSCORE) on
# BOTH staged images: `live=FROZEN` takes ZERO hits in `conwin1-…-93825ea/kernel.elf` and
# ZERO in `supstate1-…-93825ea/kernel.elf`, and that is CORRECT for both. The emitter
# (display_tegra.rs:2677) prints `live={}` with `"LIVE"` and `"FROZEN"` chosen at print
# time as separate literals, so the contiguous byte string `live=FROZEN` exists ONLY ON
# THE WIRE and can never be in the `.rodata` of ANY image, healthy or broken. The artifact
# grep this file recommends 40 lines below (the `path=jd2-…` cross-check) is valid THERE
# because those tokens are contiguous literals; it is invalid here.
# WHAT THE ARTIFACT CAN ANSWER is whether the INSTRUMENT is compiled in — grep the tag
# `[orinconwin] win=`, present in conwin1 and absent in supstate1, so this row is
# unfireable on supstate1 BY CONFIGURATION (harmless for a FORBID, but not coverage).
# `scripts/orin-specscore.py --image <staged dir>` draws exactly that distinction without
# being told: it scores this row WIRE on conwin1 and DEAD on supstate1.
FORBID \[orinconwin\] win=.* live=FROZEN

# --- INSTRUMENT 2: the DISCRIMINATED takeover, and the unattributable line it replaces --
# THIS IS THE ONLY PAIR IN THIS BLOCK WHOSE REACHABILITY IS GUARANTEED BY THIS FILE. Both
# rows key on the takeover line that `REQUIRE JD4.*console OWNS the panel` (in the bring-up
# block at the head) already demands of every scored flight, so neither can be dismissed as
# "the path was not driven": if the REQUIRE is satisfied the line printed, and these rows
# adjudicate its SHAPE.
#
# THE DEFECT THE FOLD REMOVED. `jd2_console_pump` (main.rs:2801, `cfg(tegra, aarch64)` —
# present on EVERY tegra image) and `jd2_supstate_phase2` (main.rs:7524, additionally
# `feature = "supstate"`) each print a `console OWNS the panel` pair, and until the fold
# the four literals were BYTE-IDENTICAL. Neither a serial capture nor a grep of the
# artifact could say which site a boot had printed — the one question the state lift's own
# falsifier turns on. The fold's measurement on the `arm-tegra-supstate` kernel.elf at
# 0ed6fee2 makes it worse than the two-run case it looked like: `LC_ALL=C grep -a -o` found
# exactly ONE `.rodata` run for each literal, because `jd2_supstate_phase2` never returns
# and LLVM drops the legacy phase 2 as unreachable — so a SINGLE unattributable run read as
# confirmation. Each site now names itself; the tokens are deliberately longer than 8 bytes
# so they cannot be LLVM-immediate-encoded out of `.rodata` and defeat the artifact grep.
#
# THE TWO FORBIDS MATCH THE PRE-FOLD SHAPE AND NOTHING ELSE. The old wire text was
# `…(Screen back buffer live); first key 0x..` and `…(Screen back buffer live);
# screen-on-boot …`; the new text puts `path=<site>; ` between the `);` and the tail, so a
# post-fold line cannot match either row. Verified against the source of both revisions
# (`git show 0ed6fee2:…/main.rs` lines 2890/2898/7566/7574 vs HEAD's).
# FAILS WHEN: a scored flight prints a takeover line carrying NO `path=` token — a stale
# pre-fold image flown by mistake (a real and expensive bench error, and the one this pair
# is worth the most against), or the discriminator reverted out of the source.
# IMAGE-AGNOSTIC BY CONSTRUCTION: it does not need to know whether `supstate` is armed,
# which is what makes it the one failable row here that needs no knob argument.
# ⚠ THE ONE WAY IT COULD RED HONESTLY-BUT-WRONGLY, stated rather than left for the next
# reader: this wire is known-lossy (UARTC shared with the SPE's TCU), so a dropped run of
# exactly the `path=jd2-…; ` bytes would leave the pre-fold shape behind. That is a 20+
# byte contiguous loss, a different order from the punctuation drops this file usually
# guards against, but it is not impossible. A red here is cross-checked the same way the
# fold proved the tokens in the first place: `LC_ALL=C grep -a` the staged kernel.elf for
# `path=jd2-supstate-phase2` and `path=jd2-console-pump`. If the artifact carries the
# token and the capture does not, the finding is the wire, not the build.
# The patterns start AFTER the em dash in `:: tegra: JD2 — console OWNS …` and are
# contiguous ASCII end to end, this file's standing rule for anything crossing UARTC.
# ⚠ WHAT THESE TWO ARE AND ARE NOT COVERAGE OF, measured 2026-08-31 (orin 11, SPECSCORE).
# On a CURRENT image these rows are UNMATCHABLE BY CONSTRUCTION, and that is the assertion,
# not a defect: all four takeover literals (main.rs:2890/2898/7566/7574) now put
# `path=jd2-…; ` between the `);` and the tail, so no format string in the tree can put
# either pattern on the wire. An automated reachability check therefore scores both rows
# `FOREIGN` — "nothing this kernel emits can match this" — on conwin1 AND supstate1, which
# is EXACTLY RIGHT and must not be filed as a broken rule. Read it as: these two carry zero
# coverage of the CURRENT build, and 100% of their value is against a STALE one. They earn
# their place the day a pre-fold image is flown by mistake, and on that day they are the
# only rows in this file that will notice. The proof that they still bite is in
# `scripts/specs/jetson-sync1-green.capture`'s header: the pre-fold boot5b capture reds row
# two 13 times, and the same capture with only the `path=` token restored goes green.
FORBID console OWNS the panel \(Screen back buffer live\); first key
FORBID console OWNS the panel \(Screen back buffer live\); screen-on-boot
# THE DISCRIMINATION READOUT ITSELF. OPTIONAL AND NOT PENDING, on the `[net4V no-lease
# verdict]` precedent verbatim: BOTH absences are legitimate depending on the image, so
# neither gets a ⏳ that reads as "awaiting". `jd2_supstate_phase2` is `#[cfg]`-erased
# without `supstate`, so its token CANNOT appear on a default image; `jd2_console_pump` is
# compiled into every tegra image but its phase 2 is unreachable under `supstate`.
#   pump ✅ + phase2 ◦   a DEFAULT (non-supstate) flight, taking the legacy path. Correct.
#   pump ◦ + phase2 ✅   a SUPSTATE flight with the state lift in force. Correct, and this
#                        is the reading the fold's falsifier exists to make possible.
#   pump ✅ + phase2 ◦   ON A SUPSTATE IMAGE this is the defect: the legacy phase 2 ran.
#                        The table cannot tell this from row 1 — see the grammar note.
#   pump ✅ + phase2 ✅   IMPOSSIBLE from one boot. A merged capture; re-slice.
#   pump ◦ + phase2 ◦   with the JD4 REQUIRE satisfied, the takeover line carried no path
#                        token at all and the two FORBIDs above have already reddened.
# ⚠ NO FORBID IS WRITABLE FOR ROW 3 AND THAT IS A GRAMMAR LIMIT, NOT AN OVERSIGHT — the
# same limit the ORIN-SUPSTATE block above records for the half-restructure. The rule that
# wants writing is CONDITIONAL ("on a supstate image, FORBID path=jd2-console-pump"), and
# mbench has no `WHEN <guard> FORBID <rx>` any more than it has the conditional REQUIRE
# that block names as the x86-witness.spec grammar hole. Keying it unconditionally would
# red every healthy DEFAULT flight, which is the SMPMARK error in its purest form. The
# reading table is the instrument until the grammar grows a guard.
OPTIONAL path=jd2-console-pump;
OPTIONAL path=jd2-supstate-phase2;

# --- INSTRUMENT 3: ORIN-STKDEPTH — the boot-core stack DEPTH at the furniture seam ------
# ARMED BY `orinfurn` (UNAOS_ORINFURN=1, arroyo:1245, default OFF; implies
# `desktop_firmware` + `orinclick` -> `tegra_el0` -> `tegra`). NEVER FLOWN ON THIS BENCH:
# `[orinfurn]` and `[orinstkdepth]` both take ZERO hits across the whole capture tree, so
# every row here is a prediction read out of the source, not a transcription off a wire.
#
# THE ROW THAT COULD FAIL, AND WHY ITS ARMING IS NOT AN ARGUMENT BUT A `#[cfg]` IDENTITY.
# The two SP reads share ONE gate: the anchor `tegra_stk_anchor()` on `kernel_main`'s
# `bootpace::record("entry")` line (main.rs:88) and the seam read beside `[orinfurn] arm`
# (main.rs:7896) are both `#[cfg(all(target_arch = "aarch64", feature = "orinfurn"))]`.
# So there is NO image in which the reader is compiled and the anchor is not — the
# instrument cannot come up short on CONFIGURATION, which is the objection that keeps
# every other row in this block off REQUIRE. `DEPTH-UNAVAILABLE` therefore always means
# something happened, never that something was missing from the build.
# FAILS WHEN: an `orinfurn` boot reaches the furniture seam and the depth is not
# derivable. The instrument names which of the two it was, on the same line:
#   reason=anchor-never-ran     the anchor is compiled in and did not store. The
#                               `#[inline(always)]` that keeps the read in `kernel_main`'s
#                               own frame is load-bearing; a build that reordered it out
#                               would land exactly here.
#   reason=anchor-below-seam    the two reads are not on one descending frame chain — the
#                               stack was SWITCHED between `kernel_main` and the seam. On
#                               today's Orin the boot stack is UEFI's and is never
#                               switched, so this is a finding about the boot path and not
#                               about the instrument. ⚠ If a future arc legitimately
#                               switches stacks there, this row will red while the
#                               instrument is telling the truth — and the right response
#                               is to retire the row, because the DEPTH number it guards
#                               would have become meaningless at the same moment.
# CANNOT FALSE-RED AN UNARMED BOOT: the `[orinstkdepth]` tag is absent from every image
# without `orinfurn`. Read as a pair with the PENDINGs, same three-way as instrument 1:
#   arm ⏳                        unarmed or the seam was never reached. NOT SCORED.
#   arm ✅ + depth-consumed ✅    the depth was taken. The pass.
#   arm ✅ + this row HIT         armed, seam reached, no depth derivable. Quote the line.
#
# ⚠ DEPTH, NEVER HEADROOM, and the row is worded so it cannot be misread as the latter.
# `depth-consumed=` is the bytes the chain `kernel_main -> tegra_early_stop ->
# tegra_desk_furn` consumed BETWEEN the two reads. No remaining-headroom number is
# derivable in-kernel on this board today (the Orin boot stack is the firmware's and is
# never switched, this link defines no `__stack_top`, and the bounding `MemoryRegion`
# slice is consumed by `memory::init`), and the instrument says so on the wire rather than
# inventing one. NOTHING HERE CLEARS §5.2 — it clears the half that is takeable and names
# the half that is not, which is the opposite of clearing a stop-line by argument.
#
# THE ARMING LINE, PENDING on the gate-line shape: `[orinfurn] arm` is printed before
# anything in the seam can decline, so on an armed image ANY reached seam prints it. This
# row exists to tell "the instrument did not fire" from "the instrument was not armed" —
# it is NOT a rule and can never fail, which is the whole grammar limit this block works
# inside. Pattern stops before the `(ORIN-DESKFURN …)` prose: that tail carries a `§` and
# an em dash, and this file never puts a multi-byte character inside a UARTC pattern.
PENDING \[orinfurn\] arm click=[0-9]+ conwin=[0-9]+ desk=[0-9]+
# THE HEALTHY ARM. Pattern STOPS AT `bytes` and deliberately never reaches `anchor-sp=` /
# `seam-sp=`: `arch_arm64.md:6263` (§ORIN-RAS-ADDR) forbids keying any row on an ADDR
# value, and while these two are stack pointers rather than RAS sinks the rule is written
# as a rule and this file does not carve exceptions into it. The DEPTH is the measurement;
# the addresses are context for a human reading the quoted line.
PENDING \[orinstkdepth\] depth-consumed=[0-9]+ bytes
FORBID \[orinstkdepth\] DEPTH-UNAVAILABLE

# --- INSTRUMENT 4: ORIN-RASTGLASS — the latch that stopped indicting healthy boots ------
# ARMED BY `rast` (UNAOS_RAST=1, arroyo:257; `rast` alone gates the census and does NOT
# imply `tegra`, and `orin_rast_census` is itself `cfg(all(tegra, aarch64))`, so the
# conjunction is exact). Armed on every recent metal flight — 38 `[orinrast]` census hits
# in `capture/line-acm0/orin.log` — so unlike instrument 3 this family has a wire history,
# and the counts below are transcriptions, not predictions.
#
# WHAT THE FOLD REPAIRED. `jd2_supstate_phase2` never returns, so on a `supstate` image the
# legacy phase 2 — the only site that latched `RG_CONSOLE_OWNS` — is unreachable and the
# latch was never set; the phase-2 census call was missing from that copy too, so the
# census stopped dead at the phase-1 boundary. With `owns` false, a console that has taken
# the panel and repainted the cube away scores `RAST-PAINTED-OVERWRITTEN` — the arm the
# instrument's own doc calls "the only arm that indicts a repainter" — instead of
# `RAST-SUPERSEDED-BY-CONSOLE`. A CORRECT BOOT WAS BEING REPORTED AS A DEFECT. The fold
# restored both the boundary latch and the census call to the supstate loop.
#
# FAILS WHEN: a `rast` boot's census finds the cube painted at `post`, absent at `late`,
# and the console-owns latch NOT set at sample time. Post-fold that means a genuine
# repainter inside the window where the cube is supposed to be visible — the verdict that
# would have named boot7j's failure. Pre-fold it was ALSO what a healthy supstate boot
# printed, which is exactly why arming this row was not safe until 405b21f6.
# ⚠ THIS TOKEN IS NOT NEW and the row must not be read as evidence about the fold. The
# verdict string predates it; what the fold changed is that a supstate boot can now reach
# the correct arm instead of this one. Zero hits tree-wide today, on 8 SUPERSEDED and 24
# SURVIVED — every one of them from a NON-supstate image, where the legacy latch was
# always reachable and the defect could not appear.
# CANNOT FALSE-RED AN UNARMED BOOT: no `rast`, no `[orinrast]` tag anywhere in the image.
FORBID \[orinrast\] census .* -> RAST-PAINTED-OVERWRITTEN
# THE ARMING LINE, PENDING on the gate-line shape. It MATCHES ON EVERY RECENT CAPTURE and
# mbench will therefore advise promoting it to REQUIRE. THE ADVICE MUST NOT BE TAKEN,
# permanently, same as TEGRA-SD and the ORINCONWIN pair: `rast` is default OFF and a
# REQUIRE would red every unarmed flight on CONFIGURATION.
PENDING \[orinrast\] census seq=[0-9]+ t=[0-9]+ post=
# THE LATCH'S POSITIVE WITNESS, and it is the ONE thing this block wants to require and
# cannot. `console-owns=1` on a census line proves BOTH halves of the fold's fourth fix at
# once: the boundary latch fired, AND a census ran after the takeover to observe it. But
# the OTHER half of that fix — the census call restored to the supstate loop — regresses as
# an ABSENCE (the census simply stops at the phase-1 boundary), and mbench forbids lines,
# not absences. So a lost census is INVISIBLE to this file and a lost LATCH is visible only
# through the FORBID above. Stated plainly because it is the sharper of the two findings
# this block produced: the spec format can score the wrong verdict and cannot score the
# missing instrument.
OPTIONAL \[orinrast\] census .* console-owns=1
# THE HEALTHY POST-TAKEOVER ARM — the design working as specified, and the arm a supstate
# boot could not reach before the fold. 8 hits tree-wide, all pre-supstate.
OPTIONAL \[orinrast\] census .* -> RAST-SUPERSEDED-BY-CONSOLE
# THE HONEST NEGATIVES, OPTIONAL AND DELIBERATELY NOT FORBID. `RAST-NEVER-PAINTED` HAS
# FIRED ON REAL METAL (`capture/line-acm0/orin.log:35236`, off a `RAST-PARTIAL` post
# sample), so a FORBID would red a flight this bench has actually taken; the honest reading
# is a paint-path measurement, not a regression in the console. The two `LATE-*` arms are
# BROKEN MEASUREMENTS rather than detected faults — the instrument's own doc keeps them
# separate from OVERWRITTEN for that reason ("a sample that did not happen must not look
# like a sample that passed", and "reporting a broken measurement as a detected overwrite
# would be the same overclaim this rung exists to remove"). Rows so a reader is told the
# measurement did not happen; not FORBIDs, so an unreadable panel does not convict a build.
OPTIONAL \[orinrast\] census .* -> RAST-NEVER-PAINTED
OPTIONAL \[orinrast\] census .* -> (RAST-LATE-UNREADABLE|RAST-LATE-BUDGET)


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
