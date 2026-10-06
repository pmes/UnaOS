# STATUSBASELINE — the 188 grandfathered status cells read against the wire (rmbp-ledger B437)

Host-only. Cut from 4ead840a (merge17; STATUSTABLE B425 merged at 6f19ddda). No kernel byte moves.

## Design

**Finding.** STATUSTABLE (B425) made `docs/dev/STATUS.tsv` the one table and grandfathered 188 claiming cells in
`docs/dev/STATUS.baseline` (rmbp-ledger 126, LEDGER.md 42, queue STATE lines 20). A grandfathered cell is exactly the
shape Peter named: a status word (flew, unflown, proven, confirmed, refuted, landed on metal) with no flight and no line
behind it.

**The seam.** None in code. The table is the seam: every cell's claim becomes, or cites, an `ST<n>` row whose line
`tools/status-check.py` finds byte for byte in that flight's capture. Rows are resolved from the captures by a script
(the whole line is copied out of `f<N>-boot*.log` or `FLIGHT<N>.md`, never typed), so no quoted line can drift.

**Rules applied to every cell.**
1. A metal claim the wire supports gets a `confirmed` row with the whole line, and the cell cites it.
2. A claim the wire does not support is rewritten in the cell to say what the wire shows, cites the row that shows it
   (`refuted` when the wire contradicts the claim), and is listed below with before and after.
3. A claim with no flight (a host test, a QEMU run, a gate drill, a code read, a git fact) gets an `unflown` row.
4. An "unflown" that later flew is rewritten to "unflown at hand-back" plus the flight that flew it.
5. Orin and Pi captures (`docs/dev/evidence/orin*`, render<N>) are outside the gate's `f<N>` capture model: those claims
   get an `open` row naming the capture, never a borrowed rmbp flight number.
6. No flight number is invented: every `f<N>` is the bench's, and a cell whose flight the evidence does not name says so.

**Milestones.** M1 this design. M2 the rows (ST43–ST265), the citations in all 188 cells and the baseline emptied
(one commit: the gate holds only with all three). M3 the rewritten-cell list below and the DRIVERS-METHOD §1 entry.

**Witness.** Not a metal line: `python3 tools/status-check.py` → `rows=265 … claiming-uncited=0 baseline=0 -> PASS`,
`--baseline` prints nothing, `--selftest` 11/11.

**Owed.** The B408–B425 rows carry their ledger id in the claim with an empty `row-refs`, because those ledger rows are
not in this tree (the seat fills `row-refs` at the fold). The enum heads of B271 and B297 still read `flown` (their prose
now says they did not fly); GATE-LEDGER owns that vocabulary, so the head is the seat's call.

## Result

223 new rows (ST43–ST265): 120 confirmed, 12 refuted, 11 open, 80 unflown; every quoted line resolved from its capture.
188 cells now cite a row; `STATUS.baseline` holds only its header. 74 cells were rewritten (86 edits, below); 4 more cells
were annotated without changing a claim (B10 flight 4's postmortem is a capture the gate does not read; S12's wire has no
flight number in the evidence; two queue lines dated "unflown" that later flew).

B408–B425: rows ST251–ST265 for the fifteen arcs not already in the table (all unflown or open: they postdate flight 25's
image). B410 GPUBLIT3 is carried by ST27–ST32, B411 GEN7 by ST33–ST39, B415 WIFI5 by ST1–ST26.

### What the wire said that the cells did not (the findings)

- **Stamped by the image, not read from the wire.** 34 rows were set to "flown — boot 19 … the arc flew" in one sweep.
  Thirty did fly (ST100–ST129). Five did not: B271 SELFINSTALL (no `install ssd` line on boot 19; boot 20's verb was
  compiled out), B297 GATE-CHARTER (a host gate), B298 UNAFSX86 and B302 F3F4 (every boot-19 root line reads
  `unafs=unbuilt`; UnaFS first mounts on flight 21), B299 ATTRSURF (its witness SKIPPED, `no-unafs-volume`; first PASS
  flight 22).
