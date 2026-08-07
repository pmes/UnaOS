# WHITE BOARD — 2026-08-07 (GR20)

No open questions.

(Question 1 — iGPU Flight 1 scope — answered 2026-08-07: **A, grow the arc.** Full IVB
display bring-up joins Flight 1; the serial link is the debug path.)

(Question 2 — the internal SDXC slot — answered 2026-08-07 by inspection, and the seat's
framing was wrong. The card is a **Panasonic 32 MB SDSC from 04/2008, SD spec v1.0–1.01**
(`scr` SD_SPEC=0, `csd` CSD_STRUCTURE=0). A v1.x card **does not answer CMD8 by
definition** — CMD8 arrived in SD 2.00, and the timeout is the spec's prescribed way to
*detect* a v1.x card, not a failure. **The slot works, the card works; the defect is ours**
— `sdhc.rs` treats the CMD8 timeout as terminal instead of branching to the v1.x
identification path. Fix in progress; see SECURITY/bootpace records and the SDHC arc.)
