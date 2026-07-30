# SD Host Controller (SDHCI) — x86

The 2012 rMBP has a built-in SD card reader. Until SDHC-1 the x86 kernel could not
see it, and the reason was structural rather than accidental: `PciScanner::scan()`
matched exactly one PCI class triple and returned on the first hit, so the only
block device this architecture could reach was a USB mass-storage LUN behind that
one controller.

Sections 1–5 record **SDHC-1 milestone 1**: the enumeration widening and the
read-only discovery witness. Milestone 1 deliberately stops at discovery — it
programs nothing and transfers nothing.

Section 6 records **SDHC-2 milestone 2**: claiming and driving the controller
through card identification to a verified single-block read. Milestone 1's lines
are still emitted first and unchanged, so the two logs stay comparable.

---

## 1. What the enumeration used to be

`drivers/pci.rs`, `PciScanner::enumerate_buses()`:

```rust
if class_code == 0x0C && subclass == 0x03 && prog_if == 0x30 {
    return Some((Self::get_bar_address(bus, device, func), bus, device, func));
}
```

Class 0x0C / subclass 0x03 / progIF 0x30 is xHCI, and the `return` ends the walk.
Storage-class functions — mass storage (class 0x01) and SD host controllers
(class 0x08 / subclass 0x05) — were never examined, never reported, and never
reachable.

## 2. The widening (SDHC-1)

`PciScanner::storage_inventory()` is a **separate pass** over the same bus/slot/
function space. `enumerate_buses()` is untouched, so xHCI discovery is
byte-identical in behavior; the storage census is additive.

It reports one line per storage-class function:

| Match | Inventory tag | Status |
| --- | --- | --- |
| class 0x08 / subclass 0x05 | `sdhci` | SD Host Controller — the SDHC-1 target |
| class 0x01 / subclass 0x00…0x08 | `scsi` `ide` `raid` `ata` `sata` `sas` `nvm` | inventoried for the record; no driver on this arch |
| class 0x01 / other subclass | `mass-storage` | inventoried for the record |

`storage_inventory()` returns the class-0x08/0x05 functions in discovery order for
the SDHC probe. It issues **no config-space write**: BAR0 is read as the firmware
left it and is never *sized* (sizing needs the write-all-ones / restore dance on
the BAR register), and no COMMAND bit is touched.

Census line format:

```text
[PCI-STOR] storage-class census (class 0x01 mass-storage, class 0x08/0x05 SDHCI)...
[PCI-STOR] bdf B:S.F VVVV:DDDD class=CC sub=SS progif=PP (kind) bar0=0xADDR
[PCI-STOR] no SD host controller (class 0x08/0x05) on this machine    # when none matched
```

## 3. The milestone-1 witness (`drivers/sdhc.rs`)

`drivers::sdhc::probe()` is called from `arch::x86_64::pci::init()` **after** the
GPU dispatch (paging and the frame allocator that the MMIO mapping needs are long
up) and **before** the network-controller block — that block `return`s early on a
non-Intel NIC, which on the 2012 rMBP (Broadcom Wi-Fi) would otherwise swallow the
witness entirely.

For each SD host controller it maps BAR0 Uncacheable via
`arch::memory::map_mmio_window(bar0, 0x1000)` — the same seam the GPU drivers use
for their BARs — and reads four registers:

| Offset | Register | What it answers |
| --- | --- | --- |
| `0xFE` | Host Controller Version (16-bit) | which SDHCI specification version, plus the vendor version |
| `0x40` | Capabilities | base clock, max block length, voltage support, DMA/high-speed/64-bit, slot type |
| `0x44` | Capabilities_2 | UHS-I modes, driver strength, clock multiplier (raw) |
| `0x24` | Present State | is a card in the slot right now, is card-detect stable, write-protect pin |

Specification Version Number (bits 7:0 of `0xFE`) decodes as `0x00`→1.00,
`0x01`→2.00, `0x02`→3.00, `0x03`→4.00, `0x04`→4.10, `0x05`→4.20; anything else is
reported as `unknown` rather than guessed.

### Read-only, and why that matters

The module issues **no MMIO write of any kind**: no software reset, no clock or bus
power programming, no voltage switch, no command, no DMA, no interrupt enable. It
also does not enable the function's memory decode. If the firmware left decode off
or BAR0 unassigned, the honest result is to say so and stop — which the probe does,
with a named reason. A controller the firmware left quiescent is therefore observed
exactly as quiescent, so the next sitting's serial log is evidence about *the
machine*, not about anything the kernel did to it.

