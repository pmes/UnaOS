# QUIETBOOT — the boot prints the boot (rmbp-ledger B311, R80)

Branch `exec-rmbp-quietboot`, cut from 3160b02a. Ruling: R80 (`docs/dev/RULINGS.md`), "tests are still
running at boot. unless it HAS to run at boot wait for me to run them. we want to see the real boot
speed now."

## Finding

Flight 19 deferred the 27 registered tests (`:: TESTS: deferred=27 fire=tests at_boot=0 ::`), but the
boot still printed everything that is NOT a registered test: periodic censuses with their own timers,
and arcs' boot-time witnesses that call their fixture directly under `#[cfg(feature = "witness")]`
(the metal image carries `witness` because the `tests` registry's fixtures are compiled under it).
`witness` means "fixtures compiled in"; `tests-at-boot` means "run them at boot". Every direct
`witness` boot call is this ruling's class.

## Seam

`CHARTER: Kernel — kernel-by-ruling` (R80). No handler's domain: the kernel's own boot output.
* WITNESS → `crate::tests::register("<name>", fn)`. Under `tests-at-boot` (every QEMU battery lane
  arms it) `register` RUNS the fixture on the spot, so a lane sees the old line in the old order
  and no spec pin moves. On metal it waits for `tests` / `tests <name>`.
* CENSUS → `crate::census` (one new file, the registry the ruling needs): a bit per sampler, default
  OFF, the sampler's code unchanged; `census list | start <name|all> | stop [<name|all>]` turns one on
  at runtime; `UNAOS_CENSUS=1` (feature `census`) boots with all on. The QEMU battery verbs arm
  `UNAOS_CENSUS` beside `UNAOS_TESTS_AT_BOOT` (arroyo's head `case`), so every pinned census line stays
  reachable on the lane that pins it.
* MEASUREMENT → `:: BOOT: firmware->loader=<ms> loader->desktop=<ms> total=<ms> lines=<n> ::`, once,
  at the first `:: FIRSTBOOT:` stage line (the boot's first screen: installer, login screen or
  desktop). BOOTCLOCK's loader stamp is reused (`bootpace::note_loader_entry`), not re-measured;
  `lines` is SERIALLOCK's own `LINES` counter. `tests quietboot` asserts `lines` under the threshold.

## M1 — the census (before any code change)

Window: from the log's first line of each boot to its first `:: FIRSTBOOT:` line.
Boot 1 = log lines 1..2711 (installer stage), 2710 lines. Boot 2 = 10716..13406 (login screen),
2690 lines. Boot 1's installer→desktop span (2711..4457, 1746 lines, human-paced: the operator set the
root password and made una) is listed as `b1 inst→desk` because the ruling's counts (SERIAL ×64,
VBJITTER ×19 …) are measured over it. Tag = the `:: WORD` after the stamp, or the `[tag]`, or the bare
first word. Counted with `awk` (tags.awk in the session scratchpad: strip the stamp, take the tag).

