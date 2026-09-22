# x86-splash.spec — SPLASHGATE (rmbp-ledger B149): the boot splash, ON A WIRE, for the first time in
# this repo's history.
#
#   QEMU gate:  ./arroyo test-splash 120    -> target/serial.log
#               (the verb re-execs with UNAOS_SPLASHLANE=1 UNAOS_WC=1 UNAOS_QEMU_FULL=1 and then
#                `x86_spec_replay` replays this file; `x86_test_completion` reads ITS OWN COMPLETE
#                marker out of this file too — see the `X86_TEST_SPEC` override in `arroyo`.)
#
# WHAT THIS FILE IS FOR, and it is not a new fixture. CRYSTALBOOT (fold b9e673d3, rmbp-ledger B137)
# fixed the broken crystal — `splash::advance` laying facet edges onto the LIVE desktop one frame
# after the Kepler takeover's full-panel clear — and then could not test it. `./arroyo test`
# FORCE-ARMS the kernel's `witness` feature (`arroyo`, the `case` at the head of the file:
# `export UNAOS_WITNESS="${UNAOS_WITNESS:-1}"`, which no caller can turn off from outside because
# `:-` substitutes on an EMPTY value too), and `main.rs:225-234` compiles the `boot_splash` CALL out
# under `any(usbdebug, bootlog, witness)`. So:
#
#   * no battery run in this repo's history has ever had a splash on its panel;
#   * the flight-11 image carried `witness` AND `usbdebug` for the fixtures and had none either;
#   * and CRYSTALBOOT's own mutation test — re-introduce the paint, watch a gate red — HAD NO GATE.
#
# LAWS §5: an instrument's presence is proven in the artifact, and a check that cannot fire is an
# absent one. This file, and the `test-splash` verb that replays it, are that gate.
#
# SCOPE — a `./arroyo test-splash` boot and nothing else, and the scope is a statement about the
# BUILD rather than about the fixture. This is the SHIPPED POLARITY: witness, usbdebug and bootlog
# all OFF, `wc` ON. LAWS §5 says a default-quiet knob has two polarities and the gate must compile
# the one that SHIPS; `witness` is the battery's polarity and is not the operator's, and the image a
# person boots off a card is the only image that paints a crystal at all. Every existing x86 QEMU
# leg compiles that image away. This one boots it.
#
#   WHICH KNOBS THE LANE ARMS, and why each is load-bearing rather than defensive:
#     UNAOS_WC=1          `splash::retire`, both of its call sites and `fbcon::panel_console_live`
#                         are ALL `#[cfg(feature = "wc")]` — b9e673d3 gated them for byte-identity
#                         (`./arroyo knoboff bt` must not convict a splash change of being a
#                         Bluetooth change). Without the knob the fold compiles to nothing, the
#                         retirement witness does not exist, and the REQUIRE below is red.
#     UNAOS_QEMU_FULL=1   the three-valued run mode: `mode=full` on the sidecar is the only reading
#                         that certifies the tail was not truncated. docs/dev/QUEUE.md §5,
#                         2026-09-17: "a truncated run must never be a pass."
#     UNAOS_WITNESS       NOT SET, and its absence is the verb. `test-splash` is the one x86 verb
#                         the head-of-file `case` in `arroyo` does not name, deliberately.
#
#   `x86_pick_capture_spec` refuses to replay this file on any other polarity BY NAME — a capture
#   from a witness/usbdebug/bootlog build is a splash-less kernel, and scoring these pins against it
#   would be calling a splash-less image a splash pass.
#
# THE ARTIFACT IS CERTIFIED BEFORE ANY LINE HERE IS SCORED. `splash_artifact_cert` (arroyo) counts
# both strings below in the ELF THE BUILDER LEFT BEHIND — the binary that actually booted, not the
# pre-builder one — with a present-control and an absent-control, and demands exactly one hit each.
# That is the half a replay cannot do: it separates "the witness did not FIRE" from "the witness was
# never BUILT", which are different defects with different fixes. Measured on this file's own gate:
# `LC_ALL=C grep -a -o -F ':: SPLASH: crystal cluster traced'` = 1 and `':: SPLASH: retired at '` = 1.
#
# WHAT THIS GATE DOES NOT PROVE, said plainly rather than left for a reader to assume. The fold's
# SEAM arm — `advance` asking `fbcon::panel_console_live()` before any pixel moves — is METAL-ONLY
# and is dead on this gate by construction: `PANEL_CONSOLE` has exactly one writer,
# `fbcon::panel_console_resume`, which is the Kepler takeover's, and QEMU has no Kepler (measured:
# zero `panel_console_resume` lines, and the retirement below is stamped by the `gui` BACKSTOP on
# every capture of this lane). So this file gates the RETIREMENT — that the splash is handed over,
# once, at a named site, on the build a person boots — which is exactly what makes the seam's own
# mutation test runnable on metal at all. The seam belongs to x86-witness.spec and the bench.
#
# NO NUMERICS are pinned that the kernel does not itself compute: the ray count, the millisecond
# stamp and the ledger's counters are all `\d+`. What is literal is the SITE STRING, because the
# site is the claim (b9e673d3: "`<site>` is load-bearing and is the whole reason this is a function
# rather than two stores").

# --- THE SPLASH WAS PAINTED. `boot_splash` ran, which on this build means the `boot_splash` CALL was
# --- compiled in, which means none of witness/usbdebug/bootlog was armed — the polarity assertion,
# --- made by the kernel rather than by the shell. `3 shards` is literal (it is the cluster, not a
# --- count that drifts); `\d+ spectrum rays` is `NRAYS` and is not this file's business.
REQUIRE :: SPLASH: crystal cluster traced — 3 shards, \d+ spectrum rays ::

