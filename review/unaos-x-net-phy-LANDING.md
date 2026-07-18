# X-NET-PHY — landing report

**Branch:** `hw-rmbp` (base `main` @ `adb8de3`). **Arc:** promote the shared smoltcp `phy::Device`
adapter from `arch/aarch64/net_phy.rs` to an **arch-neutral** module and fold the x86 `smolnet.rs`
e1000e phy onto it — the cross-arch pass the NET-PHY landing proposed (review/unaos-net-phy-LANDING.md
§Flagged). A pure code move + generalization; **zero behavior change** on every arch/knob.

## Home chosen

`crates/kernel/src/net_phy.rs` — a **flat top-level file** at the crate root, next to `smolnet.rs`.

Not `crates/kernel/src/net/phy.rs`: the kernel depends on an **extern crate** named `net`
(`net = { path = "../net" }`, used as `net::ethernet` / `net::arp` in `smolnet.rs`). An internal
`crate::net` module would **shadow that extern crate** inside this crate and break those paths. A flat
`net_phy.rs` is the arch-neutral home that avoids the collision and matches `smolnet.rs`'s flat layout.

## What landed

| milestone | what |
|---|---|
| M1 | Moved the module `arch/aarch64/net_phy.rs` → `crate::net_phy` (`src/net_phy.rs`). Generalized the cfg to `any(net4, vnet, smolnet)` (each pulls `dep:smoltcp`) so the x86 default build compiles it too. Declared `pub mod net_phy;` in `lib.rs` (arch-neutral, no `target_arch` gate); removed the old `pub mod net_phy;` from `arch/aarch64/mod.rs`. Updated the two aarch64 use-sites (`rtl8168_tegra.rs`, `virtio_net.rs`) from `crate::arch::aarch64::net_phy::` → `crate::net_phy::`. Added the **RX-observer seam**: `SmoltcpPhy<N, O: RxObserver = ()>` — `observe` runs on every received frame before the tokens mint; `()` is a zero-cost no-op (aarch64 shape, unchanged). |
| M2 | Ported `smolnet.rs` onto `SmoltcpPhy`. Removed the duplicated `E1000Phy` struct + `PhyRxToken`/`PhyTxToken` + `Device`/`RxToken`/`TxToken` impls + local `FRAME_CAP`. Added `E1000Nic: RawNic` (over `e1000::raw_rx`/`raw_tx`/`hw_addr`) and `ArpSnoop: RxObserver` (holds `target` + `snoop`, `observe` = the retained `snoop_arp`). `type SmolPhy = SmoltcpPhy<E1000Nic, ArpSnoop>`; the two constructions became `SmolPhy::with_observer(ArpSnoop::new(target))`, and `dev.snoop` → `dev.obs.snoop`. |
| M3 | `arch_arm64.md`: rewrote §NET-PHY (arch-neutral home, the `RawNic`+`RxObserver` shape, why not a `net` module, the x86 gate) + updated the two pointer lines (`arch/aarch64/net_phy.rs` → `crate::net_phy`). This report. |

## The RX-observer seam (the one real design decision)

The pre-share x86 `E1000Phy` carried **instance state the aarch64 adapter lacks** — `target` + `snoop` —
and its `receive()` snooped every inbound ARP reply for the peer MAC that smoltcp hides (the `arp`/`ping`
shell path recovers it from the wire). Folding onto the shared adapter without regressing that meant
giving the adapter a per-frame hook. `SmoltcpPhy` is now generic over `O: RxObserver` (default `()`):

- **aarch64** (`net4`/`vnet`): `SmoltcpPhy::<Nic>::new()` ⇒ `O = ()`, `observe` is `#[inline(always)]`
  empty ⇒ compiles to the **exact** pre-share datapath. No aarch64 source outside the two `use` lines
  changed; witnesses byte-for-byte identical.
- **x86** (`smolnet`): `SmoltcpPhy::<E1000Nic, ArpSnoop>::with_observer(..)` ⇒ `observe` calls the
  retained `snoop_arp` at the same point `E1000Phy::receive` did. The persistent stack still builds it
  with `target = [0;4]` (inert snoop); only the blocking `pump` sets a real target and reads `obs.snoop`.

## Zero-behavior-change accounting

