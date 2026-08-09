# WHITE BOARD — 2026-08-09 (GR23, third pass)

Questions for Peter, each with the background to answer it. Nothing else lives here.
Q1–Q7 are ANSWERED and off the board (kernel shell → midden: merged; radios/3D: running;
wifis4a: merged; kit: sync later; ceramic+paper: merged; MEGABOOM filter: merged; lanes: both
kept, drill-sergeant register adopted).

---

## Q9 — The window controls: may I give them RED / YELLOW / GREEN?

**This is why the minimise button killed your STAT and console.** The three discs are painted
`0x3d5f92 / 0x678cba / 0x92aac9` — three shades of blue, progressively lighter — because those
are the only three control colours the kit table carries. Close is the LEFTMOST disc. You clicked
expecting minimise and got the one that kills, and nothing on the glass told you which was which.
The metal capture shows it exactly: a press at x=2765 routed `-> close` and reaped the row, while
x=2791 (the middle disc) routed `-> drag`.

Being fixed now WITHOUT touching the palette: per-disc hit testing, minimise wired to the existing
`set_hidden`, zoom wired to a remembered pre-zoom rect, and an unmistakable NON-COLOUR affordance —
a glyph inside each disc (×, −, +) drawn in the existing `ink` role. A symbol is SHAPE, not
palette, so the shared-source law is intact.

**But the Mac standard you asked for in A2 is red/yellow/green, and that is a PALETTE change the
kit does not carry.** The whole point of the Mac colours is that the destructive control is
unmistakable at a glance, without reading a glyph. Options:
  a) **Glyphs only** (what is building now) — law-clean, no palette invented, ships today.
  b) **Add three semantic control colours to the kit** (`kits/crispy/theme.json` first, then
     re-lift into `theme.rs` per the law) — a real red/yellow/green in Crispy's refined register,
     not the raw macOS hues. This is the honest way to get what A2 asked for.
  c) Keep the blues and rely on position alone (status quo, minus the glyphs).

I recommend (b) ON TOP OF (a) — glyphs now, and you tell me the three hues (or approve me
proposing three in the Scandinavian-minimal register for your yes/no). **The kit edit is yours;
the re-lift is mine.**

## Q10 — Should a minimised window be restorable, and how?

`set_hidden` parks a window so it stops starving the compositor — that is the mechanism minimise
needs. **What does not exist is a way back.** There is no dock, no taskbar, no window list. If
minimise hides a window and `<TAB>` focus cycling cannot reach it, minimise is a ONE-WAY TRIP and
the arc has been told to REFUSE to wire it rather than ship that.

So: what should bring a window back?
  a) `<TAB>` reaches hidden windows too (cheapest — the focus ring already exists; a hidden window
     un-hides when focused).
  b) A dock/taskbar strip — real work, and it is a desktop-furniture design decision that is yours.
  c) A `windows` command in midden that lists and restores by name (fits the convergence: the shell
     is the way you reach things until there is furniture).

I lean (a) now and (c) soon, because both are small and neither commits you to furniture you have
not designed. **If (a) is fine, say so and minimise ships with this arc.**

## Q11 — BT-L3 read `considered=0` because the MEGABOOM was off. Retest, or go active?

Boot AS printed `peer NOT SELECTED — name=MEGABOOM considered=0 matched=0` — and you said you
forgot to turn the speaker on, which fully explains it: **zero devices considered means the room
looked empty to a passive scan, not that the filter rejected your speaker.**

Next boot with it powered on and nearby is the real test. Two outcomes and what each means:
  - `considered=N matched=1 … peer SELECTED … MEGABOOM` → connect + clean release. Done.
  - `considered=N matched=0` with the speaker ON and near → its name is in a SCAN RESPONSE, not in
    the advertisement. A passive scan never sees those. **Going ACTIVE means transmitting a
    SCAN_REQ to every advertiser in the room** — a different thing from listening, and your call,
    not the seat's. (`MAYBE:short-name-prefix` in the log would instead mean it was heard under a
    shortened name — that path is already handled.)

**No action needed unless the second outcome happens.** Recorded so the question is already framed
when it does.
