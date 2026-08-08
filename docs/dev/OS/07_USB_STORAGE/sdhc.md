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

## 7. Milestone 3 (SDHC-3) — ADMA2 + multi-block

Milestone 2 stopped at a verified single-block PIO read and said why (§6.6): a DMA
engine stacked on an unverified PIO path puts two unproven layers into one log, and a
failure does not say which layer it indicts. Milestone 3 takes that argument
seriously and applies it to itself. It lands in three rungs, each of which is a
complete, shippable driver:

| Rung | What is new | What is still trusted from below |
| --- | --- | --- |
| **3a** | CMD18 command semantics (multi-block + Auto CMD12) | the FIFO drain, the command layer, PIO |
| **3b** | Bus Master, a descriptor table, ADMA2 | the CMD18 semantics 3a proved |
| **3c** | the A/B verdict and the fallback policy | both of the above |

So if 3b's engine misbehaves on metal, 3a's `match=1` line — printed in the *same
boot*, above it — has already excluded "multi-block is wrong" as the explanation.

### 7.1 Multi-block PIO (the control extended)

`read_blocks_512_pio` issues one CMD18 READ_MULTIPLE_BLOCK for up to
`MB_MAX_BLOCKS` = 64 blocks (32 KiB), with Block Count Enable, Auto CMD12, and the
same block-by-block FIFO drain milestone 2 verified. **No DMA, no Bus Master, no new
grant to the device** — the risk surface added by this rung is exactly the command
encoding and nothing else.

Three things about it are not cosmetic:

* **Block Count Enable is required, not optional.** Auto CMD12 makes the controller
  issue the closing STOP_TRANSMISSION itself; the block counter is what tells it when
  the transfer ended. Enabling the one without the other leaves the card streaming.
* **Auto CMD12 has its own error bit.** Error-half bit 8 (`INT_ERR_AUTO_CMD`, bit 24
  of the combined status word) was already inside `INT_ERR_ANY`'s `0xFFFF_0000` mask,
  so it already convicted; milestone 3 adds it to `int_error_name` so the failure line
  says `auto-cmd12` instead of `other-error`.
* **Every failure path closes the CARD, not just the host.** `abort_data_transfer`
  issues CMD12 and *then* resets the CMD/DAT circuits. The controller-side reset clears
  only the host's circuits; an aborted CMD18 leaves the card in data state, and the
  next command issued to it is rejected for a reason that has nothing to do with that
  command. One failure would become a cascade indicting the wrong step — the exact
  class of misdiagnosis this driver exists to prevent.

The witness, `verify_multiblock`, runs from `bring_up` right after §6.5's
verification. It reads LBA 0–7 as **eight separate CMD17s through the unmodified
public `read_block_512`**, then reads the same eight blocks as **one CMD18**, then
compares them byte for byte:

```text
[sdhc-mb] window lba=0 blocks=8 match=1 first-diff=none ctl-fnv=0x................ mb-fnv=0x................
```

`read_block_512` is byte-for-byte the function milestone 2 verified on metal, and
milestone 3 does not touch it in any rung. That is what makes this a control rather
than two runs of the same code agreeing with each other.

Reading the result:

| Log | Meaning |
| --- | --- |
| `window … match=1 first-diff=none` | CMD18 delivered the same bytes as eight CMD17s. |
| `window … match=0 first-diff=blkN,offM` | They ran and disagreed. The offset is the evidence: `off0` of some block means a lost block boundary, `off ≡ 0 (mod 4)` a lost word. |
| `cmd18 … FAILED int=0x........ (name)` and **no `match=` line at all** | CMD18 never completed. The named reason is on that line; the absence of a verdict line is itself the signal. |
| `control cmd17 lba=N FAILED` | The control is unavailable, so there is no comparison — reported as such, never as a multi-block verdict. |
| `SKIPPED — card holds N blocks` | The card is smaller than the 8-block window. |

The destination buffer is pre-filled with a `0xA5` sentinel before the transfer, so
"the transfer wrote nothing" cannot masquerade as "the transfer matched" — which a
zeroed buffer compared against a zeroed control would.

All waits are bounded in the same rdtsc units as milestone 2: `DATA_TIMEOUT_MS`
(200 ms) per block for Buffer Read Ready, `DATA_TIMEOUT_MS` for Transfer Complete,
and `abort_data_transfer`'s CMD12 and reset are bounded by `CMD_TIMEOUT_MS` and
`RESET_TIMEOUT_MS` respectively. No unbounded spin is added anywhere.

### 7.2 The ADMA2 engine and the DMA contract

This is the first DMA in the driver and the first time the SDHCI function is ever
granted Bus Master. The engine is built to a contract that is stated here in full,
because every clause of it is load-bearing.

**Descriptor table.** One 4 KiB page, `alloc_zeroed(4096, 4096)`, allocated once at
engine init and never freed. A 4 KiB-aligned 4 KiB region cannot cross a 64 KiB
boundary, so the table satisfies ADMA2's boundary rule *by construction* rather than
by a check that could be wrong. 512 slots; no transfer here uses more than three.

**Data buffer.** A driver-owned 32 KiB bounce (64 × 512 = one `MB_MAX_BLOCKS` chunk),
allocated once, never freed. **DMA lands only here**, and is then copied to the
caller. That buys two things: every caller-alignment and caller-lifetime constraint
disappears, and a DMA write that straggles in *after* a transfer was declared timed
out lands inside memory this driver owns forever — never in a caller's buffer that
has since been freed or reused.

**Physical addresses.** On x86_64 the kernel heap is identity-mapped physical RAM
handed to devices as physical == bus addresses — `arch::x86_64::memory` states this
and prints a `HEAP: WARNING` on serial if the heap window itself ends above 4 GiB —
so `ptr as u64` *is* the bus address, the same seam `drivers::xhci::ring` uses. Two
refusals run before the engine is ever called `Ready`:

* **reachability** — address + length must be below 4 GiB, because the ADMA2-32
  descriptor address field and `REG_ADMA_ADDR` are 32 bits. Above it, the address is
  unreachable, and is refused **by name** rather than truncated into a valid-looking
  address that points at the wrong RAM;
* **alignment** — 4-byte, because the field's low bits are address bits, not flags, so
  a misaligned buffer is a *wrong* address rather than a slow one. The 4096-byte
  layout guarantees it; it is asserted anyway, since a guarantee that is never checked
  is an assumption.

Either refusal yields `Unavailable("dma window above 4GiB")` (or `…misaligned`),
**named on serial** — never a silent drop to PIO.

**Descriptor format.** 32-bit ADMA2 (spec 3.00 §1.13): 8 bytes = `attr:u16 |
length:u16 | address:u32`, attributes Valid(0), End(1), Int(2), Act[5:4] = 10b
(Tran). The builder splits the span at every 64 KiB physical boundary **and** caps
each descriptor at 32 KiB — the first satisfies the boundary constraint, the second
keeps every length inside the 16-bit field without ever relying on the
version-dependent "length 0 means 65536" encoding. Only the last descriptor carries
End; no descriptor carries Int, because the driver is polled and a descriptor
interrupt would raise a status bit nothing consumes. Before the transfer is issued the
lengths are summed and checked against `count × 512`; a mismatch refuses, prints, and
returns `Err(Io)`. A chain describing fewer bytes than the block count demands is
precisely what raises ADMA Length Mismatch on real silicon, and catching it here names
the bug instead of reading it back off the wire.

**Cache and ordering — zero fence instructions, and why.** The buffers are Write-Back
kernel heap and the platform is I/O-coherent: CPU caches and the PCIe DMA path snoop
each other. That is the same contract `drivers::xhci::dma_coherency` documents and
ships on for every xHCI ring and data buffer in this kernel — on x86_64 its clean and
invalidate functions are empty bodies that compile to nothing. Ordering falls out of
x86-TSO plus the UC BAR mapping: descriptor stores go to WB memory, the MMIO writes
that follow (ADMA System Address, then the Command register that issues the transfer)
go to the UC window, and a UC store may not pass earlier stores — so the descriptors
are globally visible before the write that makes the controller fetch them. This is
byte-for-byte the seam the xHCI doorbell already relies on. In the other direction the
UC read of Interrupt Status is the ordering point: PCIe producer/consumer ordering
requires a read returning from the device to push the DMA writes it posted ahead of
itself, so once that read reports Transfer Complete, every byte the engine wrote is in
memory. The compiler is a separate matter from the hardware: the bounce buffer is
filled and read through **volatile** accesses, because the compiler sees a region the
driver wrote a sentinel into and no subsequent writer, and would otherwise be free to
fold a plain read back to the sentinel.

**Bus Master.** Enabled at engine init, read-modify-write on PCI COMMAND bit 2 through
the same accessors `claim` uses, with a before/after readback printed. It is
deliberately **not** enabled in `claim`: that keeps every log line up to and including
the claim line comparable across all three milestones, and it means the grant is made
only after the controller has advertised ADMA2 *and* every address check has passed —
if any of them refuse, the function keeps exactly the capabilities milestone 2 gave
it. If the bit does not read back set: `Unavailable("bus-master did not latch")`.

**Failure path.** On `INT_ERR_ADMA`, any other error, or a timeout during a DMA
transfer, one line carries the raw Interrupt Status, the raw ADMA Error Status (`0x54`,
with bits [1:0] decoded to ST_STOP / ST_FDS / ST_TFR), the ADMA System Address read
back from `0x58` — it advances as the engine walks the table, so it names the faulting
descriptor — and the Auto CMD Error word (`0x3C`). Then a bounded CMD12
STOP_TRANSMISSION, `reset_cmd_dat`, and W1C. The CMD12 is not optional: an aborted
transfer leaves the *card* in data state, and the next command would be indicted for
it.

**ADMA2 is gated on `CAP_ADMA2`, read fresh every boot.** The `14e4:16bc` "SDHCI 3.00
with ADMA2 + 64-bit" note that motivated this milestone is a bench observation with no
committed log, so nothing is hard-coded from it: the capability register decides, and
the mode is 32-bit ADMA2 unconditionally so the unverified 64-bit half of that claim
is never load-bearing.

**The in-boot smoke transfer** reads 8 blocks at LBA 0 and prints:

```text
[sdhc-adma] engine ready bm=1 table=0xPHYS buf=0xPHYS descs-max=512 mode=adma2-32
[sdhc-adma] read lba=0 blocks=8 ok wrote=1 bytes=4096
```

Three readings are distinguishable, which is why a verdict is printed rather than a
status:

| Log | Meaning |
| --- | --- |
| `engine ready …` then `read … ok wrote=1` | The engine ran and DMA-wrote the buffer. |
| `UNAVAILABLE (reason)` and no `read` line | The engine never ran, and the line says why. |
| `read … ok wrote=0` | The transfer completed but the engine wrote nothing into the sentinel-filled buffer — the failure a bare `Ok` would have concealed. |

**One structural change reaches back into 7.1.** A read of exactly one block issues
CMD17 with Multi/Single Block Select clear, not CMD18 with a block count of one — in
*both* the PIO and the ADMA2 path. Both forms are spec-legal, but the single-block
form is the one every card and controller in the field is exercised on, and keeping
the rule identical in both paths is what makes §7.3's A/B comparison meaningful: an
A/B test whose two arms issue different commands for the same window measures more
than the variable under test.

### 7.3 The A/B witness and the fallback policy

**The witness.** `verify_adma_ab` reads three windows — head (LBA 0), middle
(`num_blocks / 2`), tail (`num_blocks - 8`) — each of 8 blocks, each **twice**: once
as eight separate CMD17s through the untouched `read_block_512`, once through the
ADMA2 engine. Then it compares them byte for byte.

```text
[sdhc-ab] window lba=0 blocks=8 match=1 wrote=1 first-diff=none
[sdhc-ab] window lba=NNNNNNN blocks=8 match=1 wrote=1 first-diff=none
[sdhc-ab] window lba=NNNNNNN blocks=8 match=1 wrote=1 first-diff=none
[sdhc-ab] verdict windows=3 match=3/3 — adma2 agrees with the pio control byte-for-byte
```

Three windows and not one, because "wrong only for some addresses" is a real failure
mode: a descriptor built from a truncated or mis-shifted address reads the right bytes
near the start of the card and the wrong ones further in. One window at LBA 0 cannot
see that.

