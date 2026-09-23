# METALPINS M1 — `x86-witness.spec` as-is (94e90eae) replayed on flights 12 and 11

rmbp-ledger B195. Spec at `exec-rmbp-metalpins` parent `94e90eae`: 1528 lines, 92 directives (42 REQUIRE, 38 FORBID,
6 OPTIONAL, 4 PENDING, 2 COMPLETE; counted with `grep -oE '^[A-Z]+' | sort | uniq -c`). Captures are READ-ONLY copies
taken into the executor's scratch before any replay:

| flight | image | source | lines | sha256 |
|---|---|---|---|---|
| 12 | 4, hw-rmbp@6d8d3d2d | `~/unaos-bench/scratch/rmbp-0915/logs/foldgate/f12-boot1.log` | 6899 | `2fb8f05506d222b5cce3ba502cea524ae1b989633b7b2adaa6849a446808b49d` |
| 11 | 3, hw-rmbp@56bbe53b | `~/unaos-bench/scratch/rmbp-0915/gmux7-logs/f11.log` | 5597 | `bee186f1d91b33dd5ccf39e08b646b0a8b7d652a5259ea9372197188ac7fa8ed` |

Command, from `unaos/`: `./arroyo mbench --replay <copy> --spec scripts/specs/x86-witness.spec --platform x86 > <log> 2>&1; echo rc=$?`.
Full tables: `m1-f12-baseline.txt`, `m1-f11-baseline.txt` (ANSI stripped; the first line is `arroyo`'s own banner).

## Verdict lines

- flight 12, rc=1: `❌ MBENCH FAIL — 37/42 required witnesses, 19 forbidden hit(s), 6899 lines scanned, pending 3/4 matched [mode unknown: no run sidecar]` — FIRST-SHORTFALL `x86-witness.spec:322`; end-of-run marker `PULSE-A` seen @ line 4330.
- flight 11, rc=1: `❌ MBENCH FAIL — 36/42 required witnesses, 18 forbidden hit(s), 5597 lines scanned, pending 4/4 matched [mode unknown: no run sidecar]` — FIRST-SHORTFALL `x86-witness.spec:182`; `PULSE-A` seen @ line 4073.

## Every red on flight 12, classified

"Voided" = a press-driven fixture that ran while the login window held every press (`[login] screen open window=2` at
8358 ms; `[login] press at=(1158,491) control=none answered=0 swallowed=1` at 48497 ms). "Real" = the capture shows the
absence or failure independent of the window.

| # | directive (spec line) | hits | class | reading |
|---|---|---|---|---|
| 1 | REQUIRE `[wc-d] verify win=1 … coverage=full … -> PASS` (:322) | 0 | real, both flights | the `UNAOS_WCDVALVE` valve reads CLOSED 6745→32164 ms and 32323 ms→end; the win=1 `-> DEFERRED` at 6677 ms is never paid |
| 2 | REQUIRE `[wc-d] paygo … state=complete … budget=2 -> PAID` (:335) | 0 | real, both flights | same valve reading |
| 3 | REQUIRE `[drag-occ] … bars= … fillclip_dock_px=` (:1335) | 0 | voided | no `[drag-occ]` line at all; `[wm-act] direct … grab=false route=false … -> FAIL`; flight 11 carries 4 |
| 4 | REQUIRE `:: DOCK: … vacate=true … :: PASS ::` (:1406) | 0 | voided AND real | FLIGHT12.md §Fixtures names it; flight 11 (no window) reads the same `vacate=false` FAIL — B187/DOCKVAC |
| 5 | REQUIRE `:: CLICK-BAND: … band_lines=3 :: PASS ::` (:1417) | 0 | voided | `band_lines=0 :: FAIL ::` at 50015 ms; PASS on flight 11 |
| 6 | FORBID `:: EHCI-HID: \[1\] EPACE-TRIM M8 SLOW-XFER` (:807) | 2 | real | at 3832 ms, before the window opened: `addr=8 hub=3.2 spd=FS … wlen=8 stg=3 xfer=2000ms` (the bcm5974's address, `[tp] mode … addr=8`); 0 on flight 11 |
| 7 | FORBID `:: DOCK: .* :: FAIL ::` (:1407) | 1 | = row 4 | |
| 8 | FORBID `:: CLICK-BAND: .* :: FAIL ::` (:1418) | 1 | = row 5 | |
| 9 | default FORBID `-> FAIL` | 3 | see below | `[clickroute] … deliver=false -> FAIL`, `[wm-act] direct … -> FAIL`, GLASSFIX2 |
| 10 | default FORBID `FAIL ::` | 12 | see below | the 12 `:: NAME: … FAIL ::` lines |

The 14 distinct FAIL-bearing lines behind rows 9-10 (`awk 'index($0,"-> FAIL") || index($0,"FAIL ::")'`):

| line | fixture | class |
|---|---|---|
| 3494 | `[clickroute] route hit=true deliver=false … -> FAIL` | voided — in FLIGHT12.md's nine |
| 3509 | `:: MENUBATT: … jitter_paint=false change_paint=false damage_ok=false :: FAIL ::` | NOT in the nine; not press-driven (`menubar.rs` leg 6, `compose()` declined four retries); fixture new in image 4 (0 lines on flight 11); cause not established from the wire |
| 3510 | `:: MENUFIRST: … crystal=absent … painted=false :: FAIL ::` | voided — in the nine |
| 3545 | `:: DOCK: … vacate=false :: FAIL ::` | voided — in the nine (and real, row 4) |
| 3760 | `:: WINMENU: … routed_open=false … :: FAIL ::` | NOT in the nine; press-driven, PASS on flight 11 — same class as the nine |
| 3792 | `:: PULSEQUIT: … quit_routed=false … :: FAIL ::` | NOT in the nine; press-driven, PASS on flight 11 — same class |
| 3812 | `:: APPQUIT: … :: FAIL ::` | voided — in the nine |
| 3882 | `:: CLICK-BAND: … band_lines=0 :: FAIL ::` | voided — in the nine |
| 3898 | `:: MENUDROP: … open=false … :: FAIL ::` | voided — in the nine |
| 3932 | `:: SERIALDOOR: … control=false wire=false … :: FAIL ::` | voided — in the nine |
| 3999 | `[wm-act] direct partition=true grab=false … -> FAIL` | NOT in the nine; drag-driven, PASS on flight 11 — same class |
| 4125 | `:: GLASSFIX2: … cascade overlaps=14 worst=win1-over-win3:39rows … -> FAIL ::` | REAL, both flights (flight 11: `overlaps=8 … -> FAIL ::`); FLIGHT12.md §Fixtures lists GLASSFIX2 under PASS, which the wire contradicts |
| 4181 | `:: APPPIN: … :: FAIL ::` | voided — in the nine (also FAIL on flight 11) |
| 4354 | `:: SHOTMENU: … :: FAIL ::` | voided — in the nine (also FAIL on flight 11) |

## Flight 11's reds (for the go-red column), not classified further

Shortfalls :182, :199, :322, :329, :335, :1406; forbidden (18): `-> FAIL` 3 (HDA-TONE, `[dmgovlp] verdict`, GLASSFIX2),
`FAIL ::` 7 (HDA-TONE, DOCK, DOCKID, APPQUIT, GLASSFIX2, APPPIN, SHOTMENU), `[wc-g] … -> RACE` 3, `[wc-g] … -> BLIT` 4,
`:: DOCK: .* :: FAIL ::` 1.

## Two FLIGHT12.md readings the wire contradicts

- GLASSFIX2 is `-> FAIL ::` on flight 12 (line 4125), not a PASS.
- `[hda] codec=0 vid=1013:4206` is not the first read of the Cirrus codec: flight 11 prints the same line at 25738 ms.
