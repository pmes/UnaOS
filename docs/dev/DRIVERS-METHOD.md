# DRIVERS-METHOD — how a driver is written here (the seat's working method, binding on every driver arc)

Peter, 2026-10-06, verbatim: "i'm not sure if you are tracking the methodology of driver writing or just the history but you need to
come up with a working method for yourself. i do know there has been more than one time where you locked yourself out of later steps
by closing out earlier steps you have not exhausted. you need a method to unravel the mysterious contexts in these blobs" — and, the
same day (R101): "there is only one way for you to improve on writing drivers and that's writing drivers."

This file is the method. Every driver brief names it; every driver design doc carries the sections in §6. It applies to the
Kepler (GPUBLIT/KF ladders), the Intel gen7 lane, the BCM4331 Wi-Fi ladder, USB (xhci/ehci/usbnet), HDA, SMC/gmux, and anything
else that talks to silicon through registers, rings, firmware or context blobs.

## 1. Where the method has failed us (named, so it is not repeated)

- **Rungs closed by inference.** bcm4331.md §S3 reads "SETTLED FROM EVIDENCE: reachability is a NO-OP" — settled by reading, and
  the ladder stopped there for a month with S4's wall "pinned" from a research pass that never flew. GPUBLIT (B371) wrote the
  Kepler channel table's hi word as a bare `0x400`, clearing the runlist field, because the encoding was a "RECALLED GUESS
  [EXT-UNPINNED]" committed as if settled; three flights timed out before GPUBLIT2 read nouveau's `gk104.c:77` and found it.
- **One change per boot with the walls already known.** GPUBLIT2 read three more walls (PTE aperture, RAMFC words, two init
  registers) and left them unapplied under the one-change rule. Each would have cost a flight. A flight that moves one word is a
  flight wasted when the dump could have discriminated all three.
- **Gates written to pass what was built.** VOLUMES passed `shown=EFI,UnaOS,boot` because the gate encoded the seat's reading,
  not the spec's or Peter's shape (R89). A witness checks the expected shape from the source, never the output we happened to get.
