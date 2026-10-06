# KEPLERGR2 — the September GR leg through the ONE bind path; the CE leg's executed unwind (ledger B463)

Branch `exec-rmbp-keplergr2` (cut from f4e0613c, merge18). Continues `keplergr.md` (B421) and `gpublit3.md` (B410); structured
by DRIVERS-METHOD §6. Files: `drivers/gpu/kepler_fifo.rs` (`host`: gate widened to the fifo leg, `bind_ext`, the leg's state
word; `kgr`: `sept=` on its walls line), `drivers/gpu/kepler.rs` (the September leg's RAMFC / USERD / binds / runlist submit
routed; the `SeptRoute` seam at the file's tail), `drivers/gpu/mod.rs` (module gate), `drivers/gpu/kepler_gpublit.rs` (the
unwind on a non-ok self-test; `tests gpublit` re-lays before its rerun), `gpublit3.md` §7, `unaos/scripts/kepler-capture.list`.
No new file under video/ fs/ install/ selfhost/ or the shell files (no CHARTER line owed), no knob, no verb, no dotfile.

**Finding (from the wire, f25-boots.log 08:19:39Z).** The September leg (`kepler.rs`, `nvidia-kepler-fifo`) ran on image 18:
`fold family=pbdma n=2 last=pbdma-eng-mask set`, `family=pfifo n=2 last=PFIFO_CHAN[1] post-submit: 00=00002210 04=11000001`,
`family=sched n=3 last=sched-status post-submit err=00000002 (present) stat=00000005`. It runs AFTER `kepler_gpublit::arm`
and wrote its own RAMFC (+0x0C bit 31, +0x94 devm 0, +0xE4 0), its own 4 KiB USERD page outside the 0x2254 table, bound chid 1
seven times in its own encoding (hi = 0x400, which zeroes the runlist field), and submitted a THREE-entry runlist naming
chids 1, 2 and 3 on runlist 0 — chid 2 is GPUBLIT's CE channel (bound on runlist 4). So on the next image (0x2254 set by
GPUBLIT3) the leg would be a second bind path on the CE's chid and on the GR chid `tests kgr` binds.

