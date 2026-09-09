# FLIGHT-RESULT — render6 (orin 15, 2026-09-06T01:27–01:39Z)

Image `render6-20260906T0049Z-f2eae02` (hw-jetson `f2eae02a`; the branch tip at flight time `a05c2c8e` differs
only by LANDING-REPORT-3.md), kernel.elf sha `bec22fd0280a1e85`, ELF max vaddr `0x2db188`. Knob line = render4's +
`UNAOS_ORINCLICK=1 UNAOS_TCUPROBE=1`; effective features
`witness,ehcihid,holocron,tegra,orinclick,tegra_el0,tegrasmp,orinrender,desktop_firmware,orinrx,tcuprobe,deskcascade`.
One boot, one power cycle (Peter cut it at ~01:39Z after the first Print Screen). Scored per `FLIGHT.md` §C with
render4's scorers plus the render6 additions (`scorers-render6.sh` here; output `render6-scores.txt`; marks
`render6-marks.txt`; paced injector log `render6-paced-inject.out`). The seat's live monitor missed the `CPU_ON`
lines (its pattern lacked the `(aff=…)` group) — the FILE is the evidence, the monitor was only the watch.

## Pin and purity (§C.1–C.2)
- Anchor `KELF min=0x0 max=0x2db188 pg=732` is the LAST anchor in `orin.log` (unwrapped line 56818) and in
  `raw.log` (112911); `unknown.log` still ends at render2's `0x2d92a8`. Excerpt `render6-boot1.log` = 3944 lines.
- Purity: `orin_marks=675 pi_marks=0 lines=3944 -> PURE`.

## Scores
| q | scorer line (verbatim) | verdict | ledger |
|---|---|---|---|
| A15 | `cpu_on_success=5 cpu_on_error=0 el3_abort=0 poweroff=0 online_line=1` | PASS — pass 3 of the APTEXT layout (render3b, render4, render6); 0 deaths since APTEXT | A15 |
| A1/§5.2 | `[u7stk] at=boot-core:post-cascade … len=32768 used=240 hw=15552 headroom=17216` (pre: hw=240) | PASS (unsaturated); render4 15584, render3b 15472 — third sample, stable within 112 B | A1 |
| A18 | `cascaded=1 refuse=0 pulsewin_open=1 pulsewin_decline=0 strip_kept=0 census_strip=retired census_pulsewin=2` | PASS, second pass; `[pulsewin] open win=2 … box=1290x212 at (10,914)`, `presents=1` | A18 |
| A19 | wire: `band_cleared=1 shell_present=1 jd2_probe=1` (`[realdesk] band-cleared x=0 y=34 w=1920 h=1166 bg=2d2b55 shell=win=3 surf=960x466 box=970x510 at (515,402)`, `shell-present win=3 outcome=Composited`); pixels: `A19-pngband.py SCREEN0.PNG` → band `non-bg=0/60200 (0.0%)`, controls 0/60200 and 0/30000 | **PASS — the band is gone** (render4 read 855/15050 = 5.7%). A19FIX `6f56eff8` flown | A19 |
| A17 | `armed=1 ok=1 refusals=0 names=[ SCREEN0.PNG ]`; card: `SCREEN0.PNG size=6913793 sig+IHDR=True 1920x1200 -> VALID`, sha `3f4ee48a018a0c46` card == harvest | one press only (the boot was cut before a second); status unchanged from render4's flown | A17 |
| A16 | `lsr_lines=1 iir_lines=1 census=482 rx_final=3 ovrf_final=0`; burst `keys=3 rx_after=3 ovrf=0` (s, t, CR); paced 50 ms/byte `keys=0 rx_after=3 ovrf=0` | competing reader CONFIRMED with the bytes in hand — see A22: the two bytes UARTC lost sit in the TCU RX mailbox. Paced delivered 0/5 (render4: 1/5) while the mailbox stayed full and unconsumed | A16 |
| A22 | `arm=1 stop=0 census=484 full_final=1 nbytes=2 full_edges=1 changes=1 data=[74 65 00] -> ROW1`; arm line `[tcu] hsp top0=0x3c00000 aon=0xc150000 tx-mbox=1 rx-mbox=0 … cells=[0x125 0x1 0x0] … #mbox-cells=2/2`; after the burst `[tcu] rx-mbox raw=0x82006574 full=1 nbytes=2 data=[74 65 00] flush=0 hwflush=0 … full-edges=1 changes=1` and it stays for the rest of the boot | **TCURX-DESIGN §7 row 1: the SPE forwards console RX into the HSP mailbox and parks it until consumed.** `0x82006574` = bit31 full, count 2, byte0 0x74 `t`, byte1 0x65 `e`. Fix = rung 2 (consume in the drain; TCURX2 executor, orin 15). Open datum: with the mailbox full, the paced leg reached neither UARTC nor the mailbox (the §4 UNKNOWN — hold/drop while full) | A22 |
| A20 | `arm_click1=1 orinclick_armed=1 clickroute_press=0 consumed=0 routing_census=0`; every `[orinclick] census … btn=0 press=0 … -> IDLE-NO-CLICKS`; `MOUSE-1: 1 reports` once, never again; `[cursor3] present tail=repaint offers=0 taken=0 -> BRACKETED` ×6 | **FAIL — the pointer is DEAD all boot, upstream of the router.** Control render4 (same image minus `orinclick`+`tcuprobe`): `MOUSE-1: 992 reports`, `[cursor3] … offers=3 … COMPOSED`. One of the two added knobs kills the pointer path. Mechanism: CLICKDEAD executor (orin 15) | A20 |
| A10 | `menu_open=1 esc_seen=0 dismiss=1` — the only menu lines are the cascade's crystal self-test (`SHARD-MENU: crystal_press=open via=crystal-glyph` / `dismiss reason=outside`) | NOT TESTED: no pointer, no menu press possible | A10 |
| liveness | `heartbeat=1 el1=1 arm=1 armed=1 live=483 redzone=0 exceptions=0` | PASS | — |

