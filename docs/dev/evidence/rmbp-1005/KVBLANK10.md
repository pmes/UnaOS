# KVBLANK10 — the Kepler MSI is one-shot until re-armed through the BAR0 config mirror; the re-arm never ran (ledger B369)

CHARTER: Kernel — driver. Branch `exec-rmbp-kvblank10`, cut from `exec-rmbp-merge14` (2d4e1b12). Code: the tail
of `unaos/crates/kernel/src/drivers/gpu/kepler_vblank.rs` (section `KVBLANK10`), two same-line appends in the same
file (the arm sites), one body change in `kv8_isr_enter`. No new file, no new knob, no new verb (`tests kvblank`
exists), no dotfile. R80: nothing new runs at boot.

## Finding (flight 22, `rmbp-0915/flight22/f22-boots.log`)

The `KVBLANK8: snap at=test-arm` line, read whole (the brief quoted its head):

    msi cap=0x68 msgctl=0081 msi_en=1 pvm=0 mask=0 pend=0 addr=00000000:FEE02000 data=0044
    pci cmd=0006 bm=1 intx_dis=0 st_intx=0 lapic here=2 dest=2 valid=1 vec=0x44 isr=0 irr=0 tpr=00 ppr=00
    books irq=1 isr_calls=1 acks=1 head_rearms=1 head_rearm_rb=1 pmc_rearms=1 pmc_rearm_rb=1
    msi_rearms=1 mirror_rearms=0 eoi_ok=1 eoi_miss=0 pmc_post=00000000 hsum_post=01000000

Every stage the row asks about is ALREADY right on the metal:

| stage | read back | verdict |
|---|---|---|
| MSI capability | id 05 at 0x68, MsgCtl 0081: enable=1, MME=0, 64-bit=1, per-vector mask NOT capable | present, 64-bit layout (addr lo +4, addr hi +8, data +C) |
| address / data | `00000000:FEE02000` / `0044` | dest LAPIC 2, vector 0x44 — `FEE00000 \| (2 << 12)`, data = vector |
| LAPIC | `here=2 dest=2 valid=1`, TPR 0, PPR 0, ISR/IRR clear | the arming core IS the destination, not masked by priority |
| PMC | `intr0=04000000 en_host=1 mask=04000000` | PDISPLAY (bit 26) raised, routed, line enabled |
| PDISP | head 0 `en=1 host=02000003` | vblank (bit 0) enabled and latched |
| delivery | `isr_calls=1` | ONE message arrived — routing, vector and IDT are proven |

So the address, the data, the enable, the vector and the LAPIC id cannot be the fault: one message was
delivered through exactly that programming. And `pmc_post=00000000` (read inside that ISR, after the head ack)
proves the PMC output FELL — the next vblank presents a fresh edge. What never happened is the one step
nouveau does on every Kepler interrupt: **`mirror_rearms=0`**.

nouveau (data only, read from the open driver, `drivers/gpu/drm/nouveau/nvkm`):

- `subdev/pci/gk104.c`: `gk104_pci_func = { .cfg = { .addr = 0x088000, .size = 0x1000 }, … .msi_rearm = nv40_pci_msi_rearm }`.
- `subdev/pci/nv40.c`: `nv40_pci_msi_rearm(pci) { nvkm_pci_wr08(pci, 0x0068, 0xff); }` and `subdev/pci/base.c`:
  `nvkm_pci_wr08` = `nvkm_wr08(device, cfg.addr + addr, data)` — a BYTE write of `0xFF` to BAR0 `0x088068`, the
  PCI-config MIRROR in BAR0, not a CF8/CFC config cycle.
- `core/intr.c` `nvkm_intr()`: "Disable all top-level interrupt sources, and re-arm MSI interrupts." —
  `nvkm_intr_unarm_locked(device); nvkm_pci_msi_rearm(device);` FIRST, then the handlers, then the re-arm.
- `subdev/pci/base.c` `nvkm_pci_init()`: "Ensure MSI interrupts are armed, for the case where there are already
  interrupts pending (for whatever reason) at load time." — `msi_rearm` once at init, after `pci_enable_msi`.
- `subdev/mc/nv04.c`: unarm = `wr32(0x000140, 0)`, rearm = `wr32(0x000140, 1)`; `subdev/mc/gt215.c`: per-source
  allow/block in `0x000640`; `subdev/mc/gk104.c` `gk104_mc_intrs`: DISP = `0x04000000` (bit 26).
- `engine/disp/gf119.c` (gk104 display is gf119-family): init writes `0x6100b0 = 0x00000307` (supervisor
  interrupts, "disable everything else"); per head `vblank_get` = `mask(0x6100c0 + head*0x800, 1, 1)`; the head ISR
  reads `0x6100bc + hoff`, acks by writing the read value back (`nvkm_mask(…, 0, 0)`), then reads `0x6100c0`. The
  head bits summarise into `0x610088` bit `24 + head`.

