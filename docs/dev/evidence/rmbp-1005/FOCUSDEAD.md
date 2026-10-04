# FOCUSDEAD (B327) — M1: the premise is refuted by the wire and the code; the arc is re-aimed

Branch `exec-rmbp-focusdead`, cut from 4e48ab03. Source: `docs/dev/evidence/rmbp-0915/flight20/f20-boots.log`
(boot 20, lines 10011..10198), read with `awk 'index($0,"[tag]")'`.

## Finding as filed
After `tests net` passed (`:: TESTS: ran=1 pass=1 fail=0`, `[gui] app-exit t=504s dur=1s wedged=false`),
every key showed `[quarry] key_route key=0x.. focus=0 took=0` and no `[midden] cmd=` followed; the filing
read this as "focus left on window 0, nothing takes the key, the shell never sees it".

## M1 — what the wire and the code actually say
1. `focus=0` IS THE SHELL. `video/wm.rs` `focus_asid()`: "the ASID that currently holds focus (`0` = the
   shell)"; `FOCUS_ASID` is cleared to 0 by `focus_release`, "the close paths' narrow drop-to-shell". The
   close paths already hand focus to the shell — that is what 0 means.
2. `took=0` is Quarry declining the key, after which `wc_route_event` passes it on to the shell. The
   SERIALDOOR comment in `arch/x86_64/syscall.rs` (`wc_route_event`) records the shape from flight 10:
   "`focus=0 took=0` with `[midden] cmd=` 19 ms later".
3. The same pair appears before EVERY command that worked on boot 20. The keys of `tests net` itself
   (15:38:50..53) are all `focus=0 took=0`, and that line dispatched. Count: 202 `focus=0` lines on the
   boot, 4 `focus=1`.
4. THE KEYS AFTER t=504s REACHED THE SHELL. Rebuilt from the `USB-DEBUG: KEY` lines (caps lock was on,
   `ALLKEYS caps=1` at 15:39:14): `bg /apps/NET.BIN` + Enter. The Enter is followed by
   `[gui] app-enter t=528s` (15:39:18). `gui_watchdog::on_app_enter` is called in exactly one place,
   `main.rs` `handle_key`, right before `shell::dispatch_command`, and only when the shell's line editor
   has taken an Enter. So the shell had the line and was dispatching it.
5. THEN THE WHOLE MACHINE WENT SILENT. After `[gui] app-enter t=528s` there is no other line at all: no
   `[midden] cmd=`, no `[gui] app-exit`, and no 5-second rollups (`[wcn]`, `[wpace]`, kepler, PWR,
   SMC-BATT all stop; the last ones are at 15:39:15). The next timestamped line is the next boot,
   `[16:16:37Z] SMP: starting APs` (line 10270). "257 lines of normal rollups" is the 24 s BEFORE the
   Enter, while Peter was typing; it is not a locked-out operator. Peter's own words (FLIGHT20.md
   line 9): "net.bin test locked out input".

## Where the dispatch stopped
Before the `[midden]` line, `dispatch_command` runs only `history_record` (a lock only the shell takes),
`help::intercept` (pure), `midden_facts` (`proc_table_rows` is a `const fn` on x86) and
`midden_core::plan` (for `bg`, a HOST_VERBS lookup with no volume probe). None of these can block on
their own. The likelier reading: the `[midden]` line was formatted and handed to the console taps, and
the hang came straight after, in `bg`'s `spawn_user_image_bg` of NET.BIN or in NET.BIN's first
syscalls (entropy, resolve, the usbnet fulfiller). On this machine the wire is the FTDI console on
xHCI (`DRAINCAP: SKIP — no 16550 on this machine`), and USB Ethernet is on xHCI too. A BSP or xHCI wedge
caused by the network client would stop the console and the rollups together, which is exactly what
the wire shows. I can't tell those two apart from this capture.

## Why M2/M3 are NOT built as briefed
- M2(b), "on focus=0 with live windows, re-resolve focus to the top-most live window and deliver the
  key", would take keys away from the shell. On boot 20, `win=2` (kernel, z=258) and `win=3`
  (`asid=0x1`, z=140, 19.6/s) were live the whole time while the shell held focus=0. The rule would have
  sent every key Peter typed to one of them.
- M2(a): the close paths already drop focus to the shell (`focus_release`, CLOSE-TEARDOWN; the
  `[closemin] … next_focus=… focus=1` fixture). There's no evidence of a close that leaves a stale
  non-zero focus. A "top-most live window" rule would also change today's drop-to-shell behaviour.
- M3 (`tests focus`) would only show that the shell takes a key at focus=0, which is already the
  shipped behaviour.

## Re-aim (for the seat)
The real fault is a NET HANG: on boot 20, `bg /apps/NET.BIN` (the NETRING3 ring-3 network client, B306;
staged as `APPS/NET.ELF` on this tree) froze the machine within 3 s of `[gui] app-enter`, before any
dispatch witness reached the wire. Its neighbours are USBNET7 (rx_ok=0, the bulk-in re-arm) and the
xHCI that carries both the FTDI console and usbnet. Next witness to ask for on boot 22: launch `NET`
by bare name with UNAOS_USBDEBUG, and add a `[bg] spawn path=… pid=…` line ahead of the spawn plus a
first-syscall breadcrumb in the netring3 fulfiller, so the wire shows which side stopped.

Owed: the NET HANG arc (not this branch). Nothing is built on this branch beyond this note.
