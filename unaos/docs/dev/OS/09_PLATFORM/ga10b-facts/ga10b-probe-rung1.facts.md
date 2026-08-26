# GA10B first-probe-rung hardware facts

Status: IMPORTED, awaiting independent §6 review ack. This file has passed the
extractor's own terms-review pass (below) and is imported into the facts area so it can
be reviewed and cited; it must receive an **independent seat's §6 ack** (conflict-of-
interest guard) before it informs any kernel code. Extracted under Peter's ruling
2026-08-25 (Option 1); see `CLEAN_ROOM_POLICY.md` §6 and `../ga10b-clean-room.md`. It
carries hardware facts and `nvgpu:file:line` provenance POINTERS only.

Review verdict (extractor self-review, 2026-08-25): PASS.
- Contains only register offsets, bit-field positions/masks, magic constants, and
  required ordering. No function bodies, no algorithms beyond the required boot
  ordering, no copied comment prose, no copied macros, no struct/config expression.
- Every fact carries an `nvgpu:<path>:<line>` pointer; no pointed-at text is
  reproduced. Pointing is permitted; copying is not.
- Scope is the first probe rung only.
- SECOND REVIEWER REQUIRED before any tree import: an independent seat re-checks this
  file against the same terms (COI guard — the extractor does not clear its own import
  alone). Record that ack in the import commit.

Source of record: L4T r36.4.3 (JetPack 6.2) nvgpu, quarantine checkout
`~/unaos-bench/scratch/quarantine/nvgpu/`, tarball sha256
2c177804679e3ed650dabec6fa958388579896f170570c6171a1b6c386669216 (see PROVENANCE.txt).
Offsets are BAR0-relative unless noted; paths relative to `drivers/gpu/nvgpu/`.

## Aperture framing
- EXT: GA10B BAR0 physical base is a Tegra234 DTB `gpu@...` `reg` fact — resolve from
  the Orin FDT (`fdt_tegra.rs`), never from nvgpu.
- FACT: GSP falcon (v1) base in BAR0 = 0x00110000. nvgpu:hal/gsp/gsp_ga10b.c:51; nvgpu:include/nvgpu/hw/ga10b/hw_pgsp_ga10b.h:63
- FACT: GSP falcon2 (RISC-V / priscv) base in BAR0 = 0x00111000. nvgpu:hal/gsp/gsp_ga10b.c:46; nvgpu:include/nvgpu/hw/ga10b/hw_pgsp_ga10b.h:62
- FACT: PMU falcon2 base in BAR0 = 0x0010b000 (distinct engine). nvgpu:hal/pmu/pmu_ga10b.c:61; nvgpu:include/nvgpu/hw/ga10b/hw_pwr_ga10b.h:62
- priscv offsets below are falcon2-base-relative: absolute = 0x00111000 + off (GSP).

## (a) GPU power/clock state via BPMP
- SEQ: rail state is read as Linux runtime-PM suspend, not a GPU register. nvgpu:os/linux/platform_ga10b_tegra.c:372-384
- SEQ: static power-gate config is pushed to BPMP via MRQ_STRAP, request {cmd=STRAP_SET, id, value}, one transfer per mask. nvgpu:os/linux/platform_ga10b_tegra.c:347-368,407-469
- FACT: Tegra234 MRQ_STRAP ids — OPT_GPC=1, OPT_FBP=2, OPT_TPC_GPC0=3, OPT_TPC_GPC1=4. nvgpu:os/linux/platform_ga10b_tegra.c:57,59,61,63
- SEQ: BPMP replies — -BPMP_EINVAL hard error; -BPMP_ENODEV "unsupported", proceed; -BPMP_EACCES "not permitted", proceed. nvgpu:os/linux/platform_ga10b_tegra.c:360-368,484-502
- SEQ: static-PG MRQ path guarded by is_silicon; BPMP owns PG straps on silicon. nvgpu:os/linux/platform_ga10b_tegra.c:415-421
- NOTE: first-rung power query is a BPMP power-domain transaction asserted BEFORE any
  BAR0 touch. The Tegra234 GPU power-domain id is a BPMP-ABI/DTB constant (EXT) — not
  in this nvgpu subtree; resolve from the Orin DTB `power-domains` phandle.

## (b) Falcon / GR reset + boot-ROM handshake
### Core selection
- SEQ: GSP runs Falcon or RISC-V (falcon2); choice derived from fuse features FCD/DCS. is_falcon2_enabled ⇒ RISC-V path. nvgpu:common/falcon/falcon_sw_ga10b.c:52-71

