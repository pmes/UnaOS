# PLAN — Kepler GPU driver continuation (approved 2026-07-21)

**Ruling (Peter):** the Kepler driver is useful and continues. Rationale: the hw-rmbp
bench machine's GPU is a Kepler GK107 (GeForce GT 650M), and Kepler is the last NVIDIA
generation that runs without signed firmware — a 100% cleanroom driver is feasible.
Companion spec: `falcon_microcode_spec.md` (this directory). Cleanroom policy in that
spec's warning block is binding: hardware facts only, no blobs, no GPLv2-only code.

**Lane:** `unaos/crates/kernel/src/drivers/gpu/**` + `video/` touch points, feature
`nvidia-kepler` / knob `UNAOS_KEPLER` — off by default (quiet-boot law: gate, never
delete). Branch: `UnaOS-gemini` until the integrator says otherwise.

## Current state (post fix-r1, commit ebef9f97)
Detect + probe skeleton: PCI class scan, BAR sizing, chip ID via NV_PMC_BOOT_0,
translate()-guarded MMIO, VRAM report, PDISPLAY heuristic scan (fragile), VramAllocator
(hardcoded 256 MB), PushBuffer stub. No mutation of scanout yet. aarch64 stubbed out.

## Milestones (each independently gated, each ends with `UNAOS_KEPLER=1 ./arroyo check`
green both arches + x86 QEMU suite green + this doc updated)

### K-GPU-1 — Honest resource discovery
- Replace hardcoded VRAM/BAR values: size VRAM from NV_PFB_RAM_AMOUNT with sanity
  bounds; record real BAR1 size from the sizing probe; refuse init on zero/absurd values.
- Fix the BAR-sizing ordering defect: clear the COMMAND memory-decode bit around the
  0xFFFFFFFF write, restore after.
- Map BARs through the paging interface explicitly (UC) instead of relying on the
  identity map — the translate() guard becomes an assert, not the mechanism.

### K-GPU-2 — Real scanout takeover (replaces the PDISPLAY scan)
- Read the current mode from the display engine head registers (EVO/PDISPLAY channel
  state) instead of pattern-matching 16K registers for the FB address.
- Double-buffered handoff from the GOP framebuffer: allocate a new scanout surface in
  VRAM, copy, flip, verify on QEMU (`-device` NV emulation is absent — QEMU oracle is
  "cleanly refuses"; the real oracle is an rMBP metal sitting at an arc boundary).

### K-GPU-3 — PFIFO + pushbuffer bring-up
- Real channel setup: PFIFO poll/init, GPFIFO ring in VRAM, DMA push of NOP + fence;
  oracle = fence value observed written back by the GPU (metal).

### K-GPU-4 — PGRAPH Falcon firmware (the long pole)
- Implement the IMEM/DMEM upload path per `falcon_microcode_spec.md`.
- From-scratch init microcode (open reimplementation; nouveau's open ucode design is
  GPLv2-territory — treat as facts-only, write our own).
- Oracle: PGRAPH idle/ready status after boot, then first 2D blit via PGRAPH.

### Standing constraints
- Every register named must trace to public hardware facts (envytools/nouveau *facts*,
  never code). The proprietary macOS Falcon blob is off-limits in every form.
- No protection weakening; MMIO mappings UC and bounded; no unwrap/panic in probe paths.
- rMBP session owns shared kernel-core files — if a milestone needs `video/framebuffer.rs`
  surgery beyond the existing `base()` getter, flag it in the report before touching.

Adversarial review at each milestone boundary; metal verification only at arc
boundaries per repo law.
