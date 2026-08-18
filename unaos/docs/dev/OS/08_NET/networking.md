# Networking — the smoltcp stack (DEFAULT since SMOLNET-DEFAULT)

Direction: [`ROADMAP.md` §1b](../../../../../docs/ROADMAP.md). This is the **doc of record** for
UnaOS networking. As of **SMOLNET-DEFAULT** (2026-07-17, Peter's ruling — "make smoltcp the default,
retire the hand-rolled line but don't shut out the possibility we resume hand-rolling our own") the
mature [smoltcp](https://github.com/smoltcp-rs/smoltcp) crate (0.13.1, 0BSD, `#![no_std]`,
heap-optional) is the **default** x86 TCP/IP stack. It got here through the SOCK-1..7 + zeolite arcs
(round 9 onward, from 2026-07-12), each documented in full below.

## Today's stack (smoltcp default, hand-rolled opt-out)

smoltcp owns the shell's `ping`/`arp`/`netinfo`, the ring-3 socket syscall family (SOCK-2..7), the
DNS sinkhole (zeolite), and the boot connectivity witnesses — **by default**. The hand-rolled
`crates/net` line is **retired as the default** but stays in tree, live, and resumable (see
[The retired hand-rolled line](#the-retired-hand-rolled-line-resumable) below).

| | Default (smoltcp) | Opt-out (`UNAOS_NOSMOLNET=1`) |
| :--- | :--- | :--- |
| `ping` / `arp` / `netinfo` | **smoltcp** ICMP socket + interface (`smolnet.rs`) | hand-rolled `net::` engines (`drivers/e1000.rs`) |
| ring-3 socket syscalls (SOCK-2..7) + zeolite | **smoltcp** (persistent `STACK`) | absent (feature not compiled) |
| `connect` / `fetch` / `udpsend` | hand-rolled `net::tcp` / `net::udp` (smoltcp has no shell equivalent yet — SOCK-9+) | hand-rolled |
| DHCP, TCP echo listener, boot connectivity self-test | hand-rolled (still runs alongside smoltcp) | hand-rolled |
| aarch64 | **smoltcp is never compiled** (x86-only dep + arm_features strip) | identical — hand-rolled only |

Note the hand-rolled `crates/net` crate is a **live dependency regardless of the knob**: even when
smoltcp is the default, the driver's `service_net()` poll, the boot self-test, DHCP, the TCP echo
listener, `connect`/`fetch`/`udpsend`, and the `net::arp::learn` reuse the smoltcp `Device` snoops for
MAC surfacing all run through it. The `smolnet` feature is purely **additive**.

## The knob

`smolnet` is the kernel crate's cargo feature; it activates an **x86-only optional dependency** on
`smoltcp` (declared in `crates/kernel/Cargo.toml` under `[target.'cfg(target_arch = "x86_64")'.dependencies]`)
and its module + every call site are additionally `#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]`.
It is **pushed by default** on x86; `UNAOS_NOSMOLNET=1` opts out (the negative-knob idiom mirroring
PORTSW-1/EHCI-4). So:

- **Default, x86** → `smolnet` compiles; `smolnet.rs` and the shell/witness/socket call sites compile in.
- **Opt-out (`UNAOS_NOSMOLNET=1`), x86** → the feature is dropped, smoltcp is never pulled, and the
  binary is byte-identical to the pre-flip x86 default (the hand-rolled stack is the whole net path).
- **aarch64 (either way)** → **byte-identical**, enforced at build. smoltcp is x86-only (the optional
  dep does not exist for aarch64, and every call site is `target_arch = "x86_64"`-gated), so the
  aarch64 compiler never emits one byte of smoltcp code. But cargo hashes the *enabled-feature set*
  into each crate's `-Cmetadata` fingerprint, so merely *listing* `smolnet` in the aarch64 feature set
  would shift the aarch64 binary's bytes (same code, same size, different symbol manglings). To keep
  the aarch64 (jetson/pi) media hash unperturbed, `unaos/arroyo`'s `arm_features` helper **strips
  `smolnet`** from the two shared aarch64 kernel compiles (`build_kernel_aarch64`, `check_both`'s
  aarch64 leg). Proven: the aarch64 kernel is byte-identical (default == opt-out == pre-flip
  `ehcihid` baseline). Pi `kernel8` builds from its own curated `K8_FEATS` (never carried smolnet).

The knob is plumbed in **two** places that must stay in sync (both rebuild the x86 kernel):
`unaos/arroyo` (the feature map — pushes `smolnet` unless `UNAOS_NOSMOLNET` is set; strips it for
aarch64 via `arm_features`) and `unaos/builder/src/main.rs` (the builder rebuilds the x86 kernel
before launching QEMU, so it maps `UNAOS_NOSMOLNET` → drop `smolnet` independently; it produces only
x86 media, so no aarch64 strip is needed there).

## The retired hand-rolled line (resumable)

The hand-rolled `crates/net` stack (ARP / ICMP / UDP / DHCP + the Go-Back-N TCP engine) is **retired
as the default**, not removed. Per never-trash-code it stays in tree — catalogued in
[`crates/net/README.md`](../../../../crates/net/), architecture in
[`06_NETWORK_STACK/network_stack.md`](../../../../../docs/dev/OS/06_NETWORK_STACK/network_stack.md) —
and remains **live** (it backs the surfaces smoltcp has not replaced, listed in the table above). To
**resume hand-rolling our own** — or to run the hand-rolled stack as the whole net path — build with
`UNAOS_NOSMOLNET=1`; that opt-out x86 build is byte-identical to the pre-flip default.

## The Device seam

smoltcp binds to a `smoltcp::phy::Device`. `smolnet.rs` implements one, `E1000Phy`, over the
existing e1000e RX/TX descriptor rings through **additive** raw accessors on the driver — the
existing `poll()` / `transmit()` paths are untouched:

- `e1000::raw_rx(&mut [u8]) -> Option<usize>` — pops one completed RX descriptor's raw Ethernet
  frame and recycles the descriptor (the same head/tail protocol `poll()` uses), **without** the
  hand-rolled `observe` / `net::ingress` responder dispatch. smoltcp owns the stack when it drives.
- `e1000::raw_tx(&[u8])` — thin wrapper over the driver's private `transmit` (shares the TX ring +
  `tx_count`).
- `e1000::hw_addr() -> Option<([u8;6],[u8;4],bool)>` — `(MAC, current IP, link-up)` for the
  interface config and `netinfo`.

All three briefly lock `NET_DEVICE` per call. The `E1000Phy` owns its own RX/TX scratch buffers so
smoltcp's RX and TX tokens borrow disjoint fields (smoltcp hands out both from one `receive()` to
build a reply in place). ARP MAC surfacing: smoltcp hides the resolved neighbor MAC, so the Device
snoops inbound ARP replies for the target IP (via `net::arp::learn`, a read-only reuse) — that is how
the `arp` command still prints `is-at <mac>`.

## Discipline and constraints (inherited from the driver)

- **Poll-driven only.** smolnet never runs in the MSI interrupt handler (which stays ack-only). Each
  shell op (`ping` / `arp`) and the boot witness is a bounded, blocking poll-pump on the caller's
  stack; on this single-CPU main loop, `service_net()`'s hand-rolled `poll()` is not running
  concurrently, so the two RX drains never race.
- **Frame-stealing window (knob-on residual).** During a smolnet pump, inbound frames are drained by
  the smolnet `Device`; anything that is not the pump's ICMP/ARP traffic (e.g. a DHCP offer or a
  packet for the hand-rolled TCP `:7` listener) is dropped by smoltcp rather than served — bounded by
  the pump (worst case a silent gateway holds it for `PUMP_ITERS`), same single-CPU semantics as the
  hand-rolled `ping`'s own pump. SOCK-2's persistent `Interface` retires the per-op pump.
