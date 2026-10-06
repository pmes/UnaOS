# STATUSBASELINE — the 188 grandfathered status cells read against the wire (rmbp-ledger B437)

Host-only. Cut from 4ead840a (merge17; STATUSTABLE B425 merged at 6f19ddda). No kernel byte moves.

## Design

**Finding.** STATUSTABLE (B425) made `docs/dev/STATUS.tsv` the one table and grandfathered 188 claiming cells in
`docs/dev/STATUS.baseline` (rmbp-ledger 126, LEDGER.md 42, queue STATE lines 20). A grandfathered cell is exactly the
shape Peter named: a status word (flew, unflown, proven, confirmed, refuted, landed on metal) with no flight and no line
behind it.

**The seam.** None in code. The table is the seam: every cell's claim becomes, or cites, an `ST<n>` row whose line
`tools/status-check.py` finds byte for byte in that flight's capture. Rows are resolved from the captures by a script
(the whole line is copied out of `f<N>-boot*.log` or `FLIGHT<N>.md`, never typed), so no quoted line can drift.

**Rules applied to every cell.**
1. A metal claim the wire supports gets a `confirmed` row with the whole line, and the cell cites it.
2. A claim the wire does not support is rewritten in the cell to say what the wire shows, cites the row that shows it
   (`refuted` when the wire contradicts the claim), and is listed below with before and after.
3. A claim with no flight (a host test, a QEMU run, a gate drill, a code read, a git fact) gets an `unflown` row.
4. An "unflown" that later flew is rewritten to "unflown at hand-back" plus the flight that flew it.
5. Orin and Pi captures (`docs/dev/evidence/orin*`, render<N>) are outside the gate's `f<N>` capture model: those claims
   get an `open` row naming the capture, never a borrowed rmbp flight number.
6. No flight number is invented: every `f<N>` is the bench's, and a cell whose flight the evidence does not name says so.

**Milestones.** M1 this design. M2 the rows (ST43–ST265), the citations in all 188 cells and the baseline emptied
(one commit: the gate holds only with all three). M3 the rewritten-cell list below and the DRIVERS-METHOD §1 entry.

**Witness.** Not a metal line: `python3 tools/status-check.py` → `rows=265 … claiming-uncited=0 baseline=0 -> PASS`,
`--baseline` prints nothing, `--selftest` 11/11.

**Owed.** The B408–B425 rows carry their ledger id in the claim with an empty `row-refs`, because those ledger rows are
not in this tree (the seat fills `row-refs` at the fold). The enum heads of B271 and B297 still read `flown` (their prose
now says they did not fly); GATE-LEDGER owns that vocabulary, so the head is the seat's call.

