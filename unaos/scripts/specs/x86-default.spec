# x86-default.spec — the DEFAULT x86 medium: `./arroyo test` with no fixture knob at all.
#   QEMU gate:  ./arroyo test  (any wall; `UNAOS_WC=1` optional — nothing here is compositor-gated)
#               → unaos/target/serial.log, replayed by `x86_spec_replay` (arroyo names this file in
#               code as X86_DEFAULT_SPEC, so GATE-SPECROOTS resolves it GATED; no RUN-BY needed).
#
# WHY THIS FILE EXISTS AND x86-test.spec DOES NOT CARRY THESE FOUR LINES. It was written there
# first, and ONE QEMU RUN SAID NO — the measurement is recorded here because it is the whole reason
# the tree now has two specs for one verb:
#
#   `x86_test_completion` reads X86_TEST_SPEC — x86-test.spec — UNCONDITIONALLY, on every x86 leg,
#   and since LADDERTAIL `qemu_await.py --settled` gates on `Matcher.complete()`, which is the
#   end-of-run marker AND every REQUIRE. So a REQUIRE in x86-test.spec is not a pin on the default
#   leg; it is a pin on ALL of them. Three default-medium pins put there reddened
#   `UNAOS_WC=1 UNAOS_QEMU_FULL=1 ./arroyo test-fat sf 300` — the sf image is a SUPERFLOPPY with no
#   partition table, so the three could not match — and the verb died at its TESTTRUNC step calling
#   the run truncated. The capture was not truncated: the SAME capture, replayed against
#   x86-test.spec AS IT SHIPS, settles `status=complete complete_at=2200 rc=0`, and against
#   x86-fat.spec passes 36/36. A leg-specific pin in the file that answers "did ANY leg finish?"
#   converts every OTHER leg's healthy boot into an inconclusive one.
#
# THE SPLIT THAT FOLLOWS: x86-test.spec answers end-of-run for every leg and pins NOTHING ELSE (its
# header now carries this measurement); the per-leg pins live in a per-leg file that only
# `x86_spec_replay` reads — x86-fat.spec (sf), x86-ahci.spec (SATA boot + installer) and this one
# (the default medium). `x86_pick_capture_spec` names exactly one of them per fixture.
#
# ── WHAT IS PINNED: DEFAULTMEDIUM (`9617c437`) ──────────────────────────────────────────────────
# That commit changed what `./arroyo test` is handed: the default x86 stick stopped being a raw
# pattern in sector 0 and became an MBR-partitioned disk whose slot 1 is a FAT32 volume carrying the
# kernel, with the BOT scratch writer fenced BELOW the partition. Nothing scored it. The verb's
# verdict was a fault scan plus an end-of-run marker, and BOTH are happy on the OLD medium — so the
# day the builder stops writing the table, or writes it and the writer stops respecting it, this
# verb goes green on a fixture nobody asked for. That is the SPECROWS shape: a witness that landed
# with no pinned line is a regression riding a green verb.
#
# THE SHAPE, NOT THE NUMBERS. `count=`/`end=`/`ceiling=` move with the image size and `accepted=`
# with the table; what is fixed is that there IS a partition table, that slot 1 is a FAT32-LBA
# partition and is ACCEPTED, and that the scratch writer derived its keep-out FROM THAT TABLE.
# MEASURED against two recorded DEFAULTMEDIUM-era default captures on this bench —
# `defaultmedium-logs/serial-default.log` (the DEFAULTMEDIUM fold gate) and
# `quarrydock-logs/serial-unarmed.log` (the newest default `test` capture in the seat): 3/3 on both.
REQUIRE :: PART: mbr census handle=global protective=0 accepted=[1-9][0-9]* rejected=[0-9]+ ::
REQUIRE :: PART: mbr handle=global slot=1 type=0x0c boot=0x[0-9a-f]+ start=[0-9]+ count=[0-9]+ end=[0-9]+ ACCEPT ::
REQUIRE :: USB: \[usbw\] scratch geometry: USB last_lba=[0-9]+ \(num_blocks=[0-9]+\), keep-out ceiling=[0-9]+ \[mbr-partition-table\] ::
# The FORBID partner, and it is a REAL red spelling rather than an invented negation: `keep-out
# ceiling=0 [raw (no container in sector 0)]` is EXACTLY what this line printed BEFORE DEFAULTMEDIUM
# — recorded at `x86bind-logs/final-serial.log` on this bench — i.e. the writer treating the whole
# device as scratch. The REQUIRE above cannot catch that answer arriving under a different handle;
# this convicts it by name.
FORBID :: USB: \[usbw\] scratch geometry: .* keep-out ceiling=0 \[raw
#
# AND THE STORAGE PAIR, the same two shapes x86-fat.spec and x86-ahci.spec pin — one kernel line
# pinned by three legs, which is the one duplication this tree's spec contract permits (it is NOT a
# second spelling taught to the kernel; it is the same spelling asserted on three fixtures).
REQUIRE :: \[fatverb\] storage settle: waited=[0-9]+ms settled=found handles=global=(present|absent) sdhc=(present|absent|unbuilt) ::
REQUIRE :: USBREG: publish slot=[0-9]+ lun=[0-9]+ ix=[0-9]+ block_size=[0-9]+ num_blocks=[0-9]+ disks=[1-9][0-9]* ::
REQUIRE :: X86BIND: root=global:/KERNEL\.ELF serial=0x[0-9a-f]+ by=[a-z]+ bootinfo=0x[0-9a-f]+ agrees=(yes|no) mounts=[0-9]+ layout=true -> PASS ::
FORBID :: \[fatverb\] storage settle: .* settled=ceiling
FORBID :: STORSLOT: storage records FULL
FORBID :: X86BIND: root=-
#
# NOT HERE, and each for a measured reason rather than an oversight:
#   `:: STORSLOT: claim …`   its arc (`265542b9`) landed AFTER both recorded default captures on
#                            this bench, so no default capture in the seat carries it. It IS pinned
#                            in x86-fat.spec and x86-ahci.spec, both of which a gate run watched go
#                            green in this commit. Promote it here the first time a default `test`
#                            capture is recorded with it on the wire — do not promote it on the
#                            argument that it "must" print, which is how an unwatched gate is born.
#   `[dock] pins=…`          QUARRYDOCK's dock census is the COMPOSITOR's, absent from every boot
#                            without `UNAOS_WC` — and this file's leg does not require that knob.
#                            Pinned in x86-ahci.spec, whose leg is WC-armed by construction. This is
#                            the APPPIN trap x86-test.spec's header names, one subsystem along.
#   `:: INSTALLVERB: …`      unreachable without an operator: `install_verb` (`shell.rs:8271`) is
#                            called only from the shell's `"install"` arm (`shell.rs:5509`), and no
#                            QEMU leg in this tree TYPES it — INSTALLVERB's own arc drove it from a
#                            typist script. There is nothing on this wire to pin. The fix is a
#                            typist fixture, not a looser regex.
#
# NO `COMPLETE` MARKER HERE: x86-test.spec owns end-of-run for every leg (see the top of this file),
# and `x86_spec_replay` refuses to score any capture that marker calls short. A second marker here
# could only ever disagree with the first.

# ── CONTRACT (SPECRUN, 2026-09-15; this file joins it at birth) ─────────────────────────────────
# A PINNED LINE IN THIS FILE IS CHANGED TOGETHER WITH THE KERNEL LINE IT PINS, IN THE SAME COMMIT —
# re-pinned to the new wording (naming the arc that changed it), or dropped with the reason stated.
# It is never worked around by teaching the kernel a SECOND spelling of the same witness.
# TAIL-APPEND ONLY: pins get cited positionally, and a header insert moves every citation silently.
#
# GATE-SPECROOTS (`scripts/spec-roots.sh`, a leg of `./arroyo check`) reds by name on any spec under
# scripts/specs/ that is neither named in `arroyo`'s CODE nor carries a RUN-BY line. This file is
# named in code (X86_DEFAULT_SPEC, and again in `x86_pick_capture_spec`'s case), so it resolves
# GATED — and if a future change unwires that case, `check` says so by name instead of letting the
# file rot into prose.

# ── WINMENUSPEC (2026-09-16), TAIL-APPENDED past the contract block, as SPECROWS did to x86-fat.spec ──
# WINMENU's park timeout, forbidden BY NAME on the leg the fold gate runs (`UNAOS_WC=1 ./arroyo
# test` replays this file). WINMENUFLAKE (`5f7674c3`) made `winmenu::selftest` wait up to 250 ms for
# the menubar's publication and print `-> SKIP reason=menu-unpublished-after=<ms>ms` on a miss —
# never a FAIL, so mbench's DEFAULT_FORBIDS let it through green. After the park a miss is a finding
# about the compositor and reds this verb (docs/dev/FIXTURE_FLAKES.md Class 6a). One format string
# at two sites, leg 1 `name=gate` and leg 6 `name=VUG` (`video/winmenu.rs:1880`, `:2006`).
#
# NOT HERE, and it is the APPPIN trap this file's own NOT-HERE list names for `[dock] pins=`:
#   `REQUIRE :: WINMENU: … :: PASS ::`   `crystal::selftest`, the only route to `winmenu::selftest`,
#                            is called under `#[cfg(all(feature = "witness", feature = "wc"))]`
#                            (`arch/x86_64/syscall.rs:17532-17533`), so the verdict is ABSENT from
#                            the knob-free `./arroyo test` this file also serves. The REQUIRE is
#                            pinned in x86-ahci.spec, whose leg is WC-armed by construction. This
#                            FORBID is honest on both polarities: the line cannot print without
#                            the knob, and with it a healthy boot prints the PASS spelling instead.
# Measured: absent (0 hits) on every recorded default-lane capture on this bench that carries the
# fixture (`logs/foldgate/seattest1.log`, `quarrydock-logs/serial-unarmed.log`,
# `defaultmedium-logs/serial-default.log`); present (rc=1, FORBID hit) on the same capture with the
# kernel's own SKIP spelling injected — the go-red recorded in the WINMENUSPEC commit.
FORBID :: WINMENU: .* -> SKIP reason=menu-unpublished-after=[0-9]+ms ::
#
# ── VECTORS (2026-09-22), TAIL-APPENDED past WINMENUSPEC, as WINMENUSPEC was past the contract ──
# The IDT vector table, pinned as ONE line, because the whole claim of rmbp-ledger B168 is that
# replacing five hand-written `pub const`s with an allocator MOVED NO NUMBER. `interrupts::vectors`
# hands out `xhci` 0x40, `nic` 0x41 and `ehci` 0x43 in that order (0x42 is `ipi`, reserved by name
# before the first `alloc` runs), so every capture recorded on this bench stays byte-comparable
# across the fold — and this rule is what says so on the next boot rather than on a reader's word.
#
# THE ORDER AND THE NUMBERS ARE THE ASSERTION, not the counts. `allocated=` and `free=` are left
# open (`[0-9]+`) because a later device joining the allocator moves both LEGITIMATELY and must not
# red this lane; `table=` is pinned character for character, because a number moving there is
# exactly the regression this arc exists to make visible. A later arc that renumbers on purpose
# re-pins this line in the same commit, which is the contract at the head of this file.
#
# NO FORBID PARTNER, and the reason is measured rather than stylistic: B160's go-red showed that a
# FORBID which can no longer match reads ✅ with 0 hits — indistinguishable from one that passed.
# The failure mode here is a vector SILENTLY CHANGING, which a REQUIRE on the exact table convicts
# and no negation could state more sharply. The allocator's own refusals (`-> REFUSED
# reason=duplicate-name`) are not forbidden either: they are the module's correct behaviour, and
# the one that fires in a healthy boot is none.
#
# Measured: 0 hits on `vectors-logs/base-serial.log`, the default capture taken at this branch's
# parent 2495f3a2 before a line of this arc was written (the go-red — the run that has no allocator
# in it), and 1 hit on the capture the same verb produced after.
REQUIRE \[vectors\] allocated=[0-9]+ free=[0-9]+ table=timer:0x20,xhci:0x40,nic:0x41,ipi:0x42,ehci:0x43,spurious:0xff == witness ::

# ── BUSX86 M3 (2026-09-22), TAIL-APPENDED past the contract block, the WINMENUSPEC shape ────────────
# THE EQUIVALENCE WITNESS, pinned on the lane it actually runs on. `busx86_midden_launcher` is
# UNCONDITIONAL — no `witness`, no `wc`, no storage knob — because the bus is not optional surface on
# this arch (the `SYS_MSEND`/`SYS_MRECV` dispatch reasoning verbatim), so unlike WINMENU's verdict the
# PASS spelling is present on the knob-free `./arroyo test` and can be REQUIRED rather than only
# FORBIDden in its negative. It skips cleanly with a named line when the box has no storage or fewer
# than three placement cores; neither is true of this lane.
#
# WHY `diff=0` IS IN THE REQUIRE AND NOT LEFT TO `PASS`. `PASS` is a conjunction of nine things
# (sequence, seal, cleanup, unlinks, teardown, drain, the payload pair, the write side, the bad
# frame), and a future edit that loosened any ONE of them would keep printing `PASS` while the
# equivalence claim itself quietly stopped being checked. `diff=0` is the claim: nine legs, each the
# bus's errno against the DIRECT syscall's, compared byte-for-byte. Pinning both means the line has
# to carry the claim AND the verdict, and neither can go green without the other.
REQUIRE :: BUSX86-EQ: .* legs=9 same=9 diff=0 .* -> PASS ::
# The FORBID partner is a REAL spelling, not an invented negation: the launcher prints exactly this
# line with `-> FAIL` on any divergence, and the go-red for this arc (one leg's compare forced to
# answer -EPERM, reverted) produced it — `diff=1 [... cat-other:-EACCES/-EPERM ...] -> FAIL`.
FORBID :: BUSX86-EQ: .* -> FAIL ::
# AND THE STAMP WITNESS, which BUSX86 M2 landed unpinned. It is the transport half M3 stands on: if
# the stamping or the mailbox regressed, every M3 leg would still compare equal (both legs would be
# wrong together) and `diff=0` would say nothing. One kernel line, two arcs, one lane.
REQUIRE :: BUSX86-STAMP: .* :: PASS \[w=0x3f/0x3f\] ::
FORBID :: BUSX86-STAMP: .* :: FAIL
# The U10 deferred-op queue is ONE deep and its overflow is a DROPPED acknowledged mutation. M3's
# cleanup is choreographed around that (one unlink per GO step, the launcher draining between), and
# this is the line that convicts a future fixture that stops doing so. MEASURED as a real red on this
# bench: the M3 build that skipped an unlink on -EMFILE printed it (`op=2 name=7 16 bytes`).
FORBID :: U10: OP QUEUE FULL
#
# ── TSTETAP (2026-09-22), TAIL-APPENDED past VECTORS ──────────────────────────────────────────────
# `absorbed=` ON THE `tste` TAP, because until this line NOTHING ON ANY LANE GATED IT — and it is the
# count of boot verdicts the kernel actually recorded. `selftest::capture` is the ONLY record `tste`
# can replay a boot fixture from (it cannot re-run them), so a scanner that stops recognising the
# `-> PASS` marker does not make any fixture fail: it makes every fixture DISAPPEAR from `tste`'s
# `[boot-time]` section, as if it had never executed. That is the exact defect SERWIT-2W was written
# to end, and it had no gate.
#
# MEASURED, NOT REASONED — and the go-red is the whole reason this rule exists in this form. Change
# `N_PASS` in `crates/kernel/src/selftest.rs` from `b"-> PASS"` to `b"-> PASSED"` — one mis-spelt
# marker, nothing else — and the wc lane reads
#     :: SERWIT-2 tap tste: submitted=579 absorbed=0 staged=0 dropped=0 suppressed=579 …
# against the green tree's
#     :: SERWIT-2 tap tste: submitted=579 absorbed=21 staged=0 dropped=0 suppressed=558 …
# — every verdict lost. AND THE VERB STILL EXITED 0. `./arroyo test` was green, the spec replay was
# green, the boot was green; 21 recorded verdicts had become 0 and no gate on this bench said a word.
# That silence is what this line ends.
#
# WHAT IS PINNED AND WHY EACH ONE.
#   * `absorbed=[1-9]\d*` — the assertion. Not a threshold: the boot fixture population moves with
#     every knob, so any number above zero is the honest floor, and zero is the failure.
#   * `dropped=0` — SERWIT-2's own claim, that this ring has NO loss path left but the ring genuinely
#     filling (which is counted separately and reported by `run()`). A non-zero here is a regression
#     to the `try_lock`-and-discard shape SERWIT-2 deleted.
#   * `inflight=0` — every exit from `capture` charges exactly once, so nothing may be left in
#     flight when the tally prints. `submitted=[1-9]\d*` keeps a tap that never ran from acquitting.
#   * `staged=`, `suppressed=`, `torn=`, `in_progress=` are `\d+`: they move with the traffic.
#
# Measured green on NINE independent captures — this branch's `sertaps-logs/R1-serial.log` (the
# fe385712 baseline) and `R2-serial.log` (after the arc), and all seven DOCKID2 captures under
# `docs/dev/evidence/rmbp-0922/dockid2/` — and red on exactly one, `R3-serial.log`, the mutation
# above. No FORBID partner, for B160's reason as cited by the VECTORS block.
REQUIRE :: SERWIT-2 tap tste: submitted=[1-9]\d* absorbed=[1-9]\d* staged=\d+ dropped=0 suppressed=\d+ torn=\d+ inflight=0 in_progress=\d+ ::
#
# ── SERTXPIN (2026-09-22), TAIL-APPENDED past TSTETAP, as TSTETAP was past VECTORS ────────────────
# `[sertx]` — SERIALTX's transmit-cost census (rmbp-ledger B154) with TAPSMAX's per-tap decomposition
# folded into it (B161) — WAS PINNED IN NO SPEC AT ALL. B161 says so in as many words, and the gap is
# the expensive kind: this is the ONE line on the wire that says what a print costs and which of the
# four post-mask taps is paying. Every arc that has ever chased a dark window read it. Nothing on any
# lane asserted it was still being printed, or still had its fields.
#
# WHY HERE AND NOT IN x86-test.spec: the head of this file. `[sertx]` is UNCONDITIONAL and x86-only
# (serial_ring.rs:1780 — the images that produce dark windows are built witness-FREE, so gating the
# census on `witness` would have deleted it from exactly the boots that need it), so it is present on
# every x86 leg — but x86-test.spec answers "did ANY leg finish?" for all of them, and a pin put
# there is a pin on the arm legs' completion check too. It goes in the per-leg files: here, and in
# x86-wc.spec in the same commit.
#
# WHAT THE SHAPE ASSERTS, FIELD BY FIELD, and why almost all of them are open.
#   * `tap_max=fbcon:…,ftdi:…,tste:…,rec:…` and the `tap_sum=` twin are pinned as a FOUR-NAME ORDERED
#     TABLE, character for character in the names and the commas, for the reason the VECTORS rule
#     above gives: a reader lines `tap_max=` up against the four `:: SERWIT-2 tap …:` lines WITHOUT A
#     LOOKUP, and that only works while the order is `taps()`' order. A tap added, dropped or
#     reordered silently turns every recorded `tap_max=` into a different measurement wearing the old
#     shape. The NUMBERS are `\d+` — they are a cost on a shared bench and they move with the load.
#   * `masked_b=0` is the one VALUE pinned, and it is the census stating SERIALTX's own claim: a span
#     in which not one of `bytes=` went out behind the mask. It is asserted on a QUIET span, which is
#     what presence semantics buy — measured 14 of 17 `[sertx]` lines on the capture below, and 4 of
#     7 on the leanest of the seven DOCKID2 captures — and pre-SERIALTX no span with `prints>0` could
#     have reached 0 at all, because every print wrote its own line synchronously under the mask.
#   * everything else is `\d+`, deliberately. `prints=`, the three `_us` terms and the two `_cy`
#     terms are wall costs; pinning any of them would red this lane on a busy bench, which is a
#     finding about the bench and not about the kernel.
#
# NO FORBID PARTNER — B160's rule, cited by the VECTORS block above for the same reason: a FORBID
# that can no longer match reads ✅ with 0 hits. The failure mode here is the line GOING SILENT or
# LOSING A FIELD, which a REQUIRE on the whole shape convicts and no negation can state.
#
# GO-RED, MEASURED ON TWO REAL CAPTURES RATHER THAN REASONED, and they fail on DIFFERENT halves:
#   * `logs/foldgate/g9-test-x86-default.log` (cd642fd8, pre-SERIALTX) carries ZERO `[sertx]` lines —
#     the census did not exist. 0 hits, MISSING.
#   * `docs/dev/evidence/rmbp-0922/dockid2/r3-wc-serial.log` (pre-TAPSMAX) carries 22 `[sertx]` lines,
#     20 of them `masked_b=0`, and STILL 0 hits: its census stops at `taps_us_max=` and has no
#     `tap_max=`/`tap_sum=` at all. That second red is the one that matters, because it is the shape
#     regression this rule exists to catch rather than the absence any reader would have noticed.
REQUIRE \[sertx\] prints=\d+ masked_us_max=\d+ masked_us_mean=\d+ drain_us=\d+ emit_us=\d+ spin_us=\d+ bytes=\d+ masked_b=0 fifo_b=\d+ taps_us=\d+ taps_us_max=\d+ tap_max=fbcon:\d+,ftdi:\d+,tste:\d+,rec:\d+ tap_sum=fbcon:\d+,ftdi:\d+,tste:\d+,rec:\d+ sink=(uart|ftdi|both|none) hz=\d+ masked_cy_max=\d+ masked_cy_sum=\d+

# ── FATLFN (2026-09-22, R60), TAIL-APPENDED past the contract block, as WINMENUSPEC was ──────────
# THE NEW PIN, and why it belongs on THIS file rather than on x86-test.spec: it is a per-leg claim
# about the DEFAULT medium, which is the only lane whose boot reaches `fat::probe_once` with a
# WRITABLE FAT32 volume under it (`FS: FAT mounted: FAT32 vol@LBA2048 …` is on every default capture
# on this bench; FRGUARD prints `writes ALLOWED (guard DISARMED)` on the same wire). Put in
# x86-test.spec it would gate EVERY x86 leg, including the `sf` superfloppy and the AHCI fixture,
# which is exactly the mistake this file's header records and exists to prevent.
#
# WHAT IS PINNED. `fs/fat.rs` §FATLFN gave the create path VFAT long names: the component-slot run,
# the `~n` short alias, a contiguous-run allocator that grows the directory, and a slots-before-the-
# short-entry write order. The fixture writes PETER'S OWN NAME — `Screenshot 2026-09-22 at
# 17.31.02.png`, 36 characters, two spaces, three dots, the string `format_83` refuses — reads it
# back on a FRESH mount by BOTH spellings, checks the run's ordinals and checksum on the medium, and
# then cuts a write after 2 of 3 slots and proves the orphan run is ignored.
#
# THE SHAPE, NOT THE MOMENT. The date and time come from a FIXED synthetic `WallTime` (the QEMU lane
# has no clock — `clock::now()` is `None` all boot, which is why `prtscr` still says
# `name_from=clock-unset` here), so they could be pinned literally; they are pinned as a SHAPE
# anyway, because what the leg is about is the formatter's spelling — four-digit year, dotted
# seconds, `.png` — and a literal would have to be re-typed the day the fixture picks another
# moment. `slots=3` and `alias=SCREEN~1\.PNG` ARE literal: they are the two facts another VFAT
# implementation would see, and neither may drift silently.
REQUIRE :: FAT-LFN: created=Screenshot [0-9]{4}-[0-9]{2}-[0-9]{2} at [0-9]{2}\.[0-9]{2}\.[0-9]{2}\.png slots=3 alias=SCREEN~1\.PNG readback=ok alias_readback=ok checksum=ok torn_k=[0-9]+ orphans_ignored=ok -> PASS ::
# The FAIL spelling is already convicted by mbench's DEFAULT_FORBIDS (`FAIL ::`) and by `arroyo`'s
# own fault scan, so it is NOT restated here. What neither of those catches is the witness going
# QUIET-BUT-HONEST: `SKIPPED` is the correct answer on a read-only medium (the rMBP's own boot
# volume) and on a lane with no FAT volume, and it is the WRONG answer on this one. A default leg
# that starts skipping has lost its writable volume — a real finding — and would otherwise read as
# green, since the REQUIRE above going unmatched is reported as a short REQUIRE rather than as
# positive evidence of what went wrong. This FORBID names it.
FORBID :: FAT-LFN: SKIPPED

# ── STOR-1 M1 (2026-09-23), TAIL-APPENDED past BUSX86 M3, the same shape it used ──────────────────
# THE CREATED-NAME ENTRY, pinned on the lane it runs on. `stor1_name_launcher` is UNCONDITIONAL — no
# `witness`, no `wc`, no storage knob — for BUSX86 M3's reason verbatim: `SYS_OPEN` is not optional
# surface on this arch, so the PASS spelling is present on the knob-free `./arroyo test` and can be
# REQUIRED rather than only FORBIDden in its negative. It skips with a named line when there is no
# free address-space slot or the volume still carries a STOR1.BIN a previous boot left; neither is
# true of this lane.
#
# WHY THE BITMASK IS IN THE REQUIRE AND NOT LEFT TO `PASS`. Same argument BUSX86-EQ's `diff=0` makes
# one arc earlier: `PASS` here is a conjunction of six ring-3 legs plus five kernel-side conditions
# (signalled, sealed, row cleared, queue drained, entry torn down), and an edit that stopped SCORING
# a leg would keep printing `PASS` with a smaller mask. `w=0x3f` is the claim — six legs, of which
# bits 2 and 3 are the milestone itself (a name re-opens after its last descriptor closed, and reads
# back byte-exact) and bit 5 is the contract that must NOT have moved with it (unlink still removes
# the name, and a plain re-open after it is -ENOENT).
REQUIRE :: STOR1-NAME: .* :: PASS \[w=0x3f/0x3f\] ::
# The FORBID partner is a REAL spelling, not an invented negation: the launcher prints exactly this
# line on any short mask, and the go-red for this milestone produced it — `created_desc_any_row` put
# back at `sys_open_dynamic`'s gate, which is the OLD identity model, restored and then reverted.
FORBID :: STOR1-NAME: created-name entries FAIL

# ── STOR-1 M2 (2026-09-23), TAIL-APPENDED past STOR-1 M1 ──────────────────────────────────────────
# RENAME, pinned on the same unconditional lane for the same reason: `SYS_RENAME` is dispatched with
# no feature gate at all, so its witness is present on the knob-free `./arroyo test`.
#
# `w=0x7f` carries the claim the same way M1's mask does — seven ring-3 legs, of which bit1..3 are
# the move itself (rename returns 0, the OLD name is -ENOENT, the NEW name reads back byte-exact),
# bit4 is "a refused rename mutates nothing" and bit5 is the x86-only `-EBUSY` on a live source
# together with its own retirement (the same rename returns 0 the moment the handle closes). The
# PASS spelling additionally requires the kernel-side OWNER-ONLY leg (`acl_ok`), which no mask bit
# can carry because a single ring-3 program cannot be two principals.
REQUIRE :: STOR1-MV: .* :: PASS \[w=0x7f/0x7f\] ::
# The FORBID partner is the launcher's own FAIL spelling, produced by this milestone's go-red (the
# `owned_unlink_permitted` refusal deleted from `rename_created`'s authorization, reverted).
FORBID :: STOR1-MV: rename FAIL

# ── STOR-1 M3 (2026-09-23), TAIL-APPENDED past STOR-1 M2 ──────────────────────────────────────────
# THE WRITE SIDE ON THE WIRE. `BUSX86-WR` is a NEW witness rather than three more legs of BUSX86-EQ
# for a mechanical reason: that fixture is a `global_asm!` block in the middle of a 26k-line file
# whose panic `Location` records embed line numbers (B94), so this seat extends the CLAIM and appends
# the witness. Eleven legs: the BANDY-WR round trip (write->cat byte-exact and the DIRECT syscall
# sees the same file; mv->cat(new) byte-exact + cat(old) -ENOENT; rm->cat -ENOENT), the BANDY-EQ2
# denials (write/rm/mv of a FOREIGN-owned file each -EACCES), the BANDY-ACL integrity check (the
# denials left the foreign file there, still foreign, with no stolen name), and the CEILING leg — a
# bus `cp` made while two created files are held open, read back directly byte-exact, which is the
# shape rmbp-ledger B171 recorded as `cp-copy open_rc=-24` before a `cp` stopped costing a slot.
REQUIRE :: BUSX86-WR: .* :: PASS \[w=0x7ff/0x7ff\] ::
FORBID :: BUSX86-WR: write side FAIL
#
# AND BUSX86-EQ's WRITE-SIDE LEGS ARE RE-PINNED HERE, in the commit that changed the kernel line they
# pin, which is this file's contract. They used to read `write/rm/mv=3/3 -ENOSYS` — an assertion that
# the three verbs DID NOTHING. They do something now, so the pin becomes the exact triple that
# fixture's own frames must produce, and it asserts strictly more than the old one: `-EBUSY` says the
# ACL ran and the live-source refusal ran, `ok` says the owner was admitted, and `-ENOENT` says the
# ordering between two destructive verbs on the same name is the one the wire asked for.
REQUIRE :: BUSX86-EQ: .* write/rm/mv=-EBUSY/ok/-ENOENT .* -> PASS ::

# ── SPECPINS2 (2026-09-23, rmbp-ledger B184), TAIL-APPENDED past STOR-1 M3 ────────────────────────
# THE TWO EHCI-HID SELF-TESTS, pinned for the first time. Both print on every x86 QEMU boot and until
# this block NO SPEC READ EITHER: a fixture whose verdict no spec reads can stop running and the verb
# stays green, which is the DOCKID2 shape (rmbp-ledger B165, `:: DOCKID:` pinned by no spec).
# UNCONDITIONAL on this lane: `drivers::ehci::init` is compiled by `ehcihid`, which `arroyo` arms by
# default (opt-out `UNAOS_NOEHCIHID=1`), and it calls `isr_selftest` and `pass_period_selftest`
# (`drivers/ehci/mod.rs:18091`, `:18095`) with no knob of their own. Rows: ISRARM is B146/B154's
# completion-interrupt fixture, PASSPERIOD is B146's pass-period census fixture (EHCIDARK, da8c8e83).
#
# WHY HERE AND NOT BESIDE `:: BPACE: ehci-hid-done` IN x86-witness.spec. That file is the METAL
# witness battery (paygo + logts + witness armed) and is replayed against a bench capture, never
# against this lane. Flight 11's capture (`~/unaos-bench/scratch/rmbp-0915/bootwaits-logs/f11.log`)
# carries ISRARM at 295 ms and NO PASSPERIOD line, because flight 11's image (56bbe53b) predates it:
# `git grep 'PASSPERIOD self-test' 56bbe53b` is empty. A REQUIRE there would red the one metal
# capture the bench holds for a reason about the image, not the boot; the metal pin waits for a
# flight-12 capture.
#
# THE SHAPE. ISRARM's booleans are literal because PASS is their conjunction (`order_ok && payload_ok
# && rearm_ok && toggle_ok && full_ok && modes == 2`), so a literal costs nothing and a later edit
# that drops a field reds here instead of narrowing silently. `depth=` is the ring size, left open.
# PASSPERIOD's numbers are the fixture's own synthetic ruler (99 x 1000 us + one 40000 us stall), and
# the verdict already folds them (`max_us == 40_000 && mean_us == 1_390 && n == 100`), so they are
# `\d+` here and the verdict gates.
REQUIRE :: EHCI-HID: ISRARM self-test: modes=2 \(overlay-direct \+ qTD-chain\) depth=\d+ fifo=true payload=true rearm=true toggle=true ringfull-refuses=true -> PASS == witness ::
REQUIRE :: EHCI-HID: PASSPERIOD self-test: samples=\d+ backwards-delta-refused=true tick=1000us stall=40000us -> pass_period_us_max=\d+ pass_period_us_mean=\d+ -> PASS == witness ::
# The FAIL spellings are the fixtures' own verdict token, stated here so this file gates alone (the
# DEFAULT_FORBIDS `-> FAIL` catches them too). The ISRARM SKIP is the fixture's one decline: `phys_of`
# refused the static `DMA_POOLS[0]` slot (`drivers/ehci/mod.rs:15236`), the same refusal
# `arm_interrupt_ep` would make, so the ISR path went UNTESTED. The REQUIRE above already misses on
# it; the FORBID names it, because a skipped ISR fixture is the silence this block exists to end.
FORBID :: EHCI-HID: ISRARM self-test: .* -> FAIL
FORBID :: EHCI-HID: ISRARM self-test SKIPPED
FORBID :: EHCI-HID: PASSPERIOD self-test: .* -> FAIL

# ── IOAPIC2 (2026-09-23, rmbp-ledger B191), TAIL-APPENDED past SPECPINS2 ──────────────────────────
# THE ISRARM DECISION NAMES ITS REASON, AND THE STALE SENTENCE IS GONE. Flight 12 (`f12-boot1.log`,
# 5833 ms) printed `[ioapic] route bdf=0:29.0 pin=INTA line=0 -> REFUSED reason=no-firmware-line`
# and, the SAME millisecond, `ISRARM REFUSED — this function offers no usable MSI capability, and
# there is no IOAPIC in this kernel to route INTx to` — on an image whose I/O APIC census had just
# printed `ioapics=1 … gsis=24`. The EHCI refusal was a FIXED STRING written before the I/O APIC
# existed. It is now composed from `arch::x86_64::ioapic_route_intx_why`'s `Err(reason)`, which is
# the token the route's own `[ioapic]` line printed (knob-off: `no-ioapic-in-kernel`, the one case
# where the old sentence was true), and the armed line names the LIVE path (`via=msi addr=…` or
# `via=ioapic-intx gsi=<n>`) instead of reading "MSI vector" on both.
#
# KNOB-NEUTRAL BY CONSTRUCTION, because this file is replayed by every default-medium
# `./arroyo test`, with or without `UNAOS_IOAPIC`: the REQUIRE accepts either arm of the decision
# and requires only that the arm be STATED (a path, or a refusal with a reason token) — it certifies
# the decision's wording, not a limitation (LAWS §5, "Require a PROPERTY"). Measured: knob-off lane
# (the harness usb-ehci offers no MSI capability) prints `REFUSED reason=no-ioapic-in-kernel`; the
# `UNAOS_IOAPIC=1` lane prints `armed via=ioapic-intx gsi=<n>`.
REQUIRE :: EHCI-HID: \[\d+\] ISRARM (armed via=(msi addr=0x[0-9a-f]+|ioapic-intx gsi=\d+) vector 0x[0-9a-f]+,|REFUSED reason=[a-z0-9-]+ )
# The sentence flight 12 printed beside a live I/O APIC. Neither polarity may print it again.
FORBID there is no IOAPIC in this kernel to route INTx to
#
# IOAPIC2 M2 — THE CHIPSET PIRQ ROUTE. Under `UNAOS_IOAPIC=1` the builder places the harness usb-ehci
# at 0:29.0 (the PCH's EHCI #1 slot) and `ioapic::pirq_gsi` derives its I/O APIC input from the
# ICH9 LPC's own registers — `D29IR` behind RCBA picks the PIRQ for the function's pin, PIRQ A..H is
# I/O APIC input 16..23 — which is the path the rMBP's 0:29.0 takes. Measured on that lane:
#   [ioapic] pirq bdf=0:31.0 id=8086:2918 family=ich9 rcba=0xfed1c000 pirqa=0x0a … pirqd=0x0b …
#            fn=0:29.0 pin=INTD d29ir=0x3210 -> pirq=D gsi=19 line=11 fw_line=11 fw_agree=yes
#   [ioapic] armed bdf=0:29.0 gsi=19 vector=0x43 masked=false … unmasked_lo=0x0000a043 …
# KNOB-NEUTRAL, so both rows are FORBIDs: a knob-off boot prints no `[ioapic]` line and neither can
# fire there, which is correct — there is no route to judge. `fw_agree=` compares two INDEPENDENT
# derivations of the same PIRQ: ours (the pin through `D29IR`, then `PIRQ[n]_ROUT` bits 3:0) and
# firmware's (OVMF computed the Interrupt Line from its own table and wrote it to 0x3C). `no` means
# one of them is wrong. GO-RED (source mutation, the wrong PIRQ index — `idx + 1`): `pirq=E gsi=20
# line=10 fw_line=11 fw_agree=no`, and with the typist's 120 events on the wire the vector is never
# delivered (`ISRARM IRQ DEAD … irq=0`). `masked=` is derived from the entry's read-back bit 16, so
# an unmask that did not stick reds here and not only in the ISR counters.
FORBID \[ioapic\] pirq .* fw_agree=no
FORBID \[ioapic\] armed .* masked=true
