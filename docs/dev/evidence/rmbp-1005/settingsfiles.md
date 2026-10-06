# SETTINGSFILES (rmbp-ledger B407, R98) — settings are FILES, one per domain, in `<home>/settings/`

Cut from eb7bb461 (exec-rmbp-merge17). MACPARITY §16 B1, the Be inheritance. No knob.

## Finding (read from the code)
- ONE store: `prefs.rs::path()` = `<home>/.config/unaos/preferences.toml`, every namespace in one TOML, written by
  the USERS.DAT swap (`.new`, read back, parse, rename). Every write rewrites every key.
- Writers reach it three ways, all through `prefs::set_applied` = `prefs_core::wire::persisted_set`: the kernel
  Settings/dock/keys via `prefs_client` (the bus, deferred to `prefs-flush` off the render task — INPUTSTALL M5),
  a ring-3 program's PREF_SET (PREFS.ELF relays into the same kernel store), the `pref` verb.
- prefs_core's schema (33 rows) carries a `doc` per key but nothing writes it to the file; the codec's emitter
  drops comments. No `app.*` namespace and no way for a program to declare its keys.
- Quarry's Show Info is a NOTICE (size, owner); ATTRCOLUMNS (B402) is building `video/quarry/getinfo.rs`.

## The seam (R79)
- **Principia — shared-core** (`prefs_core`): new `files.rs` — `domain_of(ns,key)` (the split rule), `split`,
  `render` (the human-readable file: `# auto-saved <ISO> by <who>`, the schema doc as a comment above each key),
  `stamp_of`; new `declare.rs` — the `app.<name>` stanza (`PrefDeclare` body, owned kinds, the check), and
  `wire::VERB_DECLARE` (20) in `fulfil`. Both rings link it; the host Principia adopts it next (owed).
- **Kernel as fulfiller** (`prefs.rs`): the SAME tree, the SAME bus, the SAME schema; the store under it is
  `<home>/settings/<domain>`. A set saves ONLY its domain's file (the same swap per file). The old file is
  migrated once and deleted. A deleted domain file resets that domain at the next service read.
- Domains: `system.display.*` -> `display` (but `display.wallpaper` -> `desktop`), `system.dock.*` -> `desktop`,
  `system.login.*` -> `login`, `system.audio.*` -> `sound`, `system.pointer.*` -> `trackpad`, every other
  `system.*` (power, settings.tab, …) -> `general`; `app.<name>.*` -> `<name>`; any other namespace
  (`vein`) -> `<ns>`.

## Milestones
- M1 prefs_core: `files.rs`, `declare.rs`, `wire::VERB_DECLARE`; host tests (round-trip of a rendered file,
  the domain table, the declared clamp).
- M2 kernel store split: `prefs.rs` per-domain load/save/migrate/reset-on-delete; una-abi `BUS_VERB_PREF_DECLARE`,
  bus gate + both dispatch arms; the `by` of each write (`prefs_client` = the pane, the verb, a program).
- M3 Lumen declares `app.lumen.window.frame` through `vein_ring3::prefs::declare` (WINMEMORY row 12's key).
- M4 Settings: one line per pane `Stored in settings/<domain>` that reveals the folder in Quarry; Quarry Show Info
  on a settings file lists its keys and the auto-saved line (`video/quarry/settingsinfo.rs`, a separate file
  ATTRCOLUMNS' getinfo.rs joins at the merge).
- M5 `tools/prefs-schema-check.py` per domain; `tests prefs` SETTINGSFILES leg.

## Witness (the next flight)
First boot of the image (once): `[prefs] migrated preferences.toml -> settings/<n> files (R98)`; every load
`:: PREFS: path=<home>/settings domains=<n> loaded=<n> saved=<n> ns=system -> PASS ::`; per write
`[prefs] saved settings/<domain> keys=<n> by=<who>`; a deleted file
`[prefs] settings/display absent -> defaults (deleted by the user)`; `tests prefs` ->
`:: SETTINGSFILES: domains=<n> files=<n> migrated=<0/1> readable=1 reset_on_delete=ok app_ns=ok alone=1 dir=<home>/settings -> PASS ::`
(`alone=1`: the app write touched no other domain file). Lumen's wire line carries ` declared=0` and the kernel says
`[prefs] declared app.lumen keys=1 -> settings/lumen (R98)`. Settings link: `[settingsfiles] reveal dir=<home>/settings opened=1 from=tab<n>`;
Show Info on a settings file: `[settingsfiles] info path=<p> keys=<n> saved=<iso>`. `tools/prefs-schema-check.py` prints
`domains (declared/referenced) desktop=4/4 display=5/5 general=2/2 login=1/1 lumen=1/0 sftest=1/1 sound=3/3 trackpad=1/1 vein=19/19`.

## Owed
- Host Principia (`handlers/principia/src/prefs.rs`) still writes its one `preferences.toml`; it adopts
  `prefs_core::files` next (the same functions).
- Lumen DECLARES its frame key; writing the frame on move/close is the wm's (WINMEMORY row 12).
- Live reload of a hand-edited file (only a delete is noticed live; an edit is read at the next login).
- PrefDeclare took bus verb 20 (una-abi `BUS_VERB_PREF_DECLARE`): the seat checks no other wave-arc minted 20.
- A deleted file resets the TREE (every reader answers the schema default at its next read); a live consumer that
  holds a hardware level (volume, brightness) re-applies on its next PrefChanged/login, not at the delete.
