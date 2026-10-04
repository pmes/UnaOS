# FLIGHT 20 — image 13 (rmbp12flight20, hw-rmbp@26c35637: wave 7 + the fix wave), 2026-10-04, main bench

Capture: `f20-boots.log` (15546 lines, two boots: fresh card → una's desktop → tests; a forced reboot after `tests net` locked the input → the login screen).

## Peter, verbatim (the glass)
- "i could see the task bar at the bottom" (Settings → Display, slider at far left)
- "tones and i double clicked the wav file and it played!!!!! there's a bit of popping like there's an analog switch slamming"
- "lumen crashes"
- "i had to reboot which worked fine because net.bin test locked out input"
- "ls.lnx worked"
- "install is an unknown command"

## 1. Green
- **AUDIO7 — SOUND ON THE rMBP, FIRST TIME**: `tests hda` makes a TONE (three runs: PASS, PASS, PASS; `[hda] gpio … data=0x08 speaker_bit=1 … drive_oe=0x0a`), and a double-clicked WAV PLAYS. Flights 13-19 were a screech. The popping: the speaker-enable GPIO (`gpio data=0x08` set before each run and `restored` after — "an analog switch slamming") toggles the class-D amp around every run; it should be raised once and stay while audio is in use (AUDIO8, small).
- **BRIGHTFLOOR**: `:: BRIGHTFLOOR: floor=1 set0_on=1 load_clamped=1 reset_ok=1 full=1 mono=1 driver=gmux -> PASS`; `[backlight] level=12 reg=767 readback=767 max=1023 on=1 driver=gmux via=login`; the slider at far left kept the task bar visible.
- **Boot 2 → the login screen** (`stage=login-screen … why=store-has-users`), "worked fine".
- **LINUXABI3**: `path=/apps/ls.lnx exit=0 syscalls=25 enosys=[] ms=59 -> PASS` (flight 19: hung 50 s after one syscall).
- **R80 partly**: the censuses are gone from the boot; `:: BOOT: firmware->loader=13679ms loader->desktop=10324ms total=24004ms lines=2615 ::` (boot 2: 23097 ms, 2584 lines).
- Installer path again; VEIN registers (`:: VEIN: registered=4 provider=echo … -> PASS` on the first `bg`).

## 2. Findings
- **R80 NOT DONE (QUIETBOOT)**: the image's own fixture says so — `:: QUIETBOOT: lines=2615 bound=250 … -> FAIL` — ten times over its bound; Peter: "it is still running a bunch of stuff at boot". Still printing before the desktop: HOMESOIL ×3, PRTSCR-DIR-FIX ×2, FTDI ×2, X86BIND, WXAUDIT ×2, WINX-3, WCDLATCH, WALLPAPER, USERSREADY/USERSMOUNT, and ~2500 lines of bring-up prose (PCI/ACPI/gen7/igpu/kepler censuses). The sweep moves the fixtures behind `tests`, the censuses behind `census`, and the bring-up prose behind a `UNAOS_BOOTLOG`-style knob — the boot prints its stages and refusals only.
- **LUMENCRASH**: `lumen` → `:: BGRUN: bg /apps/LUMEN.BIN — loaded 12672 bytes, entry 0x10000000000, pid=45 …` → `:: RING-3 FAULT: task 'bg-user' KILLED — vec=13 err=0x0 rip=0x1000000003f cr2=0x0 ::` — a #GP at entry+0x3f, twice (pid 45, 46): the program faults in its first instructions (an SSE/AVX instruction with the FPU/XSAVE state not enabled for ring 3, a misaligned stack at entry, or a syscall shape the shim rejects) — the cloud disassembles LUMEN-X86.ELF at +0x3f. (LUMENAPP in the boot-21 wave replaces LUMEN.BIN; the fault at +0x3f likely moves with it unless the cause is the entry ABI.)
- **FOCUSDEAD (was "NETLOCK" — the first reading was wrong, the wire corrected it)**: `tests net` did NOT hang: `:: ENTROPY: … -> PASS`, `:: RESOLVE: name=api.anthropic.com einval=1 -> SKIP reason=no-answer`, `:: TESTS: ran=1 pass=1 fail=0` — one second. Then `[gui] app-exit t=504s dur=1s wedged=false`, and from there every key Peter typed reached the router and went NOWHERE: `EHCI-HID: KEY: 'b' … [quarry] key_route key=0x62 focus=0 took=0`, `'g' … focus=0 took=0` (he was typing `bg …`), no `[midden] cmd=` ever again, 257 lines of normal rollups until the forced reboot. The focus was left on window 0 (nothing) after the test runner's window exited: keys are routed, no window takes them, the shell never sees them. Mechanism: the app-exit path does not hand focus back to the shell/console; a focus of 0 after an exit must fall back to the last live window. Not the network.
- **USBNET6 still red**: `[usbnet] link=up speed=1000 mac=9c:69:d3:28:6e:f4 rx_ctl=0x02aa rx_ok=0 rx_drop=0 rx_pad=0` and `:: USBNET6: … -> FAIL`: the chip no longer drops, and NOTHING is received at all (rx_ok=0): with rx_ctl=0x02aa (AB|AM|SO?) the chip's RX is started but no frame arrives — the bulk-in URB may never be re-armed after the first completion, or the MEDIUM_STATUS_MODE receive enable is missing. No lease, no SNTP, no clock from the net.
- **tests fail=9**: `failed=[quietboot,helpdoc,vein,windowlist,hdaboth,hda220,hda2,lumen,usbnet]` — quietboot/usbnet/lumen above; `vein` = `:: VEIN: registered=-17` on the SECOND `bg` (EEXIST: already registered — the fixture should accept it); `VEINBUS: mode=scratch kats=19/19 … live=absent -> FAIL` (no live daemon at test time); windowlist `rows=10 live=3 …`; hdaboth/hda220/hda2 (the second DAC still silent/odd per run); helpdoc again.
- **install unknown**: the verb is `#[cfg(feature = "installdemo")]` (shell.rs:3062/5606) and `UNAOS_INSTALLDEMO=1` is not on the line — the cloud's read list named the verb without its knob. Not a bug; a knob decision for boot 21 (SELFINSTALL2 dry-run).

## 3. Owed
QUIETBOOT (R80, the real sweep: fixtures → tests, censuses → census, bring-up prose → a knob; bound 250), LUMENCRASH (#GP at entry+0x3f), FOCUSDEAD (focus falls back to a live window after an app exit), USBNET7 (rx_ok=0 with rx_ctl=0x02aa: the bulk-in re-arm / RX enable), AUDIO8 (the amp GPIO stays up while audio is in use — the pops), the vein EEXIST fixture, helpdoc, windowlist, hda2/hdaboth/hda220, UNAOS_INSTALLDEMO on the boot-21 line if the dry-run is wanted.
