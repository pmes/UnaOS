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
# MINIMUM BUILD GENERATION — `32724cb4` (2026-08-06), and this is a SECOND scope axis, distinct
# from the knob set above. The GR18 witness round put eight new wires on the x86 console, and the
# REQUIREs added for them below are red on any capture from an image older than the commit that
# emitted them — not because the boot was sick, but because the line did not exist yet. Stated as
# ONE sha rather than eight because the directives land as one block and `32724cb4` is the latest
# of the introducing commits:
#   `609d9b3a`  BUY-2, which MOVED the EPACE-TRIM M8 line (see that block — this is why it is
#               OPTIONAL and not REQUIRE)
#   `bdfb3b4c`  `late=` on the SMC-BATT witness; `index enumeration STOP-NOTE at idx`
#   `a2cada19`  the paygo terminals; SMC WALK-QUIET, whose `index walk done` survives as THE
#               standing per-boot witness
#   `a0a2d163`  WXN-x86 M1 — the PDPT NX sweep, `WXAUDIT-NXE`
#   `32724cb4`  M1 hardened — the `-> VACUOUS` verdict, the WXAUDIT leaf histogram, `WXN-FBWC`
# MEASURED, not asserted: replaying this spec against Boot V (metal, the last capture from before
# `a0a2d163`) reds exactly FOUR of the six REQUIREs this block added — the WXN sweep verdict, the
# WXAUDIT histogram, `WXAUDIT-NXE` and `WXN-FBWC`, none of which its kernel could print — and passes
# the other two, `late=` on the SMC-BATT witness and `index walk done`, both of which it could. That
# split is the scope line drawn by evidence rather than by claim, and it is the reason this section
# names a sha instead of saying "recent build".
# BOOT V REDS FOUR MORE, and they are NOT this block's: `[wc-g] … coverage=full`, `[wc-g] … -> PAID`,
# the `paygo=yes` rollup and `[wc-d] … coverage=full` were all already in force and all already red
# there. Boot V's video battery never reached the 15 000 ms deferral horizon — its capture carries a
# second `BPACE: entry` at 12 952 ms, i.e. the machine came back round before the deferred passes were
# due — so its wc-g windows end at `state=waiting … -> DEFERRED` (14 of them) with a single wc-d
# `complete … PAID`. That is a capture that did not sit long enough, not a kernel that never paid, and
# it is exactly the distinction the header's ORDERING paragraph hands to `--wcg` rather than to this
# grammar. Recorded so that nobody replaying Boot V reads eight reds and attributes them to one cause.
# Use `git log` to date a capture, and check it sat past 15 s, before reading any red here.
# One wire from that round is NOT yet in force — `igpu-blt`, which ships PENDING; its block below
# carries the promotion sha (`f11e1fc0`) and the reason.
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

# --- WC-D PAYGO: THE SAME PAIR, ON THE SECOND INSTRUMENT --------------------------------------
# Peer commit `0f1d3dfc` (video/wm.rs) gave the wc-d scan-out verify the treatment wcg.rs gave
# the glass read-back: the FIRST verify per window walks a 1-in-16 source-column lattice and
# marks itself `coverage=lattice16`; the deferred verify runs at full coverage past the shared
# 15 000 ms threshold and closes the battery with `[wc-d] paygo … state=complete … -> PAID`.
# `budget=2` is a literal in that format string — wc-d has two STAGES where wc-g budgets four
# samples, and `taken=` counts stages CLOSED.
#
# WHY THIS BLOCK EXISTS AT ALL. The `-> PASS` REQUIRE above is satisfiable by the LATTICE
# verdict alone, so on a post-`0f1d3dfc` build a boot whose deferred verify never arrives stays
# green while the panel read-back covers one source column in sixteen for the rest of the boot —
# every verdict PASS, and honestly so, about the pixels it looked at. That is exactly the hole
# the wc-g block closes with paired directives, reopened one instrument over. The REQUIRE above
# is kept as the cross-build FLOOR (it matches on every build, paygo or not, because
# `coverage=` is an insertion the `.*` spans); this block is the paygo-specific pair.
#
# PROMOTED PENDING → REQUIRE, 2026-08-06 (GR18). These four shipped as PENDING because no
# capture then in existence carried the lines: `0f1d3dfc` landed after the s73 sitting, so
# boots 7 and 8 predated it, and a REQUIRE would have gone falsely red on every capture ever
# taken — the "witness that cannot match a real boot" defect, committed deliberately (mbench's
# PENDING idiom; round6-rmbp.spec is the in-tree precedent). The promotion condition — the
# first capture from a `witness` + `logts` + UNAOS_WCG_PAYGO build at or after `0f1d3dfc` —
# was met by Boot U (metal, s73 capture, kernel `3477640c` @ `7814d258`): mbench replay
# reported "pending 6/6 matched", each flagged "MATCHED: consider promoting to REQUIRE".
# From that boot forward, a build that stops printing any of these four lines goes RED.
#
# VERIFICATION, stated because it is NOT a capture match. Each pattern was checked against
# lines generated from the exact format strings at `0f1d3dfc`, with boot 7's real win=1
# console-window field values substituted:
#   wm.rs:2809  "[wc-d] verify win={} surf={}x{} band={} scale={}x at ({},{}) panel={}x{}
#                checked={}{} bad_cache=0 bad_ram=0 ram_indep={} moved={} nonzero={}
#                cksum={:#018x} first=none -> PASS"
#   wm.rs:3374  wcd_coverage_note(step) -> " coverage=lattice16" | " coverage=full"
#   wm.rs:3335  "[wc-d] paygo win={} state={} emit={} lattice_n={} deferred={} defer_ms={}
#                since_entry_ms={} clock={} taken={} budget=2 -> {}"
# The same four lines are embedded as a fixture in tools/serial-analyzer.py, which now reads
# BOTH instruments' paygo wire (one reader rule, as wm.rs's own comment intends).
#
# Stage 1, and the marker's position is asserted, not spanned: `coverage=` is an INSERTION
# between `checked=` and `bad_cache=`, which is what keeps the pi4 gate's existing `.*` spans
# and `-> PASS`/`-> FAIL` terminals matching what they always matched. Pinning the neighbours
# here is what would catch a marker that drifted to a different field boundary.
# Synthesized line:
#   [wc-d] verify win=1 surf=1312x736 band=0..64 scale=1x at (784,457) panel=2880x1800 checked=83968 coverage=lattice16 bad_cache=0 bad_ram=0 ram_indep=no moved=0 nonzero=8300 cksum=0x6ea90580b6e52525 first=none -> PASS
REQUIRE \[wc-d\] verify win=1 .*checked=[0-9]+ coverage=lattice16 bad_cache=0 bad_ram=0 .*-> PASS
# Stage 2 — the deferred verify actually arrives. Without this the directive above rewards a
# build that samples the panel once at 1/16 and never looks again.
# Synthesized line:
#   [wc-d] verify win=1 … checked=83968 coverage=full bad_cache=0 bad_ram=0 … first=none -> PASS
REQUIRE \[wc-d\] verify win=1 .*checked=[0-9]+ coverage=full bad_cache=0 bad_ram=0 .*-> PASS
# The deferral line, with the two constants that must track the `coverage=lattice16` literal
# (wm.rs asserts `WCD_LATTICE_N == 16` against wcg's `PAYGO_LATTICE_N` at compile time; this
# keeps them honest ON THE WIRE), and `budget=2` — which is also the assertion that nobody
# quietly gave wc-d wc-g's four-sample depth.
# Synthesized line:
#   [wc-d] paygo win=1 state=waiting emit=1 lattice_n=16 deferred=1 defer_ms=15000 since_entry_ms=5186 clock=entry taken=1 budget=2 -> DEFERRED
REQUIRE \[wc-d\] paygo win=[0-9]+ state=waiting emit=[0-9]+ lattice_n=16 deferred=[0-9]+ defer_ms=15000 since_entry_ms=[0-9]+ clock=entry taken=[0-9]+ budget=2 -> DEFERRED
# …and the battery closes. Same reasoning as the wc-g `-> PAID` REQUIRE: a gate in which every
# window sat at `state=waiting` forever satisfies every other directive here and has replaced
# the verify with an indefinite postponement.
# Synthesized line:
#   [wc-d] paygo win=1 state=complete emit=2 lattice_n=16 deferred=7 defer_ms=15000 since_entry_ms=17410 clock=entry taken=2 budget=2 -> PAID
REQUIRE \[wc-d\] paygo win=[0-9]+ state=complete .*taken=[0-9]+ budget=2 -> PAID

