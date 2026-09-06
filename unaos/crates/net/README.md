# net

A hand-written, dependency-free TCP/IP stack for the UnaOS kernel.

`#![no_std]` for kernel builds (uses only `core`); `std` is linked only under
`cargo test` so the estimator unit tests run on the host. No external
dependencies.

## Status: RETIRED AS THE DEFAULT — live, and available for resumption

As of **SMOLNET-DEFAULT** (2026-07-17, Peter's ruling) the x86 kernel's default
TCP/IP path is the mature [smoltcp](https://github.com/smoltcp-rs/smoltcp) stack
(`crates/kernel/src/smolnet.rs`); this hand-rolled crate is **no longer the
default**. It was **not** trashed and **not** removed — the code is correct on its
own terms, and per the project's never-trash-code rule it is an asset kept in
tree, catalogued here, and **available for reuse/resumption**.

It is also still **live**, regardless of the knob: the e1000/e1000e driver's
main-loop `service_net()` poll, the boot connectivity self-test, the DHCP client,
the TCP echo listener, the shell's `nc` / `nc -u` / `curl` commands, and the
`net::arp::learn` reuse the smoltcp `Device` snoops for MAC surfacing all depend
on this crate unconditionally. smoltcp has not yet replaced those surfaces
(SOCK-8+ future work), so retirement here is a **default + status** change, not a
code removal.

- **Resume hand-rolling / run this stack as the whole net path:** build with
  `UNAOS_NOSMOLNET=1` (e.g. `UNAOS_NOSMOLNET=1 ./arroyo test 40`). That drops the
  `smolnet` cargo feature, and this crate serves `ping`/`arp`/`ifconfig` too (not
  just nc/curl). The opt-out x86 build is byte-identical to the
  pre-flip default.
- **Doc of record for the live default stack:** `unaos/docs/dev/OS/08_NET/networking.md`.
- **This crate's own architecture doc:** `docs/dev/OS/06_NETWORK_STACK/network_stack.md`
  (carries a retired-line banner pointing back here and at 08_NET).

## Layers
- `ethernet`, `arp`, `ipv4`, `icmp`, `udp`, `dhcp` — L2–L4 framing and the
  stateless services.
- `tcp` — a stateful TCP engine: a multi-connection listener (`TcpListener`) and
  an active-open client (`TcpClient`), with a byte-stream send buffer + sliding
  window (`SendRing`), adaptive RTO (RFC 6298 + Karn), retransmission,
  multi-extent out-of-order reassembly, and honest receive-window flow control.

## Entry points
- `net::ingress(buffer, arp_state, tx_buf) -> Option<usize>` — the stateless
  reply path (ARP / ICMP echo / UDP echo). The NIC driver calls it per frame.
- `TcpListener` / `TcpClient` — driven directly from the kernel for stateful TCP.

## Testing
- `cargo test -p net` — host unit tests (29; RFC 6298 estimator, `SendRing`,
  listener flow control).
- `unaos/scripts/net-inject.py` — a rootless Ethernet frame injector for
  loss/reorder/streaming/flow-control scenarios against the running kernel.

See [`docs/dev/OS/06_NETWORK_STACK/network_stack.md`](../../../docs/dev/OS/06_NETWORK_STACK/network_stack.md)
for the full architecture, and the e1000/e1000e NIC driver in
`unaos/crates/kernel/src/drivers/e1000.rs` for the interrupt-driven (MSI vector
0x41) transport.
