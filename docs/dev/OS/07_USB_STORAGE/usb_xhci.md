# USB (xHCI) and Mass Storage

The kernel drives USB 3.0 through an interrupt-driven xHCI host-controller driver
(`unaos/crates/kernel/src/drivers/xhci/`) and exposes attached USB mass storage
through a block interface (`drivers/block.rs`). Input devices (keyboard, mouse,
tablet) are delivered through the same controller.

> **Branch note.** The xHCI driver is part of the shared interrupt foundation and
> is present on the combined integration branch alongside SMP, networking, and
> video.

---

## 1. Driver structure

`drivers/xhci/`:

| File | Responsibility |
| --- | --- |
| `mod.rs` | Controller lifecycle, the interrupt path, enumeration, the storage/hub service routines. |
| `trb.rs` | Transfer Request Block (`Trb`) layout. |
| `event.rs` | The event ring (`EventRing`, the ERST table). |
| `ring.rs` | Command/transfer rings (`TransferRing`). |
| `context.rs` | Device/slot/endpoint context structures. |

---

## 2. Controller bring-up

`xhci::init(base_address)` takes the MMIO base handed over by the bootloader and:

1. Reads `CapLength` and computes the operational register base.
2. Halts the controller and waits for `USBSTS.HCH`.
3. Resets it (`USBCMD.HCRST`) and waits for the reset to complete and the
   controller-not-ready bit to clear.
4. Programs DCBAAP / CRCR / the event-ring segment table, enables interrupter 0,
   sets `CONFIG` (MaxSlotsEn), and starts the controller.

On real hardware the driver also performs the **BIOS→OS handoff** (claiming the
controller from firmware) before reset. Hardware handshakes are bounded by a spin
budget (`wait_until`) so a wedged controller cannot hang the boot.

Boot evidence: `xHCI: Controller Started!`, `MaxSlots=64, MaxPorts=8`,
`Interrupter 0 enabled (IMAN.IE set, interrupt-driven)`.

---

## 3. Interrupt model (MSI → local APIC)

xHCI interrupter 0 is delivered by **MSI to IDT vector 0x40** (`XHCI_MSI_VECTOR`).
The handler (`xhci_msi_handler`) calls `xhci::interrupt_ack()` — a **lock-free**,
interrupt-context-safe routine that clears `IMAN.IP` (preserving `IMAN.IE`) and
`USBSTS.EINT`, then signals the local APIC EOI. The handler does **not** drain the
event ring; that is done by the main loop, which owns the controller lock.

This separation matters now that the scheduler is live: the MSI handler touches
no locks or scheduler state, so it can land at any time without deadlocking the
main-loop service routines.

---

## 4. The main-loop service path

The BSP's `kernel_main` loop (the BSP is the hardware-service core) drains and
services the controller each iteration:

```rust
if let Some(xhci) = &mut *XHCI_CONTROLLER.lock() {
    xhci.poll_events();    // drain the event ring (TRB completions)
    xhci.service_storage(); // run any queued Bulk-Only Transport transaction
    xhci.service_hubs();    // single-tier hub port servicing
}
```

`service_storage()` runs synchronous **Bulk-Only Transport (BOT)** transactions in
this safe, non-interrupt context (the actual SCSI READ(10)/WRITE(10) + Command
Status Wrapper exchange), rather than inside the MSI handler.

---

## 5. Block storage interface

`drivers/block.rs` presents the enumerated USB mass-storage device as a block
device:

- `info() -> Option<BlockDeviceInfo>` — vendor/product, block size, block count.
- `read_block(lba, &mut buf) -> Result<usize, BlockError>`
- `write_block(lba, &buf) -> Result<(), BlockError>`

The shell exposes these as `diskinfo`, `read <lba>`, and `write <lba> <byte>`.

Boot evidence: `xHCI: READ(10) LBA0 CSW status=Passed residue=0`,
`xHCI: >>> MISSION SUCCESS (BOT + CSW). TARGET ACQUIRED. <<<`.

---

## 6. Enumeration robustness (metal-informed)

Root-port enumeration is a staged FSM (`debounce → await-reset → reset-settle →
enable-slot → address-device → …`) driven by `service_enum()` with a watchdog on
every stage. Two behaviors come straight from the 2012 rMBP metal bench
(2026-07-08), where a hot-plugged High-Speed SD reader trained at Full-Speed
(failed HS chirp) and then failed every `ADDRESS_DEVICE` with USB Transaction
Error (code 4):

