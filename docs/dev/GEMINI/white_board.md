# WHITE BOARD — 2026-08-09 (GR23, second pass)

Questions for Peter, each with the background to answer it. Nothing else lives here.
**Q1–Q5 of this morning are ANSWERED and off the board** — the kernel shell is being shut down and
routed through midden, the radios/3D jobs are running, wifis4a is being conditioned for merge, the
Crispy kit is "stolen from pi, sync later" (the law now rests on the in-tree triangulation, which
was verified bit-exact), and ceramic + paper-on-more-surfaces is building.

---

## ✅ Q6 — ANSWERED: connect to Peter's own speaker, "MEGABOOM". Being built now.

> *"can you program it to find my speaker id 'MEGABOOM'?"*

So the filter is BY ADVERTISED NAME (not BD_ADDR — he does not know its address, and a name filter
is what makes it reproducible for him): `BT_L3_PEER_NAME = Some("MEGABOOM")`, case-insensitive
substring, reusing L2's existing AD Local Name decode rather than a second parser. Nothing else in
the room gets connected to. ⚠ ONE THING MAY COME BACK AS A QUESTION: if the MEGABOOM puts its name
only in a SCAN RESPONSE rather than in the advertisement, a PASSIVE scan never sees it — and going
active means TRANSMITTING, which is a separate decision and will be brought back here rather than
taken silently.

**Original question, kept for the record:**

## Q6 (original) — BT-L3 will briefly CONNECT TO A STRANGER'S DEVICE. Is that acceptable to fly?

L3 is written and in review. It scans, picks **the first `ADV_IND` peer it hears**, opens a
link-layer connection, and releases it within milliseconds. No pairing, no bonding, no data read,
no service discovery — a connect and a clean disconnect. But in a populated room (a café, an
apartment building, anywhere near other people) a `UNAOS_BT=1` boot will make an unsolicited
connection to **someone else's watch, earbuds, fitness band or phone**. From their side it is a
brief unexplained connection from an unknown device, and it may show up in their logs or briefly
occupy their peripheral's single connection slot.

Options, cheapest first:
  a) **Fly as-is** — it is a millisecond link-layer connect, no data exchanged.
  b) **Restrict the peer by address** — connect only to a BD_ADDR you name (your own test device),
     refusing everything else. Costs one constant and makes the arc a lab instrument rather than
     something that touches strangers.
  c) **Restrict by OUI or by a name prefix** in the advertisement — a middle ground.
  d) Only fly it somewhere with no other radios.

⚠ **THE REVIEW SHARPENED THIS — it is not always "a few milliseconds".** L3 BOUNCED on a cancel
race whose *more probable* ordering leaks the link: when the connection establishes just as the
wait expires, the cancel returns `Command Disallowed` while the real `Connection Complete` sits
ahead of it in the queue and is discarded, so **UnaOS holds an open link to that device for the
rest of the boot** (the event endpoint is then quiesced, so no disconnect is ever read — it dies
only on the peer's supervision timeout or a power cycle) while the capture certifies
`left_outstanding=none`. That is fixed. But it means the failure mode was: indefinitely hold a
stranger's peripheral — and the sharp case is **another machine's BLE keyboard or mouse, which a
CONNECT_IND takes away from its owner for the duration of the link.** An independent reviewer
reached your Q6 question on its own, from the code.

The allow-list and an RSSI floor are being built now; the allow-list DEFAULTS TO OFF pending your
ruling, as a single obvious line. I would take (b) for anything flown outside a controlled bench —
it answers the same engineering question (does the connect path work end to end?) with nobody
else's hardware involved. **Your call; it is a one-constant change either way and nothing flies
until you rule.**

## ✅ Q7 — ANSWERED: keep both lanes, and be a drill sergeant about it.

> *"try being more drill sergeant like 'what's your major malfunction gemini?!?!?!'"*

Adopted in RELAY.md as of this pass. First application: kepler's round-4 plan is CORRECT on all
eleven bounce items — and its verification plan said "read the QEMU output to confirm SUCCESS
instead of HANG." **QEMU has no Kepler.** A green run there would mean the code took a path that
never touched the hardware, which is worse than HANG, not better. That got said in those words.
Its open question (Falcon-side CHAN_VALID vs fuzzing the engine ID) is answered: finish the one
already written — it is decisive in both directions, and the losing outcome queues the other
candidate for free.

**Original question, kept for the record:**

## Q7 (original) — kepler and igpu have now each bounced FOUR and TWO times. Keep spending lane cycles?

Both lanes came back today claiming their bounce lists were fully addressed. Both were wrong in
ways that would have cost a boot (details in RELAY.md):
- **kepler #4**: the fix DELETED the IMEM page-pad, so the upload is still invalid — the same
  never-uploaded defect that caused bounces 1–3, reintroduced by a different mechanism. Plus the
  phase gate is inverted (host waits for the falcon's give-up marker *before* sending the command,
  so both legs just HANG) and the SUCCESS witness prints identical strings for the two ack values
  its own proposal stakes everything on.
- **igpu r13**: the newly-added `GMUX_SWITCH_EXTERNAL` write is never restored; the restore values
  became fresh readbacks that truncate a `0xFFFFFFFF` timeout sentinel to a literal `0xFF` written
  into the display mux; and the runbook instructs the operator to power-cycle during what is
  really a 20-second dark window.

Neither is a dead end — the fixes are known and written down. But both lanes have now produced
confident "all fixed" reports that did not survive review, and each round costs a review pass.
**Do you want to keep both lanes running, pause one, or have the seat take one of them over
directly?** (3D is the decisive road per your own ruling, so kepler is the one I would keep.)

## Q8 — `std` in userspace: I owe you a recommendation, and it is coming from the midden arc.

You asked *"we need std in userspace, no?"* — I did not answer it off the cuff because it decides
how big the convergence is. The midden arc is producing a written assessment (target spec, libc or
a std shim, allocator, threads, fs, net — what it costs, what it buys, and what the alternatives
are given `libs/gneiss_pal` already plays that role on hosts). **No action needed from you yet;
this is a placeholder so the question does not get lost.** I will bring you the recommendation with
a size estimate.
