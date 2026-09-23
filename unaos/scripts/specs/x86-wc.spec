# x86-wc.spec — the x86 window-compositor QEMU leg: DMGOVLP (overlap-forced banded damage, with
# the sprite parked on the stack) plus the two ladder witnesses it depends on for ordering.
#
#   QEMU gate:  UNAOS_WC=1 UNAOS_QUARRY=1 UNAOS_FTDIRX=1 UNAOS_QEMU_FULL=1 ./arroyo test 240 -> target/serial.log
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

# --- STRIPVAC: the furniture strip hands back what it vacates (CURSORBG) -----------------------
# A strip that shrinks paints its uncovered ends flat `wm::DESKTOP_BG` through `strip::erase_rect`,
# and until CURSORBG it told NOBODY — no `wm::damage_intersecting`, no desktop present request —
# while `crystal`, `winmenu` and `wm::drain_deferred` all pair their `DESKTOP_BG` write with both.
# On aarch64 `wm::occ_clip` is `OccClip::none`, so a window can lie under the dock's ends and the
# flat erase stamps a hole nothing repaints: render11 measured `[strip] rollup tenant=dock …
# flat_px=33696 -> FLAT-VACATE` against Peter's "background drawing issue" on the same boot.
#
# The fixture forces one vacate with a genuine uncovered span and scores the census. It reds by
# reverting one call: without `restore_vacated` the line reads `uncovered=1 restored=0 flat=1`.
# No numerics are pinned that the fixture does not itself fold into PASS/FAIL — the three counts
# below are the claim, so they are literal rather than \d+.
REQUIRE :: STRIPVAC: .* uncovered=1 restored=1 flat=0 .* :: PASS ::
FORBID :: STRIPVAC: .* :: FAIL ::
# A SKIP here is a fixture that did not run: on this spec's own gate (QEMU 1280x800, word4 surface)
# neither skip arm is honest, so one appearing means the panel or the scratch was lost — a red.
FORBID :: STRIPVAC: .* :: SKIP ::

# --- SPECPINS (2026-09-22) — THE THREE 2026-09-17 FIXTURES THIS GATE COULD SCORE AND DID NOT -----
# --- MENUDROP, SERIALDOOR and W5SPIN all landed on hw-rmbp on 2026-09-17, each with a green QEMU
# --- run quoted in its own ledger row, and NOT ONE of them was pinned by a directive. A fixture
# --- whose verdict no spec reads is a fixture that can stop running silently — the hole the PTRDEAD
# --- block above exists to close, reopened three times in one day. B121's and A9's rows both end
# --- with an OWED clause asking for exactly these lines. They are here now.
# ---
# --- THE RUN-BY LINE GREW TWO KNOBS FOR THIS BLOCK, and they are not decoration: `UNAOS_QUARRY=1`
# --- because SERIALDOOR's leg 3 is Quarry's `\r` (without it the leg reports `skip-knoboff`, which
# --- is honest but scores nothing), and `UNAOS_FTDIRX=1` because the tag producer is that module —
# --- `serialdoor_selftest` is `#[cfg(all(witness, wc, ftdirx))]` and its knob-off stub prints
# --- `:: SERIALDOOR: ftdirx knob off — no tag producer compiled :: SKIP ::`. The SKIP forbid below
# --- is therefore ALSO the knob check: run this file without `UNAOS_FTDIRX=1` and it reds by name
# --- rather than passing on a fixture that never ran.

# --- MENUDROP (rmbp-ledger B121) — the MENUBAR band the x86 router never had. `band_lines=2` is the
# --- field that BITES and it is pinned literally rather than as `\d+`: the fixture's own verdict
# --- already ANDs `opened && closed`, so a PASS proves the two presses worked — but `band_lines` is
# --- the count of `[clickroute] … band=menubar` lines the ROUTER owed, and it is the only field
# --- that separates "the router's new arm ran" from "`winmenu::press_at` did the work directly",
# --- which is precisely the confusion that let `winmenu::selftest` pass on every x86 boot while the
# --- metal press was inert. `routed_open`/`open`/`routed_close`/`closed` are named for the standing
# --- reason this file states at DMGOVLP: a later edit that drops one from the line reds this rule
# --- instead of silently narrowing what it asserts.
REQUIRE :: MENUDROP: .* routed_open=true open=true routed_close=true closed=true band_lines=2 :: PASS ::
FORBID :: MENUDROP: .* :: FAIL ::
# --- MENUSTAT (rmbp-ledger B148, fold 2026-09-22) — the battery item in the bar's status area, decoded from the
# flight-11 SMC bytes by the fixture (`src=fixture`, witness-gated, chained from menubar::selftest on this lane), with its
# go-red inside the passing run (`red_pct=100`) and the layout assertion that the clock's rect and the caption floor do
# not move with the item present or absent. Pinned by the seat at the fold in the MENUDROP shape; measured by the next gate.
REQUIRE :: MENUBATT: .* decode_ok=true gone_red=true absent_ok=true layout_ok=true .* :: PASS ::
FORBID :: MENUBATT: .* :: FAIL ::
# --- MENUFIRST (rmbp-ledger B156) — the bar's FIRST PAINT, and the number the arc was sent to find.
# The brief's 5051 ms gap does not exist: `[menubar] live` is a ~5 s ROLLUP and `compose` ticks it ABOVE
# the damage test and the paint, so flight 11's `paints=0`@27616ms and `paints=1`@32667ms are one pass and
# ONE paint (`paint=1130360cyc/419us` reads identically at 32667, 37866 and 43053 ms, where the ledger
# accumulates). Metal measured `after_enable_ms=1`; this gate measures `after_enable_ms=1` on its own edge.
# THREE fields BITE and none of them is `.*`. `bounded=true` is the claim — `after_enable_ms <= bound_ms`,
# two composite frames. `gone_red=true` is its CONTROL: leg 4 pushes the edge back by the brief's OWN
# 5051 ms (`RED_INJECT_MS`, self-calibrated into cycles through the same `strip::cycles_to_us` the reading
# uses) and requires the bound to answer false — without it `bounded=true` would pass on a predicate that
# ignored its argument, and with it a green run is also the standing statement that the recorder WOULD have
# caught that gap had it been real. And `crystal=drawn` is Peter's own word from flight 11 ("shows a broken
# crystal"), which the draw path answers is a COMPLETE gem on an EMPTY bar — `crystal_facet` reads only
# `const` inks and `const` geometry, so the mark cannot be half-drawn by timing.
# `unstamped`/`stamped`/`painted`/`recorded` are named for the standing reason this file gives at DMGOVLP:
# a later edit that drops one reds this rule instead of silently narrowing what it asserts.
REQUIRE :: MENUFIRST: after_enable_ms=\d+ bound_ms=\d+ model=[a-z:+]+ crystal=drawn .* unstamped=true stamped=true painted=true recorded=true bounded=true gone_red=true :: PASS ::
FORBID :: MENUFIRST: .* :: FAIL ::
# --- and the WITNESS itself, which is the line a metal capture is actually read for — emitted from the
# paint site, once per boot. `model=` is pinned as a shape and not as a value on purpose: what it names is
# the state of the MACHINE at the seam (`partial:caption+clock` on this gate, `partial:caption+batt` on
# flight 11's metal), and a spec that demanded `complete` would be gating on the two rows B156's STOP hands
# to `wm.rs` and `video/status.rs`. What is pinned is that the bar SAYS which, and that the gem was drawn.
REQUIRE \[menubar\] first-paint at=\d+ after_enable_ms=\d+ model=[a-z:+]+ crystal=drawn rect=\d+x\d+\+\d+\+\d+
# --- A SKIP is `wm::create` declining, or the bar never publishing the caption inside 250 ms. On
# --- this gate (QEMU 1280x800, one 8x8 fixture row) neither is honest — same rule as DMGOVLP and
# --- STRIPVAC above — so a SKIP means the fixture lost its panel or its window table.
FORBID :: MENUDROP: .* :: SKIP ::

