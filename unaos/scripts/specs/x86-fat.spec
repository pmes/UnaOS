# x86-fat.spec — the x86 U-chain over the sf FAT image.
#   QEMU gate:  UNAOS_FATIMG=sf ./arroyo test 150   → unaos/target/serial.log
#   (the battery's "x86 test-fat sf 300" step asserts the same chain)
#
# Default (irqstorage knob-OFF) build: 22 `-> PASS` verdicts, no MISSION line
# (the sf fixture path skips the storage demo banner). The knob-ON build adds
# the STOR-1 lines — OPTIONAL below so both builds pass this spec.

# --- the aggregate: 22 fixture verdicts --------------------------------------------
COUNT 22 -> PASS

# --- ring-3 bring-up + fault plumbing ----------------------------------------------
REQUIRE U2-0c: self-NMI taken on IST -> PASS
REQUIRE U2-0c: canonical-rcx guard refuses
REQUIRE U1a: user exited ok
REQUIRE U1b: fault isolation.*-> PASS
REQUIRE U2-0a: TF\+SYSCALL survived -> PASS
REQUIRE U3: per-process CR3 isolation
REQUIRE U3.5: ring-3 preemption.*-> PASS
REQUIRE U2: loaded program exited ok -> PASS

# --- the capability chain ----------------------------------------------------------
REQUIRE U4x: x86 process model.*-> PASS
REQUIRE U5x: x86 capabilities.*-> PASS
REQUIRE U6x: x86 general object table.*-> PASS
REQUIRE U6bx: x86 real File handles.*-> PASS
REQUIRE U7x: cross-process transfer.*-> PASS
REQUIRE U8x: revocation trees.*-> PASS

# --- the FAT write/lifecycle chain -------------------------------------------------
REQUIRE U9x: real File writes.*-> PASS
REQUIRE U10: file growth.*-> PASS
REQUIRE U10c: file create.*-> PASS
REQUIRE U10d: file delete.*-> PASS
REQUIRE U11x: open-file lifecycle.*-> PASS
REQUIRE U11m2:.*-> PASS
REQUIRE U6gx: UnaFS owner/grants.*-> PASS

# --- STOR-1 knob-ON (irqstorage) lines: present only on that build ------------------
OPTIONAL bx-blockreq: PASS
OPTIONAL S3: synchronous write-through.*-> PASS

# --- forbidden: any CPL-0 fault diagnostic (defaults -> FAIL / FAIL :: / PANIC are
# --- always on; ring-3 intentional faults print nothing — verified on a clean run) --
FORBID EXCEPTION:

# --- STOR-1 S5 shared-backing witness (knob-ON only — the default sf run is knob-OFF, so
# --- OPTIONAL here; a dedicated knob-on spec is a ledgered tooling candidate. Knob-on FAT
# --- ideal PASS count = 25 incl. S5; the S5 FAIL text evades arroyo's detector, so witness
# --- PRESENCE is the real gate on knob-on runs — seat fold at the S5 merge, 2026-07-11) ----
OPTIONAL S5: cross-process read serves LIVE shared backing

# --- STOR-1 knob-ON uncounted witnesses (S4-mf2 / S4-race / S6-witness / S7-openany): they use
# --- the `— witness OK ::` idiom (NOT `-> PASS`), so they do NOT add to the COUNT above and the
# --- default FORBIDs (`-> FAIL`, `FAIL ::`, `PANIC`) do NOT catch their `FAIL — …` failure text.
# --- So on knob-on runs, PRESENCE (OPTIONAL) + an explicit per-witness FORBID on the FAIL variant
# --- is the gate. Knob-off the sf run omits them entirely, so all stay silent — both builds pass.
# --- (S7 = STOR-1 S7: an open of an arbitrary on-disk file, off the pre-stage set — 2026-07-12.) --
OPTIONAL S4-mf2: RW open of staged code
OPTIONAL S4-race
OPTIONAL S6-witness: NAMESPACE cross-core RMW
OPTIONAL S7-openany: a non-staged on-disk file
# --- (S8 = STOR-1 S8: a RW open of an arbitrary on-disk file overwrites live, overwrite-only — 2026-07-15.) --
OPTIONAL S8-write: a non-staged on-disk file
# --- (S9 = STOR-1 S9: a RW dynamic on-disk file GROWS past EOF live, bounded per-write/per-file — 2026-07-16.) --
OPTIONAL S9-grow: a dynamic on-disk file
FORBID S4-mf2 FAIL
FORBID S4-race FAIL
FORBID S6-witness FAIL
FORBID S7-openany FAIL
FORBID S8-write FAIL
FORBID S9-grow FAIL

