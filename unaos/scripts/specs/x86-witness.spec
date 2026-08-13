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
# Every wire from that round is now in force. `igpu-blt` was the last one still PENDING; Boot Y
# (2026-08-07 metal, image built at `776fb13c`, i.e. past its promotion sha `f11e1fc0`) matched it —
# mbench reported "pending 1/1 matched", flagged "consider promoting to REQUIRE" — and its block
# below carries the promotion and the capture line that earned it.
#
# A SECOND MINIMUM-GENERATION FLOOR, kept separate from the one above because it is a different sha
# and a different arc: the WXN-M2 directives below need an image at or after `e8b11513` (x86/wxn: M2
# — the huge-leaf splitter), the first build that can print a `:: WXN-M2:` line at all. On any
# capture older than that the M2 REQUIRE is red because the wire did not exist, exactly as the
# `32724cb4` block is red on Boot V; Boot Y is the first capture that carries it. The BPACE/GPACE
# block at the end of this file has NO generation floor — the boot-pace ledger is ungated
# (bootpace.rs says so in as many words) and predates every capture in the bench archive.
#
# A THIRD SET OF FLOORS, GR20 (2026-08-07), and they are listed per-directive rather than as one sha
# because unlike the GR18 block these wires landed in four unrelated arcs and their blocks are spread
# through this file:
#   `8c8eb802`  `:: WXAUDIT-CORES:` — the per-core CR0 witness (M3a). Boot AA carries WXAUDIT-NXE and
#               NOT this line: that is the pre-floor capture, and it is what the floor means.
#   `1d0d93c6`  `:: video: edid` — the EDID carry-through. Also absent from Boot AA.
#   `f94e280c`  `:: sdhc: card v1.x …` + `CARD IDENTIFIED` on THIS bench's card. The emitters are far
#               older, but a pre-v2.00 card could not be identified at all before this commit, so on
#               the bench's 29 MiB v1.x card the practical floor is here. BOOT AB IS THE PROOF and it
#               is a better one than a claim: its capture reads `card-inserted=1`, `bus ready …
#               card present`, then `cmd8 send-if-cond FAILED … card is pre-v2.00 or absent` and the
#               identification stops. Card in the slot, healthy controller, no witness — the exact
#               shape of a pre-floor red, and NOT a media problem.
#   `b2f4a090`  `:: sdhc: w1 …` — the SDHC-4a write self-test's verdict line.
#
# A CARD-PRESENCE SCOPE AXIS, third alongside the knob set and the generation floors, and it belongs
# to the three `sdhc:` REQUIREs only. Those lines print once per boot ON WHICH A CARD WAS IDENTIFIED;
# the bench's boot volume is the 59 GiB USB stick, so the SD reader's card is scratch media that a
# sitting could in principle run without. A capture taken with an empty reader prints
# `[sdhc] bdf 3:0.1 bus is up (powered, clocked) but NO CARD is inserted — nothing to identify`
# and reds those three directives. That line is named here so the reader diagnosing such a red can
# tell "wrong capture for this spec" from "the wire died", which is the whole distinction the
# generation-floor paragraphs above exist to preserve. Boots AC/AD/AE/AF all carry a card.
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

# --- WXN-x86 M2: THE HUGE-LEAF SPLITTER, AND THE REFUSAL NOTHING WAS WATCHING -----------------
# `e8b11513` is the milestone that takes `kern_WX` from M1's 1535 to 305 on this bench: inside the
# GiBs M1 had to spare whole, M2 NX's every 2 MiB leaf with no executable code in it, splits the ONE
# leaf that straddles `.text`, and leaves executable exactly `xpages + 1` pages (the ELF's executable
# extent plus the AP trampoline page). It had NO reader anywhere in the tree when it flew — zero
# references in any `.spec`, zero in tools/serial-analyzer.py — which is the finding this block
# closes, and it is a sharper one than the usual "an emitter stopped printing":
#
#   THE MILESTONE FAILING TO HAPPEN LOOKED EXACTLY LIKE THE MILESTONE HAPPENING. If M2 refuses,
#   `kern_WX` simply stays at M1's 1535, every other directive in this file still passes, and
#   `--wxn` reports "ok kern_WX never rose" — a true sentence that reads as success. mbench would
#   have printed a full PASS on a boot where the splitter refused outright.
#
# THE POSITIVE WITNESS. Named-and-shaped, never valued, for this file's standing reason: `nx_2m`
# and `nx_4k` are functions of the firmware's map and `xpages` of the image's size, so pinning any
# of them would make this spec bench-specific. What the pattern DOES pin is that every arm of the
# report is present and in the emitter's order — `xseg=`/`xpages=` (the ELF-derived keep set),
# `demote_1g=`/`split_2m=`/`pool_used=` (the structural edits and the static pool they came from),
# `nx_pdpt=`/`nx_2m=`/`nx_pt=`/`nx_4k=` (the four retirement levels, which is the field group
# `--wxn` reconciles the sweep against the audit with) and `keep_x=` (what survives executable).
# A field that went away, or an insertion that reordered them, reds HERE rather than silently
# breaking the analyzer's arithmetic one screen over.
# `pool_used=N/M` is pinned as a PAIR on purpose: the denominator is `WXN_POOL_CAP`, and a pool
# resized without the pre-pass being resized with it is the one condition that reaches
# `panic!("WXN-M2: pool exhausted mid-edit")` — which the default PANIC FORBID would then catch, but
# only after the boot has died. Seeing the ratio on every boot is what makes it predictable instead.
# Capture line (Boot Y, 2026-08-07 metal — the FIRST capture carrying this wire):
#   :: WXN-M2: xseg=[0x7B21C000,0x7B34C000) xsegs=2 xpages=304 tramp=0x8000 spare_n=2 demote_1g=0 split_2m=1 pool_used=1/16 nx_pdpt=0 nx_2m=1022 nx_pt=0 nx_4k=719 keep_x=305 already_nx=0 skip_user=0 fb=0x90020000 fb_delta=0x0 pge=0 flush=cr3-reload -> SPLIT ::
REQUIRE :: WXN-M2: xseg=\[0x[0-9A-F]+,0x[0-9A-F]+\) xsegs=[0-9]+ xpages=[0-9]+ .*demote_1g=[0-9]+ split_2m=[0-9]+ pool_used=[0-9]+/[0-9]+ nx_pdpt=[0-9]+ nx_2m=[0-9]+ nx_pt=[0-9]+ nx_4k=[0-9]+ keep_x=[0-9]+ .*-> SPLIT ::
# `-> REFUSED`, and this is the rule the milestone actually needed. The existing
# `FORBID :: WXN-x86: .*-> REFUSED` is scoped to the M1 tag and CANNOT match an M2 line — a fact
# worth stating because the two rules look interchangeable and are not. M2 has four refusal arms
# (memory.rs `wxn_split_stage`: already run and the pool's pages are live tables; no
# read-only PT_LOAD, so no executable extent; an executable extent outside the image; and a static
# pool too small for the pre-pass's `need_pd`/`need_pt`), and the LAST of those is the falsifier the
# Boot Y playbook committed to in writing — "a REFUSED arm ⇒ pool sizing wrong". Every arm returns
# before a single entry is written, so the kernel map keeps M1's whole 1535-leaf residue.
# REACHABLE: matched against all four format strings, generated with Boot Y's field values. The
# first arm carries no fields at all (`:: WXN-M2: -> REFUSED (already run; …) ::`), which is why the
# `.*` is allowed to span nothing.
FORBID :: WXN-M2: .*-> REFUSED
# `-> VACUOUS` — the F6 verdict M1 carries, adopted by M2 for the identical reason: `wrote == 0`
# after a descent that did not refuse, i.e. every entry took a `skip_user` branch and the pass is a
# no-op that logs like a success. Exclusive with the REQUIRE above, and here for the diagnosis the
# REQUIRE cannot give: mbench prints the offending line for a FORBID hit and only a pattern for a
# missing REQUIRE, so this rule hands the reader `skip_user=1024 keep_x=0` instead of "witness
# missing". Emitter: the `wrote == 0` terminal at the tail of `wxn_split_stage`.
FORBID :: WXN-M2: .*-> VACUOUS ::

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