# --- SERIALDOOR (rmbp-ledger A9) — the wire is a console. THREE legs are named because the verdict
# --- is only worth pinning with its CONTROL in it: `control=true` is an UNTAGGED Esc being eaten by
# --- the live key door, and without it a green `wire=true` is indistinguishable from a door that
# --- died. `quarry=true(...)` keeps the parenthesised state out of the pin — `ran`, `ran-nodoor`,
# --- `skip-unopened`, `skip-knoboff` — because a knob-off Quarry is not this door's defect and the
# --- fixture already folds the skips to `true`; what is pinned is that the leg REPORTED. The three
# --- census numbers are `\d+` and not literals: `claimed`/`outstanding` depend on how many bytes
# --- leg 4 pushed before the drain ran, which is another task's scheduling (the WINMENUFLAKE rule),
# --- and `overrun` is a ring-pressure fact. `e2e=pushed` is literal — it is a statement about what
# --- the FIXTURE did, not about what the shell got back, and it must not quietly become a verdict.
REQUIRE :: SERIALDOOR: .* control=true wire=true quarry=true\(.*\) claimed=\d+ outstanding=\d+ overrun=\d+ e2e=pushed :: PASS ::
FORBID :: SERIALDOOR: .* :: FAIL ::
# --- Both SKIP arms, and here they mean different things — no panel / table full is the DMGOVLP
# --- rule again, but `ftdirx knob off` is the RUN-BY line being disobeyed. Either way a run that
# --- prints one has scored nothing, which is the only thing a spec must never call a pass.
FORBID :: SERIALDOOR: .* :: SKIP ::

# --- DOCKID (DOCKID2, 2026-09-22; rmbp-ledger B165, FIXTURE_FLAKES §1d) — A FOURTH FIXTURE THIS
# --- GATE COULD SCORE AND DID NOT. `dockid_selftest` has printed `:: DOCKID: … :: FAIL ::` on this
# --- bench twice in 46 QEMU boots and three times on the rMBP's own metal (flights 8 and 11), and
# --- NO directive in ANY spec named it: every one of those reds was convicted by `mbench`'s builtin
# --- DEFAULT_FORBIDS seeing the bare `:: FAIL ::` and nothing else. That is the hole this whole
# --- block exists to close, and it means the fixture could have stopped running entirely without a
# --- single gate noticing — which is not hypothetical here, because the SKIP arm below is new.
# ---
# --- THE REQUIRE IS SHAPED FOR THREE THINGS AT ONCE. (a) ABSENCE, which DEFAULT_FORBIDS
# --- structurally cannot see — it reads a FAIL, never a missing line (pi4-regression.spec:2042
# --- states the same argument). (b) THE NEW FIELDS: `reconciled=<ran>/<drives> folds=<n>` is
# --- DOCKID2's Class 6 leg and a later fold that drops it must red HERE rather than silently
# --- narrowing what the verdict asserts — the standing reason this file gives at DMGOVLP and
# --- MENUDROP. (c) THE HONEST SKIPS, which are kept legal: the fixture's two `fixture — table full`
# --- arms are real properties of a full window table and are matched by the second alternative, so
# --- this rule reds on a LOST fixture and never on a declined one.
REQUIRE :: DOCKID: (.* reconciled=\d+/\d+ folds=\d+ :: (PASS|SKIP) ::|fixture — table full.* :: SKIP ::)
FORBID :: DOCKID: .* :: FAIL ::
# --- AND THE SKIP ARM IS CLOSED ON THIS LANE, which is the half that carries the new risk. DOCKID2
# --- gave the fixture the declined step's own reading: `wm::composite()` returns identically whether
# --- it ran a pass or was DECLINED by `COMP_GATE`, so `composite_reconciled` asks the dock's own
# --- reconcile counter, waits the holder out BOUNDED (250 ms) and retries, and prints
# --- `:: DOCKID: reconciled=false … -> SKIP ::` when a composite it drove never reached
# --- `dock::compose`. That is the right verdict on METAL, where five real cores hold the gate and
# --- all three sightings are exactly this. It is NOT an honest outcome HERE: on this lane the
# --- fixture runs on the boot task and drives its own composites, and 250 ms is over fifty times
# --- the mean honest TCG pass (`[comp2] pass_us=4265`), so a run that still could not reconcile has
# --- a contention problem this gate must report and not swallow. REQUIRE-or-skip above,
# --- FORBID-the-skip here — pi4-regression.spec:2032/2052's shape, and the reason a widened
# --- assertion was refused: the skip is a PROPERTY, and a property is pinned, not hidden.
FORBID :: DOCKID: .* reconciled=false
# --- BOTH SPELLINGS, because one of them can go missing and the other must still bite: the line
# --- above is the fixture's REASON line, the one below is the VERDICT carrying the same outcome as
# --- `:: SKIP ::` with its `reconciled=<ran>/<drives>` short. The honest `fixture — table full`
# --- skips match NEITHER — they are properties of a full window table, not of a declined pass, and
# --- the REQUIRE above keeps them legal.
FORBID :: DOCKID: .* reconciled=\d+/\d+ folds=\d+ :: SKIP ::

