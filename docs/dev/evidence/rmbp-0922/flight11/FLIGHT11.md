# Flight 11 — the score (rMBP, 2026-09-22, image 3)

One attended boot, Peter at the glass, two storms, shutdown. Pre-registration: `PLAYBOOK-flown.md` beside this file. **Capture (read-only source):** `~/unaos-bench/capture/rmbp12-flight8/ttyUSB0.log` after `=== SQUAWK MARK … flight11 card written 56bbe53b image3 511c5aca ===`, read with `awk 'index($0,"<tag>")'`. **Image:** `UnaOS-rmbp-esp-rmbp12flight11-20260922T1415Z-56bbe53`, `kernel.elf` sha256 `511c5aca94eb55ea92935beeb0d12350493497454d6d8461e6dd07995c59a8c8` size 4054064, built from `hw-rmbp@56bbe53b`; GATE8 10/11 rc=0 (knoboff-witness rc=1 by design); banner-cert ok=42. data/ five ELF twins dropped (CARDROOT option 0).

## 1. Score card

| row | verdict on the wire | status |
| --- | --- | --- |
| A12 BOOTCLOCK | `firmware->loader=12850ms loader-read=676ms loader-jump=18ms` (flight 10: 12992) | flown, unchanged |
| FTDI ring | `FTDI-CAP: replayed=481534 cap=1048576 lost=0 head_cut=n` | flown, INTACT |
| B127 HDA | census `0:27.0 8086:1e20` AND `1:0.1 10de:0e1b` (Kepler HDMI audio); `codec=0 vid=1013:4206 rev=0x00100302` — the CS4206, READ FOR THE FIRST TIME; walk `codecs=1 nodes=20 dacs=5 pins=10 path=found -> PASS`; speaker pins 0x0a/0x0b `eapd=0`; tone `lpib=0 -> 38396 (max 192000) bcis=0 … -> FAIL`, Peter heard nothing | flown, walk PASS, tone FAIL → HDAAMP |
| B124 WIFI3/WIFI4 | `upload REFUSED reason=macctl-shape macctl=0xc0020403 shm-enabled=0 want=1 … NOTHING has been written`; `wifi4: end ok=0 stage=gate` | flown, REFUSED safely → WIFISHM |
| A7/B125 GMUXDPCD | `rung=03 name=pp ok=1 … SETTLE ms=210 moved=0` → `rung=04 name=dpcd try=1/7 ok=1 dpcd_rev=0x11` → `LADDER highest=06/10 name=end ok=1 pending=2 gmux=MATCH why=none … dpcd_tries=1` — TIMING THEORY CONFIRMED, first full ladder | flown, PASS → GMUX7 |
| B126 CTRLBIND | twelve `ctrlbind pbdma<i> target=<t> … bind=ok witness=silent`; `chan-restored 04=00000001(want 00000000) verdict=DIRTY` | flown, KF9 CLOSED (silent ×12); restore DIRTY → KF9CLOSE |
| B122 BOOTFAILS | `[wc-x] move-vacate … painted=true desktop=5/5 -> PASS`; `[clickroute] route … -> PASS` | flown, PASS |
| B128 DMGOVLP | `[dmgovlp] verdict … adopt_stretch=0/4 -> FAIL` (image 3 predates the fix) | flown, expected FAIL |
| B123 CARDROOT | `[quarry] open census cwd=/ entries=16 dirs=6 files=10` | flown, PASS (option 0) |
| A9 SERIALDOOR | `[serialdoor] key=0x0d win_focus=0xffffff54 … -> shell` → `[midden] cmd="help" -> TerminalOutput len=3793` | flown, PASS |
| B121 MENUDROP | `[winmenu] open title=Application items=3 at (40,34) … kind=app owner=4`; `press … band=menubar -> closed` | flown, PASS |
| A10 QUARRYCLICK/QUARRYFONT | dir rows `kind=dir -> select` then cd; file rows `kind=file -> select` twice, never open; font good on the glass (Peter) | flown, clicks PASS, file open FAIL → QUARRYOPEN |
| A1/B119 W5SPIN | two storms: `GATE STOLEN` 0, `REHOMED` 0, `is DEAD` 0; `PRESENT-BANDED SPIN` 3697 lines, 1138 `site=2 waited_us=500 -> GAVE-UP` | flown, NO DEAD CORES; contention → VUGPERF |
| A5 / WC-W | `[wc-h] … torn=223`, `[wc-w] … amp=4.44x -> WIDENED`, `[wpace] rate=176.0/s`; Peter: vug performance way off, tearing | flown, open → VUGPERF |
| B117 pointer | `[ptrinstall] installs=4775 reports=4775 lag_max_ms=678 folds=2917` (43 ms / 56 folds before the storm); Peter: mouse jumping, sticky | flown, identity holds, fluidity FAIL → PTRLAG |
| PRTSCR | `PRTSCR-VOL: rung1=read-only rung2=absent -> NO TARGET`; `REFUSED READ-ONLY … internal SD reader is mounted READ-ONLY … no writable USB volume attached` | flown, refused → SCRSHOTRW (queued) |
| startup / crystal | Peter: startup slow, broken crystal at first paint; `[menubar] live passes=1 paints=0` at 27616 ms, first paint at 32667 ms | open → CRYSTALBOOT (queued) |
| shutdown | works (Peter) | flown, PASS |

