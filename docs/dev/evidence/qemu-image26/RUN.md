# QEMUIMAGE26 — the image-26 tree booted once under KVM, the whole `tests` list typed over the door

R114 (docs/dev/RULINGS.md): the one-time exception to R78 for this arc. ONE QEMU/KVM boot. Branch `exec-rmbp-local-qemuimage26`, parent `62a3901a8`.

## Verdict

- Reached the desktop on a fresh store (the setter path). `tests list` printed nothing on the wire, so the names typed were the 224 `register("…")`/`defer("…")` names in `crates/kernel/src` (`play` skipped, as the brief says). 37 of those are not in this build (`ran=0`). The other **186 fixtures ran: 48 FAIL by the kernel's own tally**, the rest PASS or SKIP. The run finished: `LIST-END` at 01:23:21 local, +570 s of QEMU wall. It was not cut short.

- Verdict lines inside the fixture windows: 300, by word: PASS=177, SKIP=57, FAIL=54, PENDING=2, REFUSED=2, SEEN=2, MOUNTED=1, RESOLVED=1, OK=1, SCENE-RESTORE=1, VUG=1, NOT-READY=1.

- The ledger rows are B558 (`tests list` silent on the door) and B559..B607, one for each fixture that printed a `-> FAIL` (49 fixtures; `brightstep` printed a FAIL line, but the tally counted it under no fixture).

## The command line (exact)

```
#!/bin/bash
# QEMUIMAGE26 launcher: the bench's exact line, + the QEMU door plumbing (USBSERIAL, FTDIRX_INJECT, QMP, KVM).
set -u
W=/home/pmes/src/github.com/pmes/UnaOS-hw-pi4/.claude/worktrees/agent-a6af1cf8df5c43780
S=/tmp/claude-1000/-home-pmes-src-github-com-pmes-UnaOS/d109204f-6aa4-4cba-b358-efd3b8453b2d/scratchpad/q26
for v in $(compgen -e | awk '/^UNAOS_/'); do unset "$v"; done
cd "$W/unaos" || exit 1
# --- the bench's line, verbatim ---
export UNAOS_WIFI_FW_PATH=/home/pmes/src/github.com/pmes/UnaOS-bunker/drivers/broadcom-b43; set -a; . ./scripts/image-line.env; set +a; export UNAOS_AETHER=1 UNAOS_MIDDEN=1 UNAOS_UNA=1 UNAOS_SONDE=1
# --- QEMU-only plumbing (builder knobs, not kernel features) ---
SOCK="$W/unaos/target/f.sock"; PORT=4490   # a UNIX socket path must be < 108 bytes (the scratchpad path is not)
while lsof -iTCP:"$PORT" -sTCP:LISTEN >/dev/null 2>&1; do PORT=$((PORT+1)); done
export UNAOS_USBSERIAL=1 UNAOS_FTDIRX_INJECT="$SOCK" UNAOS_QEMU_FULL=1
export UNAOS_QEMU_EXTRA="-accel kvm -qmp tcp:127.0.0.1:${PORT},server,nowait"
compgen -e | awk '/^UNAOS_/' | sort > "$S/knobs.txt"
echo "knobs=$(wc -l < "$S/knobs.txt") port=$PORT sock=$SOCK"
rm -f target/sdcard.img target/serial.log
python3 "$S/drive.py" "$W/unaos" "$SOCK" "$PORT" "$W/unaos/target/serial.log" "$S" 1500 > "$S/drive.log" 2>&1 &
echo "drive_pid=$!" > "$S/pids"
date +%s > "$S/t_start"
./arroyo twoboot --boot 1 1560 > "$S/arroyo.out" 2>&1 &
echo "arroyo_pid=$!" >> "$S/pids"
wait
```

The bench's line, verbatim: `cd unaos && export UNAOS_WIFI_FW_PATH=/home/pmes/src/github.com/pmes/UnaOS-bunker/drivers/broadcom-b43; set -a; . ./scripts/image-line.env; set +a; export UNAOS_AETHER=1 UNAOS_MIDDEN=1 UNAOS_UNA=1 UNAOS_SONDE=1`.

**Knob count.** image-line.env @62a3901a8 carries 44 exports, plus the 4 on the bench line, which makes **48 line knobs**. QEMU-only plumbing adds 4: `UNAOS_USBSERIAL=1` and `UNAOS_FTDIRX_INJECT=<sock>` attach the emulated FT232 as a bidirectional socket, which is the door. `UNAOS_QEMU_FULL=1` turns off the spec's early exit. `UNAOS_QEMU_EXTRA=-accel kvm -qmp tcp:127.0.0.1:4490,server,nowait` gives KVM and the QMP typist for the first-user form. The environment held 52 `UNAOS_*` in all (list below). The arroyo verb is `twoboot --boot 1 1560`: that is test_x86_64 with the boot-1 spec and no SELFBUILD staging, and it is outside the battery-arming case, so `tests-at-boot`/`census` stay OFF. That is the metal shape: `:: TESTS: deferred=94 fire=tests at_boot=0 ::`.

The first-user form was typed over QMP exactly as `scripts/twoboot.sh` types it: `una`, Tab, `correct-horse`, Tab, `correct-horse`, Enter. Door lines were typed into the FTDI socket 40 ms per byte, ending in CR. The driver waited for each `[midden] cmd="tests <name>"` echo, then for that fixture's `:: TESTS: ran=` line, before it typed the next.

<details><summary>the UNAOS_* environment</summary>

```
UNAOS_AETHER
UNAOS_AHCI
UNAOS_BEAM
UNAOS_BT
UNAOS_BTC
UNAOS_BUSREG
UNAOS_DEADMAN
UNAOS_FACET
UNAOS_FTDIRX
UNAOS_FTDIRX_INJECT
UNAOS_GMUX_IGD
UNAOS_HDA
UNAOS_HDATONE
UNAOS_IOAPIC
UNAOS_IVB
UNAOS_IVB3D
UNAOS_IVB3D_BLIT
UNAOS_IVB3D_R8
UNAOS_KEPLER
UNAOS_KEPLER_CE
UNAOS_KEPLER_TAKEOVER
UNAOS_KEPLER_VBLANK
UNAOS_LIDSLEEP
UNAOS_LINUXABI
UNAOS_LOGIN
UNAOS_LOGTS
UNAOS_LUMEN
UNAOS_MIDDEN
UNAOS_NETRING3
UNAOS_NOASPM
UNAOS_QEMU_EXTRA
UNAOS_QEMU_FULL
UNAOS_QUARRY
UNAOS_SELFDIAG
UNAOS_SELFHOST
UNAOS_SMC
UNAOS_SONDE
UNAOS_UNA
UNAOS_UNAFS
UNAOS_USBNET
UNAOS_USBSERIAL
UNAOS_UVC
UNAOS_VIDEO
UNAOS_WC
UNAOS_WCDVALVE
UNAOS_WCG_PAYGO
UNAOS_WIFI
UNAOS_WIFI2
UNAOS_WIFI3
UNAOS_WIFI4
UNAOS_WIFI_FW_PATH
UNAOS_WITNESS
```
</details>

## Wall

- Build: about 12 min (697 s, launch to the refused QEMU start), in the refused first launch (below). The relaunch's build was a cargo no-op: 8 s from the launch to QEMU's socket accepting.
- QEMU: started 01:13:50 local and was stopped by pid at 01:23:36. **QEMU wall 585 s, against a 1500 s cap.**
- An earlier launch attempt never started a guest. QEMU refused the FTDI socket path because the scratchpad path is longer than the 108-byte UNIX socket limit (`Path must be less than 108 bytes`). That was a build plus a refused QEMU start, with no boot. The socket moved to `unaos/target/f.sock`.
- The first driver (`drive.py`) exited when `tests list` printed no tail. Its exit closed the socket, which hot-unplugs the FT232. `drive2.py` reconnected (re-plugged), and the door answered again: `tests list` echoed, still with no output. It then typed the source's names.

## `:: BOOT:`

```
[07:13:52Z] :: BOOT: firmware->loader=2014ms loader->desktop=4232ms total=6247ms lines=152 steps=2 slowest=users-load:23 over=0 ::
```

The desktop was reached at +16 s of QEMU wall (form open at +7 s, typed at +14 s, `[boot] services up` at +16 s).

## B558: `tests list` is silent on the headless door

Both `tests list` lines echoed and then printed nothing: no fixture rows and no `N deferred, M ran at boot` tail. No `[serialdoor] line verb=` line appears anywhere in the run. `tests door` reads `lines=0`, so serialdoor::key's Enter arm never counted a line, and the door's lines reach `[midden]` by another route. A routed verb's console output (`tests list` is console-only; a fixture's own `serial_println!` lines do reach the wire) never comes back to the door.

```
[07:13:52Z] :: [midden] cmd="tests list" -> Host verb=tests ::
[07:14:21Z] :: [midden] cmd="tests list" -> Host verb=tests ::
```

## Every `:: NAME: … -> VERDICT` line, by fixture (verbatim)