### Witness format

One block per SDHCI function, all lines prefixed `[sdhc]`:

```text
[sdhc] bdf B:S.F VVVV:DDDD bar0=0xADDR cmd=0xCCCC mem-decode=1
[sdhc] bdf B:S.F hcver=0xVVVV spec=3.00 vendor-ver=0x00
[sdhc] bdf B:S.F caps=0x........ caps2=0x........ base-clk=NNMHz max-blk=512 v3.3=1 v3.0=0 v1.8=1 sdma=1 adma2=1 hispeed=1 64bit=0 slot=removable
[sdhc] bdf B:S.F present=0x........ card-inserted=1 cd-stable=1 write-protected=0
```

The probe stops after the first line, with exactly one of these, when it cannot
proceed:

```text
[sdhc] no SD host controller found (class 0x08/0x05)
[sdhc] bdf B:S.F bar0 unassigned by firmware — no MMIO probe
[sdhc] bdf B:S.F bar0 is an I/O BAR (io=0xADDR) — MMIO probe skipped
[sdhc] bdf B:S.F memory decode OFF (cmd=0xCCCC) — MMIO probe skipped (read-only: we do not enable it)
```

## 4. QEMU coverage

QEMU's generic `sdhci-pci` reports the *same* class triple as the rMBP's reader
(class 0x08 / subclass 0x05), so the probe has real coverage in the x86 headless
gate. `builder/src/main.rs` attaches it **default-on**, last in the device list so
no existing device's PCI slot assignment moves:

```
-device sdhci-pci,id=sdhci0
-drive if=none,id=sdcard0,format=raw,file=target/sdcard.img
-device sd-card,bus=sd-bus,drive=sdcard0
```

Notes:

- QEMU names `sdhci-pci`'s child bus plainly `sd-bus` (`hw/sd/sdhci.c`), **not**
  `<id>.sd-bus`. Getting this wrong makes QEMU refuse to start at all, which
  presents as an empty serial log rather than a failing test.
- `target/sdcard.img` is a blank 16 MiB file, created on demand. Power-of-two size
  is a QEMU `sd-card` requirement. Milestone 1 transfers no data; the card exists
  so the present-state witness is not trivially empty.
- **`UNAOS_NOSDHCI=1`** opts out (no controller, no card). This is a QEMU *argument*
  knob only — no kernel feature — so the media is byte-identical whichever way it
  points. Mapped in both `unaos/arroyo` and `builder/src/main.rs`.

### Observed on the gate (QEMU 10.2, `pc-q35-10.0`)

```text
[PCI-STOR] storage-class census (class 0x01 mass-storage, class 0x08/0x05 SDHCI)...
[PCI-STOR] bdf 0:5.0 1b36:0007 class=08 sub=05 progif=01 (sdhci) bar0=0x81085000
[PCI-STOR] bdf 0:31.2 8086:2922 class=01 sub=06 progif=01 (sata) bar0=0x0
[sdhc] bdf 0:5.0 1b36:0007 bar0=0x81085000 cmd=0x0007 mem-decode=1
[sdhc] bdf 0:5.0 hcver=0x2401 spec=2.00 vendor-ver=0x24
[sdhc] bdf 0:5.0 caps=0x057834b4 caps2=0x00000000 base-clk=52MHz max-blk=512 v3.3=1 v3.0=0 v1.8=1 sdma=1 adma2=1 hispeed=1 64bit=0 slot=removable
[sdhc] bdf 0:5.0 present=0x01ff0000 card-inserted=1 cd-stable=1 write-protected=0
```

The class-0x01 line is the widening proving itself on a device nobody was looking
for: the q35 machine's ICH9 SATA controller, previously invisible to this kernel.

Negative control (`UNAOS_NOSDHCI=1`):

```text
[PCI-STOR] bdf 0:31.2 8086:2922 class=01 sub=06 progif=01 (sata) bar0=0x0
[PCI-STOR] no SD host controller (class 0x08/0x05) on this machine
[sdhc] no SD host controller found (class 0x08/0x05)
```

## 5. What milestone 1 does not answer — and how it was answered

QEMU-green is not hardware. The open question this witness exists to settle on the
bench is whether the 2012 rMBP exposes its reader as a PCIe SDHCI function at all
(as opposed to behind an internal USB bridge, which is how some Apple readers of
that era are wired), and if so at which spec version and base clock. That decides
what a milestone-2 bring-up sequence has to look like. Read the answer off the
`[PCI-STOR]` / `[sdhc]` lines of the next attended rMBP boot; if `[PCI-STOR]`
reports no class-0x08/0x05 function, the reader is not on PCI and the SDHCI arc
should stop there.

