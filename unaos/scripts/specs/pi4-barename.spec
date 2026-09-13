# pi4-barename.spec — the TYPED battery for the Pi 4. Three witnesses, and they exist only
# in a capture where someone actually typed.
#
#   QEMU gate:  UNAOS_K8_SCRIPT=scripts/specs/pi4-barename.inject ./arroyo kernel8-test 300
#   Metal:      mbench.py --follow ~/pi-serial.log --inject /tmp/pi.in \
#                   --script scripts/specs/pi4-barename.inject --platform pi --spec <this file>
#
# ── WHY THIS IS A SECOND FILE AND NOT THREE MORE LINES IN pi4-regression.spec ──────────────
# `kernel8-test`'s UART0 was `-serial file:` — WRITE-ONLY. The suite could never type, so no
# interactive path on this arch has ever been gated: `exec-barename` proved bare-name launch
# by hand-driving a bidirectional chardev and said so in as many words (PARITY.md
# §6.6a-closed — "the Pi's regression suite still cannot type, so none of the above is
# *gated*"). SUITETYPE gives `test_kernel8` that bidirectional chardev behind
# `UNAOS_K8_SCRIPT`, DEFAULT OFF.
#
# Default off means a REQUIRE for a typed line, placed in the base spec, would red the
# classic gate for doing exactly what it is supposed to do — the identical argument the
# `[dragperf]` and `[dragwedge]` families make in pi4-regression.spec about `desktop_firmware`-gated
# fixtures, where the conclusion was "FORBID, not REQUIRE, and the REQUIRED COUNT MUST NOT
# MOVE". Same conclusion here, one step further: those families were stuck with a FORBID
# because there was one spec and one battery. A second spec, asserted ADDITIONALLY and only
# when the knob is armed, keeps the base count fixed AND still gets real REQUIREs.
#
# THE SUITE FLOOR, in both modes (base count 117 -> 119 on 2026-08-31, TABFIXTURE: two REQUIREs
# in pi4-regression.spec §4b' gate the TAB rotation — see that block. Nothing about the ARGUMENT
# below changes; only the number the base spec contributes. This file's own 3 are untouched):
#   knob OFF (classic `./arroyo kernel8-test 210`) — 119/119 required, 0 forbidden, from
#     pi4-regression.spec alone. UNCHANGED BY THIS FILE, by construction: the base spec is not
#     edited by the SUITETYPE arc and the qemu argv is byte-for-byte the same argv.
#   knob ON  (`UNAOS_K8_SCRIPT=... ./arroyo kernel8-test 300`) — 119/119 from the base spec
#     AND THEN 3/3 required, 0 forbidden, from this file. 122 required witnesses across the
#     two batteries. The base spec runs FIRST and a non-PASS base verdict short-circuits:
#     this file is only ever consulted on a capture the base spec already called complete.
#
# NO `COMPLETE` DIRECTIVE HERE, deliberately. Truncation is the base spec's judgement to
# make — it owns the end-of-run markers — and `test_kernel8` will not reach this file unless
# the base spec returned PASS. A second, later COMPLETE marker here would only create a
# second way to answer a question already answered.
#
# WINDOW — measured, not guessed (gate run 2026-08-18, host load average 18.9 at launch):
#   readiness (`:: BANDY-ACL:`) at t=17.0 s   —   `vug` typed at t=29.1 s
#   `nosuchprogram` at t=43.3 s               —   `jobs` at t=58.0 s
# So the typed script is finished inside the first ~70 s and the rest of the window is the
# free-running steady state the classic gate also sits through. 300 is what this gate was
# verified at and what to use; the cost over the classic 210 is margin, not need. A window
# too short reports TRUNCATED from the BASE spec before this file is ever read, which is the
# honest answer — a typist that had nothing to type at is not a launch regression.

