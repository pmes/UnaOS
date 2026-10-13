# SESSIONWEDGE (rmbp-ledger B525): why the pump stopped at desktop ignition

Base d4f10a15 (has TRASHLOCK c416a2cc). Wire: `docs/dev/evidence/rmbp-0915/flight29/f29-boot1.log` (image 22 = cccb7ae5, WITHOUT TRASHLOCK).
QEMU: not run. The seat's amendment withdrew the before/after runs mid-build (R107), and QEMU has no SMC, so the smc-batt leg could not have been reproduced there anyway. Reasoned from the code and the wire.

## The wire
- 14:35:19 `[boot] phase=desktop … at=20617ms` (`boot::session_opened`, login.rs:755), then `[login] desktop bare: furniture=none (R88) why=sessio`. The capture ends mid-word. The pinned log has no `[dock] press` lines. Those were reported from the first card's capture in the queue STATE, not this one.
- The rMBP has no 16550, so every line goes to the FTDI mirror ring (arch/x86_64/serial.rs `_print` → `ftdi::mirror`). The ONLY thing that drains that ring onto the cable is the usb-pump's xHCI step (main.rs usb_pump `service_ftdi` → xhci/mod.rs `drain_ftdi`, 64-byte bulk-OUTs, a 4 ms slice per pass). So a line cut mid-word is the pump's last packet. Either the pump never drained again, or the sink turned itself off.
- After the wire died, Peter saw the mouse working, and on the first card the Dock answered presses. On x86 the trackpad decodes on `hid-pump` and xHCI input on `xhci-poll`, and both share the pump's core (cpu 7 on f29: `[hid] pump task=hid-pump cpu=7`). So that core was NOT spinning with interrupts masked. The pump stopped in an UNMASKED wait, or the sink disabled itself.

## Where each new task runs, and what it takes against the pump
worker_cpu(0) = cpu 2 on f29 (`[wallpaper] probe … on=worker cpu=2`). In the Exclusive tier, xhci_worker_cpu(0) (smp.rs:351) is that same core.

| task | core | locks / work | against the pump |
|---|---|---|---|
| smc-batt (B518, smc.rs tail) | worker_cpu(0) | `TXN` (raw spin `sync::Mutex`, no mask) per key, every 1–32 s sweep | the pump's desktop-app step takes `TXN` too: `status::poll` (desktop_uefi.rs:727-728), BATTLIVE `adapter::read`, LIDSLEEP `MSLD` |
| selfhost-/fatverb-/prtscr-witness (B518) | worker_cpu(0) | FAT reads; on metal `tests::defer` returns true at once, so they exit (shell.rs:9072) | none at ignition |
| wall-probe (B514) | worker_cpu(0) | `users` lock briefly, UnaFS stat/decode (MOUNT masked) | re-armed by the build the pump runs (`desktopbuild::service` → `wallpaper::rearm`) |
| dock-trash (B509) | xhci_worker_cpu(0) | UnaFS (MOUNT masked) | ran at 14:35:17 |
| bootlog-flush (B516) | xhci_worker_cpu(0) | UnaFS write of up to 256 KiB, MOUNT masked, every 4096 ms (up to 483 ms on f28) | none directly |
| xhci-poll (B512) | the pump's core | the xHCI loan for `poll_events` only, then `sleep_ticks(1)` every pass (xhci/mod.rs tail) | a Busy costs the pump ONE pass, never the cable. It yields every tick, so the brief's "never yields" premise is refuted by the code |

## The chain on image 22
1. At session open, worker_cpu(0) carries masked UnaFS takers (dock-trash 14:35:17, bootlog-flush, wall-probe, the jobs/filetypes workers) beside unmasked FAT-on-SDHC CARD users. f29 14:35:02 shows the card lock already contended across tasks: `[lock] spin-wait name=sdhc.rs:997 … waiter=9@cpu7 on=12@cpu1`. Without TRASHLOCK this is B523's inversion: a masked MOUNT holder spins on CARD while the CARD holder is preempted on the same pinned core. That core never ticks again, and every task pinned to it (smc-batt included) freezes where it stood.
2. smc-batt spends a real share of each second inside `TXN` (f28: 85–148 ms sweeps). Frozen there, it keeps `TXN` held forever.
3. The pump's next SMC read on the desktop-app step spins on `TXN` with interrupts ON (sync.rs:69-88, no bound). The pump core stays live: hid-pump and xhci-poll keep running, so the sprite moves. The render core stays live, so the Dock answers. The FTDI ring is never drained again, so the wire dies on its last packet. This is the only chain I found that fits all three observations. A pump spinning masked (on MOUNT, say) would have frozen the trackpad as well.
4. A second, independent way the cable dies mid-line, still live on the base: `pump_until_ftdi_done` (xhci/mod.rs:13843) checks its wall-clock budget right after `hlt()` without draining once more. A pump kept off its core for about 2 s while it waits on a bulk-OUT comes back, calls TIMEOUT, and `disable_ftdi_tx` turns the cable off for the rest of the boot. Its own line rides the sink it just killed.

