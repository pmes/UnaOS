# x86-witness.spec — the witness battery on x86 metal, and the pay-as-you-go deferral that
# GR17 put under it.
#   Metal:  ~/rmbp-serial.log (rmbp-bench-connect.sh bridge capture; the FTDI console is
#           TX-only — capture and assert, never --inject; mbench refuses it on this platform)
#   Run:    ./arroyo mbench --replay <capture> --spec scripts/specs/x86-witness.spec --platform x86
#           ./arroyo mbench --follow ~/rmbp-serial.log --spec scripts/specs/x86-witness.spec \
#                   --platform x86 --timeout 300
#   Build:  UNAOS_WC=1 (+ the kepler knobs) + `witness` + `logts` + UNAOS_WCG_PAYGO=1.
#
# SCOPE, stated first because it is what keeps every directive below non-vacuous. This spec
# asserts a PAYGO-ARMED, LOGTS-PREFIXED, WITNESS-ARMED x86 boot and nothing else. A
# witness-armed boot WITHOUT the paygo knob emits no `[wc-g] paygo` line at all, so the paygo
# REQUIREs would be red on a perfectly healthy capture — that is not a defect in the boot, it is
# the wrong spec for it. x86-fat.spec / rmbp-boot.spec / round6-rmbp.spec cover the other
# configurations; this one covers the instrument.
#
# WHY THIS FILE EXISTS. Through GR17 the pi4 gate (pi4-regression.spec) was the ONLY automated
# reader of any `[wc-g]`/`[wc-d]`/`[wc-h]`/`[wc-k]` line anywhere in the tree. Every x86 witness
# finding — the 17.1 s kepler block, its four-phase decomposition, the paygo deferral that took
# it to 2.56 s — was read by hand or by `tools/serial-analyzer.py --wcg`, and nothing would have
# gone red if an emitter had silently stopped printing. docs/dev/OS/01_BOOT_HAL/bootpace.md §10h
# recorded that as a deliberate coverage gap ("no x86 spec yet reads any `[wc-g]` line"). This
# file closes it.
#
# PROVENANCE. Every pattern below was verified against the REAL metal capture
# ~/unaos-bench/capture/rmbp-gr16-s73/ttyUSB0.log, boots 7 and 8 — the two paygo boots
# (`kepler=2564ms` on both, from 17 077 ms; kernel `aee612a7…`, tree `f7421a20`). The quoted
# line beside each directive is the actual capture line it was matched against. A REQUIRE that
# cannot match a real boot is a vacuous instrument, which is the defect this tree keeps
# convicting; so is a FORBID whose text no emitter can produce, and each FORBID below names the
# `unaos/crates/kernel/src/…` site that emits it.
#
# WHAT THIS GRAMMAR CANNOT SAY, and who says it instead. mbench matches LINES, independently and
# without order: there is no way to write "a lattice pass came BEFORE the full pass for this
# window", "this window's deferral census only ever grew", or "every window that kept presenting
# past defer_ms eventually got full coverage". Those are ORDERING and PER-WINDOW-AGGREGATE
# claims, and they belong to `tools/serial-analyzer.py --wcg`, whose paygo section reports the
# lattice/full pass split per window, the deferred census (greatest `emit=` wins — the census is
# a running total, so summing it is always wrong), the PAID/UNPAID status at capture end, and
# WARNs the un-covered-window case outright. This spec pins PRESENCE and forbids VERDICTS; the
# analyzer pins the shape of the sequence. Neither subsumes the other.

