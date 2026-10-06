# NOTIFYPANE (rmbp-ledger B435) — Settings > Notifications and the alert sound

Cut from 4ead840a (exec-rmbp-merge17). MACPARITY rows 24 (the Notifications pane) and 26 (the sound half).

## Finding
NOTIFY (B418) shows, stacks and collects every post the same way; its only preference is
`system.notify.dnd`, a General-tab box (row 8). Nothing names an app's own rule, DND has no schedule, and
no sound plays on a notification or an alert (MACPARITY 26: "an alert sound of our own" owed). The tone path
exists (`drivers/hda_play.rs`: `start` / `feed` / `finish`, driven by the device-service tick).

## The seam (R79)
- **The rules are Principia's, in the shared core**: `prefs_core::notify` (new, `no_std`, both rings) — the
  per-app stanza `system.notify.<app>.{allow,style,sound}` declared as SETTINGSFILES `DeclKey`s (checked by
  `declare::check_kind`, the doc is the file comment), the key parser, and the DND window rule
  `in_window(hour, from, until)`. One domain: `settings/notify` (`files::domain_of`, `SYSTEM_DOMAINS`).
  `notify.dnd` moves into it; `notify.dnd_from` / `notify.dnd_until` (0..23, equal = no schedule) are SCHEMA rows.
- **The kernel caches, never re-implements**: `video/notifypane.rs` (CHARTER: Kernel — wm) holds the cell —
  the apps NOTIFY has seen plus the stored stanzas, loaded ONCE at login on NOTIFY's service pass
  (`prefs_client::pref_list`), changed live by the pane, each change latched and written by that same
  service pass (never a bus call in the click or press router). NOTIFY's `pass` reads the cell with
  `try_lock` at post time (queue-only read).
- **The sound** is the hda tone path's: `hda::play::request_alert` latches an atomic; the device-service
  tick (`play::service`) synthesizes a 180 ms two-partial chime (our own, 880 + 1320 Hz, decaying) and
  feeds it through `start`/`feed`/`finish`. Refused while the output stream is already sounding (a player,
  a WAV, a dialog's own tone) — never two sounds stacked; one sound per NOTIFY pass however many posts.

## Milestones
- M1 prefs_core::notify (stanza, parser, window rule, unit tests); files.rs `notify` domain; SCHEMA rows
  dnd_from/dnd_until, settings.tab max 7; kernel prefs set checks the stanza, the file comment is its doc.
- M2 the cell (`video/notifypane.rs`): load at login, live set + latched save; NOTIFY's pass applies
  allow (blocked = not posted, not ringed), style (center = collected), the DND window, and the sound gate.
- M3 the alert tone in `hda_play.rs` (request latch, service-tick synth, busy refusal).
- M4 Settings > Notifications (tab 7 on this tree): DND box + schedule, the app rows (Allow, Banner/Center,
  Sound), the footer `settings/notify`; DND leaves the General tab.
- M5 `tests notifypane`, MACPARITY rows 24/26.

## Witness (the next flight reads)
- `[notifypane] loaded apps=<n> dnd=<0|1> dnd_window=<f>-<u> via=login`
- `[notifypane] set app=<a> <field>=<v> applied=1` then `[notifypane] saved notify.<a>.<field> ok=<0|1>`
- `[notify] post app=<a> title=<t> -> blocked(allow=0)` (an app turned off)
- `[notifypane] sound app=<a> -> requested|off(app)|dnd|busy|nopath`, then `[play] alert hz=880+1320 ms=180 eff_rate=<r> -> started`
- `tests notifypane` → `:: NOTIFYPANE: apps=<n> allow=<n> sound=<ok|none> dnd_window=<f>-<u> -> PASS :: block=ok center=ok window=ok sound_gate=ok`

## Owed
- The per-app badge control (Mac's fourth column): NOTIFY has no per-app badges, only the bell's count.
- Time-sensitive / critical alerts bypassing DND. The alert's volume is the system output's (no own level).
- Ordering of tabs at the fold: FILETYPES (B423) is tab 7 on the merged tree; Notifications follows it
  (TABS 9, `settings.tab` max 8 at the fold). Both look their index up by name (`NP_TAB`).
