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

---

## See also
- [`docs/dev/OS/`](../) — other kernel subsystem documentation.
- `unaos/crates/net/` — the implementation.
- `unaos/scripts/net-inject.py` — the test harness.
