# x86-login.spec — THE LOGIN FLOW ON THE x86 WC LADDER: the screen a person sits down at, the
# credential they type, and the way back out. LOGINFLOW (rmbp-ledger B157), 2026-09-22.
#
#   QEMU gate:  UNAOS_WC=1 UNAOS_QUARRY=1 UNAOS_FTDIRX=1 UNAOS_LOGIN=1 UNAOS_LOGINST=1 \
#               UNAOS_QEMU_FULL=1 ./arroyo test 240 -> target/serial.log
#               ./arroyo mbench --replay target/serial.log \
#                        --spec scripts/specs/x86-login.spec --platform x86
#
# ── WHY THIS FILE EXISTS AT ALL ──────────────────────────────────────────────────────────────────
# The LOGIN arc has shipped five fixtures across four commits — `LOGIN` (M1, users+session),
# `LOGIN-SCREEN` (M3/M4, the state machine), `LOGIN-IGNITION` (SO43), `LOGIN-PRESS` (SO36/SO44, the
# pointer barrier) and now `LOGIN-CONTROL` (SO44's second half) — and until this file NOT ONE of them
# was read by any directive. `grep -rn 'LOGIN' scripts/specs/` came back empty. That is precisely the
# hole `x86-wc.spec`'s PTRDEAD block was written to close and the one its SPECPINS block found
# reopened three times in a single day: a fixture whose verdict no spec reads is a fixture that can
# STOP RUNNING SILENTLY, and a login screen that stops being tested is the first thing anyone sees.
#
# ── WHY NOT IN x86-wc.spec ───────────────────────────────────────────────────────────────────────
# MEASURED, not preferred. Every line below needs `UNAOS_LOGIN=1` AND `UNAOS_LOGINST=1`, and
# `x86-wc.spec`'s RUN-BY line carries neither: pinning these there would red every existing run of
# that gate, which is the one thing a spec must never do to a gate that is working. Its SPECPINS
# block grew the run-by line for `UNAOS_QUARRY`/`UNAOS_FTDIRX` because those knobs were free on that
# lane; `loginst` is NOT free — it performs a BOOT-TIME WRITE of a KNOWN credential to the medium
# (`una` / `correct-horse`), which is exactly why it has its own knob and never rides `UNAOS_LOGIN`
# (the `hcronst`/`prtscrst` rule, `arroyo:604`). A separate file with its own RUN-BY is the honest
# shape, and GATE-SPECROOTS accepts a `knobleg` declaration for exactly this case.
#
# ── SCOPE ────────────────────────────────────────────────────────────────────────────────────────
# A `UNAOS_WC=1 UNAOS_LOGIN=1 UNAOS_LOGINST=1 ./arroyo test` boot and nothing else. Without `login`
# the screen does not compile and every line below is red; without `loginst` the fixtures do not
# compile and print nothing, which the ABSENCE rule below catches by name rather than passing.
# The METAL half — a person at the trackpad — is the bench's business and is flight 12's.
#
# NO NUMERICS are pinned that a fixture does not itself fold into PASS/FAIL. `rows=`, `answered=`,
# `swallowed=`, `logins=`, `heals=` and the geometry are `\d+`: they depend on the panel, on how many
# users the medium already carried, and on how many presses the leg needed. The BOOLEANS are literal,
# because they are the claim.

# ── 1. THE STORE AND THE SESSION (M1) ────────────────────────────────────────────────────────────
# The foundation every line below stands on: a user can be created, the credential verifies, a wrong
# one is refused, the session opens under `user:una`, the home exists and the ACL admits its owner.
# `linked=`/`home=`/`acl=`/`epoch=`/`users=`/`volume=` are reported by the fixture and folded into its
# own verdict, so they are matched loosely and the VERDICT is what gates.
REQUIRE :: LOGIN: users\+session create=.* verify=ok wrong=refused login=ok principal=user:una .* -> PASS ::
FORBID :: LOGIN: users\+session -> FAIL
# A SKIPPED here is a harness with no FAT volume. On THIS gate the x86 default medium carries a FAT32
# volume (DEFAULTMEDIUM), so a skip means the medium was lost — a red, not a shrug.
FORBID :: LOGIN: users\+session -> SKIPPED