| tag | b1 boot | b1 inst→desk | b2 boot | class | disposition |
|---|---|---|---|---|---|
| `:: SMC-SCOUT` | 516 | 0 | 516 | KNOB-RECON | the `idx N = KEY` walk is `UNAOS_SMCWALK` — a flight-line knob, not code (drop it from the line) |
| `:: gen7` | 475 | 0 | 475 | KNOB-RECON | gen7 recon ladder, `UNAOS_GEN7*` knobs on the flight line |
| `:: kepler` | 417 | 43 | 417 | KNOB-RECON + BOOT-NEEDED | video ignition verdicts stay; the recon rungs (ctrlbind/FENCE/ucode-post …) are their knobs |
| `:: EHCI-HID` | 147 | 39 | 148 | BOOT-NEEDED + WITNESS | enumeration/arm lines stay (driver bring-up); `report-parser`/`vendor-multitouch`/`[tp] dispatch` self-tests → `tests ehci` |
| `[serwit]` | 144 | 0 | 144 | WITNESS | SERWIT-1 transport burst, the ring-3 battery's last leg → off on a deferred boot (owed: `tests ring3`) |
| `:: kdisp` | 119 | 0 | 119 | KNOB-RECON | Kepler display bring-up trace |
| `:: igpu` / `igpu-dpy` | 69 / 35 | 0 | 69 / 35 | KNOB-RECON | iGPU reachability census, IVB knobs |
| `:: KFBIND` / `KDHEAD` | 38 / 36 | 0 | 38 / 36 | KNOB-RECON | kepler rung knobs |
| `xHCI` (bare) | 38 | 135 | 38 | BOOT-NEEDED | xHCI bring-up + enumeration (the 135 are the dongle's live enumeration) |
| `[sdhc]` | 33 | 0 | 33 | BOOT-NEEDED | SD host bring-up (the boot card) |
| `:: [relics]` | 26 | 0 | 26 | WITNESS | the shell-relics TSTE leg → `tests tste` |
| `SMP` / `APIC` / `[ioapic]` | 24 / 20 / 15 | 0 | same | BOOT-NEEDED | core bring-up |
| `:: X200` / `:: EHCI-CONFIG` | 22 / 19 | 8 / 0 | 22 / 19 | KNOB-RECON | register dumps, their knobs |
| `:: PART` | 21 | 0 | 21 | BOOT-NEEDED | partition table read (mount) |
| `:: KBDWIT` | 21 | 11 | 21 | WITNESS (default-ON knob) | owed — `kbdwit` is builder default-ON; a knob-line question |
| `[clip]` / `[termsel]` | 18 / 13 | 0 | 18 / 13 | WITNESS | ride the EHCI parser self-test chain → `tests ehci` |
| `[uvc]` | 17 | 0 | 17 | KNOB-RECON | `UNAOS_UVC` |
| `[wc-h]` `[wc-x]` `[wc-d]` `[wc-b]` `[wc-g]` `[wc-a]` | 14/12/12/9/8/7 | 46/0/20/57/20/9 | ~same | WITNESS-INSTRUMENT | witness-gated compositor instruments; owed (second pass) |
| `:: TSTE` | 13 | 12 | 13 | WITNESS | midden / fatverb / vfsroute / layout / font legs → `tests tste` |
| `[NVIDIA]` / `[Intel iGPU]` | 12 / 12 | 0 | 12 / 12 | BOOT-NEEDED | GPU discovery, one line per fact |
| `:: bt-l0..l4` / `bt-c1` | 11 | 3+16+25+5+33 | 11 | KNOB-RECON | `UNAOS_BT`/`UNAOS_BTC` campaign |
| `[deadman]` | 10 | 103 | 9 | CENSUS | 1 Hz ISR rollup → `census deadman` |
| `:: HOMESOIL` | 9 | 0 | 9 | WITNESS | synthetic legs → `tests homesoil` |
| `:: STAGE-PHYS` | 9 | 0 | 9 | BOOT-NEEDED | page-table staging |
| `:: WXPROBE` | 8 | 0 | 8 | KNOB-RECON | |
| `[serwit3]` / `:: SERWIT-1` | 6 / 1 | 0 | 6 / 1 | WITNESS | ring-3 battery → off on a deferred boot |
| `:: U1a` `U1b` `U2-0a` `U2-0c` `U3` `U3.5` | 3+2+2+2+2+3 | 0 | same | WITNESS | ring-3 battery; the CR3 probe → `tests u3`, the spawns → owed `tests ring3` |
| `HEAP` (bare) | 5 | 0 | 5 | BOOT-NEEDED | the heap choice |
| `:: VBJITTER` | 2 | 17 | 2 | CENSUS | → `census vbjitter` |
| `:: WCPAR` / `[wcpar]` | 1 / 2 | 8 / 8 | 1 / 2 | CENSUS | → `census wcpar` |
| `:: SMPLOAD` / `SMPLOAD-JUDGE` | 0 / 0 | 11 / 1 | 0 | CENSUS / WITNESS | → `census smpload`; the judge fixture → `tests smpload` |
| `:: PTRSTUTTER` | 0 | 9 | 0 | CENSUS | → `census ptrstutter` |
| `:: SERIAL` | 0 | 64 | 0 | CENSUS | 1 Hz → `census serial` |
| `[wc-w]` | 0 | 101 | 0 | CENSUS | rollup → `census wcw` |
| `[mirror]` | 0 | 97 | 0 | CENSUS | tap-loss announcements → `census mirror` |
| `:: BPACE` | 0 | 128 | 0 | MEASUREMENT (kept) | the boot-phase ledger; x86-splash.spec's COMPLETE marker — see design questions |
| `[schedx86]` | 1 | 42 | 1 | CENSUS | 5 s depth/load rollup → `census schedx86` |
| `:: STACK` | 0 | 22 | 0 | CENSUS | → `census stack` |
| `[rtwit]` | 0 | 21 | 0 | CENSUS | → `census rtwit` |
| `[ptrinstall]` | 0 | 19 | 0 | CENSUS | → `census ptrinstall` |
| `:: USBNET` | 0 | 20 | 0 | BOOT-NEEDED + CENSUS | `candidate`/`up … mac= -> PASS`/`bus=xhci … link=` stay (driver verdict); the 16 `rx=N tx=N` rollups → `census usbnet` |
| `[sertx]` | 0 | 8 | 0 | CENSUS | → `census sertx` |
| `:: SDHCWR` | 0 | 2 | 0 | WITNESS | → `tests sdhcwr` (the write census, printed after each FAT write burst) |
| `:: PRTSCR*` | 0 | 4 | 0 | WITNESS | the dir-fix / refusal legs → `tests prtscr` |
| `:: SELFHOST` | 0 | 2 | 0 | WITNESS | the 24 MB SRC.TGZ verify → `tests selfhost` |
| `:: PREFS` | 0 | 1 (+1 at desktop) | 0 | BOOT-NEEDED | the LOAD line stays (the desktop reads prefs); the save/kat half is already `tests prefs` |
| `:: SHORTCUTS` / `:: LINUXABI` | 0 | 0 | 0 | already deferred | registered by R77 (`tests shortcuts`, `tests linuxabi`); 0 boot lines on flight 19 |
| `:: FIRSTBOOT` / `BOOTCLOCK` / `SPLASH` / `[vfs]` / `[users]` / `[login]` / `PART` | — | — | — | BOOT-NEEDED | stage, clock, mount, bind |

Totals: boot 1 2710, boot 2 2690. KNOB-RECON (a flight-line choice, not this arc's code) is ≈1900 of
each; CENSUS + WITNESS before the first stage ≈ 290 per boot, and ≈ 870 more on boot 1's
installer→desktop span. So the code sweep can take a boot to the first screen from ≈2700 to ≈2400
lines; the rest of R80's "real boot speed" is the knob line (see design questions).

## Milestones

* M1 — this table.
* M2 — `census.rs` + `census` verb + `UNAOS_CENSUS` knob; SERIAL VBJITTER SMPLOAD WCPAR PTRSTUTTER deadman
  wc-w mirror schedx86 STACK rtwit ptrinstall sertx usbnet-rx behind it.
* M3 — WITNESS → `tests`: `tste`, `homesoil`, `ehci`, `selfhost`, `sdhcwr`, `prtscr`, `smpload`, `u3`;
  the ring-3 spawn battery + SERWIT-1 run only under `tests-at-boot`.
* M4 — the `:: BOOT:` line + `tests quietboot`.

## Witness

`:: BOOT: firmware->loader=<ms> loader->desktop=<ms> total=<ms> lines=<n> ::` once per boot, right
after the first `:: FIRSTBOOT:` line; `tests quietboot` →
`:: QUIETBOOT: lines=<n> bound=<B> census=<on-bits> -> PASS|FAIL ::`.

## Built (branch exec-rmbp-quietboot)

* M2 `census.rs` + gates (one same-line statement per sampler): `serial_line::census_poll`,
  `kepler_vblank::vbjitter_witness`, `sched::emit_smpload_witness` / `emit_stack_witness` /
  `emit_load_witness`, `wcpar::emit`, `ehci::ptrstutter_witness`, `deadman::tick`, `screen` `[wc-w]` rollup,
  `serial_ring::mirror_service` / `tx_rollup`, `rtwit::rollup`, `usbnet::rollup`, `sdhc::wr_burst_flush`,
  main.rs `[schedx86] depth` + `ptrinstall_rollup`. Verb row in `midden_core` HOST_VERBS + help row + dispatch arm.
  Knob: Cargo `census`, arroyo `_feats` + head-`case` export for the battery verbs, builder push, K8_FEATS arm,
  `banner-cert.sh` row `census|:: CENSUS: armed=all|-|measured(1)` (measured on a metal+census ELF).
* M3 `tests::defer(name, f)`: first statement of the fixture; inline under `tests-at-boot` or while `tests`
  runs, else registered once. Sites: `shell::midden_witness` (tste), `shell::fatverb_storage_witness`,
  `bootdisk::homesoil_selftest`, `ehci` parser chain (ehci), `selfhost::verify_source_once`,
  `prtscr::selftest_once`, `sched::smpload_selftest`; main.rs ring-3 block: `sched::enable()` stays, the
  U1a..U3.5 + SERWIT-1 body runs only under `tests-at-boot`, else `tests u3` (= `u3_probe_once`). Registry
  CAP 48 → 80. `test-selfhost` re-execs with `UNAOS_TESTS_AT_BOOT=1`; `x86-witness.spec` (metal) names
  `UNAOS_TESTS_AT_BOOT=1 UNAOS_CENSUS=1` in its Build line.
* M4 `bootpace::boot_line()` from `users::stage_witness` (first call only), `note_loader_entry` from
  `bootclock_report`, a lock-free per-tag tally in `serial_line` (48 slots, closed at the BOOT line),
  `tests quietboot` (bound 2500, SKIP on tests-at-boot/census builds, prints `top=[tag:n,…]` when over).

## Expectation vs flight 19

Boot 1 to the installer stage: 2710 lines before. Taken out by this arc on the same knob line: SERWIT/U-battery
~165, TSTE+relics ~39, EHCI self-test chain (+clip/termsel/keymap/keyrepeat/tpframe/appclip) ~45, HOMESOIL 9,
deadman/schedx86/etc ~15 → expected ~2420, under the 2500 bound. Installer→desktop (1746) loses ~870
(SERIAL 64, BPACE stays, deadman 103, wc-w 101, mirror 97, schedx86 42, STACK 22, rtwit 21, ptrinstall 19,
USBNET rx 16, VBJITTER 17, SMPLOAD 11, WCPAR+wcpar 16, PTRSTUTTER 9, sertx 8, SDHCWR 2, PRTSCR 4, SELFHOST 2).
Without the recon knobs (SMCWALK, gen7, kepler rungs, KFBIND/KDHEAD, BT/BTC, wifi, uvc, EHCI-CONFIG) the
first stage should come in near 500 lines.

## Owed / design questions

1. The ~1900 KNOB-RECON lines are the flight line's choice; R80's "real boot speed" flight wants the recon
   knobs off (UNAOS_SMCWALK alone is 516 lines). A seat decision, not code.
2. BPACE (128 reprints on boot 1) is kept: it is the boot-phase ledger and x86-splash.spec's COMPLETE marker.
   Gating it needs the splash lane to arm UNAOS_CENSUS first.
3. `[deadman]` is now a census: a wedge on a quiet boot has no 1 Hz line unless `census start deadman` or
   UNAOS_CENSUS. Seat: keep it always-on?
4. `tests ring3` (the U1a..U3.5 spawn battery from the shell) is not built: it awaits verdicts on a worker
   core at early boot; driving it from the desktop needs the TSTE-2 launcher refactor.
5. Witness-gated compositor instruments (`[wc-h]` `[wc-b]` `[wc-d]` `[wc-g]` `[wc-a]`), KBDWIT (builder
   default-ON) and the `[sdhc] write census` are a second pass.
6. Fixtures with an internal once-latch (fatverb, prtscr, smpload, selfhost) print on the FIRST `tests` run only.
7. The BOOT line rides `fs/users.rs` (FIRSTBOOT): a build without `login` prints none.
