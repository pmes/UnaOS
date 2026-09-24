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
REQUIRE :: LOGIN: users\+session create=.* verify=ok wrong=refused login=ok principal=user:una#\d+ .* -> PASS ::
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

# ── 7c. THE IDENTITY IS NEVER REISSUED (SECLOGIN M2, rmbp-ledger B169) ──────────────────────────
# B157 gap 3: x86 compared a RECYCLABLE users-table id, aarch64 compared the NAME — two arches, two
# rules, and a recreated user could inherit. Now both compare the `uid` (x86 in its u32 tables,
# aarch64 inside `user:<name>#<uid>`), allocated from a counter that never decreases. The leg creates
# A, deletes A, creates B INTO A's FREED SLOT (`slot_reused=true` is measured, not assumed), recreates
# A, and asks the real ACL about B and the second A against a row A's first uid owns: both refused
# with `reason=recycled-id`, the owner admitted. GO-RED: the allocator mutated to v1's `slot + 1` →
# `same_slot_refused=false -> FAIL` on both arches from one mutation.
REQUIRE :: LOGIN-IDENT: a_uid=\d+ b_uid=\d+ a2_uid=\d+ slot_reused=true owner_ok=true same_slot_refused=true same_name_refused=true reason=recycled-id -> PASS ::
FORBID :: LOGIN-IDENT: .* -> FAIL
REQUIRE \[users\] delete user=identa uid=\d+ \(slot \d+ freed; uid never reissued
# The principal string a session prints is the canonical one, uid included.
REQUIRE \[users\] login ok user=una id=\d+ principal=user:una#\d+

# ── 7d. LOG OUT ENDS THE SESSION (SECLOGIN M3, rmbp-ledger B169) ────────────────────────────────
# B157 gap 4: Log Out killed STAMPS, not processes — the session's programs kept running and kept
# their windows, so the next person's login screen came up over the last person's desktop. Now
# `session_logout` walks the process table for every running row stamped in the closing epoch,
# closes its windows and kills it through the close box's own path, THEN bumps the epoch. The leg
# launches STAT.ELF under a session (the desktop's own launcher), logs out, and proves the pid gone
# and the window gone. A SKIP is a medium with no STAT.ELF — not this lane's (WINX-2 loads it off
# the same volume every boot), so it is FORBIDden here.
REQUIRE :: LOGIN-END: pid=\d+ stamp=stamped windows_before=\d+ ended=1 windows=\d+ pid_gone=true window_gone=true -> PASS ::
FORBID :: LOGIN-END: .* -> FAIL
FORBID :: LOGIN-END: .* SKIPPED
REQUIRE \[users\] logout epoch=\d+ ended=\d+ windows=\d+

# ── 7d. THE CREDENTIAL FILE IS KERNEL-OWNED (SECLOGIN M4 + VFSOWNED, rmbp-ledger B169/B181) ─────
# B157 gap 2. The predicate is pinned as a property. The x86 RESOLVER line USED to be pinned as a
# MEASUREMENT with both readings allowed, because the guard line lived in `fs::vfs::el0_locate`,
# outside the SECLOGIN grant (multiuser.md §6). VFSOWNED (B181) landed that line, so the pin is now
# the PROPERTY and only the property: the resolver REFUSES. `resolver=refused` is the fixture's own
# spelling (`fs/users.rs` login_kown_fixture, lower case) — not `REFUSED`; the wire is the authority.
# The FORBID closes the other arm, which is the reading this pin exists to make unshippable: a REQUIRE
# that still admitted `OPENED` would certify the hole (LAWS §5: never require a limitation), and a
# REQUIRE alone cannot catch the family taking the wrong arm (pi4-regression.spec:2032/2052's shape).
REQUIRE \[users\] kernel-owned pred=ok resolver=refused
FORBID \[users\] kernel-owned .*resolver=OPENED
FORBID :: LOGIN-KOWN: .* -> FAIL

# ── 7e. THE SALT HAS A SOURCE, AND THE EPOCH IS 64 BITS (SECLOGIN M5, rmbp-ledger B169) ─────────
# B157 gaps 5 and 6. `source=` is pinned as the three names the module can say; on THIS lane
# (`-cpu qemu64,+x2apic`, no RDRAND) it reads `jitter` and the probe says `cpuid.01h.ecx.30=0`; under
# the builder's `UNAOS_CPU=qemu64,+x2apic,+rdrand` it reads `rdrand` — the flip, with no new knob.
# GO-RED: `rand::jitter_fill` mutated to a constant → `distinct=false -> FAIL`.
REQUIRE \[rand\] source=(rdrand|rndr|jitter) probe=\S+ bits=256
REQUIRE :: LOGIN-RAND: source=(rdrand|rndr|jitter) distinct=true nonzero=true same_source=true salts_differ=true draws=\d+ epoch_bits=64 -> PASS ::
FORBID :: LOGIN-RAND: .* -> FAIL

# ── 7f. THE HOME NAMES ITS VOLUME BY SERIAL (SECLOGIN M6, rmbp-ledger B169) ─────────────────────
# B157 gap 8: `volume=el0-fat` named the ROLE. Now the FAT volume serial (BS_VolID, eight hex
# digits), so a flight-12 capture tells the card from a stick. The roster policy (gap 7) is a
# predicate in `video/login.rs` with the default unchanged, so `LOGIN-CONTROL`'s `rows=` is unchanged.
REQUIRE \[users\] home=/home/una (created|exists) volume=[0-9a-f]{8}

# ── 8. ABSENCE — the hole every block above exists to close ─────────────────────────────────────
# A fixture that stops running prints nothing and passes silently. Every REQUIRE above gates PRESENCE
# as well as health for its own leg, which covers it. What is NOT covered by any of them is the boot
# reaching the storage pass at all: `users::service` is where the whole battery is chained from, and
# until the root volume answers it prints nothing and returns. This line is that pass's own witness.
# ⚠ SPECPINS2 (2026-09-23, rmbp-ledger B184): ON x86 THIS PIN IS FIXTURE-PROOF, NEVER THE BOOT'S LINE.
# The only emitter is `users::screen_open_at_ignition` (`fs/users.rs:1089`), and its only callers are
# the Tegra desk cascade (`main.rs:9144`, inside `tegra_desk_cascade`, cfg aarch64 + `deskcascade`)
# and the login fixture's IGNITION leg (`video/login.rs:852` for HELD, `:856` for OPEN, inside
# `ignition_leg`, cfg `loginst`). The x86 boot opened the screen through `screen_open_once` at
# `main.rs:6380` (`x86_render_service`), which prints no `[login] ignition` line — and SINCE LOGIN13 M1
# (R63, B189) it opens nothing: that site calls `users::boot_session`, whose line §10 pins. So on this lane both
# matching lines are the fixture's two arms — measured at lines 1260 (HELD) and 1261 (OPEN) of
# `~/unaos-bench/scratch/rmbp-0915/specpins2-logs/run3-login-serial.log`, directly above
# `:: LOGIN-IGNITION:` at 1263 — and a green here says the storage pass reached the battery, which is
# what the paragraph above claims, and NOTHING about how the x86 boot's own screen came up. Kept, not
# removed: it is the storage-pass witness. Do not read it as the ignition's.
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

# ── 9. SPECPINS2 (2026-09-23, rmbp-ledger B184), TAIL-APPENDED past the contract block ──────────
# THE WINDOWED OPEN. `[login] screen open window=<n> box=<w>x<h> at (<x>,<y>)` (`video/login.rs:501`)
# is the one line that says the screen got a REAL `wm` row, and no spec read it. §7 pins what a person
# does at the screen; this pins that there was a screen to do it at.
#
# RE-READ BY LOGIN13 M1 (R63, rmbp-ledger B189): THE BOOT'S OWN OPEN NO LONGER EXISTS ON ANY x86 IMAGE —
# `main.rs:6380` now calls `users::boot_session(desktop)`, which opens nothing (§10). The paragraph below
# is the pre-R63 measurement; its conclusion (every windowed line on this lane is the loginst battery's)
# holds a fortiori, and on the METAL the first windowed line is now the root session's Log Out.
# WHO PRINTS IT ON THIS LANE, measured, because it is NOT the boot. The x86 boot's own open WAS
# `screen_open_once` at `main.rs:6380`, gated `if desktop` where `desktop = desktop_owns_backdrop()` =
# `desktop_uefi::is_active()` — and `desktop_uefi::activate`'s only caller is the Kepler takeover
# (`drivers/gpu/kepler_display.rs:511`). QEMU has no Kepler, so that call never runs here. Every
# windowed line on this lane is `login::open` reached from the loginst battery: the first
# (`window=2`, line 1213 of `~/unaos-bench/scratch/rmbp-0915/specpins2-logs/run3-login-serial.log`)
# follows `[login] logged out — screen returns` at 1203, i.e. `reopen_after_logout`
# (`video/login.rs:525`), inside the PRESS leg (its verdict at 1259); the rest are the CLOSE leg's
# (1269, 1289) and the CONTROL leg's (1296, 1317; verdict at 1321). Same function and the same format
# string as the boot's open, so the WORDING is pinned here; the boot's own line is metal-only and is
# scored from the metal capture, the same limit §8's ignition pin carries.
#
# NO NUMERICS: the id and the geometry depend on the window table and the panel (this file's header).
REQUIRE \[login\] screen open window=\d+ box=\d+x\d+ at \(\d+,\d+\) modal=true
# The two declines that are HONEST elsewhere and not here: `no surface yet` (`wm::spawn_geometry`
# answered nothing) and `create refused` (`wm::create_at` returned `WIN_NONE`). This lane has a panel
# and a window table, so either one is the screen falling back to a headless form on a machine that
# can draw it. `window=no (fixture — headless form)` is NOT forbidden: the fixtures ask for it.
FORBID \[login\] screen open window=no \(no surface yet
FORBID \[login\] screen open window=no \(create refused

# ── 10. R63 — THE BOOT IS ROOT, NOT A LOGIN SCREEN (LOGIN13, rmbp-ledger B189), TAIL-APPENDED ─────
# Peter, flight 12 (RULINGS R63): *"for boot 13 lets boot into root like we have been i will add my user
# and log out then log into the user account"*. Flight 12's screen opened at boot as a WINDOW over a live
# desktop (`[login] screen open window=2 box=1330x764 at (775,345)`) and never had the keyboard.
#
# M1 — THE BOOT'S OWN LINE. Printed by `main.rs`'s x86 site (`x86_render_service`, the one that used to
# open the screen) through `users::boot_session`, and by NOTHING ELSE — the fixture below drives the
# decision (`users::boot_ignition`) without printing it, so this REQUIRE is the BOOT's, not the fixture's
# (the trap SPECPINS2 measured in §8/§9). On QEMU it reads `desktop=false` (no Kepler takeover); on the
# metal `desktop=true`. GREEN CERTIFIES: the boot reached the render service's ignition site and the
# screen was DOWN when it left it. The FORBID is the defect's own reading.
REQUIRE \[login\] boot session=root desktop=(true|false) screen=closed
FORBID \[login\] boot session=\S+ desktop=\S+ screen=open
# The fixture: `root_at_boot` (no user session, root not closed, at the head of the loginst battery),
# the decision driven with `desktop=true` — the metal's arm, which no QEMU boot presents — and
# `desktop=false`, the screen down after each. GO-RED (LOGIN13 M1, run on this gate): the pre-R63
# statement put back inside `boot_ignition` (`if desktop { screen_open_once(); }`) reads
# `desk_screen=open … -> FAIL —`. GREEN CERTIFIES: the seam that replaced the boot's open cannot open
# the screen on a desktop boot, and `screen_built=true` says the screen was compiled so the claim bites.
REQUIRE :: LOGIN-BOOTROOT: session=root\(uid0\) root_at_boot=true desk_screen=closed nodesk_screen=closed still_root=true screen_built=true -> PASS ::
FORBID :: LOGIN-BOOTROOT: .* -> FAIL
#
# LOGIN14 (R65, rmbp-ledger B198) — ROOT'S PASSWORD IS CHOSEN ON THE GLASS AT THE FIRST BOOT-TO-ROOT. At the
# store's load `users::root_credential_ignition` makes root's row with NO credential (`KDF_UNSET`) and opens
# the set-password form of the login screen (or defers it to the desktop ignition when the store loads
# first). The fixture drives BOTH orders every run: with the desktop marked not-up the ignition must ARM
# and open nothing, and the desktop step must then open it (`deferred_leg=armed-then-opened` — R48's
# question about the rMBP's order answered at runtime, not by which order QEMU happened to take). It runs FIRST in the loginst chain (every later
# login fixture assumes the screen is down): it puts root's row back to unset (a previous run of this image
# set it), drives the REAL ignition, types the password, Tab, a DIFFERENT retype, Enter through the LIVE
# x86 key router (`wc_route_event`) — the form must stay and the row stay unset — then the matching pair,
# after which the row verifies, the wrong word does not, the screen is down and the session is still root's.
# GO-RED (run on this gate): `set_first_password` mutated to skip the write reads `set=false verify=FAIL`.
# GREEN CERTIFIES: the boot-13 alert exists, takes the keyboard, refuses a mismatch, writes exactly what was
# typed twice, and gives the root desktop back.
REQUIRE :: LOGIN-ROOTPW: reset=true deferred_leg=armed-then-opened opened=true keys_routed=true mismatch_kept=true set=true verify=ok wrong=refused screen=closed root_after=true -> PASS ::
FORBID :: LOGIN-ROOTPW: .* -> FAIL
REQUIRE \[login\] set-password screen open user=root login_after=false in_place=false
REQUIRE \[login\] root password unset row=present -> set-password screen deferred to the desktop ignition
REQUIRE \[login\] root password unset \(store loaded before the desktop\) -> set-password screen now
REQUIRE \[login\] set-password user=root retype mismatch
REQUIRE \[users\] password set user=root first=true
REQUIRE \[login\] set-password screen closed user=root
# A root row exists from this gate on; the screen must not log root in yet (root is reached by booting, R63).
FORBID \[login\] session open user=root
# The property the form exists for: nothing typed at it reaches the wire. `root13-pw` is root's credential
# (typed twice), `other-pw` the mismatched retype.
FORBID root13-pw
FORBID other-pw
#
# M2 (LOGIN13) as amended by LOGIN14 (R65) — `adduser <name>`: ROOT ADDS A USER WITH NO PASSWORD; THE PERSON
# CHOOSES IT AT THEIR FIRST LOGIN, on the same set-password form. The shell's prompt (`users::prompt_key`,
# the function `main.rs::handle_key` offers every key to first) is now `passwd [<name>]`'s: the fixture
# drives the REAL `adduser` (no prompt; the row is unset and verifies NOTHING), then the REAL `passwd boot13`
# from root, the password twice, nothing echoed, and the four refusals. GO-RED (LOGIN13 M2, still valid):
# the root check in `adduser_begin` inverted reads `created=unset:false … -> FAIL —`. GREEN CERTIFIES: root
# can add a user with no credential touching the line, an unset row is not a passwordless login, root can
# set a password from the shell without it touching the line editor (`echo=none`), the typed credential is
# the stored one (`verify=ok`), and the refusals each speak their own word and change nothing.
REQUIRE :: LOGIN-ADDUSER: root=true created=unset:true prompted_at_adduser=false unset_verify=refused passwd_prompted=true echo=none uid=\d+ set=true verify=ok dup=exists empty=empty-password mismatch=mismatch on_line=password-on-line passwd_on_line=password-on-line -> PASS ::
FORBID :: LOGIN-ADDUSER: .* -> FAIL
# The success lines (the store's uid, the home's own verdict, the password's own line) and the refusals.
REQUIRE \[users\] adduser user=boot13 id=\d+ home=/home/boot13 created=(true|false) password=unset
REQUIRE \[users\] home=/home/boot13 (created|exists) volume=[0-9a-f]{8}
REQUIRE \[users\] password set user=boot13 first=true
REQUIRE \[users\] adduser REFUSED user=boot13 reason=exists
REQUIRE \[users\] passwd REFUSED user=boot13 reason=empty-password
REQUIRE \[users\] passwd REFUSED user=boot13 reason=mismatch
REQUIRE \[users\] adduser REFUSED user=boot13p reason=password-on-line
REQUIRE \[users\] passwd REFUSED user=boot13 reason=password-on-line
# THE PROPERTY THE PROMPT EXISTS FOR, as a rule: nothing the fixture typed at the prompt reaches the wire.
# `boot13-pw` is the credential (typed twice), `one-pw`/`two-pw` the mismatched pair.
FORBID boot13-pw
FORBID (one|two)-pw
#
# M3 — LOG OUT CLOSES THE ROOT SESSION, AND THE SCREEN IS THE ONLY THING TAKING INPUT. The fixture runs third,
# while root is still the session: the shell's `logout` from root with an EMPTY store is refused; a program
# launched in the root session (`STAT.ELF`, uid 0 in the root epoch) is ENDED by the root session's Log Out
# (the shell's `logout` -> `users::log_out_to_screen` -> `login::reopen_after_logout`, the crystal row's own
# action); the screen comes up on a real row; `adduser` is then refused `not-root`; and every key is driven
# through the LIVE x86 key router (`wc_route_event`) — the path flight 12's keys took to
# `[wc-c] focus tab-cycle` — and must be consumed there and land in the form: the name, Tab to the password
# field, a wrong password (the one-answer denial), the right one (a session as `boot13`, home
# `/home/boot13`). GO-RED (LOGIN13 M3, run on this gate): the screen-first fold deleted from `wc_route_event`
# (`arch/x86_64/syscall.rs:7312`) reads `keys_routed=false name_typed=false … -> FAIL —`. GREEN CERTIFIES:
# root's Log Out is refused with nobody to log in as, ends root's programs when it is not, returns the
# screen, and the screen — not the focus ring, not a focused app — receives the keyboard.
REQUIRE :: LOGIN-ROOTOUT: root_before=true empty_refused=no-users pid=\d+ root_stamped=true others=\d+ root_after=false pid_gone=true window_gone=true screen=up screen_window=true not_root=not-root keys_routed=true name_typed=true tab=password wrong=denied login=boot13 cleaned=true -> PASS ::
FORBID :: LOGIN-ROOTOUT: .* -> FAIL
REQUIRE \[users\] logout REFUSED session=root reason=no-users
REQUIRE \[users\] root session closed ended=\d+ windows=\d+
REQUIRE \[users\] session-end pid=\d+ slot=\d+ user=0 windows=\d+ kill=
REQUIRE \[users\] adduser REFUSED user=boot13x reason=not-root
# The one-per-open key witness: what a flight-13 capture reads to know the keyboard reached the screen,
# without a typed byte on the wire (the §7 denial FORBIDs still hold for this user's denial below).
REQUIRE \[login\] key taken by the screen
REQUIRE \[login\] denied user=boot13
REQUIRE \[users\] login ok user=boot13 id=\d+ principal=user:boot13#\d+
REQUIRE \[login\] session open user=boot13
FORBID wrong-pw
#
# M4 — THE x86 CREDENTIAL-FILE REFUSAL IS TYPED AND HAS A VERDICT (VFSOWNED's owed items, B181 -> B189).
# `El0LocateError::KernelOwned` is what `fs::vfs::el0_locate` returns for `USERS.DAT`/`USERS.NEW`; the x86
# fixture asks the resolver for both and passes only on that variant — a refusal for any other reason is
# not this one. §7d's `[users] kernel-owned pred=ok resolver=refused` token is unchanged (its parenthetical
# no longer claims the guard "is owed to the seat": VFSOWNED landed it). GO-RED (LOGIN13 M4, run on this
# gate): the guard returning `Invalid` again reads `err=OTHER,OTHER -> FAIL —`. GREEN CERTIFIES: x86 has a
# verdict for SECLOGIN M4 (aarch64 has had one since ARMUSERS), and the refusal is the typed one.
REQUIRE :: LOGIN-KOWN: pred=ok resolver=refused err=KernelOwned,KernelOwned reason=kernel-owned -> PASS ::
# ── LOGIN15 (rmbp-ledger B213, flight 13 §1) — THE GUARD THAT HELD THIS WHOLE FILE'S CHAIN SHUT ON THE METAL.
# `users::service()` returned at `block::info().is_none()` on every rMBP pass (nothing sets the global slot
# there), so none of the fixtures above ever ran on flights 12 and 13 while this lane read 53/53: the guard
# measured the QEMU disk's shape. Readiness is now "any registry holds a disk"; the fixture below is the
# predicate on the rMBP shape, the QEMU shape and none, with the old guard's rMBP answer as the go-red.
REQUIRE :: USERSREADY: rmbp-shape\(global=0 sdhc=1 ahci=1\)=1 .* none=0 old-guard-on-rmbp=0 .* -> PASS ::
FORBID :: USERSREADY: .* -> FAIL
REQUIRE :: USERSMOUNT: rmbp-shape=sdhc qemu-shape=global none=none old-mount-on-rmbp=none this-boot via=(global|sdhc) -> PASS ::
FORBID :: USERSMOUNT: .* -> FAIL
REQUIRE \[users\] load volume=el0-fat\(rw\) via=(global|sdhc) 
