# ATTRKEYS (rmbp-ledger B452) — one attribute-key registry for both rings

**Finding** (ARCH-2026-10-06 F9, MED): the attribute key names live in six homes — `fs/filetype.rs` (`una:type`),
`fs/assoc.rs` (`una:opener/icon/name/preferred`), `fs/attrfacts.rs` (`media:*`, `doc:title`, `image:animated`,
`una:facts-mtime/attrtimes/view`), `fs/appres.rs` (`una:app.*`, `una:apps`), `una-abi` (the trash keys),
`midden_core` (`RES_KEY_*`) — plus raw literals in `video/player.rs:185,189`, `drivers/ehci/bthid.rs` (`bt.*`),
`handlers/vein/src/vault.rs:38,40` (`una:embed-*`) and UNAOSVOLUME's `jobs_core` (`job:*`, not on this tip).

**Seam** (R79): `una_abi::attr_keys` — the crate both rings already link, where `TRASH_QUERY` lives. `const` only,
zero bytes of code. Every other home becomes an ALIAS of the registry constant (call sites unchanged);
`midden_core`, `vein` take a path dep on `una-abi` (no deps, no code). `ALL` lists every key; a `const` assert
refuses two equal names at compile time, a host test says the same and that every key has its namespace.
**Names are unchanged** — they are on-disk format (UnaFS attributes on the flown cards); `bt.*` keeps its
un-namespaced spelling (BTKEYSEAL moves those bonds into Holocron; renaming them here would orphan the bonds).

**Milestones**
- M1 — `una_abi::attr_keys` (all keys, `ALL`, const uniqueness assert, host test); every kernel, midden_core,
  matrix, vein and una-res literal replaced by the constant (aliases keep the old names).
- M2 — GATE-ATTRKEYS: `scripts/attrkeys-check.py` — a `"una:` `"media:` `"doc:` `"job:` `"image:` `"bt.` string
  literal in Rust code (comments skipped) outside `una-abi/src/lib.rs` is refused; today's non-key sites are a
  shrink-only baseline (`scripts/attrkeys.baseline`); `--selftest` controls; run by `arroyo check` after
  GATE-CHARTER. Shaped as one leg (`attrkey|<file>|<literal>`) for arch-check.py to absorb at the fold.

**Witness**: no new boot line (R80; a rename-free refactor has nothing to arm). The next metal boot's existing
`[attrcolumns] … facts=media:duration_ms=…` and `[attrcolumns] edit doc:title=1 una:type=1` lines must print the
SAME key spellings as flight 25 — a drifted name would show there.

**Owed**: `jobs_core` (`job:*`, branch exec-rmbp-unaosvolume) aliases the registry at the fold (its keys are
registered here); arch-check.py absorbs the leg when GATE-ARCH folds.
