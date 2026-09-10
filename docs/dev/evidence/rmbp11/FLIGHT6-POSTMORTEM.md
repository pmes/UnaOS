# FLIGHT 6 POSTMORTEM — rmbp 11 · image rmbp11flight6 @ f751cb78 · bench, 2026-09-03

Status: FLOWN AND SCORED (2026-09-03, bench). Boot 1 powered off by Peter at ~412 s; boot 2 ended by the `reboot` verb at 114.96 s; boot 3 carried the chords. All three boots are flight 6 (img span 0x134f198 each). Commits above f751cb78: 857c6dc8 BOOTFADT · 21b792b0 docs · bc10a469 PWRNAME.
column is filled from the wire only (`~/unaos-bench/capture/rmbp11-flight6/ttyUSB1.log`, `awk`).

## Media truth
- Staged tree: `~/unaos-bench/flash/rmbp/UnaOS-rmbp-esp-rmbp11flight6-20260903T1853Z-f751cb7/` (MANIFEST line 637)
- kernel.elf sha256 built: f71b57552727aa4d2abf0ce5419754991b7e2b4a12dfcaa775c5909dd6b970e7 (3088752 B)
- Card write: Peter wrote it himself between sessions; the card is in the laptop (reader empty), so no host sha check was possible.
- What BOOTED: **flight 6, POSITIVE match.** The wire prints no sha, so the image was scored by its mapped span:
  `WXN-x86: img=[0x7976C000,0x7AABB198)` = span 0x134f198 = `readelf -l` LOAD span of the staged flight-6 kernel.elf
  EXACTLY; flight 5's kernel spans 0x1351290 (and its own capture printed img span 0x1351290). Corroboration:
  `PRTSCR-ST` lines = 0 this boot vs 2 in flight 5's capture (the selftest is compiled out of flight 6).