# ── 2. THE IGNITION (SO43) ───────────────────────────────────────────────────────────────────────
# The screen comes up because the DESKTOP EXISTS, never because a console route was installed. Arm 2
# drives the ORIN's own tuple off render14's wire (`bar=1 … activate=false`) on this arch's ladder,
# which is what makes the rule arch-neutral by construction. GO-RED (LOGINBOOT, measured): put
# `console_routed` back into the rule inside `users::screen_open_at_ignition` (`if desktop_up` ->
# `if desktop_up && console_routed`) and this line reads `tegra_opened=false -> FAIL —`.
REQUIRE :: LOGIN-IGNITION: no_desktop_held=true tegra_opened=true .* -> PASS ::
FORBID :: LOGIN-IGNITION: .* -> FAIL

# ── 3. THE POINTER BARRIER (SO36 + SO44's first sentence) ────────────────────────────────────────
# SESSGATE's leg, pinned for the first time. It mints a REAL row in the app band, proves `hit_test`
# names it, proves `press_route` DECLINES that point with the screen DOWN — the control, without
# which a gate that consumed everything would be indistinguishable from this one — then proves it
# CONSUMES the same point and `(0,0)` with the screen UP while the row is still hit-testable.
# `screen_down_routed=false` is the term that BITES and it is pinned LITERALLY: it is the control,
# and a `true` there is a gate that has become a constant.
REQUIRE :: LOGIN-PRESS: win=\d+ at=\(\d+,\d+\) behind_named=true screen_down_routed=false screen_up=true up_at_window=true up_at_corner=true still_named=true logged_in=true after_login_routed=false -> PASS ::
FORBID :: LOGIN-PRESS: .* -> FAIL
# A SKIP is a panel below 256x256 or `wm::create` declining. On this gate (QEMU 1280x800, one 64x64
# fixture row) neither is honest — `x86-wc.spec`'s standing DMGOVLP/MENUDROP rule — so a SKIP means
# the fixture lost its panel or its window table.
FORBID :: LOGIN-PRESS: .* SKIP

