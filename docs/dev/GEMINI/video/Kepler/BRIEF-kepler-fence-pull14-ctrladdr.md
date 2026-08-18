# BRIEF — kepler-fence pull 14: PBDMA CTRL_ADDR TARGET audit (+ disp-era USERD fallback)

Lane: **kepler-fence** — `unaos/crates/kernel/src/gpu/kepler.rs` ONLY.
New session? Read `docs/dev/GEMINI/README.md` first, then `video/INDEX.md`,
then `docs/dev/OS/08_VIDEO/KEPLER-METAL-LOG.md` sitting #12.

## Where the wall stands (do not re-derive)

Sitting #12 refuted pull-13's flush hypothesis: `flush-executed 0x70000
pre=0 post=0 iters=1` ran between instance writes and validate and the chip
STILL stripped VALID/POLL (`WITNESS FAILED - bits stripped`, err=2). Refuted
so far: 3 runlist encodings (s8), USERD_SNOOP (s10, tombstoned), USERD_HI
bit31 (s11 — the bit persists in instance memory, err=2 anyway), PFIFO_FLUSH
(s12). Proven working: ORDER=9, eng-masks/playlist/PMC writes stick, clocks
fine, scheduler reads the runlist (playlist_rd advances). New s12 evidence on
record: full RAMFC post-submit dump (+10=0000FACE, +30=FFFFF902) and per-PBDMA
readbacks — eng_mask 0x01/0x6E/0x10, **CHID=0 ACTIVE=0 ib_put=ib_get=0 on all
three PBDMAs**: no PBDMA ever bound to our channel.

## This pull — the two pre-committed fallbacks from your pull-13 proposal

**Milestone 1 — CTRL_ADDR TARGET audit.** Audit the PBDMA engine `CTRL_ADDR`
target bits (the `TARGET` enum values are `XXX-unconfirmed` in rnndb —
ADDRESSES with citation only, no borrowed semantics). Read-and-report the
current CTRL_ADDR words for all 3 PBDMAs, then step the documented TARGET
encodings for the instance-block/USERD pointer target (VID_MEM vs SYS_MEM
class values) one at a time, re-running the s10 witness ladder after each.
Save/restore every word you touch; evidenced restore on failure.

**Milestone 2 (only if M1 refutes) — disp-era USERD enablement.** Probe
whether channel/USERD config requires enablement via PDISPLAY/EVO-core-era
state, per your fallback framing. Read-only reconnaissance first; any write
step needs its own marker and restore.

## Exact serial markers (the bench greps these — verbatim)

- `:: kepler: ctrladdr pbdma<N> pre=XXXXXXXX ::` (all three, before changes)
- `:: kepler: ctrladdr pbdma<N> try target=<enum> wrote=XXXXXXXX rb=XXXXXXXX ::`
- the existing s10 ladder markers unchanged: `WITNESS PASSED - bits stuck!` /
  `WITNESS FAILED - bits stripped` → `sched-status` → `DISCRIMINATOR` →
  fence poll
- `:: kepler: ctrladdr restored pbdma<N> rb=XXXXXXXX ::` on every exit path
- absence-honesty: `:: kepler: ctrladdr pbdma<N> ABSENT? rb=XXXXXXXX ::` if
  the read looks like decode-miss (sentinel discipline, never bare zeros)

## Gates (DONE = all of these)

Full-knob check (`UNAOS_IVB UNAOS_KEPLER UNAOS_KEPLER_TAKEOVER
UNAOS_KEPLER_FIFO ./arroyo check`, both arches) + builder-path esp-x86 build +
`strings` proof of the new markers in kernel.elf AND BOOTX64.EFI + default
QEMU regression green. Bounded polls; commit ALL docs+code; delete scratch
files; `git status` clean.

Proposal first (`PROPOSAL-kepler-fence-pull14.md`, STATUS: PROPOSED) — no
implementation until approved.
