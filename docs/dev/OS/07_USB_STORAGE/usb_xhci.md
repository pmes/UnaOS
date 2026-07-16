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

## 8. Status and limitations

Implemented: controller bring-up + BIOS→OS handoff, interrupt-driven event
delivery, device enumeration (with connect debounce + bounded, paced retry
recovery, hot-plug re-enumeration after disconnect, and a bounded retry on a
zeroed hub-downstream descriptor read), single-tier USB hubs (HS/FS **and
SuperSpeed** hubs; HID **and mass storage** downstream), BOT mass-storage
read/write, and HID input. aarch64 uses a polled variant (no interrupts there
yet). See §7c for the XENUM-1 enumeration-robustness fixes.

Not yet implemented: endpoint STALL recovery, multi-tier hubs, and broader class
support. The `skip_xhci` Cargo feature (`UNAOS_SKIP_XHCI=1`) disables USB bring-up
entirely — used on real hardware where firmware may still own the controller, so
the video stack can come up promptly.

---

## See also
- `unaos/crates/kernel/src/drivers/xhci/`, `drivers/block.rs` — the implementation.
- [`scheduler.md`](../02_KERNEL_CORE/scheduler.md) — why the lock-free MSI handler and the main-loop service split matter under a live scheduler.