On a mismatch the line prints the **first differing block index and byte offset**,
absolute LBA, and both bytes — the evidence, not a summary. `off0` of a block means a
lost block boundary; an offset ≡ 0 mod 4 means a lost word; anything else, a byte.

If ADMA2 is unavailable the witness prints
`[sdhc-ab] SKIPPED — adma2 unavailable (reason)` rather than vanishing. A witness that
disappears when the thing it measures is absent is indistinguishable from a witness
that was never called.

**The fallback policy.** `read_blocks_512` is the module's public counted read: it
chunks at `MB_MAX_BLOCKS`, uses ADMA2 while the engine is `Ready`, and PIO otherwise.
When a live ADMA2 transfer fails, **three lines are printed before this function can
return success**:

1. the transfer's own failure line — raw Interrupt Status, ADMA error state, faulting
   descriptor;
2. `[sdhc-adma] engine DISABLED — falling back to PIO (reason) …`, once, as the engine
   is latched `Faulted` for the rest of the boot;
3. the PIO retry's own result line.

A driver that quietly retried on PIO and returned `Ok` would report a healthy card
while its DMA engine was dead. **Success is never reported without the failure that
preceded it on the wire.** The `Faulted` latch is what keeps this from becoming noise:
the engine is disabled for the boot, not re-failed and re-printed on every read.

### 7.4 The witness a metal boot prints, in order

Extending §6.7. Everything through `verify mbr … all-fit=1` is milestone 2's, byte for
byte and in the same order; milestone 3 appends:

```text
[sdhc-mb] window lba=0 blocks=8 match=1 first-diff=none ctl-fnv=0x................ mb-fnv=0x................
[sdhc-adma] host-control1 0xHH -> 0xHH readback=0xHH dma-select=0b10
[sdhc-adma] bus-master grant bdf B:S.F cmd 0xCCCC -> 0xCCCC bus-master 0 -> 1
[sdhc-adma] engine ready bm=1 table=0xPHYS buf=0xPHYS descs-max=512 mode=adma2-32
[sdhc-adma] read lba=0 blocks=8 ok wrote=1 bytes=4096
[sdhc-ab] window lba=0 blocks=8 match=1 wrote=1 first-diff=none
[sdhc-ab] window lba=NNNNNNN blocks=8 match=1 wrote=1 first-diff=none
[sdhc-ab] window lba=NNNNNNN blocks=8 match=1 wrote=1 first-diff=none
[sdhc-ab] verdict windows=3 match=3/3 — adma2 agrees with the pio control byte-for-byte
```

Note the ordering: the `[sdhc-adma]` engine lines are printed *after* milestone 2's
verification and after `[sdhc-mb]`, because engine init runs after identification and
the witnesses run in rung order. The `claim` line still reports `bus-master=0` — the
grant happens later, in `adma2_init`, which is what keeps that line comparable with a
milestone-1 or milestone-2 log.

**How to read the result without the author present.** Milestone 3 succeeded iff the
log reaches `[sdhc-ab] verdict windows=3 match=3/3` **and** `[sdhc-mb] … match=1`
appears above it. Rows extending §6.7's table:

| Last line seen / line present | What it means |
| --- | --- |
| `[sdhc-mb] cmd18 … FAILED int=… (name)` and no `[sdhc-mb] window … match=` | Multi-block command semantics failed. Everything below is moot; ADMA2 was never the suspect. |
| `[sdhc-mb] window … match=0 first-diff=…` | CMD18 and CMD17 disagree on PIO, with no DMA involved at all. A driver bug in the drain or the block count, not a DMA bug. |
| `[sdhc-adma] UNAVAILABLE (controller advertises no ADMA2)` | `CAP_ADMA2` read 0 this boot. PIO serves every read; compare against the `caps=` survey line above. **Not** a failure. |
| `[sdhc-adma] UNAVAILABLE (dma window above 4GiB)` | The heap landed above the 32-bit ceiling. Check for the `HEAP: WARNING … above 4 GiB` line much earlier in the boot — the identity/<4 GiB premise this driver relies on is broken. |
| `[sdhc-adma] UNAVAILABLE (bus-master did not latch)` | PCI COMMAND bit 2 did not stick. Compare the `cmd 0x… -> 0x…` readback on the grant line. |
| `[sdhc-adma] UNAVAILABLE (dma-select did not latch)` | Host Control 1 bits [4:3] did not take 10b; the controller may not implement ADMA2 despite the capability bit. |
| `[sdhc-adma] read … ok wrote=0` | The transfer completed but the engine never wrote the sentinel-filled buffer. The controller reported success for a DMA that did not happen — the loudest thing this milestone can find. |
| `[sdhc-adma] … FAILED int=… adma-err=0xEE (ST_FDS…) adma-addr=0x… desc=N` | The engine faulted. `desc=N` names the descriptor; `ST_FDS` means it could not even fetch it (suspect the table address), `ST_TFR` means the data transfer of that descriptor failed. |
| `[sdhc-adma] engine DISABLED — falling back to PIO (…)` | A live transfer failed and the boot continues on PIO. The reads that follow are correct but not DMA; the failure above it is the real event. |
| `[sdhc-ab] SKIPPED — adma2 unavailable (reason)` | No A/B verdict this boot, and the reason is named. |
| `[sdhc-ab] verdict … match=0/3` or `match=1/3` | ADMA2 and PIO disagree about the card's contents. The per-window `first-diff` offsets above are the evidence. This is a real defect, not a bad card. |
| `[sdhc-ab] verdict … NO window could be compared` | Both arms failed or the control failed; this boot carries no verdict either way. |

### 7.5 Knobs — none added

**None**, mirroring §6.8. Milestone 3 is default-on with no new environment knob and
no new cargo feature, so there is no way for it to ship disabled with a green gate.
ADMA2 is gated on `CAP_ADMA2` alone — a runtime fact this boot's controller reports —
and never on a build-time switch, so nobody has to remember to turn it on and nobody
can turn it off to make a log look better.

Every hardware wait is bounded in the same rdtsc units milestone 2 established:
`DATA_TIMEOUT_MS` (200 ms) per PIO block, `DATA_TIMEOUT_MS + 2 ms × count` for an
ADMA2 completion (≤ 328 ms at the 64-block chunk bound), and `CMD_TIMEOUT_MS` /
`RESET_TIMEOUT_MS` inside CMD12 and `reset_cmd_dat`. No unbounded spin is added
anywhere.

**Still out of scope, deliberately: block-layer registration.** `drivers::block`'s x86
path still has no backend selector, so a card published into the global
`BLOCK_DEVICE` would silently contend with the USB stick for that slot. Milestone 3
stops at `read_blocks_512` as a module-public API, exactly as milestone 2 stopped at
`read_block_512`. A faster read path is not a reason to claim a global another driver
is already using; that is its own arc.

---

## 8. Milestone 4a (SDHC-4a) — the first write to a CARD

Milestones 1–3 wrote only the **controller**: memory decode, a reset, bus power, the
clock divider, Host Control 1, Bus Master, and the command/transfer-mode registers.
The medium was untouched end to end, which is why `arch/x86_64/pci.rs:638` still calls
the probe "the read-only SDHC census". Milestone 4a is the first build of this kernel
in which a byte can reach the card.

It adds exactly **one primitive** — `write_block_512`, a polled single-block CMD24 —
plus a boot self-test that exercises it against a sector the driver has *proven* is
empty. Multi-block CMD25, block-layer registration and any filesystem write are 4b/4c.

> **✅ The healthy path is METAL-CONFIRMED as of Boot AD, 2026-08-07 — see §8.11.** Every
> error leg remains QEMU-only or unexercised, and §8.9's named gaps are unchanged.

### 8.1 PIO, not the ADMA2 engine that is already sitting there

Milestone 3 left a working, A/B-verified ADMA2 engine in this driver, and 4a does not
use it. Four reasons, in the order that decides it:

1. **The file's own DMA law does not hold in the write direction.** `sdhc.rs`'s scope
   limits say DMA "lands only in driver-owned memory … a DMA write that straggles in
   after a transfer was declared timed out therefore lands somewhere harmless,
   forever." That protection is a property of the **bounce buffer**, and the bounce
   buffer protects **host memory**. Reverse the direction and the engine no longer
   writes host memory — it reads host memory and writes **the card**. A descriptor the
   engine is still walking when the driver declares a timeout lands on the medium, and
   no bounce buffer can make that harmless, because the medium is the thing being
   protected. The one DMA law this driver has is silent exactly where a write needs it.
2. **An armed engine cannot be recalled; a PIO write can.** Nothing is committed until
   the controller has taken all 128 words and raised Transfer Complete, so every
   failure before that point is a write that never happened.
3. **Layering — the argument §7 already made and won.** Milestone 3a added CMD18
   "still on PIO, so new COMMAND semantics are proved … with no new risk surface at
   all". CMD24 is new command semantics *and* a new direction, and `verify_adma_ab`
   has only ever proved the engine in the **read** direction. Putting the first card
   write on an unproved DMA direction would leave "write semantics" and "DMA write
   direction" jointly suspect.
4. **There is nothing to win.** One block is 128 uncached word writes — tens of
   microseconds — against a card programming time in milliseconds (why
   `PROG_BUSY_TIMEOUT_MS` is 500). A single-block write is programming-bound, not
   bus-bound. CMD25 is where DMA starts to pay; that is 4b's argument to make, with a
   proved CMD24 underneath it.

### 8.2 The ladder

`drivers::emmc2::write_block_512` (emmc2.rs:706-786), the ladder the Pi has already
run on real silicon, mirrored step for step:

| step | what | bound |
| :-- | :-- | :-- |
| W1C status, Block Size 512, Block Count 1, Transfer Mode | `TM_BLOCK_COUNT_EN` only — **`TM_DIR_READ` omitted** is what makes it a write; there is no positive "write" bit | — |
| CMD24 WRITE_SINGLE_BLOCK | R1, CRC + index check, data present | `CMD_TIMEOUT_MS` x2 |
| `r1_check` **before a byte is pushed** | the card's own verdict (WP_VIOLATION, OUT_OF_RANGE, CARD_IS_LOCKED) — a card that rejected CMD24 never accepts the FIFO words | — |
| Buffer **Write** Ready (bit 4, not the read path's bit 5) | then push exactly 128 little-endian words; short buffers zero-padded | `DATA_TIMEOUT_MS` |
| Transfer Complete | the block is off the FIFO | `DATA_TIMEOUT_MS` |
| **programming busy** — wait for Command Inhibit (DAT) to clear | the card holds DAT0 low while it burns flash; `send_command`'s own 100 ms entry wait is too short for a legal 250 ms busy | `PROG_BUSY_TIMEOUT_MS` (500) |
| CMD13 SEND_STATUS | programming-phase failures (CARD_ECC_FAILED, generic ERROR, WP_ERASE_SKIP) are reported by **no** controller interrupt — only by a later SEND_STATUS | `CMD_TIMEOUT_MS` x2 |

Two deltas from the emmc2 twin, both this file's own established discipline:

* **failed data phases close the CARD's side** with `abort_data_transfer` (CMD12 +
  `reset_cmd_dat`), not just the host's. A write aborted mid-FIFO leaves the card in
  `rcv`, and the next command issued to a card in `rcv` is rejected for a reason that
  has nothing to do with that command. emmc2 returns without it.
* **the CMD13 verdict checks CURRENT_STATE**, not only `R1_ERROR_MASK`. `prg` is not
  an error bit: a card still programming answers CMD13 with a clean R1 whose state
  field reads `prg`, and a check that only masks for errors reads that as success.

### 8.3 Four gates, outermost first

| # | gate | refuses when | witness |
| :-- | :-- | :-- | :-- |
| 1 | **build** — `#[cfg(feature = "sdw")]` on `write_block_512` | always, unless `UNAOS_SDW=1` | `armed=0` on the `w1` line |
| 2 | **physical switch** — Present State bit 19, *inverted* (1 = write ENABLED), re-read at the write | the slider says LOCKED | `reason=wp-switch` |
| 3 | **the card's CSD** — `PERM_WRITE_PROTECT` (CSD[13]), `TMP_WRITE_PROTECT` (CSD[12]) | the card declares itself read-only | `reason=csd-perm-wp` / `csd-tmp-wp` |
| 4 | **blank-sector proof** — the scratch sector is read first | it is neither all-zero nor already carrying this driver's marker | `reason=nonblank` |