# --- WXAUDIT-CORES: THE MASKS' OWN EVIDENCE, PER CORE ------------------------------------------
# `8c8eb802` (GR20) added the line the census above cannot be checked without. WXAUDIT-NXE publishes
# two BITMASKS — `nxe_mask` and `wp_mask` — and its `-> PASS` is a popcount identity over them. What
# nothing witnessed until this line was the MSR/CR0 readings those bits were derived from: a mask is
# a claim about eight cores made in one word, and a publication-ordering bug that left a slot unread
# would still popcount to whatever the other cores set. `wxn_cores_report` (smp.rs:189) prints each
# core's live CR0 verbatim, BSP-emitted from the array the APs filled, immediately after the census
# it cross-checks — so `wp_mask` bit i can be reconciled against bit 16 of core i's real CR0 by an
# analyzer instead of being believed.
# The wire is exactly as gated as WXAUDIT-NXE — same function, unconditional call at smp.rs:162 — so
# this REQUIRE adds no scope axis, only a floor (`8c8eb802`, see the header).
# NOTHING IS PINNED BY VALUE. `n=` is the core census (8 on this bench, 1 on a UP host), the CR0
# values are firmware state, and `wp=`/`nxe=` are masks whose width follows `n`. What IS pinned is
# that the array is NON-EMPTY and comma-separated hex: `cr0=[]` beside `n=8` is a witness reporting
# nothing while looking like a report, and that is the one shape the pattern refuses.
# Capture line (Boot AF @ 171 ms; AC/AD/AE byte-identical):
#   :: WXAUDIT-CORES: n=8 cr0=[0x80010013,0x80010011,0x80010011,0x80010011,0x80010011,0x80010011,0x80010011,0x80010011] wp=0xFF nxe=0xFF ::
REQUIRE :: WXAUDIT-CORES: n=[0-9]+ cr0=\[0x[0-9A-F]+(,0x[0-9A-F]+)*\] wp=0x[0-9A-F]+ nxe=0x[0-9A-F]+ ::
# THE UNFILLED SLOT, forbidden. `CORE_CR0` is `.bss`-resident and zero-init (smp.rs:93), and the
# emitter's own contract is that the first `n` slots are ones the BSP has already observed — the same
# publication ordering that makes `wp_mask`'s bits valid. A slot reading `0x0` is therefore not a
# strange CR0, it is A CORE THAT NEVER STORED ONE: no live x86 core in long mode can hold CR0=0 (PE
# and PG alone are 0x80000001), so the value is unambiguous. It would mean the ordering broke and the
# masks beside it are a claim about a core nobody read — which is precisely the hazard this witness
# was added to close, silently reintroduced. The REQUIRE above cannot see it: an array of zeros is
# still `0x0,0x0,…`, non-empty and well-formed.
# Scoped INSIDE the brackets (`[^]]*`) on purpose, so `wp=0x0` / `nxe=0x0` — legal readings on a host
# whose firmware leaves CR0.WP clear, which the WXAUDIT-NXE block above documents for this very
# bench — cannot satisfy it. REACHABLE: matched against Boot AF's own line with one slot rewritten to
# `0x0`, which it catches; zero hits on AC, AD, AE and AF as they stand.
FORBID :: WXAUDIT-CORES: .*cr0=\[[^]]*0x0[,\]]

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

# --- EDID: THE FIRMWARE'S VIEW OF THE PANEL, CARRIED ACROSS THE HANDOFF -----------------------
# `1d0d93c6` (GR20) carries the panel's raw EDID base block from the bootloader into the kernel and
# witnesses it in one line from `kernel_main` — before `arch::memory::init` consumes `BootInfo`, on
# both arches and in EVERY build (video/mod.rs:205). It is the only independent statement of what
# the firmware knew about the panel, and the iGPU display lane's mode-set will read its bytes.
#
# THE LINE, NOT THE VALUE, and on this bench that distinction is the whole directive. Metal reads
# `present=0`: the rMBP's gmux routes the panel to the Kepler and the firmware's GOP handle publishes
# no EDID protocol, so nothing is carried and `present=0 hdr=- sum=- native=- len=0` is the CORRECT
# reading, documented as such at the emitter. A REQUIRE pinning `present=1` would be red on every
# capture this bench has ever produced; a REQUIRE pinning `present=0` would go red on the commit that
# finally lands the AUX read and starts carrying a block — the gate-red-when-it-works disease the
# EPACE-TRIM M8 block refuses at length. So the pattern spans both arms and asserts what is
# invariant: THE WITNESS SPOKE, with its field list intact and its tokens from the emitter's own
# vocabulary (`-`/`OK`/`BAD`, `-`/`WxH`).
# What it catches is the failure this witness has: the handoff silently dropping the block — a
# `BootInfo` field lost across a bootloader change, `init_edid` no longer called from `kernel_main` —
# which produces NO line at all, since `present=0` is itself an emission and absence is not.
# The `.*` between `native=` and `len=` spans the `pclk_khz=`/`ext=` pair that only the `present=1`
# arm prints; both arms end `len=N ::`.
# Capture line (Boot AF; AC/AD/AE byte-identical):
#   :: video: edid present=0 hdr=- sum=- native=- len=0 ::
# The other arm, generated from the emitter's format string at video/mod.rs:235:
#   :: video: edid present=1 hdr=OK sum=OK native=2880x1800 pclk_khz=337750 ext=1 len=256 ::
REQUIRE :: video: edid present=[01] hdr=(-|OK|BAD) sum=(-|OK|BAD) native=(-|[0-9]+x[0-9]+) .*len=[0-9]+ ::
# NO FORBID ON `hdr=BAD` / `sum=BAD`, stated rather than omitted. Both are honest readings of a real
# panel's corrupt block, and the emitter is fail-closed around them (`edid_block()` returns None, so
# nothing can be programmed from a block that failed its own checks). They are a finding about the
# HARDWARE, not a fault in the kernel, and this file forbids verdicts the kernel owns.

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

