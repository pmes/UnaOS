# x86-ptr.spec — THE RELATIVE POINTER, driven by a robot, so PTRINSTALL2's identity is on a gated wire.
#   QEMU gate:  ./arroyo test-ptr 120
#               → unaos/target/serial.log, replayed by `x86_spec_replay` (arroyo names this file in
#               code as X86_PTR_SPEC, so GATE-SPECROOTS resolves it GATED and it needs no RUN-BY).
#               The verb re-execs with UNAOS_PTRLANE=1 UNAOS_WC=1 UNAOS_QEMU_FULL=1, attaches
#               `-device usb-mouse,bus=xhci.0` through UNAOS_QEMU_EXTRA beside the builder's
#               `usb-tablet`, and drives the mouse with `scripts/qmp_type.py --pointer-kind rel`.
#
# WHY A FIFTH x86 SPEC (PTRLANE, rmbp seat, 2026-09-16; rmbp-ledger B117). PTRINSTALL2 (39a4c30f)
# moved the relative-report install to the producer — `x86_input_service` -> `x86_ptr_install`
# (main.rs tail) BEFORE the offer/fold — and instrumented it with six `wc`-only counters whose
# identity is `installs == reports` (every relative report the producer takes is installed once, by
# the producer, whoever holds focus). Its own gate could not measure that: every x86 QEMU leg carries
# a `usb-tablet` only (absolute -> `MouseAbsolute` -> `set_abs`, never the producer install), so the
# wire on every lane read the ZERO CONTROL, `:: PTRINSTALL: installs=0 reports=0 folds=0
# lag_max_ms=0 coalesced=0 drains=0 ::`, and engine.md §PTRINSTALL says so under "What is NOT
# measured". This file is the lane that measures it.
#
# WHAT THE TYPIST DOES, because every pin here is downstream of it. The mouse is a QEMU `usb-mouse`
# (HID boot mouse, `proto=2 relative`, decoded by drivers/xhci/mod.rs into `pal::Event::Mouse`
# deltas). After THREE witness lines — the mouse's own enumeration line, the desktop's theme line
# and the boot battery's last one-shot fixture — and XHCIKBD's measured 5 s settle, it injects
# PTRLANE_MOVES = 36 `rel` moves 50 ms apart (+-24 px on both axes, alternating; every step a
# distinct nonzero report, one HID report per QMP sync) and NO button. Not one click: PTRPRESS
# (`ptrpress_note`, armed on every `witness` build) stalls 900 ms on the first button edge and then
# swallows the first release after it — its injected fault, scored `-> FAIL` 6 s later unless the
# burst-and-tail shape recovers it — and a lone press would red this leg for a fixture reason. The
# moves are the measurement.
#
# THE COUNT IS LITERAL, and that is a choice with a cost stated. foreman (GATE-FOREMAN's dialect, the
# regex crate) refuses backreferences, so `installs=(\d+) reports=\1` cannot be written; the
# equality is pinned as the number the typist injects. A relative report lost anywhere in the pipe
# (QMP -> usb-mouse -> xHCI interrupt-IN -> `pal::EVENT_QUEUE` -> producer) therefore reds this leg
# BY NAME rather than hiding inside a `\d+` — which is the property the PTRPRESS and XHCIKBD
# fixtures exist to score, and a red here is read against theirs. `PTRLANE_MOVES` in arroyo's
# `test_x86_64` and the two 36s below are changed TOGETHER.
#
# NO `COMPLETE` MARKER HERE, for x86-install.spec's reason exactly: `x86_test_completion` reads
# x86-test.spec on every x86 leg, and `x86_spec_replay` refuses any capture that marker calls short.
# The late `:: PTRINSTALL:` line prints at 60 s of uptime, AFTER that marker, which is why the verb
# runs the full wall (UNAOS_QEMU_FULL=1): a graced exit would end the run with it unprinted.
#
# ── 1. TWO POINTERS ENUMERATED, and the relative one is the mouse ────────────────────────────────
# The first thing engine.md said this lane would measure: whether the xHCI HID path enumerates a
# second pointer beside the tablet at all. `proto=2 relative` is the boot-mouse shape the decoder
# reads as deltas; the tablet's twin reads `proto=0 absolute`. Both, so the tablet fixtures' device
# is still there and the mouse is not a replacement for it.
REQUIRE :: MOUSE-1: HID pointer detected vid:pid=0627:0001 proto=2 relative ep=0x8[0-9a-f] mps=\d+ interval=\d+ == witness ::
REQUIRE :: MOUSE-1: HID pointer detected vid:pid=0627:0001 proto=0 absolute ep=0x8[0-9a-f] mps=\d+ interval=\d+ == witness ::
#
# ── 2. THE DRIVER DELIVERED THE REPORTS (below the producer) ─────────────────────────────────────
# UI1-MOUSE's bounded witness: the first report and every 32nd. With 36 injected, the 32nd line is
# the proof that at least 32 relative reports left the xHCI decoder as deltas — a pin one layer
# under the counters, so a short count in §3 can be read as producer-side or pipe-side.
REQUIRE :: MOUSE-1: 1 reports, last dx=-?\d+ dy=-?\d+ buttons=0x00 == witness ::
REQUIRE :: MOUSE-1: 32 reports, last dx=-?\d+ dy=-?\d+ buttons=0x00 == witness ::
#
# ── 3. THE IDENTITY: installs == reports == 36, and the lag is small ─────────────────────────────
# The periodic line rides the `[schedx86] depth` 5 s gate once something relative has moved; at
# least one sample after the burst carries the final count. The late line is the one-shot at 60 s.
# `lag_max_ms` — the longest wait between the producer taking a relative report and the render
# service's drain of it — MEASURED 9 ms on this file's own gate (`./arroyo test-ptr 120`, 2026-09-16,
# every sample; one input-service pass is ~1 ms, and the drain is the render loop's period under
# TCG), and bounded below 50: five times the measurement, and still an order under the stall
# CHOP inferred (hundreds of ms). The late line is the one-shot at 60 s and reads the high-water.
REQUIRE \[ptrinstall\] installs=36 reports=36 lag_max_ms=\d+ coalesced=\d+ drains=\d+ folds=\d+
REQUIRE :: PTRINSTALL: installs=36 reports=36 folds=\d+ lag_max_ms=([0-9]|[1-4][0-9]) coalesced=\d+ drains=\d+ ::
#
# ── 4. FORBIDDEN: reports taken and not installed ────────────────────────────────────────────────
# The shape PTRINSTALL's tree printed structurally (installs=0 with reports>0 — the STOP on the
# wire) and the shape a producer that skips the install would print again. Either line.
FORBID :: PTRINSTALL: installs=0 reports=[1-9]
FORBID \[ptrinstall\] installs=0 reports=[1-9]
#
# ── 5. THE SHARED QUEUE WAS NOT RAIDED ───────────────────────────────────────────────────────────
# `ptrdead_selftest` pushes 192 relative events onto `pal::EVENT_QUEUE` and drains them itself; the
# typist holds until the boot battery is over, so neither fixture can steal the other's reports.
# The line is x86-wc.spec's, verbatim (skip is an honest value there and here; all-skip is not).
REQUIRE \[ptrdead\] backlog whole=(true|skip) nodrop=(true|skip) order=(true|skip) .* fpop12=-?[0-9]+ fpop3=-?[0-9]+ .* -> PASS
FORBID \[ptrdead\] backlog whole=skip nodrop=skip order=skip

# ── CONTRACT (SPECRUN, 2026-09-15) ──────────────────────────────────────────────────────────────
# A PINNED LINE IN THIS FILE IS CHANGED TOGETHER WITH THE KERNEL LINE IT PINS, IN THE SAME COMMIT —
# re-pinned to the new wording (naming the arc that changed it), or dropped with the reason stated.
# It is never worked around by teaching the kernel a SECOND spelling of the same witness.
# Tail-append is this repo's safe form: pinned lines are cited positionally.