```

## tests aether
[07:14:21Z] :: AETHER: page /system/doc/welcome.html -> PASS painted=15902 lines=57 blocks=21 elements=43 rules=15 doc_h=949 ms=20 start_ms=49 heap_kib=4335 ::
[07:14:21Z] :: AETHER: focus -> PASS href=keys.html at=48,55,138 ::
[07:14:21Z] :: AETHER: hover -> PASS href=keys.html ::
[07:14:21Z] :: AETHER: select -> PASS chars=12 ::
[07:14:21Z] :: AETHER: clip -> PASS set=12 chars=12 ::
[07:14:21Z] :: AETHER: nav -> PASS how=click path=/system/doc/keys.html lines=37 hist=2/2 ::
[07:14:21Z] :: AETHERAPP: image=/apps/AETHER.ELF window=elf painted=15902 lines=57 hover=1 nav=1 select=12 -> PASS ::
## tests appearance
[07:14:21Z] :: APPEARANCE: literals_outside_theme=0 tokens=34 sets=2 mode=light accent=crispy repaint_ms=3 -> PASS :: flipped=1 tokens_ok=1 restored=1 switches=2
## tests appmenu
[07:14:21Z] :: APPMENU2: default_set=5 chords=[Q,W,H,M,comma,optesc] crystal=[lock,forcequit] publishers=unbounded -> PASS ::
## tests appres
[07:14:21Z] :: APPRES: programs=26 with_res=15 icons_drawn=26 cached_attrs=0 -> PASS :: lumen=staged root_attrs=false sights=13 decodes=32
## tests appswitch
[07:14:21Z] :: APPSWITCH: apps=0 order=mru strip=skip-oneapp activate=skip-oneapp -> PASS ::
## tests apptrust
[07:14:21Z] :: APPTRUST: foreign=listed registrant=refused asked=once copied=skip rebind=asked icon=root launch=asked copy=REFUSED -> FAIL :: classify=true volume=apptrust-probe planted=no asks=4 grants=0 copy_admin=user:una anon=ADMITTED
## tests assocstamp
[07:14:21Z] :: ASSOCSTAMP: stamp=none blocks_read=0 ms=0 -> SKIP reason=no-attr-root ::
## tests attr
[07:14:21Z] :: ATTRSURF: set=0 get=0 list=0 query=0 denied=0 reason=no-unafs-volume -> SKIP ::
## tests attrcolumns
[07:14:21Z] :: ATTRCOLUMNS: typed_facts=0/24 -> SKIP :: reason=no-test-f ::
## tests bandy3
[07:14:21Z] :: BANDY3: registered=2 relayed=2 replied=1 orphan=1 -> PASS ::
## tests battery
[07:14:21Z] :: BATTERY: live=ok watts_src=none strip_kept=ok -> PASS ::
## tests bezel
[07:14:21Z] :: BRIGHTKEYS: key=up level=13/16 gmux_written=0 indicator=1 -> PASS ::
[07:14:21Z] :: BEZEL: brightness=ok volume=ok mute=ok fade_ms=1500 scale=shared -> PASS ::
## tests blitter
[07:14:21Z] :: KCOMP: blitter=cpu census=10 hot=window-present cpu_us=104 gpu_us=- gpu=unavailable fallback_ok=1 -> PASS ::
## tests blitwire
[07:14:21Z] :: BLITWIRE: loop_us=378852 bytes=12214800 raw_bus=32 net_us_meas=281872 net_bus_meas=43 net_us_full=135519 net_bus_full=90 floor=64 bound=001 arm=010 -> PASS ::
## tests boot80
[07:14:21Z] :: BOOT80: mount_ms=9 mount_blocks=8 mount_cmds=1 blocks_read=9 cmds=4 ms=20 users_store=dat stage=desktop types=0 ra_window=0 store_blocks=1 store_bound=64 types_blocks=0 bound=6 from=walk3x2xra1 foreign_wr=0 types_from=stamp stamp=none -> FAIL ::
## tests bootfat
[07:14:21Z] :: BOOTFAT: write_as_root=- unlock_path=- -> SKIP :: reason=fat-root (no UnaFS root: the FAT is /) ::
## tests bootfatseam
[07:14:21Z] :: BOOTFATSEAM: direct_writers=- users=- holocron=- lfn=- fat_unlock=- -> SKIP :: reason=fat-root (no UnaFS root: the FAT is /) ::
## tests botpark
[07:14:21Z] :: BOT-PARK: selftest ladder=true reenum=true surrender=true cycles=true dead=true success=true reprobe=true backoff=true unplug=true pressure=true place=true ladder_max=6 surrender_max=2 cycle_max_ms=45000 dead_max=8 reprobe_ms=60000 pass_pump_ms=2000 slots=4 -> PASS ::
## tests brightfloor
[07:14:21Z] :: BRIGHTKEYS: key=up level=13/16 gmux_written=0 indicator=1 -> PASS ::
[07:14:21Z] :: BRIGHTKEYS: key=down level=12/16 gmux_written=0 indicator=1 -> PASS ::
[07:14:21Z] :: BRIGHTSTEP: raws=1024 down_ok=1 up_ok=1 flight22=1 seeded=1 -> PASS ::
[07:14:21Z] :: BRIGHTFLOOR: floor=1 set0_on=0 load_clamped=1 reset_ok=1 full=0 mono=1 driver=gmux -> FAIL ::
## tests brightstep
[07:14:21Z] :: BRIGHTSTEP: steps=5 readback_ok=0 slider_sync=bad keys=ok driver=gmux -> FAIL ::
## tests bt
[07:14:21Z] :: BTHID: fixture inquiry-rssi addr/class/psrm/clock/rssi of a synthetic 0x22 event -> PASS ::
[07:14:21Z] :: BTHID: fixture eir-name complete name preferred over shortened -> PASS ::
[07:14:21Z] :: BTHID: fixture sdp-descriptor attribute 0x0206 -> 101 descriptor bytes -> PASS ::
[07:14:21Z] :: BTHID: fixture descriptor-map ptr rid=2 x=8/8 y=16 btn=3@0 wheel=24/8 kbd=Some(1) boot=true decode=(5,-5,1,-1) -> PASS ::
[07:14:21Z] :: BTHID: fixture l2cap-config mtu=48 parsed; streaming mode flagged; hint-bit option skipped -> FAIL ::
[07:14:21Z] :: BTHID: inquiry=0 paired=0 connected=0 hid_reports=0 -> FAIL ::
## tests btkeyseal
[07:14:21Z] :: BTKEYSEAL: sealed=n/a plain_left=0 objects=0 pending=0 unsealed=0 holocron=no-holocron codec=ok -> SKIP ::
## tests canonguard
[07:14:21Z] :: U2-0c: canonical-rcx guard refuses 0x8000_0000_0000_0000 -> PASS ::
## tests clock
[07:14:21Z] :: CLOCKCORE: source=apic tsc_ms_vs_hpet_ms=0 ref=pm window_ms=250 masked_clock_ms=20/20 masked_max_ms=0 masked_sites=0 cores_ticking=6/6 skew_max_cycles=0 -> FAIL ::
## tests columnsview
[07:14:21Z] :: COLUMNSVIEW: panes=0 depth=0 preview=skip keys=skip -> SKIP :: reason=no-test-f ::
## tests cursorshape
[07:14:21Z] :: CURSORSHAPE: set=hand index=0->1 einval=-22 focus_away=arrow rows=8 distinct=yes -> PASS ::
## tests dimidle
[07:14:21Z] :: DIMIDLE: idle_min=10 blanked_at_ms=271968 woke_at_ms=0 wake_key_swallowed=0 -> FAIL ::
[07:14:21Z] :: TESTS: dimidle -> SKIP reason=no-verdict ::
## tests dns
[07:14:21Z] :: TESTS: dns -> SKIP reason=no-verdict ::
## tests dockgone
[07:14:21Z] :: DOCKGONE: vacates=0 paints=0 present=1 ends=2 ends_px=35620 -> PASS ::
## tests dockpins
[07:14:21Z] :: DOCKPINS: pins=4 direct=4 landed=2 shell_windows=0->0 shell_posted=0 verb_posted=0 -> FAIL ::
## tests door
[07:14:21Z] :: DOOR: headless=0 ran=0 window=0 lines=0 -> FAIL ::
## tests dragdrop
[07:14:21Z] :: DRAGDROP: fixture=/home/una/DragDropTest -> SKIP :: reason=no-home-folder ::
## tests dragdrop2
[07:14:21Z] :: DRAGDROP2: fixture=/home/una/DragDrop2Test -> SKIP :: reason=no-home-folder ::
## tests ehci
[07:14:21Z] :: TPDRAG: decoded=true mover_seq=0110 buttons=0111 d1=-21/0 d3=6/0 -> FAIL ::
[07:14:21Z] :: TPFRAME: frames=7 fingers_max=1 deltas_ok=true click_edges=down@4,up@5 corpus=3 d=-7/-3,-1/0 lift_reset=true relx10=2/2 wit_1in64=true legacy_ok=true mismatch_yes=true mismatch_no=true -> PASS ::
[07:14:21Z] :: KEYMAP: table=crispy resolved=11 screenshot=ok region=ok copy=ok paste=ok cut=ok selectall=ok ctrl_c_ascii=0x03 ctrl_c_action=none pc_table_alt_c=copy prtsc=ok extramods=ok logout=ok rows=63/37 -> PASS ::
[07:14:21Z] :: APPCLIP: delivered=4 copy=ok paste=ok len=10 line_match=true cut=empty selectall=ok epoch_clear=ok -> PASS ::
[07:14:21Z] :: TERMSEL: resolved=13/13 delivered=13 left=ok copy_sel=ok home=ok esc=ok copy_line=ok cut=ok cut_empty=ok edit=ok pc=ok -> PASS ::
[07:14:21Z] :: KEYREPEAT: delay_ms=400 rate_hz=25 first_repeat_ms=400 repeats=8 cancelled=1 -> PASS ::
## tests ehciisr
[07:14:21Z] :: EHCI-HID: ISRARM self-test: modes=2 (overlay-direct + qTD-chain) depth=64 fifo=true payload=true rearm=true toggle=true ringfull-refuses=true -> PASS == witness ::
## tests elfbss
[07:14:21Z] :: STORMFAULT: elfbss bss=65536 exit=0 seg_last=bss refuse_window=1 refuse_stack=1 -> PASS ::
## tests exec
[07:14:21Z] :: EXECNAME: resolved=6/6 windowed=1 console=5 -> PASS ::
## tests exfat
[07:14:21Z] :: EXFAT: kat=0/0 mounted=- files=0 -> SKIP :: reason=no-exfat-medium ::
## tests facetanim
[07:14:21Z] :: FACETANIM: gif=absent webp=absent apng=absent frames_held=0 -> SKIP ::
## tests fatverb
[07:14:21Z] :: TESTS: fatverb -> SKIP reason=no-verdict ::
## tests filetype
[07:14:21Z] :: FILETYPE: typed=0 sniffed=0 ext=0 assoc=- override=- quarry=- dir=/ reason=create FTPIC.DAT -> FAIL ::
## tests filetypes
[07:14:21Z] :: FILETYPES: types=30 build_ms=0 blocks=0 masked_max_ms=0 -> FAIL :: login=none worker=failed on=worker login_ms=0 login_blocks=0 bound_ms=20000 masked_bound_ms=50 bar_ms=425 preferred=0 resolved=ok registry=none objects=0 source=registrants sticky=skip amend=skip openwith=2 builds=2
## tests finemotion
[07:14:21Z] :: FINEMOTION: one_count_moved=200/200 ten_count_moved=20/20 floor=none stall_s=0 iso=ok jump=clamped retouch=rebased max_step=999 div=5.55u/px range=7994 -> PASS ::
## tests firstuser
[07:14:21Z] :: FIRSTUSER: flow=mac first_user=una role=admin root=locked prompts=1 login_window=users migrated=0 at=tests -> FAIL ::
## tests flightring
[07:14:21Z] :: FLIGHTRING: rolling_kib=3617 pinned_kib=64 tail_live=1 prefill_lines=16 wrapped=1 live_wrapped=0 test_lines=49797 -> PASS ::
## tests folderview
[07:14:21Z] :: FOLDERVIEW: codec=ok fat_writable=0 -> SKIP :: reason=no-unafs-test-folder ::
## tests font
[07:14:21Z] :: KERNELFONT: faces=10 cache_kib=2048 evictions=0 fallback=none glyphs_drawn=6307 ms_per_1000=7 -> PASS ::
## tests gen7
[07:14:21Z] :: GEN7LADDER: rungs=none ggtt=none wake=none r7=not-reached us=0 replay=0 -> SKIP (no boot bank: igpu::init never reached the IGD BAR0 on this boot) ::
## tests gpublit
[07:14:21Z] :: GPUBLIT-TEST: rerun=refused boot=no-takeover -> FAIL ::
## tests hda
[07:14:21Z] :: HDA-PCM: amp=4096 default=4096 peak=4095 min=-4096 q27=115 interleave=1 peak_ok=1 sine_ok=1 le_ok=1 frames=48000 hz=440 secs=1 fade_ms=20 edge0=0 edge_last=0 -> PASS ::
[07:14:21Z] :: HDA-TONE: lpib_advanced=1 walked=1 wraps=1 bcis=2 tag_ok=1 fifo_ready=1 run_ms=1200 members=1 -> PASS :: amp=4096 :: run_bit=1 run_readback_ok=1 dac_pwr=D3 settled_ms=0 gpio=0x00 hp_gpio=0 spk_gpio=0 pin_pwr=[D?,D-] run_reasserts=0 stall_reasserts=0 sdfmt=0x0011 chan=[0] eapd=[0] vendor=1af4:0022 ::
[07:14:21Z] :: HDA: runs=1 fields_stable=1 dac_fmt_match=1 tag_match=1 lpib_moved=1 -> PASS :: run=3 sdfmt=0x0011 srst=1/1 stop=1 ::
[07:14:21Z] :: AUDIO8: runs=1 plays=0 amp_up=1 amp_down=0 pops_bracketed=0 -> PENDING :: boot_up=3 boot_down=2 warm=0 holdoff_ms=5000 ramps=3/3 ::
## tests hda1
[07:14:21Z] :: HDA-PCM: amp=4096 default=4096 peak=4095 min=-4096 q27=115 interleave=1 peak_ok=1 sine_ok=1 le_ok=1 frames=48000 hz=440 secs=1 fade_ms=20 edge0=0 edge_last=0 -> PASS ::
[07:14:21Z] :: HDA-TONE: lpib_advanced=1 walked=1 wraps=2 bcis=2 tag_ok=1 fifo_ready=1 run_ms=1200 members=1 -> PASS :: amp=4096 :: run_bit=1 run_readback_ok=1 dac_pwr=D3 settled_ms=0 gpio=0x00 hp_gpio=0 spk_gpio=0 pin_pwr=[D?,D-] run_reasserts=0 stall_reasserts=0 sdfmt=0x0011 chan=[0] eapd=[0] vendor=1af4:0022 ::
[07:14:21Z] :: HDA: runs=2 fields_stable=1 dac_fmt_match=1 tag_match=1 lpib_moved=1 -> PASS :: run=4 sdfmt=0x0011 srst=1/1 stop=1 ::
[07:14:21Z] :: AUDIO8: runs=2 plays=0 amp_up=1 amp_down=0 pops_bracketed=0 -> PENDING :: boot_up=3 boot_down=2 warm=1 holdoff_ms=5000 ramps=4/4 ::
## tests hda2
[07:14:21Z] :: HDA-PCM: amp=4096 default=4096 peak=4095 min=-4096 q27=115 interleave=1 peak_ok=1 sine_ok=1 le_ok=1 frames=48000 hz=440 secs=1 fade_ms=20 edge0=0 edge_last=0 -> PASS ::
[07:14:21Z] :: HDA-TONE: lpib_advanced=1 walked=1 wraps=2 bcis=2 tag_ok=1 fifo_ready=1 run_ms=1200 members=1 -> PASS :: amp=4096 :: run_bit=1 run_readback_ok=1 dac_pwr=D3 settled_ms=0 gpio=0x00 hp_gpio=0 spk_gpio=0 pin_pwr=[D?,D-] run_reasserts=0 stall_reasserts=0 sdfmt=0x0011 chan=[0] eapd=[0] vendor=1af4:0022 ::
[07:14:21Z] :: HDA: runs=3 fields_stable=1 dac_fmt_match=1 tag_match=1 lpib_moved=1 -> PASS :: run=5 sdfmt=0x0011 srst=1/1 stop=1 ::
[07:14:21Z] :: AUDIO8: runs=3 plays=0 amp_up=1 amp_down=0 pops_bracketed=0 -> PASS :: boot_up=3 boot_down=2 warm=2 holdoff_ms=5000 ramps=5/5 ::
## tests hda220
[07:14:21Z] :: HDA-PCM: amp=4096 default=4096 peak=4095 min=-4096 q27=230 interleave=1 peak_ok=1 sine_ok=1 le_ok=1 frames=48000 hz=220 secs=1 fade_ms=20 edge0=0 edge_last=0 -> PASS ::
[07:14:21Z] :: HDA-TONE: lpib_advanced=1 walked=1 wraps=2 bcis=2 tag_ok=1 fifo_ready=1 run_ms=1200 members=1 -> PASS :: amp=4096 :: run_bit=1 run_readback_ok=1 dac_pwr=D3 settled_ms=0 gpio=0x00 hp_gpio=0 spk_gpio=0 pin_pwr=[D?,D-] run_reasserts=0 stall_reasserts=0 sdfmt=0x0011 chan=[0] eapd=[0] vendor=1af4:0022 ::
[07:14:21Z] :: HDA: runs=4 fields_stable=1 dac_fmt_match=1 tag_match=1 lpib_moved=1 -> PASS :: run=6 sdfmt=0x0011 srst=1/1 stop=1 ::
[07:14:21Z] :: AUDIO8: runs=4 plays=0 amp_up=1 amp_down=0 pops_bracketed=0 -> PASS :: boot_up=3 boot_down=2 warm=3 holdoff_ms=5000 ramps=6/6 ::
## tests hda880
[07:14:21Z] :: HDA-PCM: amp=4096 default=4096 peak=4095 min=-4096 q27=55 interleave=1 peak_ok=1 sine_ok=1 le_ok=1 frames=48000 hz=880 secs=1 fade_ms=20 edge0=0 edge_last=0 -> PASS ::
[07:14:21Z] :: HDA-TONE: lpib_advanced=1 walked=1 wraps=2 bcis=2 tag_ok=1 fifo_ready=1 run_ms=1200 members=1 -> PASS :: amp=4096 :: run_bit=1 run_readback_ok=1 dac_pwr=D3 settled_ms=0 gpio=0x00 hp_gpio=0 spk_gpio=0 pin_pwr=[D?,D-] run_reasserts=0 stall_reasserts=0 sdfmt=0x0011 chan=[0] eapd=[0] vendor=1af4:0022 ::
[07:14:21Z] :: HDA: runs=5 fields_stable=1 dac_fmt_match=1 tag_match=1 lpib_moved=1 -> PASS :: run=7 sdfmt=0x0011 srst=1/1 stop=1 ::
[07:14:21Z] :: AUDIO8: runs=5 plays=0 amp_up=1 amp_down=0 pops_bracketed=0 -> PASS :: boot_up=3 boot_down=2 warm=4 holdoff_ms=5000 ramps=7/7 ::
## tests hdaboth
[07:14:21Z] :: HDA-PCM: amp=4096 default=4096 peak=4095 min=-4096 q27=115 interleave=1 peak_ok=1 sine_ok=1 le_ok=1 frames=48000 hz=440 secs=1 fade_ms=20 edge0=0 edge_last=0 -> PASS ::
[07:14:21Z] :: HDA-TONE: lpib_advanced=1 walked=1 wraps=2 bcis=2 tag_ok=1 fifo_ready=1 run_ms=1200 members=1 -> PASS :: amp=4096 :: run_bit=1 run_readback_ok=1 dac_pwr=D3 settled_ms=0 gpio=0x00 hp_gpio=0 spk_gpio=0 pin_pwr=[D?,D-] run_reasserts=0 stall_reasserts=0 sdfmt=0x0011 chan=[0] eapd=[0] vendor=1af4:0022 ::
[07:14:21Z] :: HDA: runs=6 fields_stable=1 dac_fmt_match=1 tag_match=1 lpib_moved=1 -> PASS :: run=8 sdfmt=0x0011 srst=1/1 stop=1 ::
[07:14:21Z] :: AUDIO8: runs=6 plays=0 amp_up=1 amp_down=0 pops_bracketed=0 -> PASS :: boot_up=3 boot_down=2 warm=5 holdoff_ms=5000 ramps=8/8 ::
## tests helpdoc
[07:14:21Z] :: HELPVERB: verbs=109 documented=108 missing=[wifi] groups=8 -> FAIL ::
## tests hidstall
[07:14:21Z] :: HIDSTALL: halted_at_login=0 recovered=0 proxy_retired=0 hid_stall_s=0 key_queue_max_ms=2542 dock_timeouts=0 masked_max_ms=9 -> FAIL :: hid_gap_bound_ms=16 key_bound_ms=50 release_timeout_ms=400 holders=3 hold_worst_ms=11 hold_worst=? releases_waited=0 lane_max_ms=0 pump_cpu=5 held_stall_s=0 stall_gap_max_ms=0 dock_held=0 timeout_release_ms=0 dock_undecoded=0 fr=worker:c2 fr_worst_ms=282
## tests holocron
[07:14:21Z] :: HOLOCRON: ring=none root=none verbs=0 owner=una#16297596 put=ok get=ok denied=1 corrupt=1 -> PASS ::
## tests homesoil
[07:14:21Z] :: TESTS: homesoil -> SKIP reason=no-verdict ::
## tests imgview
[07:14:21Z] :: TESTS: imgview -> SKIP reason=no-verdict ::
## tests inputstall
[07:14:21Z] :: INPUTSTALL: key_queue_max_ms=1520 comp_max_ms=0 hid_gap_max_ms=4 strand_pct=0 bound=50 -> FAIL :: events=463 frames=0 strands=0 strand_bound_pct=1 span=45s via=tests
## tests installbare
[07:14:21Z] :: INSTALLBARE: phase=setter services=0 windows=0 valve=none first_key_ms=0 key_max_ms=3 keys=32 hid_gap_max_ms=12 held=7 -> PASS ::
## tests jobscan
[07:14:21Z] :: JOBSCAN: on=worker chunks=0 bar_ms=425 max_chunk_ms=0 pages=0 -> FAIL :: state=landed bound_ms=50 fallbacks=0
## tests jobsui
[07:14:21Z] :: JOBSUI: counts=ok pending_said=1 items_shown=0 -> PASS :: state=landed live_pending=0 verb_items=0 read=0 first=-
## tests kvblank8
[07:14:21Z] :: KVBLANK8: lost_at=- isr_calls=0 eoi=- rearm=- -> SKIP (no GK107 head armed: bar0=0 bdf=0 head=0) ::
## tests ladder
[07:14:21Z] :: U7x: cross-process transfer — SYS_XFER attenuated, child received + used the cap, revoke enforced, single-writer intact -> PASS ::
[07:14:21Z] :: U8x: revocation trees — parent revoke kills re-grant + re-transfer, generation-tagged inbox, ledger clean -> PASS ::
[07:14:21Z] :: U9x: real File writes — open-RW+seek+write+readback OK, RO-write/wrong-kind/revoked-cap all -EACCES, staged write FLUSHED to FAT (on-disk sector changed + size unchanged) -> PASS ::
[07:14:21Z] :: U10: file growth — open-RW+grow-write+readback OK, original cluster intact, RO-write -EACCES, staged grow FLUSHED to FAT (on-disk size grew + appended data present + FAT copies consistent) -> PASS ::
[07:14:21Z] :: U10c: file create — O_CREAT|RW+write+readback+idempotent-reopen OK, CREATED on FAT (on-disk entry present + content + exactly one dir entry, no duplicate) -> PASS ::
[07:14:21Z] :: U10d: file delete — create+write+unlink OK, sibling read -EACCES, re-open -ENOENT, DELETED on FAT (dir gone + chain freed in all FAT copies + cluster re-allocatable) -> PASS ::
[07:14:21Z] :: U11x: open-file lifecycle — open+read/close/double-close(-EBADF)/use-after-close(-EACCES)/reopen+read OK, gen-tagged file-id rejects a stale sibling to a reused slot -> PASS ::
[07:14:21Z] :: CFU2-WGATE: kernel->user write validated against LIVE leaf W — RO+X page (U3 ELF shape, the VUG/PULSE bypass) REFUSED -EFAULT, RW page ACCEPTED, RW->RO straddle REFUSED while its in-page head is accepted, RO page still readable -> PASS ::
[07:14:21Z] :: U11m2: cross-process unlink-defers-free — open-across-rows + read-after-unlink OK, delete op HELD past unlink, released at last CLOSE and at TEARDOWN, re-create -EBUSY while pending, DELETED on FAT (gone + chain freed all copies + re-allocatable + name re-creatable) -> PASS ::
[      ?ms] :: U6gx: UnaFS owner/grants — non-owner open -EACCES, owner grant admits R|W (content crossed), non-owner grant -EACCES, grantee unlink -EACCES (delete owner-only), revoke re-denies, owner unlink defers while grantee holds + re-create -EBUSY, ownership dies at last close -> PASS ::
[07:14:21Z] :: BUSX86-EQ: ring-3 midden (busx86-midden) — denied via the bus == denied via the DIRECT syscall, byte-same errno; permitted via one == permitted via the other: legs=9 same=9 diff=0 [ls-own:ok/ok ls-other:ok/ok ls-none:-ENOENT/-ENOENT cat-own:ok/ok cat-other:-EACCES/-EACCES cat-none:-ENOENT/-ENOENT cp-own:-ENOENT/-ENOENT cp-other:-EACCES/-EACCES cp-none:-ENOENT/-ENOENT] payload cat=same cp-own-content=n/a-source-removed-by-this-fixture's-own-rm refused-frame-queued-nothing=true write/rm/mv=-EBUSY/ok/-ENOENT (BANDY-2 write side FULFILLED by STOR-1 M3 — it answered -ENOSYS x3 here until that arc; this fixture's own rm is what takes its cp-own source away, and the cp payload claim now lives in BUSX86-WR bit10) badframe=-EINVAL identity=(row,SLOT_GEN)=(1,9) seq=true sealed=true cleanup=true unlinked=true cleared=true drained=true ops_replayed=1 -> PASS ::
[07:14:21Z] :: BANDY-ATTR: attribute verbs on the wire (set/get/list/query/stat request goldens 11..=15, typed bodies, fail-closed) -> PASS [w=0x3f] ::
[07:14:21Z] :: APPMENU: verb=publish owner=7 items=3 depth=2 bar=0/0 -> PASS ::
[07:14:21Z] :: APPMENU: owner=7 items=3 published=1 -> PASS ::
[07:14:21Z] :: APPMENU: readback owner=7 items=3 match=true -> PASS ::
[07:14:21Z] :: APPMENU: verb=publish owner=7 reason=items-cap -> REFUSED ::
[07:14:21Z] :: APPMENU: refuse items=65 principal=caller registry_unchanged=true -> PASS ::
[07:14:21Z] :: APPMENU: owner=7 items=3 pick_to=owner others_quiet=true -> PASS ::
[07:14:21Z] :: APPMENU: owner=7 closed reaped=true -> PASS ::
[07:14:21Z] :: APPMENU: owner=7 reaped empty=true -> PASS ::
## tests launcher
[07:14:21Z] :: LAUNCHER: programs=1 files=0 settings=1 math=ok open=ok recency=via-prefs ms=187 -> PASS ::
## tests lidsleep
[07:14:21Z] :: LIDSLEEP: msld=no-key raw=- flips=0 backlight_off=fail prior=767 off_rb=- restore_rb=- armed=0 -> FAIL ::
## tests linuxabi
[07:14:21Z] :: LINUXABI: path=/apps/HELLO.LNX exit=0 syscalls=2 enosys=[] ms=56 -> PASS ::
[07:14:21Z] :: LINUXABI-KAT: path=/apps/SYSKAT.LNX checks=0 fail=none exit=? syscalls=0 enosys=[] -> SKIP reason=fixture-not-staged ::
## tests linuxabi2
[07:14:21Z] :: LINUXABI2: fork_ok=1 pipe_ok=1 dents=12 stdin=1 -> PASS ::
## tests linuxabi3
[07:14:21Z] :: LINUXABI3: cr4=osfxsr save=fx sse_lnx=ok fork_fp=ok busybox=skip -> PASS ::
## tests loader
[07:14:21Z] :: LOADER: stages=7 slowest=gop:74ms timeouts=0 -> PASS ::
## tests lockreg
[07:14:21Z] :: LOCKREG: locks=192 wrapped=262290095/262316547 held_max_ms=357 inversions=4 released_on_death=ok named=ok -> FAIL ::
## tests loginfurn
[07:14:21Z] :: LOGINFURN: at_login windows=0 login_items=0 services=0 console_prefill_lines=none ring_tail_lines=48 -> PASS :: console=unopened (ring_tail_lines = what the console's prefill replays when it opens: the current boot's tail in the one ring)
## tests loginrender
[07:14:21Z] :: LOGINRENDER: handler_max_ms=271 zero_pass_max_ms=271 worst_what=quarry.settings:271 moved=wallpaper-probe:10,prefs-watch:167 -> FAIL :: bound_ms=100 anchor=login-ok window=closed window_s=30 turns=130 zero_pass_turns=130
## tests logwit
[07:14:21Z] :: LOGWIT-1: no marker came back prefixed from the FTDI capture in 4 attempt(s) (bare=0 absent=4 ring-unreadable=0) — the CLOCK-2b tap prefix is NOT on the bench's evidence stream -> FAIL ::
## tests lumen
[07:14:21Z] :: LUMENAPP: image=/apps/LUMEN.ELF window=elf provider=echo key=none -> PASS ::
[07:14:21Z] :: LUMENCRASH: spawned=1 first_line=21 -> PASS :: bound_ms=20000
[07:14:21Z] :: LUMENUX: md=ok history=ok clip=ok scroll=131072 -> PASS font=dejavu-sans ::
## tests lumenfast
[07:14:21Z] :: LUMENAPP: image=/apps/LUMEN.ELF window=elf provider=echo key=none -> PASS ::
[07:14:21Z] :: LUMENFAST: cold_sight_ms=1 cold_first_line=42 sight_ms=2 spawn_ms=22 first_line=20 font_served_kib=1769 -> PASS :: bound_ms=1000
## tests menubar
[07:14:21Z] :: MENUBAR: items=6 overlaps=0 w=1280 x=input:987,volume:1017,net:1047,battery:1095,notify:1175,clock:1223 -> PASS ::
## tests metrics
[07:14:21Z] :: UIMETRICS: ppi=0 scale=1.0 bar=34 title=34 cell=7x16 magnified=0 consts=0 -> PASS ::
## tests midden
[07:14:21Z] :: MIDDENAPP: image=/apps/MIDDEN.ELF window=bad lines=0 plan=none -> SKIP ::
## tests nameindex
[07:14:21Z] :: NAMEINDEX: names=0 prefix_ms=0 src=none -> SKIP :: reason=root-not-unafs ::
## tests net
[07:14:21Z] :: ENTROPY: source=jitter bytes=8192 windows_distinct=true ones=16334+16311 short=true ok=1 -> PASS ::
[07:14:21Z] :: RESOLVE: name=api.anthropic.com einval=1 -> SKIP reason=no-answer ::
## tests netclock
[07:14:21Z] :: NETCLOCK: polls_per_s=0 tx_per_s=0 loan_held_max_us=0 rx_ok=6 -> SKIP reason=e1000-link ::
## tests netfetch
[07:14:21Z] :: NETFETCH-PARSE: url=true basename=true request=true head=true -> PASS ::
## tests nethang
[07:14:21Z] :: NETHANG: path=resolve held_locks=1 bounded=1 link=1 masked=1 ms=1999 masked_hlt=3 capped=2 rc=no-answer -> FAIL ::
## tests notice
[07:14:21Z] :: NOTICE: typed_through=ok modal=none flash=none -> PASS ::
[07:14:21Z] :: DIALOG: anatomy=ok default=right esc=cancel modal=app focus_theft=0 confirm=ok toast=ok -> PASS ::
[07:14:21Z] :: DIALOG2: alert_window=deleted errors_to_dialog=3 info_to_toast=4 login_power_row=1 bus_verbs=3 origin=explicit -> PASS ::
## tests notify
[07:14:21Z] :: NOTIFY: stack_max=4 center=ok ring=100 badge=ok dnd=0 posted=2 -> FAIL :: press=ok action=ok expire=ok dnd_collect=FAIL focus=kept dropped=0
## tests notifypane
[07:14:21Z] :: NOTIFYPANE: apps=2 allow=2 sound=ok dnd_window=0-0 -> PASS :: block=ok center=ok window=ok sound_gate=ok dnd_now=0
## tests openertrust
[07:14:21Z] :: OPENERTRUST: registrant=skip foreign_path=skip carried=skip -> PASS :: kept=skip foreign=none (the root takes no attributes: no file can carry una:preferred)
## tests panicscreen
[07:14:21Z] :: PANICSCREEN: drawn=1 restored=1 log_path=/var/log/panic-0.txt writable=0 hold=0 sealed=0 -> PASS ::
## tests passperiod
[07:14:21Z] :: EHCI-HID: PASSPERIOD self-test: samples=100 backwards-delta-refused=true tick=1000us stall=40000us -> pass_period_us_max=40000 pass_period_us_mean=1390 -> PASS == witness ::
## tests player
[07:14:21Z] :: PLAYER: reason=no-TEST.WAV-and-no-writable-path -> FAIL ::
## tests playwav
[07:14:21Z] :: PLAYWAV: path=- reason=no-writable-path -> FAIL ::
## tests pointerfeel
[07:14:21Z] :: POINTERFEEL: armed window_ms=5000 on=task cpu=2 via=tests -> SKIP reason=verdict-follows :: (move one finger on the trackpad for 5 s; the verdict follows)
## tests power
[07:14:21Z] :: POWER-UI: panel_ok=true notice_ok=true -> PASS ::
## tests prefs
[07:14:21Z] :: PREFS-CODEC: kats=9/9 -> PASS ::
[07:14:21Z] :: PREFS-FIXTURE: codec=1 types=0 file=0 temp_gone=1 malformed_refused=0 save_held=0 restored=1 loadclamp=0 clamp=0 wire=diverged schema_rows=49 -> FAIL ::
[07:14:21Z] :: SETTINGSFILES: domains=0 files=0 migrated=0 readable=0 reset_on_delete=fail app_ns=fail alone=1 dir=/home/una/settings -> FAIL ::
## tests prefscap
[07:14:21Z] :: PREFSCAP: own=fail foreign=refused system=18-settable declare_cap=64 declare_own=1 declare_foreign=refused deputy=refused -> FAIL ::
## tests prof
[07:14:21Z] :: PROFILE: hz=1000 samples=5999 dropped=0 top=0x3b56c500 -> PASS ::
## tests prof2
[07:14:21Z] :: PROFILE2: stacks=3600 depth_max=11 names=3 r3_syms=skip linux_hook=1 sys_prof=1 -> PASS ::
## tests prtscr
[07:14:21Z] :: PRTSCR: fixture door=free waited_ms=0 took=1 refused=1 released=1 -> PASS ::
## tests ptrlag
[07:14:21Z] :: TESTS: ptrlag -> SKIP reason=no-verdict ::
## tests pwwire
[07:14:21Z] :: PWWIRE: kat=17/17 trace=redacted keytrace=class keys_withheld=21/21 lines=2906 window=2000 hits=0 -> PASS ::
## tests quarry2
[07:14:21Z] :: QUARRY2: columns=type,origin sort=name|size|mtime|type types=+markdown,+json gif_frames=2 stepper=ok listing_type=skip origin=skip(no-volume) -> PASS ::
## tests quarry3
[07:14:21Z] :: QUARRY3: sidebar=ok toolbar=ok views=skip(no-test-f) search=FAIL quicklook=0/24 -> FAIL :: favorites=1/6 locations=UNAOS,UNAOS SDHC4,boot search_src=walk routed=0 owed=- ::
## tests quarrylive
[07:14:21Z] :: QUARRYLIVE: base=/home/una reason=mkdir -> SKIP ::
## tests queuesleep
[07:14:21Z] :: QUEUESLEEP: armed idle_s=30 on=task cpu=2 via=tests -> SKIP reason=verdict-follows :: (the window runs on its own task; the shell and the tests table are free; the verdict line follows in ~33 s)
## tests quietboot
[07:14:21Z] :: QUIETBOOT: lines=152 bound=250 census=0 -> PASS ::
## tests rawcore
[07:14:21Z] :: RAWCORE: could not write /home/una/RAWTEST.ARW -> FAIL ::
## tests refusalui
[07:14:21Z] :: REFUSALUI: opener=alert toast_action=answered fwpin=toast rootacl=FAIL -> FAIL :: repeat=SHOWN-TWICE queued=0
## tests renderwedge
[07:14:21Z] :: POINTERFEEL: reports=0 rate_hz=0.0 gaps=0 gap_max_ms=0 worst_holder=- route=- -> SKIP :: reason=no-touch bound_ms=16 window_ms=5000 touch_wait_ms=20000 reassert=0 fr=worker:c2 fr_worst_ms=282 login_gap_ms=12 login_gap_holder=unheld/step=net-hda via=tests
[07:14:21Z] :: RENDERWEDGE: age_ms=5241 last_handler=fixture holder=scratch:rw-holder:263@cpu2 -> SEEN :: wait=- render=4@cpu1 watch_cpu=2 wedges=0 fixture=1
[07:14:21Z] :: RENDERWEDGE: fixture=1 job=timeout:1500 wait=timeout:303 watch=seen:5241 holder=scratch:rw-holder:263@cpu2 released=1 cleaned=1 -> PASS :: job_bound_ms=1500 wait_bound_ms=300 wedge_ms=5000 watch_cpu=2
## tests ring3abi
[07:14:21Z] :: RING3ABI2: getrandom=1 args=2 whoami=una net_window=elf big_window=elf arm_sbrk=skip -> PASS ::
## tests ring3win
[07:14:21Z] :: RING3WIN: model=elf window=67108864 big_ok=1 sbrk=3170304 freed=1 -> PASS ::
## tests ringlogin
[07:14:21Z] :: RINGLOGIN: ring=no-holocron at=none reconnect_before_unlock=FAIL rekey=ok settings_rekey=ok settings_rekeyed=0 arm=x86 passwd_rekeyed=0 door=none posted=0 taken=0 report=none -> FAIL ::
## tests rootdisk
[07:14:21Z] :: ROOTDISK: root=fat lib=- apps_alias=- user_dir_at_root=- -> SKIP :: reason=fat-root (no UnaFS volume is /) ::
## tests sanity
[07:14:21Z] :: SANITY: const_asserts=17 runtime_moved=51 left=0 -> PASS ::
## tests scrollback
[07:14:21Z] :: SCROLLBACK: rows=2000 cols=77 view_off=100 marker_ok=ok sel_ok=ok clear_ok=ok -> PASS ::
## tests selfbuild
[07:14:21Z] :: SELFBUILD1: tcc=skip hello=skip probe=none probe_syscalls=0 probe_missing=[] -> SKIP reason=tcc-hello-not-staged ::
## tests selfbuild2
[07:14:21Z] :: LINUXABI-KAT2: threads=0 counter=0 futex_waits=0 epoll=skip statx=skip sigreturn=skip -> SKIP (HELLO.C not staged) ::
[07:14:21Z] :: SELFBUILD2: kat2=skip probe=skip hi=0 cat_lines=0 probe_syscalls=0 probe_missing=[] -> SKIP ::
## tests selfbuild3
[07:14:21Z] :: LINUXABI-KAT3: -> SKIP (HELLO.C not staged) ::
[07:14:21Z] :: SELFBUILD3: mmap=skip shared=skip anon_mib=0 resident_pages=0 libc=none tcc_libc=skip -> SKIP ::
## tests selfbuild4
[07:14:21Z] :: LINUXABI-KAT4: -> SKIP (fixture not staged) ::
[07:14:21Z] :: SELFBUILD4: rust=skip threads=0 counter=0 hashmap=skip fs=skip panic_caught=0 fork=skip pipe=skip exec_lazy=0 cow_pages=0 -> SKIP ::
## tests selfbuild5
[07:14:21Z] :: LINUXABI-KAT5: -> SKIP (fixture not staged) ::
[07:14:21Z] :: SELFBUILD5: mremap=skip altstack=skip lld=skip link_ms=0 link_peak_mib=0 relink_runs=0 -> SKIP ::
## tests selfbuild6
[07:14:21Z] :: SELFBUILD6: dyn=skip lld_dyn=skip rustc_version=skip rustc_hello=skip proc_macro=skip relocs=0 ms=11 -> SKIP ::
## tests selfdiag
[07:14:21Z] :: SELFDIAG: provider=echo stage=mkdirs err=-95 dir=/boot/SDFIX bootlog=fat-root -> FAIL ::
## tests selfhost
[07:14:21Z] :: TESTS: selfhost -> SKIP reason=no-verdict ::
## tests serwit2
[07:14:21Z] :: TESTS: serwit2 -> SKIP reason=no-verdict ::
## tests sessionwedge
[07:14:21Z] :: PUMPWEDGE: age_ms=2091 last_step=- holder=- -> SEEN :: pump=1@cpu5 passes=451370 wedges=0 bound_ms=2000 fixture=1
[07:14:21Z] :: SESSIONWEDGE: bare_to_built_ms=425 pump_gap_max_ms=435 pump_passes=4218 through=1 armed=xhci-poll:1,smc-batt:1,bootlog-flush:0,wall-probe:5 watch=seen:2091 cleared=damage rects=1 -> PASS :: window_ms=5000 wedge_ms=2000 wedges=0 watched=1
## tests setterwedge
[      ?ms] :: SETTERWEDGE: from=setter watch=armed armed_ms=12737 watch_cpu=2 running=1 replay=ok replay_ms=4 census=1 cable=empty -> PASS :: ignite_ms=12737 fixture_census=1 rescues=1 rescue_bytes=0 wedges=0
## tests settings
[07:14:21Z] :: SETTINGS: controls=11 tabs=9 loaded=0 saved=0 -> PASS ::
[      ?ms] :: SETTINGS: controls=11 tabs=9 loaded=0 saved=1 -> PASS ::
[07:14:21Z] :: SETTINGS: controls=11 tabs=9 loaded=0 saved=1 -> PASS ::
[07:14:21Z] :: SETTINGS: controls=11 tabs=9 loaded=0 saved=1 -> FAIL ::
[07:14:21Z] :: SETTINGS-USERS: listed=1 added=1 deleted=1 refused=2 -> PASS ::
[07:14:21Z] :: SETTINGS: controls=11 tabs=9 loaded=0 saved=1 -> PASS ::
[07:14:21Z] :: SETTINGS: controls=11 tabs=9 loaded=0 saved=1 -> PASS ::
[07:14:21Z] :: SETTINGS: controls=11 tabs=9 loaded=0 saved=1 -> PASS ::
[07:14:21Z] :: PREFSUI: slider_drag=ok login_items=0 modes=1 -> PASS ::
## tests settingsbus
[07:14:21Z] :: SETTINGSBUS: via=kernel set=0 get=ne changed=0 direct_writers=0 -> FAIL ::
## tests shelltask
[07:14:21Z] :: SHELLTASK: shell_task=none here_tid=4 -> SKIP reason=not-on-shell-task (type `tests shelltask` in the shell window) ::
## tests shelltask2
[07:14:21Z] :: SHELLTASK2: abort=ok locks=0 queued_kept=0 dockpin=task dock_lines=0 run=task aborts=0 -> PASS ::
## tests shellux
[07:14:21Z] :: SHELLUX: history=up-down completions=verbs+paths ctrl=[c,l,a,e,u,w] cwd=/ -> PASS ::
## tests shortcuts
[07:14:21Z] :: SHORTCUTS: entries=40 scopes=11 shown=40 -> PASS ::
## tests shot
[07:14:21Z] :: PRTSCR-VOL: rung=1 source=global serial=0xA2E90CE2 label=UNAOS rw=yes -> MOUNTED ::
[07:14:21Z] :: PRTSCR-DIR: theme=crispy user=una home=/home/una dir=/home/una/Desktop path=home/una/Desktop created=3 reason=session -> RESOLVED ::
[07:14:21Z] :: SHOTZIP: raw=3072800 deflated=54491 ratio=56.3x enc_ms=178 filter=sub chain=32 blocks=fixed-huffman verify=inflate -> PASS ::
[07:14:21Z] :: SHOTMOUNT: via=fat path=/home/una/Desktop/DESKTOP.PNG bytes=54548 -> FAIL ::
[07:14:21Z] :: PRTSCR: DESKTOP.PNG 1280x800 54548 bytes -> OK :: source=global serial=0xA2E90CE2 dir=home/una/Desktop bytes_written=54548 chunks=1 kind=panel rect=0,0,1280x800 ::
[07:14:21Z] :: SHOT: state=desktop file=/home/una/Desktop/DESKTOP.PNG bytes=54548 settle=ok samples=3 settle_ms=201 mask=clock@1223,0,45,34;glyphs@987,0,236,34 mask_file=/home/una/Desktop/DESKTOP.MSK mask_bytes=0 (create /home/una/Desktop/DESKTOP.MSK: Unsupported) -> FAIL ::
[07:14:21Z] :: SHOT: state=desktop reason=create /home/una/Desktop/DESKTOP.MSK: Unsupported -> FAIL ::
[07:14:21Z] :: GLASSEYES: states=6 shot=desktop png=none mask=none -> FAIL ::
## tests shotmenu
[07:14:21Z] :: TESTS: shotmenu -> SKIP reason=no-verdict ::
## tests sinkdrain
[07:14:21Z] :: SINKDRAIN: FAIL — staged=8 drained=8 on_cable=0 tally_on_cable=true peeked=70B peek_tries=0 residue=0 dropped=0 mirror_dropped=0 -> FAIL ::
## tests smallfix3
[07:14:21Z] :: SMALLFIX3: boot_writers=unarmed event_codes=unique bus_verbs=unique action_codes=unique theme_rows=derived -> PASS :: events=12 verbs=27 actions=49 crispy_rows=63 pc_rows=37 ::
## tests smallfix4
[07:14:21Z] :: SMALLFIX4: event_codes=unique bus_verbs=unique registrable=unique host_verbs=unique action_codes=unique launch_name=lumen -> PASS :: events=12 verbs=27 registrable_tags=3 host_verb_rows=102 actions=49 ::
## tests smallfix5
[07:14:21Z] :: SMALLFIX5: stamp_rule=ok player_flag=ok spawn_arms=4 refusal_owner=0/0 pwr_window_ms=60000 boot80_types=stamp probe=idle -> PASS :: invalidated=0 left=f6-early-return(witness-counted) ::
## tests smallfix6
[07:14:21Z] :: SMALLFIX6: fwvol=skipped(volume=boot) openers=player/player windowcap=registered notfound=said lag=1039@2437:none lumen=split -> FAIL :: spans=users-load:23,root-mount:3,stage-resolve:90
## tests smallfix7
[07:14:21Z] :: SMALLFIX7: deferred=announced verdicts=6:sdhc,DRAINCAP,SERWIT-1B,PWRDRAIN,SERWIRE,SERIALTX word=ok ownrun=ok prtscr=registered ring=48 preheap=parked saves=0 -> FAIL ::
## tests smpload
[07:14:21Z] :: SMPLOAD-JUDGE: flat=0/0 pegged=0/0 burst=1/1 twice=1/2 untracked=0/0 fail_at=2 -> PASS ::
## tests sntp
[07:14:21Z] :: SNTP-X86-GATE: x86 sntp client battery PASS [w=0x3f] (parse-ok+iso|reject-short|kod|reject-alarm|fixture-clock|live-untouched) -> PASS ::
## tests sock
[07:14:21Z] :: TESTS: sock -> SKIP reason=no-verdict ::
## tests sonde
[07:14:21Z] :: SONDE: fixture=318B fns=3 streams=depth,ir,color fmt=Z16 w=848 h=480 xu=3 hid=4 hwmon_cmd=ok depth_units_reply=100 -> PASS ::
[07:14:21Z] :: SONDE: dev=none streams=- fmt=- w=0 h=0 depth_scale=- frames_2s=0 -> SKIP(no-device) ::
## tests soundopeners
[07:14:21Z] :: TESTS: soundopeners -> SKIP reason=no-verdict ::
## tests spawnstorm
[07:14:21Z] :: WINDOWCAP3: fixed_pools=none procs_limit=54 from=mem spawned=54 refused_at=54 paused_ms=51 machine_alive=1 -> PASS :: base=0 reaped=1 slots=54 heap_free_mib=203->141 refusal="process limit reached (R90: memory's limit, `[wm] limit` line) — the launcher was paused"
## tests splash
[07:14:21Z] :: SPLASH: steps=3 slowest=stage-resolve:90 over=0 live=0 watch=task cleared=damage rects=1 owed=0 -> PASS ::
## tests stackroom
[07:14:21Z] :: STACKROOM: tasks=11 aps=5 ist=24 armed=350 live=11 failed=0 ipi_flush=timeout shootdowns=371 timeouts=23 bsp=owed page=4096 -> FAIL ::
## tests statustray
[07:14:21Z] :: STATUSTRAY-T: volume=ok slider=ok mute=ok net=no input=ok clock=ok unset=ok absent=ok opens=0 picks=4 -> FAIL ::
## tests svclatch
[07:14:21Z] :: SVCLATCH: passes_idle_locks=0 comp_alloc_per_pass=0 damage_gen=ok -> PASS :: opens=0 comp_passes=147 comp_grows=5 heap=0 dmg_walks=1750 dmg_skips=2784 session_gen=1 ::
## tests termcolor
[07:14:21Z] :: TERMCOLOR: spans_max=16 sgr_ok=ok erase_ok=ok palette=16+256+rgb -> PASS ::
## tests testf
[07:14:21Z] :: TESTF: staged=0/24 missing=all -> FAIL :: reason=no-dir (looked in /system/test-f, /boot/system/test-f; the builder stages it from builder/testf.list) ::
## tests trackpad
[07:14:21Z] [strip] vacate tenant=dock box=1200x52+40+736 uncovered_px=936 erased=yes src=flat by=dock::compose reason=shrink -> SCENE-RESTORE
[07:14:21Z] :: SETTINGS: controls=11 tabs=9 loaded=0 saved=1 -> PASS ::
[07:14:21Z] :: TRACKPADPANE: speed=5 tap=0 natural=1 secondary=two-finger three_drag=0 applied=live -> PASS :: drag=1->10 toggles=ok curve_default=x1
## tests tste
[07:14:21Z] :: TSTE: midden.dispatch -> PASS ::
[07:14:21Z] :: TSTE: midden.route -> PASS ::
[07:14:21Z] :: TSTE: midden.resolve -> PASS ::
[07:14:21Z] :: [midden] resolve "vug" -> VUG.ELF ::
[07:14:21Z] :: TSTE: midden.precedence -> PASS ::
[07:14:21Z] :: TSTE: shell.basics.table -> PASS ::
[07:14:21Z] :: TSTE: shell.basics.help -> PASS ::
[07:14:21Z] :: TSTE: shell.basics.echo -> PASS ::
[07:14:21Z] :: TSTE: shell.basics.wc -> PASS ::
[07:14:21Z] :: TSTE: shell.basics.grep -> PASS ::
[07:14:21Z] :: TSTE: shell.relics.renamed -> PASS ::
[07:14:21Z] :: TSTE: shell.relics.help -> PASS ::
[07:14:21Z] :: [fatverb] write -> REFUSED READ-ONLY (volume=boot path=/0 reason=the volume rides an internal SATA disk, which is mounted READ-ONLY — the block layer refuses every write to it and this image compiles no ATA write opcode at all (AHCI / AHCIBOOT); handles=global=present sdhc=present) ::
[07:14:21Z] :: TSTE: shell.relics.write_raw -> PASS ::
[07:14:21Z] :: TSTE: vfs.aclsym.dir -> PASS ::
## tests u20c
[07:14:21Z] :: U2-0c: self-NMI taken on IST -> PASS ::
## tests u3
[07:14:21Z] :: U3: per-process CR3 isolation (A=0xa5a5a5a5000000a1 B=0x5a5a5a5a000000b2 distinct) -> PASS ::
## tests una
[07:14:21Z] :: UNAAPP: image=/apps/UNA.ELF window=bad panes=0 lines=0 saved=skip highlight=0 -> SKIP ::
## tests unafs
[07:14:21Z] :: UNAFSX86-T: root=boot (not unafs) -> SKIP ::
## tests unafsgrow
[07:14:21Z] :: UNAFSGROW: from=512 to=768 fsck=ok -> PASS ::
## tests unafsroot
[07:14:21Z] :: USBUNPUB: gen_before=1 gen_after=3 ix=1 placed=true publish_step=true stale_refused=true stale_quiet=true retracted=true retract_step=true cleared=true noop_refused=true noop_quiet=true -> PASS ::
[07:14:21Z] :: LBA32: first=0x00000000 last=0xffffffff refused=3/3 wrapto=0x00000000 byteoff=2199023255552 -> PASS ::
## tests usbnet
[07:14:21Z] :: USBNET: bus=xhci slot=4 mac=52:54:00:12:34:57 link=down speed=0 usb=? rx=6 tx=5 rx_drop=0 tx_drop=0 errors=0 rx_ok=0 rx_crc=0 rx_drop_err=0 rx_short=0 rx_chip_drop=0 rx_xfers=8 rx_zlp=2 rx_arms=9 buf=2048 re=0 -> PASS ::
[07:14:21Z] :: USBNET6: chip=ecm rx_ctl=0x0000 mfb=qctrl-buf2048 frames=0 dropped=0 first_ethertype=none reason=no-frame-5s -> FAIL ::
[07:14:21Z] :: USBNET7: rx_ok=0 first_frame_ms=1012 kicks=0 resets=0 idle_rungs=2 ethertype=0x0800 window_ms=none -> PASS ::
## tests usbnet7
[07:14:21Z] :: USBNET7: rx_ok=0 frames=1 first_frame_ms=4 ethertype=0x0806 dhcp=offer kicks=0 resets=0 idle_rungs=3 -> FAIL ::
## tests usbnetftdi
[07:14:21Z] :: USBNETFTDI: ftdi_alive=1 rx_armed=1 events=0 beats=7 drives=18488 -> PASS ::
## tests usbnetrx
[07:14:21Z] :: USBNETRX: armed=1 doorbell=1 ep_state=running frames=0 window=0 td=unread usb=fs -> FAIL ::
## tests usbstor
[07:14:21Z] :: USBSTOR: boot_medium_kept=0 stick_at_boot=1 pw_write=ok mounted=UNAOS:fat32 hotplug=- -> FAIL :: reason=boot-medium-not-pinned refused=0 ::
## tests users
[07:14:21Z] :: HOMEPRIV: root=fat home_owner=n/a -> SKIP (a FAT root carries no owner row, SO35) ::
## tests usersready
[07:14:21Z] :: USERSREADY: rmbp-shape(global=0 sdhc=1 ahci=1)=1 qemu-shape(global=1 sdhc=1 ahci=0)=1 none=0 old-guard-on-rmbp=0 this-boot ready-by=global=1 sdhc=1 ahci=1 -> PASS ::
[07:14:21Z] :: USERSMOUNT: rmbp-shape=sdhc qemu-shape=global none=none old-mount-on-rmbp=none this-boot via=sdhc -> PASS ::
## tests uvc
[07:14:21Z] :: uvc: selftest fixture=289B formats=2 frames=3 alts=2 probe_roundtrip=ok probe_reply=ok -> PASS ::
[07:14:21Z] :: uvc: stream-selftest hdr=ok asm frames=4 good=1 short=1 errf=1 over=1 ep=ok ss=ok cut=ok -> PASS ::
[07:14:21Z] :: UVC: selftest=PASS stream_selftest=PASS host=- frames_2s=- fps=- -> SKIP reason=qemu-no-uvc-stream ::
## tests verdicts
[07:14:21Z] :: VERDICTS: before_deferred=6 moved=17 reworded=18 announced=1 tags=sdhc,DRAINCAP,SERWIT-1B,PWRDRAIN,SERWIRE,SERIALTX -> FAIL ::
## tests videoplayer
[07:14:21Z] :: VIDEOPLAYER: path=TEST.WEBM reason=no-file (system/test-f or /home) -> SKIP ::
[07:14:21Z] :: VIDEOPLAYER: path=TEST.MP4 reason=no-file (system/test-f or /home) -> SKIP ::
[07:14:21Z] :: VIDEOPLAYER: reason=no-TEST.WEBM-or-TEST.MP4 -> SKIP ::
## tests volumes
[07:14:21Z] :: VOLUMES: shown=- efi=- -> SKIP reason=fat-root (homes live on the FAT by design) :: pure=true ::
## tests wcdlatch
[07:14:21Z] :: WCDLATCH: ids=2 calls=24 said=2 want_said=2 counted=48 want_counted=48 -> PASS ::
## tests wifi
[07:14:21Z] :: WIFI5: rungs=27 confirmed=13 refuted=2 open=10 parked=2 upload_unwind=reboot-proven staged=0/3 fw=absent uploaded_this_boot=0 ucode_rev=none rx_frames_5s=not-watched phy_reset=not-run tune=declined(seat-rule) rx=declined(no-counter) -> NOT-READY reason=firmware-not-staged (UNAOS_WIFI_FW_PATH stages the set onto the card's /FIRMWARE/) ::
[07:14:21Z] :: WIFI1: radio ABSENT or cross-check FAILED — no bring-up to verify -> SKIP ::
## tests window
[07:14:21Z] :: WINDOW2: bytes=67108864 image_limit=67108864 alloc48m=ok fixed20m=skip(no-writable-home) lld_pie=skip(lld-not-staged) kdf_ring3=ok -> SKIP ::
## tests windowcap
[07:14:21Z] :: WINDOWCAP: fixed_cap=none limit=172 procs=54 from=mem:220 opens_refused=0 -> PASS :: via=tests
## tests windowlist
[07:14:21Z] :: WINDOWLIST: rows=0 live=2 focused=2 minimised=0 show_desktop_ok=false -> FAIL ::
## tests winmemory
[07:14:21Z] :: WINMEMORY: saved=0 saves_this_boot=0 restored=ok cascade=ok clamp=ok codec=ok close_save=ok step=39 area=34..724 -> PASS ::
## tests winx
[07:14:21Z] :: WINX-1: ring-3 windows — create/paint/readback/present OK, sleep+yield OK, unowned present -EBADF, surface verified kernel-side, 2 presents, teardown clean -> PASS ::
## tests winx3
[07:14:21Z] :: WINX-3: ELF loader — 2 PT_LOADs mapped W^X, entry 0x1000000000b, ring-3 witness 0x7f through the ELF path, 2 presents, wrong-arch + W+X + rx-crosses-final-window-page images refused (edge loads), reap clean -> PASS ::
## tests wxaudit
[07:14:21Z] :: WXAUDIT-0: classifier fires on W+X, clears RO-X, honours parent NX, voids on NXE=0 -> PASS ::
## tests x86bind
[07:14:21Z] :: X86BIND: root=ahci5:/kernel.elf serial=0xfabe1afd by=content bootinfo=0xfabe1afd agrees=yes mounts=7 layout=true -> PASS ::
## tests xhcimedia
[07:14:21Z] :: BRIGHTKEYS: key=up level=12/16 gmux_written=0 indicator=0 -> FAIL ::
[07:14:21Z] :: XHCIMEDIA: router=one paths=ehci,xhci keys=8 bright=ok media=ok volume=ok other=ok -> PASS ::
```

