# GEN7WAKE (B489) — the IVB forcewake ack, polled where Ivy Bridge puts it

Branch `exec-rmbp-gen7wake`, cut from 21521a53 (merge19). One rung (R101): R3's MT handshake.

## Finding (flight 26 boot 3, image 19, `tests gen7`, read with awk)

- `r3 cand=mt … wrote=00010001 req_post=00000000 ack_off=130044 ack_post=00000000 … classification=fw-no-decode iters=3288 cyc=20000832`
  — the request was written to `0xA188` (FORCEWAKE_MT, mask form), the ack polled ~8 ms on `0x130044`.
- `r3 col=mt blk=bcs name=RING_CTL v0=0000F001`, `RING_START v0=04000000`, `INSTPM v0=00000010`,
  `RENDER_IDLE_POLL v0=00000005 varies=1` — under that hold the GT block was LIVE (4 structured, 1 varies).
- `r3 step name=FW_RELEASE_MASK off=00A188 wrote=00010000 pre=00010001 post=00010000` — the request
  register READ BACK our write once the GT was up. The writes land; "MMIO writes do not land" is wrong.
- `r3 cand=renfw … gone_struct=0`; after release the battery is dark again. The MT request is the wake.
- So the wake is real and the ack register is the defect: `0x130044` is Broadwell's GTSP1 (the tree's
  BDW pin). Ivy Bridge's MT forcewake ack is `FORCEWAKE_MT_ACK 0x130040`, data bit 0 = the kernel
  thread's bit (the Linux i915 IVB forcewake domain: `FORCEWAKE_MT 0xA188` / `FORCEWAKE_MT_ACK 0x130040`,
  chosen when `ECOBUS 0xA180` bit 5 `FORCEWAKE_MT_ENABLE` reads set under a hold; ack timeout 50 ms).
  The PRM never published the Gen7 GPM block (gen7.md §2.3); this pin is EXT (the open-source driver),
  carried as `class=IVB-EXT-I915` on the wire, not as a PRM citation.
- gmux is NOT the cause: the panel is the Kepler's on every boot and the GT still answered under the hold.

## Seam

Kernel driver (`drivers/gpu/gen7.rs`, existing file, no new file, no new knob — rides `gen7`/`gen7r8`).

## Milestones

- M1 — `g7regs`: `IVB_FORCEWAKE_MT_ACK 0x130040`, `IVB_ECOBUS 0xA180` (bit 5), `IVB_GT_CORE_STATUS 0x138060`
  (bits 2:0 = RC state, read FIRST, before the request). R3 candidate A and `R6_CANDS` row `mt` poll
  `0x130040` bit 0 (mask 1) under a 50 ms budget. Under the hold R3 reads ECOBUS; the witness prints.
- M2 — the ladder: `GEN7LADDER … wake=acked` on an acked wake; an unacked wake is `-> DECLINED reason=`
  with the reads that prove it (ack, ECOBUS, RC state), not a FAIL; `Dark` stays FAIL.

## Witness (the next flight reads)

    [gen7] wake req=00A188:00010001 ack=130040:00000001 after_us=<n> rc=<0..7> ecobus=<v> mt_enable=1 -> acked reason=mt-ack-bit0-set
    :: GEN7LADDER: rungs=R1-R7 ggtt=boot-bank wake=acked r7=<…> us=<n> replay=0 -> PASS|FAIL ::
    or  -> DECLINED reason=<mt-enabled-ack-silent|ecobus-mt-disabled-legacy-pair-owed> ack=130040:<v> ecobus=<v> rc=<v>

## Owed

- If ECOBUS bit 5 reads 0 under the hold, IVB runs the LEGACY pair (`FORCEWAKE 0xA18C` / `FORCEWAKE_ACK
  0x130090`); that is the next rung. The tree's third R6 candidate writes `0x130090` as a REQUEST — on IVB
  that offset is the legacy ACK; left untouched here, named for the seat.
- R4..R8 on an acked wake have never flown on this part; this arc opens their gate, it does not prove them.
