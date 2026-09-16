# x86-install.spec — THE INSTALLER'S OPERATOR PATH, typed at the shell prompt by a robot.
#   QEMU gate:  ./arroyo test-install 180
#               → unaos/target/serial.log, replayed by `x86_spec_replay` (arroyo names this file in
#               code as X86_INSTALL_SPEC, so GATE-SPECROOTS resolves it GATED and it needs no
#               RUN-BY). The verb arms UNAOS_WC / UNAOS_INSTGUI / UNAOS_INSTALLDEMO / UNAOS_AHCI /
#               UNAOS_PART_DISK=builder/part-fixture.img / UNAOS_QEMU_FULL and drives the QMP typist.
#
# WHY A FOURTH x86 SPEC (INSTALLTYPIST, rmbp seat, 2026-09-16), and it is the one spec in this
# directory whose subject is a KEY PATH rather than a boot path. SPECROWS (53e0122b) wired three
# per-leg specs and had to leave one STOP standing, quoted from `docs/dev/OS/rmbp-queue.md`:
#
#     "STOPs: INSTALLVERB gets NO pin — `install_verb` (shell.rs:8271) is reachable only from the
#      shell's typed \"install\" arm (shell.rs:5509) and no QEMU leg types it, so its census is not
#      on any gated wire; a typist fixture is owed, not a looser regex."
#
# That is the whole reason this file exists. INSTALLVERB (f82bc1d2) built the verb and proved it by
# hand — a shell script outside the repo poking `scripts/qmp_type.py` at a port — so the evidence
# was real and reproducible by nobody. A looser regex would have been the wrong fix twice over: the
# lines below are not printed on ANY leg that does not type, so a regex relaxed until it matched
# would be a pin on nothing. The fixture was owed; `test-install` is the fixture.
#
# WHAT THE TYPIST DOES, because every pin here is downstream of one keystroke burst and a reader
# needs to know which. Through the emulated usb-kbd (QMP `send-key` → xHCI/EHCI HID → the shell's
# key path — no back door, and `:: [midden] cmd=` below is the proof the REAL parser saw the text):
#
#   1. `install`            the read-only census of every registered disk, with the preview
#   2. `install global 2`   the ACT: write into the fixture's one empty, big-enough slot
#   3. `install global 1`   the GO-RED: the APFS slot, which must refuse and move no byte
#   4. `install --gui`      the graphical installer, then Enter (census) and Enter (install-go)
#
# THE FIXTURE IS WHAT MAKES REFUSALS POSSIBLE. `scripts/make-gpt-fixture.py` builds a five-slot GPT
# disk — a foreign FAT volume, an APFS-signature volume, an empty target, an ESP-typed slot and an
# undersized slot — because a blank disk cannot refuse anything. Slot 2 is the only installable one
# and every other slot is a distinct refusal REASON, which is why the pins below name slots rather
# than counting them.
#
# NO `COMPLETE` MARKER HERE, for x86-ahci.spec's reason exactly: `x86_test_completion` reads
# X86_TEST_SPEC (x86-test.spec) on EVERY x86 leg including this one, so end-of-run is already
# answered by the marker measured for it, and `x86_spec_replay` refuses to score any capture that
# marker calls short. A second marker here could only disagree with the first.
#
# MEASURED, before this file was wired: INSTALLVERB's own gate capture,
# `~/unaos-bench/scratch/rmbp-0915/installverb-logs/run4-serial.log`, on the same knob set
# (`⚡ kernel features: witness,ehcihid,kbdwit,sdhcblk,installdemo,smolnet,wc,instgui,sdwrite,ahci`),
# replayed green against this file before the verb's own run. Every REQUIRE below is a line that
# capture carries; every FORBID is a spelling it does not.
#
# ── 1. THE TYPIST REACHED THE PARSER ─────────────────────────────────────────────────────────────
# These two are the pins that make this leg different from every other spec in this directory, and
# they belong at the top: they assert that a KEYSTROKE became a parsed command. `[midden] cmd=` is
# printed by the shell's own dispatcher from the line it actually read, so a typist that raced the
# prompt and dropped a character produces a DIFFERENT string here, not a missing one — which the
# verbatim quoting below catches and a `COUNT` of install lines would not.
REQUIRE :: \[midden\] cmd="install" -> Host verb=install ::
REQUIRE :: \[midden\] cmd="install global 2" -> Host verb=install ::
REQUIRE :: \[midden\] cmd="install global 1" -> Host verb=install ::
REQUIRE :: \[midden\] cmd="install --gui" -> Host verb=install ::
#
# ── 2. THE CENSUS (burst 1: `install`) ───────────────────────────────────────────────────────────
# `disks=` loosely: the count is the machine's, not the verb's. `disks=0` is forbidden below, which
# is the only value that would make the rest of this section vacuously satisfiable.
REQUIRE :: INSTALLVERB: census disks=[0-9]+ ::
REQUIRE :: INSTALLVERB: census disk=global transport=global ::
# The fixture's own table, read back through the partition engine. `parts=5` IS pinned exactly —
# it is a fact about `make-gpt-fixture.py`, not about the machine, and a fixture that silently grew
# or lost a slot would make every refusal pin below assert a different disk.
REQUIRE :: PINSTALL: census disk=.* parts=5 foreign=[0-9]+ friend=[0-9]+ empty=[0-9]+ ::
REQUIRE :: PINSTALL: census part=1 type=7c3457ef lba=[0-9]+\.\.[0-9]+ sectors=[0-9]+ content=APFS ::
REQUIRE :: PINSTALL: census part=3 type=c12a7328 lba=[0-9]+\.\.[0-9]+ sectors=[0-9]+ content=ESP ::
#
# ── 3. THE FOUR REFUSALS AND THE ONE OFFER (burst 1) ─────────────────────────────────────────────
# Each refusal is pinned by its REASON token, because the reason is the guard that fired and the
# target alone would not say which. `Refusal::say` (install/partition.rs:389-428) owns these
# spellings; a pin here is changed together with that file, in the same commit.
REQUIRE :: PINSTALL: refusal target=global:disk reason=disk-has-foreign-volumes foreign=[0-9]+ friend=[0-9]+ -> guard OK ::
REQUIRE :: PINSTALL: refusal target=global:part0 reason=partition-not-empty content=FAT -> guard OK ::
REQUIRE :: PINSTALL: refusal target=global:part1 reason=partition-not-empty content=APFS -> guard OK ::
REQUIRE :: PINSTALL: refusal target=global:part3 reason=partition-is-esp -> guard OK ::
REQUIRE :: PINSTALL: refusal target=global:part4 reason=partition-too-small have=[0-9]+B need=[0-9]+B -> guard OK ::
# And the ONE slot the census may offer. `part2` is pinned exactly: the whole point of the fixture
# is that exactly one slot is installable, and a preview naming any other slot is a FORBID below.
REQUIRE :: INSTALLVERB: preview target=global:part2 content=empty sectors=[0-9]+ -> INSTALLABLE ::
# COUNT and not REQUIRE, and >=5 rather than the 13 this capture carries: the total is a function of
# how many times the operator asks, which is the TYPIST's business and not the guard's. What is a
# fact about the guard is that refusing is its ordinary answer on this disk. (LAWS §5: COUNT means
# hits >= n, so a later arc that types one more command can never red this line.)
COUNT 5 :: PINSTALL: refusal target=
#
# ── 4. THE ACT (burst 2: `install global 2`) ─────────────────────────────────────────────────────
# The write, its verification, and the neighbours. `-> PASS` on the `wrote` line is the kernel's own
# comparison of `verified` against `files` (shell.rs:8266/:8274 are the two arms), so the counts are
# matched loosely here and the VERDICT is what is pinned; the `-> FAIL` arm is caught by mbench's
# built-in FORBID set and needs no line in this file.
REQUIRE :: INSTALLVERB: install target=global:part2 as_esp=0 neighbours=[0-9]+ ::
REQUIRE :: INSTALLVERB: wrote part=2 files=[0-9]+ bytes=[0-9]+ verified=[0-9]+/[0-9]+ -> PASS ::
REQUIRE :: INSTALLVERB: neighbours untouched=[0-9]+/[0-9]+ -> PASS ::
#
# ── 5. THE GO-RED THE OPERATOR CAN TYPE (burst 3: `install global 1`) ────────────────────────────
# Asking for the APFS slot BY NAME, which is the mistake this guard exists to survive: on the bench
# rMBP slot 1's real-world counterpart carries Catalina. `err=NotBlank — nothing written` is the
# verb's disposition and `refusal target=part1` is the engine's; both are pinned because either one
# alone could be printed by a path that still wrote.
REQUIRE :: INSTALLVERB: install target=global:part1 err=NotBlank — nothing written ::
REQUIRE :: PINSTALL: refusal target=part1 reason=partition-not-empty content=APFS -> guard OK ::
#
# ── 6. THE GRAPHICAL INSTALLER (burst 4: `install --gui`, then Enter, Enter) ─────────────────────
# The same engine reached through the compositor instead of the parser, which is the half INSTALLVERB
# could demonstrate and not gate. Step 1 is the census screen (READ-ONLY by construction), step 2 is
# the attended Enter that commits — onto the UNDERSIZED slot, so the answer is a refusal and not a
# write. `whole_disk_offered=0` is pinned exactly: the census found foreign volumes, so R25 says the
# whole-disk affordance must not open, and that is the strongest single assertion in this file.
REQUIRE :: INSTALLVERB: --gui — installer window requested \(opens on the next main-loop pass\) ::
REQUIRE \[wc-x\] instgui census step=1 gpt=1 parts=[0-9]+ installable=[0-9]+ whole_disk_offered=0 — READ-ONLY, nothing written
REQUIRE \[wc-x\] instgui install-go step=2 part=[0-9]+ \(attended Enter on the census screen\)
REQUIRE :: PINSTALL: refusal target=part4 reason=partition-too-small have=[0-9]+B need=[0-9]+B -> guard OK ::
REQUIRE \[wc-x\] instgui part-install part=[0-9]+ refused \(TooSmall\) — nothing was written
#
# ── 7. THE RED SPELLINGS ─────────────────────────────────────────────────────────────────────────
# Every FORBID below is a line this kernel CAN print — each is quoted from a live arm in
# `shell.rs`, `install/partition.rs` or `video/instgui.rs` — and none of them is a `-> FAIL`, which
# is the point: mbench's built-in forbids catch the failures that announce themselves, and these
# catch the ones that would read as success. A guard that stops refusing does not print FAIL; it
# prints a perfectly ordinary write line naming the wrong slot.
#
# 7a. A WRITE INTO A SLOT THAT MUST BE REFUSED. Four slots, four forbids, spelled out rather than
# `wrote part=[^2]` — a character class would also match a two-digit slot on a bigger fixture and
# read as a pass by accident (LAWS §5: wrong-lenient beats wrong-strict, but silent is neither).
FORBID :: INSTALLVERB: wrote part=0
FORBID :: INSTALLVERB: wrote part=1
FORBID :: INSTALLVERB: wrote part=3
FORBID :: INSTALLVERB: wrote part=4
# The GUI half of the same defect (`video/instgui.rs:1026`). Slot 4 is the one the typist commits
# on, so this is the exact line that would appear if the too-small guard stopped firing — including
# on its `-> PASS` arm, which no built-in forbid would ever look at.
FORBID :: INSTGUI: wrote part=4
#
# 7b. A SLOT THAT MUST BE REFUSED, OFFERED AS INSTALLABLE. The preview is what the operator reads
# before choosing, so a wrong offer is a defect one keystroke ahead of a wrong write.
FORBID :: INSTALLVERB: preview target=global:part0
FORBID :: INSTALLVERB: preview target=global:part1
FORBID :: INSTALLVERB: preview target=global:part3
FORBID :: INSTALLVERB: preview target=global:part4
#
# 7c. THE ONE INSTALLABLE SLOT, REFUSED. The mirror image, and the failure mode a refusal-heavy
# guard drifts toward: an installer that refuses everything passes every FORBID above and is
# useless. Scoped to `global:` because the post-write GUI census legitimately refuses `instgui:part2`
# — by then slot 2 carries the FAT volume burst 2 just wrote, which is the correct answer.
FORBID :: PINSTALL: refusal target=global:part2
#
# 7d. THE NEIGHBOURS. `untouched=[0-3]/4` is the shortfall spelling on THIS fixture (four
# neighbours); the `-> FAIL` arm of the same line is the built-in's. The other three are the paths
# where the verb cannot answer the question at all, and an unanswerable neighbour check must never
# be read as an untouched one (LAWS §5: an absence is evidence only if the producing path ran).
FORBID :: INSTALLVERB: neighbours untouched=[0-3]/4
FORBID :: INSTALLVERB: neighbours unmeasured
FORBID :: INSTALLVERB: neighbour part=[0-9]+ CHANGED across the install ::
FORBID :: INSTALLVERB: neighbour part=[0-9]+ post-read failed
#
# 7e. THE FIXTURE ITSELF FAILING TO PRESENT. Each of these makes the whole file vacuous rather than
# red — an empty census satisfies no REQUIRE but would leave the reader hunting for which pin broke
# — so they are forbidden by name and the replay says so in one line.
FORBID :: INSTALLVERB: census disks=0
FORBID :: INSTALLVERB: census disk=global gpt=unreadable
FORBID :: INSTALLVERB: census disk=global bind err=
# The act refused on the ONE slot it must accept. `install target=global:part2 err=` is the verb's
# nothing-written arm (shell.rs:8259) aimed at the installable slot — the same class as 7c, one
# layer down, and the line burst 2 would print if the write path regressed into the guard path.
FORBID :: INSTALLVERB: install target=global:part2 err=