## Per-fixture tally (the kernel's `:: TESTS: ran=` line for each `tests <name>`)

```
DONE activity 0.6s :: TESTS: ran=0 pass=0 fail=0 failed=[] skipped=[] ::
DONE aether 1.3s :: TESTS: ran=1 pass=7 fail=0 failed=[] skipped=[] ::
DONE ahciw 0.5s :: TESTS: ran=0 pass=0 fail=0 failed=[] skipped=[] ::
DONE appearance 0.7s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE appmenu 0.8s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE appres 0.8s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE appswitch 0.6s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE apptrust 0.9s :: TESTS: ran=1 pass=0 fail=1 failed=[apptrust] skipped=[] ::
DONE assocstamp 0.7s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[assocstamp] ::
DONE attr 0.4s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[attr] ::
DONE attrcolumns 0.7s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[attrcolumns] ::
DONE bandy3 0.5s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE battery 0.6s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE bezel 1.2s :: TESTS: ran=1 pass=2 fail=0 failed=[] skipped=[] ::
DONE blitter 0.6s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE blitwire 0.6s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE boot80 0.5s :: TESTS: ran=1 pass=0 fail=1 failed=[boot80] skipped=[] ::
DONE bootfat 0.6s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[bootfat] ::
DONE bootfatseam 0.7s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[bootfatseam] ::
DONE botpark 0.6s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE brightfloor 1.5s :: TESTS: ran=1 pass=2 fail=1 failed=[brightfloor] skipped=[] ::
DONE brightstep 1.9s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[brightstep] ::
DONE bt 0.4s :: TESTS: ran=1 pass=4 fail=2 failed=[bt] skipped=[] ::
DONE btkeyseal 0.6s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[btkeyseal] ::
DONE canonguard 0.7s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE ce 0.4s :: TESTS: ran=0 pass=0 fail=0 failed=[] skipped=[] ::
DONE clickband 0.6s :: TESTS: ran=0 pass=0 fail=0 failed=[] skipped=[] ::
DONE clickroute 0.7s :: TESTS: ran=0 pass=0 fail=0 failed=[] skipped=[] ::
DONE clock 0.7s :: TESTS: ran=1 pass=0 fail=2 failed=[clock] skipped=[] ::
DONE columnsview 0.7s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[columnsview] ::
DONE crystal 0.6s :: TESTS: ran=0 pass=0 fail=0 failed=[] skipped=[] ::
DONE ctrldecline 0.7s :: TESTS: ran=0 pass=0 fail=0 failed=[] skipped=[] ::
DONE cursorshape 0.7s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE dimidle 3.6s :: TESTS: ran=1 pass=0 fail=1 failed=[dimidle] skipped=[dimidle] ::
DONE dmgovlp 0.6s :: TESTS: ran=0 pass=0 fail=0 failed=[] skipped=[] ::
DONE dns 0.4s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[dns] ::
DONE dock 0.4s :: TESTS: ran=0 pass=0 fail=0 failed=[] skipped=[] ::
DONE dockgone 0.6s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE dockpin 0.6s :: TESTS: ran=0 pass=0 fail=0 failed=[] skipped=[] ::
DONE dockpins 21.4s :: TESTS: ran=1 pass=0 fail=1 failed=[dockpins] skipped=[] ::
DONE door 0.4s :: TESTS: ran=1 pass=0 fail=1 failed=[door] skipped=[] ::
DONE dragdrop 0.6s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[dragdrop] ::
DONE dragdrop2 0.6s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[dragdrop2] ::
DONE dragperf 0.6s :: TESTS: ran=0 pass=0 fail=0 failed=[] skipped=[] ::
DONE dragwedge 0.6s :: TESTS: ran=0 pass=0 fail=0 failed=[] skipped=[] ::
DONE ehci 1.4s :: TESTS: ran=1 pass=5 fail=1 failed=[ehci] skipped=[] ::
DONE ehciisr 0.6s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE elfbss 0.5s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE exec 0.9s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE exfat 0.5s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[exfat] ::
DONE facetanim 0.6s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[facetanim] ::
DONE fatverb 0.6s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[fatverb] ::
DONE fileopen 0.6s :: TESTS: ran=0 pass=0 fail=0 failed=[] skipped=[] ::
DONE filetype 0.6s :: TESTS: ran=1 pass=0 fail=1 failed=[filetype] skipped=[] ::
DONE filetypes 0.6s :: TESTS: ran=1 pass=0 fail=1 failed=[filetypes] skipped=[] ::
DONE finemotion 0.7s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE firstuser 0.6s :: TESTS: ran=1 pass=0 fail=1 failed=[firstuser] skipped=[] ::
DONE flightring 0.7s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE folderview 0.7s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[folderview] ::
DONE font 0.4s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE gen7 0.4s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[gen7] ::
DONE gpublit 0.6s :: TESTS: ran=1 pass=0 fail=1 failed=[gpublit] skipped=[] ::
DONE hda 1.9s :: TESTS: ran=1 pass=3 fail=0 failed=[] skipped=[] ::
DONE hda1 1.9s :: TESTS: ran=1 pass=3 fail=0 failed=[] skipped=[] ::
DONE hda2 1.9s :: TESTS: ran=1 pass=4 fail=0 failed=[] skipped=[] ::
DONE hda220 2.0s :: TESTS: ran=1 pass=4 fail=0 failed=[] skipped=[] ::
DONE hda880 2.0s :: TESTS: ran=1 pass=4 fail=0 failed=[] skipped=[] ::
DONE hdaboth 2.1s :: TESTS: ran=1 pass=4 fail=0 failed=[] skipped=[] ::
DONE helpdoc 0.6s :: TESTS: ran=1 pass=0 fail=1 failed=[helpdoc] skipped=[] ::
DONE hidstall 0.6s :: TESTS: ran=1 pass=0 fail=1 failed=[hidstall] skipped=[] ::
DONE hittest 0.6s :: TESTS: ran=0 pass=0 fail=0 failed=[] skipped=[] ::
DONE holocron 0.6s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE homesoil 0.6s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[homesoil] ::
DONE imgview 0.6s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[imgview] ::
DONE inputstall 0.7s :: TESTS: ran=1 pass=0 fail=1 failed=[inputstall] skipped=[] ::
DONE install 0.6s :: TESTS: ran=0 pass=0 fail=0 failed=[] skipped=[] ::
DONE installbare 0.7s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE instgui 0.6s :: TESTS: ran=0 pass=0 fail=0 failed=[] skipped=[] ::
DONE inwedge 0.6s :: TESTS: ran=0 pass=0 fail=0 failed=[] skipped=[] ::
DONE jobscan 0.6s :: TESTS: ran=1 pass=0 fail=1 failed=[jobscan] skipped=[] ::
DONE jobsui 0.5s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE kblit 0.5s :: TESTS: ran=0 pass=0 fail=0 failed=[] skipped=[] ::
DONE keplerlog 0.6s :: TESTS: ran=0 pass=0 fail=0 failed=[] skipped=[] ::
DONE kgr 0.4s :: TESTS: ran=0 pass=0 fail=0 failed=[] skipped=[] ::
DONE kvblank 0.6s :: TESTS: ran=0 pass=0 fail=0 failed=[] skipped=[] ::
DONE kvblank8 0.6s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[kvblank8] ::
DONE ladder 4.0s :: TESTS: ran=1 pass=19 fail=0 failed=[] skipped=[] ::
DONE launcher 0.9s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE lidsleep 1.9s :: TESTS: ran=1 pass=0 fail=1 failed=[lidsleep] skipped=[] ::
DONE linuxabi 0.9s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE linuxabi2 0.9s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE linuxabi3 0.9s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE loader 0.5s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE lockreg 0.8s :: TESTS: ran=1 pass=0 fail=1 failed=[lockreg] skipped=[] ::
DONE loginfurn 0.6s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE loginrender 0.7s :: TESTS: ran=1 pass=0 fail=1 failed=[loginrender] skipped=[] ::
DONE loginwindow 0.7s :: TESTS: ran=0 pass=0 fail=0 failed=[] skipped=[] ::
DONE logout 0.5s :: TESTS: ran=0 pass=0 fail=0 failed=[] skipped=[] ::
DONE logwit 0.5s :: TESTS: ran=1 pass=0 fail=1 failed=[logwit] skipped=[] ::
DONE lumen 1.0s :: TESTS: ran=1 pass=3 fail=0 failed=[] skipped=[] ::
DONE lumenfast 1.1s :: TESTS: ran=1 pass=2 fail=0 failed=[] skipped=[] ::
DONE menubar 0.6s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE metrics 0.6s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE midden 0.5s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[midden] ::
DONE movevacate 0.7s :: TESTS: ran=0 pass=0 fail=0 failed=[] skipped=[] ::
DONE name 0.4s :: TESTS: ran=0 pass=0 fail=0 failed=[] skipped=[] ::
DONE nameindex 0.6s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[nameindex] ::
DONE net 2.4s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE netclock 0.6s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[netclock] ::
DONE netfetch 0.6s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE nethang 2.6s :: TESTS: ran=1 pass=0 fail=1 failed=[nethang] skipped=[] ::
DONE notice 0.8s :: TESTS: ran=1 pass=3 fail=0 failed=[] skipped=[] ::
DONE notify 0.8s :: TESTS: ran=1 pass=0 fail=1 failed=[notify] skipped=[] ::
DONE notifypane 0.7s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE openertrust 0.7s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE panicscreen 2.7s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE passperiod 0.7s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
SKIPPED play (brief: never mp3 play)
DONE player 0.5s :: TESTS: ran=1 pass=0 fail=1 failed=[player] skipped=[] ::
DONE playwav 0.6s :: TESTS: ran=1 pass=0 fail=1 failed=[playwav] skipped=[] ::
DONE pointerfeel 0.7s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[pointerfeel] ::
DONE power 0.5s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE prefs 0.7s :: TESTS: ran=1 pass=1 fail=2 failed=[prefs] skipped=[] ::
DONE prefscap 0.9s :: TESTS: ran=1 pass=0 fail=1 failed=[prefscap] skipped=[] ::
DONE prof 1.4s :: TESTS: ran=1 pass=2 fail=0 failed=[] skipped=[] ::
DONE prof2 1.2s :: TESTS: ran=1 pass=2 fail=0 failed=[] skipped=[] ::
DONE prtscr 0.5s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE ptrdead 0.6s :: TESTS: ran=0 pass=0 fail=0 failed=[] skipped=[] ::
DONE ptrlag 0.5s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[ptrlag] ::
DONE pwwire 0.5s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE quarry2 0.6s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE quarry3 0.6s :: TESTS: ran=1 pass=0 fail=1 failed=[quarry3] skipped=[] ::
DONE quarrylive 0.7s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[quarrylive] ::
DONE quarryops 0.6s :: TESTS: ran=0 pass=0 fail=0 failed=[] skipped=[] ::
DONE queuesleep 0.7s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[queuesleep] ::
DONE quietboot 0.6s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE rawcore 0.6s :: TESTS: ran=1 pass=0 fail=1 failed=[rawcore] skipped=[] ::
DONE refusalui 0.9s :: TESTS: ran=1 pass=0 fail=1 failed=[refusalui] skipped=[] ::
DONE renderwedge 8.0s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE ring3abi 0.6s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE ring3win 0.9s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE ringlogin 1.6s :: TESTS: ran=1 pass=0 fail=1 failed=[ringlogin] skipped=[] ::
DONE rootacl 0.6s :: TESTS: ran=0 pass=0 fail=0 failed=[] skipped=[] ::
DONE rootdisk 0.6s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[rootdisk] ::
DONE sanity 0.5s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE scrollback 0.7s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE selfbuild 0.6s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[selfbuild] ::
DONE selfbuild2 0.7s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[selfbuild2] ::
DONE selfbuild3 0.7s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[selfbuild3] ::
DONE selfbuild4 0.7s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[selfbuild4] ::
DONE selfbuild5 0.7s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[selfbuild5] ::
DONE selfbuild6 0.7s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[selfbuild6] ::
DONE selfdiag 0.6s :: TESTS: ran=1 pass=0 fail=1 failed=[selfdiag] skipped=[] ::
DONE selfhost 0.6s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[selfhost] ::
DONE selfinstall 0.7s :: TESTS: ran=0 pass=0 fail=0 failed=[] skipped=[] ::
DONE serwit2 0.6s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[serwit2] ::
DONE sessionwedge 1.0s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE setterwedge 0.7s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE settings 1.4s :: TESTS: ran=1 pass=8 fail=1 failed=[settings] skipped=[] ::
DONE settingsbus 0.7s :: TESTS: ran=1 pass=0 fail=1 failed=[settingsbus] skipped=[] ::
DONE shelltask 0.6s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[shelltask] ::
DONE shelltask2 0.7s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE shellux 0.6s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE shortcuts 0.6s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE shot 0.9s :: TESTS: ran=1 pass=1 fail=4 failed=[shot] skipped=[] ::
DONE shotmenu 0.6s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[shotmenu] ::
DONE shotregion 0.7s :: TESTS: ran=0 pass=0 fail=0 failed=[] skipped=[] ::
DONE sinkdrain 0.6s :: TESTS: ran=1 pass=0 fail=1 failed=[sinkdrain] skipped=[] ::
DONE smallfix3 0.6s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE smallfix4 0.6s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE smallfix5 0.6s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE smallfix6 0.6s :: TESTS: ran=1 pass=0 fail=1 failed=[smallfix6] skipped=[] ::
DONE smallfix7 0.6s :: TESTS: ran=1 pass=0 fail=1 failed=[smallfix7] skipped=[] ::
DONE smpload 0.6s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE sntp 0.4s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE sock 0.4s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[sock] ::
DONE sonde 0.7s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE soundopeners 0.8s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[soundopeners] ::
DONE spawnstorm 0.9s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE splash 0.5s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE srcextract 0.7s :: TESTS: ran=0 pass=0 fail=0 failed=[] skipped=[] ::
DONE stackroom 0.9s :: TESTS: ran=1 pass=0 fail=1 failed=[stackroom] skipped=[] ::
DONE statustray 1.2s :: TESTS: ran=1 pass=0 fail=1 failed=[statustray] skipped=[] ::
DONE svclatch 0.6s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE termcolor 0.6s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE testf 0.5s :: TESTS: ran=1 pass=0 fail=1 failed=[testf] skipped=[] ::
DONE textedit 0.6s :: TESTS: ran=0 pass=0 fail=0 failed=[] skipped=[] ::
DONE trackpad 0.9s :: TESTS: ran=1 pass=2 fail=0 failed=[] skipped=[] ::
DONE trash 0.5s :: TESTS: ran=0 pass=0 fail=0 failed=[] skipped=[] ::
DONE tste 0.7s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[tste] ::
DONE typematic 0.6s :: TESTS: ran=0 pass=0 fail=0 failed=[] skipped=[] ::
DONE u20c 0.4s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE u3 0.4s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE una 0.4s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[una] ::
DONE unafs 0.5s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[unafs] ::
DONE unafsgrow 0.6s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE unafsroot 0.9s :: TESTS: ran=1 pass=2 fail=0 failed=[] skipped=[] ::
DONE usbnet 5.5s :: TESTS: ran=1 pass=2 fail=1 failed=[usbnet] skipped=[] ::
DONE usbnet7 10.6s :: TESTS: ran=1 pass=0 fail=1 failed=[usbnet7] skipped=[] ::
DONE usbnetftdi 5.7s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE usbnetrx 5.6s :: TESTS: ran=1 pass=0 fail=1 failed=[usbnetrx] skipped=[] ::
DONE usbstor 0.6s :: TESTS: ran=1 pass=0 fail=1 failed=[usbstor] skipped=[] ::
DONE users 0.5s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[users] ::
DONE usersready 0.7s :: TESTS: ran=1 pass=2 fail=0 failed=[] skipped=[] ::
DONE uvc 0.4s :: TESTS: ran=1 pass=2 fail=0 failed=[] skipped=[] ::
DONE verdicts 0.6s :: TESTS: ran=1 pass=0 fail=1 failed=[verdicts] skipped=[] ::
DONE videoplayer 0.7s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[videoplayer] ::
DONE volumes 0.6s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[volumes] ::
DONE vugres 0.5s :: TESTS: ran=0 pass=0 fail=0 failed=[] skipped=[] ::
DONE wcdlatch 0.6s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE wcdskip 0.6s :: TESTS: ran=1 pass=2 fail=0 failed=[] skipped=[] ::
DONE wifi 0.4s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[wifi] ::
DONE window 1.0s :: TESTS: ran=1 pass=0 fail=0 failed=[] skipped=[window] ::
DONE windowcap 0.6s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE windowlist 0.9s :: TESTS: ran=1 pass=0 fail=1 failed=[windowlist] skipped=[] ::
DONE winmemory 0.9s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE winresize 0.6s :: TESTS: ran=0 pass=0 fail=0 failed=[] skipped=[] ::
DONE winsnap 0.6s :: TESTS: ran=0 pass=0 fail=0 failed=[] skipped=[] ::
DONE winx 0.7s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE winx3 0.7s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE wmdirect 1.1s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE wxaudit 0.6s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE x86bind 0.6s :: TESTS: ran=1 pass=1 fail=0 failed=[] skipped=[] ::
DONE xhcimedia 1.6s :: TESTS: ran=1 pass=1 fail=1 failed=[xhcimedia] skipped=[] ::
LIST-END
```

