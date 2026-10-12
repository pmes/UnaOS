# FLIGHT 29 — image 22 on the UNAFS CARD IMAGE (rmbp12flight29, hw-rmbp@cccb7ae5), 2026-10-12, main bench

Capture: `f29-boot1.log` (pinned; boot 1 only — boots 2 and 3 put nothing on the wire). Medium: the 4.1 GiB card image. Knob line = `unaos/scripts/image-line.env` (corrected: 44 = image 21's) + `UNAOS_AETHER=1 UNAOS_MIDDEN=1`. Tree = merge21 (fourteen arcs: the flight-28 fix wave — QUEUESLEEP, LOGINRENDER, FILETYPES, RENDERWEDGE — AETHERMETAL M1, AETHERHOVER, the Una campaign's first three, RING3EXEC, VFSTXCOST). Stick out, dongle out.

## Peter, verbatim (the glass)
- "image 22 is ready we didn't check it thoroughly so there could be errors"
- "desktop ... mouse was working within 15sec" / "i tried opening shell but OS seems wedged" / "i think it's wedged again" / "stuck on resolving the session again"

## 1. What boot 1 proved before the wedge
`:: LOADER: … stages=firmware:11ms,gop:9ms,volumes:4ms,kernel:2653ms,elf:6ms,discover:5ms,jump:24ms`; `GPUBLIT: selftest=ok … -> blitter=gpu`; `:: BOOT: … loader->desktop=6428ms total=19533ms lines=138 steps=2 slowest=users-load:84`; the first-user form; `[users] adduser … password set … login ok user=una`; `[login] session open user=una role=admin`; `[boot] phase=desktop from=setter … at=20617ms`; `[login] desktop bare: furniture=none`. Nothing typed ran (the door never answered).

## 2. Findings
- **SESSIONWEDGE (boot 1)**: after `[login] desktop bare` at 14:35:19 the kernel printed NOTHING more — no `[desktop] built`, no `[jobs]`, no services, no compositor rollups — except three `[dock] press` lines at 14:35:43–48 (`app=console -> armed`, `-> launch`, `strip tiles=8 raised=none`) and then silence; the seat's `tests queuesleep` / `tests loginrender` never reached `[midden]`. Peter: the mouse moved for ~15 s (the sprite), the shell would not open, "wedged". Before login: `[lock] held-too-long` ×7 at 14:35:02 (unafs.rs:567 74 ms, users.rs:364 80 ms, wm.rs:30196/30381–30385 72 ms; `[lock] spin-wait name=sdhc.rs:997 … on=12@cpu1`). The desktop ignition on this image holds the render path and the serial sink — the flight-28 fix wave (QUEUESLEEP/LOGINRENDER/RENDERWEDGE) is where to look first.
- **RESOLVEHANG (boots 2 and 3, warm and cold)**: the splash stops at "Resolving the session" with NOTHING on the wire (the FTDI binds after stage-resolve; a hang there is silent by construction). Boot 1 of the fresh card passed this stage (`users=0`, the first-user path); every boot after it — with una in the store — hangs in `users::stage_resolve` on the login-screen path. Flight 26's "starting" hang, now with a step word. The second boot of any card that has a user hangs before the console.
- The correction of `image-line.env` (INSTALLDEMO/INSTGUI out, UVC in) stands; the build was green (banner-cert 52/0, pins MATCH).

## 3. Owed (the cloud's, in Peter's order)
RESOLVEHANG (the second boot of a card with a user hangs at stage-resolve — reproduce on a card with a users store; the users-store load / the login-screen path under this fold), SESSIONWEDGE (the desktop ignition holds the render path and the serial sink after `desktop bare`), both BEFORE anything else on this image — the whole flight-28 read list is owed again. A kernel build AND a two-boot metal-shaped leg in every fold.