# --- WC-D TEARDOWN INTERLOCK: THE ABORT, AND THE BATTERY THAT NEVER ADJUDICATED --------------
# Round 3 (`6f1225b9`) brackets the read-back against foreign panel writes and makes every
# verdict say what the panel did; `98ffcf02` settled the field list (`stable=` on PASS only —
# it was removed from the FAIL arm as a structurally-forced constant, and the aarch64 FAIL arm
# got its coverage slot back). When the interlock catches a write under the read-back, the
# verify ABORTS rather than adjudicating: `-> SKIP (teardown)`.
#
# PI4 NEEDS NOTHING, verified rather than assumed. The whole interlock is arch-gated off
# aarch64: `panel_stable` and `WCD_ABORTS` are `#[cfg(all(feature = "witness", target_arch =
# "x86_64"))]`, and the abort block itself is `#[cfg(target_arch = "x86_64")]` (wm.rs:2844).
# `SKIP (teardown)` has exactly ONE emitter in the tree and it is inside that gate, so no
# aarch64 build can print it. pi4-regression.spec's `REQUIRE \[wc-d\] verify win=.*bad_cache=0
# bad_ram=0.*-> PASS` and `FORBID \[wc-d\] verify .*-> FAIL` are also untouched: on aarch64
# `wcd_coverage_note` returns "" and the plain PASS/FAIL arms (wm.rs:2942/:2969) are
# byte-identical to what they were. Nothing to add there, and nothing to ask the integrator for.
#
# FIELD ORDER MATTERS HERE AND IT IS NOT WHAT IT LOOKS LIKE. `retry=` is printed BEFORE the
# terminal, not after it — the format string ends `… aborts={}/{} retry={} -> SKIP (teardown)`
# (wm.rs:2860). A pattern written as `-> SKIP \(teardown\).*retry=no` reads naturally and can
# NEVER match, because nothing follows the terminal on that line. Stated because it is the exact
# shape of vacuous instrument this file exists to refuse, and it was caught by generating the
# line from the format string instead of writing the regex from the prose description.
#
# THE FORBID — `retry=no`. `retry=` answers "will this window be verified AGAIN", and
# `WCD_ABORT_MAX` is 6, so `retry=no` means the abort budget is spent: `wcd_seal` closes the
# window and the glass is never read for it again. A transient abort (`retry=yes`) is a cost —
# the reference is handed back and the next composite re-verifies. A terminal one is COVERAGE
# ABANDONED, and it is silent on every other line in this spec: the window simply stops
# producing verdicts, and silence is not matchable. This is the directive that makes it loud.
# Synthesized from wm.rs:2860 (Boot R win=3 field values, aborts past WCD_ABORT_MAX=6):
#   [wc-d] verify win=3 surf=128x128 band=none scale=6x at (9,21) panel=2880x1800 checked=36864 coverage=full bad_cache=0 bad_ram=0 ram_indep=no moved=12 nonzero=36864 cksum=0x1122334455667788 first=(9,21) got=0x000000 want=0x1e1e1e rect=768x768+9+21 fills=15->17 fact=0/1 desk=3->4 dact=0/0 aborts=7/6 retry=no -> SKIP (teardown)
FORBID \[wc-d\] verify .*retry=no -> SKIP \(teardown\)
# The SHAPE, reported but never required. `OPTIONAL`, not `PENDING`, and the difference is the
# advice each one gives a reader who sees it match: mbench prints "consider promoting to
# REQUIRE" for a matched PENDING, which is exactly wrong for a fault path — nobody should ever
# REQUIRE a teardown abort. `OPTIONAL` reports it as informational and never fails, which is the
# behaviour wanted and the honest label for it. Same reasoning the pi4 gate applies to `[wc-f]`'s
# retryable `-> DEFER`, which it deliberately leaves unforbidden.
# A `retry=yes` abort is a real event worth seeing in the table (it prices the interlock's
# contention); a `retry=no` one trips the FORBID above as well as showing up here.
OPTIONAL \[wc-d\] verify .*aborts=[0-9]+/[0-9]+ retry=(yes|no) -> SKIP \(teardown\)
# THE SEALED TERMINAL — the battery that never adjudicated. When the abort budget is spent under
# paygo, `wcd_seal` is not enough on its own: a reader following `[wc-d] paygo` would otherwise
# see a window reach `taken=2` and then go silent, sealed and unexplained on the one wire that
# tracks the battery. So it emits its own terminal (wm.rs:2886) — `state=sealed … -> UNPAID`,
# and deliberately not `PAID`, because the coverage was never bought.
# FORBIDDEN outright. Every other paygo directive in this file is satisfied by a sealed window:
# it printed a lattice verdict, it printed a DEFERRED line, its constants were right. What it
# never did is adjudicate its own surface, and this is the only line that says so.
# NOTE the vocabulary: `-> UNPAID` here is the KERNEL's verdict. `serial-analyzer --wcg` also
# prints "open at capture end" for a window still spending its budget when a log ends — a
# statement about the capture, deliberately reworded away from "UNPAID" so the two cannot be
# confused. Only this one is a fault.
# Synthesized from wm.rs:2886 + the paygo format at wm.rs:3929:
#   [wc-d] paygo win=3 state=sealed emit=7 lattice_n=16 deferred=3 defer_ms=15000 since_entry_ms=9000 clock=entry taken=1 budget=2 -> UNPAID
FORBID \[wc-d\] paygo win=[0-9]+ state=sealed .*-> UNPAID

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
# BOTH INSTRUMENTS, one rule. `wcg::paygo_clock` is the shared source — wc-d's gate calls it
# too (wm.rs `wcd_admit`) — so an unarmed clock strands the scan-out verify on its lattice
# stage by the identical mechanism, and the rule is written `\[wc-[gd]\]` rather than left to
# be noticed and re-derived when the next instrument adopts the shape.
# Emitters: `paygo_note` in crates/kernel/src/video/wcg.rs, `wcd_paygo_note` in video/wm.rs.
# Zero hits across all 8 boots of the s73 capture — and REACHABLE: the pattern was matched
# against a hand-reddened copy of boot 7's 15453 ms line with `clock=entry` replaced by
# `clock=unarmed`, which it caught.
FORBID ^\[\s*[0-9]{5,}ms\] .*\[wc-[gd]\] paygo .*clock=unarmed

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