- **Fully static / stack-local — no heap growth.** Each op builds a throwaway `Interface` + one ICMP
  socket with fixed-size (stack) socket storage, neighbor cache (smoltcp-internal, fixed), and RX/TX
  scratch. Nothing reaches the heap allocator. smoltcp's neighbor cache re-ARPs per op — that is the
  "ARP-triggering poll" the `arp` command relies on.
- **smoltcp features enabled:** `medium-ethernet` (the e1000e is an Ethernet L2 device; brings
  smoltcp's built-in ARP), `proto-ipv4` (IPv4 addressing / routing / checksums for 10.0.2.0/24),
  `socket-icmp` (the ICMP echo socket). `default-features = false`; no `alloc`/`std`.

## The QEMU witness

The witness medium is QEMU's slirp user-net: its virtual gateway `10.0.2.2` answers ICMP echo from
the guest. `smolnet::witness_tick()` — a one-shot driven from `service_net()` knob-on (after the
`NET_DEVICE` guard is dropped, since the ping pump re-locks per ring op) — pings the gateway ×4
through the smoltcp interface and emits the uncounted witness line:

```
:: SOCK-1: smoltcp icmp echo 10.0.2.2 4/4 replies — witness OK ::
```

Reproduce: `./arroyo test 60` (MISSION SUCCESS + the line above).

## SOCK-2 — the UDP socket syscall family (ring 3 reaches the network)

SOCK-2 adds the first four **socket syscalls** and the persistent smoltcp state they need. It is the
same `smolnet` feature (default-on), x86-only, byte-identical knob-off / aarch64.

### The persistent stack

SOCK-1's ICMP path builds a throwaway `Interface` + one socket per op. A UDP socket must survive
*between* `bind` and `recvfrom` (separate syscalls), so SOCK-2 promotes the interface + a real
`SocketSet` to a persistent singleton, `smolnet::STACK` (`spin::Mutex<Option<SmolStack>>` — the
`NET_DEVICE` mirror). **Everything is static / BSS:** the socket-set storage, every UDP socket's
packet buffers, and the device RX/TX scratch all live in `SmolStack` fields or `static mut` arrays
borrowed `&'static mut` exactly once under the lock (edition-2024: via `addr_of_mut!` +
`from_raw_parts_mut`, never an autoref through a raw deref). No heap. The smoltcp `socket-udp`
feature is added (`default-features` still off).

**Stack-frame discipline.** Because the ~3 KiB device RX/TX scratch lives in `SmolStack` (BSS), only
smoltcp's own ~2 KiB `poll()` frames touch the caller's stack — and only on the **BSP main-loop
witness path** and the **IF-masked syscall path**, never an AP scheduler stack. The persistent stack
is pre-built (`smolnet::init()`) from a large-stack context (the launcher task / the BSP witness)
before any ring-3 `sys_socket`, so the one-time ~4 KiB construction transient never lands on a ring-3
task's 16 KiB syscall stack.

### The syscalls

| # | Syscall | Gate | Returns |
| :--- | :--- | :--- | :--- |
| 19 | `sys_socket(domain, type, proto)` | `SHARED_ROW` refused | a socket handle (UDP/IPv4 only: AF_INET/SOCK_DGRAM) |
| 20 | `sys_bind(handle, port)` | Socket + `CAP_WRITE` | 0, or `-EINVAL` |
| 21 | `sys_sendto(handle, msg_ptr, msg_len)` | Socket + `CAP_WRITE` | bytes sent, or `-EAGAIN` |
| 22 | `sys_recvfrom(handle, buf_ptr, buf_len)` | Socket + `CAP_READ` | header+payload bytes, or **`-EAGAIN`** when empty |

`sendto`/`recvfrom` carry the peer address as an **8-byte header** `[ip[4]][port u16 LE][pad u16]`
prepended to the payload, so the whole exchange fits the 3-argument x86 syscall ABI. User buffers are
bound-checked against the ring-3 window exactly like `sys_write`/`sys_open` (the same cheap window
check; full `copy_from_user` is still a later arc).

**`recvfrom` is NON-BLOCKING.** The syscall handler runs IF-masked (it cannot `hlt`/block), so
`recvfrom` drives a **bounded** poll pump (iteration-, not clock-bounded, exactly like SOCK-1's ICMP
pump) and returns the datagram if one landed, else `-EAGAIN`. Because the pump reads the RX ring
directly (not interrupt-gated) and runs to completion inside one IF-masked syscall, the whole
ARP → egress → reply round-trip can complete within a single `recvfrom` without the main-loop
`service_net()` poll racing it for frames (single CPU holds the core in the handler).

### Sockets as capabilities

A socket is a new object-table kind, `KIND_SOCKET` (already scaffolded as the U6bx/U9x kind
negative), whose value word is the persistent `SocketSet` id (+1-biased so it is never the
0-Empty / `u64::MAX`-RESERVING sentinel). `sys_socket` mints a handle carrying `CAP_READ|CAP_WRITE`;
**send needs `CAP_WRITE`, recv needs `CAP_READ`**, both enforced at the single `handle_resolve`
CHECK the File syscalls use — so `SYS_CAP` GRANT attenuates a socket cap to send-only or recv-only,
and `SYS_XFER` can hand a socket to another process, for free. A dying task's sockets are freed at
`clear_handle_row` teardown (the persistent socket + its static buffers reclaimed), so a reused slot
inherits none.

### The round-trip witness

The witness medium stays slirp — its **built-in DNS server at `10.0.2.3:53`** answers a DNS query
with a real UDP datagram, so a genuine send→receive round-trip works under the default
`./arroyo test` slirp backend with **no external injector and no netdev change** (the
`scripts/net-inject.py` gateway UDP echo on port 9998 is the alternate medium under
`UNAOS_NET=socket`). Two witnesses fire knob-on:

- **M1, kernel-side** — `smolnet::witness_tick2()`, one-shot from `service_net()` on the BSP: opens a
  UDP socket in the persistent set, sends a DNS query, receives the reply, and emits
  `:: SOCK-2: smoltcp udp dns query 10.0.2.3:53 -> 64 bytes back — witness OK ::`.
- **M2, ring-3** — an inline position-independent fixture (`sock2-udp`, the u9x/u11x idiom, not an
  on-disk bin) makes all four syscalls from ring 3 and proves an end-to-end datagram round-trip,
  conveying a 5-bit witness (socket/bind/sendto/recvfrom/source-is-10.0.2.3:53). Its launcher prints
  `:: SOCK-2: ring-3 udp round-trip — … recvfrom returned a datagram FROM 10.0.2.3:53, socket teardown clean -> PASS ::`.

  The fixture runs on an AP while the BSP's hand-rolled `service_net()` also drains the NIC, so it
  **loops sendto+recvfrom** (up to 16 rounds): a reply stolen by the BSP poll (the SOCK-1
  frame-stealing residual, now cross-CPU) is simply retried, and each `recvfrom`'s own IF-masked
  bounded pump is where a fresh reply is captured.

Reproduce: `./arroyo test 60` — MISSION SUCCESS plus both lines above.

## Security surface

SOCK-2 is the first ring-3 network reach, so [`SECURITY.md`](../../../../../docs/SECURITY.md) gains
its networking row. In short: a socket is a capability (kind + rights) enforced at `handle_resolve`
exactly like a File; ring 3 gets **datagram** send/recv on a bound UDP socket and **nothing else** —
no raw-frame access (the `raw_rx`/`raw_tx` Device accessors are kernel-only), no interface
configuration, no promiscuous receive; the peer address is data the kernel copies through the
bound-checked window; `recvfrom` cannot block the IF-masked handler.

## SOCK-3 — TCP client sockets (ring 3 gets a byte stream)

SOCK-3 adds **TCP client sockets** on the same persistent stack, same `smolnet` feature (default-on), x86-only,
byte-identical knob-off / aarch64. A TCP socket rides the existing `STACK` singleton + `reg` registry —
a slot now carries a `SockKind` tag (`Udp` / `Tcp`) and, for TCP, its own static stream ring buffers
(`TCP_RX_DATA` / `TCP_TX_DATA`, 2 KiB each, BSS). The UDP and TCP sockets share one id space and one
generation counter, so a handle's value word maps to exactly one registry slot.

### Two REQUIRED folds from the SOCK-2 review (designed in from the start)

1. **A slot GENERATION fences the handle value word.** SOCK-2's value word was only `+1`-biased (safe
   only because sockets are non-transferable). SOCK-3 packs `(gen << 32) | (sid + 1)` (`sock_id_pack`)
   and validates the generation in `socket_id_of` → `smolnet::sock_valid` (present, owner-matched,
   generation-matched) — the exact U11x file-id discipline (`file_id_pack` / `file_desc_validate`).
   `SOCK_GEN[sid]` bumps on every free (`stack_close` / `free_row_sockets`), so a stale handle to a
   freed-and-reused registry slot resolves to `-EACCES`, never a rebind. A gen-0 socket packs to exactly
   `sid + 1`, so SOCK-2's UDP fixtures are unchanged. This closes the recycled-slot UAF **before** any
   arc makes a socket transferable.