# --- IGPU-BLT: THE CENSUS THAT NOW FLIES (PROMOTED PENDING → REQUIRE) --------------------------
# PROMOTED PENDING → REQUIRE, 2026-08-07 (GR19), on the capture the block below named in advance.
# It shipped PENDING because no capture then in existence carried the line, and the promotion
# condition it stated — "the first x86 metal capture from an image at or after `f11e1fc0`" — was met
# by BOOT Y (metal, s73 capture, image built at `776fb13c`): mbench replay reported
# "pending 1/1 matched", the directive flagged "MATCHED: consider promoting to REQUIRE". From that
# boot forward, a build that stops printing the census goes RED instead of going unnoticed, which is
# the whole point of the idiom — Boot X is the capture that proved silence was the failure mode.
# Capture line (Boot Y @ 1793 ms), the `absent` arm as predicted for this dual-GPU bench:
#   :: igpu-blt: ring=absent why=no-active-surface — every iGPU display plane is off (gmux routes the panel elsewhere); CPU path carries the console ::
#
# The history below is kept verbatim, because it is the record of why a REQUIRE would have been the
# wrong directive for two rounds and is the right one now:
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
# It catches both. Zero hits on Boot V, W and X, as expected; one hit on Boot Y, which is what
# promoted it.
REQUIRE :: igpu-blt: ring=

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

# --- SDHC: THE CARD THAT COULD NOT BE IDENTIFIED, AND THE WRITE THAT PROVES ITSELF ------------
# Three directives, one subject: the SD stack's identification and its write self-test. All three
# carry the CARD-PRESENCE scope axis stated in the header — they print once per boot ON WHICH A CARD
# WAS IDENTIFIED — and the `f94e280c` / `b2f4a090` floors.
#
# WHY THIS IS A REGRESSION FLOOR RATHER THAN A NEW WITNESS. `f94e280c` is a fix whose failure mode is
# SILENCE: before it, CMD8 timing out was treated as an error and the whole identification aborted,
# so a healthy pre-v2.00 card in a healthy reader produced a bring-up that stopped mid-ladder and
# said nothing further. Boot AB is that capture, quoted in the header. A revert — or any future
# refactor that lets an expected timeout end the ladder again — restores exactly that silence, and
# silence is what these REQUIREs convert into a red.
#
# --- 1. THE IDENTIFICATION WITNESS. Every field on it was measured on this boot: the spec version
# from whether CMD8 was answered, the class and addressing from ACMD41's CCS (cross-checked against
# the CSD structure), the block count from the CSD arithmetic, the RCA from CMD3.
# THE CARD IS NOT PINNED — this is the field the pattern most invites pinning and most must not.
# `v1.x SDSC byte-addressed` describes THE MEDIA IN THE SLOT, not the kernel: the emitter is
# deliberately one parameterised statement rather than two branch-printed ones, so the same line
# reads `v2.00+ SDHC block-addressed` for a modern card. Pinning `v1.x` would red the sitting that
# swapped the scratch card, which is a statement about Peter's desk and not about this OS. What IS
# pinned is the emitter's VOCABULARY, alternation by alternation: a rewrite that dropped the class
# word, the addressing word or the `size=… MiB` term reds here. `blocks=` and `rca=` are shaped and
# never valued (`rca` is assigned by the card at CMD3 and legitimately differs per insertion).
# Emitter: drivers/sdhc.rs:1459.
# Capture line (Boot AF @ 2452 ms; AC/AD/AE the same but for the millisecond stamp):
#   :: sdhc: card v1.x SDSC byte-addressed blocks=60800 size=29 MiB rca=0x5bbc ::
REQUIRE :: sdhc: card (v1\.x|v2\.00\+) (SDSC|SDHC|SDXC) (byte|block)-addressed blocks=[0-9]+ size=[0-9]+ MiB rca=0x[0-9a-f]+ ::
# --- 2. THE LADDER REACHED THE END. The line above is printed inside `identify`; this one is printed
# by its CALLER, after `identify` returned a card and before the card is published (sdhc.rs:3359). The
# two are not redundant: an identification that produced a witness and then failed on CMD7 SELECT or
# on the ADMA2 hand-off would print the first and not the second, and the card would never reach
# `CARD.lock()` — a stack that describes a card it cannot serve. Requiring both is what separates
# "identified" from "identified and published".
# `bdf` is shaped, not pinned: the reader is 3:0.1 on this bench and that is a slot, not a fact
# about the driver.
# Capture line (Boot AF @ 2452 ms):
#   [sdhc] bdf 3:0.1 CARD IDENTIFIED — 60800 blocks, byte-addressed, csd v1
REQUIRE \[sdhc\] bdf [0-9]+:[0-9]+\.[0-9]+ CARD IDENTIFIED — [0-9]+ blocks, (byte|block)-addressed, csd v[0-9]+
# --- 3. THE WRITE SELF-TEST'S VERDICT LINE, AND WHY THE VERDICT IS NOT IN THE PATTERN.
# SDHC-4a's `write_block_512` is `#[cfg(feature = "sdw")]` — OFF by default — but the four-gate
# ladder, the stash read and THIS LINE compile and run on every boot, armed or not (sdhc.rs:2943-2948
# says so in as many words, and `let armed = cfg!(feature = "sdw")` at :3163 is the whole difference).
# That is what makes a REQUIRE legitimate here where it is not legitimate for `pcicensus` or
# `bcmarecon`: the wire is unconditional, only its VERDICT moves with the knob. So the verdict is
# what the pattern refuses to name.
# BOTH CONFIGURATIONS ARE IN THE BENCH ARCHIVE, which makes this a measurement rather than a claim:
#   Boot AC (default build):   `armed=0 … verify=DRYRUN restore=SKIPPED reason=would-write -> DRYRUN`
#   Boots AD/AE/AF (UNAOS_SDW=1): `armed=1 … verify=IDENTICAL restore=IDENTICAL reason=none -> PASS`
# One pattern matches all four. A REQUIRE naming `-> PASS` would red every default boot; one naming
# `-> DRYRUN` would red every armed one. The refusal arms (`-> REFUSED`, `lba=NONE`, `class=?`,
# `blank=?`) are matched too, and deliberately: a write-protect slider, a GPT card or a non-blank
# scratch sector is the ladder REFUSING CORRECTLY, which is media state and not a kernel fault.
# `-> FAIL` is also matched by this pattern and is NOT thereby condoned — the default FORBID set
# (`FAIL ::`) reds it, which is the division of labour this file uses everywhere: the REQUIRE asserts
# the instrument ran, the FORBIDs own the verdicts.
# Emitter: `sdw_verdict`, drivers/sdhc.rs:3106. Every field is a `&str` there so "not measured" has a
# token of its own, which is why `\S+` is the right shape for the five word fields and `[0-9]+` would
# be wrong.
# Capture lines:
#   :: sdhc: w1 armed=0 lba=60799 wp_sw=1 csd_perm=0 csd_tmp=0 class=MBR blank=1 verify=DRYRUN restore=SKIPPED reason=would-write -> DRYRUN ::
#   :: sdhc: w1 armed=1 lba=60799 wp_sw=1 csd_perm=0 csd_tmp=0 class=MBR blank=1 verify=IDENTICAL restore=IDENTICAL reason=none -> PASS ::
REQUIRE :: sdhc: w1 armed=[01] lba=(NONE|[0-9]+) wp_sw=[0-9]+ csd_perm=[0-9]+ csd_tmp=[0-9]+ class=\S+ blank=\S+ verify=\S+ restore=\S+ reason=\S+ -> [A-Z]+ ::