### The go/no-go, and the exact standing of the evidence

The GR9 brief records that a GR8 bench census found the reader as a **PCIe SDHCI
3.00 controller at `14e4:16bc`**, with ADMA2 and 64-bit addressing. `14e4:16bc`
is a Broadcom BCM57765-family SDXC/MMC card reader, which is consistent with the
part in this machine. That is a **go** for milestone 2.

It is recorded here with its provenance, because the standing of that evidence
matters: **no serial log carrying those lines is committed to this repository**,
and the string `16bc` appears nowhere in the tree. The go therefore rests on a
bench observation reported through the brief, not on an artifact this document
can point at. Two consequences, both honoured by the milestone-2 driver:

1. **Nothing is hard-coded from it.** The driver matches on class 0x08/0x05, never
   on `14e4:16bc`; it reads the spec version and drives the divider encoding that
   version implies; it takes the base clock, the supported bus voltages, and the
   addressing mode from the hardware on every boot. If the reported capabilities
   are wrong, the driver still does the right thing — or stops with a named reason.
2. **The next attended boot re-establishes the evidence.** The `[sdhc] raw …` line
   prints the version and capability words verbatim before anything is decoded from
   them, so the bench log becomes the artifact this section currently lacks.

---

## 6. Milestone 2 (SDHC-2) — driving the controller

Milestone 2 turns the read-only observer into a driver. It claims the function,
resets the controller, programs bus power and the SD clock, runs the card
identification ladder, and reads one block — verified. Milestone 1's discovery
lines are all still emitted first and unchanged, so a milestone-2 log is directly
comparable with a milestone-1 log up to the `claim` line.

The strategic point: this path reaches a card **without xHCI and without USB
Bulk-Only Transport**. None of the ring, phase-desync, or rescue-ladder machinery
of the previous two rounds sits underneath it.

### 6.1 Scope held on purpose

| Held | Why |
| --- | --- |
| **PIO only; Bus Master left as firmware set it** | No DMA is issued, so the controller never needs to master the bus. Enabling it would grant a capability nothing here uses. |
| **1-bit bus, default speed (≤25 MHz)** | A 4-bit bus needs ACMD6 and a Host Control 1 change; high speed needs CMD6. Neither is required to read a block. |
| **No `drivers::block` registration** | The x86 block layer has **no backend selector** — `publish_usb_geometry` claims the global `BLOCK_DEVICE` unconditionally — so a card registered there would silently fight the USB stick for that slot. This is the PI-FS-2 clobber the aarch64 side already documents, in reverse. Geometry is published through `sdhc::card_num_blocks` / `sdhc::read_block_512` instead. Wiring it in needs an arc that gives x86 a real selector. |
| **No ADMA2, no multi-block** | See §6.6. |

Registers are named and accessed at their **spec-defined widths** (8/16/32), not
through the Broadcom 32-bit combined views `drivers::emmc2` uses. Those work on the
Pi because the VideoCore wrapper serves them; a generic SDHCI part is only required
to honour the widths the spec assigns.

### 6.2 The two fields that are traps

The witness rule exists because this project has convicted six instrument lies, each
a field read at a moment the hardware defines it as meaningless. This controller
offers two more, and the driver's ordering is built around them:

- **Card Inserted** (Present State bit 16) is **undefined while Card State Stable**
  (bit 17) reads 0 — and *Software Reset For All restarts the debounce*, so the
  reading taken immediately after the reset is exactly the meaningless one.
  `wait_card_detect` waits for the stable bit first, and reports a debounce that
  never settles as **its own outcome** rather than collapsing it into "no card".
- **Internal Clock Stable** (Clock Control bit 1) reads 0 on a healthy controller
  whose internal clock has not been enabled — bit-for-bit what a dead one reads. It
  is only polled after Internal Clock Enable has been written.

A third, less obvious: **Interrupt Status Enable must be armed before any polling.**
With it at 0, every Interrupt Status poll reads a permanent 0, which makes a working
controller indistinguishable from a dead one. Signal Enable stays 0 — this driver is
polled, not interrupt-driven.

The response registers are likewise only valid after Command Complete; before that
they hold the *previous* command's response, which is the most convincing kind of
wrong answer.

### 6.3 Clock programming carries both encodings