# --- END-OF-RUN MARKERS ------------------------------------------------------------------
# The truncation verdict (mbench rule 2): without a marker, a bench cable pulled mid-boot reads
# as a regression in arcs that touched nothing — the trap pi4-regression.spec's header documents
# at length. Both markers here are STRUCTURAL, never verdicts, which is the property that
# matters: a regression in any witness below must still read FAIL, not "inconclusive".
#   1. `:: PULSE-A: start pid=… ::` is PULSE.ELF's own first line from ring 3
#      (crates/user-pulse/src/main.rs). PULSE-W is the LAST fixture in the x86 launcher chain, so
#      reaching it means the boot got past every window fixture that drives this battery. It
#      lands at 25177 ms (boot 7) / 25090 ms (boot 8) — after the latest REQUIRE below, which is
#      win=4's `-> PAID` at 22563 / 22661 ms.
#   2. `PULSE-W: … witness skipped ::` covers the launcher's four honest early exits (no FAT
#      volume / PULSE.ELF absent / outside the ring-3 window / read failed —
#      arch/x86_64/syscall.rs:13531-13552). Those boots also ran to the end of the chain, so they
#      must fail on their missing witnesses rather than read as short. Same construction as the
#      pi4 gate's `:: BANDY-RT:` second marker, and for the same reason.
COMPLETE :: PULSE-A: start pid=
COMPLETE :: PULSE-W: .*witness skipped ::

# --- THE LOGTS PRECONDITION ---------------------------------------------------------------
# This one directive is load-bearing for the `clock=unarmed` FORBID far below, which is the only
# rule in this file whose reachability depends on the CAPTURE'S SHAPE rather than on an emitter.
# That FORBID guards itself with the 12-column logts stamp (the grammar has no other way to say
# "past early boot"), so on a capture with no logts prefix it would match nothing — not because
# the boot was healthy, but because the timestamps it keys on were never printed. A FORBID that
# silently cannot fire is worse than no FORBID: the green reads as evidence.
# So the prefix is REQUIRED here, ON A PAYGO LINE, and the time-guarded rule below is then known
# to be live rather than assumed to be.
# Capture line (boot 7, and boot 8 at 5028 ms):
#   [   5005ms] [wc-g] paygo win=1 state=waiting emit=1 lattice_n=16 deferred=1 defer_ms=15000 …
REQUIRE ^\[\s*[0-9]+ms\] \[wc-g\] paygo win=[0-9]+

# --- THE FOUR-PHASE PROF LINE (GR17 M1) -----------------------------------------------------
# The line that decomposed the 2.87 s pass and made §10h's cost model checkable to a fraction of
# a percent: three FNV checksum phases plus the glass read-back, with the scale terms
# (`surf_bytes`, `probes`) beside them. The read-back is 98.7 % of the pass, so this line is
# where a regression in the PCIe glass-read path — a revert of the bulk u64 read to three
# single-byte volatile reads per probed pixel, say — shows up as a number rather than as an
# unexplained boot slowdown. It is REQUIREd for the reason the whole file exists: the phase
# decomposition currently has no automated reader at all, and an emitter that stopped printing
# would cost the next investigator the same week it cost this one.
# Every field is pinned by NAME and shape, not by value: `surf_bytes`/`probes` are functions of
# the panel (2880x1800 on the bench, 3 862 528 B / 965 632 probes for the console window) and
# pinning them would make this spec panel-specific for no gain.
# Capture line (boot 7 @ 4999 ms):
#   [wc-g] prof win=1 seq=0 surf_bytes=3862528 cks_blit_us=5738 civac_us=5738 cks_after_us=5744 probes=60352 readback_us=102831
REQUIRE \[wc-g\] prof win=[0-9]+ seq=[0-9]+ surf_bytes=[0-9]+ cks_blit_us=[0-9]+ civac_us=[0-9]+ cks_after_us=[0-9]+ probes=[0-9]+ readback_us=[0-9]+

