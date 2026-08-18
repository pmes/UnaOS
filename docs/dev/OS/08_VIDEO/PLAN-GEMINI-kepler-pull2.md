# PLAN — Kepler pull 2 (for the Gemini session): corrections + K-GPU-2 + K-GPU-3

**Format note:** this is a "big pull" — run all phases in sequence without waiting for
per-phase review. Each phase still ends with its gate; commit per phase on
`UnaOS-gemini` so review can bisect. Review happens once, at the end of the pull.
Metal verification stays at the arc boundary (Peter's bench).

**Standing gate for every phase:** `./arroyo check` AND `UNAOS_KEPLER=1 ./arroyo check`
green both arches; x86 QEMU suite (`./arroyo test`) zero FAIL/panic; REPORT updated.
**Flag every out-of-lane file** (anything outside `drivers/gpu/**`) in the commit
message and REPORT — the K-GPU-1 `memory.rs` touch was correct but unflagged.

## Phase C — corrections to K-GPU-1 (small, do first)
1. `arch/x86_64/memory.rs::map_mmio_window` — do NOT set `PTE_USER` on intermediate
   tables it creates; MMIO windows are kernel-only. (Out-of-lane touch: flag it.)
2. BAR1 sizing: check the BAR type bits before sizing as 64-bit (`(lo >> 1) & 0x3 ==
   0x2`); fall back to 32-bit sizing otherwise. Refuse probe on an IO-space BAR.
3. QEMU oracle line: on QEMU (no NV device) the driver must log a single clean
   "no Kepler device" line and touch nothing — add that witness in `:: ... ::` frame so
   serial-analyzer picks it up.

## K-GPU-2 — real scanout takeover (replaces the PDISPLAY heuristic scan)
4. Delete the 64 KB blind register scan. Read the active EVO/PDISPLAY head state to get
   the current scanout surface address, stride, and resolution (cleanroom: envytools
   register facts only — cite each register offset in a comment with its public-fact
   source; no nouveau code).
5. Allocate a new scanout surface via VramAllocator (respect the 32 MB GOP skip),
   copy the GOP framebuffer contents, and reprogram the head to the new surface —
   behind a NEW knob `UNAOS_KEPLER_TAKEOVER=1` (default off; UNAOS_KEPLER alone stays
   read-only probe). Both knobs wired in arroyo.
6. Fallback: if head state doesn't decode sanely (values fail bounds checks against the
   known GOP mode), log the raw values in a witness frame and abort takeover — never
   reprogram on a guess.
7. Oracle: QEMU = clean refusal witness. Metal (arc boundary, not yours): screen stays
   stable through the flip, kernel keeps writing text via the new surface.

## K-GPU-3 — PFIFO + first pushbuffer round-trip
8. PFIFO bring-up: reset/enable PFIFO via PMC, allocate a GPFIFO ring + userd in VRAM,
   bind a channel. Every register documented as in (4).
9. Submit NOP + a semaphore/fence write; poll (bounded, with timeout → witness + clean
   abort) for the fence value written back by the GPU to a VRAM address we chose.
   This is the "GPU executed our command" proof — the whole milestone.
10. Gate all of K-GPU-3 behind `UNAOS_KEPLER_FIFO=1` (default off).
11. Oracle: QEMU = clean refusal. Metal = fence value observed, logged in a witness
    frame (`:: kepler: fence <value> ::`).

## Explicitly OUT of this pull
- K-GPU-4 Falcon/PGRAPH microcode (next pull; spec already in this directory).
- Any touch of `video/framebuffer.rs` beyond read-only use of `base()` — if takeover
  needs more, STOP that phase, write the need into REPORT, continue with K-GPU-3.
- aarch64 paths (stubbed abort stays).

**End-of-pull deliverable:** one REPORT section per phase with oracle output, the list
of out-of-lane touches, and a short "what metal must verify" checklist for Peter's
sitting (staged media rules apply — the integrator stages, not you).
