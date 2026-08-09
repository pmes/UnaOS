# WHITE BOARD — 2026-08-09 (GR23, fourth pass)

Questions for Peter, each with the background to answer it. Nothing else lives here.

**Everything asked earlier today is ANSWERED and off the board:** kernel shell → midden (merged) ·
radios/3D (running) · wifis4a (merged) · the Crispy kit "stolen from pi, sync later" · ceramic +
paper (merged) · MEGABOOM by name (merged) · controls red/yellow/green top-left (merged) · dock
(merged) · **and the lanes: the seat has taken BOTH, Gemini is shut down.**

---

## Q13 — The three control hues: veto by eye, or leave them?

I showed you these on the actual chrome. They are MERGED and on the next card either way — this is
a "change them or don't" question, not a blocker.

| control | hex | reading |
|---|---|---|
| close | `0x00C25F55` | clay red, hue 6° |
| minimise | `0x00C89C52` | ochre, hue 39° |
| zoom | `0x005E9468` | sage, hue 135° |

They keep macOS's HUE ANGLES (6/39/135) so the meaning is unmistakable, but pull saturation and
value into the register the rest of the table lives in — `#FF5F57 / #FEBC2E / #28C840` are fully
saturated and shout against near-white chrome and a muted accent. **If you want the louder macOS
hues instead, that is one line each.** Provenance is honest in `theme.rs`: derived, your ruling,
no kit hash, pending re-lift when `kits/crispy/` becomes reachable.

## Q14 — The MEGABOOM: the next boot's RAW line decides, and one outcome needs your ruling.

Boot AS read `name="."` for `88:c6:26:cc:2d:3c` — your speaker's exact address, so **we did hear
it.** The parser was investigated and CLEARED: introducing the off-by-one produces `".MEGABOOM"`,
not a bare `"."`. The real defect was that **one unprintable byte and a real `.` rendered
identically** — the witness could not tell us which. Fixed: any name under three characters now
prints its RAW BYTES.

So the next boot with the speaker on and near tells us which world we are in:
  - **`name="MEGABOOM"`** → connect and clean release. Nothing needed from you.
  - **RAW shows a real Local Name we mis-decoded** → my bug, I fix it, nothing needed from you.
  - **RAW shows the advertisement carries NO name** (or a genuine one-byte one) → **passive LE
    matching can NEVER find this speaker.** Then it is your call between: match by ADDRESS
    (`88:C6:26:CC:2D:3C`, zero transmit, works today) or **ACTIVE scanning, which TRANSMITS a
    SCAN_REQ to every advertiser in the room.** I will not switch to active silently.

Recorded so the question is already framed when the capture lands.

## Q15 — When do you want the next boot? A lot is merged that has never been on glass.

Trunk is 99/99 and carries, none of it yet flown: **the controls top-left in red/yellow/green with
working minimise and zoom** · **the dock** (every window has a way back, hidden ones included) ·
**the close-isolation fix** (closing STAT no longer takes the console — the console was never
closed, it stopped being COMPOSITED) · **the console furniture fix** (it was being drawn with a
live close button that would have reaped your only surface) · the BT name raw-bytes witness · the
midden shell convergence.

**Plus two flights that are ready and are yours to schedule:**
  - **igpu round 13 — CLEARED TO FLY** after five adversarial rounds. It blanks the panel for
    ~2.4-2.5 s and answers whether the eDP AUX lines are gmux-routed. It discriminates decisively in
    3 of 4 outcomes; the 4th ambiguity is named and is the irreducible limit of one boot. Being
    ported into our tree now.
  - **kepler FENCE** — the seat is re-authoring it. Not ready; will be reviewed before it flies.

I can stage media whenever you say. **The desktop work and the igpu flight can ride the same card
or separate ones — your call, since the igpu flight blanks the panel and the desktop work wants
you clicking things.**
