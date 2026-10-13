# RESOLVEHANG2 (rmbp-ledger B531, with STOREMOVE B530) — evidence

Seat: cloud, branch exec-rmbp-resolvehang2, cut from cd59927b. R107: read, design, write, ONE compile, no boot.
Compile: `cargo +nightly check --release --target ../../x86_64-unaos.json -Z build-std=core,compiler_builtins,alloc
-Z build-std-features=compiler-builtins-mem -Z json-target-spec --features ehcihid,kbdwit,sdw,sdhcblk,smolnet,sdwrite,wc,login,loginst,unafs`
in unaos/crates/kernel -> rc=0 (no warning in the three touched files). Target dir deleted.

## 1. The cause — read from code with boot 2's path in hand

Boot 2's path (store has una): `users::service` (fs/users.rs:1236) -> users-load -> root-mount/assoc-seed ->
stage-resolve (`resolve_tail`, the old :1267 block) -> `stage_resolve` (:3049) -> `stage_publish(Desktop,
"store-has-users")` -> back in `stage_resolve`, `boot2 && DESKTOP_IGNITED` (boot 1's create-user form proved the
glass is up at this point: `sub=create-user-form at_ms=3401` on f30-boot1.log) -> `screen_boot2` (:3100) ->
`login::open_boot2` (video/login.rs:1964) -> `open()` -> `open_as(State::Open)` (:669) ->
`FORM.lock().windowed = true; repaint();` (:707-708) ->

**video/login.rs `repaint` (:490) takes `let f = FORM.lock();` at :491 and holds it to `drop(f)`; on the log-in form
(State::Open — the only branch that does not return early) it calls `power_row(px)` at :585, and `power_row`
(:2461) begins `if !power_row_on() {` (:2462), and `power_row_on` (:2457) is `FORM.lock().state == State::Open`.**
`crate::sync::Mutex` (sync.rs:18) is a non-reentrant spin lock: the stepping task spins on the FORM lock it
already holds, forever, in `lock()`'s slow path. That task is the main loop's device-service pass — the same pass
that runs `xhci.service_ftdi()` (main.rs:1664) ahead of `users::service` (main.rs:1704) — so the FTDI is never
brought up and the hang is silent on the wire by construction. The glass keeps the last composited frame: the
splash with stage-resolve's word, "Resolving the session".

Why boot 1 passes: boot 1's screen is `open_as(State::CreateUser)`; `repaint`'s CreateUser branch (:499-522) drops
`f` and returns before `power_row`. Every WINDOWED paint of the log-in form deadlocks (boot 2, Log Out's reopen,
the lock screen); the headless form returns at :492-493 before anything, which is why the QEMU loginst fixtures (headless)
never caught it. Introduced by 12bfe0cc (DIALOG2 M1-M3, 2026-10-06, "the login power row"): `power_row(px)` inside
the guard and `power_row_on`'s own lock arrived in that one commit. Flight 29 was the first boot 2 after it.

