# Flight 10 — the score (rMBP, 2026-09-17, image 2b)

One attended boot, Peter at the glass. The pre-registration is `PLAYBOOK-flown.md` beside this file, copied verbatim from `~/unaos-bench/PLAYBOOK-rmbp.md` after the flight.

**Capture (read-only source):** `~/unaos-bench/capture/rmbp12-flight8/ttyUSB0.log` from line 11269 (`=== SQUAWK MARK flight10`) — the same file that holds flights 8 and 9. Every line below is copied verbatim, read with `awk 'index($0,"<tag>")'`, never a bare `grep`. In-boot `[ NNNNNms]` timestamps restart at the boot.

**Image.** `UnaOS-rmbp-esp-rmbp12flight10-20260916T2132Z-fa4dcf0`, `kernel.elf` sha256 `2b0db1e530ea115ad15d8f2809681a11d5338b33cb21f174f134c14b2fd25549` size 3966448, built from `hw-rmbp@fa4dcf0b` (16 folds after flight 9). Knob line: the STATE line of `docs/dev/OS/rmbp-queue.md` at fa4dcf0b (gate7 flight line, +`UNAOS_QUARRY=1 UNAOS_KEPLER_KFBIND=1 UNAOS_KEPLER_KDHEAD=1 UNAOS_IVB3D_R8=1 UNAOS_BAR1WEDGE=1 UNAOS_AHCI=1`), recorded in `~/unaos-bench/flash/rmbp/MANIFEST`. Load offset on this ELF: wire memcpy 0x79454510 − ELF memcpy 0x2fa510 = 0x7915a000 (used to resolve every rip below with `llvm-nm -n`).

## 0. Boot anchors

```
=== SQUAWK MARK 2026-09-17T14:49:29Z session-start ===
[    116ms] :: BOOTCLOCK: firmware->loader=12992ms loader-read=666ms loader-jump=20ms kernel-entry=36851193537 tsc_hz=2693860780(measured) ::
[  69526ms] :: FTDI-CAP: replayed=474580 cap=1048576 lost=0 head_cut=n ::
[  26703ms] :: X86BIND: root=sdhc:/kernel.elf serial=0x00000000 by=content bootinfo=0x00000000 agrees=unknown mounts=4 layout=true -> PASS ::
[  26714ms] :: [fatverb] storage settle: waited=0ms settled=found handles=global=absent sdhc=present ::
[  26345ms] [dock] pins=2 quarry=yes console=yes shell=yes pulse=no tiles=3 quarry_compiled=yes ::
```

## 1. Score card