2. **No TCP pump holds the global `STACK` lock across a full pump.** `tcp_pump_chunked` advances the
   interface in `TCP_CHUNK`-sized chunks and **drops the `STACK` guard between chunks**, so another CPU's
   socket syscall never spins on `STACK.lock()` for a whole ~400 k-iteration pump. Each chunk re-acquires
   the lock and re-validates the reg slot, so a concurrent teardown is observed as the connection
   vanishing (returned `Refused` / `Eof`), never a use-after-free.

### The blocking model (the SOCK-3 design crux, resolved)

The IF-masked syscall handler can never block, but a TCP handshake is multi-RTT. `sys_connect` is
**non-blocking with a ring-3 poll model**: the first call issues the SYN (from state Closed — idempotent,
a re-call while SYN-SENT just pumps) and pumps a bounded loop chasing ESTABLISHED. It returns `0`
(established), `-EINPROGRESS` (still handshaking — ring 3 re-invokes `connect`), or `-ECONNREFUSED` (the
peer reset). Slirp's RTT is microseconds, so in practice the handshake completes inside the first call;
the poll model is what keeps a slow/lossy peer from ever wedging the core. `sys_send` enqueues + a short
egress pump (`-EAGAIN` tx-full, `-ENOTCONN` not connected); `sys_sock_recv` drives a bounded poll pump
returning the bytes, `-EAGAIN` when none is available yet, or a clean `0` once the peer's FIN is delivered
and the rx ring is drained (end-of-stream). Nothing blocks.

### The syscalls

| # | Syscall | Gate | Returns |
| :--- | :--- | :--- | :--- |
| 23 | `sys_connect(handle, msg_ptr, msg_len)` | TCP socket + `CAP_WRITE` | `0` established / `-EINPROGRESS` / `-ECONNREFUSED` |
| 24 | `sys_send(handle, buf_ptr, buf_len)` | TCP socket + `CAP_WRITE` | bytes queued, or `-EAGAIN` / `-ENOTCONN` |
| 25 | `sys_sock_recv(handle, buf_ptr, buf_len)` | TCP socket + `CAP_READ` | bytes read, `0` at end-of-stream, or `-EAGAIN` |

`sys_socket(domain, type, proto)` selects the transport: `type = SOCK_STREAM(1)` → TCP,
`type = SOCK_DGRAM(2)` → UDP (unchanged). `connect`'s peer address is the same 8-byte header shape SENDTO
uses (`[ip[4]][port u16 LE][pad]`); send/recv carry **no** per-call address (a stream is connected). A UDP
handle routed to a stream syscall (or vice versa) is rejected on the `SockKind` tag as `-EACCES` **before**
smoltcp's typed accessor can panic. Next free syscall number: **49**.

> **SOCKNUM (WINX-1, 2026-07-29) — the family moved from 19–27 to 40–48.** The numbers quoted throughout
> this document are the CURRENT ones. The socket verbs originally opened at 19 and grew to 27, which
> silently violated the **cross-arch shared-number law**: a syscall number names the same verb on every
> arch (the ABI is Linux-style and shared; ring-3 code above the per-arch asm stubs is arch-neutral Rust
> that names verbs by number). aarch64 had already spent that range — 19 `MSEND`, 20 `MRECV`,
> 21 `THREAD_SPAWN`, 22 `THREAD_EXIT`, 23 `THREAD_JOIN`, 24 `FB_MAP`, 25 `FB_PRESENT`, 26 `FUTEX`,
> 27 `INPUT_POLL`. Nothing caught it because the two families never had to coexist: x86 compiled no
> window/thread verbs and aarch64 compiles no socket verbs. Bringing the WINDOW verbs up on x86 made the
> collision load-bearing (`SYS_INPUT_POLL` and `SYS_ACCEPT` would both have had to be 27 in one
> dispatch), so the x86-ONLY family — the arch alone in using these ids — moved to a free contiguous
> block at 40–48 with its relative order preserved. 19–27 now mean on x86 exactly what they mean on
> aarch64. Every caller was in-tree (the inline-asm fixtures), and the SOCK-2/3/4/6 + zeolite legs of
> the headless suite are the completeness proof.

### The round-trip witnesses

The witness medium stays slirp and is **hermetic under the default `./arroyo test` backend**: slirp
forwards a guest TCP connection to its built-in DNS resolver (`10.0.2.3:53`) out over TCP, so a
**DNS-over-TCP** query (RFC 7766: a 2-byte big-endian length prefix + the DNS message) is a genuine 3-way
handshake + stream round-trip — the TCP analogue of SOCK-2's UDP-DNS medium, with no external injector and
no netdev change. Two witnesses fire knob-on:

- **M1, kernel-side** — `smolnet::witness_tick3()` opens a TCP socket, poll-connects to `10.0.2.3:53`,
  sends the DNS-over-TCP query, receives the reply, and emits
  `:: SOCK-3: smoltcp tcp connect 10.0.2.3:53 established, 64 bytes back — witness OK ::`.
- **M2, ring-3** — an inline position-independent fixture (`sock3-tcp`) makes the three stream syscalls
  from ring 3 (poll-connect, send, poll-recv) and proves an end-to-end byte-stream round-trip, conveying a
  5-bit witness (socket/connect-established/send/recv-bytes/real-DNS-reply). Its launcher prints
  `:: SOCK-3: ring-3 tcp round-trip — socket/connect/send OK, recv returned a byte stream FROM 10.0.2.3:53, socket teardown clean -> PASS ::`.

Reproduce: `./arroyo test 90` — MISSION SUCCESS plus both lines. The `sock3-tcp` fixture
runs on an AP; a stolen segment is handled by the same non-blocking poll/retry discipline SOCK-2 uses.

## SOCK-4 — transferable sockets (a socket cap moves across processes)

SOCK-4 (scope B) makes a socket **capability movable to another process** — the socket analogue of the
U7x/U8x console-cap transfer. Same `smolnet` feature (default-on), x86-only, byte-identical knob-off / aarch64. It
adds **no new syscall** (next free number stays **49**): a socket rides the existing `SYS_XFER` (13) /
`SYS_RECV` (14) transfer machinery, which already special-cased `KIND_SOCKET` — this arc makes that path
actually *work* and proves it safe.

### The two changes that make a socket transferable

1. **`sys_socket` mints `CAP_GRANT`.** SOCK-2/3 minted `CAP_READ|CAP_WRITE` (no `CAP_GRANT`), so `SYS_XFER`
   (which requires `CAP_GRANT` on the source) could never move a socket. The mint now carries
   `CAP_READ|CAP_WRITE|CAP_GRANT`: a socket is the owning process's own resource, and `CAP_GRANT` cannot be
   self-added later (rights only attenuate), so transferability must be endowed at mint. Send still needs
   `CAP_WRITE`, recv `CAP_READ`, so a transfer/grant can still ATTENUATE to send-only / recv-only, and a
   transfer that drops `CAP_GRANT` (single-level) keeps the grantee from re-delegating.

