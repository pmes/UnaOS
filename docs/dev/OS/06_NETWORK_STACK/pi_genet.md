# Pi 4 Ethernet — BCM2711 GENET v5 driver

The Raspberry Pi 4's on-board Gigabit Ethernet is a Broadcom **GENET v5** unimac
controller (`ethernet@7d580000`, `brcm,bcm2711-genet-v5`) driving an external
**BCM54213PE** RGMII PHY. The driver is `arch/aarch64/genet.rs`, armed by the
`genet` cargo feature (`UNAOS_GENET=1`, default OFF). It DTB-resolves the block,
brings up the MAC/PHY/DMA rings per the Linux `bcmgenet` v5 programming model,
and binds a persistent [smoltcp](../../../../unaos/docs/dev/OS/08_NET/networking.md)
interface on top.

This document records the **DMA-coherency and RX/TX datapath findings** proven on
Pi silicon during the R23s1f campaign (the first DHCP lease and first TCP service
on the Pi). The bring-up foundation — DTB resolve, the register/datapath model,
autoneg-honest link (PI-GENET-2), and the ring-wrap fix (PI-GENET-3) — is in
[`arch_arm64.md` §PI-GENET](../01_BOOT_HAL/arch_arm64.md). This doc picks up from
the first frames on the wire.

---

## 1. The load-bearing design fact — GENET buffers are cached, the DMA does not snoop

