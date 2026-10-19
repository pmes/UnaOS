# BP-net — links, the stacks, names and time, Wi-Fi, ring-3 net (read at 1719fe54)

## 1. What it is
- Links: e1000e (x86, QEMU), USB Ethernet on xHCI (`unaos/crates/kernel/src/drivers/xhci/usbnet.rs`, CDC-ECM front-end; framing in `usbnet_core`), Pi GENET (`unaos/crates/kernel/src/arch/aarch64/genet.rs`), Orin RTL8168 (`unaos/crates/kernel/src/arch/aarch64/rtl8168_tegra.rs`, `net4`), virtio-net on QEMU virt (`vnet`).
- Stack: smoltcp is the x86 default (`unaos/crates/kernel/src/smolnet.rs:16-23`); aarch64 pulls it only under `net4`/`vnet` (`unaos/crates/kernel/Cargo.toml:1042-1052` `unaos/crates/kernel/Cargo.toml:1369-1382` `unaos/crates/kernel/Cargo.toml:1656`). One shared `phy::Device` adapter, `unaos/crates/kernel/src/net_phy.rs`.
- The hand-rolled stack (`unaos/crates/net`) stays compiled and live: driver DHCP, `nc`, `curl`, the TCP echo listener; `UNAOS_NOSMOLNET=1` makes it the whole path (`docs/dev/OS/06_NETWORK_STACK/network_stack.md:3-16`).
- Wi-Fi: the BCM4331 ladder (`wifi/**`, `unaos/crates/kernel/src/drivers/bcma.rs`, `wifi_core`) with firmware staged from the root volume; receive rungs (WIFI9) and the first transmit (WIFI10) run only under `tests wifi`.
- Ring 3 reaches the network through sockets (`SYS_SOCKET`..`SYS_ACCEPT`, 40-48) and `SYS_RESOLVE` (57) (ring3.md).

## 2. Invariants
| id | statement | anchor | held by |
|---|---|---|---|
| BP-net-1 | One `smoltcp::phy::Device` adapter for every NIC seam, over a three-call `RawNic` trait; drivers do not carry their own copy. | `unaos/crates/kernel/src/net_phy.rs:4-20` | compile |
| BP-net-2 | smoltcp stays the kernel's x86 default until Aether's network layer has run on metal against it; the hand-rolled replacement (NETCORE) is cut only after (R111). DEPS row owed. | R111; `unaos/crates/kernel/src/smolnet.rs:16-23` | `gates` row `deps` |
| BP-net-3 | aarch64 images without `net4`/`vnet` never pull smoltcp; the knob-off image is byte-identical. | `unaos/crates/kernel/Cargo.toml:1042-1046` `unaos/crates/kernel/Cargo.toml:1380-1382` | `./arroyo knoboff net4` (exit 0 = identical) |
| BP-net-4 | No network service starts before the Desktop phase's services gate (net, usbnet, bt, wifi). | `unaos/crates/kernel/src/boot.rs:98-126` (BP-boot-3) | `[boot] services up held_starters=` |
| BP-net-5 | USB Ethernet is ONE driver on the shared xHCI path serving every board; the controller owns the endpoints, the module owns identity, bring-up and the two frame rings. | `unaos/crates/kernel/src/drivers/xhci/usbnet.rs:16-30` | `[usbnet]` census lines |
| BP-net-6 | Ring 3 gets one name resolver per arch (x86 `smolnet::resolve`, aarch64 `net_phy::net6::lookup`) behind one syscall body. | `unaos/crates/kernel/src/netring3.rs:1-10` | `tests net` |
| BP-net-7 | Nothing transmits RF at boot: WIFI10's probe request fires once per boot from `tests wifi`, after WIFI9 tuned channel 11 and read a plausible MAC (R80, R110). | `unaos/crates/kernel/src/wifi/bringup.rs:2606-2625`; `unaos/scripts/image-line.env:33-34` | `:: wifi10: …` line; `already ran this boot` guard |
| BP-net-8 | Wi-Fi bring-up is a ladder, one change per rung; arc 1 is config-space reads only; register writes begin at arc 2 and are reversible; the microcode upload sits with the reset prologue. | `unaos/crates/kernel/src/wifi/mod.rs:1-30` | the `:: wifi<n>:` end lines (`wrote-*=` census) |
| BP-net-9 | The vendor firmware set is staged on the card's UnaFS `/FIRMWARE/` from a bench path (media knob), never in the tree; digests pinned in `unaos/firmware/b43.pins`. | `unaos/scripts/image-line.env:69`; BP-shape-11 | `gates` row `image-line` (media row shape) |
| BP-net-10 | The entropy source (RDRAND/RNDR or a declared timer-jitter fallback) is said once per boot; a DRBG sits on top for ring 3. | `unaos/crates/kernel/src/lib.rs:112`; `unaos/crates/kernel/src/netring3.rs:13-20` | `[rand] source=` line |
| BP-net-11 | Every outbound connect/send the stack counts names its starter: the sending task's open `net_tick::scope` (DHCP, the SOCK ladder, SNTP; a routed shell verb as `fetch`/`shell`; ring 3's resolve as `ring3`), else the site's own name (a ring-3-owned socket is `ring3`), else `nobody`, which stays 0 after login with no user verb (BP-bar-1). x86 counts at smolnet's `stack_connect`/`stack_sendto_inner`/`stack_send`; aarch64 NET6 at the SNTP/DNS builders. | `unaos/crates/kernel/src/net_tick.rs:111-123` `unaos/crates/kernel/src/net_tick.rs:282` | `tests idlenet` (`:: IDLENET: window_s=60 connects= unattributed=0 sends= -> PASS ::`) |

