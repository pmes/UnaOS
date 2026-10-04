# TESTFIX3 — the FLIGHT 19 `tests` fails, fixed (B315)

Branch `exec-rmbp-testfix3`, cut from 3160b02a. Source: FLIGHT19.md §2, `f19-boots.log`
(`:: TESTS: ran=45 pass=170 fail=9 failed=[helpdoc,bandy3,prefs,playwav,clickroute,trash] ::`).
PLAYWAV belongs to AUDIO7 and is not touched here.

| test | failed line (abridged) | cause | fix | kind |
|---|---|---|---|---|
| helpdoc | `HELPVERB: … missing=[battery]` | `battery` verb had no DOCS row | row added (help.rs) | real (docs) |
| bandy3 | `w=0x1f7/0x1ff` (bit 3) | bit 3 probed tag 11 as "unassigned"; merge9 gave 11 to ATTR_SET | probe 126; pin ATTR_SET/PREF_GET valid | fixture |
| prefs | `PREFS-CODEC: kats=8/9`; `PREFS: … loaded=0 saved=0 -> FAIL` | PREF_GET golden verb byte 11 (now 16); the PREFS FAIL line was the fixture's own malformed-file leg printing the load witness | golden 16; that reload says `[prefs] fixture malformed reload … (expected refusal)`; counters restored | fixture |
| trash | `TRASH: trashed=2 restored=1 emptied=1 index_ok=3 -> FAIL` | collision leg: `TRASHFX.TXT~1` is not 8.3; FAT `move_entry` refuses it (`reason=Unsupported`) | `collision_name` → `TRASHF~1.TXT` on an 8.3 leaf | real op bug |
| clickroute | `deliver=false depth=0/0 kernel=false desktop=false` | `tests power` injects 7 %; `lowbat_service` posted a REAL Low Battery alert (login-screen modal), which swallowed every later press (`[login] press … swallowed=2`) | `status::reading_is_fixture()`; the monitor ignores fixture readings | real (fixture leak into the live monitor) |
| move-vacate | `painted=false desktop=0/5 stale=0/5` (boot) | probe ran under the full-panel splash hold | now `tests movevacate`; SKIP while a modal row holds the top | fixture ordering |
| WCPAR | `bands=126 serial_us=1821 speedup_pct=16 load=idle -> FAIL` | 14 us of work per band — dispatch-bound; not a regression | idle floor applies only at >= 20 us/band | verdict rule |

Witness shapes are unchanged. Expected on the next `tests`: `HELPVERB … missing=[] … -> PASS`,
`BANDY3: … -> PASS`, `PREFS-CODEC: kats=9/9 -> PASS`, `PREFS-FIXTURE: codec=1 … -> PASS` with no
`:: PREFS: … -> FAIL` between, `TRASH: … -> PASS` with `[fs] mv … TRASHF~1.TXT`, no
`:: POWER-UI: lowbat notice` during `tests power`, `[clickroute] route … -> PASS`, and
`[wc-x] move-vacate … -> PASS` under `TESTS: run movevacate`.

Owed: the clickroute verdict and the knock-on fails (TERMSEL2, WINMENU, PULSEQUIT, APPQUIT,
QUARRYDOOR, APPPIN, SHOTMENU) need a metal `tests` to confirm. The other test-run fails (kvblank
bound, TERMSEL2 legs once the press lands) are not in this arc. TRASHTIME carries the collision fix.