| rung / row | verdict on the wire | status |
| --- | --- | --- |
| A12 BOOTCLOCK | `firmware->loader=12992ms loader-read=666ms loader-jump=20ms` — the slow boot is 13 s of firmware (and the ⌥ picker) before the loader; the ELF read is 0.7 s | flown, ANSWERED |
| FTDI ring (FBWCWIT/SINKDRAIN) | `FTDI-CAP: replayed=474580 cap=1048576 lost=0 head_cut=n` — first flight whose early lines are evidence | flown, INTACT |
| B115 RENDSTACK / U7XSTACK | `STACK render high=16112 of 32768` on metal (QEMU 15536) — the old 16 KiB would have overflowed | flown, PASS |
| STEALMS (1.5 s steal) | `GATE STOLEN … after 1528ms` / `1768ms` / `1750ms` — three steals, all inside 1.5 s + one pass; no 4 s hold | flown, WORKS |
| A1 / B119 W5 | three dead cores, all `W5: nmi core=cN taken=y … in_blit=n`, rips 0x79253da2 / 0x79253da6 / 0x79253da6 = `video::wm::present_banded +0x462/+0x466` — a SOFTWARE SPIN, not a parked BAR1 store; register dump identical each time (`pri_fault=80300001 bar1_fault=22010860/8482d000/00000002/00221500 pbus_intr=c pfifo_intr=1`) | flown, W5 ANSWERED → W5SPIN |
| B117 PTRINSTALL / PTRINSTALL2 | installs == reports, `lag_max_ms=44`; cursor `flicker_frames=0` | flown, PASS |
| A10 QUARRYCLICK | Quarry opens (`win=5 cwd=/ 21 entries`) but every in-window press is `-> consume deliver=0` | flown, DEFECT → QUARRYCLICK |
| B121 MENUDROP | menubar presses at y≤11 route to `win=0` (no menubar band on x86); Pulse publishes no View menu | flown, DEFECT → MENUDROP |
| A9 FTDICR | `help\r` over the wire: `[quarry] key_route key=0x0d focus=1 took=1` — a focused Quarry ate Enter; the same byte passed at `focus=0` and submitted `cmd="helpdatehelp"` | flown, wire OK → SERIALDOOR |
| B122 BOOTFAILS | `[wc-x] move-vacate … painted=false -> FAIL` and `[clickroute] route … deflect=true -> FAIL` on the boot ladder, green on QEMU | flown, DEFECT → BOOTFAILS |
| B123 CARDROOT | `/` lists the flat card root (APPS/ B43/ EFI/ apps/ boot/ volumes/ + BLOCK.TXT GROW.BIN HELLO.BIN PULSE.ELF S8W.BIN SCRATCH.BIN) | flown, DEFECT → CARDROOT |
| A7 GMUX ladder | `pre-switch … gate=ACCEPT`, `rung=00 name=census ok=1`, switched, `LADDER highest=03/10 name=dpcd ok=0 gmux=MATCH why=aux-timeout-error elapsed_ms=9` (flight 8 reached 05/10 on the same channel) | flown, rung 3 → GMUXDPCD |
| WIFI2 / S4 | `wifi2: end ok=1 stage=d11 d11=FOUND`, set-validation VALID, `upload REFUSED reason=wifi3-not-armed` | flown, PASS; WIFI3 next |
| CE-LADDER | `r1_ptop=partial r2_ce_probe=present r3_rlscan=no-data r5_inst=void r2b_arm=skipped` | flown, unchanged |
| B8 NIC | `Found network controller (class 0x02) vendor 0x14e4 at 3:0.0` — the BCM4331 is the only NIC; the row's 'no driver' is the wifi ladder | flown |
| SMC | `SMC-BATT: present=true soc=80% … ac=derived:discharging` | flown, PASS |
| A8 BT | `bt-c1: page summary attempts_run=2/2 … -> NOT REACHED` | flown, open |

## 2. The lines

### STEALMS and the three dead cores

