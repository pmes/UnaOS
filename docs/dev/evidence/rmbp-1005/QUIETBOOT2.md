# QUIETBOOT2 — the boot prints its stages and its refusals (rmbp-ledger B325, R80)

Branch `exec-rmbp-quietboot2`, cut from 4e48ab03 (merge11). Ruling R80: "tests are still running at
boot. unless it HAS to run at boot wait for me to run them. we want to see the real boot speed now."
Peter on boot 20: "it is still running a bunch of stuff at boot."

## Finding

Boot 20 (`docs/dev/evidence/rmbp-0915/flight20/f20-boots.log`, boot 1) printed 2433 wire lines from its
first line to `:: BOOT: firmware->loader=13679ms loader->desktop=10324ms total=24004ms lines=2615 ::`
(the kernel's own counter says 2615; ~180 lines precede the capture). The image's judge said
`:: QUIETBOOT: lines=2615 bound=2500 census=0 -> FAIL ::` — the tree's bound was 2500, not 250, and its
`top=` named neither SMC-SCOUT nor gen7 because the 48-slot tag tally was full before they spoke.

The census splits into two populations, measured by rebuilding the brief's x86 metal shape (plus the
builder's default-ON `ehcihid,kbdwit,smolnet`) at 4e48ab03 and asking the release ELF whether each wire
line's format literal is in it (scratchpad `reach.py`; `LC_ALL=C grep -a -F` on the artifact):

* **~1,880 lines come from flight-line KNOBS that are not in the metal shape** (the literal is not in
  the image): `smcwalk` (493 `idx N = KEY` lines), `gen7` (475), the Kepler rungs
  (`nvidia-kepler-fifo/kfbind/kdhead/ce/ctrladdr/ctrlbind`, ~400 of the 417 `kepler` + KFBIND + KDHEAD),
  the iGPU display ladder (104), `ioapic` (14), `uvc` (17), `bt` (~13), X200 (22). No code change
  silences these; the flight line does (design question 1).
* **~550–670 lines are reachable on the metal shape**, and they are this arc's sweep.

## M1 — the census (boot 20, boot 1, wire lines 1..2433)

Tag = the `:: WORD` after the stamp, the `[tag]`, or the bare first word (`awk`/python over the wire).
`reach` = lines whose literal is in the metal-shape ELF at 4e48ab03.

| # | tag | lines | reach | disposition |
|---|---|---|---|---|
| 1 | `:: SMC-SCOUT` | 516 | 8 | KNOB `smcwalk` (493 idx lines); the 19 key/begin/end lines → CENSUS |
| 2 | `:: gen7` | 475 | 0 | KNOB `gen7` (GEN7R8 owns the ladder; not touched) |
| 3 | `:: kepler` | 417 | 18 | KNOB Kepler rungs; `mirror-hdr`/`fecs-ledger` → CENSUS; `vblank …` (13) left to KVBLANK8 |
| 4 | `:: EHCI-HID` | 144 | 144 | PROSE → bootlog (errors/STOP-NOTEs stay); ISRARM/PASSPERIOD self-tests → FIXTURE; ISRARM rollup → CENSUS |
| 5 | `:: kdisp` | 119 | 94 | CENSUS (Kepler display register walk) |
| 6 | `:: igpu` | 69 | 0 | KNOB (iGPU ladder) |
| 7 | `xHCI` (bare) | 38 | 38 | PROSE → bootlog (errors, `xHCI: Disk`, `xHCI: KEY` stay) |
| 8 | `:: KFBIND` | 38 | 0 | KNOB `nvidia-kepler-kfbind` |
| 9 | `:: KDHEAD` | 36 | 0 | KNOB `nvidia-kepler-kdhead` |
| 10 | `:: igpu-dpy` | 35 | 0 | KNOB |
| 11 | `[sdhc]` | 33 | 33 | PROSE → bootlog (FAILED/MISMATCH/refusing stay) |
| 12 | `SMP` | 24 | 24 | PROSE → bootlog (WARNING lines stay) |
| 13 | `:: X200` | 22 | 0 | KNOB |
| 14 | `:: PART` | 21 | 1 | KNOB (`mbr-raw`); `fat mounted` = STAGE |
| 15 | `:: KBDWIT` | 21 | 21 | CENSUS (default-ON witness instrument) |
| 16 | `APIC` | 20 | 12 | PROSE → bootlog (ABORTED stays) |
| 17 | `:: EHCI-CONFIG` | 19 | 19 | CENSUS (register walk; STOP-NOTE stays) |
| 18 | `[uvc]` | 17 | 0 | KNOB `uvc` |
| 19 | `[ioapic]` | 15 | 1 | KNOB `ioapic`; `id=` → PROSE (REFUSED stays) |
| 20 | `[NVIDIA]` | 12 | 8 | PROSE → bootlog (`Error` lines stay) |
| 21 | `[Intel iGPU]` | 12 | 0 | KNOB |
| 22 | `[wc-d]` | 12 | 9 | CENSUS (verify/skip-rollup); latch-check → FIXTURE `wcdskip` |
| 23 | `[wc-h]` | 12 | 12 | CENSUS per-present lines; the `rollup` (KCOMP's `blitter=` witness) stays |
| 24 | `:: bt-l0` | 11 | 0 | KNOB `bt` |
| 25 | `[wc-x]` | 11 | 4 | CENSUS (DECLINE/REFUSE stay) |
| 26 | `:: x86` (fb-wc, mmio-map) | 10 | 10 | PROSE → bootlog |
| 27 | `[wc-g]` | 10 | 6 | CENSUS |
| 28 | `[wc-b]` | 9 | 9 | CENSUS (`[wc-b] fixture` included) |
| 29 | `:: STAGE-PHYS` | 9 | 0 | KNOB |
| 30 | `:: WXPROBE` | 8 | 2 | CENSUS |
| 31 | `[vfs]` | 7 | 6 | STAGE (the mounts); the plan-walk line is HOMESOIL's → FIXTURE |
| 32 | `[wm]` | 6 | 6 | PROSE → bootlog (REFUSED stays) |
| 33 | `[pcih]` | 6 | 1 | CENSUS (`rp-boot`) |
| 34 | `[ahci]` | 6 | 6 | PROSE → bootlog (errors stay) |
| 35 | `:: [sntp-x86]` + GATE | 7 | 7 | FIXTURE `tests sntp` |
| 36 | `HEAP` | 5 | 4 | PROSE → bootlog |
| 37 | `:: BLK` | 5 | 4 | FIXTURE (`unafsroot` → LBA32 legs); `retraction SKIPPED` → PROSE |
| 38 | `[GPU]` | 5 | 3 | PROSE → bootlog |
| 39 | `[wc-a]` / `[cursor3]` | 5 / 5 | 5 / 5 | CENSUS / PROSE |
| 40 | `:: SCHED-X86` | 5 | 5 | PROSE → bootlog |
| — | `:: HOMESOIL` ×3, `SDWRITE-POSTURE`, `USBUNPUB`, `LBA32`, `SO49` | 8 | 8 | FIXTURE `tests unafsroot` (one defer covers all, `homesoil` re-enters it) |
| — | `:: [dns-x86]` + GATE, `NETFETCH-PARSE` | 6 | 6 | FIXTURE `tests dns`, `tests netfetch` |
| — | `:: U2-0c` ×2 | 2 | 2 | FIXTURE `tests u20c`, `tests canonguard` |
| — | `:: WXAUDIT-0` / `WXAUDIT x86` | 2 | 2 | FIXTURE `tests wxaudit` |
| — | `:: WXAUDIT-NXE` / `-CORES` | 2 | 2 | CENSUS (NXE: a FAIL always prints) |
| — | `WXN-x86/FBWC/M2/M3B-PRE`, `RTC`, `sdhc: w1`, `SMC-DIAG`, `SMC-BATT AC-W`, `EPACE`, `GPACE` | 12 | 12 | CENSUS (`WXN-M3B … -> REFUSED` stays) |
| — | `PTRPAINT`, `WCDLATCH`, `BLITWIRE`, `PTRLAG`, `USERSREADY`/`USERSMOUNT`, `X86BIND` | 7 | 7 | FIXTURE `ptrpaint wcdlatch blitwire ptrlag usersready x86bind` |
| — | `HEAP -> PASS`, `CLOCKBAR`, `MENUBAR-OCC-PAR`, `WALLPAPER -> PASS`, `[wcpar]` | 5 | 5 | CENSUS (a FAIL prints) |
| — | `PRTSCR-DIR-FIX` ×2 + `PRTSCR-REFUSE`, `WINX-3` ×2, `SINKDRAIN`, `U2.5 FTDI TX mirror`, `BOT-PARK` | after `:: BOOT:` | | FIXTURE `prtscrdir winx3 sinkdrain botpark`; FTDI mirror PASS → bootlog |
| — | `SPLASH`, `BOOTCLOCK`, `FRGUARD`, `SDHCPOST`, `PART: fat mounted`, `[vfs] … mount`, `USBNET-EHCI link=up`, `AHCI selfcheck`, `FIRSTBOOT`, `BOOT`, every REFUSED/FAIL/STOP-NOTE | ~27 | | STAGE — stays |

A side finding: `smolnet::sntp_x86_gate` ran at boot and its canned reply SET THE CLOCK to
2026-07-22T15:30:45Z (`:: [sntp-x86] pre-existing anchor left in place (canned value overwrote it …)`);
boot 20's `CLOCKBAR … text=15:30 from=sntp` is that canned value. Deferred, it no longer touches the clock.

## Seam

`CHARTER: Kernel — kernel-by-ruling` (R80). No new file. Three dispositions, three mechanisms:

* FIXTURE → `crate::tests::defer(name, f)` as the fixture's first statement (QUIETBOOT's shape), or
  `crate::tests::register(name, f)` at the boot call site, or the new `tests::defer_fast(name, f, &latch)`
  for a fixture on a path the boot passes many times (a service pass, a paint). Under `tests-at-boot`
  (every QEMU battery lane) they run inline, in the old order; on metal `tests <name>` fires them and the
  witness line is the same text.
* CENSUS → `crate::census_println!` (tail of `census.rs`): prints under `census` or `tests-at-boot`.
* PROSE → `crate::bootlog_println!` (tail of `bootlog.rs`): prints under `bootlog` (`UNAOS_BOOTLOG=1`) or
  `tests-at-boot`. Both macros are `if cfg!(…) { emit }` — the arguments stay type-checked, and in the
  quiet image the literal and the call are gone (measured: `periodic DMA smoke pass` 1 → 0 in the ELF).

The sweep is a rename of `serial_println!` at 825 sites chosen by the family of their first format
literal (scratchpad `sweep.py`), never at a site whose literal says fail/refus/error/panic/timeout/
STOP-NOTE/wedge/lost/mismatch/invariant (205 such sites kept), and nowhere under `arch/aarch64/`.
Line-neutral: every changed file keeps its line count except the tail appends.

## Milestones

* M1 — this census.
* M2 — fixtures behind `tests`: `wxaudit unafsroot u20c canonguard sntp dns netfetch ehciisr passperiod
  wcdlatch wcdskip blitwire ptrlag usersready x86bind prtscrdir ptrpaint winx3 sinkdrain botpark`
  (registry CAP 80 → 128).
* M3 — `census_println!` / `bootlog_println!` and the sweep.
* M4 — `QUIETBOOT_BOUND` 2500 → 250; the tag tally 48 → 128 slots (so `top=` names the real loudest);
  `rmbp-boot.spec` / `round6-rmbp.spec` (metal bench specs) demote the moved REQUIREs to OPTIONAL.

## Witness

`:: BOOT: firmware->loader=<ms> loader->desktop=<ms> total=<ms> lines=<n> ::` then, on `tests quietboot`,
`:: QUIETBOOT: lines=<n> bound=250 census=0 -> PASS ::`.

## Estimate

Identified survivors on the metal shape, before `:: BOOT:`: ~27 wire lines (the STAGE row above, 13 of
them KVBLANK8's `kepler: vblank …`), plus heuristic slack and the ~7 % the wire misses → **~45 lines
(bound 250)**. With boot 20's knob line (smcwalk, gen7, Kepler rungs, iGPU, ioapic, uvc, bt) the count is
~1,800 and QUIETBOOT stays FAIL: those knobs are not in the metal shape and must leave the R80 flight line.

## Owed / design questions

1. The recon knobs (row 1–3, 6, 8–10, 13, 18, 21, 24, 29) are ~1,880 of boot 20's lines. A seat decision
   (the flight line), not this arc's code; GEN7R8/KBLIT/KVBLANK8 own the GPU ladders.
2. The brief's metal shape lists `census`: a `census` image arms every sampler AND every walk this arc
   moved, and `tests quietboot` SKIPs on it. The R80 flight line wants `census` OFF.
3. `bootlog` is the existing hold-the-boot-log mode (main.rs halts before the GUI under it), so the
   prose is visible on a bootlog image or a `tests-at-boot` image, not on a desktop one. A separate
   non-halting `bootprose` knob is the alternative if the seat wants prose on a running desktop.
4. Compile-time `census` cannot turn the compositor instruments (`[wc-*]`) on at runtime; a `census start
   tables` bit is the follow-up if wanted.
5. KVBLANK8's boot `vblank selftest` arms and `vblank-intr census` (13 lines) are left for that arc.
6. The ring-3 battery after `:: BOOT:` (U5x/U7x/U8x/SERWIT-2), SCHED-X86 PLACE-CHECK, PTRLOST, SHOTMENU,
   DOCKPIN, LOGOUTDESK, DIMIDLE still print between the installer and the desktop: a third pass.
7. `tests ehciisr` drives the ISR path against a hand-built completion on the LIVE ring for one pass.