Gate 1 is compile-time rather than a runtime branch precisely so the claim is
checkable from the artifact: a default build contains **no** `[sdhc-w]` string and no
CMD24 command word (verified — 0 occurrences disarmed, 13 armed).

`WP_GRP_ENABLE` (CSD[31]) is decoded and printed but **not** gated on: it governs
CMD28/CMD29 group protection, and this milestone sends neither. A bit that governs
commands nobody sends cannot refuse anything.

### 8.4 The self-test, and why it is safe under a power cut

The shape is `arch::aarch64::sdmmc_tegra`'s ORIN-SDMMC-2 paranoia ladder
(sdmmc_tegra.rs:740-845) — pick a scratch sector, stash it, write a stamped pattern,
verify, restore, verify the restore — plus **two checks that precedent does not make**:

* **`inside-partition-N`.** ORIN-SDMMC-2 picks the card's last LBA and refuses only
  when sector 0 is GPT (because the backup GPT header lives there). It never checks
  whether that last LBA falls inside a declared **MBR** partition — which on any card
  partitioned "use the rest of the disk", the default of every partitioning tool, it
  does. 4a refuses.
* **`nonblank`.** Every other check reasons from a **table**, and a partition table is
  a claim about the disk written by software that is not running now: it can be stale,
  damaged, or a lie. This one reasons from the sector itself. Whatever the table says,
  512 zero bytes are 512 zero bytes.

Gate 4 is what makes the stash/restore window harmless. The ladder is exposed between
the pattern write and the verified restore, and a power cut in that window strands the
pattern on the card — but the sector was **proven empty first**, so the only thing a
cut can strand is this driver's own labelled pattern where zeros used to be. The
pattern carries `UNAOS-SDHC4A-SCRATCH`, so a stranded sector is self-identifying, and
`sdw_blank` accepts "already ours" as writable — one interrupted test does not
permanently disqualify the sector.

A consequence worth stating: because the stash is provably 512 zeros or the driver's
own marker, there is **nothing worth dumping** if a restore fails. ORIN-SDMMC-2 dumps
its stash as 32 lines of hex precisely because its stash may hold real data; 4a omits
the dump, and that omission is a *consequence* of gate 4, not a shortcut around it.

The restore is also a **second write with different content, verified** — a ladder that
wrote once could have been lucky; one that writes twice and verifies both has shown
the path is repeatable.

### 8.5 The witness

Exactly one line per boot on which a card was identified, whatever happens:

```
:: sdhc: w1 armed={0|1} lba={n|NONE} wp_sw={0|1} csd_perm={0|1} csd_tmp={0|1}
   class={?|GPT|MBR|MBR-empty|unpartitioned} blank={?|0|1}
   verify={IDENTICAL|MISMATCH|DRYRUN|FAILED|UNREADABLE|SKIPPED}
   restore={IDENTICAL|MISMATCH|REWRITTEN|REWRITE-FAILED|FAILED|UNREADABLE|SKIPPED}
   reason={...} -> {PASS|REFUSED|DRYRUN|FAIL} ::
```

`verify=`/`restore=` read `SKIPPED` on every path that did not reach them — never
silently omitted — so the line's **absence** means one thing only: the ladder never ran
(no controller, no card, or bring-up stopped earlier).

**No field reports a value the boot did not measure.** Two of them earn that the hard
way, and both were caught by adversarial review of the first draft:

* **`blank=` is `?`, not `0`, on every path that returns before the scratch sector is
  read** — rungs 1–6. It was a `u8` and those rungs passed a literal `0`, so a
  `reason=inside-partition-0` refusal asserted *"this sector holds bytes we did not put
  there"* about a sector nobody had read. `blank=0` now appears on exactly one line, the
  `nonblank` refusal, where it is a measurement taken by `sdw_blank`. `blank=1` appears
  only after that same call returned true.
* **`restore=IDENTICAL` means byte-compared equal, and nothing else.** The happy path
  reads the restored sector back and runs `first_difference`; the three armed FAIL paths
  re-write the stash and return without reading anything, and they now say **`REWRITTEN`**
  (the write call returned `Ok`) or **`REWRITE-FAILED`**. The same token used to carry
  both meanings, and it did so precisely on the paths where the card's state is most in
  doubt. The FAIL paths deliberately do **not** read back: a card that has just failed a
  data phase is the worst place to spend two more commands, so the ladder reports what it
  knows and names the difference instead of manufacturing a comparison.

`class=?` likewise means sector 0 was never read (rungs 1–3), not "no partition table".

**Everything except the write itself is unconditional.** The picker, all five refusals,
the stash read and this line compile and run on every boot, armed or not. A disarmed
boot prints `armed=0 ... -> DRYRUN` *together with the LBA it would have written*, so
"would arming this build write, and where" is answered by a boot that writes nothing.
That is also why `armed=` is on the wire: it is the field that catches a knob wired
into `arroyo` but not into `builder/src/main.rs` — the x86 ESP carries the **builder's**
kernel, and WXN-M3b lost a bench boot to exactly that omission.

The unconditional half costs two CMD17s on a boot that already issues thirty-odd
through `verify_read` / `verify_multiblock` / `verify_adma_ab`.

### 8.6 QEMU coverage — real, and better than expected

`builder/src/main.rs:702-721` already attaches `sdhci-pci` + a **blank 16 MiB
`sd-card`** by default (`UNAOS_NOSDHCI=1` opts out), so unlike the aarch64 SDMMC arc —
which has no QEMU model at all and prints a compiled-present-only line — 4a's write
path **executes in the regression suite**. The emulated card is unpartitioned and
blank, so every gate passes and the ladder runs end to end.

Three runs, `./arroyo test 60`, all RC=0, no `EXCEPTION`, no `panicked at`, no `-> FAIL`:

| run | knobs | `w1` verdict | `PASS` lines |
| :-- | :-- | :-- | :-- |
| dry run | — | `armed=0 lba=32767 wp_sw=1 csd_perm=0 csd_tmp=0 class=unpartitioned blank=1 verify=DRYRUN restore=SKIPPED reason=would-write -> DRYRUN` | 36 |
| armed | `UNAOS_SDW=1` | `armed=1 lba=32767 ... class=unpartitioned blank=1 verify=IDENTICAL restore=IDENTICAL reason=none -> PASS` | 37 (36 + the `w1` PASS) |
| armed, poisoned scratch | `UNAOS_SDW=1` | `armed=1 lba=32767 ... class=unpartitioned blank=0 verify=SKIPPED restore=SKIPPED reason=nonblank -> REFUSED` | 36 |

(`PASS` counted as `awk '/-> PASS|result=PASS/'` over the serial log. The absolute
number depends on the predicate; the load-bearing part is the **delta of exactly +1**
between the disarmed and armed runs, which is the `w1` line itself.)

The third run is the instrument-can-fail control: four bytes (`DE AD BE EF`) were
written into the last sector of `target/sdcard.img` **on the host**, and the ladder
refused. Host-side `sha256sum` of the card image was byte-identical across the armed
PASS run (`080acf35…643e`), and the poison bytes survived the refusal untouched
(`df14e0aa…49f4`). *Honest limit:* an unchanged hash across the PASS run is consistent
with both "written and correctly restored" and "never persisted to the backing file" —
the restore puts the sector back to zeros either way. The guest's own
`verify=IDENTICAL`, taken through a fresh CMD17, is the evidence that the pattern
reached the card model.

Two further controls synthesise a partition table into sector 0 of the same 16 MiB
image, and are the runs that exercise the refusals the blank default cannot reach:

| control | card shape | `w1` verdict |
| :-- | :-- | :-- |
| MBR, whole-disk | one `0x0c` entry `start=2048 count=30720`, `end == num_blocks` — the bench card's exact shape | `class=MBR blank=? verify=SKIPPED restore=SKIPPED reason=inside-partition-0 -> REFUSED` |
| protective MBR + backup header | one `0xEE` entry, `EFI PART` written at LBA 32767 | `lba=NONE class=GPT blank=? … reason=gpt -> REFUSED`, backup header byte-intact afterwards |

Both are also the demonstration that `blank=?` is real: each of these paths returns
before the scratch sector is read, and each used to print `blank=0` — a measurement
that had not been taken. On the first control's card, LBA 32767 is 512 zero bytes, so
`blank=0` was not merely unmeasured but wrong.

What QEMU cannot reach: silicon timing (Buffer-Write-Ready latency, real DAT0
programming-busy — QEMU deasserts busy instantly, so `PROG_BUSY_TIMEOUT_MS` and the
CMD13 `prg` check are never exercised there), the write-protect **switch** (the
emulated card reports write-enabled), and both CSD write-protect bits (both read 0).
Gates 2 and 3 are therefore **metal-only** and currently unfired.

### 8.7 Knobs

`UNAOS_SDW=1` -> cargo feature `sdw`. Wired in **both** `arroyo` and
`builder/src/main.rs` — the x86 ESP carries the builder's kernel, so a knob mapped in
only one of them ships the feature disabled while the operator believes it is armed.
For a *write* arm that is the most consequential version of that bug in the tree.

`sdw` rides `arm_features`'s strip list alongside `smolnet` and `kbdwit`:
`drivers/sdhc.rs` is reached only from `arch/x86_64/pci.rs`, so the feature emits no
aarch64 code, and stripping it keeps every aarch64 media hash byte-identical whichever
way the x86 write arm points. It is also carried on the `x86-all` check leg — its gate
is a real `#[cfg]`, so without a leg that enables it the entire CMD24 ladder would be
type-checked by nothing.

### 8.8 What 4b/4c still need

* **Block-layer registration.** `drivers::block`'s `BACKEND` selector
  (block.rs:27-33) is compiled `all(target_arch = "aarch64", feature = "baremetal")`,
  so x86 has no selector to register with; `publish_usb_geometry` claims the global
  `BLOCK_DEVICE` unconditionally there (block.rs:317-320). Giving x86 the selector is
  4b's content — named here, not designed here.
* **The FAT single-writer hazard.** `flight_recorder.rs:39` documents SINGLE FAT
  WRITER — the flush reserves `/UNAOS.LOG` once and then writes **in place**. A second
  writable backend appearing on x86 interacts with that reservation, and the
  interaction must be settled before any filesystem write is routed to the card.
* **A GPT-aware scratch picker.** A GPT header declares `FirstUsableLBA`, and the gap
  below it is unallocated by the disk's own account — which would give a GPT card a
  provably-safe scratch region instead of 4a's blanket refusal.
* **Multi-block CMD25**, at which point the ADMA2 write direction becomes worth its
  risk — with a proved CMD24 underneath it.

### 8.9 Named gaps in 4a's protection, carried into 4b

These are gaps in the *gates*, not open questions about the code. Each one is a thing
4a does not protect against, stated here so 4b inherits a list rather than a surprise.

* **GPT is a heuristic, not a reading.** `sector0_survey` sets `class=GPT` from one
  fact: a partition entry of type `0xEE`. The GPT header at LBA 1 is never read. A GPT
  disk with a **hybrid MBR** — an Apple idiom, and the bench machine is a 2012 rMBP —
  or with a rewritten or damaged protective MBR classifies as `MBR` or `Unpartitioned`,
  and its last LBA (the **backup GPT header**) becomes a scratch candidate. Gate 4
  backstops it by content and was proven to do so in QEMU (a hybrid-MBR card with a
  real `EFI PART` at the last LBA refused with `reason=nonblank`, header byte-intact) —
  but the gate that fires is not the gate that was designed to fire. **4b: one extra
  CMD17 reading the `EFI PART` signature at LBA 1 makes rung 4 mean what it says.**
* **A picker that moves off the last LBA loses that backstop.** The backup GPT *entry
  array* occupies the 32 LBAs below the last, and on a sparse table those sectors are
  largely **zero** — so `sdw_blank` would wave them through. Any 4b picker that walks
  down from the end of the disk must read the GPT header first; the `nonblank` content
  check will not catch it a second time.