```
[ 665299ms] :: [wcser] PASS OVERDUE holder=c1 age_ms=1000 pending=true win=10 phase=42 at=pw-strip row=10 blits_retired=15146419 blit_aim=0x1544030 blit_inflight=0 == tripwire ::
[ 667651ms] :: [wcser] PASS OVERDUE holder=c2 age_ms=1000 pending=true win=7 phase=33 at=span-flush row=134 blits_retired=15322727 blit_aim=0x219778 blit_inflight=1 == tripwire ::
[ 841058ms] :: [wcser] PASS OVERDUE holder=c3 age_ms=1000 pending=true win=9 phase=49 at=pw-exit row=287 blits_retired=34614963 blit_aim=0xa660d0 blit_inflight=0 == tripwire ::
[ 665828ms] :: [wcser] GATE STOLEN from c1 by c5 after 1528ms — the holder was in phase 42 row 10 and had not moved. blits_retired=15146419 blit_aim=0x1544030 blit_inflight=0 The acquirer was CHOSEN, not next in line: pick=pool render=1 svc=7; desktop resumes on this core, c1 is DEAD and its singleton roles are owed a re-home == tripwire ::
[ 668402ms] :: [wcser] GATE STOLEN from c2 by c0 after 1750ms — the holder was in phase 33 row 134 and had not moved. blits_retired=15322727 blit_aim=0x219778 blit_inflight=1 The acquirer was CHOSEN, not next in line: pick=pool render=1 svc=7; desktop resumes on this core, c2 is DEAD and its singleton roles are owed a re-home == tripwire ::
[ 841827ms] :: [wcser] GATE STOLEN from c3 by c5 after 1768ms — the holder was in phase 49 row 287 and had not moved. blits_retired=34614963 blit_aim=0xa660d0 blit_inflight=0 The acquirer was CHOSEN, not next in line: pick=pool render=1 svc=7; desktop resumes on this core, c3 is DEAD and its singleton roles are owed a re-home == tripwire ::
[ 665834ms] :: [wcser] REHOMED the render role from DEAD c1 to c2 — GUI_CHANNEL_X86 has a consumer again and the input path is ungated; c1 and its in-flight window stay lost == tripwire ::
[ 668425ms] :: [wcser] REHOMED the render role from DEAD c2 to c3 — GUI_CHANNEL_X86 has a consumer again and the input path is ungated; c2 and its in-flight window stay lost == tripwire ::
[ 841833ms] :: [wcser] REHOMED the render role from DEAD c3 to c4 — GUI_CHANNEL_X86 has a consumer again and the input path is ungated; c3 and its in-flight window stay lost == tripwire ::
[ 665828ms] :: W5: site=post-steal from=c5 dead=c1 held=1528 aim=0x1544030 pmc_intr=00000000 pfifo_intr=00000001 flush=00000000 ep_pcists=0010 pbus_intr=0000000c pri_fault=80300001/00000000 fault_mask=00000000 bar1_fault=22010860/8482d000/00000002/00221500 ::
[ 668402ms] :: W5: site=post-steal from=c0 dead=c2 held=1750 aim=0x219778 pmc_intr=00000000 pfifo_intr=00000001 flush=00000000 ep_pcists=0010 pbus_intr=0000000c pri_fault=80300001/00000000 fault_mask=00000000 bar1_fault=22010860/8482d000/00000002/00221500 ::
[ 841827ms] :: W5: site=post-steal from=c5 dead=c3 held=1768 aim=0xa660d0 pmc_intr=00000000 pfifo_intr=00000001 flush=00000000 ep_pcists=0010 pbus_intr=0000000c pri_fault=80300001/00000000 fault_mask=00000000 bar1_fault=22010860/8482d000/00000002/00221500 ::
[  26630ms] :: W5: nmi core=c6 taken=y rip=0x793472ed in_blit=n cs=0x8 memcpy=0x79454510..0x7945453f icr=ok ::
[  26635ms] :: W5: nmi core=c6 taken=y rip=0x79436233 in_blit=n cs=0x8 memcpy=0x79454510..0x7945453f icr=ok ::
[ 665828ms] :: W5: nmi core=c1 taken=y rip=0x79253da2 in_blit=n cs=0x8 memcpy=0x79454510..0x7945453f icr=ok ::
[ 668402ms] :: W5: nmi core=c2 taken=y rip=0x79253da6 in_blit=n cs=0x8 memcpy=0x79454510..0x7945453f icr=ok ::
[ 841827ms] :: W5: nmi core=c3 taken=y rip=0x79253da6 in_blit=n cs=0x8 memcpy=0x79454510..0x7945453f icr=ok ::
```

### Pointer and stack

```
[  46145ms] [cursor11] compose-through scope=fixture passes=108 bracketed=27 px_deferred=14688 px_installed=6048 px_redrawn=1728 flicker_frames=0 px_absorbed=0 absorb_refused=1 -> THROUGH
[  48288ms] [cursor11] compose-through scope=desk passes=108 bracketed=27 px_deferred=14688 px_installed=6048 px_redrawn=1728 flicker_frames=0 px_absorbed=0 absorb_refused=1 -> THROUGH
[  60411ms] [cursor11] compose-through scope=live passes=108 bracketed=27 px_deferred=14688 px_installed=6048 px_redrawn=1728 flicker_frames=0 px_absorbed=0 absorb_refused=1 -> THROUGH
[  60478ms] [cursor11] compose-through scope=desk passes=110 bracketed=27 px_deferred=14976 px_installed=6336 px_redrawn=1728 flicker_frames=0 px_absorbed=0 absorb_refused=1 -> THROUGH
[  63963ms] :: PTRINSTALL: installs=96 reports=96 folds=0 lag_max_ms=7 coalesced=0 drains=96 ::
```

### Quarry, menubar, wire

