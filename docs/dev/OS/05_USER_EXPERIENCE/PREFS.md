# PREFS — how a preference reaches the code that paints it

Status: the kernel store **flown** (it loads on every boot of flights 32/33, §1); SETTINGSBUS fixed-unflown at the
merge12 fold (cfc307f2), PREFSKERNEL likewise ("unflown at the fold, the fixture passes on flight 22", B345's status
cell); the ring-3 theme reads (UNAKEYS) fixed-unflown; **ring-3 PrefChanged
delivery OWED** (queued as PREFCHANGED, `docs/dev/OS/rmbp-queue.md:17`). The keys themselves are
[`docs/dev/PREFS-SCHEMA.md`](../../PREFS-SCHEMA.md) (generated). Ledger rows (`docs/dev/OS/rmbp-ledger.md`): B337
SETTINGSBUS (line 371), B345 PREFSKERNEL (379), B552 UNAKEYS (584); the store's history B300 PREFS, B407
SETTINGSFILES, B408 APPEARANCE (`docs/dev/LEDGER.md` lines 333, 440, 441).

Principia owns every settings and preference decision (`prefs_client.rs` header: "Principia owns every settings and
preference decision (LAWS §Handler manifest)"). Nothing below decides a preference; it carries one.

## 1. The one store

- **One store, Principia's format.** `unaos/crates/kernel/src/prefs.rs` reads and writes the same store
  `handlers/principia` owns on the host; the value model and codec are `unaos/libs/sys/prefs_core`, the crate both
  link, so the two rings cannot drift (prefs.rs header).
- **A folder of files** (SETTINGSFILES B407, R98): `<home>/settings/<domain>` — `display`, `login`, `desktop`,
  `sound`, `trackpad`, `general`, and one per program; `prefs_core::files::domain_of` is the rule. A write rewrites
  only its domain's file; a file the user deletes resets that domain at the next service read (prefs.rs header).
- **One clamp, one fulfiller** (PREFSKERNEL B345). `prefs::set` validates through `prefs_core::schema::check`
  (clamped values answered `clamped=true`); `prefs::bus_fulfil` (prefs.rs:848) is a thin call into
  `prefs_core::wire::fulfil` — one body for the kernel, the host and the reference tree. A load clamps out-of-range
  file values once (`[prefs] load clamped=<n>`).
- **Loaded once per login.** `get` never does I/O (prefs.rs header). On the wire, before and after login:

```
f33-boot2.log:125  [05:59:57Z] [prefs] load clamped=0
f33-boot2.log:126  [05:59:57Z] :: PREFS: path=/settings domains=0 loaded=0 saved=0 ns=system -> loaded ::
f33-boot2.log:386  [06:00:17Z] :: PREFS: path=/home/una/settings domains=0 loaded=0 saved=0 ns=system -> loaded ::
f33-boot2.log:4096 [06:08:29Z] :: PREFS: path=/home/una/settings domains=1 loaded=0 saved=1 ns=system -> loaded ::
```

(all under `docs/dev/evidence/rmbp-0915/flight33/`). The path moved from `/home/una/.config/unaos/preferences.toml`
on flight 22 (`flight22/f22-boots.log:1016`) to the settings folder.

## 2. The bus: PREF_GET / SET / LIST and PrefChanged (SETTINGSBUS, B337)

The verbs are una-abi 16 PREF_GET, 17 PREF_SET, 18 PREF_LIST, 19 PREF_CHANGED (`una-abi/src/lib.rs:851–858`). Before B337 the kernel's own Settings
window called `prefs.rs` directly while host Principia spoke the bus — two writers that could disagree. Now the
kernel desktop (the Settings window, the dock pins, the `wallpaper` verb, the brightness and volume key sync) is a
bus CLIENT, `unaos/crates/kernel/src/prefs_client.rs` (`pref_get` :312, `pref_set` :322, `pref_list` :335).

Each call builds a v1 frame and runs it through the frozen `frame_parse` + `request_validate` exactly as a ring-3
frame is checked, then (prefs_client.rs header):

1. under `busreg`, offers it to `bus_route::route_request` under Principia's ring-3 tag (R3PREF_GET 128 / LIST 129 /
   SET 130) from the reserved kernel-client row `KCLIENT_ROW`, stamped with the window's user (`user:<name>`,
   kind 5 — the kernel stamps, nothing claims). If PREFS.ELF (`crates/user-prefs`) holds the tag it answers:
   `via=prefs.elf`;
