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

## The road from here

| Arc | Content |
| :--- | :--- |
| SOCK-1 | smoltcp dep + `Device` adapter + `ping`/`arp`/`netinfo` + ICMP witness, knob-gated |
| **SOCK-2** (this) | the UDP socket syscall family (`sys_socket`/`bind`/`sendto`/`recvfrom`, #19–22) + persistent `SocketSet`, ring 3 reaches the network |
| SOCK-3+ | TCP sockets; DHCP via smoltcp; aarch64 NIC bring-up; retire the hand-rolled shell surface |