# --- CLOCK-X1: THE WALL CLOCK'S OWN PAYGO SPLIT ----------------------------------------------
# `fca26306` split the x86 wall-clock witness the same way GR17 split the video battery: an ARMED
# half that samples `uptime` and defers, and a VERDICT half delivered from `bootpace::
# service_dump()` on the first service pass. `b4870d14` then taught its cross-check to convict
# and woke the dead backwards branch. Between them they put four distinct lines on the wire and
# left ZERO automated readers — the same gap §10h recorded for `[wc-g]`, one subsystem over.
#
# WHAT THIS GRAMMAR CANNOT SAY, again, and what is bought instead. The witness's real rule is an
# IMPLICATION — "an armed line with no verdict line after it means the boot never reached a
# service pass" — and mbench matches lines independently, with no ordering and no conditionals.
# It cannot express that. What it CAN do is the cheap 90 %: pin that BOTH halves exist, and
# forbid all three fault verdicts. A boot that armed and never delivered then shows as one
# PENDING matched and one not, which is legible in the table even though it is not a verdict.
# The implication itself stays a human rule, and saying so here is the point — an "absence is
# loud" contract that is loud only to humans is worth writing down as such rather than
# pretending the gate covers it.
#
# PROMOTED PENDING → REQUIRE, 2026-08-06 (GR18), for the wc-d block's reason exactly: these
# shipped PENDING because the s73 sitting's first 8 boots predate `fca26306` and print the
# PRE-split verdict with no `[paygo: …]` clause — REQUIREs would have red the spec on every
# capture that then existed. The promotion condition (first capture from an image at or after
# `fca26306` + `b4870d14`) was met by Boot U (metal, kernel `3477640c` @ `7814d258`): both
# matched on mbench replay ("pending 6/6 matched"). From that boot forward these are load-bearing.
#
# VERIFICATION: patterns checked against lines generated from the exact format strings at
# `b4870d14` (`arch/x86_64/syscall.rs`), with boot 7's real field values substituted.
#
# The ARMED half — syscall.rs:6038. `SAMPLED`/`DEFERRED` is the whole claim: the 1 Hz edge is no
# longer paid for inside `sched`, and a build that quietly went back to paying there would stop
# printing this line while every other CLOCK-X1 directive still passed.
# Synthesized line:
#   :: CLOCK-X1: TSC invariant, ~2693 MHz; uptime 15 s SAMPLED — second-advance DEFERRED to the first service pass (pay-as-you-go; a capture with no verdict line below never reached one) == witness ::
REQUIRE :: CLOCK-X1: TSC invariant, ~[0-9]+ MHz; uptime [0-9]+ s SAMPLED — second-advance DEFERRED to the first service pass
# The VERDICT half — syscall.rs:6141. Pinned through the FINAL clause `b4870d14` settled on, field
# by field, because each one is a term the review added for a reason: `deferred N ms TSC / N ms
# APIC` are the two independent timebases the cross-check compares (the old clause carried a
# whole-second APIC count, far too coarse to convict — a 10 % `tsc_hz` error moved a 3 s deferral
# by 300 ms and rounded away entirely), and `core=` names which CPU delivered the pass. A clause
# that lost the TSC term would silently return the cross-check to the state that could not
# convict, and would still match a pattern written loosely enough to span it.
# Both terminals are accepted here on purpose: this directive asserts the INSTRUMENT DELIVERED,
# and the SKEW half is the FORBID's business below — the same split the wc-g rollup uses.
# Synthesized line:
#   :: CLOCK-X1: TSC invariant, ~2693 MHz; monotone (rdtsc +2143545592); uptime 15->16 s (JD17 x86-frozen clock now advances) [paygo: deferred 796 ms TSC / 800 ms APIC, uptime +1 s, core=0 — CONSISTENT] == witness ::
REQUIRE \(JD17 x86-frozen clock now advances\) \[paygo: deferred [0-9]+ ms TSC / [0-9]+ ms APIC, uptime \+[0-9]+ s, core=[0-9]+ — (CONSISTENT|SKEW)\]
#
# The three fault verdicts. NONE of them carries `-> FAIL` — they all end `== witness ::`, the
# uncounted idiom — so the default FORBID set (`-> FAIL`, `FAIL ::`, `PANIC`) sails straight past
# every one. That is precisely why they need naming here, and it is the same trap x86-fat.spec
# documents for the STOR-1 witnesses.
#
# FROZEN — syscall.rs:6125. The JD17 defect itself, un-fixed: uptime not advancing at all. It now
# prints BOTH deadlines (`N ms APIC / N ms TSC`) because the APIC deadline alone cannot survive
# the case it most needs to — if the tick is dead too, `elapsed_ms` stays 0 forever and the poll
# goes silent for the whole boot, i.e. the deadline would be measured by one of the counters
# whose death it exists to report.
# Synthesized line:
#   :: CLOCK-X1: FROZEN — uptime still 15 s after 3000 ms APIC / 2998 ms TSC (rdtsc +8076000000, core=0); the JD17 second derivation does NOT advance == witness ::
FORBID :: CLOCK-X1: FROZEN
# SKEW — the cross-check convicting. The two timebases disagree by more than the scaled tolerance,
# so one of them is lying about how much time passed; which one is the investigation, but neither
# answer is healthy. Anchored on the clause rather than the bare word so it cannot be satisfied by
# the word appearing in some future comment or unrelated line.
# Synthesized line (the CONSISTENT line above with its terminal flipped):
#   … [paygo: deferred 796 ms TSC / 1400 ms APIC, uptime +1 s, core=0 — SKEW] == witness ::
FORBID :: CLOCK-X1: .*\[paygo: .*— SKEW\]
# NON-MONOTONE — syscall.rs:6109, and REACHABLE now rather than dead. `b4870d14` moved the
# backwards test AHEAD of the frozen branch: `u2 <= u1` swallowed `u2 < u1`, so a backwards uptime
# was reported by the frozen line, which prints `u1` — the PRE-regression second — and hid the
# fault behind a healthier-looking verdict. Either counter running backwards breaks the clock's
# monotonicity contract.
# Synthesized line:
#   :: CLOCK-X1: NON-MONOTONE — rdtsc 900000000 -> 800000000, uptime 16 -> 15 s (core=3); the clock's monotonicity contract is broken == witness ::
FORBID :: CLOCK-X1: NON-MONOTONE