**The BCM2711 GENET DMA engine does not snoop the A72 data caches.** The RX and TX
ring buffers are allocated from Write-Back **cacheable** DRAM, so every hand-off
between the CPU and the controller must be bracketed by explicit cache
maintenance, or one side reads stale memory. This is the same non-coherent-DMA
class as the [xHCI driver](../07_USB_STORAGE/usb_xhci.md#dma-coherency-xhci-coherence)
on this SoC, and it manifests as a **coherency pair** — one op on each direction
of the ring:

| Direction | Op | When | Arc |
| --- | --- | --- | --- |
| **RX** | `dc ivac` (invalidate) | per buffer line, *before* the CPU reads the length + copies the frame | GENET-5 (`0370ebc1`) |
| **TX** | `dc cvac` (clean/write-back) | over the frame buffer, *before* publishing the descriptor | GENET-7 (`acf11670`) |

Both are applied over the identity-mapped buffer span and are no-ops on a coherent
host. The maintenance lives in `rx_frame_raw` (invalidate before length read + copy)
and `raw_tx` (clean before descriptor publish).

- **RX without the invalidate (GENET-5):** the CPU read stale pre-refill cache lines
  — real lengths off the descriptor but zero/garbage payload, so the ethertype was
  unparseable and **0 frames classified out of 151 popped** (boot-P10). The RBUF
  status-block strip was checked first and exonerated; cache staleness was the
  actual defect. Vindicated at boot-P11/P13: the same pops that read garbage now
  decode real router ARP and sane ethertypes.
- **TX without the clean (GENET-7):** the TDMA producer/consumer indices drained
  cleanly (`prod==cons`, no ring stall) yet **the frames never reached the bridge**
  — the switch's address cache never learned the Pi's MAC — because the MAC
  DMA-read stale DRAM and egressed garbage or nothing. Condemned on the bench by
  elimination (the same share dongle leased the Orin and a second Mac through the
  same wire). GENET-7 is the direct TX mirror of GENET-5 and was **the fix** that
  produced the first lease.

> The RX-clean / TX-stale split is why the Pi's no-lease symptom was NOT the Orin's:
> on the Pi, inbound frames popped and drained (RX ring alive); the gap was TX
> egress. On the Orin the frames never popped. Two different diseases in the same
> non-coherent-DMA family.

---

## 2. The rx[0] first-slot quirk — the descriptor length is authoritative

On **every boot**, the *first* popped RX frame reads an all-zero in-buffer 64-byte
status block even after `dc ivac`, while its descriptor `length_status` word carries
a valid length and the payload at `RX_STATUS_PAD` is the real frame (metal:
`rx[0] dsc=0x01987f80 len=342 et=0x0800`, the live DHCP OFFER). From `rx[1]` onward
`sb_pre == sb_post == dsc` — the status block *is* populated for later slots.

The RDMA completion writes back the descriptor `length_status` and advances the
producer index, but the 64-byte status-block write **lags or is skipped on the first
ring pass**. The design ruling (GENET-8, `565bf8c7`):

- **The descriptor `length_status` is the authoritative length source.** The in-buffer
  status block is kept only as the `[genet5]` coherency witness, never as a length.
- The GENET-6 bounded re-poll (`31f0d269`) — "status block reads 0 ⇒ DMA not yet
  visible, re-read to rescue" — was **REFUTED on metal**: 10k spins never flipped the
  block, the frame was already complete, and the spin burned the DHCP window while
  frames queued. The re-poll was removed.
- This also refutes the early-index hypothesis: a slot popped before completion would
  show `dsc_ls == 0` (init clears it), but `dsc_ls` is a valid length ⇒ the slot WAS
  completed; only the status block lagged.

---

## 3. PHY LED selectors — the LEDs are on the PHY, not the MAC

The RJ45 link/activity LEDs on the Pi 4 are driven by the external **BCM54213PE**
PHY (MDIO addr 1, `PHYID1 = 0x600d`), **not** the GENET MAC. The stuck-solid-amber
LED observed through the whole campaign was therefore *not* a MAC datapath symptom —
the `EXT_RGMII` `OOB_DISABLE` / forced `RGMII_LINK` bits were a red herring.

Left at power-on defaults, a PHY LED selector maps an LED to a solid
link/full-duplex source, so it stays lit the entire time a gigabit link is up. The
fix (GENET-8, `565bf8c7`) adds `mdio_write` plus a **BCM54xx shadow-register writer**
and programs the LED-selector shadow registers (`0x0d` / `0x0e`) to standard
link + activity sources (no tied-on selector), following `bcm-phy-lib` /
`brcmphy.h`. This is PHY-side / MDIO-only — no MAC datapath change, no protection
weakened. Metal-confirmed OUT at boot-P18 power-on.

---

## 4. Net service architecture

Once the interface is DHCP-configured, two long-lived kernel tasks keep the stack
alive and serving. Both live in `genet.rs` and register via
`crate::arch::sched::spawn` from within the driver (no shared kernel-core file is
touched).

### `net9` — the persistent poll task (PI-NET-9, `15c6626e`)

`bind_smoltcp`'s original DHCP+ping window was bounded: it returned, and the
interface stopped being polled, so the gateway's later ARP who-has / ICMP echo
requests (arriving seconds after the lease) hit a dead stack. PI-NET-9 gives the
DHCP-configured smoltcp `Interface` + `Device` a home beyond that window (a static
`NetService` behind a `spin::Mutex`, single-core-owned like `GENET_DEVICE`) and a
forever kernel task (`net9`) that polls it every **~4 ms** (1 per-core tick).
smoltcp's `iface.poll` answers ARP and ICMP echo by itself — no protocol code. Reply
counts come from a TX-seam classifier in `raw_tx` (ARP opcode 2 / ICMP type 0), so
the `[net9] answered arp=N icmp=N` witness is an emission proof, not a poll-loop
guess (rate-limited, change-only — default-quiet).

### `net10` — the first TCP service (PI-NET-10, `4055900c`)

PI-NET-10 grows the persistent `SocketSet` to hold one passive smoltcp TCP socket
(static leaked 2048/4096 ring buffers) listening on **port 80**. Each `net9` poll
takes one bounded HTTP service step (`NetService::http_step`): re-arm `listen(:80)`
from any non-open state (so the service survives repeated requests and rude clients —
smoltcp needs an explicit re-listen after RST / half-open close), drain the request
(path ignored), then on a writable TX half emit a small static `HTTP/1.0 200` page
(OS name, hw-pi4 tip SHA via `option_env!(UNAOS_GIT_SHA)` — see BUILD-SHA-1,
`2b330f57` — else the branch label, uptime, `[net9]` reply counters, served count,
configured IPv4) and close.

> **Scheduler placement note (PI-SCHED-1, `4490e7bf`).** The `net9` task's ~4 ms poll
> was observed parking on the *render* core, and all GUI (`vug`) load landing on the
> orphan-reaper core. PI-SCHED-1 adds a Pi-gated per-spawn placement witness plus a
> `core_load_report()` so every core placement is auditable on the target. This is a
> **probe only** — log-only, it cannot alter scheduling — but it flags that net-poll
> co-tenancy with the render core is a placement item worth an eventual policy arc.

### `net11` / `net12` — mDNS responder + the TCB pool (PI-NET-11 `142fe2ee`, PI-NET-12 `b26d2dbe`)

PI-NET-11 adds a UDP socket bound to **5353** that answers `unaos.local` A queries
(`mdns_step`), joining `224.0.0.251`. PI-NET-12 grows the single :80 listener into a
**pool of 4** independent TCBs (the accept backlog) plus a **3 s idle-reaper**: a
listener that leaves LISTEN but never completes a request is force-aborted (RST) and
re-armed, fixing an accept wedge where a silent peer pinned a TCB forever. A `[net12]`
census witness reports `(listen, active)` and flags `SATURATED` (listen==0) / reap edges.

### QEMU regression gate (PI-NET-13, `nettest`)

The GENET datapath **no-ops under QEMU raspi4b** (no GENET modelled — `genet_bringup`
returns at the DTB skip), so every TCP/HTTP/mDNS behavior above was previously
**metal-only**. PI-NET-13 makes them QEMU-testable **without hardware**:

* **`NetService<D: Device>` is now generic** over the smoltcp `Device` (default type
  parameter `SmoltcpPhy<GenetNic>`, so the metal path and the `NET_SERVICE` static are
  byte-identical). The pool/reaper/http/mdns methods never touched the device — only
  `iface.poll` does — so the **same** `http_step` / `mdns_step` / `http_census` /
  `render_http_response` service code runs over any seam. The socket-pool construction
  is factored into `build_net_sockets`, shared by `arm_net_service` and the gate.
* **A loopback seam + scripted peer** (`mod nettest`, `#[cfg(feature = "nettest")]`):
  two in-kernel frame FIFOs wire a `KernelLoopNic` to a `PeerLoopNic`; the real
  `NetService` runs on one end, a plain smoltcp "peer" interface on the other. A
  **manually-advanced clock** drives both stacks *and* the idle-reaper deadline, so the
  PI-NET-12 wedge scenario is deterministic and NIC-free. It runs at the **top** of
  `genet_bringup` (before the DTB skip) so raspi4b executes it.
* **Witness** — a self-checking bitmask line, asserted by the battery:
  `:: NET-GATE: tcp/http/mdns loopback battery PASS [w=0xf] (basic|flood-reap-recover|fin-recycle|table-full) ::`
  * `0x1` full handshake + `GET` → `HTTP/1.0 200`
  * `0x2` half-open flood → `SATURATED` (listen==0) → idle-reaper aborts 4 TCBs → recovery fetch 200
  * `0x4` served connection's FIN close recycles the TCB back to LISTEN
  * `0x8` a table-full 5th connection while saturated is RST/refused

  **Deterministic-reaper note:** the reap scenario advances the clock in small steps
  (not one jump) so the peer keeps answering keepalive probes — the transport
  `set_timeout`/`set_keep_alive` never aborts, leaving the **app** idle-reaper as the
  sole force-abort path (so `[net12] reaped` counts what PI-NET-12 actually fixed).

Arm with `UNAOS_NETTEST=1 UNAOS_PI=1 ./arroyo kernel8-test` (implies `genet`). Default
OFF ⇒ the loopback module + its call vanish; the kernel8 image is byte-identical to a
plain build. `nettest` is hardware-free, so it also runs under `test-arm` if armed.

### `net14` — the outbound client, "UnaOS asks" (PI-NET-14)

Everything above **answers** the network; PI-NET-14 **initiates**. After the serving
pool + mDNS responder arm (and are witnessed), `arm_net_service` runs `net14_ask` **once
on the BSP, before the `net9` poll task is spawned** on a secondary — so there is no
concurrency against the pool, and it uses **its own temporary, stack-buffered sockets**
(local `SocketSet`s freed on return). The pool is never touched: the first `[net12]`
census the poll task reports is a clean `listen=4`.

* **DNS client** — a UDP :53 A-query to the lease's DNS server, resolved with retransmit
  over a bounded real-time window. The DNS server is currently the **gateway** (the
  lease's DNS-server option lives in smoltcp's `dhcpv4::Config` but `NetConfig` in the
  shared `net_phy.rs` does not surface it yet — a cross-lane fold; the gateway is the
  resolver on a typical home router, and is the brief's stated fallback).
* **Hostile-input hardening** — the response parser (`net14_parse_a` / `net14_skip_name`)
  treats every byte as adversarial: the header length + transaction id are checked, the
  QR bit must say "response", a non-zero RCODE surfaces as a typed `ServerErr`, the
  question/answer sections are walked with all fixed fields bounds-checked, the answer
  walk is capped, and — the key guard — **compression pointers are never dereferenced**.
  A pointer is two bytes and terminates a name, so returning `off+2` is both correct for
  skipping and **immune to the compression-loop DoS by construction** (no visited-set
  needed); the label walk additionally carries a 127-hop cap. Any violation returns a
  typed error, never a panic or an unbounded loop.
* **HTTP client** — a TCP connect out to the resolved address on :80, a `GET / HTTP/1.1`
  with `Host` + `Connection: close`, capturing the status code, byte count, and a
  sanitised body excerpt. A short `set_timeout` distinguishes a black-holed SYN
  (`connect timeout`) from a peer RST (`connect refused`).
* **Failure honesty** — every leg has a one-line witness so a metal boot localises the
  failure: `dns … => timeout (no upstream?)`, `malformed response (rejected)`,
  `server rcode N`, `no A record`, `connect refused (RST)`, `connect timeout`,
  `connected, no response`, or a `(non-200)` note on the GET line. On a bench segment
  with no upstream reachability, a clean `dns timeout` **is** an acceptable metal
  outcome — the QEMU gate is the correctness proof.
* **Client gate** (`nettest::run14`, same loopback seam, kernel = client / peer = server):
  `:: NET14-GATE: dns/http client battery PASS [w=0xff] (parse-ok|parse-malformed|parse-loop|parse-rcode|dns-rt|http-200|refused|timeout) ::`
  * `0x01`/`0x02`/`0x04`/`0x08` — pure parser checks (no sockets): well-formed A
    resolves; a truncated response is rejected; a non-terminating name bails on the hop
    cap; an NXDOMAIN RCODE surfaces as `ServerErr`.
  * `0x10` a live loopback DNS query/response resolves `example.com`.
  * `0x20` a live loopback HTTP `GET` returns `HTTP/1.1 200`.
  * `0x40` connect to a closed peer port is RST/refused; `0x80` connect to a black-hole
    address (no host on the segment) hits the transport timeout.

Expected metal witnesses (target host `example.com`):
`:: PI-GENET: [net14] dns example.com -> <ip> ::`,
`:: PI-GENET: [net14] GET http://example.com/ -> HTTP/1.1 200 (<n> bytes) ::`,
`:: PI-GENET: [net14] body: <excerpt> ::`.

### DHCP shape

The DHCP window is **15 seconds** (GENET-6, `31f0d269`). The original 5 s window
closed just before a real 342-byte OFFER landed at boot-P13; 15 s is the shape that
leased on the Orin, where bootpd answered the *second* DISCOVER. On no lease the
driver falls back to a static address and reports it honestly. The station MAC comes
from the DTB `local-mac-address` property of the GENET node.

### PI-NET-15 — serving the filesystem (`GET /fs/`)

The serving pool gains filesystem routes on the same `:80` listener:

* `GET /` — the status page (now carrying a link to `/fs/`).
* `GET /fs/` — an HTML listing of the native unafs root (name + size; files linked).
* `GET /fs/<NAME>` — the file's bytes with a `Content-Type` from the 8.3 extension
  (`.htm`/`.html` → `text/html`, `.txt` or no-extension → `text/plain`, else
  `application/octet-stream`). A missing file / rejected name → `404`; a file beyond the
  RAM cap → `413`.

**Lock-vs-serve design.** The unafs mount lock (`with_unafs`) masks IRQs around the
polled SD read (~0.7 s worst case on a real card). The serve path therefore reads the
**whole file into a bounded RAM buffer under ONE short hold** (cap `FS_CAP` = **64 KiB**),
then streams that buffer out through the normal TX path — the lock is **never** held
across `send_slice`, and a file larger than one TX ring drains across several poll steps
(the listener stays *active*, never parked; the idle-reaper still covers a stall). A file
whose inode size exceeds the cap is refused `413` *without* being read. Every fs request
emits a `[net15]` witness carrying the hold duration in **ticks and ms**; a hold past
`FS_HOLD_WARN_MS` (50 ms) appends a `WARN: with_unafs IRQ-mask > 50ms` suffix so the bench
can watch the mask cost (QEMU's emulated SD routinely trips this on the first read).

**Hostile-input path handling.** The `<NAME>` after `/fs/` is validated as a single 8.3
component: 1..=12 bytes from `[A-Za-z0-9._-]`, with `.`/`..` rejected explicitly. That one
charset check rejects directory traversal (`..`, a nested `/`), `%`-escapes (nothing is
decoded — `%` is simply not in the set), zero-length, and oversize names. Bounds-checked
throughout; the request line is captured into a fixed `REQ_CAP` (256 B) buffer.

* **FS route gate** (`nettest::run15`, same loopback seam + scripted peer, reading the K3
  fixture volume the `kernel8-test` image carries):
  `:: NET15-GATE: fs route battery PASS [w=0x1f] (fs-list|exact-bytes|traversal-reject|404|oversize-413) ::`
  * `0x01` `GET /fs/` lists `K3HELLO.TXT`; `0x02` `GET /fs/K3HELLO.TXT` returns the exact
    fixture bytes; `0x04` `GET /fs/../evil` is rejected (`404`); `0x08` a missing file
    `404`s; `0x10` an oversize file is refused `413` (driven against `K3PAT.BIN` via a
    gate-scoped cap override — no card state added). A multi-block full serve of
    `K3PAT.BIN` at the real cap (12288 bytes, pattern-faithful) is a diagnostic line.

Expected metal witnesses:
`:: PI-GENET: [net15] fs route armed (root listing + /fs/<name>, cap 64 KiB) ::`,
then per request e.g.
`:: PI-GENET: [net15] GET /fs/K3HELLO.TXT => 200 37 bytes (with_unafs hold <t> ticks / <n> ms) ::`.

### PI-FS-5 — the shell shares the HTTP namespace (`ls /usb`, `fdisk -l`)

The Pi shell's storage verbs are wired to the **same two volumes the HTTP `/fs` tree
exposes**, so the browser and the console never disagree about what is mounted:

* **`ls`** (`shell::pi_ls`) routes by path prefix. `ls /` (and any non-`/usb` path) lists
  the native **unafs** volume — the store `GET /fs/` serves — via `pi_ls_collect`
  (`with_unafs` + `resolve_path`/`ls`). A `/usb` or `/usb/<sub>` path instead lists the
  live **USB FAT** mount via `pi_usb_ls_collect`, which re-mounts read-only through
  `fat::mount_source(BlockSource::Usb)` — the identical API the `GET /fs/usb` route
  (PIUSB-27) uses — and walks each path component by its **display name** (LFN-aware,
  PI-FS-3), so `ls /usb/SUBDIR` and long names like `Long Filename Example.txt` resolve.
  When the USB stick is mounted, bare `ls /` appends a `usb/` pseudo-entry, mirroring the
  `/fs/` HTML listing's `usb/` link. `ls -l` keeps PI-FS-4's size + FAT last-write date
  columns on FAT paths (a dashed date on unafs, which carries no mtime).
* **`fdisk -l`** on the Pi reports **both** storage devices: the **SD card** (emmc2
  geometry from `block::info()` — the global device that hosts unafs + the FAT boot
  partition) and, when present, the **USB stick** (its own geometry from
  `block::usb_info()`, plus the FAT **type / volume size / label** read from the live
  read-only mount via `fat::label()` / `fat::volume_bytes()`). x86 keeps its single-device
  report (its one block device *is* the USB stick).

Both verbs render panel-only on the bench, so each mirrors its output to serial: `ls`
paths (unafs and `/usb`) witness as `:: ls1: <path>: <names> (N file, M dir) ::`;
`fdisk -l` lines mirror as `:: fs5: <line> ::`. The `pi_usb_ls_witness` fires from the
PIUSB-27 mount witness (once per bring-up + every hot-plug), so `UNAOS_FATIMG=1 ./arroyo
test-arm` proves `ls /usb` headlessly, e.g.
`:: ls1: /usb: … Long Filename Example.txt MixedCaseName.md … SUBDIR/ (10 file, 2 dir) ::`
and `:: ls1: /usb/SUBDIR: LEVEL1.TXT Nested Directory/ (1 file, 1 dir) ::`.

---

## 4a. "UnaOS knows what time it is" — SNTP client + kernel wall-clock (PI-NET-16)

UnaOS has a free-running counter (CNTPCT) and a 250 Hz tick but no notion of *civil*
time. PI-NET-16 adds one: an SNTP client (RFC 4330, client mode 3, v4) that learns the
UTC second from the network, and a monotonic-anchored wall-clock that ticks it forward
between syncs off CNTPCT.

**Wall-clock representation.** On each successful sync the module captures a
`WallClock { anchor_unix, anchor_cntpct, stratum }` — the UTC Unix second the server
reported plus the CNTPCT value read in the same breath. The live UTC second is
`anchor_unix + (cntpct_now - anchor_cntpct) / cntfrq`: the free-running counter supplies
the elapsed time, so the clock advances monotonically between the ~6-hourly re-syncs with
no wall-clock hardware and never runs backwards within one anchor. State is a module-local
`spin::Mutex<Option<WallClock>>` with a small read API (`wall_unix_now`, `wall_anchor`).

**Integrator fold — LANDED (CLOCK-1).** The kernel-wide clock service now lives in shared
core: [`crate::clock`](../02_KERNEL_CORE/clock.md) owns the Unix-second anchor, the source
tag (`Sntp{stratum}`/`Manual`/`Unset`), the civil (Hinnant) math, and the ISO-8601 renderer,
behind the same arch monotonic seam JD17 uses (aarch64 CNTPCT/CNTFRQ, x86_64 invariant TSC).
PI-NET-16's SNTP client keeps its `wall_set`/`wall_unix_now`/`wall_anchor`/`render_iso8601`
names as **thin forwarders** into `crate::clock` — every call site and the NET16-GATE witness
stay byte-identical; `NET16_SYNCS` still bumps on each set. So `time` (shell), and eventually
log timestamps and fs mtimes, read the same clock the SNTP client anchors. (Log-timestamp and
fs-mtime adoption remain separate follow-up arcs; this fold does not rewire `fat_stamp()`.)

**Parser fold — LANDED (NET-SNTP-FOLD).** The RFC 4330 wire logic PI-NET-16 authored — the
reply parser, the request builder, `ntp_to_unix`, the `Sntp` outcome enum, and the wire
constants (NTP↔Unix epoch delta, era-1 offset, sanity band, request first byte, NTP port) —
was extracted by SNTP-X86 into the shared, arch-neutral [`crate::net_sntp`](../../../../unaos/crates/kernel/src/net_sntp.rs)
so the pi/genet client and the x86 smolnet client render one parser. This fold retires
genet's duplicate: `net16_parse_sntp`/`net16_build_request` are now thin forwarders to
`crate::net_sntp::parse`/`build_request`, the local `enum Sntp` is `use crate::net_sntp::Sntp`,
`NTP_PORT` aliases the shared constant, and the `run16` gate fixture calls
`crate::net_sntp::build_reply`. The two parsers were **byte-identical** at the fold (same field
checks, same band bounds `1_700_000_000..=4_000_000_000`, same KoD/alarm/version handling), so
no reconciliation was needed and `crate::net_sntp` was not modified. Every `[net16]`/`[net16t]`
witness line and `NET16-GATE [w=0x3f]` pass unchanged. (Fold also repaired a CLOCK-1 orphan: the
gate's live-set-clock scenario referenced the removed `WALL_CLOCK` static; it now calls
`crate::clock::clear_anchor()`, the direct equivalent — that reference had silently broken the
`nettest`+`genet` build since CLOCK-1.)

**2036 rollover stance.** NTP's 32-bit seconds field rolls over 2036-02-07. The conversion
is era-aware per RFC 4330 §3: a value with the high bit **set** is era 0 (1900-based,
covering 1968..2036) and maps to Unix via `ntp - 2208988800`; with the high bit **clear**
it is era 1 (2036-based) and maps via `ntp + 2085978496`. We are in era 0 today; a resolved
time is additionally clamped to a sanity band (~2023-11 .. ~2096) so a misconfigured or
spoofed server cannot jam the clock to a nonsense year.

**Hostile-input handling.** `net16_parse_sntp` bounds-checks the 48-byte reply before any
field read, then validates: LI≠3 (an alarm/unsynchronized server is rejected), version in
3..=4, mode==4 (server), stratum≠0 (stratum 0 = Kiss-o'-Death → rejected, RFC 4330 §8),
stratum≤15 (reserved rejected), and a non-zero transmit timestamp inside the sanity band.
No float math (1 s resolution; the fraction word is ignored). Every failure is a typed
outcome with its own one-line witness: `sntp timeout`, `sntp malformed (rejected)`,
`sntp KoD (rejected)`.

**Sync lifecycle.** The initial sync runs once on the BSP at `arm_net_service` time
(blocking, bounded, own temporary sockets — the `net14_ask` discipline): resolve
`pool.ntp.org` via the NET-14 DNS client (DNS server = the gateway today, the same
`NetConfig` cross-lane gap NET-14 notes; falls back to querying the gateway directly as the
time source if DNS times out), query it, set the clock, print the boot witness. Re-sync
rides the persistent poll loop as a non-blocking state machine (`sntp_step`) over a UDP
socket in the service's own pool: on the ~6 h cadence it fires one request and reads the
reply on a later poll — never blocking the 4 ms poll. A missed/rejected reply schedules a
nearer retry.

* **SNTP client gate** (`nettest::run16`, same hardware-free loopback seam):
  `:: NET16-GATE: sntp client battery PASS [w=0x3f] (parse-ok+iso|reject-short|kod|reject-alarm|live-set-clock|resync-timeout) ::`
  * `0x01` the parser accepts a well-formed reply → the exact injected instant, round-tripping
    through the NTP epoch **and** rendering to `2026-07-22T15:30:45Z`; `0x02` rejects a short
    (<48 B) packet; `0x04` surfaces a stratum-0 KoD; `0x08` rejects an LI=3 alarm reply;
    `0x10` a live loopback exchange drives the real `sntp_step`, anchors the wall-clock, and
    the anchored ISO matches; `0x20` a re-sync to a black-hole address takes the honest
    timeout path.

Boot witness (metal): `:: PI-GENET: [net16] dns pool.ntp.org -> <ip> ::` then
`:: PI-GENET: [net16] sntp <ip> -> 2026-07-22T14:03:07Z (stratum N, rtt ~M ms) ::` on
success, or `[net16] sntp <ip> => sntp timeout` on a bench segment without upstream. Each
re-sync prints `[net16] resync <ip> -> <iso> (stratum N)`. On the status page Peter sees a
new `time (UTC): 2026-07-22T14:03:07Z` line (or `unsynced (no SNTP yet)` before the first
sync).

### PI-UI-3 — `date` / `time` / `ifconfig` at the Pi shell

The SNTP sync above anchors the *civil* (Unix-epoch) clock via `clock::set_anchor` — it does
**not** plant the JD17 FAT anchor (`clock::set`). The `date` verb historically read only the FAT
anchor (`clock::now()`), so on a synced Pi — lease + SNTP both logged — `date` still printed
`clock not set` while `time` (which reads the civil anchor) showed the real UTC. PI-UI-3 makes
`date` read the **unified** clock: `clock::unix_now()` → `civil_from_unix` first, falling back to
the FAT anchor, then to the honest UNSET state. `date` and `time` now agree, and a networked board
shows the real date with zero operator action.

`ifconfig` on the Pi previously reached for `drivers::e1000::info()` (the x86 Intel NIC) and always
printed "No network device ready." PI-UI-3 gives it a GENET backend — `genet::netinfo()` reads the
settled IPv4 + lease flag from `net_phy::settled_ipv4()`, the registered MAC from `GENET_DEVICE`,
and the gateway recorded at `bind_smoltcp` (a new `NET_GW` atomic) — printing MAC/link, IP(dhcp|
static)/gateway, and the civil-clock sync state, matching the x86 verb's line shape.

Verb output renders panel-only on the bench, so each of `date`/`time`/`ifconfig` also emits a
`:: ui3:<verb>: <line> ::` serial witness with identical content (via the shell's `ui3_say`
helper) — a headless capture can verify the values without the panel.

---

## 4b. "UnaOS is discoverable" — DNS-SD service advertisement (PI-NET-17)

PI-NET-11 makes the Pi answer `unaos.local`; PI-NET-17 makes it *discoverable*. It
layers DNS-SD (RFC 6763) onto the same mDNS UDP socket so a macOS/Bonjour browser
(`dns-sd -B _http._tcp`, Safari's network browser, the Finder network view) lists the Pi
by name and can connect to its HTTP status service without being told the address.

**Advertised service.** One instance, `"UnaOS Pi 4"`, of type `_http._tcp.local`, port
**80** (the net10 status service), target `unaos.local`, with a minimal TXT `path=/`. The
service labels are compile-time constants (`LBL_INSTANCE`/`LBL_SERVICE`/`LBL_META`/
`LBL_HOST`); the instance label carries a space — legal in a DNS-SD instance label, which
is a single length-prefixed label on the wire, not a dotted name.

**Query shapes answered** (all over the existing 5353 socket, dispatched by
`mdns_classify` after each poll's `mdns_step`):

* **PTR `_http._tcp.local`** → the one-shot bundle a resolver needs: **PTR** (service →
  instance) in the ANSWER section, plus **SRV** (instance → `unaos.local:80`), **TXT**
  (`path=/`), and **A** (`unaos.local` → lease) stuffed as ADDITIONAL records
  (ANCOUNT=1, ARCOUNT=3) — so a browser connects without a second round-trip.
* **PTR `_services._dns-sd._udp.local`** (the meta-query) → the service-type PTR
  (`_services._dns-sd._udp.local` → `_http._tcp.local`), so a browser enumerating service
  *types* sees `_http._tcp`.
* **SRV / TXT for the instance** → that record directly (SRV additionally stuffs the A).
* **A `unaos.local`** → the net11 host answer (unchanged).

TTLs follow RFC 6763 §10: host-name-bearing records (A, SRV) 120 s; shared records (PTR,
TXT) 4500 s. SRV/TXT/A carry the cache-flush bit (class `0x8001`); the shared PTRs do not
(class `0x0001`). Response names are written **in full** (no compression pointers — the
net11 responder writes full names too; simpler and legal).

**Unsolicited announcements.** On bring-up (once the interface is configured) the poll
task fires **3 gratuitous multicast announcements** ≥1 s apart (RFC 6762 §8.3), each
carrying PTR+SRV+TXT+A in the ANSWER section (ANCOUNT=4), so a browser already listening
learns the Pi immediately rather than only on its next query. State is two `NetService`
fields (`announce_left`/`announce_next_ms`); `announce_step` rides the same 4 ms poll and
never blocks. After the last announcement it prints the witness once.

**Hostile-input handling.** `mdns_classify` decodes the first-question QNAME once through
`mdns_read_name` — every byte bounds-checked, the label count capped, compression pointers
followed with the **same hop-cap discipline `net14_skip_name` uses** (each jump must go
strictly backward and is re-bounds-checked; hop count capped so a pointer loop terminates),
reserved length bits rejected — then compares the decoded labels case-insensitively against
the advertised names. A malformed packet, an unknown name, or an unknown QTYPE classifies to
`None` and is dropped silently. Response records are written with every byte bounds-checked
against the output buffer; a builder overflow drops the response rather than emit a truncated
one.

**Query-answer census.** Three counters (`NET17_PTR`/`NET17_SRV`/`NET17_TXT`; PTR counts
both the service-PTR and meta-PTR answers) drive a change-only, rate-limited
`[net17] answered ptr=<p> srv=<s> txt=<t>` witness (the net11 cadence). Host A answers keep
counting on the existing `[net11] answered N queries` line.

* **DNS-SD gate** (`nettest::run17`, same hardware-free loopback seam — a scripted peer
  sends real mDNS queries at the responder over the loopback `Device`):
  `:: NET17-GATE: dns-sd advertisement battery PASS [w=0xf] (ptr-bundle|meta-ptr|malformed-ignored|unknown-type-ignored) ::`
  * `0x1` a PTR query for `_http._tcp.local` returns the bundle, asserted field-by-field
    (PTR → instance, SRV port 80 → `unaos.local`, TXT `path=/`, A → the kernel IP);
    `0x2` the meta-query returns the service-type PTR; `0x4` a truncated query is ignored
    (no counter moves, no reply); `0x8` an unknown QTYPE (MX) for the service name is
    ignored.

Boot witness (metal): `:: PI-GENET: [net17] dns-sd advertising _http._tcp.local instance
"UnaOS Pi 4" :80 (announce x3) ::` at arm, then after the announcements
`:: PI-GENET: [net17] dns-sd announced _http._tcp (UnaOS Pi 4 -> unaos.local:80) ::`.
What **Peter** sees: the Pi appears by name in Safari's Bonjour/network browser and in
`dns-sd -B _http._tcp` on the Mac.

---

## 4c. "UnaOS says no fast" — mDNS negative responses (PI-NET-18)

net11/net17 make the Pi *answer* the queries it can. But a macOS/Safari client always queries
**A and AAAA in parallel** for any `.local` name, and UnaOS has no IPv6. The mDNS responder used to
simply drop the AAAA query (owned name, unmatched QTYPE → `None`), so the client waited out a
timeout before falling back to the A address — the "trying really hard" first-connection stall.
PI-NET-18 closes that: when a query asks a type we do **not** hold for a name we **own**, we answer
with an **NSEC** record (RFC 6762 §6.1) that asserts exactly which types the name *does* have. The
client learns immediately "no AAAA exists here" and proceeds on the A address with no timeout.

**What changed.** `mdns_classify` no longer drops an owned-name/unmatched-QTYPE query — it returns
`MdnsAsk::Nsec(NsecName)` keyed to which name matched. `mdns_step` dispatches it to
`build_nsec_response`. A name we do **not** own is still dropped silently (unchanged). The
hostile-input decoder (`mdns_read_name`) and the bounds-checked `put_*`/`rec_*` writers are reused
verbatim — `rec_nsec` bounds-checks every append and drops the response on overflow.

**The NSEC record** (`rec_nsec`, RFC 4034 §4 / RFC 6762 §6.1): NAME, TYPE=NSEC (47),
CLASS = IN | cache-flush (`0x8001`), TTL 120, RDATA = the **Next Domain Name** (in mDNS this is the
record's *own* name — there is no ordered zone) followed by **one type bitmap window**. Every type we
advertise (A=1, PTR=12, TXT=16, SRV=33) is < 256, so window block 0 covers them all and a single
window suffices. The bitmap is **MSB-first within each byte** (RFC 4034 §4.1.2: bit *k* of window *b*
= type `256·b + k`), and its length is trimmed to the highest byte carrying a set bit:

| Owned name | Types present | Bitmap field bytes (window, len, bitmap…) |
| --- | --- | --- |
| `unaos.local` | A (1) | `00 01 40` |
| instance (`"UnaOS Pi 4"._http._tcp.local`) | SRV (33) + TXT (16) | `00 05 00 00 80 00 40` |
| `_http._tcp.local` / `_services._dns-sd._udp.local` | PTR (12) | `00 02 00 10` |

For A=1: byte 0, bit `7-(1%8)=6` → `0x40`. For TXT=16: byte 2, bit `7-0=7` → `0x80`. For SRV=33:
byte 4, bit `7-1=6` → `0x40`.

**Proactive NSEC (RFC 6762 §6.2).** The host **A** answer (`build_mdns_response`) now also stuffs the
host NSEC (A-only) as an **ADDITIONAL** record (ANCOUNT=1, ARCOUNT=1), so a client that also wanted
AAAA never even has to ask — the negative arrives with the positive. The builder was rewritten onto
the same `rec_a`/`rec_nsec`/`put_*` helpers (previously hand-indexed).

**Census.** A `[net18] nsec <N>` witness (change-only, rate-limited, same cadence as `[net17]`) counts
NSEC responses emitted.

* **Negative-response gate** (`nettest::run18`, same hardware-free loopback seam + scripted peer):
  `:: NET18-GATE: mdns negative-response battery PASS [w=0xf] (host-nsec|instance-nsec|foreign-silence|a-additional) ::`
  * `0x1` AAAA `unaos.local` → NSEC asserting A-only, **exact bitmap `00 01 40`**; `0x2` AAAA the
    instance → NSEC asserting SRV+TXT, **exact bitmap `00 05 00 00 80 00 40`**; `0x4` AAAA for a name we
    do not own → silence (no reply, no counter move); `0x8` the host A answer carries the NSEC
    additional (ARCOUNT≥1, bitmap `00 01 40`).

  (NET17's scenario D — "unknown QTYPE ignored" — was retargeted to a **foreign** name, since an unknown
  type for a name we *own* now legitimately gets an NSEC; the owned-name negative path is NET18's.)

Expected **metal effect.** First-connection latency from Safari to `http://unaos.local/` drops: the
parallel AAAA query is answered instantly with a negative instead of timing out, removing the
multi-second "trying really hard" stall on the first connect.

---

## 4d. "UnaOS announces itself" — gratuitous host-name publish (NET-CARRIES)

net11/net17/net18 all *answer queries*: the Pi resolves `unaos.local`, serves DNS-SD, and returns
NSEC negatives — but only reactively, once a client asks. NET-CARRIES adds the **proactive** twin: on
bring-up the responder **multicasts the host's own address record**, so a Mac on the LAN caches
`unaos.local` **before** it ever queries.

**What changed.** `build_host_announcement` builds a gratuitous host publish — QR=1 AA=1, QDCOUNT=0,
**ANCOUNT=2, ARCOUNT=0**, both records authoritative in the ANSWER section (RFC 6762 §8.3):

* an **A** record (`unaos.local` → lease IP), cache-flush, TTL 120; and
* an **NSEC** record asserting the host has **A only** (no AAAA) — bitmap `00 01 40`, the same
  assertion net18 returns reactively.

`announce_step` now multicasts this host publish alongside each of the 3 gratuitous DNS-SD service
announcements (same `announce_left`/`announce_next_ms` cadence, ≥1 s apart, never blocks the poll).
Publishing the NSEC **with** the A is what makes a dual-stack client's *first* connect snappy: the Mac
reads "`unaos.local` has A, and no AAAA" straight off the multicast cache and never issues (nor waits
out) an AAAA query. It is the announcement-path extension of net18's reactive negative and net11's
reactive A — no query round-trip needed at all.

The builder reuses the bounds-checked `rec_a` / `rec_nsec` / `put_*` writers verbatim; on overflow it
returns 0 and the host publish is simply skipped (the service announcement still goes out).

* **Host-name publish gate** (`nettest::run20`, a pure deterministic builder assertion — the record is
  total and output-only, so no loopback pump is needed):
  `:: NET20-GATE: mdns host-name publish battery PASS [w=0x7] (header|a-record|nsec-a-only) ::`
  * `0x1` header is a gratuitous response (QR=1 AA=1, QDCOUNT=0, ANCOUNT=2, ARCOUNT=0); `0x2` the A
    record resolves `unaos.local` → the lease IP (cache-flush, RDLENGTH 4); `0x4` the NSEC asserts
    A-only, **exact bitmap `00 01 40`**.

Boot witnesses (metal): `:: PI-GENET: [net20] host-name publish armed (unaos.local A + NSEC A-only,
announce x3) ::` at arm, then after the announcements
`:: PI-GENET: [net20] host published unaos.local A=<ip> + NSEC A-only (AAAA-negative) ::`.

Expected **metal effect.** `ping unaos.local` from a Mac resolves off the cached announcement without a
query round-trip, and `dns-sd -G v4v6 unaos.local` returns the v4 address immediately with a negative
for v6 — no AAAA stall on the very first connect.

---

## 5. Bench verification

| Claim | Arc / commit | Boot | Verdict |
| --- | --- | --- | --- |
| RX invalidate-before-read fixes garbage payloads | GENET-5 `0370ebc1` | P13 | Real payloads (`rx[1..5]` pre16==post16) where P10 read garbage — VINDICATED |
| rx[0] first-slot status block is unwritten; descriptor length authoritative | GENET-8 `565bf8c7` | P13, P17 | `dsc` valid + `sb=0` for slot 0; GENET-6 re-poll refuted (10k spins never flip) |
| DHCP window 15 s (was 5 s) | GENET-6 `31f0d269` | P13→P16 | 5 s closed just before a live frame; 15 s leased |
| TX clean-before-publish is the egress fix | GENET-7 `acf11670` | P16 | **FIRST DHCP LEASE — 192.168.2.3/24**, unicast OFFER/ACK to our MAC, gateway ARP + ping back |
| Lease survives / regresses clean | — | P17 | Lease regression PASS |
| PHY LED selectors fix stuck amber | GENET-8 `565bf8c7` | P18 | LED OUT at power-on — METAL-CONFIRMED |
| First TCP service (HTTP :80 over net9) | NET-9/10 `15c6626e` / `4055900c` | P19 | **"UnaOS answers"** — Mac browser loaded `http://192.168.2.3/` off the Pi; screenshot in hand |
| TCP/HTTP/mDNS regression gate runs off-metal | NET-13 `nettest` | QEMU raspi4b + test-arm | `:: NET-GATE: ... PASS [w=0xf] ::` — basic 200, flood→SATURATED→reaped +4→recovery 200, FIN recycle, table-full RST, mDNS answered |
| Outbound DNS + HTTP client gate runs off-metal | NET-14 `nettest` | QEMU raspi4b + test-arm | `:: NET14-GATE: ... PASS [w=0xff] ::` — parser accepts well-formed / rejects truncated + non-terminating name + surfaces RCODE, live loopback DNS resolve, HTTP 200, connect refused (RST), connect timeout |
| "UnaOS asks" — outbound resolve + fetch | NET-14 | *(pending metal)* | `[net14] dns example.com -> <ip>` + `GET http://example.com/ -> HTTP/1.1 200 (<n> bytes)` + body excerpt; or an honest per-leg failure witness |
| FS route gate (`GET /fs/` + `/fs/<name>`) runs off-metal | NET-15 `nettest` | QEMU raspi4b | `:: NET15-GATE: ... PASS [w=0x1f] ::` — `/fs/` lists the fixture, exact bytes, traversal `/fs/../evil` → 404, missing → 404, oversize → 413; multi-block `K3PAT.BIN` (12288 B) pattern-faithful |
| "UnaOS serves its filesystem" — browse `/fs/` | NET-15 | *(pending metal)* | `[net15] fs route armed (…, cap 64 KiB)` + per-request `GET /fs/<name> => 200 <n> bytes (with_unafs hold <t> ticks / <n> ms)` (WARN suffix if the hold > 50 ms) |
| SNTP client + wall-clock gate runs off-metal | NET-16 `nettest` | QEMU raspi4b + test-arm | `:: NET16-GATE: ... PASS [w=0x3f] ::` — parser accepts well-formed + renders ISO / rejects short / surfaces KoD / rejects LI=3 alarm, live loopback sets the clock, re-sync black-hole timeout |
| "UnaOS knows what time it is" — SNTP sync | NET-16 | *(pending metal)* | `[net16] dns pool.ntp.org -> <ip>` + `[net16] sntp <ip> -> <iso>Z (stratum N, rtt ~M ms)`; status page gains a `time (UTC)` line; or an honest `sntp timeout` |
| DNS-SD advertisement gate runs off-metal | NET-17 `nettest` | QEMU raspi4b (`UNAOS_NETTEST=1 UNAOS_PI=1 kernel8-test`) | `:: NET17-GATE: ... PASS [w=0xf] ::` — PTR query → PTR+SRV+TXT+A bundle (field-asserted), meta-query → service-type PTR, malformed query ignored, unknown QTYPE ignored |
| "UnaOS is discoverable" — DNS-SD `_http._tcp` | NET-17 | *(pending metal)* | `[net17] dns-sd advertising _http._tcp.local instance "UnaOS Pi 4" :80 (announce x3)` + `[net17] dns-sd announced _http._tcp (UnaOS Pi 4 -> unaos.local:80)`; Pi appears by name in Safari's Bonjour browser / `dns-sd -B _http._tcp` on the Mac |
| mDNS negative-response (NSEC) gate runs off-metal | NET-18 `nettest` | QEMU raspi4b (`UNAOS_NETTEST=1 UNAOS_PI=1 kernel8-test`) | `:: NET18-GATE: ... PASS [w=0xf] ::` — AAAA `unaos.local` → NSEC A-only (bitmap `00 01 40`), AAAA instance → NSEC SRV+TXT (`00 05 00 00 80 00 40`), AAAA foreign name → silence, host A answer stuffs NSEC additional |
| "UnaOS says no fast" — NSEC negative for AAAA | NET-18 | *(pending metal)* | first-connection Safari stall gone (AAAA answered negative, no timeout); `dns-sd -q unaos.local AAAA` returns a negative immediately; `[net18] nsec N` witness counts responses |

QEMU `raspi4b` (bcm2838) does **not** model GENET; the DTB census makes bring-up a
clean pre-MMIO skip (`kernel8-test` stays green). Every *metal* finding above is
attended-metal-only — but PI-NET-13's `nettest` loopback gate now exercises the
TCP/HTTP/mDNS *service logic* deterministically in QEMU (hardware-free), so those
regressions fail the battery rather than waiting for a bench sitting.
