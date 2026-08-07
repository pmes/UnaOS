# RELAY

## → igpu — Flight 1a is MERGED (`9de5e3e3`). Metal just killed your EDID source. Rung 3 is now mandatory.

**Merged.** Gate 11/11 green, so the bounce is closed. Cut your next branch fresh from
trunk. Two things ride along as Flight 1b preconditions:
- `highest` is `let mut highest = 0` and **never assigned** (`igpu.rs:1026`) — it prints
  `00` on every exit and throws `unused_mut`. Truthful today (rung 0 is all there is), a
  lying witness the moment 1b adds a rung. Derive it.
- RUNBOOK transcript still omits the `dump_plane` and `ring=absent` lines.

**THE FINDING THAT CHANGES YOUR PLAN.** Boot AB flew the seat's EDID carry-through — the
bootloader path that was supposed to hand you the panel's EDID block. On metal it printed:

```
:: video: edid present=0 hdr=- sum=- native=- len=0 ::
```

**The rMBP's UEFI does not publish `EFI_EDID_ACTIVE_PROTOCOL` /
`EFI_EDID_DISCOVERED_PROTOCOL` on the GOP handle.** The code is correct; there is nothing
to carry. And the firmware's own buffer dies at `exit_boot_services`, so there is no later
route either. QEMU showed the same absence — for once it was the accurate model, not the
unrepresentative one.

So: **ladder rung 3 (DPA AUX → DPCD → EDID) is now MANDATORY and is your ONLY source of
panel timings and link rate.** Flight 1c cannot be built on a bootloader-supplied EDID —
do not plan around one. Rung 3 moves up: it is the gate on everything from rung 4 onward,
because without EDID you have no timings to program and no link rate to train at.

**Your assignment — Flight 1b, reads-only, per the ladder
(`docs/dev/GEMINI/video/iGUI/LADDER-igpu-bringup.md`):** bring up the DPA AUX channel and
read DPCD + EDID over it. Reads only, zero display writes, so it is safe to fly on
regression media. Deliverables: the AUX transaction path with TSC-bounded waits (no
`arch::ms()`); `DPCD_REV` and the EDID header + checksum as the pass predicate; the
128-byte block dumped or summarised on the wire; and — per the ladder — the witness must
be able to say NO (an absent or corrupt EDID prints and refuses, it does not fabricate a
default). Fix the two 1a leftovers above in the same branch.

Before handoff: `./arroyo check` green on all 11 legs. Four rounds running, the delivered
artifact has not compiled on its own target — this is the one thing that must change.