Not in this build (`ran=0`, the `no such fixture` answer): activity ahciw ce clickband clickroute crystal ctrldecline dmgovlp dock dockpin dragperf dragwedge fileopen hittest install instgui inwedge kblit keplerlog kgr kvblank loginwindow logout movevacate name ptrdead quarryops rootacl selfinstall shotregion srcextract textedit trash typematic vugres winresize winsnap.

## Files

- `serial.log`: the whole COM1 capture (16550, under 2 MB, not split). Read it with awk.
- `progress.txt`: the driver's per-fixture record. `names-used.txt` holds the names typed.
- `run.sh`, `drive.py`, `drive2.py`: the launcher and the two door drivers, as run.

## Notes for the reader

- This is a QEMU witness, not metal. Many reds are QEMU's model: no gmux/SMC/BCM4331/BT radio, no Kepler takeover, and an emulated FT232 with no tap prefix. Several reds touch the user's home or volume (prefs, settings, shot, rawcore, filetype, player): the store is the emulated SD card's FAT, and a create on `/home/una/Desktop` returned `Unsupported`. Rows are filed for every FAIL as the brief says; each row's status says which class it looks like, but no row is triaged further than that.
- The kernel clock stamped every fixture line `[07:14:21Z]` for the six minutes of fixtures, so the LOGTS stamps did not advance after SNTP synced (`offset_ms=29000`). `tests clock` FAIL may be the same thing.

