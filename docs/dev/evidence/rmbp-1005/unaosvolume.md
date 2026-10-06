# UNAOSVOLUME (rmbp-ledger B427) — the jobs as one aspect of the one UnaOS volume

## Design (written before the code)

**Finding.** The status table (`docs/dev/STATUS.tsv`, STATUSTABLE B425), the four ledgers and the four queues are
text in git; UnaFS already has everything a store needs (typed attributes, eq/ord B+tree indexes, `query`, the
host `FileDevice`), and Mica (CODEX §2: the Ledger — structured data) was a README with no crate. QUERYFOLDER (B420,
not on this tip) saves a query as a file under a `queries` folder typed `application/x-vnd.una-query`; ATTRCOLUMNS
(B402, on this tip) reads a folder's columns from its `una:view` attribute. Nothing named `jobs` exists in any ring.

**The seam (R79).** ONE store, ONE writer, ONE core:
- `unaos/libs/sys/jobs_core` — shared core, `no_std` + `alloc`, zero deps (the `midden_core` shape): the record
  model, the `job:*` attribute names and the closed status set, the parsers of STATUS.tsv / the ledgers / the
  queues, the EXPORT writers (byte-identical to what `tools/status-check.py` reads), `verify` (a line found
  byte-for-byte), the `status=… flight=…` → UnaFS query translation, the volume layout (`/jobs/...`).
- `handlers/mica` — the owner handler and the ONLY writer: `mica jobs build|add|cite|verify|list|query|export|witness`
  over `unafs` (the library `tools/unafs` wraps). The volume is the store; STATUS.tsv and the ledger status cells are
  its export, and the STATUSTABLE gate keeps running on the export until UnaOS runs it.
- the kernel — a READER this arc (`fs/jobs.rs`, CHARTER Mica — shared-core): `/jobs` is an ordinary folder of the
  UnaFS root, so Quarry lists it with ATTRCOLUMNS' columns (`una:view` written by the builder) and the saved query
  `/jobs/queries/Open jobs` is QUERYFOLDER's shape (a file, `una:type = application/x-vnd.una-query`, text
  `job:status == open`) — no second query store. At `login ok` it prints one line (R93); nothing tests at boot (R80).

**Layout.** `/jobs/status/ST<n>` (body = the claim sentence) · `/jobs/ledger/<track>/<id>` (body = the ledger row,
verbatim; `<track>` = rmbp, orin, pi, trunk — the four ledgers' ids collide, so the track is a folder) ·
`/jobs/queue/<track>/<nnn>-<NAME>` (body = the queue item line) · `/jobs/queries/Open jobs`. Typed attributes:
`job:kind`, `job:id`, `job:seq` (Int, the source order), `job:status` (closed set: open confirmed refuted parked
unflown fixed-unflown flown landed dropped, `-` for a ledger cell with no enum head), `job:flight` (the bench's
`f<n>` or absent — never invented), `job:line`, `job:set_by`, `job:refs`, `job:owner`, `job:arc`, `job:track`.

**Milestones.** M1 `jobs_core` + its KATs (round trip on the real files). M2 `handlers/mica` (the verbs, the
image builder, `cargo test -p mica`: repo → volume → export byte-identical, the witness). M3 `./arroyo jobs-image`
(`target/jobs-unafs.img`) and the card build: `/jobs` onto p2 after ROOTDISK2's put loop, from the same builder.
M4 kernel `fs/jobs.rs` + the login line. M5 STATUS.md: the volume is the store, the TSV its export.

**Witness.** Host: `:: UNAOSVOLUME: records=<n> claims=<n> queue=<n> export=identical verify=<n>/<n> ::`
(`mica jobs witness`, `./arroyo jobs-image`). Metal, at login: `[jobs] volume=/jobs records=<n> claims=<n>
ledger=<n> queue=<n> queries=<n>` (records=0 / `volume=absent` on a card built without it).

**Owed.** UnaOS running the seat's workflow (the roadmap); a kernel WRITER (the bus verbs Mica's fulfiller would
answer); the `Open jobs` folder opens as a live query only once QUERYFOLDER folds (until then it is a text file).