2. otherwise (nobody holds the tag, a full mailbox, the knob off, or no answer inside `RELAY_WAIT_MS` = 50 ms,
   prefs_client.rs:51) the kernel's `prefs::bus_fulfil` answers — the same function both syscall dispatchers call
   for 16..18: `via=kernel`. A relayed SET that times out is NOT redone by the kernel (it is queued at PREFS.ELF and
   lands in order).

**PrefChanged (kernel side).** `prefs::changed` — every accepted write, whoever wrote it — calls `on_changed`
(prefs_client.rs:164), which builds the verb-19 frame of record (kind REPLY, corr 0, status 0, body `<ns>.<key>`
NUL `<literal>`) into the subscription queue (depth `Q_CAP` = 16) while a kernel subscriber is registered
(`subscribe`, :158). The Settings window drains it (`changes_drain`, :190), re-reads the key with a fresh get, applies
it (so the key sync never writes an old level back over another client's write) and repaints
(`docs/dev/evidence/rmbp-1005/SETTINGSBUS.md`, "PrefChanged").

**Wire.** `[prefsbus] set <ns.key>=<v> via=<kernel|prefs.elf> status=<n>` per set (gets are silent; prefs_client.rs
:327–328); `[prefsbus] relay verb= corr= timeout=50ms — …` on a relay timeout (:254, :257); fixture `:: SETTINGSBUS:
via=<kernel|prefs.elf> set=1 get=eq changed=1 direct_writers=0 -> PASS ::` (SETTINGSBUS.md, Witness). No
`[prefsbus]` or `:: SETTINGSBUS` line appears in the pinned captures read for this doc (flights 22, 30–33).

## 3. Ring 3 reads the theme (UNAKEYS, B552)

AETHER.ELF, UNA.ELF, MIDDEN.ELF and LUMEN.ELF take their colours from Principia's store, not from literals. Each crate
has one `theme.rs` (e.g. `unaos/crates/user-una/src/theme.rs`), the only file in the crate that spells a colour (the
appearance gate's rule, `scripts/appearance-check.py`, applied to ring 3). It is a bus client:

- **What.** `system.appearance.mode` / `.accent` / `.highlight` over PREF_GET (16); UNA.ELF also PREF_LIST (18) of
  `tabula` for Tabula's `tabula.theme.<kind>` map. `auto` resolves by SYS_TIME's hour as
  `video::appearance::local_hour` does. With an unset store the four paint the Light set, Principia's default (B552).
- **The palette** is the kernel's (`video/theme.rs` LIGHT / DARK rows and the eight ACCENTS), COPIED into each
  `theme.rs` because ring 3 links no kernel file (B552; owed: a shared no_std palette core — PALETTECORE is queued,
  rmbp-queue.md:17).
- **When.** `load` once at start; then `due` from the frame loop every `POLL_MS` = 2000 ms
  (user-una/src/theme.rs:28, `due` at :338), and at once when a verb-19 frame is met in the mailbox while a reply is
  awaited. A re-read that moves the look repaints everything.
- **Why a poll.** Ring-3 delivery of verb 19 is owed by the kernel (prefs_client.rs header: "Ring-3 delivery of
  verb 19 (an interest-registration verb) is OWED"), and SYS_MRECV blocks on an empty mailbox, so a program cannot
  wait for it beside its input ring (user-una/src/theme.rs header). A ring-3 REQUEST of verb 19 is still refused
  `-EINVAL` by both dispatchers (SETTINGSBUS.md, "PrefChanged").
- **Wire (expected, unflown):** `:: UNA (or AETHER, MIDDEN, LUMEN): theme mode=light/dark accent=<name>
  bus=0/-errno ::` at start and on each change (B552).

## 4. Open

- **PREFCHANGED** (queued, rmbp-queue.md:17): the ring-3 subscriber table and interest-registration verb, so verb 19
  reaches a program's mailbox and the 2 s poll retires.
- **PALETTECORE** (queued, same line): one no_std palette both rings link, so the four copied palettes cannot drift.
- The ring-3 tags 128..130 retire when a ruling lets a ring-3 Principia take 16..19 (SETTINGSBUS.md, Owed).
- Read-only device-path peeks (`powerui` low-battery percent, `hda_amp` hold-off, lumen's `vein`) stay try-lock reads
  of the cache, where no bus wait is allowed (SETTINGSBUS.md, Owed).