# --- PAYGO, PASS 1: THE LATTICE PASS ON THE CONSOLE WINDOW ---------------------------------
# win=1 is the console window on this bench (`[wc-x] console-window win=1 panel=2880x1800
# surf=1312x736 …` at 5185 / 5208 ms), and it is the expensive one: 965 632 probes at ~2.94 µs
# each is the 2.87 s pass §10g convicted. Paygo's first pass over it samples every 16th source
# pixel — 1/16 of the probes, said out loud on the wire as `coverage=lattice16`.
# THE DEFECT THIS CATCHES is the one that would make paygo a lie rather than a saving: a build
# whose first pass silently went back to full coverage would still print CLEAN, still print a
# rollup, still pass every other directive in this file — and would quietly hand back the 11.5 s.
# The `coverage=` marker exists precisely because a small `fbbad=` denominator does not say WHY
# it is small; this directive is the marker's only automated reader.
# `seq=0 .* coverage=lattice16` is deliberately narrow: win=1 prints other `seq=0` lines later
# (the deferred full pass at 17403 ms is one), and only the lattice one may satisfy this.
# Capture line (boot 7 @ 4999 ms; boot 8 @ 5022 ms):
#   [wc-g] win=1 seq=0 own=no scale=1x … fbbad=0/60352 coverage=lattice16 us=5119 rectscan_us=6814 slow=no -> CLEAN
REQUIRE \[wc-g\] win=1 seq=0 .*coverage=lattice16 .*-> CLEAN

# --- PAYGO, THE DEFERRED PASS: FULL COVERAGE STILL ARRIVES ----------------------------------
# The other half of the bargain, and the half that makes the lattice pass honest rather than a
# coverage cut: once the window is past `defer_ms` since kernel entry, the full-coverage pass
# runs. Without this directive the spec above would REWARD a build that never paid — cheap first
# pass, no second pass, 60 352 of 965 632 pixels ever verified, green gate.
# ORDERING IS NOT ASSERTED HERE and cannot be: see the header. This says full coverage was
# reached for the console window at some point in the boot; `--wcg`'s paygo section says it came
# after the lattice pass and reports the window UNPAID at capture end when it did not.
# Capture line (boot 7 @ 17403 ms; boot 8 @ 17472 ms) — note the denominator is now the whole
# surface, 16x the lattice line above:
#   [wc-g] win=1 seq=0 own=no scale=1x … fbbad=0/965632 coverage=full us=4908 rectscan_us=6814 slow=no -> CLEAN
REQUIRE \[wc-g\] win=1 .*coverage=full .*-> CLEAN

# --- PAYGO, THE DEFERRAL LINE ITSELF --------------------------------------------------------
# The census line, with the two constants that MUST track the `coverage=lattice16` literal above
# pinned by value on purpose. `lattice_n=16` is the step the lattice pass actually walked and
# `coverage=lattice16` is a `&'static str`; wcg.rs keeps them in step with a const assertion, and
# this pair of directives keeps them in step ON THE WIRE, which is where a reader believes them.
# A build that changed PAYGO_LATTICE_N to 8 and left the marker alone would print
# `coverage=lattice16` over a step of 8 — a witness lying about its own coverage — and goes red
# HERE rather than being believed.
# `defer_ms=15000` is pinned for the matching reason: it is the horizon the honesty check in
# `--wcg` measures "kept presenting past defer_ms" against, and a silently shortened horizon
# would make that check pass by moving the goalposts rather than by paying.
# `clock=entry` is the guard field — `clock=unarmed` beside `since_entry_ms=0` is how the emitter
# distinguishes a real zero reading from an absent clock (see the FORBID below).
# Capture line (boot 7 @ 5005 ms):
#   [wc-g] paygo win=1 state=waiting emit=1 lattice_n=16 deferred=1 defer_ms=15000 since_entry_ms=5005 clock=entry taken=1 budget=4 -> DEFERRED
REQUIRE \[wc-g\] paygo win=[0-9]+ state=waiting emit=[0-9]+ lattice_n=16 deferred=[0-9]+ defer_ms=15000 since_entry_ms=[0-9]+ clock=entry taken=[0-9]+ budget=[0-9]+ -> DEFERRED

