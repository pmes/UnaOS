# STATUSTABLE (rmbp-ledger B425) — one status table, and a gate that refuses a status word without a wire line

Cut from df15f19d (merge17). Host-only: no kernel code, no knob, no boot.

## Design

**Finding.** A claim's status was prose typed in four places (ledger status cells, queue STATE lines, each
ladder doc, the evidence files) and nothing compared a status word with the wire. WIFI5 M1 read flights 13–25
and found the BCM4331 upload "never flew" false: it flew and verified on every boot of flights 13–20
(`f13-boot1.log` 33884 ms, `rev=666 -> UPLOADED`). DRIVERS-METHOD §3 said "confirmed only by a quoted wire
line"; it was a rule per ladder, not one table, and no gate held it.

**The seam.** ONE table, `docs/dev/STATUS.tsv` (tab-separated; `id claim status flight line set-by row-refs`),
and ONE gate, `tools/status-check.py` (GATE-STATUS), run by `arroyo check` beside GATE-CHARTER. The ladder
docs keep their reasoning; the table holds the status. Ids are `ST<n>`, not `S<n>`: `docs/dev/LEDGER.md`
already owns `S<n>` (shared seams, `→ S<n>` cross-refs GATE-LEDGER resolves) and the BCM4331 stages are named
S0..S8 — a bare `S4` in a Wi-Fi cell would have satisfied the gate without citing anything.

**The gate's rules.** confirmed/refuted need a flight and a whole quoted line, found byte-for-byte in that
flight's capture (`docs/dev/evidence/**/f<N>-boot*.log` or `FLIGHT<N>.md`); any quoted line is checked
whatever the status; unflown carries no flight; a ledger status cell (rmbp-ledger, LEDGER) or a queue STATE
line saying flew / never flew / unflown / proven / confirmed / refuted / landed on metal must cite an `ST<n>`
row. The enum word at a cell's head (`fixed-unflown`, GATE-LEDGER's vocabulary) is not a claim; the prose
after it is. Cells older than the gate are keyed in `docs/dev/STATUS.baseline`, which only shrinks (a keyed
cell that now cites, or no longer claims, fails until its key is deleted); a new claiming cell never gets a key.

**Milestones.** M1 the table (42 rows: the Wi-Fi ladder from WIFI5's §7, GPUBLIT3's rungs, GEN7's rungs,
DRIVERS-METHOD §1's named claims) + the gate + `--selftest` (a fixture line in no log FAILS). M2 the Wi-Fi
cells B338/B131/B124 (with the seat's CORRECTION, from main) and the GPUBLIT/GEN7/NETFRAME/VOLUMES/INPUTSTALL
cells rewritten to cite rows; the baseline for the rest. M3 `docs/dev/STATUS.md`, LAWS line, DRIVERS-METHOD §3
pointing at the table, STRUCTURAL_GATES row, the `arroyo check` leg.

**Witness.** No boot: the witness is the gate's own line, `status-check: rows=<n> … claiming-uncited=<n>
baseline=<n> -> PASS`, and `status-check selftest: 9/9 fixtures behave -> PASS`.

**Owed.** The baseline's cells (counted in the report) are rewritten as each row is next touched; row-refs to
the seat-held rows B410/B411/B415 are added once those rows fold (they are not in this tree).
