# FLIGHT-RESULT — render4 (orin 14, 2026-09-05T23:41–23:5xZ)

Image `render4-20260905T2340Z-518dca3` (hw-jetson `518dca3e`, kernel.elf sha `726459c5cdb23b8f`, ELF max vaddr
`0x2d4400`), knob line unchanged from render3b (`UNAOS_TEGRA=1 UNAOS_TEGRA_EL0=1 UNAOS_WITNESS=1 UNAOS_ORINRENDER=1
UNAOS_DESKCASCADE=1 UNAOS_ORINRX=1 UNAOS_HOLOCRON=1`). One boot, one power cycle, scored per `FLIGHT.md` §C with the
scorers verbatim (`~/unaos-bench/scratch/orin14/scorers.run.sh`, output `render4-scores.txt`).

## Pin and purity (§C.1–C.2)
- Anchor `KELF min=0x0 max=0x2d4400 pg=725` is the LAST anchor in `orin.log` (unwrapped line 54045) and in
  `raw.log` (110138); `unknown.log` still ends at render2's `0x2d92a8`. Excerpt `render4-boot1.log` = 1879 lines
  (216 KB — the census cadence is ~1 line/s per instrument; kept whole because §C.5 and §C.7 count census lines).
- Purity: `orin_marks=483 pi_marks=0 -> PURE`.

## Scores
| q | scorer line (verbatim) | verdict | ledger |
|---|---|---|---|
| A15 | `cpu_on_success=5 cpu_on_error=0 el3_abort=0 poweroff=0 online_line=1` | PASS — pass 2 of the APTEXT layout (render3b, render4); 0 deaths since APTEXT | A15 |
| A1/§5.2 | `[u7stk] at=boot-core:post-cascade … len=32768 used=240 hw=15584 headroom=17184` (pre: hw=240) | PASS (unsaturated); render3b was 15472 — +112 B, the DESKSCENE seam's own frames | A1 |
| A18 | `cascaded=1 refuse=0 pulsewin_open=1 pulsewin_decline=0 strip_kept=0 census_strip=retired census_pulsewin=2` | PASS; `[realdesk] backdrop=desktop-scene retired=pulse-band,status-line bottom_reserved=104->64`; `[pulsewin] open win=2 … box=1290x212 at (10,914)`; `presents=1` and stays 1 | A18, A10 (window back; Esc not pressed) |
| A17 | `armed=2 ok=2 refusals=0 names=[ SCREEN0.PNG SCREEN1.PNG ]`; both `-> capturing` lines preceded their `-> OK` | PASS; card: both 6.9 MB, signature + IHDR 1920x1200 + all CRCs OK, different bytes (shas `12bec046…`, `47ca7177…`); harvested to `~/unaos-bench/scratch/orin14/render4-card-harvest/` | A17 |
| A16 | `lsr_lines=1 iir_lines=1 census=310 rx_final=4 ovrf_final=0`; RX-LIVE line `iir=0xc1 fifo=on`; burst: `keys=3 rx_after=3 ovrf=0` (s, t, CR); paced 50 ms/byte: serial keys=1 (`s`), `rx_after=4 (+1)`, ovrf=0 (the other 7 keys in that window are Peter's USB keyboard: `s 0x08 s t o r m 0x0a`, xHCI, not serial) | DISCRIMINATORS PRESENT; A16-SCORE.md table: burst rx<5 & ovrf=0 & paced rx<5 (worse than burst) & fifo=on ⇒ **COMPETING READER** — the SPE/TCU firmware drains the same RBR; pacing gives it more time per byte and it wins more of them. Overrun refuted twice (ovrf=0 both legs, FIFO on). No FCR write would have fixed it | A16, A4 |
| A19 | SCREEN0.PNG decoded: top-left band (x 0–700, y 34–120) `non-bg=855/15050 (5.7%)` vs controls 0/15050 and 0/6000 | CONFIRMED on metal — the pre-cascade console text survives the desktop-clear under the menubar | A19 |
| liveness | `heartbeat=1 el1=1 arm=1 armed=1 live=312 redzone=0 exceptions=0` | PASS | — |

## Observed at the bench (Peter)
- Keys typed at the USB keyboard reach the console (`storm` + backspace + Enter echoed as `KEY` lines).
- **Clicks do not land.** The render arm line says `click=0`: `UNAOS_ORINCLICK` is not in the flight's knob line
  (frozen at render3b's), so the boot-mouse (xHCI slot 4, SET_PROTOCOL OK) enumerates but nothing routes its presses.
  Not a regression of A1 — a recipe gap; row A20. The cascade refuses only ORINCONWIN/ORINDESK/ORINTENANT, so the
  next flight adds `UNAOS_ORINCLICK=1` (verify first that `orinclick` and `deskcascade` compose in `main.rs`).
- Panel: not reported line by line; the wire's `[realdesk]` + `[pulsewin] open` + `presents=1` are the proxies.

## Not scored
- A10 (Esc on the pulse window's menu): needs a click to open the menu; clicks do not land (A20). Next flight.