# --- WXN-x86 M1: THE PDPT NX SWEEP AND ITS VERDICT -------------------------------------------
# `a0a2d163` put the first NX bit this kernel has ever written into its own map, and `32724cb4`
# gave the sweep a VERDICT after its adversarial review found the milestone could be completely
# vacuous and still print a line that read entirely normal (memory.rs F6, quoted in full at the
# emitter). That verdict has had no automated reader since it landed, which is the same gap §10h
# recorded for `[wc-g]` — one subsystem over, with a protection rather than an instrument behind it.
#
# NOTE ON THE PREFIX, because it is the one place this file's timestamp idiom does not apply: this
# whole block fires from `arch::init`, BEFORE the bootpace entry stamp, so every line here carries
# `[      ?ms]` rather than a millisecond count. No `[0-9]{5,}ms` guard is available on these rules
# and none is wanted — they are once-per-boot lines with no late-boot failure mode to key on.
#
# THE POSITIVE WITNESS. `pdpt_seen`/`nx_set`/`residue_leaves` are named and shaped rather than
# valued: the first two are functions of the firmware's map (1024 seen / 1022 set on this bench,
# the two spared GiBs being the image and the AP trampoline) and `residue_leaves` is what the
# WXAUDIT line below must independently report as `kern_WX`. Pinning any of them by value would
# make this spec firmware-specific for no gain; pinning them by NAME catches the field going away.
# Capture line (Boot W; Boot X identical but for `ehdr=0x7B235000` / `img=[0x7B235000,0x7B8E7290)`):
#   :: WXN-x86: ehdr=0x7B233000 img=[0x7B233000,0x7B8E6DEA) gib_img=1 gib_tramp=0 spare_n=2 pdpt_seen=1024 nx_set=1022 huge_leaf_nx=0 skip_spare=2 skip_user=0 skip_pml4_user=0 skip_selfmap=0 already_nx=0 skip_fb_lock=0 skip_fb_base=0 skip_fb_walk=0 residue_leaves=1535 (1g=0 2m=1023 4k=512 pt=1) pge=0 flush=cr3-reload wp=0 -> SWEPT ::
REQUIRE :: WXN-x86: ehdr=0x[0-9A-F]+ img=\[0x[0-9A-F]+,0x[0-9A-F]+\) .*pdpt_seen=[0-9]+ nx_set=[0-9]+ .*residue_leaves=[0-9]+ .*wp=[0-9]+ -> SWEPT ::
# THE TWO NEGATIVE TERMINALS, and an honest account of what they add. Both are structurally
# EXCLUSIVE with the REQUIRE above — `wxn_pdpt_sweep` has exactly one call site (arch/x86_64/mod.rs:46)
# and prints exactly one terminal per boot — so a VACUOUS or REFUSED boot already reds this spec by
# way of the missing SWEPT. What the FORBIDs buy is the DIAGNOSIS: mbench's replay path prints the
# offending line for a FORBID hit and only a pattern for an unmatched REQUIRE, so with these two
# rules a failed gate hands the reader `skip_pml4_user=1024 pdpt_seen=0` — the actual cause — instead
# of "required witness missing". That is worth two lines, and it is the whole claim being made for
# them; they are not asserted to catch anything the REQUIRE cannot.
# `-> VACUOUS` — `wxn_pdpt_sweep`'s F6 verdict (memory.rs:1907 at `32724cb4`; anchor on the
# function, the line drifts). `nx_set == 0` while the map has unspared PDPT entries: the sweep
# wrote NOTHING. The concrete route is firmware that set U/S on its identity PML4 entries, at which
# point every descent takes `skip_pml4_user` and the milestone is a no-op that logs like a success.
# The failure is fail-safe (the sweep can only under-protect), which is why it is a line and not a
# panic — and precisely why an automated reader is the only thing that will ever notice it.
# REACHABLE: matched against the emitter's format string with `nx_set=0 pdpt_seen=0
# skip_pml4_user=1024 residue_leaves=66047` substituted (the U/S map Boot V actually shows the
# WXAUDIT half of — `kern_WX=66047`), which it catches.
FORBID :: WXN-x86: .*-> VACUOUS ::
# `-> REFUSED` — three emitters, all fail-closed early returns that write no entry, all three inside `wxn_pdpt_sweep` (memory.rs:1687
# (EFER.NXE clear, so bit 63 is RESERVED and every entry would fault the next translation through
# it), :1696 (the phdr walk found no image bounds, so the sweep cannot know which GiB to spare) and
# :1720 (the image spans more 1 GiB regions than WXN_MAX_SPARE). Each is the RIGHT behaviour for its
# condition and each leaves the kernel map unprotected, so each is a fault to report, not to hide.
# REACHABLE: matched against all three format strings, generated with this bench's field values.
FORBID :: WXN-x86: .*-> REFUSED

