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

### 2d. The pre-CCS-scan settle — 500 ms → 150 ms (M4) → conditional 150/100 ms (CCSTRIM)

`start()` powers every root port, waits, and only then samples CCS to build the
initial enumeration queue. The wait exists because a boot-owned USB3 device whose
SuperSpeed link dropped on our `HCRST` needs time to re-train
(RxDetect → Polling → U0); the pre-settle code read CCS immediately and a real
USB3 stick was missed at scan time. USB2 keyboards and mice re-detect fast enough
to be caught without it — the USB3 stick was not.

M4 changes two things about that wait.

**Timebase.** It was `hw_wait_budget() / 4`. `hw_wait_budget()` is a *policy*
number — "how long may a wedged handshake burn before we call it dead" — and
`cycles_per_ms`'s own doc-comment already forbids tying spec settles to it,
because the day the timeout policy changes, every USB timing constant silently
rescales. It also made the settle's real wall clock arch-dependent and
unprintable: ~500 ms on a calibrated x86, ~694 ms on the Pi's fixed 150 M-tick
budget. It is now `settle_ms * cycles_per_ms()` — the same nominal number on both
arches, and `settle_ms=` can state the value that actually ran. (M4 spelled that
local `SETTLE_MS`, a single constant; CCSTRIM made it a local chosen between
`SETTLE_PP_APPLIED_MS` and `SETTLE_PRE_POWERED_MS`, so the old name no longer
exists anywhere in the driver.)

**Length: 150 ms** (M4; now conditional — see CCSTRIM below). USB3 link training
reaches U0 in tens of ms typically; the spec's outer bound is
`tPollingLFPSTimeout` = 360 ms (USB 3.2 §6.9). That outer bound is now enforced
where it belongs — see below — instead of by padding the settle, so the common
case (link already trained, or a machine with no USB3 device attached) pays
150 ms rather than 500 ms.

**The 360 ms floor moved into the Polling debounce.** The scrub loop that runs
straight after the settle collects USB3 ports sitting at `CCS=0, PLS=Polling`,
waits, re-reads, and warm-resets the ones still there (the Panther Point
stuck-in-Polling erratum). A healthy link may *legitimately* be in Polling for up
to 360 ms, so declaring one stuck before then would warm-reset a link out of a
legal state. The old pairing satisfied that only by accident (500 ms settle +
100 ms debounce = 600 ms). It is now explicit: the debounce is
`POLLING_DECIDE_MS (360) − settle_ms`, i.e. whatever is left of the spec window,
so shortening the settle cannot shorten the Polling verdict. A link that finishes
training during that window reads `CCS=1` at the re-check, is *not* warm-reset,
and is picked up by the CCS scan in the ordinary way.

**Why a short settle cannot lose the device.** Two backstops, both pre-existing:

- the CAS / warm-reset rungs immediately after the settle (`CAS=1`, or
  `PLS ∈ {Inactive, Compliance}`) kick a link a hot reset cannot recover, and the
  resulting `CSC`/`PRC` flows through `handle_port_status`;
- a late `0 → 1` CCS edge latches `CSC` *after* the scrub has cleared every change
  bit (so `PSCEG` is armed), the controller posts a Port Status Change Event, the
  polled main loop drains it, and `handle_port_status` queues the port.

A slow device therefore enumerates **later**, not never.

**Metal tell-tale of a regression here.** The boot device arriving via the
CSC / warm-reset path instead of the initial scan. On a healthy boot the capture
reads `xHCI: Port N connected (Status: …); queued for enumeration.` immediately
after `xHCI: port settle complete before CCS scan (settle_ms=<N>)`. If instead the
boot device's port shows up through
`xHCI: [Port N] unsolicited reset complete; queuing for enumeration.` or a plain
hot-plug `CSC` line — or if `xHCI: USB3 port N stuck in Polling (debounced);
warm-resetting.` names the boot port — the settle is too short for this machine
and the settle must go back up — which since CCSTRIM means the branch named by
the capture's own `/on`|`/pre` markers, `SETTLE_PP_APPLIED_MS` or
`SETTLE_PRE_POWERED_MS`, not one shared number. `settle_ms=` is printed live precisely so a
capture can be dated against the constant that produced it. Since CCSTRIM that
tell-tale is no longer something to spot by eye: see `CCSMARGIN-LATE` below.

Saving: ~350 ms of the time to first console, on a boot whose dominant cost is
elsewhere (see `docs/dev/OS/01_BOOT_HAL/bootpace.md` §6a) — the point of doing it
now is that it is spec-justified on its own terms, not that it is the big win.

#### The CCSMARGIN witness — how much headroom the settle actually has

Until CCSMARGIN, **the settle constant was covering a phenomenon that had never
been measured on either arch**: nothing anywhere recorded *when* CCS asserts, so
every capture on x86 and on the Pi showed only that CCS *had* asserted by the end
of the wait, and the sole failure signal was the pass/fail tell-tale above, after
the fact, with no number attached. Nobody could say whether the then-current
150 ms sat on
comfortable margin or one slow device away from missing a port — and the two
seats do not even ask the same question of it. x86 asks "has a USB3 link finished
Polling"; the Pi asks "has the VL805's firmware booted far enough to present its
root ports", a bring-up observed in the hundreds of milliseconds. One constant
covers both.

The settle now samples `PORTSC` once per millisecond while it waits and records,
per port, the elapsed millisecond at which `CCS` first reads 1. One line follows
the `xhci-settle` stamp:

```
xHCI: ccs-margin settle_ms=150 ppc=1 ports=4 first_assert_ms=[p1:0/pre p2:none/on p3:none/on p4:87/on] latest=87 margin_ms=63 result=CCSMARGIN
```

- every root port `1..=MaxPorts` is listed, so the absent ones are stated rather
  than implied;
- **`none` ≠ `0`.** `none` is "no `CCS=1` observed by the deadline". `0` is a real
  measurement — the port already read `CCS=1` when the settle began, i.e. the
  device was attached before the machine was powered and never needed the wait.
  `p1:0` is common and benign, **not** an error;
- **`ppc=`** (CCSTRIM) — `HCCPARAMS1` bit 3, Port Power Control (xHCI 5.3.6).
  `ppc=0` means the controller implements no port power switching at all: `PP`
  reads 1 permanently and VBUS is never removed, so every `/pre` below is an
  *observation*. `ppc=1` with `/pre` is the weaker claim — `PP` survived our
  `HCRST`, which strongly implies VBUS did too, but no register says so;
- **`/on` vs `/pre`** (CCSTRIM) — did *we* apply `PP` to this port, or did we find
  it already powered by firmware? This decides what the settle's clock means for
  that port **and which settle value the boot runs**. On an `/on` port VBUS really
  did transition, so USB 2.0's `TSIGATT`
  attach allowance is genuinely running and a nonzero `first_assert_ms` is
  attach signalling. On a `/pre` port VBUS has been valid since firmware, the
  allowance expired long before the kernel existed, and the same number is
  measuring something else entirely. Without this marker `latest=` is not
  interpretable, and GR11's reading was not;
- `margin_ms` = `settle_ms − latest`, signed and raw. This is the whole point: it
  turns "did it work" into "by how much".

**Three readings that must differ** (instrument-baseline law):

| situation | line reads |
|---|---|
| healthy boot | every connected port a small `first_assert_ms`, `latest` well under `settle_ms`, `margin_ms` comfortably positive |
| mechanism did not run — nothing plugged into any root port | every port `none`, `latest=none`, `margin_ms=none` |
| the failure the settle exists to prevent | `margin_ms` **0 or negative** — the final at-deadline sweep was the first sample to see `CCS=1`, so the port reached the initial scan with no headroom |

`margin_ms=none` is deliberate. Printing the whole budget when zero ports were measured would
report maximum apparent headroom from no measurements at all — the instrument
lying about its own baseline.

A port asserting *later* than the deadline cannot be timed by an instrument that
stops at the deadline: it reads `none`, indistinguishable here from an empty port,
and is disambiguated by the tell-tale above (that device arrives through the
CSC / warm-reset path). The in-loop samples can only ever report
`elapsed < settle_ms`, i.e. a strictly positive margin, which is why one final
sweep runs *at* the deadline — an instrument that cannot print its own failure is
not an instrument.

Two things the witness deliberately does **not** do:

- **It does not touch the settle.** The `while` condition, its timebase, its exit
  and everything after it are byte-identical to M4's; the sampling reuses the same
  `cycles_per_ms()` and only spends, inside the busy-wait, time that used to go to
  `spin_loop()`. The diff that introduced it deletes no line of the settle. An
  all-ones `PORTSC` (`0xFFFF_FFFF`, the PCIe no-response pattern, not a legal
  register value) is discarded rather than read as `CCS=1` — the false positive a
  VL805-behind-PCIe seat is exposed to.
- **It adds no BPACE stamp.** The ring stores `(cycle, tag)` and a stamp's value
  *is* the instant `record()` was called; the latest assert is only identifiable
  after every port has been swept, so it cannot be stamped when it happened, and a
  stamp placed at the end of the settle would read ~`settle_ms` on every boot
  forever — the recorder, not the phenomenon. M4's ring arithmetic (n=31 of
  CAP=64) is untouched and `dropped=` stays 0.

Arch-neutral by construction — no `cfg(target_arch)`, no second timebase — so the
Pi 4 track gets the same measurement of its own VL805 for free at merge.

#### CCSTRIM — spending the margin, and making the spend falsifiable

**Seven metal boots**, not one: `rmbp-gr11/ttyUSB1.log` ×3 and
`rmbp-gr12/ttyUSB0.log` ×4 all carry the CCSMARGIN line, all byte-identical —

```
xHCI: ccs-margin settle_ms=150 … latest=21 margin_ms=129 result=CCSMARGIN
```

— with `Max Ports = 8`, all eight ports already powered (`status 0x2a0`), and
zero variance across the seven. `polling_candidates` is empty on every one, so
the Polling debounce never runs and any trim is fully realised as boot time on
this machine. 129 ms of a 150 ms budget unused, and GR11's note "safe with 7×
headroom, could go lower". Going lower on that reading is still a trap, for
reasons the reading itself cannot show.

**What the wait is actually made of.** Two phenomena were folded into one
constant:

| | phenomenon | outer bound | where it is enforced |
|---|---|---|---|
| (a) | port power → USB 2.0 device signals attach | `TSIGATT` = **100 ms from `VBUS_min`** (USB 2.0 Table 7-14) | here, and only here |
| (b) | USB3 link training `RxDetect → Polling → U0` | `tPollingLFPSTimeout` = 360 ms (USB 3.2 §6.9) | `POLLING_DECIDE_MS − settle_ms`, *not* here |

M4 already moved (b) out: the Polling debounce is defined as `360 − settle_ms`,
so the USB3 verdict window is 360 ms from port power **whatever this constant
is**. (b) places no floor on the settle. Only (a) does — and (a) exists only on a
port this boot actually energised.

**Why the constant is conditional and not flat.** `settle_start` is taken *after*
the `PP`-write loop, so the port power-on-to-power-good ramp sits **inside** the
budget while `TSIGATT`'s clock only starts at the far end of it. A flat 100 would
therefore hand a conformant device on a freshly energised port strictly *less*
than the 100 ms it is allowed — the floor violated by the constant named after
it. But the ramp only exists for `/on` ports, and the driver knows which those
are. So:

| condition | `settle_ms` | why |
|---|---|---|
| any port energised by us this boot (`/on`) | **150** | 100 ms `TSIGATT` + 50 ms ramp allowance. The metal-proven incumbent; nothing in evidence justifies trimming a path nobody has measured |
| every port already powered (`/pre`) | **100** | no ramp to allow for, and no `TSIGATT` clock running at all |

On the seven rMBP captures every port is `/pre`, so that bench takes the 100 ms
branch and the full ~50 ms saving.

**What the `/pre` branch is really waiting for.** Not attach signalling. VBUS
never transitioned, so `TSIGATT` expired long before the kernel existed; the
21 ms is post-`HCRST` root-port connect re-detection, for which **no external
standard sets any bound at all**. 100 is therefore not a spec floor on this
machine — it is the measured phenomenon with an order of magnitude of headroom,
retained at the `TSIGATT` figure because it is the least arbitrary number
available and because the other tracks inherit this constant. The `TSIGATT` floor
is doing its work for `hw-pi4` and `hw-jetson`, not for the rMBP.

**Why not far lower, given `latest=21`.** Because the cost of undershooting is
not symmetric with the saving. A port that asserts at `t`:

- `t ≤ settle_ms` — caught by the initial scan; being a boot-scan entry it
  *skips* the 100 ms `TATTDB` connect debounce, so enumeration starts at
  `settle_ms`;
- `t > settle_ms` — falls to the CSC / hot-plug path, which is **not** a
  boot-scan entry and pays the debounce in full, so enumeration starts at
  `t + 100`.

Undershooting by one millisecond costs a hundred. The whole prize is
`150 − settle_ms`; the penalty whenever the real population's tail exceeds the
new value is `+100`.

**It survives the uncalibrated timebase — up to a stated limit.**
`cycles_per_ms()` falls back to a flat 2,000,000 cycles/ms (an assumed 2 GHz) when
`apic::tsc_hz()` reads 0 (no ACPI PM timer, or `calibrate` returning
ABORTED/REJECTED). A nominal `N` ms then spins `N × 2e6` cycles, which on a part
whose real invariant TSC is `f` GHz is `N × 2 / f` ms of real time. **The
direction of the error is a property of the machine, not of the helper**: above
2 GHz waits run short, below 2 GHz (low-power mobile parts, QEMU TCG) they run
long. Only the short direction can be unsound, and only for a constant chosen at
an external floor.

For the `/on` branch, 150 nominal is `300 / f` ms real, so it stays at or above
the 100 ms `TSIGATT` floor **for any invariant TSC up to 3.0 GHz** — this bench's
2.693 GHz gives ~111 ms, where a flat 100 would already have given ~74 ms. Above
3.0 GHz even the 150 branch falls under the floor on the fallback path (~86 ms at
3.5 GHz); the fallback is a stopgap for an uncalibrated timebase, not a guarantee,
and a seat expecting to run there must calibrate rather than lean on these
numbers. Within that bound it is a second, independent reason the constant is not
flat — and `cycles_per_ms`'s doc comment, which used to claim a wrong guess is
"never unsound", now states the formula and the limit instead of a new absolute.

**The three failure tokens.** A trim justified by a witness that cannot see its
own failure mode is not justified.

| token | meaning |
|---|---|
| `result=CCSMARGIN-TIGHT` | positive margin, but under a fifth of the budget — the population lives at the floor |
| `result=CCSMARGIN-BLOWN` | `margin_ms ≤ 0` — the at-deadline sweep was the first `CCS=1`; no headroom at all |
| `result=CCSMARGIN-LATE` | a port the settle never saw delivered its connect edge *afterwards*, through the recovery path |

`CCSMARGIN-LATE` is the one that matters, because it is the only signal that
escapes the deadline. Ports that end the settle unseen are latched; ports the
initial scan finds are cleared; the first connect edge on a still-latched port
prints

```
xHCI: !! ccs-margin LATE port=N t_seen_ms=<t> settle_ms=<s> short_by_ms<=<t−s> (missed the initial CCS scan; recovered via CSC; t_seen is drain time, an upper bound) result=CCSMARGIN-LATE
```

**`t_seen_ms` is discovery time, not assert time.** This kernel runs the xHC with
interrupts off at boot, so the Port Status Change TRB waits in the event ring
until the main loop's first `poll_events()` drain — and between the settle and
that drain sits `pci::init`, ~4.5 s of iGPU/Kepler bring-up on this media, with
the first drain measured at **≈4997 ms** after `settle_start`. So the true assert
lies in `(settle_ms, t_seen_ms]` and the true shortfall in `(0, short_by_ms]`.
Do **not** read `short_by_ms` as "add this to the settle". What the line proves is
the categorical thing — the initial scan missed a port it was supposed to catch.

That measurement is also why the window is **not** wall-clock. The first version
of this detector closed at a fixed 2 s, roughly 3 s before the first drain could
ever run: it could not fire on this machine at all, and a falsifier that is dead
on arrival is worse than none because its silence reads as a pass. The window is
a boot-phase boundary instead — armed until the end of the first `poll_events()`
pass that *completes* at or after `CCS_LATE_FLOOR_MS` (2 s). Everything latched
during boot is still in the ring when that pass drains it, so it is reported;
anything arriving afterwards is a human with a cable and stays silent. The floor
only stops a pathologically early empty pass from closing the window before a
slow port has had time to assert.

**`result=CCSMARGIN-` (with the trailing dash) matches every failure of this trim
and nothing on a healthy boot** — that is the pattern to arm a bench waker on,
and with the window fixed it can now actually fire. The healthy line still ends
`result=CCSMARGIN`, so pre-CCSTRIM capture greps keep working.

**Saving: ~50 ms** on the rMBP, where all seven captures are `/pre` and
`polling_candidates` is empty on every one. Zero on an `/on` boot (the constant
stays 150 there by construction), and zero on any boot where the Polling debounce
does run — the debounce absorbs the trim exactly (`360 − settle_ms`) and the boot
pays 360 ms either way. That last part is deliberate: the trim must not be able
to shorten a spec window.

**What would invalidate this.** Not sample size — the `/pre` branch rests on seven
identical boots, and 150 on the `/on` branch is unchanged from the value the
project has always shipped. It falls to: any boot printing `CCSMARGIN-LATE`; a
`first_assert_ms` above ~80 on an `/on` port; or an rMBP capture showing `/on`
ports at all, which would mean the pre-powered premise the 100 ms branch rests on
does not hold on that machine after all. None of the three was observable before
this change.

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

### 4a. Console-first ordering (BOOTPACE M2)

The order of the hooks in that ladder is not incidental, and as of this arc it is
fixed by two rules. Both x86-reachable ladders in `main.rs` (the `usbdebug` loop
and the GUI loop) now run:

```rust
xhci.poll_events();
xhci.service_ftdi();      // <- ahead of storage
xhci.service_storage();
xhci.service_hubs();
xhci.service_hid_setproto();
xhci.service_slot_disposal();
xhci.service_enum();
```

and `service_storage()` holds its deferred bring-up while enumeration is still in
flight:

```rust
if !self.storage_pending_bringup { return; }
if self.enum_active || !self.ports_to_enumerate.is_empty() { return; }  // latch stays SET
```

**Why.** On metal the FTDI console is the only instrument that exists, and it arms
at the *end* of its own port's enumeration. Previously, the storage device's
Configure-Endpoint completion armed `storage_pending_bringup` and the very next
`service_storage()` ran the multi-second TUR / INQUIRY / READ CAPACITY chain —
plus the FAT mount and the first flight-recorder flush behind it — while the
remaining ports, *including the FTDI's*, were still queued. Every one of those
seconds elapsed with no console attached, so those lines reached a second host
only as a replay out of the 64 KiB capture ring (`ftdi.rs`), which discards the
**oldest** on overflow. Deferring means all ports enumerate back-to-back, the
console arms, and only then does the storage chain start — live on the wire.

**Cost.** Storage becomes ready later by the tail of enumeration only: hundreds of
milliseconds, worst case one wedged port's bounded watchdog. Every `service_enum`
stage has such a watchdog (`recover_enumeration` on expiry), so `enum_active`
cannot stay true indefinitely and the deferral cannot become a hang. The latch is
left set — this is a postponement, never a skip.

**No protocol change.** This is ordering only: no budget, no settle, no timing
constant, no interrupt-model change. Nothing downstream needed adjusting either —
`fat::probe_once`, `flight_recorder::service` and the storage fixtures all gate on
`block::info()` internally, so they simply follow the block device's later arrival.

**Dating a capture.** The `:: BOT: knobs … result=KNOBS ::` line carries
`order=console-first`, on the same terms as `cbw=always-awaited`: not a knob, a
statement of what the build always does. A log whose KNOBS line lacks the field
predates this reordering, and its storage-chain timings were taken with the
console not yet armed. Artifact proof:
`strings unaos/target/x86_64_esp/kernel.elf | grep -c 'order=console-first'` ≥ 1.

**What is measurable.** BPACE (`docs/dev/OS/01_BOOT_HAL/bootpace.md`) reads the
reordering directly: `ftdi-up` now precedes `stor-bringup` on the ledger, where
before this arc it landed after `fr-flush`. That inversion — and only it — is the
proof the reordering took effect; `gui=` is unaffected, because the GUI handoff
happens before the service loop that runs any of these hooks even starts.

The Pi's `pump_usb_into_gui` ladder (`main.rs`, `all(aarch64, baremetal)`) is
deliberately **unchanged**: `service_ftdi` is a no-op there.

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

### 5i. PIUSB-43 — the enum-portsc witness (PA6: which connect branch died)

**The gap.** PA5c metal (boot via GR12) ran the FULL 30 s `enum-pump` budget with **zero**
port-status-change events off the ring, then thousands of idle `[piusb26]` pump passes and no slot, no HID, no BOT. The
wire could not distinguish four different deaths: no device electrically present (CCS never set) ·
CCS set but no Port Status Change Event generated · events written but never consumed off our ring ·
PP/link state regressed after the M3 line.

> **⚠ CORRECTED 2026-08-01 (s1v), from the PA5c capture itself — read this before the verdict key.**
> The framing above ("clean through ports-powered, then the connect died") does **not** hold, and the
> instrument was designed against it. The same capture says, verbatim:
>
> ```
> xHCI: TIMEOUT (~150000000 cyc) waiting for USBCMD.HCRST=0 (reset)
> xHCI: TIMEOUT (~150000000 cyc) waiting for USBSTS.CNR=0
> xHCI: FATAL — USBSTS.CNR still 1 after 801 polls (~150000000 cyc); aborting xHCI register programming
>        (spec 5.4.1: op/runtime writes while CNR=1 are dropped)
> xHCI: init_pointers SKIPPED — controller never left Not-Ready (CNR=1)
> xHCI: start SKIPPED — controller never left Not-Ready (CNR=1); RS=1 not issued
> ```
>
> So on PA5c: **RS=1 was never issued and no ring was ever programmed.** Three consequences bind every
> reading below.
> 1. `M3: 5 root port(s) powered (PORTSC.PP set)` and `enumerate: xHCI self-coherent` are
>    stage-intent lines, not state observations — by our own FATAL line, the PP write was dropped.
>    `PP=1` read back is therefore not our write landing (a firmware-set value is equally consistent).
> 2. `popped=0` / `pend=0` are **forced by construction**, not observed: ERSTBA/ERDP/CRCR/DCBAAP were
>    never written. `EVENTS-UNCONSUMED` cannot fire, and `CCS-NO-EVENT` degenerates into a
>    restatement of "no event path was ever built".
> 3. A device **was** electrically present at both sample points (pump entry and after pump exit;
>    the capture samples endpoints, not continuously) — port 1 read `0x400202e1`
>    (CCS=1, CSC=1, PP=1, PLS=7 Polling) and `[enum] port 1 connect (device attached)` fired. The
>    "no device ever existed to the kernel" reading was wrong.
>
> **Admissibility rule:** whenever `init_pointers SKIPPED` / `start SKIPPED` appear in a capture, a
> `[piusb43]` verdict must be quoted **beside them, never alone** — under CNR=1 the PORTSC and IMAN
> reads inherit the same doubt the FATAL line raises about that register window. (Note this is an
> EXTENSION of the spec cite, which covers dropped op/runtime *writes* under CNR=1, not read doubt —
> conservative, and stated as an extension rather than as the spec's own claim.) The sentence below
> ("it can execute in the state it reports on") holds for the pump's own MMIO, not for a controller
> that never left Not-Ready.
>
> **Upstream cause (s1v, settled from captures, zero boots).** The mailbox transport dies mid-boot:
> alive at `[v3d55] GET_CLOCK_RATE … (mailbox OK)`, dead from `[v3d75a] SET_ENABLE_QPU(1) — MAILBOX
> FAILED` onward, for every caller. All three `NOTIFY_XHCI_RESET` attempts then fail, so the VL805
> firmware is never reloaded after our PERST/RC reset and the controller cannot leave CNR. The
> `set_enable_qpu` is reached only via `v3d75_fabric_condition()`, whose sole caller
> `empty_frame_bisection()` early-returns on `!V3D_DEEP`. The second send site reaches the same tag
> reply-lessly under `v3d81_qpu` (`v3d.rs:7204-7211` from `v3d81_replyless_notify()`), which is the
> last statement of the same function — so it rides `V3D_DEEP` too. The send is behind
> **`UNAOS_V3D_DEEP=1` alone**, and dropping that one knob removes it with no code change.
>
> **Two controls, and they prove different things — do not conflate them.**
> - `capture/pi4-r23s1q` (2026-07-29) ran **`deep=on`** on a build that *predates* the `[v3d75a]`
>   rung — zero `SET_ENABLE_QPU` sends anywhere in it — with zero mailbox timeouts, `NOTIFY … tag
>   HONOURED` ×3, `CNR cleared after 1 polls`, `RS=1`, `Max Ports = 5`, an enumerated HID mouse. It
>   isolates **the send**, not the knob, and positively rules out `deep=on` by itself as the cause.
> - **PA6** (`67d2b094`, 2026-08-01) is the knob counterfactual: same tip code as the failing image,
>   `UNAOS_V3D_DEEP` dropped. Metal result — **0** mailbox timeouts, `tag HONOURED` ×3, `CNR cleared
>   after 1 polls`, `enum-xhci-handoff took=1ms` (vs 5566 ms), `enum-pump took=7694ms` (early exit,
>   vs the full 30000 ms budget), port 1 walking `0x400002e1` → `0x40000e03` (PED=1, PLS=0, High-
>   Speed), hub walked, `SLOT 1 ENABLED & ADDRESSED`, `keyboard ARMED … -> PASS`, boot-mouse
>   configured. Mass storage did not enumerate on that boot
>   (`note='no mass-storage device enumerated'`) — unattributed; no storage device is known to have
>   been attached, so it is not evidence about the BOT path either way.

**The instrument.** A read-only sampler inside the enum pump: at pump START, every ~2 s of pump
time (capped at 13 interior samples), and once at pump END, one line carries every root port's RAW
PORTSC word beside its decode (frozen-register discipline — no derived-only claims) plus the
event-ring consumer state (`dequeue_index`/cycle, `popped` = total TRBs ever consumed, a
`has_event` freshness peek, raw IMAN with IP/IE). PORTSC reads latch nothing (change bits are RW1C
and the witness never writes), IMAN is read not acknowledged, and no pump logic, budget, or write
is touched. Default-ON: the instrument exists for the failing path, and it can execute in the state
it reports on (samples ride the same MMIO the pump already trusts); QEMU raspi4b never reaches the
pump (`XHCI_READY` gate), so its log stays byte-identical. Budget: ≤17 lines per boot
(1 HCSPARAMS1 + 1 start + ≤13 periodic + 1 end + 1 verdict).

**Witness grammar.**

```
:: PIUSB: [piusb43] portsc <start|pump|end> t=<ms>ms p1=0x…(CCS=_ CSC=_ PED=_ PP=_ PLS=_ spd=_) … p5=…
    | evt deq=<idx> cyc=<b> popped=<n> pend=<0|1> IMAN=0x…(IP=_ IE=_) ::
:: PIUSB: [piusb43] verdict=<branch> — <evidence> ::
```

**Reading key — the verdict names only what the samples show, in this priority:**

| Verdict | Meaning / evidence |
| --- | --- |
| `EVENTS-UNCONSUMED` | ring holds a fresh TRB at exit (`pend=1`) AND `popped=0` all pump — events reached the ring, the consumer never took one |
| `REGRESSED` | PP=0 at pump start (regression between M3 and the pump), PP 1→0 during the pump, or PLS moved with CCS never set — before/after raw words quoted |
| `CCS-NO-EVENT` | CCS/CSC seen on some port (mask printed) yet `popped=0` and nothing pending — device seen electrically, no PSC event reached the ring; event path suspect |
| `NO-CCS-EVER` | every port sampled CCS=0 CSC=0 at every sample, PP held, PLS stable — no device electrically visible to the VL805 |
| `UNRESOLVED` (poison) | a PORTSC word read back as poison (`0xffffffff`/`0xdeadbeef`/`0xdeaddead`) — the register window stopped decoding, so every PORTSC-derived branch is disqualified. Checked AFTER the ring branch, which is derived from the event ring in DRAM and stays sound while the BAR is dark; the raw ring words ride the line so nothing observed is lost |
| `UNRESOLVED` | a state the samples cannot separate — the line names the deciding read (e.g. CCS seen *and* `popped>0`: a TRB-type census of the consumed events would decide) |

**Poison discipline (s1v).** A non-decoding BAR reads `0xffffffff`, which *decodes* as
`CCS=1 CSC=1 PED=1 PP=1` — indistinguishable from a live connect on the decode alone. Before this
was guarded, that word set `saw_ccs` and drove a confident `CCS-NO-EVENT` ("a device was seen
electrically") off a dead bus. `sample()` now runs every per-port word through the file's existing
`is_poison()`, prints it as `p<N>=0x…(POISON — not decoded)`, accumulates nothing from it, and
latches the port so `verdict()` takes the poison branch. `HCSPARAMS1` gets the same treatment: poison
there would yield `MaxPorts=0xff`, which the clamp would silently turn into 8 — three ports past the
VL805's five, sampled as if they existed; on poison the witness falls back to the known 5 and says so.

Support: `EventRing.popped` (drivers/xhci/event.rs), a monotonic consumed-TRB counter incremented in
`pop()` — `dequeue_index` alone is mod-256 and cannot distinguish "no events" from "exactly one lap".
Ledger note: the numbers PIUSB-41/42 were already spent on the P60 default-quiet dump (5h above), so
this instrument is PIUSB-43 with serial tag `[piusb43]`.

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
  attach. **Hot-plug ports only** since BOOTPACE M4 — see below.
- **Escalating retry pacing** — recovery re-resets are spaced 200/400/600 ms
  (attempt-scaled), and a code-4-with-FS-speed failure logs an explicit
  failed-HS-chirp hint in the serial capture.

### 6a. Initial-boot-scan ports skip the connect debounce (BOOTPACE M4)

TATTDB asks for 100 ms of **stable attach** before the port reset. A port queued
by the initial CCS scan in `start()` has already served it, several times over and
in wall clock that is not ours to charge twice:

- the device was physically attached before the machine was powered — the boot
  stick *is* the UEFI boot device;
- `start()` powered the port and then held the pre-scan settle (§2d) before
  sampling CCS at all;
- it was observed `CCS=1` at the end of that settle, which is the stability
  evidence the debounce exists to gather.

So `start()` records those ports in `boot_scan_ports` alongside
`ports_to_enumerate`, and `start_next_port` sends a marked port straight to
`issue_enum_reset` instead of parking it in the `debounce` stage. Witness:
`xHCI: [enum port N] initial boot scan — attach already stable through the settle;
skipping the 100 ms connect debounce.`

**Hot-plug ports keep the full 100 ms, unchanged.** That path is metal-proven, not
theoretical: the 2012 rMBP bench (2026-07-08) hot-plugged a High-Speed SD reader
which, reset immediately on the connect event, trained at Full-Speed (failed HS
chirp) and then failed every `ADDRESS_DEVICE` with USB Transaction Error (code 4).
Nothing about that case is improved by removing its debounce.

The mark is **consumed when the port is popped**, not when it is used. A port that
is popped, found `CCS=0`, skipped, and later returns through `CSC` is a genuine
(re)attach and pays the full debounce — as does any port re-queued by
`requeue_after_settle` or by the hot-plug path. The 50 ms `TRSTRCY` reset-settle
is untouched on both paths.

Saving: 100 ms per boot-scan port on the critical path to the first console and
the first block read (two ports on the rMBP bench = ~200 ms).

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

#### 10f-bis-1. Reading the `PTR:` line cold — the three fields do not count the same thing

Boot 3 (2026-08-19) printed lines of the shape `seen=104 delivered=6 recovered=0`, and read naively that
looks like 98 lost clicks. It is not: **`seen` is counted per REPORT, `delivered` and `recovered` per
EDGE.** Written out (all three live in `IntEp::note_buttons`, `drivers/ehci/mod.rs`, and all three are
`usbdebug`-only statics):

| Field | Counter | Incremented when | Unit |
| --- | --- | --- | --- |
| `seen` | `PTR_PRESS_SEEN` | every report whose primary bit is set (`buttons & 0x01 != 0`) — no edge test at all | one per **report** |
| `delivered` | `PTR_PRESS_DELIVERED` | the report is judged a NEW press: a down edge (`down && !prev`) **or** the §10f-bis re-press recovery (`down && prev && quiet ≥ 120 ms`) | one per **press**, and one `pal::Event::Button` is owed for each |
| `recovered` | `PTR_PRESS_RECOVERED` | the recovery arm specifically fired — i.e. this delivery only happened because of the quiet-gap rule | one per **press**, a strict **subset of `delivered`** |

Three consequences that make a line readable cold:

- **`seen` ≫ `delivered` is the healthy state, not a loss.** A HID report is a *level*: while a finger
  rests on the pad the endpoint keeps re-reporting "primary down", and each of those reports bumps `seen`
  while none of them is a new press. `seen / delivered` is therefore roughly *reports per hold* — Boot 3's
  104/6 is ~17 reports across 6 presses, which for a ~1 kHz endpoint drained by a frame-rate service loop
  is an ordinary set of human-length clicks. The ratio measures **poll rate × hold duration**, and nothing
  about correctness. `seen == delivered` would be the *suspicious* reading (every press seen exactly once
  means the loop is missing nearly every report of every hold).
- **`recovered` is the only defect signal on the line.** `recovered = 0` means every press this boot
  arrived as a clean down edge. `recovered > 0` means that many presses would have been **silently
  dropped** before §10f-bis — each one is a release report that the one-report-per-service-pass endpoint
  missed, leaving `prev_buttons` latched at `0x01`. So `recovered` counts *repairs*, and a rising
  `recovered` is evidence about the polling rate, not about the pad.
- **The line prints only on a delivery.** It sits inside the `edge || repress` branch, so every `PTR:`
  line is emitted *at* a press and the numbers are running totals as of that press. `delivered` therefore
  increases by exactly 1 between consecutive lines from the same endpoint; `seen` jumps by however many
  down-reports the previous hold produced. `[i]` is the interrupt-endpoint index, so a boot with a
  trackpad and an external mouse interleaves two independent ledgers.

Nothing here is derivable from a single line in isolation except `recovered`: to say anything about lost
input you compare `recovered` against zero, never `seen` against `delivered`.

**And `delivered` measures the BELT, not the outcome — Boot 5 (2026-08-19) is the worked example.** That
card's clicks did nothing on screen while `delivered=` climbed once per physical click. Both readings
were correct, and they are about different layers: `delivered` counts what `IntEp::note_buttons` handed
to `pal`'s event queue, and it says *nothing* about what the drain that pops the queue does with the
event. On Boot 5 the answer was "discards it" — the `usbdebug` drain had no router call at all, so every
`Event::Button` fell into its catch-all arm (fixed by USBDBG-ROUTE; see `08_VIDEO/engine.md`). So when
clicks are reported dead, read the two witnesses as a **pair**, in this order:

1. `PTR: ... delivered=` rising per click ⇒ the pad, the endpoint and the edge logic are fine, and the
   defect is at or above the drain. Stop looking at USB.
2. no `PTR:` movement ⇒ the defect is on the belt, and `recovered` says whether polling is the cause.

The instrument on the far side is `[clickroute]`, emitted by the router itself: `delivered` rising with
**no** `[clickroute]` line for the same click is the exact signature of a drain that never routes.

**Pairing note, updated for USBDBG-INVERT (2026-08-19).** The *reading* above is unchanged, but the
"which drain?" half of it has simplified: on an `x86_64` + `wc` build there is no longer a second drain
to be in. The `usbdebug` terminal loop is compiled out on that combination and the card runs the
ordinary desktop path (`08_VIDEO/engine.md` §USBDBG-INVERT), so *every* x86 desktop build — knob on or
off — pops its events in `x86_render_service` and routes them through `wc_route_event`. Step 1's
conclusion is therefore now "the defect is at or above the drain, and the drain is the render service's"
rather than "…and which drain you are in depends on the knob". The `USB-DEBUG:` print lines survive on
that path (they moved ahead of the router, printing the RAW report), so a capture can still show
`PTR: ... delivered=` rising, the matching `USB-DEBUG: BUTTON` line, and the `[clickroute]` disposition
for one physical click — the three layers of the same press, in order, on one wire. A build WITHOUT
`wc` still has the terminal loop, and there step 1's original wording applies verbatim.

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

## 12. FRWRITE — why the x86 flight-recorder write times out on metal (2026-07-26)

Two metal boots on the 2012 rMBP, with **two different USB sticks**, both timed out in the
flight recorder's write path. Two sticks failing identically kills the device-fault
hypothesis, so this section is a read of OUR write path, in code, with the arithmetic.

### Metal ground truth

* Boot 1 (stick A): `:: BLK: io-cause op=write lba=121 bot_err=Timeout ::`; recovery then got
  `why=nocompletion` on every EP0 request, `csw_sig=0x0`.
* Boot 2 (stick B, fresh 2 GB): `:: BOT: pump … used=1275711592 … n=1 timeouts=0 result=OK ::`,
  then `:: BOT: pump … used=16163799966 n=68 timeouts=1 result=TIMEOUT ::` on
  `op=write lba=9834`; first recovery `reset=fail halts=fail`, second `reset=ok halts=cleared`.
* Both boots: `UNAOS.LOG` reservation failed.
* QEMU: green, fast, every knob.

TSC calibrated at ≈2.6938 GHz, so `used=1275711592` = **0.474 s** for ONE stage wait, and
`budget=16163799966` = **6.000 s** (`hw_wait_budget()` 2 s × 3, `mod.rs` `pump_until_bot_done`).
The QEMU comparator from §11 is `peak=32724164` over `n=2691` — 0.012 s. Metal's *first* wait
is 39× QEMU's *worst*.

### 12.1 What the FR reservation actually writes — the roadmap's "~128-sector WRITE(10) burst" is FALSE

There is no burst. There cannot be one:

* `flight_recorder.rs` `RESERVE_BYTES = RING_CAP + 512 = 66048` bytes = **129 sectors**, written
  by `reserve_log()` as a single `fs.write_grow(0, 0, …, &zeros)` of a 66048-byte zero buffer.
* `fat.rs::write_grow` step 3 is a **per-sector read-modify-write loop**: `read_sector(lba)` →
  patch → `write_sector(lba)`, one sector at a time. `write_at` (every later flush) is the
  identical shape.
* `fat.rs::write_grow` step 2 calls `alloc_cluster()` per cluster, and `alloc_cluster` ends in
  `zero_cluster(c)` — another per-sector write loop over the whole cluster — plus a FAT-region
  scan from cluster 2 and a `set_fat_entry` RMW across all FAT copies.
* `block.rs::write_block` / `read_block` hardcode `blocks = 1`: `storage_write10(lba, 1)`,
  `storage_read10(lba, 1)`.
* `xhci/mod.rs` `configure_bulk_endpoints_sync` allocates `scsi_data_buffer` with
  `Layout::from_size_align(512, 64)` — **512 bytes**. A multi-block WRITE(10) is not merely
  unused, it would overflow the only DMA buffer the storage slot owns.

So the maximum transfer size per BOT transaction anywhere in this driver is **512 bytes**.
Worked cost of one reservation on a 32 KiB-cluster volume (3 clusters for 66048 bytes):

| stage | single-sector BOT transactions |
| --- | --- |
| `alloc_cluster` × 3 → `zero_cluster` (64 sectors each) | ~192 WRITE(10) |
| FAT scans + `set_fat_entry` RMW (all copies) | ~20 mixed |
| `write_grow` data RMW, 129 sectors | 129 READ(10) + 129 WRITE(10) |
| directory `size` publish (RMW) | 2 |
| *(pre-FRWRITE)* first flush pad to `RESERVE_BYTES` | 129 READ(10) + 129 WRITE(10) |
| **total** | **≈730** |

> **SUPERSEDED (2026-08-03, BOT-CBW).** The paragraph below rests on the CBW carrying no IOC. That
> premise was **convicted on metal** — see
> [§17](#17-bot-cbw--the-straddle-convicted-and-the-cbw-becomes-a-stage-2026-07-30) for the A/B that
> did it, §16.3 for the source audit that preceded it, and
> [§17.9](#179-provenance-of-the-no-ioc-position-and-what-is-still-owed) for where the premise came
> from. `bot_transfer_once` now pushes the CBW with `control: (1 << 10) | (1 << 5)` — Normal **plus
> IOC** — rings its own doorbell for it and pumps it to completion before the data stage is *built*,
> so the CBW is a first-class awaited stage.
>
> **The count is three, not two**, on any transaction that carries data: CBW, data, CSW. (A
> zero-length transaction — `data_len == 0`, e.g. TEST UNIT READY — awaits two, CBW and CSW; the
> data stage is guarded on `data_len > 0`.) Every row of this section's ≈730-transaction table is a
> single-sector READ(10) or WRITE(10) and therefore carries a data stage, so against **this
> section's own numbers** the reservation is **≈2190 pump waits**, not ≈1460. Only the multiplier
> moves: the ≈730 transaction count, the 512-byte-per-transaction ceiling and M1's shape are all
> untouched, and the paragraph's closing point — that the roadmap's mental model of 1 is off by
> three orders of magnitude — holds a fortiori.
>
> **Two limits on quoting ≈2190.** It is arithmetic, not a measurement: no capture counts the pump
> waits of a whole reservation. And it re-prices the *pre-MULTIBLK* model this section froze —
> §12.5 item 3 removed ~258 of the ~730, and §13 replaced the single-sector geometry outright
> (§12.6: "drops from ~730 transactions to single digits"). ≈2190 is what §12's model implies once
> the CBW is awaited; it is not today's cost.

Each transaction awaits **two** Transfer Events (data stage, then CSW — the CBW TRB carries no
IOC and is fire-and-forget), so the reservation is **≈1460 pump waits**. The roadmap's mental
model was 1. The real number is three orders of magnitude larger.

Boot 2 wedged at `n=68` — ≈34 transactions, roughly **5 % of the way into the reservation**.

### 12.2 The data-stage TD shape — audited, and NOT the bug

`bot_transfer_once` (`xhci/mod.rs`) builds the data stage as exactly **one Normal TRB**:

```
ring.push(Trb { parameter: data_phys, status: data_len,
                control: (1 << 10) | (1 << 5) | (1 << 2) })   // Normal | IOC | ISP
```

`data_len` is always 512. Reviewed against the things that make a real EHCI-behind-xHCI
(Panther Point) route misbehave, and cleared:

* **No chaining needed and none present.** One TRB, one TD, one event. A 512-byte
  64-byte-aligned buffer cannot cross a 64 KiB page boundary, so the classic
  split-buffer requirement never arises.
* **No Event Data TRB, correctly.** With a single-TRB TD, IOC on that TRB *is* the completion
  event; an Event Data TRB would add nothing.
* **No IDT misuse.** Immediate Data is never set; `parameter` is always a buffer address.
* **Ring wrap is correct.** `ring.rs::push` writes a Link TRB with TC=1 at index
  `num_trbs - 1` and toggles the cycle bit. Rings are 16 TRBs; a data-carrying BOT transaction
  pushes 3, so the ring wraps every ~5 transactions — nothing special happens at 34.
  The Link TRB is written lazily rather than at ring init; that is safe, because the untouched
  slot holds an all-zero TRB whose cycle bit (0) stops a prefetching controller.

**Therefore the "per-TRB event wait" hypothesis is REFUTED for the data stage: there is only
one TRB.** 0.474 s is the latency of a single 512-byte transaction's single completion event,
not 470 chained waits.

### 12.3 The mechanism, named

Two separate things, and they must not be conflated:

> **FIGURE SUPERSEDED (2026-08-03, BOT-CBW).** `~1460` below is §12.1's figure and inherits its
> correction: with the CBW awaited the same ≈730 transactions cost **≈2190** awaited completion
> events. The amplification argument is unaffected — it is strengthened.

**(M1) Transaction amplification — why it is always the FR write that dies.** The write path
issues ~1460 awaited completion events to lay down 64 KiB. On QEMU each costs microseconds and
nobody notices. On metal each costs a real USB round trip, and — critically — each one is an
independent chance to hit whatever wedges this controller. The FR reservation is not the only
FAT writer, it is simply the one that issues two orders of magnitude more transactions than
anything else in the boot, so it is where a per-transaction failure probability first cashes
out. `lba=121` (boot 1, FAT/reserved region) and `lba=9834` (boot 2, data region) are two
different points inside the *same* long sequence — consistent with a per-transaction hazard,
inconsistent with a specific bad LBA.

> **SUPERSEDED-UNLESS (BOT-RESCUE, 2026-07-29).** The M2 reading below — "a LOST completion event"
> — is the *lost-CSW hypothesis*, and it was never directly tested: it was inferred from
> `used == budget` with a far smaller `peak`, which distinguishes "the event never arrived" from
> "the budget was tight" but **not** "the event was lost" from "the device never produced one".
> Witness 5 (§14.3) now tests it directly: a CSW signature of `0x53425355` in the buffer at timeout
> means the status phase landed and only the event went missing (M2 as written); `sig=0x0` means the
> status phase never happened at all — a transport wedge, a different fault class, and the one the
> 2026-07-29 Alcor capture points to. **Treat M2 as superseded unless a capture shows witness 5
> reporting `valid=yes`.** Nothing below is retracted; it is awaiting the discriminating reading it
> was always missing.

**(M2) The wedge itself — a LOST completion event, not a tight budget.** The TIMEOUT line's own
reading rule (`pump_until_bot_done`, the IVY comment) is: `used == budget` with `peak` far below
it means the completion event never arrived. Boot 2 has exactly that — `used` = the full 6 s
budget, last reported `peak` = 0.474 s, 12× smaller. The transfer did not run long; it never
completed. The `why=nocompletion`/`csw_sig=0x0` on boot 1's recovery says the same thing on the
other stick. Widening the budget would change nothing.

### 12.4 The measurement gap this capture exposed

`note_bot_pump` prints only when the peak **doubles**. Between the `n=1` line at 0.474 s and the
`n=68` timeout, every one of the 67 intervening waits could have been anywhere in [0, 0.95 s)
and no line would say so. The capture therefore **cannot** answer "is the whole write path
running at half a second per sector, or was the first transaction a one-off spin-up?" — and
that distinction decides whether M1 alone is fatal.

Closed here by accumulating **every** wait: `BOT_PUMP_CYCLES` sums each completed pump wait, and
the SUMMARY and TIMEOUT witnesses gained `sum=`/`mean=` (`sum / n`). Mean-vs-peak is exactly the
discrimination that was missing. The per-transfer `result=OK` line is byte-unchanged.

**QEMU calibration for the new field**, from this arc's gates — the reference metal is read against:

```
UNAOS_HUBSTORAGE=1 ./arroyo test-fat sf 300   (behind a hub, route=0x1 depth=1)
:: BOT: pump budget=14397530250 peak=10277608 sum=1772213888 mean=236832 n=7483
   nowait=0 timeouts=0 storage_slot=2 route=0x1 depth=1 result=SUMMARY ::

UNAOS_IRQSTORAGE=1 UNAOS_FATIMG=sf ./arroyo test 200   (root port)
:: BOT: pump budget=14393257680 peak=30858468 sum=1963446398 mean=751990 n=2611
   nowait=0 timeouts=0 storage_slot=1 route=0x0 depth=0 result=SUMMARY ::
```

So under QEMU `peak / mean` is 43× and 41× — a healthy transport is dominated by cheap
transactions with a rare outlier. Metal's `peak` is 0.474 s; if metal's `mean` comes back within
an order of magnitude of that, the transport is uniformly slow and M1 is fatal on its own.

### 12.5 Changes landed in this arc

1. `xhci/mod.rs` — `BOT_PUMP_CYCLES` accumulator in `note_bot_pump`; `sum=`/`mean=` fields added
   to `:: BOT: pump … result=SUMMARY ::` and `:: BOT: pump … result=TIMEOUT ::`.
2. `xhci/mod.rs` — the PH-2 comment claiming "a ~128-sector WRITE(10) burst" corrected to what
   the code does (§12.1).
3. `flight_recorder.rs` — `PAD_NEXT` now defaults to `false` and is set from a new `reused` flag
   returned by `reserve_log()`. The pad exists only to clear a *previous boot's* stale tail, so
   it is needed only in the reuse case; in the create/grow cases `reserve_log` has just
   zero-filled the entire file, and the pad was rewriting known zeros at a cost of ~129
   READ(10) + ~129 WRITE(10). **This removes ~258 of the ~730 transactions above (−35 %)** on
   every first-time-reserve boot. The witness now reads
   `:: FR: UNAOS.LOG reserved 66048 bytes @cluster N reused=false — … ::`.

None of these is a fix for M2, and none is claimed to be. They shrink M1 by a third and make the
next metal sitting able to *measure* M1 instead of inferring it.

### 12.5a Gate hazard found while landing this — `builder/fat-*.img` is MUTATED in place

QEMU writes the FAT test image **in place**: after `UNAOS_FATIMG=sf ./arroyo test`, the mtime of
`builder/fat-sf.img` has moved and it carries that run's `UNAOS.LOG`, its allocations and its
directory edits. Consecutive runs are therefore **not independent**, and a run against a dirty
image is a different experiment from a run against a clean one — during this arc the
`zeolite` DNS-sinkhole leg failed 3/3 on a carried-over image and passed 3/3 on a freshly built
one, with the *identical* kernel. Always run `./arroyo fat-img` immediately before any
`UNAOS_FATIMG=` / `test-fat` gate, and never A/B two kernels against the same image generation.

### 12.6 The real fix, deferred as a brief (multi-block BOT transfers)

The structural repair for M1 is to stop doing 512 bytes per USB round trip. Scoped:

1. Grow `scsi_data_buffer` from 512 B to a bounded staging buffer (32–64 KiB), allocated with
   64-byte alignment as now.
2. Let `scsi_read10`/`scsi_write10` carry `blocks > 1` (the CDB already encodes it; only the
   buffer size forbids it).
3. Build the data stage as a **chained multi-TRB TD** — CH=1 on all but the last, IOC+ISP on the
   last only, each TRB's buffer split at 64 KiB boundaries per xHCI 1.2 §4.11.7.1. This is where
   an Event Data TRB may become worthwhile, and it is the only part with real xHCI risk.
4. Add a multi-block entry point to `block.rs` and route `fat.rs`'s per-sector RMW loops through
   it for runs of whole sectors (a full-sector overwrite needs no read at all — that alone is a
   further 2× on every FAT write).

Steps 1–3 are in the xHCI lane; step 4 touches `fs/fat.rs` and `drivers/block.rs`, which are not
this arc's lane. That is why it is a brief and not a patch. Expected effect: the FR reservation
drops from ~730 transactions to single digits, and M2 — whatever wedges one transaction on this
controller — stops being reached hundreds of times per boot.

> **BUILT, 2026-07-29 — see §13.** The lane was widened by the operator and all four steps landed,
> with one deliberate departure: step 3's chained multi-TRB TD was **not** built. Over-aligning the
> staging buffer to 64 KiB makes a single Normal TRB legal at every size the buffer can hold, so the
> §12.2 data-stage shape is preserved verbatim and the "only part with real xHCI risk" is discharged
> rather than managed. §13 also records what this section could not have known: on the QEMU fixture's
> 512-byte-cluster geometry the dominant amplifier was `alloc_cluster`'s free search, not the
> per-sector data loop.

### 12.7 What metal must verify next

1. `mean=` on the SUMMARY/TIMEOUT lines. If `mean` is within a small factor of `peak`, every
   transaction is slow and M1 is fatal on its own. If `mean` is orders below `peak`, the 0.474 s
   was a one-off and M2 is the whole story.
2. That `reused=false` appears on the first boot of a fresh stick and `reused=true` on the
   second — and that the log content is still correct in both (no stale tail with `reused=true`).
3. Whether the reservation now gets further before wedging (a higher `n=` on the TIMEOUT line),
   which is the direct read of the −35 % transaction count.

---

## 13. MULTIBLK — the multi-block BOT path (2026-07-29)

This is §12.6's deferred brief, built. It is a fix for **M1 (transaction amplification)** and
**explicitly not** a fix for **M2 (the lost completion event)**, which remains unexplained. What it
does to M2 is shrink the number of times per boot the wedge can be reached, and — new — make the
next metal capture able to *characterise* it.

### 13.0 Diagnosis re-verification at tip (693e097f)

Every load-bearing claim in §12 was re-checked against the current tree before anything was designed.
All of it still held:

| §12 claim | at tip |
| --- | --- |
| `scsi_data_buffer` is 512 B | confirmed — `Layout::from_size_align(512, 64)` in `configure_bulk_endpoints_sync` |
| every block-layer caller passes `blocks = 1` | confirmed — all four call sites in `drivers/block.rs` |
| `fat.rs` write paths are per-sector RMW loops | confirmed — `write_at`, `write_grow` step 3, `zero_cluster`, `set_fat_entry_inner`, every directory-slot mutator |
| the data stage is ONE Normal TRB (IOC+ISP), not a chain | confirmed — unchanged since the §12.2 audit |
| `sum=`/`mean=` are on the SUMMARY/TIMEOUT witnesses | confirmed — §12.5's instrumentation is intact |
| `reserved … reused=false` witness | confirmed on every QEMU FAT run |

Nothing in §12 has gone stale. One thing §12 did **not** know, because it was measuring a metal boot
rather than a QEMU battery, is recorded in §13.4: on the QEMU fixture geometry the dominant amplifier
is not the data loop at all.

### 13.1 Design

1. **`scsi_data_buffer`: 512 B → 32 KiB, aligned to 64 KiB** (`STORAGE_DATA_BYTES` /
   `STORAGE_DATA_ALIGN`, `xhci/mod.rs`). The alignment is the whole trick. xHCI 1.2 §4.11.7.1 forbids
   a Normal TRB's buffer from crossing a 64 KiB boundary; a 32 KiB buffer aligned to 64 KiB *cannot*.
   So the data stage stays **exactly** the single-TRB / single-TD / single-IOC shape §12.2 audited and
   cleared, at every transfer size up to the buffer. §12.6 step 3 proposed a chained multi-TRB TD with
   hand-rolled boundary splitting and called it "the only part with real xHCI risk"; over-aligning the
   buffer **discharges** that risk instead of managing it. There is no new TRB shape on the wire.
2. **`scsi_read10` / `scsi_write10` carry a real `blocks`**, bounded by `STORAGE_MAX_BLOCKS` (= 64).
   The CDB always encoded the count; only the buffer forbade it. An inadmissible count is the new
   `BotError::BadRequest`, raised *before* anything is queued so it never drags the pipe through Reset
   Recovery.
3. **`drivers/block.rs` gains counted entry points** — `read_blocks` / `write_blocks` /
   `read_blocks_usb` / `write_blocks_usb`, plus `MAX_BLOCKS_PER_OP`. The diff is **purely additive**:
   `git diff drivers/block.rs` removes zero lines. `read_block` / `write_block` and both `_usb`
   singles are byte-identical, so the installer engine's verify ladder and the shell's raw
   `write <lba> <byte>` keep the exact path they were audited on. A request that is not a whole number
   of blocks, or is larger than the staging buffer, is **refused** — never truncated, because a
   short write that reported success is the one failure a filesystem cannot detect.
4. **`fs/fat.rs` coalesces contiguous extents.** `collect_chain` materialises a chain (caching the FAT
   sector across hops); `contiguous_sectors` measures how far a run stays consecutive on disk;
   `write_span` / `read_span` do the transfers. A span splits into head-partial / whole-sector body /
   tail-partial, and **the body is written with no preceding read at all** — the RMW exists only to
   preserve bytes outside the write, so it is needed only at the two ends. `zero_cluster`,
   `write_grow`, `write_at`, `read_file` and `read_at` all route through this. Six near-identical
   directory walks (read / locate / free-slot × fixed-root / chain) collapse onto one
   `walk_dir_sectors`, whose chunk size **doubles** as a scan proves itself long — so an early exit
   costs exactly what it used to and a full scan costs a logarithmic number of transfers.
5. **`alloc_cluster` gets a rotating start** (`ALLOC_HINT`, the in-memory equivalent of FAT32's
   FSInfo `FSI_Nxt_Free`). See §13.4 — this turned out to be the single largest amplifier in the QEMU
   battery. Search ORDER only: every cluster is still visited at most once per call (the scan wraps
   once back to cluster 2), the F3-M1 compare-and-claim under `FAT_MUTATION` is untouched, and the
   zero-fill-after-claim order — and with it the information-disclosure invariant — is unchanged.

### 13.2 Measured effect

All runs on this Linux box, kernel rebuilt per row, image rebuilt fresh per run per §12.5a.

| gate | baseline `n=` | after `n=` | cut |
| --- | --- | --- | --- |
| `UNAOS_FATIMG=1 UNAOS_WC=1 ./arroyo test 90` (fixture, 512 B clusters) | 10889 | **3969** | 2.74× |
| the same, on a **32 KiB-cluster** volume (metal-shaped, §13.3) | 2667 | **779** | 3.4× |
| `UNAOS_WC=1 ./arroyo test 90` (no FAT volume) | 51 | 51 | — |
| `./arroyo test-arm 22` (aarch64-virt) | 92 | 92 | — |

Verdict sets are unchanged. The FAT-attached gate's 40 PASS/FAIL lines diff clean against baseline
except for two timing-sampled lines (one `[wc-d] verify … -> PASS` sampled 5× before and 4× after,
all `bad_cache=0 bad_ram=0`, and WINX-8's parenthetical race wording). `./arroyo test-fat sf 300`
also runs green apart from the same two WINX teardown FAILs, with `U11m2 … -> PASS` (the ledgered
end-to-end FAT mutation gate) and `n=3853`. `timeouts=0` everywhere.

`n=` counts pump WAITS; a data-carrying transaction awaits two (data stage, then CSW), so the
transaction counts are half these. The new `:: BOT: tx … result=SIZES ::` witness reports the census
directly — on the 32 KiB-cluster run, `single=337 multi=52 maxlen=32768 rd_sectors=394
wr_sectors=789`: 389 transactions moving 1183 sectors, i.e. **3.0 sectors per USB round trip**, where
before every round trip moved exactly one.

### 13.3 The 32 KiB-cluster measurement, and why it needed building

The tracked fixture is `mkfs.vfat -s 1` — **512-byte clusters** — because the U10 GROW.BIN fixture
asserts "512 bytes == exactly one cluster". That is the *worst possible* geometry for cluster-level
coalescing: a cluster **is** a sector, so `zero_cluster` has nothing to merge and a data span rarely
crosses into a contiguous neighbour. The sticks that actually wedge on metal are formatted with
32 KiB clusters, which is the geometry §12.1's ~730-transaction arithmetic is priced against.

So the row above was measured against a purpose-built image: same staged contents, `mkfs.vfat -s 64`,
2600 MiB. The size is not arbitrary — FAT type is decided by **cluster count**, not by what the
formatter writes in the BPB, and a 96 MiB volume at 32 KiB clusters is 3072 clusters, i.e. FAT12,
which `parse_bpb` correctly refuses (`FS: no FAT filesystem (NotFat)`). 2600 MiB gives ~83200
clusters, over the 65525 FAT32 threshold — and is also the realistic pairing, since 32 KiB clusters
are what a formatter picks for a multi-gigabyte stick in the first place.

This is a **measurement** image, not a battery fixture: the U10 512-byte-cluster fixtures do not hold
on it. It is built by a throwaway patched copy of `make-fat-img.sh`; nothing about it is committed.

### 13.4 What the measurement found that §12 could not have

On the fixture geometry the per-sector data loop was **not** the dominant amplifier. Coalescing the
data path, the read path and the directory walks together took `n=` only from 10889 to 9261. The
remaining ~4600 transactions were `alloc_cluster`'s free search, which restarted at cluster 2 on every
call: allocating the flight recorder's 129 clusters (66048 bytes at 512-byte clusters) re-read the
~20 FAT sectors in front of the free region 129 times — **~2580 sector reads, more than every data
transfer in the boot put together**. The rotating start took the same run to 3959.

That is worth recording as a general lesson and not just a fix: §12.1's transaction table was built by
reading the *write* path, and it was right about the write path, but a table derived from code review
undercounts whatever the reviewer was not looking at. The census witness now measures it instead.

### 13.5 M2 — what is now instrumented, and what remains open

**No claim is made that M2 is fixed, or understood.** It was not reproduced in QEMU during this arc
(`timeouts=0` on every run above), so there is no new evidence about its *cause*. What changed is the
instrumentation, and the reason it is now worth having: while every transfer was the same 512-byte
single-TRB shape, there was nothing for a wedge to correlate *with*. Transfer sizes now span two
orders of magnitude.

Two new witnesses, both on their own lines so every pre-existing line stays byte-comparable with
captures taken before this arc:

* `:: BOT: pump shape stage= dir= len= trb_idx= wrapped= … result=TIMEOUT-SHAPE ::` — printed
  immediately after the existing TIMEOUT line, naming the transaction that did not complete.
* `:: BOT: tx single= multi= maxlen= wrapped_tx= rd_sectors= wr_sectors= max_blocks= result=SIZES ::`
  — the population those fields must be read against, at summary time.

How to read a metal TIMEOUT-SHAPE:

| observation | reading |
| --- | --- |
| `stage=data` with a large `len=` | the wedge prefers big TDs — a controller/stick burst boundary. Multi-block would then be trading M1 for M2 and the buffer size must come back down. |
| `stage=data len=512` | size is not the discriminator; the amplification cut is pure profit. |
| `stage=csw` | the data phase landed and only the 13-byte status event went missing — points at the event ring, not the transfer. This is what §12.3's `csw_sig=0x0` evidence *suggests*, and this line is what would confirm it. |
| `wrapped=true`, against `wrapped_tx=` / total transactions | the direct ring-wrap correlation test. §12.2 could only argue from ring arithmetic (16 TRBs, 3 pushed per transaction, so ~1 in 5 wraps); this observes it. |
| `timeouts=` split against `rd_sectors=`/`wr_sectors=` | whether M2 is direction-specific. §12's evidence is all from the write path, but that is also where all the traffic was. |

**Still open**, unchanged by this arc: why a completion event is lost at all while the controller
itself stays healthy (stop-ep/set-deq return cc=1, the dequeue pointer moves). Nothing here explains
that, and a boot that still wedges after this arc has *not* refuted the M1 analysis — it has confirmed
that M1 was only ever the amplifier.

### 13.6 aarch64 impact — inert today, liftable in one arc

`fs/fat.rs` and `drivers/block.rs` are shared with the Pi track, so this is stated precisely.

* **`drivers/block.rs`** — additive. The SD backend (`drivers::emmc2`) exposes only single-block
  CMD17/CMD24, so `read_blocks`/`write_blocks` **loop** those: byte-for-byte the same card traffic, in
  the same order, that the per-sector callers produced before. Nothing about the microSD path changes.
* **`fs/fat.rs`** — the coalescing is real on aarch64 too, but it lands on a block layer that
  immediately re-splits it, so the card sees no difference. Measured: `./arroyo test-arm 22` reports
  `n=92` before and `n=92` after, identical.
* **The lift is one arc, and the pi seat should want it.** Implementing CMD18 (READ_MULTIPLE_BLOCK) /
  CMD25 (WRITE_MULTIPLE_BLOCK) behind `read_blocks`/`write_blocks` puts this entire coalescing layer
  onto the microSD **with no filesystem change at all**, because the coalescing lives above that seam,
  not below it. The `alloc_cluster` rotating start and the directory-walk collapse are arch-neutral
  and benefit the microSD immediately.

### 13.7 Guard ladder — untouched, provably

`git diff --stat` for this arc names exactly three files: `drivers/block.rs`, `drivers/xhci/mod.rs`,
`fs/fat.rs`. `install/mod.rs` (the blank-check refusal and the verify ladder), `fs/vfs.rs`,
`shell.rs` and `flight_recorder.rs` are **not in the diff at all**, and `drivers/block.rs`'s diff
deletes zero lines. The destructive path therefore reaches the disk through the same single-block
functions, with the same bounds, that it did before.

### 13.8 Not done

* **`set_fat_entry` is still one sector RMW per FAT copy per link** (4 transactions per cluster
  linked). Batching a whole run of chain links into one FAT-sector write is the next available cut,
  and it is the largest remaining item on the fixture geometry — but it moves crash-ordering
  guarantees, so it wants its own arc rather than a corner of this one.
* **`ALLOC_HINT` is not persisted to FSInfo.** Writing that sector would be a new on-disk mutation on
  the destructive path; deliberately declined.
* **WINX-2 / WINX-8 are still FAIL**, before and after, on both geometries, and for a reason outside
  this lane: `killed=false (kill armed — the task retires at its next preemption)`. Storage is not
  what fails them — the FAT read side is demonstrably green in the same run.

---

## 14. BOT-RESCUE — escalation, surrender, and a timeout that names its own failure (2026-07-29)

### 14.0 The failure, and the audit verdict

**Metal ground truth (2026-07-29, x86).** A USB card reader (Alcor Micro `058f:6362`) went fully
non-responsive during a `WRITE(10)`. EP0 died with it — the device stopped answering control
transfers too, so this was not a bulk-pipe desynchronisation. The driver's BOT recovery ladder
(class reset → clear-halt ×2 → stop-ep/set-deq ×2 → one retry) ran to completion, reported success,
retried, failed, and was re-entered by the next block operation. Forever. `retry_ok=0` on every
cycle; ~6 s of busy-spin per stage timeout, which starved the desktop.

**Audit verdict — the ring code is EXONERATED.** The evidence is the DCS agreement: on every
recovery cycle the Set TR Dequeue Pointer command was a provable no-op, because the endpoint
context's `ctxdeq` *already* equalled the value the command was about to write, cycle state
included. The controller had fetched everything the driver produced and had advanced its dequeue
pointer past it. A driver-side ring that is already exactly where the controller is cannot be the
reason the controller produces no completion.

**The trigger is device-side, on a write.** The controller stayed healthy throughout — the FTDI
slot kept transferring on the same controller, the event ring kept delivering, and every command
completed `cc=1`. Nothing global was wedged; one device stopped answering.

**The defect was the ladder, not the fault.** A device can die; that is not a kernel bug. Looping on
a dead device at 6 s per attempt with no ceiling is. The ladder had no top (nothing stronger than a
class reset to escalate to) and no floor (no way to give up), so a permanent device failure became a
permanent system failure. §14.1 gives it a top, §14.2 a floor, §14.3 the instruments to tell which
was needed.

### 14.1 The escalation ladder

Rungs fire only after **`BOT_RESCUE_N_CONSEC = 2`** consecutive failed recovery+retry cycles on the
slot, and each fires at most once per streak. Two, not one: a single failure is exactly what the
existing class-level Reset Recovery exists to absorb (PIUSB-38's induced stall recovers on the first
try). Two, not more: every rung costs a full attempt budget of busy-spin, and no capture has ever
shown a device that failed the ladder twice recovering on a third.

Any transaction that **completes** — including one completing with a `Failed` CSW, because a device
that answers is not a device that is wedged — resets the streak and the rung counter to zero.

| Rung | Action | Spec | Notes |
|---|---|---|---|
| (existing) | Bulk-Only Mass Storage Reset + clear-halt ×2 + stop-ep/set-deq ×2 + one retry | BOT 1.0 §5.3.3/§5.3.4 | unchanged; §11a |
| **(a)** | ~~**Reset Device** for the slot, then Configure Endpoint onto rings reset to a known enqueue/cycle state~~ **RETIRED (M1a, 2026-07-30)** — replaced by a **ring rebase**: Stop/Reset Endpoint → `TransferRing::reset` → Set TR Dequeue Pointer at each ring base with DCS=1 | ~~xHCI 1.2 §4.6.11, §4.6.6~~ → xHCI 1.2 §4.6.9, §4.6.8, §4.6.10 | the honest limit below turned out to be fatal, not merely honest; see §16.12 |
| **(b)** | **Root-port power cycle** — PORTSC PP down 100 ms, up, 300 ms settle; re-enumeration delegated | USB 2.0 §7.1.7.3 | root ports only |
| **(c)** | **SURRENDER** — mark FAILED, retract, refuse all further transfers | §14.2 | terminal |

**Back-off.** 50 ms doubling to a 400 ms cap between cycles. A device wedged mid-internal-stall — a
flash controller in an erase/wear-levelling window is the usual suspect for a `WRITE` that kills EP0
— is made *worse* by hammering: each new transaction restarts its command timeout.

**Settles are spec-scale, not budget-derived.** `Self::cycles_per_ms()` reads the arch timebase
(calibrated TSC via `apic::tsc_hz` on x86, `CNTFRQ_EL0` on aarch64, each with an honest fallback).
Deriving the dwell times from `hw_wait_budget()` would silently rescale every USB timing constant
the day the timeout policy changed; VBUS de-energise and `bPwrOn2PwrGood` are not policy.

**Budgets.** The **first** attempt keeps its ~6 s (`hw_wait_budget() × 3`, unchanged) — a real device
can legitimately stall 1–4 s on a write, and shortening this would fail slow-but-healthy sticks.
**Escalation retries** use ~2 s (`× 1`): by then the device has burned two full ladders without
answering, the only question left is "did the heavy reset revive it", and a revived device answers
in milliseconds. The extra 4 s buys no information and is paid in frozen desktop. The multiplier
lives in `bot_budget_scale`, which is the historical `3` at all times except inside an escalation
retry.

**Rung (a) — the honest limit, recorded because the witness will show it.** Reset Device returns the
xHC's internal slot state to the post-Address condition without touching the Root Hub Port Number or
Route String, but it leaves the Slot in the **Default** state. This rung deliberately does **not**
re-address: the device was not port-reset, so it still holds its USB address and would not answer a
`SET_ADDRESS` at 0 — re-addressing without a port reset would be the *unsound* move, not the
thorough one. A controller that therefore refuses the follow-up Configure Endpoint with Context
State Error (`cc=19`) is **expected**, is printed as such, and simply means the rung did not take;
the ladder moves on to (b), whose port reset is what makes a re-address lawful. The rung is kept
because it is cheap (two bounded commands), because it is the correct fix when the fault is purely
the xHC's slot state, and because `cc=19` here is itself evidence about which half is sick.

> **Amended (2026-07-30, ONSET + M1a) — the reasoning above is vindicated; the conclusion is not.**
> The paragraph's central claim is **correct and was worth writing**: re-addressing without a port
> reset is unsound, and metal confirmed the `cc=19` it predicted, seven times across two builds
> (§16.7). What it got wrong is the cost. The rung is **not** cheap and it is **not** merely a no-op
> when it fails: Reset Device *succeeds*, leaving the Slot in **Default** and every bulk endpoint
> **Disabled** (`epin=1->0 epout=1->0` on every instance), which is strictly worse than where the
> ladder started, and from which nothing short of rung (b)'s port cycle recovers. Nor is Configure
> Endpoint reachable by any legal continuation: **neither BSR setting of Address Device reaches
> Addressed on a device that was not port-reset**, so there is no repair to insert. The rung has
> therefore been **retired**, not completed — [§16.12](#1612-m1a--what-the-code-lane-landed-and-the-correction-it-forces-on-167)
> has the replacement and the spec citations. `cc=19` remains evidence, but the price of collecting
> it was the slot.

**Rung (b) — re-enumeration is delegated, not open-coded.** Removing and restoring VBUS raises
Connect Status Change on the port; the settle drains the event ring, so the driver's own tested,
single-threaded, one-port-at-a-time `handle_port_status` path sees the disconnect and the reconnect
and re-enumerates onto a **fresh slot** — retracting the old slot's block-registry entry through
`dispose_disconnected_slots` on the way. Open-coding a synchronous re-address here would race that
path for the same port with no way to win. Every PORTSC write masks off PED (write-1-to-**disable**)
and PR (write-1-to-**reset**) per `clear_port_change`'s discipline, and the port is left **powered**
on every exit path. Hub-downstream devices are refused by this rung: their power is the hub's to
switch, via a class request on the hub's own slot, and reaching for it through a pipe that may
itself be the sick one is not a recovery.

### 14.2 Surrender semantics

Surrender does three things and then stops:

1. **Retract**, through GR7's `block::unpublish_usb_geometry(slot_id)` — the *same* machinery a
   physical unplug uses. That is the design point, not a shortcut: the FAT layer, the shell's `df`
   and the installer's per-frame disk list already handle a disk that **disappears**. Every block
   entry point re-reads the registry on each call and fails honestly with `BlockError::NotReady`;
   the installer's captured `BlockDeviceId` refuses to resolve rather than retargeting. A disk that
   has stopped answering is, to every consumer above the driver, indistinguishable from one that was
   pulled. Saying so in the one vocabulary they already understand beats inventing a second failure
   mode for them all to learn. Matching is by slot id, so a microSD published with `slot_id: 0` can
   never be retracted here.
2. **Print one verdict line** (`:: BOT: SURRENDER … ::`), naming the fault class, the streak, what
   each counter spent, and whether the block layer actually had an entry to drop.
3. **Refuse every later transfer to that slot**, at the top of `bot_transfer`. This gate is the arc's
   actual guarantee: a sick disk can never again spin the system at ~6 s per attempt forever. Every
   path into the driver's storage I/O passes through `bot_transfer`, so a caller that missed the
   retraction cannot revive the stall.

`storage_slot` is cleared and `storage_note` becomes `storage device FAILED (BOT rescue
surrendered)`, which is what `diskinfo` shows on a serial-less machine.

**Reversal is physical.** The surrender is cleared when the slot is **disposed** (disconnect) or
**re-enumerated** (`bring_up_storage`), so it binds to the disk that earned it and not to a recycled
slot number — a slot id handed to the next device must not refuse that innocent device's transfers.
A replug is a clean slate and needs no operator action beyond the replug.

### 14.3 Witness grammar, with a reading key

Four new timeout lines and one extended success line. The pre-existing
`:: BOT: pump budget=… result=TIMEOUT ::` and `… result=TIMEOUT-SHAPE ::` lines are **byte-unchanged**
so captures stay comparable across the arc boundary.

```
:: BOT: timeout pipes slot=S in_dci=D in_epstate=E in_ctxdeq=0x… in_dcs=B in_enq=I in_cycle=C in_ntrb=N
                      out_dci=… out_epstate=… out_ctxdeq=… out_dcs=… out_enq=… out_cycle=… out_ntrb=…
                      foreign=F result=TIMEOUT-PIPES ::
:: BOT: timeout trb wait=0x… pipe=in|out|unknown dw0=… dw1=… dw2=… dw3=… trb_cycle=B ring_cycle=C
                    trb_type=T result=TIMEOUT-TRB ::
:: BOT: timeout csw sig=0x… tag=0x… residue=R status=S valid=yes|no — <verdict> result=TIMEOUT-CSW ::
:: BOT: pump budget=… used=… peak=… … timeouts=… IMAN=0x… USBSTS=0x… result=OK ::
:: BOT: ring refuse slot=S dci=D dir=… enq=I cycle=C ntrb=N ctxdeq=0x… dcs=B — … ::
:: BOT: deqprobe slot=S dci=D enq=I running_epstate=E running_ctxdeq=0x… -> stopped_epstate=E
                 stopped_ctxdeq=0x… stop_ok=yes|no stop_cc=C stop_why=… restore_ok=yes|no
                 epstate_after=E verdict=… ::
```

**Reading key.**

* **`TIMEOUT-PIPES` (witness 1)** — *the* discriminator, and the one thing the 2026-07-29 capture
  could not print. For **both** bulk DCIs: EP State (§6.2.3: 0 Disabled, 1 Running, 2 Halted,
  3 Stopped, 4 Error), the controller's own TR Dequeue Pointer with its Dequeue Cycle State, and
  **our** enqueue index and cycle bit.

  **`ctxdeq` is only a position when `epstate` is not Running.** (Corrected 2026-07-29 by
  GUARD-STATE; the original key below was written as if the field were live and is what let the
  guard be built wrong.) The Output Endpoint Context TR Dequeue Pointer field is written back by
  the controller on a Running → Stopped/Halted transition, and otherwise set by Configure Endpoint
  and Set TR Dequeue Pointer (xHCI 1.0 §4.8.3, §6.2.3). While the endpoint is **Running** the
  field is architecturally undefined; real Intel silicon (Panther Point, xHCI 1.0) leaves it frozen
  at the last written **birth** value, while QEMU refreshes it live. The line therefore prints
  `in_ctxdeq=0x… (stale: EP running)` whenever `in_epstate=1`, and the same for `out_`. A tagged
  reading supports **no verdict at all** — not "the controller is behind us", not "the controller
  stopped fetching". Get a Stopped-state read first (the recovery ladder's own `stage=stop-ep`
  line brackets one) and read that.

  With a Stopped(3)/Halted(2) read in hand:
  * `ctxdeq` **behind** our enqueue → the controller never fetched the work. Host/endpoint fault;
    ring surgery is the right family of fix.
  * `ctxdeq` **past** the awaited TRB and `dcs` **agreeing** with our `cycle` → the controller
    fetched and issued everything. The **device** is silent, and no host-side ring surgery can help.
    This is the reading the audit reached by hand; it is now printed.
* **`deqprobe` (GUARD-STATE)** — the once-per-boot experiment that makes the above a recorded fact
  on whatever silicon the capture came from, instead of an inference. Fired exactly once, from the
  plain-success return of `bot_transfer` (a transaction that passed on its first attempt: both rings
  idle, nothing in flight, no sense handler, no escalation streak), on the bulk IN endpoint. It
  reads `epstate`/`ctxdeq` while Running, issues **Stop Endpoint**, reads both again from Stopped,
  then puts the controller back exactly where it said it was (**Set TR Dequeue Pointer** to the
  value just read, reserved bits 3:1 cleared, DCS kept) and rings the doorbell to return the
  endpoint to Running. If Stop Endpoint fails it stops there and reports `verdict=inconclusive`;
  it never leaves an endpoint parked.
  * `running_ctxdeq != stopped_ctxdeq` → `verdict=ctxdeq-stale-under-running`. The Running read was
    a birth value. Expected on Panther Point.
  * `running_ctxdeq == stopped_ctxdeq` → `verdict=ctxdeq-live`. QEMU's reading; its calibration for
    this arc is `enq=3 running_epstate=1 running_ctxdeq=stopped_ctxdeq stop_cc=1 restore_ok=yes
    epstate_after=1`.

  **ONSET correction (2026-07-30) — the `epstate_after` calibration above is backwards.** Metal
  reads **`epstate_after=3`**, twice, on two slots and two rings (`rmbp-gr8/ttyUSB1.log` lines 3710,
  4144). That is the *correct* reading and `epstate_after=1` is the QEMU artefact: the probe rings
  the doorbell on an **empty** ring and samples immediately, and real silicon does not leave Stopped
  until there is a TRB to fetch, while QEMU transitions eagerly. The endpoint is not parked — 218
  and 94 further awaited stages retired after the two probes respectively — so the claim above it
  ("it never leaves an endpoint parked") stands; only the expected value of the field is corrected.
  See [§16.1](#161-the-platform-grading-is-on-the-record--and-this-keys-calibration-line-was-backwards).
* **`foreign=F` (witness 4) — RETIRED. Do not read a verdict off this field.**

  > **Retraction (2026-07-30, ONSET).** This bullet previously read: *"`F > 0` proves the event ring,
  > interrupter and event delivery stayed alive for other traffic throughout, so a missing completion
  > is this slot's problem, not a global wedge. `F = 0` on a boot with other live devices (FTDI, a
  > HID) says the opposite and moves the investigation somewhere else entirely."* The second sentence
  > is wrong, and the instrument cannot support the first either. `pump_until_bot_done` is a
  > **synchronous spin** (`mod.rs:6947–6970` at `0825ed08`): while it waits, the driver submits no
  > FTDI transfer, so no FTDI TRB is outstanding, so no foreign Transfer Event can arrive. `F` is
  > pinned at 0 **by construction**, on a healthy boot exactly as on a wedged one — which is why all
  > 14 `TIMEOUT-PIPES` lines of `rmbp-gr8/ttyUSB1.log` read `foreign=0` across two driver builds. The
  > baseline discipline the bullet cites was applied correctly and did not save it: the field's
  > healthy-but-idle reading is indistinguishable from total failure, so it can falsify nothing. This
  > is the **fifth** entry in the instrument-lie ledger ([§16.9](#169-the-instrument-lie-ledger)).
  >
  > The liveness `foreign` was built to prove is proven *positively* on the same lines instead: the
  > recovery ladder's own `resync … cc=1` command completions arrive **during** the silence (boot F,
  > lines 3751–3754), so the command ring executed, the xHC posted events, the interrupter delivered
  > them and `drain_event_ring_once` consumed them. The replacement witness proposed for the code
  > lane is **`evts=`** — all event-ring TRBs consumed during this wait, of any type — which, unlike
  > `foreign`, can be non-zero, and that is what would make a zero reading mean something.
* **`TIMEOUT-TRB` (witness 2)** — the awaited TRB's four raw dwords **as read back from DRAM**
  (behind an invalidate), its stored cycle bit against the ring's live one, and its decoded type.
  A TRB whose cycle bit does not match the colour the controller expects is one the controller is
  entitled to ignore forever. Dwords that do not match what we believe we wrote mean the write never
  landed — a coherency/aliasing arc, not this one.
* **`TIMEOUT-CSW` (witness 5)** — the CSW buffer as the controller left it. The CSW is DMA-written,
  so a genuine timeout leaves the pre-transfer zero fill. `valid=yes` (signature `0x53425355`) means
  the status phase **landed** and only its completion event went missing — a different fault class,
  and the direct test of §12.3's lost-CSW hypothesis (see the SUPERSEDED-UNLESS note there).
  `valid=no` means the status phase never happened: a transport wedge.
* **`IMAN=` / `USBSTS=` on `result=OK` (witness 3)** — the healthy **baseline**. The timeout line has
  always printed both registers, but a reading with nothing to compare against cannot falsify
  anything: `IMAN=0x3` on a wedged boot is evidence only if the same instrument recorded what IMAN
  reads on a working one, on the same boot. The OK line is throttled to doubling peaks, so this stays
  logarithmic in the budget — a handful of lines per boot. QEMU's calibration for this arc:
  `IMAN=0x2 USBSTS=0x0`.
* **`ring refuse`** — M2's lap guard fired (§14.4). On a healthy device this line is unreachable.
* **`recover evidence … pipe=…`** — now carries the truth. It previously printed
  `pipe=none wait_trb=0x0 stage_done=no stage_cc=0` on **every capture ever taken**, because
  `run_bot_stage` took the pending record and dropped it before propagating the error, so recovery's
  read was always `None`. That was a structural lie about the driver's own state, not a finding about
  the device; any pre-2026-07-29 capture showing `pipe=none` should be re-read as *no data*.

### 14.4 Ring hygiene (M2)

Three fixes in the ring layer. The audit exonerated the ring as the **cause**, but a recovery ladder
that hammers a stalled endpoint is exactly what would step in these traps.

1. **`TransferRing` tracked no consumer position**, so `push` could lap a stalled controller — a
   direct xHCI 1.2 §4.9.1/§4.9.2 violation, since the Cycle bit is the only producer/consumer
   handshake and advancing past the dequeue pointer overwrites TRBs the controller has not fetched.
   `TransferRing::would_lap` now answers from the controller's own TR Dequeue Pointer, read out of
   the output device context by `bot_ring_guard`, consulted before each of the three bulk pushes in
   `bot_transfer_once`. A refusal is `BotError::RingFull`, which feeds the same Reset Recovery
   ladder. **Healthy path unaffected by construction:** each BOT stage is awaited to completion
   before the next is queued, so at most **one** TRB is outstanding on a 16-TRB ring; the refusal
   threshold is 14.

   **ONSET correction (2026-07-30) — the premise in bold above is false in our own source.**
   `bot_transfer_once` pushes the CBW (`mod.rs:5317`) and then the data TRB (`mod.rs:5345`) with
   **no pump between them**, and the CBW is built `control: 1 << 10` — Normal type, **no IOC, no
   ISP** — so it posts no completion at all and the driver has no witness that it was ever consumed.
   For an OUT data stage `data_dci == out_dci`, so the two TDs ride the **same** ring under a
   **single** doorbell (`mod.rs:5377–5378`). At least **two** TRBs are therefore outstanding on the
   OUT ring at every write, not one. (Line numbers as of `0825ed08`, before M1a's changes to this
   file.) This is the *same* premise the already-retracted §14.5 rested on: the
   "at most one TRB outstanding" claim was never established, it was assumed, and it is now known to
   be wrong in two independent ways. The refusal threshold of 14 is not endangered by two — the
   correction is to the argument, not to the number — but no later reasoning may cite the premise.
   See [§16.3](#163-the-by-construction-premise-in-144-item-1-is-false-in-the-source).

   > **SUPERSEDED (2026-08-03, BOT-CBW) — the correction above is itself now historical, and the
   > bolded premise it corrected is TRUE AGAIN.** The ONSET correction describes the source as it
   > stood at `0825ed08`. Since BOT-CBW the CBW is pushed with `control: (1 << 10) | (1 << 5)`
   > (Normal | IOC), rings its own doorbell and is pumped to completion before the data stage is
   > built, so **at most one TRB is outstanding** — by construction in the source, not merely in the
   > argument. See [§17.3](#173-the-mechanism). Read the correction as: the premise was false when
   > §14.4 was written, and asserting it without checking is what the audit caught. Its standing
   > instruction ("no later reasoning may cite the premise") is **lifted for the current source**
   > and **retained for any capture predating 2026-07-30** — those boots really did run two TDs
   > under one doorbell, and §16.0's provenance rule applies to them.

   **GUARD-STATE correction (2026-07-29).** That "by construction" argument was wrong on real
   silicon, and §14.5 below retracts the claim it supported. `bot_ring_guard` compared our live
   enqueue against a field that is only meaningful when the endpoint is **not** Running (§14.3's
   corrected key). On Panther Point the Running read is the frozen birth value, so as soon as our
   enqueue ran 14 TRBs past the ring base — which a healthy device does in the ordinary course of
   traffic — `would_lap` returned true and the guard manufactured a `RingFull` on a device that was
   working perfectly, dragging it into recovery, reset-device, port-cycle and SURRENDER. The guard
   now reads `ep_state_of` **first** and refuses only from Halted(2), Stopped(3) or Error(4); for a
   Running endpoint it returns `Ok` unconditionally, which is the pre-M2 behaviour. That is correct
   and not a retreat: the lap hazard the guard exists for only materialises across recovery retries,
   and those run against Stopped endpoints — exactly where the guard still applies unchanged.
2. **`push_noop` hard-coded `cycle=1`.** Today's only caller is the command ring's bring-up probe,
   where the ring is virgin and `cycle_bit` is already true — a no-op for every call the driver
   makes. It was a trap for any later call: after one wrap the hard-coded 1 is the *stale* colour and
   the ring silently stops being consumed. It reads the field now.
3. **`EventRing::clear` zeroed 256 BYTES, not 256 TRBs** (16 of 256 slots), and reset neither
   `dequeue_index` nor `cycle_bit` — a cleared ring would be consumed from the middle with the wrong
   expected colour, replaying 240 stale entries as fresh events. It is **currently uncalled** and
   stays that way; fixed because a two-line helper that silently corrupts the consumer handshake is
   what a future controller-reset path would reach for. The ERDP is deliberately not written there.

`TransferRing::reset` returns a ring to a known enqueue/cycle state; rung (a) needs it, because
continuing from wherever a failed transaction left the pointers would leave driver and controller
disagreeing about both position and colour from the first push.

### 14.5 Blast radius on a healthy device — RETRACTED, and the honest account

> **Retraction (2026-07-29, GUARD-STATE).** This section previously asserted that BOT-RESCUE could
> not touch a healthy device "by construction". That was false on metal. The lap guard (§14.4 item 1)
> was itself the blast radius: it read the Output Endpoint Context TR Dequeue Pointer under a
> **Running** endpoint, where on Intel Panther Point the field is frozen at its birth value, compared
> that frozen value against our advancing enqueue, and refused a stage on a perfectly healthy
> mid-traffic device. The refusal is a `BotError::RingFull`, which is a transport fault, which enters
> Reset Recovery — and because the guard fires again on the retry for the same reason, the streak ran
> the full ladder: reset-device, port-cycle, SURRENDER. BOT-RESCUE stormed working disks. Every gate
> was green throughout, because QEMU refreshes the field live and the guard therefore never misfires
> in emulation. The bullets below were true of the escalation ladder and false of the guard; the last
> one is now correct only because GUARD-STATE made it so.
>
> The general lesson, recorded because it outlasts this bug: an argument of the form "this can never
> fire on a healthy device" is a claim about the *reading* the predicate takes, not just about the
> predicate. It is only as good as the guarantee that the field being read means what the argument
> assumes — and here nothing in the code or the doc had ever established that. `deqprobe` exists so
> that assumption is measured on every boot rather than asserted.

* The surrender gate is `bot_surrendered_slot == slot_id`; that field is set **only** by rung (c),
  reachable only after two consecutive failed recovery+retry cycles *and* two failed rungs.
* Every completing transaction calls `bot_rescue_clear`, so streak and stage are 0 on a healthy
  device and no rung is reachable.
* `bot_budget_scale` is the pre-arc `3` except inside an escalation retry, restored immediately.
* The `Ok` / `Ok(Failed)` / `Err(NoDevice)` / retry-succeeded returns of `bot_transfer` are the
  pre-arc returns verbatim. The only changed return is the terminal `Err(cause)` after a failed
  recovery, which now pays a back-off first and returns the same error below `N_CONSEC`.
* The lap guard is three volatile reads and arithmetic before the ring is touched, and — **since
  GUARD-STATE, and only since** — returns `Ok` on every healthy transfer, because it now bypasses
  entirely for a Running endpoint. Before that it was the one thing in this arc that *did* have a
  blast radius on a healthy device.
* Witnesses are pure reads — endpoint contexts, ring fields, DRAM behind an invalidate, two MMIO
  reads. No dispatch decision is taken from the foreign-event counter, so event routing is unchanged.
  The one exception, added by GUARD-STATE and named as such: **`deqprobe` issues commands** (one Stop
  Endpoint, one Set TR Dequeue Pointer, one doorbell), exactly once per boot, on an idle endpoint
  immediately after a first-attempt success, and restores the controller to the position it reported.
  It is a deliberate, bounded, once-only disturbance, and it is the price of not having to infer this
  class of fact from a rare capture ever again.

### 14.6 Gates, and what QEMU cannot prove

* `./arroyo check` green for **both** arches — the shared file still builds for aarch64, and none of
  the new code is x86-gated, so the pi seat inherits the same ladder and the same healthy path.
* `./arroyo test` — **31 PASS / 0 FAIL**, baseline maintained; and again under `UNAOS_BOTFAULT=1`,
  where the injected CSW-stage failure is rescued by the **existing** first rung
  (`retry result=pass recoveries=1 retry_ok=1`) and **no** escalation fires — the intended reading.
* `./arroyo test-fat sf 300` — the FAT-attached config still loads both ELFs.
* `strings` on the artifact finds `result=TIMEOUT-PIPES`, `result=TIMEOUT-TRB`,
  `result=TIMEOUT-CSW`, `stage=reset-device`, `stage=port-cycle`, `BOT: SURRENDER`, `ring refuse`.
* GUARD-STATE re-ran the same set: `./arroyo check` green both arches; `./arroyo test 90` **31 PASS /
  0 FAIL**; `./arroyo test-fat part 90` **41 PASS / 0 FAIL + 1 LIVE**; `strings` on `kernel.elf`
  finds `BOT: deqprobe`, `(stale: EP running)` and `ctxdeq-stale-under-running`. The FAT gate ran
  4005 pump waits with `timeouts=0` and **no `ring refuse` line**.

**QEMU cannot exercise the escalation.** Its `usb-storage` never stalls, so no streak can reach
`N_CONSEC`, and rungs (a), (b) and (c) — along with the three timeout witness lines and the lap
refusal — are **never reached** in emulation. This is stated plainly rather than claimed as coverage.
Only witness 3's healthy baseline is exercised there, and it printed.

**QEMU also cannot prove the GUARD-STATE fix.** Its context TR Dequeue Pointer refreshes live, so the
guard never misfired there in the first place — every gate was green before the fix and is green
after it, and that agreement is worth exactly nothing as evidence. The proof is the `deqprobe` line
on the next metal boot: `verdict=ctxdeq-stale-under-running` establishes the premise on the affected
silicon, and the absence of a `ring refuse` line across a full FAT workload establishes the
consequence. QEMU's contribution is the control: it printed `verdict=ctxdeq-live`, which is the
platform difference itself becoming a recorded fact rather than a surprise.

### 14.7 What metal must verify

1. **The ladder terminates.** A wedged reader must produce at most: two ordinary recovery cycles,
   one `stage=reset-device`, one `stage=port-cycle`, one `SURRENDER` — and then silence on that slot.
   Total wall-clock from first failure to surrender should be well under a minute, against the
   previous *unbounded*.
2. **Witness 5's verdict.** `valid=yes` at timeout would resurrect §12.3's M2 reading; `valid=no`
   would retire it and confirm the transport-wedge class.
3. **Witness 1's `ctxdeq` vs `enq`.** Confirms or refutes the audit's device-silent verdict directly,
   instead of by inference from set-deq being a no-op.
4. ~~**`foreign=`** should be non-zero on any boot with the FTDI console attached. Zero would mean
   the interrupter *was* globally wedged after all, contradicting the audit.~~
   **RETIRED (2026-07-30, ONSET).** The expectation was structurally wrong: the pump is a synchronous
   spin, so no foreign transfer is ever outstanding while it waits and `foreign=0` is guaranteed on
   healthy and wedged boots alike. Metal duly printed `foreign=0` on all 14 timeouts, and it means
   nothing. Retraction and the replacement witness are in §14.3's `foreign=F` bullet.
5. **Rung (a)'s `cfgep_cc`.** A `cc=19` is expected (§14.1); anything else is new information.
   **Answered and closed (2026-07-30, ONSET).** Metal printed `cc=19` seven times across two builds,
   and §16.7 explains it: Reset Device succeeds, leaves the Slot in Default, and Configure Endpoint
   is then architecturally required to fail. **The rung has since been retired** (M1a), so this
   expectation no longer has a rung to attach to — see §14.1's amendment and
   [§16.12](#1612-m1a--what-the-code-lane-landed-and-the-correction-it-forces-on-167).
6. **The desktop stays live** through a disk failure — the whole point.
7. **`deqprobe`'s verdict** (GUARD-STATE). `ctxdeq-stale-under-running` on Panther Point confirms the
   premise of the guard fix. `ctxdeq-live` there would mean the s53 capture is explained by something
   else and the fix, while harmless, is not the whole story.
8. **No `ring refuse` line on a healthy boot.** This is the fix's actual assertion. Any `ring refuse`
   now carries `epstate` 2, 3 or 4 by construction, so it is a real finding rather than an artefact.

   **ONSET verdict (2026-07-30).** The **primary assertion HOLDS on metal.** All 9 `ring refuse`
   lines in `rmbp-gr8/ttyUSB1.log` (1852, 1882, 2026, 2343, 2582, 2786, 2839, 3188, 3241) are in
   **pre-GUARD-STATE** boots; the two tip boots carry **zero** across 218 + 94 awaited stages.
   Three of the nine read `ctxdeq=0x2020b181`, which the tip build's own `deqprobe` (line 4144)
   labels `running_ctxdeq` and identifies as slot 1's bulk-IN ring **base | DCS=1** — the pre-fix bug
   and the post-fix instrument that names it, on the same silicon in the same file.

   **The sub-claim is unverifiable on any capture ever taken.** The `ring refuse` format string
   (`mod.rs:5014–5018` at `0825ed08`) emits `slot dci dir enq cycle ntrb ctxdeq dcs` and **no
   `epstate`**. The construction argument is sound *in source* — `bot_ring_guard` matches
   `ep_state_of` against `2 | 3 | 4` and returns `Ok` for everything else **before** reading `ctxdeq`
   (`mod.rs:5000–5004`) — but a claim whose witness is absent from the log is an **inference, not a
   finding**, and no capture can ever promote it. The field is [§16.10](#1610-what-the-next-metal-boot-must-be-able-to-print)'s
   first instrumentation ask.

### 14.8 Still open — each owed its own arc

> **Superseded in part (2026-07-29, BOT-PHASE — see [§15](#15-bot-phase--the-phase-desync-holes-and-the-cbw-found-in-a-directory-sector-2026-07-29)).**
> Item 1 below has since been given its mechanism. A read-only audit recovered a **Command Block
> Wrapper written into a FAT directory sector**, and its `dCBWTag = 28` matches the first-failing
> transaction of captured storm boot GR8-B3 — joining code, capture and medium at one transaction
> number. The cause is a **dirty ring at every error exit**: abandoned TRBs left live with the
> controller's dequeue on them, replayed by the next doorbell into a device whose phase machine had
> moved on. §15 closes it with six fixes. What survives from item 1 is narrower than it is written
> here: the **loss of the FIRST completion** — the event that starts a storm — is still unexplained,
> and ~~`foreign=0` across the silence is still the part most wanting one~~. Item 2 is untouched by
> that work and remains fully open. `cfgep_cc=19` (§14.7 item 5) is made *safe* by §15's fix 6 but is
> still not *explained*, and is now tracked in §15.9.
>
> **Amended (2026-07-30, ONSET — see [§16](#16-onset--the-gr8-cold-read-what-the-capture-establishes-and-what-it-falsifies-2026-07-30)).**
> The `foreign=0` clause is struck: the field is 0 by construction on this platform and wants no
> explanation (§14.3's `foreign=F` retraction). The first-completion loss remains open, now with a
> reproduced, index-locked shape (§16.8). Item 2's figure did not survive the arithmetic and is
> restated below. `cfgep_cc=19` is **closed** as a sequencing defect of ours (§16.7), which also
> answers §15.8 item 7 in the negative.

Two facts about the metal reader survive GUARD-STATE untouched. Neither is explained by the stale-field
defect, and neither should be quietly absorbed into the story of it.

1. **The pre-guard genuine transient (GR7 era).** On a fixed workload, the reader times out
   deterministically at **n = 68**, with **~6.4 s of total event silence**, `foreign=0`, and recovery
   on the ladder's ordinary retry. This predates the lap guard entirely — it is the failure BOT-RESCUE
   was built to survive, not one BOT-RESCUE caused. Deterministic recurrence at a fixed transaction
   index is the strongest handle available and has not been pulled; ~~`foreign=0` across the silence
   is the part that most wants an explanation, since it says the interrupter went quiet globally.~~

   **ONSET update (2026-07-30). The handle has now been pulled, and the `foreign=0` clause is
   struck.** Boots B and G of `rmbp-gr8/ttyUSB1.log` are the same failure twice — same awaited-stage
   index (`n=94`), same `single=44 multi=3 wrapped_tx=2`, same `io-cause op=write lba=17303`, `peak`
   agreeing to 6 ppm — across two *different* driver builds. The recurrence is reproduced and
   index-locked, and it survives the GUARD-STATE and BOT-PHASE fixes exactly as §15.9 item 1
   predicted. `foreign=0` says nothing about the interrupter (§14.3's retracted bullet); the
   interrupter is demonstrably alive across the silence. The shape, the authoritative dequeue read
   and the ranked lead are in [§16.8](#168-still-open-the-first-completion-loss-now-has-a-shape-and-a-lead).
2. **~9 ms mean per 512-byte transfer.** ~~Two to three orders of magnitude slower than the transfer
   itself can account for. The shape is SMI-like (long, opaque, host-side stalls), and the xHCI SMI
   enable bits have been **confirmed clear** — so the obvious suspect is already eliminated and the
   real source is unidentified.~~

   **ONSET correction (2026-07-30) — the figure does not survive the arithmetic, and the SMI-shaped
   reading has nothing behind it.** The "~9 ms" was a small-`n` mean dominated by **one** sample: a
   single reproducible **~625 ms** wait, the *first* BOT stage after `SET_CONFIGURATION`, whose value
   is spread under 1% across five of the seven boots — the signature of a device-side media-init
   timer, not an opaque host stall. Excluding it, the per-awaited-stage mean is **0.94–1.9 ms**, and
   boot D — which has no such outlier — reports a **raw** mean of **0.94 ms** with nothing removed.
   That residue is one to two ticks of our own **1000 Hz** APIC heartbeat: `IRQ_COUNT=0` on every
   boot, so the xHCI interrupt is never taken and the polled pump's `hlt()` can only be woken by the
   1 ms tick. The item is **restated, not deleted**: there is still a real cost here — roughly
   1.9–3.4 ms per transaction, ~3–8× a fully serialised BOT transaction's floor — but it is a
   **polled-driver quantisation artefact of ours**, not a two-to-three-orders-of-magnitude platform
   stall, and no further effort should go into hunting a host stall on this evidence. Derivation in
   [§16.6](#166-the-pace-anomaly-is-our-own-1-khz-polled-pump). This remains a throughput ceiling on
   USB storage on this platform rather than a correctness bug.

   **The SMI half of the old claim has no witness at all.** "The xHCI SMI enable bits have been
   confirmed clear" is not supported by any capture: xHCI prints only `BIOS->OS handoff complete.`
   and never prints USBLEGSUP/USBLEGCTLSTS, while EHCI prints both for both controllers every boot.
   See [§16.11](#1611-checklist-items-this-capture-closes--and-one-it-does-not).

---

## 15. BOT-PHASE — the phase-desync holes, and the CBW found in a directory sector (2026-07-29)

### 15.0 The finding

A read-only audit of a corrupted USB stick recovered a FAT directory entry whose bytes were **not a
directory entry**. They were a **Command Block Wrapper** — the driver's own 31-byte BOT command
header, written into a sector that belonged to the filesystem.

That is the whole arc in one sentence: the driver put a command where data belonged. Everything
below is the mechanism that allows it, the six holes that make up that mechanism, and the witnesses
that make each one countable instead of reconstructible only from a wrecked medium.

This is a different class of defect from §14. BOT-RESCUE was about a device that stopped answering
and a ladder with no top — a **liveness** failure, loud and self-announcing. This is a **safety**
failure: silent, and it damages the medium. Every gate in this tree was green throughout the period
the corruption was created.

### 15.1 The mechanism: a dirty ring is a phase slip

A BOT transaction is three serialized phases on two bulk rings:

```
   bulk OUT ring:   [ CBW ] ---> [ DATA(out) ] ................
   bulk IN  ring:   ............ [ DATA(in) ] ---> [ CSW ]
```

The device runs a phase state machine of its own, and the two machines stay in step **only** because
each side retires one phase before starting the next.

Every error exit from `bot_transfer` used to return with whatever it had already pushed **still on
the rings**, and with the controller's TR Dequeue Pointer still parked on those TRBs. Nothing retired
them and nothing repointed the controller. So the *next* transaction's doorbell did not start a new
transaction — it **resumed the abandoned one**, replaying a stale CBW, and on the write path a stale
payload, into a device whose phase machine had moved on.

From there the two machines run exactly one phase apart. What the host sends as *data* the device
reads as a *command*; what the host sends as a *command* the device consumes as *data* and writes to
the medium. A 31-byte CBW landing in a data phase is written to whatever LBA the device last
latched. That is how a Command Block Wrapper ends up in a directory sector.

Three properties make this worse than it first looks:

* **The shared-ring aggravator.** The CBW and an OUT data stage ride the **same** bulk-OUT ring. An
  abandoned WRITE therefore strands *both* — a command wrapper **and** up to 32 KiB of file payload,
  in that order, ahead of the next doorbell. On the write path the compounding case is the common
  one.
* **Addresses recur.** The rings are 16 TRBs and a transaction pushes up to three, so a given TRB
  address comes round again roughly every five transactions. A completion matched by address alone
  cannot tell a live stage from a long-retired one.
* **It is self-perpetuating.** Once out of phase, the next transaction fails too — and failed the
  same dirty way, re-arming the same trap.

### 15.2 Medium forensics — code → capture → medium

The corrupted card was re-inserted and read back. Raw device access was refused, so the readings
below are the **kernel-decoded views** through a mounted vfat; where the decode depends on the vfat
driver's own field mapping that is stated rather than glossed.

A 31-byte CBW overlaid on a 32-byte FAT 8.3 directory entry aligns like this — and this table is
what turns the wreckage into a readable record of the transaction that caused it:

| dirent field | offset | CBW field at that offset |
|---|---|---|
| `name[0..3]` | 0–3 | `dCBWSignature` = `55 53 42 43` (`"USBC"`) |
| `name[4..7]` | 4–7 | `dCBWTag` (LE) |
| `name[8..10]` | 8–10 | `dCBWDataTransferLength` bytes 0–2 |
| `attr` | 11 | `dCBWDataTransferLength` byte 3 |
| `NTRes` | 12 | `bmCBWFlags` |
| `crtTimeTenth` | 13 | `bCBWLUN` |
| `crtTime` | 14–15 | `bCBWCBLength`, `CDB[0]` (opcode) |
| `crtDate` | 16–17 | `CDB[1..2]` |
| `lstAccDate` | 18–19 | `CDB[3..4]` |
| `fstClusHI` | 20–21 | `CDB[5..6]` |
| `wrtTime` | 22–23 | `CDB[7..8]` |
| `wrtDate` | 24–25 | `CDB[9..10]` |
| `fstClusLO` | 26–27 | `CDB[11..12]` |
| `fileSize` | 28–31 | `CDB[13..15]` + one byte past the CBW |

**What the medium says.**

1. **The name is the signature.** Raw name bytes `55 53 42 43 1c` — `"USBC"` followed by `0x1C`, the
   name terminating at the first invalid byte. This is not an inference from a pattern; it is the
   BOT signature constant, in a directory entry.

2. **The tag names the transaction.** `name[4..7]` is `dCBWTag` little-endian, and the observed
   bytes give **`dCBWTag = 28`**. The audit's first-timeout table for the GR8 boots reads
   `n = 142, 94, 28, 16, 17`, and **28 is the first-failing transaction of GR8-B3**. The CBW on the
   medium is the first-failure transaction of a captured storm boot. That is the closing link of the
   chain — **code → capture → medium, joined by one transaction number**. No step of it is
   circumstantial.

3. **The size confirms the overlay.** `fileSize = 973,078,528 = 0x3A000000`, i.e. bytes 28–31 read
   `00 00 00 3A`. `CDB[13..15]` are zero (a 10-byte CDB zero-padded to 16), and `0x3A` is the byte
   *past* the 31-byte CBW. The file size is not a size at all; it is CDB padding plus one byte of
   whatever followed the wrapper in the staging buffer.

4. **The timestamp recovers the transfer length.** `wrtTime = CDB[7..8]`, and for a `WRITE(10)` CDB
   those are the transfer length in **blocks, big-endian**. `st_mtime` decodes to raw `315533280` =
   the FAT epoch (1980-01-01 UTC, `315532800`) **+ 480 s = 8 minutes exactly**. Reading that
   backwards: FAT time packs minutes in bits 10:5, so 8 minutes is the field value `0x0100`, so
   `CDB[7] = 0x00, CDB[8] = 0x01` — **a transfer length of one block**. A single-sector `WRITE(10)`,
   which is exactly the shape of the flight recorder's dirent-update write. The exact epoch offset
   also shows no timezone skew was applied to this field, so the derivation stands on its own.

   **`st_ctime` is not evidence.** It reports the same 8-minute value, but `crtTime` is
   `bCBWCBLength, CDB[0]` = `0x0A, 0x2A` = `0x2A0A`, which decodes to 05:16:20, not 00:08:00. The
   agreement is therefore a Linux vfat artefact (ctime mirroring mtime), not a second independent
   reading. It is recorded here so nobody later cites it as corroboration.

5. **`lstAccDate` is a partial LBA, with a caveat.** `lstAccDate = CDB[3..4]`, which for `WRITE(10)`
   are LBA bits 23:16 and 15:8. The decoded date, 2026-07-29, would give `LBA[23:16] = 0xFD` and
   `LBA[15:8] = 0x5C`. **Do not rely on this**: 2026-07-29 is also the date the card was re-read, and
   a read-write mount updates `lstAccDate`. Confirming the target LBA needs the raw sector.

6. **The head is a phantom.** The file's first 64 bytes read as zeros — `fstClusLO`/`fstClusHI` are
   CDB bytes, so they address a cluster chain that was never allocated to anything.

7. **The entry is a ghost.** `readdir` shows it; `stat` on the path fails with *No such file*. The
   invalid byte in the name means the entry can be listed but not opened or deleted by normal means.
   Recorded here so that this dead end is not re-investigated as a separate filesystem bug — it is a
   consequence of the corrupt name, nothing more.

8. Linux additionally warned at mount: *"Volume was not properly unmounted. Some data may be
   corrupt."*

### 15.3 Cross-platform: the same hole family on VL805

The Pi seat's independent audit of the aarch64 BOT path found **the same hole family** on Broadcom
VL805 silicon — three of the six exposed, plus a push with no capacity check at all. The mechanism
is therefore not an artefact of one controller, one vendor's quirks, or one arch's code path: it is
in the shared BOT state machine, and it is portable. That is the strongest available argument that
the fixes belong at this layer rather than behind a platform quirk.

### 15.4 The fix set

Six fixes, in audit priority order. Fixes 1, 2 and 6 close ways the driver can leave hardware and
software disagreeing; fixes 3 and 4 close ways it can *believe a false report* about what happened.

**1. The single chokepoint — no error exit returns with a dirty ring.** `bot_transfer` is now a thin
wrapper around `bot_transfer_body`; every `Err` other than `NoDevice`/`BadRequest` (raised before
anything is built or queued) passes through `bot_clean_rings`, which stop/reset-endpoints, Set TR
Dequeue Pointer on **both** bulk rings, and drains the event ring. `resync_bulk_ep` is the tool — it
already existed and was already correct; the defect was never the tool, only that the error paths did
not call it.

Wrapping the whole body, rather than patching the known exits, is the point. It covers the
below-`N_CONSEC` early return and the terminal escalate return the audit named; it covers the
`RingFull` refusals, the stalled status stage, and — stated explicitly because two boots in the
capture ran it — **the pre-BOT-RESCUE shape, a bare `return Err` with no cleanup of any kind**. It
also covers whatever exit the next arc adds, which an enumeration of exits would not.

It cleans **both** rings unconditionally, not just the pipe the failed stage awaited, because of the
shared-ring aggravator in §15.1: naming one pipe would leave the other loaded with a CBW and a
payload.

**2. The ring guard runs before the CBW push.** The lap guard used to be checked per stage,
immediately before that stage's own push — one stage too late. The CBW went on the OUT ring first,
so an `Err(RingFull)` from the *data* or *status* guard returned with the CBW stranded, un-rung and
unretired: the guard meant to prevent a dirty ring was manufacturing one. All rings the transaction
will touch are now checked up front, so a refusal leaves both rings byte-untouched.

The same pass removed three discarded `push` results (`.ok()`, `.unwrap_or(0)`) on the CBW/DATA/CSW
paths. `push` cannot fail today, so this is behaviourally inert — but `.unwrap_or(0)` would have made
a failed push wait on `ring_base + 0`, a real and recurring TRB address, which is another aliasing
vector into fix 4. A fabricated wait address is never better than an honest failure.

**3. Short-transfer honesty.** `run_bot_stage` returned only the completion code, so the Transfer
Event's residue — the bytes that did **not** move — was discarded, and `cc=13 SHORT PACKET` was
accepted as success with the CSW queued behind it. `BotPending` now carries `residue`
(first-write-latched against duplicate-Success quirks, like `Ep0Pending::data_seen`), and the data
stage is checked against its own `dCBWDataTransferLength`.

The treatment is deliberately **asymmetric**, and the asymmetry is the spec's:

* **OUT short is a fault.** BOT 1.0 §6.7.3 case 9 (Ho > Do): the device stopped accepting bytes, so
  it is *not* in its status phase. Queueing the CSW there is precisely the step that slides the two
  machines apart. It now feeds the recovery path.
* **IN short is not.** §6.7.2 case 4 (Hi > Di): `REQUEST SENSE` (18), `INQUIRY` (36) and
  `READ CAPACITY` (8) all name a *maximum* allocation length, and a conforming device may return
  less and then sit in its status phase with the shortfall in `dCSWDataResidue`. Failing those would
  break bring-up on correct hardware.

The IN case is instead given a check with real teeth: **`dCSWDataResidue` is now validated.** It was
decoded and handed to the caller but never compared with anything — so a transfer that moved zero
bytes and returned `bStatus = 0` with a full residue was reported to the FAT layer as a clean
success. The device's residue is *its* claim about how many bytes moved; the Transfer Event residue
is the *controller's*. Two independent witnesses of one quantity: if they disagree, one of the two
state machines is a phase out, and that is not success.

**4. De-aliased event matching.** `handle_event_trb` matched a BOT stage by TRB address and
*additionally claimed any error completion on either bulk DCI*. That blanket claim spans a slot's
whole bulk traffic, and addresses recur every ~5 transactions, so between them a stale event could
retire a live stage with someone else's completion code. Two narrowings, both minimal:

* The blanket error claim is gone. Errors that name a TRB are matched by address like anything else
  — a bulk STALL carries its TRB pointer, so the property the blanket claim was added for (a stalled
  command must not burn the full pump timeout) is preserved. The fallback survives only for an error
  whose pointer addresses nothing in either bulk ring (Ring Underrun, Ring Overrun, VF Event Ring
  Full), where "it can only be ours" is the only attribution available — and it is **counted**
  (`ev_unaddressed=`) rather than silent.
* A first-write latch on `done`: a second event for an already-completed stage is refused rather
  than allowed to overwrite the recorded completion code, and counted (`ev_late=`).

`BotPending` also carries a monotonic `generation`. It is **not** a wire tag and does not pretend to
be — a Transfer Event carries only a TRB pointer, so nothing can round-trip an identity. It is the
log key that ties a completion, a strand line and a timeout to one stage in a log where addresses
repeat.

**5. `send_scsi_read` deleted.** An uncalled fire-and-forget legacy path that pushed a hand-built CBW
with a **hardcoded tag** (`0xDEADBEEF`) and a data TRB onto both bulk rings, rang both doorbells, and
awaited nothing. It could not have been reached by any live call path, and it would have been a
loaded gun aimed at this state machine — a permanent supply of untracked TRBs and a tag no CSW
validation could ever match. Removed rather than documented.

**6. The `cfgep_cc=19` disagreement.** `rescue_reset_device` `reset()`s both rings — driver-side
enqueue 0, cycle 1 — because the Configure Endpoint that follows was going to point the controller at
each ring's base with DCS=1 in the same breath. When that command fails, and on metal it fails with
`cc=19` Context State Error (§14.7 item 5 already expects it), the controller was **never repointed**:
it is parked wherever the wedged transaction left it, on a ring whose contents the reset just zeroed,
while the driver believes it is at the base. The next push then writes a TRB the controller never
fetches, or one it fetches at the wrong colour. The rung now issues an explicit Set TR Dequeue
Pointer at each ring's base with DCS=1 — exactly what the failed Configure Endpoint would have
programmed, and legal from Stopped/Error (xHCI 1.2 §4.6.10). If that fails too, the rung still
returns false and the ladder proceeds to the port cycle, where re-enumeration rebuilds both sides.
What can no longer happen is a quiet return with the two sides out of step.

> **Amended (2026-07-30, ONSET + M1a).** The repoint's *intent* is right and is retained, but at the
> call site described above it could never land: by the time it ran, Reset Device had already left
> both endpoints **Disabled**, and Set TR Dequeue Pointer is legal only from Stopped or Error — so
> metal printed `ok=no cc=19 … epstate=0` on both pipes and the following retry failed with
> `completion code 12` (Endpoint Not Enabled). That is §15.8 item 7 answered **NO** (§16.7). M1a
> **retired the surrounding Reset Device + Configure Endpoint rung** and rebuilt it as a ring rebase
> from Stop/Reset Endpoint onward, which is the only state where this repoint is reachable at all.
> The fix is unchanged; what changed is that it is now placed where it can execute.
> [§16.12](#1612-m1a--what-the-code-lane-landed-and-the-correction-it-forces-on-167).

### 15.5 Witness grammar

| line / field | where | says |
|---|---|---|
| `:: BOT: strand when=pre\|post … epstate= enq= cycle= ntrb= ctxdeq= dcs= ctxdeq_valid= gap= live= gen= ::` | every error exit, per pipe | the ring as the error found it, and as the cleanup left it |
| `:: BOT: clean slot= cause= in_resync= out_resync= in_live= out_live= undrained= ::` | every error exit | whether the cleanup succeeded on both pipes |
| `:: BOT: dtl_vs_moved slot= dir= dtl= moved= residue= cc= verdict= ::` | any short data stage | host-side shortfall, and how it was judged |
| `:: BOT: residue_disagree slot= dir= dtl= host_moved= dev_residue= dev_moved= bstatus= ::` | CSW validation | the device and the controller disagree about bytes moved |
| `:: BOT: csw_bytes slot= why= tag_want= b=… ::` | every CSW rejection | **all 13 raw CSW bytes** |
| `:: BOT: rescue stage=repoint …` | fix 6 | the controller being repointed after a failed Configure Endpoint |
| `tag_mismatch= bad_sig= abandoned_in= abandoned_out= undrained= short_in= short_out= ev_late= ev_unaddressed=` | the `result=SUMMARY` line | boot totals for all of the above |

Two of these deserve their reading key stated, because a witness read wrong is worse than none:

**`ctxdeq_valid=`.** The `live=` count is derived from the Output Endpoint Context's TR Dequeue
Pointer, and GUARD-STATE established that this field is only architecturally defined while the
endpoint is **not Running** (xHCI 1.2 §4.8.3); on Intel Panther Point it is otherwise frozen at its
birth value. So the **pre** scan is advisory — under a Running endpoint it reads a stale field and
will under-count — and the **post** scan, taken after the endpoints are Stopped, is authoritative.
This is why the asserted counter is `undrained=` (post) and not `abandoned_*` (pre): the assertion is
placed on the reading that means what it says. `ctxdeq_valid=` states which kind each line is rather
than leaving a reader to know it.

**`undrained=` is fix 1's own regression witness.** It counts pipes that still held valid-cycle TRBs
after the cleanup, or whose resync failed. **It must read 0 on every boot.** A non-zero reading is
the primary hole reopened, and it says so without anyone having to reconstruct it from a corrupted
filesystem afterwards. Slots with no reachable ring (device gone, or surrendered — where
`bot_transfer` refuses every later transfer at its first line) are skipped explicitly and *not*
counted, so the number stays an assertion rather than drifting into noise.

**`csw_bytes` resolves a specific open question.** The 2026-07-29 capture recorded one tag of
`0xACABAAA9` with nothing to read it against, and the two candidate explanations — a **torn read** of
a partially DMA-written CSW versus an **overlay** of another payload onto the CSW buffer — are
distinguished by the bytes *around* the tag, which were never printed. A valid `USBS` signature with
a wrong tag is a stale-but-well-formed CSW (a phase slip); high entropy across all 13 is an overlay;
expected bytes mixed with zeros is a torn read. The boot totals now give it a denominator, which is
the difference between "this happened once" and a rate.

### 15.6 Blast radius

**The healthy path is byte-identical**, and each claim is checkable:

* Fix 1 adds nothing to any success path — the wrapper inspects the result and calls the cleanup only
  on `Err`.
* Fix 2 reorders three `bot_ring_guard` calls that, since GUARD-STATE, return `Ok` immediately for a
  Running endpoint. Nothing is pushed and no doorbell rings between them, so the reordering is not
  observable; only the `RingFull` refusal moves earlier. The `push` result changes are inert (`push`
  always returns `Ok` today).
* Fix 3 adds arithmetic on a value the event already carried. On a transfer where everything moved,
  `moved == data_len` and no branch is taken. The gates below ran 2002 data stages with
  `short_in=0 short_out=0` and no `residue_disagree`.
* Fix 4 only ever *narrows* what may claim a stage; it cannot cause a claim that did not happen
  before. The stall-delivery property is preserved because a STALL carries its TRB pointer.
* Fix 5 removes dead code with no callers.
* Fix 6 is reachable only from escalation rung (a), after a failed Configure Endpoint — unreachable
  on a healthy device.

Nothing here weakens a protection. No page permission, checksum, SMEP/NXE/WXN or validation gate is
touched; fixes 2, 3 and 4 each *add* a check, and fix 3 converts a previously accepted condition into
a rejected one.

### 15.7 Gates

* `./arroyo check` — green, **both** arches.
* `./arroyo test 90` — **31 PASS / 0 FAIL** (baseline maintained).
* `./arroyo test-fat part 90` — **41 PASS / 0 FAIL + 1 LIVE**, 4005 pump waits, `timeouts=0`.
* `UNAOS_BOTFAULT=1 ./arroyo test 90` — **31 PASS / 0 FAIL**. The injected CSW-stage failure is
  rescued by the existing first rung (`recoveries=1 retry_ok=1`), and the new witnesses read
  `abandoned_in=0 abandoned_out=0 undrained=0` afterwards — the rings are clean after recovery.
* Every new counter reads **0** on all three boots.

**What QEMU cannot prove — stated plainly, as §14.6 does.** The chokepoint's cleanup fires only when
`bot_transfer` *returns* an error, and QEMU never produces one: the single injected fault is always
rescued by the first rung, so the transaction returns `Ok`. `bot_clean_rings`, `bot_strand_witness`
and `TransferRing::strand_scan` are therefore **not executed** in emulation — `strings` proves the
text is in the artifact, and that is all emulation establishes. Their correctness rests on inspection
(no new `unwrap`, every `Option` handled, the scan bounded by the ring length) and on the metal boot
that will first exercise them. The `undrained=0` readings above are real but weak: they are taken on
boots where the cleanup path never ran.

Equally: fix 3's OUT-short branch, fix 4's `ev_unaddressed` fallback, fix 6's repoint, and every
`ring refuse` remain unreached in emulation, for the same reason §14.6 gives — QEMU's `usb-storage`
does not stall, short-change or wedge.

### 15.8 What metal must verify

1. **`undrained=0` on every boot**, including boots that see real transport faults. This is the
   arc's central assertion.
2. **A `:: BOT: strand ::` pair at every error exit**, with `when=post` showing `live=0` on both
   pipes. ~~A `when=pre` line with `live>0` and `ctxdeq_valid=yes` is the first direct observation of
   the stranded TRB this arc is about.~~

   **ONSET correction (2026-07-30) — the `when=pre` half of this item cannot fire from the current
   call site, so its `gap=0 live=0` readings are not findings.** On both tip boots the `pre` scan
   runs *after* `bot_recover`'s own `resync stage=set-deq` has already repointed the controller onto
   our enqueue (boot F: 3752/3754 → 3758/3759; boot G: 4192/4194 → 4198/4199). `gap=0 live=0` is
   therefore true **by construction** — the observation the item asks for is destroyed by the
   cleanup that precedes it. This is the **sixth** entry in the instrument-lie ledger
   ([§16.9](#169-the-instrument-lie-ledger)). **Fixed by M1a (landed, gated, awaiting metal):** the
   pre-scan now runs inside `resync_bulk_ep`, between each pipe's stop and its own `set-deq`, which
   is the only window where `ctxdeq` is defined *and* still on the strand — so **this item can now
   fire**. `undrained=` (post) is unaffected: it is taken after the endpoints are
   Stopped, it is the arc's asserted counter, and its `0` readings on boots F and G stand.
3. **No further CBW-shaped corruption** across a sustained write workload on the affected reader.
4. **`csw_bytes` on the next tag mismatch** — torn read or overlay, finally decidable.
5. **`short_out=`** should be 0; any non-zero is a phase fault caught before it reached the medium,
   and each one is a transaction that would previously have been reported to FAT as success.
6. **`ev_late=` / `ev_unaddressed=`** — the first real measurement of event aliasing on this
   hardware.
7. **Rung (a)'s `cfgep_cc=19` followed by `stage=repoint`** — whether the repoint lands, which
   determines if §14.8's second open item is now explained.

   **ONSET answer (2026-07-30): NO — the repoint does not land, and at its current call site it
   cannot.** Set TR Dequeue Pointer is legal only against a Stopped or Error endpoint (xHCI 1.2
   §4.6.10), and at that moment both bulk endpoints are **Disabled**: boot F lines 3804/3805 print
   `ok=no cc=19 why=cc-error … epstate=0` on the IN and the OUT pipe, and the retry that follows
   fails with `completion code 12` = **Endpoint Not Enabled** (line 3810). Fix 6 is a no-op wherever
   the endpoints have been disabled, which is every time this rung runs. Why they are disabled — and
   whose fault that is — is [§16.7](#167-cfgep_cc19-closed--a-sequencing-defect-of-ours).

   **Consequence (M1a, landed).** The answer stands for the code as it was, and it is what retired
   the rung: the Reset Device + Configure Endpoint step is gone, replaced by a ring rebase, and fix
   6's repoint now sits in the only state from which it is reachable. Whether it *lands* is a metal
   question again — the item is re-armed, not closed.
   [§16.12](#1612-m1a--what-the-code-lane-landed-and-the-correction-it-forces-on-167).

### 15.9 Still open

§14.8's list, updated:

1. **The genuine wedge now has its mechanism.** The deterministic first-timeout, the storm boots and
   the medium corruption are one story, closed by §15.1–15.2 and joined at `dCBWTag = 28`. What
   remains open from that item is narrower and should not be absorbed into the fix: the **FIRST
   completion loss** — the event that goes missing to *start* a storm — is still unexplained. This
   arc stops a lost completion from corrupting the medium; it does not explain why one is lost.
   ~~`foreign=0` across the silence remains the part most wanting an explanation.~~

   **ONSET update (2026-07-30).** The first-completion loss is **still open** — but the `foreign=0`
   clause is **retired**, not answered: the field is structurally incapable of reading anything else
   on this platform (§14.3's `foreign=F` retraction). What the GR8 capture *did* add is a shape: 3 of
   3 genuine onsets are byte-identically an OUT data stage of 512 B at ring index 0 on a wrap, and
   two of them are the same failure at the same stage index and the same LBA on different builds.
   [§16.8](#168-still-open-the-first-completion-loss-now-has-a-shape-and-a-lead).
2. ~~**`cfgep_cc=19`** — fix 6 makes the failure *safe* but does not explain *why* Configure Endpoint
   is illegal at that moment. `stage=repoint`'s own `cc` on the next metal capture is the next rung
   of that question. Still open.~~

   **CLOSED (2026-07-30, ONSET) — as a sequencing defect of ours.** Reset Device **succeeds**
   (`resetdev_cc=1 resetdev_why=ok`) and leaves the Slot in **Default** (xHCI 1.2 §4.6.11), disabling
   every endpoint but the Default Control Endpoint — which the driver's own before/after field
   records on the same line as `epin=1->0 epout=1->0`. Configure Endpoint is legal only against an
   Addressed or Configured Slot, so Context State Error is **architecturally required** there, not
   merely "expected" as §14.1 words it. ~~Our ladder omits **Address Device** between the two
   rungs.~~ All seven `cc=19` instances in the capture fit, across both builds (lines 2022, 2380,
   2630, 2835, 3237, 3806, 4321), each preceded by `resetdev_cc=1` and an `epX=…->0` transition. The
   endpoints were not "already gone" — **we disable them.**

   **Corrected (2026-07-30, M1a): the struck sentence named the wrong remedy.** There is no lawful
   re-address without a port reset — Address Device with BSR=1 leaves the Slot in Default, and with
   BSR=0 it must `SET_ADDRESS` to 0, which this device will not answer — so §14.1's argument holds
   and inserting Address Device would only have added a second command that must fail. **Rung (a) has
   been retired** in the code lane and replaced with a ring rebase built from commands that are legal
   where the endpoints actually are.
   [§16.7](#167-cfgep_cc19-closed--a-sequencing-defect-of-ours) has the diagnosis;
   [§16.12](#1612-m1a--what-the-code-lane-landed-and-the-correction-it-forces-on-167) has the verdict
   and what landed.
3. **~9 ms mean per 512-byte transfer** — ~~unchanged from §14.8, still deferred, still should not
   be.~~ **Restated (2026-07-30, ONSET):** the figure was a one-outlier artefact and the SMI-shaped
   reading has no evidence behind it; the honest number is ~0.94–1.9 ms per awaited stage, which is
   one to two ticks of our own 1 kHz polled pump. See the corrected §14.8 item 2 and
   [§16.6](#166-the-pace-anomaly-is-our-own-1-khz-polled-pump). Still a throughput ceiling; no longer
   a mystery, and no longer a reason to hunt a host stall.
4. **The aarch64 `piusb36_read10_two_trb` twin** carries the same short-transfer hole fix 3 closes in
   `bot_transfer_once`, and is out of this arc's lane. It is flagged for the Pi seat's lift.

---

## 16. ONSET — the GR8 cold read: what the capture establishes, and what it falsifies (2026-07-30)

A read-only pass over the GR8 metal capture, `~/unaos-bench/capture/rmbp-gr8/ttyUSB1.log` (4347
lines), read against the driver source at `0825ed08`. Nothing in this section is itself a fix. The
code lane's response — **M1a**, which landed one of the implied fixes, **refuted another**, and
turned two more into default-off metal experiments — is recorded in
[§16.12](#1612-m1a--what-the-code-lane-landed-and-the-correction-it-forces-on-167), and the
subsections it corrects say so where they stand.

Every claim below is tagged with how it was obtained, because §14.5's lesson was that the difference
matters more than the conclusion:

* **Observed** — a line in the capture.
* **Derived** — arithmetic on observed values.
* **Inferred from source** — read in the code at `0825ed08`, not witnessed on a wire.
* **Hypothesis** — with its falsifier named.

### 16.0 Provenance — read this before quoting any number from this capture

**Observed. The file is not one boot. It is seven.** `xHCI: Controller Started!` appears at lines
257, 1246, 1751, 2251, 3068, 3540, 4052 — call them **A–G**. (A's start is missing; line 1 of the
file is a truncated mid-line, so the watcher attached during A.) **Two driver builds are present**,
and the log says which:

| build marker | boots |
|---|---|
| `result=SUMMARY` **without** `tag_mismatch=/undrained=/ev_late=` | A, B, C, D, E |
| `result=SUMMARY` **with** those fields; `:: BOT: strand ::`, `:: BOT: clean ::`, `:: BOT: deqprobe ::`, the `(stale: EP running)` tag on `timeout pipes` | **F, G only** |

Those extra fields are GUARD-STATE + BOT-PHASE, i.e. tip `0825ed08`. **Only boots F and G carry the
GUARD-STATE/BOT-PHASE witnesses; A–E are older builds.** Any census that attributes this file's
14 `TIMEOUT-PIPES`, 9 `ring refuse`, 7 `cfgep_cc=19` or 7 `SURRENDER` lines to "the s55 boot" is
mixing pre- and post-fix evidence. Per-boot attribution is used throughout below.

**Inferred from source, confirmed by arithmetic: `n=` in the pump lines counts awaited STAGES, not
transactions.** `BOT_PUMP_COUNT` is incremented once per completed pump wait (`mod.rs:7037`, from
`note_bot_pump`) and `BOT_STAGE_GEN` once per `run_bot_stage` (`mod.rs:6887`); `bot_transfer_once`
calls `run_bot_stage` **twice** per transaction — the data stage (`mod.rs:5380`) and the CSW stage
(`mod.rs:5490`). The CBW is pushed but never separately awaited (§16.3). Boot F's first timeout reads
`n=218 gen=219` with `single=99 multi=10`; 99 + 10 = **109 data stages**, and 109 × 2 = **218**.

**Consequence, and it propagates to every rate quoted from a pump line: boot F's "~219 clean
transactions" is ~109 transactions / 218 awaited stages.** Halve any transaction count derived from
`n=`, and read `mean=` as per-stage, not per-transaction.

> **SUPERSEDED FOR POST-2026-07-30 CAPTURES (BOT-CBW).** The two paragraphs above are correct for
> **this file's boots A–G and every capture predating BOT-CBW**, and they stay as the reading key
> for them. They are wrong for anything newer. Since
> [§17](#17-bot-cbw--the-straddle-convicted-and-the-cbw-becomes-a-stage-2026-07-30),
> `bot_transfer_once` calls `run_bot_stage` **three** times on a data-carrying transaction — CBW,
> data, CSW — so the divisor is **3, not 2** (a `data_len == 0` transaction still awaits two). The
> rule generalises to: **divide `n=` by the stage count of the build that produced the log, and date
> the build from the KNOBS line** — `cbw=always-awaited` means three, `botcbwioc=off-cbw-unawaited`
> means two (§17.7). `mean=` remains per-stage in every build. The `single=`/`multi=` cross-check
> this section used (99 + 10 = 109 data stages, × 2 = 218) still works, with ×3 in place of ×2.

**Line-number citations into `mod.rs`/`ring.rs` are as of `0825ed08`** and have already drifted:
M1a landed against the same files after this read (§16.12). Cite them as of that commit, not as of
tip.

### 16.1 The platform grading is on the record — and this key's calibration line was backwards

**Observed.** Two `deqprobe` lines, both metal, both the same verdict, on two different slots and two
different rings:

```
3710: :: BOT: deqprobe slot=2 dci=5 enq=6 running_epstate=1 running_ctxdeq=0x2020bd41
      -> stopped_epstate=3 stopped_ctxdeq=0x2020bda1 stop_ok=yes stop_cc=1 stop_why=ok
      restore_ok=yes epstate_after=3 verdict=ctxdeq-stale-under-running ::
4144: :: BOT: deqprobe slot=1 dci=5 enq=6 running_epstate=1 running_ctxdeq=0x2020b181
      -> stopped_epstate=3 stopped_ctxdeq=0x2020b1e1 stop_ok=yes stop_cc=1 stop_why=ok
      restore_ok=yes epstate_after=3 verdict=ctxdeq-stale-under-running ::
```

**§14.7 item 7 is confirmed on metal, twice, independently.** The Output Endpoint Context TR Dequeue
Pointer read under a Running endpoint on this silicon is the **birth value**, not a position.

**Derived, and load-bearing for §16.4.** Boot G's `running_ctxdeq=0x2020b181` against
`stopped_ctxdeq=0x2020b1e1` with `enq=6` pins slot 1's bulk-IN ring base at **`0x2020b180`**
(`0x1E0 − 0x180 = 0xE0` = index 6 = `enq`). So `0x2020b181` is literally *ring base | DCS=1* — the
value Configure Endpoint wrote at birth.

**The one correction: `epstate_after`.** §14.3's calibration line gave QEMU's `epstate_after=1` as
the expected reading. Metal reads **3**, both times. Reading the source, the probe rings the doorbell
on an **empty** ring and samples `epstate` immediately; real silicon does not leave Stopped until
there is a TRB to fetch, while QEMU transitions eagerly. **Metal is right; `epstate_after=1` is the
QEMU artefact.** The endpoint is not left parked — 218 and 94 further awaited stages retired after
the respective probes. §14.3 carries the correction inline.

### 16.2 `foreign=` is a dead instrument on this platform

**Observed.** All 14 `TIMEOUT-PIPES` lines in the file read `foreign=0`, across both builds.

**Inferred from source.** `pump_until_bot_done` is a synchronous spin (`mod.rs:6947–6970`). While it
spins, nothing submits an FTDI transfer; no FTDI TRB is outstanding; no FTDI Transfer Event can
arrive. `foreign` is pinned at 0 **by construction** — on a healthy boot exactly as on a wedged one.
The capture arriving over that same FTDI proves only that the driver drained its serial queue
*before* and *after* the wait, not during it.

**Observed, and it answers the question `foreign` was built to answer — positively.** During the
6-second silence the driver issues Stop Endpoint and Set TR Dequeue Pointer and gets Command
Completion Events back (boot F, lines 3751–3754: `resync stage=stop-ep … cc=1`, `resync
stage=set-deq … cc=1`, twice). A `cc=1` on a command means the command ring executed, the xHC posted
an event, the interrupter delivered it and `drain_event_ring_once` consumed it. **The event ring,
interrupter and event delivery are demonstrably alive throughout the silence** — which refutes the
"global interrupter wedge" reading `foreign=0` seemed to license, 15 lines after the line that
seemed to license it.

**Also observed, and it is witness 3 doing its job:** `IMAN=0x3 USBSTS=0x18` at the timeout, and
`IMAN=0x3 USBSTS=0x18` on the healthy `result=OK` baseline of the same boot (line 3709). Identical.
Those registers discriminate nothing here, and the only reason anyone can say so is that §14.3's
healthy baseline was recorded alongside.

§14.3's `foreign=F` bullet carries the retraction and names `evts=` as the replacement the code lane
should carry. §14.7 item 4 and §15.9 item 1's closing sentence are retired.

### 16.3 The "by construction" premise in §14.4 item 1 is false in the source

**Inferred from source, and it is our own code, not the device's behaviour.** §14.4 item 1 argued
that the lap guard cannot affect a healthy device because "each BOT stage is awaited to completion
before the next is queued, so at most **one** TRB is outstanding". `bot_transfer_once` does not do
that:

* the CBW is pushed at `mod.rs:5317` with `control: 1 << 10` — **Normal type, no IOC, no ISP**, so it
  posts **no completion at all**;
* the data TRB is pushed at `mod.rs:5345` with **no pump between the two pushes**;
* for an OUT data stage `data_dci == out_dci`, so the doorbell at `mod.rs:5377` is a **single**
  doorbell covering **both** TDs (the second doorbell at `mod.rs:5378` is conditional and does not
  fire).

So **at least two TRBs are outstanding on the OUT ring at every write**, and the driver holds **no
witness that the CBW was ever consumed**. Only the data and CSW stages are awaited, which is also why
`n=` counts two per transaction (§16.0).

This matters beyond the arithmetic: **it is the same premise the already-retracted §14.5 rested on.**
The retraction there was written as a story about one stale field; the premise underneath it was
never true either, and had never been established anywhere in the code or the doc. §14.4 carries the
correction inline. The 14-TRB refusal threshold is not endangered by two outstanding TRBs — what is
retired is the argument, not the number.

**M1a's response is an experiment, not a fix.** `UNAOS_BOTCBWIOC` (default-off) gives the CBW TRB IOC
and awaits it as its own stage — making the code do what §14.4 item 1 already claimed it did — at a
cost of one extra ~1 ms tick per transaction (§16.6). Its value is evidentiary: with it on, "the CBW
was consumed" stops being an assumption, which is what would let §16.8's rank-1 hypothesis be
narrowed to the data TD alone.

**SUPERSEDED (2026-07-30, BOT-CBW).** The experiment ran, on metal, one variable, and came back
one-sided. It is now the **unconditional** behaviour and the knob is deleted — see
[§17](#17-bot-cbw--the-straddle-convicted-and-the-cbw-becomes-a-stage-2026-07-30). This paragraph is
kept as written because the sequence matters: the defect was found by auditing our own claim against
our own source, and only then tested.

### 16.4 `ring refuse` — the assertion holds, the sub-claim is unfalsifiable

**Observed.** All 9 `ring refuse` lines (1852, 1882, 2026, 2343, 2582, 2786, 2839, 3188, 3241) are in
**pre-GUARD-STATE** boots C, D and E. **Boots F and G — the tip build — carry zero**, across 218 + 94
awaited stages. §14.7 item 8's primary assertion is what the fix actually claims, and the capture
supports it.

**Observed + derived, and it is as clean a confirmation of the GUARD-STATE diagnosis as this evidence
class allows.** Three of the nine (1852, 2343, 3188) read `slot=1 dci=5 enq=14 ctxdeq=0x2020b181
dcs=1`. From §16.1 that value is slot 1's bulk-IN ring **base | DCS=1** — the exact value boot G's
`deqprobe` labels `running_ctxdeq` and calls a birth value. `would_lap` therefore computed
`used = (14 + 16 − 0) % 16 = 14`, `14 + 2 ≥ 16` → refuse: **from a frozen birth value, against a
device that was working.** The pre-fix bug and the post-fix instrument that names it are in the same
file, on the same silicon. (The other six split into two shapes — `enq=14` against dequeue index 0,
and `enq=0` against dequeue index 1 — our enqueue a full lap ahead of a pointer that never moved.
Same mechanism.)

**The sub-claim cannot be checked on this or any capture.** §14.7 item 8 also asserted that any
`ring refuse` "now carries `epstate` 2, 3 or 4 by construction, so it is a real finding rather than
an artefact". The format string (`mod.rs:5014–5018`) emits `slot dci dir enq cycle ntrb ctxdeq dcs`
and **no `epstate`**. The construction argument is sound *in source* (`bot_ring_guard` matches
`ep_state_of` against `2 | 3 | 4` and returns `Ok` for everything else **before** reading `ctxdeq`,
`mod.rs:5000–5004`) — but a claim whose witness is absent from the log is an **inference, not a
finding**. No `ring refuse` in this file carries `epstate=1` or `0`, because none carries `epstate`
at all: the live defect is *not observed* and *not excluded*.

### 16.5 `strand when=pre` cannot observe what it was built to observe

**Observed.** In both tip boots the `when=pre` scan runs **after** `bot_recover`'s own
`resync stage=set-deq` has already repointed the controller onto our enqueue:

| boot | resync `set-deq` | `strand when=pre` |
|---|---|---|
| F | 3752, 3754 | 3758, 3759 |
| G | 4192, 4194 | 4198, 4199 |

So `gap=0 live=0` on those lines is true **by construction**, not a finding: the cleanup that the
scan was meant to precede has already run. §15.8 item 2's `when=pre` half can never fire from this
call site, and the readings it produced must not be cited as evidence that the rings were clean at
the error exit.

**Fixed in the code lane by M1a (landed, `./arroyo check` green both arches, awaiting metal).** The
pre-scan moved out of `bot_clean_rings` and into `resync_bulk_ep`, between each pipe's stop and its
own `set-deq` — the only window where `ctxdeq` is both architecturally defined (endpoint Stopped) and
still parked on the strand. **§15.8 item 2 can now fire**; whether it does is a metal question, not a
gate question ([§16.12](#1612-m1a--what-the-code-lane-landed-and-the-correction-it-forces-on-167)).

**`undrained=` (post) is unaffected.** It is taken after both endpoints are Stopped, where the field
it derives from is architecturally defined (§15.5), it is the arc's asserted counter, and its `0`
readings on boots F and G stand.

### 16.6 The pace anomaly is our own 1 kHz polled pump

**Derived, calibration first.** `hw_wait_budget() = tsc_hz × HW_WAIT_SECONDS` with
`HW_WAIT_SECONDS = 2` (`arch/x86_64/mod.rs:102, 115–118`). The capture's observed `hw_wait_budget` is
**5 387 700 130** cycles (boot F, line 3742), so **TSC = 2.6939 GHz** and **1 ms = 2 693 850 cycles**.
The BOT budget is 3× that = 16.16 × 10⁹ cycles ≈ 6 s, which matches every `pump budget=` line. The
APIC heartbeat is **1000 Hz** — one tick per millisecond (`arch/x86_64/apic.rs:158–162`,
`TICK_HZ = 1000`).

**Derived, per awaited stage, with and without the single bring-up outlier:**

| boot | build | n | peak (ms) | raw mean (ms) | mean ex-peak (ms) |
|---|---|---|---|---|---|
| A | pre | 148 | 622.9 | 5.32 | **1.12** |
| B | pre | 94 | 626.9 | 7.88 | **1.23** |
| C | pre | 28 | 624.9 | 23.5 | **1.24** |
| D | pre | 18 | 0.99 | **0.94** | **0.93** |
| E | pre | 19 | 623.9 | 34.6 | **1.88** |
| F | tip | 218 | 475.9 | 3.87 | **1.69** |
| G | tip | 94 | 626.9 | 7.89 | **1.24** |

Two readings follow, and together they retire the SMI-shaped account in §14.8 item 2.

1. **The "~9 ms mean per 512-byte transfer" was a one-outlier artefact.** Five of the seven boots
   carry a single ~625 ms wait — the **first** BOT stage after `SET_CONFIGURATION` (printed at `n=1`,
   e.g. line 3709) — and with `n` between 18 and 218 that one sample owns the mean. Its cycle counts
   are 1 678 052 412 / 1 688 825 376 / 1 683 430 866 / 1 680 738 993 / 1 688 819 643 — a spread under
   **1%** across five independent boots, which is the signature of a **device-side firmware timer**
   (the SD reader's media-init latency on the first TUR / READ CAPACITY), not an opaque host stall.
   Boot F's peak is a different constant (475.9 ms); boot D has no such outlier at all and reports a
   **raw** mean of **0.94 ms**, with nothing removed. That is the honest number.
2. **The residual 0.94–1.9 ms per awaited stage is one to two 1 kHz ticks, and it is our code.**
   `pump_until_bot_done` drains the event ring and then calls `crate::hlt()` (`mod.rs:6961–6968`),
   which sleeps until an interrupt. **`IRQ_COUNT=0` on every boot** — the xHCI interrupt is never
   taken — so the only thing that can wake the pump is the 1 ms periodic APIC tick, and per-stage
   latency is quantised to it. Corroborating: `IMAN=0x3` reads identically on the healthy and the
   timeout lines, i.e. IP is set and never RW1C'd, so an MSI cannot re-arm — consistent with
   `IRQ_COUNT=0` and with a purely polled driver.

**What the capture cannot say.** It carries only `sum`, `peak` and `n`, so it cannot produce a
distribution and cannot answer whether the waits are uniform, bimodal or bursty. That needs the
histogram in [§16.10](#1610-what-the-next-metal-boot-must-be-able-to-print) item 3.

**Nothing here supports the SMI-shaped reading.** Per transaction the cost is ~1.9–3.4 ms (two awaited
stages), roughly 3–8× a fully serialised BOT transaction's floor — not the two-to-three orders of
magnitude §14.8 item 2 claimed. It remains a throughput ceiling worth removing; it is no longer a
reason to hunt a host stall.

> **SUPERSEDED (2026-08-03, BOT-CBW + BOOTPACE M3).** The parenthetical "(two awaited stages)"
> is the no-IOC premise, and it holds only for the pre-BOT-CBW boots this section reads. Under
> [§17](#17-bot-cbw--the-straddle-convicted-and-the-cbw-becomes-a-stage-2026-07-30) a data-carrying
> transaction awaits **three** stages. The per-stage measurement above (0.94–1.9 ms, one to two
> 1 kHz ticks) is what this capture supports and is unchanged; only the multiplier that turns it
> into a per-transaction figure moves. §17.4's original per-transaction projection was then itself
> revised by BOOTPACE M3's spin-then-halt pumps — see §17.4's addendum and §17.8 item 4. Do not
> quote ~1.9–3.4 ms as a current per-transaction cost on either count.

### 16.7 `cfgep_cc=19` closed — a sequencing defect of ours

**Observed** (boot F, line 3806, and the same shape on all seven instances):

```
:: BOT: rescue stage=reset-device slot=2 ok=no resetdev_cc=1 resetdev_why=ok
        cfgep_cc=19 cfgep_why=cc-error indci=5 outdci=2 inmps=512 outmps=512
        epin=1->0 epout=1->0 n=1 ::
```

Reset Device **succeeds**. xHCI 1.2 §4.6.11: it transitions the Slot to **Default** and disables every
endpoint except the Default Control Endpoint — and the driver records that transition itself, on its
own line, as `epin=1->0 epout=1->0`. Configure Endpoint is legal only against an **Addressed** or
**Configured** Slot, so against Default a Context State Error is **architecturally required**, not
merely "expected" as §14.1 words it. That is a driver gap, in our lane, and it accounts for **every**
`cc=19` in the file — all seven, across both builds (lines 2022, 2380, 2630, 2835, 3237, 3806, 4321),
each preceded by `resetdev_cc=1` and an `epX=…->0` transition.

> **Correction (2026-07-30, M1a) — the remedy this section first inferred was wrong.** The sentence
> that stood here read *"Our escalation ladder omits **Address Device** between the two rungs."* It
> is **struck**: Address Device with BSR=1 leaves the Slot in Default and does not satisfy §4.6.6
> either, and with BSR=0 it must `SET_ADDRESS` to address 0, which only a just-port-reset device
> answers. There is **no lawful re-address without a port reset**, so §14.1's argument is correct and
> the insertion would have added a second command that must fail in front of one that already did.
> **The diagnosis above is unchanged; only the inferred fix was wrong.** What landed instead — the
> retirement of the rung — is [§16.12](#1612-m1a--what-the-code-lane-landed-and-the-correction-it-forces-on-167).

**The `epstate=0` discriminator is answered.** The endpoints were **not** already gone when the rung
ran — **we disable them**, at a precisely located step, and the driver's own before/after field says
so.

**Consequence 1 — §15.8 item 7 is answered NO.** Fix 6's `stage=repoint` cannot land from the call
site it had. Set TR Dequeue Pointer requires Stopped or Error (xHCI 1.2 §4.6.10) and the endpoint is
**Disabled**: lines 3804 and 3805 print `ok=no cc=19 why=cc-error … epstate=0` on both pipes, and the
retry that follows fails with `completion code 12` = **Endpoint Not Enabled** (line 3810). This
verdict stands as the correct reading of the code as it was, and it is what motivated M1a's
retirement of the rung ([§16.12](#1612-m1a--what-the-code-lane-landed-and-the-correction-it-forces-on-167)).

**Consequence 2 — the design question is SETTLED, against the remedy this section first inferred.**
§14.1 argued that re-addressing without a port reset would be the *unsound* move, because the device
still holds its USB address and would not answer `SET_ADDRESS` at 0. **§14.1 is right.** Neither BSR
setting of Address Device reaches Addressed here, so the rung had no lawful continuation at all — the
fix was to **retire** it, not to complete it. Full verdict and what landed in
[§16.12](#1612-m1a--what-the-code-lane-landed-and-the-correction-it-forces-on-167).

**One thing the capture does not say.** Whether a device-side condition made Reset Device necessary
in a way that also explains the onset. Reset Device ran ~12 s *after* the onset, on the third
consecutive failure. It is a consequence, and it cannot be the cause.

### 16.8 Still open: the first-completion loss now has a shape and a lead

The loss of the **first** completion — the event that starts a storm — is **still unexplained**. What
the GR8 capture adds is a shape, a reproduction, and one ranked hypothesis.

**Observed — the onset shape, 3 for 3.** Boots C, D and E do not belong to this question: their first
BOT failure is a `ring refuse` (1852, 2343, 3188), i.e. the pre-GUARD-STATE false `RingFull`, since
fixed. The three genuine onsets are:

```
1367 (B): stage=data dir=out len=512 trb_idx=0 wrapped=true  single=44 multi=3  wrapped_tx=2
3735 (F): stage=data dir=out len=512 trb_idx=0 wrapped=true  single=99 multi=10 wrapped_tx=7
4175 (G): stage=data dir=out len=512 trb_idx=0 wrapped=true  single=44 multi=3  wrapped_tx=2
```

**Byte-identical: an OUT data stage, 512 bytes, landing at ring index 0 — i.e. on a wrap.** From
source, `BOT_LAST_WRAP` is set exactly when the data push returned index 0 (`mod.rs:5352`), and
`TransferRing::push` writes the Link TRB **lazily**, at push time, only once
`enqueue_index == num_trbs − 1` (`ring.rs:106–124`) — so for the whole of each lap index 15 holds a
**stale-cycle** TRB, and the data TD at index 0 sits immediately behind a Link TRB written moments
earlier. A data stage lands at index 0 roughly one time in 8–16, so 3/3 is not a coincidence anyone
should spend long defending.

**Observed — and the victim is named:**

```
1386 (B): :: BLK: io-cause op=write lba=17303 bot_err=Timeout (first, once) ::
3767 (F): :: BLK: io-cause op=write lba=2080  bot_err=Timeout (first, once) ::
4207 (G): :: BLK: io-cause op=write lba=17303 bot_err=Timeout (first, once) ::
```

All three are the flight recorder's write path (each followed by `:: FR: UNAOS.LOG reservation failed
(Io) ::`); boot F's LBA 2080 is the FAT itself.

**Observed — boots B and G are the same failure twice.** Same `n=94`, same
`single=44 multi=3 wrapped_tx=2`, same `trb_idx=0 wrapped=true`, same `lba=17303`, `peak` agreeing to
**6 ppm** (1 688 825 376 vs 1 688 819 643) and `sum` to 0.13% — on **two different driver builds**
(B is pre-GUARD-STATE, G is tip). **This is §14.8 item 1's deterministic-recurrence handle,
reproduced**, and it survives the GUARD-STATE and BOT-PHASE fixes exactly as §15.9 item 1 predicted.
A deterministic, index-locked failure is a function of ring position and workload — it is neither a
stochastic device fault nor an SMI.

**Observed — the authoritative read at the onset, and the caveat §14.3's key omits.** Boot F, in
order:

```
3736: timeout pipes … out_epstate=1 out_ctxdeq=0x2020be40 (stale: EP running) out_enq=1 … foreign=0
3737: timeout trb wait=0x2020be40 pipe=out dw0=0x21e60000 dw1=0x0 dw2=0x00000200 dw3=0x00000425
                  trb_cycle=1 ring_cycle=1 trb_type=1
3738: timeout csw sig=0x0 … valid=no
3753: resync stage=stop-ep dci=2 dir=out ok=yes cc=1 epstate=1->3
3754: resync stage=set-deq dci=2 dir=out ok=yes cc=1 want=0x2020be51 ctxdeq=0x2020be41->0x2020be51
```

Line 3736's `ctxdeq` is tagged stale and supports nothing — correct behaviour by the driver, correct
reading by us. Line 3737 is authoritative: `dw3=0x425` decodes as TRB Type 1 (Normal), C=1, IOC
(bit 5), ISP (bit 2), and `dw2=0x200` = 512 bytes. **The TRB is well-formed and its cycle matches the
ring's; nothing is wrong with what we wrote.** Line 3754 is the authoritative *dequeue* read, because
3753 forced the Running→Stopped transition with `cc=1`: the pre-write value `0x2020be41` is address
`0x2020be40` with DCS=1 — **the awaited TRB itself**, at ring index 0. The identical structure holds
in boots B and G (G: `wait=0x2020b280` at 4177 against `ctxdeq=0x2020b281->0x2020b291` at 4194).

Read against §14.3's key with a Stopped read in hand, `ctxdeq` *at* the awaited TRB rather than past
it is **"the controller never fetched the work"**, not "the device is silent", and `TIMEOUT-CSW
valid=no` is consistent. **The honest caveat, stated because the key does not state it:** a TD the
xHC *has* fetched and is retrying (device NAKing) leaves the dequeue in the same place. The
architectural discriminator is a Stop Endpoint Transfer Event with completion code **26 (Stopped)**
or **27 (Stopped — Length Invalid)**, which the driver never prints. Boot G's SUMMARY reads
`ev_late=0 ev_unaddressed=0`, which leans the same way. **Suggestive, not decisive** — see
[§16.10](#1610-what-the-next-metal-boot-must-be-able-to-print) item 4.

**Hypothesis (rank 1) — the doorbell that must restart the controller across a Link TRB does not
take.** At a wrap, `bot_transfer_once` leaves the CBW at index 14 (cycle C), the Link at index 15
(cycle C, TC=1) and the data TD at index 0 (cycle !C), under **one** doorbell (§16.3) that arrives
*after* the Link was written into a slot the controller may already have inspected and rejected as
stale. That fits the onset shape (index 0, on a wrap), the index-locked determinism, the dequeue
parked at index 0 with DCS toggled, and `valid=no`. **Falsifier:** any onset with `wrapped=false` on
the tip build, or the deterministic failure surviving unchanged when the wrap position moves.
**Neutral discriminating experiments, one variable each:** change only the transfer-ring length
16 → 64 and see whether the failure moves or disappears; ring the doorbell a second time after a
wrapped OUT push (redundant doorbells are architecturally legal and weaken nothing); and, if either
implicates the Link, pre-place the Link TRB at ring construction so index 15 is never a stale data
slot. **The first of these is now armed as a default-off knob, `UNAOS_BOTRING64`** — an experiment
awaiting a boot, not a fix
([§16.12](#1612-m1a--what-the-code-lane-landed-and-the-correction-it-forces-on-167)).

**Lower-ranked, kept only with their falsifiers named.** USB 2.0 link power management — the failing
device is on a **USB 2.0** root port (`port 1 [usb2] … sp=3(HS)`), so the LPM at issue is **L1 /
PORTPMSC**, not USB 3 U1/U2, and we program no LPM state at all; falsified by PORTSC PLS reading U0
continuously across the silence. PCH port-mux residue — we write XUSB2PR and USB3_PSSEN ourselves
every boot (`:: PORTSW-1: … routed 0x0->0xf …`) and have never tested that `0xf` is right for this
board; the controlled comparison is a boot that leaves the mux at the firmware default. **Device or
card fault is not admissible** and is recorded only to say why: UEFI reads this card through this
reader on this port flawlessly every boot, nothing in this capture is a controlled experiment against
the device, and until the hypotheses above are excluded *and* a firmware-baseline experiment
reproduces the failure, it is not a hypothesis but a place to stop looking.

### 16.9 The instrument-lie ledger

The class, stated generally: **a field read at a moment the hardware or the runtime defines it as
meaningless is not weak evidence — it is an instrument that cannot falsify anything, and it will be
believed because it printed.** The sibling rule is the instrument-baseline law (a counter read
against its own pre-run total is not a rate). This read found the fifth and sixth entries.

| # | instrument | why it could not mean what it printed | recorded in |
|---|---|---|---|
| 1 | `[cursor12]` rollup (compositor, cross-seat) | printed cumulative totals covering passes that ran before the sprite existed — the healthy-but-idle reading was indistinguishable from total failure | compositor docs; instrument-baseline law |
| 2 | `recover evidence … pipe=none` | `run_bot_stage` took the pending record and dropped it before propagating the error, so recovery's read was always `None` — a structural lie about the driver's own state, on **every capture ever taken** | §14.3 |
| 3 | `ctxdeq` under a Running endpoint | architecturally undefined while Running; frozen at the birth value on Panther Point, refreshed live by QEMU — read as a position, it built the lap guard wrong | §14.3, §14.4, §14.5 |
| 4 | wc-d live reference (compositor, cross-seat) | same class, other subsystem | compositor docs |
| **5** | **`foreign=`** | the pump is a synchronous spin, so no foreign transfer is ever outstanding while it waits; **0 by construction**, healthy or wedged | §14.3, [§16.2](#162-foreign-is-a-dead-instrument-on-this-platform) |
| **6** | **`strand when=pre`** | runs *after* `bot_recover`'s `set-deq` has already repointed the controller, so `gap=0 live=0` is true by construction and the observation it was built for is destroyed before it is taken | §15.8 item 2, [§16.5](#165-strand-whenpre-cannot-observe-what-it-was-built-to-observe) |

Entries 2, 3, 5 and 6 are all in this driver, and all four printed green readings that were quoted as
evidence. The standing consequence: **a new witness is not done until someone has stated what it
reads in the healthy case and in the "mechanism has not run yet" case, and shown those differ.**

### 16.10 What the next metal boot must be able to print

Each item names the decision it unblocks. These are asks on the code lane, not changes made here.

1. **`epstate=` and `ctxdeq_valid=` on the `ring refuse` line** (`mod.rs:5014`). Unblocks §14.7 item
   8's sub-claim, which is otherwise unfalsifiable on every capture ever taken (§16.4). Two fields;
   both values are already in hand at that point in the function.
2. **Port state at the timeout: PORTSC with PLS decoded, plus PORTPMSC and PORTLI**, for the failing
   port and every other connected port, on or immediately after `TIMEOUT-PIPES`, and once at
   bring-up as the baseline. Unblocks the LPM and port-mux hypotheses entirely, for the cost of a few
   volatile reads. Note the current state precisely: a periodic topology summary *does* print PORTSC
   with PLS decoded (e.g. lines 885–892), but **never on a timeout line**, and **PORTPMSC and PORTLI
   are never printed at all**. One summary happens to land inside boot G's storm before any recovery
   touched the port (line 4271: `port 1 … CCS=1 PED=1 PLS=0(U0) sp=3(HS)`), which leans against a
   link-down reading — but it is not a read at the timeout and is not part of the witness set.
3. **A log₂ bucket histogram of per-stage wait cycles on the SUMMARY line** (8–10 counters). Unblocks
   the uniform/bimodal/bursty question and any test of §16.6's tick-quantisation reading. If the
   buckets pile at 1 and 2 ticks, the pace question closes as a polled-driver artefact.
4. **Transfer Events counted by completion code during recovery, naming 26 and 27 explicitly.**
   Unblocks the last ambiguity in §16.8: "the controller never fetched the TD" versus "it fetched it
   and the device is NAKing".
5. **A per-endpoint doorbell counter on `TIMEOUT-PIPES` (`db_in=`, `db_out=`), with the ring index at
   the time of the last one.** Unblocks the rank-1 hypothesis directly. There is no line anywhere in
   the capture saying a doorbell was written, so a doorbell that was written and did not take, and a
   doorbell that was never written, are currently indistinguishable.
6. **Link TRB forensics: when `wrapped=true`, dump the TRBs at index `ntrb−1` and `ntrb−2` as raw
   dwords** alongside the awaited TRB. Puts the Link's cycle bit, TC bit and target address on the
   record instead of being reasoned about from source.
7. ~~**Move the `strand when=pre` scan above `bot_recover`**~~ — **DONE (M1a).** The scan now runs
   inside `resync_bulk_ep`, between each pipe's stop and its own `set-deq`. Unblocks §15.8 item 2
   (§16.5, [§16.12](#1612-m1a--what-the-code-lane-landed-and-the-correction-it-forces-on-167)).
   Landed and gated; the reading itself is still owed by metal.
8. **xHCI USBLEGSUP/USBLEGCTLSTS pre/post, in the EHCI grammar.** Unblocks the standing claim that
   the xHCI SMI enable bits are clear, which currently has **no witness in any capture** (§16.11).
9. **`evts=` — all event-ring TRBs consumed during this wait, of any type** — replacing or augmenting
   `foreign=`. `drain_event_ring_once` already returns per-event. Unlike `foreign`, it can be
   non-zero, which is what would make a zero reading mean something (§16.2).
10. **`tag=`, `cdb0=` and `lba=` on the `TIMEOUT-SHAPE` line.** The timing-out transaction is
    currently identified only by `csw_bytes` on a CSW rejection and by `BLK: io-cause` after the
    fact; §15.2's code→capture→medium join had to be reconstructed from a wrecked filesystem and
    should be readable from the log directly.
11. **`t_ms=` on the pump lines.** No BOT line carries a wall clock, which blocks correlating the
    onset with the 1 kHz tick, with any periodic external event, and with the concurrent port-2
    enumeration that was mid-`reset-settle` at both the B and G onsets (lines 1362–1364, 4170–4172).
    Boot F's onset had no port activity, which is what keeps port-reset concurrency out of §16.8's
    ranked list — but the coincidence deserves a timestamp before it is dismissed.

### 16.11 Checklist items this capture closes — and one it does not

**Closed.** Two entries on the real-silicon-gap checklist print on **every** boot in this file and
need no further investigation:

* **Scratchpad buffers** — `xHCI: scratchpad: 16 buffer(s) x 4096 bytes; DCBAA[0]=…` (lines 254,
  1243, 1748, 2248, 3065, 3537, 4049).
* **BIOS→OS handoff** — `xHCI: BIOS->OS handoff complete.` (lines 241, 1230, 1735, 2235, 3052, 3524,
  4036).

**Not closed — and the claim that said otherwise has no witness.** The standing statement that "the
xHCI SMI enable bits are confirmed clear" (§14.8 item 2) is supported by **nothing in any capture**.
xHCI prints only the one handoff line above and **never** prints USBLEGSUP or USBLEGCTLSTS. EHCI
prints both, for both controllers, every boot — e.g. `:: EHCI-CONFIG: [0] USBLEGCTLSTS@0x6c:
pre=0xc00c0000 post-own=0xe00c0000 cleared->0x000c0000 …` and `:: EHCI-CONFIG: [0] USBLEGSUP@0x68:
OS-own set, BIOS-owned cleared …`. xHCI should print them in the same grammar
([§16.10](#1610-what-the-next-metal-boot-must-be-able-to-print) item 8). Until it does, the SMI half
of the handoff is an **assumption**, not a finding.

### 16.12 M1a — what the code lane landed, and the correction it forces on §16.7

The code lane's M1a arc answered §16.7's open design question, and **it refuted the fix this section
originally implied**. Recorded here in full, because the wrong fix was the more obvious one and would
otherwise stay plausible.

> **RETRACTION (2026-07-30, M1a).** §16.7 and §15.9 item 2 originally named the defect as *"our
> escalation ladder omits **Address Device** between Reset Device and Configure Endpoint"*, and left
> §14.1's competing soundness argument standing as an open question. **That proposed insertion was
> wrong, and §14.1's argument is correct: there is no lawful re-address without a port reset.**
>
> * **Address Device with BSR=1** (xHCI 1.2 §4.6.5) issues no `SET_ADDRESS`, but leaves the Slot in
>   **Default** with address 0. It does **not** reach Addressed, so §4.6.6's precondition is still
>   unmet and Configure Endpoint still answers `cc=19`. BSR=1 exists for the enumeration sequence,
>   every step of which presumes a just-port-reset device.
> * **Address Device with BSR=0** must send `SET_ADDRESS` to address **0** (USB 2.0 §9.1.1, §9.4.6),
>   which only a device in Default state — entered on a port reset and nowhere else — answers. The
>   wedged reader still holds its assigned address.
>
> So inserting Address Device between the rungs would have added a second command that must fail, in
> front of one that already did. The failure mode this section diagnosed is real and its explanation
> (§16.7's first two paragraphs) is unchanged: Reset Device succeeds, leaves the Slot in Default,
> disables the endpoints, and Configure Endpoint is then architecturally required to return `cc=19`.
> What was wrong was the inferred remedy. **This is the difference between a correct diagnosis and a
> correct fix, and the record should show that we got one and not the other.**

**What landed instead** (code lane; gated `./arroyo check` green on **both** arches; **awaiting
metal**):

1. **Rung (a)'s Reset Device + Configure Endpoint is RETIRED.** The rung could never succeed — Reset
   Device succeeds, but the Configure Endpoint behind it is architecturally required to fail, and no
   lawful step closes that gap (the retraction above) — and failing was not free: it turned Running
   endpoints into **Disabled** ones (`epin=1->0 epout=1->0`, observed on all seven instances) with no
   path back except the port cycle the rung existed to avoid. A rung whose success is architecturally
   impossible and whose failure costs the slot is worse than no rung.
2. **It is replaced by a ring rebase**, built only from commands legal where the endpoints actually
   are: **Stop Endpoint / Reset Endpoint** (xHCI 1.2 §4.6.9 / §4.6.8) → `TransferRing::reset` →
   **Set TR Dequeue Pointer** at each ring base with DCS=1 (§4.6.10). `rebuild_bulk_input_ctx` and
   its Configure Endpoint are deleted.
3. **A real sub-finding, recorded because it was found on the way.** The retired rung called
   `TransferRing::reset` **before** the endpoints were stopped, violating that function's own stated
   safety precondition — *"the caller must have stopped/reset the endpoint first, so the controller
   is not concurrently fetching from this ring"* (`ring.rs`). The driver was zeroing a ring the
   controller could still be walking. The rebase honours it: stop first, then reset, then repoint.
   This is independent of the `cc=19` story and would have been worth fixing on its own.
4. **Fix 6's `stage=repoint` (§15.8 item 7) keeps its intent** and now sits in the only state where
   it is reachable. This is a consequence of the retirement, not a change to fix 6. **§16.7's
   "answered NO" verdict stands as the correct reading of the code as it was**, and is what motivated
   the retirement.
5. **The `strand when=pre` scan (§15.8 item 2, §16.5) moved** out of `bot_clean_rings` and into
   `resync_bulk_ep`, between each pipe's stop and its own `set-deq` — the only window where `ctxdeq`
   is both architecturally defined (endpoint Stopped) and still parked on the strand. **It can now
   fire.** `undrained=` is untouched.

**Two metal knobs, default-off — experiments awaiting a boot, not fixes.** Neither is a claimed
correction and neither should be quoted as one:

* **`UNAOS_BOTRING64`** — transfer ring 16 → 64 TRBs. This is [§16.8](#168-still-open-the-first-completion-loss-now-has-a-shape-and-a-lead)'s
  rank-1 discriminator with one variable changed: wrap frequency and wrap positions both move,
  nothing else does. **If the deterministic failure at LBA 17303 moves or vanishes, the Link/wrap
  mechanism is causal; if it stays put, the rank-1 hypothesis dies.**
* **`UNAOS_BOTCBWIOC`** — the CBW TRB is given IOC and awaited as its own stage, i.e. the code is
  made to do what §14.4 item 1 already claimed it did (§16.3). Cost is one extra ~1 ms tick per
  transaction (§16.6). If the onset survives with the CBW *confirmed consumed*, the two-TDs-one-
  doorbell reading is out and the rank-1 hypothesis narrows to the data TD alone.
  **RESOLVED (2026-07-30) — and the falsifier fired the other way.** The onset did **not** survive:
  with the CBW confirmed consumed the failure vanished (`n=1108 timeouts=0` against
  `n=737 timeouts=3`, one variable). The two-TDs-one-doorbell reading is **in**, not out, and this
  knob is now unconditional behaviour with the feature deleted from the build —
  [§17](#17-bot-cbw--the-straddle-convicted-and-the-cbw-becomes-a-stage-2026-07-30). `UNAOS_BOTRING64`
  above remains a default-off diagnostic; ring length was never convicted.

### 16.13 The standing frame

Every conviction in this section is against our own driver, our own boot chain, or a gap in our own
instrumentation: the guard that read a meaningless field, the rung that could only ever fail and took
the endpoints down with it, the pump that never awaits its own CBW, the scan that runs after the
cleanup it was meant to precede, the mean that was one sample — and, in §16.12, our own first
proposed remedy. Hardware blame remains inadmissible on this evidence — UEFI reads this card, through
this reader, on this port, flawlessly, every boot, and no controlled experiment against the device
has been run. The null hypothesis is our code until something falsifiable says otherwise.

---

## 17. BOT-CBW — the straddle convicted, and the CBW becomes a stage (2026-07-30)

The onset that §16.8 could only rank has an **experiment against it, and the experiment came back
one-sided.** `UNAOS_BOTCBWIOC` stops being a knob in this arc: the CBW carries IOC and is awaited as
its own stage in every build, and the feature is deleted from `Cargo.toml`, `arroyo` and
`builder/src/main.rs` so that no build can turn it off. This section records what convicted it, what
it cost, and which of this document's own earlier claims it retires.

### 17.1 The A/B, and why it is a clean one

**Observed.** Two metal boots on the **same tree** (`46f8f37e`), **one variable** — whether the CBW
is awaited. Both forced the flight recorder's **RESERVATION** path (if `UNAOS.LOG` already exists,
later boots take write-in-place and never reach the failing code, so this had to be forced or the
comparison would have been between a run of the mechanism and a run of nothing). Both at ring
length **16**. Both carried the ONSET-3 ring hardening.

| | CBW awaited | CBW unawaited |
|---|---|---|
| awaited stages (`n=`) | **1108** | 737 |
| `timeouts=` | **0** | **3** |
| Link crossings (`wrap_push=`) | 81 | 83 |
| `:: BLK: io-cause …` | none | `op=write lba=33742` |

**Derived, and it is the reason the numbers are worth quoting.** `wrap_push=` is within 2.5% across
the pair, so the failing boot was not simply the one that crossed more Links; and the passing boot
moved **50% more** awaited stages while taking **zero** timeouts. A hazard that fires three times in
737 stages and zero times in 1108 is not a difference in exposure. The denominator problem that made
every earlier wrap experiment unreadable (§16.8, and the `wrapped_tx=` correction below) is closed
here by `wrap_push=` being on both lines.

### 17.2 `stopev_res=512` — the device took zero bytes

**Observed.** The onset witness on the failing boot:

```
resync stopev dci=2 dir=out ev_stopped=1 stopev_n=1 stopev_fresh=yes
              stopev_dci=2 stopev_trb=0x20212ec0 stopev_res=512
```

**Derived, from xHCI 1.2 §4.6.9 and §6.4.2.1.** Completion code 26 (Stopped) is posted only for a TD
the controller had **fetched and was executing**, and the Transfer Event's TRB Transfer Length field
is the **residue** — the bytes of that TD that did *not* move. A residue of **512 on a 512-byte OUT
data stage** is the whole TD: the controller had the work, presented it, and the **device accepted
zero bytes and never entered the data phase.** That is not a lost doorbell, not a lost completion and
not a bad TRB; it is the **CBW→DATA handoff**.

**Do not conflate this with the CSW-shaped stops.** The same failing boot also carries
`stopev_res=13` on several **IN**-pipe events. The CSW is 13 bytes, so those are status-stage stops —
a different phase, and not the onset. **Only the `dci=2 dir=out res=512` line is the onset witness.**

### 17.3 The mechanism

**Inferred from source, and now supported by the A/B.** Before this arc `bot_transfer_once` pushed
the CBW with `control: 1 << 10` — Normal type, **no IOC, no ISP**, so it posted no completion at all
— and then pushed the data TRB with **no pump between the two pushes** (§16.3). For an OUT data
stage `data_dci == out_dci`, so a **single** doorbell covered **both** TDs. At a wrap the two sit on
opposite sides of a **Link traversal**: CBW near the end of one lap, data TD at index 0 of the next.

So the host had **two TDs outstanding on one endpoint under one doorbell, across a Link**, and held
**no witness that the CBW was ever consumed**. The device's BOT state machine and ours were free to
disagree about which phase was current, and §17.2 is what that disagreement looks like from the host
side: the controller executing a data TD at a device that is still waiting for a command.

**The fix is to remove the straddle, not to reason about it.** The CBW now carries IOC, gets its own
doorbell, and is pumped to completion before the data stage is *built*. At most one TD is outstanding
on any endpoint at any time — which is what [§14.4](#144-ring-hygiene-m2) item 1 has claimed since it
was written and what §16.3 showed was false in the source. `stage=cbw` on a `TIMEOUT-SHAPE` line is
now a reachable reading, and it means "the device never even took the command".

### 17.4 The cost, stated because it is now permanent

**Derived, from §16.6's calibration.** The pump is polled: `pump_until_bot_done` drains the event
ring and calls `hlt()`, `IRQ_COUNT=0` on every boot, and the only thing that wakes it is the **1 kHz**
APIC tick. Every awaited stage therefore costs **at least one ~1 ms tick**. Adding the CBW takes a
transaction from `T + D` to `2T + D` — one extra tick each, on **every BOT transaction, forever**.

This is not written off as noise and it is not hidden in a knob. It is a real throughput ceiling and
it is the price of a transport that stays phase-synchronised with a real device; §17.1's right-hand
column is what the alternative costs. The ceiling itself has a known remedy that is **not** this arc's
— take the xHCI interrupt instead of polling (§16.6's `IRQ_COUNT=0` and the never-RW1C'd `IMAN=0x3`),
which would return the cost to interrupt latency for **all three** stages at once.

> **ADDENDUM (2026-07-30, BOOTPACE M3). The cost was not permanent — it was the cost of sleeping
> FIRST.** The paragraphs above are correct about the mechanism and wrong about its inevitability.
> `hlt()` sleeps until an interrupt; with xHCI interrupts not enabled the only wake is the 1 kHz APIC
> tick; therefore *a stage that begins by calling `hlt()`* costs a tick no matter how fast the
> controller answered. But nothing required the pump to begin there. All three synchronous pumps —
> `pump_until_bot_done`, `pump_until_ep0_done`, `pump_until_cmd_done` — now busy-poll
> `drain_event_ring_once()` under a bounded `now_cycles` sub-deadline of **~200 µs**
> (`Xhci::spin_window`, `cycles_per_ms()/5` — a spec-scale constant chosen against controller
> latency, deliberately **not** derived from `hw_wait_budget()`) before falling into the sleep. A
> healthy controller posts a completion in single-digit to low-tens of microseconds, so nearly every
> awaited stage now returns from inside the spin window and the tick quantisation is gone.
>
> **The hlt fallback is kept, and that is not an oversight.** A pure spin never exits under QEMU TCG
> — the note at the top of `pump_until_bot_done` says so, and it is still true. Past the window the
> path is byte-identical to what it was: one `hlt()` per pass, the same wall-clock deadline
> arithmetic, the same timeout lines. Under TCG the spin merely wastes ≤200 µs per pass.
>
> **What did NOT change.** No budget, no settle, no timeout, no protocol timing, and — explicitly —
> no interrupt state: no IMAN write, no MSI change. Taking the interrupt remains the future remedy,
> now for the residue rather than for the whole tick. The KNOBS line carries `pump=spin+hlt` so a
> capture can be dated; artifact proof is
> `strings unaos/target/x86_64_esp/kernel.elf | grep -c 'pump=spin+hlt'` ≥ 1.
>
> **No new counter was added for it**, deliberately: the existing instruments already carry the
> verdict. See §17.8 item 4.

### 17.5 Corrections this arc makes to its own earlier claims

Recorded in full rather than quietly dropped: three readings that were quoted as evidence in this
document did not survive the boots that convicted the straddle.

> **RETRACTION (2026-07-30, BOT-CBW). §16.8's rank-1 hypothesis — "the doorbell that must restart
> the controller across a Link TRB does not take" — is RETIRED.** ONSET-3's recovery posts
> `ev_stopped=1` (cc=26), which xHCI 1.2 §4.6.9 defines as posted only for a TD **in progress**. The
> controller had crossed the Link, had **fetched** the data TD, and was owed no further doorbell. A
> missed or non-taking doorbell is not the mechanism. The onset *shape* §16.8 recorded (OUT, 512 B,
> `trb_idx=0`, `wrapped=true`, 3 for 3) was real and pointed at the right place; the mechanism
> inferred from it was wrong. **The Link crossing is where the straddle becomes lethal, not what
> fails.**

> **CORRECTION (2026-07-30, BOT-CBW). `wrapped_tx=0` never meant "this boot had no Link crossings".**
> It counts **data-stage pushes landing at index 0** — nothing else. A boot can cross the Link many
> times and report `wrapped_tx=0` simply because no *data* push happened to be the crossing one. The
> 64-TRB boot that was read as "the wrap is not causal" had crossings throughout, and moved **more**
> I/O than the boot that wedged. Link crossings were never eliminated in **any** boot. This is why
> `wrap_push=` exists and why §17.1 quotes it on both sides: without a count of the crossings
> themselves the experiment has no denominator and can conclude nothing.

> **CORRECTION (2026-07-30, BOT-CBW). `cc=27` alone proves nothing.** Stopped — Length Invalid was
> read as a signal in its own right; on the IN pipe it arrived with `gap=0 live=0` and `deq == enq`,
> i.e. **no TD at all**. The completion code has to be read together with the pipe, the residue and
> the ring state, which is exactly what `stopev_res=` was added to make possible. `cc=26` with a
> **full-length residue on a pipe that had a TD** is a finding; a bare code is not.

The standing rule these three share is [§16.9](#169-the-instrument-lie-ledger)'s: a witness is not
done until someone has stated what it reads in the healthy case, what it reads when the mechanism has
not run, and shown those differ.

### 17.6 The ring hardening is NOT claimed to explain any capture

**Stated as a boundary on the evidence.** The same ONSET-3 commit shipped two ring changes: the
**Link TRB is pre-placed at ring construction** (so the last slot is never a stale data slot holding a
wrong-cycle TRB for most of a lap), and the **payload is written at index 0 before the Link is
armed** (so the consumer can never be handed a live Link pointing at a slot whose contents are not
yet there). Both are correctness hardening, both are right on their own terms, and **neither is
offered as the cure.**

The reason is in §17.1: **the failing boot had them.** The knob-off run carried the pre-placed Link
and the payload-before-arm ordering and still took three timeouts and still produced
`op=write lba=33742`. That is what exonerates the ring work as the explanation — and it is the same
observation that convicts the straddle, because with the ring hardening held constant across the
pair, the CBW await is the only thing left that changed.

### 17.7 What is a knob, and what is not

* **`UNAOS_BOTRING64` — still a knob, still default-off, unchanged.** The metal evidence never
  convicted ring length. It grows the storage slot's two bulk rings 16 → 64 TRBs, changing wrap
  frequency and every wrap position and nothing else. It is a **diagnostic**, and it stays one; the
  64-TRB boot moved more I/O than the boot that wedged, which is a data point and not a fix.
* **`UNAOS_BOTCBWIOC` — deleted.** Not defaulted-on: **deleted**, from `Cargo.toml`, `arroyo` and
  `builder/src/main.rs`. Setting the variable does nothing. A fix that a build flag can switch off is
  a fix that will eventually ship switched off, and this project has twice shipped media with a knob
  wired into `arroyo` but not into `builder/` — green everywhere, disabled on the metal, invisible
  until the boot came back identical.
* **The KNOBS line keeps a tag for it, because captures are compared across boots.** The third field
  of `:: BOT: knobs … result=KNOBS ::` now reads **`cbw=always-awaited`** in every build. A log
  carrying `botcbwioc=off-cbw-unawaited` or `botcbwioc=ON-cbw-awaited` is from **before** this arc,
  and that is how any future reader dates a capture against this section.

### 17.8 What metal must verify next

1. **The A/B holds at ring 64.** Same forced-reservation workload, CBW awaited, `UNAOS_BOTRING64=1`.
   Expected: `timeouts=0` with `wrap_push=` roughly a quarter of the ring-16 figure. A timeout here
   would say the straddle was not the whole mechanism.
2. **A long run.** §17.1's passing boot is `n=1108`. The failure was ~1 in 250 stages on the failing
   side, so a clean `n=1108` is roughly a 4× margin — good, not conclusive. A multi-thousand-stage
   run with `timeouts=0` is what closes [§14.8](#148-still-open--each-owed-its-own-arc) item 1's
   deterministic-recurrence handle.
3. **`stage=cbw` has never been printed.** If it ever appears on a `TIMEOUT-SHAPE` line, the device
   is refusing the command itself and this section's mechanism does not cover it.
4. **The measured cost — REVISED by BOOTPACE M3 (see §17.4's addendum).** The original expectation
   was `mean` near **three** ticks per transaction rather than two, on the reasoning that each of
   the three awaited stages costs one 1 kHz tick. With the spin-then-halt pumps that reasoning no
   longer applies to a healthy controller, and the expectation inverts:

   - `mean` on `:: BOT: … result=SUMMARY ::` should fall to **well under one tick** per
     transaction — the three stages now cost controller latency (microseconds), not three ticks.
   - `BOT_WAIT_BUCKETS` bucket 0 ("under 1 ms") should hold **nearly all** stages. Before this
     change it held almost none.
   - The KNOBS line must read `pump=spin+hlt`. If it does not, the build predates the change and
     the two readings above do not apply to it.

   A `mean` still sitting near three ticks **with** `pump=spin+hlt` present is the falsifying
   result: it would say completions are genuinely arriving later than 200 µs, i.e. the tick was
   never the dominant cost and something else is. That is a finding, not a failure — but it must be
   reported rather than explained away.

### 17.9 Provenance of the no-IOC position, and what is still owed

Added 2026-08-03, after an archive sweep run from the pi4 seat answered three standing questions
about where the fire-and-forget premise came from. It is recorded here because the premise outlived
its own evidence by a wide margin, and because §12.1 derived a headline number from it for a week
after §17 convicted it.

**The position was inherited, and it was never measured.** *(read-from-git)* The no-IOC CBW push
(`Trb { parameter: cbw_phys, status: 31, control: 1 << 10 }`) traces back through
`git log -S'control: 1 << 10 }'` to `8c25f448` ("USB stack work") and `fa82b728` ("Phase 1: Local
APIC + xHCI MSI-X"). Neither commit carries a measurement. The justification the source carried was
the bare assertion the current comment quotes back at itself — "the CBW is fire-and-forget (the
device consumes it before it can respond)".

**It was then re-asserted twice, on spec reasoning alone.** *(read-from-git)* `52382e22` (CBW-FAULT,
2026-07-30) and its re-derivation `c3947e22` (BOT-PHASE, 2026-08-01, which reached trunk) both argue
the missing IOC is deliberate, from xHCI 1.2 §4.10.2 (an error terminates a TD and posts a Transfer
Event irrespective of IOC) plus a cost argument (one wasted event and MSI per transaction). Both
arguments are a priori. `52382e22`'s gate is QEMU only, with `kernel8-test` — the pi target —
blocked on the host; `c3947e22`'s gate is `./arroyo check` green on both arches and nothing else. So
the position reached the shared trunk on a type-check, carrying zero runtime evidence.

**The only A/B in the project's history is §17.1's, and it convicted the position.** *(read-from-git)*
`efa52ebe` / `dfa570f0` are the sole controlled comparison anywhere in the log: same tree
(`46f8f37e`), one variable, both boots forced onto the flight recorder's RESERVATION path, both at
ring 16, both carrying the ONSET-3 hardening — `n=1108 timeouts=0` awaited against `n=737
timeouts=3` unawaited, with `wrap_push=` within 2.5% across the pair so the failing boot was not the
more exposed one.

**The two seats re-derived the question independently and blind.** *(read-from-git)*
`git merge-base --is-ancestor efa52ebe 52382e22` reports not-an-ancestor; the merge base is
`0825ed08` (2026-07-29), and `git branch --contains` places the two commits on two different
worktree branches. They are ~8 hours apart on the same day. The pi seat re-asserted no-IOC while the
disproving A/B already existed on a branch it could not see. This is a provenance fact about the
argument, not a criticism of either seat: it is why the premise survived a re-derivation that looked
like independent confirmation and was not one.

**The narrow spec claim was never refuted, and is still true.** *(read-from-git)* An error does post
a Transfer Event irrespective of IOC. What the a-priori argument missed is the *straddle* (§17.3) —
two TDs outstanding on one endpoint under one doorbell, across a Link — which is about phase
synchronisation, not about error visibility. `mod.rs`'s comment at the CBW-FAULT claim arm records
this reconciliation, and the counter's own doc-comment states that `cbw_fault=0` does not mean no
CBW failed.

**x86 metal evidence for the awaited architecture.** *(booted-capture)* Beyond §17.1's A/B, the
trunk-unification boot `rmbp-s66-cand444` ran the awaited-CBW path clean on the 2012 rMBP:
`n=100 timeouts=0 undrained=0 cbw_fault=0` with `storage_slot=1` — a real denominator, i.e. BOT
transactions actually ran.

**What is still owed: the same reading on Pi hardware.** *(booted-capture)* The archive sweep found
`cbw_fault=` on a real SUMMARY line in exactly three capture files; the only pi one
(`capture/pi4-r23s1x/ttyACM0.log`, two occurrences) reads
`n=0 storage_slot=0 db_in=0 db_out=0 cbw_fault=0`. **Those readings are vacuous, not passing** — no
BOT transaction ran on that boot, so the zero is the absence of a measurement, not a clean one. No
pi capture in the archive carries a BOT pump SUMMARY with `n>0`, and the firing line
`:: BOT: cbw fault … ::` has never appeared on any rig. The awaited-CBW architecture is therefore
compiled and QEMU-green on aarch64 but **unverified on Pi silicon**.

> **Discriminator, queued for the pi seat.** A Pi 4 metal boot with USB storage enumerated
> (`storage_slot != 0`) driving BOT traffic to `n > 0` — the flight-recorder reservation path or FAT
> traffic on the `Usb` source — with the `:: BOT: pump … result=SUMMARY ::` line captured. Until
> such a boot exists, "the awaited CBW is correct on pi" and "CBW-FAULT is clean on pi" are both
> unsupported in either direction. A second, independent gap: `cbw_fault` has no non-zero reading on
> any rig, so the safety net itself is untested code — that one needs a deliberate CBW-error
> injection or a STALLing device, on either rig.

---

## 11. USBFALL — fail-closed backend substitution + an honest FAT lock span

USB-WRITE made `BlockSource::Usb` writable. Two things had quietly gone stale behind that change; USBFALL
fixes both without touching the xHCI driver or the BOT deadline.

**F1 — the `Default` write no longer falls through to the stick.** On `aarch64 + baremetal` the canonical
block backend is the microSD: `emmc2::probe()` runs on the BSP synchronously, before any FAT-writing
consumer, and `register_sd` flips `BACKEND` to `BACKEND_SD`. If the card fails identify, `BACKEND` stays
`BACKEND_XHCI`, a later-enumerated USB stick populates the global `BLOCK_DEVICE` through
`publish_usb_geometry`, and from then on every `BlockSource::Default` **write** silently lands on the
stick — one physical device substituted for another, with no error anywhere. `write_block` now calls
`guard_default_write_backend` first: with no SD registered it returns `BlockError::NotReady` and prints
`:: USBFALL: no SD backend registered — refusing Default WRITE … ::` once. A failed identify produces an
honest no-writable-volume boot instead of misdirected writes. `FatBackend::read_only()` reflects the same
condition (`drivers::block::default_writable()`), so a `Default` mount on a no-SD Pi answers "read-only
volume" (`VfsError::Unsupported`) **up front** rather than looking writable and failing late with an opaque
`Io` on every write — that late-failure asymmetry was the honesty gap in the first cut of F1. Reads
deliberately still fall through (a read cannot corrupt the wrong device, and the USB mount has its own
`read_block_usb` handle) — the residual is ledgered in [`SECURITY.md`](../../SECURITY.md): a no-SD Pi can
still *read* a `Default` volume off a substituted stick. The guard is
`baremetal`-gated on purpose: on QEMU-virt aarch64 (`test-arm`) the SD backend is never compiled,
xHCI **is** the legitimate sole backend, and the function does not exist — those builds are byte-identical
to pre-USBFALL. The rule is about substitution on a platform that has a canonical backend, not about xHCI.

> **The x86 half of that sentence expired (FRGUARD, GR21, 2026-08-07).** SDHC-4b gave x86 a canonical
> backend that is NOT the `Default` target — the internal SD registers as handle `Sdhc` and leaves the
> global slot empty — and Boot AI-2 caught the flight recorder writing `/UNAOS.LOG` onto a card the
> operator had hot-plugged to read. `x86_64 + sdhcblk` now has its own arm of `default_writable`, keyed
> on the boot volume's `BS_VolID` as the UEFI loader reported it. See the FRGUARD row in
> [`SECURITY.md`](../../SECURITY.md).
>
> **⚠ Pi lane — F1 has a hole on aarch64 that FRGUARD found and did not close.**
> `guard_default_write_backend` is called from `write_block` **only**. Since MULTIBLK, every
> whole-sector run goes through `fs/fat.rs::write_sectors` → `block::write_blocks`, which carries **no
> guard on `aarch64 + baremetal`**. So a `Default` multi-sector FAT write on a no-SD Pi still lands on
> the substituted stick — precisely the failure F1 exists to prevent, on the path that carries almost
> all of the bytes. The x86 arm guards both entry points; the aarch64 arm was left byte-identical
> because that was the GR21 arc's contract. **The fix is one `#[cfg]`'d call at the head of
> `write_blocks`.** Ledgered as an open item in [`SECURITY.md`](../../SECURITY.md).

**F2 — the lock span, stated per source.** `fat.rs`'s `with_fat_lock` justified holding `FAT_MUTATION`
across block I/O with "the aarch64 I/O is polled, so the span is a couple of bounded polled sector
transfers". That premise is source-blind. On `BlockSource::Usb` the same RMW runs
`write_block_usb → storage_write10 → scsi_write10 → bot_transfer → pump_until_bot_done`, whose deadline is
`hw_wait_budget() * 3` = 450 M CNTVCT ticks (~8 s at 54 MHz) and whose pump body executes `wfi` under the
`PSTATE.I` this very hold masked. It is bounded and it is not a deadlock — WFI wakes on a pending physical
interrupt with I set — but a failing transfer means a multi-second non-preemptible hold (×`num_fats`) with
the scheduler stopped. The LOCK SPAN paragraph now states `Default` and `Usb` separately, and every `FatFs`
call site takes the lock through `with_fat_lock_src`/`with_dir_lock_src`, which carry the `BlockSource`
explicitly so a newly added source must answer the paragraph rather than inherit the polled premise. The
span itself is unchanged (narrowing it for one source would fork the RMW's atomicity argument; shortening
the deadline belongs to the xHCI layer). What is added is evidence: `note_masked_usb_hold` witnesses the
first masked-IRQ hold taken on a `Usb` volume, behind `UNAOS_WITNESS` — compiled out of a default-quiet
build. **This witness is METAL-ONLY, not merely default-quiet.** No QEMU gate can fire it: `kernel8-test`
mounts only `Default` (SD) volumes and the raspi4b model enumerates no storage stick, and `test-arm` never
compiles the `Usb`-vs-`Default` distinction the same way. Its absence from the gate logs proves nothing
about the path — it proves only non-regression. **Still owed from metal:** the line itself, and the actual
stall on a *failing* BOT write under a held FAT lock; QEMU's BOT does not fail the way a real stick does.

**F3 — the stale read-only assertions retired.** `fat.rs`'s `BlockSource` doc, both `piusb27_*` mount
comments, `fs/vfs.rs`'s `FatBackend` doc and `genet.rs`'s `/fs/usb` route comment each still claimed the
PIUSB-27 invariant that `write_sector` refuses any non-`Default` source and no write can reach the stick.
USB-WRITE removed it; `FatBackend::read_only` already returns `false` for every source on aarch64. Each now
says what is true, and distinguishes "this route only reads" from "this source cannot be written".

**Gates (USBFALL).** `./arroyo check` green both arches; `./arroyo kernel8` clean; `./arroyo kernel8-test 90`
MBENCH PASS 80/80 required, 0 forbidden; `./arroyo test-arm` MISSION SUCCESS. No USBFALL line appears in
either log: the F1 refusal is byte-inert on the healthy-SD boot path (which is the point), and the F2
witness is metal-only (see above — its absence is non-regression, not evidence).

---

## 12. BOT-PHASE — the phase-desync holes, closed on the VL805 path (lift 0825ed08, 2026-07-29)

### 12.0 The finding, and why it is lifted

On the gemini (x86, Panther Point) track, a read-only audit of a corrupted USB stick recovered a
FAT directory entry whose bytes were **not a directory entry**: they were a **Command Block
Wrapper** — the driver's own 31-byte BOT command header, written into a sector that belonged to the
filesystem, with a `dCBWTag` matching the first-failing transaction of a captured storm boot
(code → capture → medium, joined by one transaction number). The full forensics live in that
tree's usb_xhci.md §15 (commits `ae052e95`/`0470f498`, branch tip `0825ed08`).

Our own independent audit of this tree's aarch64/VL805 BOT path found **the same hole family**, at
these (pre-lift, `cb837f69`) sites:

* error exits with no ring resync — `bot_transfer`'s data-stage `TransferError` return, the
  status-stage `Err` propagation, the stall return, and both CSW-validation rejections;
* no ring capacity check at all (`TransferRing::push` tracked no consumer position);
* discarded push results (`.ok()` on the CBW push, `.unwrap_or(0)` on the data and CSW pushes —
  a failed push would have waited on `ring_base + 0`, a real, recurring TRB address);
* `cc=13 SHORT PACKET` accepted as blanket success on both the data and status stages;
* `BotPending` carrying no residual (the Transfer Event residue was discarded);
* event matching that claimed **any** error completion on either bulk DCI, unqualified by
  generation or position.

Two independent audits, two controllers (Intel Panther Point, Broadcom VL805), one mechanism. The
mechanism is in the shared BOT state machine, not in a vendor quirk — which is the argument for
lifting the fix set at this layer rather than re-deriving it behind a platform flag. This tree is
the VL805 confirmation of the lift source's §15.3.

### 12.1 The mechanism: a dirty ring is a phase slip

A BOT transaction is three serialized phases on two bulk rings. The device runs a phase machine of
its own, and the two machines stay in step **only** because each side retires one phase before
starting the next. Every error exit from `bot_transfer` used to return with whatever it had already
pushed **still on the rings**, and the controller's TR Dequeue Pointer parked on those TRBs. The
*next* transaction's doorbell then did not start a new transaction — it **resumed the abandoned
one**, replaying a stale CBW (and on the write path a stale payload) into a device whose phase
machine had moved on. From there the two machines run one phase apart: the host's data is read as a
command, its command consumed as data and written to the medium.

Aggravators (all apply here as on x86): the CBW and an OUT data stage share the bulk-OUT ring, so
an abandoned WRITE strands a command wrapper AND payload together; TRB addresses recur every ~5
transactions on a 16-TRB ring, so address-only matching aliases; and the condition is
self-perpetuating once entered.

### 12.2 The fix set, as landed here

Six fixes in the lift source; five land here, one is not applicable. Where the lift consults
`ep_state` or controller-written context fields, the GUARD-STATE discipline is carried verbatim:
the Output Endpoint Context's TR Dequeue Pointer is only architecturally defined while the endpoint
is **not Running** (xHCI 1.2 §4.8.3), Intel silicon demonstrably freezes it at a birth value under
Running, and the VL805 — a different xHC whose behaviour here is **unverified** — is trusted no
further. Every consumer of the field either refuses to act on a Running-state reading
(`bot_ring_guard`) or labels it advisory (`ctxdeq_valid=no-ep-running` on the strand witness).

**1. The single chokepoint.** `bot_transfer` is now a thin wrapper around `bot_transfer_body`;
every `Err` other than `NoDevice` (raised before anything is built or queued) passes through
`bot_clean_rings`: Stop/Reset Endpoint (whichever the EP State admits), Set TR Dequeue Pointer on
**both** bulk rings at each ring's live enqueue slot, and an event-ring drain. The supporting tools
(`ep_state_of`, `ep_ctx_deq`, `recover_cmd`, `resync_bulk_ep`, `TransferRing::strand_scan`/
`would_lap`/`contains`) did not exist in this tree and are lifted with the fix. Wrapping the whole
body covers every audited exit and whatever exit a later arc adds.

**2. Ring capacity honesty, up front.** This tree had **no** capacity guard at all. The lifted
`bot_ring_guard` (backed by `TransferRing::would_lap`, xHCI 1.2 §4.9.1) now runs for every ring the
transaction will touch **before anything is pushed** — the lift source's own lesson, folded in: a
per-stage guard that runs after the CBW push manufactures the stranded-TRB condition it exists to
prevent. Per GUARD-STATE it refuses only from Halted/Stopped/Error, where the consumer position is
defined; a Running endpoint is never refused. The same pass propagates all push results
(`.map_err(RingFull)`) instead of discarding them — inert today (`push` always returns `Ok`), but a
failed push now fails honestly instead of waiting on a fabricated address.

**3. Short-transfer honesty.** `run_bot_stage` returns `(completion_code, residue)`; `BotPending`
carries the Transfer Event residue, first-write-latched (`residue_seen`) against duplicate-Success
quirks, exactly like `Ep0Pending::data_seen`. The data stage is judged against its own
`dCBWDataTransferLength`, and the treatment is deliberately **asymmetric — the asymmetry is the
spec's, and it is honored here, not "fixed"**:

* **OUT short is a phase fault** (BOT 1.0 §6.7.3 case 9, Ho > Do): the device stopped accepting
  bytes, so it is *not* in its status phase; queueing the CSW there is the step that slides the two
  machines apart. It fails the transaction (the chokepoint then cleans the rings).
* **IN short is legal** (BOT 1.0 §6.7.2 case 4, Hi > Di): `REQUEST SENSE` (18), `INQUIRY` (36) and
  `READ CAPACITY` (8) all name a *maximum* allocation length, and a conforming device may
  under-return and then sit in its status phase with the shortfall in `dCSWDataResidue`. Failing
  those would break bring-up on conforming devices.

The IN case is policed by the check with teeth instead: **`dCSWDataResidue` is now validated**
against the Transfer Event residue. The device's residue is *its* claim about bytes moved; the
event residue is the *controller's*. Two independent witnesses of one quantity — disagreement is a
phase fault (`residue_disagree`, `Err(TransferError(13))`), and a transfer that moved zero bytes
with `bStatus=0` is no longer reported to FAT as a clean success.

**4. De-aliased event matching.** The blanket "claim any error completion on either bulk DCI" is
gone from `handle_event_trb`. Errors that name a TRB are matched by address like anything else (a
bulk STALL carries its TRB pointer, so stall delivery to the pump — the property the blanket claim
existed for — is preserved). The fallback survives only for an error whose pointer addresses
nothing in either bulk ring (Ring Underrun / Ring Overrun / VF Event Ring Full), and it is
**counted** (`ev_unaddressed=`) rather than silent. A first-write latch on `done` refuses late
events for an already-completed stage (`ev_late=`). `BotPending` gains a monotonic `generation` — a
log key, not a wire tag (a Transfer Event carries only a TRB pointer), tying a timeout to the
strand lines that follow it.

**5. `send_scsi_read` deleted.** This tree carried the same uncalled fire-and-forget legacy as the
lift source: a hand-built CBW with a hardcoded `0xDEADBEEF` tag, pushed to both rings with
`.unwrap()`, doorbells rung, nothing awaited. A permanent supply of untracked TRBs and a tag no CSW
validation could match. Removed rather than documented.

**6. The `cfgep_cc=19` repoint — not applicable here.** The lift source's fix 6 patches
`rescue_reset_device`, a BOT-RESCUE escalation rung that `reset()`s both rings and can then fail
Configure Endpoint. This tree has no BOT-RESCUE ladder and no code path that resets a bulk ring's
producer state and re-programs the context in two steps, so the disagreement that fix closes cannot
arise here. If the ladder is ever lifted, its fix 6 comes with it.

### 12.3 PIUSB36-PHASE — the aarch64-only twin, closed

The lift source's §15.9 flagged `piusb36_read10_two_trb` (the PIUSB-36 step-5 two-TRB TD-shape
probe, aarch64-only) as carrying the same holes, out of its lane. Closed here, in our lane:

* it now runs through the **same chokepoint** (a wrapper calls `bot_clean_rings` on every `Err` but
  `NoDevice`);
* its ring headroom is checked **up front** and all four push results are propagated;
* its data-stage stall arm no longer returns immediately — that was the **pre-PIUSB-38 wedge
  shape** (the device stalls the DATA phase, not the command, so it sits in its status phase with a
  Failed CSW ready; abandoning the transaction leaves the two BOT machines one phase apart, and
  every later command on the slot inherits the wedge). It now clears the halt and **collects the
  CSW** (resync), escalating to full Reset Recovery only if the status stage also fails — exactly
  `bot_transfer_body`'s shape;
* its data stage gets fix 3: IN-short is allowed (same honored asymmetry) and cross-checked against
  `dCSWDataResidue`; the CSW signature is now validated (it was skipped) with the same counters and
  `csw_bytes` grammar.

The probe's two-TRB TD shape (256 B + 256 B, chain on the first, IOC on the second) — its entire
diagnostic point — is untouched.

### 12.4 Witness grammar

| line / field | where | says |
|---|---|---|
| `:: BOT: strand when=pre\|post … epstate= enq= cycle= ntrb= ctxdeq= dcs= ctxdeq_valid= gap= live= gen= ::` | every error exit, per pipe | the ring as the error found it, and as the cleanup left it |
| `:: BOT: clean slot= cause= in_resync= out_resync= in_live= out_live= undrained= ::` | every error exit | whether the cleanup succeeded on both pipes |
| `:: BOT: resync stage=stop-ep\|reset-ep\|skip\|set-deq … epstate=A->B ::` | inside the cleanup | each recovery command's outcome, with EP state before/after |
| `:: BOT: ring refuse slot= dci= … ::` | up-front guard | a stage refused instead of lapping the controller |
| `:: BOT: dtl_vs_moved slot= dir= dtl= moved= residue= cc= verdict= ::` | any short data stage | host-side shortfall, and how it was judged (`phase-fault` vs `short-in-allowed`) |
| `:: BOT: residue_disagree slot= dir= dtl= host_moved= dev_residue= dev_moved= bstatus= ::` | CSW validation | device and controller disagree about bytes moved |
| `:: BOT: csw_bytes slot= why= tag_want= b=… ::` | every CSW rejection | all 13 raw CSW bytes (torn read vs overlay vs stale-but-well-formed, decidable) |
| `:: BOT: stage timeout slot= wait_trb= gen= ::` | pump timeout | the generation that ties the timeout to its strand lines |
| `:: BOT: phase tag_mismatch= bad_sig= abandoned_in= abandoned_out= undrained= short_in= short_out= ev_late= ev_unaddressed= result=SUMMARY ::` | once per boot (topology summary) | boot totals for all of the above |

Reading keys (lifted, and they matter): `ctxdeq_valid=` states whether the `live=` count came from
a defined field (`epstate=2/3/4`) or an advisory Running-state read. **`undrained=` is fix 1's own
regression witness and must read 0 on every boot** — it counts pipes still holding valid-cycle TRBs
after the cleanup (from the *post* scan, the defined reading), or whose resync failed. Slots with
no reachable ring (no output context) are skipped explicitly, so the number stays an assertion.

### 12.5 Blast radius

The healthy path is byte-identical: fix 1 runs only on `Err`; fix 2's guards return `Ok`
immediately for a Running endpoint and nothing is pushed between them; fix 3 is arithmetic on a
value the event already carried (`moved == data_len` on a full transfer takes no branch); fix 4
only narrows what may claim a stage; fix 5 removes dead code. Nothing weakens a protection —
fixes 2/3/4 each *add* a check, and fix 3 converts a previously accepted condition (silent short
transfer) into a rejected one.

### 12.6 Gates, and what QEMU cannot prove — honest coverage

* `./arroyo check` — green, both arches.
* `./arroyo kernel8-test` — MBENCH PASS, 87/87 required witnesses, 0 forbidden.
* `./arroyo test-arm` — MISSION SUCCESS (the BOT proof through the shared xhci path), and the
  SUMMARY ledger reads `tag_mismatch=0 bad_sig=0 abandoned_in=0 abandoned_out=0 undrained=0
  short_in=0 short_out=0 ev_late=0 ev_unaddressed=0`.
* `strings` on `target/pi_baremetal/kernel8.img` proves every witness above (the SUMMARY ledger,
  `strand`, `clean`, `resync`, `ring refuse`, `dtl_vs_moved`, `residue_disagree`, `csw_bytes`,
  `stage timeout`) is present in the kernel8 builder artifact.

**Stated plainly: the error-exit and strand witnesses never execute in QEMU.** The chokepoint's
cleanup fires only when `bot_transfer` *returns* an error, and QEMU's `usb-storage` produces none
on these boots (injected faults are rescued earlier, and it does not stall, short-change or wedge).
`bot_clean_rings`, `bot_strand_witness`, `resync_bulk_ep`, `TransferRing::strand_scan`, the
OUT-short branch, the `residue_disagree` rejection, the `ev_unaddressed` fallback, every `ring
refuse`, and the piusb36 stall-collect arm are therefore **not executed** in emulation — `strings`
proves the text is in the artifact, and that is all emulation establishes. The `undrained=0`
readings above are real but **weak evidence**: they are taken on boots where the cleanup path never
ran. Their correctness rests on inspection (no new `unwrap` on these paths, every `Option` handled,
the strand scan bounded by the ring length) and on the first metal boot, which is the proof.

### 12.7 What metal must verify

1. **`undrained=0` on every boot**, including boots that see real transport faults — the central
   assertion.
2. A `:: BOT: strand ::` pair at every error exit, `when=post` showing `live=0` on both pipes; a
   `when=pre` line with `live>0` and `ctxdeq_valid=yes` is the first direct observation of a
   stranded TRB on the VL805.
3. Whether the VL805 refreshes `ctxdeq` under Running (compare pre-scan `live` against post-scan) —
   the GUARD-STATE question this tree has never answered on its own silicon.
4. `short_out=0`; any non-zero is a phase fault caught before it reached the medium.
5. `ev_late=` / `ev_unaddressed=` — the first real measurement of event aliasing on the VL805.
6. The piusb36 stall arms, if a stall ever occurs during the matrix: the probe should now come back
   with a Failed CSW instead of wedging the slot.

---

## 18. KEYREPEAT-X86 — key repeat on the EHCI keyboard (Boot AL, 2026-08-08)

### 18.1 The observation

Peter, live on the rMBP bench during Boot AL: *"so far so good with keys except no key repeat."*
Every other property the GR21 EHCI keyboard arc landed holds on metal — caps lock, Ctrl+letter, no
stuck keys, `kill` works, SPACE both pauses and unpauses, shifted symbols release as themselves.
Holding a key produced exactly one character.

### 18.2 The cause, and the refuted premise

`decode_boot_keyboard` pushes `Event::Key` at the LEVEL — every held keycode, every report — and its
doc block concluded from that: *"repeat on this arch is the DEVICE's, carried by the re-reported
level."* That premise is false for this device. No `SET_IDLE` is sent on the EHCI HID path (stated
independently in the KBDWIT note), so the internal keyboard runs its default report-on-change
behaviour: a key held still produces no further reports at all. No re-reported level, therefore no
repeat. The level loop was correct and simply had nothing to loop over.

The same fact, from the same cause, is what UVUG-5 was written for on aarch64.

### 18.3 The fix: the shared tracker, not a second one

Two review-found boundaries of the fix, on the record before the metal proof:

- **`pal::pump_and_poll` does not tick.** The three top-level x86 service loops all tick the
  tracker; `pump_and_poll` (the inner pump kernel full-screen demos hold while they block a
  shell command) services EHCI but deliberately does NOT call `typematic_tick` — on the
  SCHED-X86 shape the service core's `x86_usb_pump` keeps ticking concurrently, and a second
  ticker on the demo core would race it. The cost is confined to the inline-BSP boot shape
  (<2 APs) with a kernel demo active: reports still arm, but no repeats inject for the
  demo's duration. Known, bounded, and the wire shows it (no `[keystat]` between the demo's
  entry and exit lines on that shape).
- **Cross-device detach coupling.** `note_keyboard_detached` is bumped by ANY keyboard slot
  teardown, including an xHCI external keyboard detaching or enum-recovering — which disarms
  a hold on the internal EHCI keyboard mid-repeat (safe direction: a lost repeat, never a
  stuck key). "Repeat stopped when an unrelated USB device bounced" is this, not a bug.
- Operator note: holding **Enter** at the shell now re-executes the line at ~25/s — same as
  the Pi, correct, and new on this machine.

`pal.rs`'s host-side typematic tracker was `#[cfg(all(target_arch = "aarch64", feature =
"baremetal"))]`. Its cfg is widened to

```rust
#[cfg(any(
    all(target_arch = "aarch64", feature = "baremetal"),
    all(target_arch = "x86_64", feature = "ehcihid")
))]
```

on `mod typematic`, `pack_held`, `typematic_note_report`, `typematic_hold_rollup`,
`typematic_lapse_disarm` and `typematic_tick`. The `typematic_test_*` aids stay aarch64-only — only
the aarch64 selftest calls them.

Widening rather than reimplementing is the point: this tracker was purpose-built for a keyboard
whose correct behaviour is SILENCE while a key is held. Its wedge classes are already cured, on
metal, on the Pi — report-level arm/disarm so no dropped `KeyUp` can strand a hold (P51), the
UVUG-9 evidence gate so silence is never read as a wedge (the ~10-repeat stop), the PAL-TYPEMATIC
lapse re-arm, and the half-full `EVENT_QUEUE` backpressure guard. A private x86 copy would re-open
all of them.

Three wiring points, all x86-side:

1. **Feed.** `decode_boot_keyboard` calls `pal::typematic_note_report(newest_press, &held)` at the
   report level, before the release edges, with both the newest press and the full held set resolved
   through the same `ascii_of` fold the `Event::Key` pushes use. One-for-one with the xHCI feed.
2. **Detach.** `flush_held_releases` (endpoint death) now also calls `pal::note_keyboard_detached()`.
   The synthesised `KeyUp`s it emits are EVENT-level and the tracker deliberately does not observe
   the event stream, so without this a key armed at the instant of death would repeat until the
   30 s `HOLD_MAX_MS` backstop. This is layer 2, used exactly as xHCI's `reset_soft_state` uses it.
3. **Pump.** `main::x86_typematic_pump()` calls `pal::typematic_tick()` once per device-service pass
   and pushes the returned ascii into `EVENT_QUEUE`, from all three mutually-exclusive x86 service
   loops (`usbdebug`, the inline BSP console loop, and `x86_usb_pump`), so repeat does not depend on
   how many APs came online. Injecting into `EVENT_QUEUE` means the repeat rides the same routing a
   real press takes — `x86_input_service` → `GUI_CHANNEL_X86` → the render task, with the same asid
   focus rules and the same per-process ring for a focused ring-3 app.

Constants are the shared ones: `DELAY_MS` 400, `RATE_MS` 40 (~25 chars/s).

### 18.4 SET_IDLE(rate) was considered and rejected

The alternative was to send `SET_IDLE` with a non-zero rate so the device re-reports and the existing
level loop delivers repeat. Rejected on four counts, any one of which is sufficient:

- **No initial delay.** Repeat would begin at the first idle period, so an ordinary 80 ms tap emits
  several characters. Delay-then-rate needs host logic regardless.
- **Rate is the device's, in 4 ms units, and is coarse and unenforceable.**
- **Devices may STALL `SET_IDLE`.** The boot-protocol spec permits it. A device that refuses leaves
  x86 with no repeat at all, i.e. the arc silently does not land.
- **It inverts the held-state contract.** The `SET_IDLE(0)` silence is what the GAME-MODE held-state
  consumers were written against; making this endpoint stream changes what every consumer sees.

By contrast the host tracker is silence-proof and device-independent.

### 18.5 What is NOT changed

The `Event::Key` / `Event::KeyUp` edge logic in `decode_boot_keyboard` is untouched. GR21's release
synthesis is metal-proven as of Boot AL and the tracker is a pure observer at that site — it pushes
nothing from the decoder.

**aarch64 is behaviourally untouched, and this was measured, not asserted.** `./arroyo kernel8` is
reproducible on this tree (same sources → same hash, verified by a repeat build). Comparing the
aarch64 kernel ELF before and after the change:

| section | result |
| --- | --- |
| `.text` (0xd2618 bytes) | **byte-identical** |
| `.data` | **byte-identical** |
| `.rodata` | **1 byte differs** |
| `.symtab` / `.strtab` | shifted strings |

The single `.rodata` byte is a `core::panic::Location` line number for a `#[track_caller]` site in
`main.rs`, `3529 → 3535` — the six lines the two x86 pump call sites add above it. No aarch64
instruction changed.

### 18.6 Witness grammar

- `:: KEYREPEAT-X86: first synthesised repeat — key=0x.. '<c>' (host typematic armed on the EHCI
  keyboard) == witness ::` — once per boot, at the first repeat. New in this arc, x86 only. The
  rollup below only prints when a hold ENDS, so without this a capture taken mid-hold could not
  distinguish "repeating" from "never reached".
- `[keystat] typematic hold end — key=0x.. repeats=N re-arms=M window=Wms (boot: repeats=.. re-arms=..)`
  — shared code, per hold. `window=30000` is the expected value here (this keyboard does not idle
  re-report, so it never earns the tight 1000 ms window).
- `[keystat] typematic re-arm — …` — a lapse that recovered. Should not appear on a healthy hold.
- `[uvug9] typematic hold-max — …` — the 30 s backstop. Should not appear at human hold lengths.

No new knob. Repeat is on whenever `ehcihid` is, which is default-ON.

### 18.7 What metal must verify

See `PREDICTION-keyrepeat.md` at the tree root for the falsifiable statement.

---

## 19. DBLSTROKE — the press loop was level-triggered (Boot AN, 2026-08-08)

Peter, at the bench on Boot AN: *"key repeat good but typing fast causes double stroke."* Held-key
repeat (§18) is correct on metal and normal-speed typing is correct; typing FAST doubles characters.

### 19.1 The mechanism, read straight off the capture

`decode_boot_keyboard`'s press loop pushed `Event::Key(ascii)` for **every keycode in every report**.
A USB boot report is a LEVEL — bytes 2..8 re-state the full held set — so any report arriving while a
key is still down re-delivered that key as a fresh press. Fast typing is *defined* by overlap (the
next key goes down before the last comes up), so every overlapped pair produces exactly such a
report. `EHCI-HID: KEY:` is logged once per push, so Boot AN convicts it with no new instrument:

```
[1228135ms] KEY: 'h'                  report [h]     press edge
[1228275ms] KEY: 'h'   KEY: 'e'       report [h,e]   'h' RESTATED, 'e' pressed
[1228413ms] KEY: 'e'   KEYUP: 'h'     report [e]     'e' RESTATED, 'h' released
[1228427ms] KEY: 'l'                  report [l]     press edge
```

Six `Event::Key` for the four physical presses of "help", twice in the capture (1228135 ms and
1233645 ms). Type slowly enough that each key is released before the next goes down and every report
carries one key: no duplicate is possible. That is the reported symptom, exactly bounded.

Ruled out on the same evidence: the typematic tracker (doubles 24..140 ms apart with no 400 ms delay
elapsed; `re-arms=0` boot-wide; it pushes nothing from the decoder), the CLICK-3 silence/re-press
recovery (pointer path — it stamps no keyboard state), a duplicated pump (the two pushes carry two
different report timestamps from one `serial_println!` site), and the console/line-editor consumer
(the doubling is in the producer's own line, before any consumer sees an event).

### 19.2 The fix

The press loop now pushes only on a press EDGE — `!prev_keys.contains(&keycode)` — reading the same
`prev_keys`/`cur_keys` diff the release loop has always read, in the opposite direction. Press and
release are one fact seen twice and cannot disagree about how many there were. This is also the
contract the rest of the tree was already written against: `vug.rs` GAME-MODE states it verbatim
("the HID path delivers a Key on the PRESS edge and a KeyUp on the RELEASE edge").

Nothing pays for the lost level repeat, because there was none to lose *on this machine*: no
`SET_IDLE` is sent on this path (KBDWIT) and the internal keyboard is report-on-change, which is why
§18 had to add the host tracker at all. Level-driven repeat here was only ever the doubling, paced by
the operator's other fingers rather than by any repeat rate. On a hypothetical idle-re-reporting
keyboard the tracker's 400 ms/40 ms is a better repeat than an unthrottled poll-rate spew.

Untouched: the release loop, the dead-endpoint flush, shifted-symbol release folding, Ctrl folding,
caps lock and the LED SET_REPORT, and the tracker feed (still `newest_press` + the full held set
through the same `ascii_of` fold).

### 19.3 Witness grammar

- `[keystat] ehci press — edges=E restated=R rollover_reports=P dbl=D window=50ms` — first three
  suppressions each print, then one line per 32. `restated` counts the pushes the edge gate
  suppressed, i.e. the doubles the old loop would have emitted; `R>0` is the positive proof that the
  operator typed with overlap, which is what makes a clean `dbl` result meaningful rather than merely
  untested. `rollover_reports` counts reports with two or more character keys down — "typing fast"
  measured rather than described.
- `[keystat] ehci double-push — ascii=0x.. pushed twice Nms apart (<= 50ms); the PRODUCER is
  doubling, not the console (boot dbl=..)` — first 8 individually, then counted. An INDEPENDENT
  detector over what the decoder actually pushes, so it stays valid however the input side is
  rewritten. It is what makes the next boot decisive either way: doubles on screen with `dbl=0` and
  `restated>0` moves the fault downstream to the console echo or the line editor; doubles with
  `dbl>0` refutes this diagnosis and names the ascii and interval.

Both counters are boot totals, zero-cost on a boot where nobody types (no line is emitted). x86-only
by construction — `drivers/mod.rs` gates `pub mod ehci` on `all(target_arch = "x86_64", feature =
"ehcihid")`, so aarch64 compiles none of it and its codegen is byte-identical.

### 19.4 What metal must verify

`~/unaos-bench/scratch/gr22/dblstroke-predictions.md` — the falsifiable statement, with the
falsifiers spelled out. Headline: "help" typed at speed yields exactly four `EHCI-HID: KEY:` lines,
`dbl=0`, `restated>0`, and §18's `[keystat] typematic hold end` unchanged.

## 20. MTFIX — the Bluetooth radio ate the trackpad's interrupt-EP slot (Boot AN, 2026-08-08)

### 19.1 The symptom

Boot AN (trunk `d7155e29`) had **no mouse**: over a 20-minute sit with the operator working
the trackpad, not one motion decode and no cursor. The keyboard (addr 6) and its typematic
repeats worked the whole time. Boot AL (trunk `eacef0bb`, before bt-l0) had a working cursor.

### 19.2 The conviction — a static slot budget, printed verbatim in the log

Each EHCI controller owns a fixed pool of interrupt-endpoint slots in its `DmaPool`
(`drivers/ehci/qh.rs`), and `MAX_INT_EPS` was **4**. Controller [1]'s arm order on the rMBP:

| ms | endpoint | slot |
|---|---|---|
| 1773 | keyboard addr 6 IN1 | `int_slots[0]` |
| 1799 | boot-mouse addr 7 IN1 | `int_slots[1]` |
| 1824 | **bt-l0 HCI event endpoint, addr 8 IN1** | `int_slots[2]` |
| 1859 | trackpad's boot-keyboard interface, addr 9 IN3 | `int_slots[3]` |
| 1860 | trackpad's **vendor-multitouch** interface, addr 9 IN1 | *(none — skipped)* |

`bt_arm_events` took `int_slots[int_next]` and bumped `int_next`, so the radio's event
endpoint — which is read synchronously by the L0 sequence and is deliberately never handed to
`Controller::service` — nevertheless owned one of the HID budget's four slots for the whole
boot. The internal trackpad's multitouch interface is the **last** endpoint enumerated, so it
is the one that fell off the end. It was never armed: no QH, no frame-list link, no `int_eps`
entry. The log says so plainly, at 1860 ms:

```
:: EHCI-HID: [1] static int-EP pool exhausted (4) — endpoint skipped ::
:: EHCI-HID: [1] M1 armed vendor-multitouch addr=9 ep=IN1 mps=64 interval=2 id=0x44 … == witness ::
```

Before bt-l0 the same machine used exactly 4 of 4. The radio pushed it over by one.

### 19.3 The second defect — a witness that could not fail

The `M1 armed vendor-multitouch` line above is the endpoint that had just been skipped.
`arm_interrupt_ep` returned `()`, and **both** call sites printed their `== witness` arm line
and stamped `bootlog` unconditionally after calling it. That is what made "the trackpad is
armed and silent" the working theory for a whole sitting — the log asserted an arm that never
happened. `arm_interrupt_ep` now returns `bool`; the witnesses and the `ehci:trackpad-armed`
milestone are gated on it, and the exhaustion trace is a `STOP-NOTE` naming addr/intf/ep.

### 19.4 The fix

1. `DmaPool` gains `#[cfg(feature = "bt")] pub bt_slot: IntSlot` — the HCI event endpoint gets
   its own slot, outside `int_slots`. Knob-off the field does not exist and the pool layout is
   unchanged.
2. `bt_arm_events` uses `bt_slot`, no longer touches `int_next`, and refuses a second arm on
   the same controller (`Controller::bt_evt_armed`). The quiesced QH stays **linked** in the
   periodic chain for the life of the boot (`bt_quiesce_events` only clears Active — the chain
   must not be rewritten behind endpoints armed after it), so re-arming that slot would rebuild
   a QH the controller is still walking *and* splice its own physical address in as its own
   `horiz` successor: a self-loop in the frame list.
3. `MAX_INT_EPS` 4 → 6. Freeing the BT slot alone restores the exact pre-bt-l0 4/4 fit; the
   headroom is what keeps one extra plugged-in HID device from starving the internal trackpad
   again, given an arm order that puts it last. Cost is two static `IntSlot`s per controller.

Refuted along the way: the ordering hypothesis ("the BT event endpoint sits ahead of the
trackpad in `int_eps` and the service loop starves everything after it"). `bt_arm_events` never
pushes into `int_eps`, so `Controller::service` cannot see or mis-iterate past it.

### 19.5 What metal must verify

`~/unaos-bench/scratch/gr22/mtfix-predictions.md` carries the falsifiable statement. In short:
no `static int-EP pool exhausted` line anywhere in the boot; the `M1 armed vendor-multitouch`
witness still present at ~1860 ms and now true; cursor armed and motion decoding; and every
bt-l0 witness (census/claim/reachability, `HCI_Reset -> CmdComplete status=0x00`, `hci_ver=0x06
… -> BROADCOM`) unchanged on a `UNAOS_BT=1` boot.

---

## 21. BT-L2 — LE scan: the radio discovers the room (`UNAOS_BT=1`, 2026-08-08)

### 21.1 Where L2 sits

`bt_probe` (in `drivers/ehci/mod.rs`, feature `bt`) now runs three stages against the Broadcom HCI
controller behind the internal Bluetooth hub:

| stage | what it does | witness prefix |
|---|---|---|
| L0 | claim by endpoint evidence, arm the event endpoint, `HCI_Reset`, `Read_Local_Version` | `bt-l0:` |
| L1 | BD_ADDR, buffer size, supported features/commands, first write (`Set_Event_Mask`) | `bt-l1:` |
| **L2** | **LE scan: mask, scan parameters, enable, bounded drain, disable** | `bt-l2:` |
| L3 | connect to one LE peer, and always let go (§22) | `bt-l3:` |

No new endpoint is armed at any stage past L0 — L2 is more commands and a drain on the endpoint
that already exists, so the MTFIX slot budget of §20 is untouched.

### 21.2 The event mask is the whole ballgame

L1's `HCI_Set_Event_Mask` wrote the Bluetooth Core **reset default**, `0x0000_1FFF_FFFF_FFFF`. That
value is correct as a write-path exercise and wrong for a scanner: it stops at bit 44, and **LE Meta
Event is bit 61**. Every LE Advertising Report is delivered as an LE Meta Event (event code `0x3E`,
subevent `0x02`), so a scan run behind the reset default enables correctly, hears every device in
the room, and reports *nothing* — a clean, silent, entirely wrong "no devices found".

L2 therefore rewrites the mask to `0x2000_1FFF_FFFF_FFFF` (bit 61 = octet 7 bit 5 = `0x20`) before
anything else, and only then sets the **LE** event mask (`HCI_LE_Set_Event_Mask`, OGF 0x08 / OCF
0x0001 => `0x2001`) to `0x1F` — the LE reset default, whose bit 1 is LE Advertising Report. Both
writes are witnessed with their status; if either does not return `0x00` the scan does not start,
because a scan behind a closed channel would produce a number that means nothing.

This is also why a zero-device rollup is a real finding here rather than an ambiguity: the rollup's
zero-device line states that both masks were confirmed, so zero means silence on the air.

### 21.3 Scan parameters, and why these numbers

`HCI_LE_Set_Scan_Parameters` (OGF 0x08 / OCF 0x000B => `0x200B`), seven bytes:

| field | value | why |
|---|---|---|
| LE_Scan_Type | `0x00` passive | listen only, never transmit SCAN_REQ: discovery must not perturb the room, and cannot be observed by it. Cost: no SCAN_RSP payloads, so names come only from the advertising PDU. |
| LE_Scan_Interval | `0x0060` = 60 ms | rotates all three advertising channels (37/38/39) in 180 ms, so a 500 ms window covers each several times. |
| LE_Scan_Window | `0x0060` = 60 ms | **equal to the interval = continuous scanning.** At any lower duty cycle a device could advertise entirely inside the deaf half and the arc would report an empty room it never listened to. A short bounded window is only honest at 100 % duty. |
| Own_Address_Type | `0x00` public | the BD_ADDR L1 read. Passive scanning transmits nothing, so this declares rather than decides. |
| Scanning_Filter_Policy | `0x00` accept all | the white list is empty on a freshly reset controller; any other policy filters everything out. |

`HCI_LE_Set_Scan_Enable` (`0x200C`) then enables with `filter_duplicates = 1`, which makes the
report count a measure of devices rather than of how chatty the room is.

### 21.4 The bounded window, and the read primitive it forced

The drain runs for `BT_L2_SCAN_MS` = **500 ms** of wall clock, measured on the TSC. 500 ms is chosen
against real advertising intervals: connectable-discoverable devices (phones, watches, headphones)
sit in the 20-300 ms band and are seen several times over, while a device on a 1.28 s low-power
interval can be missed. **The rollup reports a window, never a room.**

Making that bound real required splitting the L0 read primitive. `hw_wait_budget()` on x86 is
`HW_WAIT_SECONDS` = 2 s, so the old `bt_read_event` (arm + wait one full budget) would cost seconds
per silent read and no bounded window could exist. `bt_read_event` is now:

- `bt_arm_read` — arm one interrupt-IN transfer, no wait;
- `bt_wait_read` — poll the armed transfer for a **caller-supplied** budget;
- `bt_read_full_event` — reassemble one whole event across packets (the L1 mechanism, unchanged),
  returning `Got` / `Idle` / `Stop`.

`Idle` is the case that only a deadline-driven reader can hit: the first packet's budget expired
**with the transfer still armed**. Arming a second qTD over a live one would clobber a descriptor
the controller may be executing, so `Idle` is not silently retried — the armed transfer is handed
forward (`armed`) to the next command through `bt_hci_command_ex`, whose CommandComplete lands in
it. Continuation packets *inside* an event always use the full budget: abandoning an event half-read
is what actually desynchronises the toggle, so a mid-event expiry is `Stop`, not `Idle`.

L0/L1 pass `hw_wait_budget()` and get their pre-L2 behaviour, message for message.

### 21.5 Decoding, and what is reported

An LE Advertising Report carries, per report: Event_Type(1), Address_Type(1), Address(6, little
endian — rendered MSB-first like L1's `bd_addr=`), Length_Data(1), Data, RSSI(1, signed; `127` =
not available). The AD payload is walked as `(Length, AD_Type, Value)` triples far enough to pull a
**Complete (0x09)** or **Shortened (0x08) Local Name**; a complete name ends the walk.

The spec renders the report fields as parallel arrays when `Num_Reports > 1`; controllers in
practice emit exactly one. Rather than guess a layout this arc has never seen on the wire, the first
report is decoded and the remainder are **counted** and named in the rollup
(`multi_report_events=`, `extra_reports_not_decoded=`).

Devices are keyed by (address, address type) into a 16-entry table — a device that rotates its
resolvable private address mid-scan is genuinely a different address on the air, and this table
reports the air. Nothing is printed inside the loop: serial at 115200 is slower than the event
stream, so a per-report print would make the instrument change what it measures. Reports for a
seventeenth distinct address are counted and the rollup says the table truncated; **silent
truncation would read as "that is all there was".**

### 21.6 Off on every exit path

A radio left scanning burns power and floods the event endpoint for the rest of the boot, on the
same EHCI controller as the internal keyboard and trackpad. So `HCI_LE_Set_Scan_Enable(0)` runs on
every path that could have started a scan — including the **unconfirmed** enable, where the command
went out on EP0 but no CommandComplete came back and the controller must therefore be assumed to be
scanning. The paths that return *before* the enable (mask refused, parameters refused, an explicit
nonzero enable status) never started anything and have nothing to undo; each says so.

If the disable's own CommandComplete is not observed, the witness says exactly that: the EP0 write
is what stops the radio and it was attempted; what is missing is the confirmation, not the attempt.

### 21.7 The stage guard

L1's review flagged that L1 ran even where L0 had timed out, on a toggle whose relationship to the
device was unknown. Blast radius was nil because L1's only write was idempotent. A scan is not: it
turns on a repeated event stream. So each stage now records whether it **confirmed** — `reset_ok`,
`ver_ok`, `l1_ok` (every row well-formed with status `0x00`, including the `n >= 65` reassembly
guard of review C1) — and L2 starts only if all three hold *and* the LMP feature mask claims
`LE(controller)`. Otherwise it prints which one failed and leaves the radio as L1 left it.

### 21.8 The firmware boundary

Every LE command used here is defined by the Bluetooth Core spec, not by Broadcom. A status
`0x01` (Unknown HCI Command) on any of them would mean the controller's ROM does not carry LE until
a patchram `.hcd` is loaded — which is the clean-room boundary of
[`docs/MANIFESTO/CLEAN_ROOM_POLICY.md`](../../../MANIFESTO/CLEAN_ROOM_POLICY.md) and belongs in
UnaOS-bunker. `bt_l2_cmd` witnesses that status explicitly and the sequence stops. **No firmware
path is added by this arc.**

### 21.9 What metal must verify

`~/unaos-bench/scratch/gr22/btl2-predictions.md` carries the falsifiable statement.

---

## 22. BT-L3 — connect to one LE peer, and always let go (`UNAOS_BT=1`, 2026-08-09)

### 22.1 Where L3 sits, and the gate in front of it

L3 runs inside `bt_le_scan`, **after** L2's mandatory `HCI_LE_Set_Scan_Enable(disable)` has come
back with status `0x00`. Initiating while a scan is enabled is a state this arc declines to enter:
the Core spec permits a controller to answer `HCI_LE_Create_Connection` with Command Disallowed
while scanning, and that refusal would be indistinguishable from a controller that cannot connect at
all. So the order is **scan → disable (confirmed) → connect → let go**.

The gate is three conditions, all of which must hold or no create is issued:

1. the drain latched a peer — the first `ADV_IND` of the window (§22.2);
2. the event endpoint is not halted — L3 must be able to *read* its own events;
3. the scan disable returned status `0x00`.

Any other combination prints `connect NOT ATTEMPTED` naming which condition failed. That is also the
only way to have nothing outstanding *by construction*.

No new endpoint is armed. L3 is more commands on the endpoint L0 already owns, so §20's slot budget
is untouched, and L3 writes **no event mask**: L2 took the LE reset default `0x1F` whole rather than
just the Advertising Report bit it wanted, and **bit 0 of that value is LE Connection Complete**.
The outer `HCI_Set_Event_Mask` bit 61 (LE Meta) L2 widened is likewise not narrowed on the way out.
L3 states that inheritance in its first witness line rather than rewriting a mask to be sure.

### 22.2 Which advertiser, and why only one kind

Of the five `Event_Type` values an advertising report can carry, only two are connectable:
`ADV_IND` (`0x00`, connectable undirected) and `ADV_DIRECT_IND` (`0x01`, connectable **directed**).
L3 accepts **`ADV_IND` only**. `ADV_DIRECT_IND` names an initiator address in its payload and that
address is not ours — connecting to a device actively soliciting a different peer is an intrusion,
and the controller would ignore our CONNECT_IND anyway. `ADV_SCAN_IND` (`0x02`) and
`ADV_NONCONN_IND` (`0x03`) are non-connectable by definition; `SCAN_RSP` (`0x04`) is not an
advertisement.

#### 22.2a Who L3 is *allowed* to connect to — the name filter (white board Q6, ruled)

First-heard **reaches into the room.** On a bench with neighbours, the first `ADV_IND` is a
stranger's phone, a tracker, or — the case that decided this — another machine's BLE keyboard or
mouse, which a CONNECT_IND takes away from its owner for as long as the link is held. Two
independent reviews reached that conclusion separately, and **Peter ruled**: the bench connects to
his own speaker, an Ultimate Ears **MEGABOOM**, and to nothing else.

The filter is **by advertised name, not by BD_ADDR** — the address is not known, and a name is what
makes the run reproducible for whoever is at the bench: turn the speaker on, boot, and it is the
peer.

```rust
const BT_L3_PEER_NAME: Option<&str> = Some("MEGABOOM");
```

| filter | rule | when it applies |
|---|---|---|
| connectable | the address must have been heard advertising `ADV_IND` (`Event_Type` `0x00`) at least once | always, and the flag is **sticky** (`BtDev::conn_seen`). `devs[i].evt` is last-report-wins, so a device that advertised connectably and then had a scan response overheard would otherwise end the window looking like a `SCAN_RSP` and be refused a connection it was soliciting. |
| address type | `Peer_Address_Type` must be `0x00` public or `0x01` random | always. `0x02`/`0x03` are the *resolved identity* forms — a 4.0 part does not accept them in `HCI_LE_Create_Connection`, and this arc has no resolving list to have produced one honestly. Posting one raw is an out-of-range parameter dressed as a peer. |
| **name** | advertised Local Name **contains** `BT_L3_PEER_NAME`, case-insensitive | when `Some` and non-empty. `Some("")` is "no filter written by accident" and is **not** honoured as one. |
| RSSI floor | `BT_L3_RSSI_FLOOR` = **-60 dBm** | only when the name filter is `None` — a *named* peer across the room is still the right peer. RSSI `127` means *not available*; a floor cannot be applied to an unknown value and admitting unknowns would make the rule decorative, so `127` is skipped under its own name. |

**Selection happens after the window, off the merged device table** — not inside the drain loop.
The name is why: a device's Local Name may arrive in a *later* report than its first sighting, so a
first-heard rule evaluated in-loop would judge a device on a name it had not yet said. The table has
merged all of it by then, and the pass is free (nothing prints inside the loop; this runs once over
at most 16 entries with the radio already quiet).

**The name decode is L2's, reused unchanged** — the `BT_AD_NAME_COMPLETE` (`0x09`) /
`BT_AD_NAME_SHORT` (`0x08`) walk in `bt_le_drain`, including the empty-Complete-name guard fixed
earlier. There is no second name parser.

> **THE MERGE RULE THAT WOULD HAVE LOST THE SPEAKER.** The table's name merge was
> *first-name-wins* (`if devs[i].nlen == 0 && nlen > 0`). A device that advertises a **Shortened**
> Local Name first and the **Complete** one in a later report kept the short one for the whole
> window — so a MEGABOOM heard as `MEGA` and then as `MEGABOOM` would print `SKIP:name-mismatch`
> and never be connected to, with a log that looks like a clean, correct no-match. That is exactly
> the failure Peter would experience as *"it didn't find my speaker"*, with nothing in the capture
> admitting it. The merge is now **monotone-more-information**: a stored name is replaced when
> nothing is stored, when a Shortened one is superseded by a **Complete** one, or when both are
> shortened and the new one is longer. A Complete name is never replaced — the device has said
> there is no more name to wait for. `BtDev::ncomplete` tracks which kind filled the slot.

And because a shortened name is a **prefix** of the complete one, a miss against it is not a miss
against the device. When the target could still **straddle the cut** (some non-empty suffix of the
stored name equals the corresponding prefix of the target), the verdict is
`MAYBE:short-name-prefix(heard, NOT connected — needs the complete name)` and the count surfaces on
the `peer NOT SELECTED` line as `maybe_short_name=N`, with the line telling the reader to stop
before concluding the device was absent. **A maybe is never connected to** — this arc does not reach
for a device on a guess — but *"my speaker was not found"* and *"my speaker was heard and could not
be confirmed"* are different lines in the capture, because they have completely different fixes. Matching is ASCII-case-insensitive on purpose: advertised
names are UTF-8 and a correct Unicode fold is a table this driver has no business carrying, so
non-ASCII bytes compare verbatim — which can miss, never falsely hit.

> **PASSIVE SCAN, AND WHAT IT MAY NOT HEAR.** L2 scans passively; it sends no `SCAN_REQ`. A name
> that a device carries *only* in its `SCAN_RSP` is reachable here only when some **other** nearby
> device solicits it and our controller is listening. The merge accepts a name from any report type,
> so an overheard scan response does supply one — but it is not guaranteed. Whether a MEGABOOM puts
> its name in the `ADV_IND` payload or only in the scan response is a question metal answers, not
> this document. **Switching to an active scan is NOT done here**: active scanning transmits a
> `SCAN_REQ` to every advertiser in the room, which is a larger decision than this arc's brief and
> is Peter's to make. If the bench shows `SKIP:no-name-advertised` against the speaker's address,
> that is the finding, and active scan is the separate decision it opens.

Every candidate is witnessed. Each distinct device's own L2 line now carries an **`l3=` verdict**,
so a capture answers *"why not that one?"* for every device in the room — a peer that was not
selected is otherwise indistinguishable from a peer that was never heard:

```
:: bt-l2: [N] dev 03 addr=.. type=public evt=ADV_IND rssi=-48dBm reports=7 name="MEGABOOM" l3=SELECTED == witness ::
:: bt-l2: [N] dev 04 addr=.. type=random evt=ADV_IND rssi=-71dBm reports=2 name="Pete's Buds" l3=SKIP:name-mismatch == witness ::
:: bt-l3: [N] peer rule — NAME FILTER ARMED, name="MEGABOOM" (case-insensitive substring ...) == witness ::
:: bt-l3: [N] peer SELECTED addr=.. type=public — ... considered=N matched=1 == witness ::
```

and when the speaker is off or out of range, in those words, with no create issued:

```
:: bt-l3: [N] peer NOT SELECTED — no device passed the filters: name=MEGABOOM considered=N matched=0.
   NO HCI_LE_Create_Connection is issued, nothing is outstanding ... == witness ::
```

The verdicts include `SKIP:no-name-advertised`, `SKIP:name-mismatch`, `MAYBE:short-name-prefix`,
`SKIP:identity-address-type`, `SKIP:below-rssi-floor`, `SKIP:rssi-unavailable`, `not-connectable`,
`SELECTED`, and `also-matched` (a second device answering the same name — not an error, but connecting to both is
not on offer and picking silently would hide the ambiguity). A `~(cut)` next to a
`SKIP:name-mismatch` is the one combination worth reading twice: the match was tried against a name
truncated at `BT_L2_NAME_MAX` (24 bytes), so it may be a **false miss**, and it is visible rather
than silent.

The RSSI floor, the fallback rule, is a **mitigation and not a guarantee** and the witness says so
in those words: RSSI is not distance, a high-power advertiser two rooms away can clear -60 dBm and a
shielded one on the desk can fail it. It is worth having on its own terms because the failure it
prevents is the one that matters — silently connecting to the *loudest* stranger.

### 22.3 `HCI_LE_Create_Connection` (`0x200D`), 25 bytes, every one justified

| field | value | why |
|---|---|---|
| LE_Scan_Interval / Window | `0x0060` / `0x0060` = 60 ms, continuous | L2's argument unchanged: at a lower duty the peer could advertise entirely inside the deaf half and a bounded window would report a failure it never listened for. |
| Initiator_Filter_Policy | `0x00` use the peer address below | `0x01` uses the white list, which is empty on a freshly reset controller and would match nothing. |
| Peer_Address / _Type | from the report | the only values L3 does not choose. |
| Own_Address_Type | `0x00` public | unlike the passive scan, an initiator **transmits** — so this now decides what goes on air, and the honest value is the BD_ADDR L1 read. |
| Conn_Interval_Min / Max | `0x0018`/`0x0028` = 30/50 ms (range `0x0006..=0x0C80`) | a range, not a point, so the peer picks something it already runs; 30-50 ms is the ordinary interactive band and short enough that the link is up and down inside the bounded window. |
| Conn_Latency | `0x0000` (range `0..=0x01F3`) | the link is held for milliseconds, so there is no power to save, and zero removes the `(1+latency)*interval_max*2 <= timeout` interaction. |
| Supervision_Timeout | `0x0064` = 1000 ms (range `0x000A..=0x0C80`) | the spec floor here is 100 ms; 1000 ms clears it 10x. Deliberately not the minimum — a timeout at the floor makes an ordinary retransmission look like a lost link, and this arc would then report a peer failure it caused itself. |
| Min/Max_CE_Length | `0x0000` / `0x0000` | no ACL data moves, so any CE length we asked for would be an invented constraint on a scheduler that knows better. |

**Create_Connection does not return a Command Complete.** It returns a **Command Status** (`0x0F`),
because its real result arrives later as an `LE Connection Complete` meta event. That single fact is
why L3 cannot reuse `bt_hci_command_ex` — which matches only `0x0E` — and why `bt_l3_await` exists.
`HCI_Disconnect` (`0x0406`) is the same shape; `HCI_LE_Create_Connection_Cancel` (`0x200E`) is the
odd one out and *does* answer with a Command Complete.

### 22.4 The teardown, on every exit path

An unresolved create is not a loose end, it is a **stuck controller**: the Initiating state persists
and later LE commands are refused with Command Disallowed for the rest of the boot. So:

* create resolved into a live link → `HCI_Disconnect(handle, reason 0x13 Remote User Terminated)`,
  then a bounded wait for `Disconnection Complete` (`0x05`);
* create issued and not seen to resolve → `HCI_LE_Create_Connection_Cancel`, then the
  `LE Connection Complete` (status `0x02`) that reports the withdrawal;
* create refused with an explicit nonzero Command Status → the controller never entered the
  Initiating state, so **no cancel is owed** and none is issued;
* the EP0 control-OUT for the create itself failed → nothing reached the radio, same conclusion.

#### 22.4a The cancel race, in the ordering that actually happens

Between the first two bullets there is a genuine race, and the **likely** ordering is not the one
the first cut of this arc handled. That cut assumed: cancel → Command Complete `0x00` → `LE
Connection Complete` `0x00` with a handle. Per Core Vol 4 Part E the commoner sequence is the
reverse. `HCI_LE_Create_Connection_Cancel` answers **`0x0C` Command Disallowed** when the
controller is no longer Initiating — which is exactly its state once the connection *has*
established — and the `LE Connection Complete` carrying the real handle was queued **ahead of** that
Command Complete.

So the wait for the Command Complete reads the meta event **first**. It is not what that wait asked
for, and the loop stepped over it: `here += 1; *seen += 1; continue`. That discarded the only handle
by which the link could ever be released. What followed was worse than a leak, it was a leak with a
clean certificate: the `0x0C` branch set `outstanding = false`, printed *"there was no create to
cancel"*, and the tally computed `(live=false, outstanding=false)` → **`left_outstanding=none`**.
On metal that is an open LE link to a stranger's device for the **rest of the boot** —
`bt_quiesce_events` deactivates the event qTD immediately afterwards, so no `Disconnection Complete`
can ever be read, and the link dies only on the peer's supervision timeout or a power cycle.

The fix is a **latch**, `BtL3State`, carried across every wait of one L3 run:

* any `LE Connection Complete` with status `0x00` that a wait walks past has its handle latched
  (`live_handle`) instead of discarded — and the `take()` on consumption means the several places
  that consult it cannot double-count one connection;
* a walked-past `LE Connection Complete` with a **nonzero** status sets `resolved_nonzero`: no link,
  but the create resolved, which is the *other* thing `0x0C` can mean;
* `blind` is set whenever a wait did **not** read its window to term — a truncated event stepped
  over undecoded, an unreadable endpoint, or the structural `BT_L3_EVT_MAX` = 16 cap reached (the
  second entry to the same hole: pre-scan-disable advertising reports draining during the
  `BT_L3_CONN_MS` wait can exhaust the cap). It is what makes the *absence* of a latch admissible
  as evidence or not.

The latch is consulted at three points: before any cancel is considered (§3b, so a cancel is never
issued against a link that already exists), inside the `0x0C` branch, and once more after the whole
teardown block, since the cancel's own short-event / timeout / unreadable branches would otherwise
leave without looking. Every consultation that finds a handle falls through into the disconnect
path and prints `LATCHED LINK RECOVERED`.

That also lets `0x0C` say something it previously **asserted with no evidence at all**. It now reads
as *never-initiating* only when no connection event of any status was walked past — and an
established connection would have queued one ahead of this very Command Complete, so that absence is
real evidence. When `blind` is set the line says so, in the same breath, as a caveat rather than a
verdict.

#### 22.4b An unreadable endpoint is latched, and no read is retried after it

As in L2, `BtEvt::Stop` forbids further **event reads**, not EP0 writes. Every teardown command is
still *sent* on a path where the endpoint has become unreadable; what the arc will not do is claim a
reply it could not read, and the tally then reports the outcome as unconfirmed.

`Stop` is now **latched in `BtL3State`**, and `bt_l3_await` returns immediately — without arming —
once it is set. That is not tidiness. A later `bt_read_full_event` finds `armed == false` (the halt
cleared it) and re-arms, writing a fresh `QTD_ACTIVE` overlay, which **clears the QH's Halted bit
while the device's STALL condition stands** — the endpoint then looks healthy and is not. The
`SENT UNREAD` witnesses distinguish the two facts they used to conflate: a read that was *attempted*
and found the endpoint dead, versus no read attempted because an earlier section already latched it.

### 22.5 The armed invariant, preserved by construction

`bt_l3_await` **never calls `bt_arm_read`.** Every read goes through `bt_read_full_event`, which
arms only under `if !*armed` and clears `*armed` only where a transfer actually retired — a
completed qTD, or a `QTD_ERR_MASK` halt — and hands it forward on both timeout paths. L3 threads
`bt_probe`'s single `armed` flag through every wait and mints none of its own. A toggle desync here
would silently corrupt every later HCI read on the controller the internal keyboard and trackpad
share, which is why the property is structural rather than reviewed.

### 22.6 The tally, and the must-not-appear condition

One line ends the stage, with its zeros audited (a counter that only appears when nonzero cannot be
read as evidence that nothing happened):

```
:: bt-l3: [N] L3 tally — elapsed=NNNms events_read=N connections_attempted=N connections_completed=N
   disconnections_confirmed=N cancels_issued=N unconsumed_latched_handle=none left_outstanding=none == witness ::
```

`left_outstanding=` reads `none` on every correct path and names what is left on every incorrect
one. **The arc ending with a live connection or an unresolved create is the must-not-appear
condition**, and the tally declares it in those words.

Two things about this line were themselves wrong and are fixed:

* **`elapsed=` no longer fabricates a zero.** It used `epace_ms(..).unwrap_or(0)`, which printed
  `elapsed=0ms` on exactly the run §22.7 claimed could not masquerade — the *uncalibrated* one,
  where `epace_ms` returns `None`. A fabricated zero in milliseconds is indistinguishable from an L3
  that did nothing. It now uses `epace_fmt`, the rest of the file's answer: raw cycles with a `cy`
  unit when the TSC rate is unknown, so the line reads `elapsed=NNNNNNNNcy` and cannot be mistaken
  for a measured millisecond.
* **`unconsumed_latched_handle=` is new, and it closes an invariant that rested on an argument.**
  The claim "no undisconnected handle reaches the tally" held because a single create cannot produce
  two status-`0x00` Connection Completes — an argument, not a construction. If a latched handle is
  still held when the tally runs, it is an **open link this arc did not release**, so it is now
  reported rather than dropped. It reads `none` on every correct path.
* **The `(live, outstanding) = (true, true)` arm had no producer.** Every site that sets `live` also
  resolves `outstanding` in the same breath — a connection event is precisely what takes the
  controller out of the Initiating state — and `outstanding` is never set again after the create.
  A dead arm asserting an unreachable condition is a claim, so `live` is now matched first and
  swallows both: if a link is held, that is the headline whatever `outstanding` says.

### 22.7 What L3 costs

`bt` is off by default; all of this is the cost of a `UNAOS_BT=1` boot, added to L2's ~590 ms empty
/ ~765 ms at cap. Four constants and nothing else: `BT_L3_CMD_MS` = 300 ms (local answers),
`BT_L3_CONN_MS` = 1200 ms (the only wait with air time in it), `BT_L3_DISC_MS` = 600 ms, and
`BT_L3_EVT_MAX` = 16 as a structural per-wait event cap.

| path | added |
|---|---|
| no peer passed the filters (speaker off, out of range, or nothing connectable heard) | **0 ms** |
| create refused | ≤ 300 ms |
| connect + clean release, typical | ~100-400 ms |
| create timed out, cancelled | ≤ 2100 ms |
| **cancel loses the race, then disconnects — worst case** | **≤ 3000 ms** of event waits |

**The ≤ 3000 ms was not a bound when it was first written, and now is.** `bt_read_full_event` gave
every **continuation** packet the whole of `hw_wait_budget()` (~1.1 s calibrated on the bench part,
the fixed 2.5e9-cycle guess uncalibrated), and only the *first* packet of an event was bounded by
the caller's window. An `LE Advertising Report` is three or more packets on a 16 B endpoint, and L3
makes up to six waits, each able to begin reassembling one — so a wait that had "bought" 300 ms
could stall for 300 ms **plus one full budget per continuation packet**. The real worst case was
~3.0 s **+ up to ~12 s**.

**A DURATION IS NOT A DEADLINE — and the first attempt at this fix got that wrong too.** It bounded
the continuation *phase* rather than each packet, but started that phase's clock **after the first
packet landed**. A first packet arriving at the window edge therefore handed the continuation phase a
*fresh full window*, and one wait could still take `first_budget + cont_budget` ≈ **2×** the caller's
window: ~6000 ms, not 3000. The re-verify caught it.

`bt_read_full_event` now takes **three** budgets, and it takes three because two could not express
the thing that matters:

| budget | bounds | measured from |
|---|---|---|
| `first_budget` | the first packet | the wait for that packet |
| `cont_budget` | each continuation packet, individually | that packet's own wait |
| **`call_budget`** | **the whole call** | **function ENTRY, before anything is armed** |

Each continuation waits `min(cont_budget, call_budget − elapsed_since_entry)`. `bt_l3_await` passes
its remaining window for all three, so one `bt_read_full_event` cannot outlast that window however
many packets the event takes and however late in the window its first packet lands. *That* is what
makes the bound a caller states the bound it gets.

The reason continuations were unbounded in the first place still holds and is preserved: abandoning
an event half-read desynchronises the toggle, so a continuation expiry returns `Stop` (the endpoint
is finished with) while a first-packet expiry returns `Idle` (nothing was lost). `bt_hci_command`
and L2's drain pass `call_budget = u64::MAX`, which makes `min(cont_budget, MAX − elapsed)` exactly
`cont_budget` — **byte-for-byte their pre-existing behaviour**. Both are deliberately left alone:
the command path is bounded structurally by `BT_EVT_MAX` instead, and for the drain, a report
arriving in the last milliseconds of the window is worth finishing and there is no teardown behind
it waiting on the clock.

With `call_budget` in place each of the six waits is bounded by its own constant, so the 3000 ms is
now an arithmetic sum and not a description: 300 (create Command Status) + 1200 (Connection
Complete) + 300 (cancel Command Complete) + 300 (post-cancel meta) + 300 (disconnect Command Status)
+ 600 (Disconnection Complete). On top of it sit **up to three EP0 control-OUTs** (create, cancel,
disconnect), each with its own transfer budget; those are writes, not waits, and are not included in
the figure.

With `tsc_hz() == 0` every L3 window collapses to `hw_wait_budget()/4`, so the worst case *falls* —
and the tally now prints `elapsed=NNNNcy` rather than a fabricated `0ms` on that run (§22.6), which
is what actually makes an uncalibrated run unable to masquerade as a calibrated one.

### 22.8 What metal must verify

`~/unaos-bench/scratch/gr23/btl3-predictions.md` carries the falsifiable statement, including the
refutation that outranks every other line: **HID dead after `bt-l3` = the endpoint invariant broke.**

---

## 23. BTNAME — the speaker was heard, and its name was one character (`UNAOS_BT=1`, 2026-08-09)

Boot AR was flown with Peter's speaker deliberately switched ON. BT-L3 reported
`peer NOT SELECTED — name=MEGABOOM considered=0 matched=0`, and the brief for the next arc was
written on the hypothesis that an Ultimate Ears MEGABOOM is a **Bluetooth Classic** device that an
LE scan can never see, so the next rung had to be BT-L4 (HCI Inquiry).

**That hypothesis is refuted by our own capture, and BT-L4 was not built.** This section records
what the evidence actually says, the instrument the arc added instead, and the one road that does
still need Classic.

### 23.1 The device, from the host's own stack (ESTABLISHED)

With the speaker connected to the development host, `bluetoothctl` reports:

```
Device 88:C6:26:CC:2D:3C (public)
    Name: MEGABOOM        Class: 0x00240418      Icon: audio-headphones
    UUIDs: Audio Sink (A2DP), A/V Remote Control (AVRCP), Advanced Audio Distribution,
           Handsfree, Headset, Headset HS, Serial Port, PnP Information, 0000_61fe (UE proprietary)
```

Class of Device `0x00240418` decodes, per Assigned Numbers §2.8 (bits 1:0 format, 7:2 minor,
12:8 major, 23:13 service):

| field | value | meaning |
|---|---|---|
| Major Device Class | `(0x0418 >> 8) & 0x1F` = `0x04` | Audio/Video |
| Minor Device Class | `(0x18 >> 2) & 0x3F` = `0x06` | Headphones |
| Service bits | 18, 21 | Rendering, Audio |

And Boot AR's LE sighting was:

```
:: bt-l2: [1] dev 01 addr=88:c6:26:cc:2d:3c type=public evt=ADV_IND rssi=-97dBm reports=1 name="." ::
```

**The same address, byte for byte.** So the speaker is dual-mode and uses its public identity
address on both transports: it emits connectable LE advertisements (`ADV_IND`) *and* carries its
audio over Classic profiles. A2DP has no LE transport in the core spec, so the audio side is
necessarily BR/EDR; the LE side is almost certainly the companion-app channel behind that
proprietary `0x61FE` service.

### 23.2 What the capture proves, and what it does not

**ESTABLISHED**

- The receive path works. A real `ADV_IND` was reassembled off a 16-byte interrupt endpoint,
  decoded, and its six address bytes match the host's ground truth exactly. A payload that passes
  the controller's CRC and yields the right address is an intact report, not noise.
- Therefore a rollup of `distinct_devices=0` (boots 2-4 of `gr23-bootAR`) is a statement about
  **the room**, not about our stack: the mask writes are witnessed at status 0x00, and the same
  build heard a device on boot 1.
- The device is reachable by LE scan. It does not need Inquiry to be *found*.
- `-97 dBm` is essentially the noise floor, from a device in the same room. Discovery on a bounded
  500 ms window is therefore expected to be intermittent regardless of anything in this section.

**NOT ESTABLISHED (and we cannot test the speaker's firmware)**

- Whether the LE advertisement ever carries the friendly name "MEGABOOM", in `ADV_IND` or only in
  a `SCAN_RSP`. Our scan is PASSIVE, so a name that lives in the scan response is heard only if
  somebody else solicits it.
- Whether LE advertising is continuous or gated on pairing mode.

### 23.3 The `"."`, and why the parser is not the defect

`name="."` means the decode produced **exactly one byte, and that byte was not printable ASCII** —
the witness renders every unprintable byte as `.`. The leading suspicion was the AD walk, because
an AD `Length` octet **counts the type octet but not itself**, and the classic off-by-one on that
produces a one-character result. Every candidate was tested against the code:

| candidate | verdict |
|---|---|
| off-by-one on `Length` | **REFUTED.** `src = &data[off+2 .. off+1+l]` is `l-1` bytes — correct. Introducing the off-by-one yields `".MEGABOOM"` (a *leading* dot, name intact), not a bare `"."` |
| type byte read as the first name byte | **REFUTED.** `src` starts at `off+2`, past the type octet |
| truncation (`~(cut)`) firing wrongly | **REFUTED.** `ncut` needs `src.len() > 24`, and the witness carried no `~(cut)` |
| walk terminating early on a zero-length structure | **REFUTED as a cause of `"."`** — that path yields `nlen == 0`, which prints `name=(none)` |
| transport/reassembly misalignment | **REFUTED.** `bt_arm_read` arms exactly `mps` (one qTD = one packet); `bt_wait_read` derives length from the qTD residual; reassembly runs to `2 + Parameter_Total_Length`. And a misaligned payload could not have produced a byte-exact address |
| the payload genuinely carried a one-byte name | **NOT REFUTED — the surviving explanation** |

The walk was also unchanged between the L2-only build that flew boot 1 and today's (`0c1d121f`
vs HEAD, modulo the empty-name guard), so nothing regressed under it.

**Conclusion: the name filter did its job. The decode is spec-correct. What the capture could not
do was let anyone *check* that** — because a one-character real name and a seven-byte misparse
render identically. That, not the parser, is the defect this arc fixes.

### 23.4 What the arc built

**1. The walk moved to `drivers/ehci/bt_name.rs`** — the AD walk, the name cap, and the two match
rules (`bt_name_contains_ci`, `bt_name_maybe_ci`). Nothing in that file performs I/O or touches a
register, which is what makes the next two items possible.

**2. `bt_name_fixture()` — an in-kernel fixture, run before the radio is asked anything.** Eight
payloads whose decode is known in advance, driven through the *same* `bt_decode_local_name` the
drain runs, one witness line each plus a tally. It is unconditional and sits ahead of every L2
gate: a boot where the scan never starts is exactly the boot where the decode still needs to
answer for itself. Legs: a realistic `ADV_IND` carrying a Complete Local Name of `MEGABOOM` behind
two other AD structures; the observed one-byte-name payload; shortened-only; shortened-then-
complete; the empty-complete-does-not-erase-shortened regression; no-name-at-all; a `Length` that
runs past the payload; and an over-long name that must be marked cut.

**3. `tools/btname_harness.rs` — the same source, runnable on the host.** It `include!`s
`bt_name.rs` rather than copying it, so it cannot pass on a transcription the kernel does not run:

```
rustc -O tools/btname_harness.rs -o ~/unaos-bench/scratch/gr23/btname && ~/unaos-bench/scratch/gr23/btname    # pass=15 fail=0
```

Falsifiability was demonstrated, not asserted: breaking the length handling to
`&data[off+1 .. off+1+l]` turns 6 of the 15 legs red (exit 1), including
`megaboom-complete want name="MEGABOOM" | got name=".MEGABOOM"`. Restored, green again.

**4. The RAW payload witness.** Any device whose decoded name is shorter than three characters —
**including `(none)`** — now prints the bytes the walk actually saw:

```
:: bt-l2: [1] dev 01 RAW ad — decoded name is 1 char(s), too short to trust: Data_Length=NN
   bytes=[02 01 06 ...] — this is the payload the name walk actually saw; ... == witness ::
```

The raw payload is stored with the report that supplied the stored name (or the latest nameless
one), so `raw` and `name` always describe the same payload rather than two different reports.

**No HCI command, no radio state, and no transfer was added.** `bt_name_fixture` takes `&self`,
arms no qTD and touches no endpoint, so the endpoint-`armed` invariant is untouched by
construction — there is no new `bt_arm_read` call site anywhere in the diff.

### 23.5 The RSSI floor does not reject this device

Confirmed unchanged: the floor (`BT_L3_RSSI_FLOOR = -60`) is applied **only** in the match arm
where no name filter is armed. With `BT_L3_PEER_NAME = Some("MEGABOOM")` the floor is skipped
entirely, so the speaker's -97 dBm cannot disqualify it. This is deliberate — a named peer across
the room is still the right peer — and it matters more now that we know how weak the signal is.

### 23.6 The Classic road, named and not started

LE discovery reaches this device, but an LE connection reaches its **control** service, not its
audio. Playing sound through it requires the Classic stack:

- `HCI_Write_Inquiry_Mode(0x02)` + `HCI_Inquiry` (GIAC `0x9E8B33`) to discover, draining
  `Inquiry Result` (0x02) / `with RSSI` (0x22) / `Extended Inquiry Result` (0x2F), with
  `HCI_Inquiry_Cancel` (0x0402) on every exit path. **BUILT — see §26**, which is where this
  sketch got cashed and where it got corrected: the inquiry is what supplies the page's
  `Page_Scan_Repetition_Mode` and `Clock_Offset`, and `HCI_Write_Inquiry_Mode` turned out **not**
  to be needed (§26.3).
- **The event mask must be widened again**: Inquiry Complete (bit 0) and Inquiry Result (bit 1)
  are inside L1's reset default, and Inquiry Result with RSSI (bit 33) is too — but **Extended
  Inquiry Result is bit 46, outside the `0x00001FFFFFFFFFFF` default**, so EIR (where a friendly
  name arrives without a separate `HCI_Remote_Name_Request`) needs the mask extended exactly as L2
  extended it for LE Meta at bit 61.
- Then the actual work: ACL link, L2CAP, SDP, AVDTP, and an SBC encoder for A2DP. (The ACL link is
  BT-C1; L2CAP and AVDTP signalling are BT-C2, §24. SDP is skipped, with reasons, in §24.2.)

That last line is the whole point. Discovery is a rung; **A2DP is a stack**, and it is a much
larger road than the L0-L3 ladder that precedes it. It is named here so the size of it is a
decision rather than a surprise.

Note also that inquiry **transmits** — it is not the passive listen L2 does. That is the normal way
any Bluetooth host discovers an audio device, but it is a disclosure Peter gets to make, not one to
smuggle inside a rung.

### 23.7 What metal must verify

`~/unaos-bench/scratch/gr23/btname-predictions.md` carries the ranked, falsifiable outcomes. The
decisive one: with the speaker on and in range, the fixture prints 8/8 and the RAW line names the
real cause — a payload with no name structure convicts the walk (and becomes a ninth leg), a
payload carrying a genuine one-byte name exonerates it and makes passive name-matching structurally
unable to find this speaker.

The refutation that outranks the rest is unchanged from §22: **HID dead after `bt-l2` = the
endpoint invariant broke** — though this arc adds no `bt_arm_read` call site to break it.

---

## 24. BT-C2 — the signalling road: an L2CAP channel and one AVDTP DISCOVER (`UNAOS_BTC=1`, 2026-08-11)

### 24.1 Where C2 sits

BT-C1 proved the transport. On Boot A (`~/unaos-bench/capture/gr25-bootA/ttyUSB0.log`) the second
page train reached the speaker:

```
:: bt-c1: [1] Connection Complete (0x03) — status=0x00 handle=0x000b peer=88:c6:26:cc:2d:3c
             link_type=0x01(ACL) encryption=0x00 -> BR/EDR LINK ESTABLISHED
:: bt-c1: [1] page summary — attempts_run=2/2 pages_on_air=2 page_timeouts=1 -> REACHED
```

An ACL link is a pipe, not a service; nothing above it existed. BT-C2 builds the first thing that
does, and it runs in exactly the position BT-L4 occupies on the LE side — inside `bt_c1_page`,
after every path that can establish or recover a handle, and **before** the mandatory
`HCI_Disconnect`. It returns unit, so it is structurally incapable of skipping the teardown.

The stage claim, stated so it can be falsified: *an L2CAP channel to PSM 0x0019 opened and was
configured in both directions, and here is the list of stream endpoints the speaker published on
it.* No codec negotiation, no `SET_CONFIGURATION`, no `OPEN`, no `START`, no encoder, no media.

### 24.2 SDP is skipped, and why that is a judgement rather than an omission

The conventional order is SDP first: open a channel to PSM 0x0001, send an
`SDP_ServiceSearchAttributeRequest` for `AudioSink` (UUID 0x110B), and read the PSM out of the
returned protocol descriptor list. That is a continuation-state-driven parser over a variably-typed
data-element tree — a larger PDU surface than everything else in this stage combined.

**The AVDTP PSM is not discovered, it is assigned**: 0x0019, fixed by Assigned Numbers. So the
`CONNECTION_RSP` on PSM 0x0019 answers the same question SDP would, more directly: a device with no
AVDTP answers `result=0x0002 (PSM not supported)`, which is a complete negative. And the
`AVDTP_DISCOVER` response that follows lists the endpoints that *actually exist*, which is strictly
more than an SDP record asserts. SDP is not ruled out — a real stack wants it for AVRCP and for the
sink's supported-features bitmask — it is ruled out *here*.

(The brief that commissioned this stage named PSM **0x0017** for AVDTP signalling. 0x0017 is AVCTP,
the AV/C remote-control transport; AVDTP is **0x0019**. The code carries 0x0019.)

### 24.3 The PDU plan, and the witness for each step

Every PDU is printed in both directions. Outbound lines carry `-> OUT<ep>`, inbound lines `<-`.

| # | PDU | Direction | Witness line |
|---|-----|-----------|--------------|
| 1 | `L2CAP_CONNECTION_REQ` (0x02), PSM 0x0019, SCID 0x0040 | out | `-> OUT2 CONNECTION_REQUEST psm=0x0019(AVDTP) scid=0x0040 …` |
| 2 | `L2CAP_CONNECTION_RSP` (0x03) | in | `<- CONNECTION_RESPONSE ident= dest_cid= src_cid= result= status= -> …` |
| 3 | `L2CAP_CONFIGURATION_REQ` (0x04), MTU option = 48 | out | `CONFIGURATION_REQUEST sent — … option=MTU(0x01) len=2 value=48 …` |
| 4 | `L2CAP_CONFIGURATION_RSP` (0x05) | in | `<- CONFIGURATION_RESPONSE … result= -> SUCCESS/…` |
| 5 | peer's `L2CAP_CONFIGURATION_REQ` | in | `<- CONFIGURATION_REQUEST ident= dest_cid= flags= option_bytes=` + one line per option |
| 6 | `L2CAP_CONFIGURATION_RSP` | out | `-> OUT2 CONFIGURATION_RESPONSE result=0x0000 …` |
| — | channel state | — | `L2CAP channel state — scid= dcid= host->peer_configured= peer->host_configured= -> OPEN / NOT OPEN` |
| 7 | `AVDTP_DISCOVER` (signal 0x01) | out | `AVDTP_DISCOVER sent on cid= — header=[00 01] label=0 packet_type=0b00(SINGLE) …` |
| 8 | `AVDTP_DISCOVER` response | in | `<- AVDTP_DISCOVER RESPONSE ACCEPT label= — N Stream End Point(s)` + `<- SEP i/N — seid= in_use= media_type= tsep=` |
| 9 | `L2CAP_DISCONNECTION_REQ` (0x06) | out | `-> OUT2 DISCONNECTION_REQUEST` |
| 10 | `L2CAP_DISCONNECTION_RSP` (0x07) | in | `<- DISCONNECTION_RESPONSE … -> THE CHANNEL IS CLOSED BY AGREEMENT` |

Steps 3–6 are two **independent** handshakes. A BR/EDR channel enters the OPEN state only when the
local device has accepted the peer's configuration *and* the peer has accepted the local device's
(Core Vol 3 Part A §6.1.3). Driving only the first half is the classic way to build a channel that
exists in the log and never carries a byte, so the `L2CAP channel state` line prints every flag and
the AVDTP signal is not sent unless all of them agree.

The open condition is `our_cfg_done && peer_cfg_done && !peer_cfg_refused && !peer_closed`, and the
third term is not decoration. Configuration is a *sequence*: a peer may send a first request this
host accepts and a second asking for a retransmission mode it refuses, leaving `peer_cfg_done` true
from the first and `peer_cfg_refused` true from the second. Without that term the stage would print
"OPEN. THE SIGNALLING ROAD TO A2DP EXISTS" and send AVDTP down a channel it had just told the peer
it could not configure. Both configuration paths also check the **CID**: a request or response
naming a channel this host does not own is answered (so the peer's RTX timer does not stall) but
cannot complete this channel's half of the handshake.

`bt_c2_await` services peer-initiated signalling inline rather than discarding it — a
`CONFIGURATION_REQUEST` gets a real decision, an `INFORMATION_REQUEST` gets an honest
`result=0x0001 (not supported)`, an `ECHO_REQUEST` gets an echo, a `DISCONNECTION_REQUEST` gets a
response, and any other **request** gets `COMMAND_REJECT`. An unanswered request keeps the peer's
RTX timer running and it will retransmit; that is how a signalling channel deadlocks against a
silent host.

Requests and responses are told apart by **parity**, not by an enumeration: Vol 3 Part A §4
allocates signalling codes in request/response pairs with the request even and the response odd
(0x02/0x03, 0x04/0x05, … through 0x18/0x19). Any odd code is a response and is stepped over,
never rejected — a `COMMAND_REJECT` answers a request, never a response. Listing only the response
codes the stage happens to implement would have sent rejects for 0x0D, 0x0F, 0x11, 0x13, 0x17 and
0x19.

Matching is by **CID and command code, never by Identifier**. This stage has one request
outstanding at a time, so a code on the right channel is unambiguous, whereas matching on the
Identifier this host chose would make a peer that echoes the wrong one — which several embedded
stacks do — look like silence. The Identifier is printed on every decoded PDU regardless, so a
capture can still check the echo.

**MTU = 48** is chosen because it is the BR/EDR mandatory minimum (§5.1), so no conforming peer may
refuse it — and because a full-size SDU (48 + 4 L2CAP + 4 ACL = 56 B) still arrives in one 64-byte
bulk-IN. A streaming arc renegotiates upward on its own channel.

**What is refused.** The peer's configuration is accepted option by option. Everything that
constrains the *peer* — MTU, flush timeout, QoS, FCS, extended window — is accepted as proposed,
because a Basic-mode responder honours it by doing nothing. The exception is `RETRANSMISSION AND
FLOW CONTROL` (type 0x04) with a mode byte other than 0x00: that asks for Enhanced Retransmission,
Streaming or Flow Control mode, none of which this stage implements, so it is answered
**`result=0x0002` (Rejected — no reason given)** and the stage stops before AVDTP rather than
promising framing it cannot produce. Options carrying the HINT bit (0x80) are ignored wholesale,
which is what the hint bit means.

`0x0002` and not `0x0003`: `0x0003` is *failure — unknown options*, a claim this host cannot make,
since it recognised the option perfectly well and declined the mode inside it. A peer reading
`0x0003` would retry without the option instead of giving up.

### 24.4 The data-toggle bug this arc had to fix first

A bulk pipe's USB data toggle belongs to the **endpoint** and persists for the life of the pipe: it
is reset by `SET_CONFIGURATION`, `CLEAR_FEATURE(ENDPOINT_HALT)` or a port reset (USB 2.0 §5.8.5,
§9.4.5), and by nothing happening at the Bluetooth layer.

BT-L4 runs one ACL exchange on the **LE** link and leaves both toggles at DATA1. BT-C2 then runs on
**the same two endpoints**, a whole classic page later. A BT-C2 that started from DATA0 — which
every reading of "a fresh link" suggests — would have its first OUT silently discarded by the radio
as a retransmission and its first IN mis-sequenced, and the capture would have shown a speaker that
accepted a channel request and never answered.

The toggles are therefore carried on the controller (`bt_acl_tog`), written by every ACL
transaction that **retires** (a transaction that times out moves no data and advances nothing), and
witnessed by the C2 transport line, which says where they came from:

```
:: bt-c2: [1] transport — BR/EDR handle=0x000b ACL pair addr=7 bulk_out=OUT2/64B bulk_in=IN2/64B
              acl_buffers=6 start_toggle=(out DATA1, in DATA1) — CARRIED OVER FROM BT-L4's LE
              exchange … stage_cap=6000ms per_pdu_window=1500ms packet_cap=48 tx_cap=12
```

`bt_acl_tog` and `bt_acl_bufs` are gated on `btc`, not on `bt`, because BT-C2 is their only reader —
and `bt_acl_tog_set` exists so the *write* disappears in a `bt`-only build too.

One recorded negative, because it is a fact about the tooling this codebase leans on: **rustc's
`dead_code` lint does NOT flag a field that is only ever assigned.** A controlled two-file
experiment confirms it — a field set only in a struct literal warns `field is never read`, and the
same field assigned once through `self.f = v` does not. So a `bt`-only build with these fields
gated on `bt` produced *no* warning either before or after the gating change. The gating is right
on its merits; the compiler was never going to catch it being wrong. Write-only state is invisible
to the lint, which is the same blind spot as an instrument that cannot fail.

### 24.5 Budgets, and what the stage can cost a boot

| Bound | Value | What it bounds |
|---|---|---|
| `BT_C2_SIG_MS` | 1500 ms | one signalling response. L2CAP's RTX minimum is 1 s (§6.2.1), so a shorter window would give up inside a conforming peer's permitted response time |
| `BT_C2_STAGE_MS` | 6000 ms | the **whole stage**, from its first line. Every wait takes `min(SIG_MS, remaining)`, so no sequence of slow answers outruns it |
| `BT_C2_PKT_MAX` | 48 | ACL packets read across the stage — the second bound, so a chatty peer cannot spin a loop whose per-read deadline keeps being met |
| `BT_C2_TX_MAX` | 12 | ACL packets sent in **total**. A correct exchange sends 5 |
| `BT_C2_INFLIGHT_MAX` | 2 | ACL packets **unacknowledged at once** — the depth bound, see below |
| `BT_C2_OPT_PRINT_MAX` | 8 | config options *printed* per request (all are decided on) |
| `BT_C2_SEP_PRINT_MAX` | 16 | stream endpoints *printed* per response (all are counted) |
| `BT_L4_TXN_MS` | (shared) | one bulk-OUT token retiring |

**The two print caps bound the one cost in this stage that no deadline observes.**
`serial_println!` is synchronous: at 115200 baud a ~200-byte witness line is ~17 ms of wall clock
that `BT_C2_STAGE_MS` never sees. A maximum-length `CONFIGURATION_REQUEST` carries ~120 options and
a peer ignoring the negotiated MTU could declare far more SEPs, so an uncapped transcript turns a
6-second stage into minutes of printing — a denial of service against the boot, driven entirely by
the peer. Every option is still *decided on* and every SEP still *counted*; only the transcript is
truncated, and each truncation says how much it dropped. A refusable option is printed whatever the
cap, so a refusal is never left without its evidence. `bt_c2_answer_config` also re-checks the
stage cap **before printing**, not only before waiting.

**Depth versus total.** The Core spec caps the host at `HC_Total_Num_ACL_Data_Packets`
unacknowledged packets (Vol 4 Part E §4.1.1), tracked by `Number Of Completed Packets` — an event
this stage never reads, because it lands on the HCI event endpoint BT-C1 is draining. So there is
no accounting, and the substitute is a hard depth limit no legal buffer count can be below: 2, with
the transport gate independently refusing a controller reporting fewer than 2 buffers. A packet
arriving from the peer clears the counter; that is a completed round trip, not a completion event,
and the tally does not pretend otherwise. Past the limit the stage declines to send and says so.

Nothing is left alive, and the reason is structural rather than careful: an L2CAP channel lives
inside an ACL link, and `bt_c1_page` releases that link unconditionally on the next line. What C2
*can* leave behind is an **unconfirmed close**, and `left_outstanding=` is computed from state to
say so — not a literal in the format string. That distinction matters because the must-not-appear
grep grammar is `awk '/left_outstanding=/ && !/=none/'`, and a field that can only ever print
`none` is an instrument that cannot fail.

**The C2 → C1 coupling is witnessed, not assumed.** Every packet C2 sends queues a
`Number Of Completed Packets` event that nobody reads, ahead of BT-C1's `HCI_Disconnect` looking
for its `Disconnection Complete` through `bt_l3_await` — which walks past a *capped* number of
unwanted events per wait. A queue C2 filled can therefore push that confirmation past the cap and
make BT-C1 print its must-not-appear `A LIVE BR/EDR LINK` for a reason about queue depth rather
than about the link. The `C2->C1 coupling` line prints the count so a capture can test that reading
first and blame the link only after excluding it.

The transport gate is BT-L4's plus one term — `acl_buffers >= 2`, read by BT-L1 from
`HCI_Read_Buffer_Size` and latched on the controller. C2 has one moment (answering the peer's
configuration request while its own is still in flight) at which two host-to-controller packets may
be unacknowledged, and a one-buffer controller would have the second silently dropped. Boot A
reports `acl_num=6`.

Reassembly exists here and did not in BT-L4: L4's exchange fit one 64-byte max packet by
arithmetic, but the **signalling** channel's MTU is the peer's business and a
`CONFIGURATION_REQUEST` may exceed one USB transaction. The ACL header's `Data_Total_Length` is the
authority, and it is read before anything else is believed.

### 24.6 How the stage degrades

Every refusal keeps BT-C1's verdicts unchanged and its `HCI_Disconnect` unconditional.

| Observation | Reading |
|---|---|
| no `bt-c2:` lines at all | the page never reached a link; C1's `page summary` says why |
| `L2CAP NOT ATTEMPTED` | the transport gate refused — chain mode, no ACL bulk pair, or `acl_buffers < 2`. The line names which |
| `CONNECTION_RESPONSE … result=0x0002` | the device publishes no AVDTP service. A complete answer about the peer |
| `CONNECTION_RESPONSE … result=0x0003` | **security block.** C1 reported `encryption=0x00` and this arc pairs with nothing, so this is the expected refusal from a speaker that insists on bonding — and it names Secure Simple Pairing as the next arc's prerequisite rather than leaving it to be guessed |
| `CONNECTION_RESPONSE … result=0x0004` | no resources: commonly already streaming from another source |
| `L2CAP channel state … NOT OPEN` | a configuration direction is missing or was refused; the four flags say which |
| `STAGE CAP SPENT … NO WAIT WAS MADE` | the 6-second stage budget ran out before this PDU was awaited. **Nothing after it is evidence about the peer** — it was never listened for. Without this line the caller's "NO … within the window" would blame the peer for silence during a window that never opened |
| `NOT SENT — N packet(s) are already unacknowledged` | the depth limit held. This host will not transmit past a bound it cannot verify |
| `NO AVDTP response within the window` | the channel is open and configured, so the transport is proven and the peer's answer is what is missing. Two different findings, and the line separates them |
| `AVDTP_DISCOVER RESPONSE REJECT` / `GENERAL REJECT` | the peer's AVDTP answered. Transport proven end to end; the signal is what it refused |

### 24.7 What metal must verify (Boot B)

QEMU has no Bluetooth radio, so this stage is **compile-only** off the bench: `./arroyo check` and
`UNAOS_WC=1 ./arroyo check` type-check it, and `strings` proves the witness text is present with
`UNAOS_BTC=1` and absent without it. Nothing else about it can be exercised without the speaker.

Ranked, falsifiable:

1. **The decisive line** — `<- CONNECTION_RESPONSE … result=0x0000` followed by
   `L2CAP channel state … -> OPEN`. That is the arc's whole claim.
2. **`start_toggle=(out DATA1, in DATA1)` on the transport line.** If the run reached BT-L4 and this
   prints DATA0, the toggle carry-over is broken and every conclusion below it is void.
3. **`AVDTP_DISCOVER RESPONSE ACCEPT` with at least one `media_type=0x00 tsep=SNK` SEP.** The
   endpoint the next arc configures. `in_use=true` on all of them means the speaker is streaming
   from another source.
4. **`result=0x0003 (SECURITY BLOCK)`** is a *successful* run of this stage in the sense that
   matters: it proves the L2CAP request reached the peer's service layer and returns a specific,
   actionable reason. It converts the next arc from "codec negotiation" into "pairing first".
5. **`C2 tally … left_outstanding=none`**, and BT-C1's own `C1 tally … left_outstanding=none`
   unchanged behind it. A C2 that broke C1's teardown is the must-not-appear condition — and if C1
   *does* report a live link, read the `C2->C1 coupling` line above it before concluding anything:
   the unread completion events C2 queued are the competing explanation.
6. **HID alive after `bt-c2`.** This stage adds no `bt_arm_read` call site, but it is the first code
   to drive the ACL bulk pipes repeatedly, and the shared EP0 data buffer is what it builds packets
   in. A dead trackpad after these lines indicts the buffer sharing.

### 24.8 The arc after this one

`AVDTP_GET_CAPABILITIES` / `GET_ALL_CAPABILITIES` (signals 0x02 / 0x0C) on a discovered sink SEID,
to read its Media Codec capability — for A2DP that is the SBC capability block (sampling
frequencies, channel modes, block length, subbands, allocation method, bitpool range). Then
`SET_CONFIGURATION` (0x03), `OPEN` (0x06), a **second** L2CAP channel on PSM 0x0019 for the media
transport, `START` (0x07), and an SBC encoder feeding RTP-framed media packets.

Two things that arc must budget for that this one did not: **pairing**, if the `CONNECTION_RSP`
comes back `0x0003`; and **flow control**, because a media stream is the first thing this driver
will send that can exceed `HC_Total_Num_ACL_Data_Packets` and therefore the first that needs the
`Number Of Completed Packets` event read rather than walked past.

---

## 25. BT-RETRY — the bring-up chain is re-triggerable, not one-shot at boot (`UNAOS_BT`/`UNAOS_BTC`, 2026-08-11)

**The architecture gap, from Boot B metal.** Every stage above (BT-L0 through BT-C2) ran exactly
once, inline on the synchronous EHCI enumeration walk, at 2–13 s of boot. That is the *only* time
the chain ran. On Boot B Peter rebooted the MEGABOOM *after* our boot had completed: our single
LE-scan + classic-page chain had already fired and finished (`bt-l2` selected the speaker by address
at 2308 ms, `bt-c1` paged twice and `PAGE TIMEOUT`'d both, then nothing), so a speaker that becomes
page-scannable a minute later is never paged again — and it pairs to a phone instead. **You cannot
ask a user to reboot the machine to pair a speaker.** This section makes the chain re-runnable on
demand, post-boot. It is the *first half* of the BT-C2 review's "make it async before default-on":
the chain is now re-invocable; moving it *off* the synchronous service pass is the second half.

**The factoring.** `bt_probe` was one monolith: selection/census (no wire traffic) followed by the
wire chain (`HCI_Reset` → version → L1 → LE scan → L3 connect → C1 page → C2 → quiesce). The wire
chain is now `bt_bringup_wire(t, intf, e)`, with two call sites:
- **boot path** — `bt_probe` runs selection once, records the claimed radio in `Controller::bt_radio`
  (`Target` + interface + event-EP mps — all that a re-run needs; the radio stays configured and the
  event QH stays linked for the life of the boot), then calls the wire chain;
- **post-boot** — `bt_retrigger` reconstructs the event endpoint (`bt_evt_ep_current`, which rebuilds
  the `BtEvtEp` view of the already-linked slot **without re-arming it** — the QH is spliced into the
  periodic list exactly once, `bt_evt_armed`) and calls the *same* wire chain.

**1. The trigger: a keyboard chord, `Ctrl+Alt+B`.** Detected edge-triggered in
`decode_boot_keyboard` (keycode `0x05` present now, absent last report, with Ctrl and Alt held). It
is the one input affordance a spatial OS with no menu bar has today; `Ctrl+Alt+b` resolves to no
printable character, so the chord types nothing. It only *records* the request in a module atomic
(`BT_RETRIGGER_PENDING`); the chain runs in the **drain** at the end of `service_ehci_hid`, under the
`EHCI_HID` lock the decode path already holds — decoupled so the source (a keyboard, possibly on a
different EHCI function) and the radio need not be the same controller. **Idempotent:** the store
does not stack (newest source wins, one chain per drain), and while a chain runs the synchronous
service pass is blocked, so no second press is even decoded. The two alternatives — a periodic
re-scan timer (bounded, "open the pairing menu" cadence) and a dock/shell affordance — are deferred:
the first needs a timer hook and a back-off policy, the second needs a dock that does not exist yet.

**2. State machine — safe to re-enter.** Two declines, each witnessed and each falsifiable:
- **no radio** claimed on any controller → the drain reports it and runs nothing;
- **mid-flight** (`bt_chain_busy`, set at `bt_bringup_wire` entry, cleared on every exit) → decline,
  do not start a second chain. This is the guard the async half will lean on; under today's
  synchronous model it cannot be observed true (the chain runs to completion in one pass), and the
  code says so.

**The stale-link ESCAPE HATCH — not a decline, and this matters.** `bt_left_link = Some` marks a
classic link a prior run left up (the MUST-NOT-APPEAR `left_outstanding` case: `bt_c1_page` finished
with `live` still true). It is written only at `bt_c1_page`'s end and would be cleared by nothing but
a reboot — so treating it as a permanent "already connected, do not re-page" is *exactly* the failure
this arc exists to abolish: a speaker powered off **ungracefully** leaves the `HCI_Disconnect`
unconfirmed, `live` stays true, the latch wedges `Some`, and when the user powers the speaker back on
and presses the chord they would get "already connected" forever — "reboot the machine to pair a
speaker." Instead, on `Some` the re-trigger runs a **reboot-free escape**: resync the event toggle,
attempt a best-effort `HCI_Disconnect` on the stale handle, then clear the latch **unconditionally**
and fall through to re-page. Whether the link was still live (torn down now — no double-connect) or
already dead (the disconnect fails and re-paging is exactly right), the outcome is: latch cleared,
chain runs. Witnessed on the `STALE LINK … reboot-FREE escape` line, so a capture proves the recovery.

**Device-toggle resync — the boot path's free lunch a re-trigger has to pay for.** Each run resets
its **host-side** software toggles: the event endpoint's per-run `toggle` starts DATA0, and C2 reads
`bt_acl_tog`. On the boot path the `SET_CONFIGURATION` right before the chain also resets the
**device-side** endpoint toggles to DATA0 (USB 2.0 §9.4.5), so both sides agree. A re-trigger issues
no `SET_CONFIGURATION`, so the device's *sticky* toggles sit wherever the boot run left them while the
host restarts at DATA0 — and on an odd parity the first post-retrigger IN is dropped by the device as
a duplicate, the bounded wait expires, and the chain reads as a **dead radio, intermittently**
(parity-dependent, the worst kind to diagnose). So `bt_retrigger` issues `ClearFeature(ENDPOINT_HALT)`
on the event IN endpoint and on the ACL bulk IN/OUT pair — each resets the device toggle to DATA0 —
and resets `bt_acl_tog` to `(DATA0, DATA0)` to match. Both sides now provably start DATA0, with no
dependence on unspecified device reset behaviour. Witnessed on the `event-EP toggle resync` and `ACL
toggle resync` lines. With the toggles resynced, the `armed` state re-minted, and the C1/C2 teardown
starting clean, a re-run leaks no HCI state and double-arms no scan.

**3. Honest bounding.** A re-trigger is **one more bounded chain**, not a background storm: the LE
scan window, plus (under `btc`) up to `BT_C1_PAGE_ATTEMPTS` page trains of `page_timeout` each — the
same shape the boot run's merged 4-window / 2-attempt bound already pays. Worst case added per
trigger in a quiet room is that one chain's wall-clock (dominated by the two ~5.12 s page trains —
≈12.4 s classic when this was written, **≈18.6 s since §26 put an inquiry in front of the page**),
witnessed on the `bt-l2`/`bt-c1` lines it emits, then it returns and the service loop resumes. A trigger in a quiet room bounded-fails exactly like the boot run.

**4. Gated as today.** `bt` for the chord/request/re-trigger, `btc` for the classic page and the
`bt_left_link` latch; default behaviour is unchanged except that the chain is now *also* reachable
post-boot. QEMU has no Bluetooth, so the chain itself is **compile-only** there (present and reachable
— `strings` shows the `bt-retry:` witnesses in a `bt`/`btc` build and none in a default build — but
never exercised); the chord's routing is what a fixture can reach.

**Witness format** (`bt-retry:` prefix — a metal capture proves the re-trigger fired, what it
recovered, and that the DATA0 resync happened):
```
EHCI-HID: BT pairing chord (Ctrl+Alt+B) — requesting Bluetooth re-trigger == witness ::
:: bt-retry: [N] src=1 STALE LINK handle=0x0.. — best-effort teardown before re-paging -> ...; the latch is CLEARED either way, so this is a reboot-FREE escape ... ::   (only when a link was left up)
:: bt-retry: [N] event-EP toggle resync — ClearFeature(ENDPOINT_HALT) on IN.. -> CONFIRMED (device toggle now DATA0 ...) == witness ::
:: bt-retry: [N] ACL toggle resync — ClearFeature(ENDPOINT_HALT) on bulk_in=IN.. bulk_out=OUT.. confirmed=2/2, host bt_acl_tog reset to (DATA0,DATA0) ... ::
:: bt-retry: [N] src=1 FIRING — re-running the boot bring-up chain against the radio claimed at boot (addr=..) ... ::
:: bt-retry: [N] src=1 COMPLETE — the chain returned; the bt-l2 scan summary and (under btc) the bt-c1 page summary above are its outcome == witness ::
```
plus the two declines: `DECLINED — a bring-up chain is already in flight`, and `request src=1 but NO
controller claimed a radio at boot`. The stale-link line is the reboot-free escape (§2); the two
resync lines are the DATA0 device/host toggle match a re-trigger must issue in place of the boot
path's `SET_CONFIGURATION`.

**What Boot C proves.** Peter reboots the speaker mid-session — after our boot's single chain has
already page-timed-out — and presses `Ctrl+Alt+B`. The chord line, then a fresh `bt-retry: FIRING`,
a fresh `bt-c1` page train against the now-page-scannable MEGABOOM, and either a BR/EDR link or
another bounded timeout — none of which existed on the boot path a minute earlier. If the boot run
had left a link up, the capture shows the `STALE LINK … reboot-FREE escape` line clearing it before
the re-page rather than a permanent "already connected"; every re-trigger carries its `event-EP` /
`ACL toggle resync` lines proving the DATA0 match; and a press in a quiet room bounded-fails exactly
like boot. The two SHOULD-FIX defects the escape hatch and the resync close are each therefore
provable from a single Boot C capture, on the exact scenario the arc exists for.

---

## 26. BT-PAGE — the page was aiming blind: inquiry first, then page (`UNAOS_BTC=1`, 2026-08-11)

### 26.1 The ground fact, and why it is not the speaker's

Boot D (`~/unaos-bench/capture/gr26-bootD/ttyUSB0.log`, `bt-c1` lines [3817..14064 ms]) ran two full
5.12 s page trains at the MEGABOOM and got `Connection Complete status=0x04` (PAGE TIMEOUT) for
both. **Peter confirmed the speaker was in pairing mode for the whole of that boot.** A device in
pairing mode is page-scanning *and* inquiry-scanning, so two full trains against it are not a fact
about the speaker.

The `bt-c1` witness of the day headlined the opposite reading — *"it is off, out of range, or not
page-scanning … THIS IS THE ORDINARY RESULT FOR A POWERED-OFF SPEAKER"* — and a reader following
that prose would have closed the investigation on a wrong cause. §26.4 is what replaced it.

Two further facts from the capture history bound the problem:

- **The page is not broken; it is intermittent.** `~/unaos-bench/capture/gr25-bootA/ttyUSB0.log`
  shows this exact configuration REACHING the speaker — `Connection Complete status=0x00
  handle=0x000b … BR/EDR LINK ESTABLISHED` at 11899 ms — but only on **attempt 2 of 2**, after the
  first train timed out. gr25-bootB and gr26-bootD then got 2/2 timeouts. A path that succeeds on a
  random-looking subset of trains is a path with an alignment problem, not a dead one.
- **"Boot AS connected the MEGABOOM" is about LE, not classic.** The `gr24-bootAS` connection at
  2377 ms is `bt-l3`'s `LE Connection Complete`; that flight's `UNAOS_BTC` was not armed. The
  classic runs in that capture's second session both timed out at the then-1280 ms deadline. The
  BT-C1 source comments cited "Boot AS" as the classic contrast case; that citation was wrong and
  is corrected in-source to gr25-bootA.

### 26.2 What the page was missing

`HCI_Create_Connection` carries two fields whose only legitimate source is an inquiry:

| Field | Boot D sent | Where it should come from |
|---|---|---|
| `Page_Scan_Repetition_Mode` | `0x02` (R2) — a guess | the peer's `Inquiry Result` |
| `Clock_Offset` | `0x0000`, bit 15 **clear** = "not valid" | the peer's `Inquiry Result`, with bit 15 **set** |

With the clock offset absent the controller cannot start its page train on the peer's clock phase;
it must sweep for it. With the repetition mode guessed it must size the train for the worst case.
That is precisely the shape of a page that reaches a listening device only when the phases happen
to line up — i.e. gr25-bootA's second train.

### 26.3 The sequence, before and after

```
BEFORE (Boot D)                         AFTER (this arc)
                                        HCI_Inquiry(GIAC, 5.12s, unlimited)   0x0401
                                          <- Inquiry Result(s)  0x02 / 0x22 / 0x2F
                                             harvest psrm + clock_offset for the target
                                          <- Inquiry Complete   0x01   (or early exit on target)
                                        HCI_Inquiry_Cancel                    0x0402  (if not complete)
HCI_Write_Page_Timeout 0x2000           HCI_Write_Page_Timeout 0x2000
HCI_Create_Connection                   HCI_Create_Connection
  psrm=0x02 (guess)                       psrm=<harvested>       (fallback 0x02)
  clock_offset=0x0000 (invalid)           clock_offset=<harvested>|0x8000  (fallback 0x0000)
```

Design notes that are decisions rather than details:

- **`HCI_Inquiry_Cancel` is mandatory, not tidiness.** A controller still in the Inquiry state
  answers `Create_Connection` with Command Status `0x0C` (Command Disallowed). An inquiry left
  running would make the page fail for a local-side reason a capture would read as the speaker's
  fault, so the cancel runs on every path where `Inquiry Complete` was not read — including the
  paths where the event endpoint went unreadable, since it rides EP0 and a halt does not touch EP0.
- **Bit 15 of `Clock_Offset` is a named constant** (`BT_C1_CLOCK_OFFSET_VALID`). An `Inquiry Result`
  reports the offset with that bit clear; paging without setting it makes the controller ignore the
  offset entirely, which is indistinguishable in a capture from never having harvested one.
- **The three result shapes are decoded separately.** `Inquiry Result` (0x02) spends two bytes on
  Reserved and none on RSSI; `Inquiry Result with RSSI` (0x22) and the fixed part of
  `Extended Inquiry Result` (0x2F) spend one on each. The per-response stride is 14 in all three, so
  a decoder that got the shape wrong would still walk the list correctly and read a **wrong clock
  offset** off every entry — the worst kind of wrong, because the resulting page fails exactly like
  an unaligned one.
- **`HCI_Write_Inquiry_Mode` is NOT issued**, contrary to the sketch in §23.6. The reset mode
  (`0x00`, standard `Inquiry Result`) already carries both fields this arc needs, and mode `0x02`
  would additionally require widening the event mask for Extended Inquiry Result at bit 46. EIR
  carries a friendly *name*, which is not what a page needs; the decoder accepts 0x2F defensively in
  case a part sends it anyway.
- **The early exit is what bounds the good case.** The wait ends on whichever comes first: an
  `Inquiry Complete`, or the target answering. `BT_C1_INQUIRY_LEN` (5.12 s) is paid in full only on
  a boot that does *not* hear the target — which is exactly the boot where the listening time has to
  be defensible.
- **The harvested fields are unauthenticated, and the mode is range-checked.** An inquiry response
  is not attributable to the device it names: any radio in range may answer with any BD_ADDR,
  including the target's, and it supplies the `Page_Scan_Repetition_Mode` and `Clock_Offset` this
  host then pages with. Both are therefore treated as hostile input. Lengths are never trusted —
  `Num_Responses` is a claim, and each 14-byte record is bounds-checked against the event's own
  `Parameter_Total_Length`, so a lying count truncates the walk (and sets `blind`) instead of
  running off the buffer. The mode is range-checked against `BT_C1_PSRM_MAX` (0x02): `0x03..0xFF`
  are Reserved for Future Use, and paging with one would be answered Command Status `0x12` (Invalid
  HCI Command Parameters) with **no train transmitted at all** — one bad byte turning the arc's own
  payload path into a boot that pages nothing. An out-of-range mode is refused, `BT_C1_PSRM` is
  substituted, the clock offset is kept (it is independent, and all 15 of its value bits are legal),
  and a witness line says the mode specifically was rejected. The clock offset needs no equivalent
  check; a wrong one costs a missed page, which is the outcome the stage already reports.
- **The inquiry window has its own event cap** (`BT_C1_INQUIRY_EVT_MAX`, 128) rather than L3's
  `BT_L3_EVT_MAX` of 16. Every other L3 wait listens for one named reply and treats other events as
  noise; the inquiry window's payload *is* the events it walks past. Sixteen is a handful of devices
  in a quiet room and is reached in the first second of a busy one — and because hitting the cap
  sets `blind`, the summary would have reported `read_to_term=false` for a target that was about to
  answer. The wall clock (`BT_C1_INQUIRY_MS`) is the real bound and always was: every read is handed
  the window's remaining time, so no number of events can outlast it.
- **The address itself is now cross-checked.** `BT_L3_PEER_ADDR_BYTES` was read off an LE
  advertisement, and a dual-mode device need not page under the address it advertises. The inquiry
  answers on classic, so its result list is the first evidence this project has gathered about which
  BD_ADDR the speaker actually pages under. Every response is printed; one sharing the target's
  three-byte OUI without matching in full is flagged. **It is not paged** — the address rule is
  Peter's (white board Q14) and this arc does not widen it — it is reported.

### 26.4 The witness, and the prose that was wrong

`bt-c1` gains three lines before the page (`inquiry parameters`, one `inquiry result` per response,
`inquiry summary` + `page fields`), and the page's own lines now report which values they carried:

```
:: bt-c1: [N] inquiry parameters — lap=0x9E8B33(GIAC ...) inquiry_length=0x04(=5120ms) ... == witness ::
:: bt-c1: [N] inquiry result — addr=.. psrm=0x..(R.) clock_offset=0x.... class_of_device=...... [rssi=-..dBm] event=0x..(..) -> .. == witness ::
:: bt-c1: [N] inquiry summary — responses=N target_found=.. same_oui_seen=.. read_to_term=.. -> .. == witness ::
:: bt-c1: [N] page fields — psrm=0x..(R.) clock_offset=0x....(bit15 ..) source=.. == witness ::
:: bt-c1: [N] page parameters — ... psrm=0x..(R., HARVESTED from the peer's own inquiry response) clock_offset=0x....(bit15 SET = VALID ...) ... == witness ::
:: bt-c1: [N] page summary — ... aligned_by_inquiry=.. -> .. == witness ::
:: bt-c1: [N] C1 tally — ... inquiry_responses=N page_aligned_by_inquiry=.. pages_attempted=.. ... == witness ::
```

The `inquiry summary` verdict is the line that separates four cases the old capture collapsed into
one: the target answered; the room is busy and the target is not in it; the room is silent (which
indicts *this host's* receive path as much as the room); or the inquiry did not run to term and
establishes nothing.

**The prose change.** Both PAGE-TIMEOUT readings were rewritten so our-side causes carry equal
weight with the peer's, and neither is presented as the headline:

- with a harvested response, the peer-side readings are **excluded by evidence** — the inquiry heard
  the address, so "off / out of range / not scanning" is not available to a reader at all;
- without one, the line states the readings as a set of equal weight (no harvested offset, a
  possibly-wrong address, or the peer's own state) and points at the `inquiry summary` as the thing
  that tells them apart, rather than at the reader's prior.

The `page summary`'s old "the short-deadline artefact Boot AS produced at 1280 ms" contrast was
also removed: per §26.1 that citation was wrong, and the honest contrast is gr25-bootA.

### 26.5 Cost

The classic stage's bounded worst case moves from **≈12.4 s to ≈18.6 s** (`BT_C1_INQUIRY_MS` 5600 ms
plus two `BT_L3_CMD_MS` round trips), and with the LE stage's repeat scan in front of it the worst
boot is ≈21 s. All of it is paid only by a boot that set `UNAOS_BTC=1`. The good case is much
shorter and is the point: an inquiry that hears the target exits early and the page it then makes is
aligned, so the run that ends in a link on the **first** train is what this buys.

`Ctrl+Alt+B` (§25) is unchanged and now more useful: a re-trigger re-runs the whole chain, so a
speaker put into pairing mode *after* boot is inquiry-scanning at the moment the chord fires.

### 26.6 What metal must verify (Boot B)

QEMU has no Bluetooth radio, so the stage is compile-only there; `strings` on a `UNAOS_BTC=1`
kernel shows every new witness (`inquiry parameters`, `inquiry summary`, `page fields`,
`HCI_Inquiry_Cancel (0x0402)`, `page_aligned_by_inquiry`, `THE PEER-SIDE READINGS ARE EXCLUDED`) and
a default build shows none. The falsifiable outcomes on metal, ranked:

1. **The arc works.** `inquiry summary … target_found=true`, then `page parameters … psrm=0x..
   (HARVESTED …) clock_offset=0x….(bit15 SET = VALID …)`, then `Connection Complete … status=0x00
   … BR/EDR LINK ESTABLISHED` on **attempt 1/2** — where gr25-bootA needed two trains and Boot D got
   none. `C1 tally … page_aligned_by_inquiry=true pages_attempted=1 links_established=1`.
2. **Aligned and still refused.** `target_found=true` and a PAGE TIMEOUT anyway. The peer-side
   readings are then excluded by the capture itself, and the remaining candidates are the harvested
   offset ageing between inquiry and page, this controller's page train, or the transport. This is
   the outcome that would send the next arc at the controller.
3. **The address is wrong.** `target_found=false same_oui_seen=true` with an `inquiry result` line
   naming a Logitech/UE-OUI address that is not `88:c6:26:cc:2d:3c`. That address is then the next
   arc's whole brief, and Boot D's timeouts were never about reachability.
4. **The target is not on classic.** `target_found=false same_oui_seen=false responses>0` — the
   radio and receive path are proven by the other devices, and what is unproven is that this BD_ADDR
   is on the air on classic at all.
5. **`responses=0`.** A GIAC inquiry heard nothing in a populated room. That indicts this host's
   receive path at least as much as the room, and the `inquiry summary` says so.

The refutation that outranks all of them is unchanged from §22: **HID dead after `bt-c1` = the
endpoint invariant broke.** This arc adds no `bt_arm_read` call site — every read goes through the
existing `bt_l3_await`, whose `armed` threading is what preserves the invariant by construction.

---

## 27. BOT-PARK — the retry ladder had no floor a slot id could not walk around (2026-08-17)

### The capture

`~/unaos-bench/scratch/pi0-b1b2/boot3-inputdeath-tail.txt`, Pi 4 metal. A `Generic USB SD Reader`
(058f:6362) behind hub slot 1 port 1 wedged at the transport level — CBW out, `IRQ_COUNT=0`, event
ring provably empty per the [piusb40] necropsy. Read the tail as a **cycle**, not as a list of
failures:

```
:: BOT: SURRENDER slot=2 … retracted=yes          <- the per-slot floor DID fire, as designed
xHCI: HUB slot 1 port 1 disconnect: slot 2 …      <- the ladder's OWN hub-port power-cycle rung (b')
:: PIUSB: [piusb25] storage enumerated: slot 5 …  <- the same reader, re-enumerated, NEW slot id
:: BOT: SURRENDER slot=5 …                        <- a whole fresh ladder allowance, spent
:: PIUSB: [piusb25] storage enumerated: slot 2 …  <- and back again. Forever.
```

Nothing in the ladder was wrong. Every rung did what §17's arcs built it to do. What was missing is
a verdict that **outlives a slot id**:

* `bot_surrendered_slot` is one `u8`. It binds the floor to a number the controller recycles, so a
  device whose prescribed cure is a cold re-enumeration escapes its own surrender **by being
  re-enumerated by that very cure**.
* Because the field holds exactly one slot, parking a second device releases the first. Slot 5's
  surrender is literally what put slot 2 back on the wire.
* `bot_fail_streak` and `bot_rescue_stage` are driver-global, and the disconnect path called
  `bot_rescue_clear` on them. A disconnect raised *while a ladder was mid-flight* therefore did not
  end that ladder — it handed it its allowance back.

Measured cost: a core at 99% for the whole sitting, `timeouts=` still climbing when the device was
pulled, at ~8.3 s of pump budget (`hw_wait_budget() * BOT_BUDGET_SCALE_FIRST`) per attempt.

### The fix: an account keyed to the device, not the slot

`BotDevLedger`, keyed by `BotDevIdent { root port, route string, VID, PID }` — every field of which
a re-enumeration reproduces exactly, and none of which is a slot id. Four entries; the per-slot
surrender is untouched underneath it. Four mechanisms, in the order they bite:

1. **Escalating back-off between ladder entries**, doubling `BOT_PARK_BACKOFF_MS` (100 ms) to
   `BOT_PARK_BACKOFF_MAX_MS` (4 s). It is **not spun**: it is a deadline `bot_park_gate` tests and
   declines, so the wait is paid in main-loop passes that render frames. (The in-ladder
   `BOT_RESCUE_BACKOFF_MS` settle between *rungs* is metal-earned and unchanged.)
2. **A bounded total retry budget per device**: `BOT_PARK_LADDER_MAX` (6 ladder entries),
   `BOT_PARK_SURRENDER_MAX` (2 surrenders), `BOT_PARK_CYCLE_MAX_MS` (45 s of pump wall clock).
   Whichever fires first PARKS the identity, and the park **skips the remaining rungs** — rung (b)/
   (b') is the port power cycle, i.e. the act that re-enumerated the device into a fresh allowance.
3. **Bounded work per pass, and a dead-ring budget cap.** Three bounds, because the core-eater is a
   *composition* of waits, not any single one — the boot3 per-pass measurement at the four c3=99%
   windows read 1,498,784,103 / 1,972,189,353 / 1,060,628,143 / 1,348,032,519 cycles against a
   normal pass of 119-134, i.e. 20-37 s inside one pass:
   * `BOT_PARK_PASS_LADDERS` (1) yields to the desktop loop after one ladder;
   * `BOT_PARK_PASS_MS` (10 s) refuses to *start* another wait in a pass that has already spent it —
     checked at the park gate, at the post-recovery retry and at each rung's retry, which are the
     three places a ladder chains a further multi-second wait. It never truncates a wait in flight
     and never touches `hw_wait_budget()`, and it applies only to an identity that already has an
     account (a healthy boot's *entire* BOT time is ~5 s, half this bound);
   * `BOT_PARK_DEAD_STREAK` (2) consecutive timeouts with a *provably idle* ring (no events, no
     foreign events, no doorbells — the [piusb40] signature) cut this device's pump budget by
     `BOT_PARK_DEAD_DIV` (8), so the steady state after a proven wedge is ~350 ms per attempt rather
     than ~8.3 s. Applied with `min`: it can only shorten a wait, and a healthy device never earns
     it.
4. **Guaranteed teardown on disconnect, and the unpark rule.** `bot_park_note_disconnect` runs
   *before* `bot_rescue_clear` on both disposal paths. If the disposed slot is the one under the
   running ladder it latches `bot_ladder_abort`, which the ladder checks between rungs and before
   every retry. The account is closed **only** for a disconnect this driver did not itself cause:
   both power-cycle rungs arm a self-cycle attribution window (`bot_park_arm_self_cycle`), and a
   disconnect inside it on the same route is the driver's own cure, not an operator replug.

### Witnesses

* `:: BOT: PARKED slot=… port=… route=… vid=… pid=… why=… ladders=…/… surrenders=…/… gens=…
  cycles=… ms=… dead_streak=… ::` — one per parked device, naming the clause that fired and the
  total cycles spent. This is the line that makes the next metal wedge self-diagnosing.
* `:: BOT: park census … result=CENSUS ::` / `park rollup accounts=… parked=… refused=…
  backoff_refused=… aborts=… capped=… yields=… ::` — printed beside the BOT SUMMARY. Every
  pre-existing BOT line keeps its bytes; the ledger speaks on its own lines.
* `park ladder-abort`, `park retry-refused`, `park yield`, `park keep`, `park clear`,
  `park refuse-bringup` — one line per event, each naming why.

**Clean boot reads `accounts=0 parked=0 refused=0 backoff_refused=0 aborts=0 capped=0 yields=0`**
(measured, `test-arm`, storage ready, `timeouts=0`), so any non-zero reading is itself the finding.

### Fixtures

QEMU models no wedge — `usb-storage` always answers — so a fixture that needed the real fault would
be permanently vacuous. Two things exist instead:

* `bot_park_selftest()` (`:: BOT-PARK: selftest … -> PASS ::`, REQUIRE + FORBID in
  `pi4-regression.spec`) exercises the discipline's arithmetic **and its keying** on every boot of
  both arches, with no controller — so it holds on a `skip_xhci` capture, which every pi4 regression
  capture is. Assertion `reenum=` is exactly the property the metal cycle violated: the same
  identity arriving on a different slot id must find the same, still-parked, account. Assertion
  `pressure=` forbids an eviction policy that could drop a parked device to make room.
* `UNAOS_BOTWEDGE=1` (feature `botwedge`, default OFF, **test only**) injects a synthetic transport
  wedge on the storage slot after its first 24 transactions — `Timeout` with nothing put on the
  wire, the shape of the metal fault. Measured on `test-arm`: the wedge arms, one ladder is charged,
  and the escalating back-off then declines 15 further attempts (`backoff_refused=15`) instead of
  paying a pump budget for any of them. It makes storage unusable by design, so every fixture
  downstream of a mounted disk fails on such a run — never enable it on media.

### R24 — the floor did not latch on metal, twice, and why (2026-08-17)

`BOT: PARKED` never printed on boot5 (41 pump timeouts) or boot6 (84), against a 45 s wall-clock
clause. It was not a threshold set too high. **The whole ledger was switched off for the one device
it was built for**, and the mechanism is one line:

```rust
if s.vid == 0 && s.pid == 0 { return None; }   // bot_ident, pre-R24
```

`slots[].vid/pid` are written in exactly ONE place — the intercepted device-descriptor event on the
**root** enumeration path. A hub-downstream device never reaches it. boot6's capture contains one
`>>> VENDOR ID` banner in its entirety, `[2109]` for the VIA Labs hub, and none for the wedged
'Generic USB SD Reader' hanging off it. So `bot_ident` returned `None` for that reader on every
call, and every BOT-PARK hook begins with that call: `bot_park_charge`, `bot_park_note_ladder`,
`bot_park_note_surrender`, `bot_park_budget_cap`, `bot_park_gate`, the census. All no-ops, all boot.

The capture convicts it three ways, each independently sufficient:

| observation in boot6 | what it rules out |
|---|---|
| 60 × `park yield` — the ladder WAS entered 60 times | the wedge failing to reach the escalation ladder. `bot_park_note_ladder` runs *before* the yield and charged none of 60 against `LADDER_MAX=6` |
| 97 × `pump budget=450000000`, no other value | the dead-ring budget cut having engaged at all — `bot_park_budget_cap` was `None` every time, so `dead_streak` was never even incremented |
| no `park census` / `park rollup` line at all | an account existing but sitting under threshold |

**The fix: the account is keyed on the ATTACHMENT POINT.** `BotDevIdent` equality is now root port +
route string and nothing else (`same_place`); VID:PID is carried, printed and upgraded in place when
it is learned, but is never part of the key. Port + route is what a re-enumeration reproduces and
what this driver can always observe; VID:PID is what it happens to have parsed. `bot_ident` now
requires only an active slot with a root port — a device the driver is running BOT transfers against
has been addressed, configured and endpoint-probed, and refusing to hold it to account because a
descriptor banner never printed is the bug. Selftest clause `place=` is this property.

Two further defects the same capture exposes:

* **The verdict sat on the ladder's critical path.** `verdict()` was consulted only in
  `bot_park_note_ladder`, i.e. only on a ladder ENTRY — while the wall-clock and dead-ring clauses
  are charged by the PUMP, which runs whether or not a ladder follows. It is now read in
  `bot_park_gate`, the one place every transfer and every bring-up passes through. A park reached
  there also surrenders, against the current publish generation, since no ladder will do it.
* **The per-pass cap's pass never ended.** boot6's `pass_ladders=` climbs `2,3,4 … 33` with no reset:
  the SCSI probe chains that reach the ladder are straight-line sequences inside one desktop
  iteration and never return through `poll_events`. A per-pass cap whose pass never ends is an off
  switch — after the first entry every ladder yielded at the top, so no rung ran and no surrender was
  ever reached, while the pump went on paying a full budget per attempt. `bot_pass_roll` now ends a
  pass on `BOT_PARK_PASS_MS` as well.

### R24 — the desktop throttle

boot6's vug ran at wf=1-2 against PA42's 25-41 on the same build, because each wedged attempt eats a
multi-second pump budget **on the desktop's own thread** (`main.rs` → `service_storage`). The bound
is `BOT_PARK_PASS_PUMP_MS = 2000`: the pump wall-clock one main-loop pass may spend, summed across
all slots.

* Charged **only on the timeout arm** of `pump_until_bot_done`. A completion costs nothing, so the
  FAT layer walking a large file through dozens of sequential READ(10)s in one frame can never trip
  it, however much work it does. What is bounded is the pass's *unproductive* time.
* Enforced at `bot_transfer_body`'s entry — every BOT transaction in the driver funnels through it —
  and **not** gated on the identity having an account, unlike `BOT_PARK_PASS_MS`. The first wedged
  attempt on a device the ledger has never heard of is exactly the metal case.
* Second half in the pump: for an identity that already HAS an account, the pass remainder is
  `min`-ed into the budget, floored at `hw_wait_budget()` so no rung's retry is ever starved below
  the base metal-earned handshake budget. A device with no history keeps
  `hw_wait_budget() * BOT_BUDGET_SCALE_FIRST` untouched — nothing here shortens a healthy wait.

Worst case per pass, therefore: **one** first-attempt budget for an unknown device, decaying to
`1/BOT_PARK_DEAD_DIV` of it the moment the dead-ring streak opens the account, then to nothing at
PARKED. boot6's several-budgets-per-pass composition is unreachable.

The census is also printed at the verdict now, not only from `log_summary_once` — that fires on the
main loop's 2000th pass, and boot6 never got there.

### R24 — the fixture reaches the verdict

`UNAOS_BOTWEDGE=1` returns `Timeout` without pumping, so it accrued nothing: the ledger's wall-clock
clause was unreachable in QEMU by construction, which is why the previous gate could only watch the
back-off decline attempts. The injection now charges the wait it stands in for
(`hw_wait_budget() * bot_budget_scale`, classified `dead` — the injected wedge IS the `[piusb40]`
signature), and credits the same fictional span against the back-off deadline, because the ledger
must not accrue on one clock while the gate refuses on another. Both credits are `cfg`-gated to the
feature; on metal a real ~7.2 s wait outlasts `BOT_PARK_BACKOFF_MAX_MS` on its own.

Measured, `UNAOS_BOTWEDGE=1 ./arroyo test-arm`:

```
:: BOT: park account-open port=1 route=0x0 vid=46f4 pid=0001 named=yes anon_total=0 … ::
:: BOT: PARKED slot=1 port=1 route=0x0 vid=46f4 pid=0001 why=cycles cause=Timeout ladders=4/6
   surrenders=0/2 gens=0 cycles=3600000000 ms=57600 max_ms=45000 dead_streak=8 … parked_total=1 ::
:: BOT: park rollup accounts=1 parked=1 … backoff_refused=3 … yields=2 pump_refused=0 anon=0 … ::
```

Go-red, plain `./arroyo test-arm` on a healthy `usb-storage`: `xHCI: storage ready.` and
`accounts=0 parked=0 refused=0 backoff_refused=0 pass_refused=0 aborts=0 capped=0 yields=0
pump_refused=0 anon=0`. The ledger opens nothing and the throttle refuses nothing on a clean boot;
the selftest proves the arithmetic in both directions on every boot of both arches.

### Owed on metal

The park verdict on a REAL wedge — `:: BOT: PARKED … ::` naming the reader's own port and route, its
census line immediately after, and the core coming back to the desktop — is still metal-owed; QEMU
proves the mechanism, not the fault. boot7 must print `park account-open … named=no` for the
hub-downstream reader (the direct falsification of the R24 miss), then PARKED, then a bounded
`pump_refused=` instead of 84 uncut budgets, with the vug back in PA42's 25-41 wf band.

### Open: the reader may be MULTI-LUN, and we never ask (R24, unfixed)

Peter's hardware note — the wedging device is a **multi-format** reader, several card slots, "may
show up funny with a bunch of blank slots". Convicted from code, not yet fixed:

* **`GET MAX LUN` (`bmRequestType 0xA1`, `bRequest 0xFE`, BBB §3.2) is never issued anywhere in this
  driver.** No occurrence in `drivers/xhci/`.
* **`bCBWLUN` is hardwired to 0** — one write, `*cbw_buf.add(13) = 0` in `build_cbw`. Every CBW this
  OS has ever sent went to LUN 0.

If the seated card is on any other LUN, every media-dependent command addresses an empty slot. The
boot6 evidence lines up exactly: INQUIRY (no media dependence) always completes — `[piusb40]`'s
post-wedge INQUIRY control returns `Ok`, so the bulk pipes are demonstrably alive *after* the wedge —
while READ CAPACITY / READ(10) always wedge. And `[piusb41]` caught the desync in the act:
`READ CAPACITY reply REJECTED — block_size=83886080 last_lba=0x55534253`. `0x55534253` is `USBS`,
the CSW signature: the device declined the data phase and answered with status, and the driver read
that status into the capacity buffer — a device saying "nothing here" in the one way we do not parse.

The wire-in — `GET MAX LUN`, then per-LUN `TEST UNIT READY` to find the seated slot, then CBWs
addressed there — is the named follow-up. It **composes with** this section rather than replacing it:
an all-slots-empty reader must still PARK cleanly instead of grinding, which is what the floor above
now guarantees.

## 27. BT-SSP — Secure Simple Pairing: the link becomes a bond (`UNAOS_BTC=1`, 2026-08-12)

§24's L2CAP attempt named its own successor: a speaker that answers `CONNECTION_RESPONSE` with
result `0x0003` (SECURITY BLOCK) is demanding an authenticated, encrypted link before it will open
the AVDTP PSM. This stage is that authentication — Secure Simple Pairing in the "just works"
association model, run on the live BR/EDR handle §26's page just established, *before* the C2
attempt on the same link, so one boot carries both arms of the SECURITY BLOCK experiment.

### 27.1 The handshake, and why it is one dispatch loop

After `HCI_Write_Simple_Pairing_Mode` (0x0C56, Vol 4 Part E §7.3.59) and an event-mask widening
(§27.2), `HCI_Authentication_Requested` (0x0411, §7.1.15) starts a handshake whose event order
belongs to the **controller**, not the host: `Link Key Request` (§7.7.23), `IO Capability
Request`/`Response` (§7.7.40/41), `User Confirmation Request` (§7.7.42), `Simple Pairing Complete`
(§7.7.45), `Link Key Notification` (§7.7.24), `Authentication Complete` (§7.7.6). A sequence of
narrow waits would walk past — and lose — any family member arriving out of the assumed order
(`bt_l3_await` discards non-matches), so the stage adds one want, `BtL3Want::SspAny`, that matches
the whole family plus Command Status/Complete, and a dispatch loop answers each event as it
arrives: no key held → `Link_Key_Request_Negative_Reply` (§7.1.11); IO capabilities →
NoInputNoOutput, no OOB, MITM-not-required + dedicated bonding (§7.1.29 — the truth about a
machine with no consent UI at boot, which resolves the model to Just Works per Vol 3 Part C
§5.2.2.6); user confirmation → auto-accepted with the numeric value **printed**, so the capture
shows what was accepted unseen (§7.1.30). After `Authentication Complete` status 0x00,
`HCI_Set_Connection_Encryption` (0x0413, §7.1.16) is issued inline and the stage ends on the
`Encryption Change` event (§7.7.8). Bounds: `BT_SSP_STAGE_MS` (8 s hard cap), a per-event window,
and a structural turn cap — C1's mandatory disconnect runs after it on every path.

Events outside the just-works flow are **parked, not improvised**: `PIN Code Request` (legacy
fallback), `User Passkey Request`, `Remote OOB Data Request` are each witnessed with their raw
event bytes, answered with the spec's negative reply (no invented PIN, no guessed passkey), and
end the stage.

**Operator note — HID pauses during pairing.** The whole BT chain runs inside one
`service_ehci_hid` pass holding the EHCI_HID lock, and this stage's worst case is `BT_SSP_STAGE_MS`
(8 s). During a pairing the internal keyboard and trackpad — serviced by that same lock — are
therefore unresponsive for up to that window; a healthy just-works pairing spends a fraction of it,
but a peer that engages and then stalls holds HID for the full cap. This is expected, not a hang,
and clears the instant the stage reaches its tally. (It is the same lock-hold that blocks the link
key from a synchronous filesystem write — §27.3.)

### 27.2 The event mask, again

The mask in force after §21 is the reset default plus LE Meta (bit 61) — and the reset default
ends at bit 44, while the six SSP events live at bits 48..53. Without a new
`HCI_Set_Event_Mask` the controller runs the pairing and tells the host *nothing*: the same
clean, silent, entirely wrong shape as §21's missing bit 61. The stage writes
`0x203F_1FFF_FFFF_FFFF` (default + SSP family + LE Meta) before requesting authentication, and
leaves it in place under the same provenance note as L2's widening.

### 27.3 The bond, and the persistence gap (honest)

`Link Key Notification` delivers the bond: a 16-byte link key (type 0x04, unauthenticated
combination — the just-works outcome), stored in `Controller::bt_ssp_key`. The key bytes are
**withheld from the log on purpose** — a serial capture must not carry the bond secret. A
re-triggered chain (`Ctrl+Alt+B`, §25) re-enters the stage with that key and answers the
controller's `Link Key Request` positively (§7.1.10), so its authentication completes with **no
second pairing** — the bond doing its job inside one session.

**The gap:** a real bond survives power-off; this one cannot yet. The kernel's writable VFS rides
USB mass storage serviced by this same driver under the EHCI_HID lock the BT chain already holds,
so a store write from inside the chain would re-enter the lock. Until a deferred-write path (or a
non-USB store) exists, every boot pairs afresh and the speaker accumulates one bond entry per
boot. The Link Key Notification witness names this on the wire.

### 27.4 What metal must verify (the next BT boot)

`strings` on a `UNAOS_BTC=1` kernel shows the `bt-ssp:` family (38 lines: `stage=arm`,
`stage=ssp-mode`, `stage=event-mask`, `stage=auth-request`, `stage=link-key-request`,
`stage=io-cap-request`, `stage=io-cap-response`, `stage=user-confirm`, `stage=pairing`,
`stage=link-key`, `stage=auth-complete`, `stage=encrypt`, `PARKED`, `SSP tally`); a default build
shows none. The falsifiable outcomes, ranked:

1. **The rung holds.** `stage=pairing … PAIRED`, `stage=link-key … STORED IN RAM`,
   `stage=auth-complete … AUTHENTICATED`, `stage=encrypt … ENCRYPTED (E0 …)`, tally
   `-> BONDED AND ENCRYPTED` — and then §24's `CONNECTION_RESPONSE` **without** result 0x0003:
   the SECURITY BLOCK lifted by this stage on the same boot.
2. **Bonded but still blocked.** Tally `BONDED AND ENCRYPTED` and C2 still reads 0x0003 — the
   speaker wants something beyond authentication+encryption (or a fresh L2CAP attempt on a new
   link), and that becomes the next brief.
3. **The peer refuses the bond.** `Simple Pairing Complete status=0x05` — commonly a speaker
   already bonded to another host and not in pairing mode. The re-trigger chord after putting it
   in pairing mode is the second experiment, free.
4. **A park fires.** `PIN Code Request` raw bytes = the controller ignored SSP mode; that indicts
   the 0x0C56 write path or the part's ROM, not the peer.
5. **Silence mid-handshake.** The `NO family event` witness names the last milestone reached;
   a stall at `stage=auth-request` with the mask line REFUSED above it is the mask, not the air.

The §22 refutation outranks all of these unchanged: **HID dead after `bt-ssp` = the endpoint
invariant broke.** The stage adds no `bt_arm_read` call site; every read rides `bt_l3_await` and
`bt_hci_command`, whose `armed` threading preserves the invariant by construction.

What C3 (audio) still needs is not more security: AVDTP `SET_CONFIGURATION`/`OPEN`/`START` on the
SEP §24 discovers, an SBC encoder, and the ACL data path at streaming rate — plus, eventually, the
deferred-write path that turns this session bond into a persistent one.

---

## 28. BT-BOND — the Holocron seam and the bond store (`UNAOS_HOLOCRON=1` / `UNAOS_HCRONST=1`, 2026-08-21)

> **HCR1 (2026-08-21)** — an adversarial review of M1 returned four SHOULD-FIX findings, all four
> fixed in the commit this note rides. §28.1a corrects what the flush guard can assert and bounds its
> witness; §28.2a splits the boot-time-write selftests behind `hcronst`; §28.3a replaces the live-leaf
> overwrite with a stage-verify-swap and quarantines a refused image; §28.7 item 1 closes the aarch64
> cfg-coverage hole. Where an M1 statement is now wrong it is corrected in place and the correction
> says what it used to say — the old text is the evidence for why the fix exists.

§27.3 names the gap this section closes: the link key SSP produces lives in
`Controller::bt_ssp_key`, in RAM, for one session. A real bond survives power-off. The obstacle was
never the filesystem — it was **where the key arrives**. This section is the store, the seam it sits
on, and the deferral that makes writing it possible at all.

**Milestone status.** M1 (this section as written) landed the seam, the record schema, the codec, the
table rules and the proofs. It adds **no HCI command and no radio access whatsoever**: nothing in
`bt_ssp_pair` calls into the store yet, so §27.3's gap paragraph is still accurate and is
deliberately left standing. **M2** wires the Link Key Notification and Link Key Request arms to the
store and rewrites §27.3 accordingly; **M3** proves eviction end to end and finishes the LE-identity
population rule. Reading §27.3 and §28 together today: the mechanism exists and is proven; the
driver has not yet been pointed at it.

### 28.1 The re-entrancy wall (why this is not just a file write)

The whole BT chain runs inside one `service_ehci_hid()` pass holding the `EHCI_HID` mutex
(`drivers/ehci/mod.rs`). The writable FAT volume rides USB mass storage whose I/O goes through
`drivers/block.rs` → `xhci::claim()` + `storage_read10`/`write10`. A filesystem write issued from
inside the BT chain would therefore

* contend the xHCI storage loan **from inside** the EHCI service pass, and
* hold the internal keyboard and trackpad hostage for the write's duration — on top of the 8 s
  worst case §27.1's operator note already describes.

So the store is split in two, and the split is the whole design:

| Phase | Where it runs | What it costs |
| --- | --- | --- |
| `holocron::put` / `remove` (and `btbond::stage_store` / `stage_remove` above them) | anywhere, lock held or not | a `memcpy` into a fixed table and a `bool`. No I/O, no allocation, no wait. |
| `holocron::flush_if_dirty` | the main loop, storage-gated, no driver lock held | one whole-file rewrite through the FAT write path. |

`flush_if_dirty` **checks that invariant rather than asserting it in a comment**: on x86 with the HID
path built it consults `EHCI_HID.is_locked()` before issuing any block I/O. What that check can
actually assert is the subject of the next subsection, because M1 overstated it.

### 28.1a What the flush guard can and cannot say (HCR1, 2026-08-21 — corrected)

M1 shipped the guard documented as *"is the EHCI HID mutex held **on this core** right now?"*, and the
witness it fed asserted *"…and this call site is inside it"*. Both claims were wrong.
`spin::Mutex::is_locked` is a **global** predicate: it reports that the lock is taken and never by
whom. It cannot distinguish

* **this call stack is inside a `service_ehci_hid()` pass** — the bug the seam exists to prevent — from
* **another task is mid-pass** — benign, and expected. `main.rs` spawns `usb-pump` (which calls
  `holocron::service()`) and `input` (which reaches `service_ehci_hid()` at roughly 1 kHz through
  `pal::pump_and_poll`) as two preemptible tasks on the same `svc_cpu`. An interleaving that finds the
  lock taken is an ordinary scheduling outcome on a **correct** build.

Two consequences, both real:

1. the printed line stated as fact something the evidence could not support;
2. the refusal `return`ed **before** `flush_fails`/`gave_up`, so it never consumed the
   `HCRON_FLUSH_ATTEMPTS` budget. The module's own stated law — *a volume that vetoes writes must not
   be able to make the main loop print forever* — was enforced on the I/O-failure path and not on this
   one. While the store was dirty and the lock contended it printed **once per main-loop pass,
   unbounded**. And because the same commit gave `x86-fat.spec` a hard `FORBID [hcron] flush REFUSED`,
   a benign scheduler interleaving on an armed run could red a spec three tracks share.

**What it is now.** The predicate is renamed `ehci_hid_busy()` and read only in the direction it is
sound in:

| reading | what it proves | action |
| --- | --- | --- |
| not held | nobody holds it, so **this stack does not**. A proof. | write |
| held | **UNKNOWN** — both readings say the same thing about this instant. | defer |

A `true` is therefore a **deferral**, never a verdict about the caller. Retries stay unbounded:
deferral is not a failure, it costs one atomic load per pass, and a legitimately long service pass (an
SSP chain runs for seconds, and it is precisely that chain which makes the store dirty) must not cost
the boot its persistence. What is bounded **by construction** is the witness — `HCRON_DEFER_NOTES = 2`
lines per boot, counted where the print happens so the counter counts output rather than intent. The
first line explains the reading; the second, at `HCRON_DEFER_STUCK = 4096` consecutive deferrals,
states both interpretations and stops. Deferral touches neither `flush_fails` nor `gave_up`, which
count I/O that was *attempted* and refused by the volume.

**The bound is proven by an executed fixture, not by this paragraph.** `defer_bound_fixture_once()`
takes `EHCI_HID` for real — `try_lock`, never `lock`, so it can never hold the keyboard and trackpad
hostage; a pass that cannot take the lock instantly is simply not its pass — drives `flush_if_dirty`
4160 times, and checks that every pass reached and took the deferral return, that **zero** writes were
issued (`seq` unmoved, the dirty flag it set still set), and that the witness emitted no more than the
cap. It restores the dirty flag and the deferral accounting afterwards, so a boot's real budget is not
spent by a test, and it writes nothing at all — which is why it rides `holocron` rather than the
`hcronst` write knob.

```
:: [hcron] deferral bound: EHCI_HID HELD, flush_if_dirty driven 4160 times (past the 4096-pass
   escalation and 64 further) — every pass deferred, ZERO writes issued (seq unmoved at 0, still
   dirty), and the witness emitted 2 line(s) against a cap of 2 -> PASS ::
```

Go-red, measured rather than argued: with the cap removed from `note_defer` (i.e. the pre-HCR1
behaviour restored) the same boot printed **4160** deferral lines and the fixture reported
`… 4160 deferred passes emitted 255 witness lines against a cap of 2 … -> FAIL ::`, which the
`arroyo test` verdict caught on its own, before any spec replay.

**A sharper predicate is possible and is deliberately not in this file.** Recording an owner — a
marker set and cleared around the body of `service_ehci_hid`, compared against
`sched::current_task_id` — would answer the question M1's doc comment claimed to answer. It belongs on
the EHCI side of the seam, in `drivers/ehci/mod.rs`, where the pass is bracketed; the store can only
sample a lock it does not own. Until that exists, the honest statement is the one above: the guard
proves the safe case and defers the ambiguous one. (The three `main.rs` call-site comments still say
"it refuses while `EHCI_HID` is held"; that file was outside this arc's lane and the wording is stale
there.)

### 28.2 Where the code lives

| File | Contents | Gate |
| --- | --- | --- |
| `src/fs/holocron.rs` | the seam: framing, CRC, class registry, in-RAM table, `load_once` / `publish_store_file` / `flush_if_dirty`, the framing fixture, the deferral-bound fixture | `holocron` |
| `src/drivers/ehci/btbond.rs` | the first client: bond record schema v1, codec, table rules, either-form lookup, the codec KAT | `holocron` (inside the x86-gated `drivers/ehci/`) |
| the two store round-trip selftests, in those same two files | boot-time WRITES to the medium — see §28.2a | `hcronst` (implies `holocron`) |

#### 28.2a Two knobs, because one of them writes the user's medium (HCR1)

`holocron` arms the **store**. `hcronst` arms the two **selftests** — `holocron::selftest_once`, which
writes and unlinks `/HCRON/HCRNTEST.DAT`, and `btbond::selftest_once`, which stages a fixture bond
through the real flush and so creates `/HCRON/BTBOND.DAT` and leaves it behind as an empty store.

Both check `write_veto()` first and both self-clean, so neither is reckless. The split is not about
recklessness; it is this repo's standing convention for a destructive write, the same one that gives
`sdw` a knob apart from `sdhcblk` so a build can carry the SD block backend without carrying the
card-write. Arming a **mechanism** must not be the same act as arming a **test that writes**: as M1
shipped it, M2's real consumer could not have the store without two boot-time writes to the user's
boot medium. The store itself touches the medium only when a record is actually staged.

The deferral-bound fixture is deliberately *not* behind `hcronst`: it writes nothing, so arming the
store arms its own bound-proof. `hcronst` is mapped in **both** `arroyo` (`UNAOS_HCRONST=1`) and
`builder/src/main.rs`, by the same s42/INSTGUI rule that applies to `holocron`.

`handlers/holocron/` remains a ring-3 design-stage stub — no crate, no entry point, no code. This is
therefore **not** an RPC to a handler that does not exist: it is the minimal kernel-side vault the
future userspace Holocron adopts. The seam is arch-neutral (it drives only `fs::fat` and
`crate::hash`), so an armed aarch64 build gets the store and its framing fixture and nothing
Bluetooth-shaped; the bond client sits inside `drivers/ehci/`, which is
`all(target_arch = "x86_64", feature = "ehcihid")`-gated.

**Named-path divergence, recorded because it is real.** The BT-BOND design specifies the rewrite
"through the VFS/FAT write path (`fs/vfs.rs` `create`/`write`)". In this tree
`impl VfsBackend for FatBackend` — with `resolve_parent`, `fat_err` and `fat_create_err` — is
`#[cfg(target_arch = "aarch64")]`. On x86_64, the platform this arc is for, `FatBackend` is a struct
with no backend impl and cannot be mounted into a `MountTable` at all. The store therefore calls
`fs::fat`'s dir-aware twins directly (`locate_in_dir` / `create_dir` / `create_in_dir` /
`delete_located` / `write_grow`) — the exact primitives the aarch64 `FatBackend` adapter wraps,
reached the way the arch-neutral `shell.rs` file verbs and `flight_recorder.rs` already reach them,
landing on the same `block::write_block_usb` BOT WRITE(10) path. Adding an x86 arm to `fs/vfs.rs` is
the alternative and is out of this arc's lane.

### 28.3 On-disk format (v1) — `/HCRON/BTBOND.DAT`

```text
header:  magic "HCRN" | ver u8 = 1 | count u8 | seq u32 (LE) | hdr_crc32 (LE)     -- 14 bytes
record:  class u8 | len u8 | body[len] | crc32(class, len, body) (LE)             -- 6 + len bytes
```

`hdr_crc32` covers the ten bytes before it. Each record's CRC covers its **framing bytes as well as
its body**, so a flipped `class` or `len` is caught by the same check that catches a flipped body
byte. CRC-32/ISO-HDLC comes from the arch-neutral `src/hash.rs` — the same variant the GPT writer and
the gzip trailer check already use — so an image the kernel wrote is checkable by host tools without
new code. `seq` is a monotonic write counter, bumped on every successful flush; there is no RTC on
this machine, so `seq` is the only clock the store has.

The directory and leaf are 8.3-clean by construction (`HCRON`, `BTBOND.DAT`); the store creates
`/HCRON` on first use. The write is a **whole-file rewrite** because the record count can shrink and
an in-place overwrite would leave a tail of the previous image behind — but *which file* is rewritten
is the point of the next paragraph.

#### 28.3a The update is a SWAP, not an overwrite (HCR1, 2026-08-21)

M1 wrote the live leaf directly: `delete_located(BTBOND.DAT)` → `create_in_dir` → `write_grow`. The
CRC catches a torn **record**; it is no help against a torn **update**. That sequence leaves a window,
as wide as the whole grow, in which the previous generation is already gone and the new one is not yet
whole — and a failure or a pull anywhere inside it leaves **no store at all** rather than a
stale-but-valid one. It composed badly with the load path, too: a refused image triggers `clear()`,
which marks the table dirty, so the next flush overwrote the refused file. One medium bit-flip
therefore discarded the user's bonds permanently, with nothing left to recover them from.

Both are closed. `publish_store_file` is now a four-step swap:

1. **Stage** — whole-file rewrite of `/HCRON/BTBOND.NEW`. The live leaf is untouched, so a failure
   here costs nothing and the next pass retries.
2. **Prove** — read the temp back off the medium and `parse_image` it. The old generation is not
   allowed to die on the strength of a `write_grow` return code; it dies only once the bytes that will
   replace it have been read back through the same path a future boot will read them through, and have
   parsed. This is also a torn-write check on the write that just happened.
3. **Drop** the live leaf (`mark_dir_deleted` + free chain).
4. **Rename** the temp over it — `rename_entry` rewrites the name field of the temp's directory entry
   in place, one directory-sector RMW, the smallest window `fs::fat` can offer.

Nothing here is atomic in the hardware sense; FAT cannot be. What it is, is **recoverable at every
point**. The window between 3 and 4 is one sector write wide and the data is intact under the temp
name throughout it, so `load_once` covers it directly: if the live leaf is **absent** and
`/HCRON/BTBOND.NEW` **parses**, that image is adopted and the table is marked dirty so the next flush
finishes the swap the crash interrupted. The live leaf is always tried first, and a live leaf that
merely fails to parse never falls back to the temp — that is a finding about the medium, not a reason
to reach for a file whose own provenance is a crash.

**And a refused image is quarantined, not overwritten.** Before `clear()` marks the table dirty,
`load_once` renames the refused bytes to `/HCRON/BTBOND.BAD` (one generation kept; a second refusal
replaces it). The witness says which way it went, because "kept" and "lost" must never be
indistinguishable on the wire:

```
:: [hcron] load: bad record crc (BTBOND.DAT) -> store starts EMPTY, fail-closed (nothing partially
   adopted); refused bytes KEPT as /HCRON/BTBOND.BAD (one generation; recoverable off the medium)
   == witness ::
```

The store's own paths never delete a leaf outright any more — they swap over it. `unlink_store_file`
survives only for the selftest's scratch leaf and rides `hcronst` with it.

**Fail-closed, with no partial adoption.** Bad magic, an unknown version, a bad header CRC, a bad
record CRC, a truncated body, an over-long body, more records than the table holds, or trailing bytes
past the last record — every one refuses the **whole** image and the store starts empty, witnessed
with which refusal fired. A store that adopted the records it managed to read before the damage would
be a store whose contents depend on where the corruption happened to land. On a refused load the
table is marked dirty, so the next flush **replaces** the bad image rather than leaving a file every
future boot will refuse identically — and since HCR1 the refused bytes are **moved aside first**, to
`/HCRON/BTBOND.BAD`, so "replaces" no longer means "destroys the only copy". See §28.3a.

**Keys.** The framing carries no key field: the key is a span *inside* the body, declared per class
by `class_key_span`. `put` takes the caller's key explicitly and refuses a key that does not equal
that span, so a class codec and the store cannot drift apart about what a record's identity is. Class
registry, v1: `HCRON_CLASS_BTBOND = 0x01`, key = `bd_addr`, six bytes at body offset 2.

### 28.4 Bond record schema v1 (class 0x01 body)

```text
ver           u8   = 1
flags         u8    bit0: LE identity present; bits 1..7 reserved = 0
bd_addr       [6]   BR/EDR page address, WIRE order (LSB first)
bd_addr_type  u8    0x00 public — mirrors the HCI address-type vocabulary
link_key      [16]
key_type      u8    verbatim from Link Key Notification (0x04..0x08)
le_addr       [6]   LE identity/advertise address, wire order (zeros when flags bit0 is clear)
le_addr_type  u8    0x00 public / 0x01 random (meaningful only when flags bit0 is set)
seq_used      u32   the holocron write counter at last successful use — the LRU clock
```

**37 bytes.** (The BT-BOND design document tallies this same field list as "31 bytes"; that is an
arithmetic slip in the prose, not a different layout. The field list is normative.)

**Both identity forms from day one.** Today the only address this tree knows is the LE advertise
address (`bt_name.rs`'s `BT_L3_PEER_ADDR_BYTES`, `88:c6:26:cc:2d:3c`); the BR/EDR page address is
whatever `Connection Complete` binds a handle to, and §26's page trains to the advertised address
still time out — the live hypothesis being that a dual-mode device pages under a different BR/EDR
address. Carrying both forms means that when the address question resolves, a bond written by the
pairing path already records the address it authenticated on **and** the address the peer was first
seen under, so a lookup by either form hits the same record and no schema bump is needed.

**Lookup rule.** `bd_addr` first (the primary key, one indexed hit); on a miss, a record whose
`le_addr` equals the query address and whose presence flag is set. The witness names which form
answered — that line is evidence about the address hypothesis, for free.

**Table rules.** `BTBOND_MAX = 4`. `bd_addr` is the primary key, so a re-pair of a known address
**replaces** its record rather than appending — §27.3's "one bond entry per boot" accumulation cannot
happen. When the class is full and a new address arrives, the record with the smallest `seq_used` is
evicted, witnessed. LRU by write counter, because there is no clock.

### 28.5 At-rest posture (what is claimed, and what is not)

The FAT volume is plaintext and this machine has no protected key storage — no TPM, no SEP path.
v1 stores the link key **plaintext-on-media, CRC'd, and says so**. The CRC is torn-write detection,
**not** authentication: it stops a half-written record from being adopted; it stops nobody who can
write the file. A kernel-embedded cipher key would be theatre — recoverable from the image by anyone
holding the medium — and is explicitly not claimed.

What v1 does enforce is process hygiene: key bytes are never printed to serial (the standing
`bt-ssp` law, extended — every `[btbond]` witness carries addresses, key *types*, counts and
sequence numbers, never key material), staging buffers are zeroized on every exit path including the
fixtures', and a vacated table slot is wiped rather than merely unlinked.

Threat scope, stated plainly: the asset is a BR/EDR unauthenticated combination key for a
loudspeaker. Exposure of the file to someone holding the physical medium enables impersonating this
host *to the speaker*, or eavesdropping that link. It does not open the machine. Vault encryption
(hardware-backed or passphrase-derived) is Holocron-proper's job and the format's `ver` byte is the
migration hook. The ledger entry is in [`SECURITY.md`](../../../SECURITY.md).

### 28.6 Boot ordering, stated rather than engineered around

`service_ehci_hid()` — where the boot-time BT chain runs — is polled from the very first main-loop
passes, while the FAT mount is a later storage-ready one-shot. So on a boot that arms the radio, the
first BT chain **can** run before the store is loadable. That window is real and this design accepts
it rather than reordering boot: `holocron::is_loaded()` exists precisely so a miss taken inside the
window is witnessed as *"store not loaded yet"* rather than as *"no such bond"*. The paths that
matter for reconnection — the `Ctrl+Alt+B` re-trigger (§25) and any future auto-reconnect — run long
after storage is up.

The store's whole main-loop presence is one call, `fs::holocron::service()`, placed beside
`fs::fat::probe_once()` at all three storage-ready passes `main.rs` carries (which pass a given build
reaches depends on its knobs; the one-shots inside make it speak exactly once). Order inside is the
argument in miniature: pure fixtures → `load_once` → the class clients → `flush_if_dirty` **last**,
so a record staged this pass reaches the medium this pass — deferred past the driver's lock, not
deferred by a whole extra loop iteration.

### 28.7 What QEMU proves, and what only metal can

QEMU models no BT controller — the internal hub and radio are bench hardware — so every HCI leg is
metal-only. What a QEMU boot *can* prove, and does:

1. **Compile + reachability.** `./arroyo check` green on both arches, armed and unarmed; `strings` on
   a `UNAOS_HOLOCRON=1` builder-path kernel shows the `[hcron]` and `[btbond]` witness families, and
   a default build shows none (the §27.4 discipline, extended). Both knobs are mapped in **both**
   `arroyo` and `builder/src/main.rs` — mapped in one alone would put `holocron`/`hcronst` in the
   `⚡ kernel features:` banner over a kernel with the modules compiled out.

   **The aarch64 leg (HCR1).** `holocron` and `hcronst` are deliberately not stripped by
   `arm_features`, on the grounds that the seam emits real aarch64 code and stripping would silently
   *disarm* the knob there rather than preserve a byte-identity that does not apply. Until HCR1 that
   argument was untested by the standing gate: `holocron` appeared in no aarch64 cfg leg at all, so an
   aarch64 regression in the very file the decision exists to protect would not have been caught. Both
   knobs are now appended to `arroyo`'s `arm-pi` leg — **and to `x86-all`**, which is not optional:
   `x86_cfg_universe` computes "aarch64-only" as *named by an arm-\* leg and by no x86-\* leg* and
   subtracts that set from the pairwise-mix universe, so naming them on `arm-pi` alone would have
   traded the aarch64 hole for an x86 one. Named on both, the universe is unchanged (still 8 mix legs,
   12 in total) and both arches type-check the seam. These are type-check legs: `arm_features`, which
   is what media builds go through, is untouched, and `kernel8` builds from its own curated
   `K8_FEATS` — so no Pi or Jetson media hash moves.
2. **The framing fixture** (`:: [hcron] framing fixture … -> PASS ::`) — pure, no hardware. Eight
   legs: a clean serialize→parse round-trip, then every refusal **made to fire** (body CRC, header
   CRC, truncation, trailing bytes, magic, version), then the untouched copy still parsing, so seven
   refusals cannot be a parser that refuses everything.
3. **The codec KAT** (`:: [btbond] codec fixture … -> PASS ::`) — pure. A synthetic Link Key
   Notification (the 25-byte assembly the SSP arm parses) → record → encode → decode field-identical;
   short and wrong-version refusals; the class registry's key span agreeing with the schema's
   `bd_addr`; the either-form lookup rule discriminating; and record → holocron framing → parse →
   decode with a one-byte corruption refused in between.
4. **The deferral bound** (`:: [hcron] deferral bound … -> PASS ::`) — needs a block device and
   nothing else, writes nothing, rides `holocron`. See §28.1a for what it drives and why an argument
   would not have been enough.

5. **The store round-trip through real FAT** — two witnesses, both self-cleaning, both behind
   `hcronst` because both write the medium (§28.2a):
   `:: [hcron] store round-trip … -> PASS ::` writes an image to a scratch leaf, reads it back
   byte-identical, then flips one byte **on the medium** and proves the load refuses it; and
   `:: [btbond] store round-trip … -> PASS ::` stages a fixture bond on `aa:bb:cc:dd:ee:ff` through
   the real table, flushes it, looks it up by **both** identity forms, then evicts and re-flushes so
   the medium is left as it was found. Its two `flush -> … ok` lines are also what proves the publish
   swap of §28.3a completed: `ok` means the temp was staged, read back, parsed, and renamed over the
   live leaf — not merely that a write returned.

   Gate: `UNAOS_HOLOCRON=1 UNAOS_HCRONST=1 ./arroyo test-fat sf 150`, asserted by
   `scripts/specs/x86-holocron.spec` (REQUIREs) and `scripts/specs/x86-fat.spec` (OPTIONAL/FORBID, so
   the default gate stays green). A `UNAOS_HOLOCRON=1`-only capture satisfies the holocron spec's §1–§3
   and is short on §4/§5 — the honest outcome of the knob split, not a regression.

**What the specs forbid, restated (HCR1).** `x86-fat.spec` used to carry
`FORBID [hcron] flush REFUSED`, firing knob-on or knob-off. Per §28.1a that line could fire on a
correct build, so a benign scheduler interleaving on an armed run could red a spec three tracks share.
It is gone from both specs. What gates instead is the deferral-bound fixture's `-> FAIL ::` variant —
reachable on **every** armed run rather than only on the schedule that happens to contend the lock —
together with the standing `FORBID [hcron] flush -> … GIVING UP`.

Mock *HCI event* injection into `bt_ssp_pair` is deliberately not attempted: that dispatch loop is
welded to the EP0/interrupt-EP transport, and a mock seam there would be invasive scaffolding for
little proof. The parse arm is covered by the codec KAT instead.

**The metal falsifier, which is not this milestone's job:** pair → power-cycle → the Link Key Request
answered from a persisted bond → `Authentication Complete` status 0x00 with **no SSP exchange**. It
needs M2's wiring. Known risk, stated: §26's page trains to the speaker still time out, so the metal
proof may need the discoverable-inquiry discriminator to land first. The QEMU-provable store is
correct independently of it.

### 28.8 Standing holds, respected by construction

* **Scan_Enable hold.** This path adds **zero** HCI commands. M1 touches no HCI surface at all;
  M2 will touch `Link_Key_Request_Reply`, which §27 already sends. No `HCI_Write_Scan_Enable`
  (0x0C1A) call site exists anywhere in the tree and this work introduces none — reconnection stays
  outgoing-page-shaped. The hold needs no exception and none is designed around.
* **Key bytes never on serial.** Every `[btbond]` and `[hcron]` witness carries addresses, key types,
  counts and sequence numbers. Never key material.
* **Default OFF.** With the knobs unset both modules and every call site vanish and both arches are
  byte-identical. Unlike `bt`/`btc`, `holocron` and `hcronst` are **not** stripped by `arm_features`:
  the seam emits real aarch64 code, so stripping would silently disarm the knobs rather than preserve
  a byte-identity that does not apply. That claim is now under type-check on both arches — see §28.7
  item 1.
* **No boot-time write without asking for one (HCR1).** `holocron` alone touches the medium only when
  a record is actually staged; the two selftests that write at boot are behind `hcronst`, by the same
  convention that gives `sdw` a knob apart from `sdhcblk` (§28.2a).
* **Never fewer than one valid generation on the medium (HCR1).** The flush stages, verifies and swaps
  rather than overwriting; a refused image is quarantined rather than destroyed (§28.3a).

---

## 29. KBDFLAP — a stalled interrupt endpoint was retired for the boot, anonymously (2026-08-21)


### The observation

rMBP metal boot 10 (capture `rmbp2-boot8`, 2026-08-19). The internal keyboard delivered zero key
events across a four-minute boot while the trackpad — the other interface of the **same device**,
addr 8 — streamed the whole time. It read as the "armed and silent" class §10's KBDWIT probe exists
to adjudicate. It was not that class. One line in the capture decides it:

```
[   2444ms] :: EHCI-HID: [1] STOP-NOTE interrupt endpoint halted (token 0x000a8d42)
            — endpoint retired, not forced ::
```

`0x000a8d42` decodes (EHCI 1.0 §3.5.3) to Total-Bytes 10, PID IN, IOC 1, **CERR 3**, status `0x42`
= Halted + SplitXState. Total-Bytes 10 is `mps=10`, and the only 10-byte endpoint armed in that
boot is `addr=8 ep=IN3` — the internal keyboard. Its non-status half is byte-identical to the live
token KBDWIT printed for the same endpoint on the neighbouring boot 9 (`seen=0x000a8d80`), so the
identity is measured, not inferred. The keyboard was retired 621 ms after arming.

That retirement is also why the endpoint was **absent** from boot 10's KBDWIT dump while three
healthy endpoints appeared: `service()` skips a `dead` entry before the probe can run. The
instrument built for this symptom was structurally silent on the boot that showed it.

### Why the line could not say so

It carried a raw token and nothing else — no address, no endpoint number, no kind. Every rMBP boot
in the corpus prints two such halts:

```
STOP-NOTE interrupt endpoint halted (token 0x00088141)   <- 05ac:820a, the BT HID proxy keyboard
STOP-NOTE interrupt endpoint halted (token 0x00048141)   <- 05ac:820b, the BT HID proxy mouse
```

`0x...8141` is **CERR 0**, status `0x41` = Halted + ERR: three consecutive transaction errors, the
TT answering ERR. Neither of those endpoints has produced a report in any capture in the corpus, so
retiring them is correct and is unchanged. Because two of them print every boot, the reader's prior
is "the usual two", and the one boot where a third named the real keyboard read exactly like them.

### The two halts are different faults, and the token already said which

CERR unburned with no error bit set is the signature of a **STALL handshake**: the controller
stopped because the device said stop, not because the wire failed. USB 2.0 §9.4.5 defines exactly
one recovery for that — `ClearFeature(ENDPOINT_HALT)`, which also resets the device-side data
toggle to DATA0 — and the interrupt path did not have it.

| metal token  | CERR | status | class           | recoverable |
|--------------|------|--------|-----------------|-------------|
| `0x000a8d42` |  3   | `0x42` | `stall`         | yes         |
| `0x00088141` |  0   | `0x41` | `xact-err-burn` | no          |
| `0x00048141` |  0   | `0x41` | `xact-err-burn` | no          |

Classification order is the semantics: babble, data-buffer error and missed-microframe are tested
first — each is a host/bus fault that can coexist with the Halted bit and none is answered by
clearing a device-side stall; the transaction-error arm then absorbs XactErr, the split ERR
handshake, and `CERR == 0`; only Halted with the counter intact and no error bit anywhere is a
stall.

### What landed

Three things, all inside `drivers/ehci`:

1. `halt_class()` — classify the halted token and say whether §9.4.5 is the answer to it.
2. The STOP-NOTE now names addr, endpoint, kind, mps, class, every decoded status bit, the report
   count, the clear budget and its own verdict word. The `STOP-NOTE interrupt endpoint halted`
   prefix is preserved so existing `awk` filters still match; the anonymous `(token …)` form is
   gone from the media.
3. Bounded recovery for `class=stall` only: at most `HALT_CLEARS_MAX` (2) clears per endpoint per
   boot, `ClearFeature(ENDPOINT_HALT)` followed by re-arm at **DATA0**.

**Ordering is load-bearing.** The clear is deferred past the `int_eps` borrow (a control transfer
needs `&mut self`, the same shape ALLKEYS P1's `led_pushes` uses) and must reach the *device*
before the overlay is rewritten: a fresh `QTD_ACTIVE` clears the QH's Halted bit while the device's
stall condition survives, which is the trap `BtL3State::stopped` already documents. The re-arm
starts at DATA0 because the clear reset the device's toggle; continuing the stream's toggle would
have the first post-recovery packet discarded as a retransmission, and the endpoint would read
silent a second time.

A refused clear, or an exhausted budget, retires the endpoint exactly as before, held-key flush
included. Two clears, not one and not unbounded: one is indistinguishable from "retry once and
hope", and unbounded buys a control transfer on every pass of the ~1 kHz service loop for the rest
of the boot.

### What is not convicted

**Why the device stalled is not decided by this capture, and no claim is made.** The intake
hypothesis — a single keyboard slot the two armed keyboards collide over — is refuted:
`MAX_INT_EPS` is 6, four endpoints were armed, both keyboards took distinct slots, and boot 10's
own KBDWIT dump shows the QH chain intact. The ordering story is refuted too: boot 9 and
`gr23-bootAR`'s second run both arm the same two keyboards 80 ms apart in the same order without
stalling.

What is convicted, and fixed, is that a transient stall became a dead keyboard for a whole boot,
under a line that could not name which endpoint had died.

### Reading the next boot

`class=stall` followed by `HALT-CLEAR … -> ok — re-armed DATA0` and subsequent traffic means the
hiccup was recovered. `-> REFUSED — endpoint retired` convicts the device side without another
sitting. `class=xact-err-burn` on `05ac:820a` / `05ac:820b` is the routine per-boot pair and is not
a fault to chase.

---

## 30. BT-RELEASE2 — the refused teardown, and the lever the stack did not have (`UNAOS_BT=1`, 2026-08-21)

### The observation

rMBP metal boot 11 (capture `rmbp3-boot11`). BT-L3 opened an LE link and could not close it:

```
[ 2350ms] bt-l3: LE Connection Complete — status=0x00 handle=0x0040 role=MASTER(initiator)
          peer=88:c6:26:cc:2d:3c/public interval=0x0027 supervision_timeout=1000ms -> CONNECTED
[ 2350ms] bt-l4: ACL OUT2 SENT 15/15 bytes  (ATT_READ_BY_TYPE_REQ, Battery Level)
[ 2950ms] bt-l4: L4 tally — acl_packets_with_data=0 ... answered=false
[ 2950ms] bt-l3: HCI_Disconnect (0x0406) SENT — handle=0x0040 reason=0x13
[ 2953ms] bt-l3: HCI_Disconnect -> CommandStatus status=0x12 -> REFUSED. THE CONNECTION IS STILL
          LIVE and this arc has no second lever for it
[ 2953ms] bt-l3: L3 tally — events_read=5 ... disconnections_confirmed=0
          left_outstanding=A LIVE CONNECTION — THIS IS THE MUST-NOT-APPEAR CONDITION
```

`0x12` is Invalid HCI Command Parameters. The link was then held for the remaining ~180 s of the
boot, with the whole classic stage (§26, §27) running on top of it.

### What the capture refutes, byte for byte

The five candidates, and what boot 11's own bytes do to them:

- **DMA corruption of the staged command block — REFUTED.** `bt_acl_txn` leaves its bulk-IN qTD
  ACTIVE on the `nodata` path with `overlay[3] = data_buf_phys`, and that is the *same* EP0 data
  buffer `bt_hci_send` stages `HCI_Disconnect` into; the ASS handshake in `bt_acl_txn` exists for
  exactly this race. It did not fire here, and it could not have: an ACL-IN DMA fills `data_buf`
  **from offset 0**, so it would clobber the two opcode bytes first — and the Command Status this
  host matched on required `pkt[4..6] == 0x0406`. The opcode echoed back intact. Whatever the
  controller disliked, it was in the three parameter bytes, not in the packet's head.
- **Handle byte order or width — REFUTED.** `[handle as u8, (handle >> 8) as u8]` on `0x0040` is
  `[40 00]`, correct little-endian; the same convention produced the 25-byte `Create_Connection`
  block the controller *accepted* three seconds earlier, and BT-C1's `page bytes` line shows the
  arc rendering LSB-first correctly on the same boot. `0x0040` is inside the Core range
  `0x0000..0x0EFF`.
- **Latched from the wrong field — REFUTED.** The Connection Complete decoder and the
  `bt_l3_await` latch both read `pkt[4..6]` of a 21-byte LE meta event, and both printed
  `handle=0x0040`; the disconnect printed the same value back.
- **Sent before the controller finished setting up — REFUTED.** 600 ms and a completed ACL-OUT
  separate the Connection Complete from the disconnect.
- **The reason byte `0x13`, or a handle the controller had already retired — NOT SETTLED, and the
  capture cannot settle them.** Both are on the table and the boot carried no evidence either way,
  because `events_read=5` names three events and the teardown's waits **walked past two more
  without recording them**. One of those two may well have been the `Disconnection Complete` for
  handle `0x0040`: BT-L4 sat for 600 ms with the event endpoint unread while an ATT response never
  came, and anything the controller emitted in that window queued up behind it.

That is the finding. The arc had no way to distinguish "the reason byte was rejected" from "the
handle was already gone", and no lever to try either.

### The mechanism, and the fix (`drivers/ehci/mod.rs`)

`HCI_Disconnect` carries exactly two parameters. The teardown is now a discrimination over them,
and no step guesses:

1. **`BtL3State::disc_seen` — the latch for the other end of the link's life.** The exact sibling
   of `live_handle` (§22): any `Disconnection Complete` (0x05) with status 0x00 that *any* wait
   walks past is latched with its handle and reason. It is consulted **before a single byte is
   sent**. A link the peer ended is a link that is down, and issuing a disconnect on a retired
   handle is itself a plausible reading of `0x12`.
2. **`HCI_Read_RSSI` (0x1405) as a handle probe.** Two parameter bytes, one Command Complete, and
   — the reason it is admissible under the arc's standing "no unrequested radio operation" rule —
   **it transmits nothing**: Vol 4 Part E §7.5.4 makes it a read of the controller's own record of
   the last packet received on that handle. Status `0x02` (Unknown Connection Identifier) means the
   controller has no such handle, so **no link is held**; status `0x00` proves the opposite, and
   with it that the refused disconnect's handle was legal. Boot 11's
   `HCI_Read_Local_Supported_Commands` reply carries `cmds[15]=0xfe` and Read_RSSI is octet 15
   bit 3, so this controller advertises it — the code does not rely on that, and treats `0x01`
   (Unknown HCI Command) as "the probe is unavailable here".
3. **One retry with reason `0x05` (Authentication Failure)**, run only when the probe found the
   link live. `0x05` is on the same short list the Core spec permits a host to send
   (0x05, 0x13-0x15, 0x1A, 0x29, 0x3B), so the retry varies **exactly one byte** against a control
   that has already been established. Accepted where `0x13` was refused ⇒ the reason byte was the
   invalid parameter, and the link is released in the same round trip.
4. **The teardown event trail.** A bounded record (`BT_L3_TRAIL_MAX` = 8 codes, saturating count)
   of what the teardown's waits stepped over, printed on any teardown that did not go cleanly. It
   is what boot 11 was missing.

`bt_l3_disconnect` keeps its `bool` return, and it still means only one thing — *a
`Disconnection Complete` with status 0x00 for this handle was observed* — so
`disconnections_confirmed=` cannot start meaning two things. The richer verdict rides on
`BtL3State::release` (`Confirmed` / `HandleGone` / `StillLive` / `Inconclusive`), and `HandleGone`
is the one that changes an outcome: it clears `live` on both the LE (§22) and classic (§26) paths,
so the tally stops reporting a live connection the controller does not have, and `bt_left_link`
(§25) stops latching a dead handle.

**Cost:** `0` on any boot that lets go the first time. At most `BT_L3_CMD_MS` (probe) +
`BT_L3_CMD_MS` + `BT_L3_DISC_MS` (retry) = **+1.2 s**, paid only by the boot that is currently
leaking a link for its entire life.

### Does the held link explain the speaker never pairing?

**On this host's side, no — and the capture is unambiguous about it.** With the LE link believed
live the whole time, the controller *accepted and ran to completion* both classic operations:

```
[ 2956ms] bt-c1: HCI_Inquiry (0x0401) -> CommandStatus status=0x00 -> ACCEPTED
[ 8079ms] bt-c1: Inquiry Complete (0x01) status=0x00 — the controller ran the full 5120ms
[ 8082ms] bt-c1: HCI_Create_Connection (0x0405) -> CommandStatus status=0x00 -> ACCEPTED
[13206ms] bt-c1: observed=5124ms deadline_in_force=5120ms (100% of it) ... status=0x04 PAGE TIMEOUT
```

A controller blocked by a live LE link answers `0x0C` (Command Disallowed) or returns early. This
one inquired and paged for 100 % of both deadlines. **The held LE link does not block inquiry or
page on the BCM20702.**

**On the peer's side it is the leading suspect, and the capture cannot rule it out.** Two facts
push against the working assumption that `88:c6:26:cc:2d:3c` is an LE-only mouse and therefore an
irrelevant bystander:

- its advertisement's **Flags byte is `0x1A`** — `02 01 1a` in the raw AD — and bit 2
  (*BR/EDR Not Supported*) is **clear**, with bits 3 and 4 (*Simultaneous LE and BR/EDR*, controller
  and host) both **set**. That is a dual-mode device advertising itself as one. An LE-only mouse
  advertises `0x06`.
- `03 03 61 fe` is a Complete 16-bit Service UUID list containing **0xFE61 = Logitech**, and
  Ultimate Ears is a Logitech brand — so the UUID is consistent with the MEGABOOM as well as with
  a Logitech mouse.

So the ordering the arc runs is wrong-shaped for a dual-mode target: BT-L3 takes an LE link to the
target at 2.35 s and **never lets go**, and BT-C1 then spends 5.1 s inquiring for it and 10.2 s
paging it while that link is held. A dual-mode peer with an active LE link from this host is under
no obligation to keep inquiry-scanning or page-scanning for it — and the inquiry heard **zero
responses of any kind** in 5120 ms with all three result shapes unmasked (bits 1, 33 and 46),
which is a silence worth explaining.

The fix is also the experiment. There are exactly two readings and one boot separates them:

- **The leak was the cause.** With the teardown confirmed, BT-C1's inquiry runs with no LE link
  held; the target answers the inquiry, the page is clock-aligned, and a link forms.
- **The leak was a bystander.** The inquiry still returns `inquiry_responses=0` and both trains
  still time out — in which case the remaining suspect is receive sensitivity, which the LE side
  already hints at (one device, 5 reports, `-88dBm` across a 500 ms window), and the address
  constant should be re-derived from a classic inquiry rather than from an LE advertisement.

`BTNAME=0` across the whole boot is a consequence of the above, not an independent fault: no
classic link ever formed, so nothing was ever there to name.

**And there is a blunter answer that sits in front of all of it.** The operator identifies
`88:c6:26:cc:2d:3c` as a Logitech M196 — *not* the speaker. `BT_L3_PEER_ADDR` is that constant, it
`DECIDES ALONE` (§22), and BT-C1 pages whatever BT-L3 selected: boot 11's `page bytes` line reads
`params=[3c 2d cc 26 c6 88]`, twice. If that address is the mouse, then **across a 200 s boot with
a speaker powered on and in pairing mode two metres away, this stack never once addressed the
speaker** — and no amount of teardown correctness will pair a device that is never paged. The
teardown leak is real, is fixed here, and is *not* the reason the speaker did not pair on boot 11;
the target constant is. That is a peer-selection question, not a teardown question, and it belongs
to a different arc. The Flags/UUID evidence above is recorded because it does not sit comfortably
with the identification — an M196 should advertise `BR/EDR Not Supported` set, and this advertiser
does not — and settling *which device that address is* is the first thing the next sitting should
do, before any conclusion is drawn from another page timeout against it.

### Reading the next boot — the metal watch list

Confirmations, in the order they can appear:

- `bt-l3: HCI_Disconnect (0x0406) SENT — handle=… reason=0x13 (REMOTE-USER-TERMINATED)
  params=[.. .. 13] = Connection_Handle(2,LE) Reason(1), parameter_total_length=3` — the staged
  block is now in the capture. Byte order is settled from the line itself, with no inference.
- **The clean case:** `Disconnection Complete — status=0x00 handle=… -> DISCONNECTED` followed by
  `L3 tally — … disconnections_confirmed=1 … left_outstanding=none`. That is the fix working.
- **The peer-terminated case:** `LINK ALREADY RELEASED — a Disconnection Complete status=0x00 …
  was WALKED PAST by an earlier wait`, then `disconnections_confirmed=1` with **no HCI_Disconnect
  sent at all**. This convicts the boot-11 refusal as a command aimed at a retired handle.
- **The stale-handle case:** `HANDLE PROBE -> status=0x02 (UNKNOWN CONNECTION IDENTIFIER) — THE
  CONTROLLER HAS NO SUCH HANDLE`, and the tally reading `left_outstanding=none — THE HANDLE WAS
  ALREADY RETIRED`. Same conviction, reached by the probe instead of the latch;
  `disconnections_confirmed` stays `0` **and that is correct** — nothing was released, because
  nothing was there.
- **The reason-byte case:** `HANDLE PROBE -> status=0x00 … THE LINK IS LIVE`, then
  `SECOND LEVER WORKED — the same handle that was refused with reason=0x13 was ACCEPTED with
  reason=0x05`, and `disconnections_confirmed=1`. This convicts the controller's parameter check.

Refutation — what says the fix did not settle it:

- `SECOND LEVER EXHAUSTED — the retry was REFUSED too (retry_status=0x12), on a handle the probe
  called LIVE` plus `left_outstanding=A LIVE CONNECTION`. Both permitted reasons refused on a
  handle proven live exonerates the parameter *values*, and the next discriminator is then the
  command **packet** — the `parameter_total_length` byte or the EP0 data stage that carries it —
  which needs a controller-side read of what it received and is a different arc.
- `teardown event trail — N event(s) walked past …, codes=[…]` is the line to read in either
  direction. `05` in that list means a `Disconnection Complete` went by; `13` is
  Number Of Completed Packets from BT-L4's ACL OUT and is expected.

Gates: `./arroyo check` green both arches (12 cfg legs, `x86-all` carries `bt,btc`) and green again
with `UNAOS_BT=1 UNAOS_BTC=1`. No QEMU leg — QEMU has no Bluetooth controller and could never have
judged this. Reachability was proven with `strings` on the armed builder-path `kernel.elf`.

### The verdicts, flown (BTREGRESS, 2026-08-22) — read this before anything above it

§30 above proposed two experiments and asserted one "blunter answer". **All three are now settled
from captures already on disk. Two of §30's positions are refuted. Nothing above this subsection
should be quoted without it.**

#### 1. "The leak was the cause" — REFUTED. The leak was a bystander.

BT-RELEASE2 shipped in `f66b1480` and flew on `rmbp4-boot13` (2026-08-22, four boots). The teardown
now works exactly as §30's "peer-terminated case" predicted, and the page failed anyway:

```
[ 2958ms] bt-l3: SECOND LEVER NOT NEEDED — a Disconnection Complete status=0x00 handle=0x0040
          reason=0x3e was walked past by this teardown's own waits ... RELEASE CONFIRMED
[ 2958ms] bt-l3: L3 tally — ... disconnections_confirmed=1 ... left_outstanding=none
[ 8085ms] bt-c1: inquiry summary — responses=0 target_found=false ... THE INQUIRY HEARD NOTHING
[19613ms] bt-c1: page summary — attempts_run=2/2 pages_on_air=2 page_timeouts=2 -> NOT REACHED
```

No LE link is held when BT-C1 runs, the inquiry still hears nothing, both trains still time out.
This is §30's own second reading, and it is the one that happened. **The teardown fix is correct
and is not the pairing fix.** The `left_outstanding=A LIVE CONNECTION` condition is closed.

#### 2. "The blunter answer" — that this BD_ADDR is a Logitech M196 mouse — REFUTED, in band, twice.

§30 argued the stack "never once addressed the speaker". The refutation was already sitting in two
captures, printed as three undecoded hex bytes:

```
gr26-bootC  2026-08-11  bt-c1: inquiry result — addr=88:c6:26:cc:2d:3c psrm=0x01(R1)
                              clock_offset=0xac50 class_of_device=240418 event=0x02(standard)
rmbp1-boot1 2026-08-18  bt-c1: inquiry result — addr=88:c6:26:cc:2d:3c psrm=0x01(R1)
                              clock_offset=0x3800 class_of_device=240418 event=0x02(standard)
```

`class_of_device` is printed MSB-first hex with no prefix, so this is **0x240418**: Major Device
Class `0x04` = **Audio/Video**, Major Service Classes with **Audio** (bit 21) and **Rendering**
(bit 18) set. An audio sink. A mouse is Major Device Class `0x05` (Peripheral). These are *classic
inquiry responses* — they came from the BR/EDR controller at the address the page is aimed at, so
they also kill the sibling theory that a dual-mode device pages under an address other than the one
it advertises. Peter's 2026-08-22 ruling says the same; this is the wire agreeing with him seven
days before it was asked. `bt_c1_cod_decode` now decodes this field on the line itself so the next
reader does not have to.

#### 3. The RSSI premise is misattributed — it is an LE measurement, not a BR/EDR one.

The `-86 dBm`/`-88 dBm` readings used to exclude "off" and "out of range" come from `bt-l2`, which
is the **LE scan**, reporting `evt=ADV_IND` on the LE PHY. They prove the peer's LE radio is alive.
They say nothing about BR/EDR inquiry scan or page scan. In the same boots the peer's *BR/EDR*
inquiry response count is `0`. The two are different radio states on the same chip, hundreds of log
lines apart, and reading one as evidence about the other is what kept "range/power" alive as a
refuted-but-repeated claim. The page-summary prose now states this scope explicitly.

#### 4. There is no regression commit. The bisect window is clean and it convicts nothing.

Links were established on exactly two boots, both 2026-08-11: `gr25-bootA` (attempt 2/2) and
`gr26-bootD` boot 2 (attempt 2/2). Diffing the HCI command traces, the entire on-air difference
between the era that linked and the era that has not is **one command**: `HCI_Inquiry` (0x0401),
added by the `bt-page` arc (`1cf20d47`, merge `12529544`, 2026-08-11), together with the two
read-backs `HCI_Read_Inquiry_Mode` (0x0C44) and `HCI_Read_Page_Timeout` (0x0C17), neither of which
transmits. `bt-ssp` (`d8216d1b`, 2026-08-12) is exonerated outright: it issues nothing before a
link exists, and no link has existed since.

**`bt-page` is exonerated too, by same-day controls.** On 2026-08-11, with no inquiry stage in the
image, `gr25-bootB` (two boots), `gr25-bootC` and `gr26-bootD` boot 1 produced **8 page trains and
0 links**. The successes are 2 of 12 blind trains on a single day, not a working baseline. And on
2026-08-18 `rmbp1-boot1` boot 1 ran an inquiry that *heard the target*, paged it with the peer's own
`psrm=R1` and a valid harvested clock offset — and both trains timed out. An inquiry that helps the
page cannot be the thing that broke it.

What did change with the era, on both transports at once: the peer stopped answering. `bt-l4`'s ATT
Battery-Level read reports `answered=true` on 2 of 5 boots on 2026-08-11 and `answered=false` on
**every** boot since (14+). A defect in the BR/EDR page path cannot stop an LE ATT read from being
answered. The BR/EDR page timeout is the controller's own verdict about the air, delivered on the
event endpoint, on a full-length train (`observed=5123ms deadline_in_force=5120ms ... FULL`).

#### 5. What is actually left, and the one boot that settles it

Every candidate this arc has chased is now weakened or dead: peer identity (dead), peer address
(dead), held LE link (dead), power/range (dead — but only by the *inquiry*, not by the LE RSSI),
clock-offset ageing, train phase, and controller train length (all three measured and excluded on
`rmbp4-boot13`'s `CANDIDATE-3 ... FULL` lines).

The survivor is the one thing no outbound page can measure: **the peer's page-scan enable.** A
device that answers an inquiry is *inquiry*-scanning. Inquiry scan and page scan are separate
enables, and "discoverable but not connectable" is an ordinary state for a speaker that is already
bonded or connected elsewhere. Every attempt so far has tested the same direction.

**The experiment is a direction test, and it is one boot.** Make this host page-scannable and let
the speaker connect *inbound*: issue `HCI_Write_Scan_Enable` (0x0C1A) with `0x03` (inquiry scan +
page scan) before BT-C1's outbound attempts, put the speaker into pairing mode, and wait one page
window. The two outcomes are exclusive and neither needs a follow-up question:

- **A `Connection Request` (0x04) event arrives** ⇒ the RF path works in both directions and the
  peer's BR/EDR radio will initiate; the fault is our outbound page train or the peer's page-scan
  enable, and the arc moves to `HCI_Write_Page_Scan_Activity`/train-length work.
- **Nothing arrives, while the same boot's inquiry still hears the target** ⇒ the peer is
  discoverable and not connectable in either direction; the fault is the peer's connectable state
  (bonded/connected elsewhere, or past its pairing window) and no host-side page change will fix it.

⚠ **This needs Peter's explicit go before it is built.** `Write_Scan_Enable` makes the machine
discoverable and page-scannable, which is precisely what BT-C1's existing witness prose says the
arc deliberately does *not* do ("paging out needs none of it, while enabling it would make this
machine discoverable"). It is a new posture, not a tweak, and it belongs behind its own knob.

> **Built as §32 (BT-DIR, 2026-08-25), behind its own `UNAOS_BTDIR=1` knob.** Two of this
> subsection's details did not survive contact and §32 records why: the write goes **after** the
> outbound page train, not before it (before would time-slice the controller against the very train
> this section's evidence is drawn from, destroying the control), and the two outcomes above are
> **three** — a boot whose inquiry heard nothing is `VOID`, not `NEGATIVE`, and recent boots report
> `responses=0`, so VOID is the likely one.

---

<!-- SYNC-FOLD 2026-08-22: trunk added this section as §28 while the rmbp track added §§28-30
     (BT-BOND / KBDFLAP / BT-RELEASE2). Both are EOF appends; the section number is the only
     collision. Trunk's BOTSEQ is renumbered 28 -> 31 because docs/SECURITY.md cites §28.5
     (BT-BOND at-rest posture) and no reference anywhere cites BOTSEQ by number. Body verbatim. -->

## 31. BOTSEQ — the mount runs before the probe matrices (2026-08-21)

The behavioural follow-up BOTCLAIM (e3c466f3) deferred to its own arc.

**The conviction chain (BOTCLAIM, 2026-08-21 pi capture, 3 boots, identical cycle).** On the
bench card reader the bring-up chain (TUR / INQUIRY / READ CAPACITY / READ(10) LBA0) passes every
cycle. The device then wedges INSIDE `service_storage`'s own diagnostic chain — the PIUSB-36/37/38
probe matrices + write selftest that ran between the storage publish and the piusb27 mount —
notably piusb37's read12-lba0 (tag 0x19: data lands, CSW never sent) and its READ CAPACITY (tag 5:
nothing posted). Stop-EP/Set-TR-Dequeue recovery then fails cc=19 (epin=2/epout=3), and the mount
read — issued after the matrices in the same pump pass by construction — lands on dead pipes:
`:: BLK: io-cause op=read-usb lba=0 bot_err=Timeout ::` → no FAT → park → hub-cycle → repeat.

**What changed (sequencing only — no probe deleted, no probe internals touched, bring-up and
recovery untouched).** The bring-up pass no longer runs the matrices inline. It arms
`storage_diag_pending` and returns right after the LBA0 sanity read, printing the witness

```
:: BOT: [botseq] mount-first attempted lba0=ok|err matrices=deferred ::
```

The storage-ready edge was raised inside `bring_up_storage`, so the SAME pass's tail mounts the
volume (`piusb27_service` on the Pi pump, `probe_once` on x86). The first block-layer
`storage_read10`/`storage_write10` issued while the latch is armed sets `storage_postpublish_io` —
proof the mount attempt reached the wire — and the next `service_storage` pass consumes the latch
and runs the exact PIUSB-36/37/38 matrices + write selftest, verbatim, in `storage_diag_matrices`.
Mount verdict before matrices, by construction, on every platform and pump (the pi boot pump's
`service_storage` calls skip the diag branch because no post-publish I/O exists yet).

Why deferral and not the knob option: the diff stays inside `xhci/mod.rs` (no arroyo/Cargo/builder
feature plumbing, no `main.rs` hunks), and the diagnostics still run every boot — QEMU's matrix
and `[usbw]` coverage is preserved one pass (~4 ms) later, so no spec or battery grep moves.

**What the next metal flight settles.** Until now the mount sat at a fixed position AFTER the
probes, so "the wedge follows certain commands" and "the wedge follows sequence position" were
collinear. With the mount first: a boot where `[botseq] ... lba0=ok` is followed by a mounted FAT
and THEN the wedge inside the matrices convicts the probes' command mix (read12/read16/pre-sense/
induced-stall), not sequence depth — and the desktop line gets its filesystem before the reader
dies. A mount that still times out ahead of any matrix reopens the position/depth theory.

## 32. BT-DIR — the direction test: let the peer page **us** (`UNAOS_BTDIR=1`, 2026-08-25)

§30.5 named the one surviving hypothesis and the one boot that settles it. This is that boot's
instrument. Everything §30.5's five subsections established is taken as settled here and is not
re-argued: there is no regression, the peer is `88:c6:26:cc:2d:3c` with `class_of_device=0x240418`
(Audio/Video, Audio + Rendering — an audio sink), the RSSI premise was an LE measurement
misattributed to BR/EDR, and the failure stage is unambiguous — the controller certifies a full
page train (`observed=5123ms` against a `5120ms` deadline it reads back to us) and answers
`Connection Complete status=0x04` PAGE TIMEOUT, both attempts, every boot.

**The question, and why no outbound page can ask it.** Answering an inquiry proves the peer is
*inquiry*-scanning. Page scan is a separate enable (Core Vol 4 Part E §7.3.18), and "discoverable
but not connectable" is the ordinary resting state of a speaker already bonded elsewhere. Every
attempt this project has ever made tested one direction. So: make **this host** page-scannable, hold
one page window, and report what arrives.

### The phase structure is the experiment

A controller with page scan enabled time-slices its idle mode between inbound scan windows and any
outbound page train it is running. Running both at once would degrade the very train four boots have
been measuring and hand back a confounded number — which is why §30.5's "before BT-C1's outbound
attempts" is the one part of that sketch this section overrides.

| Phase | What runs | Scan state |
|---|---|---|
| 1 | The existing `btc` outbound page train, through `bt_c1_tally` and the teardown | **disabled** — behaviourally identical to a plain `btc` boot |
| 2 | `Write_Scan_Enable = 0x03`, read back, hold `BT_DIR_WINDOW_MS` (6400 ms) | enabled (inquiry scan + page scan) |
| 3 | `Write_Scan_Enable = 0x00`, read back, print both | disabled again |

**Phase 1 is proven unperturbed by construction, not by care.** `bt_dir_probe` is the *last
statement* of `bt_c1_page` — after the tally has printed, after the teardown, after the BT-RETRY
latch is written — and every line of it, including its constants and its call site, is
`#[cfg(feature = "btdir")]`. A build without the knob does not contain it, so a `btc` boot is
byte-identical to the boots this arc's control was measured on. That is also the arc's `strings`
control: the `UNAOS_BTC=1` artifact carries 74 `:: bt-c1: [` strings and **zero** `:: bt-dir: [`
strings; the `UNAOS_BTDIR=1` artifact carries nine.

Phase 2 additionally refuses to run at all when phase 1 ended holding a live link or an unresolved
page (`live || outstanding` — both are phase 1's own MUST-NOT-APPEAR conditions), because enabling
page scan on top of either is exactly the overlap being avoided. **The refusal prints**
(`result=SKIPPED-PHASE1-UNCLEAN`): a skip that says nothing is the same silence as a window that
heard nothing, and those two must never be confusable.

### No accept path, and that is scope

The reading wanted is the `Connection Request` **event** arriving. `HCI_Accept_Connection_Request`
(0x0409), `Reject_Connection_Request` (0x040A), `Set_Event_Filter`, SDP, RFCOMM, A2DP, a written
local name and a written Class of Device exist nowhere in this tree, so nothing would meet an
inbound connection anyway. A request arriving and going unanswered — resolved by the controller's
own connection-accept timeout — **is the result, not a bug**.

### The exits, all printed by one line

`bt_dir_probe` emits `BT-DIR RESULT` (the numbers) and `BT-DIR READING` (the conclusion they
license) on **every** path including "nothing arrived". A line that fired only on success could not
tell "no request" from "the code never ran" — the `COMP_REVENANTS` defect, which this repo has now
shipped five times in a week.

| `result=` | Meaning |
|---|---|
| `REQUEST` | A `Connection Request` (0x04) arrived **from `88:c6:26:cc:2d:3c`**. RF works **both** ways and the peer CAN page. The fault is our train (clock offset / page-scan repetition mode) or the peer's page-scan enable in the outbound direction only. The arc moves to `Write_Page_Scan_Activity`/train-length work. |
| `REQUEST-OTHER` | A request arrived from some *other* device, or from one whose event was too short to carry an attributable address. `Scan_Enable = 0x03` makes this host answerable to the whole room and there is no `Set_Event_Filter` in this tree, so this is an ordinary outcome — and it must never print as `REQUEST`, which is the one verdict that redirects the project. It is still a real finding: it proves this host's page scan was genuinely live, which strengthens every other verdict the boot could have reached. A stranger's request does **not** end the window; only the peer's does. |
| `NEGATIVE` | Nothing arrived **and this same boot's inquiry heard the target**. The peer is not connectable in either direction; no host-side page change fixes it and this line of work should stop. |
| `VOID` | Nothing arrived **and the inquiry heard nothing either**. The peer was absent or off: *the test did not run against anything*. **This is the most likely outcome** — recent boots report `responses=0` — and a void run misread as a negative is exactly how a wrong conclusion gets recorded, which is why the word is printed rather than inferred. |
| `SCAN-NOT-ENABLED` | The controller **refused** `Write_Scan_Enable` (nonzero status). Nothing about the peer is learned; the silence is the instrument's, not the radio's. |
| `SCAN-UNREAD` | The write went out on EP0 but no Command Complete came back. Distinct from `SCAN-NOT-ENABLED` because the posture differs: the radio may be page-scannable *right now*. "The write was unread so scan must be off" is exactly the inference this stage refuses to make. |
| `EVENT-ENDPOINT-DEAF` | Page scan was enabled, but the HCI event endpoint stopped being readable, so a request could not have been *observed* even if one was sent. `bt_hci_command` rides EP0, which a halted event endpoint does not touch — so the write can succeed on a boot where nothing could ever be heard. The deafness is this host's. |
| `WINDOW-TRUNCATED` | Page scan was enabled, but the window ended on the re-entry cap rather than the clock: the room was busy enough to spend all 64 re-entries on other traffic. The window was not held to term. |
| `WINDOW-SHORT` | The measured hold came in under three quarters of `BT_DIR_WINDOW_MS`, or could not be measured at all. The uncalibrated-TSC case is the one that matters: `epace_ms` returns `None` **and** `bt_l3_budget` falls back to `hw_wait_budget() / 4` (~0.27 s), so the window really is ~24x shorter than the constant on the same line. |
| `READS-INCOMPLETE` | The window ran, but an event went past that could not be read in full (`st.blind` set during the window, snapshotted at entry so phase 1's blindness is not charged here) — and it might have been the request. A silence not read to term is not a silence. |
| `SCAN-UNCONFIRMED` | Accepted, but `Read_Scan_Enable` did not come back `0x03`. A silence under it is not evidence — though an arriving request still would be, since nothing can page a host whose page scan is off. |
| `SKIPPED-PHASE1-UNCLEAN` | Phase 1 left a live link or an unresolved page. Nothing was written to the radio. |

### The witness discipline, in three specifics

1. **The window is only spent when the instrument exists.** The Command Complete status of
   `Write_Scan_Enable` is checked *before* the wait runs, so a refused write never records a
   6.4-second silence against the peer. The one deliberate asymmetry: an **accepted** write whose
   readback could not be obtained still spends the window, because an arriving request is
   self-certifying.
2. **The write is not assumed to have taken.** `HCI_Read_Scan_Enable` (0x0C19) reads it back and the
   value is printed — the same discipline BT-C1/AGE applies to `Read_Page_Timeout`, for the same
   reason: status `0x00` says ACCEPTED, not IN FORCE. Both the status byte and the readback print
   `none` rather than a placeholder when the wire supplied nothing.

   The readback runs on the **far side** of the window, not between the write and the wait. Review
   caught the original ordering eating the measurement: `bt_hci_command_ex`'s drain has none of
   `bt_l3_await`'s latching — it `continue`s past every event that is not the Command Complete it
   asked for — so a `Connection Request` landing in the readback's drain would have been logged only
   as a skipped `bt-l0` event and the boot could have printed `NEGATIVE` over a request that did
   arrive. Reading `0x03` after the hold is also the stronger statement: the scan was in force
   *across* the window, not merely at its start.
3. **The window is a wall clock, not an event count.** `bt_l3_await` caps at `BT_L3_EVT_MAX` (16)
   events per call, so a busy room can burn the cap on unrelated traffic and return with most of the
   window unspent. Phase 2 re-enters it (up to `BT_DIR_AWAIT_REENTRIES` = 64), each time handing it
   only what remains, so the total is bounded by `BT_DIR_WINDOW_MS` and not by the re-entry count.
   `window_held_ms` on the result line is the measured hold, not the constant. And if the re-entry
   cap *is* what ended the loop, that is `WINDOW-TRUNCATED`, not a silence — the loop tests the
   clock before it tests the cap, so exhausting the cap proves the clock had not expired, and the
   reading needs no timing arithmetic that could go stale.

4. **Phase 3 does not read through a halt.** `BtL3State::stopped` makes this a rule, not a
   preference: once latched, a later `bt_read_full_event` re-arms and writes a fresh `QTD_ACTIVE`
   overlay, clearing the QH's Halted bit while the *device's* STALL is untouched. `bt_read_full_event`
   has no self-guard — the guard lives in the callers, as it does in `bt_l3_disconnect` and the C1
   cancel path. So when the event endpoint is dead the restore goes out with `bt_hci_send` (EP0,
   which a halt does not touch) and its reply is deliberately unread; the line reports
   `SENT BUT UNVERIFIABLE` rather than claiming a confirmation it cannot have — or claiming
   `NOT RESTORED` about a write that almost certainly took.

The ordering of the verdict chain is the standing law *an absence is only evidence if the thing that
would have produced it was actually attempted*, applied **six** times before the peer is ever blamed:
a refused write, an unread write, a request from the wrong device, a deaf event endpoint, a
truncated or short window, and reads that did not complete are each eliminated ahead of `VOID` and
`NEGATIVE`. Five of those six rungs came out of the arc's adversarial review, which is the honest
record: the first draft would have printed `REQUEST` for a neighbour's phone and `NEGATIVE` after a
window that listened for zero milliseconds.

`BT_DIR_WINDOW_MS` = 6400 ms is five page-scan intervals at the controller's reset default (0x0800
slots x 0.625 ms = 1.28 s), and the actual interval is **read** by `HCI_Read_Page_Scan_Activity`
(0x0C1B) with `intervals_covered=` printed, so a controller running a longer interval makes the
shortfall visible instead of silent. Five is sized against the *other* side: a peer paging us runs
its own train under its own page timeout, commonly the 5.12 s default, and a shorter window could
close mid-train — manufacturing the exact false negative this arc exists to avoid.

### Posture restoration is hygiene, not safety

Phase 3 is unconditional once phase 2 was attempted — **including after a refused write**, because
"the write was refused so scan must still be off" is an inference and this stage does not trade in
those. It writes `BT_DIR_SCAN_OFF` (0x00), reads it back, and prints both values:

```
:: bt-dir: [N] PHASE 3 POSTURE RESTORED — wrote scan_enable=0x00 status=0x00 readback_status=0x00
   readback=0x00 -> RESTORED AND CONFIRMED — the radio is back in the posture the next boot's
   control assumes
```

`readback_status` is printed apart from `readback` because "the controller answered with a nonzero
status" and "the controller answered nothing" are different facts that would otherwise both render
as `readback=none` — and "the posture is restored" is precisely the claim that must not rest on two
different failures printing identically. Every status field on every `bt-dir` line follows this
rule: the buffers are zeroed locals, so a silent command would otherwise read back a `0x00` success
the wire never supplied.

The reason is not the radio's safety. A machine left in a different radio state than the one the
*next* boot's control assumes is a contaminated control, so a restore that is accepted but
unconfirmed, or refused outright, says so in those words on its own line and the result line carries
`restore_confirmed=`.

### What QEMU proves, and what it does not

**QEMU has no Bluetooth radio.** `UNAOS_WC=1 ./arroyo test` proves the build and the boot are intact
and **nothing** about the link: the test artifact carries zero `bt-c1`, zero `bt-dir` and zero
`bt-l0` strings, because the test path arms neither knob. This section's exits can only be read off
a bench boot with `UNAOS_BTDIR=1`, the speaker in pairing mode, and the peer in the room.

## See also
- `unaos/crates/kernel/src/drivers/xhci/`, `drivers/block.rs` — the implementation.
- `unaos/crates/kernel/src/drivers/ehci/`, `drivers/ehci_scout.rs` — the EHCI-3 HID driver (§10) and the EHCI-1/2 scout + shared wake (§9/§9a).
- [`scheduler.md`](../02_KERNEL_CORE/scheduler.md) — why the lock-free MSI handler and the main-loop service split matter under a live scheduler.
