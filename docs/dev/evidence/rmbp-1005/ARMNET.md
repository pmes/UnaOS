# ARMNET (B346) — the aarch64 smoltcp clock: real time, poll on `poll_delay` or RX arrival

CHARTER: Kernel — driver (no new kernel file; edits `clock.rs` (tail), `net_phy.rs`, `smolnet.rs` (`now_ms` body),
`census.rs` (one bit), `tests.rs` (same-line call + tail fn), `arch/aarch64/virtio_net.rs`, `arch/aarch64/rtl8168_tegra.rs`).
Branch `exec-rmbp-armnet`, cut from b8eeb37d, `exec-rmbp-netclock` merged first (tests.rs keep-both).

## Finding
NETCLOCK (B335) put the x86 stack on the wall clock. The aarch64 stack, `net_phy::net6` (the persistent interface the
shell verbs and the EL0 socket syscalls use, over virtio-net on QEMU virt and rtl8168 on the Orin), still feeds smoltcp
`POLL_CLOCK`, which goes up by 1 ms on every poll. Its pumps are poll counts (`SEND_PUMP` 20k, `RECV_PUMP`/`CONNECT_PUMP` 400k,
`CHUNK` 4k), so one `sendto` moves smoltcp forward 20 s and one `recvfrom` 400 s. ARP re-requests, TCP retransmits and every
smoltcp timer run as fast as the CPU polls. The `ping`/`arp` verbs' throwaway interfaces do the same (`clock += 1`). So do the
virtio bring-up ping and the rtl8168 post-DHCP window (real clock, but a poll on every pass). `dhcp_or_static` has a real clock but
also polls on every pass. Each driver reads CNTPCT itself with `cnt*1000/frq`, which wraps after about 4 days.

## The seam (the NETCLOCK shape, one clock per arch)
- **Clock**: `clock::stack_ms()` is THE network-stack clock on both arches. It is `clock::uptime_ms()` (aarch64: CNTPCT/CNTFRQ,
  the generic timer; x86: the calibrated invariant TSC), with the pre-calibration cycle fallback, clamped so it never
  steps back. `uptime_ms` is made overflow-free (it was `ticks*1000`, which saturates after about 4 days). `smolnet::now_ms`,
  `net6::now_ms` and both aarch64 drivers' `now_ms` read it.
- **RX probe**: `NicOps` gains `rx_ready: fn() -> bool`. virtio: RX used-ring `idx != last_used`. rtl8168: the descriptor at
  `rx_cur` has OWN clear. Each is a pure read, so it skips the armed-build scans that `rx_frame_raw` runs on an empty ring.
- **One poll site** `net6::poll_now`: polls at `stack_ms()`, counts it, and sets `NEXT_POLL_MS` from `poll_delay` (1 s if
  nothing is scheduled, 1 ms floor). `poll_due` = deadline reached or `rx_ready`. `kick()` after a socket enqueue/connect.
- **Pumps bounded by wall time**: `pump_until(budget_ms, check)` takes `STACK` for one gated poll plus the check, then releases it.
  Budgets: recv/connect 2000 ms (the x86 NETHANG cap); a send flushes until its socket's queue is empty, capped at 200 ms
  (aarch64 has no idle service to carry it later).
- **`service_poll()`** (pub): one gated poll with `try_lock`. `tests netclock` drives it; the main-loop call site is owed (below).
- **DHCP/ARP on real timers**: `dhcp_or_static_gated(.., rx_ready)` polls on the DHCP socket's own `poll_delay` or an RX
  arrival. `dhcp_or_static` stays as the wrapper genet uses, with a 1 ms cadence floor in place of a probe. The ping/arp
  throwaway interfaces and the two driver windows use the same gate.
- **TX wait bounded by wall time**: the rtl8168 `transmit` OWN-clear wait was 1,000,000 spins. It becomes 5 ms of `stack_ms`.
  virtio's transmit never waits.

## Milestones
- M1 `clock::stack_ms` + `uptime_ms` overflow fix; `net6` onto `now_ms`/`poll_now`/`pump_until`; `NicOps::rx_ready` (both drivers);
  `POLL_CLOCK` gone.
- M2 `dhcp_or_static_gated` + the ping/arp/bring-up windows on the gate; rtl8168 TX wait capped at 5 ms of wall time.
- M3 `tests netclock` on aarch64 (`net6::netclock_selftest`, SKIP with no NIC or no link). Census bit `net6`, whose rollup
  `:: NET6: polls= tx= arp_reply= dhcp= nic= ::` prints at most every 5 s from the poll site.
- M4 `06_NETWORK_STACK/network_stack.md`: the one-clock rule.

## Witness (arm tracks; `tests netclock` with a cable)
`:: NETCLOCK: polls_per_s=<≤50> tx_per_s=<≤5> nic=<virtio-net|rtl8168> rx_ok=<n> -> PASS ::`; no NIC / no link ->
`:: NETCLOCK: polls_per_s=0 tx_per_s=0 nic=<name> rx_ok=0 -> SKIP reason=no-nic|no-link ::`.

## Owed
- The aarch64 main-loop call to `net6::service_poll()` (aarch64 has no `service_net`; between syscalls nothing polls, so an
  inbound ARP request waits for the next socket call). It exists and is gated; the call site is for the arm tracks.
- genet (Pi 4) has no RX-ready probe yet. Its `dhcp_or_static` runs on the 1 ms floor, not on a probe.

## Results
- merge `exec-rmbp-netclock` 47a63c7d (tests.rs keep-both: `defer_fast`, then `ensure_netclock`). M1 4bf3632f, M2 40bdbfcd, M3 22eb63f1, M4 e62d21ef.
- The ping/arp throwaway-interface gate landed in M1 with the rest of `net6`, not M2. `dhcp_or_static_gated` takes `rx_ready: Option<&dyn Fn() -> bool>`.
- Compile legs (inline, from `unaos/crates/kernel`, sequential, target removed after each):
  aarch64 `login,loginst,virt_el0,vnet,net6` exit 0; aarch64 `login,loginst,tegradesk,desktop_firmware,tegra_el0,tegra,tegrasmp,net4,pcie3,pcie2,net6,sntp6`
  exit 0; x86 metal shape + `smolnet` (the builder's default) exit 0. No warnings in the touched files.
  charter-check exit 0. Not compiled: the Pi `genet` shape (its `dhcp_or_static` signature is unchanged).
