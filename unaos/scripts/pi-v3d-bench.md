# pi-v3d-bench.md — PI-V3D-1 attended Pi 4 sitting (V3D GPU foundation)

The **positive** verification of the VideoCore VI (V3D 4.2) bring-up. QEMU `raspi4b` does not model
V3D, so everything past the presence gate is exercised only here, on real Pi 4 silicon. This card is
for an **attended** sitting (LC drives, Peter physical). It is **not** part of the QEMU DONE gate —
that gate is graceful-degradation + zero-regression only.

## Build & stage

```
# From the hw-jetson/us-v3d1 worktree's unaos/ dir. Unmount UNAOS before kernel8 builds.
UNAOS_V3D=1 UNAOS_PI=1 ./arroyo kernel8
```

Then stage the flashable image to `~/unaos-bench/flash/pi4/` (stamp + sha256 + MANIFEST) — never
flash a `target/` path directly (unaos-flash-staging rule). Bench card = the **32 GB** card (the
16 GB UNAOS card is retired). Rebuild any usbdebug ESP LAST if you also touch x86.

**Pre-staged for this sitting:** `~/unaos-bench/flash/pi4/UnaOS-pi4-baremetal-20260717T201428Z-b08e529-V3D.img`
(sha256 `bf3909933c0fe7dd43c3a3524f1ab355a3f94fd34598690c8e0d90288b1ce092`; kernel8.img
`73035653d83a4c9a…`; built from `us-v3d1` with `UNAOS_V3D=1`, pristine — not booted in QEMU; see the
MANIFEST entry for full detail).

**Timing note (call site):** the V3D bring-up is triggered from `mailbox::init_framebuffer` (the
byte-identity-preserving site), which runs in `build_boot_info` — slightly *before* `arch::init`
installs the EL1 exception vectors. This is QEMU-safe (only the no-fault hub IDENT0 read runs there).
On metal every access is to the live V3D block or the mapped arena (no unbacked address) and all waits
are finite backstops, so a V3D fault is not expected — but if the sitting sees an `AARCH64 EXCEPTION`
during the V3D lines, capture ESR/ELR/FAR: it means a V3D access faulted before vectors were live.

## What to watch on serial (`~/pi-serial.log` via the bridge)

The knob-on boot should show the V3D chain **instead of** the QEMU absence line:

| Line | Meaning |
|------|---------|
| `:: V3D: power domain 10 ON ::` | firmware powered the V3D domain (works in QEMU too) |
| `:: V3D: clock id 5 set to 500000000 Hz ::` | V3D clock programmed (works in QEMU too) |
| `:: V3D: HUB_IDENT0 = 0x........ ::` | **non-zero on metal** (QEMU shows `0x00000000` → skip) |
| `:: V3D: HUB_IDENT1..3 = ... ::` + `CTL_IDENT0..2 = ...` | the full IDENT block (metal only) |
| `:: V3D: PRESENT — tech version raw 0x.. (expect V3D 4.2 ...) ::` | decode; expect the 4.2 field |
| `:: V3D: M1 probe PASS ...` | powered + clocked + IDENT live |
| `:: V3D: MMU CTL=... VIO_ADDR=0x00000000 DEBUG=... (mapped 64 arena pages @ 0x...) ::` | MMU programmed, **no violation address latched** |
| `:: V3D: M2 MMU PASS ...` | MMU enabled, TLB flushed, arena confined |
| `:: V3D: M3 clear-job PASS (GPU cleared buffer; CPU byte-verified) ::` | **the money shot** — the GPU cleared the buffer and the CPU verified the bytes |

Plus, on the HDMI panel: a **64×64 teal square** top-left (the verified clear target blitted into the
framebuffer).

## Pass / stop criteria

- **PASS** = `M1 probe PASS` + `M2 MMU PASS` + `M3 clear-job PASS` all present, **0 `AARCH64
  EXCEPTION`**, and the full `pi4-regression.spec` still 49/49 (the V3D bring-up must not regress the
  syscall/CoW/BANDY chain — it runs on the BSP after `emmc2::probe`, ahead of the AP workload).
- **STOP + record** (do not improvise a fix — report the exact lines) if any of:
  - `HUB_IDENT0` reads `0x00000000` or `0xFFFFFFFF` on metal (power/clock ordering or wrong base) →
    the probe will skip; capture the IDENT values.
  - `:: V3D: MMU ... VIO_ADDR=` shows a **non-zero** violation address (the CL referenced outside the
    arena — a confinement failure; capture `VIO_ADDR`/`DEBUG`).
  - `M3 clear-job did not verify` or `:: V3D: verify mismatch at word N — got ... expect ...` (the
    GPU wrote the wrong bytes / store-config or packet encoding needs refinement — this is the
    expected first-metal refinement point; capture the mismatch word + value).
  - `:: V3D: ... timeout waiting for CT1 render (backstop) ::` (the RCL never completed — the finite
    backstop fired instead of hanging; capture and report, the packet stream likely needs the exact
    4.2 field encoding).
  - Any `AARCH64 EXCEPTION` with `FAR=0xfec.....` (a V3D MMIO access faulted — power/clock or base
    address; capture `ESR`/`ELR`/`FAR`).

## Notes

- The clear-job packet field encoding is the attended-metal refinement (M3 framing is correct + arena
  bounded; the exact 4.2 per-packet bit layout is finalized here against live IDENT/MMU/CLE behavior).
  A `did not verify` on the first metal boot is a refinement finding, not a regression — the anti-hang
  backstops guarantee it degrades to a clean witness line, never a hang.
- References of record: Linux `v3d_regs.h` / `v3d_mmu.c`, Mesa `v3d_packet_v33.xml` (4.2), lk-overlay
  `v3d.c`. See `docs/dev/OS/01_BOOT_HAL/arch_arm64.md` §PI-V3D.
- Lens NIT (recorded): V3D-side addresses (page-table base, CL begin/end, store target) are `u32`
  truncations — correct on the Pi 4 (kernel statics in low RAM), but any future arena above 4 GiB
  would truncate silently. If the sitting relocates buffers, keep them under 4 GiB.
