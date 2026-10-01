# RTCCLOCK (R75) — the CMOS RTC as a clock source

**Finding.** Flights 17-18: the menubar clock stayed `--:--` the whole boot (SNTP had no lease). TESTFIX2: "there is no RTC
source in ClockSource, so from=rtc cannot occur" (`video/menubar.rs` CLOCKBAR).

**Mechanism.** `arch/x86_64/rtc.rs` (new): `read()`/`read_full()` — UIP wait, two agreeing snapshots, BCD/12h decode per status B,
century from FADT byte 108 (`fadt`) else 20xx (`assumed`); `write()` inverse (SET bit, BCD per status B). `clock.rs`:
`ClockSource::Rtc`, `unix_from_civil` (inverse of `civil_from_unix`), `anchor_from_rtc` (declines under an Sntp/Manual anchor, so
priority sntp > verb > rtc; SNTP/`set_anchor` always replace it), `TZ_MIN` (`UNAOS_TZ_MIN`, option_env, default 0), `rtc_boot()`
called from main.rs right after `bootpace::record("calib")` (TSC must be calibrated so the anchor ticks). `clock::set` (`date -s`)
writes the CMOS back as shown-minus-TZ_MIN and prints `[rtc] written`. CLOCKBAR/time/date print `rtc`. Non-x86: `:: RTC: rtc=none -> PASS ::`.

**M3.** `fat_stamp()` already prefers `unix_now()` (clock.rs), and every FAT writer (fat.rs 3254/3390/3470/3536/5819/6909) stamps from it,
so an RTC anchor stamps file mtimes and `ls -l`/Show Info with no fat.rs change. (Seen: no code change needed; boot-17 `ls -l` confirms.)

**Witness.** `:: RTC: y= mo= d= h= mi= s= bcd= h24= century=fadt|assumed tz_min= -> PASS|FAIL ::`.
**Pins.** x86-default.spec REQUIRE `:: RTC: y=` + FORBID FAIL; x86-wc.spec CLOCKBAR FORBID `from=placeholder` (rtc accepted).
**Knob.** UNAOS_TZ_MIN: arroyo comment + builder comment + k8-reach.registry row (option_env, no feature).

## Written
Boot 17 should show `:: RTC: y=2026 ... century=fadt|assumed tz_min=0 -> PASS ::` early, then `:: CLOCKBAR: anchored=1 text=HH:MM from=rtc drawn=1 -> PASS ::`
from the first draw; after `date -s ...`: `[rtc] written y= ... ok=1`; a reboot keeps the set time.