### Legacy Falcon regs (falcon-base-relative)
- FACT: irqmask 0x018; irqdest 0x01c; idlestate 0x04c. nvgpu:include/nvgpu/hw/ga10b/hw_falcon_ga10b.h:66,67,72
- FACT: cpuctl 0x100 — startcpu bit1, halt_intr bit4. nvgpu:.../hw_falcon_ga10b.h:75,76,78-79
- FACT: hwcfg 0x108; bootvec 0x104; dmactl 0x10c (require_ctx bit0). nvgpu:.../hw_falcon_ga10b.h:95,91,93,94
- FACT: hwcfg2 0x0f4 — riscv_br_priv_lockdown bit13 (==1 ⇒ BR priv lockdown engaged). nvgpu:.../hw_falcon_ga10b.h:120,123-124

### RISC-V boot-ROM interface — priscv (falcon2-base-relative)
- FACT: cpuctl 0x388 — startcpu_true=0x1, halted bit4. nvgpu:include/nvgpu/hw/ga10b/hw_priscv_ga10b.h:62,63,64
- FACT: br_retcode 0x65c — result bits[1:0]; FAIL=0x2, PASS=0x3; 0x0/0x1 ⇒ BR never reached a verdict (RISC-V core never bootstrapped). nvgpu:.../hw_priscv_ga10b.h:65,66,67,68
- FACT: bcr_ctrl 0x668. nvgpu:.../hw_priscv_ga10b.h:69
- FACT: bcr_dmacfg 0x66c — target_noncoherent_system=0x2, lock_locked=0x80000000. nvgpu:.../hw_priscv_ga10b.h:76,77,78
- FACT: BCR DMA addrs — fmccode lo/hi 0x678/0x67c, fmcdata lo/hi 0x680/0x684, pkcparam lo/hi 0x670/0x674. nvgpu:.../hw_priscv_ga10b.h:70-75
- FACT: riscv_irqmask 0x528; riscv_irqdest 0x52c; boot_vector lo/hi 0x380/0x384. nvgpu:.../hw_priscv_ga10b.h:79,80,81,82

### Boot-ROM handshake ordering (SEQ — order is the fact; no code reproduced)
nvgpu:hal/falcon/falcon_ga10b_fusa.c:43-52,54-85,87-135,138-161
1. brom_config: write BCR DMA addrs → bcr_dmacfg = noncoherent|lock_locked → bcr_ctrl=0x111.
2. (alt) set_bcr: bcr_ctrl=0x11.
3. optional: write riscv_boot_vector_lo/hi.
4. start: write priscv_cpuctl = startcpu_true(0x1).   [WRITE — probe omits]
5. completion (READ-ONLY): poll br_retcode; PASS result==0x3, FAIL==0x2.
6. halt (READ-ONLY): priscv_cpuctl bit4 (falcon2), else falcon_cpuctl halt_intr bit4.
7. priv-lockdown (READ-ONLY): falcon_hwcfg2 bit13.
- FACT: GSP engine reset pgsp_falcon_engine BAR0 0x001103c0 — assert bit0=0x1, deassert=0x0. nvgpu:hal/gsp/gsp_ga10b.c:54-57; nvgpu:include/nvgpu/hw/ga10b/hw_pgsp_ga10b.h:64-66  [WRITE — probe omits]

### Security-state fuses (read-only)
- FACT: opt_priv_sec_en 0x820434 (the project's OPT_PRIV_SEC_EN; set ⇒ secure boot enforced). nvgpu:include/nvgpu/hw/ga10b/hw_fuse_ga10b.h:86
- FACT: opt_sec_debug_en 0x821040; opt_wpr_enabled 0x8205ec; opt_vpr_enabled 0x82067c. nvgpu:.../hw_fuse_ga10b.h:85,83,79

### Die-characterization (read-only)
- FACT: top_num_gpcs 0x022430 value bits[4:0] (GA10B Orin Nano = 2 GPC, EXT cross-check). nvgpu:include/nvgpu/hw/ga10b/hw_top_ga10b.h:62-63
- FACT: top_device_info_cfg 0x0224fc; device_info2 indexed table; version_init=0x2. nvgpu:.../hw_top_ga10b.h:80-98
- FACT: mc_enable 0x000200; mc_elpg_enable 0x00020c (xbar 0x4, l2 0x8, hub 0x20000000); mc_device_enable(i) indexed. nvgpu:include/nvgpu/hw/ga10b/hw_mc_ga10b.h:67,69,70-72,73
