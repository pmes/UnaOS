# rmbp12 flight 11 — image 3: the 2026-09-17 folds, the rescues to come after (2026-09-22)

ONE image, ONE card, ATTENDED. Board dark, card in the rMBP's SD slot, FTDI on the host. Boot with ⌥ held and pick the card.
Tree: UnaOS-rmbp-esp-rmbp12flight11-20260922T1415Z-56bbe53 — built at hw-rmbp@56bbe53b (GATE8 10/11 rc=0; knoboff-witness rc=1 by design vs f8f8ce8c; banner-cert ok=42 noverdict=0).
Knob line = gate7's flight line + UNAOS_WIFI3=1 UNAOS_WIFI4=1 UNAOS_KEPLER_CTRL_ADDR=1 UNAOS_KEPLER_CTRLBIND=1 UNAOS_HDA=1 UNAOS_HDATONE=1 (recorded in the MANIFEST line).
⚠ UNAOS_WIFI3=1 is the DESTRUCTIVE prologue (bcm4331.md §5 risk 4): the d11 core is reset and the firmware-resident microcode is destroyed; only a successful upload restores a working radio. Peter's go 2026-09-17. macOS reloads its own firmware on the next boot; nothing persists.
data/: the five ELF twins are DROPPED (CARDROOT option 0) — `/` must list 16 entries.

## Pre-registered wire, in boot order (score each; absence of a REQUIRE is a finding)
1. `:: BOOTCLOCK: firmware->loader=<ms> …` — compare to flight 10's 12992 (same media class).
2. `:: FTDI-CAP: replayed=… lost=0` — the ring stays intact.
3. `[hda] census bdf=0:27.0 id=8086:1e20 class=04 sub=03 …` → `[hda] reset crst=1 statests=… codecs=[0]` → `[hda] codec=0 vid=XXXX:XXXX rev=…` (FIRST EVER read of this codec; Cirrus 0x1013 expected, not assumed) → one `[hda] node=0x..` per widget → `[hda] path … dev=speaker` → `[hda] walk … speaker_pin=0x..` → `:: HDA: … -> PASS ::` → `[hda] tone stream=0 lpib=0 -> N … bcis>=2 tag_ok=1` → `:: HDA-TONE: … -> PASS ::` AND a 1 s 440 Hz tone from the speakers. PASS with silence = a real finding (EAPD/amp; the walk printed them).
4. `:: wifi2: … ucode upload words=9938 … -> UPLOADED ::` (wifi3; a REFUSED here names why) → `:: wifi4: begin` → `radio-id raw=0x… mfg=0x17f expected-mfg=0x17f MATCH` → `radio-id verdict=VALID` → `radio-ver … vs 0x2059 MATCH|DIFFERS (ADVISORY)` → `phy-alive … verdict=PHY-ALIVE` → `wifi4: end ok=1 stage=phy … restore=MATCH`. Without UPLOADED: `:: wifi4: REFUSED reason=wifi3-upload-not-proven ::`.
5. `:: igpu-dpy: rung=00 name=census ok=1 …` → `rung=03 name=pp ok=1 pp_write=DECLINED why=pp-bits-uncited …` → `rung=03 name=pp SETTLE ms=210 … moved=0` → `rung=04 name=dpcd try=<n>/7 ok=1 dpcd_rev=0x11` (an early try failing and a later succeeding CLOSES the timing question; 7 identical failures reopen the channel question) → `LADDER highest=06/10 name=end ok=1 … pp_settle_ms=210 aux_port=DPA(0x64010) aux_div=0x0C8 dpcd_tries=<n>`. Compare on name=, rungs renumbered.
6. `:: kepler: ctrlbind begin …` → twelve `:: kepler: ctrlbind pbdma<i> target=<t> … witness=<latched|silent> …` (or `skipped reason=ctrladdr-readonly`) → `ctrlbind chan-restored … verdict=clean`. Twelve silent closes KF9; one latched = the campaign's first channel validate.
7. Boot ladder: `[wc-x] move-vacate … from=(8,34) … painted=true desktop=5/5 -> PASS`; `[clickroute] route hit=true deliver=true … desktop=true nofab=true deflect=true -> PASS` with `[clickroute] press at (2,36) … -> consume` and one `[clickroute] furniture deflect …`. `[dmgovlp] … adopt_stretch=0/4 -> FAIL` EXPECTED to persist (DMGOVLP cut, not in this image).
8. `/` (Quarry or `ls`): 16 entries — `APPS/ B43/ EFI/ apps/ boot/ volumes/ BLOCK.TXT GROW.BIN HELLO.BIN S8W.BIN SCRATCH.BIN SRC.SHA SRC.TGZ hello.txt kernel.elf readme.txt`; STAT/VUG/VUGC/VUGX/PULSE.ELF ABSENT from `/`, PRESENT under `/apps`.
9. `[quarry] open … face=noto20-aa cell=9x20` — Quarry's text in the anti-aliased face (glass: no staircase edges).

## Peter drives (each is a scored fixture)
- Open Quarry from the dock; click a row: `[quarry] press at (sx,sy) row=n kind=dir|file -> cd|select|launch` + `[clickroute] press … -> quarry deliver=0`. Flight 10 had `-> consume deliver=0` and nothing happened.
- Click the app name in the menu bar (the title box, x 34..91 on flight 10): `[clickroute] press at (x,y) band=menubar -> open deliver=0` then `[winmenu] open title=<name> …`; second press `-> closed`. Pulse's View menu WILL NOT appear (APPMENU: no menu verb in the ABI — Peter's call, not this image).
- Type `help` + Enter over the cable with a window focused: `[serialdoor] key=0x0d win_focus=<n> ring=… -> shell (the wire is a console)` then `:: [midden] cmd="help" -> TerminalOutput …`, and NO `[quarry] key_route key=0x0d focus=1 took=1`.
- `storm` (twice): ZERO `GATE STOLEN` / `REHOMED the render role` / `is DEAD and its singleton`; instead `:: [wcser] PRESENT-BANDED SPIN site=<1|2> waited_us=<n> on=shadow<n> -> GAVE-UP|RELEASED ::` on a collision and the desktop continuing on the SAME core; `[wpace] … spin=<n> wedge=<n>` per window. A GATE STOLEN with no PRESENT-BANDED SPIN before it = a different holder wedge (reopens PCIE-RP-RECOVERY §12.4).
- gmux: `gmux=MATCH` at the revert; if the panel blanks and stays, power-cycle (single-use media rule).

## Known / expected
- knoboff-witness rc=1 vs f8f8ce8c: by design (STEALMS immediates, PTRINSTALL2/3, RENDSTACK).
- `[dmgovlp] adopt_stretch=0/4 -> FAIL` persists. `gmux REFUSED aux-timeout` may still appear on the ladder's pre-switch control read (that is the control, not the verdict).
- KFBIND/KDHEAD/CE-LADDER lines as flight 10 (no change in this image).