# --- WXAUDIT: THE MAP CENSUS, AND THE HISTOGRAM APPENDED UNDER IT -----------------------------
# The audit walk `wx_audit_report` (memory.rs:1152) publishes after `syscall::init` has armed
# EFER.NXE and CR4.SMEP, so its numbers describe an actually-enforcing kernel. There was NO reader
# of this line in any spec in the tree — not here, not in x86-fat.spec, not in rmbp-boot.spec —
# which is stated because the natural assumption is that a line this old must already be covered.
#
# `l1=/l2=/l3=` are `32724cb4`'s leaf histogram (1 GiB / 2 MiB / 4 KiB counts over the WHOLE map),
# APPENDED and never inserted: the emitter's own comment makes that the discipline so every existing
# `awk` over this line keeps matching. This directive is what turns the discipline into a check —
# it names the pre-existing fields IN THEIR ORIGINAL ORDER and then requires the histogram after
# them, so an edit that inserted a field mid-line (breaking every positional reader on the bench)
# goes red here rather than being discovered by a reader whose columns silently shifted.
# The pattern deliberately STOPS at `l3=` rather than anchoring ` ::`, because the emitter appends
# one more optional token — see the FORBID below. Two independent claims, two rules.
# `kern_WX=` is not pinned by value on purpose: it is `residue_leaves` from the sweep above (1535 on
# this bench, from 66047 before the sweep existed — Boot V shows exactly that), and it becomes an
# ASSERTED zero at M3, not here.
# Capture line (Boot W; Boot X identical but for `walk=1717kcyc`):
#   :: WXAUDIT x86: leaves=66047 user=0 user_WX=0 kern_WX=1535 (2048 MiB) tables=1028 nxe=1 walk=1720kcyc l1=0 l2=65535 l3=512 ::
REQUIRE :: WXAUDIT x86: leaves=[0-9]+ user=[0-9]+ user_WX=[0-9]+ kern_WX=[0-9]+ \([0-9]+ MiB\) tables=[0-9]+ nxe=[0-9]+ walk=[0-9]+kcyc l1=[0-9]+ l2=[0-9]+ l3=[0-9]+
# ` TRUNCATED` — the walk ran out of budget and the census above describes PART of the map. This is
# the one token that makes every number on that line an underestimate, including `user_WX=0`, which
# is the audit's whole point: an unwalked subtree cannot report the W∧X page it contains. The
# REQUIRE above matches a truncated line perfectly well (that is what stopping at `l3=` costs), so
# without this rule a truncated audit reads as a clean one. Emitter: the `a.truncated` tail of
# `wx_audit_report`'s census line (memory.rs:1165).
# REACHABLE: matched against the format string with `truncated=true` substituted onto Boot W's
# field values, which it catches. Zero hits on Boot V, W and X.
FORBID :: WXAUDIT x86: .* TRUNCATED ::