- **Connect debounce** — 100 ms (USB 2.0 TATTDB) between the connect event and
  the first port reset, so the reset never lands on an electrically unsettled
  attach.
- **Escalating retry pacing** — recovery re-resets are spaced 200/400/600 ms
  (attempt-scaled), and a code-4-with-FS-speed failure logs an explicit
  failed-HS-chirp hint in the serial capture.

**Mass storage behind a hub** is detected at the *interface* level (MSC devices
report class 0 at the device level), gets a synchronous bulk Configure-Endpoint
from the hub bring-up path, and hands off to the same `service_storage()` SCSI
bring-up as a root-port device. QEMU reproduction: `UNAOS_HUBSTORAGE=1` attaches
the usb-storage behind a `usb-hub`.

---

## 7. Input

Boot-protocol HID keyboards, mice, and tablets enumerated on the controller are
translated to console input events (a USB HID scancode → ASCII table lives in
`mod.rs`), feeding the same `pal::Event` stream the console consumes.

### 7a. HID pointer (mouse / tablet) path

A pointer interface is detected during config-descriptor parse: `bInterfaceProtocol
== 2` is a **boot mouse** (relative signed deltas → `mouse_is_relative = true`);
protocol 0 is an **absolute pointer** (usb-tablet). The pointer gets its own
`mouse_ring` (interrupt-IN) and `mouse_data_buffer` (separate from the keyboard's,
so a composite kbd+mouse dongle arms both endpoints without their transfers racing
into one buffer). After the device-level `SET_CONFIGURATION` the read is armed
(`queue_mouse_read`), and a boot mouse then gets a main-loop `SET_PROTOCOL(boot)`
(the absolute pointer is not a boot interface and is skipped — the request would
STALL). Each report decodes to `pal::Event::Mouse{dx,dy}` (relative) or
`MouseAbsolute{x,y}` (absolute 0..32767).

**Interrupt-IN dup-Success guard (metal-robustness).** A boot-mouse report is
*always* shorter than the endpoint MPS, which is exactly the case Panther Point's
`XHCI_SPURIOUS_SUCCESS` quirk (device 0x1e31) fires on: after the Short Packet the
controller can post a *duplicate* Success event for the same TD. This is the same
hazard the async EP0 path already guards with `ep0_expect_phys` (§6). `queue_mouse_read`
now records the physical address of the Normal TRB it armed in `mouse_expect_phys`,
and the transfer dispatch processes only the completion whose TRB matches — a dup
(pointing at the already-consumed TD) is ignored, so the same report is never decoded
twice (which would double cursor motion) and the ring is never over-armed. QEMU posts
no dup, so `param` always matches and the guard is transparent there.

### 7b. Serial mouse-witness (bench-assertable)

Because the cursor is invisible on a serial-only capture (the metal bench reads the
kernel only over the FTDI cable), the pointer path emits an assertable, uncounted
witness (`== witness ::` idiom — never `-> PASS`, so no mbench COUNT shifts):

- **Enumeration** — one line per pointer as its interrupt-IN read is armed:
  `:: MOUSE-1: HID pointer detected vid:pid=VVVV:PPPP proto=N relative|absolute
  ep=0xNN mps=N interval=N == witness ::`.
- **Report traffic** — a *bounded* counter (first report, then every 32nd — never
  one line per report, which would flood the cable): `:: MOUSE-1: N reports, last
  dx=.. dy=.. buttons=0xNN == witness ::` (relative) or `.. last x=.. y=.. ..`
  (absolute).

The witness is silent when no pointer enumerates (aarch64 QEMU virt, no-mouse, and
`UNAOS_SKIP_XHCI` never reach the arm site). QEMU exercises both decode paths:
default topology has `usb-tablet` (absolute); add `-device usb-mouse,bus=xhci.0`
(via `UNAOS_QEMU_EXTRA`) for the relative path. Inject motion over QMP
(`input-send-event` with `rel`/`abs` axes) to drive the counter.

### 7c. Enumeration robustness (XENUM-1)