2. **`sys_recv` MIGRATES socket ownership.** The persistent registry slot records an **owner row**, and
   `sock_valid` (the gen fence's owner check) requires `owner == caller_row`. A transferred socket handle
   carries the same gen-fenced value word, but its registry owner is still the GRANTOR's row — so without a
   migration it would fail `sock_valid` at the grantee (`-EACCES`). When `sys_recv` installs a received
   `KIND_SOCKET` cap it calls `smolnet::reassign_owner(sid, gen, from_row, new_row)` (`xfer_socket_migrate`):
   under the `STACK` lock, iff slot `sid` is still present at the SAME generation AND still owned by the
   transfer's SENDER (`from_row`, from the transfer record), its owner moves to the receiving
   row. Only the owner field changes — the smoltcp socket + its stream/packet buffers are untouched, so a
   bound port or an in-flight connection survives the hand-off (a MOVE, not a re-open).

### Single-owner, gen-fenced, safe by construction

A socket has exactly **one owner at any instant**. After the move the grantor's original handle is
owner-mismatched — a `sys_send`/`sys_recv` through it is `-EACCES` (the cross-row stale-handle rejection).
Teardown stays single-owner-correct: `free_row_sockets` frees a socket only for its current owner, so the
grantee's exit reclaims the socket and the grantor's exit reclaims nothing (its dangling handle is just
cleared). And the SOCK-3 **generation fence** does the rest: once the owner frees the slot, `SOCK_GEN[sid]`
bumps, so a stale cross-row handle carrying the old generation is `-EACCES` against BOTH the freed slot and
whatever socket first-fit-reuses it — **no rebind** (the U11x file-id discipline, socket edition). A stale
deposit (the socket freed+reused between `SYS_XFER` and `SYS_RECV`) fails `reassign_owner`'s gen check, so
the received handle is dead-on-arrival rather than stealing a different tenant's socket.

### The witnesses

Both fire knob-on, chained after the SOCK-3 verdict:

- **M1, kernel-side** — `sock4_kernel_check()` drives the real syscall bodies (`sys_xfer_from` /
  `sys_recv_for`) over two scratch rows: mint (owner A) → transfer → RECV migrates ownership to B → the moved
  cap **resolves for B** AND A's handle is `-EACCES` → then the **gen-rebind proof**: B frees the socket
  (gen bumps), a fresh socket first-fit-reuses the slot at the new generation, and B's old-generation handle
  stays `-EACCES`. Folded into the launcher verdict (`kernel=true`).
- **M2, ring-3** — a two-fixture demo (`sock4-grantor` / `sock4-grantee`, the U7x idiom, on dedicated APs).
  The grantor mints a UDP socket, proves cross-process attenuation (an over-rights `SYS_XFER` is `-EACCES`),
  and transfers it (dropping `CAP_GRANT`); the grantee `SYS_RECV`s it and completes a **datagram round-trip
  to slirp's resolver on the MOVED socket** (`bind`/`sendto`/`recvfrom` from `10.0.2.3:53`); then the
  grantor's post-transfer `SYS_SENDTO` through its migrated-away handle is `-EACCES`. A single-writer
  snapshot (the deposit is pending while the grantee's row is still untouched) + teardown-clear round it out:
  `:: SOCK-4: transferable sockets — grantee received + round-tripped the moved socket, grantor's migrated-away handle -EACCES, gen-rebind rejected, teardown clean -> PASS ::`.

Reproduce: `./arroyo test 90` — MISSION SUCCESS plus the SOCK-4 PASS line (and the SOCK-1/2/3
witnesses intact). The demo needs a NIC + three APs; it skips cleanly otherwise.

### Residuals (ledgered)

- **Move, not copy.** `SYS_XFER` is documented as depositing an attenuated *copy*; for a stateful,
  owner-scoped socket the transfer is a **move** (the grantor's handle dies once the grantee receives). This
  is the safe choice — co-ownership would break the single-owner teardown model.
- **First-recv-wins on a double transfer (review fix — the steal fence).** As landed, a double transfer was
  last-recv-wins: the grantor's residual handle still carries `CAP_GRANT` (rights are handle-local), so a
  SECOND `SYS_XFER` after the move could re-migrate the socket at the new recipient's RECV — a covert
  revocation yanking a live, in-use socket back out from under its owner (contradicting the "grantor's
  handle dies" model above). The review fix makes `reassign_owner` demand the transfer's **sender still own
  the socket at delivery**: the first RECV wins, every later deposit's handle arrives dead, and the current
  owner is undisturbed. Proven by the M1 kernel check's step 4b (deposit lands, migration refused, B still
  resolves).
- **Single-level.** A transferred socket cap drops `CAP_GRANT`, so the grantee cannot re-delegate it;
  cascading re-transfer + revoke of a socket is the revocation-tree machinery's concern, deferred.
- **UDP demo.** The two-fixture demo round-trips a UDP socket; a TCP socket transfers identically (same
  `KIND_SOCKET`, same value word, same migration), exercised by the kind-agnostic M1 kernel check.

## SOCK-5 — DHCP via smoltcp (the persistent stack leases its own address)

SOCK-5 (scope B) retires the persistent stack's **knob-on dependency on the hand-rolled DHCP lease**.
Before this arc, the persistent smoltcp interface was configured with a *static* address copied from
`e1000::hw_addr()` — an address the **hand-rolled** `crates/net` DHCP client had obtained. SOCK-5 gives
the smoltcp stack **its own** `dhcpv4::Socket`, so knob-on it acquires and applies its lease
autonomously. Same `smolnet` feature (default-on), x86-only, byte-identical knob-off / aarch64. **No new
syscall** (next free number stays **49**) and **no new ring-3 surface**: DHCP is a kernel-internal
interface-configuration function, not a capability ring 3 can invoke.

### The mechanism

The DHCP client rides the existing persistent `STACK`. The socket-set storage grows by one reserved
slot (`NSOCK + 1`) for a **kernel-internal** DHCP socket that is **never recorded in `reg`** — so it
never counts against ring-3 socket allocation (`stack_open*` still sees exactly `NSOCK` slots) and no
`free_row_sockets`/`stack_close` ever touches it. `SmolStack` gains a `dhcp: Option<SocketHandle>`.

`ensure_stack` builds the interface **with the static `hw_addr` lease + slirp gateway applied at
build** (via `apply_ipv4_config`, the one config surface) and adds the `dhcpv4::Socket` — so ANY
first-touch, including a lazy ring-3 `sys_socket` on a boot where no launcher pre-built the stack,
yields a configured, working interface with no pump under the lock. The acquisition itself,
`dhcp_acquire`, runs **only from `init()`** (the large-stack boot path) and is one-shot
(`DHCP_ATTEMPTED`): it pumps the interface until the DHCP socket emits `Event::Configured`
(`DISCOVER → OFFER → REQUEST → ACK`), **releasing the `STACK` lock every `TCP_CHUNK` iterations**
(the same SOCK-2-review lock-release discipline the TCP connect pump follows — the review fix: the
original landing held the lock, IF-masked, for the whole budget), then REPLACES the static config
with the leased `address` (CIDR) and `router` (default gateway). It is **iteration-bounded**
(`DHCP_PUMP`, clock-free like every other pump); on a silent server the build-time **static config
simply stands**, so the SOCK-1/2/3/4 witnesses keep a configured interface either way. Everything
stays **static / BSS** — the `dhcpv4::Socket` carries its own fixed internal storage, no heap.

### The witness

One kernel-side, one-shot witness fires knob-on (M1), latched so it prints exactly once at the first
stack build:

```
:: SOCK-5: smoltcp dhcpv4 lease 10.0.2.20/24 gw 10.0.2.2 — witness OK ::
```

The proof is end-to-end and self-checking: under slirp's DHCP server the leased address is
`10.0.2.20` — **not** the `10.0.2.15` static default the interface used to hard-code — and the SOCK-2
UDP-DNS, SOCK-3 TCP-DNS, and SOCK-4 transfer round-trips **all still pass on that DHCP-assigned
address**, confirming the whole socket family now runs on a lease smoltcp obtained itself. (A silent
server would instead print `… no offer — fell back to static … — witness INCOMPLETE ::`.)

Reproduce: `./arroyo test 90` — MISSION SUCCESS plus the SOCK-5 witness line and the
SOCK-1/2/3/4 lines intact.

### Residuals (ledgered)

- **One-shot acquisition, no lease renewal.** The boot acquisition configures the interface once;
  smoltcp's DHCP renew/rebind timers are not driven past that (the persistent stack keeps the lease for
  the session — adequate under slirp's effectively-infinite lease, and the fallback covers a silent
  server). A future arc can keep the DHCP socket pumping in `service_net` for renewal.
- **The hand-rolled DHCP still runs in the driver** (it is the live stack under `UNAOS_NOSMOLNET=1`)
  and, with smoltcp default-on, still leases the driver's own `hw_addr` — the two DHCP clients share the NIC's
  single MAC, so slirp hands them the same address. Fully retiring the hand-rolled `crates/net` DHCP is
  part of the eventual "retire the hand-rolled stack" arc, not this one.

## SOCK-6 — TCP server / listen sockets (ring 3 accepts inbound)

SOCK-6 (scope A) gives ring 3 the **server side** of TCP: a socket can be armed as a passive listener and
accept an inbound connection. Same `smolnet` feature (default-on), x86-only, byte-identical knob-off / aarch64. It
adds **two syscalls** over the persistent stack:

| # | Syscall | Gate | Returns |
| :--- | :--- | :--- | :--- |
| 26 | `sys_listen(handle, port)` | TCP socket + `CAP_WRITE` | `0`, or `-EINVAL` (port 0 / not TCP / already open) |
| 27 | `sys_accept(handle)` | listening socket + `CAP_READ` | a **fresh** socket handle for the connection, `-EAGAIN` (none yet), or `-EINVAL` (not listening) |

`sys_listen` arms the socket passively (`tcp::Socket::listen`), a configuring authority like `bind`/`connect`
(so it needs `CAP_WRITE`). `sys_accept` is NON-BLOCKING with the same ring-3 poll model as `sys_connect`:
it pumps a bounded loop chasing an inbound handshake; `-EAGAIN` means none arrived yet (ring 3 re-drives
accept). Accepting an inbound connection is a receiving authority, so it needs `CAP_READ`.

**smoltcp's listen→established model** is that the listening socket *becomes* the connection in place — it
does not spawn a child — so on accept the connection rides the listener socket's existing stream buffers.
`sys_accept` mints a **fresh `KIND_SOCKET` handle** aliasing the same gen-fenced socket-id (the SOCK-4
multi-handle-to-one-socket pattern), carrying `CAP_READ|CAP_WRITE|CAP_GRANT`, so the caller streams on it
(`send`/`sock_recv`) and can `SYS_XFER` it to a handler (inetd-style accept→hand-off). It is a
**single-accept-per-listen** model: to accept another connection, open + listen a fresh socket. Next free
syscall number: **28**.

### The witness (net-inject, `UNAOS_NET=socket`)

Slirp's NAT will not open a connection **into** the guest, so — unlike SOCK-1..5's guest-initiated,
hermetic-under-slirp round-trips — the server witness needs a peer that active-opens inward.
`scripts/net-inject.py sock6` (against `-netdev socket,listen=127.0.0.1:5555`, the `UNAOS_NET=socket`
builder mode SOCK-1 already wired — **no builder change**) crafts raw Ethernet frames to ARP-resolve the
guest, complete a 3-way handshake to the listener, send a probe, and verify the guest echoes it.

`smolnet::witness_tick6()` is a **stateful** BSP-main-loop witness (arm → accept → serve → done): it arms a
listener on port `8080`, polls accept across passes, and on a connection receives the peer's probe, echoes
it, and closes. Under the **default (hermetic) slirp** backend no peer ever connects, so it prints the
honest PENDING note once and keeps listening cheaply:

```
:: SOCK-6: smoltcp tcp listen :8080 armed — awaiting inbound connect (UNAOS_NET=socket injector) — witness PENDING ::
```

Under `UNAOS_NET=socket` with the injector it completes and emits:

```
:: SOCK-6: smoltcp tcp accept :8080 — received 11 bytes, echoed 11 back — witness OK ::
```

Reproduce (hermetic, PENDING): `./arroyo test 90` — MISSION SUCCESS plus the PENDING line
and the SOCK-1/2/3/5 lines intact. Reproduce (server round-trip, OK): launch the builder with
`UNAOS_NET=socket UNAOS_SERIAL_LOG=<log> UNAOS_TEST_SECS=60` and, concurrently,
`python3 scripts/net-inject.py 127.0.0.1:5555 sock6` — the injector prints `GUEST SOCK-6 SERVER OK` and the
serial log carries the `witness OK` line.

### Residuals (ledgered)

- **Single-accept-per-listen.** ✅ **RESOLVED by SOCK-7** (below) — the listener now survives accepts.
- **The witness is a stateful BSP-loop poll, not a ring-3 fixture.** The two syscalls are the delivered
  ring-3 surface (cap-gated, `SockKind`-tagged) and wrap the exact `stack_listen`/`stack_accept` seam the
  witness exercises over the wire; a dedicated ring-3 accept fixture is deferred.
- **`copy_from_user` for socket buffers** remains the deferred hardening all of SOCK-2..6 carry.

## SOCK-7 — persistent-listener acceptor pool (the listener survives accepts)

SOCK-6's listener was **consumed** by its accept: smoltcp has no listen→child model — a completed handshake
transitions the *listening* socket to ESTABLISHED **in place**, so the listener *became* the connection and a
second inbound connect to the same port was refused. SOCK-7 makes `sys_listen` arm a **persistent** listener:
each `sys_accept` hands the caller a fresh connection and the port **keeps listening**. Same `smolnet` feature
(default-on), x86-only, byte-identical knob-off / aarch64. **No new syscall** (the two SOCK-6 numbers
26/27 are unchanged; next free stays **28**) and **no new ring-3 surface** — the change is behind the
existing `sys_accept`.

### The mechanism (design shape (i): decouple stream buffers from the registry slot)

Two shapes were on the table: **(i)** decouple the per-slot stream buffers from the reg-slot index so an
accepted connection can be peeled to a fresh slot while the listener slot re-arms; **(ii)** a pre-armed pool
of N listeners on one port (accepted ones peel off, the pool re-arms). **Shape (i) was chosen** — it is the
minimal enabling primitive, needs **no extra BSS** (the existing `NSOCK` TCP buffer sets suffice), and both
shapes ultimately require the same decoupling (a pool socket that becomes ESTABLISHED must still hand its
buffers to a connection while a fresh listener takes over). Shape (ii)'s reserved-slot pool would only add
kernel-internal listener slots on top of this same primitive.

The buffers were previously **pinned to the reg-slot index** (`TCP_RX_DATA`/`TCP_TX_DATA[sid]`). SOCK-7 turns
them into a small **free-list**: `SmolStack` gains `tcp_buf_used: [bool; NSOCK]`, and a `Tcp` reg entry now
carries the buffer-set index its socket was built from (`SockKind::Tcp(buf)`). On accept, when the listener
socket reaches ESTABLISHED, `peel_and_rearm` (under a single `STACK`-lock hold, no pump):

1. **peels** the established smoltcp socket — which keeps the buffer set it already owns — into a **fresh reg
   slot** (the connection the caller gets a handle for); and
2. **re-arms** the original listener slot **in place** with a brand-new smoltcp socket on a **fresh buffer
   set**, listening again on the same port (recorded by `stack_listen` in `SmolStack.listen_port[sid]`).

The listener slot keeps its **socket-id and its generation** (no bump), so the caller's listener handle stays
valid across unbounded accepts; each accept returns a **fresh gen-fenced connection socket-id**. Bounded by
`NSOCK`: peeling needs a free reg slot **and** a free buffer set. The invariant *used buffer sets = live TCP
sockets ≤ used reg slots* means **whenever a reg slot is free, a buffer set is free too** — the peel never
starves. When **neither** is free (all `NSOCK` slots busy), the completed handshake stays buffered in the
listener socket and is peeled by a later accept once a slot frees (`Pending`/`-EAGAIN`) — never lost, and the
listener is never consumed. `ring 3`'s `sys_accept` now mints the `KIND_SOCKET` handle for the **peeled
connection socket-id** (not an alias of the listener), still deriving rights as the **intersection** of
`CAP_READ|CAP_WRITE|CAP_GRANT` with the listener handle's current rights (the SOCK-4 attenuation boundary is
unchanged).

### The witness (net-inject `sock7`, `UNAOS_NET=socket`)

`smolnet::witness_tick6()` is extended into a **two-accept** stateful BSP-loop machine on **one persistent
listener** (port `8080`): it accepts + echoes a **first** inbound connection (latching the SOCK-6 line — basic
accept still works, the regression), the listener **survives**, then accepts + echoes a **second** connection
to the same port (latching the SOCK-7 line — the whole point). `scripts/net-inject.py sock7` drives exactly
these two sequential connections (handshake → probe → echo-verify → close, twice). Under the **default
(hermetic) slirp** backend no peer connects, so both honest PENDING notes print once and it keeps listening
cheaply:

```
:: SOCK-6: smoltcp tcp listen :8080 armed — awaiting inbound connect (UNAOS_NET=socket injector) — witness PENDING ::
:: SOCK-7: persistent listener :8080 armed — awaiting a SECOND inbound connect (survives accept) — witness PENDING ::
```

Under `UNAOS_NET=socket` with the `sock7` injector it completes and emits **both** OK lines — the second is
the proof the listener was not consumed by the first accept:

```
:: SOCK-6: smoltcp tcp accept :8080 — received 11 bytes, echoed 11 back — witness OK ::
:: SOCK-7: smoltcp tcp accept :8080 #2 — received 12 bytes, echoed 12 back on a PERSISTENT listener (second inbound connection accepted after the first was consumed) — witness OK ::
```

Reproduce (hermetic, both PENDING): `./arroyo test 90` — MISSION SUCCESS plus the two PENDING
lines and the SOCK-1/2/3/5 lines intact. Reproduce (persistent round-trip, both OK): launch the builder with
`UNAOS_NET=socket UNAOS_TEST_SECS=85 ./arroyo test 90` and, concurrently,
`python3 scripts/net-inject.py 127.0.0.1:5555 sock7` — the injector prints `GUEST SOCK-6 ACCEPT OK` then
`GUEST SOCK-7 PERSISTENT LISTENER OK`, and the serial log carries both `witness OK` lines. (Under the socket
netdev there is no slirp DNS/DHCP responder, so the guest-initiated SOCK-2/3/5 DNS/DHCP witnesses read
INCOMPLETE on that medium — inherent to the injector backend, not a regression; they pass under slirp.)

### Residuals (ledgered)

- **The persistent listener holds one reg slot + one buffer set for its lifetime**, leaving `NSOCK − 1` for
  concurrent connections. Adequate for the witness (one connection at a time) and small servers; a larger
  listener/connection budget is a sizing change (grow `NSOCK` + the static buffer arrays), not a design one.
- **NSOCK back-pressure buffers, does not queue.** When all slots are busy a completed handshake waits in the
  listener socket until a slot frees (accept returns `-EAGAIN`); there is no separate accept backlog queue.
- **The witness is a stateful BSP-loop poll, not a ring-3 fixture** (as SOCK-6) — a dedicated ring-3
  persistent-accept fixture is deferred; `sys_accept` is the delivered, cap-gated ring-3 surface.
- **`copy_from_user` for socket buffers** remains the deferred hardening all of SOCK-2..7 carry.
- **Persistence is guaranteed across a *completed* accept, not against a malicious half-open** (review
  lens, at landing). A peer that sends SYN and withholds the final ACK parks the listener socket in
  `SynReceived`; when smoltcp exhausts its SYN-ACK retransmits the socket goes `Closed`, the next
  `stack_accept` reports `NotListening`, and ring 3 gets `-EINVAL` and must re-`listen`. Inherited
  smoltcp behavior (present in SOCK-6's single-accept model too), a liveness — not memory/capability —
  bound; a kernel-side auto-re-arm or SYN-flood mitigation is future work.

## SINKHOLE-1 (zeolite) — the DNS resolver / sinkhole, x86 QEMU proof

SINKHOLE-1 is the first slice of the UnaOS DNS sinkhole ("the Pi-hole concept, done the UnaOS way"): a
**ring-3 resolver** that binds UDP `:53`, answers BLOCKED names with `0.0.0.0` (the sinkhole), and
FORWARDS everything else to the upstream resolver. Same `smolnet` feature (default-on), x86-only, byte-identical
knob-off / aarch64. It adds **no new syscall** (next free stays **28**) and **no new ring-3 surface** — the
resolver is a ring-3 fixture (`zeolite-resolver`, the sock2 inline-blob idiom) composed entirely from the
already-landed capabilities: the SOCK-2 UDP syscalls (`sys_socket`/`bind`/`sendto`/`recvfrom`) for serve +
forward, and the **STOR-1 S7 dynamic-open path** (`sys_open` RO + `sys_read`) for the blocklist. That
composition is half the point: the resolver holds ONLY its two UDP sockets and a read-only blocklist
descriptor — capability-scoped by construction, with no web-stack bloat.

### The blocklist is a FILE (STOR feeds NET)

The blocklist lives on the FAT volume as `BLOCK.TXT` (planted by `make-fat-img.sh`). As of **ZEOLITE-2**
(below) it is **real hosts-file format** — the format actual sinkhole lists ship in (see the ZEOLITE-2
section); SINKHOLE-1 shipped a toy format (one UPPERCASE name per line). At start the resolver opens it via
the S7 dynamic-open path (`sys_open("BLOCK.TXT")` → not staged → `open_dynamic_ondisk` → a real on-disk
read) and `sys_read`s it into a ring-3 buffer — a genuine cross-subsystem composition witness (STOR feeds
NET). The S7 path needs `UNAOS_IRQSTORAGE=1` + a mounted FAT (`UNAOS_FATIMG=sf`); with **no** FAT the open
fails and the resolver falls back to a tiny **builtin list** (same entries, same format) and prints an
honest `builtin list (no FAT)` marker, so the pure-net legs still witness under the default (smolnet) alone.

### The witness-medium decision (resolved: split legs)

The two legs have conflicting media, and medium **(a)** — a guest-internal round-trip under hermetic slirp
via a second ring-3 client — was **probed and ruled out**: smoltcp on a single ethernet interface does
**not** deliver a datagram socket-to-socket to the guest's own address (there is no loopback medium; a probe
sending to `own-ip:5353` from a second socket got nothing back). So the proof is medium **(b), split legs**:

- **The FORWARD + composition leg proves hermetically** (under the default, and fully under
  `+IRQSTORAGE +FATIMG`). The resolver runs two self-tests through its **hardened parser**: it parses an
  inline query for `ADS.EXAMPLE`, matches the blocklist (BLOCKED), and **builds a well-formed `0.0.0.0`
  A-answer** (the sinkhole decision + response construction, proven without a peer); then it parses an
  inline query for `una.os`, confirms it is NOT blocked, and **forwards** it to the upstream resolver
  (`10.0.2.3:53`, slirp's built-in DNS), relaying the real answer. One line reports it:
  `:: zeolite: blocklist from BLOCK.TXT via S7 dynamic-open, blocked ADS.EXAMPLE -> 0.0.0.0 (answer built), forwarded una.os -> 10.0.2.3:53 real answer relayed — witness OK ::`
  (`builtin list (no FAT)` when no FAT; `INCOMPLETE` on the injector medium, which has no upstream resolver).

- **The SERVE (sinkhole-over-the-wire) leg needs the `UNAOS_NET=socket` injector.** Slirp's NAT will not
  inject an inbound query, so — like SOCK-6/7 — the resolver binds `:53` and, over a bounded window,
  `recvfrom`s an inbound query; a blocked name is sinkholed to `0.0.0.0` over the wire to the client.
  `scripts/net-inject.py dns` injects a DNS A-query for `ADS.EXAMPLE` and verifies the guest answers
  `0.0.0.0`. Under the default hermetic slirp no peer connects, so the guest prints the honest PENDING note
  and the injector run drives the OK:
  `:: zeolite: resolver bound :53 — awaiting an inbound query (UNAOS_NET=socket net-inject dns) — witness PENDING ::`
  → under the injector: `:: zeolite: served an inbound query on :53 — blocked name sinkholed to 0.0.0.0 over the wire — witness OK ::`.

Reproduce (hermetic, forward OK + serve PENDING): `./arroyo test 90`. Full composition
(blocklist from FAT): rebuild a **fresh** superfloppy first (`bash scripts/make-fat-img.sh sf`, or use
`./arroyo test-fat sf` which rebuilds it), then `UNAOS_IRQSTORAGE=1 UNAOS_FATIMG=sf ./arroyo test 200`
— `./arroyo test` reuses `builder/fat-sf.img` as-is, and a STALE one whose GROW.BIN was already grown
by a prior run trips U10/S4 with `grew_ok=false` (the stateful-fixture trap, not a regression).
Over-the-wire sinkhole: launch `UNAOS_NET=socket UNAOS_TEST_SECS=150 ./arroyo test 155` and concurrently
`python3 scripts/net-inject.py 127.0.0.1:5555 dns`.

### Hostile-payload parse hygiene

This is the first arc where our code parses a HOSTILE payload (a DNS name off the wire). The parser
(`zdns_parse_and_match`, ring 3) is **strictly bounded**: the packet length is validated before every field
access; the cursor is checked against the packet end before each length byte and label body; any label
whose length byte has EITHER high bit set is **REFUSED** — this rejects DNS compression pointers (`0b11…`)
AND the reserved `0b01`/`0b10` forms alike, so **no pointer is ever followed and no compression loop can
exist**; the assembled name is capped (≤ 250, well under its 256-byte buffer) and each label ≤ 63; and any
malformed packet is rejected as `malformed` without a crash (the fixture simply declines to sinkhole/forward
it). The name match is case-insensitive (both sides upper-cased). The DNS **response** builder writes a
fixed 16-byte answer (name-pointer `0xC00C`, TYPE A, CLASS IN, TTL 0, RDLENGTH 4, RDATA `0.0.0.0`) appended
to the copied question — no attacker-controlled length feeds any arithmetic.

### Residuals (ledgered)

- **Serve is proven guest-side; the end-to-end injector rendezvous is timing-sensitive.** The resolver
  fixture spawns late (after the storage chain) and serves for a bounded window; the injector must announce
  its own MAC (a gratuitous ARP — the guest ARPs for the client to address the reply) and retransmit a
  fixed-port query. The guest-side sinkhole is deterministic (`witness OK` observed over the wire); the
  committed hermetic gate carries the PENDING line, as SOCK-6/7 do.
- **Single in-flight forward, no cache, no query log, single question / A-record only, fail-`malformed`**
  (a packet the parser rejects is neither sinkholed nor forwarded — the conservative choice for a hostile
  payload). All per the first-slice scope. (Blocklist **ingest**, subdomain **matching**, and **metrics**
  were the next slice — see ZEOLITE-2 below; cache, query log, and multi-in-flight forwarding remain future
  work, along with the aarch64/GENET NIC and the kit.)
- **`copy_from_user`** for socket/name buffers remains the deferred hardening all of SOCK-2..7 carry.

## ZEOLITE-2 (zeolite) — real blocklist ingest, subdomain matching, metrics

ZEOLITE-2 is the honest second slice: it makes the resolver's list handling **real** without adding a
syscall, a ring-3 surface, or a kernel-net change (all still `#[cfg(all(feature = "smolnet", target_arch =
"x86_64"))]` — aarch64 and the knob-off path stay byte-identical). Three milestones, all inside the
`zeolite-resolver` blob + its launcher.

### M1 — hosts-format blocklist ingest

SINKHOLE-1's blocklist was a toy format (one bare `UPPERCASE` name per line, exact whole-name compare).
ZEOLITE-2 parses **real hosts-file format** — what Steven Black hosts, AdAway, and friends actually ship:
an IP redirect target (`0.0.0.0` / `127.0.0.1`) followed by whitespace and the domain, with `#`/`;`
comments (whole-line and trailing) and blank lines tolerated. Per line the `FILEBUF` walk in
`zdns_parse_and_match` now: skips leading whitespace; drops blank + `#`/`;` comment lines; takes the
**domain** as field-2 when a second whitespace-delimited field exists (the `0.0.0.0 domain` form) else
field-1 (bare `domain`, back-compat); compares the domain to the queried name case-insensitively. The
planted `BLOCK.TXT` and the builtin fallback both adopt the format:

```
# zeolite DNS sinkhole blocklist (hosts format)
0.0.0.0 ads.example
0.0.0.0 track.example   # inline comment tolerated

; semicolon comments and blank lines are skipped
127.0.0.1 telemetry.example
```

The parser stays **hostile-input-hardened**: a blocklist file is untrusted input exactly like a packet, so
every byte access is bounds-checked against `file_len` — a truncated IP field, a line with no domain, a
2048-byte line, an all-comment file, or embedded control bytes can never read past the buffer or crash; a
malformed line simply matches nothing and is skipped.

### M2 — label-boundary suffix (subdomain) matching

A blocked base domain now sinkholes its subdomains, the way a real sinkhole does: with `ads.example` on the
list, a query for `www.ads.example` is **BLOCKED**, but `notads.example` — which shares the *string* suffix
`ads.example` yet not on a **label boundary** — is **NOT**. The compare matches the blocklist domain against
the tail of the queried name and blocks iff `name == domain` OR `name` ends with `"." + domain` (the byte
before the tail must be `.`). Bounds are unchanged: the tail offset is `namelen − domainlen` with
`domainlen ≤ namelen` enforced first, so no read crosses the name buffer. Two inline self-tests witness both
directions (`WWW.ADS.EXAMPLE` → BLOCKED, `NOTADS.EXAMPLE` → not over-blocked).

### M3 — resolver metrics (the honest source for a stats view)

The resolver counts its own activity — **queries seen**, **blocked (sinkholed)**, **forwarded upstream** —
and reports it in a verdict line:

```
:: zeolite: metrics — 4 queries seen, 2 blocked (sinkholed), 1 forwarded upstream ::
```

This is the pitch's stats/admin-panel **honest data source** — a future quartzite/Surface widget reads
these numbers rather than fabricating them. It carries out with **no new syscall**: the three counts
saturate at 63 and pack into the spare high bits of the single witness/exit word (seen → bits `[10:16]`,
blocked → `[16:22]`, forwarded → `[22:28]`, all clear of the `bit0..9` decision flags); the launcher decodes
and prints them. Hermetically the tally is deterministic (the fixed self-tests give 4 / 2 / 1), so the
regression asserts it; over the wire the serve loop bumps seen/blocked per real query.

The leg-1 witness line now folds in the ingest + suffix proof:

```
:: zeolite: hosts-format blocklist from BLOCK.TXT via S7 dynamic-open, blocked ADS.EXAMPLE -> 0.0.0.0 (answer built), subdomain WWW.ADS.EXAMPLE sinkholed + NOTADS.EXAMPLE not over-blocked, forwarded una.os -> 10.0.2.3:53 real answer relayed — witness OK ::
```

Reproduce (hermetic, forward+suffix+metrics OK, serve PENDING): `./arroyo test 90`. Full
composition (hosts-format blocklist from FAT): `UNAOS_IRQSTORAGE=1 UNAOS_FATIMG=sf ./arroyo
test 200`. The over-the-wire serve leg is unchanged from SINKHOLE-1 (injector-driven; PENDING hermetically).

## SOCK-8 — x86 DNS client on the shared `net_dns` (`pool.ntp.org` for SNTP)

Until SOCK-8, smolnet had no way to turn a name into an address, so SNTP-X86 could only target the gateway.
SOCK-8 gives smolnet a **resolver** built on the same hostile-input-hardened wire logic the pi/genet
PI-NET-14 client uses — now **extracted to `crate::net_dns`**, exactly as SNTP-X86 extracted the SNTP parser
to `crate::net_sntp`. `net_dns` is arch-neutral (`#![no_std]`, no `alloc`, no I/O) and carries three pieces:
`build_query` (a minimal RD=1 A-record query, per-label ≤ 63 B **and** total encoded name ≤ 255 B — the one
guard tighter than PI-NET-14), `skip_name` (the compression-loop-immune name walk — a pointer is a two-byte
terminator that is **never dereferenced**, so a loop cannot form; reserved length bits rejected; label hop
cap), and `parse_a` (txid match, QR check, RCODE→`ServerErr`, bounds-checked answer walk capped at 64, first
A/IN/4-byte RDATA wins). genet migrates its `net14_*` onto this file in a later fold — the shared parser is
byte-for-byte the hardened original plus the stricter total-name cap, so the migration is a strict tighten.

### The mechanism

`smolnet::resolve(host)` sends one A-record query over the **persistent UDP stack** to `dns_server()` and
parses the reply with `net_dns::parse_a`. `dns_server()` prefers the **DHCP-provided DNS server** — captured
in `dhcp_acquire` from the lease's `dhcpv4::Config::dns_servers` (first entry) into the `CURRENT_DNS` atomic,
the smolnet analogue of the pi side's `NetConfig.dns` — and **falls back to the gateway** when the lease
carried no DNS option. The socket is kernel-owned (`usize::MAX`, never a ring-3 slot), bound to an ephemeral
port distinct from the SOCK-2 witness and the SNTP client, and always closed before return; a malformed /
no-answer / server-error reply returns `None` so the caller can fall back. The **SNTP client now prefers a
resolved `pool.ntp.org`** (`resolve(SNTP_POOL_HOST).unwrap_or_else(sntp_target)`), keeping the gateway
fallback and the honest witnesses.

### The witnesses

The one-shot boot witness resolves `pool.ntp.org` and prints, e.g.:

```
:: SMOLNET: [dns] pool.ntp.org -> 93.184.216.34 (via 10.0.2.3) ::
```

Under the hermetic `./arroyo test` slirp backend slirp's built-in forwarder (`10.0.2.3`) forwards DNS to the
host resolver, so a **live resolve may actually succeed** as a bonus witness; where the lease surfaces no DNS
server the resolver falls back to the gateway (which answers no DNS) and prints the honest
`:: SMOLNET: [dns] pool.ntp.org no answer (via <server>) ... ::` line — the mission stays green either way.
The deterministic `witness`-battery gate proves the parser with canned datagrams in **any** environment,
never depending on external reachability:

```
:: DNS-X86-GATE: x86 dns client battery PASS [w=0xf] (parse-A|reject-truncated|reject-loop|rcode) ::
```

`w` bits: `0x01` a well-formed A reply (with the universal compression pointer in the answer name, never
dereferenced) parses to the exact address; `0x02` a truncated answer RR is rejected as `Malformed`; `0x04` a
compression **loop** (a QNAME that is a self-pointer) is rejected **without hanging** — loop-immune by
construction; `0x08` a non-zero RCODE (NXDOMAIN=3) surfaces as `ServerErr(3)`. It runs from `main.rs`
alongside `sntp_x86_gate` (x86 + `witness` + `smolnet`). No new syscall.

## The road from here

| Arc | Content |
| :--- | :--- |
| SOCK-1 | smoltcp dep + `Device` adapter + `ping`/`arp`/`netinfo` + ICMP witness, knob-gated |
| SOCK-2 | the UDP socket syscall family (`sys_socket`/`bind`/`sendto`/`recvfrom`, #40–43 — see SOCKNUM; #19–22 as landed) + persistent `SocketSet`, ring 3 reaches the network |
| SOCK-3 | TCP client sockets (`sys_connect`/`sys_send`/`sys_sock_recv`, #44–46 — see SOCKNUM; #23–25 as landed) + gen-fenced socket handles + chunked-lock TCP pump, ring 3 gets a byte stream |
| SOCK-4 | transferable sockets — `sys_socket` mints `CAP_GRANT`, `sys_recv` migrates socket ownership, a socket cap moves cross-row (gen-fenced, single-owner); no new syscall |
| SOCK-5 | DHCP via smoltcp — the persistent stack leases its own address with `dhcpv4::Socket`; no new syscall, no ring-3 surface, static fallback |
| SOCK-6 | TCP server/listen sockets (`sys_listen`/`sys_accept`, #26–27) — ring 3 accepts inbound TCP; accept mints a fresh gen-fenced socket cap; net-inject `UNAOS_NET=socket` witness |
| SOCK-7 | persistent-listener acceptor pool — `sys_listen` arms a listener that survives accepts; each `sys_accept` peels a fresh gen-fenced connection + re-arms the listener in place (buffer free-list, shape (i)); no new syscall; net-inject `sock7` two-connection witness |
| SINKHOLE-1 (zeolite) | ring-3 DNS resolver/sinkhole — binds `:53`, blocks names from `BLOCK.TXT` (read via the S7 dynamic-open path) with `0.0.0.0`, forwards the rest to `10.0.2.3:53`; hardened hostile-payload parser; no new syscall; net-inject `dns` sinkhole witness |
| **ZEOLITE-2** (zeolite, this) | real hosts-file blocklist ingest (IP + domain, `#`/`;` comments, blank lines — hostile-input-hardened); label-boundary suffix matching (a blocked base domain sinkholes its subdomains); resolver metrics (queries seen / blocked / forwarded — the honest source for a stats view); no new syscall |
| **SNTP-X86** (CLOCK-1 x86 follow-up) | x86 smolnet SNTP client: one RFC 4330 client-mode request over the persistent UDP stack, reply parsed by the shared arch-neutral `crate::net_sntp` (ported from pi/genet PI-NET-16; genet migrates onto it later), then `crate::clock::set_anchor(unix, mono, Sntp{stratum})` — the x86 `time` verb gains a network writer. Targets a resolved `pool.ntp.org` when DNS is available (SOCK-8), else the live default gateway (DHCP-leased router / slirp 10.0.2.2). One-shot boot witness `:: SMOLNET: [sntp] <server> -> <ISO> (stratum N) ::` (honest `no reply` note under hermetic slirp — the gateway answers no NTP); the deterministic `witness`-battery gate `:: SNTP-X86-GATE: ... PASS [w=0x1f] ::` proves the parser + clock-anchor path with canned datagrams in any environment. No new syscall |
| **SOCK-8** (x86 DNS, this) | x86 smolnet DNS client on the shared arch-neutral `crate::net_dns` (PI-NET-14 wire logic extracted, `net_sntp` pattern; genet migrates onto it later; the one strictly-stricter addition = a ≤ 255-octet total-name cap in `build_query`). `smolnet::resolve(host)` queries the DHCP-provided DNS server (`CURRENT_DNS` from the lease's `dns_servers`, the smolnet analogue of `NetConfig.dns`) with a gateway fallback, over the persistent UDP stack; the SNTP client now prefers a resolved `pool.ntp.org`. One-shot boot witness `:: SMOLNET: [dns] <host> -> <ip> (via <server>) ::` (honest `no answer` under a non-forwarding gateway; a live slirp resolve via 10.0.2.3 is a bonus); deterministic `witness`-battery gate `:: DNS-X86-GATE: ... PASS [w=0xf] ::` (well-formed A / truncated / compression-loop / rcode). No new syscall |
| SOCK-9+ | aarch64 NIC bring-up (Pi GENET); genet migrates onto shared `net_dns`; the rest of the DNS appliance (cache, query log, stats view, kit); retire the hand-rolled shell surface + `crates/net` DHCP |
