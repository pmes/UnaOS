# FLIGHT 21 — image 14 on the UNAFS CARD IMAGE (rmbp12flight21, hw-rmbp@f65c5cc9), 2026-10-04, main bench

Capture: `f21-boots.log` (4521 lines, one boot, ended by a clean `POWER: action=shutdown`). The medium: the dd'd card image (GPT p1 ESP + p2 UnaFS). Knob line = image 13's − VEIN + UNAFS.

## Peter, verbatim (the glass)
- "too much is still going on at boot. machine seemed locked up now tests quietboot failed"
- "lumen opened"
- "nethang failed"
- "hda passed"
- "storm failed"
- "time set correct!!!"

## 1. Green — the UnaFS root flies (R82)
- `:: UNAFSX86: root=unafs disk=sdhc blocks=131072 gen=1 home=/home -> PASS ::`, `[vfs] boot mount /boot = fat boot volume source=sdhc rw=yes`, `[vfs] apps mount /apps = fat … rooted=APPS` — the first boot with /home on a UnaFS partition; the installer path ran on it (setter → create-user → una's desktop at 18:10:17).
- **LUMENAPP / LUMENCRASH**: `lumen` (bare) → `:: LUMEN: start provider=echo model=reverse key=none transport=none ::` — "lumen opened" (flights 20: #GP at entry+0x3f).
- **AUDIO8**: `tests hda` ×3 → `:: AUDIO8: runs=3 plays=0 amp_up=1 amp_down=0 pops_bracketed=0 -> PASS :: boot_up=1 … holdoff_ms=5000 ramps=3/3` — ONE amp up, no restore between runs; "hda passed".
- **NETHANG (wire)**: `:: NETHANG: path=resolve held_locks=0 bounded=1 link=1 masked=1 ms=80 masked_hlt=3 capped=0 rc=no-answer -> PASS ::` and `:: TESTS: ran=1 pass=8 fail=0` — the wire says PASS where Peter read "nethang failed": either the glass showed a different verdict or he read `rc=no-answer`; the shell kept taking keys afterwards (FOCUSDEAD fixed: `tests` were run after it).
- **The clock**: "time set correct!!!" — NOT from the network: no `[dhcp] lease`, `[sntp] target=0.0.0.0 from=none`, `rx_ok=0`. The RTC carried flight 19's `date -s` (RTCCLOCK) and the boot read it: the clock is right because the RTC is right. The dongle still receives nothing (USBNET7 unflown: no `:: USBNET7:` line printed — `tests usbnet` was not run).
- `tests quietboot` names its top printers: `top=[kepler:395,gen7:390,igpu:69,00:51,KFBIND:38,KDHEAD:36,igpu-dpy:35,X200:22]`.
- Clean shutdown via the power menu: `:: POWER: action=shutdown windows_closed=6 flushed=0 hda_stopped=1 -> going ::`.

## 2. Findings
- **BOOT80** — CORRECTION (2026-10-05, B350 on exec-rmbp-boot80): the users store is on FAT and loaded at 18:07:44, before the root mount; the 60 s was FILETYPE's type seed: 54 UnaFS commits, each rewriting the whole refcount map one sector per SD command (~57000 commands). Fixed by dirty-leaf commits, 64-sector read-ahead and a one-transaction seed. Original reading kept below:
- **BOOT80 (the "locked up")**: `:: BOOT: firmware->loader=12746ms loader->desktop=80753ms total=93499ms lines=1327 ::` — boot 20 reached the desktop stage in 10.3 s; this boot took 80.8 s. The gap is between `:: PREFS: path=/.config/unaos/preferences.toml loaded=0 saved=0 ns=system -> PASS` at 18:07:53 and `:: PTRINSTALL:` at 18:08:41 / `stage=installer … why=store-loaded` at 18:08:56: ~60 s in which ONLY the one-per-second kepler vblank census printed — the kernel waited, silently, on the UnaFS side (the users store load / stage resolve on p2: no `[users] load` or USERSMOUNT line printed at all on this boot — the R80 sweep took those witnesses too, so the wait is unnamed). Mechanism to read: `users::try_load` / `stage_resolve` on a UnaFS root (a full-volume scan of 131072 blocks at the card's ~100-350 KB/s is 60-ish seconds). The splash held the glass meanwhile: "machine seemed locked up".
- **QUIETBOOT still FAIL**: `lines=1327 bound=250 census=0 -> FAIL` (boot 20: 2615). 1327 − the GPU recon prose (kepler 395 + gen7 390 + igpu 69 + KFBIND 38 + KDHEAD 36 + igpu-dpy 35 = 963) = ~360. The recon knobs on the line are the bench's call (R80): UNAOS_KEPLER_KFBIND, UNAOS_KEPLER_KDHEAD, UNAOS_KEPLER_CTRLBIND/CTRL_ADDR, UNAOS_IVB3D/IVB3D_R8 (gen7), UNAOS_BAR1WEDGE, UNAOS_SMCWALK, UNAOS_RTWIT, UNAOS_WITNESS's igpu census — the next line drops them (they are flight-8..12 instruments, every one flown) and the kepler vblank 1 Hz census goes behind `census`. "too much is still going on at boot".
- **STORMFAULT** — CORRECTION (2026-10-05, B351 on exec-rmbp-stormfault): the loader hypothesis below is REFUTED — the loader zeroed VUG.ELF's bss (ends at +0x3514); the fault at +0x56000 is the vug's blit running past surface slot 0 because user-vug derived its base as `_start - 0xb0` after EXECNAME's PT_NOTE moved the entry to 0xe8. Fixed via `__ehdr_start`. Original reading kept below for the record: `storm` → `:: STORM: launched 6/6 vugs ::` then SIX `:: RING-3 FAULT: task 'bg-user' KILLED — vec=14 err=0x6 rip=0x10000001845 cr2=0x10000056000 ::` — every VUG page-faults at the same instruction on a WRITE (err=0x6: user, write, not-present) to 0x10000056000 = image base + 0x56000: the program's bss/stack page past the loaded span is not mapped under the new ELF loader (EXECNAME's `.ELF` path / the 4 MiB window relink): VUG.ELF's data segment (or its stack) extends to 0x56000 and the loader maps only the file-backed pages. "storm failed". Flight 20's storm on the FAT card ran (the old `.BIN` loader path).
- **NETHANG read**: wire PASS vs glass "failed" — to resolve next flight: what did the shell print?

## 3. Owed
BOOT80 first (the 60 s silent wait on the UnaFS root; name it with a line, then fix it), the recon knobs off the line (the bench's call — proposed list above), STORMFAULT (the ELF loader's bss/stack mapping for VUG.ELF: write fault at base+0x56000), USBNET7 unread (`tests usbnet` next flight), NETHANG's glass verdict, the kepler 1 Hz census behind `census`.