Three real, metal-observed enumeration/hub gaps surfaced at the mouse bench
(2026-07-15) and were closed additively in the shared enum/hub path. None is a
HID-decode bug; all are in the connect/hub topology layer. QEMU cannot reproduce
the underlying silicon timing/topology, so each fix emits an assertable trace and
is metal-verified at an attended bench.

- **M1 — hot-plug CSC re-queue.** A live re-plug of an already-enumerated device
  logged `CSC during enumeration (reset side-effect); not re-queuing` and never
  re-enumerated (workaround: power-cycle with the device pre-plugged). Root cause:
  a disconnect (CSC with CCS=0) left the device's slot **active**, so the
  subsequent re-plug (CSC with CCS=1) matched the `has_slot` guard and was dropped
  as a reset artifact.

  **The CSC classification (the crux).** The USB reset the driver issues to
  enumerate a port itself asserts CSC — but CCS stays **1** throughout (the reset
  never drops the connection). A genuine unplug→replug produces a CCS **0→1**
  edge. So a CCS=0 edge is the unambiguous "the device physically left" signal
  that separates a genuine hot re-plug from the self-induced reset artifact, and
  the deferred re-queue is armed **only** by a real CCS=0 edge — it can never be
  armed by the reset artifact, so it cannot loop.

  Fix: on a disconnect edge, `dispose_disconnected_slots` tears down every slot
  bound to the port (storage/FTDI/HID bindings cleared, DISABLE_SLOT queued,
  `reset_soft_state`) so a re-plug enumerates as a fresh connect. A disconnect on
  the port currently mid-enumeration instead arms `enum_saw_disconnect` + a
  deferred re-queue (`requeue_after_settle`, drained at the top of
  `start_next_port`) so a device replugged during its own enumeration is not lost
  while the in-flight FSM keeps ownership of its half-built slot. The
  reset-artifact CSC (CCS stable, no disconnect edge) is still swallowed.

- **M2 — hub-downstream zeroed-descriptor retry.** A device downstream of a
  working hub enumerated as `class=0x0 vid=0000 pid=0000` + "no HID interrupt
  endpoint" — GET_DESCRIPTOR(device) returned all zeros (the documented vid=0000
  hub-downstream intermittency). `enumerate_downstream` now retries the 18-byte
  device-descriptor read up to `XENUM_DESC_RETRIES` (4) times whenever it reads
  all-zero/short (`bLength < 18` or `bDescriptorType != 0x01`), zeroing the buffer
  before each attempt and pacing a settle between them. A descriptor that never
  reads valid leaves the device **unconfigured** (honest), not enumerated.

- **M3 — SuperSpeed hub descriptor (0x2A).** A USB3 hub read "0 downstream ports
  (characteristics 0x0903)" because `bring_up_hub` always requested the USB2 Hub
  Descriptor (`bDescriptorType` 0x29); SuperSpeed hubs answer the SuperSpeed Hub
  Descriptor (0x2A). `bring_up_hub` now branches on the hub's trained speed
  (slot-context Speed field, SS IDs ≥ 4): 0x2A for SS, 0x29 for HS/FS. A hub
  reporting 0 ports is treated as a failed bring-up (aborted) rather than a
  silently-marked 0-port hub that strands every device behind it.

Bench-assertable trace substrings: `torn down on disconnect` / `re-queuing
deferred hot re-plug` (M1), `device-descriptor all-zero/short` /
`never read valid after` (M2), `desc-type 0x2a` / `reported 0 downstream ports`
(M3), and the `HUB slot N speed S (SS|HS/FS) desc-type 0xNN: P downstream ports`
line the QEMU hub path already prints.

**Metal verdict (2026-07-15, attended rMBP sitting; log
`rmbp-xenum1-metal-2026-07-15.log`):**

- **M1 ✅ METAL-CONFIRMED.** Repeated live unplug→replug cycles on the real
  Panther Point xHCI: each disconnect edge tore down the port's slots
  (`slot N torn down on disconnect; queued for DISABLE_SLOT`, including a full
  hub subtree of four slots at once) and each re-plug logged
  `device connected (hot-plug); queuing for enumeration` and enumerated fresh —
  the mouse re-enumerated and reported after every re-plug. The old failure
  (`not re-queuing`, device lost until power-cycle) did not occur.
