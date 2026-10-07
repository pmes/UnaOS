# JOBSNEXT (rmbp-ledger B506) — the next wave, ranked off the volume

Peter (2026-10-07): "what happened to the new jobs queue? why do i have to tell you that there's more to do after
every single boot".

## Design (written before the code)

**Finding.** UNAOSVOLUME (B427) put the jobs on the volume (flight 27: `[jobs] volume=/jobs records=1675 claims=382
ledger=774 queue=519`) and Mica writes them, but nothing RANKS them: after every flight the seat re-reads the queue by
hand and Peter has to say "there's more". The queue's own words also lie to a naive filter: `queue_status` maps only
FIXED/DROPPED/FLOWN/LANDED/PARKED, so `✓ DONE  GMUX-1` reads `open`.

**The seam (R79).** The ranking is ONE pure function in the shared core `unaos/libs/sys/jobs_core` (no_std, the
`midden_core` shape) that both rings link: `owed_names` (a FLIGHT<n>.md `## 3. Owed` section → the CAPS item heads
in Peter's order), `next_open` (the open-for-the-next-wave predicate), `gpu_lane` (R101's Kepler / Intel slot of a
record), `ledger_ref` (the A/B/E/SR row an item cites) and `rank_next` (the order). Mica is the writer and the host
reader; the kernel is a reader through its VFS. No second store, no second ranker.

**The order.** Candidates: queue items and ledger rows of the track whose `job:status` is `open`, that carry no
`job:branch` (not cut) and whose words do not close them (`✓`, DONE, BUILT, FIXED, FLOWN, LANDED, DROPPED,
RESCUED, or a ledger cell that names a branch — that arc is running). Tier (a): every §3 name, in Peter's order,
matched to the open queue item with that `job:arc` (else the newest open ledger row with it); a §3 name with no
open record still prints, status `owed` — the flight said it. Tier (b): R101's two GPU slots — the first open
Kepler rung and the first open Intel rung by `job:seq`; they are ALWAYS in the n (tier (a) yields its tail to
them). Tier (c): the rest of the track's open queue items by `job:seq`. Row: rank, id, name, status, the ledger row
it cites, the brief (`docs/dev/evidence/**/<NAME>.md`, host only) or `-`.

**The order on the volume.** The kernel cannot read the repo, so the §3 order is a record of its own:
`/jobs/owed` (body = the names, one per line; `job:flight` = `f<n>`), written by `mica jobs owed` and by `build`
(the latest flight). `load` walks only status/ledger/queue, so the census and the export are untouched.

## Milestones

- **M1** jobs_core: `JOB_BRANCH`/`JOB_TIP` registered (una-abi, GATE-ATTRKEYS), `owed_names`, `next_open`,
  `gpu_lane`, `ledger_ref`, `rank_next`, the header line; unit tests.
- **M2** Mica: `jobs next --img I [--repo R] [--track rmbp] [--n 13] [--after-flight N]`, `jobs cut --img I <id>
  --arc A --branch B`, `jobs land --img I <id> --status S --tip T`, `jobs owed --repo R --img I [--after-flight N]`;
  `build` writes `/jobs/owed`; host tests over a fixture volume. Export unchanged: the ledger cell and STATUS.tsv
  still come from `export` (cut/land change attributes, never the row's text).
- **M3** kernel: the shell's `jobs next [n]` reads `/jobs/owed` and the records through the VFS and prints the same
  ranking via `jobs_core::rank_next`. Plain `jobs` is unchanged (the background-program reaper). No new knob
  (`fs::jobs` already rides `unafs`).

## The witness

Host: `[jobs] next flight=f27 track=rmbp owed=<k> open=<m> ranked=<n> gpu=kepler:<id>,intel:<id>` then one row
per rank. Metal (Peter types `jobs next` at the shell after login, nothing at boot — R80): the same header with
`flight=f<n>` read off `/jobs/owed`, e.g. `[jobs] next flight=f27 track=rmbp owed=11 open=<m> ranked=13 gpu=…`;
`flight=-` means the card's volume predates `/jobs/owed` (a rebuilt card carries it).

## Owed

The ledger cell for a `land` is still the seat's hand (export writes the cell from the row text, which `land` does
not edit). The brief column is host-only. The §3 parser takes item HEADS: an item without a CAPS head ("the play
verb", "Cmd-Tab") is not ranked — the flight author names it in CAPS or the queue holds it.
