# KEPLERGR4 — the GK107 GR engine's class, SET_OBJECT and one readable method (rmbp-ledger B627)

Branch `exec-rmbp-keplergr4`, cut from bd3d399d. R101 (two GPU slots always; the Kepler slot refilled with the next
rung), R95 §2 (nouveau read for hardware facts, cited file:line), R83 (no bit guessed), R52 (a GPU ucode/firmware
question is Peter's ruling), R80 (nothing at boot: `tests kgr` only). Sources were read from Linux `master`
(drivers/gpu/drm/nouveau) and NVIDIA open-gpu-doc `master` on 2026-10-13. Only ids, offsets, values and names
were taken.

## 1. Where the ladder stands (the premise, read off the tree's own wire)

- KEPLERGR (B421) / KEPLERGR2 (B463), flight 27: `:: KGR: bind=ok ramfc_get=1 nop=ok -> PASS ::`. The GR channel (chid 1)
  is bound through `kepler_fifo::host` and a host NOP lands.
- KEPLERGR3 (B503), flight 28 (`docs/dev/evidence/rmbp-0915/flight28/f28-boot1.log:3260-3262`):
  `[kgr] fecs chipset=E7 class=A097 cpuctl=00000010 chan_cur=00000000 ctx210=00000000 -> halted`, then
  `:: KGR: … setobj=declined method=- -> DECLINED reason=fecs-halted …`. FECS sits HALTED and the instance block
  carries no GR context, so no GR method was pushed.
- Owed by B463: the PFIFO preamble (PMC bit 8, PBDMA enable 0x204, 0x2390) was not routed through `host`.

## 2. Design (written before the code)

**Seam.** The rung lives in a new file `unaos/crates/kernel/src/drivers/gpu/kepler_gr.rs` (`//! CHARTER: Kernel — driver`).
It is declared from `kepler_fifo.rs` as `kepler_fifo::gr4` through `#[path]`, so gpu/mod.rs is not touched, and it
uses the same gate as `kgr` (x86_64 + `nvidia-kepler` + `nvidia-kepler-takeover`). The file holds the facts, the push
image and the witness. `kgr::tests_kgr` does all the IO. The PFIFO preamble is a new `host::preamble`, the ONE path,
and both the kgr leg and the September leg (`SeptRoute::route`) call it.

**Knob (my call, recorded).** There is no new feature. The rung rides `nvidia-kepler` + `nvidia-kepler-takeover`,
which have been on the image line since flight 10 (`image-line.env`: UNAOS_KEPLER, UNAOS_KEPLER_TAKEOVER), and runs
only under `tests kgr`, as KEPLERGR3 did. I rejected the two alternatives:
- `nvidia-kepler-kfbind` implies `nvidia-kepler-fifo` (Cargo.toml:129). That compiles in the September leg, which
  runs at boot under `cfg!` and poisons FECS before `tests kgr` (B463, seat note ST331). UNAOS_KEPLER_FIFO left the
  line at image 20 for that reason.
- A new `nvidia-kepler-gr4` needs a knob in `unaos/arroyo`'s map and a `feats.push` in `builder/src/main.rs`
  (knob-parity.sh's two vocabularies). Both files are outside this brief.

So `image-line.env` and `banner-cert.sh` need no change: the rung adds no feature to the banner.

**Rungs (continuing keplergr3.md G8..G11).**

| rung | claim | read / write | verdict |
|---|---|---|---|
| P1 | PFIFO is out of reset | 0x200 \|= 0x100 (+ GR's PTOP reset bit), read back | `[kfifo] preamble … -> ok` needs bit 8 set |
| P2 | the PBDMAs are counted as nouveau counts them | 0x204 <- FFFFFFFF, read the bits that stick, write the pre-image back | `pb_valid=` (metal s#5/s#6 named 3) |
| P3 | every probed PBDMA is enabled | 0x204 <- pb_valid, read back == pb_valid | `pb_en=pre->post` |
| P4 | each PBDMA's runlist mask is READ, never written | 0x2390 + i*4 | `runm=` (the September write of 1 is retired) |
| G8' | the class is pinned and printed | PMC_BOOT_0 chipset E7 -> gk104_gr sclass | `[kgr] class=0xA097 … src=pinned(…)` |
| G9 | FECS runs ctxsw ucode | FECS CPUCTL 0x409100 bit 4, CHAN_CUR, inst+0x210 | halted -> **ucode gate** (§3) |
| G10' | SET_OBJECT KEPLER_A lands | method 0x0000 = A097 on subch 0, host WFI fence, PGRAPH_INTR bit 5 | `set_object=ok` |
| G12 | GR executes a READABLE method | I2M: 256 bytes inline from the pushbuffer page into K_DST (window +0x6000), CPU reads 64 dwords back | `landed=1` = 64/64 match |

**Why I2M and not m2mf.** Nouveau's GK104 GR `sclass` (gr/gk104.c:486-491) lists FERMI_TWOD_A, KEPLER_INLINE_TO_MEMORY_A,
KEPLER_A and KEPLER_COMPUTE_A. It has no memory-to-memory class. FERMI_MEMORY_TO_MEMORY_FORMAT_A (0x9039, class.h:56)
is Fermi's, and `nouveau_bo.c:1027` (`nvc0_bo_move_m2mf`, nouveau_bo9039.c) is its only user. On Kepler, nouveau moves
buffers with the copy class A0B5: "COPY" on the CE channel, or "GRCE" on the GR channel (nouveau_bo.c:1021-1022),
both through `nve0_bo_move_copy`. The GR-side method whose effect the CPU can read is inline-to-memory. KEPLER_A
carries it (open-gpu-doc cla097.h:109-193). Its source is a pinned page (the pushbuffer, K_PUSH) and its destination
is a second pinned page (K_DST). So `method=i2m` stands where the brief wrote `method=m2mf`.

## 3. The ucode verdict (brief item 3): the rung is UCODE-GATED; question for Peter

- Nouveau brings GR up through `gf100_gr_init` (gr/gf100.c:2343-2464), which ends in `gf100_gr_init_ctxctl` (:1882-1893).
  Without external firmware, that function goes to `gf100_gr_init_ctxctl_int` (:1832-1880). It loads FECS DMEM+IMEM
  and GPCCS DMEM+IMEM, loads the register lists, and starts the hub by writing 0x409100 = 2 (:1867-1869). It then
  waits for 0x409800 bit 31. A golden context image is generated per channel (:440-443) before any object is used.
  GK104's `fwif` (gr/gk104.c:496-499) tries `gf100_gr_load` first and falls back to `gf100_gr_nofw`.
- Flight 28 read FECS HALTED at rest (`cpuctl=00000010`, f28-boot1.log:3260). This tree loads no ctxsw ucode.
- The ucode nouveau would load here is NOT an NVIDIA blob. It is nouveau's own `gk104_grhub_code` / `gk104_grgpc_code`
  (gr/gk104.c:436-454 includes fuc/hubgk104.fuc3.h and fuc/gpcgk104.fuc3.h). Those headers are generated from
  nouveau's own falcon assembly (fuc/hubgk104.fuc3 includes hub.fuc and com.fuc). **Their licence line reads MIT**
  (`SPDX-License-Identifier: MIT`, Copyright Red Hat). The brief expected GPL; the files say MIT. The other path is
  NVIDIA's `fuc409c`, `fuc409d`, `fuc41ac` and `fuc41ad`, loaded only when `NvGrUseFW` is set (gf100.c:2596-2611).
- **I stopped at the design (R52's shape).** No ucode bytes are in the tree or in this arc's scratch notes. The
  rung pushes nothing while FECS reads halted. Its wire is `set_object=gated … -> PASS (ucode-gated)`.

**THE QUESTION FOR PETER.** GR on the GK107 executes no class method until FECS and GPCCS run context-switch microcode
and a golden context exists. Which of these does UnaOS do?
- (a) Carry nouveau's MIT-licensed GK104 hub/gpc falcon ucode: assembled from nouveau's own `.fuc` source,
  attributed, loaded by the kernel.
- (b) Write UnaOS's own FECS/GPCCS ctxsw ucode from the falcon ISA (PLAN-kepler-continuation.md K-GPU-4's
  "from-scratch init microcode"). This is the long pole.
- (c) Stage NVIDIA's `fuc409c/409d/41ac/41ad` on the media, unmodified, beside its terms (R52's Orin carve-out
  widened to the rMBP).

The PLAN's standing constraint ("nouveau's open ucode design is GPLv2-territory — treat as facts-only, write our own")
predates the finding that the files are MIT. That is why this is a ruling and not an executor's call. The PLAN
also does not cover the golden-context generation (`gf100_grctx_generate`, ctxgk104.c), which is a second body of
work under any of (a)-(c).

## 4. Citation table

| fact | value | source file:line |
|---|---|---|
| FERMI_TWOD_A | 0x902D | nouveau include/nvif/class.h:53 |
| FERMI_MEMORY_TO_MEMORY_FORMAT_A (Fermi only) | 0x9039 | class.h:56 |
| KEPLER_INLINE_TO_MEMORY_A | 0xA040 | class.h:58 |
| KEPLER_A | 0xA097 | class.h:190 |
| KEPLER_DMA_COPY_A | 0xA0B5 | class.h:243 |
| KEPLER_COMPUTE_A | 0xA0C0 | class.h:266 |
| GK104 GR sclass list | 902D, A040, A097, A0C0 | nvkm/engine/gr/gk104.c:486-491 |
| GK104 GR fwif: firmware first, then nouveau ucode | gf100_gr_load, gf100_gr_nofw | gk104.c:496-499 |
| nouveau's own GK104 FECS/GPCCS ucode | gk104_grhub_code / gk104_grgpc_code | gk104.c:436-454; fuc/hubgk104.fuc3.h:1 (SPDX MIT), :209 |
| ctxctl load + hub start | 0x409100 = 2, wait 0x409800 bit 31 | gf100.c:1832-1880 (:1867-1871) |
| GR init ends in ctxctl | gf100_gr_init -> gf100_gr_init_ctxctl | gf100.c:2343-2464, :1882-1893 |
| golden context per channel | gf100_grctx_generate | gf100.c:440-443 |
| NVIDIA firmware path (opt-in) | NvGrUseFW, fuc409c/409d/41ac/41ad | gf100.c:2596-2611 |
| Fermi m2mf SET_OBJECT shape | SET_OBJECT on the copy subchannel | nouveau_bo9039.c (`nvc0_bo_move_init`) |
| Kepler buffer moves | A0B5 COPY / GRCE | nouveau_bo.c:1021-1022 |
| PMC PFIFO bit | 0x00000100 | nvkm/subdev/mc/gk104.c:28, :36 |
| PBDMA count probe | 0x204 <- FFFFFFFF, read, restore | nvkm/engine/fifo/gf100.c:933-934 |
| PBDMA enable | 0x204 <- runq mask | fifo/gk104.c:739; fifo/base.c:146-150 |
| PBDMA runlist mask (read only) | 0x2390 + i*4 | fifo/gk104.c:392 |
| SET_OBJECT | 0x0000 | open-gpu-doc classes/3d/cla097.h:34 |
| LINE_LENGTH_IN | 0x0180 | cla097.h:109 |
| LINE_COUNT | 0x0184 | cla097.h:112 |
| OFFSET_OUT_UPPER (7:0) | 0x0188 | cla097.h:115 |
| OFFSET_OUT | 0x018C | cla097.h:118 |
| PITCH_OUT | 0x0190 | cla097.h:121 |
| LAUNCH_DMA | 0x01B0; PITCH = bit 0, FLUSH_DISABLE 5:4 = 0, INTERRUPT NONE 9:8 = 0, SYSMEMBAR_DISABLE 6:6 = 0 | cla097.h:160-173, :189-190 |
| LOAD_INLINE_DATA | 0x01B4 | cla097.h:193 |
| SET_REPORT_SEMAPHORE_A..D | 0x1B00..0x1B0C | cla097.h:3265-3274 |
| push header NON_INC_METHOD | SEC_OP 31:29 = 3 | open-gpu-doc classes/host/cla06f.h:160-176 |

## 5. Expected wire (a prediction, not a result) after `tests kgr` on the rMBP

```
[kfifo] preamble pmc=<pre>-><pre with bit 8> pb_en=<x>-><v> pb_valid=<v> pbdmas=3 runm=<…> -> ok
[kgr] walls … 
[kgr] class=0xA097 chipset=E7 sclass=902D,A040,A097,A0C0 ptop_gr=rl0/eng0 src=pinned(gk104_gr sclass; no class register)
[kgr] fecs chipset=E7 class=A097 cpuctl=00000010 chan_cur=00000000 ctx210=00000000 -> halted
:: KGR: bind=ok ramfc_get=1 nop=ok setobj=declined method=- -> DECLINED reason=fecs-halted cpuctl=00000010 ctx210=00000000 ::
:: KGR4: class=0xA097 set_object=gated method=i2m bytes=256 landed=- preamble=ok fecs=halted cpuctl=00000010 -> PASS (ucode-gated) ::
```

Other verdicts: `preamble=wall -> DECLINED reason=preamble-wall`. A channel verdict that is not PASS gives
`class=- … -> DECLINED reason=kgr-<bind-wall|commit-wall|fetch-wall|exec-wall>`. A refused fixture gives `-> REFUSED`.
Once a ucode ruling lands and FECS reads running, the same fixture pushes the GP entry and gives `set_object=ok
landed=1 -> PASS`, or `DECLINED reason=i2m-<illegal-class|stuck|short> match=<n>`, and unwinds.

## 6. The Orin

This arc is the rMBP's Kepler only. The Orin's GA10B stays at GA10B7's measured door. Nothing here touches tegra
or aarch64.