- **M3 ✅ METAL-CONFIRMED.** The USB3 hub that previously read 0 ports now
  trains SS and reads its descriptor correctly:
  `HUB slot 6 speed 4 (SS) desc-type 0x2a: 4 downstream ports (characteristics
  0x0009)`; the HS hub still takes the 0x29 branch
  (`speed 3 (HS/FS) desc-type 0x29: 4 downstream ports`). No 0-port abort fired.
- **M2 — LATENT (honest).** The all-zero/short descriptor condition did not
  reproduce at this sitting, so the retry never fired. One downstream device did
  read `vid=0000 pid=0000` but with a *structurally valid* descriptor
  (`bLength ≥ 18`, type 0x01), which M2's structural check deliberately does not
  reject — a distinct condition from the all-zero read M2 targets. The fix
  remains in place with its assertable traces; a future sitting that reproduces
  the zeroed read is the confirming evidence.

---

### 7d. Hub-downstream hot-plug (XENUM-2)

**What.** Before XENUM-2, a hub's downstream ports were scanned exactly **once**,
at `bring_up_hub` time (the `enumerate_downstream` boot sweep). A device plugged
into a hub port *after* bring-up was never noticed, and a hub-downstream
*disconnect* was never handled either — the metal workaround was to plug
downstream devices before boot. XENUM-2 configures and services the hub's
**Status Change Endpoint** (the interrupt-IN bitmap endpoint every USB hub
exposes), so devices that appear or leave behind a hub are enumerated / torn down
live. Root-port hot-plug was already solved (XENUM-1 M1); this closes the hub tier.

**Why (the crux).** A hub reports port changes out of band on a single interrupt-IN
endpoint whose payload is a bitmap: **bit 0 = the hub itself**, **bit N = downstream
port N**. The controller raises a transfer-event completion when the bitmap is
non-zero. That completion lands in the event-dispatch path, which must **not** run
synchronous control transfers (it can be nested inside another sync pump). So the
work is split exactly like the existing hub bring-up:

- **Event dispatch (no control transfers):** decode which ports changed, trace each,
  queue `(hub_slot, port)` into `hub_changes_pending`, and re-arm the interrupt-IN
  read. A Panther-Point dup-Success guard (`hub_int_expect_phys`, the same idiom as
  the pointer read) rejects a duplicated completion for an already-consumed TD.
- **Main loop (`service_hub_changes`, drained from `service_hubs`):** for each queued
  change, `GET_PORT_STATUS` (class request `bmRequestType 0xA3`, `wIndex = port`),
  then act. It is **deferred while a root port is mid-enumeration** (the
  one-port-at-a-time invariant — never interleave a downstream `ENABLE_SLOT` /
  `ADDRESS_DEVICE` into the root FSM) and **bounded per wake** (`HUB_CHANGE_BUDGET`,
  8) so a flapping port cannot starve the loop; leftover changes ride the next pass.

**M1 — configure + service the Status Change Endpoint.** At the end of `bring_up_hub`
(after the port count reads valid and the boot sweep runs), `configure_hub_interrupt_ep`
reads the hub's configuration descriptor for its single interrupt-IN endpoint, issues
one Configure-Endpoint (mirroring the HID endpoint config, preserving the Hub-marked
slot context), allocates a dedicated change buffer + ring, and arms the first read
(length `(nbr_ports + 1 + 7) / 8` bytes). Completions are decoded and each changed
port is traced and `GET_PORT_STATUS`-queried. M1 alone is visibility (no enumeration
behavior change).

**M2 — downstream connect.** On `C_PORT_CONNECTION` set with `PORT_CONNECTION = 1`,
`reset_downstream_port` clears `C_PORT_CONNECTION`, issues `SET_PORT_FEATURE(PORT_RESET)`,
awaits `C_PORT_RESET` (the existing bounded/paced loop), clears it, and reads the
trained speed; the device then enumerates through the **existing** `enumerate_downstream`
machinery with the route extended for this tier (`hub_route | (port << (4 * hub_depth))`,
depth `hub_depth + 1`, same tier-depth cap). SS hub ports force SuperSpeed (their
HS/FS speed bits do not apply) — best-effort per the brief; the HS/FS mouse/keyboard
path is exact.

