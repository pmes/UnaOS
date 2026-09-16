# W5SCOPE — every stall of flights 8 and 9, tabled (rMBP, capture of 2026-09-16)

Scoping record for `PCIE-RP-RECOVERY.md` §12 (W5 — credits / the GPU window path) and rmbp-ledger
B119. Nothing here is a repair; every cell is a measurement of one capture, with the command that
produced it.

**Source (read-only):** `~/unaos-bench/capture/rmbp12-flight8/ttyUSB0.log`, 11 265 lines, 1 399 019
bytes, two boots in one file. Read with `awk 'index($0,"<token>")'`, never a bare `grep`. Below, `$L`
names that path.

**Boot split, measured rather than inherited.** SPANFLUSH (`engine.md`, commit 269735d1) split the
file at line 9027, the second `ftdi:console-up`. That is the line at which flight 9's live stream
starts, but flight 9's boot-capture ring is *replayed* ahead of it (B114), so flight 9's early lines
(its `rp-boot`, `BAR1WEDGE:`, kepler bring-up) sit at lines 6658–9026 with timestamps under 21 s.
The honest split is the in-boot clock reset:

```
awk '{ if (match($0,/^\[ *[0-9]+ms\]/)) { ts=substr($0,RSTART+1,RLENGTH-4)+0;
       if (prev!="" && ts < prev-100000) print "reset at line " NR ": prev=" prev " now=" ts; prev=ts } }' $L
→ reset at line 6658: prev=395002 now=483
```

So **flight 8 (WC) = lines 1–6657, last in-boot timestamp 395 002 ms; flight 9 (UC) = lines
6658–11265, last in-boot timestamp 139 357 ms.** Every per-boot count below uses that boundary
(`NR<6658`).

---

## 1. The seven stalls

Columns: `t0` = first tripwire timestamp minus its `age_ms` (the hold's start); `steal` = the `GATE
STOLEN` line; `retired@n1..4` = `blits_retired` on the four 1 Hz tripwire samples (one value —
identical on all four, every stall); `next pass` = the first `[comp2] rollup` after the steal whose
`passes`/`blit_us` prove the NEW holder's CPU blits into the same BAR1 completed; `Δretired→next`
= the odometer at the next stall's `n=1` minus this stall's, i.e. blits the other cores retired
into the aperture while this core stayed parked (Δt = from this steal to the next stall's `n=1`);
`revenant window` = boot's last timestamp minus the steal (how long `revenants=` had to move, and
it never did).

| # | flight / aperture | t0 (ms) | holder | steal: by, at (ms), held | `at=` / phase | win / row | `blit_aim` | `blit_inflight` | `retired@n1..4` = at steal | next pass after steal | Δretired→next stall (Δt) | revenant window |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| S1 | f8 wc | 330272 | c1 | c0, 334303, 4030 ms | span-flush / 33 | 9 / 913 | 0xe44988 | 1 | 7 564 857 | +121 ms: `passes=1 pass_us=84025 blit_us=83991` | +4 203 959 (37.1 s) | 60 699 ms |
| S2 | f8 wc | 370382 | c2 | c0, 374635, 4253 ms | span-flush / 33 | 7 / 331 | 0x52e1d0 | 1 | 11 768 816 | +369 ms: `passes=15 pass_us=23101 blit_us=22390` | +660 190 (7.0 s) | 20 367 ms |
| S3 | f8 wc | 380643 | c3 | c4, 384897, 4253 ms | span-flush / 33 | 5 / 524 | 0x830f20 | 1 | 12 429 006 | +359 ms: `passes=38 pass_us=19779 blit_us=19236` | +469 394 (6.1 s) | 10 105 ms |
| S4 | f8 wc | 389982 | c4 | c0, 394285, 4303 ms | **pw-exit / 49** | 8 / 287 | 0xa59878 | **0** | 12 898 400 | +70 ms: `passes=1 pass_us=47211 blit_us=47006` | — (reboot typed 717 ms later) | 717 ms |
| S5 | f9 uc | 94288 | c1 | c6, 98304, 4016 ms | span-flush / 33 | 8 / 1399 | 0x15dc030 | 1 | 2 076 538 | +95 ms: `passes=6 pass_us=115562 blit_us=115030` | +108 331 (4.2 s) | 41 053 ms |
| S6 | f9 uc | 101475 | c2 | c3, 105725, 4250 ms | span-flush / 33 | 6 / 610 | 0x989878 | 1 | 2 184 869 | ≤ +2823 ms (rollup cadence): `passes=26 pass_us=105559 blit_us=104120` | +268 799 (8.6 s) | 33 632 ms |
| S7 | f9 uc | 113308 | c3 | c5, 117571, 4262 ms | span-flush / 33 | 5 / 156 | 0x270f20 | 1 | 2 453 668 | ≤ +5096 ms (rollup cadence): `passes=46 pass_us=108222 blit_us=106946` | — (reboot typed 21.8 s later) | 21 786 ms |

Commands (each column is one of these, read off the printed lines):

```
# holder, age, at=, win, row, blits_retired, blit_aim, blit_inflight — 28 lines, 4 per stall
awk 'index($0,"PASS OVERDUE"){print NR": "$0}' $L
# stealer, steal timestamp, held ms — 7 lines
awk 'index($0,"GATE STOLEN"){print NR": "substr($0,1,220)}' $L
# first [comp2] rollup after each steal (+ms, passes, pass_us, blit_us) and the boot's last timestamp
for st in 334303 374635 384897 394285 98304 105725 117571; do awk -v st=$st 'BEGIN{b=1} {
  if (match($0,/^\[ *[0-9]+ms\]/)) { ts=substr($0,RSTART+1,RLENGTH-4)+0; if (prev!="" && ts<prev-100000) b=2; prev=ts;
  want=(st>200000)?1:2; if (b!=want) next; last=ts;
  if (!done && ts>st && index($0,"[comp2] rollup")) { done=1; print "steal=" st " first-comp2-after=" ts " (+" ts-st " ms) " $0 } } }
  END { print "  boot-end-ts=" last " revenant-window=" last-st " ms" }' $L; done