# ── 4. THE PRESS IS THE SCREEN'S (SO44's SECOND sentence) — LOGINFLOW's own leg ──────────────────
# THE ONE LINE THIS FILE WAS WRITTEN FOR. Driven through the LIVE ROUTER (`wc_click_route_at`, the
# coordinate-taking entry MENUDROP's fixture also drives) and never through `press_swallow`, because
# a fixture that calls the predicate stays green on a tree whose router arm has been deleted — B121's
# lesson, one band over, and the reason `route=` is pinned LITERALLY rather than as `\w+`: a leg that
# quietly fell back to a lower seam would be asserting about a frame no person's finger reaches.
#
# The terms, and why each is literal:
#   round_trip        the leg's own surface<->panel arithmetic, checked before it is trusted. False
#                     here means every press point below landed somewhere other than where it says.
#   outside_consumed  a press at (0,0) — the FITTS corner the crystal claims — is still CONSUMED
#                     (SO36: no furniture answers before a session exists).
#   outside_quiet     …and answers NO control. THIS IS THE CONTROL TERM. A `ctl_at` that said yes to
#                     every point would pass every other term on this line and fail only this one.
#   pw_focus/nm_focus the caret that did not move before this arc. These are what a person touches.
#   row_picked        a press on user row 0 picks row 0's name OUT OF THE STORE (`users::name_at`),
#                     asserted against the store and not against a name the fixture chose, so the leg
#                     cannot pass by agreeing with itself.
#   button_press      the credential is submitted WITH THE POINTER, never with Enter — Enter is
#                     `consume_key`'s path and `LOGIN-SCREEN` already proves it.
#   opened            a session under the typed name.
#   logout/screen_back/back_press/back_focus   the way OUT, and the screen owning the pointer again on
#                     the other side of it: the round trip a second person sitting down depends on.
#
# GO-RED — RUN, not reasoned, on this gate's own line (2026-09-22, LOGINFLOW). Delete the
# `#[cfg(feature = "login")] if crate::fs::users::screen_press(x, y) { … return true; }` statement
# from `wc_click_route_at` (`arch/x86_64/syscall.rs:7452`) — SO44's seam re-opened, line count
# unchanged — and the same command reads rc=1 with:
#
#   :: LOGIN-CONTROL: route=wc_click_route_at win=1 round_trip=true outside_consumed=false
#   outside_quiet=true pw_press=false pw_focus=false nm_press=false nm_focus=true rows=1
#   row_picked=false button_press=false opened=false logout=true screen_back=true back_press=false
#   back_focus=false answered=0 swallowed=2 -> FAIL —
#
# and `:: LOGIN-SCREEN: … control=false -> FAIL —` beside it. `answered=0` is the whole sentence:
# with the router's arm gone `press_swallow` is never called, so the screen hears NOTHING — no field
# focuses, no row is picked, the button does not fire and no session opens. `nm_focus=true` survives
# only because the form's focus starts on Name, which is worth knowing about that term.
#
# ⚠ AND `:: LOGIN-PRESS:` STAYED GREEN THROUGH IT (`… up_at_window=true up_at_corner=true … -> PASS
# ::`). That is not a weakness in either leg, it is the proof they are INDEPENDENT: §3's leg drives
# `strip::press_route`, whose gate this go-red did not touch, and §4's drives the x86 router, whose
# gate it did. A tree that loses one arm reds exactly the leg that asks about that arm — which is the
# discipline SESSGATE's two go-reds established and the reason both legs are pinned here rather than
# one standing in for the other.
REQUIRE :: LOGIN-CONTROL: route=wc_click_route_at win=\d+ round_trip=true outside_consumed=true outside_quiet=true pw_press=true pw_focus=true nm_press=true nm_focus=true rows=\d+ row_picked=true button_press=true opened=true logout=true screen_back=true back_press=true back_focus=true answered=\d+ swallowed=\d+ -> PASS ::
FORBID :: LOGIN-CONTROL: .* -> FAIL
# A SKIP is `wm` naming no surface. That is the aarch64 `virt` leg's honest reading and it is NOT
# honest here — this gate has a panel and the leg mints its own row — so a SKIP means the fixture lost
# the one thing its whole claim is about.
FORBID :: LOGIN-CONTROL: .* SKIP

# ── 5. THE STATE MACHINE AND THE CLOSE BOX (M3/M4 + LOGINCLOSE) ─────────────────────────────────
# `close_route=refused` is asked for by NAME rather than accepting the fixture's own
# `close_box_refused` boolean, which is also true for `no-window`: this gate HAS a row, so the strong
# reading is the only honest one here. `heals=` is `\d+` because the CLOSE leg drives `wm::close` at
# the row on purpose and the repair is the thing being measured; on a REAL boot a non-zero `heals=`
# is a FINDING (some route closed the screen's row) and belongs in the metal spec, not this one.
# `control=true` is LOGINFLOW's leg folded into this verdict, pinned here as well as on its own line
# so that deleting the leg reds this rule instead of silently narrowing what the run asserts.
REQUIRE :: LOGIN-SCREEN: window=no esc_kept=true wrong_kept=true opened=true passes_through=true logout=true back_after_logout=true second_login=true logins=\d+ close_box_refused=true close_route=refused reopened=true heals=\d+ ignition=true control=true -> PASS ::
FORBID :: LOGIN-SCREEN: .* -> FAIL

