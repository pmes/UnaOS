STATUS: BRIEF — awaiting Gemini proposal (`PROPOSAL-igpu-pull4.md`, this directory)

# BRIEF — iGPU pull 4: speak the gmux's real protocol, then decode who owns the panel

Coordinator-authored (2026-07-22, post sitting #8 boot 1).

## What sitting #8 proved (KEPLER-METAL-LOG.md)

- iGPU display engine incl. panel-power sequencing NEVER lit at any point
  from bootloader first instruction to kernel probe. Teardown-timing theory
  dead. The mux question is now load-bearing.
- The gmux responds on the INDEXED protocol (classic PIO absent). But our
  reads returned identical bytes for two different registers at each point
  (0x39/0x39 → 0x03/0x03): the handshake is likely incomplete and the bytes
  untrusted. The boot→kernel state CHANGE is real; the decode is not.

## What pull 4 must do (read-only)

1. **Full indexed read protocol** — the complete sequence with the ready/ack
   wait between index write and value read (and any port the ready poll
   lives on), cited as hardware facts (port numbers, register indexes,
   protocol steps — facts only, no GPLv2 code).
2. **Protocol self-test first** — read the gmux VERSION registers before
   anything else. A plausible version tuple = protocol proven; garbage =
   stop and report raw bytes. Only a proven protocol's SWITCH/POWER reads
   count as decode evidence.
3. **Registers to read once proven** — version, switch/display-owner, dGPU
   power state, plus whatever register the boot→kernel 0x39→0x03 movement
   actually was if identifiable. Read at Point-0 and kernel probe, same
   boot-info carry (ABI law: initializer + both builder sides + strings-proof
   in BOTH artifacts; the gate on the kernel handoff must keep its
   intel-ivb+x86_64 arch guard — dropped twice already).
4. **Decode table in the proposal** — what the switch-register values mean
   (which GPU owns the panel per value), cited, so the sitting reads as a
   verdict without a second derivation round.

Sentinels on every fallible read; `:: igpu:` prefix on every row; read-only —
no mux writes this pull, whatever the state says.

Metal owed: sitting #9.