```
[  41962ms] [clickroute] press at (1158,491) win=5 owner=0x2 was=1 -> raise+deliver deliver=2
[  41962ms] [clickroute] press at (1354,491) win=6 owner=0xffffff7f was=1 -> consume deliver=0
[  41962ms] [clickroute] press at (2,2) band=menu -> open deliver=0
[  41962ms] [clickroute] press at (1354,491) band=menu -> dismiss deliver=0
[  41962ms] [clickroute] route hit=true deliver=true depth=1/2 kernel=true desktop=false nofab=true deflect=true -> FAIL
[  26434ms] [wc-x] move-vacate win=2 scale=18x from=(8,8) to=(178,8) box=154x188 painted=false desktop=4/5 stale=0/5 -> FAIL
[  42513ms] [quarry] key_route key=0x0d focus=0 took=0
[ 252772ms] [quarry] key_route key=0x0d focus=1 took=1
[ 256841ms] [quarry] key_route key=0x0d focus=1 took=1
[ 375912ms] [quarry] key_route key=0x0d focus=0 took=0
[    177ms] :: [midden] cmd="write 0 0" -> Host verb=write ::
[ 143793ms] :: [midden] cmd="help" -> TerminalOutput len=3793 ::
[ 375931ms] :: [midden] cmd="helpdatehelp" -> TerminalError len=44 ::
[ 396397ms] :: [midden] cmd="pulse" -> Exec pulse.elf ::
[ 252771ms] :: FTDIRX: first byte rx=1 byte=0x68 'h' idle=14104 ::
```

### GMUX / iGPU ladder

```
[  26516ms] :: igpu-dpy: pre-switch state DDC=0x02 SW_DISP=0x03 SW_EXT=0x01 SW_EXT_ST=0x21 DISP=0x03 EXT=0x21 sw_ext_state=UNACCEPTED ext_state=kepler-owned gate=ACCEPT ::
[  26522ms] :: igpu: [GMUX] switched DISPLAY, EXTERNAL, and DDC to IGD (panel BLANKED/FLICKERED) ::
[  26524ms] :: igpu-dpy: LADDER highest=03/10 name=dpcd ok=0 pending=2 gmux=MATCH why=aux-timeout-error elapsed_ms=9 ::
```

### WiFi, CE, KFBIND, gen7

```
[  26892ms] :: wifi2: end ok=1 stage=d11 d11=FOUND wrote-cfg80=6(selftest=2 moves=3 restore=1) wrote-cfg0xac=0(moves=0 restore=0) wrote-wrapper=0(enable=0 unwind=0) wrote-core-regs=0(audited — MACCTL, SHM_CONTROL, SHM_DATA and RADIO_CONTROL have no write site in this file) uploaded-bytes=0(audited) restore=MATCH elapsed=0ms ::
[  26892ms] :: wifi2: upload REFUSED reason=wifi3-not-armed — the routing IS pinned (W5: SHM routing 0x0300 = microcode memory, control word 0x03000000; bcm-specs.sipsolutions.net/SHM/ + /MicrocodeUpload/, see bcm4331.md §S4-W5) but this build does not carry the upload rung. The prologue is DESTRUCTIVE (bcm4331.md §5 risk 4: the reset destroys the microcode this radio arrives running, psm-run 
[  26441ms] :: kepler: CE-LADDER end r1_ptop=partial r2_ce_probe=present r3_rlscan=no-data r5_inst=void r2b_arm=skipped — next gate: a genuine VRAM->VRAM copy (draft R7) is WITHHELD until r2b reports ARMED (a writable CE target), r1/r3 name a CE runlist id, and r5 yields an audited instance-block layout OR the CE falcon datapath map is derived ::
[  26496ms] :: KFBIND: begin knob=UNAOS_KEPLER_KFBIND rung=KF27 subject="the PBDMA register base every FIFO verdict is read through" writes=0 restored=n/a — READ-ONLY by construction; the bind/enable write this rung is named for is SKIPPED and its reason printed below ::
[  26497ms] :: KFBIND: ptop-row i=00..07 [8006183E 00000003 90828C3E 00000007 94A30E3E 0000000B 9C03AA3E 0000000F] ::
[  26497ms] :: KFBIND: ptop-row i=08..15 [8428A23E 00000023 8840023E 00000027 8C679E3E 0000002B 98C8243E 0000002F] ::
[  26497ms] :: KFBIND: ptop-row i=16..23 [00000000 00000000 00000000 00000000 00000000 00000000 00000000 00000000] ::
[  25885ms] :: gen7: r8 verdict=r8-fb-blit-verified by=mt mode=scratch-fill wake=gt-live-already engine=BCS r7=r7-blit-verified fb=panel rect=64x64 at_x=2816 at_y=0 pitch=16384 win_slots=265 attempts=1/3 any_ctl_enabled=1 best_ctl_readback=00000001 any_head_moved=1 any_sentinel=1 any_copy=1 best_rect_match=4096/4096 spill=0 battery_moved=0/17 fw_restored=1 fw_evidence=blind ring_regs_restored=1 al
[  26599ms] :: x86_64 PCI: Found network controller (class 0x02) vendor 0x14e4 at 3:0.0 ::
```

