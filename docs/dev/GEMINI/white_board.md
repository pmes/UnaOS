# WHITE BOARD — 2026-08-09 (GR23, second pass)

Questions for Peter, each with the background to answer it. Nothing else lives here.
**Q1–Q5 of this morning are ANSWERED and off the board** — the kernel shell is being shut down and
routed through midden, the radios/3D jobs are running, wifis4a is being conditioned for merge, the
Crispy kit is "stolen from pi, sync later" (the law now rests on the in-tree triangulation, which
was verified bit-exact), and ceramic + paper-on-more-surfaces is building.

---

## Q6 — BT-L3 will briefly CONNECT TO A STRANGER'S DEVICE. Is that acceptable to fly?

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

I would take (b) for anything flown outside a controlled bench — it answers the same engineering
question (does the connect path work end to end?) with nobody else's hardware involved. **Your
call; it is a one-constant change either way and I will do whichever you say before it flies.**

## Q7 — kepler and igpu have now each bounced FOUR and TWO times. Keep spending lane cycles?

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
