# FOLD: the five paused executors folded onto the LOGIN14 branch (2026-09-24)

Branch `claude/optimistic-ramanujan-r3qyu5`, cloud session (focus rmbp). Folds, in order, each a `--no-ff`
union merge of the WIP tip the seat committed on 2026-09-23 (rows B201, B202, B199, B200, B203):
BOOTSLOW, LFNMV, WCDMEM, MENULOCK, GLASSFIX3. Conflicts were tail-appended blocks on both sides in every
case; one `}` lost at the WCDMEM/GLASSFIX3 seam in `video/wm.rs` was restored by brace and line arithmetic
against both parents over the base before any compile.

## Gates

Run on the fold tip, recorded here as they report (see the rmbp-queue STATE for the rc list until then).

| gate | result |
|---|---|
| `./arroyo check` on 8de29731 (nightly 2026-07-14, `x86_64` 0.15.5) | rc=1, 19m31s: GATE-BRACES 209/209 balance; **all 84 kernel legs rc=0** (the same 84 the clean-tree baseline has); the rc=1 is the container's, identical to the clean baseline's: the host ring-3 suites that need OpenSSL/ALSA/GTK headers (aether, phonolite, resonance, stria, una), `matrix --test finder` `write_to_readonly_dir_surfaces_loud_denial` (a read-only directory is writable to root, and this container runs as root), and GATE-LEDGER's 245 pre-existing findings (the fold's rows add 0: 245 before and after) |
| the same gate on the previous fold tip 90208e1c | rc=1 in 1.6 s: GATE-BRACES `arch/x86_64/syscall.rs` {=2799 }=2798 — the LFNMV union had lost a `}` at the LOGIN13/LFNMV seam; restored in 8de29731. Two seams, two lost braces (WCDMEM/GLASSFIX3 in `wm.rs` caught by hand arithmetic, this one by the gate): count braces against BOTH parents over the base on every union, and run the gate before the compile |

