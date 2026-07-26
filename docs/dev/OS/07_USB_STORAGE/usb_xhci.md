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
| `dma_coherency.rs` | The single DMA cache-maintenance seam (`clean` / `clean_inval` / `inval`). |

### DMA coherency (XHCI-COHERENCE)

Every xHCI DMA structure (command ring, event ring + ERST, DCBAA, device/input
contexts, scratchpad, transfer rings, transfer buffers) is allocated from the
Write-Back **cacheable** heap. On an I/O-coherent host (x86_64, and the Intel xHCI
on the 2012 rMBP) the CPU caches and the controller's DMA path snoop each other, so
a bare `fence`/`dmb` suffices. On a **non-coherent** bus they do not: the BCM2711
PCIe root complex → VIA VL805 path never snoops the A72 caches (PIUSB-8), and the
Tegra234 XUSB fabric loses its `dma-coherent` handoff at ExitBootServices. There the
CPU must **clean** (write-back) memory it produces before the controller reads it,
and **invalidate** memory the controller produces before the CPU reads it.

`dma_coherency` is the one seam that does this. Its three functions are gated by
`target_arch`, **not** by a board feature — so both aarch64 boards (Pi 4, Jetson)
get maintenance from a single path, while on x86_64 every function is an
`#[inline(always)]` empty body that compiles to nothing (the coherent path is
byte-identical to before the seam existed). Maintenance is applied at each
producer/consumer boundary:

| Structure | Boundary | Op | Site |
| --- | --- | --- | --- |
| Command / transfer ring TRBs | after CPU push, before doorbell | clean | `ring::write_trb` / `push_noop` / `replace_with_noop` |
| Ring zeroed handoff | after `alloc_zeroed`, before controller fetch | clean | `ring::TransferRing::new` |
| Event ring dequeue | before CPU read | clean+inval | `event::EventRing::has_event` |
| Event ring zeroed handoff | before controller writes | clean+inval | `init_interrupter` |
| Input context | before its command | clean | `send_command` / `run_command_sync` (one chokepoint for ADDRESS_DEVICE/CONFIGURE_ENDPOINT/EVALUATE_CONTEXT) |
| Output (device) context | zeroed handoff / before CPU read-back | clean+inval / inval | `address_device`, `address_downstream` / the context builders |
| DCBAA entry | after CPU write, before controller read | clean | `address_device`, `address_downstream`, disable-slot |
| DCBAA[0] + scratchpad array/buffers | before RS=1 | clean | `init_pointers` |
| ERST table | before ERSTBA / RS=1 | clean | `init_interrupter` |
| Control-IN data (descriptors, hub status) | evict before / invalidate after transfer | clean / inval | `sync_control`, `request_device_descriptor`, `request_configuration_descriptor` |
| Interrupt-IN reports (kbd/mouse/hub-change) | evict before arm / invalidate before decode | clean / inval | `queue_*_read` / the transfer-event dispatch |
| BOT CBW / CSW / SCSI data | evict before (both dirs), invalidate IN after | clean / clean+inval / inval | `bot_transfer` |
| FTDI TX staging buffer | before doorbell | clean | `ftdi_tx_stage` |

This **unifies** the old tegra-only `EventRing::has_event_after_invalidate` (its
`dc civac` is now the general aarch64 `has_event` path — identical behavior on
tegra, and the Pi 4 gets it too) and **retires** PIUSB-8's external attach-side
bridge in `arch/aarch64/piusb.rs` (which could only reach the three `pub` ring/ERST
structures; the internal contexts/DCBAA/scratchpad/transfer buffers are now covered
by construction). Stale-DMA of the command/event rings — a controller reading a
stale command ring and a CPU polling a stale event ring — is fixed at the source.

> **Coherency was NOT the sole cause of the Pi 4 enable-slot stall.** Full-driver DMA
> maintenance (this seam) landed at boot-P19 and the VL805's `ENABLE_SLOT` **still**
> stalled — so the non-coherent-DMA hypothesis is *refuted as the sole cause* on
> metal. The real defect was controller-firmware readiness, not cache coherency
> (§1a).

### 1a. VL805 firmware-load ordering (Pi 4) — the CNR wall

Every USB-A port on the Pi 4 hangs off one endpoint: the VIA **VL805** xHCI behind
the BCM2711 PCIe root complex (attach + the 0xdeaddead root-port memory-window fix
are in [`arch_arm64.md` §PI-USB](../01_BOOT_HAL/arch_arm64.md)). Once config and
memory cycles reached the controller, the `ENABLE_SLOT` command stalled through
every coherency and ring hypothesis. The root cause, witnessed on metal:

- **CNR (Controller Not Ready) drops op/runtime-register writes (PIUSB-10,
  `726eb24b`).** At the moment the driver programmed the interrupter / CRCR / DCBAAP
  / ERST and set `RS = 1`, boot-P20's PIUSB-9 witnesses read `USBSTS = 0x811`
  (**CNR = 1**) and **every op/runtime-register write read back 0**. Per xHCI spec
  **§5.4.1 / §4.2**, software must not write any Doorbell, Operational, or Runtime
  register after `HCRST` until `USBSTS.CNR` clears. Intel clears CNR near-instantly
  (x86 never noticed); the VL805 holds it up to ~100s of ms **while it loads its
  internal firmware**, so the Pi silently dropped every register write — the entire
  enable-slot saga was writes into a not-ready controller.

  The fix adds `wait_for_cnr_clear()`: a bounded (`hw_wait_budget`, ~2.5 s) poll of
  `USBSTS.CNR` at the *top* of `init_interrupter` — the first register programming
  after `HCRST`, immediately before ERST/CRCR/DCBAAP/CONFIG/RS. Only the pre-CNR
  halt + `HCRST` reset writes precede the wait (spec-correct). On success it emits
  `xHCI: CNR cleared after N polls`; on timeout it fails **loud** and sets
  `XHCI_CNR_OK = false`, which makes `init_pointers` and `start` skip their register
  writes (loud, no hang) rather than program a not-ready controller. x86 behaviour is
  byte-identical (CNR is always clear there).

- **The mailbox NOTIFY reports hollow success (PIUSB-11, in flight).** With the CNR
  wait in place at boot-P21, `wait_for_cnr_clear` **timed out** — the VL805's CNR
  never cleared, meaning its **internal firmware never booted**. The RPi firmware
  mailbox `NOTIFY_XHCI_RESET` returns `SUCCESS` *without* a running controller
  firmware behind it — a hollow success. **PIUSB-11 is in flight**: NOTIFY-before-reset
  ordering per the Linux VL805 quirk, with the `d03115` board-revision decode still
  pending.

---

### 1b. Post-CNR enumeration witness (PIUSB-13)

Once the CNR wall (§1a) falls, the VL805 root ports must be walked to a live HID
keyboard: port scan → reset → speed → **Enable Slot** → **ADDRESS_DEVICE** (input
context = slot + EP0 sized to the trained speed) → GET_DESCRIPTOR(device, config) →
select the boot-protocol keyboard interface → SET_CONFIGURATION → SET_PROTOCOL(boot)
→ arm the interrupt-IN endpoint → decode boot-keyboard reports (scancode → ASCII).

**None of that machinery is Pi-specific.** The entire FSM lives in the shared
`drivers/xhci` driver (`service_enum` / `service_hid_setproto` / `poll_events`, the
`HID_SCANCODE_TO_ASCII` table, the `xHCI: KEY: '…'` report line) and is driven
verbatim by the Pi's post-heap `piusb::enumerate()` pump — the same code the x86
rMBP and the Jetson run. PIUSB-13 therefore adds **no enumeration control flow**; it
adds an *observer* (`EnumWitness` in `arch/aarch64/piusb.rs`) that snapshots the
driver's read-only state each pump tick (~2 ms cadence) and emits one
`:: PIUSB: [enum] … ::` milestone line per stage transition, so a single metal boot
localizes exactly how far a keyboard got — and, on failure, the stage + completion
code that stopped it. The state the observer reads (`enum_stage_now`,
`enumerating_port_now`, `last_stall_now`, `stall_count_now`, `root_ports_now`, and
the new `DeviceSlot::keyboard_report_count`) is exposed through **aarch64-gated,
read-only accessors** on the shared `XhciController`; the `#[cfg(target_arch =
"aarch64")]` block does not compile on x86, so x86 codegen is byte-identical.

**Gating (inert while CNR is stuck).** `enumerate()` returns at the `XHCI_READY`
gate in QEMU raspi4b (no VL805 modelled — nothing is built), and on a metal boot
where the CNR wall is still up, `RS=1` is dropped, so no port ever connects, no
enum stage ever advances, and no slot is ever assigned. Every edge test in the
observer is then a no-op and it stays **silent** — it speaks only once real
enumeration makes progress. The Pi QEMU battery is unchanged (0 FAIL); the path is
metal-only by construction.

**Expected metal witness sequence** (keyboard in a VL805 root port, CNR cleared):

```
:: PIUSB: [enum] observer armed … ::
:: PIUSB: [enum] port P connect (device attached) ::
:: PIUSB: [enum] port P trained speed High-Speed (xhci speed id 3) ::
:: PIUSB: [enum] port P stage -> enable-slot ::
:: PIUSB: [enum] port P stage -> address-device ::
:: PIUSB: [enum] slot N addressed (root port P) ::
:: PIUSB: [enum] port P stage -> dev-desc ::
:: PIUSB: [enum] port P stage -> cfg-desc ::
:: PIUSB: [enum] port P stage -> set-config ::
:: PIUSB: [enum] slot N HID boot-keyboard armed (interrupt-IN ep 0x81 mps 8, root port P) ::
:: PIUSB: [enum] slot N first keyboard report received — HID pipe live … ::
xHCI: KEY: 'a' (scancode 0x4)
```

A `:: PIUSB: [enum] STALL port P @ stage <S> (<why>, completion code C) PORTSC=0x… ::`
line replaces the milestone that never came: the stage names *where* it stopped
(`enable-slot`, `address-device`, `set-config`, …) and code `C` names *why* (xHCI
completion code — e.g. 4 = USB Transaction Error, 5 = TRB Error, 17 = Parameter
Error, 19 = Context State Error), so one boot localizes the failure.

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

### 2a. 64-bit register access — split 32-bit writes (PIUSB-21)

Every 64-bit xHCI operational/runtime register (`CRCR`, `DCBAAP`, `ERSTBA`,
`ERDP`) is written and read through `write_reg64` / `read_reg64` (mod.rs), which
issue an ordered **pair of 32-bit MMIO accesses** (low dword first, then high) —
the form xHCI 5.1 explicitly permits and Linux uses universally (`lo_hi_writeq`).

This is mandatory on the Pi 4. The VL805 sits behind the brcmstb PCIe root
complex, whose BAR window does **not** carry 8-byte MMIO TLPs: a single AArch64
`str x` (64-bit store) is down-converted and its 32-bit data lane is **replicated
into both dwords**. A `0x02003240` DCBAAP write read back `0x0200324002003240`;
`ERSTBA`/`ERDP` likewise (`0x0015b7800015b780`, `0x0014fa400014fa40`). The
mirrored high dword pushes every controller DMA base above 4 GiB — outside the
`RC_BAR2` inbound window (RAM @ 0, 4 GiB) — so the command ring, ERST, and event
ring all fetch from unmapped addresses: the ring reports `CRR=1` (running) but no
command ever completes and **no event is ever posted** (the PIUSB-20 enable-slot
wall). Splitting into two 32-bit stores delivers the correct low dword and a
true-zero high dword; the controller's DMA bases land inside the window and
completions post.

x86 is unaffected: Intel/AMD root complexes carry native 64-bit MMIO (no
replication), and the two-write form is byte-identical there — no regression, and
the x86 MISSION-SUCCESS storage gate is unchanged. The RS=1 witness now prints
`DCBAAP` reassembled from two 32-bit reads alongside the raw single-load readback
(`DCBAAP_raw64`), so the replication is directly observable on the wire.

### 2b. P31b metal result — enable-slot and HID keyboard enumeration (PIUSB-21 / PIUSB-22)

**P31b METAL RESULT (2026-07-23): the 64-bit-MMIO lo/hi fix CONFIRMED** — enable-slot completed, slots 2/3 addressed, HID boot-keyboard ARMED, first report received, mouse witness up: **FIRST USB INPUT DEVICE ENUMERATED ON PI SILICON.** Residual: interrupt-IN pipe delivers one report then goes silent (no xHCI: KEY lines under typing) — **PIUSB-22 in flight** (re-queue / ERDP EHB write-order under the new lo/hi split / SET_IDLE fork).

### 2c. ERDP write-order under the 32-bit split (XHCI-INT)

The ERDP (Event Ring Dequeue Pointer, IR0 +0x18) is the one 64-bit xHCI register
with a **latch side effect**: its low dword carries EHB (bit 3, Event Handler Busy,
RW1C) and DESI alongside the low pointer bits, and the controller re-evaluates its
event-ring free space — and clears EHB — the instant the low dword is written. The
PIUSB-21 helper `write_reg64` writes **low-then-high**, which is correct for the
write-once init registers (CRCR/DCBAAP/ERSTBA: nothing latches mid-pair, RS=1 latches
them once). Applied to ERDP under the genuine two-store split that PIUSB-21 forces on
the brcmstb RC, that order latches a **torn** pointer — the low write commits the new
low bits and clears EHB while the high dword still holds its previous (often
mirror-garbage, >4 GiB) value. The controller then computes a dequeue pointer with a
stale high dword, decides the event ring is full, and stops posting transfer events:
the interrupt-IN HID pipe delivers **exactly one report, then goes silent** (the P31b
residual). The polled drain papers over it briefly — `has_event` reads the cycle bit
straight from DRAM regardless of EHB — until the controller's producer catches the
stale ERDP and halts.

Fix (`write_erdp`, mod.rs): write the **high dword first, then the low dword** (EHB +
latch) last, so the full 64-bit pointer is in place before the controller latches.
Used at both ERDP sites — `init_interrupter` (initial dequeue = ring base) and
`advance_erdp` (per-event advance with EHB clear). x86 is byte-visible identical (no
RC replication; both stores land, Intel/AMD re-evaluate on the complete pointer either
order) — the MISSION-SUCCESS storage gate is unchanged. Witness `[xhciint] ERDP
initialized to <phys> (hi-first, EHB clear)` at bring-up on both arches; per-event
advances log `[xhciint] ERDP advanced to <phys>` under the xdbg gate. QEMU raspi4b
models no PCIe RC/VL805, so the Pi witness is metal-only; the aarch64 `virt` MISSION
gate exercises the path and carries the witness.

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

### 5a. BOT IN data buffer must be evicted before the doorbell (PIUSB-34)

On P44 metal the Pi 4 read LBA0 of a known-good FAT stick as **all zeros** with a
**passing** CSW (`residue=0`) — repeatably — while READ CAPACITY(10) returned the
correct geometry. Both commands ride the identical `bot_transfer` path against the
same reused `scsi_data_buffer`, so the discriminator was purely transfer length.

Root cause (our code, not the hardware): the BOT **IN** data stage relied *only* on
the post-transfer `inval`, and — unlike every other IN-arming site in the driver —
did **not** clean the data buffer *before* the doorbell. The 512-byte buffer is
`alloc_zeroed` (eight dirty zero cache lines) and reused across SCSI commands; a
short prior IN (READ CAPACITY = 8 B, INQUIRY = 36 B) only ever touches line 0,
leaving lines 1..7 dirty-zero in the A72 D-cache. On the non-coherent BCM2711 PCIe →
VL805 path the controller DMA-writes the block straight to DRAM; a natural
write-back of those stale dirty lines in the window around the DMA clobbers the
just-written DRAM with zeros. READ CAPACITY / INQUIRY escaped because they only span
line 0, which the immediately-prior transfer's `inval` had already dropped.

Fix: `bot_transfer` now cleans the IN data buffer to DRAM *before* the doorbell
(the OUT path already did), so no dirty line survives to lose; the post-transfer
`inval` still drops the clean lines so the CPU parses fresh DRAM. This matches the
convention at every interrupt-IN / control-IN / descriptor arming site. Witness:
`[piusb34] LBA0 re-read post-invalidate: … <first-16 bytes>` (P44 zeros → P45 real
boot sector).

### 5b. DMA-address audit — the inbound-window theory is REFUTED (PIUSB-35)

P45 metal **refuted** the PIUSB-34 cache theory: with the clean-before-doorbell +
post-invalidate in place, READ(10) LBA0 *still* returned all-zero with a Passed /
`residue=0` CSW, while READ CAPACITY(10) still returned correct geometry and the
last-sector RMW candidates (last, −8, −64) all reported `Err(Stall)` (the new stall
recovery works — the pipe survives and enumeration completes). Prime new suspect:
the BCM2711 PCIe RC **inbound window / DMA address** — if the deferred-phase heap sat
above the RC's reachable window (or needed a `dma-ranges` offset we don't apply), the
VL805 would DMA the block into nowhere (stale zeros) while short control transfers on
low buffers still worked.

**Static audit refutes it.** The aarch64 bare-metal kernel heap is placed at physical
`0x0200_0000` (32 MiB), 64 MiB long (`boot::MEM_REGIONS`; `HEAP_SIZE` = 48 MiB), RAM
is identity-mapped (VA==PA in the low 1 GiB Normal block), and `init_heap_raw` hands
out addresses straight from that physical region. Every DMA structure — transfer
rings, DCBAA, the event ring, the CBW buffer (DMA-**read** by the device) and the CSW
buffer (DMA-**written** by the device, returning **Passed**) — comes from the *same*
32–96 MiB pool as `scsi_data_buffer`, which is deep inside the RC inbound window
(RAM@0, 4 GiB, `dma-ranges` 1:1, offset 0; see `piusb::M1` RC_BAR2) and far below the
classic <3 GiB VL805 DMA quirk boundary. A working CSW-write to that pool cannot
coexist with an unreachable data-write to the same pool, and READ CAPACITY reuses the
identical buffer (killing the offset variant too). No address fix is warranted.

Witness (P46, aarch64): `[piusb35] databuf phys=… in_trb=… cbw=… csw=… |
rc-inbound=[0x0,0x100000000) offset=0 | databuf in_window=… below_3G=… — … address
theory REFUTED on-metal …`. When P46 confirms `in_window=true below_3G=true`, the
address theory is dead on-metal and the discriminator returns to transfer
length / TD-shape (READ CAPACITY 8 B works, READ(10) 512 B zeros) or a genuine
device-side cause — *not* the DMA address.

### 5c. PIUSB-36 — the one-boot read-wedge experiment matrix