# --- WXAUDIT-NXE: NX IS PER-CORE MSR STATE, AND THE CENSUS ASKED THE BSP ----------------------
# The witness `a0a2d163` added for the hazard the audit above structurally cannot see: `nxe=1` on
# the WXAUDIT line is ONE core's EFER, read once, while the NX'd identity map is SHARED — every AP
# runs on the BSP's CR3. A core whose EFER.NXE is clear ignores every bit the sweep wrote, and the
# census would still print `nxe=1`. Each core ORs its own live MSR bit after its own `syscall::init`;
# `cores` is what SMP believes is online and `nxe` is how many proved it (`wxn_nxe_report`, smp.rs:113).
# `-> PASS` IS the equality `armed == cores` — the emitter computes the terminal from it — so the
# terminal is the whole assertion and no `cores=(N) nxe=\1` backreference is needed or wanted.
# `wp_mask` rides along and is deliberately not constrained: the rMBP firmware leaves CR0.WP=0 (QEMU
# leaves it 1), M1 does not set it, and NX enforcement is independent of WP. A `wp=0` reading here
# is the documented metal state, not a fault.
# NO NEW FORBID, and this is the answer rather than an omission: the failure arm (`wxn_nxe_report`'s `else`, smp.rs:120) prints
# `… -> FAIL ::`, which BOTH default FORBIDs (`-> FAIL`, `FAIL ::`) already catch. Naming it again
# would be a rule that cannot fail independently of one already in force. This is the opposite of the
# CLOCK-X1 case above, where all three fault verdicts end `== witness ::` and the defaults sail past
# them — the distinction is worth stating, because "add a FORBID for every failure arm" is the wrong
# rule and this file should not look like it follows one.
# Capture line (Boot W @ 171 ms; Boot X identical):
#   :: WXAUDIT-NXE: cores=8 nxe=8 nxe_mask=0xFF wp=0 wp_mask=0x0 -> PASS ::
REQUIRE :: WXAUDIT-NXE: cores=[0-9]+ nxe=[0-9]+ nxe_mask=0x[0-9A-F]+ wp=[0-9]+ wp_mask=0x[0-9A-F]+ -> PASS ::

# --- WXN-FBWC: THE GR15 TRIPWIRE THE SWEEP CARRIES ---------------------------------------------
# GR15's defect — `map_mmio_window` silently un-typing the framebuffer from WC back to UC, 8.7-9.1x
# on the blit path, invisible to every permission instrument for two weeks — is why the NX sweep
# reads the fb leaf before and after itself. The interlock's panicking arms are covered by the
# default `PANIC` FORBID and need nothing here (all three in `wxn_pdpt_sweep`'s fb interlock — a
# level change across the sweep, memory.rs:1951; a
# leaf bit other than PTE_NX moving, :1972; the mapping vanishing, :1998). What needs a rule is the
# arm that is NOT a panic.
#
# THE REQUIRE, and its scope limit stated rather than discovered. `-> LEAF BIT-IDENTICAL` is the
# `delta == 0` arm: the fb leaf is below the PDPT (lvl 2 on this bench), the sweep wrote only
# parents, and NOTHING about the leaf moved — `pat=1` still says WC. The emitter has a second
# CORRECT arm, `-> LEAF NX-ONLY (fb is a 1G leaf this sweep NX'd; expected)`, reachable only when
# the walk terminates at lvl 3; a host whose firmware maps the fb as a 1 GiB leaf would take it and
# would red this REQUIRE. That is a SCOPE question, not a defect, and the honest place to answer it
# is a `(BIT-IDENTICAL|LEAF NX-ONLY)` widening at the moment such a capture exists — not now, on a
# guess, which would weaken a live rule to accommodate a machine nobody has booted. Verified by
# generating the NX-ONLY line from the format string: this pattern does NOT match it.
# Capture line (Boot W; Boot X byte-identical):
#   :: WXN-FBWC: fb=0x90020000 lvl=2 e=0x00000000900010E3 pat=1 pcd=0 pwt=0 w=1 fx=0 -> LEAF BIT-IDENTICAL ::
REQUIRE :: WXN-FBWC: fb=0x[0-9A-F]+ lvl=[0-9]+ e=0x[0-9A-F]{16} pat=[0-9]+ pcd=[0-9]+ pwt=[0-9]+ w=[0-9]+ fx=[0-9]+ -> LEAF BIT-IDENTICAL ::
# `-> SKIPPED` — FORBID, not PENDING, and the choice is not close. PENDING means "a witness whose
# code is ahead of its bench" and prints "consider promoting to REQUIRE" when it matches, which is
# nonsense for this line: nobody should ever want the interlock skipped. The emitter — the `else` arm
# of `wxn_pdpt_sweep`'s `if let Some((e_before, l_before)) = fb_before` (memory.rs:2008) —
# exists so that a capture with no `WXN-FBWC:` in it is distinguishable from a build that never had
# the tripwire — F2's own reasoning — and it fires when the WRITER lock was contended, the fb base
# was unknown, or the pre-sweep walk failed. Any of those means THE GR15 TRIPWIRE DID NOT RUN and
# the sweep's effect on the panel mapping was never checked.
# Like the WXN terminals above this is exclusive with the REQUIRE and so cannot red a run the
# REQUIRE would have passed; it is here for the same reason, and it earns it more sharply — the
# `skip_lock=/skip_base=/skip_walk=` fields it puts in the failure report are the entire diagnosis,
# and they appear on NO other line.
# REACHABLE: matched against the format string with `skip_lock=1` substituted, which it catches.
FORBID :: WXN-FBWC: .*-> SKIPPED ::

