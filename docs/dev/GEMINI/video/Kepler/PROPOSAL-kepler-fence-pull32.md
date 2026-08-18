STATUS: APPROVED (GR5, 2026-07-25) with ONE BINDING AMENDMENT: read and
print PFIFO_CHAN[1] word 0 BEFORE the rewrite as well (label
`witness pre-rewrite PFIFO_CHAN[1]=…`), so the capture carries the
resting value and the post-rewrite value side by side — self-explaining
pre/post, no inference needed. Everything else as proposed, including the
PLAYLIST_RD decode (accepted — the hardware reads our exact runlist) and
keeping pull 31's bind block byte-identical.

# PROPOSAL: kepler-fence pull 32 - The Register-Side Witness Strip Test

## Context
In s35, we learned that while `CHAN_CUR` and `CHAN_NEXT` accepted our writes (`00002000`), the `ENGINE_STATUS` `CHAN_VALID` bit did not assert. A bare MMIO bind is insufficient to satisfy the Context Control unit. We also learned that our post-bind witness leg was testing the *RAM-side* channel word (`inst_off+0x0C`), not the *Register-side* PFIFO channel table where the historic bit strip occurs.

## Decoding PLAYLIST_RD (0x2013 / 0x00100003)
According to `rnndb` (`fifo/gf100_pfifo.xml`), the `PLAYLIST_RD` (`0x2280`) and `PLAYLIST_RD_LEN` (`0x2284`) registers report the active runlist state for the engines:
*   `playlist_rd=0x00002013`: This precisely echoes the `target` physical address pointer we submitted (`runlist_off >> 12`). The hardware is confirming it is reading our exact runlist in VRAM.
*   `playlist_rd_len=0x00100003`: The low 12 bits (`0x003`) echo the `LEN=3` we submitted. The upper bits (likely bit 20) indicate active engine/runlist status (e.g., active/valid flag or engine ID).

## Implementation Plan (Pull 32)
We will insert a corrected, register-side PFIFO strip test immediately after our context bind writes:
1.  **The Write**: We will write `0xC0000000 | (inst_off >> 12)` (the `VALID | POLL` bits combined with the channel identifier) directly into PFIFO's channel table at `0x800000 + (1 * 8)`.
2.  **The Read-back**: We will immediately read back `0x800000 + (1 * 8)` and print it with a `witness post-bind PFIFO_CHAN[1]=...` label.
3.  **The Interpretation**: If PFIFO strips the `0xC0` bits, the readback will yield `0x00002000` (meaning the populated `CHAN_CUR` did not satisfy PFIFO's context validation). If the bits hold (`0xC0002000`), the bind succeeded from PFIFO's perspective.

## Compliance Gates
*   Pull 31's `CHAN_CUR` and `CHAN_NEXT` write/read block stays exactly as landed.
*   We replace the RAM-side `inst_off+0x0C` logic with the correct PFIFO register logic.
*   No execution; wait for approval.
*   Report "PUSH OWED: 20".