## Does TRASHLOCK alone explain it?
TRASHLOCK covers the permanent half. It takes CARD only with interrupts masked, so a CARD holder can never be preempted and worker_cpu(0) can no longer freeze for good (step 1). It does not remove the bridge (steps 2–3). PUMPWITNESS left the pump's SMC readers on the pump, so the pump still waits on a holder that worker_cpu(0)'s scheduler can park. With TRASHLOCK in place that wait is bounded by the core's longest masked transaction (483 ms bootlog-flush, 666 ms filetypes on the wire), and the cable stalls that long. Add any other long hold on that core, or one off-core stretch past the FTDI budget (step 4), and the sink is gone again. So TRASHLOCK alone is necessary, but it is not the whole fix.

## The fix (this commit)
- smc.rs: `TXN` is taken by `txn_acquire` (file tail). A busy lock is retried on the next tick with `sleep_ticks(1)`, a spin pause when no task or IF=0, for at most 300 ms (three of one step's `SMC_WAIT_CYCLES` bound). Past that the read answers `Stuck(STEP_TXN=5)`, which every caller already holds through, and the boot's one DIAG is not spent on it (smc.rs:467/:477). The holder is recorded so PUMPWEDGE can name it. The two take sites (:508, :838) are same-line edits.
- xhci/mod.rs: when the FTDI wait's budget is spent, it does one last full drain and checks again (`ftdi_done_late`, file tail; :13843 same-line). Off-core time is no longer read as a dead cable.
- The watch: `sessionwedge::stamp` runs every pump pass (main.rs pump_top line). RENDERWEDGE's render-watch (sibling core, never the service core: sched.rs:3066) calls `sessionwedge::look` every 250 ms in the desktop phase. If the stamp is older than 2 s it prints `:: PUMPWEDGE: age_ms= last_step= holder= -> FAIL ::` once per wedge. The line goes through `serial_println!` (the printing core drains a 16550 where one exists, QEMU) and is also painted straight onto the panel via `video::WRITER`, repainted every second while the wedge lasts (the rMBP has no UART).
- `tests sessionwedge`: `:: SESSIONWEDGE: bare_to_built_ms= pump_gap_max_ms= pump_passes= through= armed=xhci-poll:,smc-batt:,bootlog-flush:,wall-probe: watch=seen: -> PASS ::`.

## The next boot's wire
- A healthy boot: `[desktop] built at=login ok bar_ms=<≤500>` right after `desktop bare`. No `:: PUMPWEDGE:` line. `tests sessionwedge` → `-> PASS` with `pump_gap_max_ms` well under 2000 and `through=1`, plus one `:: PUMPWEDGE: … -> SEEN :: … fixture=1` and a red band on the panel for about 1 s during the verb.
- If the pump stops again, the panel shows `PUMPWEDGE: the usb pump has not run for N s, last step <step>, waiting on <holder>` within 2.25 s, and the wire shows it too if the cable is alive. `holder=smc-txn:<tid>@cpu<n>` means the SMC bridge. `[smc] txn busy waiter=usb-pump holder=… -> refused` means the bound fired and the pump carried on.

## Type check (the amendment's one check)
`cd unaos && UNAOS_WC=1 UNAOS_LOGIN=1 UNAOS_UNAFS=1 UNAOS_SMC=1 ./arroyo check`: `✅ x86_64 OK`, `✅ aarch64 OK`, `✅ bootloader OK`. The per-variant matrix that runs after those was stopped at the session's 30-minute task limit, after 48 legs at rc=0 and one red: `x86-vsyncpace` rc=101, `E0599 no method named get for SlotVec` at arch/x86_64/syscall.rs:4189 and :4358. That file is not one of this arc's, so it is reported here and not touched. A first run had redded on this arc's own `video::wallpaper` reference, which is gated on `facet`; that call is now `#[cfg(feature = "facet")]`.
