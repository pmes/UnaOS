# ASSOCSTAMP (rmbp-ledger B460) — the file-type registry build skipped by a generation stamp

Cut from merge18 f4e0613c. PERFREVIEW F4 (B443).

## Finding (the wire)

- Flight 24, every boot that created the registry: `[boot] step=assoc-seed ms=2230 blocks_read=1072 blocks_written=198 created=26`.
- Flight 24, the reboot with nothing to create (12:46:44): `step=assoc-seed ms=1662 blocks_read=1191 blocks_written=0 created=0`.
  The build re-walks every known type: `read_dir`, then per type a `stat`, a registrants read (`una:apps` on the
  type object), a `list_attrs` — each a fresh path resolution — 27+ objects x 4 lookups, 1191 blocks on the card.
- On merge18 the build moved to LOGIN (FILETYPES, B423: `assoc::owe` -> `assoc::service` -> `build("login")`), so the
  1.66 s is paid on every login instead of at boot, but it is the same walk.

## The seam

The registry's own store (R79: no second store). ONE attribute on the registry directory itself:
`/system/filetypes` carries `una:filetypes.stamp` = `v<builder version> h=<FNV-1a 64 of the compiled-in type set> n=<type count>`.
The hashed set is everything the build derives an object from: `assoc::TYPE_FACTS` (mime, glyph, description),
`filetype::EXT_TABLE` (the extensions), and APPRES's `BUILTIN` resource blocks byte for byte (key + block: their
doc types, signatures and names decide `una:preferred`). `n` = the size of TYPE_FACTS united with the built-ins' doc types.

- login / verb build: read the stamp (ONE `get_attr` on one inode); equal => skip, the line says `skipped=stamp`.
- differs or absent (first boot, or an arc added a type / changed a resource) => the full `seed_in` runs once and,
  only on success, writes the stamp.
- `tests filetypes` always builds in full (forced); direct `seed_in` callers (the filetype fixture, `assoc <mime> <app>`) are unchanged.

## Milestones

- M1 `fs/assoc.rs` (`build` -> `build_stamped`, tail: stamp, check, skip) + `fs/appres.rs` (tail: `builtin_hash`).
- M2 `tests assocstamp` (fs/assoc.rs tail; registered on filetype.rs's tests line, before its comment): build forced,
  the stamp check measured (`blocks_read`, `cmds`, `ms`), a stale stamp forcing exactly one full build which re-writes
  it, then a hit.

## Witness lines

- login, every boot after the first: `[filetypes] built at=login dir=/system/filetypes created=0 filled=0 types=<n> skipped=stamp stamp=v1 blocks_read=<n> cmds=<n> ms=<n>` (ms < 10).
- the first boot with this tree / after a type change, once: `[filetypes] built at=login dir=/system/filetypes created=<n> filled=<n> types=<n> stamp=written blocks_read=<n> cmds=<n> ms=<n>`.
- `:: ASSOCSTAMP: stamp=match blocks_read=<n> ms=<n> -> PASS :: cmds=<n> stale=rebuilt rehit=ok bound_ms=10 stamp=v1 n=<n>`.

## Owed

- A ring-3 program's `una:apps` sighting (APPRES) is not in the hash: a type ONLY a ring-3 program declares gets no
  `una:preferred` filled on a stamp hit — the registrants still answer it at open time (opener_for falls back), so
  nothing opens wrongly; APPRES could clear the stamp on a new sighting if the seat wants the fill.
- A user deleting a type object or key by hand is not refilled until the next stamp change (or `tests filetypes`).
- BOOT80's `types_blocks` leg (fs/bootstep.rs) still measures the `ls` of the directory; it is unchanged by this arc.
