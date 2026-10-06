# OPENERTRUST (B447) — a file attribute must not launch an arbitrary program

Branch `exec-rmbp-openertrust`, cut from merge18 @41900c60 (FILETYPES B423 and ARCHREVIEW B441 on the tip:
`appres::registrants`, `appres::opener_of_preferred`, `assoc::opener_for_in` are present by those names).

## Finding (ARCHREVIEW F3, HIGH, security)
`assoc::opener_for_in` returns a FILE's own `una:preferred` through `appres::opener_of_preferred`, which hands back
any value it cannot map (an opener id or an absolute path) unchanged; `quarry/openers.rs` `open` has a
`prog if prog.starts_with('/')` arm that launches it. `filetype::carry_in` (the `cp` leg, `shell.rs` copy loop)
copies `una:preferred` with the bytes. So `una:preferred=/home/u/Downloads/x.elf` on a document — set on a card,
in an archive, or by any writer — runs that ELF when the DOCUMENT is double-clicked, with no act on the program.
No flight wire carries this (it is a review finding); the witness is new.

## The seam
No new store and no new table: the trust decision is made in the ONE resolver (`assoc::opener_for_in`, fs-core),
against the ONE registry (`appres::registrants` — the built-ins' compiled-in `doctypes` plus the ring-3 programs
APPRES sighted and published as `una:apps` on the type object). Be's rule: PREFERRED_APP names an app the registrar
knows, never a path.

## Milestones
- M1 — `opener_for_in`: a file's `una:preferred` is honoured only when it resolves to a REGISTRANT of the file's
  type; otherwise it is ignored (resolution falls through to the registry's preference, then the first registrant)
  with one wire line per refused file: `[openers] preferred=<v> refused=not-a-registrant path=<p> type=<m>`.
  `carry_in` drops `una:preferred` when source and destination are not the same storage
  (`MountTable::same_volume`; an unknown identity counts as foreign): `[filetype] carry=strip key=una:preferred
  reason=foreign-volume <src> -> <dst>`. The type object's own `una:preferred` (the user's FileTypes choice on the
  system volume) is unchanged.
- M2 — `tests openertrust`: `:: OPENERTRUST: registrant=ok foreign_path=refused carried=stripped -> PASS ::`
  (a probe file on the system volume: `una:preferred=org.unaos.fileview` on text resolves to `fileview`; a
  `/apps/EVIL.ELF` path resolves to the type's default and NOT the path; a copy onto another volume carries no
  `una:preferred` — `skip` when no second attribute-taking volume is mounted). SECURITY.md hardening row.

## Witness (the next flight reads)
`tests openertrust` → `:: OPENERTRUST: registrant=ok foreign_path=refused carried=stripped|skip -> PASS ::`; a
double-click on a planted document prints `[openers] preferred=/... refused=not-a-registrant` and opens it with
the type's default.

## Owed
- The refusal is a wire line, not an alert on the glass (F3 asks for a DIALOG alert): owed to the seat.
- A REGISTRANT is still whatever APPRES sighted: a program sighted from a removable volume joins `una:apps` (APPRES's
  trust, not this arc's).
- `carry_in` is the only copy path; an archive extractor or a future Quarry drag-copy must call it (or strip).
