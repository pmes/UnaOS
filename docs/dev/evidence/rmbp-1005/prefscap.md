# PREFSCAP — a program sets only its own stanza and the keys the schema marks settable (rmbp-ledger B454)

Cut from 82319dd6 (exec-rmbp-merge17). Answers SECREVIEW F4 (`docs/dev/review/SEC-2026-10-06.md`, branch
exec-rmbp-secreview).

## Finding

`prefs_core::wire::fulfil` admitted every `PREF_SET` from any caller in the session (`in_session` was the only
test): a ring-3 program could set `vein.endpoint` / `vein.claudecode.bin` / `vein.key_file`,
`system.login.items` (persistence), `system.display.mode` (F1's trigger at login). `PrefDeclare`'s stanza name
was caller-chosen (program A replaces program B's stanza) and the kernel's `DECLARED` map grew without bound
from ring 3. Second door (the confused deputy): `R3PREF_SET` (130) is relayed by `bus_route` to PREFS.ELF,
which forwards it to `PREF_SET` under ITS OWN identity — any program could launder a write through it.

## The seam

ONE decision, in the shared core both rings link: `prefs_core::cap` (new, CHARTER: Principia — shared-core).
`wire::fulfil_as(store, verb, body, in_session, Caller, out)` runs it before the store is touched;
`wire::fulfil` is `fulfil_as(.., Caller::Kernel, ..)` (the kernel's own client, Settings, the `pref` verb, host
Principia — unchanged). The kernel transport stamps the caller: `prefs::bus_fulfil_from(.., owner, ..)` names
the program from `wm::app_name_of(owner)` (the launcher-armed name the dialog verbs already use; x86 owner =
slot + 1, aarch64 = asid) — never from the body.

| caller | SET | DECLARE |
| :-- | :-- | :-- |
| Kernel (Settings, `pref`, kernel client) | every key | any name |
| Program `prefs` (Principia's own fulfiller, PREFS.ELF) | every key | any name |
| Program `<p>` | `app.<p>.*`; `system.*` rows whose writers carry `program`; `<p>.*` (a namespace is its owner's: vein from vein) | only `<p>` |
| no armed name | the `program`-writable `system` rows | refused |

Refusals answer -EACCES. The schema's new writer `program` (rendered as the `ring 3` column of
`docs/dev/PREFS-SCHEMA.md`) is the settable column; default NOT settable. 18 `system` rows carry it
(appearance, audio mute/volume, brightness, font, font size, dock autohide/position, dnd, pointer speed,
settings tab, the five trackpad rows); `display.mode`, `display.wallpaper`, `display.idle_min`, `dock.pins`,
`login.items`, `power.lowbat_shutdown_pct`, `audio.amp_holdoff_ms` and every `vein` row are not.
`declare::insert` caps the registry at `MAX_PROGRAMS` = 64 (a 65th distinct name answers -ENOSPC on the wire;
re-declaring a held name replaces it). The deputy: `bus_route::route_request` answers `R3PREF_SET` -EACCES
unless the caller is the kernel client row (the tag exists only for Settings to reach Principia).

## Milestones

- M1 prefs_core: `cap`, `fulfil_as`, `Writer::Program` + the `ring 3` column, `declare::insert` + cap; host
  tests (the capability cases beside the fuzz); PREFS-SCHEMA.md regenerated.
- M2 kernel: `bus_fulfil_from` on both syscall arms, `KernelStore::declare` through `declare::insert`, the
  `bus_route` deputy gate; `tests prefscap`; SECURITY.md row.

## Witness

`tests prefscap` (never at boot, R80):
`:: PREFSCAP: own=ok foreign=refused system=18-settable declare_cap=64 declare_own=1 declare_foreign=refused deputy=refused -> PASS ::` (deputy=skip without `busreg`)
and at a refusal on the wire: `[prefs] set <ns>.<key> refused: program=<p> may not (PREFSCAP)`.

## Owed

Host Principia's own `PrefSet` callers are host processes (fulfil = trusted) — the host capability is the
Linux process boundary, not this arc. The name `prefs` is trusted by spelling: binding it to the system image
path (`/apps/PREFS.ELF` on the root's kernel-owned tree) is the seat's call. PrefChanged to ring 3 stays owed.
