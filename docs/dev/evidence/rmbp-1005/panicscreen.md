# PANICSCREEN — B406 (MACPARITY row 37; R95 "the midnight red screen of death")

## Design (written before the build)

**Finding.** Flight 24 card 2 (`f24-boots.log`, `[12:32:44Z] === KERNEL PANIC ===` wm.rs:18092) — every fatal path
(`#[panic_handler]` main.rs, the CPL-0 #PF/#GP/`fatal_fault`/#MC arms and the #DF `panic!` in `interrupts.rs`) calls
`fbcon::panic_screen()`, which paints `PANIC_BG` (dark red) and ARMS `PANIC_MIRROR`, so every later line — the
panic text, then the other cores' `[wcser]`/`[pcih]`/`[wpace]` rollups — scrolls over the glass, and the core
ends in `hlt_loop()` until the power button. The panic text is on the serial wire and nowhere on disk.

**Seam.** Kernel — kernel-by-ruling (the kernel's own glass at its own death; no handler can run). One new file,
`video/panicscreen.rs`, x86_64; every shared file gets a one-statement same-line hook:
- `fbcon::panic_screen` → `panicscreen::seal()` (glass detached) and `panicscreen::draw()` after its fill;
  `fbcon::_print` → sealed: the line is CAPTURED for the log (panicking core only) and never painted;
  `PANIC_BG` becomes `PANEL_BG` (#1E1E1E, our dark neutral).
- `main.rs` panic handler → `note_panic(info)` first, `finish()` in place of `hlt_loop()`;
  `interrupts.rs` #PF/#GP/`fatal_fault`/#MC → `note_fault(..)` + `finish()`; #DF → `note_fault` (reason
  `double fault`, `stack overflow` when CR2 is under the current task's slab) then its `panic!`.
- `finish()`: the full text + the last 64 KiB of the flight recorder (`flight_recorder::panic_tail` — the
  FLIGHTRING seam: today the pinned head's end, FLIGHTRING swaps in the rolling tail) → `/var/log/panic-<n>.txt`
  when `/` is native UnaFS AND the panic came from an unmasked context AND the heap lock is free within 50 ms
  (else `[panic] log not written reason=<why>` — never hangs on a lock it can see); then a 10 s TSC countdown
  on the glass and `acpi_power::reboot()` (POWER's port, after `power_drain`). `UNAOS_PANIC_HOLD=1`
  (`panic_hold`) holds instead: the bench capture needs the machine up.
- Next boot (storage pass, after login): `/var/log/panic.last` → `[panic] previous boot stopped: <reason>
  log=<path>` and a DIALOG `The last session stopped unexpectedly.` [OK] [Show log] (fileview); marker retired.

**Milestones.** M1 the plain sealed screen + the hooks + finish/countdown/restart + the knob. M2 the log file and
the next-boot line + dialog. M3 `tests panicscreen`.

**Witness.** `tests panicscreen` → `:: PANICSCREEN: drawn=1 restored=1 log_path=/var/log/panic-<n>.txt
writable=<0/1> hold=<0/1> -> PASS ::`. A real panic: `[panic] screen=plain reason=<r> at=<file:line> hold=0`,
`[panic] log written path=/var/log/panic-<n>.txt bytes=<b>` (or `not written reason=`), `[panic] restart in 10 s`,
`[panic] restart via=acpi-reset`; the next boot `[panic] previous boot stopped: <reason> log=<path>`.

**Owed.** The stack walk (the x86 target has no frame pointers: RIP + the frame only); a log from a MASKED fault
path (#PF/#GP/#DF land on the screen and restart, their log says `reason=masked`: the block pump needs IRQs);
aarch64 keeps its own path (dark neutral backdrop, text on it, hlt).
