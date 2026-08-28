# x86-wc.spec — the x86 window-compositor QEMU leg: DMGOVLP (overlap-forced banded damage, with
# the sprite parked on the stack) plus the two ladder witnesses it depends on for ordering.
#
#   QEMU gate:  UNAOS_WC=1 ./arroyo test 150          -> unaos/target/serial.log
#               ./arroyo mbench --replay target/serial.log \
#                        --spec scripts/specs/x86-wc.spec --platform x86
#
# WHAT THIS FILE IS FOR. DMG-DISJOINT (banded damage under overlap) shipped, was reverted after a
# composite storm (boot 7), and re-landed with the CURSTICK widening — and then boot 8 (metal)
# wedged inside `draw_window` under six overlapping windows with the pointer parked on the stack.
# Until this spec, NO QEMU leg drove banded presents through overlapping rows at all, let alone
# under a live cursor plan: the whole failure class was metal-only by construction. The
# `dmgovlp_selftest` fixture (video/wm.rs, ladder tail in arch/x86_64/syscall.rs) forces exactly
# that shape — a chain whose far window is reachable only by RELAY, a three-way staircase with the
# REAL sprite parked on its overlap — and this spec is its gate.
#
# SCOPE — a `UNAOS_WC=1 ./arroyo test` boot and nothing else. `wc` gates the fixture's cfg and the
# console-window routing, NOT the whole video stack; without the knob the fixture does not compile
# and every line below is red. The plain `./arroyo test` run is x86-witness/x86-fat territory and
# is deliberately NOT asserted here. QEMU has no Kepler, so `desktop_uefi::activate` never runs — the
# fixture drives `wm::` directly from the witness ladder and needs no kepler knob; the METAL
# compositor path stays the bench's business (x86-witness.spec).
#
# MINIMUM BUILD GENERATION — the DMGOVLP commit (the fixture and its `[dmgovlp]` grammar do not
# exist before it). The `[wm-act]`/`[clickroute]` lines below are years older and gate nothing new;
# they are here because the DMGOVLP fixture runs LAST in the same ladder, so their PASS lines are
# the proof the ladder actually reached it — a boot that died mid-ladder must not read as "DMGOVLP
# merely skipped".
#
# NO NUMERICS are pinned anywhere in this file: every count is \d+, and the verdict's thresholds
# (adopt_stretch >= 1, narrow >= 2, cur >= 4, drag/relay > 0) live in the fixture, which folds
# them into PASS/FAIL. The expected steady state (measured on this spec's own gate) is
# narrow=3/12 adopt_stretch=4/4: only the three sprite-free CHAIN passes narrow — the CURSTICK
# widening rounds the sprite-covered staircase bands to whole boxes on an 8-row surface,
# deliberately, and pass C is whole-box by design. adopt_stretch is the term that BITES: four
# consecutive banded seeds of the stack's top, strictly below the parked arrow, drag nothing and
# can carry the sprite (move CUR3_TAKEN) ONLY through the CURSTICK widening — measured RED
# (12aa8a33-content without the widening) reads adopt_stretch=0/4 -> FAIL, while adopt= alone
# stays high on both trees (a session Adopt-closes whether or not any window carried).

# --- the ladder reached the compositor witnesses and they held --------------------------------
REQUIRE \[wm-act\] direct .* -> PASS
REQUIRE \[clickroute\] route .* -> PASS

# --- PTRDEAD (SELFTEST-RACE, 2026-08-27) ------------------------------------------------------
# --- WHY IT BELONGS HERE AND NOT IN x86-fat.spec. `ptrdead_selftest` is `witness`-gated, not
# --- `wc`-gated, so it runs on the plain `./arroyo test` boot too — but this file's SCOPE is
# --- `UNAOS_WC=1 ./arroyo test`, which is the gate the line was actually MEASURED on: 34 captures
# --- across the SELFTEST-RACE experiment (`~/unaos-bench/scratch/rmbp7/selftest/runs/`), present
# --- and well-formed on every one. Putting a REQUIRE where it has been executed is the rule
# --- x86-fat.spec's own DMG-REFUSE note asks for and could not follow (no `mtools` on that host).
# --- WHAT IT GATES that the default FORBIDs cannot: ABSENCE. `[ptrdead] ... -> FAIL` was already
# --- caught by the harness default-forbids rule (x86-fat.spec:55 is where those defaults are
# --- written down), but a fixture that stops running prints nothing and passes silently — the exact
# --- hole the storage witnesses and DMG-REFUSE each had to close.
# --- `skip` is an accepted value for the three legs and is the point of the SELFTEST-RACE change:
# --- `pal::EVENT_QUEUE` is shared and this fixture does not own it, so a run whose window was
# --- raided by a competing drain says so and declines to judge rather than convicting the fold.
# --- The VERDICT token stays PASS/FAIL — never a third value — so this REQUIRE gates presence AND
# --- health, and a `-> FAIL` still reds through the default rule. Measured: 4 raced windows in 34
# --- runs, each carrying a nonzero `fpop`; 30 clean runs with `fpop12=0 fpop3=0`.
REQUIRE \[ptrdead\] backlog whole=(true|skip) nodrop=(true|skip) order=(true|skip) .* fpop12=-?[0-9]+ fpop3=-?[0-9]+ .* -> PASS
# --- The one shape a skip must never hide: a run that skipped EVERY leg tested nothing. `fpop` is
# --- charged per stolen event, so all three legs skipping at once is a machine this fixture cannot
# --- be run in at all, and that should be looked at rather than passed.
FORBID \[ptrdead\] backlog whole=skip nodrop=skip order=skip