**Route, not retire** (the rung ledger, §2b): `tests kgr` covers every CHANNEL rung the leg had (fifo init, RAMFC, USERD, bind,
commit, fetch), but NOT its FECS/falcon rungs (fal-port, ucode POKE/ECHO, ctx-echo/poke, FENCE, H2/H3, recon, terminal poke) nor
its display mirror beacons — those are K-GPU-4's and the display's, and R19 keeps code and knob. So the leg stays, and its
channel half goes through `kepler_fifo::host`: ONE fifo_init, ONE RAMFC writer (`host::ramfc_write`), ONE USERD table (the
leg's USERD is `host::userd_slot(utab, 1)`; with no CE leg the leg registers its own page as THE table), ONE bind
(`host::bind_ext`; the leg's experiment variable POLL_ENABLE bit 30 rides as `lo_extra`), ONE runlist commit
(`host::runlist_commit`, one entry (chid 1, 0)), and the leg ENDS in `host::unwind` (before the terminal poke) so `tests kgr`
starts from an unbound chid 1. The CE leg's open question (KEPLERGR §4): yes — a non-ok self-test unwinds through the same
`host::unwind`, after the dump; `tests gpublit` re-lays the channel through `host` before its rerun and unwinds again on non-ok.

## 1. Capture

None of a WORKING state (unchanged from keplergr.md §1). M4 writes the bench's HOW into `unaos/scripts/kepler-capture.list`:
the Linux/nouveau boot, the commands that read each listed offset, no values. The reference image lands in
`docs/dev/evidence/<round>/gk107-capture/` when the bench runs it.

## 2. Rung ledger — carried from keplergr.md §2 (G0..G7, unchanged statuses) plus this arc's rungs

| # | hypothesis | writes | discriminator (confirm / refute) | status |
|---|---|---|---|---|
| G0..G7 | as keplergr.md §2 | as there | as there | as there (all open but G2 refuted s#37, G3 refuted s#10, G7 parked->reopened) |
| S0 | PREMISE: the September leg runs on the metal image and touches the CE's chid | none | f25 08:19:39Z `PFIFO_CHAN[1] post-submit ... 04=11000001` + runlist entries (chid 1, 2, 3) in the source | **confirmed f25 08:19:39Z** (fold `family=pfifo n=2 last=PFIFO_CHAN[1] post-submit: 00=00002210 04=11000001`) |
| S1 | with the leg routed, its chid-1 bind reads the same verdict as `tests kgr`'s (one path, one table) | via host | `[kfifo] sept bind` `bind=` == `[kgr] walls bind_post=` on the same boot; differing = a second path survived | open |
| S2 | the leg's three-entry runlist disturbed the CE (chid 2 named on runlist 0) | one entry (chid 1, 0) | `[kfifo] sept commit` then `:: GPUBLIT-TEST: rerun=` after it: a rerun `ok` that the boot's arm read non-ok, or the reverse, names an interaction | open |
| S3 | the leg's residue (chid 1 bound + enabled on GR's runlist) confounds `tests kgr` | `host::unwind` at the leg's end | `[kfifo] unwind chid=1 ... mismatch=0` at boot and `[kgr] walls ... sept=unwound` | open |
| C1 | the CE unwind leaves the board as the CE leg found it | `host::unwind` chid 2 | `[kfifo] unwind chid=2 ... restored=<n> mismatch=0` after `[kfifo] decode chid=2` | open |

## 2b. The September rungs vs `tests kgr` (the retire question)

| September family (f25 fold) | what it is | covered by `tests kgr`? |
|---|---|---|
| pbdma (eng-mask), pfifo, bind, witness, sched, runlist, discriminator | PFIFO init, RAMFC, chid-1 bind + witness, runlist submit/echo/scan, PBDMA CHANNEL read | YES — G1/G4/G5 (bind, RAMFC, commit), G6 (fetch, PBDMA chan in `[kgr] walls pbN=`); G2/G3 refuted by wire |
| ucode, fal, ctx, dmactl, h2/h3/h4, fence, recon, poll, terminal | FECS falcon port, ucode POKE/ECHO, ctx bind, FENCE, PRI recon, the 0x409504 poke | NO — K-GPU-4 (the falcon fold `family=ucode ... POISON`) |
| beacon, latch, disp, inst, bar1, hb, post | EVO mirror window, display latch, BAR1 identity | NO — display / KF24 |

Verdict: not every rung is covered, so the leg is ROUTED (its channel half through `host`), not retired.

## 3. This boot's tree (a boot with `nvidia-kepler-fifo` aboard, as images 17/18 were)

1. `[kfifo] sept route chid=1 rl=<n> src=<ptop|assumed> utab=<x> owner=<ce|sept> userd=<x> bind_pre=<b> intr_pre=<w>`:
   `owner=ce` = the leg is behind GPUBLIT3's table (one table). `owner=sept` = no CE leg this boot; the leg's page is THE table.
2. `[kfifo] sept bind lo_extra=<x> bind=<b> intr=<w> chan=<lo>/<hi>` (per bind site): `bind=00` -> G1 holds for chid 1 under the
   September encoding too. `bind=02` with `owner=ce` -> G1b (the table's BAR1 mapping) for BOTH chids.
3. `[kfifo] sept commit rl=<n> count=1 commit_us=<n|stuck>` -> G5 for GR's runlist.
4. `[kfifo] unwind chid=1 preempt=... restored=<n> mismatch=<n>` before the terminal poke -> S3.
5. CE leg: on a non-ok `:: GPUBLIT: selftest=` -> `[gpublit] chid=...`, `[gpublit] host`, `[gpublit] walls`, `[kfifo] decode chid=2`,
   then `[kfifo] unwind chid=2 ...` -> C1.
6. Operator: `tests kgr` -> `[kgr] walls ... sept=<off|bound|unwound>` (S3) and keplergr.md §3's tree.

## 4. Walls known and unapplied

- The leg's own PFIFO preamble still writes PMC bit 8, PMC_PBDMA_ENABLE = all ones and PBDMA0's runlist mask 0x2390 = 1 (pull-era
  writes, not bind/RAMFC/USERD); nouveau writes 0x2390 per PBDMA from PTOP (gk104.c:392) — not routed here, owed to the next rung.
- The ctrlbind (KF9b) restore writes PFIFO_CHAN[1]'s captured pre-image words (an unwind, not a bind); left as is.
- PFIFO_INTR_EN 0x2140 (gk104.c:752): not written (no handler), as GPUBLIT3/KEPLERGR.
- The leg's POLL_ENABLE (bit 30) is the September experiment variable [ONE-SOURCE: the s#9 naming; nouveau never sets it,
  gk104.c:68]; carried as `lo_extra`, refutable by S1.

## 5. Constants table

Every constant is `host`'s (keplergr.md §5). New: runlist entry (chid, 0) for the leg — nouveau gk104.c:454-455 (as kgr/GPUBLIT);
RAMFC +0x0C restore = upper_32(userd) — gk104.c:89 (the September `| 0x80000000` is dropped with the rest of its RAMFC).

## 6. Unwind

The leg: `host::Pre` captures (by READING) 0x2a04, PBDMA 0x04013c of GR's PBDMA(s), 0x2254, 0x2630 in `fifo_init`; `host::unwind`
preempts chid 1, ENABLE_CLR, unbinds, commits GR's runlist EMPTY (count 0, page never read) and restores newest first, each read
back — before the terminal poke. Under `owner=ce` the 0x2254 / 0x2a04 pre-images are the CE's values (restore = no-op on them).
The CE leg: `kepler_gpublit` keeps its `fifo_init` pre-image record (was dropped) and on a non-ok self-test runs `host::unwind`
for chid 2 (empty page = the unused W_USERD page) after the dump. `host::last_bind` is the boot's bind VERDICT and survives the
unwind (kgr's guard reads the CE's verdict, not its liveness). Destructive: no new destructive write.

## 7. What the next flight reads (in order)

1. `:: GPUBLIT: selftest=` — `ok`: unchanged. Non-ok: `[gpublit] walls ...`, `[kfifo] decode chid=2 ...`, `[kfifo] unwind chid=2 ... mismatch=0`.
2. Fifo-leg boot: `[kfifo] sept route ... owner=ce`, `[kfifo] sept bind ... bind=`, `[kfifo] sept commit ...`, `[kfifo] unwind chid=1 ...`.
3. `tests kgr` -> `[kgr] walls ... sept=unwound`, `[kfifo] decode chid=1`, `:: KGR: bind=... -> <verdict> ::`.
4. `tests gpublit` -> a re-lay (`[gpublit] relay`) then the rerun line; on non-ok, decode + unwind again.

## Milestones

- M1 — `host`: gate widened to `nvidia-kepler-fifo`, `bind_ext` (lo_extra), the leg's state word, `last_bind` kept across unwind; kgr `sept=`.
- M2 — `kepler.rs`: the September leg's RAMFC / USERD / seven chid-1 binds / runlist submit through the `SeptRoute` seam -> `host`; unwind at its end.
- M3 — `kepler_gpublit`: pre-image record kept; `host::unwind` on a non-ok self-test; `tests gpublit` re-lays first.
- M4 — `gpublit3.md` §7 step 4 = `[kfifo] decode chid=2`; `kepler-capture.list` gets the bench's HOW.
