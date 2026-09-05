# render2 FLIGHT RESULT — 2026-09-05, image render2-20260903T2157Z-8085c9c (hw-jetson 8085c9c8)

Pinned by loader identity: `KELF min=0x0 max=0x2d92a8 pg=730` at raw.log unwrapped line 102226
(render1 was max=0x2da2a8). Scored from `render2-boot.log` (924 lines at scoring time; the excerpt is committed beside this file, ANSI-stripped, first line = the loader anchor), board purity
251 tegra lines / 0 Pi lines (the 5 `PIUSB` hits are the shared USB-storage driver's witness family
printing on the Orin — a GATE-NEUTRAL census item, not Pi traffic). One boot OF RENDER2. Correction from the capture audit (RENDER2-AUDIT.md): the session
had THREE power-ons — raw.log unwrapped 100418 and 101322 are dark boots of the known foreign volume
`0xabfbdefa` (old loader wording, `max_vaddr=0x23b968`) that went silent after ExitBootServices until
Peter cold-cycled; render2 was the third. The firmware can still pick that volume.

| # | question | result | evidence (line in render2-boot.log) |
|---|---|---|---|
| 1 | ordering: DECLINE beside ROUTED, never SHELL-WINDOW | **PROVEN** | 488 `[orinconwin] … -> ROUTED` → 489 `[orinrender] arm` → 494 `spawned tid=4` → 516 `DECLINE reason=console-already-windowed`; SHELL-WINDOW: 0 |
| 2 | zero `[redzone] … orin-render` on the 32 KiB stack | **PASS** | 0 LOW-REDZONE lines for ANY task (render1: 8 orin-render + 8 jd2-console) |
| 3 | `[u7stk]` headroom, passes 1 and 2 | **MEASURED, not saturated** | 520 pass1 `len=32768 hw=13312 headroom=19456`; 541 pass2 `hw=22256 headroom=10512`. **hw=22256 > 16384: the pulse-window open pass alone exceeds the old blanket stack** — direct confirmation of defect 1's mechanism |
| 4 | presents climbs past 1 | **PASS (=2)** | max_presents=2 across 100 census lines; pass 1 (seed) + pass 2 (pulsewin open); then no further dirty pass in 98 s — see OPEN below |
| 5 | census ~1/s | **PASS** | 100 census lines over 98.5 s of `[wc-h] age_ms` ruler = **1.02/s**, 10.8% of the boot (render1: 24.08/s, 74.5%) |
| 6 | no `REFUSE reason=stage-unreserved` | **PASS** | REFUSE: 0 |
| 7 | one byte over the FIFO | **nothing** (as predicted; RX not wired) | +22 raw lines in 6 s, all periodic rollups; KEY echoes 0; marks.txt entry written |

**First-ever on the Orin:** `[pulsewin] open win=3 panel=1920x1200 surf=1280x168 box=1290x212 at (10,874)`
(line 528) — defect 4's fix opened the pulse window; its ARMED latch had never been set on this board.

**The panic-pattern hit** (477) is `[wc-x] console-window panic-fallback armed` — a witness that the
fallback is armed, not a panic. Heartbeat live at 121. No exception, no wedge; RENDER-LIVE continuous.

**OPEN (not this arc's defect, record for orin 14):** presents stays at 2 for the rest of the boot —
`ui_status::tick` never dirties again, so the status strip on the Orin does not repaint after pass 2.
`[pulse5] live c0=0ms … span_max=0ms` suggests the load sampler reads all-zero on this board, so the
bars have nothing to change. Not a regression (render1 presented once, ever); a rung-6 question.
