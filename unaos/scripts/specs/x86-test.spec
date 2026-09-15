# x86-test.spec — "did `./arroyo test` REACH THE END OF THE BOOT?", and nothing else.
#   Run:  it is not replayed by `mbench`. `unaos/arroyo`'s `x86_test_completion` asks
#         `scripts/qemu_await.py --settled` for this spec's COMPLETE marker over the finished
#         target/serial.log, and the x86 `test` leg turns the answer into PASS or `-> TRUNCATED`.
#   Also: `mbench --replay target/serial.log --spec scripts/specs/x86-test.spec --platform x86`
#         works and prints the same one-marker table, which is how a reader checks it by hand.
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
# That is why the marker is the zeolite line and not a compositor one. A marker taken from the
# wc-armed section would be ABSENT from every default boot and would red a healthy `./arroyo test`
# on the first run — the same mistake x86-witness.spec's own SCOPE note warns about for the paygo
# witnesses, made one layer down. The zeolite block is the last fixture on BOTH configurations, so
# a run that reached it got through the whole ladder that its knobs asked for.
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
# LEGITIMATE UPDATE. When the boot grows a fixture AFTER zeolite, this marker becomes early rather
# than wrong — it stops covering the new tail, and it never false-reds. Move it in the same commit
# that adds the fixture, re-measure the two runs above, and rewrite the MEASURED block with the new
# line numbers. Do not add a second COMPLETE to "cover both": markers are OR-ed
# (`Matcher.complete()` takes `any(d.hits …)`), so a second one can only ever WEAKEN this file.

# The last fixture of the x86 boot ladder: the ring-3 DNS sinkhole reporting the queries it served.
COMPLETE :: zeolite: metrics .*queries seen, .*blocked \(sinkholed\), .*forwarded upstream ::
