# Networking — the smoltcp adoption (SOCK-1)

Direction: [`ROADMAP.md` §1b](../../../../../docs/ROADMAP.md). This document describes the
networking stack as of **SOCK-1** (round 9, 2026-07-12): the first arc of the migration from the
hand-rolled `crates/net` protocol line onto the mature [smoltcp](https://github.com/smoltcp-rs/smoltcp)
crate (0.13.1, 0BSD, `#![no_std]`, heap-optional).

## Today's two-stack reality

SOCK-1 does **not** remove anything. It adds a second, feature-gated path and leaves the existing
stack byte-identical when the knob is off:

| | Knob **off** (default) | Knob **on** (`UNAOS_SMOLNET=1`) |
| :--- | :--- | :--- |
| `ping` / `arp` / `netinfo` | hand-rolled `net::` engines (`drivers/e1000.rs`) | **smoltcp** ICMP socket + interface (`smolnet.rs`) |
| `connect` / `fetch` / `udpsend` | hand-rolled `net::tcp` / `net::udp` | hand-rolled (unchanged — SOCK-2/3 own the socket rewrite) |
| DHCP, TCP echo listener, boot self-test | hand-rolled | hand-rolled (still runs) |
| aarch64 | no wired NIC — `e1000`/`net` compile but are inert | **smoltcp is never compiled** (x86-only dep) |

The hand-rolled `crates/net` crate (ARP / ICMP / UDP / DHCP + the Go-Back-N TCP engine) stays in
tree as reference and remains the live stack knob-off. It is **not touched** by this arc.

## The knob

`UNAOS_SMOLNET=1` selects the kernel crate's `smolnet` cargo feature. That feature activates an
**x86-only optional dependency** on `smoltcp` (declared in `crates/kernel/Cargo.toml` under
`[target.'cfg(target_arch = "x86_64")'.dependencies]`), so:

- **Knob off** → the feature is inactive, smoltcp is never pulled, and the binary is byte-identical
  to the pre-SOCK-1 base (both arches).
- **Knob on, x86** → smoltcp compiles; `smolnet.rs` and the shell/witness call sites compile in.
- **Knob on, aarch64** → the `smolnet = ["dep:smoltcp"]` feature resolves to a no-op (the optional
  dep does not exist for that target), so aarch64 never compiles smoltcp. The `smolnet` module and
  every call site are additionally `#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]`, so
  aarch64 is byte-identical knob-on too.

The knob is plumbed in **two** places that must stay in sync (both rebuild the x86 kernel):
`unaos/arroyo` (the feature map) and `unaos/builder/src/main.rs` (the builder rebuilds the kernel
before launching QEMU, so it maps `UNAOS_SMOLNET` → `smolnet` independently).

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

Reproduce: `UNAOS_SMOLNET=1 ./arroyo test 60` (MISSION SUCCESS + the line above).

## SOCK-2 — the UDP socket syscall family (ring 3 reaches the network)

SOCK-2 adds the first four **socket syscalls** and the persistent smoltcp state they need. It is the
same knob (`UNAOS_SMOLNET`), x86-only, byte-identical knob-off / aarch64.

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

Reproduce: `UNAOS_SMOLNET=1 ./arroyo test 60` — MISSION SUCCESS plus both lines above.

## Security surface

SOCK-2 is the first ring-3 network reach, so [`SECURITY.md`](../../../../../docs/SECURITY.md) gains
its networking row. In short: a socket is a capability (kind + rights) enforced at `handle_resolve`
exactly like a File; ring 3 gets **datagram** send/recv on a bound UDP socket and **nothing else** —
no raw-frame access (the `raw_rx`/`raw_tx` Device accessors are kernel-only), no interface
configuration, no promiscuous receive; the peer address is data the kernel copies through the
bound-checked window; `recvfrom` cannot block the IF-masked handler.

## SOCK-3 — TCP client sockets (ring 3 gets a byte stream)

SOCK-3 adds **TCP client sockets** on the same persistent stack, same knob (`UNAOS_SMOLNET`), x86-only,
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
smoltcp's typed accessor can panic. Next free syscall number: **26**.

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

Reproduce: `UNAOS_SMOLNET=1 ./arroyo test 90` — MISSION SUCCESS plus both lines. The `sock3-tcp` fixture
runs on an AP; a stolen segment is handled by the same non-blocking poll/retry discipline SOCK-2 uses.

## SOCK-4 — transferable sockets (a socket cap moves across processes)

SOCK-4 (scope B) makes a socket **capability movable to another process** — the socket analogue of the
U7x/U8x console-cap transfer. Same knob (`UNAOS_SMOLNET`), x86-only, byte-identical knob-off / aarch64. It
adds **no new syscall** (next free number stays **26**): a socket rides the existing `SYS_XFER` (13) /
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

Reproduce: `UNAOS_SMOLNET=1 ./arroyo test 90` — MISSION SUCCESS plus the SOCK-4 PASS line (and the SOCK-1/2/3
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
autonomously. Same knob (`UNAOS_SMOLNET`), x86-only, byte-identical knob-off / aarch64. **No new
syscall** (next free number stays **26**) and **no new ring-3 surface**: DHCP is a kernel-internal
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

Reproduce: `UNAOS_SMOLNET=1 ./arroyo test 90` — MISSION SUCCESS plus the SOCK-5 witness line and the
SOCK-1/2/3/4 lines intact.

### Residuals (ledgered)

- **One-shot acquisition, no lease renewal.** The boot acquisition configures the interface once;
  smoltcp's DHCP renew/rebind timers are not driven past that (the persistent stack keeps the lease for
  the session — adequate under slirp's effectively-infinite lease, and the fallback covers a silent
  server). A future arc can keep the DHCP socket pumping in `service_net` for renewal.
- **The hand-rolled DHCP still runs in the driver knob-off** (it is the live stack when `UNAOS_SMOLNET`
  is off) and, knob-on, still leases the driver's own `hw_addr` — the two DHCP clients share the NIC's
  single MAC, so slirp hands them the same address. Fully retiring the hand-rolled `crates/net` DHCP is
  part of the eventual "retire the hand-rolled stack" arc, not this one.

## The road from here

| Arc | Content |
| :--- | :--- |
| SOCK-1 | smoltcp dep + `Device` adapter + `ping`/`arp`/`netinfo` + ICMP witness, knob-gated |
| SOCK-2 | the UDP socket syscall family (`sys_socket`/`bind`/`sendto`/`recvfrom`, #19–22) + persistent `SocketSet`, ring 3 reaches the network |
| SOCK-3 | TCP client sockets (`sys_connect`/`sys_send`/`sys_sock_recv`, #23–25) + gen-fenced socket handles + chunked-lock TCP pump, ring 3 gets a byte stream |
| SOCK-4 | transferable sockets — `sys_socket` mints `CAP_GRANT`, `sys_recv` migrates socket ownership, a socket cap moves cross-row (gen-fenced, single-owner); no new syscall |
| **SOCK-5** (this) | DHCP via smoltcp — the persistent stack leases its own address with `dhcpv4::Socket`; no new syscall, no ring-3 surface, static fallback |
| SOCK-6+ | TCP server/listen sockets; aarch64 NIC bring-up; retire the hand-rolled shell surface + `crates/net` DHCP |
