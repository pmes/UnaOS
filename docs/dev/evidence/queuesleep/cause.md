# QUEUESLEEP (rmbp-ledger B512) — what the idle-desktop stall was, read off f28-boot1.log

Source: `docs/dev/evidence/rmbp-0915/flight28/f28-boot1.log`, `awk 'index($0,"[lag] stall")'`. The machine was not
sleeping on input. It was doing three different things, and the pump core was never asleep through any of them (the
usb-pump, input and hid-pump tasks all nap one 1 kHz LAPIC tick, `sleep_ticks(1)`):

1. **12:21:11Z `stage=pump stage_ms=28440 pump=selfhost`.** The SELFHOST source check (`:: SELFHOST: src verified …
   bytes=136862567` prints at 12:21:10) ran INLINE on the usb-pump for 28.4 s. It started at 12:20:41 when
   `tests winmemory` ran: `tests::defer` (tests.rs:275) returns false while ANY `tests` verb is RUNNING, so the
   pump-side `verify_source_once()` call ran its body on the pump. That loop was the only thing polling the xHCI event
   ring (the x86 MSI-X goes to the BSP and `xhci_msi_handler` only acks), so every xHCI key, click and serial-door
   byte waited behind it. EHCI was fine (`hid_gap_ms=12`) because it got its own task in INPUTSTALL2.
2. **12:22:47Z `stage=queue stage_ms=3090`, then 12:23:14Z `stage=render-handler stage_ms=35074 handler=console-launch
   masked_ms=36303`.** `jobs next` (12:22:37, `done … ms=111003`) held an IRQ-masked UnaFS transaction for 36.3 s, and
   the render task's service chain waited behind it. The serial-door line `tests gen7`, typed at about 12:22:43, was
   routed at 12:23:14, 31 s later. The 3090 ms is the stuck key's age at the one roll that came before the orphan
   expiry.
3. **From 12:27:53Z (`tests quarry2`) to the end of the log (12:36:47Z) the render task never routed again.** Every
   later stall line has `handler=- render_ms=0.0`, there is no `:: LAG:` rollup, no `[serialdoor]` line and no
   `TESTS: ran` for quarry2. EHCI keys were still decoded (`EHCI-HID: KEY` at 12:35:11) and pushed. `queue=` 1070,
   2110, 3144, 7886, 24840 and 35413 are the age of a stuck key at sparse rolls, and the 24–35 s `span_ms` values are
   the gaps between rolls, because nothing routed. This is the "long stall on an idle desktop".

Also: `pump=typematic-smc pump_ms≈95–148` every 10–20 s (`[lock] held-too-long name=smc.rs:1279 … by=9@cpu7 ms=85`),
and `pump=desktop-app pump_ms≈95–141` once a minute at :11. These are pump-step holds. Since this arc the xHCI input
no longer waits behind them.

## What this arc changed
* `drivers/xhci/mod.rs` (tail): an `xhci-poll` task on the pump core. It claims and polls the event ring every tick,
  and retries a Busy claim on the next tick, so no usb-pump step can hold xHCI input again. EHCI already has
  `hid-pump`. Both controllers are now polled at the 1 kHz tick by a task that no service step can hold. An IRQ wake
  is not needed for the 8 ms bound: the tick already wakes the core every 1 ms.
* `video/lag.rs` (tail, plus same-line folds): when the stage is pump or queue, the `[lag] stall` line ends in
  ` pump_what=<fn> pump_wait=<hlt|lock|poll-gap|claim-busy|app-gate|channel-full|handler|run>`. Case 3 would have
  read `pump_what=render@<step> pump_wait=handler|lock|hlt`.
* `tests queuesleep` (queuesleep.rs): a 30 s idle window, then one `KeyUp(0)` through `pal::push_event`, timed to the
  router.

## Owed elsewhere (STOP: not this brief's files)
* tests.rs:275 `defer`: `RUNNING.load(..)` → `RUNNING.load(..) && running_is(name)` (SMALLFIX7's `defer_fast`
  shape). Without it, any `tests` verb fires the pump-side witnesses (selfhost, fatverb) inline on the usb-pump.
* The 36 s IRQ-masked UnaFS transaction under `jobs next` (the UnaFS arc / JOBSCAN): the render task must not wait
  on it.
* The render task wedge after `tests quarry2` (video/quarry/columns.rs fixture): the next flight's
  `pump_what=render@…` names the step.