## 2. The lines

### Audio

```
[  25731ms] [hda] census bdf=0:27.0 id=8086:1e20 class=04 sub=03 progif=00 (hd-audio) bar0=0xc1c10000 irq=0
[  25731ms] [hda] census bdf=1:0.1 id=10de:0e1b class=04 sub=03 progif=00 (hd-audio) bar0=0xc1080000 irq=0
[  25738ms] [hda] codec=0 vid=1013:4206 rev=0x00100302
[  25738ms] [hda] codec=0 fg=0x01 type=0x01 (audio)
[  25740ms] [hda] path codec=0 dac=0x03 -> [10, 3] -> pin=0x0a dev=speaker hops=2
[  25740ms] [hda] walk codecs=1 nodes=20 dacs=5 pins=10 speaker_pin=0x0a hp_pin=0x09 path=found
[  25741ms] [hda] tone arm sd=0 (iss=4 => descriptor 4) fmt=0x0011(readback 0x0011) cbl=192000(readback 192000) lvi=1 bdl=0x20208080 pcm=0x20215800 bytes=192000 tag=1 srst=1/1 ctl=0x00140004
[  26941ms] [hda] tone stream=0 lpib=0 -> 38396 (max 192000) bcis=0 fifo_ready=1 run_ms=1200 sts=0x20 fifoe=0 dese=0 cbl=192000 tag=1 tag_bound=0x10 tag_ok=1
[  26941ms] :: HDA-TONE: lpib_advanced=1 bcis=0 tag_ok=1 fifo_ready=1 run_ms=1200 -> FAIL ::
```

### WiFi

```
[  28307ms] :: wifi2: upload REFUSED reason=macctl-shape macctl=0xc0020403 shm-enabled=0 want=1 big-endian=0 want=0 — the SHM-window and byte-order arguments this upload rests on do not hold for this word. NOTHING has been written ::
[  28307ms] :: wifi2: ucode upload words=9938 crc=0xb2bd00d3 psm=1 rev=0 -> REFUSED(reason=macctl-shape) ::
[  28307ms] :: wifi4: end ok=0 stage=gate radio-id=0x00000000 wrote-radio-addr=0(audited) wrote-radio-data=0(audited — 0x3F8/0x3FA are READ ports here and have no write site in this rung) wrote-core-regs=0(audited — MACCTL, SHM_CONTROL, SHM_DATA and RADIO_CONTROL(0x3E2) have no write site in this rung) wrote-wrapper=0(audited — no PHY reset and no core reset is performed; see the phy-reset REFUSED line) restore=N/A e
```