# --- DMG-REFUSE: the SYS_WIN_PRESENT_ROWS(33) refusal arms (-EBADF / -EACCES / -EINVAL), 2026-08-04.
# --- REQUIRE, not OPTIONAL: this witness is headless-complete — two inline ring-3 blobs, no block
# --- device and no panel — so it runs on EVERY x86 QEMU boot, and its ABSENCE is a gate failure. That
# --- is deliberate: a FORBID alone cannot catch silence, and a witness that can vanish quietly is the
# --- exact defect that made the storage witnesses above untrustworthy. Uses the `— witness OK ::`
# --- idiom, so it does NOT add to the `COUNT 22 -> PASS` above.
# --- The third line is the loud-absence rule: every skip path in the launcher prints `NOT RUN`, and a
# --- skip must fail the gate rather than pass it silently.
# --- VERIFICATION PROVENANCE, stated because it is NOT complete: these three lines were proven against
# --- a real capture with `mbench --replay` (green log -> exit 0; a deliberately-reddened log -> exit 1,
# --- caught by the FORBID), but on the `./arroyo test` path, NOT `test-fat` — this host has no `mtools`,
# --- so `make-fat-img.sh` aborts at `mcopy` before QEMU ever starts. The REQUIRE is sound by
# --- construction (same kernel, same `witness` build; the launcher chains off `winx7_launcher` inside
# --- `u8x_launcher`, which does NOT gate on a block device, so the FAT path runs a SUPERSET of the
# --- chain) — but it has not been executed on this exact gate. First runner of `test-fat sf`: if this
# --- line is the only red, suspect this note before suspecting the kernel.
REQUIRE DMG-REFUSE:.*19/19 probes.*witness OK
FORBID DMG-REFUSE FAIL
FORBID DMG-REFUSE:.*NOT RUN