# --- BPACE / GPACE: THE BOOT-PACE LEDGER, WHICH HAD NO GATE AT ALL ----------------------------
# THE FINDING THAT PUT THIS BLOCK HERE, stated first because it is the argument. A GR19 falsification
# round deleted all 134 `BPACE:` and `GPACE:` lines from a known-good boot slice — every `gui=`,
# `kepler=`, `sched d=`, `ehci-hid-done d=`, `xtail=`, `igpu=`, `sdhc=` reading the bench has — and
# this spec still printed a full PASS. A second probe corrupted `BPACE: total gui=` and nothing
# anywhere objected. The numbers the metal boots EXIST TO MEASURE were the numbers with no automated
# reader: `gui 2376 -> 2217`, `ehci-hid-done 1444/1446 -> 1285` and `kepler 397 -> 396` are what one
# arc bought, and a gate that passes identically whether they improved or regressed is not reading
# them. This block is not a performance gate — see the SHAPE note below — it is the assertion that
# the ledger is still on the wire and still parses.
#
# SHAPE, NEVER VALUE, and here the reason is not stylistic but structural. A directive pinning
# `gui=2217ms` would go RED ON THE COMMIT THAT BUYS THE NEXT 150 MS — the mirror image of the
# vacuous-instrument disease, and the same argument the EPACE-TRIM M8 block makes for staying
# OPTIONAL. Millisecond figures move between arcs BY DESIGN. What does not move is the ledger's
# grammar: the phase tags, the `t=`/`d=` pair, the `dropped=` counter and the `result=LEDGER`
# terminal. Those are what is pinned. The values' home is the analyzer's tables, bootpace.md, and
# the per-arc prediction table in the playbook — all read by humans, which is correct for a
# measurement and useless for a tripwire.
#
# NO GENERATION FLOOR. `bootpace::service_dump` is deliberately ungated — no Cargo feature, no env
# knob, stated in its own doc comment because "a ledger that only exists in the builds nobody boots
# on hardware is not an instrument" — so these directives are the only ones in this file that hold
# on EVERY x86 capture in the archive, including Boot V. Verified: they match Boot W, Boot X and
# Boot Y.
#
# The rollup, and `dropped=0` pinned BY VALUE, which is the one exception to the paragraph above and
# earns it: `DROPPED` counts ledger stamps that never made it into the ring (`CAP` exceeded), and a
# nonzero reading means the block below it is INCOMPLETE — phases silently missing from a report
# whose whole value is that it accounts for the boot. That is a shape fact, not a speed fact.
# `ftdi=(none|[0-9]+ms)` spans both honest readings: `none` before the FTDI console arms (the ledger
# re-emits on growth, so the first emission legitimately has no `ftdi-up` stamp yet) and a figure
# once it has. `Dur` prints `none` for an unrecorded phase, so `gui=[0-9]+ms` is exactly the
# assertion that the GUI phase WAS recorded — an unrecorded one reads `gui=none` and reds here.
# Capture line (Boot Y @ 2223 ms; Boot W @ 2380 ms and Boot X @ 2383 ms are the same shape):
#   :: BPACE: total gui=2217ms ftdi=none n=27 dropped=0 hz=2693860140 result=LEDGER ::
REQUIRE :: BPACE: total gui=[0-9]+ms ftdi=(none|[0-9]+ms) n=[0-9]+ dropped=0 hz=[0-9]+ result=LEDGER ::
# The same counter, forbidden explicitly rather than left to the REQUIRE's `dropped=0`. NOT
# redundant: the REQUIRE is satisfied by ANY emission carrying `dropped=0`, and the ledger re-emits
# several times per sit — so a boot whose first rollup is clean and whose later ones drop stamps
# passes the REQUIRE and is caught only here. Emitter: `DROPPED` in bootpace.rs, incremented when a
# stamp arrives with the ring full.
FORBID :: BPACE: .*dropped=[1-9]
# THE TWO PHASE STAMPS THIS TRACK ACTUALLY SPENDS ITS ARCS ON. `d=` is the phase's own cost and `t=`
# its offset from kernel entry; both are shaped, neither is valued. `ehci-hid-done` is BUY-1's
# subject (1444 ms on Boot W and 1446 on Boot X — a PAIR, not the single 1444 the older playbooks
# quote — and 1285 on Boot Y once BUY-1 was paid); `sched` is SCHED-X86's, and has sat at d=67 ms
# across W, X and Y. Requiring the LINES means the next arc that moves either number is still
# measured; requiring the numbers would red the arc that moved them.
# Capture lines (Boot Y):
#   :: BPACE: ehci-hid-done t=1579ms d=1285ms ::
#   :: BPACE: sched t=243ms d=67ms ::
REQUIRE :: BPACE: ehci-hid-done t=[0-9]+ms d=[0-9]+ms ::
REQUIRE :: BPACE: sched t=[0-9]+ms d=[0-9]+ms ::
# THE GPACE CENSUS — the per-device split of the PCI/USB enumeration block, and the line that holds
# `kepler=`, the single largest number this track has spent three rounds driving down (17 077 ->
# 2 564 -> 397 -> 396 ms). `xtail=` anchors the head of the field list and `kepler=` is named
# explicitly rather than spanned, because a census that kept printing while losing that one field is
# precisely the silent-regression shape the whole file is written against. `(n=N)` is required
# beside each: it is the sample count, and a reading with no `n=` is a mean over an unknown
# denominator. `== witness ::` is the uncounted-witness terminal, pinned so a verdict-bearing rewrite
# of this line cannot pass unnoticed.
# Capture line (Boot Y @ 2203 ms):
#   :: GPACE: xtail=0ms(n=1) bench=0ms(n=0) detect=5ms(n=1) igpu=1ms(n=1) kepler=396ms(n=1) sdhc=12ms(n=1) nic=0ms(n=1) resid=3ms == witness ::
REQUIRE :: GPACE: xtail=[0-9]+ms\(n=[0-9]+\) .*kepler=[0-9]+ms\(n=[0-9]+\) .*== witness ::

