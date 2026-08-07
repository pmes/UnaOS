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
