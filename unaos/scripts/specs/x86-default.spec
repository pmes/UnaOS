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