**The fix is NOT in this commit**: video/login.rs is outside the brief's files (HEAD rule 3: "a fix that needs another
file: STOP and report the exact change; do not make it"). The exact change, three lines, line-neutral:

    video/login.rs:585    power_row(px);                     ->  power_row(px, f.state == State::Open);
    video/login.rs:2461   fn power_row(px: &mut [u32]) {     ->  fn power_row(px: &mut [u32], on: bool) {
    video/login.rs:2462       if !power_row_on() {           ->      if !on {

(`power_row` has exactly one caller, :585; `power_row_on` keeps its other caller, `power_tab` :2483, which holds no
guard.) Without it boot 2 still hangs — but now LOUD (section 3).

### The suspects, each ruled out by reading
* cf00acde's splash release (B524): kept, not undone. It was not the cause: boot 2 now dies BEFORE any release —
  `open_as` releases at its tail (:731), after the first `repaint`, which never returns.
* VFSTXCOST (B522, the 16-window RaSet): the only UnaFS reads on this path before the paint are
  `loginwindow::resolve` (video/loginwindow.rs:122) — `get_attr(/home/una, una:icon, KERNEL_PRINCIPAL)` ->
  vfs.rs:2133 -> `resolve_path` + `read_authz` (unafs.rs:1402, the kernel shortcut after one `read_inode`) +
  `get_attribute`, all inside ONE `with_unafs`, nothing re-entrant, and it returns (no attr -> initials). The RaSet
  is write-through coherent (RESOLVEHANG.md). Not where the step stops.
* FILETYPES2 (B513) stamp read: `boot80_root_and_seed`'s assoc-seed step runs BEFORE stage-resolve begins (users.rs:1259);
  the glass had moved past its word ("Writing file types") to "Resolving the session". The login-time stamp read is
  `assoc::owe` from `close_into_session` (login.rs:761) — after a login, which boot 2 never reaches.
* HOMEPRIV-shaped reads on /home/una: the one read is `loginwindow::resolve`'s above (KERNEL_PRINCIPAL; no owner
  lookup, no users lock). Ruled out.
* `nameindex::post_login` (B432), `loginitems::post_login`, `keyring::ring_login`: called only from `users::login`
  (users.rs:871-873), after a password is accepted. Boot 2 hangs before the screen takes a key. Ruled out.
* LOGINRENDER / QUEUESLEEP's xhci-poll: other tasks; the stepping task neither renders through `lag` nor claims xHCI
  inside stage-resolve. Ruled out.

## 2. Why flight 30's watchdog never painted (B510/B524's `step=stage-resolve OVER`)
The hang is inside stage-resolve with the splash still held (HOLD_WIN != 0), so `over_label` was reached in
principle. Reading cannot say which gate refused; the old panel path had three it could fail silently:
(a) the panel write went through `video::WRITER.try_lock()` and only blitted when `WRITER`'s geometry EQUALLED the
held surface's (`i.width == w && i.height == h`), else nothing and no line; (b) the text went through the TrueType
engine (`video::text::draw_with`, a bounded `TT.try_lock`), and (c) the surface write needed `HOLD_SURF.try_lock()`.
None of them is needed any more (section 3). The watchdog task itself exists from the first step's begin
(`watch_start` from `begin`, users-load at ~2.5 s; f30's lock rows show cpu1/cpu2/cpu7 live by then, so
`sibling_online_cpu` had a sibling) and runs to the first screen (`any || splash::holding()`).

## 3. The witness (x86, `login`; every new line named in the commit body)
* splash.rs: `takeover_blit` keeps ITS panel handle (the one that put the splash on the glass) once, in `PANEL`
  (lock-free, written once before `PANEL_LIVE`'s Release store). `over_label` calls `over_panel` FIRST: the band
  filled with SPLASH_BG and the line drawn with the static bitmap face (`font::draw_glyph_fb`, compile-time rodata)
  straight onto that handle — no `try_lock`, no `wm`, no heap, no TrueType. `PANEL_LIVE` lasts from the takeover until
  `hold_release` has RETURNED from `wm::close` (so a step hung inside the release still paints), and ends on a hold
  that never opened.
* bootstep.rs `poll`: the panel line is repainted every `PANEL_EVERY_MS` = 1000 ms with the live step's elapsed and
  its live `last` sub-step; the wire keeps its 5 s `OVER` cadence.
* users.rs: stage-resolve WAITS for the FTDI console (`ftdi::is_live`) or `RESOLVE_WAIT_MS` = 8000 ms from the wait's
  start, whichever first — across PASSES, never inside one (the pass that would bring the FTDI up is the same pass), so
  a hang in the step is on the live wire: `[boot] step=stage-resolve waited_ms=<n> for=console console=<up|absent>
  bound_ms=8000`. x86 + `login` + NOT `loginst` (the metal's set; the loginst fixture chain keeps its inline order).
  splash.rs `hold_service`'s 5 s bound holds while the wait is pending (`users::resolve_waiting`), so the splash and
  its watchdog stay up through the step.

## 4. STOREMOVE (B530)
`store_via` was re-asked at every mount (users.rs:2731). Now the first successful store mount (`try_load`) pins the
rung AND the volume (FAT `vol_id`<<32 | cluster count) for the boot: `[users] store pinned via=<rung> vol=<fp>`;
every later `store_via` returns the pinned rung (once, when the ladder has moved: `[users] store pinned via=<p>
ladder=<l> -> stays`), and `store_mount` refuses a mount on that rung that finds another volume:
`[users] store=<rung>:<fp> write-to=<rung>:<fp> -> refused (… ladder=<l>)`.

## 5. What the fold should see
* `./arroyo twoboot` on THIS commit alone: `TWOBOOT: store_on_volume=card` (STOREMOVE: boot 1's flush follows the pin
  to the card it loaded from), boot 1 `desktop-built` (its first-user form ~8 s later: QEMU has no FTDI, `waited_ms=8000
  … console=absent`), boot 2 `hang@stage-resolve` -> FAIL, the last lines `[users] stage-resolve sub=login-screen` then
  `[boot] step=stage-resolve OVER budget_ms=927 elapsed_ms=… last=login-screen cmds=…` every 5 s.
* With the three login.rs lines of section 1 applied: boot 2 prints `:: FIRSTBOOT: stage=login-screen … -> coherent ::`,
  `[boot] step=stage-resolve end … login-screen` and the COMPLETE `[login] screen open window=… modal=true power_row=1`
  -> `TWOBOOT: boot1=desktop-built boot2=console … -> PASS`.
* Metal boot 2 (with the login.rs lines): users-load ~2.5 s, then `[boot] step=stage-resolve waited_ms=~4700
  for=console console=up` right after `:: U2.5: FTDI console up ::`, the stage-resolve sub-lines, the login window.
  Without them: the same wire up to `sub=login-screen`, then the OVER lines on the wire, and on the panel under the
  splash word `step=stage-resolve OVER - Resolving the session is taking N.N s, budget 0.9 s, last login-screen, <n>
  commands`, N moving every second.