# revenants: 96 rollups carry the field, 0 read non-zero
awk 'index($0,"revenants="){n++} END{print n+0}' $L                                  # 96
awk 'index($0,"revenants=") && !index($0,"revenants=0 "){n++} END{print n+0}' $L      # 0
# the debt lines that pair with each steal (one per dead core, owed=1)
awk 'index($0,"blit debt forgiven"){print NR": "substr($0,1,120)}' $L
```

**What the table says, before any theory.** (i) Six of seven stalls are the in-copy shape
(`at=span-flush blit_inflight=1`); S4 is a wedged core whose last blit had retired (`pw-exit`,
`blit_inflight=0`), the flight-1 downstream shape, and it is the only WC stall with that shape.
(ii) The steal fired at 4016–4303 ms on every stall; no hold ever ended any other way.
(iii) **After every one of the seven steals, the new holder's CPU blits into the same BAR1
aperture completed** — within 70–369 ms on flight 8 (a `[comp2]` pass with `blit_us` of
19–84 ms is a completed copy of megabytes into the aperture), and within one 5 s rollup on
flight 9 — while the parked core's single store never retired for the rest of the boot
(`revenants=0` on all 96 rollups; 60.7 s, 20.4 s, 10.1 s, 41.1 s, 33.6 s, 21.8 s of opportunity on
S1–S3 and S5–S7). The aperture, the link and the root port were sinking posted writes from
other cores within a fraction of a second of each steal. (iv) The odometer moved by 0.1–4.2
million blits between consecutive stalls on the same boot, all of it through the same BAR1 window
the dead core is parked on.

---

## 2. What the root-port sampler read at every crossing

All 28 `[pcih] wedge-sample` lines (16 on f8, 12 on f9) carry ONE tail after `aperture=`:

```
awk 'index($0,"wedge-sample"){s=$0; sub(/.*aperture=[a-z]+ /,"",s); print s}' $L | sort | uniq -c
→ 28 lnksta=1081 d_lnksta=0000 (2.5GT/s x8 training=0 bwmgmt=0 autobw=0) lnkctl=0040 lnkctl0=0040 aspm=off lnkdis=0 retrain=0 devsta=0000 d_devsta=0000 secsta=2000 d_secsta=2000 devctl2=0000 cto=50us-50ms(default) dis=0 aer=n uesta=00000000 cesta=00000000
```

and the 28 paired `[pcih] rp-at-wedge` lines all read `lnksta=1081 devsta=0000 secsta=2000 aer=n`
(`awk 'index($0,"rp-at-wedge")' $L | sort | uniq -c` → the same line, 28 of the 30 hits; the other
two are the `rp-boot` reference lines). Decoded per `PCIE-RP-RECOVERY.md` §11.2:

| register | value at every sample | reading for W5 |
|---|---|---|
| Link Status `cap+0x12` | `1081`, `d_lnksta=0000` | Gen1 x8, not training, no bandwidth latch since the post-enum clear — the link never left L0 in a way the RP records |
| Link Control `cap+0x10` | `0040`, `lnkdis=0 retrain=0` | ASPM off on both boots (`UNAOS_NOASPM` armed), link not disabled |
| Device Status `cap+0x0A` | `0000`, `d_devsta=0000` | no error latched on the RP; **Transactions Pending [5] = 0**: the root port had no outstanding non-posted request of its own |
| Secondary Status `0x1E` | `2000`, `d_secsta=2000` | Received Master Abort — set after the post-enum clear, and B113 names the live alternative (the wifi sweep at 23516 ms, 253 ms after the clear); not attributable to the wedge on this capture |
| Device Control 2 `cap+0x28` | `0000`, `cto=50us-50ms dis=0` | completion timeouts are ON and at the default range: a non-posted read that IS transmitted to a silent endpoint returns within 50 ms |
| AER | `aer=n` | the Ivy Bridge PEG root port carries no AER capability; `uesta`/`cesta` are constants |

No root-port register moved between the first crossing and the steal on any of the seven stalls,
on either aperture type.

---

## 3. What preceded each stall (last rollups before t0, the deadman's first sample inside the hold)

| # | last `[comp2]` before t0 | last `[wcser]` rollup before t0 | `[deadman]` first sample inside the hold | storm |
|---|---|---|---|---|
| S1 | 327577: `passes=176 pass_us=18239 max_us=35297 blit_us=18180` | 327577: `declined_pct=87 holder=1 held_ms=20 steals=0` | 330707: `comp_ms=435 gate=1/435` | begin 324427, end 324514; first stall 5.8 s after |
| S2 | 369505: `passes=241 pass_us=17769 max_us=37071 blit_us=17755` | 369505: `declined_pct=87 holder=3 held_ms=6 steals=1` | 371309: `comp_ms=927 gate=2/927` | 46 s after storm |
| S3 | 380213: `passes=7 pass_us=17906 max_us=23748 blit_us=17892` | 380213: `declined_pct=85 holder=7 held_ms=10 steals=2` | 381403: `comp_ms=760 gate=3/760` | 56 s after storm |
| S4 | 387992: `passes=153 pass_us=16786 max_us=53581 blit_us=16591` | 387992: `declined_pct=86 holder=7 held_ms=14 steals=3` | 390469: `comp_ms=487 gate=4/487` | 66 s after storm |
| S5 | 93585: `passes=24 pass_us=118601 max_us=133356 blit_us=118047` | 90733: `declined_pct=100 holder=-1 -> WEDGED` (UC price, A13) | 94548: `comp_ms=265 gate=1/260` | begin 65472, end 65558; first stall 28.8 s after |
| S6 | 98399: `passes=6 pass_us=115562 max_us=132911 blit_us=115030` | 98399: `declined_pct=99 holder=6 held_ms=95 steals=1` | 101668: `comp_ms=193 gate=2/193` | 36 s after storm |
| S7 | 108548: `passes=26 pass_us=105559 max_us=217444 blit_us=104120` | 108549: `declined_pct=98 holder=7 held_ms=52 steals=2` | 113729: `comp_ms=421 gate=3/421` | 48 s after storm |

```
for t0 in 330272 370382 380643 389982 94288 101475 113308; do awk -v t0=$t0 'BEGIN{b=1} {
  if (match($0,/^\[ *[0-9]+ms\]/)) { ts=substr($0,RSTART+1,RLENGTH-4)+0; if (prev!="" && ts<prev-100000) b=2; prev=ts;
  want=(t0>200000)?1:2; if (b!=want) next;
  if (ts<t0 && index($0,"[comp2] rollup")) c2=NR": "$0; if (ts<t0 && index($0,"[wcser] scope=live")) ws=NR": "$0;
  if (ts>=t0 && ts<=t0+1100 && index($0,"[deadman]")) dm=NR": "$0 } }
  END { print "t0=" t0; print "  comp2<t0: " c2; print "  wcser<t0: " ws; print "  deadman@hold: " dm }' $L; done
