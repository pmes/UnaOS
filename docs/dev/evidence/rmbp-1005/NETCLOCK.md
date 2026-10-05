# NETCLOCK (B335) — the smolnet poll clock, asynchronous USB TX

CHARTER: Kernel — driver (no new kernel file; edits `smolnet.rs`, `net_tick.rs`, `net_phy.rs`, `drivers/e1000.rs`
(one tail fn, one same-line append), `drivers/xhci/usbnet.rs`, the usbnet tail of `drivers/xhci/mod.rs`, one `tests.rs` registration).
Branch `exec-rmbp-netclock`, cut from 8c750d43 (merge11). USBNET7's RX arm/re-arm is untouched.

## M1 — measured from the boot-20 wire (`rmbp-0915/flight20/f20-boots.log`, awk)
Boot A = lines 72..10269, boot B = 10270..15546.
- **tx per second.** Boot A: `:: USBNET: up` at 15:31:19 (line 3071); the only TX count, line 5944 at 15:34:56 (printed because
  `tests usbnet` switched the census on), says `rx=0 tx=58380 tx_drop=0 errors=0`. 58380 frames in 217 s = **269 tx/s average,
  with nothing received**. The wire has no per-frame dump, so the frame mix is read off the code that ran (below).
- **polls per second.** `SOCK-1` is `pump()`, a fixed 2,000,000 `iface.poll` iterations when no echo comes back. It printed
  `0/4 replies` at 15:31:21, at most 2 s after `:: USBNET: up` (boots A and B both). That is **at least 1,000,000 polls/s**. Each
  poll's `raw_rx` finds RXQ empty and runs `drive()` (claim the loan, drain the event ring, one service pass), about 1 µs when
  nothing has happened.
- **loan holds.** The FTDI console and usbnet TX share `ftdi_pending`/`pump_until_ftdi_done` and its peak counter. Boot B line 13440,
  15:31:23: `:: FTDI: tx pump budget=5387715708 used=152808969 n=3703`. The budget is 2 s, so TSC = 2.694 GHz: the **worst
  synchronous bulk-OUT wait was 56.7 ms**, held with the xHCI loan (and, from a pump, `STACK`). n went from 3 to 3703 in the 4 s after
  bring-up, about **925 synchronous OUT waits/s**. Boot A's peak never doubled past n=3, 10767060 cycles (4.0 ms).
- **The loop that runs free.** `POLL_CLOCK` is "monotonic" only in name: every poll adds **1 ms of smoltcp time**, so at ~1M
  polls/s smoltcp's timers run about 1000 times faster than the wall. Each timer then fires about 1000 times too often:
  - ARP: the stack was built with the static slirp config (10.0.2.15, gw 10.0.2.2), and nothing on the bench LAN answers for
    10.0.2.2/10.0.2.3. smoltcp re-ARPs once per smoltcp-second while a packet waits. SOCK-1 (2M polls), SOCK-2 (420k) and
    SOCK-3 (about 800k) each ran an **ARP storm** of a few thousand requests in 6 s.
  - DHCP: the kernel DHCP socket stays in the set after `dhcp_acquire` gives up, in `Discovering`, and `discover_timeout` is 10 s.
    Six `dhcp_link_tick` tries × 400k polls = 240 DISCOVERs. Then **in steady state `witness_tick6` → `stack_accept` runs
    `ACCEPT_PUMP` = 40,000 polls on every `service_net` pass, for ever**: the SOCK-6/7 listener never gets its injector on
    metal. That is 40 smoltcp-seconds per pass, so **4 DHCP DISCOVERs per main-loop pass**. This is the steady **DISCOVER storm**
    that makes up most of the 269/s.
  - Every one of those frames was a synchronous bulk-OUT TD awaited with the loan held (`usbnet_tx_stage`).

## The seam
smoltcp already says when it wants polling: `Interface::poll_delay`. Feed it the real clock and only poll when it asks, or
when a frame is waiting.
- **Clock**: `now_ms()` is `clock::uptime_ms()` (the TSC-backed monotonic, which also advances with IF clear). If that is `None`,
  it falls back to the TSC over the `hw_wait_budget` rate. It is never per-poll.
- **One poll site** `poll_now(stack)` polls at `now_ms()`, counts the poll (`net_tick::note_poll`) and stores
  `NEXT_POLL_MS = now + poll_delay` (1 s when smoltcp has nothing scheduled, never under 1 ms). `poll_due()` = `now ≥ NEXT_POLL_MS`
  or a frame is waiting (`e1000::rx_ready` reads the next descriptor's DD bit; `usbnet::rx_ready` checks RXQ and drives the
  controller at most every 250 µs). `kick()` brings the next poll forward after a socket enqueue or connect.
