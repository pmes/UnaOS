# STATUS — one table for every status claim (B425, GATE-STATUS)

Peter, 2026-10-06: "how come there isn't one status table to rule them all so a false claim such as in this wifi
situation can never be made?" The seat's ledger said the BCM4331 upload "never flew"; the wire said it flew and
verified on every boot of flights 13 to 20.

**The one rule: a status word without a flight and a line is a lie the gate refuses.**

`docs/dev/STATUS.tsv` is the table, tab-separated, one claim per row, no prose cells:

| column | holds |
|---|---|
| `id` | `ST<n>` (not `S<n>`: LEDGER.md owns those, and the Wi-Fi stages are named S0..S8) |
| `claim` | one sentence |
| `status` | `open` · `confirmed` · `refuted` · `parked` · `unflown` |
| `flight` | the bench's number, `f<n>`; never invented; empty for `unflown` |
| `line` | the wire line, quoted WHOLE, exactly as the capture holds it |
| `set-by` | session (arc) and date |
| `row-refs` | the ledger rows it bears on, comma-separated |

`tools/status-check.py` (in `./arroyo check`; `--selftest` proves a line in no log fails) refuses:
`confirmed`/`refuted` without a flight and a line; any quoted line not found byte-for-byte in that flight's
capture (`docs/dev/evidence/**/f<N>-boot*.log` or `FLIGHT<N>.md`); `unflown` with a flight; and any ledger
status cell (`docs/dev/OS/rmbp-ledger.md`, `docs/dev/LEDGER.md`) or queue STATE line that says flew / never
flew / unflown / proven / confirmed / refuted / landed on metal without citing an `ST<n>` row.

Cells older than the gate are keyed in `docs/dev/STATUS.baseline`, which only shrinks: when you touch one,
give it a row and cite it, then delete its key. A ladder doc keeps its reasoning (DRIVERS-METHOD §3); its
rung statuses live here.