## Pre-registered predictions → results
| # | prediction | witness expected | result (verbatim wire) | verdict |
|---|---|---|---|---|
| A1 | `reboot` verb reaches the x86 ladder | `[orinreboot] reboot verb invoked` → `x86 mechanism: FADT RESET_REG ladder` | boot 2, 114964 ms: `KEY: '\n'` is the LAST line of the boot; the next line is boot 3's first. ZERO `[orinreboot]` lines on the wire, all three boots. | UNOBSERVABLE on this wire (see Findings 1) |
| A2 | FADT RESET_REG is present and SystemIO 0xcf9 | `FADT RESET_REG space=SystemIO addr=0xcf9 value=0x?? — writing` | absent (never reached the cable) | UNOBSERVABLE; value= UNKNOWN |
| A3 | the write resets the machine (last line before firmware output) | firmware banner follows A2 directly | Peter: "i typed reboot and it rebooted"; boot 3 came up. Which rung fired is unknown. | RESET HAPPENED (operator witness); rung UNKNOWN |
| A3' | rung 2 (record, not fail) | `write RETURNED … trying 8042 pulse` then reset | | |
| A3'' | STOP: machine stays on | `… parking in hlt` | did not happen | not triggered |
| B1 | ⌘⇧3 decodes as GUI+Shift+digit on the boot protocol | `:: PRTSCR: [prtscr] chord=cmd-shift-3 (GUI+Shift+digit) down on EHCI -> capture armed ::` | boot 3, 384457 ms: exactly that line | **CONFIRMED — the Apple internal keyboard reports ⌘ as the boot-protocol GUI bit** |
| B2 | capture lands | `SCREEN<n>.PNG 2880x1800 … -> OK` | 455094 ms: `:: PRTSCR: SCREEN3.PNG 2880x1800 15555053 bytes -> OK ::` (stick already held flight 5's SCREEN0/1 (0 B) and SCREEN2, so n=3 — no-overwrite rule held) | CONFIRMED; n=3; **70.6 s from chord to OK** |
| B3 | no-keystroke property | one `KEYUP: '#'`, zero `KEY:` for `3` | 455101 ms `EHCI-HID: KEYUP: '#' (scancode 0x20)`; zero `KEY:` lines for 0x20/0x21 in boot 3 | CONFIRMED |
| B4 | ⌘⇧4 likewise | `chord=cmd-shift-4` → `SCREEN<n+1>.PNG` → `KEYUP: '$'` | 455140 ms chord=cmd-shift-4 → 525624 ms `SCREEN4.PNG 2880x1800 15555053 bytes -> OK` → 525631 ms `KEYUP: '$' (scancode 0x21)` | CONFIRMED; 70.5 s |
| C1 | ABSENCE: no selftest | zero `PRTSCR-ST` / `prtscrst` lines | 0 in all three boots | HOLDS |
| C2 | ABSENCE: no PNG without a chord witness | every `SCREEN` line preceded by a `chord=` line | 2 PRTSCR SCREEN lines, 2 chord lines, each chord precedes its OK; the `FS:` listing lines are the stick's directory (flight 5 leftovers) | HOLDS |
| C3 | ABSENCE: right image | zero `bar1exp` lines; `fb-wc` present | bar1exp=0 all boots; fb-wc present every boot; img span = flight 6 kernel every boot | HOLDS |
| D | reboot with a window up: composite after the reset witness? | any present/composite line between A2 and firmware output | the reset witness never reaches the cable (Finding 1) | VOID on this wire; told orin 13 |

## Boot log
- Boot 1: log starts at the fb-wc line (ring replay), desktop up ~44 s, `storm` (8 bg vugs) at 126.5 s.
  **223.7 s: BAR1 wedge on c1** — `[wcser] PASS OVERDUE holder=c1 age_ms=4000 phase=33 at=span-flush row=1167
  blit_inflight=1` (227717 ms), `[pcih] rp-at-wedge lnksta=d081`, `GATE STOLEN from c1 by c4 after 4008ms`,
  `REHOMED the render role from DEAD c1 to c2 — c1 and its in-flight window stay lost`, shell re-minted as win=2
  (WCSER-REMINT). Desktop kept running on 7 cores. **c1 is the "parked core" Peter sees**: `[schedx86]` shows
  c1=100%*(bg-user) with its switch counter frozen at 354571 from 238 s on. Expected-unchanged class (BAR1 wedge,
  convicted, unfixed, trigger = paint bursts); what is NEW on metal is that the rehome + re-mint recovery held.
  Tearing: `[wc-h]` torn is concentrated on win=2 (the re-minted shell): torn=111 banded=13085 at 394 s, rising
  ~0.5/s from 162 s; the eight vug windows sit at torn=3..10. `[wcser] declined_pct=51 -> SERIAL`,
  `[wc-w] amp=1.28x -> WIDENED`. No reboot verb and no chord yet at 394 s.
- Boot 2:
- Boot 3 (if any):

## Findings (facts first, judgement second)
1. **FADTRESET's witnesses cannot reach this machine's console — by construction.** `acpi_power::reboot`
   emits through `serial::raw_write_str` → `raw_byte`, which spins on LSR and writes THR at **port 0x3F8**
   (`arch/x86_64/serial.rs:231`). The 2012 rMBP has NO 16550; its console is the FTDI cable driven by the
   kernel's own xHCI stack, fed only by `_print` → `ftdi::mirror` (staging ring) and drained to bulk-OUT by
   `Controller::service_ftdi` in the xHCI service pass. `power::reboot` and `platform_reboot` DO go through
   `_print` (so they were mirrored into the ring), but `acpi_power::reboot` disables interrupts and writes
   RESET_REG microseconds later — the service pass never ran again, so the ring's tail (the verb line, the
   mechanism line) died with the machine, and the raw-write lines went to a port that does not exist.
   The Enter keystroke's own line (`KEY: '\n'`) was the last thing pumped. This is failure mode #4 from the
   rmbp-11 baton (the 16550 assumption) reappearing in the witness design, not in the analysis.
   Consequence: **prediction A is unfalsifiable on the wire as built**; Apple's RESET_REG/VALUE and which
   rung fired are unknown. Prediction D (a composite after the reset witness) is void for the same reason —
   the whole post-verb tail is lost, composite or not.
   Fix candidates for a follow-up commit (x86 lane): (a) BOOTFADT — run `discover_reset()` (read-only) once
   at ACPI init and print `[orinreboot] FADT RESET_REG discovered: space=… addr=… value=…` while the pump is
   alive — gives A2's facts on the next boot for free; (b) a bounded synchronous FTDI drain in `reboot()`
   before the write — needs the xHCI controller, i.e. a lock the LOCKFIX design of that path forbids; judge
   separately. (a) first.
