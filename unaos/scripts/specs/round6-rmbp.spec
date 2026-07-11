# round6-rmbp.spec — the ROUND 6 opening attended rMBP bench, Boot 1.
#   Source witness list: ~/.claude/plans/unaos-round6-rmbp-bench.md §Boot 1.
#   Build:  UNAOS_VIDEOBENCH=1 UNAOS_USBDEBUG=1 ./arroyo esp-x86 (knob-ON, built LAST)
#   Run:    ./arroyo mbench --follow ~/rmbp-serial.log --spec scripts/specs/round6-rmbp.spec --timeout 300
#   ⚠ x86 serial is TX-only (FTDI bulk-IN is a kernel stub) — capture + assert ONLY,
#   never --inject on this platform (mbench refuses it).
#
# REQUIRE = captured on metal before (the 2026-07-10 STOR-1 bench, knob-ON build).
# PENDING = never captured on metal (S4 + VPERF/UI landed after that bench) — reported
# as ⏳, never fails; promote to REQUIRE after this bench first captures them.
#
# NOT serial-assertable on Boot 1 (no kernel witness line exists — reported to the
# seat, not invented here):
#   - MF2 HELLO.BIN RW-refusal (sys_open -EACCES on immutable code) has no serial
#     verdict line; "HELLO.BIN intact" is proven indirectly (U2 loads + runs it).
#   - The S4 cross-process delete-at-last-close RACES are live-interleave behavior;
#     the mechanism lines (U11m2 + the S4 headline) assert the machinery, the race
#     outcome itself is the bench's attended open question.
# Boot 2 (GUI scale-2 / vug / pulse) is eyeball+photo — out of serial scope.

# --- console-cap slice (boot-automatic; TX-only console) ----------------------------
REQUIRE U2.5-0: DR7 cleared
REQUIRE U2.5: FTDI console up
REQUIRE U2.5: FTDI TX mirror -> PASS

# --- STOR-1: service task + transfer-IRQ I/O re-confirm on the FIXED code -----------
REQUIRE STOR-1: storage service task live on cpu
REQUIRE bx-blockreq: PASS
REQUIRE S3: synchronous write-through.*-> PASS

# --- the full U-chain, knob-ON (all captured 2026-07-10) ----------------------------
COUNT 24 -> PASS
REQUIRE U2-0c: self-NMI taken on IST -> PASS
REQUIRE U2-0c: canonical-rcx guard refuses
REQUIRE U1a: user exited ok
REQUIRE U1b: fault isolation.*-> PASS
REQUIRE U2-0a: TF\+SYSCALL survived -> PASS
REQUIRE U3: per-process CR3 isolation
REQUIRE U3.5: ring-3 preemption.*-> PASS
REQUIRE U2: loaded program exited ok -> PASS
REQUIRE U4x: x86 process model.*-> PASS
REQUIRE U5x: x86 capabilities.*-> PASS
REQUIRE U6x: x86 general object table.*-> PASS
REQUIRE U6bx: x86 real File handles.*-> PASS
REQUIRE U7x: cross-process transfer.*-> PASS
REQUIRE U8x: revocation trees.*-> PASS
REQUIRE U9x: real File writes.*-> PASS
REQUIRE U10: file growth.*-> PASS
REQUIRE U10c: file create.*-> PASS
REQUIRE U10d: file delete.*-> PASS
REQUIRE U11x: open-file lifecycle.*-> PASS
REQUIRE U11m2:.*-> PASS
REQUIRE U6gx: UnaFS owner/grants.*-> PASS
REQUIRE xHCI: Disk

# --- S4 first metal (landed 2026-07-10 late — never captured on metal) --------------
PENDING S4: grow/create/delete SYNCHRONOUS in-syscall.*-> PASS

# --- VPERF (this boot DECIDES VPERF-WC — all landed post-bench, never on metal) -----
# THE DECIDER: eff=WC → VPERF-WC dead; eff=UC (or any non-WC) → VPERF-WC triggers
# bench-FIRST. Record the fbmem line VERBATIM in the seat baton either way.
PENDING vperf: fbmem mtrr=.*pte=.*pat=.*eff=.*fb=
# Expect BOTH GPUs named (GT 650M scans out via gmux default; HD 4000 also present).
PENDING vperf: display.*class
PENDING vperf: display.*owns fb
# WATCH EXPLICITLY: the one-shot try_lock attach can silently skip under AP
# contention (ledgered review note) — no line = shadow not live.
PENDING fbcon: cached-RAM shadow attached
# QEMU showed scenario vread 1.22 GB→0; the metal claim is vread=0.
PENDING vperf: scenario.*vread=0
PENDING vperf: scenario done

# --- zero faults (defaults -> FAIL / FAIL :: / PANIC always on) ----------------------
FORBID EXCEPTION:
