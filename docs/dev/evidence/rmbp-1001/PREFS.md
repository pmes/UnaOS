# PREFS — the kernel reads and writes Principia's preference store (B300)

Answers AUDIT §1 (`docs/dev/evidence/rmbp-0929/AUDIT-HANDLERS-UNAFS.md`, rmbp-ledger B287; B282 SETTINGS2's
store note). Peter: "nope. this must be fixed." R79: a kernel feature in a handler's domain sits on a SEAM.

## Finding

The kernel Settings window kept `<home>/.settings` (flat `key=value`), the dock kept `<home>/.dock` (one
name per line, its home built from a `/home/<name>/` literal), and the brightness keys, the `wallpaper`
verb and POWERMENU's low-battery shutdown percent kept nothing at all (RAM or a compile-time knob).
Principia (`handlers/principia`, CODEX System handler) owns preferences: namespace + dotted key, four
scalar types, one TOML table per namespace in `~/.config/unaos/preferences.toml`, atomic replace, bus
`PrefGet/PrefSet/PrefChanged/PrefList`. LAWS §Handler manifest routes every preference there.

## Seam: shared-core (the midden_core shape)

`unaos/libs/sys/prefs_core` — `#![no_std]` + `alloc`, `#![forbid(unsafe_code)]`, ZERO dependencies, a
member of the root host workspace and a path dep of the kernel. It holds the data model and the codec
both rings use; nothing in it knows about files, buses or principals.

- `PrefValue` Str/Int/Float/Bool (the same four as `bandy::PrefValue`), `validate_ns`/`validate_key`
  (the rules of Principia's `prefs.rs`: `[A-Za-z0-9_-]+` segments, dot-separated for keys),
- `PrefTree` — namespace → dotted key → value, with Principia's leaf rule (`a` and `a.b` cannot both
  hold values: refused at set time),
- a TOML SUBSET codec. Parse: `[ns]` / `[ns.sub.table]` headers with bare keys, `key = value` with bare
  (optionally dotted) keys, basic / literal / multi-line strings, decimal (and 0x/0o/0b) ints, floats
  incl. `inf`/`nan`, bools, `#` comments and blank lines. Anything else — arrays, inline tables,
  array-of-tables, dates, quoted keys, a scalar above the first table, a duplicate key — is REFUSED with
  its line number (`ParseError { line, why }`); nothing is silently dropped. Comments are not preserved.
  Emit: byte-identical to Principia's `PrefStore::to_toml` (its fixed header, then `toml`'s pretty
  layout: sorted keys, scalars before sub-tables, a table with no scalars of its own gets no header,
  `toml_writer`'s string-style choice and float spelling). TOML arrays are not in the subset, so
  `dock.pins` is a comma-joined string.

**Principia's side (the "more than a thin change" branch, said plainly).** Principia keeps the `toml`
crate as its reader and writer: its reader is deliberately LENIENT (an array in a hand-edited file is
skipped, its test `unsupported_value_types_are_skipped_not_fatal`), the subset codec is deliberately
STRICT, and swapping would change Principia's behaviour. What Principia takes from the core: its
`validate_ns` / `validate_key` delegate to `prefs_core` (one rule, two rings), and two cross-tests pin the
codecs together — every `to_toml` output Principia produces is accepted by `prefs_core` and re-emitted
byte-identical, and a `prefs_core` emission loads in Principia to the same values. So prefs_core's codec
is the KERNEL-facing subset and Principia reads the same file unchanged.

## Milestones

- **M1** `prefs_core` crate + goldens (a Principia-produced file parses to the same tree and re-emits
  byte-identical; the four types survive a cycle; a malformed line is refused by number); Principia
  delegates validation and gains the cross-tests. `cargo test -p prefs_core -p principia`.
- **M2** kernel `src/prefs.rs` (`//! CHARTER: Principia — shared-core`): the store at
  `<home>/.config/unaos/preferences.toml` (home from `fs::users::home_of`, never a literal; no session =
  `/.config/unaos/preferences.toml`), loaded once per login, saved by the USERS.DAT swap through the
  mount table (`preferences.toml.new` written, read back, parsed, renamed over). Namespace `system`:
  `display.brightness` `display.idle_min` `display.wallpaper` `audio.volume` `audio.mute` `pointer.speed`
  `power.lowbat_shutdown_pct` `dock.pins` `settings.tab`. settings.rs and dock.rs rewired; `.settings` /
  `.dock` reading DELETED (a one-shot migration imports and deletes them, `[prefs] migrated=<n>`); the
  brightness keys and the `wallpaper` verb persist through it; POWERMENU's shutdown percent reads it (the
  compile-time `UNAOS_LOWBAT_SHUTDOWN` becomes the default). Verb `pref get|set|list`.
- **M3** bus verbs `PREF_GET`/`PREF_SET`/`PREF_LIST` (11/12/13), additive to the frozen v1 frame,
  fulfilled in `prefs.rs` on both arches; codec KATs.
- **M4** `tests prefs`.

## Witness

`:: PREFS: path=<p> loaded=<n> saved=<n> ns=system -> PASS ::` on load and on save;
`[prefs] set <ns>.<key>=<value> ok=<0|1>` per change; `[prefs] migrated=<n>` once;
`[prefs] changed <ns>.<key>` per accepted change (the PrefChanged stand-in, below).

## Owed

- PrefChanged mailbox delivery → BANDY-3 (fulfiller/interest registration does not exist yet).
- Mirroring `system.*` as attributes on the preferences file → ATTRSURF (`set_attr` not in this tree).
- The charter row `dotfile | .config/unaos/preferences.toml | B300 (Principia's path)` at the fold.