2. **`[orinreboot]` on x86** — the family name is inherited: ORIN-REBOOT (orin's arc) created
   `power::reboot` with the `[orinreboot]`/`[orinshutoff]` token families (>8 bytes so `strings` finds them);
   FADTRESET filled the x86 arm and kept the family so one grep finds every platform's ladder. Correct
   behaviour, misleading name on a Mac. Candidate rename `[reboot]`→ needs orin's ack (their tokens, their
   tests grep them).

## PNG verification (host, stick mounted read-only at /run/media/pmes/UNAOS-DATA, 2026-09-03)
SCREEN3.PNG and SCREEN4.PNG: 15,555,053 B each; PNG signature OK; IHDR 2880x1800 depth 8 colour type 2;
3 chunks (IHDR, IDAT, IEND), IEND last; every chunk CRC valid; IDAT inflates to 15,553,800 B =
1800 * (1 + 2880*3) exactly. Copies archived in `~/unaos-bench/capture/rmbp11-flight6/`.
SCREEN0/1 (0 B) and SCREEN2 are flight 5's leftovers, which is why this flight's indices are 3 and 4.

## Anomalies not covered by a prediction
- **70 s per capture.** Chord→OK was 70.6 s and 70.5 s for a 15,555,053-byte PNG (2880x1800, stored deflate
  blocks) written to the USB stick — ~220 KB/s. The KEYUP for the digit arrived only AFTER the OK line
  (455101 vs 455094 ms), i.e. the capture ran inside the device-service pass and nothing else in that pass
  (HID polling included) moved for 70 s. Peter: "i think it was the one" — the freeze is what he felt.
  Flight 5's SCREEN2 was the same size; this is the first time the duration was measured from a keypress.
- The ⌘⇧4 chord was decoded 46 ms after SCREEN3's OK — it was pressed during the freeze and sat in the
  HID report until the pass resumed. Chords are not lost by the freeze, just deferred.

## What this changes for rmbp-12
- **Commits from this flight (hw-rmbp, above f751cb78):** `857c6dc8` BOOTFADT, `21b792b0` docs, `bc10a469` PWRNAME (boot-time FADT reset facts on
  the normal console path; QEMU value=0xf); PWRNAME (rename `[orinreboot]`/`[orinshutoff]` → `[pwrreboot]`/
  `[pwrshutoff]`, Tegra watchdog → its own `[orinwdt]`; orin 13's grant covers wdt_tegra.rs, the jetson
  spec, arroyo + lib.rs comments; acceptance = zero stale tokens incl. comments); docs (screenshot.md §9
  flight-6 bullet; PCIE-RP-RECOVERY.md's "no reboot facility" claim corrected).
- **Flight 7 predictions, pre-registered:** the boot prints `[pwrreboot] FADT RESET_REG discovered at
  boot: space=… addr=… value=…` on the rMBP — record Apple's addr/value (QEMU 0xcf9/0xf). If it prints the
  `absent` form, the `why` names which FADT check refused and the reset that flight 6 saw came from the
  8042 pulse. `reboot` at the shell still resets (operator witness); its post-verb lines are STILL expected
  absent on the cable — that is now documented, not a miss.
- **Open design question (not started):** a bounded synchronous FTDI drain before the RESET_REG write would
  make the ladder observable on the rMBP but needs the xHCI controller, i.e. a lock on a path LOCKFIX made
  lock-free. Needs a design, not a patch.
- **Capture cost:** 70 s per 2880x1800 PNG with input dead — the number an incremental/async capture arc
  has to beat.
- **Bench truth:** unattended reboots into UnaOS are impossible until the card is the default startup
  volume (⌥ picker). Chords and the reboot verb both work from the laptop's own keys.

## Constraint recorded from the bench (Peter, 2026-09-03)
**Unattended reboots into UnaOS are impossible as the machine is set up: the rMBP's firmware boots its
default volume (macOS) unless someone holds ⌥ Option at power-up for the picker.** A working `reboot`
verb therefore does not give a self-driving dev loop by itself. The rmbp-11 baton's DEV-LOOP ranking
("the reset was the expensive step") is wrong in this respect: the reset is cheap, the picker is the
human step. Any unattended-reboot plan must first make the card the DEFAULT startup volume (a macOS-side
`bless --setBoot` / Startup Disk setting, or firmware NVRAM), or it goes nowhere.
Also: peers are LIVE. A finding for orin goes to orin 13 over ccd in the same turn — not "noted for the arc".