# ── 6. LOG OUT IS A ROW IN THE CRYSTAL MENU (M4) ────────────────────────────────────────────────
# ROW -> VERB -> ACTION, resolved through the menu's own pure `item_at` at the row's centre. `real=`
# is not on the line (`action=` carries it as the word `real`), and `sep_above=true` is the placement
# claim: the row sits at the foot of the tree behind its own separator. The leg runs TWICE on this
# gate — once from `LOGIN-CONTROL`'s way-out and once from `LOGIN-SCREEN`'s — and the FORBID below is
# what makes that safe: BOTH must pass, because one FAIL anywhere in the capture reds the rule.
REQUIRE :: LOGIN-LOGOUT: row=\d+/\d+ label=Log Out resolves=true action=real sep_above=true session_closed=true screen_up=true -> PASS ::
FORBID :: LOGIN-LOGOUT: .* -> FAIL

# ── 7. THE WIRE A PERSON'S OWN BOOT PRINTS ──────────────────────────────────────────────────────
# These four lines are NOT fixture verdicts: they are the instrument in the SHIPPED image (none is
# `witness`-gated), which is the whole point — they are what a flight-12 capture will be read with
# when Peter sits down. Pinning them here is what keeps them from being reworded out from under the
# bench, the SPECRUN contract's own case.
#
# THE DENIAL, and the property it is pinned FOR: `[login] denied user=<name>` and NOTHING ELSE. A
# wrong name and a wrong password are the SAME refusal — `submit` asks `users::verify` ("one answer
# for 'no such user' and 'wrong password'") rather than reading a `UsersError` — so a reason appearing
# on this line would tell someone at the keyboard which names exist on the machine. The FORBIDs below
# are that property as a rule: the three `users_reason` spellings must never reach this tag.
REQUIRE \[login\] denied user=una
# TWO FORBIDs, and the split is a MEASURED correction rather than a belt-and-braces. The first cut of
# this rule was one alternation over the phrases a leaked reason would use — and it matched the
# kernel line's own EXPLANATORY SUFFIX, which named the cases it refuses to distinguish, and reddened
# a green run (2026-09-22, this arc, before either was committed). Both sides were changed: the wire
# line no longer spells the cases out, and the rule no longer asks a regex to tell prose from payload.
#   (a) `reason=` is THE idiom this repo reports a refusal cause with — `users_reason` returns exactly
#       the words `users.rs` would put there, and `[users] home=… NOT created reason=…` already uses
#       the token two functions away. A `reason=` on this tag IS the defect, in any wording.
#   (b) the `UsersError` variant NAMES, which is what a `{:?}` of the error would emit and is the
#       other way the distinction leaks without anyone deciding to leak it. These are identifiers, not
#       prose: no honest sentence about a login denial contains `BadName` or `UsersError`.
FORBID \[login\] denied.*reason=
FORBID \[login\] denied.*(UsersError|BadName|NotFound|Refused|Exists|Volume|Full)
REQUIRE \[login\] session open user=una
REQUIRE \[login\] logged out — screen returns
# The press witness itself — the instrument SO44's second half is read with. A `control=` word that is
# not one of the five the screen knows means the wire and `ctl_name` have drifted apart.
REQUIRE \[login\] press at=\(\d+,\d+\) control=(name-field|password-field|button|user-row|none) answered=\d+ swallowed=\d+
# And the ROW probe, which must keep reading FALLS-THROUGH: `wm::hit_test` naming an `owner_asid == 0`
# row is the REJECTED alternative both SO36 and SO44 record — it hands the screen a close box back
# (LOGINCLOSE's measured defect) and gates only the points inside the rectangle. `MODAL` on this line
# would mean the belt had been cut, so it is FORBIDden by name rather than merely not required.
REQUIRE \[login\] press-probe win=\d+ centre=\(\d+,\d+\) hit=\d+ verdict=(FALLS-THROUGH|NOBODY)
FORBID \[login\] press-probe .* verdict=MODAL