- **A premise never tested, no blob involved** (Peter: "it has happened with hardware we didn't need to use blobs with so there's
  definitely room for improvement"). NETFRAME (B368) built a four-rung RX stall ladder against the AX88179 — kicks, Stop Endpoint,
  dequeue resets — and USBNET8/9 then read the same wire and found 43 of 52 completions had arrived with no kick: the "stall" was a
  quiet LAN, and the ladder's resets were dropping the frames it was built to recover. The xHCI hub walk (`enumerate_downstream`) had
  no USB-net arm for the whole of the usbnet work, so a dongle behind any hub stayed unconfigured through several flights while the
  ladder looked at the ring. INPUTSTALL's 16 s "HID gap" was a test ladder spinning in the same pump loop as the HID pass. The
  users store mounted a USB stick because the global slot was first-come. In each case a fix was built before the fault's premise
  had been checked against what the wire could already show.
- **Blobs treated as opaque.** RAMFC, the instance block, falcon DMEM, the channel control page: words we did not understand were
  zero-filled or guessed instead of being copied from a state that worked and mutated one field at a time.
- **A status set by the image or the hand-back, never re-read from the wire** (STATUSBASELINE, B437, read the 188 cells
  STATUSTABLE grandfathered). Thirty-four rows were stamped "the arc flew" for boot 19 in one sweep; five had not
  (UnaFS read `unafs=unbuilt` all boot, ATTRSURF SKIPPED, SELFINSTALL never ran, GATE-CHARTER is a host gate). Nine rows
  stayed `done-unflown` after flight 24 flew them, and a dozen kept "unflown — wire `<the line we expect>`" after a flight
  printed a different one (KAT2 FAIL twice, `ppi=0 scale=1.0`, a decoder job that hangs). One cell quoted a witness with a
  number the capture does not carry (QUIETBOOT `bound=250` for the wire's `bound=2500`), and one "REFUTED" was a
  derivation (SO29). Every one is now a row of `docs/dev/STATUS.tsv` (ST43–ST265) or cites one; the list is
  `docs/dev/evidence/rmbp-1005/statusbaseline.md`.

## 2. The ground truth is a state that works

This holds for blob-free hardware too: the bench can read the AX88179's registers after Linux brought it up, the xHCI's slot and
endpoint contexts after Linux enumerated the hub and its children, the HDA codec's node graph after Linux set it, the SMC's
keys as macOS leaves them. A working state is the capture; the spec is what names its words.

Before writing a register, CAPTURE the device as something that works left it: UEFI GOP's Kepler state after the firmware
initialised the display, the iGPU after firmware, the AX88179 after Linux brought it up on the bench (the bench can read it over
the same bus), the BCM4331 after the firmware's own init. The capture is the FULL relevant state — every register in the block,
every word of every context blob — dumped word by word to the wire with offsets, and kept in `docs/dev/evidence/<round>/<device>/`
as the reference image. Our driver's job is then **reproduce → diff → minimal delta**: bring the device to the same state by our
own writes, diff against the capture, and the first differing word is the first bug. A word we cannot explain is copied from the
capture verbatim and marked `observed=<value> source=capture`, never zero-filled, never guessed.

## 3. The rung ledger — a rung has an exhaustion status, not a verdict by inference

Every ladder (one per device, in its `docs/dev/OS/...` doc) is a table of rungs, and every rung's STATUS is a row of
the ONE table, `docs/dev/STATUS.tsv` (`docs/dev/STATUS.md`; GATE-STATUS, `tools/status-check.py`): the doc cites
`ST<n>`, the table holds the flight and the whole wire line, and the gate finds the line on that flight's capture
byte for byte or fails. A rung carries:

- **hypothesis** (one sentence: what we believe the device needs and why — spec page, driver line, or capture word);
- **writes** (the exact registers/words it touches, with pre-image and restore);
- **discriminator** (the wire observation that CONFIRMS it and the one that REFUTES it — both named before the flight);
- **status** — its `ST<n>` row: `open`, `confirmed`/`refuted` (flight + quoted line, checked), `parked` (why, and what reopens it), `unflown`;
- **alternatives** (the other hypotheses for the same wall, each with its own discriminator).

**Rung 0 is the premise.** Every ladder's first rung is the fault's premise stated as a hypothesis with its own discriminator:
what the wire would show if the premise were FALSE (for NETFRAME: "if the link is merely quiet, completions arrive without kicks
and `needed=` reads 0"). Rung 0 is read from the wire ALREADY CAPTURED before any fix is written; a ladder whose rung 0 is `open`
builds measurement, not fixes. Where the wire cannot decide it, the first boot carries the measurement alone.

Rules: a rung is `confirmed` or `refuted` ONLY by a quoted wire line from a boot; a rung is never "settled" by reading. A rung may
be `parked` only with the alternatives listed and the reopening condition named — a `parked` rung is reopened the moment a later
rung's dump contradicts the parking reason. "Settled from evidence" without the evidence line quoted is forbidden wording.
Closing a wall means every alternative on it is `confirmed` or `refuted`, not that one was tried.

## 4. One boot answers many questions — design the dump as a decision tree

A metal boot is the scarce resource (R76/R78: QEMU proves nothing for these devices). So each boot carries EVERY non-interfering
candidate fix for the current wall and the walls already read behind it, together with a dump whose words tell them apart: for each
candidate, which word of the dump reads differently if it was the one. Interfering candidates (two writes to the same register,
or an ordering question) are sequenced across boots, with the first boot's dump chosen to decide the order. The design doc writes the
decision tree before the executor writes the code: "if `ramfc_get` > 0 then the fetch started and the wall is the PTE; if
`pfifo_intr` bit 8 then …". The bench reads the tree off the wire at the glass.

Destructive rungs (an upload that cannot be unwound, a reset that can wedge the device) are flown ALONE and LAST, flagged in the
row, with the pre-image restore proven on a non-destructive boot first (the wifi2 `unwind-selftest PASS` shape).

## 5. Triangulate every constant — two of three agree, all three cited

A written constant needs two of: the SPEC (Intel PRM volume and page; NVIDIA open-gpu-doc header and symbol; the USB/HDA/SD spec
section), a WORKING DRIVER's fact (nouveau file:line under R95 §2; Linux's driver for the chip under the same rule; the b43 spec
wiki for the BCM4331 — never its source), and the CAPTURE (the word as a working state held it). Each constant carries all it has
in a comment on its line: `// IVB PRM Vol2 p.123 · nouveau gk104.c:77 · capture f26 word 0x18`. A constant with ONE source is
written `[ONE-SOURCE: <which>]` and its rung's discriminator must be able to refute it. `[EXT-UNPINNED]` means "no source" and is
not allowed in a write path — a value with no source is read from the capture or the rung stays open.

## 6. The design doc's sections (every driver arc)

1. **Capture** — what working state was captured, when, by whom, where it lives; or "none yet" with the plan to get one.
2. **Rung ledger** — the table of §3, every rung with its status and alternatives; carried forward from the previous arc's doc,
   never restarted.
3. **This boot's tree** — §4's decision tree: the candidates carried, the dump words, the reading rules.
4. **Walls known and unapplied** — every wall already read from spec/driver/capture and not yet in code, with its source;
   nothing is "owed" without its source attached.
5. **Constants table** — every written constant with its two-of-three citations.
6. **Unwind** — the pre-image and restore path of every write; which rungs are destructive.
7. **What the next flight reads** — the exact lines, in reading order, and what each outcome means for the ledger.

## 7. The wire

Every rung prints the state BEFORE and AFTER its writes, and the engine's own status and interrupt words DECODED bit by bit from the
spec (`pfifo_intr=0x00000100 [bit8:runlist-event]`), never a bare `PASS`/`FAIL`. A witness `::` line states what the spec's expected
shape is and what was read; a gate that passes the output we happened to get is the R89 failure and is rewritten.

## 8. The seat's part

The seat does not close a rung the executor left open; the seat does not merge a driver arc whose doc lacks §6; the seat cuts the
next rung on the SAME ladder when a GPU/driver slot frees (R101), and briefs it with the previous doc, not a summary. The ladder
doc is the memory; the ledger row is the status; the method is this file.