# --- MIDDEN-M1: the shell console's interpreter is the shared no_std core ----------------------
# `shell::midden_witness` runs on BOTH arches — `main.rs` calls it under
# `#[cfg(all(target_arch = "x86_64", feature = "witness"))]` as well as on the pi4 path — so these
# four PASS lines are on every witness-armed rMBP boot, which is every boot this spec scopes to.
# They were pinned on the pi4 gate first and NOT here, and that asymmetry was the defect: the same
# fixture, printing the same four verdicts, gated on one arch and merely printed on the other.
#
# What each proves (the fixture drives `midden_core::plan` over a synthetic volume, so it needs no
# keyboard, no card and no FAT — it is the interpreter under test, not the storage):
#   dispatch   — a core verb is answered IN the core, with real text (not routed, not swallowed)
#   route      — a host verb comes back as Host with its args intact
#   resolve    — the `.elf` the user did not type is elided to a name on the volume
#   precedence — a verb still beats a program of the same stem (`stat` vs STAT.ELF), which is the
#                security half of the rule: a dropped file must not shadow a verb
#
# NOT PINNED, and stated so the omission is not read as an oversight: the fixture's companion echo
# `:: [midden] resolve "vug" -> VUG.ELF ::`. It asserts nothing the `midden.resolve` verdict above
# does not already assert on the same comparison, and its spelling is the FIXTURE's — the LIVE x86
# line reads `-> vug.elf`, because `FatVolume::is_file` walks FAT case-insensitively and the core's
# as-typed probe hits before the upper-cased one. A rule written against `-> VUG.ELF` would look
# like a claim about the live shell and be false. The live per-dispatched-line witness is not
# pinned at all here for the reason it cannot be: it needs a keystroke, and this capture is a boot.
#
# MINIMUM BUILD GENERATION for this block: the midden-core arc (`shell: one interpreter, and it is
# midden's`) and its review follow-up. Captures older than it carry no `:: TSTE: midden.` line and
# will red these four — the same honest scope axis the WXN block above draws, for the same reason.
REQUIRE :: TSTE: midden.dispatch -> PASS ::
REQUIRE :: TSTE: midden.route -> PASS ::
REQUIRE :: TSTE: midden.resolve -> PASS ::
REQUIRE :: TSTE: midden.precedence -> PASS ::
FORBID :: TSTE: midden\.\w+ -> FAIL

# --- THE KNOB-GATED WITNESSES, AND THE DIRECTIVE THIS GRAMMAR DOES NOT HAVE -------------------
# GR20 landed two more load-bearing wires that this file DELIBERATELY DOES NOT REQUIRE, and the
# reason is a hole in the grammar rather than a judgement about the wires:
#   `[PCI-CENSUS] … done: … net-class=…`   `#[cfg(feature = "pcicensus")]`  (`UNAOS_PCICENSUS=1`)
#   `:: bcma: begin …` / `:: bcma: end …`  `#[cfg(feature = "bcmarecon")]`  (`UNAOS_BCMARECON=1`)
# Both features are DEFAULT OFF. A REQUIRE for either would go red on every boot of the build
# configuration this spec's own header scopes it to — a directive that fails on CONFIGURATION rather
# than on health, which is the same defect as a REQUIRE that cannot match a real boot, wearing the
# other mask. Measured, not assumed: Boots AC and AD carry no `[PCI-CENSUS]` line at all, and only
# Boot AF carries `:: bcma:`. Requiring them would have red three of the four GR20 captures.
#
# THE MECHANISM DOES NOT EXIST. mbench has five kinds (mbench.py:258) and none of them is
# conditional presence:
#   REQUIRE   unconditional; a miss is a FAIL.
#   COUNT     the same, with a threshold.
#   FORBID    already conditional BY CONSTRUCTION — a negative rule over a wire that was never
#             emitted simply scores 0 hits, so FORBIDs on knob-gated lines are free and sound. This
#             is worth stating because it means only the POSITIVE half of the problem is open.
#   OPTIONAL  reported, never fails. Honest, and NOT coverage — see below.
#   PENDING   "a witness whose code is ahead of its bench"; never fails, and prints "consider
#             promoting to REQUIRE" when it matches. That advice is WRONG here in the same way it is
#             wrong for a fault path (the wc-d teardown block makes the identical argument): promoting
#             a knob-gated line reds every default boot. PENDING is therefore the wrong label even
#             though its pass/fail behaviour is what is wanted.
#
# WHAT A CONDITIONAL REQUIRE WOULD HAVE TO BE, written down here rather than invented in this arc,
# because adding a sixth directive kind is a change to the gate every spec in the tree runs through
# and belongs to an arc that owns mbench.py:
#
#   WHEN <guard-regex> REQUIRE <regex>
#
#     Semantics: if any line in the capture matches <guard-regex>, the REQUIRE is live and a miss is
#     a FAIL exactly as today; if no line matches the guard, the directive is reported as
#     `not-applicable` and is counted in NEITHER the numerator nor the denominator of the N/N tally.
#     Everything else follows from that: `satisfied()` returns True when the guard is absent,
#     `failed()` returns False, and `verdict_table` needs one more row state.
#
#     The guard MUST be a line the same knob emits and that no other configuration can produce —
#     `[PCI-CENSUS] full enumeration:` for the census, `:: bcma: begin` for the recon. A guard chosen
#     from an UNGATED line would make the conditional unconditional again; a guard identical to the
#     required line would make the rule vacuous (it can only fire when it is already satisfied),
#     which is the trap that must be documented at the directive rather than rediscovered per spec.
#     The census case shows the shape working: guard on the census's opening line, require its
#     `done:` summary, and a run in which the census STARTS and never finishes is a red — which is a
#     real failure mode (a truncated enumeration wedged mid-bus) that nothing catches today.
#
#     The arithmetic consequence must be stated in the record when it lands: every prior x86 record
#     quotes an N/N total, and a directive that leaves the denominator alone when dormant is the only
#     variant that keeps those numbers comparable across captures with different knob sets.
#
# UNTIL THEN, both are OPTIONAL — visible in the table, never a gate. THE COST IS REAL AND IS STATED:
# an OPTIONAL proves NOTHING. If either emitter is deleted, these two rules go from ✅ to ◦ on an
# armed capture and mbench still exits 0. They are here so a reader replaying an armed boot can see
# at a glance that the wire spoke, and they are NOT to be mistaken for the coverage the WHEN clause
# above would buy. Same idiom, and the same honest accounting, as the EPACE-TRIM M8 OPTIONAL.
# Capture line (Boot AE @ 1692 ms, Boot AF @ 1691 ms):
#   [PCI-CENSUS] done: devices=20 functions=27 printed=27 truncated=0 net-class=0x02:2 (caps dumped 2) elapsed=6ms
OPTIONAL \[PCI-CENSUS\] done: devices=[0-9]+ functions=[0-9]+ printed=[0-9]+ truncated=[0-9]+ net-class=0x[0-9a-f]+:[0-9]+
# Capture lines (Boot AF @ 1691 / 1697 ms). The pair is deliberately two rules: `begin` without `end`
# is the recon wedging inside a BAR0 read, which one spanning rule could not tell from a clean run.
#   :: bcma: begin — READ-ONLY recon of PCI class 0x02/sub 0x80 (config reads + BAR0 reads; …) ::
#   :: bcma: end ok=0 stage=chipcommon elapsed=5ms ::
OPTIONAL :: bcma: begin — READ-ONLY recon of PCI class 0x02/sub 0x80
OPTIONAL :: bcma: end ok=[01] stage=\S+ elapsed=[0-9]+ms ::