# --- GR18 WXN-x86 / WXAUDIT: THE KNOB-INDEPENDENT HALF OF THE GR18 WITNESS ROUND ---------------
# --- x86-witness.spec IS THE MASTER COPY of the eight directives below, and of the reasoning
# --- behind each one (why the two WXN terminals earn their lines when the REQUIRE already
# --- excludes them; why the WXAUDIT REQUIRE names its fields in ORIGINAL ORDER and stops at
# --- `l3=`; why WXAUDIT-NXE gets no FORBID; why `LEAF NX-ONLY` is deliberately NOT widened into
# --- the FBWC pattern yet). THE PATTERNS HERE ARE BYTE-IDENTICAL TO THAT FILE'S AND EDITS MUST BE
# --- PAIRED — a regex that drifts between the two reads as "the metal gate and the QEMU gate
# --- disagree about the map", which is the one thing this block exists to make impossible.
# --- WHY THEY BELONG IN THIS FILE, which is the whole justification for the duplication. Every
# --- other GR18 wire is knob-gated — SMC needs `UNAOS_SMC`, `[wc-g] paygo` needs `UNAOS_WCG_PAYGO`,
# --- M8/igpu-blt need their own knobs — and x86-witness.spec's SCOPE says outright that it asserts
# --- a paygo-armed, logts-prefixed, witness-armed boot AND NOTHING ELSE. These four wires are the
# --- exception: `wxn_pdpt_sweep` (memory.rs) and `wx_audit_report` (memory.rs) are called
# --- UNCONDITIONALLY from `arch::init` (arch/x86_64/mod.rs:46 and :50 — no `#[cfg]` on either the
# --- functions or the call sites), and `wxn_nxe_report` (smp.rs:108) is called from `start_aps` on
# --- EVERY exit path including the two uniprocessor ones. So a DEFAULT x86 build prints all four,
# --- and until now the only spec in the tree that read any of them was one that a default build
# --- cannot satisfy. That is a coverage hole shaped exactly like §10h's: the instrument is on every
# --- boot and nothing goes red when it stops printing.
# --- MINIMUM BUILD GENERATION — `32724cb4` (2026-08-06). These lines do not exist on an image built
# --- before it (`a0a2d163` introduced the sweep and `WXAUDIT-NXE`; `32724cb4` added the `-> VACUOUS`
# --- verdict, the WXAUDIT `l1/l2/l3` histogram and `WXN-FBWC`), so a replay of an older capture reds
# --- them because the kernel could not print them, not because the boot was sick. This is a SECOND
# --- scope axis on this file, orthogonal to the irqstorage knob split above.
# --- VERIFICATION PROVENANCE, and it is stronger than the DMG-REFUSE note above rather than weaker.
# --- All eight were replayed green against TWO metal rMBP captures (bootW / bootX, 2026-08-06) and
# --- against a real x86 QEMU capture from THIS host — a `./arroyo test` run, kept as
# --- `~/unaos-bench/scratch/qemu-x86-gr18.log` because `unaos/target/serial.log` is overwritten by
# --- the next run (`:: WXN-x86: … wp=1 -> SWEPT ::`, `l1=0 l2=524281 l3=3584`, `cores=6 nxe=6 … -> PASS ::`,
# --- `fb=0x80000000 lvl=2 … -> LEAF BIT-IDENTICAL ::`). The `test-fat sf` variant is still unproven
# --- HERE for the same `mtools` reason, but the construction argument is far tighter than
# --- DMG-REFUSE's: these four lines are emitted from `arch::init`, BEFORE the framebuffer fixture
# --- chain, before `u8x_launcher`, before any block device exists. The FAT image cannot reach them.
# --- Note on the COUNT above: `WXAUDIT-NXE`'s and `WXAUDIT-0`'s verdicts both end `-> PASS`, so they
# --- do add to the `COUNT 22 -> PASS` tally — harmless, because mbench COUNT is `>= n`, never `== n`.
REQUIRE :: WXN-x86: ehdr=0x[0-9A-F]+ img=\[0x[0-9A-F]+,0x[0-9A-F]+\) .*pdpt_seen=[0-9]+ nx_set=[0-9]+ .*residue_leaves=[0-9]+ .*wp=[0-9]+ -> SWEPT ::
FORBID :: WXN-x86: .*-> VACUOUS ::
FORBID :: WXN-x86: .*-> REFUSED
REQUIRE :: WXAUDIT x86: leaves=[0-9]+ user=[0-9]+ user_WX=[0-9]+ kern_WX=[0-9]+ \([0-9]+ MiB\) tables=[0-9]+ nxe=[0-9]+ walk=[0-9]+kcyc l1=[0-9]+ l2=[0-9]+ l3=[0-9]+
FORBID :: WXAUDIT x86: .* TRUNCATED ::
REQUIRE :: WXAUDIT-NXE: cores=[0-9]+ nxe=[0-9]+ nxe_mask=0x[0-9A-F]+ wp=[0-9]+ wp_mask=0x[0-9A-F]+ -> PASS ::
REQUIRE :: WXN-FBWC: fb=0x[0-9A-F]+ lvl=[0-9]+ e=0x[0-9A-F]{16} pat=[0-9]+ pcd=[0-9]+ pwt=[0-9]+ w=[0-9]+ fx=[0-9]+ -> LEAF BIT-IDENTICAL ::
FORBID :: WXN-FBWC: .*-> SKIPPED ::

