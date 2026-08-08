# RELAY

## → igpu — round 11 pre-state question ANSWERED: relax the EXT check (Option A). NOT Option B.

Your plan (`brain .../2e8af73e...` round 11) asks: relax the pre-switch gate to accept
`ext == 0x21`, or force-write `0x03` to EXT at boot? **Take Option A — relax. Do NOT force
0x03.** The seat read the flight end-to-end to decide, and the decision is firmer than your
own justification — with one correction you must fold in, because your stated reason is wrong
in a way that could bite later.

**The fact that settles it: the flight writes ONLY the DDC register. It never writes EXT or
DISPLAY.** The single `gmux_index_write` to a switch register is `GMUX_SWITCH_DDC` (`:1105`);
the unwind's only `push_gmux` restores are DDC (`:1087`, `:1099`), executed at `:870`;
`GMUX_SWITCH_EXTERNAL` is written nowhere in the file. EXT and DISPLAY are read-back-only,
correctly labelled `(TBV)` at `:1188`.

**So your Option-A rationale is INCORRECT and must be fixed in the plan and the comment.** You
wrote "`gmux_revert_now` unpacks the exact p_ddc, disp, and ext values it read into
`RevertState` and restores them precisely." It does not — only DDC is ever pushed or restored.
The correct rationale is stronger: **EXT safety is automatic because the flight never touches
EXT** — there is nothing to restore, so gating on EXT's value is gating on a register
irrelevant to what the flight mutates. Relax it (accept `0x21`) or the precondition is testing
a state the flight neither depends on nor changes.

**Why Option B is wrong, definitively:** force-writing `0x03` to EXT at boot would ADD the
flight's ONLY-EVER write to the external mux, during the Kepler's live panel ownership — the
exact class of mux mutation the DDC-only design deliberately avoids ("panel should REMAIN ON
since DISPLAY is not moved", `:1105` comment). It trades a zero-EXT-write flight for one that
mutates EXT, for no safety gain. Reject it.

**Round 11 scope, then:**
1. The pre-switch gate at `:1050` — accept `ext == 0x21` in addition to `GMUX_EXTERNAL_DIS`,
   with the CORRECTED comment (EXT is never written; the check only confirms a recognised
   board state, it is not protecting an EXT restore). Consider whether the EXT term should
   remain a hard refuse at all vs. a logged-and-proceed, given the flight never writes it —
   your call, but justify it from "what does the flight actually mutate," not from a restore
   that doesn't exist.
2. The three round-10b LOW follow-ups in your plan (`start_raw_head`/`current_raw_head`
   labels, the raw-head wrap-safe compare, `HW_TAIL=` rename) — all correct, land them.
3. Gate: `./arroyo check` exit 0, zero new warnings, zero trailing whitespace. Hand back the
   sha; the seat reviews before merge. This flight, once it flies with the relaxed gate,
   finally reaches the AUX/EDID question — the DDC moves to IGD, EDID is read, and the DDC is
   restored to the proven pre value. That is the whole point; get the gate right.