The divider encoding depends on the controller's spec version, and picking wrong
selects a wildly incorrect frequency that presents as "the card never answers":

- **spec 3.00+** — 10-bit divided clock, `SDCLK = base / (2·N)`, N split across
  ClockControl[15:8] and [7:6].
- **spec ≤ 2.00** — the field is **one-hot**: `0x00` = base, `0x01` = base/2,
  `0x02` = base/4, `0x04` = /8, … `0x80` = /256.

QEMU's `sdhci-pci` reports spec 2.00 while the rMBP's reader is expected at 3.00, so
both legs are live paths, not defensive padding.

### 6.4 Addressing is cross-checked, never guessed

A block-addressed card takes an LBA where a byte-addressed card takes a byte offset.
Believing the wrong one does not fail — it reads the wrong 512 bytes, silently,
forever. **CCS (ACMD41 bit 30) is the authority.** It is then cross-checked against
the CSD structure version, which the card derives from the same fact (high capacity
⇒ CCS=1 and CSD v2; standard ⇒ CCS=0 and v1). If they disagree the driver stops
rather than picking one. Two independent registers agreeing is what makes either of
them evidence.

Both addressing modes bound their argument against the 32-bit Argument register
rather than truncating into it: a silently wrapped address reads a real block, just
the wrong one.

### 6.5 Proving the block came from the card

Reading 512 bytes is easy; reading 512 bytes that are demonstrably the addressed
block of the addressed card is the deliverable. Three independent claims:

1. **LBA 0 read twice fingerprints identically** — falsifies FIFO residue and a
   desynchronised drain. Says nothing about *which* block the bytes came from.
2. **LBA 1 fingerprints differently** — falsifies the failure claim 1 is blind to:
   an address argument that never reaches the card, so every read serves the same
   staged buffer. Reported as an observation, not asserted as a pass (a card whose
   first two sectors genuinely match would read the same).
3. **The MBR cross-check** — if LBA 0 carries the `55 AA` boot signature, its
   partition table is an *independently-authored description of the same card*,
   written by whatever formatted it. Every partition extent must fit inside the
   capacity derived from the CSD. An extent past that capacity convicts the CSD
   parse or the addressing mode — the two things otherwise unfalsifiable from
   inside one boot.

A card with no MBR signature cannot supply claim 3; the log says the check is
**UNAVAILABLE on that card** rather than quietly omitting it.

Verification runs through the *public* `read_block_512` after the card is published,
so what it exercises is what a later caller would get.

### 6.6 Where milestone 2 stops, and why

**It stops after the verified single-block read. ADMA2 and multi-block are not
attempted.**

The reason is not effort. Steps 1–4 cannot be established sound without metal: QEMU
was not run this round (by direction), and QEMU would not be a verdict for this path
in any case. Building a DMA engine on top of an unverified PIO path would put two
unproven layers into one log, and a failure would not say which layer it indicted.
ADMA2 is milestone 3, gated on this boot's witness.

### 6.7 The witness a metal boot prints, in order

Healthy path, one controller, card present, MBR-partitioned:

