# SETTINGSBUS — the kernel Settings window speaks Principia's bus (ledger B337; closes B287's last leg)

Branch `exec-rmbp-settingsbus`, cut from `8c750d43` (exec-rmbp-merge11). Seam: **fulfiller** (the kernel is
one client of Principia's verbs and the fallback fulfiller of them; Principia's ring-3 program owns them when it runs).

## Finding

PREFS (B300) gave the kernel ONE store in Principia's format and the bus verbs PREF_GET/SET/LIST (una-abi 16..18),
but the kernel's own window never used them: `video/settings.rs` read the store with direct calls into
`crate::prefs` and wrote it with direct `set` calls; the dock pins, the `wallpaper` verb, the brightness/volume key
sync and the safe-mode reset did the same. So the kernel window and a bus client (PREFS.ELF, a host Principia over
the companion wire) took two different paths to one store, and nothing told the window when a bus client wrote a
key it shows (PrefChanged, verb 19, was a serial line only — "owed BANDY-3").

## The seam

`crates/kernel/src/prefs_client.rs` — `//! CHARTER: Principia — fulfiller`. A kernel-side BUS CLIENT:
`pref_get` / `pref_set` / `pref_list` each build a v1 frame (`bus::build_request`, verbs 16/17/18), run it through
the frozen `frame_parse` + `request_validate` exactly as a ring-3 frame is, stamp it with the kernel's principal for
the WINDOW'S USER (kind 5 `user:<name>`, the aarch64 session record's wire image; the kernel stamps, never a claim),
and route it:

1. under `busreg`, the same body goes to `bus_route::route_request` under Principia's RING-3 tag
   (R3PREF_GET 128 / R3PREF_LIST 129 / R3PREF_SET 130 — new, una-abi tail) from a reserved kernel-client row; if
   PREFS.ELF holds the tag the frame is RELAYED to its mailbox (re-stamped with the window's principal) and the
   kernel-stamped answer comes back into the client's inbox (bus_route's `deliver` knows the client row);
   `via=prefs.elf`;
2. otherwise (`-ENOENT`: nobody holds the tag; or the knob is off) the kernel's own fulfiller answers —
   `prefs::bus_fulfil`, the SAME function the syscall dispatchers call for verbs 16..18 — and its reply is built and
   parsed as a real REPLY frame; `via=kernel`.

PREFS.ELF (`crates/user-prefs`) registers 128/129/130 and answers each by asking the kernel store over PREF_GET /
PREF_LIST / PREF_SET from ring 3 (its fixed v1 table stays as the fallback for its own demo keys), so a relayed write
lands in the one store under PREFS.ELF's own session principal — the fulfiller never acts AS the window.

**PrefChanged.** `prefs::changed` (every accepted write, whoever wrote it) hands the key to the client, which builds
the verb-19 frame of record (kind REPLY, corr 0, status 0, kernel principal, body `<ns>.<key>` NUL `<literal>`)
into the client's SUBSCRIPTION queue when a kernel subscriber is registered. `verb_valid` admits 19 (a ring-3
REQUEST of it is still refused `-EINVAL` by both dispatchers). The window subscribes; its service pass drains the
queue, and for a key it shows it re-reads THROUGH THE SHIM (a fresh get), updates its value, applies it (so the key
sync never writes the old live level back over another client's write) and repaints.

## Milestones

- **M1** `prefs_client.rs` (get/set/list, relay + kernel fallback, inbox, subscription, `via` counters); bus_route
  delivers to the client row; `verb_valid` admits 19; una-abi R3PREF_SET 130; PREFS.ELF registers 128..130 and
  forwards to the store.
- **M2** `video/settings.rs` rewired: every read (`from_prefs`, the load, the fixture) and every write (`persist`,
  the safe-mode reset) through the shim; PrefChanged subscription and repaint-from-fresh-get.
- **M3** the other direct writers (dock pins, the `wallpaper` verb in shell.rs) through the shim; brightness and
  volume persist through settings' `persist` (already the shim after M2). `DIRECT_WRITERS` = a compile-time count
  (const fn over `include_str!` of every file that names the store) of direct store-write call sites outside
  `prefs.rs` and the shim.
- **M4** `tests settingsbus`.

## Witness

`:: SETTINGSBUS: via=<kernel|prefs.elf> set=1 get=eq changed=1 direct_writers=0 -> PASS ::` — the fixture sets
`system.display.idle_min` through the bus client, reads it back through the window's own read path
(`settings::from_prefs`), asserts equality, asserts a PrefChanged frame for that key reached the window's
subscription and the window's shown value moved to it, then restores the operator's value. Per op:
`[prefsbus] <get|set|list> <ns.key> via=<kernel|prefs.elf> status=<n>` (set only; gets are silent).

## Owed

- Ring-3 PrefChanged delivery (an interest-registration verb so PREFS.ELF or another program gets verb 19 in its
  mailbox): the frame is built; the ring-3 subscriber table is not in this arc.
- The relayed path waits at most `RELAY_WAIT_MS` for PREFS.ELF (the desktop pass may not block on ring 3); a
  GET/LIST timeout falls back to the kernel fulfiller; a SET timeout is accepted as queued at PREFS.ELF (never re-done by the kernel, which could be overtaken by the queued frame); both are counted (`relay_timeout=`).
- Principia's real tags 16..18 stay kernel-owned in BANDY3 v1 (a REGISTER of them is `-EEXIST`); the ring-3 tags
  128..130 retire when a ruling lets a ring-3 Principia take 16..19 over.
- Read-only device-path peeks (`powerui` low-battery percent, `hda_amp` hold-off) stay try-lock reads of the cache
  (they run where no bus wait is allowed); lumen's `vein` reads likewise.
