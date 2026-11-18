# wave36 cloud seat: saved state (stopped 2026-11-17, by order)

Copied from the seat's scratch directory so the next session can resume. Nothing here is a rule.

- `fold-notes.txt`: every fold compose decision and WATCH line; the END holds the stop state.
- `foldq.parked.txt`: the parked fold queue, in order (name|SR id|title|test command|sheet row). `parked-branches.txt`: its branch names.
- `foldq.sh` / `foldh.sh` / `foldresumeh.sh` / `foldlib.sh` / `fold.sh` / `keepboth.py` / `rowsplice2.py`: the fold queue runner.
- `arcpatch.sh`, `rowspan.py`, `rowreplay2.py`, `l106splice.py`, `l106fix.py`: the seat's compose tools (see fold-notes for use).
- `cut70h.py`, `cut143h*.py` .. `cut145h*.py`, `HEAD.md`, `launch-prompt.txt`: the arc generators and executor rules.
- `cut145h/`: briefs and designs of the last wave, stopped mid-build.

Paths inside the scripts point at `/root/unaos-bench/scratch/cloud/wave36`; restore the files there (or edit `W=`) before running.

State at the stop: trunk carries SR1156 (text-justify) as its last fold. Pushed, NOT folded: the branches in `parked-branches.txt` (SR1161, SR1158, SR1159, SR1164, SR1160, SR1165, SR1157, SR1163, SR1162, SR1168, SR1169, SR1167, SR1166, then the HEVC rows, SR1080, SR1006, SR1020). Pushed as WIP, not complete (no MEASURE line on the tip, do not fold as is): exec-rmbp-aethercssomcsssupports (SR1170), -aethercssomboundingstickynested (SR1173), -aetheroutlinestickyheld (SR1174), -aethercssomscrollintoviewnested (SR1175), -aetherdirpseudoauto (SR1176). Designed only: SR1171 (scroll memory per document) and SR1172 (logical properties vs the dir attribute); their designs are in `cut145h/`.