# --- WC-K2 — a desktop fill must never reach the front buffer outside a composite publish -------
#     The one `[wc-k]` subject this spec takes, because the x86 half of the mechanism has no other
#     gate. `wm::erase` no longer writes a pixel: it queues its vacated boxes and
#     `wm::drain_deferred` publishes them at the head of the composite pass the erase site already
#     ran in the same call. `wm::stage_fill` therefore has exactly one caller and is told so
#     (`from_drain`); a present taken from anywhere else prints the line these FORBID and carries
#     `outside=` into the `scope=fills` rollup.
#
#     WHY HERE AND NOT ONLY ON PI. Boot AS was an attended x86 drag ("still some flickering from
#     title bar"), and review condition 1's stranding case -- a wakeup consumed by
#     `wm::composite`'s re-run loop or its lost-wakeup block while a fill is still queued -- lives
#     in code that is `#[cfg(target_arch = "x86_64")]`. The pi4 spec cannot reach it.
#
#     NOT REQUIREs. `[wc-k]` fills need window teardown to happen at all, and this spec's boots are
#     scoped to the kepler/paygo knob set rather than to a window fixture battery, so a REQUIRE here
#     would be red on configuration instead of on health -- the trap the `build=` note above records.
#     The pi4 gate owns the positive claims (`-> BUFFERED`, `reason=route`, `outside=0`); these two
#     lines own the x86 negative one, which needs no completeness claim to be worth having.
#
#     `-> RESCUED` is deliberately NOT forbidden and not required: a rescue is review condition 1's
#     fix firing, i.e. a fill the pre-fix code would have stranded. It is a number to read, and it
#     needs a DECLINED pass to arise at all, so requiring it would be a claim about core scheduling.
#     See docs/dev/OS/08_VIDEO/engine.md §WC-K2.
FORBID \[wc-k\] .*-> UNPUBLISHED
FORBID \[wc-k\] .*outside=[1-9]

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
#   * `[wc-h]` / `[cursor*]`, and every `[wc-k]` line except the two WC-K2 FORBIDs below. Real
#     witnesses, thoroughly pinned by the pi4 gate, and out of this spec's subject. Adding
#     half-considered copies here would grow the file without growing the evidence.
#     WC-K2 IS THE NAMED EXCEPTION, and the exception has a reason rather than an appetite: the
#     defect it removes was reported on THIS machine (Boot AS, attended, x86 drag), the mechanism it
#     forbids -- a desktop fill reaching the front buffer outside a composite publish -- is
#     arch-independent code in `video/wm.rs`, and the x86 wakeup gates it depends on
#     (`COMP_GATE`/`COMP_PENDING`, review condition 1) DO NOT EXIST on aarch64 at all. So the pi4
#     gate cannot red for the x86-only half, and a claim that the fix reds the gate was, until these
#     two lines, a claim about a platform where the bug could not occur. See the block below.
#   * The SECOND `GPACE:` line — `span=417ms anchor=enum:p1 since-entry=2203ms hz=… build=kepler+
#     takeover+fifo+ivb+wc+smc+ == the pci-usb d= split ::`. Tempting, because `build=` names the
#     knob set this whole spec is scoped to. Not pinned, and the reason is that pinning it either
#     spans the field (asserting nothing) or fixes the knob string (making the spec red on the next
#     knob added, i.e. a directive that fails on configuration rather than on health). The knob set
#     is asserted where it is actually load-bearing — by the paygo and logts REQUIREs at the top,
#     which are red on a build without the knobs — and READ, per boot, by the analyzer. Recorded so
#     the next reader does not add it thinking it was overlooked.
#   * `WXN-M2`'s field VALUES — `nx_2m=1022`, `nx_4k=719`, `keep_x=305`. The shape is REQUIREd
#     above; the arithmetic that ties them to `kern_WX` (`keep_x == kern_WX`, and
#     `residue + 511*(split_2m + demote_1g) - nx_2m - nx_4k == kern_WX`) is a CROSS-LINE identity,
#     which this grammar cannot state at all. It lives in `tools/serial-analyzer.py --wxn`, which
#     reconciles the two walkers per boot and exits FINDING when they disagree.

