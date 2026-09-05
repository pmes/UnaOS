# render3b FLIGHT RESULT — 2026-09-05, image render3b-20260905T1835Z-7fb1d5d (kernel bytes hw-jetson fef6a184)

Pinned `KELF min=0x0 max=0x2d3488` (raw.log; excerpt `render3b-boot1.log`, anchor first). Preceded by
render3 (`max=0x2d2fd8`, same code minus APTEXT) dying twice at `ORIN-SMP-3 enumerated core 5` with
`Exception reason=1 syndrome=0x82000010` + IOB/ACI RAS + `Powering off core` (excerpt `render3-boot1.log`).

| # | question | result | evidence (excerpt line) |
|---|---|---|---|
| 0 | CPU_ON after APTEXT | **PASS** — `CPU_ON AP 1..5 -> SUCCESS`, `5/5 secondaries online via PSCI CPU_ON` | 397 |
| 1 | boot-stack through the cascade | **`hw=15472 headroom=17296`** (pre-cascade hw=240; 32 KiB window; unsaturated) — §5.2's number | 416, 478 |
| 2 | the cascade | **CASCADED** `windows=1 bar=1 owns_pixels=1 route=ROUTED`; `[pidesk] menubar ENABLED … PAINTED owns_pixels=true`; `crystal LIVE`; `[dock] live` | 415, 460, 466, 467, 479 |
| 3 | shell in its window | console windowed `win=1 … at (307,158) cols=185 rows=46`; render pass `DECLINE reason=console-already-windowed` (correct: the routed console IS the shell window) | 444, 507 |
| 4 | strip / presents | `SCHED: load c0=85%`; `[pstrip] redraws=24 srcdelta=23`; `presents=29` climbing (render2: 2, static) | tail |
| 5 | serial RX | `[serialrx] lsr=0x200 -> RX-LIVE`; `tste\r` → `KEY 's'`,`KEY 't'`,`KEY 0x0d`, `rx=3 (+3)` — 2 of 5 bytes lost to holding-register overrun (A16) | 498, post-tste tail |
| 6 | Print Screen | pending Peter's press at scoring time | — |
| 7 | pulse window | 0 `[pulsewin] open` lines (retired) | — |

Watcher note: the watcher's "DIED" trigger fired on a `Powering off core` match that is NOT in the
excerpt — a false positive from its raw-tail window; the board was alive and reached RENDER-LIVE.