### GMUX ladder

```
[  27738ms] :: igpu-dpy: rung=03 name=pp ok=1 pp_write=DECLINED why=pp-bits-uncited pp_window=KEYED delays_programmed=0 pp_unwind=0 pp_ctl=0xABCD0008 pp_sts=0x00000000 on_delays=0x00000000 off_delays=0x00000000 div=0x00186904 ::
[  27959ms] :: igpu-dpy: rung=03 name=pp SETTLE ms=210 budget=not-a-cited-T3 moved=0 pp_ctl=0xABCD0008->0xABCD0008 pp_sts=0x00000000->0x00000000 elapsed_ms=215 ::
[  27959ms] :: igpu-dpy: rung=04 name=dpcd try=1/7 ok=1 dpcd_rev=0x11 elapsed_ms=226 ::
[  27964ms] :: igpu-dpy: LADDER highest=06/10 name=end ok=1 pending=2 gmux=MATCH why=none elapsed_ms=231 pp_seen=both pp=0xABCD0008/0x00000000->0xABCD0008/0x00000000 pp_settle_ms=210 aux_port=DPA(0x64010) aux_div=0x0C8 dpcd_tries=1 ::
```

### CTRLBIND

```
[  27659ms] :: kepler: ctrlbind begin chan00=00000000 chan04=00000000 inst_off=02000000 ::
[  27660ms] :: kepler: ctrlbind pbdma0 target=0 ctrl_addr=00000000 bind=ok witness=silent chan00=00002000 chan04=11000001 sched=err=00000002,stat=00000000 ::
[  27663ms] :: kepler: ctrlbind chan-restored 00=00000000(want 00000000) 04=00000001(want 00000000) verdict=DIRTY ::
```

### Storms

```
[ 996123ms] [wc-w] rollup presents=4724 requested_px=34949137 presented_px=155235825 amp=4.44x full_presents=27 bracketq_met=0 bracketq_busy=0 -> WIDENED
[ 997140ms] [wc-w] rollup presents=4725 requested_px=34949461 presented_px=155236149 amp=4.44x full_presents=27 bracketq_met=0 bracketq_busy=0 -> WIDENED
[ 998177ms] [wc-w] rollup presents=4727 requested_px=34958085 presented_px=155244773 amp=4.44x full_presents=27 bracketq_met=0 bracketq_busy=0 -> WIDENED
[ 995429ms] [wc-h] rollup win=12 scope=window emit=14 age_ms=168409 pop=budgeted samples=4 budget=4 pop=all-presents torn=191 stalls=0 longpres=0 declines=0 decl_geom=0 decl_cap=0 decl_lock=0 decl_alloc=0 shrunk=0 blitnet=[0,1,0,0,0,0,0,0] beam=obs beamobs=3344 beamwaits=2768 beamwait_us=15163018 beammaxwait_us=48510 beamgiveup=80 beamcross_ppk=1530344 fixture=0 whole=1725 banded=7 lines=8 minspan=624 minspan_bytes=1
[ 996259ms] [strip] rollup tenant=crystal scope=bar emit=2 age_ms=952274 rect=170x121+0+34 scene=no pop=all-paints paints=2 paint_px=41140 torn=0 beam=obs beamobs=2 beamwait_us=4 beamcross_ppk=151 maxpaint_us=175 minpaint_us=120 rectscan_us=1120 declines=0 decl_lock=0 decl_ready=0 decl_word=0 decl_geom=0 pop=vacates vacates=0 uncovered=0 uncovered_px=0 unerased=0 unerased_px=0 forgotten=0 flat=0 flat_px=0 restored=0 
[ 997434ms] [wc-h] rollup win=8 scope=window emit=27 age_ms=268368 pop=budgeted samples=4 budget=4 pop=all-presents torn=223 stalls=0 longpres=0 declines=0 decl_geom=0 decl_cap=0 decl_lock=0 decl_alloc=0 shrunk=0 blitnet=[0,1,0,0,0,0,0,0] beam=obs beamobs=5903 beamwaits=4336 beamwait_us=10766899 beammaxwait_us=39213 beamgiveup=35 beamcross_ppk=2522328 fixture=0 whole=3911 banded=49 lines=8 minspan=18 minspan_bytes=42
[ 984934ms] [wpace] rollup wins=10 pres=878 paced=0 slept=0ms focus=0 resync=0 overrun=0 rate=175.5/s span=5002ms -> FREE
[ 989935ms] [wpace] rollup wins=10 pres=880 paced=0 slept=0ms focus=0 resync=0 overrun=0 rate=176.0/s span=5000ms -> FREE
[ 994935ms] [wpace] rollup wins=10 pres=880 paced=0 slept=0ms focus=0 resync=0 overrun=0 rate=176.0/s span=5000ms -> FREE
```

