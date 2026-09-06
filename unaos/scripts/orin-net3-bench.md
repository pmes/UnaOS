# ORIN-NET-3 bench runbook — PS widen + controller-0 link bring-up + device recon (attended; first fabric writes)

NET-2 named the exact blockers: controller-0's ECAM (`0x2e_2000_0000`, ~184 GiB) and MMIO `ranges`
(~200 GiB) sit **above** the tegra regime's 36-bit PS output ceiling, and the link is expected DOWN as
firmware leaves it. NET-3 removes the ceiling, brings the link up, and identifies what is behind it — the
last recon before a NIC driver arc. This is the lane's **first deliberate fabric-write sitting**: the
image writes to fabric in exactly three classes (TCR PS widen, appl LTSSM enable, BAR sizing), each
announced on serial *before* it is issued. QEMU models no Tegra234 RC, so the link/device answer is an
**attended-metal** deliverable — this sitting.

See `arch/aarch64/pcie_probe.rs` (`census2` + the `net3_*` metal path), `arch/aarch64/mmu_tegra.rs`
(the PS widen + `map_mmio_window`), and arch_arm64.md §ORIN-NET-3 for the design, the write ledger, the
two-ceiling reasoning, and the poison-rejection (PI-V3D-1) discipline.

## The image (one knob)

