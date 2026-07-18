# ORIN-NET-2 bench runbook — controller-0 link + device recon (attended; read-mostly)

NET-1's census named **controller 0** (`/bus@0/pcie@140a0000`, domain 8) firmware-ENABLED with a full
`appl|config|atu_dma|dbi|ecam` reg map, then read the downstream `config` window (`0x2a00_0000`) and got
`0xffffffff` — an ABSENT DECODE. NET-2 answers the two questions that scope the real driver arc (NET-3):
**is the link up, and WHAT DEVICE is behind it** (the NIC hypothesis). It is read-mostly: the ONLY writes
it performs are kernel page-table mappings; no fabric/config/BAR writes, no link retraining, no BAR
sizing. QEMU cannot see a Tegra234 root complex (the GICv3 handoff even leaves `dtb_addr=0`), so the real
link/device answer is an **attended-metal** deliverable — this sitting.

See `arch/aarch64/pcie_probe.rs` (`census2`) + arch_arm64.md §ORIN-NET-2 for the design, the
DBI-vs-downstream-window correction, the PS-ceiling finding, and the poison-rejection (PI-V3D-1)
discipline.

## The image (one knob)

- **`UNAOS_PCIE2=1 UNAOS_TEGRA=1 ./arroyo esp-jetson`** — the recon image. The `pcie2` feature is
  standalone (does not imply `tegra`); the metal build combines it with `UNAOS_TEGRA=1`. Knob-off, the
  module + both call sites vanish and the tegra loadable image is byte-identical to baseline but for a
  single ratified `Location` line literal (objcopy-verified — see the doc); zero `PCIE2` strings;
  `tegra:` count 109 unchanged.

Stage the built ESP tar to `~/unaos-bench/flash/orin/` per the flash-staging rule (stamp + sha256 +
MANIFEST); flash the staged tar, never a `target/` path.

## Hard rules for this bench

- **READ-MOSTLY.** The only writes are kernel page-table mappings. If the serial shows the probe about
  to write anything else (a config/BAR/command write, a link retrain, a power/enable write), or if a
  read requires first enabling a clock/power domain, that is a STOP — record it and report; do not
  improvise.
- **The ECAM is expected to report `BEYOND the 36-bit PS ceiling`.** Controller 0's `ecam`
  (`0x2e_2000_0000`) and MMIO `ranges` (~200 GiB) live above the tegra regime's 36-bit PS output
  ceiling; `map_mmio_window` REFUSES to widen the regime (that is NET-3) and records the blocker. This
  is the correct, in-scope outcome, not a failure. The `appl/config/atu_dma/dbi` apertures report
  `ALREADY MAPPED` (GiB-0 device window).
- **Poison is ABSENT, never present.** Any `ABSENT DECODE (poison/unclaimed)` line is the correct
  liveness verdict (`0xffffffff` / `0xdeadbeef` = no responder), NOT a bug (the PI-V3D-1 cautionary
  tale). A poison RP decode is a STOP-record (controller powered down post-UEFI) — do not touch further.
- **Any RAS/SError is a STOP.** Reading the RP's DBI registers is read-only and safe on an enabled,
  powered controller, but if the firmware quiesced the controller after UEFI a DBI read could fault —
  the exceptions vectors capture the syndrome; record it, do not retry blind.
- One serial reader only (`lsof` the port; screen(1) at 115200 is the proven rig). USB keyboard is the
  only shell input; the recon is a boot-time dump, so no interaction is needed — capture the serial.

## What to expect on the wire (grep `PCIE2` — the NET-2 sub-block; shared reg dumps carry `PCIE:`)

```
:: PCIE2: ORIN-NET-2 controller-0 link + device recon (DTB @0x… size=0x…) ::
:: PCIE2: controller 0: /bus@0/pcie@140a0000 ::
:: PCIE: compatible = "nvidia,tegra234-pcie"          (shared dump formatter → PCIE:)
:: PCIE: reg-names = "appl|config|atu_dma|dbi|ecam"
:: PCIE: reg = [20 cells, 80 bytes] …
:: PCIE2:   enabled(firmware)=true tegra-RC=true ::
:: PCIE2:   region appl   = 0x140a0000 (+0x20000) ::
:: PCIE2:   region dbi    = 0x2a080000 (+0x40000) ::
:: PCIE2:   region config = 0x2a000000 (+0x40000) ::
:: PCIE2:   region ecam   = 0x2e20000000 (+0x10000000) ::
:: PCIE2:   map dbi 0x2a080000 (+0x40000): ALREADY MAPPED (GiB-0/1 device window) — readable ::
:: PCIE2:   map config 0x2a000000 (+0x40000): ALREADY MAPPED … ::
:: PCIE2:   map ecam 0x2e20000000 (+0x10000000): BEYOND the 36-bit PS ceiling (GiB 184 >= 64) —
            reaching it needs a TCR_EL2.PS widen to 40-bit … NET-3 must widen the tegra regime first ::
:: PCIE2:   RP dbi[0x00] = 0x……  ::
:: PCIE2:   RP LIVE: vendor=0x10de device=0x…… ::         (the root port's own identity — NET-1 never got this)
:: PCIE2:   RP class=0x06 subclass=0x04 progif=0x00 rev=0x… ::   (class 06/04 = PCI-to-PCI bridge)
:: PCIE2:   PCIe cap @ 0x…: LinkCap max(genN,xM) LinkStatus cur(genP,xQ) DLL-active=0 => LINK DOWN ::
:: PCIE2:   link DOWN as-left-by-firmware => NO device enumerable below the root port. NET-3 scope:
            bring up / retrain the link (appl + PHY / LTSSM), then enumerate. ::
:: PCIE2: ORIN-NET-2 controller-0 recon DONE (read-only; page-table mappings the only writes) ::
```

## The two branches (pre-registered)

- **LINK DOWN (expected — NET-1's all-Fs predicts it).** The RP identity + `LINK DOWN` is the verdict;
  no device below. Record vendor/device/class and the LinkCap max speed/width. NET-3 = PS widen + link
  bring-up + enumerate.
- **LINK UP (surprise).** `DLL-active=1`. The probe then reads `bus1:dev0:fn0` through the `config`
  window and reports the downstream device (vendor/device/class/header-type + raw BARs, sizes UNKNOWN).
  If that read is ABSENT DECODE, the link is up but the iATU CFG region is unset (programming it is a
  fabric write ⇒ NET-3). Either way: record the device identity if any, then STOP — BAR sizing and
  driver bind are NET-3.

The box proceeds to CAPSTONE (JM6) exactly as a normal tegra boot — the recon is a read-only prologue.
Restore the boot-stick default at the end of the sitting per the standing rule.