awk 'index($0,":: STORM: begin")||index($0,":: STORM: end"){print NR": "substr($0,1,80)}' $L
```

The healthy pass immediately before every WC stall is 17–18 ms (`blit_us`), and before every UC
stall 104–118 ms — the ~6.8x the ladder predicted. The stalls are not slow passes: `[deadman]`
sees the gate held 193–927 ms at its first 1 Hz sample and the tripwire sees the same row on four
consecutive seconds. `gate=<core>/<ms>` on the `[deadman]` line comes from
`wm::deadman_gate_sample()` (`video/wm.rs:26181`, called at `deadman.rs:344`), the same two atoms
the tripwire reads — the deadman is already a ~1 Hz observer of every hold.

---

## 4. What the Kepler driver had done before each stall, and whether the GPU was idle

Every driver family's LAST line, per boot (`NR<6658` = flight 8):

```
for tok in ':: kepler:' ':: KFBIND:' ':: KDHEAD:' ':: kdisp:' '[NVIDIA]' 'PFIFO' ':: igpu-dpy:' '[GMUX]'; do
  awk -v t="$tok" 'index($0,t){ b=(NR<6658)?1:2; l[b]=NR": "substr($0,1,70); c[b]++ }
    END { for(b=1;b<=2;b++) printf "%-14s boot%d n=%d last=%s\n", t, b, c[b]+0, l[b] }' $L; done
