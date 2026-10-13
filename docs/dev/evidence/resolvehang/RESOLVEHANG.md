# RESOLVEHANG (rmbp-ledger B524) — evidence

Seat: cloud, branch exec-rmbp-resolvehang off d4f10a15. NOT reproduced on QEMU: the seat withdrew the
two-boot allowance mid-arc (amendment: "Write the fix from the wire evidence you already have, type-check once,
do not boot anything. Peter boots the metal."). The partial build that had started for the reproduction lane
was stopped before any QEMU run. The diagnosis below is from flight 29's boot-1 wire and the code, not a boot-2 capture.

## The wire we have (f29-boot1.log, boot 1 only)
    [boot] step=stage-resolve begin at_ms=3560 budget_ms=927
    [lock] held-too-long name=wm.rs:30196 at=wm.rs:30207 by=9@cpu7 ms=72 waiters=0
    [boot] step=stage-resolve end ms=299 budget_ms=927 over=0 blocks_read=224 blocks_written=0 cmds=28 installer
    :: U2.5: FTDI console up (0403:6001, 115200 8N1) ::      <- at ~7.2 s, ~3.7 s AFTER stage-resolve
Boots 2 and 3: nothing on the wire; the glass held "Resolving the session" (stage-resolve's label) with NO
B510 OVER line under it.

## The difference between boot 1 and boot 2 inside stage-resolve
`users::stage_resolve` -> `stage_publish(st, why)`:
* boot 1 (users=0): st = Installer. `stage_publish` releases nothing; the first-user form is deferred
  (`CREATE_PENDING`) and opened later by `boot_session`, whose `login::open_as` releases the splash after
  its first paint (video/login.rs `open_as` tail).
* boot 2 (una in the store): st = Desktop, why = `store-has-users`. `stage_publish`'s tail ran
  `if st == BootStage::Desktop { crate::splash::hold_release("first-screen"); }` (fs/users.rs:3043 at
  d4f10a15) — on the device-service pass, INSIDE the live stage-resolve step, before any screen exists. The
  line's own comment says login screens release from `login::open_as`; the condition did not exclude
  boot 2's login-screen resolution.

`splash::hold_release` (splash.rs:792) first swaps `HOLD_WIN` to 0, then `wm::clear_modal_top` +
`wm::close` (video/wm.rs:2643: the table lock, `DrainBarrier::drain` — an IRQ-masked bounded spin
waiting out in-flight composites — `place`/`reclaim`, then `composite()`), all from the stepping task. Two
consequences, both exactly the flight-29 symptom:
1. `HOLD_WIN == 0` makes `splash::over_label` return at its first line, so the B510 watchdog (fs/bootstep.rs
   `poll`, a sibling-core task) can no longer put `OVER` on the panel, and `hold_service`'s 5 s bound has
   nothing to release: a stall from here on is silent on the glass by construction.
2. The step label is never repainted after the release (`bootstep::open("first-screen")` is a no-op once
   the splash is not holding), so the last frame on a Kepler panel whose compositor does not come back is
   the splash with "Resolving the session" — whatever the stepping task is blocked on (the window-table
   lock / drain / composite from the device-service pass, a context boot 1 never closed a window from).
The other suspects were read on this path and are not on it in a way that differs between boot 1 and 2:
VFSTXCOST's RaSet (fs/unafs.rs tail) is coherent by write-through (every slot mirrors every write of
the one mount device; a failed fetch caches nothing); seat_with's USERS.DAT read happens in
root_credential_ignition (before stage-resolve; boot 1 read the same path and found it absent).

## The fix (fs/users.rs, fs/bootstep.rs — same-line folds, line count unchanged)
* `stage_publish`: `hold_release("first-screen")` only when `why != "store-has-users"`. Boot 2's screen
  releases the splash from `login::open_as` after its first paint (or `hold_service`'s 5 s bound), the same
  as the setter and the first-user form. The splash stays held through stage-resolve, so the watchdog can paint.
* `stage_resolve` / `stage_publish`: one wire line per sub-step —
  `[users] stage-resolve sub=<store-read|stage-witness|published|desktop-build|login-screen|create-user-form|done> …`.
* `bootstep::poll`: the panel line is painted BEFORE the wire line (a stepping task wedged holding the
  serial sink stopped the old order at its `serial_println!`), and it now leads with
  `step=<name> OVER - <label> is taking …`.

## The FTDI door
Not moved. The FTDI is an xHCI device (drivers/xhci/mod.rs `service_ftdi`, ~:13567) brought up by the
enumeration pump; on the rMBP it answered at ~7.2 s, while users-load (SDHC, no xHCI) begins at ~2.5 s.
Binding it before users-load means holding the store load on USB enumeration of an optional dongle — a
boot-order change in main.rs/xhci outside this brief. The panel route (B510 watchdog, now live through
stage-resolve on boot 2 and panel-first) is what this arc ships.

## What the next boot 2 should show
Wire (when the door is up, replayed from the capture ring):
    [boot] step=stage-resolve begin at_ms=… budget_ms=927
    [users] stage-resolve sub=store-read why=store-loaded stage=desktop users=1 at_ms=…
    [users] stage-resolve sub=stage-witness why=store-has-users at_ms=…
    :: FIRSTBOOT: stage=login-screen root=locked users=1 desktop_ignited=false why=store-has-users -> coherent ::
    [login] installer: stage=login-screen …
    [users] stage-resolve sub=published why=store-has-users at_ms=…
    [login] boot 2: login screen deferred to the glass …   (or sub=login-screen when the glass is up)
    [users] stage-resolve sub=done why=store-loaded at_ms=…
    [boot] step=stage-resolve end ms=… login-screen
    [boot] step=first-screen begin …   then `[splash] held_ms=… released_by=first-screen` from login::open_as
Glass: the login window. If it still stalls, the panel reads under the splash word
`step=stage-resolve OVER - Resolving the session is taking N s, budget 0.9 s, last <sub-step>, <n> commands`
(or `step=first-screen OVER …` if the stall is in opening the screen).
