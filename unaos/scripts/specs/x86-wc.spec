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