### SMC, BT, deadman

```
[  25732ms] :: SMC-BATT: AC-W is absent on this SMC (clean negative answer, not a fault) — AC presence is UNKNOWN; ac=derived:* is inferred from the B0AC sign, and the key is re-probed every 60000 ms == witness ::
[  25733ms] :: SMC-BATT: present=true soc=90% volt=11841mV amp=-2439mA full=9962mAh rem=8994mAh ac=derived:discharging retries=0/0 st0=0 rfail=0 rok=0 short=0 unc=0 gap=1145 busy=43 late=0 == witness ::
[  92663ms] :: SMC-BATT: present=true soc=89% volt=11823mV amp=-2183mA full=9962mAh rem=8940mAh ac=derived:discharging retries=0/0 st0=0 rfail=0 rok=0 short=0 unc=0 gap=1332 busy=43 late=0 == witness ::
[ 224285ms] :: SMC-BATT: present=true soc=88% volt=11776mV amp=-2224mA full=9962mAh rem=8841mAh ac=derived:discharging retries=0/0 st0=0 rfail=0 rok=0 short=0 unc=0 gap=1720 busy=43 late=0 == witness ::
[  25590ms] :: bt-c1: [1] page summary — attempts_run=2/2 pages_on_air=2 page_timeouts=2 page_timeout_each=5120ms conn_window_each=5600ms aligned_by_inquiry=false -> NOT REACHED, AND THE PEER NEVER ANSWERED ANY TRAIN — but read the inquiry summary above before reading that as the speaker's fault. Every attempt in the budget ended in an explicit Page Timeout for this BD_ADDR, each one a full-le
[   1116ms] [deadman] up=1 hid=0 pmp=0 hq=0 hid_ms=- comp_ms=- gate=- dec=00000000 dmg=0/0 in=0/0/0 rh=0
[   2116ms] [deadman] up=2 hid=0 pmp=0 hq=0 hid_ms=- comp_ms=- gate=- dec=00000000 dmg=0/0 in=0/0/0 rh=0
[   3116ms] [deadman] up=3 hid=0 pmp=0 hq=0 hid_ms=- comp_ms=- gate=- dec=00000000 dmg=0/0 in=0/0/0 rh=0
[   4116ms] [deadman] up=4 hid=0 pmp=0 hq=0 hid_ms=- comp_ms=- gate=- dec=00000000 dmg=0/0 in=0/0/0 rh=0
```

## 3. What flight 10 decided

- W5 is answered: the freeze at first window open and the dead cores are one software spin in `present_banded`, phase-independent (pw-strip, span-flush, pw-exit). The BAR1 aperture theory (flight 9) and the parked-store theory (SPANFLUSH) are both dead. Executor: W5SPIN.
- Six glass defects from one boot became six executors the same turn (QUARRYCLICK, MENUDROP, FTDICR→SERIALDOOR, BOOTFAILS, CARDROOT, plus the Quarry font carried in from FONTS2X), and three hardware rungs opened on the readings (WIFI4, GMUXDPCD, CTRLBIND). The fold shas are in `docs/dev/OS/rmbp-queue.md` STATE as they land.
- Flight 11 carries `UNAOS_WIFI3=1` (Peter's go, 2026-09-17) and the renumbered gmux ladder (`name=pp` at 3, compare on `name=`).

