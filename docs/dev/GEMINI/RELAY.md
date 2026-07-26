# PETER'S RELAY SHEET — not for specialists. Coordinator overwrites this with
# the message(s) Peter posts into each Gemini chat, verbatim.
# (updated 2026-07-26: pull 33 landed w/ 2 flags; pull 34 invited — H2/H3 writes + a bounded echo)

## → kepler-fence session

Fence: pull 33 is landed and gated, and it rides the next sitting. Two land-review flags, neither blocking:

1. The proposal doc was never updated with the corrected listing. The amendment asked you to re-derive the encoding for the indexed ports and cite the instruction form you picked — the code changed, the doc did not. Fold the final listing + citations into PROPOSAL-kepler-fence-pull33.md so the record matches the silicon.
2. THE ECHO LOOP IS UNBOUNDED. Pull 27's own safety argument was that a Falcon spinning forever through the rest of boot is a wedge risk we do not take — and the echo polls forever by construction. It ships this once (the panel is up before this block and every prior running-falcon boot was benign), but it must not become the pattern.

Also for your notebook, from the coordinator side: I added ONE control leg to the witness sequence this boot. Every witness write since sitting #7 has been 0xC0000000 = VALID **plus POLL_ENABLE**, and the chip's own name for err=2 is NO_POLL ("validated a channel with POLL_ENABLE, but poll area is disabled"). Sitting #8 wrote VALID-only and the bit still stripped — but s8 predates the error readback, so nobody has ever read err= with POLL clear. The new line is `:: kepler: poll-control valid-only chan=… err=… stat=… ::`. If err stays 2, the chip's reason name is a red herring we have honored for 28 sittings; if it changes, that code names the real precondition. Your legs are untouched controls.

PULL 34 INVITATION (proposal-first):
(a) BOUND THE ECHO — add a host-commandable exit: host writes a sentinel (say 0xFF) to CC_SCRATCH[0], ucode exits cleanly, host confirms cpuctl shows STOPPED. Restores the pull-27 discipline and gives us a clean "the ucode obeyed a second command" observable for free.
(b) H2/H3, never actually executed — your own STUDY listed them and s34 proved both offsets READABLE, so the pull-28 amendment now permits the writes: write ENGINE_STATUS (0x409C00) = 0x2 (CHAN_VALID) and read back; if it sticks, write ENGINE_TRIGGER (0x409C08) = 1 and read back. Readback after EVERY write, FAULT-skip discipline as pull 31. A sticking CHAN_VALID would shrink the whole ucode era to a bind sequence; a zero readback is the first direct evidence that ENGINE_STATUS is falcon-owned — which the authoring plan currently assumes without proof in either direction.
(c) If the s37 echo acked: say which image won and what that settles about the CC_SCRATCH port encoding, and propose the next ucode rung (reading a real ctx-relevant register from inside the falcon).

Bound every loop. Full listing + citations. Commit ALL docs+code, no push. Report "PUSH OWED: n".

## → kepler-display session

Display: idle and graduated. The console's paint path was restructured after a code review (layout under the interrupt mask, pixels painted outside it) — s37 re-verifies text on glass, since that path has no QEMU coverage.