# --- WCK4: THE ERASE-DISCIPLINE VERDICTS, FORBIDDEN ------------------------------------------
# --- The [drag-occ] witness measures pixels PUBLISHED into the dragged window's live box by
# --- painters that are not the dragged window (occ_px = a window over its occluder, fill_px =
# --- the coalesced desktop erase, direct = the unclipped fallback, fillover_px = the span walk
# --- emitting a covered column). Its BLEED verdict is computed from real panel writes, so it can
# --- fail; a boot that drags occluded windows and stays quiet here is the arc's claim holding.
# --- The OVERFLOW line fires only if the erase clip drops an occluder for capacity — sized
# --- unreachable today (12 windows + FURNITURE_MAX furniture strips = OCC_MAX exactly), so its
# --- appearance means the sizing law was broken by a later arc, not that the room got busy.
# ---
# --- STRIPFACTOR (2026-08-11) rewrote that sizing from an inline `+ 1` for the dock into
# --- `MAX_WINDOWS + FURNITURE_MAX`, with `FURNITURE_MAX` const-asserted equal to the strip
# --- registry's own `strip::STRIP_MAX`. A tenant added to the registry without widening the clip is
# --- now a BUILD failure rather than a silently dropped occluder, so this FORBID has moved from
# --- being the ONLY defence to being the second one — it still covers the case the assertion cannot
# --- see, a tenant whose rect exceeds what its own geometry accessor promised.
FORBID \[drag-occ\] .*-> BLEED
FORBID \[wck4\] erase clip OVERFLOW

# --- WCK5 × STRIPFACTOR: EVERY STRIP'S WINDOW-BLIT PROTECTION, PINNED ------------------------
# --- WCK4 closed the ERASE path and left the WINDOW-blit path open (its own KNOWN GAP D3): a
# --- window whose outer box overlapped the dock published its chrome over the strip on every
# --- composite and `dock::compose` repainted the strip at the tail of the same pass —
# --- disappear/reappear at motion rate, which is what Peter reported on Boot B. `occ_clip` now
# --- carries the strip, and the pair on the wire is `occclip_dock=` (window blits whose clip HELD
# --- the strip) and `occclip_dock_px=` (pixels those blits withheld because of it), beside
# --- `occdock=` (the strip as that clip saw it, or `absent`).
# ---
# --- STRIPFACTOR GENERALISED that push: `occ_clip` now carries EVERY furniture strip, so the menu
# --- bar at the TOP edge — the identical D3 hole one strip along — is protected on the same path,
# --- and the same pair is published for it: `occclip_bar=` / `occclip_bar_px=`. Both FORBIDs below
# --- are the D1 lesson stated as a regex: a blit that had a strip in its clip and withheld NOTHING
# --- is the defect, not the fix — either the span walk published the strip's columns or the box
# --- went in degenerate. `occclip_dock=0` / `occclip_bar=0` are NOT forbidden and must not be: a
# --- gesture that stayed away from a strip's edge legitimately never met it, which is exactly why
# --- the count is on the wire beside the pixel total rather than instead of it. The bar ships DEFAULT
# --- OFF, so `occclip_bar=0` is the standing reading on a gate boot; SHELLDESK (2026-08-11) makes the
# --- desktop shell enable it at desktop-ready, so on a boot that reaches the Kepler takeover the
# --- guarded case — a window dragged across a LIVE top strip — is the ordinary one rather than the
# --- hypothetical one, and the FORBID is guarding traffic instead of waiting for it.
FORBID \[drag-occ\] .* occclip_dock=[1-9][0-9]* occclip_dock_px=0
FORBID \[drag-occ\] .* occclip_bar=[1-9][0-9]* occclip_bar_px=0

# --- STRIPFACTOR × WCK5: THE occclip_bar FORBID IS PROVEN ABLE TO FIRE --------------------------
# --- The `occclip_bar=` FORBID above guards the menu bar, but the bar is DEFAULT OFF, so no boot
# --- ever drags a window across the top strip and `occclip_bar` never leaves 0 on `[drag-occ]` —
# --- the guarded field has never been seen nonzero, and a FORBID that cannot see its field move is
# --- vacuous. `menubar::selftest`'s MENUBAR-OCC leg closes that: it ENABLES the bar, drives a
# --- synthetic window box across the top strip, and runs `occ_clip`'s OWN primitives
# --- (`OccClip::push`/`prepare`, `OccRows::spans`, `span_occ`) — the present's exact arithmetic —
# --- to read the pair the compositor would fold. WCK5 fired the DOCK equivalent by pinning
# --- `for_panel`'s y to 100 under the gate's windows and reverting; this computes the identical
# --- probe rather than pinning-and-reverting.
# ---
# ---   * `occclip_bar=N>0 occclip_bar_px=N>0` — PROTECTED: the bar in the clip, the span walk
# ---     withheld the strip's columns from the crossing window. The fired witness the boot-time
# ---     FORBID needs to have seen move.
# ---   * `forbid_bar=N>0 forbid_bar_px=0` with `forbid_trips_when_removed=true` — FAULT: the strip
# ---     still counted in the population but the clip walked EMPTY (the span walk published its
# ---     columns), collapsing the pixel total to 0 — the exact `occclip_bar=N>0 occclip_bar_px=0`
# ---     state the FORBID trips on, so it is proven non-vacuous rather than trusted.
# ---   * `restored=true` — the leg's enable→probe→RESTORE cycle put the bar back to the state the
# ---     battery ARRIVED in, so nothing later in the boot sees a state the probe invented. SHELLDESK
# ---     changed this from an unconditional `set_enabled(false)`, which on an operator boot would have
# ---     switched the shell's menu bar off for the rest of the boot — a witness with a side effect on
# ---     the thing it witnesses. On a gate boot nothing has enabled the bar and the restored state is
# ---     still OFF. x86 + witness only (the primitives are `target_arch`-gated).
REQUIRE :: MENUBAR-OCC: bar_enabled=true crossed=true occclip_bar=[1-9][0-9]* occclip_bar_px=[1-9][0-9]* forbid_bar=[1-9][0-9]* forbid_bar_px=0 forbid_trips_when_removed=true restored=true :: PASS ::
FORBID :: MENUBAR-OCC: .* :: FAIL ::