# --- W5SPIN — THE CENSUS, and it is pinned for PRESENCE and not for a value. `spin=`/`wedge=` are
# --- the per-window shadow-acquire counters `pace_shadow_acquire` feeds (video/wm.rs:2049, census
# --- at :2288), appended to the per-window `[wpace] … mode=panel` line. On a healthy QEMU boot they
# --- read `spin=0 wedge=0` and NEITHER `:: [wcser] PRESENT-BANDED SPIN …` arm prints at all — one
# --- line per TRANSITION, never per present — so the transition lines cannot be required anywhere
# --- and the ONLY positive evidence that the bound exists in the shipped build is that the census
# --- fields are on the wire. That is the whole claim here: the counters print. A NUMBER is
# --- deliberately not pinned (`\d+`): contention under TCG is scheduler luck, and a gate that
# --- demanded `spin=0` would red a run that contended once and recovered correctly, which is the
# --- bound WORKING. `x86-witness.spec` carries the other half — the three lines a metal boot that
# --- lost this race prints, as FORBIDs, measured on the flight-10 capture that printed them.
# --- The `\d+` on `win=` and the `(yes|no)` on `live=` are there so this rule reds if the line is
# --- ever re-shaped rather than matching a prefix that happens to survive.
REQUIRE \[wpace\] win=\d+ asid=0x[0-9a-f]+ live=(yes|no) mode=panel .* spin=\d+ wedge=\d+ frame_us=\d+

# --- VUGPROBE / VUGPERF (rmbp-ledger B142, and the OWED clause B135 ends with) — THE FIXTURE THAT
# --- SHIPPED UNSCORED, NOW DRIVEN. VUGPERF landed `pace_pin_probe` and its own QEMU gate never
# --- printed the verdict: `[wpace] coalesced=1` summed over a 248 s boot, the one shadowed window
# --- was closed before any pass composited from it, `probes=0`, and the emitter's `if probes > 0`
# --- guard correctly said nothing. The go-red was therefore NOT RUN — a re-widened build prints
# --- the same silence, so the red would have been vacuous. That is exactly the silent-fixture hole
# --- the PTRDEAD block above exists to close, and it is why BOTH rules below are here:
# --- `:: VUGPROBE:` says the DRIVER ran, `:: VUGPERF:` says the probe it drove answered. Pinning
# --- only the second would let the driver be deleted and this gate stay green through the very
# --- `probes=0` silence the arc was written to end.
# ---
# --- `coalesced=1 shadow=true lost=false` are LITERAL, because they are the claim. The fixture
# --- (`wm::vugprobe_selftest`, ladder tail in arch/x86_64/syscall.rs) mints ONE ring-3-band row —
# --- non-zero owner outside `KERNEL_OWNER_BASE`, const-asserted, because a kernel-band row is
# --- pace-EXEMPT at `pace_admit` and could never coalesce — and presents it through
# --- `present_outcome` until one present comes back `Coalesced`. The second present lands inside
# --- the first one's 16.667 ms frame, and COALESCING is what calls
# --- `pace_shadow_refresh(.., create = true)`: the shadow is created at a REAL present boundary by
# --- the real presenter, never by poking `PACE_SHADOW` or its validity bit — the B121 mistake
# --- (`winmenu::selftest` green on every x86 boot through a seam the live path never called) is
# --- what that costs if it is got wrong. `shadow=` is the validity bit READ BACK, so `shadow=false`
# --- is the fixture asserting about a window `pace_shadow_source` would answer `Live` for, and
# --- `coalesced=0` is the pacer never folding at all. `paced=` and `tries=` are reported, not
# --- pinned: a recycled slot can carry a stamp that coalesces the FIRST present (`paced=0` is
# --- honest), and the retry budget exists because the gap between the two presents IS the first
# --- present's own composite pass (`[comp2] pass_us=4265` mean under TCG, with a 170 ms tail).
REQUIRE :: VUGPROBE: shadow-drive win=\d+ tries=\d+ paced=\d+ coalesced=1 shadow=true lost=false .* :: PASS ::
FORBID :: VUGPROBE: .* :: FAIL ::
# --- A SKIP is no panel or a full window table. On this gate neither is honest — the DMGOVLP /
# --- MENUDROP / STRIPVAC rule above, one row and the same panel — so a SKIP is a lost fixture.
FORBID :: VUGPROBE: .* :: SKIP ::

# --- And the verdict the driver exists to make scoreable. `probes=[1-9]\d*` is the whole arc: it is
# --- the population B135 did not have, and it is pinned as "at least one" rather than as a number
# --- because a second armed window on a future boot must not red this. `free_pct=` is `\d+` and the
# --- threshold is NOT duplicated here — the fixture folds `bound=free_pct>=90` into its own
# --- PASS/FAIL, and `bound=` is pinned verbatim so a later edit that loosens the bound reds this
# --- rule instead of quietly passing under a new one. GO-RED, MEASURED on this gate and not
# --- reasoned: revert `pace_shadow_source`'s tail to `r.surf = g.as_ptr() as usize;
# --- ShadowSrc::Shadow(g)` — the pass holds the shadow across the whole per-window iteration again
# --- — and the same capture reads `probes=1 free=0 held=1 free_pct=0 … :: FAIL ::`. That direction
# --- is deterministic on any host at any speed: the probe's try-lock is then contending with the
# --- pass that is itself the holder, so it CANNOT succeed.
REQUIRE :: VUGPERF: shadow-pin probes=[1-9]\d* free=\d+ held=\d+ free_pct=\d+ .* bound=free_pct>=90 :: PASS ::
FORBID :: VUGPERF: .* :: FAIL ::

# --- BOOTFAILS — HALF OF IT BELONGS HERE AND HALF DOES NOT, and the half that does was already
# --- pinned. `[clickroute] route … -> PASS` is line 43 of this file and has been since the ladder
# --- witnesses were added, so BOOTFAILS's router leg is covered without a new directive. Its OTHER
# --- leg, `[wc-x] move-vacate … -> PASS`, is NOT PINNED HERE AND MUST NOT BE — measured, not
# --- assumed: `move_vacate_probe` is called from `desktop_uefi::activate_on`
# --- (video/desktop_uefi.rs:599), this file's SCOPE paragraph says `desktop_uefi::activate` never
# --- runs under QEMU because there is no Kepler, and an unperturbed `UNAOS_WC=1 UNAOS_QUARRY=1
# --- UNAOS_QEMU_FULL=1` capture (`~/unaos-bench/scratch/rmbp-0915/quarryclick-logs/serial-clean.log`)
# --- carries ZERO `[wc-x] move-vacate` lines. A REQUIRE for it here would red every run of this
# --- gate forever. It is a PENDING in `x86-witness.spec`, where the boot that prints it lives.

