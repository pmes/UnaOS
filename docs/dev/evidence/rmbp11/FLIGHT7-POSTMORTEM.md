# FLIGHT 7 POSTMORTEM — rmbp 11 · image rmbp11flight7 @ bc10a469 · bench, 2026-09-03

Status: FLOWN 2026-09-03 (bench, one boot). Scored positive: `WXN-x86 img=[0x79761000,0x7AAB0CA8)` = span 0x134fca8 = the staged flight-7 kernel's LOAD span. Results come from the wire only (`~/unaos-bench/capture/rmbp11-flight7/`, `awk`).

## Media truth
- Staged tree: `~/unaos-bench/flash/rmbp/UnaOS-rmbp-esp-rmbp11flight7-20260903T1952Z-bc10a46/` (MANIFEST line 638)
- kernel.elf sha256 built: d31a99915e39f7e0d747f1feecf919b8e37b5283b6ffa317be862dd31ffd773e (3090024 B)
- `readelf -l` LOAD span (for scoring what booted): (fill before the boot)
- Card write verified: (fill)
- What BOOTED (WXN-x86 img span): (fill — score by this)

## Pre-registered predictions → results
| # | prediction | witness expected | result (verbatim wire) | verdict |
|---|---|---|---|---|
| 1a | FADT reset facts reach the cable | `[pwrreboot] FADT RESET_REG discovered at boot: space=… addr=… value=…` | `[pwrreboot] FADT RESET_REG discovered at boot: space=SystemIO addr=0xcf9 value=0x6 — the reboot verb will write this` | **CONFIRMED; Apple RESET_VALUE = 0x6** (QEMU 0xf), exactly the pre-registered expectation |
| 1b | or the honest absence | `[pwrreboot] FADT RESET_REG absent at boot — … (why: …)` | not printed | n/a |
| 1c | neither ⇒ bracket it | `ACPI: 8 CPU(s) discovered` and the PM-timer report present around it | | |
| 2 | PWRNAME: no stale token | zero `orinreboot`/`orinshutoff` lines | 0 | HOLDS |
| 3 | absence controls | no `PRTSCR-ST`, no `bar1exp`, `fb-wc` present | 0 / 0 / 11 | HOLDS |
| 4 | (optional) `reboot` resets again; ladder lines absent on cable as documented | operator witness | | |

## Findings
1. Apple's FADT on the 2012 rMBP (MacBookPro10,1) carries a valid RESET_REG: revision ≥ 2, checksum clean,
   RESET_REG_SUP set, SystemIO 0xcf9, RESET_VALUE 0x6 (the PCH RST_CNT "full reset" encoding; QEMU's ICH9
   emulation publishes 0xf). So flight 6's reset was rung 1 of the ladder — the FADT write — unless the write
   returned and the 8042 pulse did it, which the cable cannot distinguish (documented). No 8042 fallback was
   needed on this platform.
## Anomalies not covered by a prediction
## What this changes for rmbp-12
flight7 LOAD span=0x134fca8