# --- EPACE-TRIM M8: THE 8510's NAK, AND THE FALSIFIER THAT MUST STAY SILENT --------------------
# M8 (`7498436f`) names WHICH control request eats the ~52 ms the `05ac:8510` spends NAKing through
# enumeration. Its threshold is 8 ms against a healthy per-transfer cost of 0.13 ms — ~62x — and the
# emitter's own contract is that HEALTHY BOOTS PRINT ZERO LINES.
#
# WHY THE [0] LINE IS `OPTIONAL` AND NOT `REQUIRE`, which is the whole decision in this block. This
# file distinguishes WITNESS-PRESENCE (assert it) from BEHAVIOUR (report it, never require it), and
# an M8 line is behaviour: it is a defect being measured, not an instrument proving it is alive.
# REQUIREing it would build a gate that goes RED WHEN THE BUG IS FIXED — the mirror image of the
# vacuous-instrument disease every other comment here is written against. That is not hypothetical:
#   * Boot V (pre-`609d9b3a`):  `[0] … addr=0 … wlen=8  … xfer=50ms` — the MPS0 pre-read.
#   * Boot W (post-`609d9b3a`): `[0] … addr=2 … wlen=18 … xfer=47ms` — BUY-2 dropped the 8-byte
#     pre-read for HS targets, that line vanished, and the NAK reappeared on a DIFFERENT request.
# One landed fix already moved this line once between adjacent captures. A REQUIRE pinning `wlen=18`
# would have gone red on the very commit that bought the 50 ms back.
# The cost of OPTIONAL is real and is stated rather than glossed: nothing here proves the M8 meter is
# still armed, because its healthy output is silence and silence is not matchable. `--slowxfer` in
# tools/serial-analyzer.py is the reader that prices what IS printed (it exits FINDING on a [1] line);
# `EPACE: [n] … {xfer=…(n=…) ass=… act=…}` is the always-printed transport meter next door. Recorded
# here so the next reader does not mistake the OPTIONAL for an oversight.
# Capture line (Boot W @ 966 ms; Boot X @ 968 ms with `xfer=48ms act=48ms`):
#   :: EHCI-HID: [0] EPACE-TRIM M8 SLOW-XFER addr=2 hub=0.0 spd=HS bmreq=0x80 breq=0x06 wval=0x0100 widx=0x0000 wlen=18 stg=3 xfer=47ms act=47ms ass=0ms seq=1/8 == witness ::
OPTIONAL :: EHCI-HID: \[0\] EPACE-TRIM M8 SLOW-XFER addr=[0-9]+ .*wlen=[0-9]+ stg=[0-9]+ xfer=[0-9]+.* == witness ::
# CONTROLLER [1] IS THE FALSIFIER, and this one IS a hard rule. M8's docstring states the prediction
# it was built to be wrong about: lines on [0] only, and ZERO on [1], whose 82 control transfers cost
# 11 ms total across n=3 boots. A line on [1] falsifies the verdict's central claim — that the 52 ms
# is one device's own answer latency and not a driver-side per-transfer cost — and would mean either
# the meter or the 8 ms threshold is wrong. Either way the BUY-2 reasoning that has already been
# spent on this path rests on it, so it goes red rather than into a table nobody reads.
# Note this FORBID is NOT redundant with the OPTIONAL: they match different controllers, and a boot
# can print both. Emitter: crates/kernel/src/drivers/ehci/mod.rs:806 (`slow_xfer_witness`, `self.idx`
# is the controller index). Zero hits on Boot V, W and X.
# REACHABLE: matched against Boot W's own line with `[0]` replaced by `[1]`, which it catches.
FORBID :: EHCI-HID: \[1\] EPACE-TRIM M8 SLOW-XFER

# --- IGPU-BLT: THE CENSUS THAT DOES NOT FLY YET (PENDING) --------------------------------------
# PENDING, with the promotion condition named — the idiom this file already uses twice above, and
# the honest label for a witness whose code is ahead of every capture in existence.
#
# `6283dde3` brought up an IVB BLT ring for console fill/scroll, refusing rather than panicking down
# every path an accelerator can fail on. On THIS bench the gmux routes the panel to the Kepler, so
# every iGPU display plane reads zero, `active_surf` is None — and until `f11e1fc0` that case fell
# through in SILENCE. Boot X is the capture that proved it: the census printed nothing at all, which
# the playbook had called the worst outcome, and `f11e1fc0` added the outermost refusal arm so the
# fact is one `awk` away instead of an absence. Boot W predates the ring entirely.
# So there is NO capture carrying this line, and a REQUIRE would be falsely red on every log ever
# taken — the "witness that cannot match a real boot" defect, which this file refuses to commit.
#
# PROMOTION CONDITION: the first x86 metal capture from an image at or after `f11e1fc0`. On that
# capture mbench will report this PENDING as matched and flag "consider promoting to REQUIRE";
# do it then, and this block loses its PENDING and gains its real capture line.
#
# THE CLAIM the pattern makes is deliberately just `ring=`, spanning every arm: the module says
# SOMETHING about the ring on every boot. WHICH arm it says is the finding — `absent
# why=no-active-surface` is expected on the dual-GPU rMBP until the gmux switch lands, `up` is
# expected on an iGPU-only host — and pinning an arm would pin a machine. What must never happen
# again is the census being silent, and `ring=` is exactly that assertion.
# ONE HONEST CAVEAT, since it bounds what a future green means: the refusal arms live in `igpu::init`
# downstream of the BAR0 mapping, which has its own early return (`igpu::init`, igpu.rs:263). A host where BAR0
# does not map prints `[Intel iGPU] Error: … not mapped. Probe aborted.` and no census line — that is
# a different fault with its own loud line, and this rule would report it as a missing witness.
# VERIFICATION — NOT a capture match. The pattern was checked against lines generated from both
# shapes of the emitter's format strings:
#   igpu.rs:381  ":: igpu-blt: ring=absent why=no-active-surface — every iGPU display plane is off (gmux routes the panel elsewhere); CPU path carries the console ::"
#   igpu.rs:646  ":: igpu-blt: ring={} fills={} scrolls={} fallbacks={} spins_max={} ::"  ->
#                ":: igpu-blt: ring=up fills=1204 scrolls=88 fallbacks=0 spins_max=17 ::"
# It catches both. Zero hits on Boot V, W and X, as expected.
PENDING :: igpu-blt: ring=

