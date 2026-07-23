STATUS: APPROVED WITH AMENDMENTS (2026-07-23 — see REVIEW-kepler-fence-pull14.md; amendments are binding)

# Proposal: kepler-fence pull 14 — PBDMA CTRL_ADDR TARGET audit & disp-era fallback

## Milestone 1: PBDMA `CTRL_ADDR` TARGET Audit
As documented in `gf100_pfifo.xml`, `PSUBFIFO` (PBDMA) resides at `0x40000` with a stride of `0x2000`. The `CTRL_ADDR_LOW` register is at offset `0x08` within each PBDMA base, and bits 0:1 contain the `TARGET` enum for the instance block / USERD pointer target memory space.

1. **Initial Read & Report:**
   Read `CTRL_ADDR_LOW` (offset `0x08`) and `CTRL_ADDR_HIGH` (offset `0x0C`) for all 3 PBDMAs (bases `0x40000`, `0x42000`, `0x44000`).
   Print the exact marker: `:: kepler: ctrladdr pbdma<N> pre=XXXXXXXX ::` for all three.

2. **Step `TARGET` Encodings:**
   The `TARGET` field (bits 0:1) can hold 4 possible values. We will iterate through all 4 target values (0, 1, 2, 3), representing VID_MEM, SYS_MEM, etc.
   For each target value `T` (0..3) and for the relevant PBDMA(s) (we will write to all three to be certain, or just the ones that are populated):
   - Modify `CTRL_ADDR_LOW` by replacing bits 0:1 with `T`.
   - Print: `:: kepler: ctrladdr pbdma<N> try target=<T> wrote=XXXXXXXX rb=XXXXXXXX ::`
   - Re-run the s10 witness ladder (using the existing `WITNESS PASSED`/`FAILED`, `sched-status`, `DISCRIMINATOR`, and fence poll logic).
   - If the witness fails (err=2 remains), we will **evidenced restore** the original `CTRL_ADDR_LOW` word and print: `:: kepler: ctrladdr restored pbdma<N> rb=XXXXXXXX ::`.
   - If we read `0xFFFFFFFF` or `0xBAD0BA20`, we will print the absence marker: `:: kepler: ctrladdr pbdma<N> ABSENT? rb=XXXXXXXX ::`.

## Milestone 2: Disp-Era USERD Enablement (Fallback)
If M1 does not resolve the NO_POLL reject, the USERD pointer enablement may be tied to the PDISPLAY or EVO-core engine block, per the fallback framing.
1. We will perform a read-only reconnaissance of the PDISPLAY base (`0x610000` / `0x616000` block) and the EVO channel setup area (`0x610490`, etc.).
2. We will look for EVO core-channel offsets that might gate PFIFO poll areas, logging the exact words before attempting any writes.
3. If writes are attempted based on the reads, they will use their own markers (`:: kepler: disp-userd-recon ... ::`) and will be cleanly restored on failure.

## Gates & Compliance
- The implementation will satisfy the full-knob gate (`UNAOS_IVB UNAOS_KEPLER UNAOS_KEPLER_TAKEOVER UNAOS_KEPLER_FIFO ./arroyo check`).
- Exact grep-able markers will be strictly preserved and printed exactly as briefed.
- No blind/unbounded loops (polls bounded as fixed in pull 8).
- `git status` clean, scratch files deleted before commit.
