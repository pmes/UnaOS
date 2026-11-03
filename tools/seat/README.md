# tools/seat — the cloud seat's fold and compile scripts

These lived in the seat's scratch (`/root/unaos-bench/scratch/cloud/wave36`) and died with each container.
They are checked in so a fresh cloud session can fold and compile on its first command. Nothing here is a gate;
the gates are `unaos/arroyo` and `unaos/scripts/`. PLAYBOOK.md names the order; LAWS.md the rules.

| file | what |
|---|---|
| `foldlib.sh` | `source` it: `foldone <n>` merges `exec-rmbp-<n>` without committing and unions the sheets (`sheetmerge.py`); `foldcommit <n> <id> "<title>"` refuses conflict markers and a red blueprint, registers the row in `docs/dev/exec-branches.txt`, commits with the executor's MEASURE line carried. |
| `fold.sh` | the merge step `foldone` calls. |
| `sheetmerge.py`, `sheetfix.py` | the LEDGER / sheet row unions at a both-changed merge; `sheetfix.py <branch> <merge-base>` asserts when a row anchor moved (compose that row by hand). |
| `seat-compile.sh` | the four kernel `cargo check` legs (x86 all-rows, x86 knob-off, Orin image, tegra conwin) printing `<leg> rc=<n> tip=<sha>`; the feature lists are the `kfeats-*.txt` beside it. |

The scripts take their scratch root from `W` (default the path above). On a fresh container:

```
mkdir -p /root/unaos-bench/scratch/cloud/wave36 && cp tools/seat/* /root/unaos-bench/scratch/cloud/wave36/
```

The setup-script recipe for the cloud environment (installs the pinned nightly, the kernel targets, QEMU for the
`./arroyo test` battery, the host-suite libraries, and pre-fetches the crates) is in `docs/dev/CLOUD-ENV.md`.