```text
[PCI-STOR] storage-class census (class 0x01 mass-storage, class 0x08/0x05 SDHCI)...
[PCI-STOR] bdf B:S.F 14e4:16bc class=08 sub=05 progif=01 (sdhci) bar0=0xADDR
[sdhc] bdf B:S.F 14e4:16bc bar0=0xADDR cmd=0xCCCC mem-decode=1
[sdhc] claim bdf B:S.F mem-decode already ON (cmd=0xCCCC) bus-master=0 (PIO only)
[sdhc] map bdf B:S.F bar0=0xADDR len=0x1000 uncacheable
[sdhc] raw bdf B:S.F hcver=0xVVVV caps=0x........ caps2=0x........ present=0x........ hostctl1=0xHH pwrctl=0xPP clkctl=0xCCCC
[sdhc] bdf B:S.F hcver=0xVVVV spec=3.00 vendor-ver=0xVV
[sdhc] bdf B:S.F caps=0x........ caps2=0x........ base-clk=NNMHz max-blk=512 v3.3=1 v3.0=0 v1.8=1 sdma=1 adma2=1 hispeed=1 64bit=1 slot=removable
[sdhc] bdf B:S.F present=0x........ card-inserted=1 cd-stable=1 write-protected=0
[sdhc] reset-all srst=0x00 cleared=1 (bound 100ms)
[sdhc] power sel=0x0e pwrctl=0x0f v=3300mV on=1 (healthy-idle pre-power reading is 0x00)
[sdhc] clock mode=10bit base=NNNNNNNNHz target=400000Hz actual=NNNNNNHz clkctl=0xCCCC stable=1 sd-clk-en=1
[sdhc] card-detect present=0x........ cd-stable=1 card-inserted=1 cd-pin=1 wp-switch=1 (bound 100ms)
[sdhc] bdf B:S.F bus ready: powered, 400000Hz identification clock, card present
[sdhc] cmd0 go-idle ok
[sdhc] cmd8 send-if-cond resp0=0x000001aa echo=0x1aa ok (v2.00+ card)
[sdhc] acmd41 ocr=0x........ powered-up=1 ccs=1 (block-addressed) after N polls
[sdhc] cmd2 cid raw=[0x........,0x........,0x........,0x........]
[sdhc] cid mid=0xMM oid=OO pnm=PPPPP prv=N.N psn=0x........ mdt=YYYY-MM
[sdhc] cmd3 resp0=0x........ rca=0xRRRR
[sdhc] cmd9 csd raw=[0x........,0x........,0x........,0x........]
[sdhc] csd v2 c_size=NNNNN -> blocks=(c_size+1)*1024
[sdhc] card NNNNNNNN blocks x512 = NNNNNMiB class=SDHC addressing=block (ccs governs, csd v2 agrees)
[sdhc] cmd7 select ok (transfer state)
[sdhc] cmd16 set-blocklen=512 ok
[sdhc] clock mode=10bit base=NNNNNNNNHz target=25000000Hz actual=NNNNNNNNHz clkctl=0xCCCC stable=1 sd-clk-en=1
[sdhc] transfer clock NNNNNNNNHz engaged
[sdhc] bdf B:S.F CARD IDENTIFIED — NNNNNNNN blocks, block-addressed, csd v2
[sdhc] read lba0 ok (512 bytes)
[sdhc] lba0 head=.. .. .. .. sig=[0x55,0xaa] fnv=0x................
[sdhc] verify repeat lba0 fnv=0x................ match=1 (...)
[sdhc] verify lba1 fnv=0x................ differs-from-lba0=1 (...)
[sdhc] verify mbr p0 type=0xTT start=NNNN count=NNNNNNNN end=NNNNNNNN fits-capacity=1
[sdhc] verify mbr bdf B:S.F: partition extents vs CSD capacity NNNNNNNN blocks -> all-fit=1 (...)
```

**How to read the result without the author present.** The milestone succeeded iff
the log reaches `verify mbr … all-fit=1` with `match=1` and
`differs-from-lba0=1`. Anything short of that stops at a line that **names its own
reason**; the ladder never continues past a failure. The lines worth checking first
if it stops early:

| Last line seen | What it means |
| --- | --- |
| `bar0 unassigned` / `bar0 is an I/O BAR` / `memory decode did not stick` | The function is not claimable; nothing was programmed. |
| `reset-all … cleared=0` | The controller does not answer its own reset — check the raw `caps`/`hcver` line above it for a bus that reads back all-ones. |
| `power: caps … advertise NO supported bus voltage` | The capability word is implausible; compare it against the `raw` line. |
| `clock: Capabilities base-clock field is 0` | The controller does not report its clock and x86 has no second source (no VideoCore mailbox). Milestone 3 would need an alternative source. |
| `card-detect: debounce never settled` | Card presence is genuinely **undefined**, not absent. |
| `bus is up … but NO CARD is inserted` | The bring-up worked; put a card in. |
| `cmd8 … FAILED` | Pre-v2.00 card, or the bus is not carrying the card's answer. This milestone identifies v2.00+ cards only. |
| `MISMATCH: acmd41 ccs=… but csd structure=…` | The two addressing witnesses disagree; do **not** trust any read taken past this point in a future build that ignores it. |
| `all-fit=0` | The CSD parse or the addressing mode is wrong. This is the loudest failure the milestone can produce and it is a real defect, not a bad card. |

### 6.8 Knobs

**None added.** Milestone 2 is default-on with no new environment knob and no new
cargo feature, so there is no way for it to ship disabled with a green gate. Every
hardware wait is bounded in rdtsc units against the TSC the ACPI PM timer calibrated
long before `pci::init`, so a dead or absent controller costs bounded milliseconds
and one named line rather than freezing a serial-less laptop.

`UNAOS_NOSDHCI=1` (§4) is unchanged and remains a QEMU-argument knob only.
