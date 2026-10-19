# BP-boot — from the loader to the built desktop (read at 1719fe54)

## 1. What it is
- x86 and Orin: the UEFI loader (`unaos/crates/bootloader`) loads `kernel.elf` and hands a `BootInfo` with the MMU on. Pi 4: the GPU firmware jumps to `kernel8.img` at EL2; the kernel drops to EL1, maps, synthesizes `BootInfo` from the DTB (`unaos/crates/kernel/src/main.rs:25-77`).
- `kernel_main` (`unaos/crates/kernel/src/main.rs:80`) stamps BPACE rungs (`entry heap acpi calib smp sched pci-usb gui`), hands the panel to scheduled services, and the BSP joins the scheduler.
- ONE phase gate (`unaos/crates/kernel/src/boot.rs:65`) decides what may run: `Setter` (store unread, or no users) → `LoginScreen` (users exist, no session yet) → `Desktop` (latched by the first session).
- The desktop is an object BUILT when a session opens and torn down at Log Out (`unaos/crates/kernel/src/video/desktopbuild.rs:1-17`).
- The image's features are one file, `unaos/scripts/image-line.env` (51 exports); Pi and Orin have their own (`image-line-pi.env`, `image-line-orin.env`).

## 2. Invariants
| id | statement | anchor | held by |
|---|---|---|---|
| BP-boot-1 | `boot::phase()` is the one answer to "what may run"; pure atomics, callable under any lock; a build without `login` is `Desktop` from the first instruction. | `unaos/crates/kernel/src/boot.rs:9-28` `unaos/crates/kernel/src/boot.rs:65-91` | `tests installbare` (`unaos/crates/kernel/src/boot.rs:349`) |
| BP-boot-2 | `Desktop` is LATCHED by the first session open or the installer's `user-created`; Log Out never un-latches it. | `unaos/crates/kernel/src/boot.rs:149-170` | `[boot] phase=desktop` line, once per boot |
| BP-boot-3 | Services (net, usbnet, bt, wifi, hda, status, ring-3 probes, the WC-D valve) start only when `services_up()`: Desktop AND (furniture ready OR 1500 ms after the ignition). Starters ask `services_gate(name)`. | `unaos/crates/kernel/src/boot.rs:62` `unaos/crates/kernel/src/boot.rs:98-126`; R86, R88 | `[boot] services up held_starters=` line; twoboot |
| BP-boot-4 | Nothing runs at boot but the boot: fixtures REGISTER and run on the `tests` verb; the boot prints one `:: TESTS: deferred=<n> fire=tests ::`. `tests-at-boot` is a QEMU-lane feature, never on the image line. | `unaos/crates/kernel/src/tests.rs:1-12`; `unaos/scripts/image-line.env:13-17`; R77, R80 | `gates` row `image-line`; otherwise unheld |
| BP-boot-5 | The splash owns the glass from the GOP frame (x86 `unaos/crates/kernel/src/main.rs:233`) through the takeover (`unaos/crates/kernel/src/video/fbcon.rs:1791`) as a topmost chromeless row until the first screen is published; bounded 5000 ms, not while stage-resolve waits. | `unaos/crates/kernel/src/splash.rs:696-812` | `[splash] held_ms=… released_by=` line |
| BP-boot-6 | Before a session there is NO desktop: `strip::compose_all` paints no furniture; `desktopbuild::build` is asked at `login ok` or a store-less Desktop stage; `teardown` at Log Out. Boot 2's login screen builds nothing. | `unaos/crates/kernel/src/video/desktopbuild.rs:37-49` `unaos/crates/kernel/src/video/desktopbuild.rs:137`; `unaos/crates/kernel/src/fs/users.rs:3066-3071`; R93 | `:: DESKTOPBUILT: … -> PASS|FAIL ::`; twoboot boot 1 |
| BP-boot-7 | First boot: the first-user form (name, password, retype); that user is the administrator; root is locked (no login, no prompt). | `unaos/crates/kernel/src/video/login.rs:662` `unaos/crates/kernel/src/video/login.rs:1156`; R100 | twoboot boot 1 (`x86-twoboot-1.spec`) |
| BP-boot-8 | Later boots: the login screen IS the boot session. | `unaos/crates/kernel/src/video/login.rs:1968` | twoboot boot 2 |
| BP-boot-9 | Every boot step is bracketed `[boot] step=<n> begin … end ms=` with a budget, and says `OVER` every 5 s past it. | `unaos/crates/kernel/src/fs/bootstep.rs:115-147` `unaos/crates/kernel/src/fs/bootstep.rs:329-343` `unaos/crates/kernel/src/fs/bootstep.rs:531` | the wire (`OVER` line) |
| BP-boot-10 | x86 `login` (not `loginst`): stage-resolve waits at most 8000 ms for the serial console, then says `waited_ms=`. | `unaos/crates/kernel/src/fs/users.rs:1321` | `[boot] step=stage-resolve waited_ms=` line |
| BP-boot-11 | A knob joins or leaves the rMBP image only by a commit to `image-line.env`; a STATE cites it as `image-line.env @<sha>`. `UNAOS_LOGINST` is never on media. | `unaos/scripts/image-line.env:1-17` `unaos/scripts/image-line.env:27` | `gates` row `image-line` (`knob-parity.sh --line`); `./arroyo foldleg` |
| BP-boot-12 | Boot timing uses the free-running counter (rdtsc / CNTVCT_EL0), converted at print time; drop-NEWEST on overflow. | `unaos/crates/kernel/src/bootpace.rs:16-29` | BPACE ledger on the wire |
| BP-boot-13 | The loader's `KELF min= max=` line is the boot's identity for scoring a capture. | `unaos/crates/bootloader/src/main.rs:795`; `docs/dev/PLAYBOOK.md` §4.6 | bench scoring (unheld in-tree) |
| BP-boot-14 | Orin (`tegra`) leaves `kernel_main` early at `tegra_early_stop`; the shared GUI path below it is unreachable there. | `unaos/crates/kernel/src/main.rs:72-79` `unaos/crates/kernel/src/main.rs:190` | `./arroyo check` tegra leg |
| BP-boot-15 | Every image line names its oldest machine (`boot::OLDEST`, one const per board: rmbp12 2012, pi4 2019, orin 2023; `login` builds only, written in each line's tail comment) and the boot's first `[desktop] built` prints `:: OLDEST: board= year= image=<build sha> net=<absent|present> -> BOOTS ::`; BP-bar-1. | `unaos/crates/kernel/src/boot.rs:410-459` `unaos/crates/kernel/src/video/desktopbuild.rs:122`; `unaos/scripts/image-line.env:70` | twoboot boot 1 (`x86-twoboot-1.spec`) |
| BP-boot-16 | x86 kernel slots (ABIMAGE B683, BP-bar-8): the loader boots `KERNEL.NEW` only when `STAGE.REC` says `stage=1` and `SHUTDOWN.REC` says `why=user` (written only by `power::prelude`, the user's Restart/Shut Down), marking `why=trial` BEFORE the jump; a trial that never reached DESKTOPBUILT PASS falls back to `KERNEL.ELF` on the next boot. The decision is ONE pure fn both the loader and the kernel compile; the loader writes only these two records. | `unaos/crates/kernel/src/install/abslot.rs:118`; `unaos/crates/bootloader/src/main.rs:709` `unaos/crates/bootloader/src/main.rs:1584`; `unaos/crates/kernel/src/install/abimage.rs:85`; `docs/dev/OS/10_INSTALL/ab-image.md` | `tests abimage` (`:: ABIMAGE: decide=4/4 …`); `[loader] slot=<ELF or NEW> why=` line |
| BP-boot-17 | x86 kernel hash (KELFHASH B709, BP-bar-4/8): the builder writes `KELF.SHA` (`sha256=… file=KERNEL.ELF slot=A kelf=0x…`) beside the kernel; the loader sha256s the bytes it read and checks the row for that slot BEFORE the jump. A mismatch reads the other slot; both bad halts with `KELF: hash mismatch slot=A,B — reflash`; no record, no row, or a row for another image (`kelf=` differs) jumps `unverified` — never a lockout (BP-bar-1). The record and the check are ONE pure file the loader and the builder compile; the verdict rides `BootInfo::kelf`. | `unaos/crates/bootloader/src/kelfsha.rs:205` `unaos/crates/bootloader/src/main.rs:1717` `unaos/builder/src/main.rs:1229` `unaos/crates/boot-info/src/lib.rs:322` | `tests kelfhash` (`:: KELFHASH: slot=A hash=ok … -> PASS ::`); `[loader] kelf slot= hash= why= sha=` and `[boot] kelf slot= hash= sha=` lines; the builder's unit test |
| BP-boot-18 | Civil time (TZLOCALE B701): the anchor is UTC; a zone is applied only when a second is printed. ONE zone table (`clock/tz.rs`, fixed DST rules, UTC = index 0) and ONE locale (Principia's `system.locale.tz` / `.region` / `.first_day`, held in atomics a painter reads lock-free); every named painter prints through `locale::fmt_time` / `fmt_date`. The `[clock]` boot line keeps `civil=<counter>` and adds ` at=<local ISO> tz= off_min= dst=`. | `unaos/crates/kernel/src/clock/tz.rs:68` `unaos/crates/kernel/src/clock/tz.rs:167` `unaos/crates/kernel/src/clock/locale.rs:116` `unaos/crates/kernel/src/clock/locale.rs:214` `unaos/crates/kernel/src/arch/x86_64/clockcore.rs:283` | `tests tzlocale` (`unaos/crates/kernel/src/tests.rs:801`, `:: TZLOCALE: … dst_edges_ok=8/8 … -> PASS ::`) |
| BP-boot-19 | The A/B image's face (UPDATEUI B726, BP-bar-8): Settings > About > System image reads `abimage::status` — the loader's KELF verdict (`tests::kelf_kept`), `STAGE.REC` plus the kernel-only note `UPDATE.REC` (`src= sha= at= kelf= [fell=]`, written before `STAGE.REC`, read only when its `kelf=` matches, never read by the loader, dropped by unstage and the promotion), and a SET-ASIDE image (no stage, `why=fallback`, `KERNEL.NEW` present; `fell=<boot n>` written once by `settle` at the first DESKTOPBUILT after the fallback). A pane stage is `stage_from` = the `stage` verb's administrator gate + `stage_image`'s body; nothing restarts on its own. | `unaos/crates/kernel/src/install/abimage.rs:47` `unaos/crates/kernel/src/install/abimage.rs:373` `unaos/crates/kernel/src/install/abimage.rs:409` `unaos/crates/kernel/src/install/abimage.rs:498` | `tests updateui` (`unaos/crates/kernel/src/tests.rs:2265`, `:: UPDATEUI: listed=1 staged=1 refused=1 text=ok -> PASS ::`); `[update] ui stage from= sha= -> staged` or `-> refused:<why>` |

## 3. Seams
- `boot::phase/services_gate` are asked by: `wm::create_inner`, every service starter, the WC-D valve (`unaos/crates/kernel/src/boot.rs:17-21`).
- `boot::session_opened` is called only from `login::close_into_session` (`unaos/crates/kernel/src/video/login.rs:749`); `ignite` also from the installer advance.
- `desktopbuild::build` is called from login and `users::stage_publish` (`unaos/crates/kernel/src/fs/users.rs:3048`); on x86 it runs on the device-service pass, elsewhere inline (`unaos/crates/kernel/src/video/desktopbuild.rs:42-48`).
- The splash calls `wm::splash_open`/`set_modal_top` (video.md BP-video-5).

## 4. Wire, in order (a healthy x86 first boot)
`KELF min=… max=…` → `:: SPLASH: stage=gop …` (`unaos/crates/kernel/src/splash.rs:834`) → BPACE rungs → `:: SCHED-X86 PLACE: …` (cores.md) →
`:: SPLASH: arch=… painted` (`unaos/crates/kernel/src/splash.rs:760`) → `[splash] hold OPEN` (`:781`) → `[keyboot] root=unkeyed hold=none` | (keyed: `[keyboot] hold OPEN` + `[login] unlock form open` … `[keyboot] root unlocked` + `[keyboot] hold released held_ms=`) → `[boot] step=users-load|root-mount|stage-resolve …` →
`[loader] kelf slot=A hash=ok …` → `KELF min=… max=…` → `:: SPLASH: stage=gop …` (`unaos/crates/kernel/src/splash.rs:834`) → BPACE rungs → `:: SCHED-X86 PLACE: …` (cores.md) →
`:: SPLASH: arch=… painted` (`unaos/crates/kernel/src/splash.rs:760`) → `[splash] hold OPEN` (`:781`) → `[boot] step=users-load|root-mount|stage-resolve …` →
`[splash] held_ms=… released_by=` → `[login] installer: first-user form open` (boot 1) | `[login] boot 2: …` + `[login] screen open …` (boot 2) →
`[login] session open user=<n> [role=admin]` → `[boot] phase=desktop from=<setter|login-screen> why=session …` (`unaos/crates/kernel/src/boot.rs:161`) →
`[boot] services up first=… held_starters=…` + `:: BOOT: phase=desktop … login->desktop=…` (`unaos/crates/kernel/src/boot.rs:132-142`) → `[desktop] built at=login ok bar_ms=…` (`unaos/crates/kernel/src/video/desktopbuild.rs:118`).

## 5. Knob families on the rMBP line (`image-line.env`, one export per line)
| family | knobs |
|---|---|
| session | `LOGIN`, `INSTALLDEMO` |
| video / GPU | `WC` (required by video), `VIDEO`, `KEPLER`, `KEPLER_TAKEOVER`, `KEPLER_CE`, `KEPLER_VBLANK`, `IVB`, `IVB3D`, `IVB3D_R8`, `IVB3D_BLIT`, `IVB3D_R9`, `GMUX_IGD`, `BEAM`, `WCG_PAYGO`, `WCDVALVE`, `NOASPM` |
| storage | `AHCI`, `AHCI_WRITE`, `UNAFS`, `AHCIROOT` |
| net / radio | `USBNET`, `NETRING3`, `WIFI`, `WIFI2-4`, `WIFI9`, `WIFI10`, `BT`, `BTC`, `UNAOS_WIFI_FW_PATH` (media) |
| ring 3 / apps | `SELFHOST`, `LINUXABI`, `BUSREG`, `LUMEN`, `STRIA`, `QUARRY`, `FACET` |
| power / audio / camera / irq | `SMC`, `LIDSLEEP`, `HDA`, `HDATONE`, `UVC`, `IOAPIC` |
| instruments | `WITNESS`, `LOGTS`, `DEADMAN`, `SELFDIAG`, `FTDIRX` |

## 6. Ruled, unbuilt
- R128 (the start screen): STARFIELD paints the desktop wallpaper (video.md). Owed by the STARFIELD fold (`7f4c0df5`): the boot splash's three shards (`unaos/crates/kernel/src/splash.rs:17-35`) and the login backdrop do not yet show the scene.
- rmbp-ledger B6: cross-arch splash, call sites x86-gated (`unaos/crates/kernel/src/lib.rs:193`).

## 7. Read further
- `docs/dev/OS/01_BOOT_HAL/bootloader_spec.md`, `bootpace.md` — the loader contract and BPACE.
- `docs/dev/OS/10_INSTALL/FLIGHT35-INSTALL.md` — the installer on the image.
- `docs/dev/OS/10_INSTALL/ab-image.md` — the two kernel slots and the loader's decision (BP-boot-15).
- `docs/dev/evidence/foldbuild/IMAGE-LINE.md` — each knob's derivation.
