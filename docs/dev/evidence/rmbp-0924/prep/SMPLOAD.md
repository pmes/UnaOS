# SMPLOAD — prep

## The finding

Flight 15 §2 SMP (rmbp-ledger B227). Peter, running a vug storm: "smp is still weird though. seems
like it should spread the load better". No per-CPU load line was pinned in a spec, so there is
no glass (a spec-pinned witness) to compare against the wire (the actual boot). The `[schedx86]
load` / `[spread]` lines exist and are rich, but nothing in `unaos/scripts/specs/*.spec` REQUIREs a
periodic per-CPU busy/runq/migration reading, and there is no explicit FAIL arm for "one CPU
pegged while another idles."

## Mechanism

Counters this arc needs already exist in `unaos/crates/kernel/src/arch/x86_64/sched.rs`:

- Per-core busy/idle folding: `CoreAccount::account()` (`sched.rs:844-861`), blended
  `busy_pct()` (`sched.rs:928-948`), window `LOAD_WINDOW_MS=250` (`sched.rs:729`). Table
  `ACCT: [CoreAccount; MAX_CPUS]` (`sched.rs:1022`).
- Snapshot API: `pub fn core_load(cpu) -> CoreLoad` (`sched.rs:1085`) — `busy_pct_recent`,
  `tracked`, `pegged`.
- Ready-queue depth: `pub fn run_queue_len(cpu) -> usize` (`sched.rs:6780`), over
  `RUN_QUEUES[c]` (`sched.rs:2181`).
- Cumulative migrations: `STEAL_MOVES`/`STEAL_PASSES`, read via `fn steal_counters()`
  (`sched.rs:5994-5997`), never reset.
- Existing ~5s witness `pub fn emit_load_witness(tag: &str)` (`sched.rs:1478+`), called from
  `main.rs:7043` right after `[schedx86] depth`. It snapshots `core_load`+`run_queue_len` for
  `c in 0..meter_cpu_count()` under `without_interrupts`, prints `[schedx86] load c0=NN% ...
  q=[...] steal=M/P`, then tail-calls `emit_spread_witness` which prints `[spread] pack=..
  spare=.. rqp=[running/ready/pinned,...] steal=M/P ...` (doc block above `emit_load_witness`,
  `sched.rs` ~1560-1625).

**What is missing**: neither line is REQUIREd by any `unaos/scripts/specs/*.spec` (grep
`schedx86`/`spread\]`/`SMPBAL` across specs: no hits) — a boot can silently stop emitting them,
or show one core pegged and another idle, with no gate turning red. Neither line is a
pass/fail verdict either, just raw readings over one instant. The verdict and its spec pin are
this arc's job. `emit_load_witness` already fires ~5s; M1 adds its own ~10s gate rather than
retuning the shared one (that line also carries GUI-depth/rtwit/rtpi, out of scope here).

## Plan

- **M1 — `emit_smpload_witness()`.** New fn in `sched.rs`, placed right after
  `emit_load_witness` (anchor: its closing `}`, `sched.rs` ~1521). Reuses `core_load(c)` +
  `run_queue_len(c)`; new static `SMPLOAD_LAST_MOVES: AtomicU64` turns cumulative
  `steal_counters().0` into a per-sample delta (`migr`). Own ~10s gate via new
  `SMPLOAD_LAST_MS: AtomicU64` vs `crate::arch::ms()` (copy this file's existing rate-limit
  idiom). Line: `:: SMPLOAD: t=<s> cpus=N busy=[p0,p1,...] runq=[...] migr=<n> -> PASS ::`
  (untracked core prints `--` at that index, matching `[schedx86] load`'s convention, never a
  fabricated 0).
- **M2 — FAIL arm.** New `SMPLOAD_SKEW_STREAK: AtomicU32`. Each fire: `max_busy`/`min_busy`
  over TRACKED cores only. If `max_busy>80 && min_busy<20`, increment streak else reset to 0.
  At streak `>= 2`, print `-> FAIL (c<hi> busy=<hi>% vs c<lo> idle busy=<lo>%) ::` instead of
  PASS, naming the argmax/argmin cores.
- **M3 — wire it.** `main.rs:7043`, right after `emit_load_witness("");`, add
  `unaos_kernel::arch::sched::emit_smpload_witness();` — same task, same masked-snapshot /
  unmasked-print discipline.