# --- KEYMAP (R60/R61, 2026-09-22) -------------------------------------------------------------
# --- The binding TABLE's resolver. `video/keymap.rs`'s fixture is chained from
# --- `drivers::ehci::parser_selftest`, which this lane runs on every boot (`:: EHCI-HID:
# --- report-parser self-test:` is on every capture of it), and it is the ONLY QEMU-provable
# --- witness for the chord path: QEMU has no operator's hands, so no real chord is ever pressed
# --- here and `:: PRTSCR: [prtscr] chord=` has never appeared on this lane at all (measured
# --- 2026-09-22: 0 hits across every QEMU capture under ~/unaos-bench/scratch/rmbp-0915/*-logs/;
# --- the only captures carrying it are the metal flight-11 logs). What the fixture proves is the
# --- DECISION, which is the whole of what R60 moved out of the two USB drivers.
# --- NO NUMERICS PINNED: `resolved=` is \d+ because it counts AGREEMENTS and a future row raises
# --- it. The VERDICT token gates — `-> PASS`, and a `-> FAIL` reds through the harness default
# --- rule — and beside it three fields are named VERBATIM, chosen because each would still read
# --- `ok` if the thing it measures were broken: `ctrl_c_ascii=0x03` (R61 — the shell's byte,
# --- measured through `hid_key_ascii`, not asserted), `ctrl_c_action=none` (R61 — the table never
# --- claims Ctrl-C, which is why no terminal special case exists) and `pc_table_alt_c=copy` (R60 —
# --- the SAME role row read through a table whose `cmd_role` is `HID_MOD_ALT`; if the role
# --- indirection were cosmetic this one field would read `none` while every other stayed `ok`).
# --- GO-RED, ONE EDIT: delete the `Cmd+C` row from `theme::CRISPY_ROWS` -> `copy=no`, `resolved=`
# --- drops by one, verdict `-> FAIL`.
REQUIRE :: KEYMAP: table=crispy resolved=\d+ .* ctrl_c_ascii=0x03 ctrl_c_action=none pc_table_alt_c=copy .* -> PASS ::

# --- APPCLIP (R61, 2026-09-22) ----------------------------------------------------------------
# --- The other half of R61: KEYMAP resolved the edit chords and stopped at the decoder, with no
# --- delivery path, no clipboard and no consumer (keymap.md §6 named the seam). This row scores
# --- the whole of what was built on top of it — `pal::Event::Action` through the REAL ring, the
# --- session-owned clipboard, and the terminal's consumption of a paste — from one fixture
# --- (`video/clipboard.rs`'s `selftest`), chained BESIDE `keymap::selftest` in
# --- `drivers::ehci::parser_selftest`, which this lane runs on every boot.
# --- WHY A FIXTURE AND NOT A CHORD: the same reason the KEYMAP row above gives. QEMU has no
# --- operator's hands, `:: PRTSCR: [prtscr] chord=` has never appeared on this lane, and the new
# --- `[clip] chord=… -> delivered` line it sits beside cannot appear here either. What CAN be
# --- driven off metal is the ring, and the fixture drives it: four `Event::Action`s go in through
# --- `pal::push_event` and come back out of `pal::next_event`, the shipped `terminal_action`
# --- consumes them, and the paste arrives as `Event::Key`s on that same ring which the fixture
# --- rebuilds the line from. So every field below is a ROUND TRIP, not a restatement.
# --- THE FIELDS PINNED VERBATIM, each chosen because it would still read well if the thing under
# --- it were broken: `line_match=true` (the pasted bytes compared against the source AFTER the
# --- round trip — a delivery that never happened, a clipboard that stored nothing and a paste
# --- that pushed nothing each read `false` here, and it is the ONE field the go-red moves);
# --- `cut=empty` and `selectall=ok` (asserted AS VALUES; `unsupported` until TERMSEL, 2026-09-23,
# --- which built the selection model and changed this row as the row said it must — with nothing
# --- selected `⌘X` is declined on the wire and `⌘A` selects the line); `epoch_clear=ok` (the
# --- session ownership gate — the buffer's stamp is aged by one epoch, exactly as a log-out
# --- leaves it, and the next read must destroy the buffer instead of serving the previous user's
# --- text). `delivered=4` is pinned as a literal because it is a CONSTANT of the fixture, not a
# --- population: four actions are pushed, and any number but four means the ring lost one.
# --- NO OTHER NUMERIC IS PINNED: `len=` is \d+ so the fixture's sample text can change.
# --- GO-RED, ONE EDIT (measured, not reasoned): delete the `paste_into_ring()` call from
# --- `terminal_action`'s `Action::Paste` arm -> no `Event::Key` reaches the ring, the line is
# --- never built, and the same capture reads `paste=ok len=0 line_match=false … -> FAIL ::`.
REQUIRE :: APPCLIP: delivered=4 copy=ok paste=ok len=\d+ line_match=true cut=empty selectall=ok epoch_clear=ok -> PASS ::
FORBID :: APPCLIP: .* -> FAIL ::

