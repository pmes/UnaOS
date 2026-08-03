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
`baremetal`-gated on purpose: on QEMU-virt aarch64 (`test-arm`) and on x86 the SD backend is never compiled,
xHCI **is** the legitimate sole backend, and the function does not exist — those builds are byte-identical
to pre-USBFALL. The rule is about substitution on a platform that has a canonical backend, not about xHCI.

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

## See also
- `unaos/crates/kernel/src/drivers/xhci/`, `drivers/block.rs` — the implementation.
- `unaos/crates/kernel/src/drivers/ehci/`, `drivers/ehci_scout.rs` — the EHCI-3 HID driver (§10) and the EHCI-1/2 scout + shared wake (§9/§9a).
- [`scheduler.md`](../02_KERNEL_CORE/scheduler.md) — why the lock-free MSI handler and the main-loop service split matter under a live scheduler.
