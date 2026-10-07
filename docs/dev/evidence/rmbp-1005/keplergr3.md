# KEPLERGR3 — the first real GR method on the GK107 GR channel (ledger B503)

## Design

**Finding (flight 27, f27-boot1.log, awk `[kgr]`/`KGR`).** `:: KGR: bind=ok ramfc_get=1 nop=ok -> PASS ::` — chid 1 is bound
through `kepler_fifo::host` on GR's runlist and the PBDMA executes HOST methods (NOP + host semaphore release). Nothing has yet
reached the GR ENGINE. The walls line reads `pmc=E011216D->E011316D`: PGRAPH's PMC bit (12) was OFF before `tests kgr` and the
fixture raised it, i.e. GR came out of reset under us — FECS is expected HALTED (`cpuctl=00000010`, falcon_microcode_spec §2's
rest value) with no ctxsw ucode, and the instance block carries no GR context pointer (inst+0x210 zero). KF28's precondition is
therefore MEASURED here, after the PMC raise and after the NOP, before any GR method is pushed.

**Seam.** None new: the rung extends `kepler_fifo::kgr` (the ONE fifo path, `host`), the same file, the same window, the same
`tests kgr` fixture (R80: destructive, alone, once). No knob (rides `nvidia-kepler` + `nvidia-kepler-takeover`), no new file.

**Rungs (continuing keplergr.md §2).**
| rung | claim | read | verdict |
|---|---|---|---|
| G8 | GR's class is pinned by the chip | PMC_BOOT_0 chipset (bits 27:20) = E7 -> GK107 -> nouveau gr/gk104.c sclass KEPLER_A (A097) | `chip-unpinned` declines |
| G9 | a GR context machine is resident (KF28) | FECS cpuctl 0x409100 (bit 4 HALTED), chan_cur 0x409B00, inst+0x210 | `fecs-halted` / `fecs-poisoned` decline BEFORE any GR push |
| G10 | GR accepts SET_OBJECT | method 0x0000 data A097 on subch 0, then a host WFI semaphore fence; PGRAPH_INTR 0x400100 bit 5 ILLEGAL_CLASS | `setobj=ok` = fence landed, no ILLEGAL_CLASS |
| G11 | GR EXECUTES a method | KEPLER_A SET_REPORT_SEMAPHORE_A..D (0x1B00..0x1B0C; D = RELEASE, STRUCTURE_SIZE ONE_WORD bit 28) writes `KGR3` into the pinned BAR1 window | `method=executed` = the word moved 00000000->4B475233 |

**Class pin.** Fermi+ GR exposes no readable class-list register (PTOP names engines, not classes): the class is pinned from the chip — PMC_BOOT_0 chipset E7 (kepler.rs:1417 already checks it) maps to KEPLER_A A097 by nouveau gr/gk104.c sclass (R83/R95 §2: pinned to its public source). A chip that reads otherwise declines `chip-unpinned` with BOOT_0 on the wire.

No GPC/GR register is written; the GR side is driven only through the pushbuffer. A GR push that does not land is unwound
(`kf::unwind`, as every non-PASS GR verdict since KEPLERGR).

**Milestones.** M1 this design. M2 G8+G9: chip pin + FECS precondition measured, `[kgr] fecs` line, the DECLINED verdict.
M3 G10+G11: SET_OBJECT + report semaphore + fence pushed as GP entry 1 when G9 admits, the `[kgr] setobj` / `[kgr] method`
lines, the second `[kfifo] decode`, unwind on stuck.

**Witness (what the next metal boot prints after `tests kgr`).**
- `[kgr] fecs chipset=E7 class=A097 cpuctl=<x> chan_cur=<x> ctx210=<x> -> halted|running|poisoned`
- `[kgr] setobj class=A097 subch=0 -> ok|illegal-class|stuck|declined(<reason>)`
- `[kgr] method=report-semaphore wrote=4B475233 sem=<before>-><after> fence=<x> pgraph_intr=<x> us=<n> -> executed|not-executed|stuck|declined`
- `:: KGR: bind=ok ramfc_get=1 nop=ok setobj=ok method=executed -> PASS ::` or `... -> DECLINED reason=<why> <register>=<x> ::`

Expected on this tree (prediction, not a result): `DECLINED reason=fecs-halted cpuctl=00000010` — GR leaves reset with no ctxsw
ucode; the rung then states the wall from a reading instead of from §2's inference.

**Owed.** FECS/GPCCS ctxsw ucode upload and a golden GR context (inst+0x210) — K-GPU-4's falcon fold; without them G10/G11 cannot
run on this part. KEPLERGR2's PFIFO preamble (PMC bit 8, PBDMA enable 0x204, 0x2390) still unrouted. The residual GR-unit MMU
fault on flight 27's decode (`u00 inst=00030000 PDE_SIZE`, an instance that is not ours, already in `intr_pre`) unexplained.