# --- DMGOVLP: the one verdict line, in its exact grammar --------------------------------------
# passes: presents that ran; drained: passes whose damage set CLOSED within K extra composites;
# drag_evt/drag_px (RAW pixels — dkpx would round the slivers to 0): the closure's promotion arm
# fired; relay: forwarded damage reached the chain's far window; narrow: passes that painted
# strictly fewer pixels than whole-box; cur: passes run with a live cursor plan (the sprite leg);
# adopt/repaint: the pass tails those cursor passes took; max_ms: worst single measured
# interval; adopt_stretch: stretch passes that carried the sprite through a staged band (the
# CURSTICK conviction — appended at the line's tail, the standing insertion rule).
REQUIRE \[dmgovlp\] verdict passes=\d+/12 drained=\d+/12 drag_evt=\d+ drag_px=\d+ relay=\d+ narrow=\d+/12 cur=\d+/12 adopt=\d+ repaint=\d+ max_ms=\d+ adopt_stretch=\d+/4 -> PASS

# --- FORBIDDEN: every other DMGOVLP outcome ----------------------------------------------------
# The FAIL sweep also catches the teardown LEAK line (it ends `-> FAIL`), as does mbench's default
# FORBID set — stated here anyway so this file gates alone.
FORBID \[dmgovlp\].* -> FAIL
FORBID \[dmgovlp\] WEDGE
FORBID \[dmgovlp\] DRAIN-STUCK
# A SKIP is a leg that did not run: on this spec's own gate (QEMU 1280x800, fixture floor 512x400)
# there is no honest SKIP, so one appearing means the fixture lost its panel or its geometry —
# a red, not a shrug.
FORBID \[dmgovlp\].*SKIP

# --- VUGRES (D-3 RESUMEPAINT) — the pause/resume first-present witness ------------------------
# The ladder's tail after DMGOVLP. Flight-1 Q4's gap: nothing witnessed that a vug's FIRST present
# after a `[vugpause2]` resume edge actually happened. The fixture drives one real pause/resume
# cycle per arm: a task parked on the input futex, released by a real `set_hidden` unhide edge.
# Leg 1 must print the positive line (a measured resume→first-present gap); leg 2 arms the witness
# and never presents, so the backstop must print the negative naming the furthest stage — for the
# fixture's silent task that stage is deterministic: park-return. The verdict gates that both LINES
# printed (emit counters), not merely that the state machine cycled. No numerics pinned: gap_ms is
# scheduler time under TCG and varies run to run.
REQUIRE \[vugres\] first present win=\d+ asid=\d+ gap_ms=\d+
REQUIRE \[vugres\] NO PRESENT since resume win=\d+ — request lost at park-return .* asid=\d+ gap_ms=\d+
REQUIRE \[vugres\] selftest pos=true neg=true -> PASS
FORBID \[vugres\] selftest.* -> FAIL
# A SKIP here is a fixture that lost its panel or its window rows — on this spec's own gate there
# is no honest SKIP (same rule as DMGOVLP above).
FORBID \[vugres\] selftest -> SKIP

# TILEFIT — two windows handed the same box. PULSE-2's last-resort clamp is idempotent, so once the
# greedy flow overflows the work area every later row pins to the same y and restarts cx at GAP: the
# first box of an overflow row is byte-identical to the first box of the last row that fit. On metal
# boot 11 that gave six vugs FIVE distinct rectangles, and the operator counted five windows. The
# covered window cannot even be clicked — `wc_click_route_at` answers with the top row. ALIASED names
# the colliding pair by win id and asid, and it is read off the same `placed` array the fix consults,
# so the verdict and the fix cannot silently disagree.
FORBID \[wm\] tile-fit.* -> ALIASED
