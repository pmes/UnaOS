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
# THE SUITE FLOOR, in both modes:
#   knob OFF (classic `./arroyo kernel8-test 210`) — 117/117 required, 0 forbidden, from
#     pi4-regression.spec alone. UNCHANGED, by construction: the base spec is not edited by
#     this arc and the qemu argv is byte-for-byte the same argv.
#   knob ON  (`UNAOS_K8_SCRIPT=... ./arroyo kernel8-test 300`) — 117/117 from the base spec
#     AND THEN 3/3 required, 0 forbidden, from this file. 120 required witnesses across the
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
# `shell.rs:5225`. Every field is pinned rather than presence-checked, because each one is a
# different claim and they fail separately:
#   /fat/VUG.ELF   — the ABSOLUTE path, i.e. probe 2 of `exec_resolve` (the program-source
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
REQUIRE :: BAREXEC: /fat/VUG\.ELF \(typed 'vug'\) — loaded [0-9]+ bytes, entry 0x[0-9a-f]+, pid=[0-9]+ slot=[0-9]+ DETACHED, left RUNNING ::

# The dispatch that must precede it: the core planned an Exec, not a verb. Kept as a FORBID
# on the wrong dispositions rather than a fourth REQUIRE — if `vug` were ever re-advertised
# as a phantom verb (the `Avail::VugDemo` defect §6.6a-closed had to fix before the launch
# could work at all), the REQUIRE above already reds; this names the cause on the same page.
FORBID :: \[midden\] cmd="vug" -> Host verb=
FORBID :: \[midden\] cmd="vug" -> TerminalError

# --- 2. THE NEGATIVE CONTROL -----------------------------------------------------------
# `shell.rs:2900`. A word that is neither a verb nor a program on either volume still gets a
# terminal refusal from the core. This is what stops witness 1 from being satisfiable by a
# build that launches something for every word typed; `len=` is pinned non-zero because a
# core that produced NOTHING prints `len=0` rather than a plausible number (that property is
# the witness's own documented design, shell.rs:2895).
REQUIRE :: \[midden\] cmd="nosuchprogram" -> TerminalError len=[1-9][0-9]* ::

# --- 3. THE LEDGER SEES THE DETACHED CHILD ---------------------------------------------
# `shell.rs:4908`, the BGRUN-1 sweep. AT LEAST ONE tracked job, not "some number of jobs":
# `[1-9]` is the whole gate. Step 1 said "left RUNNING"; if the child had been reaped, or
# never adopted into the job table by `adopt_bg_job`, this line would read 0 and step 1's
# claim would be about a print statement rather than about the system.
REQUIRE :: BGRUN: jobs — [1-9][0-9]* tracked job\(s\) after the sweep ::

# The launch must not have been killed for a full job table on the way in (shell.rs:5217) —
# that path also prints a BAREXEC line, and it is a pass-shaped failure of exactly this gate.
FORBID :: BAREXEC: .* — job table full, pid=[0-9]+ killed