## Observed at the bench (Peter)
- Print Screen #1 worked (`SCREEN0.PNG … -> OK`, valid on the card).
- Clicks did not land ("you are failing"); he cut the boot. No panel description was given; the wire's `[realdesk]`,
  `[pulsewin] open`, `presents=1` and the PNG are the proxies. The PNG shows the menubar row non-bg (23800/23800,
  by design) and the band clear.

## Not scored
- A17 second press, A10 Esc: the boot ended first (pointer dead, A20).
- Probe rows A23/A12/A24: separate boots; images staged this session (`sdmmcwrite-20260906T0137Z-a05c2c8`,
  `net4-20260906T0138Z-a05c2c8`, `ga10bprobe2-20260906T0139Z-a05c2c8`; ELF max 0x2e6880 / 0x306ee0 / 0x2dc768).

## Card after the flight
`SCREEN0.PNG` (6913793 B, sha `3f4ee48a…`) is on the card and harvested to `~/unaos-bench/scratch/orin15/SCREEN0.PNG`
(sha-equal). Tidy before the next load (FLIGHT.md §A.4) or the next capture is named `SCREEN1.PNG`.

## Boot 2 (same image, 2026-09-06T01:42–01:5xZ; Peter powered on again after boot 1)
Excerpt `render6-boot2.log` (6389 lines, `orin_marks=998 pi_marks=0 -> PURE`, unwrapped anchor 61656); scores
`render6-boot2-scores.txt`. No serial injection this boot (keyboard only: `keys=7`), so A16/A22 legs are N/A and
`[tcu] rx-mbox` reads `FULL-NEVER changes=0` — consistent.
| q | scorer line | verdict |
|---|---|---|
| A15 | `cpu_on_success=5 … online_line=1` | PASS — pass 4 |
| A1 | `hw=15552 headroom=17216` | same as boot 1 |
| A18 | `cascaded=1 … census_strip=retired census_pulsewin=2` | PASS, pass 3 |
| A19 | `band_cleared=1 shell_present=1 jd2_probe=1` | wire PASS (pixels scored on boot 1) |
| A20 | `clickroute_press=5 consumed=9`; `[orinclick] census … press=10 rel=10 consumed=9 -> ROUTING`; `MOUSE-1: 3040 reports`; `[cursor3] … offers=2 … COMPOSED` | **clicks LAND** — the boot-1 pointer death is intermittent, same image |
| A17 | `armed=4 capturing=2 ok=2 refusals=0 names=[ SCREEN1.PNG SCREEN2.PNG ]` (Peter: four fast presses) | two files OK; **the two presses during an in-flight capture printed no refusal** — `Refusal::InFlight` is not on the wire; gap |
| A10 | `KEY 0x1b` ×3 with the menu open, no dismiss; `[pulsewin] menu dismiss reason=content` ×3, each right before a press | **Esc does not dismiss** — confirmed; folds into A25/R21 |
| A27 (new) | `[wm-act] drag-begin win=3 … at (1156,431) -> grabbed` … `drag-end win=3 … at (520,441) -> no-move` ×4 | **drag does not move the window** (Peter: "click and drag does not work but clicks are") |
| liveness | `heartbeat=1 el1=1 … live=736 redzone=0 exceptions=0` | PASS |
Card after boot 2: `SCREEN0.PNG` (boot 1), `SCREEN1.PNG`, `SCREEN2.PNG` — harvest + validate when the card is in the reader.