# ── 7b. THE CREDENTIAL IS STRETCHED (SECLOGIN M1 / PWHARD, rmbp-ledger B169) ─────────────────────
# One SHA-256 per guess was the whole cost of a lost card (B157 gap 1). Now PBKDF2-HMAC-SHA256 at a
# count calibrated to ~250 ms on THIS CPU (`[users] kdf calibrated`, once per boot), stored per row,
# floor 10000 refused at parse AND at create. The leg writes a scratch v1 row, verifies it through
# the legacy path, logs in (which MIGRATES it — the `rehash` line), verifies again as v2, refuses a
# wrong password and an UNKNOWN name (which now costs the same time), and deletes the scratch user.
# `kat=ok` is the RFC 6070 known answers on SHA-256 — the implementation, not just the plumbing.
# GO-RED: `calibrated_iters` mutated to answer 1 → `[users] kdf REFUSED iters=1 floor=10000` and
# `:: LOGIN: users+session -> FAIL — create_user reason=weak-kdf`.
REQUIRE \[users\] kdf calibrated iters=\d+ ms=\d+
REQUIRE \[users\] rehash user=hard1 v1->v2 iters=\d+ ms=\d+
REQUIRE :: LOGIN-HARD: kat=ok iters=\d+ ms=\d+ v2_rows=\d+ migrated=1 legacy_verify=ok migrated_verify=ok wrong=refused unknown=refused floor=10000 -> PASS ::
FORBID :: LOGIN-HARD: .* -> FAIL
FORBID \[users\] kdf REFUSED

# ── 8. ABSENCE — the hole every block above exists to close ─────────────────────────────────────
# A fixture that stops running prints nothing and passes silently. Every REQUIRE above gates PRESENCE
# as well as health for its own leg, which covers it. What is NOT covered by any of them is the boot
# reaching the storage pass at all: `users::service` is where the whole battery is chained from, and
# until the root volume answers it prints nothing and returns. This line is that pass's own witness.
REQUIRE \[login\] ignition desktop_up=(true|false) console_routed=(true|false) -> (OPEN|HELD)

# ── CONTRACT (SPECRUN, 2026-09-15) ──────────────────────────────────────────────────────────────
# A PINNED LINE IN THIS FILE IS CHANGED TOGETHER WITH THE KERNEL LINE IT PINS, IN THE SAME COMMIT —
# re-pinned to the new wording (naming the arc that changed it), or dropped with the reason stated.
# It is never worked around by teaching the kernel a SECOND spelling of the same witness.
#
# WHO RUNS THIS FILE:
# RUN-BY: knobleg — UNAOS_WC=1 UNAOS_QUARRY=1 UNAOS_FTDIRX=1 UNAOS_LOGIN=1 UNAOS_LOGINST=1 UNAOS_QEMU_FULL=1 ./arroyo test 240, then
#          ./arroyo mbench --replay target/serial.log --spec scripts/specs/x86-login.spec --platform x86
#   EVERY KNOB IS LOAD-BEARING. `UNAOS_LOGIN=1` compiles the screen and the store (without it
#   `video/login.rs` is not declared at all — `video/crystal.rs`'s tail gates the module); without
#   `UNAOS_LOGINST=1` not one fixture in this file is compiled and the run prints none of these lines;
#   `UNAOS_WC=1` is the x86 gate on the screen itself (`fs::users::screen_key`/`screen_press` are
#   `false`/no-op without it) AND on the router arm this file's §4 drives; `UNAOS_QUARRY=1` and
#   `UNAOS_FTDIRX=1` are carried so this capture is ALSO scoreable against `x86-wc.spec` without a
#   second 240 s boot (that file FORBIDs the SKIP arms those two knobs prevent); and
#   `UNAOS_QEMU_FULL=1` is the three-valued run mode — `mode=full` on the sidecar is the only reading
#   that certifies the tail was not truncated, and a spec verdict on a short capture is the false
#   green this tree's QUEUE §5 named on 2026-09-17 ("a truncated run must never be a pass"). The 240 s
#   wall is that file's too: the login battery sits late in the storage pass, behind the FAT mount.
#   NOT on the default gate: `./arroyo test` replays `x86-default.spec` and arms none of these knobs.
#
# GATE-SPECROOTS (`scripts/spec-roots.sh`, a leg of `./arroyo check`) reds by name on any spec under
# scripts/specs/ that is neither named in `arroyo`'s CODE nor carries a RUN-BY line above.
