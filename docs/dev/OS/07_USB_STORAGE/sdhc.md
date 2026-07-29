# SD Host Controller (SDHCI) — x86

The 2012 rMBP has a built-in SD card reader. Until SDHC-1 the x86 kernel could not
see it, and the reason was structural rather than accidental: `PciScanner::scan()`
matched exactly one PCI class triple and returned on the first hit, so the only
block device this architecture could reach was a USB mass-storage LUN behind that
one controller.

This document records **SDHC-1 milestone 1**: the enumeration widening and the
read-only discovery witness. Milestone 1 deliberately stops at discovery — it
programs nothing and transfers nothing.

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

## 5. What milestone 1 does not answer

QEMU-green is not hardware. The open question this witness exists to settle on the
bench is whether the 2012 rMBP exposes its reader as a PCIe SDHCI function at all
(as opposed to behind an internal USB bridge, which is how some Apple readers of
that era are wired), and if so at which spec version and base clock. That decides
what a milestone-2 bring-up sequence has to look like. Read the answer off the
`[PCI-STOR]` / `[sdhc]` lines of the next attended rMBP boot; if `[PCI-STOR]`
reports no class-0x08/0x05 function, the reader is not on PCI and the SDHCI arc
should stop there.
