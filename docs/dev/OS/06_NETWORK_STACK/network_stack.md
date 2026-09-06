# UnaOS Kernel Network Stack

> **RETIRED AS THE DEFAULT (SMOLNET-DEFAULT, 2026-07-17).** This document describes
> the **hand-rolled** `net` crate, which is **no longer the x86 kernel's default
> TCP/IP stack** — as of Peter's 2026-07-17 ruling the default is the mature
> [smoltcp](https://github.com/smoltcp-rs/smoltcp) stack. The **doc of record** for
> the live networking stack is
> [`unaos/docs/dev/OS/08_NET/networking.md`](../../../../unaos/docs/dev/OS/08_NET/networking.md).
>
> The hand-rolled code was **not** trashed and **not** removed (never-trash-code):
> it remains in tree at [`unaos/crates/net`](../../../../unaos/crates/net/) (see its
> `README.md` for the catalog entry), is **still live** (it backs the driver's
> `service_net()` poll, DHCP, the TCP echo listener, and the shell's
> `nc`/`curl`), and is the **complete opt-out stack** under
> `UNAOS_NOSMOLNET=1` — **available for resumption** if we choose to resume
> hand-rolling our own. Build with `UNAOS_NOSMOLNET=1` to run everything below as
> the whole net path. The material in this document remains accurate for that crate.

The network stack is a hand-written, dependency-free TCP/IP implementation in
the `net` crate (`unaos/crates/net`). It is `#![no_std]` for kernel builds and
uses only `core`; `std` is linked solely under `cargo test` so the estimator
unit tests can run on the host. The crate has **no external dependencies**.

> **Branch note.** This document describes the network stack as implemented on
> the `c01-03_k01-03_net-stack` branch. The kernel's USB/scheduler and video
> subsystems are developed on sibling branches (`c01-02`, `c01-04`) and are
> documented separately; they are not present on this branch.

---

## 1. Architecture

The stack has two distinct paths:

1. **Stateless ingress fast-path** — `net::ingress()` in `src/lib.rs`. Given a
   raw Ethernet frame, it parses up the stack and, when a reply is warranted,
   writes a complete outgoing frame into the caller's `tx_buf` and returns its
   length. It handles ARP replies, ICMP echo (ping), and UDP echo with no
   allocation and no retained state. The NIC driver calls it per received frame.

2. **Stateful TCP engine** — `src/tcp.rs`. TCP requires per-connection state
   (sequence numbers, timers, buffers), so it is *not* routed through the
   stateless `ingress()` path. Instead the kernel's main loop drives a
   `TcpListener` (passive/server) and/or `TcpClient` (active/client) directly
   against the NIC driver, calling their `handle()` (on receive) and `tick()`
   (on timer) methods.

Layer modules in the crate:

| Module | Responsibility |
| --- | --- |
| `ethernet` | L2 frame parse/build (`EthernetFrame`, `EtherType`, `write_frame`). |
| `arp` | Address resolution (`ArpStateMachine`, `ArpPacket`). |
| `ipv4` | L3 header parse/build + checksum (`Ipv4Header`, `write_header`, `PROTO_*`). |
| `icmp` | Echo reply (`write_echo_reply`). |
| `udp` | Datagram parse/build (`UdpDatagram`, `write_datagram`). |
| `dhcp` | DHCP client (dynamic lease, static fallback). |
| `tcp` | The TCP engine (see §4). |
| `interface` | Interface configuration glue. |

All parsers are zero-copy views over the input slice; all builders write into a
caller-provided buffer and return the byte count. There is no heap use in the
data path.

---

## 2. The stateless ingress router

`net::ingress(buffer, arp_state, tx_buf) -> Option<usize>`:

- Parses the Ethernet frame; drops invalid/undersized frames.
- **ARP:** if the request targets our IP, builds an ARP reply.
- **IPv4:** verifies the checksum and that the destination is our IP, then:
  - **ICMP** echo request → echo reply (ping responder).
  - **UDP** → echoes the datagram back with ports swapped.
  - Other protocols (including TCP) are dropped here — TCP is handled by the
    stateful engine.
- Returns `Some(len)` if a reply frame was written to `tx_buf`, else `None`.

Replies are framed bottom-up in one buffer: `Ethernet[0..14] | IPv4[14..34] |
L4[34..]`.

---

## 3. ARP and DHCP

- **`ArpStateMachine`** (`arp.rs`) is constructed with our IP and MAC. Its
  `process_packet()` answers ARP requests for our address. (The outbound resolve
  cache used for client traffic is maintained alongside the driver.)
- **DHCP** (`dhcp.rs`) is a client that obtains a dynamic lease at boot and
  falls back to a static address if no server responds.

---

## 4. The TCP engine (`tcp.rs`)

This is the most substantial part of the stack (~1,700 LOC). It implements a
practical subset of TCP with honest flow control, congestion-free reliable
delivery, and both server and client roles.

### 4.1 Segment representation
- **`TcpSegment`** — a zero-copy parser over raw bytes (`source_port`,
  `dest_port`, `seq`, `ack`, `flags`, `window`, `payload`), validating header
  length.
- **`checksum(src_ip, dst_ip, seg)`** — the IPv4 pseudo-header + segment
  one's-complement checksum.
- **`write_segment(...)`** — builds a 20-byte header (no options) + payload and
  computes the checksum.

### 4.2 Passive side: `TcpListener` + `TcpConn`
- **`TcpListener`** accepts up to `MAX_CONNS` (= 4) simultaneous connections in a
  fixed connection table (`conns: [Option<TcpConn>; MAX_CONNS]`) — no allocation.
  - `handle(frame, now, our_ip, our_mac, out)` demultiplexes an inbound segment
    to a connection by `(src_ip, src_port)`, accepts a bare SYN into a free slot
    (replying SYN-ACK), and returns any response frame.
  - `tick(now, …, out)` services per-connection RTO timers: it retransmits the
    oldest unacknowledged segment when a deadline expires, or pushes the next
    queued segment.
- **`TcpConn`** is the per-connection state machine. `ConnState` ∈ {`SynRcvd`,
  `Established`, `LastAck`}. It tracks addressing, the send buffer, the peer's
  advertised window, handshake/close flags, the RFC 6298 timer state, and the
  out-of-order reassembly slots.

### 4.3 Byte-stream send buffer + sliding window: `SendRing`
`SendRing` is a circular byte buffer (`SND_BUF` = 2048) implementing a real
sliding window rather than a single in-flight segment:
- `una` — oldest unacknowledged sequence (front of the buffer).
- `nxt` — next sequence to send (`una ≤ nxt ≤ una + len`); in-flight bytes are
  `nxt − una`.
- `push(data)` appends queued bytes (bounded by `free()`); `peek_seg(wnd, out)`
  copies the next sendable segment (bounded by the queued bytes, the MSS = 512,
  and the usable window) without advancing; `mark_sent(n)` advances `nxt` after a
  successful write; `rewind()` resets `nxt = una` for Go-Back-N retransmission;
  `ack(ack_seq)` frees acknowledged bytes from the front and advances `una`.

This lets multiple echo segments be in flight at once, bounded by the peer's
window (pipelining).

### 4.4 Adaptive RTO (RFC 6298 + Karn's algorithm)
The retransmission timeout is computed by a pure estimator,
`rfc6298_step(srtt, rttvar, valid, r) -> (srtt, rttvar, rto)`:
- First sample: `SRTT = R`, `RTTVAR = R/2`.
- Subsequent: `RTTVAR ← ¾·RTTVAR + ¼·|SRTT − R|`, then `SRTT ← ⅞·SRTT + ⅛·R`.
- `RTO = SRTT + max(G, 4·RTTVAR)`, clamped to `[RTO_MIN, RTO_MAX]`.

Karn's algorithm excludes retransmitted segments from RTT sampling. Because the
estimator is pure, it is unit-tested on the host (`rfc6298_*` tests in `tcp.rs`,
runnable via `cargo test -p net`). `RTO_MIN` equals the previous fixed base, so
the adaptive timer is never more aggressive than its predecessor on the sub-
millisecond QEMU link.

### 4.5 Out-of-order reassembly (multi-extent)
Out-of-order segments are buffered in a small fixed array of extents
(`ooo: [OooExtent; OOO_EXTENTS]`, `OOO_EXTENTS` = 4). Each `OooExtent` records
`seq`, `len`, a `fin` flag, and up to `RETX_CAP` (= 512) payload bytes.
`buffer_ooo()` stores a future segment; `drain_ooo()` repeatedly pushes
now-in-order extents into the send buffer as gaps fill, advancing `rcv_nxt` and
latching a buffered FIN. Several reordered segments can be held and drained in
order.

### 4.6 Receive window + window updates
The advertised receive window is honest: it shrinks as un-echoed data
accumulates and reaches zero when the buffer is full. When buffer space reopens
after having been full, the connection emits a window-update ACK so a peer that
paused on a zero window resumes — closing the zero-window deadlock.

### 4.7 Active side: `TcpClient`
`TcpClient` performs an active open (`open()` emits the SYN) and drives
`ClientState` ∈ {`Closed`, `SynSent`, `Established`, `FinWait`, `Done`}. It sends
a one-shot payload after the handshake and captures the response into a bounded
buffer (`CLIENT_RX_CAP` = 2048). `streaming()` switches it into a *linger* mode
that keeps acknowledging until the peer FINs or the buffer fills — used to pull a
full multi-segment HTTP response for the shell's `get` command.

---

## 5. NIC driver integration (e1000 / e1000e)

The stack is driven by the Intel e1000/e1000e driver
(`unaos/crates/kernel/src/drivers/e1000.rs`). Receive is **interrupt-driven**
via MSI: the NIC's MSI vector is wired to IDT vector **0x41** (`NIC_MSI_VECTOR`),
distinct from the xHCI vector 0x40. The driver maintains DMA descriptor rings
(receive and transmit) and, on each received frame, either runs the stateless
`ingress()` path or feeds the frame to the TCP engine. DMA buffers rely on the
identity-mapped physical memory (allocation pointer used directly as the DMA
physical address).

---

## 6. Testing

Two complementary layers:

- **Host unit tests** — the pure pieces (notably the RFC 6298 estimator) are
  tested with `cargo test -p net`. This is possible because `lib.rs` is
  `#![cfg_attr(not(test), no_std)]`.
- **Loss/reorder injection harness** — `unaos/scripts/net-inject.py` injects raw
  Ethernet frames into the guest over a QEMU socket netdev (4-byte length-
  prefixed frames). It impersonates the gateway with ARP/ICMP/TCP-echo/UDP-echo
  servers and exercises connectivity, retransmission (loss injection), multi-
  extent reordering, pipelining, streaming, and flow-control scenarios against
  the running kernel.

---

## 7. Status and limitations

Implemented: ARP, ICMP (ping responder + client), DHCP client, UDP (echo +
client), and a TCP engine with a multi-connection listener, adaptive RTO,
retransmission, multi-extent out-of-order reassembly, a byte-stream send buffer
with a sliding window, honest receive-window flow control with window updates,
and a streaming client (`get`).

Not yet implemented: a true persist timer for the *peer's* zero window (a coarse
force-send probe is used instead); congestion control; delayed-ACK/Nagle; IP
fragmentation; and a general socket API for userspace (the engine is driven
directly from the kernel today).

---

## See also
- [`docs/dev/OS/`](../) — other kernel subsystem documentation.
- `unaos/crates/net/` — the implementation.
- `unaos/scripts/net-inject.py` — the test harness.