```

| family | flight 8: n, last line | flight 9: n, last line |
|---|---|---|
| `:: kepler:` | 323, `[23178ms] fecs-ledger accesses=972` | 323, `[20975ms] fecs-ledger accesses=971` |
| `:: KFBIND:` | 38, `[23177ms] KFBIND: end rung=KF27` | 38, `[20974ms] KFBIND: end rung=KF27` |
| `:: KDHEAD:` | 36, `[22867ms] KDHEAD: end rung=KD14` | 36, `[20147ms] KDHEAD: end rung=KD14` |
| `:: kdisp:` | 119, `[23178ms] bring-up phase=scanout_handover` | 119, `[20975ms] bring-up phase=scanout_handover` |
| `[NVIDIA]` | 12, `[23178ms] Initialization complete (Phases 1-4)` | 12, `[20975ms] Initialization complete` |
| PFIFO | 6, `[23174ms] PFIFO_CHAN[1] post-submit` | 6, `[20972ms] PFIFO_CHAN[1] post-submit` |
| `:: igpu-dpy:` / `[GMUX]` | 4 / 7, `[23188ms]` | 4 / 8, `[20985ms]` |

GPU-driver lines from 60 s before each storm to the end of the boot: **0 on both boots**
(`awk` over the union of those tokens with `ts >= STORM_begin - 60000`; printed count `boot1 … 0`,
`boot2 … 0`). The last driver-issued PFIFO submission (`KFBIND` KF27, `PFIFO_CHAN[1] post-submit`)
is 307 s before S1 on flight 8 and 73 s before S5 on flight 9. The compositor never submits to
the GPU: `grep -c 'kepler_ce::\|kepler_fifo::' unaos/crates/kernel/src/video/wm.rs` = 0, and the
span-flush site is a CPU `copy_nonoverlapping` into the aperture (`video/wm.rs:10686` →
`video/framebuffer.rs:481`). **Every stall is a pure CPU blit with the GPU idle from the driver's
side** — no PFIFO/CE submission coincides with any of them; the only GPU activity is the display
engine's own scan-out of the same VRAM, which Apple's EFI programmed and this kernel never touches
(`PCIE-RP-RECOVERY.md` §2).

**gmux state at every stall:** the last mux lines on both boots are at 23188 / 20985 ms — the
`igpu-dpy` ladder's switch was reverted (`revert read-back: DDC=0x02 SWITCH_DISP=0x03 … READ_EXT=0x21`,
`ext_state=kepler-owned->kepler-owned`) and no mux line follows for the rest of either boot
(`awk 'index($0,":: igpu-dpy:")||index($0,"[GMUX]"){print NR": "substr($0,1,120)}' $L`). The
panel is on the Kepler side during every storm.

**Knobs armed on these boots:** the `⚡ kernel features:` banner is on neither wire
(`awk 'index($0,"kernel features")' $L` = 0 hits; evicted by the ring, B114). The knob line is the
bench MANIFEST (`~/unaos-bench/flash/rmbp/MANIFEST`, rows `rmbp12flight8` / `rmbp12flight9`, as
`FLIGHT8-9.md` cites it): both images carry `UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1
UNAOS_KEPLER_FIFO=1 UNAOS_KEPLER_CE=1 UNAOS_NOASPM=1 UNAOS_WIFI=1 UNAOS_WIFI2=1 UNAOS_DEADMAN=1
UNAOS_BAR1WEDGE=1 …`, image A adds `UNAOS_BAR1EXP=uc`. The wire proves the ones that matter here:
`[pcih] aspm cleared rp 0043->0040 ep 0043->0040` once per boot (lines 1314 / 8013), `aspm=off`
(`lnkctl=0040`) and `aperture=wc|uc` on every sample line, and the KFBIND/CE boot legs above.

---

## 5. Boot-time facts the design rests on (flight 8 lines; flight 9 identical in every field)

```
awk 'NR<6658 && (index($0,"bar1-identity")||index($0,"[GPU] BAR")||index($0,"mmio-map")||index($0,"PMC Enable")||index($0,"Chipset")||index($0,"PFB Reported")||index($0,"Total BAR1")||index($0,"[pcih] ep")||index($0,"[pcih] rp ")||index($0,"KDHEAD: gop")){print NR": "$0}' $L
```

| fact | line |
|---|---|
| BAR0 | `[GPU] BAR0: 0xC0000000 (Size: 16777216 bytes)`; mapped UC `:: x86 mmio-map: 0xc0000000..0xc1000000 uc=8 (PAT PA3) wc-kept=0 ::` |
| BAR1 | `:: x86 mmio-map: 0x90000000..0xa0000000 uc=113 (PAT PA3) wc-kept=15 ::` (f8, WC arm) / `uc=128 … wc-kept=0` (f9, UC arm); `[NVIDIA] Initialized VRAM bump allocator. Total BAR1 visible: 256 MB` |
| VRAM | `[NVIDIA] PFB Reported VRAM Size: 512 MB`; `Chipset: 0xE7` (GK107); `PMC Enable: 0xE011216D` |
| the framebuffer the CPU blits | `:: KDHEAD: gop w=2880 h=1800 vram_off=00020000 pitch=16384 ::` — the GOP surface at BAR1 + 0x20000 |
| BAR1 addressing | `:: kepler: bar1-identity … bar1_rb=CEA50BA5 win_pre=00003FF0 win_want=00000201 win_rb=00000201 pramin_read=CEA50BA5 win_restored=Y ::` → `VERDICT IDENTITY — BAR1 offsets ARE physical VRAM addresses on this part` |
| endpoint link | `[pcih] ep bdf=1:0.0 lnkcap=00453d03 lnkctl=0043 lnksta=1081 devctl=2930 devsta=0009 aspm_en=L0sL1 aer=y` |
| root port | `[pcih] rp bdf=0:1.0 lnkcap=0261ac83 lnkctl=0043 lnksta=d881 devctl=0020 devsta=0000 aspm_en=L0sL1 aer=n`; `BAR1WEDGE: … capver=2 v2=1`; `cto rp devcap2=00000000 ranges=0 … value=50us-50ms(default) dis=0` |

`ep devsta=0009` at boot: bits [0] Correctable Error Detected and [3] Unsupported Request Detected
are latched on the ENDPOINT at census time. Nothing ever reads the endpoint again (by design,
`pcihealth.rs` header), so whether they move during a stall is unmeasured — see §12.3 of the design
for why they stay unread.