# --- STARTHOLD: the desktop's ignition is never gated on a fixture's settle (2026-09-22) -------
# --- WHY A FORBID AND NOT A REQUIRE, written down so the next reader does not "fix" it into one:
# --- this leg cannot produce a `[wc-x] desktop-app` line AT ALL. `desktop_uefi::activate` runs only
# --- from the Kepler takeover, so `DESKTOP_APP_ARMED` is false on every `./arroyo test` boot and the
# --- whole desktop-app seam — ARMED, the storage wait, the DMG-REFUSE hold, LAUNCH — has never once
# --- executed in CI. That absence is itself part of the finding this rule records: the 15s hold it
# --- retires lived its entire life with zero QEMU coverage and was only ever measured on metal.
# --- THE RED WAS TAKEN ON A CAPTURE, not asserted — the property B160 showed a FORBID is worthless
# --- without, because a FORBID that can no longer match reads identical to one that passed.
# --- Replaying THIS FILE against the flight-11 metal log (read-only evidence,
# --- `~/unaos-bench/scratch/rmbp-0915/bootwaits-logs/f11.log`) scores this rule 1 hit:
# ---   [  43069ms] [wc-x] desktop-app HOLD-EXPIRED reason=dmg-refuse-unsettled name=/STAT.ELF \
# ---       waited=15005ms threshold=15000ms — launching anyway
# --- WHAT IT GATES: the return of a wall-clock gate on the ignition path. The refusal witness's
# --- settle is a signal the desktop cannot make arrive — the DMG launcher is the TAIL of the witness
# --- ladder, whose own bounded sub-waits sum past 15s — so the cap was smaller than the bound of the
# --- thing it waited for and had to expire on exactly the boots that needed it. Flight 11 paid the
# --- full `waited=15005ms` AND still lost the witness (`occupied=0x01 … NOT RUN`, five seconds after
# --- the launch it was supposed to precede). `held_ms=` replaces it: reported, never gated.
FORBID \[wc-x\] desktop-app HOLD-EXPIRED
# --- SCRSHOT-DESKTOP (R60, 2026-09-22; rmbp-ledger B164) — WHERE A SCREENSHOT LANDS ------------
# --- THE HOLE THIS CLOSES IS THE ONE THE SPECPINS BLOCK ABOVE IS ABOUT. `prtscr::dir_fixture` has
# --- run on EVERY x86 and arm boot since 2026-09-13 and NO SPEC HAS EVER READ IT: two PASS lines on
# --- every capture in `~/unaos-bench/scratch/rmbp-0915/*-logs/`, scored by nobody. It is ungated —
# --- deliberately, because the no-session refusal is what every board does today and a fixture
# --- behind a knob would not have measured the ordinary case once — so it is also the fixture with
# --- the most to lose from going silent, and silence is exactly what a FORBID cannot catch.
# ---
# --- KNOBS: NONE OF THEM. `dir_fixture` is compiled into every build and called from
# --- `prtscr::service`, so these two rules hold on the plain `./arroyo test` lane as well as this
# --- file's. They are HERE rather than in x86-default.spec because this is the file with a RUN-BY
# --- line a gate command actually executes, and a pin nobody replays is the landmine the CONTRACT
# --- block below names. MEASURED on this seat's SCRSHOT-DESKTOP fold capture:
# --- `UNAOS_WC=1 UNAOS_QUARRY=1 UNAOS_FTDIRX=1 UNAOS_SMC=1 UNAOS_LOGIN=1 UNAOS_QEMU_FULL=1
# --- ./arroyo test 240` — a SUPERSET of the RUN-BY line below, and the two extra knobs cannot move
# --- either rule: `UNAOS_SMC` is MENUBATT's and touches no PRTSCR line, and `UNAOS_LOGIN` moves
# --- arm B's `live=` token between exactly the two values the alternation already admits.
# ---
# --- ARM A — THE DESTINATION, AND IT IS PINNED LITERALLY BECAUSE IT IS THE RULING. R60: *"mac saves
# --- to desktop, correct? we should too, on this pioneer crispy theme anyway."* `dir=` is the
# --- destination as the USER says it and `HOME/UNA/DESKTOP` is what the MEDIUM spells (`format_83`
# --- upcases what it stores); both are literal here, and so is `theme=crispy`, because a `.*` in
# --- any of the three would let the folder move without reddening the one rule that exists to stop
# --- it. `legal83=true` is the necessary condition `format_83` (`fs/fat.rs:325`) imposes on the
# --- theme's word — one component, no dot, base 1..=8 — restated in `prtscr` because that function
# --- is private to `fs/fat.rs` and the SCRSHOT-DESKTOP arc does not touch that file. The
# --- parenthetical between them is `.*`: it is prose about the 8.3 rule, it carries no claim, and
# --- pinning prose is how a spec ends up edited for a comma.
# --- GO-RED, MEASURED on this gate and not reasoned — point `video::theme::CAPTURE_DIR` at the
# --- pre-R60 `"Screenshots"`, which is the name `format_83` refuses, and the same capture reads
# --- `dir=/home/una/Screenshots … -> HOME/UNA/SCREENSHOTS (want HOME/UNA/DESKTOP; "Screenshots" is
# --- 11 chars …) legal83=false -> FAIL ::`. BOTH halves move: the destination is wrong AND the name
# --- could never have been created. That direction is deterministic on any host at any speed — the
# --- arm is volume-free and its want is a literal, so nothing in it depends on a disk or a clock.
REQUIRE :: PRTSCR-DIR-FIX: theme=crispy dir=/home/una/Desktop home=/home/una -> HOME/UNA/DESKTOP .* legal83=true -> PASS ::
# --- ARM B — R54 SURVIVED THE MOVE, AND THAT IS THE HALF A DESTINATION CHANGE COULD HAVE BROKEN.
# --- "No session, no capture" is Peter's 2026-09-13 ruling (*"do not hack screenshots to make it
# --- work right before multi-user is in"*) and R60 changed WHICH folder, never WHETHER. Three
# --- fields are the claim and are literal: `reason=no-session` (the token), `plan_none=true` (the
# --- pure assertion) and `bytes=0`. `live=` is an alternation because the two tokens are two
# --- different FACTS about the image and both are honest — `no-login-built` is a build with no user
# --- store (`UNAOS_LOGIN` off, the default), `no-session` is one that has the store and nobody
# --- logged in (this file's measured lane arms `UNAOS_LOGIN=1`). The census is `\d+->\d+` and not
# --- `0->0`: what the arm asserts is that the two are EQUAL, which the fixture folds into its own
# --- PASS/FAIL, and a future boot that legitimately captured before this pass must not red here.
# --- `session-open-live-leg-skipped` is deliberately NOT admitted: it is the token for a board that
# --- HAS a session, no lane prints it today, and the day one does this rule should be looked at
# --- rather than pass silently through an alternation written before that boot existed.
REQUIRE :: PRTSCR-DIR-FIX: no session -> REFUSED reason=no-session plan_none=true live=(no-session|no-login-built) captures \d+->\d+ bytes=0 .* -> PASS ::
FORBID :: PRTSCR-DIR-FIX: .* -> FAIL ::
# --- AND THE LINE THAT IS NOT PINNED HERE, WITH THE MEASUREMENT THAT SAYS WHY. The LIVE witness
# --- `:: PRTSCR-DIR: theme=crispy user=… dir=/home/<name>/Desktop … -> RESOLVED ::` needs a SESSION
# --- and a WRITABLE VOLUME, and this lane has neither: `UNAOS_LOGIN=1 ./arroyo test` builds the
# --- store and nobody logs in (the `loginst` fixtures open a session and CLOSE it again —
# --- `fs/users.rs:918` puts the boot back where it found it), and the default medium is read-only
# --- to the PRTSCR-VOL ladder's rung 1 with no USB FAT attached for rung 2. A REQUIRE for it would
# --- red every run of this gate forever — the BOOTFAILS rule this file states thirty lines up.
# --- PENDING, for the boot that can print it: `UNAOS_PRTSCRST=1 ./arroyo test-fat sf` once
# --- LOGINFLOW holds a session open, and metal flight 12 under R59's read-write boot volume, where
# --- flight 11 could only print `:: PRTSCR-VOL: rung=none rung1=read-only rung2=absent -> NO TARGET
# --- ::` against a real ⌘⇧3 press at 798 s.
# --- DMGYIELD (2026-09-22; rmbp-ledger B172) — THE REFUSAL WITNESS YIELDS TO AN OCCUPIED TABLE ---
# --- THE HOLE THIS CLOSES IS THE ONE STARTHOLD OPENED, thirty lines up. `dmg_refuse_witness`
# --- demanded an EMPTY window table at entry (`syscall.rs:19073` at base ec9c371b) and printed
# --- `NOT RUN` otherwise. Its real need is FOUR FREE ROWS — one the owner takes, two the prober
# --- takes, one that must stay free as `id_free` — and every term of the grade was ALREADY
# --- slot-scoped or single-bit. With the 15 s ignition hold deleted, every metal boot launches
# --- `/STAT.ELF` into row 0 BEFORE the ladder reaches DMG, so without the yield the refusal arms
# --- are NOT RUN on every flight from 12 onward. The whole `SYS_WIN_PRESENT_ROWS(33)` refusal
# --- contract would have gone dark on metal while reading green in CI — silently, which is the
# --- failure mode this file's PTRDEAD note already names.
# ---
# --- ARM A — THE VERDICT, and `yielded_to=` is the field this arc adds. NOT pinned to a literal:
# --- on THIS lane it reads `0x00` (QEMU has no Kepler, `desktop_uefi::activate` never runs, so no
# --- app holds a row — the same absence the STARTHOLD FORBID is written around) and on metal it
# --- reads the app's mask; pinning either would red the other. What the arm gates is that the
# --- field EXISTS beside an OK verdict: a build that quietly dropped the yield would still print
# --- `19/19 … witness OK` on this lane and NOTHING else here would see the missing term.
# --- `presents=\d+` for the standing no-numerics reason. MEASURED on this seat's wc-lane capture,
# --- `UNAOS_WC=1 UNAOS_QUARRY=1 UNAOS_FTDIRX=1 UNAOS_SMC=1 UNAOS_QEMU_FULL=1 ./arroyo test 240`:
# ---   :: DMG-REFUSE: … the window still presented after all 13 refusals; yielded_to=0x00
# ---       presents=6 (slot, exactly) global=+6 (want 6) — witness OK ::
# --- and with a scratch co-tenant standing in row 0 AND PRESENTING through the witness (REVERTED),
# --- the same gate reads `yielded_to=0x01 presents=6 (slot, exactly) global=+8 (want 6) — witness OK ::`.
# ---
# --- RE-PINNED BY PRESENTSLOT (2026-09-22, rmbp-ledger B178), under the SPECRUN contract at the foot
# --- of this file: the kernel line this rule pins changed in the same commit, so the rule changed
# --- with it rather than being left to match loosely. DMGYIELD's `presents={} (want {}, exactly|at
# --- least: …)` became `presents={} (slot, exactly) global=+{} (want {})` — the graded count is now
# --- the PROBER'S OWN address-space row (`FB_PRESENT_COUNT_SLOT`), exact in BOTH cases, and the
# --- global counter is reported beside it and never graded. WHY THE TWO NEW TERMS ARE IN THE PATTERN
# --- AND NOT LEFT TO `.*`: the OLD rule still matches the NEW line verbatim (`presents=\d+ .*witness
# --- OK ::` is satisfied by it), so leaving it alone would have gated NOTHING that this arc adds — a
# --- build that reverted to grading the global counter would print `presents=6 … witness OK` on this
# --- empty-table lane and pass. `\(slot, exactly\)` is the term that says WHICH counter was graded,
# --- and `global=\+\d+` is the term that says the co-tenant's traffic was measured rather than
# --- ignored. Both are `\d+`, never literals: this lane reads `global=+6` with nothing else painting
# --- and metal reads the app's cadence.
REQUIRE :: DMG-REFUSE: .*19/19 probes from two ring-3 slots agree.*yielded_to=0x[0-9a-f]+ presents=\d+ \(slot, exactly\) global=\+\d+ \(want \d+\) .*witness OK ::
# --- ARM B — THE RETIRED DEMAND, AND THE RED WAS TAKEN ON A CAPTURE (B160: a FORBID that cannot
# --- match reads identical to one that passed). Replaying THIS FILE against the flight-11 metal log
# --- (read-only evidence, `~/unaos-bench/scratch/rmbp-0915/bootwaits-logs/f11.log`) scores this rule
# --- 1 hit:
# ---   [  48074ms] :: DMG-REFUSE: the window table was not empty at entry (occupied=0x01) — \
# ---       refusal witness NOT RUN ::
# --- and 0 hits on this arc's own after-captures. Both directions, on real wire. It reds the day the
# --- empty-table entry gate comes back, by its exact retired wording.
FORBID :: DMG-REFUSE: the window table was not empty at entry
# --- ARM C — the DMG FAIL line, which the harness default FORBIDs cannot see. `:: DMG-REFUSE FAIL —
# --- probes=… ::` contains neither `-> FAIL` nor `FAIL ::`, the same hole `x86-fat.spec` records
# --- for its S-witnesses and closes with the twin of this line (`x86-fat.spec:91`).
FORBID :: DMG-REFUSE FAIL
# --- AND THE ONE SHAPE DELIBERATELY NOT FORBIDDEN: `only N of 12 window rows are free at entry,
# --- fewer than the 4 this fixture needs (occupied=0x…) — refusal witness NOT RUN`. That is the ONE
# --- remaining honest decline, it names its count and its mask, and `x86-fat.spec:92`'s
# --- `FORBID DMG-REFUSE:.*NOT RUN` already reds on it where the table is provably empty. Forbidding
# --- it HERE would red a boot that behaved exactly as designed the day a co-tenant fixture lands
# --- ahead of DMG in this ladder — wrong-strict, the rule the next seat deletes.
# --- MENUBATT2 (rmbp-ledger B170) — THE STATUS POLL STATES THAT IT RAN. Extends the MENUBATT pair
# --- at :163. APPENDED HERE and not beside it, because this file's own CONTRACT block below names
# --- tail-append as the safe form and rows cite spec lines POSITIONALLY — an insert at :164 would
# --- move MENUFIRST's :180/:187 and every citation of them.
# --- WHY THIS PIN EXISTS. `[menubar] battery` is SILENT while the source is `Unresolved`, by its own
# --- design, so "the sweep never ran", "it ran and nothing resolved" and "this code is not in the
# --- image" all printed the same nothing. B156 read flight 11's 2.2 MB, found no `[menubar] battery`
# --- line, and concluded the second. It was the THIRD: flight 11's image is 56bbe53b (2026-09-22
# --- 08:12) and `video/status.rs` does not exist in that commit — MENUSTAT folded 7.5 h later.
# --- `[status] poll` is emitted from inside `status::poll`'s throttled body, so the LINE's existence
# --- is the proof the sweep ran, and its absence is now a statement instead of an ambiguity.
# --- `src=none` IS PINNED AS A VALUE, deliberately rather than lazily: QEMU's `isa-applesmc` answers
# --- REV/OSK0 and carries no battery key, so the MEASURED absence is this lane's truth, and with or
# --- without `UNAOS_SMC=1` the sweep resolves `none` here (knob-off takes `board_raw`'s second arm to
# --- the same place). A gate that read `src=smc` would mean QEMU grew a pack; one that read
# --- `src=fixture` would mean MENUBATT's injection escaped its restore. Both are findings, and `.*`
# --- would have hidden both.
# --- `n=`/`answered=`/`took_us=` are shapes and not values — the counts depend on where in the 240 s
# --- wall the capture is cut, and `took_us` is a measurement. They are NAMED for the standing reason
# --- this file gives at DMGOVLP: a later edit that drops one reds this rule instead of silently
# --- narrowing what it asserts.
# --- THE FORBID BITES A REAL INVARIANT, not a spelling: every sweep resolves the source before this
# --- line is emitted, so `src=unresolved` on a `[status] poll` line is unreachable unless the resolve
# --- arms stopped covering `board_raw`'s match. It is not a restatement of the REQUIRE.
# --- GO-RED, ONE EDIT (measured, B170): disarm `super::status::poll();` at desktop_uefi.rs:711 ->
# --- the capture carries ZERO `[status] poll` lines, this REQUIRE misses, `mbench` rc 1 — and that
# --- capture is flight 11's wire, reproduced.
REQUIRE \[status\] poll n=\d+ answered=\d+ src=none took_us=\d+
FORBID \[status\] poll .* src=unresolved


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
# RUN-BY: knobleg — UNAOS_WC=1 UNAOS_QUARRY=1 UNAOS_FTDIRX=1 UNAOS_QEMU_FULL=1 ./arroyo test 240, then
#          ./arroyo mbench --replay target/serial.log --spec scripts/specs/x86-wc.spec --platform x86
#   THE KNOB SET GREW WITH THE SPECPINS BLOCK and each addition is load-bearing, not defensive:
#   `UNAOS_QUARRY=1` gives SERIALDOOR's leg 3 a Quarry to focus (without it the leg reports
#   `skip-knoboff`), `UNAOS_FTDIRX=1` compiles the tag producer the whole fixture is gated on (its
#   absence prints the `ftdirx knob off` SKIP this file now FORBIDs by name), and `UNAOS_QEMU_FULL=1`
#   is the three-valued run mode: `mode=full` on the sidecar is the only reading that certifies the
#   tail was not truncated, and a spec verdict on a short capture is the false green this tree's
#   queue §5 named on 2026-09-17 ("a truncated run must never be a pass"). The wall moved 150 -> 240
#   for the same reason: the SERIALDOOR and MENUDROP fixtures sit late in the witness ladder.
#   NOT on the default gate: `./arroyo test` replays x86-test.spec, and this file asserts a
#   UNAOS_WC=1 build (on x86 the compositor's ignition is the Kepler takeover, so those knobs are
#   load-bearing here). Measured 8/8 against this seat's fold-gate wc capture on 2026-09-15.
#
# GATE-SPECROOTS (`scripts/spec-roots.sh`, a leg of `./arroyo check`) reds by name on any spec under
# scripts/specs/ that is neither named in `arroyo`'s CODE nor carries a RUN-BY line above — and a
# `RUN-BY: verb:` claim is cross-checked against `arroyo`, so this file cannot claim a runner it
# does not have. "A replay spec no gate command runs is a silent landmine."

