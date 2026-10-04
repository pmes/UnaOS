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

---

# NETHANG (the re-aim, on this branch at the coordinator's ruling) — design and result

## Finding
Boot 20, `bg /apps/NET.BIN`: the console, the 5 s rollups and the panel all stopped within 3 s of
`[gui] app-enter t=528s`, and there was no RING-3 FAULT. That means a kernel-side lock-up.

## M2: the deadlock, found by reading
NET.ELF's second syscall is `SYS_RESOLVE` (after `SYS_GETRANDOM`). Its path, with each lock and who
else takes it:

| step (IF state) | lock / resource taken | who else takes it, from where |
|---|---|---|
| `syscall_dispatch` (IF=0, SFMASK clears IF) | none | — |
| `netring3::resolve` → `e1000::hw_addr` | `NET_DEVICE` (spin, micro-hold) | BSP main loop `service_net`; the e1000 MSI ISR does NOT take it (lock-free ack) |
| `smolnet::resolve` → `stack_open/bind/sendto/recvfrom/close` | `STACK` (spin), held for the whole pump: `SEND_PUMP` 20k, `RECV_PUMP` 400k polls | BSP main loop `net_tick` (`dhcp_link_tick`, the SNTP/DNS witnesses), the shell verbs (`fetch`, `tests net`), other ring-3 socket syscalls. No ISR. |
| `iface.poll` → `usbnet::raw_rx`/`raw_tx` | `RXQ` / `TXQ` (spin, micro-hold) | the xHCI device-service pass (main loop) through `deliver`/`next_tx`. No ISR. |
| `usbnet::drive` → `xhci::claim()` | the xHCI LOAN (`XHCI_CONTROLLER` taken out; a try-claim, `Busy` otherwise) | BSP main loop xHCI pass: FTDI console TX, the HID, BOT storage. Every serial line on this machine (no 16550) leaves through the FTDI on this loan. |
| `service_usbnet` → `usbnet_tx_stage` → `pump_until_ftdi_done` | WAITS for a transfer event, holding the loan AND `STACK` | — |

The wait is `loop { drain_event_ring_once? ; crate::hlt(); if elapsed >= budget { TIMEOUT } }`. With
IF=0, `hlt` wakes only on NMI/SMI/INIT. The first pass that finds the event ring empty (right after the
doorbell, which is nearly always) halts that core for good. The 2 s TSC budget is never checked again.
That core now holds the xHCI loan for ever, so the FTDI console (the wire) and the HID are dead, and
it holds `STACK` for ever, so the main loop spins in `STACK.lock()` from `net_tick`. If NET.ELF ran on
core 0 it is the main loop itself, and core 0 is also the only core that advances `APIC_TICKS`. Of the
three candidates in the brief this is (b): the net poll runs IF-masked and waits on a USB completion,
here with an interrupt-woken idle that cannot be woken. `tests net` passed because the shell runs the
same body with IF=1, where `hlt` wakes on the next tick.
(a) is ruled out: no ISR takes `STACK`, `RXQ`, `TXQ` or the loan; the xHCI interrupt path is not
used (`IRQ_COUNT=0`, polled). (c) is ruled out: no scheduler lock is taken on this path.

## Fix
1. `arch/x86_64/mod.rs` `hlt()`: with IF clear it does one `pause` and returns, counted in
   `masked_hlt_count()`. With IF set it is the same `hlt` as before. `hlt_loop` keeps the raw `hlt`
   (a deliberate stop). Every `crate::hlt()` wait loop in the xHCI pumps (FTDI, EP0, BOT, command)
   is therefore bounded by its own TSC deadline in any context.
2. `smolnet.rs`: `stack_pump` gets a wall-clock cap (`hw_wait_budget`, 2 s) beside its iteration
   count. `stack_recvfrom` releases `STACK` between `TCP_CHUNK` chunks, re-validates the socket each
   chunk and returns `None` at the deadline, which the caller maps to -EAGAIN / ENOENT. A dead link
   now costs one resolve at most about 4 s, and the main loop gets `STACK` between chunks.
3. Not touched: the AX88179 RX arm/re-arm (USBNET7 owns it). The loan is still held across one
   synchronous TX transfer, now bounded at 2 s, which is the controller's own discipline.

## M1: breadcrumbs
- `[bg] spawn path=<p> bytes=<n> pid=pending` before the spawn, then
  `[bg] spawn path=<p> pid=<n> asid=<x> -> started` (`bg` and the bare-name launch; `pid=foreground`
  for a non-detaching bare name).
- `[net3] first syscall sys=<nr>(<NAME>) task=<id> cpu=<n> masked=<0|1>`, once per task change.
- Under `usbdebug` only: `[net] poll enter op=<sendto|recvfrom> …` / `[net] poll exit …`.

## M3: witness
`tests nethang` → `:: NETHANG: path=resolve held_locks=0 bounded=1 link=1 masked=1 ms=<n> masked_hlt=3 capped=<n> rc=<ip|no-answer> -> PASS ::`
With no link: `:: NETHANG: path=resolve held_locks=0 bounded=1 link=0 masked_hlt=3 -> SKIP reason=no-link (masked hlt returns: PASS) ::`.
Without the fix, the masked `hlt` leg freezes the machine at that line. That is the failure shape, and
`[gui] app-enter` for `tests nethang` would be the last line on the wire.

## Owed
The TCP syscalls (`SYS_SOCKET`/`CONNECT`/`SEND`/`SOCK_RECV`) go through the same `STACK` + `drive()`
path. With fix 1 they are bounded, but they still hold `STACK` for a whole connect pump (TCP_CHUNK
discipline already applies there). Releasing the loan between the TX doorbell and its completion
(an async TX) is a USBNET follow-up.