# --- AND IT WAS HANDED OVER, at a NAMED SITE. This is the line CRYSTALBOOT added and could not
# --- score, and it is the one that reds under the mutation this gate exists for: re-introduce the
# --- paint (revert b9e673d3's splash.rs folds) and `retire` is never called, so the wire falls
# --- silent here while everything else about the boot is unchanged. MEASURED, both directions, on
# --- this lane's own captures — green `:: SPLASH: retired at 2842 ms by bootpace gui stamp (main.rs,
# --- before the desktop's first paint) ::` at serial.log:295; red 0 hits, MBENCH FAIL, rc=1.
# ---
# --- THE SITE IS PINNED VERBATIM and that is the point of the rule rather than a strictness. A
# --- prefix match on `:: SPLASH: retired at \d+ ms` would be satisfied by EITHER retirement site,
# --- and the two mean opposite things: the `gui` stamp is the backstop (the splash was still up
# --- when the GUI took the panel), the `panel_console_resume` site is the seam (something else had
# --- already cleared the glass). A gate that cannot tell them apart cannot tell a fixed tree from a
# --- broken one on metal. The escapes are deliberate rather than decorative: `(`, `)` and `.` are
# --- regex metacharacters, and an unescaped `main.rs` would also match `mainXrs`.
REQUIRE :: SPLASH: retired at \d+ ms by bootpace gui stamp \(main\.rs, before the desktop's first paint\) ::

# --- THE WRONG SITE, FORBIDDEN BY NAME. On this gate the seam site is unreachable by construction
# --- (no Kepler => `PANEL_CONSOLE` is never set => `panel_console_live()` is never true), so this
# --- rule cannot fire on a healthy run — the same standing as x86-wc.spec's SKIP forbids, and for
# --- the same reason: it is what keeps the REQUIRE above a claim about WHICH site retired the
# --- splash rather than a prefix that happens to match. If it ever fires, either the fixture grew a
# --- display takeover or the seam predicate started answering true where it must not, and both are
# --- things to look at rather than to pass.
FORBID :: SPLASH: retired at \d+ ms by video/fbcon\.rs panel_console_resume

# --- A RETIREMENT STAMPED AT THE CLOCK'S ORIGIN IS A WITNESS LYING ABOUT ITS OWN TIME. `arch::ms()`
# --- returns 0 until `apic::calibrate` has run (`bootpace::origin_hz()` is 0 before it — main.rs:828
# --- states the same fact for BOOTCLOCK), and the `gui` stamp is ~3 s of boot later: this lane
# --- measures 2842 ms. A `0 ms` retirement therefore means the call moved somewhere it must not be —
# --- ahead of calibration, or into `boot_splash` itself, i.e. a splash retired before it was ever
# --- up. That is the one shape in which this witness can be PRESENT and still worthless, and a
# --- FORBID is the only directive that can say so (a REQUIRE on `\d+` is satisfied by zero).
FORBID :: SPLASH: retired at 0 ms by

# ── COMPLETE: the END-OF-RUN MARKER, and it is not an arbitrary last line ────────────────────────
# `bootpace::service_dump` re-prints the WHOLE ledger every time it grows and is DELIBERATELY
# UNGATED — no cargo feature, no env knob — because, in its own words, "a ledger that only exists in
# the builds nobody boots on hardware is not an instrument". That makes it the one terminal line
# this polarity is guaranteed to have: every other `::` battery line on an x86 boot is `witness`
# territory and absent here by construction, which is precisely why this lane cannot borrow
# x86-test.spec's zeolite marker (see the `X86_TEST_SPEC` override in `arroyo`).
#
# `gui=\d+ms` IS THE LOAD-BEARING FIELD and not decoration. The ledger prints `gui=-` until the
# `gui` phase has been recorded, and the `gui` stamp is the very call that retires the splash
# (`bootpace::record` -> `splash::advance("gui")` -> `retire`). So a capture that matches this line
# has NECESSARILY got past the point where both REQUIREs above are printed, and a capture that did
# not reach the GUI handoff is TRUNCATED rather than FAIL — which is the honest verdict, because the
# splash cannot have been retired by a stamp that never landed. Measured on this lane: four LEDGER
# emissions, the first at serial.log:356 (n=26) and the last at :629 (n=34), all `gui=3058ms`,
# against the retirement at :295.
#
# Deliberately pinned no tighter than the fields it needs: `ftdi=` is `none` under QEMU and a
# hostname-ish token on the bench, and `n=`/`hz=` are facts about the host.
COMPLETE :: BPACE: total gui=\d+ms .* result=LEDGER ::

# ── CONTRACT (SPECRUN, 2026-09-15) ──────────────────────────────────────────────────────────────
# A PINNED LINE IN THIS FILE IS CHANGED TOGETHER WITH THE KERNEL LINE IT PINS, IN THE SAME COMMIT —
# re-pinned to the new wording (naming the arc that changed it), or dropped with the reason stated.
# It is never worked around by teaching the kernel a SECOND spelling of the same witness.
#
# WHO RUNS THIS FILE. `./arroyo test-splash [secs]` — a fold-gate command the seat runs, NOT a leg of
# `check`. There is NO `# RUN-BY:` header here, for x86-ptr.spec's reason exactly: `arroyo` names
# `specs/x86-splash.spec` in CODE, twice (`X86_SPLASH_SPEC` at the head of `x86_pick_capture_spec`'s
# block, and the `X86_TEST_SPEC` override that gives this lane its own end-of-run), so
# GATE-SPECROOTS resolves it GATED and a declaration would be the weaker claim of the two.