# ── VUGART (rmbp-ledger B162) — THE VUG'S OWN FRAME IS ONE CRYSTAL ──────────────────────────────
# Peter, flight 11 (2026-09-22): "vug opens struggling so it tries to display the basic line drawing
# but it turns into abstract art." The crystal is ALWAYS a line drawing, so "abstract art" cannot
# mean a different renderer ran — it means the lit pixels on ONE presented surface are not ONE
# wireframe. `crates/user-vug` rasterises its 288x288 surface in THREE disjoint bands (worker A
# 0..108, worker B 108..216, the parent 216..288) from ONE published projection; VUGART stamps each
# band with the GENERATION its pixels came from and scores the three against the frame's own.
#
# APPENDED AT THE TAIL, per the CONTRACT block above: this file's pins are cited positionally from
# ledger rows and kernel comments, and a header insert moves every one of them silently.
#
# THE MEASURED BOUND, stated so nobody reads this rule as stronger than it is. `winx8_launcher`
# kills the CI vug as soon as THREE GLOBAL presents have landed anywhere on the machine, which on
# this bench under eight sibling executors is TWO of the vug's own frames — and the kill means the
# exit witness never runs. So the population this gate scores is `frames=2`, not a long run:
# measured green three times (`vugart-logs/serial-run2.log`, `serial-run3.log`,
# `serial-run7-fixed.log`, all `:: VUGART: frames=2 coherent=2 torn_rows=0 mixed_frames=0 -> PASS ::`).
# Two frames cannot reproduce the defect, and this rule does not claim they can. What it claims, and
# what it enforces, is that the INSTRUMENT IS IN THE SHIPPED IMAGE AND ANSWERING — which is the thing
# a flight-12 metal capture needs in order to read a real population off the wire.
#
# `frames=[1-9]\d*` and not `\d+`: `frames=0` would be the fixture reporting that it scored nothing,
# which is the silent-fixture hole the PTRDEAD and VUGPROBE blocks above exist to close.
#
# GO-RED, MEASURED ON THIS GATE AND NOT REASONED (`vugart-logs/serial-run8-gored.log`): change the
# release in `crates/user-vug/src/main.rs` from `PHASE.store(gen, …)` back to a word that does not
# advance — `PHASE.store(1, …)` is the limiting case of the defect this arc fixed — and the same
# capture reads `:: VUGART: frames=2 coherent=1 torn_rows=216 mixed_frames=1 -> FAIL ::`. 216 rows is
# exactly worker A's band plus worker B's: with no release edge neither worker runs, both bands hold
# the previous rotation with no writer in them, and the parent's own 72-row band holds the next one.
# WINX-8 goes red beside it (`presents=1`) because the parent then blocks at the barrier — that is
# the freeze half of the same mechanism, and it is why a present-only instrument could not see this.
REQUIRE :: VUGART: frames=[1-9]\d* coherent=\d+ torn_rows=\d+ mixed_frames=0 -> PASS ::
FORBID :: VUGART: .* -> FAIL ::
#
# ── TSTETAP (2026-09-22), TAIL-APPENDED past VUGART ───────────────────────────────────────────────
# `absorbed=` on the `tste` tap, pinned on this lane as well as on x86-default.spec, in the same
# commit. The full argument — why a mis-spelt verdict marker makes every boot fixture VANISH from
# `tste` rather than fail, why the recorded go-red still exited 0, and what each field buys — is
# written once at the tail of `x86-default.spec` and is not repeated here.
#
# WHAT THE SECOND COPY BUYS: this is the lane the go-red was actually taken on
# (`sertaps-logs/R3-serial.log`, `absorbed=0` where the green tree reads `absorbed=21`), and it is
# the lane with the deepest fixture population, so it is the one where a silent scanner has the most
# to lose. The default lane's copy guards the cheap leg.
REQUIRE :: SERWIT-2 tap tste: submitted=[1-9]\d* absorbed=[1-9]\d* staged=\d+ dropped=0 suppressed=\d+ torn=\d+ inflight=0 in_progress=\d+ ::
#
# ── SERTXPIN (2026-09-22), TAIL-APPENDED past TSTETAP ─────────────────────────────────────────────
# The `[sertx]` census, pinned on THIS lane as well as on x86-default.spec, in the same commit. The
# full argument for the shape — why the four-name tap table is pinned character for character, why
# every number is open, why `masked_b=0` is the one value asserted, and the two recorded go-reds —
# is written once, at the tail of `x86-default.spec`, and is not repeated here.
#
# WHAT THE SECOND COPY BUYS, because a duplicated rule has to earn itself. This lane is where the
# census is WORTH READING: the compositor is the load that produces the dark windows SERIALTX and
# TAPSMAX were written for, and `tap_max=fbcon:…` on a wc boot is the only place the panel tap's cost
# is visible at all. The default lane's copy gates the line's EXISTENCE on the cheap leg; this one
# gates it on the leg whose numbers anybody actually quotes. Measured 14 hits on the SERTAPS baseline
# capture (`sertaps-logs/R1-serial.log`, the wc lane at this branch's parent fe385712).
REQUIRE \[sertx\] prints=\d+ masked_us_max=\d+ masked_us_mean=\d+ drain_us=\d+ emit_us=\d+ spin_us=\d+ bytes=\d+ masked_b=0 fifo_b=\d+ taps_us=\d+ taps_us_max=\d+ tap_max=fbcon:\d+,ftdi:\d+,tste:\d+,rec:\d+ tap_sum=fbcon:\d+,ftdi:\d+,tste:\d+,rec:\d+ sink=(uart|ftdi|both|none) hz=\d+ masked_cy_max=\d+ masked_cy_sum=\d+
#
# ── SPECPINS2 (2026-09-23, rmbp-ledger B184), TAIL-APPENDED past SERTXPIN ─────────────────────────
# THE CLIPBOARD'S OWN WIRE, beside the APPCLIP verdict this file already pins (rmbp-ledger B176).
# `:: APPCLIP:` is the fixture's conclusion; `[clip] set len= epoch=` and `[clip] copy unit=line len=`
# are the two lines `video/clipboard.rs` prints for EVERY copy, on this lane and on metal
# (`clipboard.rs:166`, `:270`), and no spec read either. They are what a flight-12 capture of a real
# ⌘C will be read with, so their wording is pinned here the way x86-login.spec §7 pins `[login]`.
# Printed on this lane by `clipboard::selftest` (chained from `drivers::ehci::parser_selftest`,
# unconditional under the default-ON `ehcihid`): `set len=10` for the copy, `set len=16` for the
# epoch leg's stale buffer, one `copy unit=line len=10`.
#   * `unit=line` is LITERAL: it is the claim that there is no selection model and the copy took the
#     editor's whole line (B176 §3). The arc that builds a selection must come here and change it.
#   * `len=` and `epoch=` are `\d+`: the fixture's sample text and the session epoch (u64 since
#     SECLOGIN M5) move legitimately.
REQUIRE \[clip\] set len=\d+ epoch=\d+
REQUIRE \[clip\] copy unit=line len=\d+
# The FAILURE spelling is a REAL one: `set` refuses out loud rather than truncating
# (`[clip] refuse reason=too-large len= cap=` / `reason=non-text off= byte=`, `clipboard.rs:140`,
# `:150`). The fixture copies a 10-byte printable string and never asks for a refusal, so on this
# lane any `[clip] refuse` is a copy that failed; `terminal_action` would then report
# `copy=refused` and APPCLIP would FAIL too, but this rule names the cause rather than the symptom.
FORBID \[clip\] refuse reason=
#
# `[status] poll` (MENUBATT2, B170) is NOT re-pinned here: it has been pinned in this file since
# 906e669c, at the MENUBATT2 block (`REQUIRE \[status\] poll n=\d+ answered=\d+ src=none took_us=\d+`
# and its `src=unresolved` FORBID). SPECPINS2 replayed it on this arc's wc-lane capture instead.