* **`Unpartitioned` includes a live superfloppy.** `Sector0Class`'s own doc comment
  concedes it: no `0x55AA` signature means "unpartitioned, **or a filesystem starting
  at LBA 0**". A superfloppy card gets **zero** table-based protection — `sector0_survey`
  finds no signature and no extents, and rung 5 has nothing to refuse with. Gate 4 is
  the only thing between the ladder and the last sector of a live filesystem. This is
  also the *only* class the QEMU PASS run exercises (the blank 16 MiB image is
  `class=unpartitioned`), so the one path the suite proves is the path with the least
  table-based protection. **4b: probe for a filesystem at LBA 0 (FAT BPB, ext superblock
  at 1024) before treating "no partition table" as "no owner".**
* **The only two classes that can ever write** are, therefore: MBR/MBR-empty where the
  last LBA falls outside all four extents (a card whose partitioning tool left
  end-of-disk alignment slack — genuinely unallocated on MBR), and `Unpartitioned`.
  Every other class refuses.
* **A systematic wrong-LBA write is ungated.** The pattern write, the verify read, the
  restore write and the restore read all share the `lba_arg` computation, so an
  addressing error that is *consistent* is invisible to the verify: every step agrees
  with every other step and the line reports `verify=IDENTICAL restore=IDENTICAL ->
  PASS` while the bytes sit at an address nobody looked at. The stamped LBA at pattern
  bytes 32..40 catches a one-off slip, not a systematic one. `lba_arg` mirrors
  `read_block_512`'s logic, which is metal-proven on the Pi's eMMC and in QEMU — but
  this driver's read path has never identified a card on x86 metal (§8.10), so on
  `sdhc.rs` the computation is unproven. **This is why the write arm must not fly on the
  same boot as the CMD8 fix:** identification must land first and the card must be
  characterised, so that an armed flight has something to disagree with it.

### 8.10 What a metal boot on the bench rMBP actually prints today

> **SUPERSEDED IN PART by Boot AC (2026-08-07) — see §9.11.** The first and last bullets
> below were written while CMD8 still refused this card. §9 fixed that, and on Boot AC the
> card **was** identified and the `w1` line **did** print (`armed=0 lba=60799 wp_sw=1
> csd_perm=0 csd_tmp=0 class=MBR blank=1 … reason=would-write -> DRYRUN`). Concretely: the
> "no capture in which identification succeeded" claim is now false, the internal-slot card
> is no longer uncharacterised, and it is no longer protected by the CMD8 failure — the
> `sdw` build gate is the whole of its protection. The unarmed reconnaissance boot the
> first bullet called *uninformative* turned out to be the most informative boot this
> driver has had, once identification could reach it. The middle two bullets — about the
> boot medium and the boot card's MBR — stand unchanged. **The last bullet was then taken
> deliberately: Boot AD armed `sdw` and the ladder ran to a verified restore — see §8.11.**

Recorded because the first draft of this milestone assumed the opposite, and the
assumption was falsified by captures that already existed.

* **The `w1` line will not appear at all.** `bring_up` returns at the
  `let Some(mut card) = identify(...) else { return false }` guard, before
  `write_selftest(num_blocks)` is ever called. `identify()` dies at CMD8 —
  `cmd8 send-if-cond FAILED int=0x00018000 (cmd-timeout)` — in **every** rMBP metal
  capture in which the internal slot held a card (GR11, GR12, GR13, s61, s62, s66, and
  the two most recent GR20 boots). There is no capture in which identification
  succeeded. `CARD` stays `None`, `card_csd_write_protect()` returns `None`, and even a
  reached `write_selftest` would return without printing. An unarmed reconnaissance
  boot is *possible* and *uninformative*: it costs nothing and reports nothing.
* **The boot medium is not on this driver's bus.** The `UNAOS-X86` card reaches the
  kernel as `xHCI: Disk 'Generic-' 'USB3.0 CRW   -SD'` — a USB card reader on the
  xHCI/BOT stack. `sdhc.rs` drives PCI function `3:0.1 14e4:16bc` only. It is
  architecturally incapable of writing the boot medium, armed or not.
* **The boot card is MBR, and its last LBA is inside partition 0.** The same boot
  prints `PART: mbr ... type=0x0b start=2048 count=124733440 end=124735488 ACCEPT` with
  `protective=0` and `dev_blocks=124735488`. `end == dev_blocks`, so `num_blocks-1`
  falls inside the extent and rung 5 would refuse with `inside-partition-0` — before the
  stash read and before any CMD24. (Reproduced in QEMU on a synthesised card of exactly
  that shape.) The card is **not** GPT; the GPT refusal was never the operative risk.
* **The residual risk is the card in the internal SDXC slot.** `card-inserted=1
  cd-pin=1` on every recent boot; its contents and layout are unknown to anyone, and it
  is the only medium `sdhc.rs` can ever reach. Today it is protected solely by the CMD8
  failure. When a future arc fixes CMD8, this ladder runs against an uncharacterised
  card with gates 4 and 5 as the only defence — which is the second half of the
  argument in §8.9's last bullet.

### 8.11 ✅ METAL-CONFIRMED — Boot AD, bench rMBP, 2026-08-07 (first armed flight)

Tip `3f8f60c5`, capture `rmbp-gr16-s73`, slice
`~/unaos-bench/scratch/gr20/bootAD-slice.log`, pace record
[`bootpace.md §10n`](../01_BOOT_HAL/bootpace.md). Gates: mbench **28/28 required, 0
forbidden**; `serial-analyzer --wxn` **exit 0**. Read the capture with `awk`, not `grep`.

**A byte from this kernel has now reached a card.** The single passenger vs Boot AC (§9.11)
is gate 1 — the `sdw` build feature — armed. The kernel is otherwise Boot AC's build, so the
ladder ran against a card that had already been characterised by a boot that wrote nothing:

```
:: sdhc: w1 armed=1 lba=60799 wp_sw=1 csd_perm=0 csd_tmp=0 class=MBR blank=1
   verify=IDENTICAL restore=IDENTICAL reason=none -> PASS ::
```

Every gate reads the same measured value Boot AC's dry run reported (`wp_sw=1`,
`csd_perm=0 csd_tmp=0`, `class=MBR`, `blank=1`, LBA 60799 four blocks past partition 0's
`end=60795`); only `reason=would-write` is gone, which is exactly the one field that named
gate 1. §8.5's design goal — that a disarmed boot answers "would arming this build write, and
where" — is what made this an evidence-based decision rather than an experiment.

**Both `IDENTICAL`s are byte-compares, in §8.5's strict sense.** `verify=IDENTICAL` is the
stamped `UNAOS-SDHC4A-SCRATCH` pattern read back off the medium and run through
`first_difference`; `restore=IDENTICAL` is the stash written back and *re-read and compared*,
not the `REWRITTEN` token the armed FAIL paths use for a re-write nobody read back. So §8.4's
"the restore is a second write with different content, verified" held: the path wrote twice
and verified both, and the card is provably in the state it was found in.

**The card is unchanged where identification can see it.** Boots AE and AF re-ran the whole
of §9 against the same card after the write, and the raw registers are identical to the word
across all four boots (AC, AD, AE, AF) — three of them post-write:

```
[sdhc] cid mid=0x01 oid=PA pnm=S032B prv=4.5 psn=0x02465bbc mdt=2008-04
[sdhc] cmd9 csd raw=[0xff164000,0xdaf6d9cf,0x32135981,0x00005d01]
```

The self-test also passed line-identically on Boots AE and AF, so the armed ladder has now
flown three times. It is a regression floor item, not a milestone.

**Cost.** `GPACE … sdhc=327ms` against Boot AC's 293, `gui` 2498 → 2533, every other pace
lane unchanged. The lane delta is +34 ms, and the wall gap between the last pre-write line
(`[sdhc-ab] verdict`, 2482 ms) and the `w1` line (2518 ms) is 36 ms with nothing printed in
between; ACMD41 ran 397 polls here against Boot AC's 400, which is the ~2 ms of noise between
the two readings. **The whole stash/write/verify/restore/verify ladder costs 34–36 ms.**

**What this does and does not close.** §8.9's named gaps are unchanged — they are properties
of the design, not of this flight. What is now metal-proven is the healthy path: CMD24,
`r1_check` before the FIFO, Buffer Write Ready, Transfer Complete, the programming-busy wait
on Command Inhibit (DAT), and CMD13 with the CURRENT_STATE check, end to end against a real
card on a real controller. **Every error leg remains QEMU-only or unexercised** (§8.6's
coverage stands), and the fact that this ladder passes says nothing about how it fails.
4b/4c (§8.8) are untouched.

---

## 9. Pre-v2.00 (v1.x) card identification

### 9.1 The defect

Until this change the driver could not identify a **pre-v2.00 SD card at all**, on any
machine, on every boot. `identify` issued CMD8 SEND_IF_COND and treated *any* failure —
including a bare command timeout — as terminal:

```
[sdhc] cmd8 send-if-cond FAILED int=0x00018000 (cmd-timeout) — card is pre-v2.00 or
       absent; this milestone identifies v2.00+ cards only
```

That is the exact line the bench rMBP printed, and the card in its internal slot is a
genuine v1.x part: Panasonic `S032B`, manufactured 04/2008, SCR `00a5000033f00008`
(SD_SPEC = 0 ⇒ Physical Layer v1.0–1.01), CSD `005d0132135981daf6d9cfff16400000`
(CSD_STRUCTURE = 0b00 ⇒ v1.0, Standard Capacity), OCR `0x00300000` (CCS clear).

### 9.2 Why the timeout is the answer, not the failure

**CMD8 was introduced by SD Physical Layer specification 2.00.** A v1.0/v1.01 card does
not implement it and is *defined* not to respond. The spec's own card-initialisation
flow (§4.2.2) uses that silence as the discriminator: no response to CMD8 ⇒ "Ver1.X SD
Memory Card" ⇒ continue with ACMD41 and **HCS = 0**. The hardware was fine, the card was
fine, and the driver was throwing away the spec's detection method as if it were an error.

### 9.3 The three CMD8 outcomes, kept distinct

| Outcome | Meaning | Action |
|---|---|---|
| **Bare** command timeout — error half (bits 31:16) equal to bit 16 **alone** | The card drove nothing. By definition pre-v2.00. | Continue on the v1.x branch, ACMD41 HCS=0 |
| Any **other** error-half word — CRC, index or end-bit alone, a CMD line that never went idle, **or** a timeout with any other error bit set alongside it | The card answered and the answer did not survive the bus, the command never went out, or the bus faulted. | Stop — this is *not* the v1.x signature and is not made into one |
| Response with a **mismatched echo** | The card is v2.00+ (it answered) but garbled its own echo. | Stop, unchanged from before |

The predicate for the first arm is written out, because "the timeout bit is set" and "the
timeout bit is the only error bit set" are **different tests** and only the second one means
what this section claims:

```rust
Err(int) if int & (INT_ERR_ANY & !INT_ERROR_SUMMARY) == INT_ERR_CMD_TIMEOUT => …
```

`INT_ERR_ANY = INT_ERROR_SUMMARY | 0xFFFF_0000`, so masking the summary bit out leaves exactly
the error half; the summary bit is excluded because it is set for *any* error and therefore
discriminates nothing. The stricter test is required, not cosmetic: **SDHCI 3.00 §2.2.17
defines Command Timeout Error = 1 together with Command CRC Error = 1 as CMD Line Conflict** —
the host and the card drove the CMD line simultaneously. That is a bus fault and carries no
information about the card's generation, so `int = 0x00038000` (summary | timeout | CRC) must
take the *second* row, not the first. An earlier revision of this arm tested only
`int & INT_ERR_CMD_TIMEOUT != 0`, which would have classified a line conflict as a v1.x card
and made the §9.8 witness print `v1.x` about a generation it never determined.

Only the first arm is new. Nothing weakens: a corrupt bus still stops the ladder, and the two
worlds print different lines naming different evidence — both lines carry the raw `int` word,
which matters here because `int_error_name` tests the timeout bit first and so names a line
conflict `cmd-timeout` as well. The classification is made on the word, not on the name.

The bench card's captured `int = 0x00018000` decodes against this file's own constants as
`INT_ERROR_SUMMARY` (bit 15, the read-only "some error bit is set" summary) `| INT_ERR_CMD_TIMEOUT`
(bit 16). Its error half is `0x0001_0000` — bit 16 alone — so it **is** bare and still classifies
v1.x under the stricter predicate.

