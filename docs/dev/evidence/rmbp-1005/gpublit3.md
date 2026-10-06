# GPUBLIT3 — the GK107 copy engine to completion: the bind wall and every read wall on ONE boot (ledger B410)

Branch `exec-rmbp-gpublit3` (cut from 77db25d2). File touched: `drivers/gpu/kepler_gpublit.rs` (B371's, `CHARTER: Kernel —
driver`, unchanged; no new file, no knob, no verb, no dotfile). Continues `gpublit2.md`. Structured by DRIVERS-METHOD §6.
Sources (scratchpad only): nouveau v6.10 `nvkm/engine/fifo/{gk104,gf100,base,chan,runl}.c`, `nvkm/subdev/mmu/{vmmgf100,
vmmgk104}.c`, `nvkm/subdev/fb/gf100.c`, `nvkm/subdev/top/gk104.c`; open-gpu-doc `cla06f.h`, `cla0b5.h`, gv100 `dev_ram`/`dev_pbdma`.

## 1. Capture

None of a WORKING state (no Linux/nouveau on the bench GK107 yet). What we have is the state UEFI GOP leaves plus our writes,
read on two flights: flight 24 (`rmbp-0915/flight24/f24-boots.log`, 12:11:13Z) and flight 25 (`flight25/f25-boots.log`, 08:19:39Z
and the `tests gpublit` rerun 08:28:29Z) — GPUBLIT2's dump DID fly (the brief's "flight 26 has not flown it" is superseded):

    [gpublit] chid=2 runlist=4 ... gp_get=0 gp_put=1 sem=00000000 chan=00002000/11040001 pmc=E011216D pbdma=00000007 rl=00002001/00400001
    [gpublit] host ce=ce0 eng=4 rl=4 reset=6 pmc_pre=E011216D rl_pend=00100001 commit_us=stuck pfifo_intr=00000001 sched=00000000
      rl_ev=00000000 r2a04=800003FF userd_bar1=00000000 eng_stat=00000000 ramfc_put=0 ramfc_get=0 ramfc_fetch=0
      pb0=00000001/... pb1=0000006E/... pb2=00000010/00000000/00000000/0/0
    :: KEPLER: fold family=sched n=3 last=sched-status post-submit err=00000002 (present) stat=00000005 ::   (0x252c, the GR leg)

Plan for a real capture (owed, not this arc's boot): a Linux/nouveau boot on the bench reading 0x2254, 0x2a04, 0x2630, PBDMA
0x04013c and one CE channel's instance block word by word — the reference image for §2 of the method.

## 2. Rung ledger (carried from GPUBLIT/GPUBLIT2, never restarted)

| # | hypothesis | writes | discriminator (confirm / refute) | status |
|---|---|---|---|---|
| R0 | PREMISE: the host never fetched our GPFIFO (not "fetched and the CE stalled") | none | ramfc_get/pbN get = 0 confirms; >0 refutes | **confirmed f24 12:11:13Z, f25 08:19:39Z** (`ramfc_get=0 ... pb2=.../0/0`) |
| R1 | channel table hi word lacked the runlist (GPUBLIT2) | 0x800004 bits 16..19 | `chan=.../110[rl]0001` | **confirmed applied f25** (`chan=00002000/11040001`, rl 4); did not by itself start the fetch |
| R2 | the CE runlist is PTOP's | PTOP read | `ce=ce0 eng=4 rl=4`, PBDMA2 runm `00000010` serves it | **confirmed f25** (`pb2=00000010`) |
| R3 | **BIND wall: SNOOP_WITHOUT_BAR1** — Kepler's USERD lives in ONE BAR1-polled table named by 0x2254; with 0x2254 = 0 every bind errors and the runlist commit never completes | 0x2254 = 0x10000000 or table>>12; USERD moved to table + chid*0x200; 0x2100 bit 0 W1C before the bind | `bind_post=00` and `commit_us=<n>` confirm; `bind_post=02` refutes the encoding (then alt R3b) | **open** — wire: `pfifo_intr=00000001` (bit 0 BIND_ERROR, gk104.c:660) + `err=00000002` at 0x252c (code 2 SNOOP_WITHOUT_BAR1, gk104.c:600) + `userd_bar1=00000000` + `commit_us=stuck`; the fix flies first on flight 26 |
| R3b | alt: the BAR1 VM does not map the table identity (KF24) | — | `bind_post=02` with r2254 post set | open (reopens if R3 refutes) |
| R3c | alt: the error is the GR leg's (chid 1), not ours | — | `bind_pre` vs `bind_post` around OUR bind | open — discriminated this boot |
| R4 | RAMFC differs from `gk104_chan_ramfc_write` | +0x0C, +0x94, +0xE4, +0xF8, +0xFC | `ramfc_*` readback = nouveau's; fetch with a PBDMA intr0 (GPENTRY/PBENTRY/METHOD) refutes R4's sufficiency | open (applied) |
| R5 | runlist blocked by SCHED_DISABLE 0x2630 bit rl | 0x2630 &= ~BIT(rl) | `r2630` pre has bit rl set + commit now completes confirms | open (applied) |
| R6 | 0x2a04 must carry nouveau's 0xbfffffff | 0x2a04 OR | `r2a04` pre->post | open (applied) [ONE-SOURCE: nouveau] |
| R7 | PBDMA 0x04013c bits 8/28 must be clear for the CE's PBDMA | 0x04013c &= ~0x10000100 | `pb13c` pre->post | open (applied) [ONE-SOURCE: nouveau] |
| R8 | sysmem PTE: VOL bit 32 + aperture HOST(2) at bit 33 = dw1 5 (ours 2 = aperture 1) | every sysmem PTE dw1 | `pte_hi=5`; a CE0 MMU fault (unit 0x15) with reason PTE/UNSUPPORTED_APERTURE at SYS_VA refutes | open (applied) — the wall AFTER the fetch |
| R9 | the PDE span follows the big-page size the firmware left (0x100c80 bit 0: 64 KiB -> 64 MiB span, else 128 MiB) | PD layout | `fb_page=` and `pde_span_mb=`; a CE0 fault reason PDE/PDE_SIZE at SYS_VA refutes | open (applied) [ONE-SOURCE: nouveau] |
| R10 | copy class / LAUNCH_DMA / host semaphore encodings (GPUBLIT2-pinned) | pushbuffer | `selftest=ok` confirms; `mismatch@n` or PBDMA METHOD intr refutes | open (unreached) |

## 3. This boot's tree (flight 26 reads it)

All of R3..R9 are carried together: none writes a register another writes, and the bind (R3/R4/R5/R6/R7) precedes the fetch
(R8/R9), so the dump tells them apart by STAGE:

1. `bind_post=` (0x252c after OUR bind, after clearing 0x2100 bit 0): `00` -> R3 confirmed (and R3c: if `bind_pre=02` it was
   already latched). `02` -> R3 encoding refuted -> R3b. `05` INVALID_RUNLIST -> R2 reopens. `06` INVALID_CTX_TGT -> R4 (inst/PD target).
2. `commit_us=<n>` -> the runlist took. `stuck` with `bind_post=00` -> `r2630` pre bit rl (R5), then R6/R7 are the candidates.
3. `ramfc_get`/`pb2 get` > 0 -> the fetch started: R0's wall is past. `sem` released -> the self-test verdict decides R10.
4. Fetch started, no semaphore: `pfifo_intr` bit 28 -> `mmu=` mask and `fault=` decoded (unit, client, reason, VA):
   CE0 client at VA >= 128 MiB with PTE/UNSUPPORTED_APERTURE -> R8; PDE/PDE_SIZE -> R9; bit 29 -> the PBDMA's `intr0` decoded
   names (GPENTRY/PBENTRY -> R4's GPFIFO words; METHOD -> R10).

## 4. Walls known and unapplied

- None of GPUBLIT2's three is left: PTE (R8), RAMFC (R4), 0x2254/0x2a04 (R3/R6) all fly together.
- `0x2140` (PFIFO_INTR_EN, gk104.c:752) is NOT written: no PFIFO interrupt handler exists, and enabling it would raise the
  PCI line unhandled. The status words are read, not taken.
- PBDMA INTR/INTREN 0x040108/0x04010c and HCE 0x040148/0x04014c (gf100.c:358-359, gk104.c:385-386): read (intr0 in the dump),
  not written; they report, they do not gate the fetch (nouveau sets them for its interrupt path).
- KCOMP: on `selftest=ok` the hot path already takes `copy_wait` (B371 M2, `video/blitter.rs:243`); nothing to add.

## 5. Constants table

| constant | value | citations |
|---|---|---|
| USERD BAR1 table | 0x2254 = 0x10000000 or bar1_va>>12 | nouveau gk104.c:749, base.c:305-319 ("USERD + BAR1 polling area"); bind reason gk104.c:600 + wire `err=00000002` (two of three) |
| USERD per channel | table + chid*0x200 | nouveau gk104.c:116 (size 0x200), chan.c:463; GP_GET/PUT +0x88/+0x8c gf100.c:129-130 |
| BIND reasons | 01/02/03/05/06/0b | nouveau gk104.c:598-605 |
| PFIFO_INTR bits | 0,4,8,16,23,24,27,28,29,30,31 | nouveau gk104.c:660-720 |
| RAMFC words | +0x0C upper(userd), +0x94 0x30000000 or 0xfff, +0xE4 0x20, +0xF8 0x10003080, +0xFC 0x10000010 | nouveau gk104.c:88-102, devm/priv gk104.c:110-111 [ONE-SOURCE: nouveau] |
| SCHED_DISABLE | 0x2630 BIT(rl) | nouveau gk104.c:412/418 [ONE-SOURCE] |
| 0x2a04 | OR 0xbfffffff | nouveau gk104.c:740 [ONE-SOURCE] |
| PBDMA 0x04013c | clear 0x10000100 | nouveau gf100.c:357 [ONE-SOURCE] |
| sysmem PTE dw1 | 5 (VOL bit 32, HOST aperture 2 at 33) | nouveau vmmgf100.c:263,314-318,328 [ONE-SOURCE; refutable by fault decode] |
| PDE span | 0x100c80 bit 0 set -> 64 MiB (SPT 14 bits) else 128 MiB (SPT 15 bits) | nouveau fb/gf100.c:72-73, vmmgk104.c:41,55 [ONE-SOURCE] |
| MMU fault regs | 0x259c mask; 0x2800+unit*0x10 inst/valo/vahi/type | nouveau gf100.c:696-728; units gk104.c:494-495, reasons :504-521, hubclients :525-558 |
| engine status | 0x2640+eng*8 busy/faulted/chsw/load/save | nouveau gk104.c:206-216 |
| PBDMA idle | 0x3080+i*4 & 0xe000 | nouveau gk104.c:293 |

## 6. Unwind

Every register written prints its PRE-image on the wire (`r2254`, `r2a04`, `r2630`, `pb13c`, `bind_pre`). None is destructive:
all are fifo-init state nouveau leaves set for the driver's life; restore = write the printed pre-image (no code path does so
today — a CPU-selected boot leaves them, as GPUBLIT2's boots left the bound channel). 0x2254 changes the USERD table for EVERY
channel: the GR leg's chid 1 now polls `table + 0x200` (zeroed), not its own USERD — that leg already never validated
(`err=00000002`, the same SNOOP_WITHOUT_BAR1), so nothing it had is lost; the seat should know it is now behind the same fix.
The CE's 2 MiB table is VRAM inside the window, reached only through BAR1 identity (KF24).

## 7. What the next flight reads (in order)

1. `:: GPUBLIT: selftest=` — `ok` -> `[wc] blitter=gpu`, then `tests blitter` `:: KCOMP: blitter=gpu ... gpu_us=<n>` is the win.
2. On non-ok: `[gpublit] walls ... bind_pre= bind_post= r2254= r2630= ...` -> §3 step 1-2.
3. `[gpublit] host ... commit_us= ramfc_get= pb2=...` -> step 2-3.
4. `[kfifo] decode chid=2 pfifo_intr=<w>[names] bind=... eng<n>=... pb2_intr0=[names] ... fault=...` -> step 4 (KEPLERGR lifted
   the decoder into `kepler_fifo::host::pfifo_decode`; the `[gpublit] decode` line no longer exists), then — KEPLERGR2 (B463) —
   `[kfifo] unwind chid=2 preempt=... restored=<n> mismatch=<n>`: the non-ok self-test takes the channel back off the hardware.
5. The falcon fold (`family=ucode ... POISON`) is NOT this arc's; the walls line's `bind_pre` says what PFIFO owed the GR leg.

## Milestones

- M1 — R3..R9 applied together, each pinned `// nouveau <file>:<line>`; the `[gpublit] walls` line.
- M3 — the `[gpublit] decode` line: PFIFO_INTR, the bind reason, the CE's engine status, the PBDMA idle/intr0 words and the
  MMU fault unit, all decoded bit by bit against nouveau's tables.
- (M2 is B371's KCOMP hot path, unchanged: `selftest=ok` arms it.)
