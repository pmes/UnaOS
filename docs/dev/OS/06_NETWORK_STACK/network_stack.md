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

## 8. NET6 — the SHARED socket surface on aarch64 (ROADMAP §1b, SOCK-7)

Sections 1–7 describe the hand-rolled `crates/net` line and its x86 home. This section describes what
sits above the **aarch64** smoltcp seam, which is a different thing and is shared.

### 8.1 What existed before, and what was missing

`ORIN-NET-4` (`arch/aarch64/rtl8168_tegra.rs`) and `AARCH64-VNET`
(`arch/aarch64/virtio_net.rs`) each bring a NIC up, bind a **throwaway** `smoltcp::Interface` over its
rings, acquire a DHCP lease, ping once, and drop the interface. That proved the seam — render13 boot 2
leased `10.42.0.171/24 gw 10.42.0.1` on real Orin silicon — and left the network **unusable**: the
shell's `ping`/`arp` resolved to `drivers::e1000::*` off x86 and printed *"No network device ready"* on
a machine whose NIC was up and leased; there was no resolver verb; and EL0 had no socket at all
(`KIND_SOCKET` had sat in `arch/aarch64/syscall.rs` since U6 with the comment *"no net syscall routes
yet"*).

### 8.2 The shape: one surface, many adapters

NET6 (`net6`, default OFF and armed by whichever NIC knob is set — `UNAOS_NET4` / `UNAOS_NET5` /
`UNAOS_VNET` each append it, and `UNAOS_NET6=1` arms it standalone) is that surface, and it is
written **once**:

| Layer | Where | Arch-specific? |
| :--- | :--- | :--- |
| Persistent `Interface` + `SocketSet`, socket registry, gen fence | `net_phy.rs`, tail module `net6` | no |
| Shell verbs `ping` / `arp` / `dns` | same module; `shell.rs` tail routes to them | no |
| EL0 socket family `SYS_SOCKET`(40) … `SYS_SOCK_RECV`(46) | `arch/aarch64/syscall.rs` tail | the arm only |
| **The device adapter** | `virtio_net.rs`, `rtl8168_tegra.rs` | **yes — and only this** |

An adapter is four function pointers and a subsystem name:

```rust
pub struct NicOps {
    pub rx: fn(&mut [u8]) -> Option<usize>,
    pub tx: fn(&[u8]),
    pub mac: fn() -> Option<[u8; 6]>,
    pub link_up: fn() -> bool,
    pub name: &'static str,   // "virtio-net" / "rtl8168" — SUBSYSTEM-named, never board-named (R16)
}
```

registered once at bring-up (`net6::register_nic`) into a lock-free `AtomicPtr`. Nothing above the seam
knows which chip it is running on — which is the point: **`virt` RUNS the code the Orin will run.**

**Why not `smolnet`.** `smolnet.rs` is the x86 DEFAULT stack. New lines in it would move the shipped
x86 image's `panic::Location` records (`./arroyo knoboff` measures exactly that), and `arm_features()`
strips `smolnet` from every aarch64 cargo invocation to hold the aarch64 media byte-identity contract.
`net6` is therefore the aarch64 name for the same idea, and every one of its call sites is a
LINE-NEUTRAL fold onto an existing line or a file-tail item.

### 8.3 Which stack owns the lease (SOCK-7's question, answered on the wire)

There is exactly **one** DHCP client on the aarch64 seam: smoltcp 0.13's `dhcpv4::Socket`, driven by
`net_phy::dhcp_or_static`, which every aarch64 bring-up funnels through. The hand-rolled
`net::dhcp::DhcpClient` has **no aarch64 caller at all** — its only call sites are in
`drivers/e1000.rs`, whose `NET_DEVICE` registry is populated by the x86 PCI bring-up and by nothing on
this arch — so on aarch64 it is compiled, never bound to a NIC, and leases nothing. SOCK-7's *"retire
the hand-rolled … `crates/net` DHCP"* is therefore already true here **by construction**; what was owed
was the WITNESS, and both bring-ups now print it:

```
:: PCIE4:   smoltcp 0.13 Interface BOUND over RTL8168: MAC set, 10.42.0.171/24 + default gw
            10.42.0.1 [dhcp lease-owner=smoltcp-dhcpv4], medium=ethernet, polled OK; link UP …
:: AARCH64 VNET: lease-owner=smoltcp-dhcpv4 (hand-rolled crates/net DHCP has no aarch64 caller) ::
```

The x86 half of the retirement stays owed: there the hand-rolled engine is live and owns `nc` / `nc -u`
/ `curl` and the TCP echo listener. That is a separate arc.

**One lease, one client.** `net6::init` ADOPTS the config `dhcp_or_static` settled on
(`net_phy::settled_config()`) rather than running a DHCP client of its own — two clients for one MAC
could land two different addresses and then the sockets and the wire witness would disagree about
where the machine lives. Exactly one DISCOVER/REQUEST pair ever leaves the NIC.

### 8.4 The EL0 socket family on aarch64

SOCKNUM (WINX-1) moved the family to 40..48 so that *a number names the same verb on every arch*. x86
has answered 40..46 since SOCK-2/SOCK-3; aarch64 now answers the same seven, over the same shared
stack, with the same capability model — not a second one:

* a socket handle is `KIND_SOCKET` whose value word is the gen-fenced `(gen << 32) | (sid + 1)`;
* the generation is validated against the live registry at **every** use, so a stale handle to a
  freed-and-reused slot is refused and never rebinds (the SOCK-3 / U11x fence);
* send needs `CAP_WRITE`, recv needs `CAP_READ` (so `SYS_CAP` GRANT can still attenuate a socket to
  send-only or recv-only), and the mint carries `CAP_GRANT`;
* every user buffer is range-checked through `copy_from_user` / `copy_to_user` before any dereference;
* every verb is NON-BLOCKING and iteration-bounded, because the handler runs IF-masked: `recvfrom` /
  `sock_recv` return `-EAGAIN`, `connect` returns `0` / `-EINPROGRESS` / `-ECONNREFUSED`;
* `clear_handle_row` closes every socket a dying address space still owns, so an exiting program frees
  its registry slots.

### 8.5 What proves it

QEMU models **no** Tegra234, so an Orin build can never self-prove at runtime — a jetson green
certifies that the code COMPILES AND LINKS. The behaviour is proven on QEMU `virt`, over
`virtio_net.rs`, through byte-for-byte the same shared code:

```
UNAOS_GICV3=1 UNAOS_VIRT_EL0=1 UNAOS_VNET=1 UNAOS_NET6=1 ./arroyo test-arm 60
```

which runs, in order:

1. `net6::fixture()` at bring-up — a UDP round-trip through the persistent set (`open`/`bind`/`sendto`/
   `recvfrom`: the exact syscall bodies), a TCP client round-trip (`open`/`connect`/`send`/`recv`) to
   slirp's DNS-over-TCP port, and an ICMP echo to the gateway with per-sequence RTT lines;
2. the `dns` verb;
3. `net6_el0_witness()` from `virt_el0_verdict` — a flat, position-independent, one-code-page **EL0**
   program (the KILLBOUND shape, `spawn_user_image_bg`, no fixture file on any card) that walks all
   seven syscalls and reports a bitmask, then parks in `SYS_FUTEX`. It parks rather than exits because
   the VIRT-EL0 verdict requires `exited_ok == 1` (exactly `el0-hello`); a second clean exit would red
   a leg that has nothing to do with this arc.

Wire shape:

```
:: NET6: stack UP over virtio-net: 10.0.2.15/24 gw 10.0.2.2 dns 10.0.2.3 lease-owner=smoltcp-dhcpv4 [dhcp] sockets=4 ::
:: NET6: fixture: shared socket surface over virtio-net (lease-owner=smoltcp-dhcpv4) ::
:: NET6: sock udp round-trip 99 bytes from 10.0.2.3:53 -> PASS ::
:: NET6: sock tcp round-trip 10.0.2.3:53 sent=26 recv=101 -> PASS ::
:: NET6: ping 10.0.2.2 seq=1 rtt_ms=0 -> REPLY ::
:: NET6: ping 10.0.2.2 seq=2 rtt_ms=1 -> REPLY ::
:: NET6: ping 10.0.2.2 seq=3 rtt_ms=0 -> REPLY ::
:: NET6: ping 10.0.2.2 seq=4 rtt_ms=0 -> REPLY ::
:: NET6: ping 10.0.2.2 4/4 replies over virtio-net peer 52:55:0a:00:02:02 -> REPLY ::
:: NET6: dns una.os -> SERVER ERROR rcode=3 (server 10.0.2.3) ::
:: NET6: fixture: 3/3 legs passed -> PASS ::
:: NET6: el0 socket family over virtio-net — socket=true bind=true sendto=true recvfrom=true socket-tcp=true connect=true send=true sock_recv=true -> PASS ::
```

That block is the CAPTURE, not a sketch: it is `awk 'index($0,":: NET6:")'` over the gate-3 log of
the merged tree (§8.7), all twelve lines it emitted, in order. Two of them read differently from
what an author would guess and are quoted BECAUSE they do. `dns una.os` answers **SERVER ERROR
rcode=3** — slirp's resolver is real and `una.os` is a name it does not have, so NXDOMAIN is the
correct answer and the round trip is what the leg proves: the query was built, sent, matched by
transaction id and parsed. A resolver that invented an address here would be the defect. And
`fixture: 3/3 legs passed` counts UDP, TCP and ICMP — the `dns` verb runs beside the fixture and is
not one of its three legs, which is why 3/3 and an NXDOMAIN sit in the same capture without
contradiction.

⚠ **This capture is the 2026-09-12 merged tree and is kept as that record.** SO47 (§8.8) added two
legs, so the current fixture prints `fixture: 5/5 legs passed -> PASS ::` and the block above is
missing the `arp` and `dns-failpath` lines. §8.8 carries the current capture.

Compile coverage of the ARMED polarity is two KERNEL_CFG_MATRIX legs, not one: `arm-virt-net6`
(`virt_el0,vnet,net6` — the only leg that compiles the EL0 fixture launcher, and deliberately carries
no board term, so the surface is proven board-free) and `arm-tegra-net6` (`tegra,net4,net5,net6` — the
metal adapter). Both adapters must type-check against the one surface or the ONE OS claim is only true
of whichever a gate happened to build.

### 8.6 Limits, stated

* The `virt` EL0 fixture's peer address is **baked** (`10.0.2.3:53`): QEMU user-mode networking always
  serves DNS there. It is a `virt` instrument; an Orin's peers come from the lease.
* `NSOCK = 4` concurrent sockets, 1 KiB datagrams, 2 KiB stream rings — all BSS, no heap.
* No `listen`/`accept` on aarch64 yet (x86's SOCK-6/7 server side); the client halves are here.
* Live ICMP/ARP on the Orin's real link remains **attended-metal** (orin-ledger A59).
* The NET6 neighbour table (§8.8) is 8 entries, TTL 120 s, learned only from ARP frames this boot. It
  is not smoltcp's cache and does not feed it — smoltcp re-resolves per interface as it always did.
* ~~OWED: `dns` is absent from midden_core's `HOST_VERBS`.~~ **LANDED** — `("dns", Avail::Always)`
  is in the `// network` group (`libs/sys/midden_core/src/lib.rs:336`) and the verb is reachable from
  the shell; §8.8.1 argues the `Avail` and records what it costs `shell.rs`. Still owed, one word: the
  help NETWORK line (`lib.rs:622`) lists ifconfig/ping/arp and not `dns`.

### 8.7 Re-gated on the `hw-jetson` merge (2026-09-12)

NET6 was first written at `cc3ca3e8` and committed at a spend wall with its gates incomplete. The
track then moved 36 commits (APPNAME, DRAGSTALL, SDARG/SDWRITE, APPPIN, GA10B5/5B, the WCDFLOOD cfg
fix `83ee4653`), so the tree the gates had to certify is the MERGE, not the arc. A fold of two green
commits is a new configuration and is re-gated whole (LAWS §3). All five ran on `d48292af`:

| # | command (from `unaos/`) | rc | log |
|---|---|---|---|
| 1 | `./arroyo check` | 0 | `1.log` — 94 legs green, `kernel cfg coverage OK (69 legs)` |
| 2 | `UNAOS_TEGRA=1 ./arroyo check` | 0 | `2.log` — 94 legs green |
| 3 | `UNAOS_QEMU_FULL=1 UNAOS_GICV3=1 UNAOS_VIRT_EL0=1 UNAOS_VNET=1 UNAOS_NET6=1 ./arroyo test-arm 60` | 0 | `3.log` — the twelve `:: NET6:` lines of §8.5 |
| 4 | `env -u UNAOS_TEGRA ./arroyo knoboff net4 2c4e7a73` | 0 | `4b.log` — both arches byte-identical, control fired |
| 5 | `UNAOS_TEGRA=1 UNAOS_NET4=1 UNAOS_NET5=1 ./arroyo esp-jetson` | 0 | `5.log` — the tegra ESP, certified below |

(Logs under `~/unaos-bench/scratch/orin-0912b/net6/`.) Gate 3 runs under `UNAOS_QEMU_FULL=1` because
LAWS §5 says the full wall is the form an arc's DONE gate takes: a fault emitted inside the grace reds
both modes, but one emitted beyond it reds only the full wall.

**Gate 4's baseline is the SECOND parent, and that is not a detail.** `knoboff` defaults to `HEAD~1`
(`arroyo:8435`), which on a merge commit is the FIRST parent — here `bd3d0113`, this arc's own pre-merge
tip. Run that way it reports `MOVED` on both arches and is RIGHT to: the two images differ by all 36
commits of track code (x86 grew 640 bytes, so "the SIZE changed, this is code, not a line shift" — and
it was). That answer is true and useless, because the question knoboff exists to ask is whether THIS
ARC's knob-off image moved. On a merge tree the tree-without-the-arc is the other parent, so the
baseline is named: `knoboff net4 2c4e7a73`. It then reports byte-identical on both arches with the
control fired (`arm armed≠off: YES`), and the current-tree hashes are the same two values the
first run printed — the same measurement, read against the right baseline.

**Artifact certification** of the gate-5 ESP kernel, `LC_ALL=C grep -a -o -F` on
`target/aarch64_esp/kernel.elf` (2,335,632 B) — never `strings`, per LAWS §5:

```
:: NET6:                              1     lease-owner=                        2
 -> is-at                             1     smoltcp-dhcpv4                      2
 seq=                                 3     stack UP over                       2
 rtt_ms=                              2      -> A                               1
-> REPLY ::                           1     NO RESOLVER (no lease, no gateway)  1
-> NO REPLY ::                        1
:: NET7:                              0  (control, must be 0)
ZZ-NOT-IN-THIS-BUILD                  0  (control, must be 0)
```

`:: NET6:` is **1**, not twelve: `P6` is one `pub const &str` (`net_phy.rs:817`) that every witness
formats against, so `.rodata` holds a single copy and a count of 1 is the whole family present. The
per-verb fragments above are what a reader should actually grep, and they are listed because a count
of one on a deduplicated constant cannot distinguish "all the verbs shipped" from "one of them did".
The two zero rows are the controls: ten tokens in the same invocation come back non-zero, so a zero
is a fact about the artifact and not about the pattern.

⚠ **`-> NO REPLY ::` reading 1 here is the 2026-09-12 ELF and MUST NOT be used as a checklist
now.** SO47 (§8.8.2) replaced ARP's contiguous format string, so that fragment is 0 in the current
artifact and a scorer keyed on it would red a healthy build — while still firing on the WIRE for a
failing `ping`. Score `arp` on `NO REPLY (cache miss,` instead. `-> A ` likewise reads 0 now (the
resolved verdict moved into the `DnsSay` renderer); its replacement discriminator is ` (server `.

Three witness strings are **absent from this artifact, correctly**: `sock udp round-trip`,
`sock tcp round-trip` and `el0 socket family over` all read 0. They belong to `net6::fixture()` and to
`virt_el0_verdict`, which are `virt_el0`-gated; `esp-jetson` builds `tegra_el0` and builds witness-FREE
(`arroyo:44` arms `witness` for exactly the four battery commands). A card is not supposed to carry the
fixtures — it carries the verbs.

**The card knob line, measured rather than assumed:**

```
UNAOS_TEGRA=1 UNAOS_NET4=1 UNAOS_NET5=1 ./arroyo esp-jetson
```

**`UNAOS_NET6=1` is NOT needed on it.** The knob map appends `net6` to the feature set from the NIC
knobs themselves (`arroyo:1847`, `:1862`), and the run's own banner is the proof, not the mapping:

```
⚡ kernel features (jetson): ehcihid,kbdwit,sdhcblk,smolnet,tegra,bsptick,bsprun,tegrasmp,apsrun,net4,pcie3,pcie2,net6,net5,sdmmc
```

`net6` is in that set with no fourth knob on the line, and the certification above is of the artifact
that set built. The standalone `UNAOS_NET6=1` knob still exists for the `virt` runtime gate, where no
NIC knob would otherwise arm the surface.

### 8.8 SO47 — `arp` and `dns` on render14, and the two separate defects in the two verbs

Render14 boots 4 and 5 typed three verbs at the glass on one boot, over one interface, and got this:

```
:: NET6: ping 10.42.0.1 4/4 replies over rtl8168 peer 9c:69:d3:28:6e:f4 -> REPLY ::
:: NET6: arp 10.42.0.1 -> NO REPLY ::
:: [midden] cmd="dns google.com" -> TerminalError len=44 ::
```

Ping works **and learns the peer MAC**; `arp` then reports NO REPLY for that same address; `dns`
produces no `:: NET6:` line at all. Neither defect is in the stack under the verbs — the stack carries
ICMP both ways and resolves L2 for exactly the address `arp` says it cannot.

**`arp`: the verb never had a neighbour table to read.** smoltcp 0.13.1 keeps its neighbour cache as a
private field of `InterfaceInner` (`neighbor_cache`, `src/iface/interface/mod.rs:134`) and publishes no
reader — `NeighborCache::lookup` is `pub(crate)` — so a verb cannot ask smoltcp what it already
resolved. Every NET6 verb therefore built a THROWAWAY `Interface` with an EMPTY cache, and `arp`'s only
source of truth was one ARP reply captured inside its own 2 s budget. That is measurably fine on a clean
wire and measurably not on this NIC: with the verb UNCHANGED, the `virt` leg prints
`:: NET6: arp 10.0.2.2 -> is-at 52:55:0a:00:02:02 ::` right after a 4/4 ping, while on the Orin the
inbound payload path drops frames outright — render14 boot4 scores six
`[net5T] … verdict=NOWHERE — the payload never reached this DRAM at all` in twelve pops, several of them
60-byte, i.e. ARP-reply-sized (A64). Ping survives that because it needs one of four echo replies; `arp`
needed one specific frame and threw away the answer the machine had already learned.

The two candidate shapes the brief ranked ahead of this one are **refuted by measurement**, not by
argument: the verb does not wait on a queue nothing feeds (it polls the NIC ring directly and the same
loop resolves on `virt`), and its budget is not short (ping's first reply on that boot took 524 ms
against the same `VERB_BUDGET_MS = 2000`, and boot4's `[gui] app-enter t=118s` / `app-exit t=120s`
shows `arp` burning the full two seconds). A third reading has to be refused too: the absence of
`[net4F]`/`[net5T]` lines during the `arp` window is NOT evidence of a dead wire — both witnesses are
capped (8 and 12 pops on that boot) and were exhausted during the ping (LAWS §5, an absence is evidence
only if the producing path could run).

The fix is a NET6-owned neighbour table (`net_phy.rs`, `mod net6`): every ARP frame any phy in the
module receives is learned — **both opcodes**, since a request carries the sender's IP and MAC in the
same fields a reply does — the persistent stack's phy now carries a `Learn` observer so the table stays
warm between verbs, and `arp` reads the table first and probes only on a miss. Table-first is also what
`arp <ip>` means everywhere else (R26): it is the neighbour table, not a ping. The witness names its
source and the entry's age, so a cached answer can never be read as a fresh round trip:

```
:: NET6: arp 10.42.0.1 -> is-at 9c:69:d3:28:6e:f4 via=cache age_ms=19312 ::
:: NET6: arp 10.42.0.1 -> is-at 9c:69:d3:28:6e:f4 via=wire  age_ms=0 ::
:: NET6: arp 10.42.0.1 -> NO REPLY (cache miss, wire probe 2001 ms, learned=3) ::
```

` -> is-at ` is held byte-for-byte (A59's go-red shape and the artifact certification both count that
fragment). `learned=` on the failure line is a control: `0` says the learn path never ran at all, which
is a different defect from "this one address is unknown", and the wire must be able to tell them apart.

**`dns`: the verb is never reached, and could not have said so.** `len=44` is exactly
`"Unknown command. Type 'help' for assistance."` — the `Plan::Say(TerminalError)` fallthrough at
`libs/sys/midden_core/src/lib.rs:512`. Since MIDDEN-M1 there is ONE command table and it is
midden_core's `HOST_VERBS`; its `// network` group registers `ifconfig`, `ping`, `arp`, `nc`, `curl`
(`lib.rs:274-275`) and **not `dns`**. The `"dns" => net6_shell_dns(…)` arm at `shell.rs:5594` has
therefore never been reachable on any build, which is why a bare `dns` with no arguments returns the
same 44-byte error as `dns google.com`. **The one-line fix — `("dns", Avail::Always),` in that
`// network` group — has since LANDED; §8.8.1 is the argument for it and §8.6 the current state.**

What this arc lands instead is the half that made the boot unreadable. The verb already printed on
every path, but by ten separate `serial_println!` calls that a future arm could silently skip, and
three of them did not name the resolver. `dns` is now split into a printing-free `dns_lookup` returning
a `DnsVerdict`, and **one** emission point every arm reaches — so "witnesses every path, naming the
resolver and the reason" is a property of the shape rather than of a reviewer noticing. `BIND FAILED`
is split out of `SEND FAILED`, which previously conflated two different failures under
"socket unusable". A `DNS_WITNESS` counter is bumped at that one point, and the `virt` fixture asserts
it moves by **exactly one** across a lookup that returns no address.

**Proof, on `virt`, through the identical shared code** (`UNAOS_QEMU_FULL=1 UNAOS_GICV3=1
UNAOS_VIRT_EL0=1 UNAOS_VNET=1 UNAOS_NET6=1 ./arroyo test-arm 60`, rc=0). The fixture is now five legs;
legs 4 and 5 are this arc's:

```
:: NET6: ping 10.0.2.2 4/4 replies over virtio-net peer 52:55:0a:00:02:02 -> REPLY ::
:: NET6: arp 10.0.2.2 -> is-at 52:55:0a:00:02:02 via=cache age_ms=2 ::
:: NET6: fixture arp: answered from the neighbour table the ping filled -> PASS ::
:: NET6: dns a..b -> BAD NAME (unencodable) (server 10.0.2.3) ::
:: NET6: fixture dns-failpath: witnesses=1 on a path that returned no address, resolver named -> PASS ::
:: NET6: dns una.os -> SERVER ERROR rcode=3 (server 10.0.2.3) ::
:: NET6: fixture: 5/5 legs passed -> PASS ::
```

`a..b` carries an empty label, which `net_dns::build_query` refuses (`net_dns.rs:86`), so leg 5 takes
the BAD NAME arm deterministically on every platform.

**Both legs were made to fail by mutation** (LAWS §5 — a check that cannot fire is an absent one). One
run with `neigh_learn` commented out of `snoop_arp` AND the `DNS_WITNESS` bump commented out of the
emission point returned rc=1 with:

```
:: NET6: arp 10.0.2.2 -> is-at 52:55:0a:00:02:02 via=wire age_ms=0 ::
:: NET6: fixture arp -> FAIL — resolved, but the table was EMPTY after a 4/4 ping: the learn path did not run ::
:: NET6: fixture dns-failpath -> FAIL — the failing lookup emitted 0 witnesses, not 1 ::
:: NET6: fixture: 3/5 legs passed -> FAIL ::
```

The `via=wire` line in that capture is the control that matters: with the learn path dead the verb
still resolves off the wire, so leg 4 is measuring the TABLE and not merely whether `arp` answered.

#### 8.8.1 `dns` is a verb now — and why `Avail::Always`, not `Avail::Aarch64`

`("dns", Avail::Always)` joins the `// network` group in `libs/sys/midden_core/src/lib.rs`. The
alternative was `Avail::Aarch64`, whose comment reserves it for exactly this case ("a verb whose ring
arm genuinely does not compile off aarch64"). It is the wrong answer, and not as a matter of taste:

* **A gate in that same file already forbids it.** `no_verb_is_pinned_to_a_platform_without_a_capability`
  (`lib.rs:969-976`) asserts `!matches!(a, Avail::Aarch64)` over every member of `HOST_VERBS`, so the
  variant is empty BY TEST, not by convention. Measured rather than read: registering `dns` there
  failed **2 of 16** tests — `lib.rs:972` and `lib.rs:940`, "``dns`` must be a verb on
  `Facts { aarch64: false, .. }`". The enum's invitation and the enum's gate disagree; the gate wins,
  and that contradiction is now written at the variant so the next reader does not re-litigate it.
* **It would not have been narrower.** The arm is `all(net6, aarch64)`; `Aarch64` is `aarch64`. On an
  aarch64 build with `net6` off the word is a verb with no arm under BOTH spellings, so both reach the
  drift net. The only half `Aarch64` changes is x86 — and there it does not make the verb absent, it
  makes it indistinguishable from a typo: `facts.exec` is true on x86, `resolve_exec("dns")` finds no
  `DNS.ELF`, and the shell answers with **the very 44-byte `TerminalError` this fix exists to delete**.
* **R26 clause 3 ruled this case already.** `burst` and `simmer` were `Avail::Aarch64` "for no hardware
  reason" and were moved to `Always`, the ring arm left to refuse honestly and by name. `dns` has no
  hardware reason either: a resolver is not a device, x86 already resolves through `smolnet::resolve`,
  and the missing x86 `dns` verb is an UNWRITTEN ARM, not an absent capability.

**Exact was considered and is out of reach from one file.** `Facts` carries no `net6` fact; an
`Avail::Net6` + `Facts::net6` needs a PRODUCER in `midden_facts()` (`shell.rs`) to set it, and added
without one the field is false everywhere and the verb registers nowhere — strictly worse than today.
One field, one variant and one line for whoever holds both files; overkill for any smaller reason than
closing the drift named next.

**The knob-off image MOVES, and the direction is the point.** `./arroyo knoboff net6 09ddb1c4`
is **exit 1 — MOVED on both arches**, and that is correct rather than tolerated: the registration is
deliberately NOT `net6`-gated, because ONE OS means the word exists on every build and the ring arm
decides the answer. The move is isolated by measurement, not by argument. Two runs, two baselines:

| baseline | x86 knob-off | arm knob-off | delta to current |
|---|---|---|---|
| `09ddb1c4` (track tip, no arc) | 1 529 300 | 1 595 624 | x86 **+128**, arm **+64** |
| `557afe47` (the arc tip, all net6 code present) | 1 529 300 | 1 595 624 | x86 **+128**, arm **+64** |

The two baselines' knob-off images are **byte-identical** — `sha256` x86
`1b4764b522136ed7f5587c5ed9634d5c0e46f20fb80c48348f4e3f7dc9ac8729`, arm
`7cca34b5a17bbf7f35345b5fd251b4ff8c2e979d672605055072819d361ba103` — so the whole NET6 arc
contributes **zero** to the knob-off image (`knoboff net6 b8f5ded9` at `557afe47` said the same thing
directly, exit 0 with the control fired), and so does the track's own movement (`arroyo` + docs). The
entire +128/+64 is one `(&str, Avail)` row plus a 3-byte string plus `.rodata` alignment: data, not
code, and growth by exactly one table entry.

⚠ **What this cost `shell.rs`, and it is PAID (WIREHYG, A83).** `shell.rs:6103-6122` documented the
`other =>` drift net as unreachable by construction — "that set is empty today, because every `Avail`
in `HOST_VERBS` mirrors the `#[cfg]` on its arm below exactly". With `dns` registered that was no
longer true: on a build without `all(net6, aarch64)` the word reaches the net and prints `dns: not
available on this build (the verb exists; this kernel does not carry it)`. The BEHAVIOUR is right —
that is R26 clause 3's honest refusal, arriving through the net instead of a dedicated arm — and only
the COMMENT overstated. It now names `dns` as the one member of that set and why. The rewrite is
LINE-NEUTRAL (20 lines in, 20 out, `shell.rs` 7668 lines both sides): a comment that changes the line
count shifts every `panic::Location` below it and moves the knob-off image (LAWS §5). Measured rather
than argued — `env -u UNAOS_TEGRA ./arroyo knoboff net6 <this branch's merge commit>` is **exit 0,
byte-identical on both arches, control fired**. The baseline is the MERGE commit, not `knoboff`'s
`HEAD~1` default and not a track tip: the tree-without-this-commit is the only baseline that can
isolate a comment rewrite.

#### 8.8.2 The retired wire strings, and the one that is not retired at all

Swept `LC_ALL=C grep -rn -a -F` over the whole repo (excluding `.git`/`target`) plus `unaos/scripts/`,
`unaos/scripts/specs/*.spec` and `~/unaos-bench/tools/`.

**No EXECUTABLE scorer is keyed on either string** — zero hits in any `.spec`, in `mbench.py`, in
`foreman`, or in the bench tools. Every hit is prose. (One apparent hit,
`unaos/scripts/specs/pi4-regression.spec:1868`, is a substring false positive: the word is
*mis-**at**tributes*, which contains `is-at`. Worth knowing — bare `is-at` is not a safe token, which
is why the certification above greps the SPACED ` -> is-at `.)

The prose that would mis-score, each with its file:line:

| where | what it says | status |
|---|---|---|
| `docs/dev/OS/orin-ledger.md:90` (A59's status cell) | the next-flight go-red: "`arp 10.42.0.1` -> `:: NET6: arp 10.42.0.1 -> is-at <router mac> ::`", and "A `-> NO REPLY ::` … on the gateway with `link UP` is the next question" | **FIXED — WIREHYG (A83).** The arp line now carries ` via=… age_ms=…` after the MAC, arp's failure is `-> NO REPLY (cache miss, …)`, and the cell says which verb owns the bare fragment. |
| `unaos/arroyo:2054` (the `net6` knob's own doc block) | "`:: NET6: arp <ip> -> is-at <mac> ::`, `:: NET6: dns <host> -> A a.b.c.d ::`" | **STALE — `arroyo` is a gate file outside this arc's list, reported not edited.** Missing `via=`/`age_ms=`, and `dns` now always carries the `(server …)` suffix. |
| `network_stack.md:423` (§8.7's certification table) | `-> NO REPLY ::  1` | historical record of the 2026-09-12 ELF; annotated in place at §8.7. |

⚠ **The finding that matters most: `-> NO REPLY ::` is NOT retired from the wire.** It left `.rodata`
only because ARP's old format string was one contiguous literal and is now
`-> NO REPLY (cache miss, wire probe {} ms, learned={}) ::`. **PING still emits the exact bytes** — its
summary formats `" -> {} ::"` against `if received > 0 { "REPLY" } else { "NO REPLY" }`
(`net_phy.rs:1365`), which is also why the fragment was never in `.rodata` for ping in the first place.
Measured, not reasoned — a temporary fixture ping at an address slirp does not host:

```
:: NET6: ping 10.0.2.99 0/1 replies over virtio-net peer ----------------- -> NO REPLY ::
:: NET6: arp 10.0.2.2 -> is-at 52:55:0a:00:02:02 via=cache age_ms=2002 ::
```

`LC_ALL=C grep -a -o -F -e '-> NO REPLY ::' target/serial-arm.log | wc -l` = **1** on that capture.

So the hazard is worse than "the string is gone", and it cuts both ways:

* a **wire** scorer for `-> NO REPLY ::` still fires — but now only for PING, never for `arp`. One
  written to catch a failing ARP goes quietly green on exactly the failure it was built for.
* an **artifact** scorer for the same fragment now reads **0**, and would red a build that is fine.

A fragment that survives in one channel and dies in the other is the worst version of this class,
because the scorer keeps producing plausible output. **Score `arp`'s failure on
`NO REPLY (cache miss,`** — a token no other verb can reach.

#### 8.8.3 WIREHYG — the sweep widened, and three more tokens that do not mean what they say

§8.8.2's sweep was re-run wider (A83, 2026-09-13) and its two negative results are **confirmed
independently, not inherited**:

* **No executable scorer keys on any NET6 token.** `LC_ALL=C grep -rn -a -F` for `-> NO REPLY ::`,
  `is-at`, ` -> A `, `:: NET6:`, ` rtt_ms=`, `lease-owner=`, `via=cache`, `-> TIMEOUT ::`,
  `NO RESOLVER` and `SEND FAILED (socket unusable)` over every `*.spec`, `*.sh`, `*.py`,
  `unaos/arroyo` and `~/unaos-bench/tools/` returns **zero directives**. The only hits inside an
  executable file are the four comment lines `unaos/arroyo:2053-2056`, which are documentation, not
  a check. The NET6 family's scoring surface is entirely PROSE.
* **`pi4-regression.spec:1868` is a substring false positive**, read at the line: it is a `# ---`
  prose line whose word is *mis-**at**tributes*. Nothing there scores `is-at`.

**Three tokens, found by measurement, that an asserter gets wrong** — output of
`docs/dev/evidence/orin28/scorer-token-uniqueness.sh --list <tokens>`, which maps a token to every
kernel site that can emit it and to the VERB each of those sites names:

| token | asserted by | measured | why it matters |
|---|---|---|---|
| `-> NO REPLY ::` | A59's go-red, as `arp`'s failure | **1 emitter, verb `ping`** (`net_phy.rs:1360` formats ` -> {} ::` against `:1365`'s `"NO REPLY"`; tag `arg` — composed at runtime) | the headline of §8.8.2, now mechanised: the bytes did not disappear, they changed verbs |
| `-> TIMEOUT ::` | A59's go-red, as the gateway question | **SHARED: `ping` (`net_phy.rs:1345`), `JB5` (`bpmp_tegra.rs:501`), `JB7` (`bpmp_tegra.rs:537`)** | on a `UNAOS_TEGRA=1` boot the BPMP prints it during bring-up, hundreds of lines BEFORE the shell exists — a scorer keyed on it reads a power-gate or clock timeout as a network answer |
| ` (server ` | §8.7's own annotation, as `dns`'s replacement discriminator for the retired ` -> A ` | **SHARED: `dns` (`net_phy.rs:1484`, `:1491`), `NET: DHCP lease … (server …)` (`net_phy.rs:366`), `[net4j] … (server identifier)` (`rtl8168_tegra.rs:2727`)** | **this is a correction to the prescription §8.7 made one day earlier.** On a leased Orin boot `net_phy.rs:366` fires first, so ` (server ` counts >= 1 on a boot where `dns` was never typed |

` -> A ` measures **0 emitters**, which confirms §8.7's annotation: the resolved arm now renders
through `DnsSay` (`net_phy.rs:1540`, `write!(f, "A {}.{}.{}.{}")`), so the space-`A`-space run is no
longer contiguous anywhere. The sound replacements are the DnsSay phrases, each of which is unique
and contiguous: `NO A RECORD`, `BAD NAME (unencodable)`, `NO ANSWER within budget`,
`SERVER ERROR rcode=`, `MALFORMED REPLY`, `BIND FAILED (no ephemeral port)`,
`SEND FAILED (sendto refused)`. For the WIRE, `dns`'s own discriminator is the whole prefix
`:: NET6: dns ` — and note it is DEAD as an ARTIFACT token, because `:: NET6:` is the `P6` constant
in a `{}` hole and never adjoins ` dns ` in `.rodata`. Wire token and artifact token are different
objects and this family needs both named.

**The rule that comes out of it** is one line in `LAWS.md` §5, with
`docs/dev/evidence/orin28/scorer-token-uniqueness.sh` as its enforcer: a scorer keys on a token
UNIQUE to the verdict it scores, never on a fragment another emitter can reach. The script's
`--selftest` carries three controls (the composed `ping` case must read 1, the prescribed `arp`
token must read 1, an invented token must read 0) and its go-red is `--verb arp '-> NO REPLY ::'`,
which exits 1 with `WRONG-VERB(want arp) … verbs=ping`.


---

## See also
- [`docs/dev/OS/`](../) — other kernel subsystem documentation.
- `unaos/crates/net/` — the implementation.
- `unaos/scripts/net-inject.py` — the test harness.