## 3. Seams
- Net may call: the NIC drivers through `RawNic`, `usbnet_core`, `wifi_core`, `http_core`/`tls_core` (ring-3 side), `rand`.
- May call net: shell verbs (`ping`, `ifconfig`, `nc`, `curl`), `netring3` (ring-3 sockets/resolve), SNTP (`unaos/crates/kernel/src/net_sntp.rs`; aarch64 client `unaos/crates/kernel/src/net_sntp_client.rs` under `sntp6`+`net6`, `unaos/crates/kernel/src/lib.rs:49`), the status item.
- NIC drivers sit under the services gate (BP-net-4) and the xHCI placement rule (cores.md BP-cores-1).

## 4. Wire
`[net] …` · `[usbnet] …` · `[idlenet] boot|window by=` · `:: IDLENET: …` · `:: NICABSENT: …` · `[rand] source=` · `:: wifi2: upload verify words=…` (ST1, f13) · `:: wifi9: end … shm-ctl-restored=` · `:: wifi10: … tx= heard= ::` · `:: NET6: fixture: n/n` (aarch64).

## 5. Ruled, unbuilt / open
- NETCORE (the hand-rolled stack as the default): gated on R111's condition.
- Wi-Fi association, WPA, a usable link: beyond the WIFI10 rung (`unaos/crates/kernel/src/wifi/mod.rs` ladder). WIFI11's TX power control folded (`8997dbde`); flight result `unverified` here.
- The first DHCP lease on the rMBP is recorded in commit `d08a5d06`'s message (flight 35); no `ST<n>` row was read for it.
- Bluetooth beyond HID (`drivers/ehci/bt*.rs`): not read for this sheet.

## 6. Read further
- `unaos/docs/dev/OS/08_NET/networking.md` (the live stack's doc of record).
- `docs/dev/OS/06_NETWORK_STACK/bcm4331.md`, `wifi_bcma.md`, `pi_genet.md`, `network_stack.md` (hand-rolled crate).
- `docs/dev/STATUS.tsv` ST1, ST2 (the Wi-Fi upload status rows).