- **"unflown" long after the flight.** B373–B383 (merge16) were headed `done-unflown` after flight 24 flew them; B345–B372
  kept "unflown — wire …" after flights 22 to 25 read them. Several read differently from the wire the cell predicted:
  SELFBUILD2 KAT2 FAIL on 22 and 23 (PASS from 24, probe skipped), SELFBUILD3 FAIL on 22 (SKIP from 23), SELFBUILD4/5 SKIP
  not PASS, KERNELFONT2 and UIMETRICS at `ppi=0 scale=1.0` on 23 (fixed on 24 by KFONTPPI), MP3HANG's decoder job hangs on
  flight 24 (DECJOBHANG), GPUBLIT2's selftest still times out. B137's falsifier half-held: the campaign starts after the
  gui, but `ehci-hid-done` is 5540 ms, not the low hundreds.
- **A quoted line that is not the wire's.** B311 quoted `:: QUIETBOOT: lines=2615 bound=250 -> FAIL`; the capture reads
  `bound=2500 census=0`. The "ten times the bound" is R80's 250, not the image's.
- **Refuted by derivation.** SO29 said SO45 "REFUTED" the drain arithmetic; SO45 is a derivation whose own verdict is
  unflown. Now worded as a derivation.
- **A rule claimed for a flight that broke it.** SO44 said its rule's metal test was flight 12; flight 12's keys reached
  the desktop under the login screen. The first login through the screen is flight 15.