With the address theory refuted (§5b) and the cache theory refuted (§5a), the Pi-only
symptom is sharp: READ CAPACITY(10) (8 B) and every control transfer DMA correctly to
the same heap pool, yet READ(10) (512 B) returns `Passed`/`residue=0`/**all-zero** on
metal — while the *identical code shape* read real data in the early, pre-SMP,
IRQs-masked bring-up phase (P38) and QEMU virt/x86 are always fine. `service_storage`
runs a self-contained, read-only **experiment matrix** (`piusb36_matrix`, aarch64-only,
byte-identical no-op on x86) that fires six discriminating reads in one boot, each
witnessed with the first 16 bytes plus a pattern verdict:

| Step | Experiment | What it isolates |
| --- | --- | --- |
| 1 | READ(10) LBA0 into the current `scsi_data_buffer` | baseline — confirms the wedge is live this boot |
| 2 | Same, into a FRESH `alloc_zeroed` buffer PRE-FILLED with `0xA5` | `PATTERN-SURVIVED` (device never wrote) vs `ZEROS` (something wrote zeros over the pattern) vs `DATA` — the decisive *never-lands* vs *lands-zeros* split |
| 3 | Same, into a STATIC `.bss` buffer (`PIUSB36_STATIC_BUF`, phys typically <4 MiB) | region dependence (RC inbound-window / cache-color) vs universal |
| 4 | INQUIRY (36 B) into `scsi_data_buffer` | a MID-SIZE control point between 8 B-works and 512 B-fails — where the threshold sits |
| 5 | READ(10) LBA0 as TWO chained TRBs (256 + 256) | TD *shape* vs transfer *length* |
| 6 | READ(10) LBA0, snapshot immediately (A), then wait 1 ms, invalidate the SAME buffer again, re-read (B) | posted-write **visibility**: `A=zeros,B=data` ⇒ the controller's PCIe-posted DMA write was not yet globally visible when the transfer event fired and we invalidated+read |

Two structural facts anchor the analysis. First, the DATA-stage completion is **explicitly
awaited**, matched by the data TRB's own physical address in `run_bot_stage` /
`drain_event_ring_once` (not merely inferred from the CSW stage), so a logical "we read
before the transfer finished" ordering bug is excluded — leaving posted-write *visibility*
(transfer event ≠ DMA globally-visible in DRAM) as the live timing hypothesis step 6
tests. Second, the early phase (P38) ran with IRQs masked pre-SMP, whereas the deferred
phase (`service_storage`) runs on the BSP with IRQs and the preemptive timer scheduler
live; the whole transaction is held under `XHCI_CONTROLLER.lock()` with a polled event
pump (no scheduler yield between doorbell and the invalidate+read), so a context switch
cannot interpose between them — but the two phases differ in bus/interrupt activity, which
is exactly what a posted-write visibility window would be sensitive to.

Witness lines: `:: PIUSB: [piusb36] step<N>-<label> buf=… CSW=… residue=… verdict=… — <16 bytes> ::`
(step 6 prints both the immediate `A` and the `+1ms+inval` `B` snapshots). In QEMU virt
(coherent) all six steps read real data and step 6 reports `no-race-hit`; the P46 metal
run reads the verdicts as the decision tree — step 2 splits never-lands from lands-zeros,
step 6 confirms/refutes the posted-write race, steps 3–5 localize region/threshold/TD-shape.

### 5d. PIUSB-37 — chase the command into the CBW/sense (read-only)

With §5a–§5c pointing away from our transport, `piusb37_matrix` (aarch64-only, read-only)
corners the residual on the SCSI-command side in one boot: (1) a **CBW audit** — build the
exact 31-byte CBW and spec-check every field + decode the READ(10) CDB (a wrong LUN,
byte-swapped LBA, or zero blocks each produce the same zeros+`Passed` signature); (2) a
command-set / known-nonzero-LBA matrix (READ(10) of LBA 8192/16384, plus READ(12)/READ(16)
of LBA0); (3) **REQUEST SENSE** immediately after a zeros-read (a pending UNIT ATTENTION,
key 0x06, is the bridge-returns-zeros-then-GOOD candidate); (4) a **TUR drain + retry**.
The P47 capture read the CBW as **byte-perfect** (`command is NOT the fault`) and LBA
8192/16384 returned real non-zero data with `residue 0` — proving the transport/DMA/BOT are
sound and the wedge is confined to the low-LBA region — while READ(12)/READ(16) STALLed
(unsupported by the bridge, expected). Witness: `:: PIUSB: [piusb37] … ::`.

### 5e. PIUSB-38 — BOT stall recovery + event-ring resilience + low-LBA bisect

The P47 boot exposed the real defect the §5d probes uncovered: after the READ(12)/READ(16)
STALLs, **every** later command on the storage slot — REQUEST SENSE, TEST UNIT READY (×8),
the retry READ(10) — **timed out**. The bulk pipe halted and never recovered: our stack ran
a per-endpoint clear-stall but never completed BOT **Reset Recovery**, so the device and host
BOT state machines stayed out of phase and the storage slot's transfer events stopped. (HID
recovered independently via a hub re-enumeration in the same boot, so the interrupter was
**not** globally wedged — the shared single-owner event drain kept advancing ERDP and
clearing `IMAN.IP` for HID; only the storage slot's *transfer* path was dead.)

**Stall recovery (`bot_transfer` + `recover_bot_full`).** On a bulk STALL/Babble the driver
now follows the USB Mass-Storage Bulk-Only Reset-Recovery contract:

- **Data-phase stall (§6.7.2):** clear the halt on the bulk pipe (`recover_bulk_stall`:
  Reset Endpoint → Set TR Dequeue → device CLEAR_FEATURE(ENDPOINT_HALT)), then **still
  collect the CSW** so both state machines resync — the device stalled the data, not the
  command, so it is in its status phase and returns a `Failed` CSW. Skipping this CSW was the
  P47 wedge.
- **Status-phase stall, or a data-phase stall whose CSW also fails (§6.7.3 / §5.3.4):**
  escalate to **full Reset Recovery** (`recover_bot_full`): a **Bulk-Only Mass Storage Reset**
  (class request `bmRequestType 0x21`, `bRequest 0xFF`, `wIndex =` the captured MSC
  `bInterfaceNumber` — `DeviceSlot::storage_intf`), then Reset-Endpoint + Set-TR-Dequeue +
  CLEAR_FEATURE(ENDPOINT_HALT) on **both** bulk endpoints. Host and device toggles/ring
  dequeues realign, so the next CBW starts clean.

**Event-ring resilience.** Error/stall transfer TRBs are dispatched through the same single
`drain_event_ring_once` owner as every other event, which advances the ERDP and clears
`IMAN.IP` **unconditionally** after each `handle_event_trb` — so one bad pipe can never starve
the interrupter, and recovery (which itself pumps the event ring via the synchronous command /
control paths) lets unrelated HID `xHCI: KEY` events keep draining during an induced storage
stall.

**Low-LBA bisect.** `piusb38_matrix` (aarch64-only, read-only) then proves recovery and
localizes the §5c/§5d wedge in one boot: it (1) **induces** a stall (READ(16)) and confirms
TEST UNIT READY + REQUEST SENSE **complete** afterwards (`PIPE RECOVERED`), (2) exercises
`recover_bot_full` explicitly and re-proves TUR, and (3) reads a ladder **LBA 0,1,2,4,…,8192**
(same READ(10) CDB shape, only the LBA field varies) with a per-LBA zeros/data verdict, then
reports the zeros→data **boundary** and the first byte at which LBA0 and LBA8192 differ.
Because only the LBA field changes, any zeros-vs-data split is **region-specific, not a
command-shape fault** (null-hypothesis-our-code: a buffer/cache/aliasing effect on the low
region). Witness: `:: PIUSB: [piusb38] … ::`. Inert on QEMU raspi4b (no VL805 → no storage
slot); QEMU virt exercises the whole path (the full-reset TUR passes and the ladder reads real
LBA0 data), byte-identical no-op on x86.

### 5f. USBW-1 — the write self-test addressed the wrong disk (P57)

P57 ended the MISSION write proof with

```
:: PIUSB: [usbw] write lba=500223999 -> FAIL (pre-read all candidates stalled) ::
```

Both halves of that line were wrong, and the cause was **ours**, not the reader's.

**The geometry mix-up.** `mission_write_selftest` picked its scratch sector from the *global*
`block::info()`. On the Pi that global belongs to the **microSD**: the BSP registers the eMMC2
card at probe time (`:: M6g: SD card @0xfe340000 identified — 500224000 blocks (244250 MiB) ::`),
and PIUSB-28 deliberately keeps `BLOCK_DEVICE` pinned to the SD so a later-enumerated stick
cannot clobber it. The enumerated USB device is a different disk entirely — its own READ
CAPACITY, printed two lines earlier in the same boot, is
`Disk 'Generic' 'USB SD Reader' block_size=512 num_blocks=29120 (14 MiB)`. The probe therefore
aimed `lba = 500224000 - 1` — the SD's last block — down the **USB BOT pipe**, roughly 17,000×
past the reader's last LBA of 29119. All three fallback candidates (last, last-8, last-64) were
equally out of range.

**The device was right.** Each candidate came back `Ok(BotResult { status: Failed, residue: 512 })`
— a *completed* BOT round trip whose CSW reports CHECK CONDITION. The reader halted the data-IN,
our §5e stall recovery cleared the halt and still collected the CSW exactly as designed (the
`[usbw] bulk STALL recovery slot 2 ep 0x82` lines preceding each verdict), which is precisely why
the next candidate got a clean pipe and its own CSW. Recovery **engaged and worked**; the failure
label "all candidates stalled" described a transport fault that never happened. **Not** a
device quirk, and unrelated to the separate s1j finding that this card's front sectors read as
zeros.

**Fix, part 1 — the right disk.** `mission_write_selftest` now sources its geometry from
`block::usb_info()` — the dedicated `USB_BLOCK_DEVICE` handle published by the very enumeration
that owns this BOT pipe — so the scratch LBA is always bounded by the stick's own capacity. The
PIUSB-25 "storage enumerated" witness had the same defect (it read `BLOCK_DEVICE`, so it reported
the SD's 500224000 blocks under a USB label, contradicting the READ CAPACITY line just above it)
and now reads `USB_BLOCK_DEVICE` too. On x86 `publish_usb_geometry` writes both handles, so **the
geometry source is byte-identical there**; the serial output does gain the new witness lines
below.

**Fix, part 2 — the right *sector*, which part 1 alone got wrong.** Correcting the disk made the
scratch write *reachable* for the first time, and it would have landed **inside the live `/fs/usb`
volume**. The bench reader's card is a **superfloppy**: PIUSB-25 reports sector 0 as
unrecognized/raw, and `mount_source` succeeds off a BPB at LBA 0 with partition offset 0 — so the
volume's LBA space **is** the raw medium's. A whole-card FAT16 therefore runs to sector 29119 and
all three candidates (29119, 29112, 29056) sit within it, the latter two plausibly in the data
region (1024 B clusters, and `UNAOS.LOG` grows on that volume every boot, so tail clusters are not
free by construction). A failed `restore_sector` or a power loss mid-RMW would leave 512 bytes of
XOR pattern in a live file.

"Near the end of the medium" is **not** the same as "clear of the filesystem", and the old
USB-WRITE-2 comment ("NEVER near low (filesystem) LBAs") encoded exactly that false equivalence.
The real rule is **above the volume**, not top-of-medium. `usbw_keepout_ceiling` now parses sector
0 and returns the first LBA provably outside any on-disk container, fail-closed:

| sector 0 | ceiling |
|---|---|
| GPT protective MBR (type 0xEE) | whole medium — the **backup GPT header occupies the last LBA**, making top-of-medium the worst possible scratch |
| MBR partition table | highest `start + size` over the valid primary entries |
| FAT BPB at LBA 0 (superfloppy) | `BPB_TotSec16`, else `BPB_TotSec32` |
| raw / no recognizable container | 0 |
| unreadable or signed-but-unrecognized | whole medium (skip) |

Candidates below the ceiling are discarded; when the container **spans the medium** there is no
safe sector and the probe **skips outright** with
`[usbw] scratch skipped: on-disk container spans the medium (…)` rather than falling back to
writing inside a mounted volume. On the bench reader that is the expected outcome: a whole-card
FAT16 superfloppy leaves nowhere to scratch, so the metal write proof is **skipped, not faked**.
Exercising it on metal needs a medium with slack above the volume (or a dedicated scratch stick) —
flagged for the integrator.

The keep-out ceiling is witnessed on every boot as part of the geometry line
(`[usbw] scratch geometry: USB last_lba=… (num_blocks=…), keep-out ceiling=… [provenance]`), and
every skip path — including an unpublished USB handle, which used to `return` silently — now emits
a `scratch skipped:` line, so the write proof can never vanish traceless. The
all-candidates-exhausted verdict no longer claims "stalled"; it points at the per-candidate CSWs
above it.

The genuine end-of-medium fallback ladder from USB-WRITE-2 (P44: some sticks STALL a READ(10)
against the very last LBA they report) is retained — applied to the right disk, and now clamped to
the keep-out ceiling.

On the default QEMU media (`builder/usb.img`, a raw pattern image with no container in sector 0)
the ceiling is 0 and the proof runs at the last LBA exactly as before.

---

### 5g. PIUSB-40 — stage timing on the Pi 4 COLD-BUILD path, and the ladders that were bounded

**The observation (metal P59b, two data points).** When the Pi 4 firmware hands the kernel a PCIe
root complex still held in reset —

```
fw RC: RGR1_SW_INIT_1=0x00000003 PCIE_STATUS=0x00000000 (PHYLINKUP=false DL_ACTIVE=false)
PIUSB-16: ENTRY link state ... -> VC left RC in reset — COLD-BUILD path (M1 reset → M2 → M3)
```

— boot **visibly pauses for roughly three minutes** (operator-timed). When the firmware leaves the
link up (the ADOPT path), boot is fast. Whether the RC comes up in reset varies boot to boot; the
cold-vs-warm power pattern behind it is not yet characterized.

**Why a stopwatch could not localize it.** The COLD-BUILD path is a serial chain of stages, several
of which emit no serial output between entry and exit. An operator can time the whole chain but not
attribute it. The instrumentation below closes that gap.

#### The stage-timing instrument

Every bring-up stage in `arch/aarch64/piusb.rs` is bracketed off `CNTVCT` (the free-running generic
counter — always live, no init, no interrupt dependency) and emits exactly one line:

```
:: PIUSB: [piusb40] stage=<name> took=<ms>ms (t=<ms>ms) ::
```

| Stage | What it covers | Budget on the wire |
| --- | --- | --- |
| `census-power-clock` | the `[piusb32]` mailbox power/clock census | 3 mailbox calls, 500 ms each on timeout |
| `fw-state-dump` | PIUSB-5 read-only pre-reset dump | RC-own reads; CAP ladder only when link-up |
| `entry-link-discriminator` | the PIUSB-16 entry PCIE_STATUS/RGR1 reads | 2 RC-own reads |
| `dump-linkdown-rc-claim` | the link-down fall-through's `witness_rc_cpu_claim` | RC-own MISC/BAR2 reads |
| `dump-linkdown-serror-drain` | the fall-through's SError drain window | no loop; bounded by construction |
| `m1-rc-bringup` | whole M1 (bridge reset → windows → PERST → link) | sum of the two below plus ~2 ms of pulses |
| `m1-perst-settle` | the mandated post-PERST-deassert wait | fixed 100 ms (Linux `PCIE_RESET_CONFIG_WAIT_MS`) |
| `m1-link-train-poll` | PHYLINKUP+DL_ACTIVE poll | 100 ms backstop |
| `m2-enumerate-vl805` | whole M2 (bus/mem windows, BAR sizing, NOTIFY) | ~210 ms of settles + mailbox |
| `m2-mmio-verify` | pre-NOTIFY CAP[0] ladder over the outbound window | see ladder cap below |
| `m3-attach-xhci` | whole M3 | sum of the three below |
| `m3-cap-probe` | CAP[0] ladder at the assigned BAR | see ladder cap below |
| `m3-notify` | pre-HCRST NOTIFY + decode re-assert | mailbox + ~10 ms |
| `m3-xhci-handoff` | `xhci::init` — halt, HCRST, CNR | **3 × `hw_wait_budget()` ≈ 8.3 s worst** |
| `enum-notify` / `enum-xhci-handoff` | the deferred half's reload + second handoff | as above |
| `enum-rings-rs1` | rings, interrupter, RS=1 | CNR wait ≈ 2.8 s worst |
| `enum-pump` | the bounded polled enumeration walk | 30 s backstop, early exit on armed/ready |
| `bringup-total` / `enum-total` | the two halves end to end | — |

The instrument is always-on **within the `piusb` knob** and costs one counter read plus one serial
line per stage. It performs **no MMIO of its own**, so it is safe at every point it is placed. Every
`[piusb40]` line sits past a gate QEMU `raspi4b` never crosses (`bringup_inner` returns at the
`pcie@` DTB census; `enumerate` returns at the `XHCI_READY` gate), so a QEMU boot — knob on or off —
emits **zero** `[piusb40]` lines and a byte-identical log.

#### What was trimmed: the cost-blind poison-retry ladders

Three places read the **outbound memory window** in a fixed retry loop shaped
`while tries < N { settle_ms(5); read; }` — the PIUSB-5 fw-CAP probe (4 tries), the PIUSB-15
pre-NOTIFY MMIO verify (8), and the M3 CAP probe (8). Each was *budgeted as if a try cost its
settle*, i.e. "8 tries ≈ 40 ms".

That accounting is wrong on the COLD-BUILD path. These are **memory** cycles into the RC's outbound
window, not RC-own APB reads. A memory cycle the RC cannot forward is not answered — it is absorbed
by the bridge's completion-timeout machinery, and this is the same mechanism already documented in
this subsystem as stalling the CPU on the bus "for a pathologically long time" (the boot-P4 lesson
that put the link-down MMIO gate in). A ladder that reads as a 40 ms budget can therefore spend
**minutes**, serially, printing nothing between tries. Two such ladders back to back are the leading
explanation for the three minutes, and the `m2-mmio-verify` / `m3-cap-probe` lines now measure it
directly.

All three now route through one helper, `mmio_settle_read`, bounded three ways — and it matters which
one is load-bearing:

1. the original try count (unchanged — a healthy decode still completes identically);
2. **the safety bound**: a total wall-clock cap, `MMIO_LADDER_BUDGET_MS = 60`. This is what actually
   bounds the ladder. It is ≈12× the 5 ms inter-try settle these ladders were written around, so it
   never bites on a link that answers at all;
3. **a diagnostic label**, not a second bound: a per-read cost check, `MMIO_ABORT_COST_MS = 20`. The
   ladder measures each read and, when one exceeds it, stops early and *names why*:

   ```
   [piusb40] mmio-ladder @ 0x…: try N took …ms (>= 20ms) — consistent with the RC absorbing this read
   as a completion timeout rather than answering it; … Ladder STOPPED at …ms
   ```

The honest reading of (3): a read that expensive is *consistent with* a master-abort being absorbed,
and continuing would buy the same abort at the same price — but the check is there to **explain** the
time, not to be the thing that limits it. Strip it out and the wall budget still holds. 20 ms is far
above any honest Device-nGnRnE read (sub-microsecond off a decoding BAR) and far below the
multi-second stalls metal has shown, so it separates the cases without cutting a real answer short.
Because `MRS CNTPCT_EL0` is not ordered against a Device load, the measured load is `dsb sy`-bracketed
on both sides — without that the cost can under-report an absorbed read and the diagnostic silently
never fires (the wall budget would still catch it, but the diagnostic is the point of the arc).

Both terminal arms log. The exhaustion arm distinguishes "wall budget reached" from "try count reached
within budget", and the M2/M3 fail-closed messages carry the ladder's elapsed ms and whether it was cut
on per-read cost. The callers' fail-closed branches are unchanged — the verdict just arrives sooner.

**The residual is not zero.** The cost check fires *after* the offending read has already been paid,
and there are three ladders on the cold-build path, so the trimmed path still absorbs on the order of
**~3× one completion timeout** (plus the 60 ms caps) rather than the ~8+8+4 aborts the fixed ladders
could pay. If one completion timeout is itself seconds, the pause shrinks substantially but does not
vanish — and the true per-abort cost is exactly what the `took=` figure in the `mmio-ladder` line will
report for the first time. That residual is on the metal watch-list below, not claimed as fixed.

**What was deliberately NOT trimmed.**

- The **100 ms post-PERST-deassert settle** (`m1-perst-settle`). Mandated by the PCIe CEM T_PVPERL /
  device-ready window and by Linux `brcm_pcie_start_link`; PIUSB-17 exists precisely because
  skipping it produced the CNR wall. Bracketed only, so the log accounts for it honestly.
- The **100 ms PCIe config-request window** in M2 and the SCB/CNR settles from PIUSB-16/17/18.
- The **`xhci::init` timeouts** (`m3-xhci-handoff`, ~8.3 s worst on a firmwareless controller, paid
  **twice** — once early, once in the deferred half). These come from `hw_wait_budget()`
  (150e6 CNTVCT ticks ≈ 2.8 s at the Pi's 54 MHz) in `drivers/xhci` and `arch/aarch64/mod.rs` —
  **shared kernel-core, outside the Pi track's lane**. This arc measures the stage instead of
  re-cutting it, so the owning lane gets evidence before a shared timeout is touched.

#### Wedge-path audit (the P59a full boot wedge)

An earlier same-day boot did not merely pause — the wire went dead immediately after

```
fw left RC in reset — nothing to adopt/probe (…); skipping child-config + CAP probe (…)
```

and the machine was power-cycled. The audit question was whether that fall-through performs an MMIO
the skip was supposed to avoid.

**Verdict: it does not.** Everything after that line touches only the RC's **own** register block —
`witness_rc_cpu_claim` reads `MISC_CTRL` and `RC_BAR2_CONFIG_LO/HI`, the same MISC block whose ten
reads immediately above already answered (bracketed by the `piusb31` enter/exit pair, which printed).
No child config, no outbound-window memory cycle, no mailbox. The wedge is **not** a mis-skipped
downstream MMIO on this path, and no new guard is warranted there.

What remains unproven is *which* of three silent candidates ate the boot: the MISC_CTRL/RC_BAR2
reads, the SError drain window, or the caller's next stage (M1). None of them printed anything, which
is why P59a's log could not separate them. The three brackets `dump-linkdown-rc-claim`,
`dump-linkdown-serror-drain`, `entry-link-discriminator` and `m1-rc-bringup` now make the next boot
name it: **whichever `[piusb40]` line is missing is the stage that wedged.** The bracket chain is
gap-free from the census through the end of M3, so a wedge cannot be misattributed to a neighbouring
stage — the PIUSB-16 entry reads in particular used to sit in an unbracketed gap that would have
pointed the finger at M1.

#### Metal watch-list

- Which `[piusb40]` stage carries the bulk of the pause — the expectation is
  `m2-mmio-verify` + `m3-cap-probe`, and a `mmio-ladder … STOPPED` line would confirm the
  completion-timeout mechanism outright.
- `m3-xhci-handoff` and `enum-xhci-handoff`: if these show ~2.8 s multiples, the shared
  `hw_wait_budget` is a real share of the pause and the finding goes to the owning lane.
- On the wedge path: which of the three link-down brackets fails to print.
- Whether the ladder cap ever fires on a boot that *would* have succeeded (it should not — the
  budget is ≫ the healthy path's cost; a `STOPPED` line on an otherwise-good boot would mean
  `MMIO_ABORT_COST_MS` is too tight).
- **The residual.** The trim does not take the cold-build path to zero: the cost check fires only
  after the offending read is paid, so ~3× one completion timeout (one per ladder) plus the caps
  remains. The `took=…ms` figure in the first `mmio-ladder` line is the first real measurement of what
  one absorbed read costs on this silicon — it turns the remaining pause from an estimate into a
  number, and decides whether a further arc is warranted.
- Whether the `exhausted … WALL BUDGET reached` arm appears instead of the cost-cut arm: that would
  mean the aborts are individually cheap but numerous, a different shape of problem than the one this
  arc reasoned from.

### 5h. PIUSB-41/42 — the P60 verdict: the pause was our own diagnostic dump

**The measurement.** P60 is the first real cold-boot run of the 5g bracket chain, and it settles the
watch-list above. Total PCIe/VL805 bring-up: **141.8 s**. The stage table:

| Stage | `took=` | Reading |
| --- | --- | --- |
| `fw-state-dump` | **129 688 ms** | the pause |
| `dump-linkdown-rc-claim` | **32 410 ms** | nested inside the above |
| `m1-rc-bringup` | 340 ms | functional, fast |
| `m2-enumerate-vl805` | 564 ms | functional, fast |
| `m3-attach-xhci` | 332 ms | functional, fast |
| `m3-xhci-handoff` / `enum-xhci-handoff` | 18 ms / 21 ms | no 2.8 s multiple |

Three prior hypotheses die on this table. The `m2-mmio-verify` + `m3-cap-probe` ladders are **not**
where the time goes — M2 and M3 together are under a second. The shared `hw_wait_budget()` escalation
is **refuted**: the handoffs cost tens of milliseconds, not 2.8 s multiples, so nothing needs to go to
the owning lane. And the pause is not in bring-up at all: **every functional stage is fast.** The
carriers are both *our own read-only diagnostic dump* on the link-down path.

**The mechanism.** `dump_firmware_state`'s link-down path performs **13 bare RC-APB reads** — ten in
the register dump plus three in `witness_rc_cpu_claim`. 129.7 s / 13 ≈ **10 s per read**, and the
nested figure agrees independently: 32.41 s / 3 ≈ 10.8 s per read. So when the firmware hands us
`RGR1_SW_INIT_1=0x3` (INIT_GENERIC|PERST — the RC held in reset, `PHYLINKUP=false`), the RC's **own**
APB block does not answer promptly either: each 32-bit load is absorbed by the bridge's
completion-timeout machinery and stalls the CPU ~10 s. This is the boot-P4 mechanism, but applied to
the RC register block rather than to downstream/outbound cycles — the link-down gate never claimed to
protect against it, because RC-own reads had always been assumed safe *and* cheap. They are safe. They
are not cheap.

Two consequences follow. First, `MMIO_LADDER_BUDGET_MS` does **not** apply here: these are bare `r()`
loads, and the 60 ms wall only caps `mmio_settle_read` ladders. Second — the important one — the cost
is **linear in the number of registers we choose to dump**. The pause was never a hardware budget; it
was a per-register price multiplied by a diagnostic we had no reason to keep paying every boot.

**The fix (default-quiet law).** The dump's family is confirmed: on the fw-left-RC-in-reset path it
has only ever reported the firmware's teardown, and M1 reprograms every register it reads moments
later, proving the result with its own readback line. So:

- the eight extra RC-own register reads (bus window, WIN0 ×5, RC_BAR2 ×2) and the whole
  `witness_rc_cpu_claim` witness now require the **`usbdebug`** feature (`UNAOS_USBDEBUG=1`);
- the default path reads only the **two** registers the gate genuinely needs — `RGR1_SW_INIT_1` and
  `PCIE_MISC_PCIE_STATUS`, which drive both the link-down gate and PIUSB-16's discriminator — and
  prints one summary line naming the mode and the knob;
- the skipped witness prints a one-line placeholder saying what it would have cost and how to get it.

Expected cold-boot effect: the link-down path's **13 slow reads become 2**, so `fw-state-dump` should
fall from 129 688 ms to **~20 s** (2 × ~10 s). The per-read price is self-consistent inside P60 and is
not a single division: the dump's *own* ten reads cost 129.688 − 32.410 = **97.3 s ≈ 9.7 s each**, and
the nested witness's three cost **32.41 / 3 ≈ 10.8 s each**.

**What did not change.** No bring-up step, mandated settle, ordering, or protection is touched; the
link-down gate is the same gate reading the same two registers; the fail-closed branches are
unchanged. Every `[piusb40]` bracket still emits `stage=… took=…`, so the 5g wedge-localization
property survives intact — and on the skipped path a near-zero `dump-linkdown-rc-claim took=` is now
itself the evidence that the stage was *skipped* rather than *wedged*.

**Composed with PIUSB-42 — the discriminator stops paying twice.** PIUSB-41 left one residual: PIUSB-16's
`entry-link-discriminator` re-read the same two registers microseconds after the dump read them, with no
intervening write. That is now fixed in the same change. `dump_firmware_state` **returns**
`(RGR1_SW_INIT_1, PCIE_STATUS)` — the pair it reads unconditionally, above the `deep` gate, on every path
including both early returns — and the discriminator consumes it instead of re-reading. The values are
seconds stale by then, and that is sound by construction rather than by hope: the dump is read-only
("BEFORE any RC reset" is its whole contract), the only writer of `RGR1_SW_INIT_1` here is M1, which runs
strictly *after* the discriminator and gated *on* its verdict, and `PCIE_STATUS` only moves on a link event
M1 has not yet triggered. Same gate, same verdict, same PIUSB-16 witness line with the same values.

**How much that second fix is worth is bounded, not extrapolated — and the two numbers disagree.** A
uniform 2 × ~10 s per-read extrapolation predicts ~21 s, but P60's own total does not leave room for it:
141.8 s − 129.688 s (dump) − 1.236 s (m1 340 + m2 564 + m3 332) = **10.9 s for `census-power-clock` *plus*
the discriminator combined**. So the duplicated pair cost **at most ~10.9 s**, and ~21 s must not be
quoted as measured. P60's table does not cite the stage's own `took=` — the stage *is* bracketed (5g added
it), the figure simply was not carried across — so the split between the census and the discriminator is
unresolved in the evidence we have. Consequently the expected composed cold-boot total is a **band, ~21 s
to ~32 s** (down from 141.8 s), its width being exactly that unresolved split. The next cold boot settles
it directly: the bracket stays, so `entry-link-discriminator took=` collapsing to ~0 while
`census-power-clock` shows its true cost separates the two for good.

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

**PIUSB-39 — the guard must discard the data, never the pipeline.** As first written the
guard's only exit was `return`, which skipped the `queue_mouse_read` re-arm. A *single*
mismatching completion therefore retired the pointer interrupt-IN endpoint permanently,
while the keyboard's independently-armed endpoint carried on — exactly the P54b metal
fact (after an EL0 app's interactive takeover ends, the mouse is dead and the keyboard
still types). The guard stays (the dup hazard is real); its exit now discriminates using
`mouse_prev_phys`, the address of the TD `queue_mouse_read` last retired:

- `param == mouse_prev_phys` — a genuine Panther-Point dup for the already-consumed TD.
  A fresh read is *already* armed, so the dup is discarded **without** re-arming (that is
  the original UI1-MOUSE M2 protection, preserved unchanged).
- any other mismatch — the endpoint retired a TD we cannot account for, so nothing is
  guaranteed armed. The report is discarded and the read is **re-armed**.

The same two dead-pipeline holes existed on the *error* side: a completion code other
than 1/13 on either HID interrupt-IN endpoint fell through the success gate without a
re-arm. Both now trace and recover, and the recovery is **split by completion code**,
because a bare re-queue is a no-op on a halted endpoint (it ignores the doorbell until
Reset Endpoint):

- **Halting codes** — 2 (Data Buffer Error), 3 (Babble), 4 (USB Transaction Error),
  5 (TRB Error), 6 (Stall). The endpoint is Halted, so `(slot, is_mouse)` is queued on
  `hid_halt_pending` and the main-loop `service_hid_halts` runs the full un-halt:
  **Reset Endpoint** (TRB 14) → **Set TR Dequeue Pointer** (TRB 16, past the faulted
  TRB) → device `CLEAR_FEATURE(ENDPOINT_HALT)` → clear the stale `*_expect_phys`/
  `*_prev_phys` (the ring dequeue moved) → arm the read. This is the same pair the bulk
  clear-halt path uses (`reset_bulk_endpoint_host`, §7g), generalised over any DCI. It is
  deferred to the main loop for the same reason `SET_PROTOCOL(boot)` is: the sequence is
  synchronous command + EP0 traffic and must never run re-entrantly inside the event-ring
  dispatch that noticed the error. A slot that unplugged in between is skipped
  (root `PORTSC.CCS`), and slot teardown drops its queued entries.
- **Non-halting codes** — the endpoint is still Running; the read is re-armed inline, as
  the hub Status Change Endpoint does (§7d).

The keyboard guard carries the identical fix (`keyboard_prev_phys`); only its lower
traffic kept the defect from being observed on metal.

*Witness (knob-gated, `usbdebug`, rate-limited to one line per 250 ms).*
`[piusb39] mouse rearm=<n> discarded=<n> errrearm=<n> (<tag>)` — the three counters are
distinct populations and the tag names which one moved: `poll` (a normal armed read;
printed on the first arm and every 256th), `guard` (the dup-Success guard discarded a
completion and re-armed anyway), `halt` (a halted endpoint was un-halted and re-armed).
`discarded > 0` on a metal capture is direct proof the *guard's* pipeline-preserving exit
fired where the old code would have killed the pointer; `errrearm > 0` proves the *error*
path recovered. The error trace itself (`xHCI: pointer interrupt-IN error ...`) is
unconditional but capped at one line per 500 ms with a `[+N suppressed]` tail — the
pointer is the highest-rate endpoint and a Running-state error class self-sustains at
report rate.

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
the summary trace asserts the scope. After servicing (connect or disconnect or a no-op
change), **every set change bit in `wPortChange`** is acknowledged with `ClearPortFeature`
so the endpoint deasserts and can report the next change — see §7h for the full-word ack.

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
downstream-enumeration robustness. **Both addressed in §7g (XENUM-3), metal-pending.**

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

### 7g. Hub-downstream enumeration robustness (XENUM-3)

Two additive fixes to the behind-a-hub enumeration path (`enumerate_downstream` /
`address_downstream`), closing the two gaps the §7d "New metal findings" characterized.
Both are **METAL-PENDING by construction**: QEMU never posts a zeroed descriptor or a
`code 17`, so the QEMU gates prove only *no regression* — the fixes exercise on the metal
rMBP behind the VIA hub.

**M1 — descriptor-content + short-read validation.** The old device-descriptor gate
accepted any read with `bLength >= 18 && bDescriptorType == 0x01`, so the metal case — a
mouse whose descriptor came back structurally valid but **zeroed** (`vid=0000 pid=0000`,
"no HID interrupt endpoint") — passed and the paced retry never fired. Two additions:

- *Actual transferred length.* `sync_control` now sets IOC on the DATA-stage TRB so the
  controller posts a data transfer event carrying the TRB Transfer Length residual; the
  sync EP0 pump claims and consumes that event (it never reaches the async FSM) and
  records the transferred byte count in `last_control_len`. A read shorter than the
  requested 18 bytes is now detectable.
- *Bad-read predicate.* The retry loop treats a read as BAD (→ the existing bounded paced
  retry, then leave-unconfigured + slot dispose) when it errored, `last_control_len < 18`,
  the structural header is wrong, **or** the header is valid but `vid==0 && pid==0`. Trace:
  `downstream slot N device-descriptor bad read (got G of 18, bLength=… type=… vid=… pid=…, attempt A of 4)`.
- *MPS0-learn for FS/LS behind a HS hub.* A Full-Speed device's real `bMaxPacketSize0`
  can be 8/16/32, not the 64 `address_downstream` guesses — so the full read short-reads
  and strands with zeroed content. Mirroring the root path's MPS0-learn idiom, the
  downstream path now reads the 8-byte header first, and if the device's real MPS0 differs
  from what was programmed, re-issues ADDRESS_DEVICE with the learned value before the full
  read. Trace: `downstream slot N MPS0 learned M (programmed P); re-addressing.` (This one
  *does* fire in QEMU: the hubbed FS storage device reports MPS0=8 and is re-addressed
  before its descriptor read — the HUBSTORAGE gate exercises the re-address code path.)
  ⚠ Metal watch item: the re-address issues ADDRESS_DEVICE BSR=0 on an *already-Addressed*
  slot — QEMU accepts it, but it is metal-unverified; the sitting should watch for a
  code-17 cluster on the `re-addressing` line.
- *Review fold (dup-Success guard).* The DATA-stage residual consumer matches the event's
  TRB pointer against the recorded DATA TRB phys **and** first-write latches the residual:
  Panther Point's `XHCI_SPURIOUS_SUCCESS` quirk (device 0x1e31 — this machine's controller)
  can post a duplicate Success after a Short Packet for the same TD, and an unguarded
  consumer would let the dup overwrite a real short-read residual with 0, masking the
  exact strand M1 catches. QEMU posts no dup — the latch itself is metal-pending.

**M2 — downstream ADDRESS_DEVICE bounded paced retry.** `address_downstream` gave up on
the first non-success completion (metal: `code 17` = Context State Error behind the VIA
hub), where root ports have 200/400/600 ms paced recovery. It now retries the same input
context up to `XENUM_ADDR_RETRIES` (3) times with an escalating settle between attempts
(no port re-reset — a Context State Error is a controller-side transient, not a link
fault). Trace per attempt: `downstream ADDRESS_DEVICE code C (attempt A of N)` and, on
exhaustion, `downstream ADDRESS_DEVICE failed after N attempts`.

**Honest slot cleanup.** A downstream slot that was ENABLE_SLOT'd but never brought to a
usable device (failed address, descriptor never valid) previously stayed `active=true`
with contexts allocated and a live DCBAA pointer — a leaked active entry. `enumerate_
downstream` now calls `dispose_downstream_slot` on those bail paths, mirroring the
root-port recovery clean-up (soft-state reset + deferred DISABLE_SLOT; rings/contexts
leaked-not-freed until the controller releases them). Trace:
`downstream slot N disposed (unenumerated); queued for DISABLE_SLOT.` (A genuinely
addressed device of an unsupported class is left as-is — it is a real device, not a
failed address.)

QEMU gates (no-regression): `UNAOS_HUBSTORAGE=1 test 60` → MISSION SUCCESS + full U-arc
chain off the hubbed disk (MPS0-learn re-address exercised); `UNAOS_IRQSTORAGE=1
UNAOS_FATIMG=sf test 200` → S-chain 0 FAIL; `test 40` → MISSION SUCCESS; `UNAOS_NOSTORAGE=1`
clean.

**Metal verdict (2026-07-16, attended rMBP sitting; log
`rmbp-serial-2026-07-16-sitting.log`).** The M1 diagnosis is CONFIRMED with a sharper
cause than hypothesized: the FS mouse behind the HS hub answers the 8-byte header read
fine (`MPS0 learned 8 (programmed 64)`), i.e. the old zeroed-18-byte read was the
wrong-MPS0 symptom, exactly the short-read family M1 targeted. But the **re-ADDRESS
strategy is refused by real Panther Point silicon**: `ADDRESS_DEVICE` (BSR=0) on the
already-Addressed slot completes `code 19` (Context State Error) — deterministically,
all 3 attempts, reproduced 4× across 3 hub ports and a whole-hub replug (the §7g
watch-item fired as predicted, code 19 rather than 17). **M2's machinery is
metal-confirmed**: bounded paced attempts trace cleanly, and `dispose_downstream_slot`
ran leak-clean every time (disposed slots 3/7/9/14; subsequent disconnects tore down
`0 slot(s)` with correct scope — no active-slot leak, no wedge, keyboard/storage/FTDI
survived every cycle; the keyboard re-enumerated cleanly on multiple ports throughout).
**→ XENUM-4 seed:** apply the learned MPS0 via an **Evaluate Context** command (the
standard driver approach — xHCI 4.6.7 explicitly supports updating EP0 Max Packet Size
on an Addressed slot) instead of re-issuing ADDRESS_DEVICE; the mouse should then
enumerate. The device descriptor bad-read path (`bad read (got G of 18…)`) never fired
on metal — nothing reached the 18-byte read with a wrong MPS0 anymore, which is the
mechanism working as designed upstream of it.

### 7g-4. MPS0 apply via Evaluate Context (XENUM-4)

**What.** The XENUM-3 M1 MPS0-learn step now applies the learned `bMaxPacketSize0` with an
**Evaluate Context** command (TRB type 13, xHCI 4.6.7) instead of a second `ADDRESS_DEVICE`.
The 8-byte-header read and the learn predicate are unchanged; only the *apply* mechanism
changed. `evaluate_downstream_ep0_mps` builds an input context with **A1 set (EP0 only),
A0 clear** per 4.6.7 for an MPS-only update, copies the live EP0 context out of the existing
output (device) context so the EP Type / CErr / TR Dequeue Pointer that `ADDRESS_DEVICE`
established are preserved, patches MPS0 (DW1 bits 31:16), and issues the command via
`run_command_sync`. The existing output context, EP0 ring, DCBAA pointer and slot state are
untouched — no fresh allocations, no DCBAA rewrite, no second `ADDRESS_DEVICE`. On success
the path continues straight into the 18-byte descriptor read (the XENUM-3 retry loop and
bad-read predicate downstream are unchanged); on failure it traces the completion code and
falls through to the existing `dispose_downstream_slot` bail — no retry storm (a refused
Evaluate Context is a new fact to capture, not to blindly hammer).

**Why.** Metal (§7g verdict, 2026-07-16): re-issuing `ADDRESS_DEVICE` (BSR=0) on an
already-Addressed slot is refused by real Panther Point silicon with completion **code 19
(Context State Error), deterministically** (3 attempts × 4 reproductions × 3 hub ports).
Evaluate Context is the standard mechanism xHCI 4.6.7 provides for exactly this — correcting
EP0 Max Packet Size on a slot in the Addressed state (mirrors Linux `xhci_check_maxpacket`).

**Plumbing removed.** The XENUM-3 re-address path is gone: `address_downstream` no longer
takes an `mps0_override` parameter (the only override caller was the re-address), and there
is now a single MPS0-application path (Evaluate Context) rather than two competing ones. The
XENUM-3 "first-attempt context-leak residual" — a re-address allocated a *second* input/output
context/EP0 ring for the same slot, leaking the first set — disappears with the re-address:
Evaluate Context reuses the slot's existing contexts, so no second allocation happens.

**Trace substrings.**
`downstream slot N MPS0 learned M (programmed P); Evaluate Context.` (learn + apply intent),
`downstream slot N EP0 MPS updated via Evaluate Context (M).` (success), and the failure form
`downstream slot N Evaluate Context code C; disposing.` The XENUM-3 `re-addressing.` line no
longer appears.

**QEMU gates (no-regression + direct functional):** `UNAOS_HUBSTORAGE=1 test 60` → the hubbed
FS disk enumerates through the NEW path (both Evaluate Context trace lines fire, no
`re-addressing` line) + MISSION SUCCESS + full U-arc chain, 0 FAIL — QEMU now exercises the
changed apply path; `UNAOS_IRQSTORAGE=1 UNAOS_FATIMG=sf test 200` → 0 FAIL; `test 40` →
MISSION SUCCESS; `UNAOS_NOSTORAGE=1` clean.

**METAL-PENDING.** The code-19 wall itself can only fall on silicon — QEMU accepts both the old
re-address and the new Evaluate Context, so the QEMU gate proves the new path *works*, not that
it *cures* the metal strand. The rMBP sitting asserts the FS mouse behind the HS hub enumerates
and tracks (the descriptor read completing at the learned MPS0, HID interrupt endpoint bound).

---

### 7h. Full-word downstream change acknowledgement (XHUB-SS-linkstate)

**Metal finding (2026-07-17, attended rMBP sitting).** Plugging/unplugging a
**card-reader-with-no-card on the SuperSpeed hub** (slot 9) left a downstream port with
`wPortChange = 0x0040` (**C_PORT_LINK_STATE**) **unacknowledged**. The hub keeps its
change-bitmap bit set while any C_* feature stays set, so its interrupt-IN Status Change
Endpoint **kept re-firing** (observed **1158+×**), pegging the CPU until the reader was
physically pulled — the "angry Mac" storm.

**Root cause.** `service_one_hub_change` serviced the *connection* change (M2 enumerate /
M3 teardown) but acked the change word with a loop over change bits 0..4 mapping bit `i` to
ClearPortFeature selector `16 + i`. That contiguity holds **only for USB 2.0 hubs**
(connection/enable/suspend/over-current/reset → 16..20). On a **SuperSpeed** hub the
non-connection selectors are relocated and the loop never reached bits 5..7:
`C_BH_PORT_RESET` = bit 5 → selector **29**, `C_PORT_LINK_STATE` = bit 6 → selector **25**,
`C_PORT_CONFIG_ERROR` = bit 7 → selector **26** (USB 3.x §10.14.2.6, Tables 10‑8/10‑11).
So an SS link-state change was structurally impossible to ack and stormed forever.

**Fix (additive; connection semantics unchanged).** After `GET_PORT_STATUS`,
`service_one_hub_change` now acks **every set change bit** in `wPortChange`, mapping each
bit to its real selector via `hub_port_change_feature_selector(bit, is_ss)` (reserved bits
skipped). The M2 connect enumerate / M3 disconnect teardown path is untouched; only the
missing non-connection acks are added. Bounded: at most 16 `ClearPortFeature` transfers
(one 16-bit word), no new unbounded loop; the per-wake `HUB_CHANGE_BUDGET` cap still stands.

**Witness (bench-assertable):**
- `HUB slot N port P acked change bits 0xAAAA of wPortChange 0xCCCC (SS|HS/FS) — Status
  Change Endpoint quiesced.` — the `acked` mask equals the set change bits (reserved bits
  excepted); a residual `wchange & !acked` on a selectable bit would be the storm signature.

**⚠ METAL-PENDING.** QEMU does not model the SS-no-card link-state edge trigger, so the
storm cannot be reproduced headless; the ack-completeness is verifiable in code and the
connection hot-plug/teardown path stays green (§7d witnesses unchanged). The cure — the
reader-with-no-card plug/unplug no longer storms — rides the next rMBP sitting.

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

### 9a. EHCI-2 — configure-and-relook scout (EHCI-2)

The EHCI-1 metal survey found both EHCI functions **asleep** (`RS=0`, `HCHalted=1`,
`CONFIGFLAG=0`). Asleep, "no device on the port" and "device not visible while the controller
is unconfigured" are **indistinguishable**. EHCI-2 resolves that ambiguity: it runs the
**minimal wake sequence** and re-censuses the ports **twice** — once before and once after
`CONFIGFLAG=1` — so the connect state is read under both routings. It is **evidence only**:
no enumeration, no transfers, no port reset, no async/periodic schedule, no driver.

**Knob-gated, default-OFF, implies the scout.** Compiled only under the `ehciconfig` Cargo
feature (`UNAOS_EHCICONFIG=1`, mapped in both `arroyo` and `builder/src/main.rs`;
`ehciconfig = ["ehciscout"]`). Knob-off, the config path is unlinked and `ehci_scout.rs` is
**byte-identical to the EHCI-1 read-only scout**.

**Write surface (tripwire-grade).** The **only** registers written are, per EHCI function, its
own **PMCSR** (D0 wake, `PME_Status` masked), its **`USBLEGSUP` OS-owned bit** (ownership
handshake), its **`USBLEGCTLSTS`** (eecp+4: SMI-enable mask cleared + RW1C status acked after
OS ownership — a Maestro-granted write-surface extension, one register inside the same extended
capability, the standard `quirk_usb_handoff_ehci` discipline), **`USBCMD.RS`**
(run), **`CONFIGFLAG`** (route ports to EHCI), and **`PORTSC` port-power** bits (PP, only when
`HCSPARAMS.PPC=1` gives software control; PP writes mask off the RW1C bits). Nothing else — it
never touches `XUSB2PR` / `USB3_PSSEN` / any xHCI register (PORTSW-1 owns routing), never resets
a port, never rings a doorbell. Each MMIO write is `translate()`-guarded, so a BAR outside the
firmware identity map is reported skipped instead of faulting.

**Sequence, per EHCI function**, each step reported with before/after register values:
1. **PMCSR → D0** if not already (transition reported).
2. **`USBLEGSUP` OS-ownership handshake**: set OS-owned, **bounded** wait for the firmware to
   drop BIOS-owned. A stuck BIOS bit is **not forced** — it emits a `STOP-NOTE` line and the
   sequence continues to the census (never a panic). The adjacent **`USBLEGCTLSTS`** (eecp+4,
   the SMI enable/status register) is read before the OS-own write and after it, then — with OS
   ownership held — its **SMI-enable mask is cleared and RW1C status bits acked**
   (`quirk_usb_handoff_ehci` discipline; otherwise firmware-left SMI enables can raise SMIs into
   a BIOS handler on the OS-own/RS/CF/PP writes — the classic EHCI BIOS-handoff hang), and the
   final value read back: three reported values,
   `USBLEGCTLSTS@…: pre=… post-own=… cleared->…`.
3. **`RS=1`**, bounded wait for `HCHalted=0`; then **census A** (`RS=1`, `CONFIGFLAG=0`, ports
   routed to the companion); then **`CONFIGFLAG=1`** + **port-power** (PPC honored) + a paced
   ~150 ms connect-debounce settle; then **census B**.
4. The controller is left as-configured (knob-gated diagnostic boot; no teardown).

**Trace substrings (sitting-assertable).** Bounded block bracketed by
`:: EHCI-CONFIG: begin ::` … and the verdict line
`:: EHCI-CONFIG: end (N controllers, censusA=K1 connected, censusB=K2 connected) ::`. Per
function: `begin wake sequence`, `PMCSR D…->D0` (or `already D0` / `no PCI PM capability`),
`USBLEGCTLSTS@…: pre=… post-own=… cleared->…`,
`USBLEGSUP@… OS-own set, BIOS-owned cleared` (or the `STOP-NOTE … BIOS-owned bit STUCK` line),
`RS 0->1: … (running)`, `censusA = K1 connected`, `CONFIGFLAG 0->1: read-back CF=1`,
`censusB = K2 connected`, `done — censusA=K1 connected, censusB=K2 connected`.

**Pre-registered interpretation (the whole point of the arc):**
- **Census B shows ≥1 connected on an internal-plausible port** → the internals **are
  asleep-until-configured USB** → an EHCI mini-driver becomes the proposal (back to
  Maestro/Peter).
- **Both censuses 0 connected** with the controllers running and `CONFIGFLAG` exercised both
  ways → the USB hypothesis is **falsified at the config tier** → the **SPI** hypothesis
  becomes the line (proposal only; no SPI work starts here).

**QEMU result (knob-on, `UNAOS_EHCICONFIG=1 UNAOS_EHCISCOUT=1`).** The harness `usb-ehci`
(`8086:24cd`, 6 ports, `PPC=0`, EECP=0x68) walks the full sequence: OS-own handshake trivial
(`USBLEGSUP` → `0x01000001`, BIOS-owned already 0), `RS=1` sticks (`HCHalted=0`, running),
`CONFIGFLAG=1` sticks (CF read-back 1), `PPC=0` so no PP write, and **0 connected on both
censuses** — QEMU attaches no downstream device, the honest baseline. **Metal-pending:** the
attended rMBP sitting is where census B decides the two branches above.

**Known residuals / watch items.**
- **`USBLEGCTLSTS` SMI-clear is QEMU-trivial.** QEMU's `usb-ehci` exposes no set SMI enables
  (pre/post-own read `0xc0000000`, enables `0x0000`), so the clear+ack path is only truly
  exercised on metal, where the firmware may have left enables set. The three-value evidence
  line records what was found and what remained after the clear.
- On this diagnostic boot the controllers are left with `RS=1`/`CONFIGFLAG=1` **before xHCI
  init runs** — an interaction unverified on metal, acceptable for a knob-gated evidence boot.
- The PMCSR D0 write masks `PME_Status` (bit 15, RW1C) so a pending PME isn't silently acked,
  and a real D-state transition is followed by a ~10 ms settle before the first MMIO read
  (no-op when already D0). Neither path is exercisable on QEMU (`usb-ehci` has no PM cap).

**Metal verdict (2026-07-16, attended rMBP sitting; log
`rmbp-serial-2026-07-16-sitting.log`): THE ASLEEP-UNTIL-CONFIGURED BRANCH FIRED —
`censusA=0, censusB=2`.** Both Panther Point EHCI functions (`8086:1e2d` bdf 0:26.0,
`8086:1e26` bdf 0:29.0; 2 ports each, `PPC=0`) walked the full wake sequence with no
hang: already D0; `USBLEGCTLSTS pre=0xc00c0000`/`0xc00d0000` (RW1C status latched by
firmware, **SMI enables 0**) cleared/acked cleanly; OS-own trivial (BIOS bit already 0);
`RS=1` stuck on both. Census A (CF=0): 0 connected. After `CONFIGFLAG=1`: **each
controller reported exactly one connected device — `PORTSC[0]=0x00001803` connect=1,
EHCI-owned, J-state (FS/LS device present) — on both functions.** The internal
keyboard + trackpad are therefore **real USB devices, asleep-until-configured behind
the EHCI companions**; the SPI hypothesis is falsified. **→ the EHCI mini-driver line
is the proposal** (enumerate through the Panther Point rate-matching-hub topology,
periodic interrupt-IN for boot-protocol HID — reusing the existing HID decode). The
RS=1/CF=1-before-xHCI interaction watch item was benign on metal: xHCI enumeration,
storage, and the FTDI mirror all ran normally the same boot.

---

## 10. EHCI-3 — the minimal EHCI HID driver

The driver the §9/§9a evidence arcs were run for: `drivers/ehci/` (`mod.rs` driver +
`qh.rs` schedule structures), armed by `UNAOS_EHCIHID=1` (feature `ehcihid`, implies
`ehciconfig`). Goal: the 2012 rMBP's **internal keyboard and trackpad** — real USB devices
asleep behind the Panther Point EHCI companions on **non-switchable** ports (§9a censusB).
Because PORTSW (§7f) moves only the *switchable* shared ports to xHCI, the two stacks own
**disjoint port sets by hardware**: EHCI-3 runs as a permanently-active second
host-controller stack alongside xHCI (Peter-confirmed end state) and never reads or writes
an xHCI register.

**One wake path.** The EHCI-2 wake was refactored into shared halves
(`ehci_scout::wake_run` — PMCSR→D0, `USBLEGSUP` handshake + `USBLEGCTLSTS` SMI clear/ack,
`USBCMD.RS=1` — and `wake_route` — `CONFIGFLAG=1`, port power, settle). The §9a evidence
mode calls them with its censusA in between; the driver calls the same two functions.
Refactor gates held: all-knobs-OFF kernel byte-identical, §9a trace line-for-line unchanged.

**Transfer machinery (polling-first, no interrupts).** One reusable head-of-reclamation
control QH on a self-linked async list runs all EP0 enumeration synchronously in main-loop
context (the `sync_control` idiom); a 4 KiB periodic frame list points every entry at one
interrupt QH per HID endpoint, each with a single re-armed qTD (the `queue_keyboard_read`
idiom, software-tracked data toggle via DTC=1). `service_ehci_hid()` polls from the same
main-loop spot as the xHCI service hooks; no `USBINTR` write, no IDT vector, no MSI. All
DMA structures come off the identity-mapped heap, 32-bit-checked at allocation
(`CTRLDSSEGMENT=0`; Panther Point is 32-bit-only per §9).

**The topology fork (M1 evidence gate).** The first `GET_DESCRIPTOR` on the root-port
device decides on a serial line — `:: EHCI-HID: [i] M1 root device … -> TOPOLOGY A|B ==
witness ::`. Class `0x09` ⇒ **A**: the device is the integrated Rate-Matching Hub; the
driver enumerates it as a hub (hub descriptor, port power, downstream reset,
`GET_PORT_STATUS` speed learn) and reaches the FS/LS HID children through the RMH's TT via
split transactions. Anything else ⇒ **B**: direct device, no splits. The QH builder
parameterizes hub-addr/port/S-mask/C-mask (zero on B), so both branches share every other
line of machinery. **Split-mask note (design review N1):** S/C-masks are *microframe* masks
evaluated in each frame the QH is reached (EHCI 4.12.2), so the every-frame frame-list
simplification stays split-correct — S-mask 0x01 (SSPLIT µframe 0), C-mask 0x1C
(CSPLITs µframes 2–4), a 1 ms service rate that over-serves the 8 ms boot-HID interval
harmlessly. Deliberate, not inherited.

**Enumeration robustness (§6 lessons, transport-independent):** 100 ms connect debounce,
paced reset retries (200/400/600 ms), MPS0 header-learn before the full descriptor read
(the XENUM-3 short-read trap), one-device-at-a-time. **Driver-owned addressing (review
N2):** EHCI has no slot model, so a monotonic allocator owns the 7-bit address space; a
failed enumeration **burns** its address (never reused for a possibly-half-addressed
device), traced `address N BURNED`. With 2 root ports + a ≤ 8-port RMH tier and no hot-plug
rescan this arc, exhaustion (127/boot) is unreachable and traced if hit. Boot reports
decode through the **same scancode table and layout logic as the xHCI HID path**
(`HID_SCANCODE_TO_ASCII`, pub(crate)) into `pal::Event::Key`/`Mouse`; non-boot HID
interfaces (the likely Apple-trackpad case, R3) are skipped with an honest trace — the
keyboard gates M3, the trackpad splits to a follow-on if non-boot.

**Write surface (tripwire-grade).** The §9a wake surface plus, declared: `PORTSC.PR` (RW1C
change bits masked), `USBCMD.ASE/PSE`, `PERIODICLISTBASE`/`ASYNCLISTADDR`/`CTRLDSSEGMENT=0`,
`USBSTS` RW1C acks, the driver's own frame-list/QH/qTD/buffer DMA memory, EP0 device + hub-
class requests, and — a Peter-approved extension (2026-07-16) the design doc omitted — the
EHCI functions' **own PCI COMMAND** Memory-Space + Bus-Master enables (a DMA precondition;
read-checked, set only if clear, traced before/after). Never any xHCI register, never a
switchable-mask port. Every MMIO access translate()-guarded; every wait bounded; stuck
handshakes are traced STOP-NOTEs, never forced. `HCRESET` is not used (Peter item (c):
build on the metal-clean wake; reset only if M1 ever finds inconsistent port state — it has
not).

**QEMU gates (honest).** QEMU's `usb-kbd` is HS-capable and trains directly on the harness
`usb-ehci` root port, so QEMU proves **Topology B end-to-end**: wake, reset (PED=1), M1
witness, `SET_PROTOCOL(boot)`, periodic interrupt-IN, QMP `send-key` typing decoded to
`EHCI-HID: KEY` + `pal::Event` with bounded report witnesses (first, then every 32nd).
Under the knob the harness moves the QEMU keyboard from the xHCI bus to the EHCI bus
(deterministic `send-key` routing); knob-off the harness is unchanged. **QEMU cannot run
the hub tier at all**: its only hub model is full-speed, and a FS hub on the EHCI bus (no
companion, no TT model) wedges the machine before firmware prints a byte — measured, not
assumed. So **Topology A (RMH hub walk + splits) is metal-first by construction**, decided
at the sitting by the M1 witness line. `UNAOS_EHCIHID` stays **default-OFF until the
internal keyboard types on metal** (M3), then a pre-registered PORTSW-style default-ON fold
(a separate arc — Peter item (b)).

**METAL VERDICT (2026-07-17 sitting, 15-probe live debug loop): M1+M2+M3 CONFIRMED — the
internal keyboard TYPES.** The probe ledger, each step register-witnessed on serial:
firmware leaves PSE=1 (stale periodic schedule → HSE; cured by the pre-approved HCRESET);
this silicon master-aborts the **qTD-fetch overlay-load burst write** (PCI STATUS RMA=1) while
every other DMA class passes (frame-list reads, QH burst reads, dword token write-backs,
payload reads, live-port transactions — proven by a five-pass smoke battery); VT-d/PMR/FD/CG
all falsified with registers; an HSE'd controller is WEDGED until HCRESET; the periodic
engine skips SETUP-PID overlays; the **async engine executes software-primed overlays
cleanly**. The driver therefore self-adapts: chain mode (QEMU's model requires fetched qTDs)
→ first chain HSE flips the controller permanently to **OVERLAY-DIRECT** (software pre-loads
the QH overlay per stage; no qTD is ever handed to the controller) with a full HCRESET
re-init. Real topology: RMH `8087:0024` per function; fn0 → FaceTime camera `05ac:8510`;
fn1 → FTDI `0403:6001` (FS through the RMH's TT — splits work) + SMSC hub `0424:2512` →
Broadcom BT `0a5c:4500` + **Apple Internal Keyboard/Trackpad `05ac:0262`** (depth-2 tier;
keyboard boot-protocol armed ep=IN3 mps=10; trackpad = non-boot proto 0, the R3 follow-on).
Physical keystrokes decode end-to-end (`EHCI-HID: KEY` + pal events). Sitting log:
`~/unaos-bench/rmbp-serial-2026-07-17-ehci3-M3-sitting.log`.

**Knob-off identity.** The `ehci/` module and every call site are `#[cfg]`-compiled out;
`.text`/`.rodata` are byte-identical knob-off (ELF section compare). Whole-file identity is
broken only by panic-`Location` line-number metadata (`.data.rel.ro`) + symbol tables
shifting from the cfg-gated insertions in shared files — code-free drift, stated here
rather than hand-waved.

### 10a. EHCI-4 M1 — the default-ON fold

The EHCI-3 metal verdict above (M3, the internal keyboard TYPES) is the evidence gate the
pre-registered default-ON fold (Peter item (b), the PORTSW-1 pattern) waited on. So on x86
the EHCI HID driver now runs **by default**; `UNAOS_NOEHCIHID=1` **opts out**.

**Mechanism (and why it differs mechanically from PORTSW-1).** PORTSW is one self-contained
`pci.rs` function, so its fold was a literal cfg inversion (`#[cfg(not(feature = "noportsw"))]`).
The EHCI driver instead sits on a deep positive-feature implication chain
(`ehcihid` → `ehciconfig` → `ehciscout`, plus the ACPI-root retention in `acpi.rs` and the
`service_ehci_hid` main-loop hook) — inverting every gate would churn a byte-identity-proven
shared module (`ehci_scout`) and a second subsystem (`acpi`). The fold instead flips the
**plumbing**: `arroyo`/`builder` push `ehcihid` **by default** and suppress it under
`UNAOS_NOEHCIHID=1`. Every existing `#[cfg(feature = "ehcihid" | "ehciconfig" | "ehciscout")]`
gate is unchanged and now resolves on-by-default; opting out enables none of them, so the
module + all call sites unlink and the kernel is **byte-identical to the pre-fold no-EHCI
media** (proven: opt-out `.text`/`.rodata` SHA256 == the pre-fold default build,
`7bbde326…` / `f939e27d…`). The user-facing contract is the PORTSW negative-knob idiom;
the compile topology is preserved intact.

**Diagnostic decouple.** `ehciscout`/`ehciconfig` are now *compile* features (the scout
module + shared wake are built by default because the driver depends on them). The read-only
census (`scout()`) and the configure-and-relook evidence pass (`configure_and_relook()`) are
gated at their `pci.rs` call sites on new **`ehciscout_run`/`ehciconfig_run`** features, so a
default boot builds the module but runs **only the driver** — neither evidence probe fires
(no census spam, no redundant pre-init wake). `UNAOS_EHCISCOUT=1`/`UNAOS_EHCICONFIG=1` still
arm the probes; pair either with `UNAOS_NOEHCIHID=1` for probe-without-driver (the old
pure-evidence mode). QEMU-confirmed: the default boot enumerates the harness EHCI HID
(`M1 … TOPOLOGY B`, `M2 armed keyboard`) with **zero** `EHCI-CONFIG census` lines.

**Gates (M1).** Knob-matrix `./arroyo check` green both arches for default, `UNAOS_NOEHCIHID=1`,
`UNAOS_EHCISCOUT=1 UNAOS_EHCICONFIG=1`, and `UNAOS_NOEHCIHID=1 UNAOS_EHCICONFIG=1`
(pure-evidence). Default `test 40` MISSION SUCCESS with the driver enumerating on the same
boot; `UNAOS_NOSTORAGE=1` clean; `UNAOS_IRQSTORAGE=1 UNAOS_FATIMG=sf test 200` 0 FAIL / 0
PANIC with the storage service task + real FAT writes (S3/S4/S5/S8) unregressed under the
default-active driver (the R6 coexistence proof now runs as the DEFAULT config). Opt-out
byte-identity proven as above. The harness moves the QEMU `usb-kbd` onto the EHCI bus **by
default** now (was `UNAOS_EHCIHID`-gated); `UNAOS_NOEHCIHID=1` restores the pre-fold harness
(kbd on xHCI, no EHCI controller unless a scout knob asks).

### 10b. EHCI-4 M2 — the internal trackpad (report-protocol pointer path)

The keyboard is a **boot** HID (proto 1) — a fixed 8-byte report. The Apple internal trackpad
(`05ac:0262`, EHCI-3 metal verdict) is a **non-boot** HID (proto 0): it exposes its report
format only through its **HID report descriptor**, so `SET_PROTOCOL(boot)` does not apply and
the fixed boot-mouse layout cannot decode it. M2 adds that path: the last internal input device
moves the cursor.

**Sequence** (`configure_report_pointer`). For a proto-0 HID interface the driver:
1. reads the interface's HID class descriptor length (captured in the config-descriptor walk),
2. issues **GET_DESCRIPTOR(Report)** — `bmRequestType 0x81` (in | standard | **interface**
   recipient), `bRequest 6`, `wValue 0x2200`, `wIndex = interface` — into the 256-byte control
   buffer (`Buf256`, up from 64 B so a full report descriptor fits and can be dumped verbatim),
3. **parses** the descriptor (`parse_report_descriptor`) into the pointer field map,
4. leaves the interface in its native **report** protocol (no `SET_PROTOCOL` — report is the
   default after `SET_CONFIGURATION`; the boot request is not sent),
5. arms one periodic interrupt-IN QH (the same `arm_interrupt_ep` machinery) carrying the parsed
   layout, and
6. decodes each report through that layout in `service_ehci_hid` → `pal::Event::MouseAbsolute`
   (absolute axes: a tablet / the trackpad) or `pal::Event::Mouse` (relative: a mouse) — the
   **same** pointer-event path the xHCI HID stack delivers.

**Report-parser contract (what subset, why safe).** `parse_report_descriptor` is deliberately
**not** a general HID stack. It walks the short-item stream (HID 1.11 §6.2.2.2) tracking only
Usage Page / Report Size / Report Count / Report ID and the queued local Usages, and extracts
exactly the fields a pointer report needs: the Generic Desktop **X** (usage 0x30) and **Y**
(0x31) variable Input fields — their bit offset, bit size, and relative-vs-absolute flag — the
**Button** bitfield (usage page 0x09), and, as a witness only, the Digitizer **Contact Count**
(0x0D:0x54). Everything else is skipped. Safety: it never trusts the device to be a pointer —
if no variable X/Y field is found it returns `None` and the endpoint is **skipped with an honest
trace, never mis-armed**; long items (`0xFE`) and any tail past the read bytes end the walk
cleanly (a truncated read is bounded, not a fault); bit extraction is length-checked against the
report buffer, so a malformed descriptor cannot make the decoder read out of bounds. It assumes a
single pointer report (a new Report ID restarts the body offset); deep multitouch (multiple
report IDs, per-finger records) is the metal follow-up below.

**Verbatim descriptor slot (the 0262 is metal-first).** The exact `05ac:0262` report descriptor
is captured on serial at the attended sitting (`dump_report_descriptor` prints it as hex with the
declared vs read length, so a > 256 B descriptor is visibly truncated). **QEMU stand-in
(captured):** `usb-tablet` is a proto-0 **absolute** pointer — the same mechanics — and its 74-byte
descriptor parses to `X@8/16b Y@24/16b btn@0×5 id=0 body=48b` (buttons in byte 0, X in bytes 1–2,
Y in bytes 3–4, 16-bit little-endian 0..32767):

```
05 01 09 02 a1 01 09 01 a1 00 05 09 19 01 29 05 15 00 25 01 95 05 75 01 81 02
95 01 75 03 81 01 05 01 09 30 09 31 15 00 26 ff 7f 35 00 46 ff 7f 75 10 95 02 81 02 ...
```

`<< 05ac:0262 internal trackpad report descriptor — capture verbatim here at the M3 sitting >>`

**QEMU gate (honest).** `UNAOS_EHCITABLET=1` moves the harness `usb-tablet` onto the EHCI bus;
the driver enumerates it as a second root-port device, reads + parses its report descriptor, arms
the report-pointer QH, and — driven by QMP `input-send-event` absolute moves — decodes reports to
`MouseAbsolute` with the parsed offsets, matching the injected coordinates exactly (witness:
`report-pointer 1 reports … abs x=1000 y=800`, `32 reports … abs x=22700 y=13200`). The keyboard
still arms and storage MISSION SUCCESS on the same boot (M1 unregressed). **What QEMU does NOT
cover:** the real 0262 is FS **behind the RMH's TT** (split transactions — metal-only, §10) and is
likely a **multitouch** digitizer with Report IDs and a descriptor that may exceed the 256-byte
read cap; the parser decodes its primary pointer report, and the full multitouch decode + any
larger-buffer read is a metal follow-up decided at the sitting by the verbatim capture.

**Sitting-assertable trace list (M2, rMBP serial bridge armed).** On the default-ON build, after
the M1 keyboard lines (§10 trace list):
1. `:: EHCI-HID: [i] addr N intf M report descriptor (K of L B)[ …truncated]: <hex> ::` — the
   verbatim 0262 descriptor (capture it for the slot above; note if `K < L`).
2. `:: EHCI-HID: [i] M2 armed report-pointer addr=N ep=INx mps=.. interval=.. (absolute; X@../..b
   Y@../..b btn@..x.. id=.. body=..b[, multitouch]) == witness ::` — the parsed field map.
   - If instead `… report descriptor has no X/Y pointer field … skipped` ⇒ the 0262's primary
     interface is not a plain pointer (multitouch-only); report the descriptor as-is.
3. **Move the trackpad** → `:: EHCI-HID: [i] report-pointer N reports, last abs x=.. y=..
   buttons=.. fingers=.. == witness ::` (first + every 32nd) and the cursor moves via
   `pal::Event::MouseAbsolute`. `fingers>0` confirms a Contact-Count field was found.
4. STOP-NOTE lines (`GET_DESCRIPTOR(Report) failed`, `no report-descriptor length`, interrupt-EP
   halt) are the honest failure reports — relay verbatim, never forced.

**Gates (M2).** Knob-matrix `./arroyo check` green both arches. `UNAOS_EHCITABLET=1 test 40`:
the tablet enumerates on the EHCI bus, GET_DESCRIPTOR(Report) reads the full 74 B, the parser
derives the exact layout, the report-pointer QH arms, QMP-injected moves decode to `MouseAbsolute`
matching the injected coordinates, keyboard armed + storage MISSION SUCCESS same boot. Default
`test 40` (no tablet) MISSION SUCCESS with the trackpad path dormant; `UNAOS_IRQSTORAGE=1
UNAOS_FATIMG=sf test 200` 0 FAIL / 0 PANIC with the M2 code compiled in. The 256-byte control
buffer is behaviour-neutral for the ≤ 64 B enumeration reads.

---

### 10c. EHCI-5 — the internal trackpad is Apple vendor multitouch (Report ID 0x44)

The 2026-07-17 six-knob rMBP sitting typed on the internal **keyboard** (§10) but the internal
**trackpad** moved no cursor: its interface 1 is not a standard pointer. It is Apple
**vendor-defined multitouch** — **Report ID `0x44`**, usage page **`0xFF00`**, a 27-byte report
descriptor with **no** Generic Desktop X/Y field. The §10b report-descriptor parser correctly
classified it "not a cursor device; skipped" (the honest-skip discipline): the descriptor gives the
report's total size, not which bytes are a finger's X/Y, so there is nothing for the standard X/Y
gate to find. EHCI-5 closes this **decoder gap** so a **single finger drives the pointer** (relative
motion). Multitouch gestures are explicitly **out of scope** — only finger[0] is decoded.

**M1 — recognize + capture.** After the standard X/Y gate finds nothing, `parse_report_descriptor`
runs a vendor-multitouch recognition pass: an opaque **non-Constant** Input on the vendor page
(`UP_VENDOR = 0xFF00`) carrying a Report ID and a non-trivial bit size (`≥ VMT_MIN_VENDOR_BITS`,
64 bits) yields `ReportLayout{ vendor_mt: true, report_id, .. }` instead of `None`. Because this
branch runs **only** when `has_xy` is false, a real Generic Desktop pointer is never diverted onto
it (proven in QEMU: `UNAOS_EHCITABLET=1` still arms the `usb-tablet` with the unchanged layout
`X@8/16b Y@24/16b`). `configure_report_pointer` then **arms** the endpoint (rather than skipping)
with a distinct witness naming the hypothesis offsets, and `service` **dumps the raw `0x44` report
body verbatim** (`dump_vendor_report`, first + every 32nd report) — the reverse-engineering evidence
the sitting reads to confirm or correct the finger layout.

**M2 — decode → relative motion (single finger).** For a `vendor_mt` endpoint, `service` decodes the
**first finger only** via `decode_vendor_first_finger`: strip the `0x44` Report ID prefix, then read
`abs_x` / `abs_y` as **signed le16** at the hypothesis byte offsets and read presence from the touch
field. Finger-**DOWN** (false→true) seeds `last_x/last_y` **without emitting** (so the cursor never
jumps from stale coordinates); while touching, it emits
`pal::Event::Mouse { x: cur_x - last_x, y: cur_y - last_y }` — the **same** relative pointer event
the boot-mouse and xHCI paths deliver — then updates `last`; finger-**UP** clears `touching`. Every
field read is bounds-checked (`read_le16` returns `None` past the slice), so a short or malformed
`0x44` report never reads out of bounds or emits garbage motion; it simply yields no event and leaves
the touch state untouched.

**The hypothesis (KNOWN vs must-reverse-engineer).** **KNOWN at metal:** interface 1 is Apple vendor
multitouch — Report ID `0x44`, usage page `0xFF00`, 27-byte descriptor, no standard X/Y. **LEAD (the
public bcm5974 TYPE2 finger record — a hint, not a fact):** le16 fields `abs_x@+2, abs_y@+4 (signed),
touch_major@+16, pressure@+24` inside a finger record that a ~30-byte header precedes. The offsets in
code (`VMT_HDR_LEN = 30`, `VMT_FINGER_ABS_X/ABS_Y/TOUCH`) are written as clearly-labelled
`HYPOTHESIS` constants so a sitting adjusts one line each. **The caveat that matters:** bcm5974 reads
a **separate raw** USB interface with **no** Report ID; this is a HID interface with a `0x44` prefix
byte (stripped first, as `decode_report_pointer` does). So the header size, the touch/contact field
location, and whether the finger record is byte-identical are **all unconfirmed** — the M1 raw-byte
capture at the sitting is what confirms/corrects them.

**QEMU is the whole gate here BY CONSTRUCTION.** QEMU has no Apple trackpad, so the vendor path never
arms in QEMU and the offset VALUES cannot be QEMU-proven. The **only** QEMU-provable witness is the
driver-init **self-test** (`vendor_multitouch_selftest`): it feeds a synthetic Apple-style vendor
descriptor and asserts `vendor_mt` recognition (M1), then feeds two synthetic `0x44` reports (finger
A → B, one negative coordinate) and asserts the first-finger decode + relative delta, a finger-up
report reads absent, and a too-short report decodes to `None` (M2 mechanics + bounds safety). This
proves the **mechanics**; correctness on the real `0262` is a **metal-verified hypothesis**, a later
attended leg — **not** DONE for this arc.

**EHCI-5-fix — the arming-order bug (Array vs Variable).** The 2026-07-17 rMBP 3-leg sitting proved
EHCI-5's recognizer **never fired on metal**: the internal `05ac:0262` intf1 was still logged
`… has no X/Y pointer field … not a cursor device; skipped`, so its endpoint was **never armed** and
touching produced **zero** reports. Root cause was **not** the offset hypothesis (execution never
reached the finger bytes) — it was an **arming-order/shape** gap in `parse_report_descriptor`. The
captured 27-byte descriptor is
`06 00 ff 09 01 a1 03 06 00 ff 09 01 15 00 26 ff 00 85 44 75 08 96 ff 01 81 00 c0`: its Input main
item is `81 00` = **Input (Data, Array, Absolute)** — an **Array**, not a Variable. The pre-fix
vendor-recognition sat **inside** the field-mapping block gated on `is_var`, so an Array Input never
set `saw_vendor_input` and the descriptor fell through to `None`. The fix moves the vendor-page
signature test **out** of the `is_var` block: any **non-Constant** Input on `UP_VENDOR` (Array **or**
Variable) now accumulates `vendor_bits` and, past the X/Y gate, yields `vendor_mt`. The standard
`has_xy` path is untouched and still wins first; the `MAX_REPORT_FIELDS` clamp and every bounds check
are unchanged. `vendor_multitouch_selftest` now also feeds the **real captured Array descriptor** and
asserts `vendor_mt` recognition — the witness gains `real-array-descriptor recognized=true`. The
finger DECODE offsets (`VMT_FINGER_*`) remain the unchanged metal hypothesis for the next sitting;
this arc only fixes recognition/arming.

**Buffer/packet ceiling (metal-decided).** `IntSlot.buf` is `Buf64` and interrupt reads cap at
`min(mps, 64)` — one ≤ 64 B packet. A full Apple MT report is ~430 B; header (~30 B) + finger[0]
(through `pressure@+24`, i.e. body byte ~54) fits a single 64 B read, but only just. **First sitting
check:** capture the raw `0x44` body and confirm `abs_x`/`abs_y` land within the 64 B read. If the
metal capture shows them beyond 64 B, growing `Buf64` + the two `min(64)` caps is **in-lane**
(`ehci/qh.rs` + `ehci/mod.rs`) but was deliberately **not** pre-sized in this QEMU-only arc.

**Sitting-assertable trace list (EHCI-5, rMBP serial bridge armed).** On the default-ON build:
1. `:: EHCI-HID: … vendor-multitouch self-test: recognized=true (id=0x44 …), first-finger decode
   dx=-150 dy=5800 ok=true == witness ::` — fires **every boot** (QEMU included); the mechanics gate.
2. `:: EHCI-HID: [i] M1 armed vendor-multitouch addr=N ep=INx … id=0x44 … (capture; hypothesis
   X@.. Y@.. le16, touch@..) == witness ::` — the trackpad interface was recognized and armed.
3. `:: EHCI-HID: [i] vendor-multitouch raw report #k (M B): <hex> == witness ::` — **capture these
   bytes**: they are the evidence that confirms or corrects `VMT_HDR_LEN` / `VMT_FINGER_*`. Note the
   report length `M` (is it truncated at the 64 B read?).
4. **Move one finger** → the cursor moves via `pal::Event::Mouse` (relative). If it moves the wrong
   direction or the wrong distance, adjust the `VMT_FINGER_*` offsets one line each from the raw
   capture and re-run.

**Gates (EHCI-5).** `./arroyo check` green both arches. `./arroyo test 40` MISSION SUCCESS with the
vendor self-test witness present (`recognized=true`, decode `ok=true`) and keyboard/storage
unregressed. `UNAOS_EHCITABLET=1 test 40`: the standard `usb-tablet` report-pointer still arms and
decodes with the unchanged layout (vendor recognition does not perturb the standard path).
`UNAOS_IRQSTORAGE=1 UNAOS_FATIMG=sf test 200` 0 FAIL / 0 PANIC with the EHCI-5 code compiled in.

### 10d. EHCI-TRACKPAD — the bcm5974 mode switch (start the multitouch stream)

EHCI-5(-fix) proved the internal trackpad's vendor-multitouch interface is now **recognized and
armed**, but on metal the stream never starts: the Apple trackpad (`05ac:0262`) ships in a
**single-touch compatibility mode** and emits **no** `0x44` multitouch frames until it is told to. The
Linux `bcm5974` driver flips it with a class **feature-report** handshake (`bcm5974_wellspring_mode`);
this arc ports that handshake into `configure_report_pointer`, fired **once, before arming**, whenever
a `vendor_mt` interface is recognized.

**M1 — the mode switch (`bcm5974_mode_switch`).** Three EP0 control transfers, run through the same
overlay-direct / chain-mode `control()` path every other request uses:
1. **GET_REPORT(Feature)** — `bmRequestType 0xA1` (IN | CLASS | INTERFACE), `bRequest 0x01`,
   `wValue 0x0300` (report type 3 = Feature, report id 0), `wIndex 0`, `wLength 8`. Reads the current
   feature report into `data_buf`; byte 0 + length are logged.
2. **Flip byte 0** — `data_buf[0] = 0x01` (`BCM5974_MODE_VENDOR`, raw multitouch; `0x08` would be the
   NORMAL single-touch mode). The remaining 7 bytes are echoed back as read.
3. **SET_REPORT(Feature)** — `bmRequestType 0x21` (OUT | CLASS | INTERFACE), `bRequest 0x09`,
   `wValue 0x0300`, `wIndex 0`, `wLength 8`. Writes the modified report back — the request that starts
   the stream.

`wValue`/`wIndex`/`wLength`/request-ids/mode byte are the **bcm5974 driver constants verbatim**
(`BCM5974_MODE_*`). One nuance carried from the reference: `wIndex` is the driver's `REQUEST_INDEX`
(**0**), **not** the interface number — the value proven on real MacBooks; the interface number is
logged alongside so a sitting can retry with `intf` if index 0 STALLs on this exact `0262`. Every stage
logs its status, and **any stall/timeout is non-fatal**: a firmware that already streams needs no
switch, so a failed handshake never un-arms the endpoint — it traces and the caller arms regardless.

**M2 — decode.** Unchanged from §10c: once frames arrive, `service` decodes the first finger via
`decode_vendor_first_finger` into the same relative `pal::Event::Mouse` path (the XENUM FS-mouse seam).
The switch only makes the frames *arrive*; the `VMT_FINGER_*` offsets remain the metal hypothesis.

**QEMU unchanged by construction.** The switch is gated on `vendor_mt`, which QEMU's `usb-tablet`
(a standard absolute pointer) never sets, so `bcm5974_mode_switch` is never reached in QEMU — verified:
`test 22` and `UNAOS_EHCITABLET=1 test 22` both MISSION SUCCESS with **no** `bcm5974 GET_REPORT` /
`SET_REPORT` line, and the keyboard (`M2 armed keyboard`) and tablet (`M2 armed report-pointer`) paths
arm exactly as before.

**Sitting-assertable trace (metal, deferred).** On the real `0262`, after the §10c recognition lines:
1. `:: EHCI-HID: [i] M1 bcm5974 GET_REPORT(feature) addr=N intf=M got=8b byte0=0xXX == witness ::`
   — the device answered the read (or the FAILED variant if it STALLs; the write still follows).
2. `:: EHCI-HID: [i] M1 bcm5974 SET_REPORT(feature) addr=N intf=M mode=0x01 — multitouch stream
   requested == witness ::` — the switch was accepted. If instead the FAILED variant prints, try
   `wIndex = intf` (see the nuance above) or confirm the recipient interface.
3. Then the §10c `vendor-multitouch raw report #k` dumps should begin **on touch** — the proof the
   stream started. Cursor motion from the trackpad is the metal verdict for this arc's next sitting.

**Gates (EHCI-TRACKPAD).** `./arroyo check` green both arches. `./arroyo test 22` MISSION SUCCESS,
keyboard + vendor self-test unregressed, **zero** mode-switch lines (QEMU never `vendor_mt`).
`UNAOS_EHCITABLET=1 test 22` MISSION SUCCESS, `usb-tablet` report-pointer path unchanged. Metal
(the switch actually starts the stream and the cursor moves) is **deferred** to the next attended
sitting — no flash media staged, no ESP rebuilt.

### 10e. RMBP-FIX — the stream is 8-byte Report ID 0x02 (the 0x44 multitouch model is REFUTED)

The 2026-07-18 attended sitting proved the mode switch (§10d) works: the internal trackpad **streams**
after the merged mode switch — **736+ reports observed**. But the reports are **NOT** the descriptor's
opaque `0x44` / 511-byte multitouch frame the §10c hypothesis modeled. Ground truth from silicon: the
reports are **8 bytes, Report ID `0x02`** — `[0]` = id (`0x02`), `[1]` = buttons (`0x00` up / `0x01`
down, confirmed via a held press), `[2]` = dx int8, `[3]` = dy int8, `[4..=5]` zero, `[6..=7]` unknown.
This is a plain **relative** pointer stream. The old `0x44`/multitouch-frame model is **refuted on this
device path** and must not be re-litigated.

**M1 — bound the raw dump.** `service`'s `vendor_mt` branch used to call `dump_vendor_report` on the
first + every 32nd report; under touch that is ~100+ **heap-allocating** serial lines/sec, which floods
the framebuffer console until the machine appears hung. The byte characterization it existed to capture
is now **complete**, so the dump is bounded **hard**: gated to the `usbdebug` build **and** the first
**4** reports per device (`#[cfg(feature = "usbdebug")] if e.reports <= 4`). On a default/GUI build the
whole block — and `dump_vendor_report` itself — is **compiled out**: zero dumps, zero `String` alloc on
the hot path.

**M2 — retarget the decoder (`decode_trackpad_rel`).** The live path now length-checks the report
(`len >= 4`), gates on `report[0] == 0x02` (`TRACKPAD_REPORT_ID`; a short or non-`0x02` report yields
`None` → no event, no state change), and reads `buttons`, `dx` (int8), `dy` (int8) straight into the
relative `pal::Event::Mouse` seam — the same path the boot-mouse uses. A bounded one-line **format
witness** prints on the first decoded report. The refuted `decode_vendor_first_finger` + `VMT_FINGER_*` constants + `vendor_multitouch_selftest`
are **kept as documented reverse-engineering history** (per the never-trash rule) — the self-test still
runs and passes at init, exercising that decode's mechanics; it is simply no longer the live path.

**M3 — EHCI keyboard in `pal::pump_and_poll`.** The internal keyboard rides EHCI, but `pump_and_poll`
(the input pump a full-screen demo like `vug`/`pulse` runs inside its own loop) only serviced xHCI, so
the built-in keyboard could never post the keystroke to exit the demo. It now also calls
`ehci::service_ehci_hid()` under `#[cfg(all(target_arch = "x86_64", feature = "ehcihid"))]` — a harmless
no-op in QEMU (xHCI-only; no EHCI HID controller armed) and on aarch64 (compiled out).

**QEMU vs metal honesty.** All three are **metal-behavior** fixes on the real `05ac:0262` trackpad /
internal keyboard. QEMU's `usb-tablet` never sets `vendor_mt` and QEMU has no EHCI HID, so QEMU proves
only **non-regression**; the metal verdict (cursor moves from the retargeted decode; keyboard exits a
demo) accrues to the next attended sitting.

**Gates (RMBP-FIX).** `./arroyo check` green both arches. `./arroyo test 22` + `UNAOS_CPU=qemu64 test 22`
MISSION SUCCESS, keyboard + vendor self-test unregressed, 0 FAIL. `UNAOS_EHCITABLET=1 test 22` MISSION
SUCCESS, the `usb-tablet` report-pointer path arms unchanged. `./arroyo test-arm 22` 0 FAIL / 0 PANIC.

### 10f. CLICK-1 — the trackpad click became an event

§10e decoded `buttons` but deliberately emitted no click event. The GUI sitting-1 verdict closed that
gap: `service`'s `vendor_mt` branch now emits exactly **one** `pal::Event::Button(buttons)` per button
**DOWN edge** (`prev_buttons & 0x01 == 0 && buttons & 0x01 != 0`, tracked per-endpoint in
`IntEp::prev_buttons`). Release emits nothing, and a held press emits nothing further — the edge test is
what keeps a ~100 Hz report stream from becoming a ~100 Hz click storm. The observable: a trackpad click
while `vug` or `pulse` is running exits the demo exactly like a keystroke.

One bounded serial line prints per press (human-rate, so it cannot flood the framebuffer console the way
the §10e M1 dump did):

```
:: EHCI-HID: [i] trackpad click (button-down edge, buttons=0x01) == witness ::
```

Because the decode is gated inside `decode_trackpad_rel`, a short or non-`0x02` report still yields
`None` — no event, and `prev_buttons` is left untouched, so a malformed report can neither synthesize a
click nor desynchronize the edge state.

**QEMU vs metal.** QEMU never sets `vendor_mt` (its `usb-tablet` is a standard absolute pointer) and has
no EHCI HID controller, so **no** QEMU gate can exercise this branch — QEMU proves non-regression only.
The click behaviour is a **metal-only** verdict, taken at the attended rMBP sitting.

### 10f-bis. CLICK-3 — the second stationary click (s41 metal defect)

**Observed (s41, Peter at the trackpad).** *Click, pause, click* loses the second click; *click, slide,
click* works. Motion between two presses is what makes the second one register.

**Traced end to end, and the loss is BEFORE `push_event`.** `pal::push_event` and `EventQueue`
(`pal.rs`) are a plain ring with drop accounting — no dedup, no single-slot latch — and the consumers act
on every `Button` they see (`vug::drain_input` exits on any `Button`; the x86 console loop has no
`Button` arm at all). So the only state on the whole path that both gates a click **and** is cleared by
pointer *motion* is `IntEp::prev_buttons`: a motion report carries `buttons == 0x00`, which resets the
latch. That is exactly the observed asymmetry.

**Why the latch goes stale.** The interrupt endpoint is armed for **one** report per service pass, and
`service_ehci_hid` is polled from the console frame loop (`main.rs`) — i.e. at frame rate, orders of
magnitude slower than the endpoint interval. A HID report is a **level** (the current button state), so a
release landing in the gap between two polls is superseded by whatever the pad reports next. Miss one
release and `prev_buttons` stays latched at `0x01` for the rest of the boot; every subsequent *stationary*
press then fails the §10f edge test, while any motion at all clears it.

**Fix (`IntEp::note_buttons`, drivers/ehci/mod.rs).** The §10f edge test is kept and a **re-press
recovery** added that reads the *silence between reports*. A held button either re-reports at the
endpoint's rate (consecutive pressed reports far closer together than `CLICK_REPRESS_QUIET_MS` = 120 ms)
or reports nothing until release (no pressed report during the hold). Neither can produce a pressed
report separated from the previous report by a long quiet gap — a **new press after a missed release**
always is. So a report that still reads "primary down" after ≥ 120 ms of endpoint silence is treated as a
new press. No held-button case gains a spurious repeat, the release/hold emit contract is unchanged, and
the EHCI transfer machinery is untouched. All three EHCI pointer paths (trackpad `0x02`, parsed
report-pointer, boot mouse) now share this one decision.

**Witness (`UNAOS_USBDEBUG=1` only).** Presses observed at parse vs `Button` events delivered, plus how
many deliveries only the recovery caught (i.e. clicks silently lost before this arc):

```
:: PTR: [i] press seen=N delivered=M recovered=K (re-press after quiet gap) == witness ::
```

`recovered > 0` on an attended boot is direct proof of the defect and of the fix. The §10f per-press line
still prints unchanged beside it.

**QEMU vs metal.** As with §10f: QEMU has no EHCI HID controller, so the gates prove non-regression only
(`check` both arches, knob-on and knob-off; `./arroyo test` — QEMU HID pointer still enumerates and
moves). The click behaviour remains a metal-only verdict.

### 10g. IVY MT-INVESTIGATION — where does true multitouch actually live? (`UNAOS_MTRAW`)

§10e refuted the "descriptor-advertised 0x44 / 511-byte frame streams as-is" model: 736+ observed
reports were all the 8-byte Report ID `0x02` relative shape. That left the real question open — the pad
plainly *has* a multi-finger sensor, so what carries its data? This section records the cleanroom answer
and the knob-gated groundwork that tests it at the bench.

**Cleanroom sourcing.** UnaOS is GPL-3.0-or-later; the Linux `bcm5974` driver is GPLv2-**only** and
therefore license-incompatible — it was **not** consulted. The reference used is FreeBSD
`sys/dev/usb/input/wsp.c`, the Wellspring touchpad driver, which carries
`SPDX-License-Identifier: BSD-2-Clause` (Copyright (c) 2012 Huang Wen Hui) — permissive, and lawful to
learn protocol facts from. No code is copied; only the following facts are used, all from wsp.c's
**TYPE2** parameter block, the generation covering the 2012 retina MacBook Pro (Wellspring 7,
`05ac:0262`):

| Fact (FreeBSD wsp.c, BSD-2-Clause) | Value | Bearing on our path |
| --- | --- | --- |
| feature-report size / request index / switch byte index | 8 / 0 / byte 0 | identical to what §10d already sends — **not** the discrepancy |
| raw-sensor ON selector | `0x01` | identical to `BCM5974_MODE_VENDOR` — the value was never wrong |
| HID/normal OFF selector | `0x08` | matches our own metal `GET_REPORT` readback `byte0=0x08` |
| switch **ordering** | write the OFF value **first**, then the ON value, with a pause between reading and writing | **we do neither** — §10d issues a single GET→SET with no OFF write and no pause |
| raw packet shape | bare header+fingers, **no leading Report ID byte**; TYPE2 header (offset to finger[0]) 30 B, finger record 28 B, so a legal frame is `30 + 28·n` bytes | our 8-byte `0x02` packets are the **HID-mode** shape, not the raw shape |
| driver receive buffer | 1024 B | a raw frame is far larger than our 64 B interrupt buffer — it will arrive **truncated** until that buffer grows |

**The working hypothesis this yields.** MT does *not* live on a different endpoint or interface (wsp
reads the same interface-1 interrupt IN we already read) and the raw-mode selector we send is already
the documented one. What differs is the **sequence**: the mode switch is issued as a bare single write.
A pad that ignored or reverted the switch keeps emitting the compatibility-mode 8-byte packets — exactly
what §10e observed. So the candidate answer is that raw mode never engaged, and the OFF→pause→ON
ordering is what engages it.

**The groundwork (knob-gated, `UNAOS_MTRAW=1`).** `bcm5974_mode_switch` dispatches to
`bcm5974_mt_raw_probe`, which:

1. writes the NORMAL selector `0x08` and reads the mode byte back;
2. pauses;
3. writes the RAW selector `0x01` and reads the mode byte back;
4. opens a capture window: the service loop hex-dumps at most **4** reports of at most **64** bytes each
   (`MT_RAW_DUMP_MAX` / `MT_RAW_DUMP_BYTES` — the FTDI console is a 64 KiB drop-oldest ring and the
   endpoint streams at ~100 reports/s, so an unbounded dump evicts the boot log that gives it context);
5. then restores the known-good pointer mode over EP0 (`bcm5974_mode_switch_normal`) so the trackpad
   keeps working for the rest of the sitting. A failed raw write restores immediately instead.

Every stage is non-fatal, exactly as §10d's handshake is: a stall logs and falls through to the restore.
The pointer decode still runs on captured reports — `decode_trackpad_rel`'s length + Report-ID gate
rejects anything that is not an 8-byte `0x02` report, so no input-safety clamp is loosened either way.

Witnesses:

```
:: EHCI-MT: [i] mode-try val=0x08 readback=8 addr=A intf=1 (step 1/2: normal) == witness ::
:: EHCI-MT: [i] mode-try val=0x01 readback=1 addr=A intf=1 (step 2/2: raw sensor) == witness ::
:: EHCI-MT: [i] raw-report #1 len=L bytes=.. .. .. == witness ::
:: EHCI-MT: [i] mode-restored addr=A intf=1 after 4 raw report(s) == witness ::
```

**Reading the capture.** `len=` is the load-bearing field:

- `len=8` with a leading `02` → still HID mode; the OFF→pause→ON ordering did **not** engage raw mode
  either, and the next candidate is a different interface/endpoint or a longer pause.
- `len=` pinned at the endpoint's max packet size, with **no** leading `02` → a raw frame arrived and our
  64 B interrupt buffer truncated it. The buffer must grow to `30 + 28·n` before the frame can be
  decoded — that is the follow-on arc, not this one.

**QEMU vs metal.** QEMU has no EHCI HID controller and never sets `vendor_mt` (its `usb-tablet` is a
standard absolute pointer), so **no QEMU gate can produce a Wellspring frame or reach this probe at
all**. The gates here prove only non-regression, and knob-off identity is measured the same way §10's
fold measured it: `.text`/`.rodata` of the release x86 kernel are **byte-identical** knob-off (ELF
section compare against the pre-arc tree — `.text` `35f4150a…`, `.rodata` `df4602f2…` on both), and the
EHCI report-parser + vendor-multitouch self-tests are unchanged-green. Whole-file identity is broken by
panic-`Location` line-number metadata and symbol-table shift, as it always is here — a lone comment line
added to an otherwise untouched `ehci/mod.rs` moves the whole-file size by itself (902 568 B →
902 624 B), which is why the section compare, not the file hash, is the gate. **Only the attended
2012 rMBP bench can verify** (a) whether the OFF→pause→ON sequence changes the readback, (b) whether the
streamed report length/shape changes, and (c) that after `mode-restored` the cursor still moves and
clicks still register.

#### 10g-2. MTRAW DECODE — receiving and decoding the raw TYPE2 frame

The probe arc above deliberately stopped at "the frame arrives truncated". This follow-on lands the two
things that were missing, so that the moment metal confirms raw mode engages, finger data flows on the
**next** boot rather than needing another code arc. Same `mtraw` knob; knob-off still byte-identical.

**How a >MPS report actually arrives on our interrupt-IN path.** Investigated and answered: **EHCI does
the reassembly in hardware, and no software reassembly (or transfer-layer change) is needed.** A qTD's
*Total Bytes To Transfer* field (EHCI 3.5.3) is not a packet size — the controller keeps issuing
MPS-sized IN transactions against the **same** qTD, advancing the buffer pointer, until either `total`
bytes have moved or the device returns a **short packet**, which retires the qTD and leaves the residue
in Total Bytes. Our single-qTD re-arm idiom therefore already supports multi-packet reports; what it did
*not* do was ask for more than one packet. Pre-arc, `arm_interrupt_ep` armed every endpoint with
`total = mps` (≤ 64), so the controller stopped after exactly one transaction and the rest of a raw
frame was dropped — precisely the truncation §10g predicted. Two changes fix it, both `mtraw`-gated:

- `qh::IntBuf` (the per-endpoint receive buffer inside the static `DmaPool`) grows from 64 B to
  **1024 B** — wsp's `WSP_BUFFER_MAX` — and is **1024-byte aligned**, so it can never cross a 4 KiB page
  and the single qTD buffer-page pointer still covers the whole transfer (`buf[1..5]` stay zero).
- the **vendor-multitouch endpoint only** is armed (and re-armed) with `total = INT_BUF_LEN` instead of
  `total = mps`. Every other endpoint — keyboard, boot mouse, plain report pointer — keeps `total = mps`
  verbatim. `report.len()` at the far end is then the true frame length, which is exactly the datum the
  decoder length-validates on.

**TYPE2 raw frame layout** (all offsets re-derived from FreeBSD `wsp.c`, SPDX BSD-2-Clause, (c) 2012
Huang Wen Hui; no code copied; the GPLv2-only Linux `bcm5974` driver was **not** consulted). The frame
carries **no** leading HID Report ID byte, so offsets are from byte 0. Header (`struct tp_header`,
packed, little-endian — 30 B, which is exactly `FINGER_TYPE2 = 15*2`):

| Off | Size | `wsp.c` field | Used here |
| --- | --- | --- | --- |
| 0 | 1 | `flag` | — |
| 1 | 1 | `sn0` | — |
| 2 | 2 | `wFixed0` | — |
| 4 | 4 | `dwSn1` | — |
| 8 | 4 | `dwFixed1` | — |
| 12 | 2 | `wLength` | — |
| **14** | 1 | `nfinger` | finger count (also `BUTTON_TYPE2 - 1`, the offset `wsp_intr_callback` reads it at) |
| **15** | 1 | `ibt` | integrated-button byte (`BUTTON_TYPE2 = 15`) |
| 16 | 12 | `wUnknown[6]` | — |
| 28 | 1 | `q1` | — |
| 29 | 1 | `q2` | — |

Finger record (`struct tp_finger`, packed, little-endian, every field `int16` — 28 B, which is exactly
`FSIZE_TYPE2 = 14*2`), repeated from offset 30 with `.delta = 0` for TYPE2:

| Off | `wsp.c` field | Used here |
| --- | --- | --- |
| 0 | `origin` | — |
| **2** | `abs_x` | X |
| **4** | `abs_y` | Y (reported verbatim; wsp negates it — `sc->pos_y[i] = -f->abs_y` — only for its pointer path) |
| 6 / 8 | `rel_x` / `rel_y` | — |
| 10 / 12 | `tool_major` / `tool_minor` | — |
| 14 | `orientation` | — |
| **16** | `touch_major` | touch state (`!= 0` == in contact, wsp's own test) |
| 18 | `touch_minor` | — |
| 20 | `unused[2]` | — |
| 24 | `pressure` | — |
| 26 | `multi` | — |

Frame-level rules taken from `wsp_intr_callback`: a frame is valid iff `len >= 30 + 28` **and**
`(len - 30) % 28 == 0`; the finger count is range-checked against `MAX_FINGERS = 16`.

**`decode_wellspring_type2` — bounded and hostile-input-safe.** The endpoint hands the decoder whatever
the device put on the wire, so every one of those rules is a clamp, not an assumption:

- the length gate above rejects anything that is not `30 + 28·n`. This is *also* what rejects the
  ordinary 8-byte Report-ID-`0x02` HID-mode report — a pad that never left HID mode decodes to `None`,
  never to garbage finger data.
- the frame's finger-count byte is never trusted: it is clamped to `MAX_FINGERS` **and, independently,
  to the number of records the frame's length can actually hold**. It can only ever name records that
  exist.
- every field read goes through `read_le16`, which returns `None` rather than reading past the end;
  there is no indexing a short frame could drive out of bounds.
- the decoder emits **no events and mutates no state**. Decode + witness only.

**Event injection is a second, separate knob.** `mtraw_inject` (`UNAOS_MTRAW_INJECT=1`, implies `mtraw`)
differences the first finger's absolute position into `pal::Event::Mouse` deltas (applying wsp's Y
negation, clearing the reference on finger-up so a lift never emits a jump, and clamping each delta to
±128 so a garbled coordinate cannot fling the cursor). It is **default OFF and expected to stay off**:
the live pointer path remains the 8-byte `0x02` stream until metal proves raw mode is stable.

Witnesses:

```
:: EHCI-MT: type2 self-test fingers=2 x0=1500 y0=-2000 touch0=90 button=0x01 ok=true
   (len=86 hdr=30 fsize=28; count-clamp=true hid-reject=true ragged-reject=true empty-ok=true) == witness ::
:: EHCI-MT: [i] type2 frame len=L mps=M fingers=N x0=X y0=Y touch0=T button=0xB == witness ::
```

The self-test runs once at driver init under `mtraw` and is the only QEMU-provable witness for the
decoder: it feeds a synthetic 30 + 2×28 = 86-byte frame built at the cited offsets, then the hostile
cases (a count byte claiming 255 in a two-record frame → clamped to 2; the real 8-byte `0x02` report →
rejected; a length with a partial trailing record → rejected; a well-formed all-lifted frame → accepted
with zero fingers). The live line is emitted for the same bounded first-`MT_RAW_DUMP_MAX` frames as the
hex dump, so it is ring-safe at ~100 reports/s. **`len` vs `mps` on that line is the load-bearing
evidence for the buffer growth**: `len > mps` means the controller accumulated a multi-packet frame into
the grown buffer — something the pre-arc `total = mps` arming could never produce.

**Gate reach — flagged.** `unaos/builder/src/main.rs` rebuilds the x86 kernel from its **own**
env-derived feature list, and that list does not map `UNAOS_MTRAW`. So `UNAOS_MTRAW=1 ./arroyo test`
builds the QEMU media *without* `mtraw` and the self-test never runs in the battery (`arroyo` pushes the
feature to its own kernel build, which the builder then overwrites). Verified by adding the two-line
mapping locally, at which point the self-test emits `ok=true` with every clamp green; the local edit was
reverted rather than landed because `builder/` is outside this arc's lane. The integrator should add:

```rust
if std::env::var("UNAOS_MTRAW_INJECT").is_ok() { feats.push("mtraw_inject"); }
else if std::env::var("UNAOS_MTRAW").is_ok() { feats.push("mtraw"); }
```

**Knob-off identity.** `.text` `2fa55ead…` / `.rodata` `60f3a313…` — byte-identical to the pre-arc tree
(ELF section compare of `target/x86_64-unaos/release/unaos-kernel`, default feature set). Getting there
required care worth recording: the natural refactor of hoisting the armed-total into one local changes
`service_ehci_hid`'s codegen even when the knob-off *value* is identical (first +16 B, then a
same-size register-allocation swap). The landed shape uses `#[cfg]` **pairs** whose knob-off member is
the original expression verbatim and in place. Repetitive, deliberately.

**What metal must still verify.** Everything §10g listed, plus: that a raw frame's `len` on the witness
line exceeds `mps` (multi-packet accumulation actually happening), that `(len - 30) % 28 == 0` holds on
the real stream, that `fingers` tracks the number of fingers actually on the pad, and that `x0`/`y0`
move in the expected directions and ranges as a finger crosses the pad.

---

## 11. IVY — the delete-behind-a-hub investigation (BOT pump headroom)

**The metal fact under investigation** (rmbp track, 2026-07-17 three-leg sitting).
With the SD card reader on a **direct root port**, the whole storage chain passes
(S3–S9 + create/write/grow/delete). With the same reader **behind a hub**
(route `0x2`), create/write/grow and S3–S9 still pass, but the **DELETE family**
(`U10d`, `U11m2`) fails `deleted_ok=false` with a **BOT-pump TIMEOUT**.

### What the delete path does differently

Delete is not a different kind of I/O — it is *more of the same* I/O, in the
longest unbroken run in the chain. `fat::delete_located` is
`chain_clusters` (a read-only pre-validation walk) → `mark_dir_deleted` (one
directory-sector read-modify-**write**) → `free_chain`, and `free_chain` calls
`set_fat_entry` **once per cluster**, each of which is a read+write of the FAT
sector in **every FAT copy** (`num_fats`, normally 2). So an *N*-cluster file costs
roughly `N + 1 + 4N` single-sector BOT transactions, each of which is a full
CBW → \[DATA\] → CSW round trip with **two** synchronous pump waits (the DATA stage
and the CSW stage). Create/write/grow touch the FAT too, but they do not walk and
re-write a whole chain. Delete is therefore the first operation to expose any
per-transfer latency the pump budget cannot absorb — which is exactly what a
"fails only behind a hub" signature looks like.

### QEMU reproduction: NEGATIVE

`UNAOS_HUBSTORAGE=1 ./arroyo test-fat sf 300` puts the usb-storage behind a
`usb-hub` (slot 2, `route=0x1 depth=1`) and runs the full chain. **Both delete
witnesses PASS behind the hub, 0 FAIL** — byte-identical verdict lines to the
direct-attach run. The metal failure does **not** reproduce in emulation.

### Audit verdict: the pump budget is not the cause

The brief's leading hypothesis was an **iteration** budget that suffices direct
but starves behind a hub (more hops per transfer). Audited and **refuted for the
BOT path**: `pump_until_bot_done` has been a **wall-clock** `now_cycles` deadline
of `hw_wait_budget() * 3` since the tegra timerless-core work — it measures real
time, not yields, so extra hub hops cannot shrink it.

The instrumentation added here quantifies how far from the cause it is. Behind
the hub, over 2691 pump waits across the whole storage chain:

```
:: BOT: pump budget=14396988690 peak=32724164 n=2691 timeouts=0 storage_slot=2 route=0x1 depth=1 result=SUMMARY ::
```

The worst single wait used **32.7 M cycles against a 14.4 G budget — ~440x
headroom**, with the direct-attach run peaking at 35.2 M against the same budget.
Hub routing moved the worst case by *noise*, not by orders of magnitude. A budget
with 440x headroom is not the thing that timed out on metal.

**So what did?** The instrumentation is built to answer that at the next sitting
rather than guess now. On a timeout the pump prints `used` beside `peak`:

- `used == budget` while `peak` sits orders of magnitude below it ⇒ the completion
  event was **lost** (a wedged endpoint / a dropped Transfer Event), not a tight
  budget. Raising the budget would change nothing.
- `peak` creeping up toward `budget` across the run ⇒ genuine latency growth, and
  the budget (or per-hop scaling) is the right lever.

Everything currently known points at the first branch, which is consistent with
the sitting having logged this as a **reseat watch-item**. No driver-side cause
for the delete-behind-hub failure was found; the honest landing is
**instrumentation + the two bound conversions below**, not a speculative fix.

### Known gap — CLOSED by §11a below

*(Historical statement of the gap, kept because §11a is its answer.)*
There was **no BOT error recovery**. `run_bot_stage` returns `Err` on a timeout and
nothing resets the endpoint (no Reset Endpoint / Set TR Dequeue, no Bulk-Only Mass
Storage Reset, no CLEAR_FEATURE(ENDPOINT_HALT)), and `block.rs` does not retry. A
single marginal timeout is therefore **terminal and desynchronising**: the stage's
TRB stays queued, the device is left mid-BOT, and later transactions see tag
mismatches. That plausibly turns *one* hiccup behind a hub into a whole failing
delete family. Closing it needs Reset-Endpoint + Set-TR-Dequeue plumbing that
**no gate in this repo can exercise** (QEMU never times out — `timeouts=0` in every
run above), so it was recorded as the next storage arc rather than landed
blind. **That arc is §11a.**

### Changes

- `pump_until_bot_done` / `note_bot_pump` (`drivers/xhci/mod.rs`) — per-wait cycle
  accounting, a high-water mark, and the `:: BOT: … ::` witness. The OK witness
  prints only when the peak **doubles** the last reported peak, so the line count is
  logarithmic in the budget (5 lines across 3949 transactions) and the default-quiet
  boot stays quiet, while the last such line still carries the true worst case.
  `route`/`depth`/`slot` make a direct-attach log and a behind-hub log diffable.
- `pump_until_ep0_done` / `pump_until_cmd_done` — converted from raw **2000-iteration**
  budgets to `now_cycles` wall-clock deadlines. These were the last iteration budgets
  on the path to a hub-routed block device (`run_command_sync` carries hub bring-up's
  ENABLE_SLOT / ADDRESS_DEVICE / CONFIGURE_ENDPOINT; `sync_control` carries the
  descriptor fetches and the storage SET_CONFIGURATION). An iteration budget bounds
  *yields*, whose duration depends entirely on what `hlt()` does — so the brief's
  per-hop-starvation hypothesis, false for BOT, was **live here**. Both remain
  strictly bounded.

### Witnesses

```
:: BOT: pump budget=… used=… peak=… route=… depth=… slot=… n=… timeouts=… result=OK ::
:: BOT: pump budget=… used=… peak=… route=… depth=… slot=… n=… timeouts=… result=TIMEOUT ::
:: BOT: pump budget=… peak=… n=… timeouts=… storage_slot=… route=… depth=… result=SUMMARY ::
```

### PH-6 — the FTDI TX pump joins the wall-clock regime

`pump_until_ftdi_done` (the bulk-OUT pump behind the USB-serial console) was the last
**2000-iteration** budget in `drivers/xhci/mod.rs`. It is now a
`now_cycles`/`hw_wait_budget()` deadline taken at entry, exactly as `pump_until_bot_done`
does it, and its caller `ftdi_tx_stage` no longer passes an iteration count. The
one-shot `xHCI: FTDI TX pump TIMEOUT after N yields` line — which reported yields, a
quantity with no fixed duration — is replaced by the witness pair below: a completed
wait prints on a peak that at least DOUBLES the last reported one (the same log-scale
throttle `note_bot_pump` uses, so console traffic cannot flood a default-quiet boot),
and a timeout prints unconditionally.

Read `used` against `budget` the same way as for BOT: `used == budget` with a low
running peak says the completion event was **lost**; a peak sitting just under `budget`
says the budget was **tight**. Without this pair a GUI-media boot cannot distinguish a
starved FTDI pump from a dead one — which is why it lands before that boot.

```
:: FTDI: tx pump budget=… used=… n=… timeouts=… result=OK ::
:: FTDI: tx pump budget=… used=… n=… timeouts=… result=TIMEOUT ::
```

### What metal must verify

Re-run the behind-hub leg and read the `:: BOT: … ::` lines. If the delete family
fails again, the timeout line's `used` vs `peak` decides lost-completion vs
tight-budget (above) — that is the whole point of this arc. If it passes after a
reseat, the watch-item closes as a contact/reseat issue with the headroom numbers
on record.

### Gates (IVY)

`./arroyo check` green both arches. `./arroyo test 120` 0 FAIL.
`UNAOS_IRQSTORAGE=1 UNAOS_FATIMG=sf ./arroyo test 200` 0 FAIL. `UNAOS_HUBSTORAGE=1
./arroyo test-fat sf 300` 0 FAIL, `U10d` + `U11m2` both PASS behind the hub.
`./arroyo test-arm 120` 0 FAIL.

---

## 11a. BOT-RECOVER — Reset Recovery and one bounded retry

The gap §11 recorded, closed. `bot_transfer` is now a thin wrapper: it runs the
transaction once (`bot_transfer_once` — the unchanged CBW -> [DATA] -> CSW body), and
on **any** failure other than `NoDevice` it runs Reset Recovery and retries the
transaction **exactly once**. Recovery failure falls through to the pre-existing
terminal `Err`, semantics unchanged.

### The sequence (spec-cited)

| # | Step | Citation |
|---|------|----------|
| 1 | **Bulk-Only Mass Storage Reset** — bmRequestType `0x21` (host->device, class, interface), bRequest `0xFF`, wValue 0, wIndex = `bInterfaceNumber` of the MSC interface, wLength 0. Returns the *device's* Bulk-Only state machine to "ready for CBW". | USB MSC **Bulk-Only Transport 1.0 §3.1**; invoked as part of Reset Recovery, **§5.3.3** |
| 2 | **CLEAR_FEATURE(ENDPOINT_HALT)** on the bulk-IN and bulk-OUT endpoints — bmRequestType `0x02`, bRequest `0x01`, wValue 0 (`ENDPOINT_HALT`), wIndex = endpoint address. §3.1 explicitly does *not* clear the halts, which is why this is a separate step; it also resets the device-side data toggle. | **BOT 1.0 §5.3.3** steps 2-3, **§5.3.4** (stall handling); **USB 2.0 §9.4.1** |
| 3 | **Stop Endpoint** (Running) or **Reset Endpoint** (Halted/Error) per bulk endpoint, then **Set TR Dequeue Pointer** to the driver's own enqueue pointer. | **xHCI 1.2 §4.6.9 / §4.6.8 / §4.6.10**; EP State read from the output Endpoint Context dword 0 bits 2:0, **§6.2.3** |

Step 3 is not optional garnish: the USB-level reset in steps 1-2 is **invisible to the
host controller**. Its endpoint contexts are still Halted or Running and its dequeue
pointers still sit on the stranded TRBs of the failed transaction, so without it the
retry would either be swallowed (a Halted EP ignores the doorbell) or would replay the
stranded CBW/data/CSW at a device that has just been reset. The EP-state read exists
because both commands are legal only from particular states — issued blind they return
Context State Error (code 19). A plain timeout — the metal failure mode this targets —
leaves the endpoint **Running**, so it takes the Stop-Endpoint arm, not the Reset arm.
`TransferRing::enqueue_ptr_dcs()` supplies the new dequeue pointer with the ring's
current cycle bit as DCS, restoring `controller-dequeue == driver-enqueue`.

Every step is one bounded `sync_control` / `run_command_sync`. There is **no loop
anywhere** in the recovery, and the retry count is a hard 1. `bot_recover` never calls
`bot_transfer`, so recursion is structurally impossible.

### Why retrying a WRITE is safe for FAT

The retry is at the **CBW boundary**: `bot_transfer_once` rebuilds the CBW with a fresh
`dCBWTag` and re-issues the identical CDB against the identical DMA buffer.

- READ(10) is trivially idempotent.
- WRITE(10) re-sends the **same bytes** from the **same** `scsi_data_buffer` to the
  **same** LBA. `block.rs::write_block` stages the caller's buffer into that DMA buffer
  and then calls `storage_write10`; the FAT layer's single-sector read-modify-writes
  merge into that staged sector *before* the first attempt. Nothing between the two
  attempts re-reads the media or re-derives content, so both attempts are byte-identical
  whole-sector writes. Repeating an identical whole-sector write is idempotent by
  construction — the hazard a naive retry would introduce (a partially applied write,
  then a retry recomputed from the now-changed media) cannot arise here.

The retry seam is therefore **inside** `bot_transfer`, not in `block.rs`: that is the
only layer where the whole-CBW idempotence argument holds. `block.rs` is unchanged.

### Fault injection (`UNAOS_BOTFAULT=1`)

QEMU never times out a BOT transfer (`timeouts=0` in every run), so the recovery path
would otherwise be metal-only and unexercised. The `botfaultinject` feature injects
**exactly one** synthetic failure: at the CSW stage of the **first WRITE(10)**
(deterministic — after storage bring-up and after the FAT mount). The data stage really
lands first, so the device is genuinely left parked in its CSW phase with a stale CSW
pending. That makes the run a real assertion rather than a smoke test: had Reset
Recovery *not* resynchronised the device, the retry's fresh CBW would collect the stale
CSW and fail on the tag mismatch. Default OFF and fully `#[cfg]`-compiled out —
knob-off media are byte-identical. **Test-only; never on boot media.**

### Changes

- `bot_transfer` split into a recovery/retry wrapper + `bot_transfer_once`
  (`drivers/xhci/mod.rs`).
- `bot_recover` + `resync_bulk_ep` (`drivers/xhci/mod.rs`) — the sequence above.
- `DeviceSlot::msc_intf` — `bInterfaceNumber` of the MSC interface, recorded on both the
  root-port and hub-downstream discovery paths (`parse_msc_config` now returns it). It is
  the `wIndex` of the class reset; recovery is not issuable without it.
- `TransferRing::enqueue_ptr_dcs()` (`drivers/xhci/ring.rs`) — the Set TR Dequeue value.
- `BOT_RECOVER_COUNT` / `BOT_RECOVER_OK` / `BOT_RETRY_OK` / `BOT_RETRY_FAIL` counters, so
  metal can read recovery **frequency** and not just presence.
- `botfaultinject` feature + `UNAOS_BOTFAULT` knob (`arroyo`, `builder`, kernel
  `Cargo.toml`).
- BOTEV (instrumentation, no behaviour change): `ep_state_of` / `ep_ctx_deq` /
  `recover_cmd` (`drivers/xhci/mod.rs`) — bounded context reads and the
  `(ok, cc, why)` rendering of one recovery command; `TransferRing::contains`
  (`drivers/xhci/ring.rs`) — names the pipe a stranded TRB belongs to;
  `io_cause_witness` (`drivers/block.rs`) — the once-per-direction concrete cause behind
  `BlockError::Io`.

### Witnesses

```
:: BOT: recover begin cause=… slot=… ep=…/… iface=… n=… ::
:: BOT: recover entry epin=… epout=… indci=… outdci=… cmdring=running|stopped ::
:: BOT: recover stage=msc-reset iface=… ok=yes|no cc=… why=… epin=…->… epout=…->… ::
:: BOT: recover stage=clear-halt ep=0x… dci=… ok=yes|no cc=… why=… epstate=…->… ::
:: BOT: resync stage=reset-ep|stop-ep|skip dci=… dir=in|out ok=yes|no cc=… why=… epstate=…->… ::
:: BOT: resync stage=set-deq dci=… dir=in|out ok=yes|no cc=… why=… epstate=…->… want=0x… ctxdeq=0x…->0x… ::
:: BOT: resync stage=read-state dci=… dir=in|out ok=no why=no-output-context|ep-unusable[ epstate=…->…] ::
:: BOT: resync note dci=… dir=in|out illegal-reset-on-state|illegal-stop-on-state read=… now=… — Context State Error on Reset Endpoint|Stop Endpoint ::
:: BOT: recover done reset=ok|fail halts=cleared|fail ring=resync|fail ::
:: BOT: recover evidence cause=… pipe=in|out|unknown|none wait_trb=0x… stage_done=yes|no stage_cc=… csw_sig=0x… csw_tag=0x… residue=… csw_status=… epin=… epout=… ::
:: BOT: retry result=pass|fail … recoveries=… retry_ok=… retry_fail=… ::
:: BOT: fault-inject synthetic CSW-stage failure (slot …, once) ::   (UNAOS_BOTFAULT only)
:: BLK: io-cause op=read|write|read-usb|write-usb lba=… bot_err=… (first, once) ::
:: BLK: io-cause op=read|write|read-usb|write-usb lba=… csw_status=… residue=… (first, once) ::
```

Quiet boot: **zero** of these lines when nothing fails — every one is printed only from a
failure path, and the counters stay at 0, so any non-zero reading is itself the finding.

**BOTEV — reading the evidence fields.** The 2026-07-26 rMBP capture reported
`recover done reset=fail halts=fail` with no way to tell *why*; the stage lines above close
that gap without touching the recovery sequence.

- `epstate` / `epin` / `epout` — the EP State field of the endpoint's OUTPUT context
  (xHCI 1.2 §6.2.3: `0`=Disabled `1`=Running `2`=Halted `3`=Stopped `4`=Error, `255`=no
  output context), sampled immediately **before** and **after** each stage. A stage that
  reports `ok=yes` but leaves the state unchanged is doing nothing.
- `cc` / `why` — the outcome of the one bounded command or control transfer the stage
  issues. `why=ok` (cc 1 = Success); `why=cc-error` — it completed with an error code, and
  `cc` names it (`19` = Context State Error, i.e. the command was illegal from the state
  the controller actually held); `why=nocompletion` — **no completion event arrived inside
  the wall-clock budget**, so `cc` is meaningless and the ring/pipe is not being consumed;
  `why=cmdring-stopped` — the command ring is parked by an abort in progress and the
  command was never pushed.
- `ctxdeq=A->B` on `set-deq` — the controller's own TR Dequeue Pointer from the output
  context, before and after, against the `want=` value the command carried. Proves whether
  Set TR Dequeue *moved* the controller or merely returned Success.
- `recover evidence` — printed **once**, only when `recover done` reports any `fail`. It
  carries the failed transaction's own state: `pipe`/`wait_trb` = which bulk ring held the
  stranded TRB the timed-out stage was waiting on; `stage_done`/`stage_cc` = whether that
  stage ever reported a completion; `csw_sig`/`csw_tag`/`residue`/`csw_status` = the CSW
  buffer as the controller left it. A CSW signature of `0x53425355` here means the status
  actually landed and only its event went missing — a different fault from a pipe that
  never answered (signature `0x0`, the pre-transfer zero fill).
- `:: BLK: io-cause … ::` — the concrete SCSI/BOT reason for a block failure, latched to
  the first read failure and the first write failure of a boot. `BlockError::Io` and
  `FatError::Io` discard it one frame below, which is why the flight recorder could only
  say `:: FR: UNAOS.LOG reservation failed (Io) ::`. Pure logging: the returned error is
  unchanged.

All of this is instrumentation only — no retry was added, no wait was lengthened, and the
command sequence is byte-for-byte the one described above. The `resync note` lines record
the documented illegal-command-for-state case as *evidence*; correcting the sequence is a
later arc, gated on an evidence boot.

### What metal must verify

1. Re-run the delete family behind the hub on the 2012 rMBP. If a `:: BOT: recover
   begin ::` appears at all, the standing watch-item finally has a *cause* line — and if
   it is followed by `recover done reset=ok halts=cleared ring=resync` +
   `retry result=pass`, the hiccup was survived instead of cascading.
2. Confirm the **Stop-Endpoint** arm is what a real timeout takes (EP State 1 = Running,
   not 2 = Halted). Only metal can produce a real timeout.
3. Confirm a real device accepts the Bulk-Only Mass Storage Reset on its reported
   interface number (`iface=` in the begin witness) — QEMU's `usb-storage` accepts it on
   interface 0; the rMBP's hubbed SD reader is the case that matters.
4. Confirm the recovery's own control transfers do not themselves time out on a device
   that is already sick (worst case adds ~3 EP0 budgets + 4 command budgets before the
   terminal `Err` — bounded, but slower to fail than before).

## PH-2 — runtime CHECK CONDITION (sense + one retry)

Reset Recovery above handles **transport** faults — the `Err` arms of
`bot_transfer_once`. It never fired on the other failure family: a `Failed` CSW. That is a
transaction that *completed* and which the **device** rejected (SCSI CHECK CONDITION,
SPC-4 §4.5), leaving sense data pending. `bot_transfer` returned such a result verbatim
(`Ok(r) => return Ok(r)`), and the only `REQUEST SENSE` in the driver was the one inside
the bring-up TEST UNIT READY loop. A device that entered CHECK CONDITION at **runtime**
therefore failed every subsequent command, with nothing in the log saying why. The
exposure is not theoretical: the flight recorder issues a ~128-sector WRITE(10) burst on
the boot volume on every x86 boot.

`bot_transfer` now routes a `Failed` CSW to `bot_check_condition`:

1. **One** `REQUEST SENSE`, logged as the `sense` witness below.
2. **One** retry of the original command — but only if the sense key is `0x6`
   (UNIT ATTENTION: media/reset state change) or `0x2` (NOT READY: becoming ready). Every
   other key is a real rejection (ILLEGAL REQUEST, MEDIUM ERROR, DATA PROTECT …) that a
   retry would only repeat, so the failure propagates exactly as before.

Bounds and boundaries:

- `BOT_SENSE_ACTIVE` latches for the whole handler. Both the sense fetch and the retry go
  back through `bot_transfer`, so without the latch a device that answered `Failed` to its
  own sense command would recurse; with it, any nested `Failed` propagates as it did
  before this arc. **One sense, one retry, all-or-nothing** — there is no loop.
- The retry is gated on a `now_cycles`/`hw_wait_budget()` **wall-clock** deadline taken at
  entry (never an iteration count): if the sense fetch alone consumed the budget, the
  failure propagates rather than spending more of the caller's time.
- The retry goes through `bot_transfer`, so `recover_bot_full` stays in charge of any
  transport fault the retry hits. Sense + retry happen **before** — and independently of —
  Reset Recovery; none of its logic is duplicated.
- The **bring-up TUR loop is unchanged**: it already *is* a sense-and-retry loop, so it
  holds the latch across itself and keeps propagating `Failed` verbatim. The new handler
  owns the runtime path only.
- `REQUEST SENSE` DMAs into the single per-slot staging buffer, which on a WRITE(10) still
  holds the caller's payload. The handler saves and restores the 18 bytes it lands in —
  the "retry re-sends byte-identical data" invariant above is what makes that mandatory.

### Fault injection (same `UNAOS_BOTFAULT=1` knob)

QEMU's `usb-storage` never rejects a well-formed command, so this path would be metal-only
too. The knob now injects a **second, independent** synthetic failure, also exactly once:
a `Failed` CSW on the first IN transaction of ≥512 B — a READ(10), so bring-up cannot trip
it (INQUIRY is 36 B, READ CAPACITY 8 B, REQUEST SENSE 18 B) and the first hit is the FAT
layer's runtime read. Only the *decoded* status is rewritten; the transaction itself
really completed, so the device is healthy and the retry must pass. Because the device's
real sense is then NO SENSE, the handler rewrites it once to UNIT ATTENTION
(`0x6 / 0x28 / 0x00`) so the **retry** leg is exercised and not just the fetch leg. Both
injections are `#[cfg]`-compiled out by default — knob-off media are byte-identical.
**Test-only; never on boot media.**

### Witnesses

```
:: BOT: sense key=… asc=… ascq=… ::
:: BOT: sense result=fail err=… ::
:: BOT: sense-retry result=pass residue=… sense_n=… retry_ok=… retry_fail=… ::
:: BOT: sense-retry result=fail status=…|err=… sense_n=… retry_ok=… retry_fail=… ::
:: BOT: sense-retry result=skip key=… ::
:: BOT: sense-retry result=skip reason=budget key=… ::
:: BOT: fault-inject synthetic Failed CSW (slot …, once) ::        (UNAOS_BOTFAULT only)
:: BOT: fault-inject synthetic sense UNIT ATTENTION (once) ::      (UNAOS_BOTFAULT only)
```

Expected `UNAOS_BOTFAULT=1` sequence: `fault-inject synthetic Failed CSW` →
`fault-inject synthetic sense UNIT ATTENTION` → `:: BOT: sense key=0x6 asc=0x28 ascq=0x0 ::`
→ `:: BOT: sense-retry result=pass … retry_ok=1 retry_fail=0 ::`.

Quiet boot: **zero** of these lines when nothing fails; `BOT_SENSE_COUNT` /
`BOT_SENSE_RETRY_OK` / `BOT_SENSE_RETRY_FAIL` stay at 0, so any non-zero reading is itself
the finding.

### What metal must verify

1. On the 2012 rMBP, whether the flight recorder's WRITE(10) burst ever produces a
   `:: BOT: sense … ::` line at all — and with which key. That is the first time the
   sense of a runtime rejection has been readable.
2. That a UNIT ATTENTION raised by a real card reader (media change / port reset) is
   survived by the single retry rather than wedging the volume.

---

## See also
- `unaos/crates/kernel/src/drivers/xhci/`, `drivers/block.rs` — the implementation.
- `unaos/crates/kernel/src/drivers/ehci/`, `drivers/ehci_scout.rs` — the EHCI-3 HID driver (§10) and the EHCI-1/2 scout + shared wake (§9/§9a).
- [`scheduler.md`](../02_KERNEL_CORE/scheduler.md) — why the lock-free MSI handler and the main-loop service split matter under a live scheduler.