- **Fixtures.** A (PASS): idle boot, `storm` quiet, busy low/even, every fire `-> PASS`.
  Go-red: pin a CPU-bound spinner to core 0 with `steal_ok: false` (`sched.rs:383-396`),
  leave others idle — must flip 2 consecutive fires to `-> FAIL`; unpinning must return 2
  consecutive `-> PASS`. B (Peter's repro): the vug-storm verb — `SMPLOAD` and `[spread]`
  should agree (PASS <-> `pack=0`; FAIL <-> `pack>=1,spare>=1`).

## Spec pins

For `unaos/scripts/specs/x86-default.spec` (REQUIRE/FORBID are plain regex per this repo's specs
— no look-around):

```
REQUIRE :: SMPLOAD: t=[0-9]+ cpus=[0-9]+ busy=\[[0-9%,-]+\] runq=\[[0-9,]+\] migr=[0-9]+ -> PASS ::
FORBID :: SMPLOAD: .* -> FAIL
```

The `FORBID FAIL` line is the gate Peter's remark is actually asking for: it goes red the moment
two consecutive samples show one CPU pegged and another idle, on any default x86 boot — which is
exactly the case not measured today. Do not add this pin to `x86-witness.spec`/`x86-test.spec`
per the same DEFAULTMEDIUM lesson recorded at the top of `x86-default.spec` (a pin meant for one
leg belongs in that leg's own file, not the unconditional one every leg replays).

## Open questions

- Should `SMPLOAD`'s ~10 s cadence be its own independent clock (as planned in M1) or should it
  instead ride `emit_load_witness`'s existing ~5 s gate at half rate (skip every other fire)? A
  person should decide whether a second `LAST_MS` static is acceptable or whether reusing the
  render-service gate's cadence is preferred for this file's "one clock per instrument" style.
- The 80%/20% thresholds are Peter's numbers from the brief; confirm they should be literal busy
  percentages from `core_load().busy_pct_recent` (the same blended, decaying number
  `[schedx86] load` already prints) rather than a stricter windowed-only reading.

## Next-session start

1. `grep -n "LAST_MS\|AtomicU64::new(0)" unaos/crates/kernel/src/arch/x86_64/sched.rs | grep -i rate` to copy the exact CAS rate-limit idiom this file already uses elsewhere, then add `SMPLOAD_LAST_MS` / `SMPLOAD_LAST_MOVES` / `SMPLOAD_SKEW_STREAK` statics beside `STEAL_MOVES` (`sched.rs:5990` area).
2. Write `emit_smpload_witness()` per M1/M2 directly after `emit_load_witness` (`sched.rs` ~1521), reusing `core_load`/`run_queue_len`/`meter_cpu_count`/`steal_counters` — no new counters need adding to the scheduler itself, only a fold and a formatter.
3. Add the `emit_smpload_witness()` call at `unaos/crates/kernel/src/main.rs:7043` and the two spec lines to `unaos/scripts/specs/x86-default.spec`; go-red with a `steal_ok: false` pinned spin task on one core before trusting the PASS line.

## Draft code (unbuilt)

```rust
// sched.rs — after emit_load_witness's closing `}` (~sched.rs:1521)
static SMPLOAD_LAST_MS: AtomicU64 = AtomicU64::new(0);
static SMPLOAD_LAST_MOVES: AtomicU64 = AtomicU64::new(0);
static SMPLOAD_SKEW_STREAK: AtomicU32 = AtomicU32::new(0);
const SMPLOAD_PERIOD_MS: u64 = 10_000;

/// `:: SMPLOAD: t=<s> cpus=N busy=[p0,p1,...] runq=[...] migr=<n> -> PASS|FAIL ::`
pub fn emit_smpload_witness() {
    use core::fmt::Write;
    let now = crate::arch::ms();
    if now.saturating_sub(SMPLOAD_LAST_MS.load(Ordering::Relaxed)) < SMPLOAD_PERIOD_MS { return; }
    SMPLOAD_LAST_MS.store(now, Ordering::Relaxed);

    let n = meter_cpu_count();
    let mut busy = [None; MAX_CPUS]; // None = untracked ("--")
    let mut runq = [0usize; MAX_CPUS];
    x86_64::instructions::interrupts::without_interrupts(|| {
        for c in 0..n {
            let ld = core_load(c);
            busy[c] = ld.tracked.then_some(ld.busy_pct_recent);
            runq[c] = run_queue_len(c);
        }
    });

    let (moves, _) = steal_counters();
    let migr = moves.saturating_sub(SMPLOAD_LAST_MOVES.swap(moves, Ordering::Relaxed));

    let (mut hi, mut lo): (Option<(usize, u32)>, Option<(usize, u32)>) = (None, None);
    for (c, b) in busy.iter().enumerate().take(n) {
        if let Some(p) = b {
            if hi.is_none_or(|(_, m)| *p > m) { hi = Some((c, *p)); }
            if lo.is_none_or(|(_, m)| *p < m) { lo = Some((c, *p)); }
        }
    }
    let skewed = matches!((hi, lo), (Some((_, h)), Some((_, l))) if h > 80 && l < 20);
    let streak = if skewed { SMPLOAD_SKEW_STREAK.fetch_add(1, Ordering::Relaxed) + 1 }
                 else { SMPLOAD_SKEW_STREAK.store(0, Ordering::Relaxed); 0 };

    let mut w = LineBuf::new();
    let _ = write!(w, ":: SMPLOAD: t={} cpus={} busy=[", now / 1000, n);
    for c in 0..n { let _ = write!(w, "{}{}", if c==0 {""} else {","},
        busy[c].map(|p| p.to_string()).unwrap_or_else(|| "--".into())); }
    let _ = write!(w, "] runq=[");
    for c in 0..n { let _ = write!(w, "{}{}", if c==0 {""} else {","}, runq[c]); }
    let _ = write!(w, "] migr={}", migr);
    match (streak >= 2, hi, lo) {
        (true, Some((hc, hp)), Some((lc, lp))) =>
            { let _ = write!(w, " -> FAIL (c{} busy={}% vs c{} idle busy={}%) ::", hc, hp, lc, lp); }
        _ => { let _ = write!(w, " -> PASS ::"); }
    }
    serial_println!("{}", w.as_str());
}
```

```rust
// main.rs, right after the existing emit_load_witness call (line 7043)
unaos_kernel::arch::sched::emit_smpload_witness();
```
