# GATEFIX (rmbp-ledger B476): the seven gate arcs GATEREVIEW left unbuilt

Cut from merge19 5da57a2d. Host only: no kernel file, no knob. Source of the seven: `docs/dev/review/GATES-2026-10-06.md`
(F8, F7, F19 and the plant table rows C1, C6, S2, S4, S6, V1).

## Design (written before the code)

**Finding.** Seven holes, each a plant that a gate passed: V1 (an INNER match arm counts as a verb), C1 (a headerless
file in a new kernel directory is unseen), C6 (`kernel-by-ruling R999` passes), S2 (a quote found only in FLIGHT<N>.md
prose while the flight has a log), S4 (`verified on metal … PASS on flight 24` is no status word), S6 (a hand-typed
`f24-boot9.log` is wire), and ORIN (`orin*/` captures are outside the `f<N>` model, so four Orin rows sit `open`).

**Seam.** Each fix lives in the gate that owns the property; no new gate script except the bench's pin tool:
| arc | gate | rule | plant (selftest/control) | baseline |
|---|---|---|---|---|
| VERBDEPTH | verb-roots.sh | only depth-1 arms of `match command {` | control: an inner arm is not seen, its outer arm is | none (103/103) |
| CHARTERSCOPE | charter-check.sh | every `crates/kernel/src/**/*.rs` | `--selftest`: C1 `desktop/notes.rs` | `charter-scope.baseline` (138, shrink-only) |
| SEAMCITE2 | arch-check.py seamcite | every cited R/B id resolves in RULINGS.md / a ledger | selftest: `R999` and an unknown `B` | arch.baseline unchanged |
| STATUSWORDS | status-check.py T5 | + verified on metal, PASS(ED) on (the) metal, PASS(ED) on flight N | fixture S4 | none (cells corrected) |
| STATUSWIRE | status-check.py T3 | a flight with a log: the quote must be on a log | fixture S2 | none (rows re-quoted) |
| CAPTUREPIN | status-check.py T6 + `scripts/capture-pin.sh` | a capture log is wire only if `evidence/CAPTURES.pin` holds its sha256 | fixture S6 | the 13 rMBP + Orin logs pinned at the cut |
| STATUSORIN | status-check.py | flight `r<N>` = Orin render N: `orin*/**/render<N>-*.log`, `boot-render<N>-*.log`, `FLIGHT-RESULT-render<N>.md` | fixture | none |

**Milestones.** M1 VERBDEPTH · M2 CHARTERSCOPE · M3 SEAMCITE2 · M4+M5 STATUSWORDS + STATUSWIRE + CAPTUREPIN + STATUSORIN (one file, one commit) ·
M6 STRUCTURAL_GATES.md sections.

**Witness.** Host gates; the witness is each gate's exit code and its `--selftest` line (no metal line: R80, nothing at boot).

