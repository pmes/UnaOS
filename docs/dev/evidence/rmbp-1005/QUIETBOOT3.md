# QUIETBOOT3 — the boot prints its stages; the glass prints the wire's word (rmbp-ledger B352, R80)

Branch `exec-rmbp-quietboot3`, cut at 452a6127, then `git merge exec-rmbp-merge12` twice (the second for the
tip after b92481cd; both merges clean, no hand-joins, no merge12 fixes of mine). Ruling R80. CHARTER of the
touched code: `Kernel — kernel-by-ruling`; no new file.

## Finding

Boot 21 (`docs/dev/evidence/rmbp-0915/flight21/f21-boots.log`) printed `:: BOOT: … lines=1327 ::` and
`:: QUIETBOOT: lines=1327 bound=250 census=0 -> FAIL ::`. The capture holds **1144** wire lines before `:: BOOT:`
(tallied by tag with awk/python over the log; scratchpad `qb3/tally.py`). Three findings:

1. **The counter counted fragments.** `serial_line::emit` bumped `LINES` on every call, and every
   `serial_print!` is a call. The iGPU ladder's EDID dump is 8 rows x 18 `serial_print!` calls, the DPCD
   CAP line 18 more, the LADDER line several — about 160 of the 183 "lines" the capture does not hold. `top=`
   even named a tag `00:51`: the EDID's zero bytes, each a fragment that opened with `00 `. A line is now an
   emit that ENDS in `\n`; the tag tally keys on the fragment that OPENS a line (`serial_line::line_note`).