### 9.4 HCS is chosen, not assumed

ACMD41's argument was hard-coded `0x40FF8000` — HCS=1 plus the 2.7–3.6 V window. It is
now `ACMD41_OCR_WINDOW` with `ACMD41_HCS` added **only for a card that answered CMD8**.
The spec requires HCS=0 when CMD8 went unanswered: CMD8 and high capacity arrived
together in 2.00, so a card that implements neither is not required to tolerate the bit,
and a card that rejects it never leaves idle state. The chosen argument and the resulting
HCS are both printed on the ACMD41 line, so the log says which was sent.

One consequence worth knowing before anyone tightens the loop: on the v1.x path the
**first CMD55's R1 legitimately carries ILLEGAL_COMMAND (bit 22)**, because the SD spec
reports an unrecognised command in the status of the *next* command — and CMD8 was, by
construction, unrecognised. The driver does not `r1_check` CMD55; adding one there would
convict every v1.x card of the very thing that identified it. The comment in `identify`
says so at the call site.

### 9.5 Everything after the fork is shared

CMD2, CMD3, CMD9, CMD7, CMD16, the CSD decode, the CCS↔CSD cross-check (§6.4) and the
clock step are literally the same code for both generations — the divergence is the CMD8
arm and one argument bit. A v1.x card is therefore exercised by every witness a v2.00+
card is, including §6.5's three claims and §7.3's A/B verdict.

### 9.6 Byte addressing — already correct, now proven

SDSC is **byte-addressed**: CMD17/CMD18 take a byte offset, so an LBA must be multiplied
by 512. Getting this wrong reads the wrong sectors *silently*. The driver already did it
correctly — `read_block_512` and `lba_arg` (which serves the multi-block PIO path and the
ADMA2 path) both multiply for `block_addressing == false` and bound the result against
the 32-bit Argument register instead of truncating. **No change was needed, and none was
made.**

What was missing was proof. The gate's QEMU card is blank, so `verify lba1
differs-from-lba0` read 0 and no MBR cross-check was available — a byte/block mix-up
would have been invisible. Re-running against a **patterned** 16 MiB card (every sector
stamped with its own LBA, plus a real MBR) with the fingerprints computed independently
on the host:

| Witness | Predicted on the host | Printed by the driver |
|---|---|---|
| `lba0 fnv` | `0x687c435c3b908694` | `0x687c435c3b908694` |
| `lba1 fnv` | `0x6469225a013f72a5` | `0x6469225a013f72a5` |
| 8-block window fnv | `0xc7a291cdf6c836fa` | `0xc7a291cdf6c836fa` |
| MBR p0 `start=2048 count=30720 end=32768` vs capacity 32768 | fits | `fits-capacity=1` |

All three read paths (CMD17 PIO, CMD18 PIO, ADMA2) land on the addressed sector of a
byte-addressed card. That is ground truth, not two runs of the same code agreeing.

**There is a fourth caller of `lba_arg`, and it is a write.** On the trunk this arc now sits
on, SDHC-4a's `write_block_512` computes its argument through the *same* helper (`let arg =
lba_arg(card, lba)?`), so the ×512 above is the multiply an SDSC write uses as well — the
write path did not grow its own addressing. §8 owns that primitive; what belongs here is that
the byte-addressed argument is proven for it too, and that the proof needed a different
fixture than the one above.

The read fixtures cannot falsify the *write*: the self-test verifies by reading back through
`lba_arg`, so a systematic addressing error would be invisible — both halves would agree on
the same wrong sector. On the blank QEMU card it is worse, because every sector is identical
and any offset looks correct. The fixture that breaks the symmetry is
**patterned-except-last**: a 16 MiB card in which every sector is uniquely stamped from its own
LBA *except* the last, which is all zeros, so the card has exactly one blank sector (LBA
32767). The self-test's own rung 7 then becomes the discriminator, with no new instrument —
if the multiply is present it reads LBA 32767, finds it blank and proceeds to `PASS`; if the
multiply were missing, argument 32767 would land on byte offset 32767 = sector 63, which is
patterned, and the ladder would refuse with `blank=0 reason=nonblank`. QEMU printed
`blank=1 … verify=IDENTICAL restore=IDENTICAL reason=none -> PASS`, and the card image was
byte-identical afterward, so the write landed on the one sector the read had proven it was
addressing and touched none of the other 32 767 identifiable sectors. QEMU's card is CCS=0,
so the branch that executed is `lba_arg`'s `else` — the multiply itself.

### 9.7 Capacity from CSD v1.0

CSD v1.0 computes capacity from **three** fields, not v2.0's single `C_SIZE`:

```
blocks = (C_SIZE + 1) · 2^(C_SIZE_MULT + 2) · 2^READ_BL_LEN / 512
         C_SIZE      = CSD[73:62]   (12 bits)
         C_SIZE_MULT = CSD[49:47]   (3 bits)
         READ_BL_LEN = CSD[83:80]   (4 bits)
```

This code existed but was **unreachable on any v1.x card**, because CMD8 stopped the
ladder before CMD9. It is reached now. For the bench card's
`005d0132135981daf6d9cfff16400000`: `C_SIZE = 1899`, `C_SIZE_MULT = 3`,
`READ_BL_LEN = 9` ⇒ `1900 · 32 · 512 = 31 129 600` bytes = **60 800 blocks = 29 MiB**
(29.7 MiB / 31.1 MB — the same number the host reports).

The decode line now prints the fields, the derived block length and the formula, and
names a `READ_BL_LEN` outside the spec's legal 9–11 as a **suspect unpack** rather than
using it silently.

That warning is a warning, not an enforcement, and the earlier wording of this paragraph
implied otherwise. The downstream CMD16 SET_BLOCKLEN 512 backs it up in only two of the three
cases:

| `READ_BL_LEN` | `READ_BL_PARTIAL` | CMD16 SET_BLOCKLEN 512 | Effect |
|---|---|---|---|
| < 9 | either | rejected — 512 exceeds the card's maximum block length | `r1_check` stops the ladder |
| > 9 | 0 | rejected with BLOCK_LEN_ERROR | `r1_check` stops the ladder |
| > 9 | **1** | **accepted** — 512 is a legal *partial* block | **nothing refuses the card**; the inflated `num_blocks` is published to `card_num_blocks()` with only the SUSPECT line behind it |

The third row is the live case, not the hypothetical one: the bench card reads
`READ_BL_PARTIAL = 1`. The SUSPECT line therefore prints `read_bl_partial=` and says which of
the two worlds the card is in — whether CMD16 is about to stop the ladder, or whether the
capacity is being published on a warning alone. The write ladder is protected in the third row
for an unrelated reason: its scratch LBA is `num_blocks - 1`, so an inflated count puts it past
the end of the card and §8's rung 6 refuses it as `scratch-unreadable`.

### 9.8 The witness

One parameterised line, so it can say the other thing:

```
:: sdhc: card v1.x SDSC byte-addressed blocks=60800 size=29 MiB rca=0xNNNN ::
:: sdhc: card v2.00+ SDHC block-addressed blocks=NNNNNNN size=NNNNN MiB rca=0xNNNN ::
```

Every field is measured on that boot: the version from whether CMD8 was answered, the
class and addressing from ACMD41's CCS (cross-checked against the CSD structure), the
block count from the CSD arithmetic, the RCA from CMD3. `size` is floor(blocks·512 /
1 MiB) — a MiB figure, not the decimal MB a host tool prints.

The version field is cross-checked too, against the CSD structure — see §9.10. One
consequence of that check belongs here: **this statement can no longer print `v1.x SDHC
block-addressed`.** `v1.x` implies `csd_structure == 0`, which implies `ccs == false`, which
forces `SDSC` and `byte`. A line that contradicts itself about the card's generation is not
merely unlikely, it is unreachable.

`card_spec_v2()` joins `card_block_addressed()` in the module API. The two are **not**
the same fact: a v2.00+ standard-capacity card is byte-addressed as well.

### 9.9 What QEMU can and cannot test

QEMU **cannot present a pre-v2.00 card.** `qemu-system-x86_64` 10.2.2's `sd-card`
accepts `spec_version` 2 and 3 only; 0, 1 and 4 are rejected outright with
`Invalid SD card Spec version`. Both accepted versions answer CMD8, so **the CMD8-timeout
arm and the HCS=0 argument are metal-only.** Both arms are present in the linked kernel
ELF (`strings`), which proves compiled-and-linked, not reached-at-runtime — **and both were
then reached at runtime on metal, on the first flight: see §9.11.**

§9.10's contradiction check is narrower still: **no card can provoke it**, in QEMU or on the
bench, because it requires the classifier rather than the medium to be wrong. It was fired
under a throwaway forcing build, which §9.10 records in full.

What QEMU *does* cover is everything downstream of the fork, and more than before: its
card is 16 MiB, so it is itself CSD v1.0 / CCS=0 / **byte-addressed**, and the regression
run exercises the shared ladder, the CSD v1.0 arithmetic (`c_size=63 c_size_mult=7
read_bl_len=9 → 32768 blocks = 16 MiB`, matching the image size) and — with the patterned
card of §9.6 — the byte-offset argument on all three read paths. It also prints
`card v2.00+ SDSC byte-addressed`, which is the instrument demonstrating it can say the
other thing.

### 9.10 The CMD8 conclusion is cross-checked against the CSD

§6.4's cross-check validates ACMD41's CCS against the CSD structure version. Until this
change **nothing validated the CMD8 conclusion itself** — `v2_card` was the one field on
§9.8's witness that no second register could contradict, which is exactly the shape of claim
this project treats as unearned.

A second register does contradict it, for free, out of evidence the ladder has already
decoded and printed: **CSD version 2 was introduced by the same spec revision — SD Physical
Layer 2.00 — that introduced CMD8.** A card that did not answer CMD8 cannot hold a v2 CSD, so

```
!v2_card && csd_structure == 1
```

is a self-contradiction, and the driver now says so and stops:

```
[sdhc] CONTRADICTION: cmd8 concluded pre-v2.00 but the csd is structure=1 (v2), and csd v2
       arrived with the SAME spec (2.00) that introduced cmd8 — a card that did not answer
       cmd8 cannot hold a v2 csd. Evidence: v2_card=0 ccs=1 csd_structure=1. One of those two
       reads is wrong and this line cannot say which, so the generation is UNDETERMINED; the
       card is not published (the write self-test runs on every published card); stopping