**Owed (the seat's documents, reported, not edited).** The ledger cells STATUSWORDS surfaces and the ST rows STATUSWIRE
and STATUSORIN re-quote: corrected text below.

## Corrections for the seat (measured on /home/user/UnaOS docs at the seat's tip: 15 findings before, exit 0 after)

**Ledger cells (rmbp-ledger.md status column): append ` — ST<n>` to the existing cell, nothing else changes.**
B127 → `… — ST345` · B193 → ST346 · B201 → ST347 · B203 → ST348 · B230 → ST349 · B232 → ST350 · B233 → ST351 ·
B234 → ST352 · B236 → ST353 · B237 → ST354 · B238 → ST355 · B239 → ST356 · B240 → ST357
(STATUSWORDS surfaced 13 cells, not the review's 5: "PASS on the metal" ×9, "PASS, flight 13" ×3, "PASSED ON ITS FIRST
METAL BOOT" ×1. The review's two literal phrases match no uncited cell today.)

**STATUS.tsv: replace five rows (ST181 and ST236 re-quoted from the log, STATUSWIRE; ST199, ST200 and ST222 move to
`confirmed` on Orin flights, STATUSORIN) and append thirteen (ids next free after the seat's ST344; renumber the cells
with them if the seat has moved). Exact TSV, tab-separated:**
```
ST181	B373 MP3HANG: the decoder job decodes to its end on metal	refuted	f24	[play] dec spawn path=/system/test-f/TEST.FLAC jid=1 stack=65536 cpu=0	STATUSBASELINE 2026-10-06	B373
ST199	SO4 CRYSTALPUT: the crystal menu sits panel-left (Orin render13 and render14 logs carry anchor=panel-left)	confirmed	r13	[menubar] crystal menu=170x121+0+34 anchor=panel-left glyph=16x22+12 bar_w=1920 menu_x=0	STATUSBASELINE 2026-10-06	SO4
ST200	SO5: the sprite divergence measured on Orin render8 (same=0 on 8 of 8 samples)	confirmed	r8	compositor=9x9 backbuffer=18x18 same=0 n=8/8	STATUSBASELINE 2026-10-06	SO5
ST222	SR2 PRTSCR3: the sliced capture passed on Orin render8 (orin-ledger A36)	confirmed	r8	:: PRTSCR: SCREEN3.PNG 1920x1200 6913793 bytes -> OK ::	STATUSBASELINE 2026-10-06	SR2
ST236	FOCUSDEAD (then NETLOCK): tests net hangs on boot 20	refuted	f20	[gui] app-exit t=504s dur=1s wedged=false	STATUSBASELINE 2026-10-06	
ST345	B127 HDA arc 1: the CS4206 codec answers its census on metal	confirmed	f11	[hda] codec=0 vid=1013:4206 rev=0x00100302	GATEFIX 2026-10-06	B127
ST346	B193 DEADMAN: the deadman line precedes the cursor arm on metal	confirmed	f13	[deadman] up=7 hid=1 pmp=1 hq=0 hid_ms=26 comp_ms=6	GATEFIX 2026-10-06	B193
ST347	B201 SDHCBLK: the internal SD card mounts FAT read-write early on metal	confirmed	f13	:: SDHCBLK: FAT mounted READ-WRITE on the internal SD card (29721 MiB)	GATEFIX 2026-10-06	B201
ST348	B203 GLASSFIX2: the window cascade has no overlaps on metal	confirmed	f13	cascade overlaps=0 worst=win0-over-win0:0rows minted=4/4 pinned=3/3 n=10 control=1	GATEFIX 2026-10-06	B203
ST349	B230 CLOCKBAR: the fixture prints PASS on metal (the glass unread)	confirmed	f16	:: CLOCKBAR: anchored=1 text=12:34 drawn=1 -> PASS ::	GATEFIX 2026-10-06	B230
ST350	B232 DIMIDLE: the fixture prints PASS on metal (the glass unread)	confirmed	f16	:: DIMIDLE: idle_min=10 blanked_at_ms=33987 woke_at_ms=34001 wake_key_swallowed=1 -> PASS ::	GATEFIX 2026-10-06	B232
ST351	B233 WINCYCLE: the fixture prints PASS on metal (the glass unread)	confirmed	f16	:: WINCYCLE: windows=4 order=[7, 6, 5, 4] after_tab=5 zoom=1290x108->1290x108 restored=1 -> PASS ::	GATEFIX 2026-10-06	B233
ST352	B234 BRIGHTKEYS: the fixture prints PASS on metal (the glass unread)	confirmed	f16	:: BRIGHTKEYS: key=up level=13/16 gmux_written=0 indicator=1 -> PASS ::	GATEFIX 2026-10-06	B234
ST353	B236 VOLKEYS + KEYREPEAT: the fixtures print PASS on metal (the glass unread)	confirmed	f16	:: VOLKEYS: key=mute level=12/16 muted=1 amp_written=1 indicator=1 -> PASS ::	GATEFIX 2026-10-06	B236
ST354	B237 WALLPAPER: the line printed on metal is a state (src=none, nothing drawn), not a verdict (GATEREVIEW F16)	open	f16	:: WALLPAPER: src=none WxH=0x0 scaled=0x0 letterbox=0 ms=0 -> PASS ::	GATEFIX 2026-10-06	B237
ST355	B238 FILEVIEW: the fixture prints PASS on metal (the glass unread)	confirmed	f16	:: FILEVIEW: path=/hello.txt bytes=108 lines=2 rows=2 wrapped=0 -> PASS ::	GATEFIX 2026-10-06	B238
ST356	B239 NETFETCH-PARSE: the parser fixture prints PASS on metal (fetch itself unread)	confirmed	f16	:: NETFETCH-PARSE: url=true basename=true request=true head=true -> PASS ::	GATEFIX 2026-10-06	B239
ST357	B240 DOCKRUN: the fixture prints PASS on metal (the glass unread)	confirmed	f16	:: DOCKRUN: tiles=5 running=4 pinned=1 raise=ok quit=ok menu_drawn=1 -> PASS ::	GATEFIX 2026-10-06	B240
```

ST198 stays `open` (its measure is offline pixels and a QEMU screendump, not a log line).
Applier used for the measurement: the same edits, scripted; the gate on the corrected copy prints `rows=357 confirmed=158 open=12 parked=4 refuted=17 unflown=166 claiming-uncited=0 baseline=0 -> PASS`.