# ── TERMSEL (2026-09-23), TAIL-APPENDED past SERTXPIN ─────────────────────────────────────────────
# The terminal's SELECTION over the editable shell line (`video/termsel.rs`, `clipboard.md` §7): the
# chords are rows in the theme's table and the fixture `video::termsel::selftest` — chained after
# APPCLIP's in `drivers::ehci::parser_selftest`, which this lane runs every boot — resolves each one
# from a synthetic report pair through the LIVE table, pushes the action through the REAL ring and
# runs the shipped `clipboard::terminal_action` on it. QEMU cannot press a key; the ring is the proof.
# PINNED VERBATIM, each because it would read well if the thing under it were broken: `copy_sel=ok`
# (the clipboard, read back through the epoch gate, holds exactly the two selected cells and not the
# line — THE field the go-red moves), `copy_line=ok` (with nothing selected ⌘C still copies the whole
# line: APPCLIP's behaviour preserved), `cut=ok` (clipboard AND line AND selection all checked after
# ⌘X) and `cut_empty=ok` (⌘X with nothing selected declined, line untouched). `resolved=` and
# `delivered=` are pinned as the same literal because they are CONSTANTS of the fixture: thirteen
# chords go in, and any other number means the table or the ring lost one.
# GO-RED (measured, M2): make `terminal_action`'s `Copy` arm ignore the selection -> `copy_sel=no …
# -> FAIL ::`.
REQUIRE :: TERMSEL: resolved=13/13 delivered=13 left=ok copy_sel=ok home=ok esc=ok copy_line=ok cut=ok cut_empty=ok edit=ok pc=ok -> PASS ::
FORBID :: TERMSEL: .* -> FAIL ::
