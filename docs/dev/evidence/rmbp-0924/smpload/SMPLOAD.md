# SMPLOAD — the per-core load VERDICT (rmbp-ledger B227), 2026-09-24

Flight 15 (Peter, vug storm): "smp is still weird though. seems like it should spread the load better." No
per-CPU line was pinned for the flight. Prep: `docs/dev/evidence/rmbp-0924/prep/SMPLOAD.md` (the counters
already exist: `core_load`, `run_queue_len`, `steal_counters`, printed as `[schedx86] load` every ~5 s,
never judged, never pinned). Branch `claude/optimistic-ramanujan-r3qyu5`.

## What was built (`arch/x86_64/sched.rs` tail, one same-line fold in `main.rs`)

- `emit_smpload_witness()` — every ~10 s, riding the depth line's 5 s gate beside `emit_load_witness`:
  `:: SMPLOAD: t=<s> cpus=N busy=[…] runq=[…] stealable=[…] migr=<since last> streak=<k> skewed=<0|1> -> PASS|FAIL ::`
- `smpload_judge(busy, stealable, streak)` — the PURE verdict. Skew is STEALABLE work waiting behind a core
  above 80% while a core below 20% has an empty queue; FAIL when it holds two samples running.
- `run_queue_stealable(cpu)` — `census()`'s `ready - pinned`, witness-rate only.
- `smpload_selftest()` — the judge on five shapes: flat, pegged-with-empty-queues, one waiting sample, two,
  an untracked core. Pinned in `x86-wc.spec` with the SMPLOAD PASS REQUIRE and both FORBIDs.

## What the instrument taught in its first three runs (the rule was refined twice, on evidence)

1. Draft rule (busy >80 vs <20): `busy=[0,1,100,100,0,99] runq=[0,0,0,0,0,1]` FAILED every sample. Three
   cores pegged, three idle — but with EMPTY queues that is one render task per core and nothing to spread.
   A single task cannot be split; the rule was wrong, not the scheduler.
2. Second rule (a waiter on the hot core, `runq>=1`): still FAIL at t=23 — one task waited behind c5 while c0
   idled for two samples. `census()` showed it: `stealable=[0,0,0,0,0,0]` — the waiter was PINNED
   (`steal_ok=false`, the fixture battery's own spinner). A pinned waiter is a pin contract, not a spreading
   failure.
3. Final rule (a STEALABLE waiter): every sample PASS, `skewed=0`. On QEMU the x86 scheduler is not declining
   to spread anything; whether the rMBP's vug storm reads the same is exactly what boot 16 measures.

## Proof — wc lane `UNAOS_QEMU_MACHINE=pc-q35-8.2 UNAOS_WC=1 UNAOS_QUARRY=1 UNAOS_FTDIRX=1 UNAOS_KEPLER_VBLANK=1 ./arroyo test 90`

```
:: SMPLOAD-JUDGE: flat=0/0 pegged=0/0 burst=1/1 twice=1/2 untracked=0/0 fail_at=2 -> PASS ::
:: SMPLOAD: t=13 cpus=6 busy=[0,1,100,100,0,99] runq=[0,0,0,0,0,1] stealable=[0,0,0,0,0,0] migr=5 streak=0 skewed=0 -> PASS ::
:: SMPLOAD: t=64 cpus=6 busy=[0,1,0,0,0,96] runq=[0,0,0,0,0,0] stealable=[0,0,0,0,0,0] migr=0 streak=0 skewed=0 -> PASS ::
✅ MBENCH PASS — 45/45 required witnesses, 0 forbidden hit(s)   (x86-wc.spec; rc=1 is the container's SOCK-3)
```
GO-RED BY MUTATION (`smpload_judge` not advancing the streak; restored byte-identical from a saved copy):
```
:: SMPLOAD-JUDGE: flat=0/0 pegged=0/0 burst=1/0 twice=1/1 untracked=0/0 fail_at=2 -> FAIL ::
❌ MBENCH FAIL — 44/45 required witnesses, 6 forbidden hit(s)   (shared with the KVBLANK4 mutation, same build)
```

## Boot 16 reads

During a vug storm: `stealable=[…]` non-zero behind a pegged core while another core idles, two samples
running → `-> FAIL` names the skew Peter saw as a number. All PASS with cores pegged and empty queues → the
vugs are one task each and "spread" means more vugs, not a scheduler change.