2. **The 1 Hz kepler census** (`:: kepler: vblank head=` + `:: kepler: vblank-dt`, 32 captured lines and the
   whole of BOOT80's 60 s) printed from the edge path on every build with `nvidia-kepler-vblank`.
3. **963 lines are recon knobs** the bench flew (row B352). Matched to their source sites
   (`qb3/match.py`, 1138 of 1144 lines matched to a site; the 6 unmatched placed by hand), 862 captured
   lines are recon: `gen7` 390, the Kepler FIFO/ucode/FENCE/ctrlbind/ctrladdr ladder 340 (every line inside
   `kepler.rs`'s `if cfg!(feature = "nvidia-kepler-fifo")` block at 1508..3984, plus
   `try_bind_and_witness`, called only from inside it — KFBIND implies `nvidia-kepler-fifo`), the iGPU
   GMUX/AUX/`igpu-dpy` ladder (`gmux_igd`), KFBIND 38, KDHEAD 36, BAR1WEDGE and its two `[pcih] bar1wedge`
   lines. Boot 21 also flew `usbdebug` (X200, 22 lines) and `witness` (STAGE-PHYS, 9): both swept anyway.

## Milestones

* **M1** (ce437b6d) — `census::KEPLER` (`census start kepler`); `kepler_vblank::edge` returns before the
  cadence after `ladder_tick` unless the bit is on — the KVBLANK2/8/9 ladder still runs and prints its
  fixture lines (`vblank-intr census/window/vector`, `vblank-isr books`, the boot `vblank selftest` arms,
  `KVBLANK4`); `report()` stays for the fixtures that call it. The fragment-free line counter. The
  verdict-tail recorder (`serial_line::tail_arm / tail_take / verdict_tail`).
* **M2** (a3d92e3f) — THE GLASS SAYS WHAT THE WIRE SAYS. `tests` arms the recorder around each fixture; the
  console prints one line per fixture, `<name> -> <tail>`, the tail being the wire's own text after the
  LAST `-> ` of the last `PASS`/`FAIL`/`SKIP` line the fixture printed, up to its ` ::`. A fixture that
  printed no verdict gets `:: TESTS: <name> -> SKIP reason=no-verdict ::` on the wire and the same tail on
  the glass. The `tests: ran= pass= …` tally line prints on the console only after the whole suite
  (`tests` with no name); the wire's `:: TESTS: ran=…` line is unchanged. Boot 21's console said
  `tests: ran=1 pass=8 fail=0 …` for `tests nethang` (eight sub-verdicts tallied) — that sentence is gone;
  it now reads `nethang -> PASS`, the word ending `:: NETHANG: … -> PASS ::`. SKIP lines that were
  sentences are now `SKIP reason=`: QUIETBOOT (`no-boot-line`, `tests-at-boot-or-census-build`),
  LUMENCRASH (`not-spawned`), LUMENAPP (`window-bad`), LINUXABI/LINUXABI2/LINUXABI3/LINUXABI-KAT
  (`fixture(s)-not-staged`), SELFBUILD1 (`tcc-hello-not-staged`). No spec pins any of them.
* **M3** (a3d92e3f) — the sweep, table below. `tests uvc` is new (the UVC parser self-test no longer runs
  at the first USB device). Every touched file is line-neutral except the tail appends
  (`GATE-LINENEUTRAL: files=21 neutral=19 moved=2 moved-above-panic=0`; M1: `files=3 neutral=2 moved=1
  moved-above-panic=0`).

`storm` is not a fixture: its console lines (`storm: launched 6/6 vugs`) already mirror the wire. Boot 21's
"storm failed" is STORMFAULT (B351, the VUG.ELF write fault), not a paraphrase.

## The census — every pre-desktop line of boot 21, by site, and where it goes

Disposition: **STAYS** prints on the boot-22 line; **CENSUS** = `crate::census_println!` (a `census` or
`tests-at-boot` build) or a `census::on` gate; **BOOTLOG** = `crate::bootlog_println!` (a `bootlog` or
`tests-at-boot` build); **FIXTURE** = `tests <name>`; **RECON** = a recon knob the bench drops (B352).

| disposition | tag | site | boot-21 lines |
|---|---|---|---|
| CENSUS (`kepler`) | `kepler: vblank head=` / `vblank-dt` (the 1 Hz census) | kepler_vblank.rs:783, :1880 (gated at :764) | 32 |
| CENSUS | `X200` (usbdebug) | xhci/mod.rs:910 | 22 |
| CENSUS | `:: igpu:` reachability/teardown/GMUX-trace/citation/census-gap table + `[Intel iGPU]` pipe/plane/GGTT | igpu.rs 468..717 (VERDICT/Error/BOUND HIT kept) | 63 |
| CENSUS | `STAGE-PHYS` (witness) | wm.rs:19284, :19299 | 9 |
| CENSUS | `PART: mbr-raw` / `mbr … REJECT/ACCEPT` / `mbr census` | block.rs:2338..2370 | 20 |
| CENSUS | `[ioapic] iso` / `nmi` | ioapic.rs:362, :375 | 10 |
| CENSUS | `[uvc]` descriptor walk, candidate, probe stages | uvc.rs 663..736, :1118, :1212, :1245 | 10 |
| CENSUS | `[pcih] ep` / `rp` | pcihealth.rs:323, :505 | 2 |
| CENSUS | compositor rollups `[wc-g]` `[wc-k]` `[wc-h] vbl` `[wc-d] valve READ` `[strip]` `[dock]` `[menubar]` `[wcn]` `[wcser]` `[noatt]` `[wcpar]` | wcg.rs:4126, :2203, :1863; wm.rs:7713, :14435, :10377, :14140; strip.rs:556, :1440; wcpar.rs:139 | 13 |
| CENSUS (`ptrinstall`) | `:: PTRINSTALL:` late rollup | main.rs:10118 | 1 |
| BOOTLOG | `[uvc] skip/note/configured`, `bt-sched` ×2, `[ioapic] route/armed/pirq`, `igpu-blt ring=absent`, `[wc] blitter=`, `kepler: CE context banked`, `FB Format`, `[rand]`, `[fs] dirent flushed` | uvc.rs:1107,1147,1169,1199; ehci:19156-7; ioapic.rs:585,634,705,888,896; igpu.rs:884,895; blitter.rs:282; kepler_ce.rs:1066; video/mod.rs:472; rand.rs:175; fat.rs:6927 | 17 |
| FIXTURE | `:: uvc: selftest … -> PASS` | uvc.rs:1067 → `tests uvc` | 1 |
| RECON | gen7, Kepler FIFO ladder, iGPU GMUX/AUX/igpu-dpy, KFBIND, KDHEAD, BAR1WEDGE | knobs | 862 |
| STAYS | stages: `SPLASH` ×3, `[splash] hold`, `FRGUARD`, `BOOTCLOCK`, `sdhc: card`, `SDHCREG`, `AHCI: selfcheck -> PASS`, `SDHCPOST`, `[unafsbind]`, `[vfs]` ×7, `UNAFSX86 -> PASS`, `PREFS -> PASS`, `[login]` ×2, `FIRSTBOOT`, `USBNET-EHCI link=up`, `USBDBG-INVERT` | — | 26 |
| STAYS | refusals/errors: `WXN-M3B -> REFUSED`, `EHCI-HID` STOP-NOTE/EP0/bcm5974 FAILED ×5, `[uvc] probe STALLED`, `PART: unafs span check`, `igpu: VERDICT` | — | 9 |
| STAYS | KVBLANK2/8/9 ladder + boot selftests (row: they keep their fixture lines) | kepler_vblank.rs 661,705,727,900,925,952,1456..1733, vector/isr books | 23 |
| STAYS | banner-cert witnesses: `BEAMX86 -> ARMED`, `[pcih] aspm cleared`, `[ioapic] census`, `[wc-d] paygo win`, `[wc-d] valve CLOSED`, `LOGWIT-1` ×2 | — | 7 |
| STAYS | `bt-l0` census/claim/reachability (the `bt` knob's own; `claim` carries the bt token) | ehci/mod.rs:5317, 5405, 5426 | 11 |
| STAYS | the QUIET-PANEL proof `:: fbcon: glyphs-active` + the milestone replay (they paint the panel too) | fbcon.rs:1813, :1821 | 6 |
| STAYS | `SMC-BATT`, `[wc-h] rollup` (KCOMP's `blitter=` witness), `PTRLOST` | — | 3 |

Totals over the 1144 captured lines (script count; the hand rows above sum to 85 — one line sits on two sites): STAYS 84 · CENSUS 180 · BOOTLOG 17 · FIXTURE 1 · RECON 862.
Per-site rows (every one of the 1144 lines with its site): scratchpad `qb3/table.md`, `qb3/proj.json`.

**Static side (the image).** Two release ELFs were built at a3d92e3f and grepped (`LC_ALL=C grep -a -F`):
the brief's metal shape minus `census` (plus `selfdiag,ahciroot,btc`), and that plus the bench's functional
and instrument knobs (`nvidia-kepler,nvidia-kepler-takeover,intel-ivb,unaos_ivb,ioapic,uvc,noaspm,usbdebug,
logts,wcdvalve,wcg-paygo,bt`). In the moved set the literal is GONE from the image (e.g. `[wcpar] pool=`,
`:: X200: `, `:: PART: mbr-raw handle=`, `:: igpu: REACHABILITY` 0 hits) — the call is compiled out, not
skipped. Of the tree's 4574 x86 `serial_println!` sites with a decidable literal, 2634 are in the bench
image; everything that image printed before `:: BOOT:` on boot 21 is in the table above. Every banner-cert
token of a feature in the bench image measured >0 (`ioapic` 2, `beam` 2, `noaspm` 1, `intel-ivb` 1,
`nvidia-kepler-vblank` 13, `uvc` 1, `bt` 1, `logts` 1, `wcdvalve` 1, `wcg-paygo` 2, …); the one 0 is
`unaos_ivb`, whose token lives in the bootloader, not the kernel ELF.

## Witness — the expected boot-22 wire

Boot 22 on the R80 line (recon knobs off): `:: BOOT: firmware->loader=<ms> loader->desktop=<ms> total=<ms>
lines=<n> ::` with **n ≈ 85 captured-class lines + ~25 lines before the FTDI tap + variance ≈ 110–140**, then on
`tests quietboot`:

    :: QUIETBOOT: lines=<n> bound=250 census=0 -> PASS ::

and on the glass `quietboot -> PASS`. `tests nethang` ends `nethang -> PASS` on the console and
`:: NETHANG: … -> PASS ::` on the wire. `census start kepler` brings back `:: kepler: vblank head=` at 1 Hz
(`:: CENSUS: start kepler bits=65536 ::`).

## Owed / design questions

1. If the bench keeps a recon knob on, QUIETBOOT counts it: KFBIND alone is ~380 lines (FAIL). The
   `top=` table names it.
2. `bt-l0`, the KVBLANK ladder and the fbcon milestone replay still print on their knobs (~40 lines). A
   `census start bt` bit and moving the KVBLANK boot selftests behind `tests kvblank8` are the next pass.
3. `SKIP (sentence)` remains on ~110 lines of fixtures that run under `tests-at-boot` lanes (e.g.
   `[clickroute] route -> SKIP (framebuffer not ready)`); the glass prints the wire's tail verbatim, so
   those show the sentence until each is changed to `SKIP reason=` against its spec.
4. BOOT80 (the 60 s wait) is B350's; this arc only removes the census that filled it.