Pure code move + a zero-cost generalization. The adapter datapath (`receive` destructures disjoint
scratch; `TxToken::consume` copies then `N::transmit`s), `FRAME_CAP = 1536`, the smoltcp capability shape
(Ethernet, MTU 1500), the short-lock ring discipline, and every witness line are the exact code the three
seams carried. The only mechanical change to x86 semantics: the ARP snoop is now dispatched through
`RxObserver::observe` instead of an inline call — same call, same arguments, same position in `receive`.

## Files (lane-clean)

- **New:** `crates/kernel/src/net_phy.rs` (the arch-neutral shared adapter, +`RxObserver`).
- **Removed:** `crates/kernel/src/arch/aarch64/net_phy.rs`.
- `crates/kernel/src/lib.rs` — `#[cfg(any(net4,vnet,smolnet))] pub mod net_phy;`.
- `crates/kernel/src/arch/aarch64/mod.rs` — dropped the old `pub mod net_phy;` + refreshed the comment.
- `crates/kernel/src/arch/aarch64/{rtl8168_tegra,virtio_net}.rs` — `use` path updated (adapter use-site only).
- `crates/kernel/src/smolnet.rs` — adapter boilerplate replaced by `E1000Nic`/`ArpSnoop`/`SmolPhy` (no socket-layer / e1000 register logic touched).
- `docs/dev/OS/01_BOOT_HAL/arch_arm64.md` — §NET-PHY rewrite + two pointer lines.

No files outside the named lane were touched. No e1000 register logic, socket layers, sched, or usb.

## Gate results (verbatim)

| gate | result |
|---|---|
| `./arroyo check` (default, both arches) | `✅ x86_64 OK` · `✅ aarch64 OK` |
| `UNAOS_NET4=1 UNAOS_TEGRA=1 ./arroyo check` (both) | `✅ x86_64 OK` · `✅ aarch64 OK` |
| `UNAOS_VNET=1 ./arroyo check` (both) | `✅ x86_64 OK` · `✅ aarch64 OK` |
| `UNAOS_NET4=1 UNAOS_VNET=1 UNAOS_TEGRA=1 ./arroyo check` (both) | `✅ x86_64 OK` · `✅ aarch64 OK` |
| warnings attributed to `net_phy.rs`/`smolnet.rs` in ANY of the above | **none** (grep clean across the matrix) |
| `UNAOS_VNET=1 UNAOS_GICV3=1 ./arroyo test-arm 40` | `:: AARCH64 VNET: ping 10.0.2.2 RTT 5368 us (4/4 sent, 4/4 replies) => PASS ::` **and** `:: CAPSTONE COMPLETE — all 6 sync primitives verified in one boot ::` |
| knob-off `UNAOS_GICV3=1 ./arroyo test-arm 40` | `CAPSTONE COMPLETE` 6/6; **no VNET lines** (compiled out) |
| knob-off `./arroyo test-arm 22` (GICv2) | `xHCI: >>> MISSION SUCCESS (BOT + CSW). TARGET ACQUIRED. <<<` |
| `./arroyo kernel8-test` (35s window) | **0 FAIL** (Pi unaffected) |
| `./arroyo test 22` (x86 default) | `MISSION SUCCESS`; `SOCK-1` icmp 4/4 **OK**, `SOCK-2/3/5` **OK**, `SOCK-6/7` armed **PENDING** (hermetic slirp — unchanged meaning), ring-3 `SOCK-2/3/4` round-trips **PASS** |
| `UNAOS_CPU=qemu64 ./arroyo test 22` (x86) | `MISSION SUCCESS`; `SOCK-1..3/5` witnesses **OK** |

## Flagged

- The lib.rs/net_phy.rs doc comments point at `docs/dev/OS/08_NET/networking.md` as the smolnet "doc of
  record"; that file **does not exist in-tree** (a pre-existing dangling pointer, referenced also from
  `docs/dev/OS/06_NETWORK_STACK/network_stack.md`). Out of this arc's lane — not created/edited here. The
  live doc that actually describes the phy adapter is `arch_arm64.md` §NET-PHY, which this arc updated.
- Committed on `hw-rmbp` only; **not merged / not pushed** (integrator merges, Peter pushes).
