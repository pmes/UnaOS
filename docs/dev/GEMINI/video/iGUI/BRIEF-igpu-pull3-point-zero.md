STATUS: BRIEF — awaiting Gemini proposal (`PROPOSAL-igpu-pull3.md`, this directory)

# BRIEF — iGPU pull 3: Point-0 and the gmux — who scans the GOP console?

Coordinator-authored (2026-07-22, post sitting #7 + Fox DP_A cross-check).
Derivation/implementation proposal first; full-knob land-review law
(strings-proof in kernel.elf AND BOOTX64.EFI).

## The paradox to split

Canon (KEPLER-METAL-LOG #7): iGPU scanout reads fully dead at all three trace
points — including PRE-ExitBootServices — and Kepler's display engine reads
equally dead, yet the GOP text console is visibly on the panel during
Option-boot. Two candidate explanations:
(a) firmware tears scanout down BEFORE our Point-1 read (our bootloader entry
    is later in the boot than we assume — panel visibly blacks out almost
    instantly after selection, consistent);
(b) scanout state on this part lives in registers other than the ones we
    decode (wrong block, not wrong timing).

## What pull 3 adds (read-only, extends the existing trace)

1. **Point-0**: the same 8-register snapshot as the FIRST action in the
   bootloader's `main()` — before logging init, before any UEFI protocol
   work. If Point-0 is live where Point-1 was dead, (a) wins and we know the
   teardown happens inside our own bootloader's setup window. Carry it as a
   third boot-info array (`igpu_trace_0`) — same feature gate, same ABI
   discipline (initializer + builder both sides; the pull-2 land-review
   record lists every wiring hole to not repeat).
2. **gmux status readback**: read-only dump of the Apple gmux controller
   state (index/data ports in the 0x7xx IO range; switch-state and power
   registers) at Point-0 and at kernel probe time — which GPU does the mux
   currently feed the panel from? Hardware facts (port numbers, register
   indexes) are citable from any source per the license rule; no GPLv2 code
   or function bodies. If the mux says "dGPU" while both engines read dead,
   hypothesis (b) points at the KEPLER side decode, not Intel.
3. **Pipe-adjacent evidence for (b)**: alongside PIPEACONF, dump the pipe's
   PLL/clock enables and panel-power status (PP_STATUS/PP_CONTROL) — an
   engine can't be scanning with panel power off; PP_STATUS alone may settle
   whether the eDP path was EVER up post-selection.

## Notes

- Everything read-only; no gmux writes, no register pokes.
- Keep the `:: igpu:` prefix on every new serial row (bench filter law).
- Failed-read sentinel discipline: any probe that can fail must emit
  0xBAD0BA20-style sentinels, never ambiguous zeros.

Metal owed: sitting #8 (rides with kepler pull 9).