```

**It is not subsumed by §6.4's check, which passes in precisely this case.** CCS and the CSD
structure agree with each other — both say high capacity — and it is the CMD8 conclusion that
disagrees with both. The witness would then have printed `card v1.x SDHC block-addressed`,
contradicting itself on its own line, behind a cross-check that raised nothing. The realistic
route into that state is a CMD8 answer lost on the bus of a genuine SDHC card; §9.3's
bare-timeout predicate closes the specific SDHCI §2.2.17 CMD Line Conflict word, and this
check is the backstop for every other way the answer could go missing.

**Why it refuses the card instead of overriding `v2_card` with the CSD's verdict.** Both are
defensible readings of which evidence is stronger — the CSD is a CRC-checked positive
measurement, CMD8's silence is an absence — and refusal is the one that keeps the driver
honest:

1. The contradiction proves one of two reads is wrong and does not say **which**. Either
   CMD8's silence was spurious, or the 136-bit CMD9 response is not being unpacked where the
   code thinks it is — and in that second case `csd_structure`, `num_blocks` and the §8
   write-protect bits are all fiction, decoded from the same register read. Overriding names a
   winner by assumption, which is the move this arc exists to stop making.
2. The initialisation already ran down the losing branch: ACMD41 went out with **HCS=0**,
   which the spec does not permit for a high-capacity card. A card that completed power-up
   anyway did something the spec does not describe, and nothing later in the ladder
   re-establishes what state it is in.
3. `bring_up` calls `write_selftest` on **every** `identify()` that returns `Some`. Proceeding
   would hand the write ladder — a live CMD24 once `UNAOS_SDW=1` arms it — a card whose
   generation the driver has just proved it could not determine. A refused card costs one boot
   without SD storage; a wrongly published one costs a sector of somebody's card.
4. Every other unresolved identification in `identify()` stops: the CCS↔CSD mismatch, a CSD
   structure that is neither 0 nor 1, a zero-block capacity, a garbled CMD8 echo. Overriding
   here would make this the single place where the driver publishes a card it has just
   contradicted itself about.

The cost is stated rather than hidden: a genuine v2.00+ card whose CMD8 answer is lost to a
one-off bus event is now refused where the old code would have proceeded with byte addressing
that CCS happens to make correct. That is a card lost to a boot, not data lost to a guess, and
it is a bus that has already misbehaved once on this boot.

**It cannot be provoked by any card, on QEMU or on the bench.** `qemu-system-x86_64`'s
`sd-card` accepts `spec_version` 2 and 3 only and both answer CMD8, so `v2_card` is true in
every QEMU configuration and the contradiction cannot arise from the media; on metal the bench
card is v1.x **and** CSD v1.0, which is the consistent case. Reaching it needs the *classifier*
to be wrong, not the card.

So it was provoked that way, and the line above is transcribed from the run rather than
composed. A throwaway build — one added statement forcing `v2_card = false` after a good CMD8
answer, never committed — against a **4 GiB** QEMU card (CSD v2, CCS=1):

```
[sdhc] cmd8 send-if-cond resp0=0x000001aa echo=0x1aa ok (v2.00+ card)
[sdhc] FIXTURE-NOT-FOR-COMMIT: forcing v2_card=0 to provoke the contradiction check
[sdhc] acmd41 arg=0x00ff8000 hcs=0 ocr=0xc0ffff00 powered-up=1 ccs=1 (block-addressed) after 1 polls
[sdhc] csd v2 c_size=8191 -> blocks=(c_size+1)*1024
[sdhc] CONTRADICTION: … Evidence: v2_card=0 ccs=1 csd_structure=1 … stopping
```

Three things that run establishes. **The §6.4 cross-check is silent** — no `MISMATCH` line —
because CCS and the CSD structure agree with each other and only the CMD8 conclusion dissents,
which is the blind spot this section exists to cover. **The card is not published**: no
`:: sdhc: card … ::` witness and no `w1` self-test line follow, so `bring_up` took the
`identify() == None` path and the write ladder never ran. And the run is a fixture, not a card
defect — the **same 4 GiB card under the unmodified build** identifies normally
(`acmd41 arg=0x40ff8000 hcs=1 … ccs=1`, `:: sdhc: card v2.00+ SDHC block-addressed
blocks=8388608 size=4096 MiB ::`, then `w1 … -> DRYRUN`), so the check discriminates the forced
misclassification and nothing else.

In the shipped build this is a guard whose value is that it is never printed.

### 9.11 ✅ METAL-CONFIRMED — Boot AC, bench rMBP, 2026-08-07 (first flight)

Tip `f94e280c`, capture `rmbp-gr16-s73`, slice
`~/unaos-bench/scratch/gr20/bootAC-slice.log`, pace record
[`bootpace.md §10m`](../01_BOOT_HAL/bootpace.md). Gates: mbench **28/28 required, 0
forbidden**; `serial-analyzer --wxn` **exit 0**. Read the capture with `awk`, not `grep`.

**The fork was taken, on the first flight, on the register word the previous boot refused
on.** Boot AB (the immediately preceding metal boot, same machine, same card) printed
`cmd8 send-if-cond FAILED int=0x00018000 (cmd-timeout) — card is pre-v2.00 or absent` and
returned from `identify()`. Boot AC printed:

```
[sdhc] cmd8 send-if-cond BARE-TIMEOUT int=0x00018000 (cmd-timeout) — error half is bit 16
       alone, no crc/index/end-bit alongside it … Continuing on the v1.x branch with acmd41 HCS=0
[sdhc] acmd41 arg=0x00ff8000 hcs=0 ocr=0x80ff8000 powered-up=1 ccs=0 (byte-addressed) after 400 polls
[sdhc] cid mid=0x01 oid=PA pnm=S032B prv=4.5 psn=0x02465bbc mdt=2008-04
[sdhc] cmd3 resp0=0x5bbc0520 rca=0x5bbc
[sdhc] csd v1 c_size=1899 c_size_mult=3 read_bl_len=9 (512 B) read_bl_partial=1 -> blocks=… = 60800
:: sdhc: card v1.x SDSC byte-addressed blocks=60800 size=29 MiB rca=0x5bbc ::
[sdhc] bdf 3:0.1 CARD IDENTIFIED — 60800 blocks, byte-addressed, csd v1
```

`int=0x00018000` is byte-identical between the two boots. The hardware did not change; §9.3's
predicate did. **No `CONTRADICTION` line (§9.10) and no CCS↔CSD `MISMATCH` line (§6.4)
followed** — both cross-checks were live and both stayed silent, which is the outcome §9.10
predicted for a genuinely consistent card.

**The decode is corroborated by a second, independent stack.** Before the flight the same
card was read on the **host** through sysfs. The kernel negotiated its own CID/CSD/RCA from
the card during its own initialisation, and the two agree item for item:

| fact | host (sysfs) | kernel (Boot AC) |
| :-- | :-- | :-- |
| `manfid` | `0x000001` | `mid=0x01` |
| `name` | `S032B` | `pnm=S032B` |
| `serial` | `0x02465bbc` | `psn=0x02465bbc` |
| `date` | `04/2008` | `mdt=2008-04` |
| `rca` | `0x5bbc` | `rca=0x5bbc` |
| `csd` → `CSD_STRUCTURE` | `005d0132…` ⇒ `0b00` | `csd v1` |
| `scr` → `SD_SPEC` | `00a5000033f00008` ⇒ `0` | pre-v2.00 (CMD8 unanswered) |
| size | `29.7 MB` | `blocks=60800` ⇒ 29 MiB |

Two rows carry more weight than the rest. **The RCA is not a property of the card** — it is a
value assigned during initialisation, and the host and the kernel each assigned their own,
independently, and got the same one. And the **raw CSD reconciles bit-exactly** once the
controller's dropped CRC byte is accounted for: the kernel's
`cmd9 csd raw=[0xff164000,0xdaf6d9cf,0x32135981,0x00005d01]` is precisely the host's
`005d0132135981daf6d9cfff16400000` shifted right eight bits. §9.7's CSD v1.0 arithmetic —
until this boot an argument made on paper against a spec table — now has an independent
witness for its answer.

**The card was read and the reads were checked four ways, then ADMA2 against PIO at three
points including the last block:**

```
[sdhc] read lba0 ok (512 bytes)
[sdhc] verify repeat lba0 fnv=0x9c40b663cd963722 match=1
[sdhc] verify lba1 fnv=0x7da144b97d054b25 differs-from-lba0=1
[sdhc] verify mbr p0 type=0x06 start=63 count=60732 end=60795 fits-capacity=1
[sdhc] verify mbr … partition extents vs CSD capacity 60800 blocks -> all-fit=1
[sdhc-ab] window lba=0 / 30400 / 60792  blocks=8  match=1  first-diff=none
[sdhc-ab] verdict windows=3 match=3/3 — adma2 agrees with the pio control byte-for-byte
```

The `all-fit=1` line is a third-party check the driver did not author: a partition table
written in 2008 by software that is not running now agrees with our CSD capacity *and* our
addressing mode. And the last A/B window ends at LBA 60799, the final block — so §9.6's byte
addressing is proved at the top of the address range, not only near zero.

**The §8 write ladder ran unarmed, and its line is the pre-flight for arming it:**

```
:: sdhc: w1 armed=0 lba=60799 wp_sw=1 csd_perm=0 csd_tmp=0 class=MBR blank=1
   verify=DRYRUN restore=SKIPPED reason=would-write -> DRYRUN ::
```

Every field is measured on this boot, which is what §8.5 built the unconditional half for.
Against §8.3's gate table: gate 2 `wp_sw=1` (Present State bit 19, inverted — the slider says
write-**enabled**), gate 3 `csd_perm=0 csd_tmp=0` (`csd write-protect perm=0 tmp=0
wp-grp-enable=0 -> card-declares-writable`), gate 4 `blank=1` (the scratch sector was read and
is empty). §8.4's `inside-partition-N` rung also passed: partition 0 ends at 60795 and the
scratch LBA is 60799 — four blocks of slack past the table's own end. **`reason=would-write`
means gate 1 is the only refusal, so the `sdw` compile-time feature is now the entirety of
this card's protection**, and an armed boot would run the ladder through a live CMD24 to a
verified restore. That decision is now takeable on measured evidence rather than assumption,
which is exactly what §8.10 said was unavailable while CMD8 refused the card.

**Cost.** `GPACE … sdhc=293ms` against Boot AB's `sdhc=12ms`, with every other pace lane
identical — the +281 ms is the work Boot AB never performed, not a regression. **235 ms of it
is ACMD41 power-up polling** (`after 400 polls`); identification through `CARD IDENTIFIED`
costs 3 ms and the whole read/verify/A-B battery costs 43 ms. The poll cadence, not the
ladder, is what a future pace arc would attack here.

**One instrument note that belongs with this section.** `BARE-TIMEOUT` is a new witness
string, and the bench waker's critical-alert pattern contained the bare token `TIMEOUT ` — so
this healthy boot tripped a critical wake. The pattern is now `[^E-]TIMEOUT `, verified in
both directions (`BARE-TIMEOUT` no longer fires; a genuine `xhci: TIMEOUT` still does).
Recorded here because the cause is this section's text: **adding a witness string changes the
bench's alerting surface**, and a witness that reads like a fault to a regex is a witness that
needs the regex checked.

**Reproduced on three later boots, one of them against a differently-valued CMD8 register.**
Boots AD, AE and AF (§8.11, [`bootpace.md §10n/§10o/§10p`](../01_BOOT_HAL/bootpace.md)) each
identified the same card, with `cid` and `cmd9 csd raw` **byte-identical to Boot AC's** on all
four boots — so the fork is reproducible, not a one-off, and it is unaffected by the §8 write
ladder having run on the three later ones. Boot AE additionally read `int=0x00018100` rather
than `0x00018000`: bit 8 is set in the **normal** interrupt half. The classifier still took
the v1.x branch and still gave its grounds as "error half is bit 16 alone", which is what
§9.3's predicate actually tests. The discriminator has now been exercised against two
different register words, not one.
---

## 10. Milestone 4b (SDHC-4b) — the card becomes a BLOCK BACKEND

### 10.1 The gap this closes

After §8 and §9 the driver could do everything to the card and the operating system could
do nothing with it. Boot AC identified the bench card
(`:: sdhc: card v1.x SDSC byte-addressed blocks=60800 size=29 MiB rca=0x5bbc ::`, MBR
partition type `0x06` start 63 count 60732, ADMA2 cross-checked byte-for-byte against PIO at
three windows) and Boot AD wrote a sector to it and restored it
(`w1 armed=1 … verify=IDENTICAL restore=IDENTICAL -> PASS`). So `read_block_512`,
`read_blocks_512` and — behind `sdw` — `write_block_512` were all proven on metal.

None of it was reachable. `drivers/block.rs` has exactly two backends: the xHCI USB-MSC path,
and `BACKEND_SD` → `drivers::emmc2`. **Every SD arm in that file is
`#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]`** — the Pi's controller — so on
x86 `block::read_block` / `block::write_block` have always meant the USB stick and nothing
else. The card was a driver with no consumer.

SDHC-4b makes it a block backend, so `fs::fat` can mount it.

### 10.2 Selection policy — a THIRD registry handle, not a wider selector

The card is published as `BlockHandle::Sdhc` (`block::SDHC_BLOCK_DEVICE`, reached through
`block::read_block_sdhc` / `read_blocks_sdhc`), alongside the existing `Global`
(`BLOCK_DEVICE`) and `Usb` (`USB_BLOCK_DEVICE`) handles. It is **not** a new value of the
aarch64 `BACKEND` selector, and that is the whole selection policy. Three independent reasons,
any one of which is sufficient:

1. **A selector would steal the boot volume.** This machine boots from a USB card reader, so
   the stick *is* `BLOCK_DEVICE`, and `block::info()` is what the flight recorder,
   `fat::probe_once`, the shell, `fs/unafs.rs` and the installer all read. Flipping a selector
   would silently re-point every one of them at a different disk — PI-FS-2 (a 14 MiB reader's
   `num_blocks` clobbering the SD's global on the Pi), in reverse.
2. **The publish order makes it a race, not a choice.** `sdhc::probe` runs inside the x86 PCI
   probe; the stick enumerates later, and on x86 `publish_usb_geometry` claims the global
   **unconditionally**. A card registered into the global would simply be overwritten a moment
   later — and the "selector" would decide nothing while looking like it decided something.
3. **Refusal beats precedence.** With a separate handle there is no precedence rule to get
   wrong. A caller that wants the internal card must NAME it, and nothing in the tree names it
   except this arc's read-only mount witness. Every existing caller is untouched *by
   construction* rather than by a policy that has to keep being right.

**Proof of non-interference, at the source level.** There is no new statement anywhere in
`read_block`, `write_block`, `read_blocks`, `write_blocks` or `publish_usb_geometry` — not one
`#[cfg]`, not one branch. The change is additive entry points plus one extra arm in the
handle/source matches, and with the knob off the enum variant does not exist, so those matches
are byte-identical to the pre-4b tree.

**Proof of non-interference, on the wire.** §10.5.

### 10.3 SINGLE FAT WRITER — what this arc does and does not do about it

`flight_recorder.rs`'s module doc documents, with A/B evidence, the hazard a second mountable
volume walks straight into. `fs/fat.rs`'s `with_fat_lock` / `with_dir_lock` are **deliberately
INERT on x86** (masking IRQs across the `hlt`-driven xHCI BOT pump would hang the core), so on
this target the tree's rule is *at most one FAT/directory MUTATOR*. When the recorder was a
second one, the measured result was cross-linked chains (`GROW.BIN` chain length 5/6 where 2
was expected) and delete-witness first-free snapshots stolen mid-verdict: recorder stubbed out
→ 0/3 FAIL, recorder on → 3/3 FAIL. The recorder's fix was to stop being a mutator at all —
reserve `UNAOS.LOG` once, then only ever write in place.

**What SDHC-4b does:** it adds a READER.

* The SDHC volume is mounted as a **separate, by-value `FatFs`** (`BlockSource::Sdhc`) that
  shares no state with the boot volume's mount. The one cross-volume global in `fs/fat.rs` is
  `ALLOC_HINT`, which is documented advisory-only and is read by the cluster ALLOCATOR — a
  path a read-only mount never enters.
* `fat::write_sector` and `fat::write_sectors` **refuse a `Sdhc` source unconditionally, in
  every cfg**, through one function (`refuse_sdhc_write`) that names the reason once per boot
  and returns `FatError::Unsupported` — a refusal, not a device fault. This is the same
  blanket refusal PIUSB-27 shipped for `Usb`, which USB-WRITE later lifted in its own arc with
  its own witness ladder.
* Therefore no FAT entry, no directory sector and no data cluster of the internal card is
  written by this build, and the count of x86 FAT mutators is unchanged at **one**.

**What it does not do:** it does not make the volume writable, and it does not claim the
hazard is solved. Two things could make a FAT-layer write to this volume safe, and neither is
in this arc:

1. `with_fat_lock` / `with_dir_lock` become REAL on x86. They cannot simply be un-inerted —
   the reason they are inert is the BOT pump, so this needs a different mechanism (a
   non-masking mutex the storage service task can respect), not a flag flip.
2. The SDHC writer adopts the recorder's shape — reserve once at a point provably before any
   other writer exists, then only write in place, which is not a FAT mutation after bootstrap.

**The block-layer write pair is a different question, answered differently.**
`block::write_block_sdhc` / `write_blocks_sdhc` exist and route to `sdhc::write_block_512`,
but they are additionally `sdw`-gated (so a default image still contains **no CMD24 command
word at all**, preserving §8's property), and the only route to them is
`PartitionRange::write_block(s)` with an explicitly `Sdhc`-handled range. `fs/fat.rs` never
calls `PartitionRange::write_block` — its writers go straight to `write_sector` /
`write_sectors` — so no FAT mutation can arrive there. Without `sdw` the same entry point
exists and REFUSES with a one-shot witness, fail-closed in the same direction as USBFALL F1.
The installer's two `BlockHandle` matches also gained explicit `Sdhc` refusal arms: nothing
constructs an `Sdhc` install target, and a medium-destroying write is the last place to leave
an arm falling through.

### 10.4 The knob, the witnesses, and how each says NO

`UNAOS_SDHCBLK=1` → cargo feature `sdhcblk`, wired in `unaos/arroyo` (mapping + the aarch64
strip + the `x86-all` `KERNEL_CFG_MATRIX` leg) and in `unaos/builder/src/main.rs` — a knob
wired into arroyo alone ships the backend disabled while the `⚡ kernel features:` banner
claims it is on (the s42/INSTGUI and WXN-M3b lesson). Default OFF, and off means the enum
variants do not exist, no registry slot or entry point is compiled, and the registration call
site in `sdhc::bring_up` is unlinked.

Registration is the LAST thing `bring_up` does, after `verify_read`, `verify_multiblock`,
`adma2_smoke`, `verify_adma_ab` and `write_selftest`. That ordering is the point: registration
is the statement *a filesystem may trust this device*, and it should only be made about a card
whose read path this boot has already exercised and reported on.

Three distinguishable silences, which is what makes the witness falsifiable:

| observation | means |
|---|---|
| no `:: SDHCBLK:` line at all | `register_sdhc` never ran — no card identified. The `[sdhc]` bring-up lines above say why. |
| `registered …` then `no FAT volume … (NotFat)` | the card is readable but carries no BPB this reader accepts. The `:: PART: mbr-raw handle=sdhc …` census printed just above is the medium's own bytes. |
| `registered …` then `no FAT volume … (Io)` | the registry has the card but a read of LBA 0 failed — which CONTRADICTS the bring-up read witnesses, and is a finding about the driver, not the medium. |
| `registered …` with `blocks=0` | impossible: `register_sdhc` refuses a zero-block card and says so instead of publishing a device every later bound check would reject. |

### 10.5 QEMU evidence (2026-08-07)

QEMU attaches `sdhci-pci` + `sd-card` by default (§4), so the whole path is exercisable
without the bench. `target/sdcard.img` was rebuilt as a **16 MiB image mirroring the metal
card's layout verbatim** — classic MBR, primary slot 1, type `0x06`, start LBA 63 — with a
FAT16 filesystem, one file and one subdirectory on it.

Armed (`UNAOS_SDHCBLK=1 ./arroyo test-fat part 45`), the card mounts end to end:

```
:: SDHCBLK: registered internal SD card as block handle Sdhc — blocks=32768 (16 MiB)
   addressing=byte (global BLOCK_DEVICE untouched) ::
:: PART: mbr-raw handle=sdhc dev_blocks=32768 sig=55aa ::
:: PART: mbr handle=sdhc slot=1 type=0x06 boot=0x00 start=63 count=32705 end=32768 ACCEPT ::
:: PART: mbr census handle=sdhc protective=0 accepted=1 rejected=0 ::
:: SDHCBLK: FAT mounted READ-ONLY on the internal SD card (16 MiB): FAT16 vol@LBA63
   volsec=32704 bps=512 spc=4 nfat=2 fatsz=32sec reserved=4 fat@LBA67 data@LBA163
   clusters=8151 rootdir@LBA131 (32sec) ::
:: SDHCBLK: sdhc root directory (2 entries) ::
:: SDHCBLK:             82       HELLO.TXT ::
:: SDHCBLK:   <DIR>              SUBDIR ::
```

Three controls, all run at 40–45 s:

* **blank card, knob armed** → `mbr census handle=sdhc sig=absent — not an MBR` followed by
  `:: SDHCBLK: no FAT volume on the internal SD card (16 MiB, NotFat) ::`. The instrument says
  NO on a medium that deserves a NO.
* **no controller, knob armed** (`UNAOS_NOSDHCI=1`) → zero `SDHCBLK` lines, and the boot
  volume mounts normally. Registration cannot fire without a card.
* **knob OFF** → zero `SDHCBLK` lines.

**What knob-off costs the image, measured rather than asserted.** One worktree, one
`target/` dir, this tree vs `3d4ba446`, no knobs on either side: the x86 kernel ELF's `.text`
(682 479 bytes) and `.rodata` (91 336 bytes) are **byte-identical**. `.data.rel.ro` differs in
exactly 32 bytes, and every one of them is the `line` field of a `core::panic::Location` whose
`file` is the 27-character path `crates/kernel/src/fs/fat.rs`, shifted by 68 — the number of
lines of doc comment and `#[cfg]`-gated code this arc inserted ABOVE those panic sites. There
is no other difference. (A naive whole-file hash DOES differ, which is why the comparison is
per-section; and it differs for that reason alone.)

**Non-interference, measured.** Between the knob-off and knob-armed runs the boot volume is
identical — same census (`handle=global protective=0 accepted=1 rejected=0`), same mount
(`FAT32 vol@LBA2048 volsec=194560 bps=512 spc=1 nfat=2 fatsz=1497sec reserved=32 fat@LBA2080
data@LBA5074 clusters=191534 rootclus=2`), same 15-entry root — and the **named verdict sets
are identical: 37 PASS / 0 FAIL on both runs**. That set includes exactly the fixtures the
flight-recorder A/B failure destroyed: U10 (`write_grow`), U10c (`create_in_root`), U10d
(`delete_located`), U11m2 (deferred delete) and U6gx all PASS with the knob armed, and the
recorder still reports `:: FR: UNAOS.LOG reserved … — flushes are write-in-place only (single
FAT writer preserved) ::`. The only run-to-run differences in the two logs are `[wc-d]`
compositor frame counters and the recorder's reserved cluster number, which moves because the
armed kernel binary is a different size and the ESP files land differently.

### 10.6 The falsifiable metal prediction

On the next attended metal boot with `UNAOS_SDHCBLK=1`, after the `[sdhc] … CARD IDENTIFIED`
and `w1 … -> DRYRUN` lines already established by Boots AC/AD, the log must contain:

```
:: SDHCBLK: registered internal SD card as block handle Sdhc — blocks=60800 (29 MiB)
   addressing=byte (global BLOCK_DEVICE untouched) ::
:: PART: mbr handle=sdhc slot=1 type=0x06 boot=0x?? start=63 count=60732 end=60795 ACCEPT ::
:: PART: mbr census handle=sdhc protective=0 accepted=1 rejected=0 ::
:: SDHCBLK: FAT mounted READ-ONLY on the internal SD card (29 MiB): FAT16 vol@LBA63 …
```

`blocks=60800`, `start=63`, `count=60732` and `end=60795` are Boot AC's numbers, not estimates
— a different value in any of them falsifies either this arc's geometry publication or the
Boot AC reading, and the raw `mbr-raw` line printed above the verdict says which. The mount
line is the real prediction and it can fail honestly: if partition 1 turns out not to hold a
BPB this reader accepts, the line reads `no FAT volume … (NotFat)` and the arc has still
delivered a working block backend with a truthful witness. What must NOT appear under any
outcome is a `:: SDHCBLK: FAT write REFUSED …` line — nothing in this build writes through the
FAT layer to that card, so that line firing would mean a caller exists that this analysis
missed.

The boot volume's own lines (`FS: FAT mounted: FAT32 …`, the `FR: UNAOS.LOG reserved …`
witness) must be unchanged from the previous boot. Any change there is the regression this
section's selection policy exists to prevent.

### 10.7 ✅ METAL-CONFIRMED — Boot AG, bench rMBP, 2026-08-07 (first flight)

Tip `30354af6` (SDHC-4b rides at `1d47b97a`), capture `rmbp-gr16-s73`, slice
`~/unaos-bench/scratch/gr20/bootAG-slice.log`, pace record
[`bootpace.md §10q`](../01_BOOT_HAL/bootpace.md). Gates: mbench **33/33 required, 0
forbidden**; `serial-analyzer --wxn` **exit 0**. Read the capture with `awk`, not `grep`.
Boot AH (`8d76eb71`) reproduced the whole of it the same day.

**The internal card is a mountable filesystem.** §10.6's prediction was published before the
flight and came back to the number — `blocks=60800`, `start=63`, `count=60732`, `end=60795`,
all of them Boot AC's readings and none of them estimates:

```
[   2528ms] :: SDHCBLK: registered internal SD card as block handle Sdhc — blocks=60800 (29 MiB) addressing=byte (global BLOCK_DEVICE untouched) ::
[   2549ms] :: PART: mbr handle=sdhc slot=1 type=0x06 boot=0x00 start=63 count=60732 end=60795 ACCEPT ::
[   2549ms] :: PART: mbr census handle=sdhc protective=0 accepted=1 rejected=0 ::
[   2551ms] :: SDHCBLK: FAT mounted READ-ONLY on the internal SD card (29 MiB): FAT16 vol@LBA63 volsec=60732 bps=512 spc=4 nfat=2 fatsz=60sec reserved=1 fat@LBA64 data@LBA216 clusters=15144 rootdir@LBA184 (32sec) ::
[   2552ms] :: SDHCBLK: sdhc root directory (10 entries) ::
```

The mount was §10.6's real prediction and it was allowed to fail honestly as
`no FAT volume … (NotFat)`. It did not. **Ten root entries were listed by name and size** —
`HELLO.BIN 72`, `STAT.ELF 8472`, `VUG.ELF 12568`, `PULSE.ELF 12568`, `hello.txt 109`,
`readme.txt 59`, `SCRATCH.BIN 1024`, `GROW.BIN 512`, `S8W.BIN 64`, `BLOCK.TXT 197` — which is
a filesystem read, not a sector dump.

**Both must-not-appear conditions held, and they are the half that could have convicted the
arc.** Zero `:: SDHCBLK: FAT write REFUSED …` lines anywhere in the slice — a single one would
have meant a caller writing through the FAT layer to this card that §10.3's analysis missed.
And the boot volume's own witnesses are unchanged:

```
[  13118ms] FS: FAT mounted: FAT32 vol@LBA2048 volsec=124733440 bps=512 spc=64 nfat=2 fatsz=15223sec reserved=32 fat@LBA2080 data@LBA32526 clusters=1948483 rootclus=2
[  13124ms] :: FR: UNAOS.LOG reserved 262656 bytes @cluster 3 reused=true stamped=true — flushes are write-in-place only (single FAT writer preserved) ::
```

**The SINGLE FAT WRITER invariant of §10.3 is intact and says so on its own line.** The two
volumes are visibly distinct in the log rather than inferred to be: the `global` handle prints
its own independent MBR census (`PART: mbr handle=global … type=0x0b … start=2048
count=124733440 … ACCEPT`) alongside the `sdhc` one.

**Cost — outside the measured window, not zero.** The GPACE span closes at 2528 ms and the
registration, MBR parse, mount and listing all run between 2528 and 2552 ms, so **no pace lane
contains this work**: `gui` is unchanged at 2542 ms from the previous boot and `sdhc=325ms` is
identical. The wall-clock cost appears only as the first `BPACE` line moving 2548 → 2552 ms.
About 4 ms, and "it cost nothing" would be the wrong reading of an unmoved total.

**What this does and does not close.** §10.3's scope is unchanged — this handle is **read-only
by construction** and the arc adds no write path to it, so nothing here bears on 4c. The
identification path (§9) and the armed write self-test (§8) both ran on this boot unchanged
and passed, so the block backend does not disturb the layers underneath it.

---

## 11. Milestone 4c (SDHC-4c) — the FIRST PERSISTENT WRITE, bounded to a reserved extent

SDHC-4b mounted the internal card and refused every FAT-layer write to it unconditionally. 4c
replaces that single refusal with a single **permit**, at the same seam, and the card becomes
writable — but only inside one published interval of LBAs, fixed before the first write is
possible.

This is the arc that makes anything in UnaOS survive a reboot. It is deliberately the smallest
possible version of that: one file, one extent, one sector written and read back.

### 11.1 The shape: adopt-only, reserve-once

§10.3 named two paths out of 4b. 4c takes path (b) in its strongest variant — **host-staged
reserve, zero kernel FAT mutation on the card, ever**:

1. The **host** stages a fixed-size (64 KiB), contiguous `UNALOG.BIN` in the root of the card's
   FAT volume.
2. The kernel **adopts** it: locate the entry, require `size >= 64 KiB` and a valid chain head,
   walk the chain it already has, require exactly **one contiguous run**, derive the absolute LBA
   extent, prove it, publish it.
3. Every later write is admitted only inside that extent.

The kernel never calls `create_in_root`, `write_grow`, `delete_located` or `alloc_cluster` with
`source == Sdhc`, in any cfg. The flight recorder still needs those (cases B and C of
`reserve_log`) because it must work on a volume it did not prepare; the card is media the host
prepares, so 4c keeps **only** the recorder's case A — the zero-mutation case that already fires
on every real boot. There is no bootstrap window to reason about, because there is no bootstrap.

Consequence: the x86 FAT-mutator count stays at **one**, on the *boot* volume, unchanged. The card
acquires a writer that is not a mutator, so there is nothing to serialize it against and no lock is
introduced or claimed. §10.3's `ALLOC_HINT` non-interference argument survives verbatim — a path
that never allocates never enters `alloc_cluster`.

### 11.2 The writable sector set, and why it is closed

Implementation: `crates/kernel/src/fs/sdhc4c.rs` (the permit) and the reserve/verify pass in
`fs/fat.rs` (`FatFs::sdhc4c_reserve`, `FatFs::sdhc4c_write_verify`), driven from `sdhc_probe_once`.

A build can write to the internal card **only** through `fs::fat::write_sector` /
`write_sectors` with `source == Sdhc`, and both call `sdhc4c::permit_write` before the block layer
is reached. It admits `[lba, lba+count)` iff:

1. the permit state is `ST_ARMED`, and
2. `EXTENT_LBA <= lba` and `lba + count <= EXTENT_END`.

`EXTENT_LBA`/`EXTENT_END` are stored **exactly once**, by `arm`, under a compare-exchange on the
state (`ST_UNATTEMPTED -> ST_ARMING`). Nothing else in the kernel stores to them; `permit_write`
only loads. Both initialise to 0, so even a torn read yields an empty interval, which admits
nothing — the initial values fail closed on their own.

Before arming, the reserve pass proves four independent bounds:

| # | check | what it closes |
|---|---|---|
| 1 | `lba >= data_start` | **the load-bearing one.** On FAT the boot sector, the reserved sectors, both FAT copies and the FAT16 fixed root all live BELOW `data_start`. This single inequality puts every one of them permanently out of reach, whatever the chain walk returned. |
| 2 | `end <= data_start + count_of_clusters * spc` | the extent cannot run past the addressable data region |
| 3 | `in_extent(lba, n)` | the volume's `tot_sec` AND the partition table's declared length — two separate on-disk claims, the same check every read of the volume passes |
| 4 | `end <= sdhc_info().num_blocks` | the device's own capacity, asked of the block layer rather than derived from the BPB |

No knob widens the set. `permit_write` carries no `cfg` beyond the module gate, takes no bypass
argument, and is the only producer of `Ok(())` on the path. `sdw` gates whether a CMD24 ladder
exists in the image at all (§8's property, preserved — the default x86 image still contains no
CMD24 command word); it cannot loosen the bound, only remove the ability to write anything.

### 11.3 The bounds self-test

Immediately after arming, `selftest_bounds` attempts four spans that must be refused — one sector
below the extent, one above, one straddling the top edge, and one whose length overflows — through
`in_reserved_extent`, **the same function `permit_write` calls**. It cannot pass against a copy of
the arithmetic that has drifted from the real one. If any span is admitted, the permit disarms
itself on the spot and the card reverts to read-only.

It deliberately does not go through `permit_write`, so a passing self-test leaves `refusals=0` and
the must-not-appear refusal line absent: a self-test must not forge the arc's own failure
signature.

### 11.4 FRGUARD composition

FRGUARD (`drivers/block.rs`) answers a different question about a different handle: may the
*Default* slot be written, given the boot volume's `BS_VolID`. It refuses in exactly one state,
`BM_SUBSTITUTED`. That is the Boot AI-2 configuration, in which 4c's target IS the boot medium —
and it is the case the extent bound exists for. A write to the medium the kernel booted from is
admitted only inside the reserved file's own clusters, checked in absolute LBAs at the point the
CMD24 is about to be issued: one level BELOW where FRGUARD checks, and per sector rather than per
handle. FRGUARD is neither consulted nor weakened; it keeps refusing `Default` writes in that state
exactly as before. The reserve witness prints the boot serial and the card's volume serial side by
side so the two verdicts can be read together.

### 11.5 The witnesses

```
:: SDHC4C: reserve NAME=UNALOG.BIN cluster=K size=S runs=1 lba=[A..B) sectors=N permit=ARMED (…) ::
:: SDHC4C: selftest bounds extent=[A..B) — 4/4 out-of-extent spans REFUSED (…) ::
:: SDHC4C: in-place write ok bytes=W lba=[A..B) readback=MATCH fnv=0x……… ::
:: SDHC4C: tally fat-mutations-on-sdhc=0 permits=N refusals=0 cmd24=N armed=1 ::
```

Every failure instead produces `permit=UNARMED (<reason>) — the card stays READ-ONLY`, which is a
**successful** outcome of the arc: it degrades to exactly 4b. The reasons are distinguishable —
no FAT volume, absent, a directory, too short, no valid chain head, malformed chain, short chain,
fragmented, and one per failed bound.

`fat-mutations-on-sdhc` is a **real** counter, not one that can only print 0. It is incremented in
`with_fat_lock_src` / `with_dir_lock_src` — the two wrappers every FAT-table RMW (`set_fat_entry`,
`alloc_cluster`) and all six directory RMW bodies funnel through — whenever the source is `Sdhc`.
Any future code that mutates the card's FAT or directory is caught by construction, whether or not
the write beneath it is then refused.

The must-not-appear line is `:: SDHC4C: permit REFUSED …`.

### 11.6 QEMU coverage — the ladder actually executes

§4's card image used to be a blank 16 MiB file, which meant an adopt-only arc could only ever be
compiled and skipped under QEMU. `builder/src/main.rs::stage_sdhc4c_volume` now lays down a FAT16
superfloppy carrying the same reservation, written in plain Rust (no `mkfs.vfat`/`mtools`
dependency) so the geometry is fixed exactly and the builder can print the extent it staged. The
kernel's witness must name the same interval — a cross-check computed independently on the other
side of the boot, not a tautology.

`UNAOS_SDCARD_BLANK=1` restores the blank image, which is the fixture for the refusal path.

Measured on this tree, all four polarities `EXIT=0`:

| run | fixture | outcome |
|---|---|---|
| A | staged card + `sdw` | `lba=[97..225)` matching the builder's prediction, selftest 4/4, write ok, `readback=MATCH`, `cmd24=1`; a re-boot on the same image preserves the volume and writes a NEW record (different `fnv`) |
| B | staged card, no `sdw` | ARMED, `in-place write SKIPPED … no CMD24 ladder`, `cmd24=0` |
| C | blank card | `no FAT volume` → `permit=UNARMED`, card stays read-only |
| D | no `sdhcblk` | not compiled; zero `SDHC4C` lines |

Host-side byte check of `target/sdcard.img` after run A — the check no in-kernel counter can fake:
boot sector intact; the two FAT copies byte-identical; chain 2..33 contiguous with EOC at 33 and
cluster 34 still FREE; root directory entry unchanged and no new entry; sector 97 carries the
record; sectors 98..224 still 0xAA; every sector at or after 225 still zero. **Exactly one sector
of a 16 MiB medium changed, and it is the first sector of the reserved extent.**

QEMU-green is not correct — the metal predictions are published in
`~/unaos-bench/scratch/gr22/sdhc4c-predictions.md`, keyed on Boot AG's geometry
(`lba(c) = 216 + (c-2)*4`, so a 64 KiB file at cluster 2 predicts `lba=[216..344)`).

### 11.7 What 4c does NOT do

* It does not lift `sdw` to default-on. §10.3's open question Q1 stands, and this arc takes the
  recommended answer: a default image still contains no CMD24 ladder.
* It does not add CMD25. `write_blocks_sdhc` still loops single-block CMD24, and `cmd24=` is now
  the instrument that prices it — the number the one-volume collapse needs.
* It does not touch the installer's `Sdhc` refusal arms, the `publish_usb_geometry` guard, or the
  shell → `irqstorage` routing. Those remain the collapse prerequisites listed in §10.