# --- PAYGO REACHES PAID ---------------------------------------------------------------------
# `-> PAID` is the terminal paygo line for a window: its battery completed, `taken=` reached
# `budget=`, and the deferred passes were all actually run. REQUIREing it is the assertion that
# the deferral machinery TERMINATES — that a window can still finish its four samples with the
# gate in the path. A paygo build in which every window sat at `state=waiting` forever would
# satisfy the DEFERRED directive above and every FORBID in this file, and would have replaced
# the battery with an indefinite postponement. That is the failure mode a deferral gate has, and
# this is the line that catches it.
# NOT every window need be PAID at capture end, and this is deliberately not a COUNT: win=1 is
# still `emit=2 taken=2` when boots 7 and 8 end, because the console window keeps presenting for
# the rest of the boot and its remaining samples are spent as they are earned. That is paygo
# working as designed, not a shortfall — `--wcg` reports it as UNPAID-at-capture-end, which is a
# statement about the capture, not a verdict about the kernel.
# Capture line (boot 7 @ 16753 ms; boot 8 @ 16806 ms):
#   [wc-g] paygo win=3 state=complete emit=3 lattice_n=16 deferred=2 defer_ms=15000 since_entry_ms=16753 clock=entry taken=4 budget=4 -> PAID
REQUIRE \[wc-g\] paygo win=[0-9]+ state=complete .*taken=[0-9]+ budget=[0-9]+ -> PAID

# --- THE ROLLUP CARRIES THE PAYGO MARK ------------------------------------------------------
# `paygo=yes` on the per-window rollup is what tells a reader that the `samples=4` behind that
# CLEAN verdict were taken under the deferral regime rather than as four full passes. Without it
# a paygo capture and a pre-GR17 capture produce rollups that read identically while meaning
# completely different things — which is exactly how the §10g block hid for two weeks.
# The REQUIRE asserts THE INSTRUMENT RAN and stops short of the verdict, following the pi4 gate's
# WC-G rule: the finding is the arc's output, and the verdict half is the FORBIDs' job below,
# which need no completeness claim to be sound.
# Capture line (boot 7 @ 16753 ms):
#   [wc-g] rollup win=3 scope=window paygo=yes samples=4 coher=0 race=0 blit=0 clean=4 slow=0 maxus=2870 wit_us=78695 frame_us=16667 -> CLEAN
REQUIRE \[wc-g\] rollup win=[0-9]+ scope=window paygo=yes .*frame_us=[0-9]+ ->

# --- THE WC-G VERDICTS, FORBIDDEN -----------------------------------------------------------
# The three suspect verdicts the four-checksum sample exists to separate: COHER (the coherent
# view disagrees with the blit's), RACE (the source moved under the copy), BLIT (the copy itself
# is wrong). A FORBID needs no completeness claim — it catches the verdict in ANY window at ANY
# point in the boot, including long after every rollup has printed, which is why the WC-G arc
# settled on FORBIDs rather than a global summary line it could not honestly scope.
# THESE ARE SHARPER UNDER PAYGO THAN THEY WERE BEFORE IT, which is the reason they are restated
# on x86 rather than left to the pi4 gate: the lattice pass checksums a SUBSET of the surface,
# so a coherency fault in the 15/16 of pixels pass 1 skips is invisible until the deferred full
# pass runs. These rules are what make that deferred pass carry teeth.
# Emitter: crates/kernel/src/video/wcg.rs. Zero hits on boots 7 and 8.
FORBID \[wc-g\] .*-> COHER
FORBID \[wc-g\] .*-> RACE
FORBID \[wc-g\] .*-> BLIT