**M3 — downstream disconnect.** On `C_PORT_CONNECTION` set with `PORT_CONNECTION = 0`,
`disconnect_hub_port` tears down every slot in the SAME physical tree (root `port_id`
match — the xHCI route string does **not** encode the root port, so two hubs on different
root ports carry identical child route values; the root-port check disambiguates the
trees) whose route string carries this hub port's full nibble prefix
(`route_depth > hub_depth` **and** the low `(hub_depth + 1)` nibbles match) — the
route-prefix analogue of `dispose_disconnected_slots`' port-scoping, so a nested hub's
whole subtree goes with it. Bindings are cleared, `DISABLE_SLOT` is queued via the
existing `slots_to_disable` drain, `reset_soft_state` runs. Root-port slots, sibling hub
ports, and other trees are provably untouched (the match requires this tree + this
port's exact prefix);
the summary trace asserts the scope. Every latched change feature on the port is then
cleared (feature selectors 16..20) so the endpoint deasserts and can report the next
change.

**Bench-assertable trace substrings (attended sitting):**

- M1 config: `status-change endpoint configured (ep 0xNN mps M dci D); hot-plug armed`
- M1 change decode: `HUB slot N status-change: port P`
- M1 status read: `HUB slot N port P status: wPortStatus=0xXXXX wPortChange=0xXXXX (SS|HS/FS)`
- M2 connect: `HUB slot N port P connect: resetting + enumerating downstream device`
  (then the existing `HUB downstream slot … class=… vid=… pid=…` enumeration lines)
- M3 disconnect: `HUB slot N port P disconnect: M slot(s) torn down (scope: root-port R
  route-prefix 0xNN mask 0xNN, root + sibling ports + other trees untouched)`
- int-EP error recovery: `HUB slot N status-change read error (code C); re-arming.`
- No-op change (e.g. a boot-reset `C_PORT_ENABLE`): `HUB slot N port P: no actionable
  connection change`

**Metal verdict (2026-07-16, attended rMBP sitting; logs
`rmbp-serial-2026-07-16-114357-boot2-knobon.log` primary +
`…-113049-boot1-knoboff.log` supplement): M1+M2+M3 ✅ METAL-CONFIRMED.** Both hubs
(HS and SS) configured + armed their Status Change Endpoints. Repeated live hub-port
hot-plug cycles across ports 1/3/4: every disconnect tore down exactly the one
downstream slot with the full scope trace (`root + sibling ports + other trees
untouched`), every re-plug reset + re-enumerated the device through the existing route
machinery, and a keyboard hot-plugged into a hub port typed immediately (single event
per press — the §7e dup-guard also metal-clean). Storage and the FTDI mirror survived
every cycle. A transient root-port `ADDRESS_DEVICE` code-17 during a large re-plug was
absorbed by the pre-existing paced-retry recovery (§6) — the full stack held.

**⚠ New metal findings (→ XENUM-3 seed, precisely characterized in the log):**
(1) *A mouse behind the hub still strands* — but NOT via the condition XENUM-1's M2
targets: the downstream device descriptor read completes **structurally valid with
zeroed content** (`vid=0000 pid=0000`, "no HID interrupt endpoint"), so the
`bLength/bDescriptorType` check passes and the M2 retry never fires. Likely a
short/partial read (e.g. only the 8-byte header arrives for an FS/LS device behind the
HS hub); the fix shape is retrying on zero vid+pid or checking the actual transfer
length. (2) *`downstream ADDRESS_DEVICE code 17` has no retry* — the downstream
addressing path gives up on first failure, unlike root ports' paced recovery. The same
hub carried a working keyboard throughout, so the transport is sound; both gaps are in
downstream-enumeration robustness.

---

### 7e. Keyboard interrupt-IN dup-Success guard (PORTSW-1 M0)

The keyboard interrupt-IN arm had the identical latent gap the pointer path (§7a)
already closes: a boot-keyboard report is 8 bytes, *always* shorter than the endpoint
MPS, so it is exactly the short-packet case Panther Point's `XHCI_SPURIOUS_SUCCESS`
quirk (device 0x1e31) fires on — the controller can post a *duplicate* Success for
the same TD after the Short Packet. Harmless for the current external keyboard (no
metal dup observed), but PORTSW-1 (§7f) brings the *internal* keyboard onto this exact
path, where a dup would double-inject the keystrokes and over-arm the interrupt-IN ring.
`queue_keyboard_read` now records the armed Normal-TRB physical address in
`keyboard_expect_phys`, and the keyboard dispatch processes only the matching
completion (mirroring `mouse_expect_phys`). QEMU posts no dup, so `param` always
matches and the guard is transparent there. Metal proof: the `xHCI: KEY` /
`USB-DEBUG: kbd report` lines stay 1-per-keypress (no doubles).

### 7f. Panther Point EHCI→xHCI port switchover (PORTSW-1)

The 2012 rMBP (MacBookPro10,1, Panther Point PCH, xHCI id **0x1e31**) wires the
internal keyboard + trackpad to the **EHCI** companion controller by default; an
xHCI-only driver never sees them. Panther Point can *re-route* its shared USB2 ports
from EHCI to xHCI (and enable the SuperSpeed lanes) via four config registers on the
xHCI PCI function:

| off  | reg          | role                                            |
|------|--------------|-------------------------------------------------|
| 0xD0 | `XUSB2PR`    | USB2 routing SELECT (0 = EHCI, 1 = xHCI per port)|
| 0xD4 | `USB2PRM`    | USB2 routing MASK (which USB2 bits are switchable)|
| 0xD8 | `USB3_PSSEN` | SuperSpeed enable SELECT                          |
| 0xDC | `USB3PRM`    | SuperSpeed routing MASK                           |

Flipping the routable bits makes the internal devices **re-enumerate on the existing
xHCI + HID stack** — the trackpad as a plain boot mouse via `SET_PROTOCOL(boot)` (§7a),
the keyboard via the boot-kbd scancode map. No new HID code; this is the port flip plus
its sequencing and witness.

**Mask-read-before-write discipline.** The `*PRM` mask registers advertise which port
bits are switchable on *this* silicon. The write is `current | (mask & mask)` —
`SELECT |= mask` — so it sets **only advertised bits and clears none**; an unmasked /
undefined bit is never written (writing one is undefined and can wedge the controller).
If a mask reads 0 the silicon advertises no switchable ports of that class: the write is
**skipped and reported**, never forced (a STOP tripwire). Ordering matches Linux
`usb_enable_intel_xhci_ports()`: SuperSpeed enable before USB2 routing.

**Sequencing.** The flip runs in `arch::x86_64::pci::init` **before** `xhci::init`
resets and starts the controller — i.e. before enumeration, **one topology per boot,
never re-flipped live** (re-flipping after storage enumerated could drop a block device
mid-transaction).

**Default-ON, opt-out knob (default fold, 2026-07-16).** The metal verdict below
established that a no-routing boot drops *all* external USB on the 2012 rMBP, so per the
pre-registered Maestro policy the flip now runs **by default** on x86. The opt-OUT is the
`noportsw` Cargo feature (`UNAOS_NOPORTSW=1`, mapped in both `arroyo` and
`builder/src/main.rs`), reserved for the never-run no-routing topology experiment: opted
out, the routing function does not exist, **zero config-space writes**, byte-identical
no-routing media. The flip logic and its mask discipline are unchanged from the M2
opt-in — only the compile gate inverted (`#[cfg(not(feature = "noportsw"))]`) and the
witness suffix now reads `(default-on)`.

**⚠ Corrected metal-baseline framing (review fold).** The predecessor routing code
(`89d10b1`, 2026-06-24) ran **unconditionally** on every x86 build, and every prior
rMBP metal bench log (2026-07-08 → 07-15, incl. the mouse and XENUM-1 sittings) shows
`Intel xHCI port routing applied (dev 0x1e31): USB3_PSSEN 0x0000000f->0x0000000f
XUSB2PR 0x0000000f->0x0000000f`. Therefore on rMBP **metal**: **knob-ON reproduces the
register state every prior bench ran** (XUSB2PR forced 0xf), and **knob-OFF is the
NEW, never-run topology** — the reproducibility claim holds for QEMU only. Two open
facts for the bench: (1) the internals were invisible even *with* XUSB2PR=0xf, so the
arc's central hypothesis is already substantially falsified by existing metal — if the
internal HID sits outside mask 0xf, the mask-disciplined flip cannot route it; (2) the
**cold-boot XUSB2PR value was never captured** (the old line printed mask + post-write
only) — if Apple EFI does not itself route the switchable ports, a knob-off boot could
drop the *external* storage/input the regression baseline rides on. A knob-off build
prints nothing here (the function doesn't exist), so the cold value is captured as the
knob-ON witness's `before` field on the first flip after a genuine cold boot (or after
knob-off boots only, which issue no writes and so preserve it).

**Witness** (uncounted): `:: PORTSW-1: XUSB2PR mask=0x.. routed 0x..->0x.. +
USB3_PSSEN mask=0x.. 0x..->0x.. (default-on) == witness ::`. The masks + before/after
read-backs are the assertable record: after == `before | mask` confirms the mux
toggled; a smaller value means firmware (Apple EFI) locked some shared-port bits.

**QEMU is inert by design.** QEMU's qemu-xhci (0x1b36) doesn't model Panther-Point
routing, so in the default build the code reads the register block (harmless) and prints
the witness with `mask=0x0` / before == after (no write issued), then storage + MISSION run
unregressed. The real verdict was the attended rMBP bench (below).

**Metal verdict (2026-07-16, attended rMBP sitting, two cold boots):**

- **Boot 1 (knob-OFF default) — externals DROPPED, the decisive outcome.** The kernel
  booted (screen console + scrolling test ran) but every shared USB2 port was dead:
  no FTDI serial, no external keyboard/mouse, storage never enumerated. Apple EFI does
  **not** route the shared ports itself.
- **Boot 2 (knob-ON, cold) — the first-ever cold-boot register capture:**
  `XUSB2PR mask=0xf routed 0x0->0xf + USB3_PSSEN mask=0xf 0x0->0xf` — firmware leaves
  **both at 0x0**. The kernel's write is the only thing putting those ports on xHCI.
  With the flip, the full baseline returned (serial, storage chain + S8-write witness,
  external input).
- **Consequence (pre-registered Maestro policy, now triggered): the routing must be
  default-ON on this platform.** Knob-OFF as a merge default is unsafe — it silences
  serial, storage, and input at once. The default flip is a follow-up fold.
- **Pre-registered bench findings, both answered negative:** the internal keyboard and
  trackpad remained unresponsive with the flip active — they are NOT on the switchable
  mask (consistent with the §7f framing analysis) — and the EHCI-1 scout (§9) read
  **0 connected on every EHCI port** the same boot, so the internals are on neither
  surface as read. The internal-HID line needs the deeper investigation the scout's
  falsification branch pre-registered (both EHCI functions sat halted with
  `CONFIGFLAG=0`, so "asleep until ownership/power setup" remains a candidate
  explanation — analysis at the next arc boundary).
- The M0 keyboard dup-guard held on metal: single `KEY` event per press throughout,
  including from a keyboard hot-plugged behind the hub.

---

## 8. Status and limitations

Implemented: controller bring-up + BIOS→OS handoff, interrupt-driven event
delivery, device enumeration (with connect debounce + bounded, paced retry
recovery, hot-plug re-enumeration after disconnect, and a bounded retry on a
zeroed hub-downstream descriptor read), single-tier USB hubs (HS/FS **and
SuperSpeed** hubs; HID **and mass storage** downstream), BOT mass-storage
read/write, and HID input, plus **hub-downstream hot-plug** (the hub Status Change
Endpoint is configured and serviced — connect enumerates, disconnect tears down the
route-scoped subtree). aarch64 uses a polled variant (no interrupts there yet). See
§7c for the XENUM-1 enumeration-robustness fixes and §7d for XENUM-2 hub hot-plug.
A **default-ON** Panther Point EHCI→xHCI **port switchover** (§7f, opt out with
`UNAOS_NOPORTSW=1`, metal-gated policy) routes the 2012 rMBP shared USB2/USB3 ports onto
xHCI before enumeration — required on that platform, where a no-routing boot drops all
external USB (serial, storage, input).

Not yet implemented: endpoint STALL recovery, multi-tier hubs, and broader class
support. The `skip_xhci` Cargo feature (`UNAOS_SKIP_XHCI=1`) disables USB bring-up
entirely — used on real hardware where firmware may still own the controller, so
the video stack can come up promptly.

---

## 9. EHCI-1 scout — read-only EHCI reconnaissance (EHCI-1)

PORTSW-1 (§7f) established from metal logs that the mask-disciplined USB2 routing write
(`XUSB2PR = 0xf`, which ran unconditionally on every prior rMBP boot) does **not** surface
the internal keyboard/trackpad — they sit outside the 4-port switchable mask, i.e. almost
certainly on **EHCI-only** ports behind the Panther Point companion controllers. Before an
EHCI driver arc is designed, the **EHCI-1 scout** answers what that driver would face,
against real register evidence rather than assumption.

**Strictly read-only (tripwire-grade).** The scout (`drivers/ehci_scout.rs`, x86_64-only)
issues **only** PCI-config reads and MMIO reads off the EHCI BAR — zero writes to any
controller register, PCI config register, or port. It never resets a port, never touches the
BIOS/OS ownership semaphore or `CONFIGFLAG` (both are **read and reported**, never written),
never changes run/stop, never rings a doorbell. A register that cannot be read without a side
effect is skipped and reported as skipped. Each MMIO read is guarded by a page-table
`translate()` check, so a BAR outside the firmware identity map reports honestly instead of
taking a fault.

**What it reports**, per EHCI function (class `0x0C0320`), in a bounded block bracketed by
`:: EHCI-SCOUT: begin ::` … `:: EHCI-SCOUT: end (N controllers, M ports, K connected) ::`:
BDF / vid:pid / BAR0 / IRQ line; PCI power state (PMCSR); the Intel RMH note (Panther Point
EHCI ports sit behind an integrated **rate-matching hub**, so devices enumerate
hub-downstream); the capability registers (`CAPLENGTH`, `HCIVERSION`, `HCSPARAMS` →
N_PORTS/PPC/PRR/companion counts, `HCCPARAMS` → 64-bit-addr + EECP); the EHCI extended-cap
`USBLEGSUP` BIOS/OS-owned semaphore state (read-only); the operational `USBCMD` (RS/run bit),
`USBSTS` (HCHalted), `CONFIGFLAG` (CF: ports routed to EHCI vs companion); and each `PORTSC`
(connect / enabled / reset / power / owner / line state).

**Knob-gated, default-OFF.** Compiled only under the `ehciscout` Cargo feature
(`UNAOS_EHCISCOUT=1`, mapped in both `arroyo` and `builder/src/main.rs`). Knob-off: the module
is unlinked, no probe runs, media byte-identical.

**QEMU target + result.** q35's default device set has no EHCI, so under the knob the builder
attaches a standalone `-device usb-ehci` (harness-only, not a kernel write path). The scout
then reports that controller: `8086:24cd`, 6 ports, `CAPLENGTH=0x20`, `HCCPARAMS` EECP=0x68,
`USBLEGSUP` BIOS=0/OS=0, controller halted (`RS=0`, `HCHalted=1`), `CONFIGFLAG=0`
(ports routed to the companion), all ports powered, **0 connected** — the honest QEMU
baseline (QEMU attaches no downstream device). The real value is the attended rMBP bench: the
`PORTSC` block there shows whether the internals sit connected on EHCI ports and who owns them.

The scout's analysis, pre-registered metal expectations, and the recommended EHCI-driver-arc
shape live in the SCOUT report (`~/.claude/plans/unaos/review/unaos-ehci1-SCOUT.md`).

**Metal result (2026-07-16, attended rMBP sitting): the falsification branch fired.** Both
Panther Point EHCI functions (`8086:1e26`, `8086:1e2d`) were surveyed on silicon: both sat
**halted** (`RS=0`, `HCHalted=1`) with `CONFIGFLAG=0` and **0 connected across all 4 ports** —
the internal keyboard/trackpad are visible on neither the switchable xHCI mask (§7f, flip
active the same boot) nor the EHCI `PORTSC` blocks as read. Per the pre-registered rule, no
EHCI driver arc proceeds on this evidence; the open question for the next investigation is
whether the internals only appear after ownership/`CONFIGFLAG`/port-power setup (both
controllers were asleep, so "not connected" and "not visible while unconfigured" cannot yet
be distinguished), or whether they attach elsewhere entirely (e.g. SPI, as on later models).

---

## See also
- `unaos/crates/kernel/src/drivers/xhci/`, `drivers/block.rs` — the implementation.
- [`scheduler.md`](../02_KERNEL_CORE/scheduler.md) — why the lock-free MSI handler and the main-loop service split matter under a live scheduler.
