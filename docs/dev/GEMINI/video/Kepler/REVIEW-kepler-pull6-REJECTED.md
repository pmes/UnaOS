# REVIEW — Kepler pull 6 (commit fa50954e): WALL 1 REJECTED. Do not send to metal.

You implemented a CHANGES-REQUESTED item without making the change. Read this before
touching kepler.rs again.

## Wall 1 head address — REJECTED (second time)

You shipped `NV_PDISPLAY_BASE + 0x400 + (head * 0x300) + 0x60`.

That is the **exact address that already read uniform zeros on real silicon in sitting
#3** (pull 4, commit d938dd00). It is not a new derivation. The proposal was marked
CHANGES REQUESTED specifically because this address is circular, and the walkthrough
does not answer the question that was asked — it just re-asserts "correct NV_EVO_CORE
shadowed base." Re-asserting a claim that metal already falsified is not a derivation.

The bad-read guard does NOT make this acceptable. The guard will fire, log zeros, abort
— and a full bench sitting will have been spent re-confirming a fact we have had since
sitting #3. That is the exact waste the guard exists to prevent, not license to ship a
known-bad address.

**What you must actually do before this goes anywhere near metal — pick ONE, with
evidence:**
1. If your theory is "the ARMED shadow is only populated after the EVO core channel is
   activated," then PROVE the read currently happens before activation, and move the
   read to after. State the ordering in the walkthrough with line references.
2. Name the real alternative you were given and did not address: **if the firmware
   posted this display through VBIOS/CRTC direct register writes rather than the EVO
   core channel, the EVO armed shadow is legitimately zero** and scanout must be read
   from a different source — the ISO/DMA hub, or the direct CRTC/head timing registers.
   Derive THAT source from the XMLs (cite the file/offset) and read it.
3. If you cannot decide between 1 and 2 from the docs, instrument BOTH candidate
   sources in one boot (each behind the bad-read guard) so the sitting reads all of
   them at once and the non-zero one wins. That is a real one-boot experiment. Reusing
   the zero address is not.

Do not resubmit wall 1 with the `0x400 + head*0x300 + 0x60` address. If it appears
again unchanged the review will not read past it.

## Wall 2 (PBDMA bind) — ACCEPTED
`SUBFIFO_ENG_MASK[0]=1` at `0x2390` binding PBDMA 0 → PGRAPH is correct per the
approval; `pbdma-eng-mask set` witness in place. Keep it.

## §3 cleanroom — ACCEPTED
`0x490` nouveau citation removed, honest empirical note in place, core_ctrl read now
behind the bad-read guard with a `bad-core-ctrl` abort. Correct. Keep it.

## Net
Kernel compiles green both arches with all knobs — that is not the bar. Wall 1 is a
known-bad address. Fix wall 1 as above; wall 2 and §3 can stand. Nothing here reaches a
sitting until wall 1 is a genuine new derivation.