# --- FATVERB (GR24) — the shell's file verbs bind the PROGRAM SOURCE, and a write verb asks first --
# --- These two legs run from the storage-ready service pass, NOT with the other shell fixtures at
# --- main.rs step 5: an earlier cut of them fired before `pci::init` and the USB publish and passed
# --- on `handles=global=absent sdhc=absent`, i.e. on all-false inputs, which is dead rather than
# --- quiet. Each leg DRIVES A REAL VERB (`ls_path`; `fs_rm` against an unresolvable name, so the
# --- gate is exercised and nothing is mutated) and asserts the binding that verb STAMPED, requiring
# --- the stamp counter to advance — so a verb that stops routing through the shared mount helpers
# --- fails the leg instead of inheriting an earlier answer. Falsified both ways on this host
# --- (2026-08-09): armed, both PASS; with `ls_path`/`fs_rm` bypassing the helpers,
# --- `readvol -> FAIL (… read_ran=false read=never …)` and `writegate -> FAIL (gate_ran=false …)`.
# --- SCOPE: `witness` only. They need no FAT image and no `sdhcblk` — the summary line's census
# --- reads `sdhc=unbuilt` on this spec's build and the legs still carry content, because the
# --- handle-agreement and counter halves are independent of which handles exist. The wait is
# --- BOUNDED (30 s, wcx's threshold): a boot that never gets a block device still emits all three
# --- lines with an empty census, so these REQUIREs cannot go red for want of a card.
# --- MINIMUM BUILD GENERATION — the FATVERB commit (2026-08-09); older images cannot print them.
REQUIRE :: TSTE: fatverb\.readvol -> PASS ::
REQUIRE :: TSTE: fatverb\.writegate -> PASS ::
REQUIRE :: \[fatverb\] storage witness: exec=[a-z-]+ read=[a-z-]+ gate=[a-z-]+ waited=[0-9]+ms handles=global=(present|absent) sdhc=(present|absent|unbuilt) ::
# SOCK-4 (added 2026-08-19): the transferable-sockets fixture flaked 6x visible only by eye —
# the spec never gated it (FIXTURE_FLAKES.md §1b). The gen-rebind fence fix (53c630eb) closed
# the slot_reused variant; this REQUIRE makes any recurrence a red gate, not a scrollback find.
REQUIRE :: SOCK-4: transferable sockets — grantee received \+ round-tripped the moved socket, grantor's migrated-away handle -EACCES, gen-rebind rejected, teardown clean -> PASS ::

# --- BT-BOND M1 / HOLOCRON (added 2026-08-21): the classed-record store's four witnesses ---------
# --- SCOPE, and why these are OPTIONAL here rather than REQUIRE. This file's contract, stated at
# --- the top, is that BOTH the default (knob-off) and the knob-on builds pass it. `holocron` is
# --- DEFAULT OFF, so a REQUIRE would red the default `UNAOS_FATIMG=sf ./arroyo test 150` gate on
# --- every run — the exact failure the STOR-1 block above documents for its own knob-gated lines.
# --- So this file uses the idiom that block established: OPTIONAL presence (so a knob-on run's
# --- witnesses are recognised and a knob-off run stays silent and green) PLUS an explicit per-
# --- witness FORBID on the FAIL variant, which is what actually gates. The hard REQUIREs for the
# --- armed configuration live in `x86-holocron.spec`, whose whole scope is a
# --- `UNAOS_HOLOCRON=1 ./arroyo test-fat sf 150` capture.
# --- The BT-BOND design asks for a REQUIRE in THIS file; that is the one place the design and this
# --- tree could not both be satisfied, and the split above is the resolution — see usb_xhci.md §28.
# --- All four use the `-> PASS ::` idiom, so they DO add to the COUNT above on a knob-on run;
# --- harmless, because mbench COUNT is `>= n`, never `== n`.
# --- MINIMUM BUILD GENERATION — the BT-BOND M1 commit (2026-08-21). Older images cannot print them.
# --- VERIFICATION PROVENANCE: all six directives were replayed against a real capture from THIS
# --- host — `UNAOS_HOLOCRON=1 ./arroyo test-fat sf 150`, kept as
# --- `~/unaos-bench/scratch/qemu-x86-btbond-m1.log` because `unaos/target/serial.log` is overwritten
# --- by the next run — and against a knob-OFF capture of the same gate, where all four stay silent.
OPTIONAL :: \[hcron\] framing fixture: [0-9]+/[0-9]+ legs .*-> PASS ::
OPTIONAL :: \[btbond\] codec fixture: [0-9]+/[0-9]+ legs .*-> PASS ::
OPTIONAL :: \[hcron\] store round-trip: .*-> PASS ::
OPTIONAL :: \[btbond\] store round-trip: .*-> PASS ::
FORBID \[hcron\] framing fixture: .*-> FAIL ::
FORBID \[btbond\] codec fixture: .*-> FAIL ::
FORBID \[hcron\] store round-trip: .*-> FAIL ::
FORBID \[btbond\] store round-trip: .*-> FAIL ::
# --- The flush guard: a store write from inside the EHCI service pass is the bug the whole seam
# --- exists to avoid, so its refusal witness firing at all is a gate failure, knob-on or knob-off.
FORBID \[hcron\] flush REFUSED