- **`UNAOS_PCIE3=1 UNAOS_TEGRA=1 ./arroyo esp-jetson`** — the recon image. `pcie3` implies `pcie2`
  (it builds on NET-2's `census2`/`map_mmio_window`); the metal M2/M3 writes are additionally
  `tegra`-gated. Knob-off, the module blocks + call sites vanish and the tegra loadable image is
  byte-identical to baseline `3fe218a` but for a single ratified `Location` line literal
  (`.data.rel.ro` one byte `0x7a→0x83`; `.text`/`.rodata`/`.data`/`.got` identical — objcopy-verified,
  see the doc); zero `PCIE3` strings knob-off.

Stage the built ESP tar to `~/unaos-bench/flash/orin/` per the flash-staging rule (stamp + sha256 +
MANIFEST); flash the staged tar, never a `target/` path. Validate tegra media by `tegra:` count/hash,
never by size.

## Hard rules for this bench (the write-scope boundary is load-bearing)

- **Exactly three fabric-write classes, each logged before issue.** (M1) the TCR PS/IPS widen at
  MMU-enable + the Device-nGnRE page-table block `map_mmio_window` installs for the ECAM; (M2) one
  `APPL_CTRL |= LTSSM_EN` read-modify-write on controller 0; (M3) the all-ones/readback BAR-sizing
  probe **with immediate restore** on the enumerated device's BARs. Every M2/M3 write prints a
  `>>> FABRIC WRITE (Mx): …` line first. If the serial shows a write the three classes do **not**
  cover — a config/command write beyond BAR sizing, a link retrain beyond LTSSM enable, a PERST/PHY
  reprogram, a BPMP power/clock MRQ, a write to any **other** controller, a bus-master/MEM decode
  enable, MSI/DMA setup — that is a **STOP**: record it and report, do not improvise.
- **Any RAS/SError signature is a STOP.** A fabric write to a controller the firmware quiesced could
  fault; the `mmu_tegra` Part-C / healed `exceptions.rs` vectors capture the syndrome (recorded + spin).
  Record it, do not retry blind.
- **Poison is ABSENT, never present** (PI-V3D-1). `ABSENT DECODE (poison/unclaimed)` is the correct
  liveness verdict (`0xffffffff` / `0xdeadbeef` = no responder), not a bug. A poison RP decode ⇒ STOP.
- **A still-down link after the LTSSM enable is an HONEST result, not a failure.** Record the LTSSM
  state (`APPL_DEBUG`) + DLL-active and stop; further bring-up (PERST deassert / PHY retrain) is beyond
  this arc.
- One serial reader only (`lsof` the port; screen(1) at 115200 is the proven rig). The recon is a
  boot-time dump — capture the serial, no interaction needed.

## What to expect on the wire (grep `PCIE3` — the NET-3 sub-block; NET-2 reg dumps carry `PCIE2`/`PCIE:`)

The census2 NET-2 preamble runs first (controller-0 DTB dump, RP identity, link state via DBI). NET-3's
own lines then take over. The ECAM now maps (contrast NET-2's `BEYOND the 36-bit PS ceiling`):

```
:: PCIE2:   map ecam 0x2e20000000 (+0x10000000): MAPPED Device-nGnRE (new page-table block) — readable ::
:: PCIE2:   PCIe cap @ 0x…: … DLL-active=0 => LINK DOWN ::
:: PCIE3:   M2 link bring-up on controller 0 (appl @ 0x140a0000) — Linux pcie-tegra194 LTSSM-enable sequence ::
:: PCIE3:   APPL_CTRL[0x4] = 0x……… (LTSSM_EN currently clear) ::
:: PCIE3:   >>> FABRIC WRITE (M2): APPL_CTRL[0x4] 0x……… -> 0x……… (set LTSSM_EN bit7) — issuing now ::
:: PCIE3:   LTSSM_EN issued; polling DLL-active (finite backstop) ::
:: PCIE3:   M2 result after N spins: DLL-active(DBI)=… APPL_DEBUG[0xd0]=0x……… LTSSM-state=0x…… => LINK UP/DOWN ::
```

### Branch A — link stays DOWN (the pre-registered likely outcome; NET-2's all-Fs predicts it)

```
:: PCIE3:   controller-0 link STILL DOWN after the appl LTSSM-enable sequence => honest hardware result … ::
```

Record: the RP identity/class (from the NET-2 preamble), LinkCap max speed/width, the LTSSM state, and
the `APPL_DEBUG` value. Verdict = "PS widen proven (ECAM maps), LTSSM enable issued, link did not train —
hardware question." NET-3.x scope = PERST/PHY retrain.

### Branch B — link comes UP (the interesting outcome)

```
:: PCIE3:   M2 result … => LINK UP ::
:: PCIE3:   M3 enumerate downstream device via ECAM bus1:dev0:fn0 @ 0x2e20100000 ::
:: PCIE3:   DEVICE FOUND: vendor=0x…… device=0x…… ::
:: PCIE3:   class=0x…… subclass=0x…… progif=0x…… rev=0x…… ::
:: PCIE3:   >>> FABRIC WRITE (M3): BAR0[0x10] all-ones probe (orig=0x………) — write 0xffffffff, read size, RESTORE ::
:: PCIE3:   BAR0 restored to 0x……… (readback was 0x………) ::
:: PCIE3:   BAR0 = 64-bit mem (prefetch=…), size=0x……… ::
   … (BAR2/4 similar; unimplemented BARs report readback 0) …
:: PCIE3:   M3 DONE — device identified + BARs sized (originals restored); no decode-enable / no driver bind … ::
```

Record: the device vendor/device/class and every BAR's size. Confirm each `>>> FABRIC WRITE` BAR probe is
followed by its `restored to <orig>` line (the ritual restores immediately). If the ECAM read is ABSENT
DECODE, the link is up but the RP's secondary-bus numbering is unset — record and stop (programming it is
out of scope). Verdict = "device X identified, BARs sized — ready for the NIC driver arc." STOP there:
driver bind is the next arc.

### The terminal `PCIE2` line — read the correction, not the archive

Both branches then fall back into `census2`, which prints ONE last line before the boot continues. Since
orin 11 / CENSUS2LIE it is the `pcie3` variant and it reads (wrapped here; one line on the wire):

```
:: PCIE2: ORIN-NET-2 controller-0 preamble DONE — NOT read-only on this pcie3 image: past the page-table
          mappings this pass ARMS controller-0 fabric writes (the appl LTSSM enable … ; BAR-dword all-ones
          probes …). Which of them THIS boot issued is the `>>> FABRIC WRITE` lines above — read those,
          never this one. Bounding THIS PASS keeps, and nothing past it: controller 0 only, … and no
          driver bind — on a `net4` image the driver's own decode-enable comes LATER, below this line ::
```

**On every capture taken before that correction — `boot7h`, `boot7i`, `boot7j` included — this same slot
instead reads `recon DONE (read-only; page-table mappings the only writes)`, and that is FALSE.** The logs
are not edited; treat the read-only claim there as void and count the `>>> FABRIC WRITE` lines instead.
(A `pcie2`-only image still prints the original literal, where it is true — see `orin-net2-bench.md`.)

The box proceeds to CAPSTONE (JM6) exactly as a normal tegra boot — the recon is a prologue. Restore the
boot-stick default at the end of the sitting per the standing rule.
