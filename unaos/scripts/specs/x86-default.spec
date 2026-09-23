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