# --- STRIPFACTOR: THE REGISTRY'S SHAPE IS ON THE WIRE -------------------------------------------
# --- `bars=present/total` and `bar=` were added to `[drag-occ]` beside the existing `dock=` for the
# --- reason `dock=` itself exists: `fillover_px` can only see boxes that ARE in the clip, so a strip
# --- missing from the registry's walk would be erased on every drag while every other term on the
# --- line read healthy. With one tenant `dock=` answered that; with two it does not.
# ---
# --- `bar=0` WAS REQUIRED on the premise that nothing enables the bar. SHELLDESK (2026-08-11) ends
# --- that premise: the crispy desktop shell asks for its menu bar at desktop-ready (`wcx::activate`,
# --- witnessed by `[wc-x] menubar ENABLED`), so on any boot that reaches the Kepler takeover the bar
# --- legitimately owns the top `BAR_H` rows and `bar=` is legitimately the panel width. Peter, metal
# --- Boot A: *"i cannot see the menu because a shell is still posing as the desktop"* — a spec that
# --- forbade the bar from owning pixels was pinning the defect.
# ---
# --- What replaces it is the same question asked where it is still falsifiable. `bar=` is spanned
# --- (both readings are correct now, and which one a boot shows is decided by whether the desktop
# --- shell came up), and the registry's own health — a strip that owns pixels while being ABSENT from
# --- the walk, which is the erasure defect these terms exist for — is still pinned by `bars=`, by
# --- `fillclip_dock_px=`, and by the two degenerate-pair FORBIDs above. `[wc-x] menubar ENABLED` is
# --- what a capture reads to tell "the shell never asked" from "the shell asked and the bar declined".
# --- The `bars=` term is deliberately spanned rather than pinned to a value — the dock is legitimately
# --- absent while the window table is empty (`bars=0/2`) and legitimately present once it is not.
REQUIRE \[drag-occ\] .* bars=[0-9]/[0-9] bar=[0-9]+ fillclip_dock_px=
# --- SHELLDESK REVIEW — and the DEGENERATE PAIR the spanned `bar=` would otherwise stop catching.
# --- Spanning `bar=` was right (both readings are now legitimate), but it left the prose's claim that
# --- "the registry's own health is still pinned by `bars=`" unbacked: with the old FORBID gone, NO
# --- rule on this line related the two terms, and `bars=` alone cannot say a strip owns pixels while
# --- the walk missed it. This states the relation instead of the value. `erase_clip` writes both from
# --- ONE walk of `strip::rects` in one statement block — `bars=` is the count of PRESENT slots and
# --- `bar=` is `furn[MENUBAR_SLOT].map(|r| r.2)` — so `bar>0` implies that slot was `Some`, which
# --- implies `bars>=1`. `bars=0/N` beside a bar owning pixels is therefore impossible in a healthy
# --- build and is precisely the erasure defect these terms exist for: a strip on the glass that the
# --- registry's walk does not know about is erased on every drag while every other term reads clean.
# --- It holds under BOTH readings — a gate boot (`bars=0/2 bar=0`) and an operator boot (`bars=2/2
# --- bar=<panel width>`) both pass — so it constrains the mechanism, not the scenario.
FORBID \[drag-occ\] .* bars=0/[0-9] bar=[1-9]

# --- STRIPFACTOR: THE MENU BAR IS ABSENT BY DEFAULT, AND SAYS SO -------------------------------
# --- `video/menubar.rs` is tenant #2 of the strip primitive and exists this arc to PROVE the
# --- primitive is generic — a one-tenant registry proves nothing. It is inert chrome (no press
# --- seam: `press=inert` is on the line so a dead press is not read as a routing defect) and it is
# --- off unless something enables it at runtime.
# ---
# --- Six fields are load-bearing and all six are pinned on the PASS line, which already ANDs them:
# ---   * `default_off=true`  — the ARTIFACT ships with the bar off. SHELLDESK moved this from a live
# ---     read of the flag to a latch taken at the first write (`menubar::DEFAULT_LATCH`), because the
# ---     desktop shell now enables the bar at desktop-ready and a live read would report the SHELL's
# ---     decision instead of the build's default. A bar that defaulted ON still fails here — the
# ---     latch records what the first writer FOUND — which is the whole of "absent by default".
# ---   * `clip_clean=true`   — with the bar off, its slot in the registry's output is EMPTY, so it
# ---     consumes no occlusion capacity. Not merely uncounted: absent.
# ---   * `flush=true`        — when enabled the rect is (0,0,pw,BAR_H), corner to corner. A centred
# ---     or inset bar fails here.
# ---   * `member=true`       — enabled, it IS in the registry's walk and the present count rose by
# ---     exactly one. A strip that painted but never entered the clip passes `flush` and fails this.
# ---   * `floor=true/true`   — the panel floor declines below it AND admits above it, driven with
# ---     synthetic geometry so neither direction can pass by accident.
# ---   * `dismissed=true`    — turned off again, the bar erased what it owned and the slot is clear.
# ---   * `crystal_ok=true`   — the brand CRYSTAL (Peter, "instead of an apple do a small crystal")
# ---     is drawn at `crystal=WxH+X+Y` and sits wholly inside the bar, left of the title. A brand
# ---     mark that could not be shown drawn would be unfalsifiable; this pins that it IS. The crystal
# ---     is INERT this arc (part of the bar's `press=inert`); a crystal MENU is a later arc.
# --- `clock=` is NOT pinned: it reads `unsynced` on a QEMU boot with no SNTP and `set` on one with
# --- a civil anchor, and both are correct. Pinning it would red the gate on network configuration.
REQUIRE :: MENUBAR: .* press=inert default_off=true clip_clean=true flush=true member=true floor=true/true dismissed=true crystal_ok=true :: PASS ::
FORBID :: MENUBAR: .* :: FAIL ::

# --- CONSOLEWIN: THE CONSOLE'S WAY BACK, PINNED ON THE ARCH THAT HAS A CONSOLE ------------------
# --- The x86 half of the console-as-window arc had no spec rule at all until this line, which is
# --- the arch it actually ships on: `wm::ctrldecline_selftest` and `wm::closeiso_selftest` are
# --- driven from the aarch64 battery only, so pi4-regression.spec carried the whole arc and the
# --- panel where the console window EXISTS was pinned by nothing.
# ---
# --- `dock::selftest`'s restored row is kernel FURNITURE (a reserved-band owner), and the leg is
# --- the reversibility proof for the console's new minimise disc. It has to be the dock and not
# --- `<TAB>`: `focus_ring_apps` filters the reserved band out of the focus rotation, so a parked
# --- console is not in the ring, and the dock is the whole of its way back.
# ---
# --- Four fields are load-bearing and all four are pinned:
# ---   * `park=parked` — `wm::minimise` on a kernel row went down AND its owner is hidden. The
# ---     token, not a bool: `parked-visible` is what an `above_shell` that ignores `PARKED_Z` for
# ---     furniture returns, i.e. the exact regression this arc can cause, and it is a DIFFERENT
# ---     string rather than a missing line. (Verified by construction: reverting the predicate
# ---     produced `park=parked-visible/false model=false` on this gate.)
# ---   * `model=true` — the dock ENUMERATES the parked kernel row. A dock that dropped furniture
# ---     from its model would leave the console parked with no tile to press.
# ---   * `restore=true` — a synthetic press at that tile's centre brought it back above the shell.
# ---   * `specific=true` — it raised THAT row, not merely something.
# --- Pinned as one rule on the PASS line rather than four, because the fixture already ANDs them
# --- into its verdict; the fields are named in the pattern so a future edit that drops one from
# --- the line reds this rule instead of silently narrowing what it asserts.
REQUIRE :: DOCK: strip tiles=.* model=true geom=true restore=true specific=true miss=true vacate=true furniture park=parked/true :: PASS ::
FORBID :: DOCK: .* :: FAIL ::
