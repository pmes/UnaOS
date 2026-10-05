# GPUBLIT2 — the CE channel sits on the runlist its own channel-table word names (ledger B383)

Branch `exec-rmbp-gpublit2` (cut from a60219de). File touched: `drivers/gpu/kepler_gpublit.rs` (B371's,
`CHARTER: Kernel — driver`, unchanged). No new file, no new knob, no new verb, no dotfile. Sources read
(scratchpad only, never the repo): nouveau v6.10 `nvkm/engine/fifo/gk104.c`, `gf100.c`, `chan.c`,
`runl.c`, `nvkm/subdev/top/gk104.c`, `nvkm/subdev/mc/{base,gk104}.c`; NVIDIA open-gpu-doc
`classes/dma-copy/cla0b5.h`, `classes/host/cla06f.h`, `manuals/volta/gv100/dev_{ram,pbdma}.ref.txt`.

## Finding — which word is wrong

Flight 23, both boots: `[gpublit] chid=2 runlist=1 ... gp_get=0 gp_put=1 sem=00000000
chan=00002000/11000001 pmc=E011216D pbdma=00000007 rl=00002001/00100001`.

- `rl=00002001/00100001` is RIGHT: nouveau `gk104_runl_commit` (gk104.c:446-447) writes `0x002270 =
  (target<<28) | addr>>12` (VRAM target 0) and `0x002274 = (runl->id<<20) | count`; we wrote page
  0x2001 (the runlist page) and runlist 1, count 1. The entry `(chid, 0)` is `gk104_runl_insert_chan`
  (gk104.c:454-455) verbatim.
- `pbdma=00000007` is 0x000204 (PMC PBDMA enable, gk104.c:739): three PBDMAs present and on. Right.
- `pmc=E011216D`: PFIFO bit 8 (0x100, nouveau mc/gk104.c `gk104_mc_reset`) is SET; bit 6 is set, bit 7
  is not. Which PMC bit is a CE's is NOT a fixed number on Kepler: nouveau takes it from PTOP
  (`nvkm_mc_reset_mask` -> `nvkm_top_reset`, the ENUM entry's reset field, top/gk104.c:64). B371's
  `0x40` is a guess; now read from PTOP.
- **`chan=00002000/11000001` is WRONG — the hi word.** nouveau `gk104_chan_bind` (gk104.c:77) writes the
  channel's RUNLIST into `0x800004 + chid*8` bits 16..19 (`mask 0x000f0000, runl->id << 16`) BEFORE
  binding the instance (gk104.c:68) and starts it (bit 10, gk104.c:52) only after the runlist commit
  (chan.c `nvkm_chan_insert` then `nvkm_chan_allow`). Our word reads bits 16..19 = 0: channel 2 is
  bound to RUNLIST 0 (GR's), while it is listed on, and submitted with, runlist 1. B371 wrote the whole
  word `0x400` (clearing the field) and never set it. The scheduler of runlist 1 meets a channel whose
  table word says runlist 0; nothing fetches; `gp_get=0`. The CE's runlist itself was a hypothesis
  (`1`); nouveau reads it from PTOP (`0x022700 + i*4`, top/gk104.c:45-81).
- The copy class and host methods (LAUNCH_DMA `0x386`, the method header, the GPFIFO entry, the
  semaphore) are pinned against `cla0b5.h`/`cla06f.h` and are all correct; with `gp_get=0` they were
  never reached. Comments only.

## The change (ONE per boot, R87's arming decision unchanged)

M1 — the channel's runlist association, from the spec: PTOP is walked exactly as `gk104_top_parse`
does; the CE is CE0 (engine type 1), else CE1 (type 2) — never CE2 (type 3, which shares GR's
runlist). Its runlist goes into the channel-table hi word (bits 16..19) and into RUNLIST_SUBMIT; the
bind order is nouveau's (runlist field, instance, commit, then ENABLE). Its PTOP reset bit replaces
`0x40` in the PMC OR (a no-op when that bit already reads set, as bit 6 did). No PTOP CE ->
`selftest=refused(ptop-no-ce)`.

M2 — the dump says whether the fetch started: the SAME words, then the PTOP CE, the runlist's
pending bit (`0x2284 + rl*8` bit 20, gk104.c:426) and how long the commit stayed pending, PFIFO_INTR
(`0x2100`, gk104.c:658), SCHED_ERROR (`0x256c`, gk104.c:624), the runlist event (`0x2a00`), `0x2a04`
and `0x2254` (nouveau writes both at fifo init, gk104.c:740/749; we write neither — read, not written),
the CE's engine status (`0x2640 + eng*8`, gk104.c:206), the channel's RAMFC GP_PUT/GP_GET/GP_FETCH
(inst `+0x00/+0x14/+0x50`, gv100 dev_ram.ref.txt:448/453/468; same layout as nouveau's `0x08`/`0x48`
words), and per PBDMA its runlist mask (`0x2390 + i*4`, gk104.c:392), channel (`0x040120`), INTR_0
(`0x040108`, gf100.c:315) and live GP_GET/GP_PUT (`0x040014`/`0x040000`, gv100 dev_pbdma.ref.txt).

## Witness (metal)

Unchanged: `:: GPUBLIT: selftest=ok ce_us=<n> cpu_us=<n> -> blitter=gpu ::`. On a timeout the
`[gpublit]` line now ends `... ce=ce0 eng=<n> rl=<n> reset=<n> rl_pend=<word> commit_us=<n|stuck>
pfifo_intr=<w> sched=<w> rl_ev=<w> r2a04=<w> userd_bar1=<w> eng_stat=<w> ramfc_put=<n> ramfc_get=<n>
ramfc_fetch=<n> pb0=<runm>/<chid>/<intr0>/<get>/<put> pb1=... pb2=...` and `chan=` hi reads
`1<rl>...` in bits 16..19. Reading: `ramfc_get`/`pbN get` > 0 = the fetch started (then the CE or the
semaphore is the wall); `rl_pend` stuck or `sched` != 0 = the runlist itself is refused; a PBDMA whose
`runm` lacks bit `rl` = no PBDMA serves the CE's runlist.

## Owed (the next one-change candidates, read from nouveau, NOT applied this boot)

- RAMFC vs `gk104_chan_ramfc_write` (gk104.c:88-102): ours ORs `0x80000000` into USERD_HI (`+0x0c`;
  nouveau writes the upper address bits only), writes `+0x94 = 0x30000000` without `devm` (`0xfff`),
  and omits `+0xe4` (priv `0x20`), `+0xf8 = 0x10003080`, `+0xfc = 0x10000010`.
- PFIFO init: nouveau writes `0x2254 = 0x10000000 | userd_bar1 >> 12` (USERD lives in ONE BAR1 table,
  chid*0x200) and `0x2a04 |= 0xbfffffff`; ours keeps USERD in its own window and writes neither. The
  dump prints both words so the next fix is chosen from the metal, not guessed.
- The host semaphore OPERATION `0x1002` sets ACQUIRE_SWITCH (bit 12, meaningless on a release);
  RELEASE_SIZE is 16-byte (payload at +0). Correct as is; left.
