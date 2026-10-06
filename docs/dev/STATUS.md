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

## The store is the volume; this table is its export (UNAOSVOLUME, B427)

Peter, 2026-10-06: "can this be a next level jobs queue done within UnaOS using it's handlers and UnaFS? … we could
use the image we write to our boot disk as the working drive so whenever UnaOS is ready to run on its own everything
is already in place". The claims, the ledger rows and the queue items are records on a UnaFS volume, under `/jobs`:
`/jobs/status/ST<n>` (the body is the claim), `/jobs/ledger/<track>/<id>` (the row), `/jobs/queue/<track>/<nnn>-<NAME>`
(the item), every column a typed `job:*` attribute (`job:status` from the closed set, `job:flight` the bench's number
or absent, `job:line`, `job:set_by`, `job:refs`, `job:owner`, `job:arc`, `job:seq`). **Mica** (`handlers/mica`,
CODEX §2's Ledger) is the only writer, over the shared core `unaos/libs/sys/jobs_core` the kernel links to read
`/jobs`:

    ./arroyo jobs-image                                    # target/jobs-unafs.img from the repo; prints the witness
    mica jobs add    --img I --set-by S --refs B1 <claim>  # a new ST<n>, status open
    mica jobs cite   --repo R --img I --set-by S ST<n> confirmed f<n> <the wire line, whole>   # T2/T3/T4 refuse first
    mica jobs query  --img I status=confirmed flight=f24
    mica jobs export --repo R --img I                      # writes THIS table and the ledgers' rows back to git

`STATUS.tsv` and the ledgers' status cells are the volume's EXPORT, byte-identical to what `tools/status-check.py`
reads; the gate runs on the export until UnaOS runs it. The card build puts the same `/jobs` on p2 (the boot disk's
UnaFS volume), so Quarry lists it with its job columns and `/jobs/queries/Open jobs` is a saved query.
Host witness: `:: UNAOSVOLUME: records=<n> claims=<n> queue=<n> export=identical verify=<n>/<n> ::`.