- **Pumps are wall-time bounded.** Every blocking op runs `pump_until(ms, check)`: it takes `STACK` for one gated poll plus the
  check, then releases it (M4: the TCP syscalls and every other pump give `STACK` up between iterations). It spins with
  `spin_loop` until the next due poll, a waiting frame or the deadline. Budgets: recv/connect 2000 ms (the NETHANG cap), DHCP 2000 ms
  per try, send 0 (kick + one poll), accept 0 (one gated poll; ring 3 re-drives, and the witness calls it every pass).
- **The idle service.** `smolnet::service_poll()` from `service_net`, every main-loop pass: one gated poll. It applies a DHCP lease
  that lands after `dhcp_acquire` has given up (DHCP retransmits on smoltcp's own 10 s timer, in real seconds).
- **Asynchronous TX (M3).** `usbnet::raw_tx` only queues onto TXQ, the bounded 8-frame ring. Back-pressure: `RawNic::tx_ready`
  (default `true`, so aarch64 is unchanged) makes `SmoltcpPhy::transmit` return `None` while the ring is full. That is smoltcp's
  -EAGAIN: the packet stays in its socket and goes on a later poll. The data pass `usbnet_data_pass` (the controller tail)
  **reaps** the previous OUT completion and **issues** the next frame (stage into the slot buffer, TRB, doorbell). One TD is in
  flight at a time and there is no wait. The completion is claimed in `usbnet::claim` beside the IN claim, so `mod.rs`'s event
  dispatch line does not move. The stack-side `drive()` runs only the data pass (no PHY poll, no bring-up, which stay on the main
  loop's full `service_usbnet`), so the loan is held for the event drain plus one enqueue. Its hold is measured (`loan_held_max_us`).
  A TD that never completes is abandoned after 1 s, counted as `tx_stuck` on the census line.

## Milestones
- M1 this measurement. M2 the clock + gate + wall-bounded pumps + `service_poll`. M3 asynchronous TX + back-pressure.
- M4 the TCP pumps (`stack_connect/send/recv/accept`, through `pump_until`) release `STACK` every iteration and are bounded by the wall.
- M5 `tests netclock`: on a live link, 5 s of idle. The fixture runs the main loop's two net calls itself (the xHCI usbnet pass +
  `service_poll`), because the shell holds the main loop while it runs:
  `:: NETCLOCK: polls_per_s=<n> tx_per_s=<n> loan_held_max_us=<n> rx_ok=<n> -> PASS|FAIL ::`, bounds polls ≤ 50/s, tx ≤ 5/s,
  loan ≤ 500 µs. No link → `-> SKIP reason=no-link`. The census rollup `:: USBNET: rx= tx= …` gains `polls= tx_q= tx_stuck=`.
  Nothing new at boot (R80).

## Witness (what boot 21+ should print, on `tests netclock` with the dongle cabled)
`:: NETCLOCK: polls_per_s=<≤50> tx_per_s=<≤5> loan_held_max_us=<≤500> rx_ok=<n> -> PASS ::`. Expected idle profile: the DHCP
DISCOVER every 10 s (0.1 tx/s) until a lease, polls about 1/s plus one per received broadcast frame.

## Owed
- One OUT TD in flight (one staging slot at `TX_BUF_OFFSET`). A second needs a second staging buffer, the same note as USBNET7's IN side.
- `drive()` still drains the whole event ring (`poll_events`), so a hold can include another device's events. The fixture measures
  it as it is.
- The ICMP `ping`/`arp` pump keeps its own throwaway interface. It is now real-clocked and capped at 2 s.

## Results (M2..M5, one commit)
M2..M5 are one commit: the previous executor was interrupted with all four in flight across the same seven files
(the TX back-pressure in `smolnet.rs` calls into the M3 ring, so no earlier split compiles on its own).
- M2 `now_ms` (the real clock), `poll_now`/`poll_due`/`kick`, `pump_until`, `service_poll` on the `service_net` line (same-line append).
- M3 `usbnet_data_pass` + `usbnet_tx_issue` (no `ftdi_pending`, no wait), `tx_claim` on `usbnet::claim`, `RawNic::tx_ready` -> `tx_room`.
- M4 `tcp_pump_chunked`, `stack_recvfrom_bounded`, `stack_accept`, `dhcp_acquire` go through `pump_until` (one poll per `STACK` hold).
- M5 `tests netclock` (`smolnet::netclock_selftest`, registered by `tests::ensure_netclock`); census `polls= tx_q= tx_stuck=`.
Compile legs (inline, from `unaos/crates/kernel`): x86 metal shape exit 0; x86 metal shape + `usbdebug` exit 0.
charter-check exit 0. The aarch64 leg was not run (the only aarch64-visible edit is the defaulted `RawNic::tx_ready`).
Not touched: the aarch64 smoltcp stack in `net_phy.rs` (its own per-poll `POLL_CLOCK`, the same flaw, owed to the arm tracks).