# --- SMC: THE TRUNCATION COUNTER AND THE WALK THAT MUST NOT RE-WEDGE ---------------------------
# `late=` (`bdfb3b4c`) is the counter that made "every key whole" a MEASUREMENT instead of an
# inference. The truncation arm used to be invisible: a value byte that arrived after the read
# stopped was drained and discarded in silence by `close_transaction`, and `unc` — which counts only
# drains that FAIL — stayed 0, so `unc=0` did not mean "no truncation". The two arms now partition
# it. The field is APPENDED, never inserted (`batt_witness`'s own comment, smc.rs:1722), so a positional `awk` over an older log
# still lines up; this directive names the tail of the pre-existing field list and then requires
# `late=` after it, which is what turns that discipline into a check.
# PRESENCE, NOT VALUE, and here the reason is specific rather than stylistic: `late` also counts one
# BENIGN shape — a read that stopped because it filled the caller's buffer on a key longer than the
# buffer (`read_u16k`'s 2 bytes against a >2-byte key). A discarded byte either way, and the two are
# told apart only by WHICH KEY was read, which is not on this line. `late=0` is therefore not a
# verdict this grammar can pronounce; `--smc` in tools/serial-analyzer.py is where that reading lives.
# Emitter: crates/kernel/src/drivers/smc.rs:1724.
# Capture line (Boot W @ 1843 ms; Boot X @ 1845 ms):
#   :: SMC-BATT: present=true soc=90% volt=12589mV amp=519mA full=9962mAh rem=9009mAh ac=derived:charging retries=0/0 st0=0 rfail=0 rok=0 short=0 unc=0 gap=976 busy=30 late=0 == witness ::
REQUIRE :: SMC-BATT: present=.*gap=[0-9]+ busy=[0-9]+ late=[0-9]+ == witness ::
# THE WALK, and why its summary line is load-bearing where its output was not. Boot V measured the
# per-name index dump at 493 lines / ~25 KB, and the FTDI console's drain of that block displaced the
# storage bring-up behind it by ~3.5 s. `a2cada19` demoted the DUMP (now behind `UNAOS_SMCWALK=1`)
# and kept the WALK, because `read_key_by_index` is the GAP-1 sibling fix's only exerciser. This one
# line is what remains, and the emitter's own comment states the contract: "a shortfall, or this
# line's absence, says the GAP-1 sibling fix has re-wedged."
# `walked == count` is NOT pinned by backreference, though the contract invites it: the loop runs to
# `count.min(MAX_ENUM_KEYS)` with `MAX_ENUM_KEYS = 512` (smc.rs:941/1047), so an SMC with more than
# 512 keys legitimately reports `512 of N` and the equality would be a false red on that machine.
# This bench answers 493 of 493. The re-wedge itself is caught by the FORBID below, which is the
# sharper rule anyway — it names the fault instead of inferring it from an arithmetic shortfall.
# Emitter: the WALK-QUIET summary at the tail of the `#KEY` index loop, smc.rs:1108. Capture line (Boot W @ 1842 ms; Boot X @ 1843 ms; Boot V @ 1849 ms):
#   :: SMC-SCOUT: index walk done (493 of 493 names) ::
REQUIRE :: SMC-SCOUT: index walk done \([0-9]+ of [0-9]+ names\) ::
# THE RE-WEDGE, forbidden. `bdfb3b4c` split "the SMC has no key at this index" (a clean stop, and a
# normal end to an enumeration) from "a handshake wedged" (a fault that happens to end it too) so the
# two stopped sharing a line. Boot U's `index enumeration STOP-NOTE at idx 0` is the exact reading
# the sibling fix was written against — the walk dying on its first index — and its reappearance
# means the fix has regressed. Bounded and never forced, so the boot survives it; this is what makes
# it loud. Note it is invisible to the REQUIRE above WHEN IT IS NOT AT idx 0: the loop breaks and
# `index walk done (N of M names)` still prints, with a short `N` that nothing else convicts.
# Emitter: smc.rs:1092-1096 (`Err(SmcError::Stuck(step))`).
# REACHABLE: matched against the format string with Boot U's `idx 0` reading substituted, which it
# catches. Zero hits on Boot V, W and X.
FORBID :: SMC-SCOUT: index enumeration STOP-NOTE at idx [0-9]+

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
#   * `state=closed … -> UNSPENT`. NOT forbidden, and it is the one paygo terminal that most looks
#     like it should be. A window that closes with budget left has spent nothing further on a
#     surface that no longer exists — a fixture tearing down mid-battery, which is normal — and the
#     terminal exists so the wire says so instead of the window going silent mid-census. Boots W and
#     X carry many of them, all on win=3, all legitimate. The emitter draws the distinction itself
#     (video/wcg.rs:2221 names this spec and the `state=sealed … -> UNPAID` rule it DOES forbid):
#     sealed means the coverage was never bought after the abort budget was spent, closed means
#     there was nothing left to buy it for. Only the first is a fault.
#   * A COUNT of `-> PAID` terminals. The REQUIRE above asks for one and deliberately not for four:
#     `emit=`/`taken=` are still climbing on the console window when a capture ends (Boot W ends with
#     win=1 PAID at 20682 ms and win=3 still spending), so any fixed count would be a claim about
#     how long the bench sat rather than about the kernel. Boots W and X each carry three `[wc-g]`
#     and three-to-four `[wc-d]` PAID lines; the analyzer reports the per-window split.
#   * `[wc-h]` / `[wc-k]` / `[cursor*]`. Real witnesses, thoroughly pinned by the pi4 gate, and
#     out of this spec's subject. Adding half-considered copies here would grow the file without
#     growing the evidence.
