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
