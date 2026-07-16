# round6-rmbp.spec — the ROUND 6 attended rMBP bench, Boot 1.
# 2026-07-11 POST-BENCH: ALL former PENDINGs fired on the real rMBP (mbench 29/29 twice,
# pristine card + pristine re-confirm) and are PROMOTED to REQUIRE; COUNT 24→25 (S5 counted).
# A 24-with-zero-FAILs remains the ledgered ±1 console drop — eyeball before suspecting.
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
COUNT 25 -> PASS
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
REQUIRE S4: grow/create/delete SYNCHRONOUS in-syscall.*-> PASS

# --- S4/S6/S7 witnesses (landed post-round-6; FIRST captured on metal 2026-07-12 round-9
# --- bench — promoted straight to REQUIRE). S6 = the true-SMP namespace-lock proof the S6
# --- review deferred; S7 = arbitrary on-disk open + the negative owned.bin ACL leg; S4-mf2 =
# --- staged-code RW refusal; S4-race = both close+teardown synchronous-delete legs. --------
REQUIRE S4-mf2:.*refused -EACCES.*witness OK
COUNT 2 S4-race.*witness OK
REQUIRE S6-witness:.*locked 240000/240000 intact.*witness OK
REQUIRE S7-openany:.*owner ACL not bypassed.*witness OK
# --- S8 = STOR-1 S8: a RW open of an arbitrary on-disk file overwrites live IN PLACE (MF2 intact).
# --- ✅ METAL-CONFIRMED 2026-07-15 (attended rMBP bench, LC-x86): fired on 3 boots (incl. a genuine
# --- power-cut pair, both 43/43 clean, boot-5 slice-asserted standalone) — PROMOTED to REQUIRE.
# --- (STOR-1 S9 retired S8's past-EOF-returns-0 leg — growth is now the S9-grow witness's job — so the
# --- S8 message no longer says "overwrite never grows"; the regex tracks the in-place overwrite claim.)
REQUIRE S8-write:.*overwrote a live 16-byte pattern in place.*witness OK
# --- S9 = STOR-1 S9: a RW dynamic on-disk file GROWS past EOF live (bounded per-write/per-file).
# --- ✅ METAL-CONFIRMED 2026-07-16 (attended rMBP sitting, LC-x86: extended 0 -> 96 bytes real
# --- alloc+chain, readback, -ENOSPC ceiling, MF2 intact; log rmbp-serial-2026-07-16-sitting.log)
# --- — PROMOTED to REQUIRE, exactly as S8-write was.
REQUIRE S9-grow:.*extended 0 -> .* bytes.*witness OK

# --- VPERF (this boot DECIDES VPERF-WC — all landed post-bench, never on metal) -----
# THE DECIDER: eff=WC → VPERF-WC dead; eff=UC (or any non-WC) → VPERF-WC triggers
# bench-FIRST. Record the fbmem line VERBATIM in the seat baton either way.
REQUIRE vperf: fbmem mtrr=.*pte=.*pat=.*eff=.*fb=
# VPERF-WC (landed post-round-6, never on metal): the fb mapping is now retyped
# Write-Combining (PAT PA4). At the NEXT attended bench the readout must flip from
# the round-6 eff=UC to eff=WC (pat=WC, PTE PAT bit set) and the retype line fires.
# 2026-07-12 round-9 bench: BOTH fired on the real rMBP (eff=UC → eff=WC) — PROMOTED to REQUIRE.
REQUIRE vperf: fbmem .*pat=WC eff=WC
REQUIRE x86 fb-wc: retyped .* leaf\(s\) WC \(PAT PA4\)
# Expect BOTH GPUs named (GT 650M scans out via gmux default; HD 4000 also present).
REQUIRE vperf: display.*class
REQUIRE vperf: display.*owns fb
# WATCH EXPLICITLY: the one-shot try_lock attach can silently skip under AP
# contention (ledgered review note) — no line = shadow not live.
REQUIRE fbcon: cached-RAM shadow attached
# QEMU showed scenario vread 1.22 GB→0; the metal claim is vread=0.
REQUIRE vperf: scenario.*vread=0
REQUIRE vperf: scenario done

# --- zero faults (defaults -> FAIL / FAIL :: / PANIC always on) ----------------------
FORBID EXCEPTION:

# --- STOR-1 S5 (landed 2026-07-11, never on metal — the bench build is knob-ON so these
# --- fire on Boot 1; promote to REQUIRE after first capture) ---------------------------
REQUIRE S5: cross-process read serves LIVE shared backing
# The service-task boot line gained ", PRIO_HIGH" — the existing REQUIRE prefix still matches.
# u6gx's unchanged PASS line is now ALSO the deadlock-closure witness knob-on-FAT (a u6gx
# hang/FAIL here = the S5 scheduler fix or the service core's LVT timer, NOT plain ACL).