# --- WC-D: THE SCAN-OUT VERDICT --------------------------------------------------------------
# The only instrument in this file that reads the PANEL rather than a number the kernel computed
# about a surface: it re-derives the window's destination pixels from the source surface and
# reads the scan-out buffer back. `bad_cache=0` is the blit's stride/pitch arithmetic, upscale
# indexing, colour encoding and clipping being right; `bad_ram=0` is read after a bare cache
# invalidate and says the pixels reached the memory the display engine scans — MEANINGFUL ONLY
# ON METAL, which is what this spec asserts against.
# GR17 touched this path (`3c05856d` widened `read_pixel` to one aligned volatile u32 from three
# byte reads — same probes, same decisions, 2.8x), so the positive witness is REQUIREd: a
# widening that broke the read would show as a FAIL, but a widening that removed the CALL would
# show as nothing at all.
# Capture line (boot 7 @ 5180 ms; boot 8 @ 5203 ms):
#   [wc-d] verify win=1 surf=1312x736 band=0..64 scale=1x at (784,457) panel=2880x1800 checked=83968 bad_cache=0 bad_ram=0 ram_indep=no moved=0 nonzero=8300 … -> PASS
REQUIRE \[wc-d\] verify win=[0-9]+ .*bad_cache=0 bad_ram=0 .*-> PASS
# The half with teeth. NOT VACUOUS, and provably so: this rule fires on boot 8 of the very
# capture it was written from —
#   [  25043ms] [wc-d] verify win=3 surf=128x128 band=none scale=6x at (9,21) panel=2880x1800
#               checked=589824 bad_cache=0 bad_ram=197376 ram_indep=no moved=0 nonzero=589824
#               cksum=0xdd4fbfda2972ebe0 first=(9,21) got=0x000000 want=0x1e1e1e -> FAIL
# — a verify whose cache view was perfect (`bad_cache=0`) while 197 376 bytes of the scan-out
# read back black instead of desktop grey, 2 ms before `[wc-a] close win=5` / `close win=3` tore
# the window down. A verify racing a teardown, on x86, at the tail of the WINX-8 fixture. It is
# a real defect, it is OUT OF THIS ARC'S LANE (wm.rs / the fixture launcher), and it is left
# RED on purpose: weakening this rule to make boot 8 green would be the precise disease every
# other comment in this file is written against. Boot 7 replays PASS; boot 8 replays FAIL, and
# FAIL is the correct answer for boot 8.
FORBID \[wc-d\] verify .*-> FAIL

# --- THE DEFERRAL GATE'S OWN CLOCK -----------------------------------------------------------
# `clock=` is not decoration. `since_entry_ms()` returns None until the bootpace entry stamp
# lands, and the emitter refuses to print a fabricated zero: it prints `since_entry_ms=0
# clock=unarmed`, which is a DIFFERENT statement from `since_entry_ms=0 clock=entry` (a genuine
# reading taken at entry). The gate defers while it cannot read the clock — so a paygo line
# reading `clock=unarmed` late in a boot means the gate CAN NEVER OPEN, every window stays at
# pass 1, and full coverage never arrives at all. The battery would look cheap and clean and
# would be verifying 1/16 of the panel forever. Nothing else in this file catches that: every
# other directive is satisfied by a boot in that state.
# WHY THE TIMESTAMP GUARD, and why it is expressible after all. This grammar has no time
# predicate — but a logts capture carries the time IN THE LINE, and the REQUIRE at the top of
# this file pins that the prefix is present on paygo lines, so keying on it here is sound rather
# than hopeful. `[0-9]{5,}ms` is any stamp at or past 10 000 ms: a generous grace over the few
# milliseconds the entry stamp actually needs, and still 5 s inside the 15 000 ms deferral
# horizon where an unarmed clock would do its damage. An unarmed reading in the first ten
# seconds is not forbidden, because a first paygo line that genuinely predates the stamp is an
# honest reading and a false red costs a bench sitting.
# Emitter: `paygo_note` in crates/kernel/src/video/wcg.rs. Zero hits across all 8 boots of the
# s73 capture — and REACHABLE: the pattern was matched against a hand-reddened copy of boot 7's
# 15453 ms line with `clock=entry` replaced by `clock=unarmed`, which it caught.
FORBID ^\[\s*[0-9]{5,}ms\] .*\[wc-g\] paygo .*clock=unarmed

# --- THE TIMESTAMP TAP'S OWN WITNESS --------------------------------------------------------
# Every number in this file is quoted in milliseconds since kernel entry, and every one of them
# comes from the logts tap prefix. LOGWIT-1 is the fixture that proves that prefix reached the
# FTDI capture — it writes a marker, reads it back off the wire, and separately checks that its
# own matcher REJECTS an unprefixed and a malformed control. Requiring it is requiring that the
# evidence stream this whole spec reasons over is real.
# Capture line (boot 7 @ 1210 ms; boot 8 @ 1324 ms):
#   :: LOGWIT-1: tap prefix reached the FTDI capture — marker seq=1 read back with a well-formed
#      12-col prefix (kind=mono), unprefixed + malformed controls rejected, attempts=1 bare=0
#      absent=0 unread=0 -> PASS ::
REQUIRE :: LOGWIT-1: tap prefix reached the FTDI capture .*-> PASS ::
# Both failure exits, and the second is why this needs its own rule rather than leaning on the
# built-in `-> FAIL` FORBID. `logts.rs:389` prints `… -> FAIL ::` when no marker came back
# prefixed — caught by the default set. `logts.rs:341` prints `… -> FORBID (matcher vacuous — no
# CLOCK-2b prefix claim on this boot is evidence) ::` when the matcher ACCEPTED a control it must
# reject, and that line contains no `-> FAIL` at all: the default FORBIDs sail straight past it.
# It is also the worse of the two — a matcher that accepts anything makes the PASS above
# meaningless, so the run would go green on a witness that proved nothing.
FORBID LOGWIT.*(FAIL|FORBID)
FORBID matcher vacuous

# --- THE SNTP FIXTURE CLEANS UP AFTER ITSELF ------------------------------------------------
# `b3408a24` made the sntp fixture clear the canned wall-clock anchor it plants. This matters to
# THIS spec specifically, more than to any other: a canned anchor left standing flips the logts
# prefix from monotonic `[  NNNNNms] ` to civil `[HH:MM:SSZ] `, at which point every millisecond
# figure this file's comments quote stops being comparable, the timestamp-guarded `clock=unarmed`
# FORBID above stops matching (civil stamps have no `ms]`), and the analyzer's cost decomposition
# quantises to one-second resolution. The fixture's cleanup is therefore a precondition of the
# instrument, and its absence would degrade several rules here SILENTLY rather than redly.
# Emitter: crates/kernel/src/smolnet.rs:1787.
# Capture line (boot 7 @ 1210 ms; boot 8 @ 1324 ms):
#   :: [sntp-x86] canned anchor cleared — clock unanchored again ::
REQUIRE :: \[sntp-x86\] canned anchor cleared — clock unanchored again ::

# --- WHAT IS DELIBERATELY NOT PINNED, stated rather than omitted -----------------------------
#   * `kepler=2564ms`. It is the headline GR17 number and it is NOT a directive, because it is a
#     MEASUREMENT on a specific machine: the 2012 rMBP's PCIe latency sets the read-back cost,
#     and pinning a millisecond figure would turn a slower or faster host into a red. The gate
#     question is "is the paygo machinery on the wire and behaving", which the directives above
#     answer; the number's home is bootpace.md §10h and the analyzer's table.
#   * `deferred=` values (`emit=2 deferred=264` on the console window mid-wait). It is a RUNNING
#     CENSUS, so any value is legal on any line and only the sequence means anything — an
#     aggregate claim this grammar cannot make. `--wcg` reports it, greatest `emit=` wins.
#   * A `-> UNPAID` verdict. There is no such line: a window that never completes simply stops
#     emitting after its last DEFERRED, and silence is not matchable. This is the one gap in the
#     coverage above, and it is the analyzer's WARN rather than a directive here — recorded so
#     the next reader does not mistake its absence for an oversight.
#   * `[wc-h]` / `[wc-k]` / `[cursor*]`. Real witnesses, thoroughly pinned by the pi4 gate, and
#     out of this spec's subject. Adding half-considered copies here would grow the file without
#     growing the evidence.
