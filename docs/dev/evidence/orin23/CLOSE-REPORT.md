# orin 23 — CLOSE REPORT (2026-09-08 23:15Z → 2026-09-09 ~03:00Z; ended by Peter)

## Landed (on hw-jetson)
- render11 flight filed: `FLIGHT-render11-RESULT.md` (scorer11 6/6 exit 0; §18 batteries; FRIEND-DIFF three boots, two passes, no
  UnaOS entanglement; positive control designed, owed). orin 22's whole record filed out of scratch (`evidence/orin22/`).
- `./arroyo state` (branch@HEAD, behind/ahead trunk, owed above origin, dirty). Ledger fixes S30/A27/C10. RULINGS R32–R38.
- Bench: `~/unaos-bench/tools/media-writer.sh` (Peter's name; moved out of scratch, repaired: sudo HOME, partition mount, no
  sandbox spawn under root, harvest to flash/, chown). First real card write of that tool: render11.

## Executor branches (each gated on its own; the FOLD has had no gate)
| branch | tip | state |
|---|---|---|
| exec-orin23-closemin | 33fba12c | close releases focus, never raises the shell — gated, go-red proven |
| exec-orin23-cursorbg | b753a667 | vacated/uncovered regions restored from the scene — gated, go-red proven |
| exec-orin23-apsrun | 25c2203f | every secondary drops to EL1 and hosts (`apsrun` default-on) — gated incl. kernel8-test 125/125 |
| exec-orin23-facet | 3c6e406a | Facet: PNG viewer tenant, opened from the file manager — gated, fixture incl. corrupt-IDAT refusal |
| exec-orin23-prtscrsrc | 227f8c9c | capture witness names its volume (rung/source/serial/label) — gated |
| exec-orin23-netlease | eb0eaab7 | frame delivered at its own length, consumed on read; DHCP offer no longer truncated — gated, go-red proven |
| exec-orin23-fold | 40694128 | the six above merged clean; ledger ids unique; NO gate yet |
| exec-orin23-dockid | e86d3485 | WIP, UNGATED; its own finding: three `settle(...)` calls commented out by a folded `//` |
| exec-orin23-wintitle | af9a7e02 | WIP, UNGATED; R36 titles; selftest not written |
| exec-orin23-compgate | d81c9717 | WIP, UNGATED; base = fold; A46 compositor gate; wcg.rs half + fixture not written |
| exec-orin23-ga10b4 | e7c8eb24 | GA10B rung 4 brief (docs); three questions for Peter in §8 |

## Open, named
- B98 clone-merge fix: half-applied in the bootroot agent worktree; design = one `fat::same_device` predicate, two callers
  (rmbp grant on prtscr.rs recorded). Landing-blocking for exec-orin22-bootroot.
- Tearing (A46): mechanism found (compositor re-entered under the 12 ms preempt quantum; cross-core once six cores host); fix WIP.
- NIC descriptor-17 behaviour: OPEN (R19), never "ruled out"; the driver now delivers correctly through it.
- FRIEND-DIFF positive control: USB stick + root write-locked; PRTSCR-VOL makes it readable.
- RULINGS R24/R25 double-booked across tracks (SR5); R26 will collide at landing; both positions recorded; Peter's record.
- shell.rs `screenshot` verb still prints an OK line with no device (two-line follow-up).

## Defects of this seat, on the record (R34, R37, R38)
Full battery written into every brief; "not this arc" answered to glass defects; two card lines that could not run from Peter's
terminal; a stale baton title relayed to Peter as fact; process talk when outcomes were wanted. Peter lost his morning.