### Pointer

```
[ 984915ms] [ptrinstall] installs=4775 reports=4775 lag_max_ms=678 coalesced=2 drains=1856 folds=2917
[ 990112ms] [ptrinstall] installs=4775 reports=4775 lag_max_ms=678 coalesced=2 drains=1856 folds=2917
[ 995457ms] [ptrinstall] installs=4873 reports=4873 lag_max_ms=744 coalesced=2 drains=1859 folds=2950
```

### Quarry / menu / wire / screenshot

```
[ 844506ms] [quarry] press at (399,427) row=15 kind=file -> select
[ 846253ms] [quarry] press at (18,527) row=1 kind=dir -> select
[ 851023ms] [quarry] press at (389,425) row=15 kind=file -> select
[  44030ms] [winmenu] open title=wire items=3 at (40,34) title-x=40 font=chrome20-bold kind=app owner=4
[  44031ms] [winmenu] open title=wire items=3 at (40,34) title-x=40 font=chrome20-bold kind=app owner=4
[  48213ms] [winmenu] open title=Application items=3 at (40,34) title-x=40 font=chrome20-bold kind=app owner=4
[ 628222ms] :: [midden] cmd="ls" -> Host verb=ls ::
[ 729003ms] :: [midden] cmd="storm" -> Host verb=storm ::
[ 826835ms] :: [midden] cmd="pulse" -> Exec pulse.elf ::
[ 798419ms] :: PRTSCR: [prtscr] chord=cmd-shift-3 (GUI+Shift+digit) down on EHCI -> capture armed ::
[ 798421ms] :: PRTSCR-VOL: rung=none rung1=read-only rung2=absent -> NO TARGET ::
[ 798421ms] :: PRTSCR: REFUSED READ-ONLY (source=sdhc serial=0x00000000 label=UNAOS-X86 reason=the internal SD reader is mounted READ-ONLY — only the reserved flight-recorder extent admits a write (SDHC-4c), and no file verb can name it) — no writable USB volume attached either — capture skipped ::
```

PRESENT-BANDED SPIN verdicts: GAVE-UP=1695, RELEASED=2002

## 3. What flight 11 decided

- The freeze is gone: two storms, no steals, no dead cores (W5SPIN). The bound fires on nearly every other vug present (1138 give-ups), which is the tearing and the 4.4x amplification — one mechanism, VUGPERF.
- The gmux ladder completes: a 210 ms settle after the switch is the whole difference (GMUXDPCD); rungs 7-9 next (GMUX7).
- KF9 is closed by twelve silent witnesses; the seam leaves one register dirty (KF9CLOSE).
- The codec is a CS4206; the walk works; the tone's DMA stalls at a fifth and the amp is GPIO-driven (HDAAMP). The WiFi upload refused itself on the MACCTL word (WIFISHM).
- Serial door, menubar band, both boot-ladder fixtures, the 16-entry root, Quarry clicks and the anti-aliased face all PASS on metal.
- New from the glass: pointer lag 678 ms under storm (PTRLAG), file open (QUARRYOPEN), screenshot has no writable target (SCRSHOTRW), slow start + broken crystal (CRYSTALBOOT).

