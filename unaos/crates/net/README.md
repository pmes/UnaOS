# net

A hand-written, dependency-free TCP/IP stack for the UnaOS kernel.

`#![no_std]` for kernel builds (uses only `core`); `std` is linked only under
`cargo test` so the estimator unit tests run on the host. No external
dependencies.

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