Our driver: KVBLANK7 put the re-arm through CF8 (`write_config_32(0x68, v | 0xFF)` — a config cycle, `msi_rearms=1`,
did not re-arm), KVBLANK8 added the mirror write in `kv8_isr_exit` but gated it on `KV8_MSI_CAP`, which
`kv8_note_arm` caches AFTER the arm — and the arm itself fires the ISR at once (the vblank latch is pending), so
that one ISR ran with the cache at 0 and skipped the mirror. With no re-arm the function's MSI logic stays latched
for the rest of the boot: every later arm (`tests kvblank8`, the compositor's first need) sees zero messages,
`lost_at=msi`. The ordering nouveau uses avoids this twice over: re-arm at init, and re-arm at ISR ENTRY.

The INTx fallback is NOT the primary fix: the capability is present and delivered once. It stays wired as the
ladder's last stage (MSI off, the GPU's INTx pin through the IOAPIC via `ioapic::route_pci_function`, which reads
the GSI from the ACPI `_PRT`/PCH routing and prints its own `[ioapic] armed|route … REFUSED` line). Flight 22
carries no `_PRT` dump (only `[ioapic] census ioapics=1 … gsis=24`), so the GSI is probed, not read off the log.

## The seam

`CHARTER: Kernel — driver` (the existing file). No new state store; the ladder reads the hardware and the ISR's
existing books.

## Milestones

- **M1 — the nouveau re-arm, in nouveau's order.** `kv10_msi_rearm(bar0)` = byte `0xFF` to BAR0 `0x088068`
  (constant offset, as nouveau, no config cycle, ISR-safe). Called (a) in `kv8_isr_enter` right after the PMC
  unarm (`0x140` hw bit off), before any ack — `core/intr.c`'s order; (b) at both arm sites (rung 3's and
  `tests kvblank8`'s), after the MSI enable and before the PMC line enable — `nvkm_pci_init`'s "already pending"
  case; the arm also caches `KV8_MSI_CAP` before the line comes up so `kv8_isr_exit`'s mirror write is never
  skipped again. Counted: `KV10_ARM_REARMS`, `KV10_ENTRY_REARMS`.
- **M2 — the `tests kvblank` ladder.** After the sim fixture, `tests kvblank` walks the live path and names the
  FIRST stage that fails, in signal order: `msi_cap` (capability walk, 64-bit layout, address/data read back
  against `FEE00000 | lapic << 12` / vector) → `msi_en` (enable bit, MME 0, per-vector mask if capable) →
  `vector` (allocated, TPR/PPR below its class, ISR bit not stuck) → `lapic_id` (destination == this core) →
  `pmc_en` (`0x140` bit 0 and `0x640` bit 26 read back) → `pdisp_en` (head `0x6100c0` bit 0 read back) → `isr`
  (500 ms window, ISR entries vs `HEAD_STAT.VERT` vblanks, ≥ 90 %). If the MSI stages fail or deliver nothing,
  the INTx stage runs: MSI enable off, `route_pci_function`, same window. A PASS keeps the source live exactly as
  `tests kvblank8` PASS does; a FAIL restores every register it wrote.
- **M3 — (none on code)** the keep rule is unchanged: the compositor's first need (KVBLANK9 M3) and its 90 % check
  pick the fix up as-is.
- **M4 — this doc** (results below).

## Witness (a metal boot of the x86 shape, typed `tests kvblank` after the desktop is up)

    :: KVBLANK10: msi_cap=0x68/64 addr=00000000:FEE0<id>000 data=00<vec> match=1 msi_en=1 mme=0 pvm=0 vector=0x<vec> tpr=00 ppr=00 lapic_id=<id> dest=<id> cpu=<index> if=1 pmc_en=1 pdisp_en=1 arm_rearms=<n> entry_rearms=<n> isr=<i>/<v> ratio_pct=<p> wire=msi first_fail=none -> PASS ::

and the compositor's first need now reads `[wc-h] vbl_src=irq why=first-need irq=<~60> vbl_delta=60 ratio_pct=<≥90>`,
`tests kvblank8` reads `lost_at=none … -> PASS`. A failure names its stage: `first_fail=isr` with
`entry_rearms=0` means no message ever came (the re-arm is not the whole story), `entry_rearms>=1` with
`isr=1/60` means the mirror write does not re-arm on this die and the INTx line follows:

    :: KVBLANK10: intx gsi=<g> route=ok isr=<i>/<v> ratio_pct=<p> -> PASS|FAIL ::   (or route=<reason> -> REFUSED)

## Owed

- Proof is the metal only (R78). The INTx stage is untested on any hardware.
- The KVBLANK7 CF8 write at `0x68` in the ISR stays (line-sensitive file, harmless: it rewrites MsgCtl with its
  own value); retire it once a boot proves the mirror path.
- nouveau's head ack writes back ALL latched bits (`0x6100bc` = its own value); ours writes bit 0 only. Not
  changed: `pmc_post=00000000` proves bit 0 alone drops PMC bit 26.

## Results (legs run inline from `unaos/crates/kernel`, sequentially, target/ removed after each)

- x86 metal shape + `nvidia-kepler,nvidia-kepler-takeover,kvblank_trace,selfdiag,ahciroot,btc`: `cargo check` exit 0,
  no warning in `kepler_vblank.rs`.
- the same + `ioapic` (the INTx stage's `set_mask` arm compiles only there): exit 0.
- `charter-check.sh`: exit 0.
- NOTE: the x86 metal shape does NOT carry `ioapic` (`UNAOS_IOAPIC`), so on that image the INTx stage prints
  `route=no-ioapic-in-kernel -> REFUSED`; a boot that wants the fallback measured needs `UNAOS_IOAPIC=1`.
