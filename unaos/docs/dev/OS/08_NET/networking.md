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

## No security surface yet

SOCK-1 adds **no syscalls** and no ring-3 network reach — the shell commands run in the kernel. The
socket syscall family is greenfield at syscall number 19 and lands with **SOCK-2**, which is when the
security ledger ([`SECURITY.md`](../../../../../docs/SECURITY.md)) gains a networking row.

## The road from here

| Arc | Content |
| :--- | :--- |
| **SOCK-1** (this) | smoltcp dep + `Device` adapter + `ping`/`arp`/`netinfo` + ICMP witness, knob-gated |
| SOCK-2 | the socket syscall family (UDP first) over smoltcp's socket set — ring 3 reaches the network |
| SOCK-3+ | TCP sockets; DHCP via smoltcp; aarch64 NIC bring-up; retire the hand-rolled shell surface |
