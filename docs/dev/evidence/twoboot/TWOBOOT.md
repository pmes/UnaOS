# TWOBOOT — the leg's first (and only) run on exec-rmbp-twoboot (tree d4f10a15 + this arc, dirty sha d4f10a15-d9c185d)

Command: `cd unaos && ./arroyo twoboot` (2026-10-12 23:52Z, cloud seat, QEMU 8.2.2 TCG, machine=pc-q35-8.2 auto-selected).
STOPPED by the seat's brief amendment ("stop all QEMU … including the run1") during boot 2, before boot 2's 60 s cap: the
script's own verdict line never printed. The verdict below is the leg's judge over the two captures, re-read with
`./arroyo twoboot --judge target/twoboot-1.log target/serial.log` (no boot):

```
TWOBOOT: boot-2 last line: [23:54:06Z] [wallpaper] probe try=5 found=0 on=worker cpu=2 ms=5
TWOBOOT: boot1=desktop-built boot2=no-user@users-load wall_s=- -> FAIL
```

## Run lines (target/twoboot run log)
```
TWOBOOT: line=image-line.env @cccb7ae5 knobs=43 dropped=1(UNAOS_WIFI_FW_PATH) added=UNAOS_LOGIN,UNAOS_USBSERIAL caller_knobs_cleared=1 machine=pc-q35-8.2 caps=60/60s grace=5s
TWOBOOT: boot 1 — build + boot on a fresh volume, cap 60s (output: target/twoboot-1.out; typist: target/twoboot-typist.log, QMP :4470)
TWOBOOT:   [typist] connected :4470; waiting up to 60s for '[login] installer: first-user form open'
TWOBOOT:   [typist] form open at +7.5s; settling 4s
TWOBOOT:   [typist] typed name + Tab + password + Tab + retype + Enter (32 keys) at +15.4s
TWOBOOT:   [typist] the screen took a key
TWOBOOT: boot 1 arroyo rc=1 typist rc=0 qemu_wall=22.7 verb_wall=69.4 kernel.elf=df83b60b09242277 ⚡ kernel features: witness,selfhost,wcg-paygo,wcdvalve,logts,ehcihid,kbdwit,smc,sdw,sdhcblk,bt,btc,login,smolnet,wifi,wifi2,wifi3,wifi4,nvidia-kepler,nvidia-kepler-takeover,noaspm,nvidia-kepler-ce,wc,nvidia-kepler-vblank,deadman,linuxabi,busreg,lumen,netring3,selfdiag,lidsleep,videoplayer,intel-ivb,
TWOBOOT: boot 2 — the same image on the same volume (target/twoboot-vol.img), cap 60s (output: target/twoboot-2.out)
```

## Boot 1 (target/twoboot-1.log, 912 lines; sidecar mode=fast completion_at=17.0 qemu_wall=22.7 cap=60)
```
[23:53:35Z] [boot] step=users-load end ms=17 budget_ms=249 over=0 blocks_read=10 blocks_written=0 cmds=10 store=fat via=sdhc users=0
[23:53:35Z] [users] seat store=fat:/USERS.DAT (no UnaFS root: the FAT is /) ::
[23:53:35Z] :: FIRSTBOOT: stage=installer root=locked users=0 desktop_ignited=false why=store-loaded -> coherent ::
[23:53:35Z] [login] installer: first-user form open (R100: name, password, retype; that user is the administrator; root is locked; the desktop ignites for them)
[23:53:35Z] [login] key taken by the screen (the first of this open — LOGIN13/R63: while the screen is up it is the only thing taking input; no typed byte is ever printed)
[23:53:35Z] [login] installer: create-user user=una id=15755182 home_created=true
[23:53:35Z] [login] session open user=una role=admin (R100: the first user is the administrator; root is locked)
[23:53:35Z] [boot] services up first=usbnet at=9674ms held_starters=7 (R86: nothing but the setter / the login dialog ran before this)
[23:53:35Z] [boot] phase=desktop from=setter why=session at=9675ms pre_windows=0 pre_starts=0 pre_valve=0 (R86: the furniture first, then the services)
[23:53:35Z] [login] desktop bare: furniture=none (R88) why=session — no console, shell or STAT opens itself; the user opens what they want
[23:53:35Z] :: FIRSTBOOT: stage=desktop root=locked users=1 desktop_ignited=true why=user-created -> coherent ::
[23:53:35Z] [desktop] built at=login ok bar_ms=61 battery=absent dock=painted pins=7
… last line: [23:53:35Z] [usbnet] rx kick n=1 pending_ms=2000 owed_ms=0 xfers=0 rx_ok=0 resets=0
```

## Boot 2 (target/serial.log at the stop, 757 lines; same kernel.elf, UNAOS_FATIMG=target/twoboot-vol.img)
```
[23:54:06Z] [boot] step=users-load begin at_ms=2366 budget_ms=249
[23:54:06Z] [boot] step=users-load end ms=13 budget_ms=249 over=0 blocks_read=9 blocks_written=0 cmds=8 store=fat via=sdhc users=0
[23:54:06Z] [boot] step=stage-resolve begin at_ms=2548 budget_ms=927
[23:54:06Z] :: FIRSTBOOT: stage=installer root=locked users=0 desktop_ignited=false why=store-loaded -> coherent ::
[23:54:06Z] [login] installer: stage=installer (R77: R100: no user — the first-user form is the whole glass (that user is the administrator; root is locked); no desktop, no programs, no fixtures)
[23:54:06Z] [login] installer: first-user form open (R100: name, password, retype; that user is the administrator; root is locked; the desktop ignites for them)
[23:54:06Z] [boot] step=stage-resolve end ms=130 budget_ms=927 over=0 blocks_read=0 blocks_written=0 cmds=0 installer
… last line: [23:54:06Z] [wallpaper] probe try=5 found=0 on=worker cpu=2 ms=5
```

## The volumes, read host-side after the stop
```
sdcard.img: 'USERS   DAT' entries=0
twoboot-vol.img: 'USERS   DAT' entries=1
```

## Reading
- Neither flight-29 hang reproduced: boot 1 went first-user form -> typed user -> `desktop bare` -> `[desktop] built` -> services in 17 s
  (no SESSIONWEDGE on QEMU), and boot 2 never took the login-screen path, so RESOLVEHANG was not reached.
- Boot 2 FAILED for another reason the leg exposed: `users=0`, back to the first-user form. The store LOADED from the card
  (`via=sdhc`, 2.4 s, before the USB stick enumerates), while the first user's FLUSH landed on the stick (USERS.DAT is on the
  stick copy target/twoboot-vol.img, not on target/sdcard.img). `fs/users.rs` `store_via()` is re-asked at every mount (`try_load`, `flush`),
  so the rung moved from sdhc to global between load and write. On the metal shape (global=0, sdhc=1, ahci=1) the rung should not move
  unless something sets the global slot after users-load. Kernel, not this arc: reported, not touched.
- The leg's first draft kept the stick and assumed the store was there; twoboot.sh now deletes target/sdcard.img before boot 1, keeps both
  volumes, prints `store_on_volume=card|stick|none` after boot 1, and judges a boot 2 that reads `users=0` as `no-user@users-load`.