### Rewritten cells, before and after
- **rmbp-ledger.md A3** — before: «QEMU-green, UNFLOWN on metal (no rMBP boot since)» — after: «QEMU-green at hand-back, then flown on flight 8 (the head of this cell)»
- **rmbp-ledger.md A5** — before: «**Unflown: nothing here is a metal fact until the witness below is read at the glass.**» — after: «**Unflown at hand-back; read at the glass on flights 8 and 9 (the head of this cell).**»
- **rmbp-ledger.md A7** — before: «**Unflown: nothing here is a metal fact until the witness lines are read at the glass.**» — after: «**Unflown at hand-back; read at the glass on flights 8 and 9 (the head of this cell).**»
- **rmbp-ledger.md A9** — before: «QEMU-green, UNFLOWN on metal (no rMBP boot since)» — after: «QEMU-green at hand-back, then flown on flight 8 (the head of this cell)»
- **rmbp-ledger.md A10** — before: «**fixed-unflown by QUARRYCLICK**» — after: «**fixed by QUARRYCLICK (unflown at hand-back; its press line is on flight 11)**»
- **rmbp-ledger.md A10** — before: «**fixed-unflown by QUARRYFONT**» — after: «**fixed by QUARRYFONT (unflown at hand-back; its face is on flight 13)**»
- **rmbp-ledger.md A10** — before: «**fixed-unflown by QUARRYOPEN**» — after: «**fixed by QUARRYOPEN (unflown at hand-back; its fixture passes on flight 13)**»
- **rmbp-ledger.md A12** — before: «open — INSTRUMENTED AND UNFLOWN (BOOTCLOCK,» — after: «open — INSTRUMENTED, UNFLOWN AT HAND-BACK AND READ ON FLIGHTS 10 TO 19 (BOOTCLOCK,»
- **rmbp-ledger.md B89** — before: «UNFLOWN: proven on QEMU q35's ICH9 AHCI against a GPT+FAT fixture; the rMBP boot that reads the Apple SSD's model line is the flight, queued as AHCIFLY» — after: «UNFLOWN at hand-back: proven on QEMU q35's ICH9 AHCI against a GPT+FAT fixture; the rMBP boot that reads the Apple SSD's model line is the flight, queued as AHCIFLY and flown on flight 8 (the head of this cell)»
- **rmbp-ledger.md B116** — before: «fixed-unflown for the freeze LENGTH only (STEALMS» — after: «fixed for the freeze LENGTH only, unflown at hand-back and flown on flight 10 (STEALMS»
- **rmbp-ledger.md B119** — before: «fixed-unflown for the instrument, the W5 question itself stays open» — after: «fixed for the instrument, unflown at hand-back and flown on flight 10; the W5 question itself stays open»
- **rmbp-ledger.md B117** — before: «status unchanged, fixed-unflown)» — after: «status unchanged, fixed-unflown at hand-back; the census reads on flight 13)»
- **rmbp-ledger.md B136** — before: «open — the 500 ms half is **fixed-unflown** (gated below)» — after: «open — the 500 ms half is **fixed** (gated below; unflown at hand-back, QUARRYOPEN dbl=500ms passes on flight 13)»
- **rmbp-ledger.md B130** — before: «**UNFLOWN — the score is still a sound in the room**» — after: «**UNFLOWN at hand-back — the score is still a sound in the room; the bind lines read ok=1 on flight 13**»
- **rmbp-ledger.md B137** — before: «**open — code landed, finding (1) unflown and finding (2) ungateable.**» — after: «**open — code landed, finding (1) unflown at hand-back and half-held on flight 13 (the campaign starts after the gui; ehci-hid-done is 5540 ms, not the low hundreds) and finding (2) ungateable.**»
- **rmbp-ledger.md B157** — before: «UNFLOWN is the boot with a person at it» — after: «UNFLOWN at hand-back; the boot with a person at it is flight 15»
- **rmbp-ledger.md B162** — before: «and is UNFLOWN. GATES:» — after: «and was UNFLOWN at hand-back (VUGART reads coherent frames from flight 13). GATES:»
- **rmbp-ledger.md B172** — before: «the metal shape itself is UNFLOWN until flight 12» — after: «the metal shape itself was UNFLOWN until flight 12, which read it (and flight 13 repeats it)»
- **rmbp-ledger.md B173** — before: «nothing on metal yet.» — after: «nothing on metal yet at hand-back (flight 19 saves a long-named screenshot through it).»
- **rmbp-ledger.md B178** — before: «the metal shape is UNFLOWN until flight 12» — after: «the metal shape was UNFLOWN until flight 12, which read it (and flight 13 repeats it)»
- **rmbp-ledger.md B181** — before: «FLIGHT 12 is where it is seen on metal; nothing here needs metal to be true, and nothing here is proven by metal yet.» — after: «FLIGHT 12 was to be where it is seen on metal, and no capture through flight 25 carries its witness; nothing here needs metal to be true, and nothing here is proven by metal yet.»
- **rmbp-ledger.md B271** — before: «the arc flew; its verdict is in FLIGHT19 §1 (green) or §2 (finding → B311–B315)» — after: «the arc was on image 12 but its verb was never run on the boot-19 capture (no install ssd line); boot 20 answered `install ssd --dry-run` with a TerminalError (its knob was off the line), so it has not flown»
- **rmbp-ledger.md B287** — before: «(compiled, unflown; moves when boot 19 reads)» — after: «(compiled, unflown at the fold; PREFS flew on boot 19)»
- **rmbp-ledger.md B288** — before: «(compiled, unflown; moves when boot 19 reads)» — after: «(compiled, unflown at the fold; PREFS flew on boot 19, ATTRSURF first passes on flight 22)»
- **rmbp-ledger.md B289** — before: «(compiled, unflown; moves when boot 19 reads)» — after: «(compiled, unflown at the fold; BANDY3 flew on boot 19, its fixture one verb short)»
- **rmbp-ledger.md B290** — before: «(compiled, unflown; moves when boot 19 reads)» — after: «(compiled, unflown at the fold; boot 19 read unafs=unbuilt, UnaFS first mounts as the root on flight 21)»
- **rmbp-ledger.md B291** — before: «(compiled, unflown; moves when boot 19 reads)» — after: «(compiled, unflown at the fold; the boot-19 witness SKIPPED for want of a volume, ATTRSURF first passes on flight 22)»
- **rmbp-ledger.md B292** — before: «(compiled, unflown; moves when boot 19 reads)» — after: «(compiled, unflown at the fold; UnaFS first mounts on flight 21 and no capture carries a timestamp witness)»
- **rmbp-ledger.md B293** — before: «(compiled, unflown; moves when boot 19 reads)» — after: «(compiled, unflown at the fold; ATTRSURF first passes on flight 22)»
- **rmbp-ledger.md B294** — before: «(compiled, unflown; moves when boot 19 reads)» — after: «(compiled, unflown at the fold; the volume first mounts on flight 21, ATTRSURF first passes on flight 22)»
- **rmbp-ledger.md B295** — before: «(compiled, unflown; moves when boot 19 reads)» — after: «(compiled, unflown at the fold; not on boot 19 for want of a volume, catalog queries first answer on flight 22)»
- **rmbp-ledger.md B297** — before: «the arc flew; its verdict is in FLIGHT19 §1 (green) or §2 (finding → B311–B315)» — after: «a host gate (charter-check.sh in arroyo check) that rode the image-12 tree; nothing of it flies on metal, so the boot-19 capture carries no verdict for it»
- **rmbp-ledger.md B302** — before: «the arc flew; its verdict is in FLIGHT19 §1 (green) or §2 (finding → B311–B315)» — after: «the arc was on the image but did NOT fly on boot 19 (no UnaFS volume: `unafs=unbuilt`); the catalog first answers queries on flight 22 (ATTRSURF query=2)»
- **rmbp-ledger.md B298** — before: «the arc flew; its verdict is in FLIGHT19 §1 (green) or §2 (finding → B311–B315)» — after: «the arc was on the image but did NOT fly on boot 19: every boot-19 root line reads `unafs=unbuilt`; UnaFS first mounts as the root on flight 21»
- **rmbp-ledger.md B299** — before: «the arc flew; its verdict is in FLIGHT19 §1 (green) or §2 (finding → B311–B315)» — after: «the witness ran on boot 19 and SKIPPED (`reason=no-unafs-volume`), so the surface did not fly there; it first passes on flight 22»
- **rmbp-ledger.md B305** — before: «fixed-unflown in boot 21» — after: «fixed-unflown at boot 20 and flown in boot 21, where `lumen` starts»
- **rmbp-ledger.md B311** — before: «RED — `:: QUIETBOOT: lines=2615 bound=250 -> FAIL`, ten times the bound (R80 not done)» — after: «RED — `:: QUIETBOOT: lines=2615 bound=2500 census=0 -> FAIL` (the wire's bound is 2500, not 250; 2615 is ten times R80's 250, R80 not done)»
- **rmbp-ledger.md B311** — before: «fixed-unflown in boot 21» — after: «fixed-unflown at boot 20; flown in boot 21, where QUIETBOOT still fails at 1327 lines»
- **rmbp-ledger.md B313** — before: «(AUDIO8, B329, fixed-unflown in boot 21)» — after: «(AUDIO8, B329, fixed-unflown at boot 20 and flown in boot 21: one amp up, PASS)»
- **rmbp-ledger.md B314** — before: «USBNET7 (B328: bring-up order, TX on the wrong EP) fixed-unflown in boot 21» — after: «USBNET7 (B328: bring-up order, TX on the wrong EP) fixed-unflown at boot 20; flown in boot 21, still rx_ok=0»
- **rmbp-ledger.md B315** — before: «the rest are their own rows; fixed-unflown in boot 21» — after: «the rest are their own rows; the fixes flew in boot 21 (QUIETBOOT still FAIL there, AUDIO8 PASS)»
- **rmbp-ledger.md B321** — before: «`. Unflown. Owed:» — after: «`. Unflown at the fold; flight 22 passes `tests blitter` on the CPU blitter. Owed:»
- **rmbp-ledger.md B345** — before: «byte-equal through the kernel-store shim; unflown)» — after: «byte-equal through the kernel-store shim; unflown at the fold, the fixture passes on flight 22)»
- **rmbp-ledger.md B347** — before: «compile legs 0; unflown;» — after: «compile legs 0; unflown at the fold (flight 22 passes `tests unafsgrow`);»
- **rmbp-ledger.md B348** — before: «compile legs 0; unflown; NOTE:» — after: «compile legs 0; unflown at the fold (flight 22 passes `tests lumen`); NOTE:»
- **rmbp-ledger.md B349** — before: «12 threads per session on one core; unflown — wire» — after: «12 threads per session on one core; unflown at the fold, then FAIL on flights 22 and 23 (KAT2 timeout, exit 19) and PASS from flight 24 with the probe leg skipped — wire»
- **rmbp-ledger.md B351** — before: «all direct legs 0; unflown — boot 22 reads» — after: «all direct legs 0; unflown at the fold — boot 22 read»
- **rmbp-ledger.md B353** — before: «compile-proven both legs, unflown — wire» — after: «compile-proven both legs, unflown at the fold, then FAIL on flight 22 (timeout), KAT3 PASS and SELFBUILD3 SKIP (libc=none) from flight 23 — wire»
- **rmbp-ledger.md B354** — before: «legs 0; unflown — NOTE for boot 23:» — after: «legs 0; unflown (no capture through flight 25 prints its migration) — NOTE for boot 23:»
- **rmbp-ledger.md B355** — before: «all legs 0; unflown; owed:» — after: «all legs 0; unflown at hand-back (flight 22 serves it); owed:»
- **rmbp-ledger.md B356** — before: «every direct leg 0; unflown — wire `:: SELFBUILD4:» — after: «every direct leg 0; unflown at hand-back (flight 23 reads rust=ok with fork and pipe skipped, -> SKIP, not the PASS below) — wire `:: SELFBUILD4:»
- **rmbp-ledger.md B357** — before: «tests selfbuild5 written; legs 0; unflown — wire» — after: «tests selfbuild5 written; legs 0; unflown at hand-back (flight 23 reads mremap=ok altstack=ok lld=skip, -> SKIP) — wire»
- **rmbp-ledger.md B358** — before: «incl. the aarch64 desktop row; unflown — wire» — after: «incl. the aarch64 desktop row; unflown at hand-back — wire»
- **rmbp-ledger.md B359** — before: «loadable +17740; legs 0; unflown — wire» — after: «loadable +17740; legs 0; unflown at hand-back — wire»
- **rmbp-ledger.md B360** — before: «x86 + aarch64 + builder legs 0; unflown — wire» — after: «x86 + aarch64 + builder legs 0; unflown (flight 23 SKIPs every leg) — wire»
- **rmbp-ledger.md B361** — before: «HOLOCRON 86321; legs 0; unflown — wire» — after: «HOLOCRON 86321; legs 0; unflown (on every image since flight 23, its witness never printed through flight 25) — wire»
- **rmbp-ledger.md B363** — before: «fixed there too); unflown — wire» — after: «fixed there too); unflown at hand-back; flight 23 read `ppi=0 scale=1.0 cell=7x16` (no EDID, B382), flight 24 reads the line below — wire»
- **rmbp-ledger.md B364** — before: «the aarch64 desktop leg NOT run); unflown — wire» — after: «the aarch64 desktop leg NOT run); unflown at hand-back (flight 23 printed no verdict, flight 24 PASS) — wire»
- **rmbp-ledger.md B365** — before: «midden_core 26; unflown — wire» — after: «midden_core 26; unflown at hand-back (flight 23 reads it) — wire»
- **rmbp-ledger.md B369** — before: «charter + x86 legs 0; unflown — wire» — after: «charter + x86 legs 0; unflown at hand-back (flight 23 reads `[wc-h] vbl_src=irq … wire=msi`; the `:: KVBLANK10:` line itself is on no capture through flight 25) — wire»
- **rmbp-ledger.md B370** — before: «expected ~139 of 250; legs 0; unflown — wire» — after: «expected ~139 of 250; legs 0; unflown at hand-back (flight 23 reads the fold and QUIETBOOT lines=142 PASS) — wire»
- **rmbp-ledger.md B372** — before: «M4 doc; legs 0; unflown — wire» — after: «M4 doc; legs 0; unflown at hand-back; flight 23 read `ppi=0 scale=1.0` (no EDID, B382), flight 24 reads ppi=221 (the `:: UIMETRICS:` line itself is on no capture through flight 25) — wire»
- **rmbp-ledger.md B373** — before: «done-unflown — » — after: «done, flown on flight 24 (image 17), where the decoder job hangs on FLAC (DECJOBHANG) — »
- **rmbp-ledger.md B373** — before: «debug builds, untouched; unflown — wire» — after: «debug builds, untouched; unflown at hand-back — wire»
- **rmbp-ledger.md B374** — before: «done-unflown — » — after: «done, flown on flight 24 (image 17) — »
- **rmbp-ledger.md B374** — before: «charter 0; unflown. Owed:» — after: «charter 0; unflown at hand-back. Owed:»
- **rmbp-ledger.md B377** — before: «done-unflown — » — after: «done, flown on flight 24 (image 17) — »
- **rmbp-ledger.md B377** — before: «charter 98/98; unflown — wire» — after: «charter 98/98; unflown at hand-back — wire»
- **rmbp-ledger.md B378** — before: «done-unflown — » — after: «done, flown on flight 24 (image 17) — »
- **rmbp-ledger.md B378** — before: «charter 0; unflown — wire» — after: «charter 0; unflown at hand-back — wire»
- **rmbp-ledger.md B379** — before: «done-unflown — » — after: «done, flown on flight 24 (image 17) — »
- **rmbp-ledger.md B379** — before: «host tests 0; unflown. Owed:» — after: «host tests 0; unflown at hand-back. Owed:»
- **rmbp-ledger.md B380** — before: «done-unflown — » — after: «done, flown on flight 24 (image 17) — »
- **rmbp-ledger.md B380** — before: «user-diag 0; unflown. Owed:» — after: «user-diag 0; unflown at hand-back. Owed:»
- **rmbp-ledger.md B381** — before: «done-unflown — » — after: «done, flown on flight 24 (image 17) — »
- **rmbp-ledger.md B381** — before: «usbnet_core 13 KATs 0; unflown — HOW FLIGHT 24 READS IT» — after: «usbnet_core 13 KATs 0; unflown at hand-back — HOW FLIGHT 24 READS IT»
- **rmbp-ledger.md B382** — before: «done-unflown — » — after: «done, flown on flight 24 (image 17) — »
- **rmbp-ledger.md B382** — before: «charter 0; unflown — wire» — after: «charter 0; unflown at hand-back — wire»
- **rmbp-ledger.md B383** — before: «done-unflown — » — after: «done, flown on flight 24 (image 17), where the selftest still times out — »
- **rmbp-ledger.md B383** — before: «charter 0; unflown — witness unchanged» — after: «charter 0; unflown at hand-back — witness unchanged»
- **LEDGER.md SO4** — before: «UNFLOWN until render10.» — after: «UNFLOWN at the re-cut; the Orin render14 capture carries `anchor=panel-left` (docs/dev/evidence/orin28).»
- **LEDGER.md SO29** — before: «**AND SO45 HAS NOW REFUTED THE MECHANISM'S ARITHMETIC ON THIS BOARD.**» — after: «**AND SO45'S DERIVATION NOW CONTRADICTS THE MECHANISM'S ARITHMETIC ON THIS BOARD (a derivation, not a flight: SO45's own verdict is unflown).**»
- **LEDGER.md SO44** — before: «UNFLOWN is the boot with a person at it (flight 12).» — after: «UNFLOWN at hand-back; flight 12 was the boot with a person at it and its keys reached the DESKTOP under the screen (the screen never had focus); the first login through the screen is flight 15.»
- **orin-queue.md @1812f6ea1666** — before: «AX88179 front-end built for Peter's dongles, unflown)» — after: «AX88179 front-end built for Peter's dongles, unflown at writing; link up on rMBP metal from flight 17)»
- **pi-queue.md @1812f6ea1666** — before: «AX88179 front-end built for Peter's dongles, unflown)» — after: «AX88179 front-end built for Peter's dongles, unflown at writing; link up on rMBP metal from flight 17)»
- **rmbp-queue.md @1812f6ea1666** — before: «AX88179 front-end built for Peter's dongles, unflown)» — after: «AX88179 front-end built for Peter's dongles, unflown at writing; link up on rMBP metal from flight 17)»

Annotated, claim unchanged: rmbp-ledger.md B10, LEDGER.md S12, rmbp-queue.md @c7bf03d22123, rmbp-queue.md @ea48deb8053f.
The full row list is `docs/dev/STATUS.tsv` ST43–ST265 (set-by `STATUSBASELINE 2026-10-06`).
