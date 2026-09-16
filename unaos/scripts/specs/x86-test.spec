# x86-test.spec — "did `./arroyo test` REACH THE END OF THE BOOT?", and nothing else.
#   Run:  TWO consumers, both inside `./arroyo test` (SPECRUN, 2026-09-15). `x86_test_completion`
#         asks `scripts/qemu_await.py --settled` for this spec's COMPLETE marker over the finished
#         target/serial.log and turns the answer into PASS or `-> TRUNCATED`; then `x86_spec_replay`
#         REPLAYS the whole file through `mbench --replay` and reds the verb on its rc. This line
#         read "it is not replayed by mbench" until that day, and the RUN-BY block at the tail says
#         why both now run in the same verb. `mbench --replay` by hand prints the same table.
#
# WHY THIS SPEC IS ONE LINE LONG, stated first because its emptiness is the design and not an
# unfinished draft. It answers exactly one question — did the run finish? — and every directive that
# does not serve that question would be a SECOND VERDICT AUTHORITY on a path that already has one.
# `./arroyo test`'s verdict is `scan_serial_faults` (arroyo's FAULT-SCAN block) plus, now, this
# marker. A REQUIRE added here would be scored by nothing: `x86_test_completion` reads the COMPLETE
# markers and no other directive, so a REQUIRE would sit in the file looking load-bearing while
# being incapable of reddening anything — which is the "a check that cannot fire is an absent one"
# shape in LAWS §5, with the extra harm that a reader would trust it. If this verb ever needs
# positive per-witness assertions, they belong in a spec a verb actually REPLAYS (x86-fat.spec is
# that spec for `test-fat`'s chain), not in the file that only marks the end.
#
# THE SECOND HALF OF THAT PARAGRAPH IS NOW HISTORY, NOT THE LIVE REASON (LADDERTAIL, rmbp seat,
# 2026-09-15). "A REQUIRE added here would be scored by nothing" was TRUE as written: `settled()`
# answered on `matcher.markers()` alone and never called `Matcher.complete()`, so one unsatisfiable
# `REQUIRE :: NOSUCHFIXTURE-ZZZ: … :: PASS ::` appended to a COPY of this file, asked of the SAME
# healthy capture (the 2148-line `UNAOS_WC=1` run below), split its two consumers:
#
#   qemu_await.py --settled   status=complete complete_at=1980           rc=0  <- the REQUIRE was INVISIBLE
#   arroyo mbench --replay    FAIL — 0/1 required witnesses              rc=1  <- the REQUIRE was SCORED
#
# `settled()` now gates on `matcher.complete()`, and that same probe returns
# `status=truncated reason=short-witnesses:1 stopped_at=2148` rc=3. A REQUIRE here WOULD fire today.
#
# IT STILL SHOULD NOT BE ADDED — for the FIRST half of the paragraph above (this file answers one
# question; per-witness assertions belong in a spec a verb REPLAYS), and because, measured below,
# the marker already sits AFTER the ladder's last verdict. A REQUIRE would restate what the ordering
# already guarantees, while handing a flaky fixture a new way to turn a red into an inconclusive.
# Add one only in the case named at the bottom of this header.
#
# THE MARKER WAS MEASURED, NOT CHOSEN FROM THE SOURCE — the QEMU-FAST rule ("the predicate is
# derived, never invented") applied to a capture instead of to a running log. Two full-wall runs at
# `27716175` on the rmbp-0915 bench, `UNAOS_QEMU_FULL=1 ./arroyo test 120`:
#
#   default (no knobs)   1537 log lines; the fixture ladder ends at the zeolite block, whose
#                        `metrics` line is at line 1362. Everything after it is periodic
#                        instrumentation ([wc-w] / [schedx86] / [spread] rollups and the xHCI
#                        topology summary), which repeats for as long as the wall lasts.
#   UNAOS_WC=1           2149 log lines — 612 MORE, all of them EARLIER (the compositor and desktop
#                        fixtures) — and the SAME tail: zeolite `metrics` at line 1977, then the
#                        same rollups. The knob lengthens the ladder; it does not move its end.
#
# RE-MEASURED AT `31c7e5cb` (LADDERTAIL), the same two commands, and the marker still lands LAST:
#
#   default (no knobs)   1531 log lines; last ladder verdict `:: SOCK-4: … :: PASS ::` @1351,
#                        zeolite `metrics` @1370. `completion=complete`, rc=0.
#   UNAOS_WC=1           2148 log lines; last ladder verdict `:: SOCK-4: … :: PASS ::` @1969,
#                        zeolite `metrics` @1980. `completion=complete`, rc=0.
#
# That is why the marker is the zeolite line and not a compositor one. A marker taken from the
# wc-armed section would be ABSENT from every default boot and would red a healthy `./arroyo test`
# on the first run — the same mistake x86-witness.spec's own SCOPE note warns about for the paygo
# witnesses, made one layer down. `APPPIN`, the declared last leg of `witness_battery`, is exactly
# such a line: present @1887 under `UNAOS_WC=1` and ABSENT from the default boot (measured, both
# captures above), so the tail witness a reader would reach for first is the one that must not be
# the marker. The zeolite block is the last fixture on BOTH configurations, so a run that reached it
# got through the whole ladder that its knobs asked for.
#
# AND THE ORDERING IS THE WHOLE COVERAGE ARGUMENT, so it is stated as a number rather than implied:
# on BOTH boots the marker is emitted AFTER the ladder's last verdict line (1370 > 1351; 1980 >
# 1969). The ladder and the marker are the same task's output in that order, so a capture that
# contains the marker contains every ladder verdict before it, and NO wall exists that reaches the
# marker while cutting a fixture. A ladder cut short — by a short wall OR by a wedged fixture —
# therefore loses the marker too, and settles `truncated`. That is why this file needs no REQUIRE
# to cover the tail, and the recorded wedge capture in THE CONTROL below is the proof rather than
# the assertion.
#
# `metrics` RATHER THAN THE LINE AFTER IT. The genuinely last zeolite line is
# `:: zeolite: resolver bound :53 — awaiting an inbound query … — witness PENDING ::`, and it is
# rejected on purpose: it announces a witness that is still OUTSTANDING, so a boot could print it
# with the resolver having done nothing at all. `metrics` carries counts the fixture had to execute
# to produce. One line earlier, and it is the line that cannot be reached vacuously.
#
# VERDICT-AGNOSTIC BY CONSTRUCTION. The pattern quotes no `-> PASS`, and must not: a marker that
# matched only the passing spelling would report a FAILING last fixture as a TRUNCATED run, i.e.
# turn a real regression into an inconclusive one — the precise inversion this whole arc exists to
# remove. Reaching the end and failing at the end are different facts, and `scan_serial_faults`
# owns the second one.
#
# THE CONTROL (the gate standard in docs/dev/STRUCTURAL_GATES.md): this spec's zero is
# distinguishable from a broken pattern because the SAME spec over the SAME box produces both
# outcomes on demand — `./arroyo test 120` settles `complete` and `./arroyo test 8` settles
# `truncated`. A pattern that had rotted would report `truncated` for BOTH, which is what the
# recorded go-red proof compares against.
#
# SECOND CONTROL, AND IT IS THE STRONGER ONE — A WEDGE, NOT A WALL (LADDERTAIL). The wall mutation
# above only proves the marker notices a clock running out. The capture that motivated this arc is a
# DEADLOCKED FIXTURE: `~/unaos-bench/scratch/rmbp-0915/lockfix-logs/serial-GORED.log`, the LOCKFIX
# go-red, where a non-reentrant spin `Mutex` in `click_pointer_pos` wedged the battery task and took
# `LOCKFIX-B1` and `APPPIN` off the wire. That run exited 0 when it was captured — on a tree that
# PREDATED this file, which is why its own log says `no completion signal declared for this verb`.
# Replayed against this spec UNCHANGED it is refused, and the two healthy captures are not:
#
#   serial-GORED.log   --settled -> status=truncated stopped_at=1652 rc=3 ; --replay -> TRUNCATED rc=3
#   m1-wc-serial.log   --settled -> status=complete  complete_at=1980 rc=0 ; --replay -> PASS      rc=0
#   m2-default-serial  --settled -> status=complete  complete_at=1370 rc=0 ; --replay -> PASS      rc=0
#
# So a wedge and a short wall are ONE failure to this marker, and that polarity — `truncated` for the
# wedged capture, `complete` for both healthy ones, same spec, same box — is the control.
#
# LEGITIMATE UPDATE. When the boot grows a fixture AFTER zeolite, this marker becomes early rather
# than wrong — it stops covering the new tail, and it never false-reds. Move it in the same commit
# that adds the fixture, re-measure the two runs above, and rewrite the MEASURED block with the new
# line numbers. Do not add a second COMPLETE to "cover both": markers are OR-ed
# (`Matcher.complete()` takes `any(d.hits …)`), so a second one can only ever WEAKEN this file.
#
# THE ONE CHANGE THAT WOULD BREAK THE COVERAGE ARGUMENT, named so it is recognised when it happens:
# the ordering proof holds because the ladder and zeolite are the SAME task's output in that order.
# If a ladder fixture is ever moved onto a task that can be skipped or can wedge INDEPENDENTLY of
# the zeolite resolver, then a capture could carry the marker with a fixture missing, and this file
# would stop covering the tail WITHOUT ever false-redding — a silent loss, not a red. The instrument
# for that case is a per-fixture REQUIRE, and as of LADDERTAIL's second commit one WILL be scored
# here: `settled()` gates on `Matcher.complete()`, so a short witness settles
# `truncated reason=short-witnesses:<n>` and `./arroyo test` exits 1. Two rules if that day comes —
# assert only lines the DEFAULT boot honestly prints (`APPPIN` is WC-only; see above), and never
# assert a known flake (`[ptrdead] backlog` is a Class-3 flake in docs/dev/FIXTURE_FLAKES.md, and
# asserting it would convert a loaded-box flake into an INCONCLUSIVE verdict — the exact inversion
# GATE-TESTTRUNC exists to remove).

# The last fixture of the x86 boot ladder: the ring-3 DNS sinkhole reporting the queries it served.
COMPLETE :: zeolite: metrics .*queries seen, .*blocked \(sinkholed\), .*forwarded upstream ::

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
# RUN-BY: verb:test — `arroyo` names this file in code as `X86_TEST_SPEC`; `x86_test_completion`
#          reads its COMPLETE marker and `x86_spec_replay` replays the whole file, both on every
#          `./arroyo test`. LADDERTAIL measured those two consumers DISAGREEING on one capture; the
#          reason both now run in the same verb is so they can never drift apart in silence again.
#
# GATE-SPECROOTS (`scripts/spec-roots.sh`, a leg of `./arroyo check`) reds by name on any spec under
# scripts/specs/ that is neither named in `arroyo`'s CODE nor carries a RUN-BY line above — and a
# `RUN-BY: verb:` claim is cross-checked against `arroyo`, so this file cannot claim a runner it
# does not have. "A replay spec no gate command runs is a silent landmine."