# --- 1. THE BARE NAME LAUNCHES ---------------------------------------------------------
# `shell.rs:7111` (the `:: BAREXEC: {} (typed '{}') — loaded … left RUNNING ::` emitter; WIREHYG
# 2026-09-13 re-verified this and the four citations below — ALL FIVE had drifted onto unrelated
# code, so each now carries the token that makes the site re-findable when the line moves again).
# Every field is pinned rather than presence-checked, because each one is a
# different claim and they fail separately:
#   /apps/VUG.ELF  — the ABSOLUTE path, i.e. probe 2 of `exec_resolve` (the program-source
#                    root) fired. Typed from `/`, cwd-relative resolution CANNOT find this
#                    image, so a build that lost the second probe leaves the operator at `/`
#                    unable to type `vug` — the original defect, exactly.
#   (typed 'vug')  — the token as typed, which is what makes this line evidence about the
#                    OPERATOR's dispatch and not about a boot fixture launching the same file.
#   entry/pid/slot — a real load, a real process, a real job-table slot.
#   DETACHED       — the info-page detached bit is set. VUGSCENE renders only when
#                    `overlay = detached || interactive`, so this word is the drawing path.
#   left RUNNING   — the shell did NOT wait on it; step 3 below is what proves that claim.
# `— REFUSED:` and `— rejected` lines from the same site cannot satisfy this pattern.
REQUIRE :: BAREXEC: /apps/VUG\.ELF \(typed 'vug'\) — loaded [0-9]+ bytes, entry 0x[0-9a-f]+, pid=[0-9]+ slot=[0-9]+ DETACHED, left RUNNING ::

# The dispatch that must precede it: the core planned an Exec, not a verb. Kept as a FORBID
# on the wrong dispositions rather than a fourth REQUIRE — if `vug` were ever re-advertised
# as a phantom verb (the `Avail::VugDemo` defect §6.6a-closed had to fix before the launch
# could work at all), the REQUIRE above already reds; this names the cause on the same page.
FORBID :: \[midden\] cmd="vug" -> Host verb=
FORBID :: \[midden\] cmd="vug" -> TerminalError

# --- 2. THE NEGATIVE CONTROL -----------------------------------------------------------
# `shell.rs:4832` (`:: [midden] cmd="{}" -> {} len={} ::`). A word that is neither a verb nor a
# program on either volume still gets a
# terminal refusal from the core. This is what stops witness 1 from being satisfiable by a
# build that launches something for every word typed; `len=` is pinned non-zero because a
# core that produced NOTHING prints `len=0` rather than a plausible number (that property is
# the witness's own documented design, shell.rs:4826-4828).
# ⚠ THE VERDICT WORD IS NOT THIS FILE'S TO ASSERT: `TerminalError` is the `{}` in that format,
# filled at runtime by `Message::kind()` in ANOTHER CRATE (`libs/sys/midden_core/src/lib.rs:99`).
# The bytes `-> TerminalError` therefore exist on the WIRE and NOWHERE in `.rodata`, so this row
# is sound here and would read 0 in an artifact certification; and the FORBID at the `vug` block
# above stops being ABLE to fire the day that enum arm is renamed — a check that cannot fire is
# an absent one (LAWS §5). Re-key both on `kind()`'s current word when it changes.
REQUIRE :: \[midden\] cmd="nosuchprogram" -> TerminalError len=[1-9][0-9]* ::

# --- 3. THE LEDGER SEES THE DETACHED CHILD ---------------------------------------------
# `shell.rs:6773` (`:: BGRUN: jobs — {} tracked job(s) after the sweep ::`), the BGRUN-1 sweep.
# AT LEAST ONE tracked job, not "some number of jobs":
# `[1-9]` is the whole gate. Step 1 said "left RUNNING"; if the child had been reaped, or
# never adopted into the job table by `adopt_bg_job`, this line would read 0 and step 1's
# claim would be about a print statement rather than about the system.
REQUIRE :: BGRUN: jobs — [1-9][0-9]* tracked job\(s\) after the sweep ::

# The launch must not have been killed for a full job table on the way in (`shell.rs:7103`) —
# that path also prints a BAREXEC line (`shell.rs:7103`), and it is a pass-shaped failure of
# exactly this gate.
FORBID :: BAREXEC: .* — job table full, pid=[0-9]+ killed

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
# RUN-BY: verb:kernel8-test — `arroyo` names this file in code (the `UNAOS_K8_SPEC` default), so
#          the verb replays it and its rc is the verb's rc.
#
# GATE-SPECROOTS (`scripts/spec-roots.sh`, a leg of `./arroyo check`) reds by name on any spec under
# scripts/specs/ that is neither named in `arroyo`'s CODE nor carries a RUN-BY line above — and a
# `RUN-BY: verb:` claim is cross-checked against `arroyo`, so this file cannot claim a runner it
# does not have. "A replay spec no gate command runs is a silent landmine."
