# ORIN-NET-1 bench runbook — read-only PCIe/NIC census (attended; census-before-touch)

Orin has no network path. The Jetson Orin Nano devkit's NIC sits behind the Tegra234 PCIe root
complex, so networking begins by knowing exactly what the firmware (NVIDIA UEFI / L4T 39.2.0) left us
at ExitBootServices. ORIN-NET-1 is the SMP-2-style **read-only census** that scopes the real bring-up
chain (PCIe RC → NIC → smoltcp, already in-tree). It writes NOTHING to fabric/config space, enables no
clock or power domain, retrains no link, changes no power state, and adds no page-table mapping. The
QEMU regression cannot see a Tegra234 root complex, so the DTB-derived map (which controller hosts the
NIC, its identity, link state as-left-by-firmware) is an **attended-metal** deliverable — this sitting.

See `arch/aarch64/pcie_probe.rs` + arch_arm64.md §ORIN-NET-1 for the design and the poison-rejection
(PI-V3D-1) discipline.

## The image (one knob)

- **`UNAOS_PCIEPROBE=1 UNAOS_TEGRA=1 ./arroyo esp-jetson`** — the census image. The `pcieprobe` feature
  is standalone (does not imply `tegra`); the metal build combines it with `UNAOS_TEGRA=1`. Knob-off,
  the module + both call sites vanish and the tegra image is byte-identical to baseline (zero `PCIE:`
  strings; `tegra:` count 109 unchanged).

Stage the built ESP tar to `~/unaos-bench/flash/orin/` per the flash-staging rule (stamp + sha256 +
MANIFEST); flash the staged tar, never a `target/` path.

## Hard rules for this bench

- **READ-ONLY.** This is a census. If the serial shows the probe about to write anything, or if any
  read requires first enabling a clock/power domain, that is a STOP — record it and report; do not
  improvise. The probe already fails closed (records a blocker and leaves the controller un-walked)
  when a config aperture is outside the mapped GiB-0 window — that is the expected Tegra234 path, not
  an error.
- **Poison is ABSENT, never present.** Any `-> ABSENT DECODE (poison/unclaimed)` line is the correct
  liveness verdict (`0xffffffff` / `0xdeadbeef` = no responder), NOT a bug. The PI-V3D-1 false-PASS is
  the cautionary tale this rule exists to avoid.
- One serial reader only (`lsof` the port; screen(1) at 115200 is the proven rig). USB keyboard is the
  only shell input; the census is a boot-time dump, so no interaction is needed — capture the serial.

## What to expect on the wire (grep `PCIE:`)

```
:: PCIE: ORIN-NET-1 read-only PCIe/NIC census (DTB @0x… size=0x…) ::
:: PCIE: N PCIe controller node(s) found ::
:: PCIE: ── controller 0: /bus@0/pcie@14180000 ──
:: PCIE: compatible = "nvidia,tegra234-pcie|snps,dw-pcie"
:: PCIE: status = "okay"                       (or "disabled")
:: PCIE: reg-names = "appl|config|atu_dma|dbi|…"
:: PCIE: reg = [… cells …]                     (per-region base/size)
:: PCIE: ranges = [… cells …]                  (the NIC's MMIO/IO windows)
:: PCIE: num-lanes = …   power-domains = …   interrupts = …
:: PCIE:   enabled(firmware)=true tegra-RC=true ::
:: PCIE:   config walk BLOCKED — config aperture 0x2e… (GiB …) is OUTSIDE the mapped GiB-0 window;
           NET-2 must map it Device-nGnRE first (no write here) ::      <-- expected for the high config aperture
   …or, if an appl/dbi aperture resolves in GiB 0…
:: PCIE:   config walk: appl aperture 0x141… is in the mapped GiB-0 window — guarded read ::
:: PCIE:   -> LIVE: vendor=0x10de device=0x…   (or -> ABSENT DECODE … NOT present)
:: PCIE: ORIN-NET-1 census DONE (read-only; metal columns attended-pending) ::
```

The box proceeds to CAPSTONE (JM6) exactly as a normal tegra boot — the census is a read-only
prologue.

## What this sitting fills into arch_arm64.md §ORIN-NET-1 (the map's device columns)

1. **Controller inventory** — how many `pcie@` nodes, each one's `status` (which the firmware left
   ENABLED), `compatible`, lane count, power-domains.
2. **The NIC's controller** — which RC's `ranges`/child node hosts the devkit NIC, and its config
   aperture base (the NET-2 mapping target).
3. **Config-space identity (if any GiB-0 appl/dbi aperture is live)** — vendor/device/class, header
   type, firmware-left BARs; else the recorded blocker naming the un-walked aperture.
4. **Link state as-left-by-firmware** — read via the appl/DBI PCIE capability LTSSM (NET-2 once the
   config aperture is mapped; NET-1 records the aperture and the blocker).
5. **The SCOPED NET-2 chain** — the minimal bring-up steps actually needed, each with the census
   evidence: map the config aperture Device-nGnRE → confirm link up → enumerate the NIC → bind the
   in-tree smoltcp path.

Record the serial as `~/unaos-bench/jetson-serial-<date>-net1-sitting.log` and fold the device columns
into §ORIN-NET-1 (mark them metal-confirmed, dated).
