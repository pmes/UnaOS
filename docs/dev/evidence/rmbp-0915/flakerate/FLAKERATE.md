# FLAKERATE — the load-dependent reds measured as rates, with the load beside each

**Arc:** FLAKERATE (`exec-rmbp-flakerate`), tree `da2a9abc`, 2026-09-16. Six runs, all sequential in
ONE worktree (Class 3's rule: gates are never run concurrently in one worktree, because they share
`target/` and the host's cores). Nothing in the kernel was touched: this arc MEASURES.

**The box is the instrument.** Other executors were compiling and booting QEMU throughout — that IS
the load under measurement, and nobody was asked to stop. Every run's log carries its own header with
`uptime`, the 1-min/5-min/15-min load, `pgrep -c -x rustc`, `pgrep -c -f qemu-system` and `nproc=20`,
recorded immediately before the command. Verdicts are read from the FILES, never through a pipe from
a live command (LAWS §5: a pipe launders the verdict); serial captures are read with `awk` /
`LC_ALL=C grep -a -F`, never bare `grep`.

Full arroyo logs and serial captures: `~/unaos-bench/scratch/rmbp-0915/flakerate-logs/`
(`x86-run{1..4}.log` + `.serial`, `pi-run{1,2}.log` + `.serial.serial-pi.log`). Scratch is not cited
as evidence anywhere but in this sentence; everything a reader needs to check the claims is quoted
below.

---

## 1. The six runs

| # | leg | command | load1 at start | load1 at end | rustc / qemu at start | wall | rc |
|---|---|---|---|---|---|---|---|
| x86-1 | x86 | `UNAOS_WC=1 UNAOS_QEMU_FULL=1 ./arroyo test 90` | **7.90** | 15.55 | 21 / 0 | 118.4 s | **1** |
| x86-2 | x86 | same | **15.55** | 15.18 | 6 / 2 | 90.3 s | **1** |
| x86-3 | x86 | same | **15.18** | 15.09 | 7 / 1 | 90.2 s | **1** |
| x86-4 | x86 | same | **15.09** | 13.84 | 5 / 2 | 90.3 s | **1** |
| pi-1 | Pi | `UNAOS_QEMU_FULL=1 ./arroyo kernel8-test 300` | **13.84** | 13.35 | 4 / 2 | 300.1 s | **1** |
| pi-2 | Pi | same | **13.35** | 12.38 | 6 / 0 | 300.1 s | **1** |

The Pi leg is pi's under R39. It is run here because **the flake rate being measured is the SHARED
harness's**, not the Pi kernel's — `kernel8-test` is the other half of the population SO7/B26 is
about, and a rate quoted for one leg alone would not answer the quiet-box obligation.

x86-1 paid 118.4 s of a 90 s cap against 90.2-90.3 s for the other three: the extra 28 s is the cold
`target/` compile bleeding into the QEMU wall, and it is the run whose load ROSE most during
execution — the executor's own `/proc/loadavg` reads during x86-1 were 15.12, **20.57**, 20.41,
17.01, 16.37. So the header's `load1` is the load at the START, not a ceiling; the peak 1-min load
observed inside the measurement window was **20.57**, during x86-1.

**No run was TRUNCATED.** All four x86 runs printed `⚡ test: the run REACHED its completion marker`
(serial.log lines 2049 / 2054 / 2060 / 2044) and both Pi runs printed
`(the end-of-run marker was seen — the run completed…)` at 23846 and 25961 lines. The
2026-09-13 load-dependent-verdict row's case therefore **did not arise** in this measurement: there
is nothing here to record as TESTTRUNC behaving correctly, because no run ran out of wall. That is
itself a datum — at these loads the x86 leg reaches the end of its ladder with 90 s.

---

## 2. THE HEADLINE: the x86 leg did not vary at all

Four runs of a byte-identical command on a byte-identical tree, at 1-min loads 7.90 / 15.55 / 15.18 /
15.09 (peak 20.57 observed in-run):

| measure | x86-1 | x86-2 | x86-3 | x86-4 |
|---|---|---|---|---|
| distinct `:: TAG:` witness tags | 119 | 119 | 119 | 119 |
| `-> PASS` lines | 78 | 78 | 78 | 78 |
| `:: PASS ::` lines | 22 | 22 | 22 | 22 |
| `-> FAIL` lines | 3 | 3 | 3 | 3 |
| any line containing `FAIL` | 3 | 3 | 3 | 3 |
| reached completion marker | yes | yes | yes | yes |

The witness tag SET is **identical across all four** (`diff` of the four sorted tag lists: no output,
three times). The three reds are the same three lines, in the same order, in every run.

**Every fixture this arc was sent to measure was PRESENT and PASSING in all four runs.** That matters
because LAWS §5 says an absence is evidence only if the producing path ran — so the census is stated
before the rate, not after:

| fixture | occurrences per capture (r1/r2/r3/r4) | verdict in all four |
|---|---|---|
| `DOCKID` | 1 / 1 / 1 / 1 | `:: DOCKID: tiles=5 closed=win2 reopened=win2 recycle=true order=true set=true furniture=true count=true/5 pins=true/2 press=yes :: PASS ::` |
| `[dmgovlp]` | 5 / 5 / 5 / 5 | `verdict passes=12/12 drained=12/12 drag_evt=5..7 drag_px=38590..56270 relay=3 narrow=3/12 cur=12/12 adopt=25..26 repaint=0 max_ms=4..13 adopt_stretch=4/4 -> PASS` |
| `[ptrdead]` | 1 / 1 / 1 / 1 | `backlog whole=true nodrop=true order=true pushed=192 entries=1 travel=(192,-192) folded=192 dropped=0 fpop12=0 fpop3=0 cpu=3 svc=Some(5) -> PASS` |
| `PWRDRAIN` | 1 / 1 / 1 / 1 | PASS |
| `S5DRAIN` | 1 / 1 / 1 / 1 | PASS |
| `SINKDRAIN` | 9 / 9 / 9 / 9 | `staged=8 drained=8 on_cable=8` PASS |
| `SERWIT-1` | 3 / 3 / 3 / 3 | PASS |
| `DOCK` | 2 / 2 / 2 / 2 | PASS |
| `SOCK-4` | 2 / 2 / 2 / 2 | PASS |
| `DMG-REFUSE` | 2 / 2 / 2 / 2 | PASS |
| `APPPIN` | 1 / 1 / 1 / 1 | PASS |

So for the x86 `test` leg, at 1-min load up to 15.55 with a 20.57 peak: **the flake rate of every one
of these fixtures is 0 in 4.** Not "they passed" — they produced the *same* line four times.

### 2a. The three x86 reds are a DEFECT, not a flake

Red in 4 of 4 runs is, by this arc's own rule, a defect and is reported as such:

```
:: TSTE: fatverb.writegate -> FAIL (got gate_ran=true gate=declined veto=the internal SD reader is mounted READ-ONLY — only the reserved flight-recorder extent admits a write (SDHC-4c), and no file verb can name it handles=global=absent sdhc=present) ::
:: TSTE: vfsroute.refuse -> FAIL (got fat_remove_attr=Err(NoSuchVolume) root_remove_dir=Err(NoSuchVolume) root_vol=?) ::
:: X86BIND: root=- reason=kernel-not-found-on-any-volume serial=0x00000000 by=content bootinfo=0xfabe1afd agrees=unknown mounts=1 layout=false -> FAIL ::
```

**This is not a new finding and it is not filed as one.** It is the already-ruled X86BIND condition,
stated in this tree's own `docs/dev/OS/rmbp-queue.md` at `da2a9abc`: *"the DEFAULT `./arroyo test`
cannot host this rung at all"* — the ESP holding `kernel.elf` is reachable only under `UNAOS_AHCI=1`,
and the usb-storage stick is the raw `UNA-OS-DISK-001-ALPHA` pattern with no `0xAA55`, which
`mount_source` refuses. The ruling on record is to re-cut on the `UNAOS_AHCI=1` leg. Recorded here
only as the CONTROL that makes the 0-in-4 flake rate above meaningful: the leg is capable of going
red, and did, reproducibly — a zero flake count from a capture that can never print a red would be a
fact about the pattern, not about the data.

---

## 3. The Pi leg: the flakes are here, and SO7/B26's signature reproduced

| measure | pi-1 (load1 13.84) | pi-2 (load1 13.35) |
|---|---|---|
| lines captured | 23846 | 25961 |
| distinct `:: TAG:` witness tags | 106 | 106 |
| `-> PASS` | 101 | 96 |
| `-> FAIL` | 0 | 1 |
| `-> COHER` | 0 | 3 |
| `-> RACE` | 0 | 1 |
| MBENCH REQUIREs met | **125/126** | **124/126** |
| MBENCH forbidden hits | **0** | **6** |
| end-of-run marker | seen | seen |

**The higher-load run is the clean one.** pi-1 at load1 13.84 carried no `[wc-*]` red; pi-2 at load1
13.35 carried four. Whatever orders these outcomes, the 1-min load at run start does not.

### 3a. SO7 / B26's signature, reproduced, with `moved=` non-zero

pi-2, `[wc-d] verify` (the FORBID hit at capture line 983) against pi-1's same fixture at line 998:

```
pi-2  [wc-d] verify win=1 surf=288x288 band=none scale=1x at (17,51) panel=640x480 checked=82944 bad_cache=783 bad_ram=829 ram_indep=yes moved=1064 sprite_px=0 nonzero=82944 occluded=0 occ=0/0 cksum=0xd731c913edbb9654 first=(160,132) got=0xc9a6e8 want=0x1e1e1e fills=2->2 fact=0/0 desk=9->9 dact=0/0 -> FAIL
pi-1  [wc-d] verify win=1 surf=288x288 band=none scale=1x at (17,51) panel=640x480 checked=82944 bad_cache=0   bad_ram=0   ram_indep=yes moved=0    sprite_px=0 nonzero=82944 occluded=0 occ=0/0 cksum=0x5d5eea128d5f8d71 first=none        fills=2->2 fact=0/0 desk=13->13 dact=0/0 stable=yes -> PASS
```

and the `[wc-g]` rollups for the same window:

```
pi-2  [wc-g] rollup win=1 scope=window samples=4 coher=2 race=1 blit=0 clean=1 slow=1 maxus=25054 wit_us=48019 frame_us=16667 -> COHER
pi-1  [wc-g] rollup win=1 scope=window samples=4 coher=0 race=0 blit=0 clean=4 slow=1 maxus=17549 wit_us=31948 frame_us=16667 -> CLEAN+SLOW
```

with the two individual hits:

```
pi-2:947  [wc-g] win=1 seq=0 own=no  … fbbad=1830/82944 … us=25054 rectscan_us=10000 slow=yes -> COHER
pi-2:977  [wc-g] win=1 seq=9 own=yes … fbbad=1647/82944 … us=9555  rectscan_us=10000 slow=no  -> RACE-BLIT
```

**Three things this measurement settles, and one it does not.**

1. **`moved=1064` is non-zero.** SO7 says the verifier *already detects and reports the moved
   reference and then renders `-> FAIL` anyway — the instrument is not blind, it is unheeded*, and
   that the cheap fix is a distinct `-> MOVED` / `-> RESAMPLE` verdict. This run is that case,
   measured: the reference moved 1064 times under the verifier, and the fixture called it FAIL. The
   fix SO7 proposed would have turned this exact red into an honest non-verdict.
2. **`own=no` on the first hit** is the boot-seam concurrent-writer tell SO7 attributes at
   `wcg.rs:412` (repainted as COLLATERAL), with `fbbad=1830/82944` beside it — the other face SO7
   names.
3. **`maxus` is the outlier.** 25054 µs on the failing run against 17549 µs on the clean one — the
   starvation reading §1c asks a sighting to confirm, confirmed one level up (`[wc-g]` rather than
   `[dmgovlp]`).
4. **What it does NOT settle: `bad_cache != bad_ram`.** 783 vs 829. SO7 records that the observed
   shape is `bad_cache == bad_ram` (91/91, 867/867, 6816/6816) and that the one asymmetric run
   (145/83) *"is a SEPARATE observation, not this shape"*. This is the **second** asymmetric sighting
   and it is banked as such, not folded into the symmetric family.

### 3b. TWO witnesses vanished from a completed run — and one of them proves the harness's own sentence wrong

pi-1 and pi-2 each carry 106 distinct witness tags, and each is missing exactly one tag the other
has:

| tag | pi-1 | pi-2 | rate |
|---|---|---|---|
| `:: PWRDRAIN:` | present (and **FAIL**) | **absent** | 1 in 2 |
| `:: UI1:` | **absent** | present | 1 in 2 |

and at line level, `U5` started in both runs and finished in one:

```
pi-1:551  :: U5: capabilities — rights + CHECK + grant/attenuate/revoke + routed sys_write ::      (banner)
pi-1:558  :: U5: capabilities — write-cap OK, no-cap -EACCES, attenuated grant bounded, revoke enforced, teardown-clear clean -> PASS ::
pi-2:556  :: U5: capabilities — rights + CHECK + grant/attenuate/revoke + routed sys_write ::      (banner)
pi-2      (no verdict line — MBENCH: `❌ REQUIRE U5: capabilities.*-> PASS   0 hits — MISSING`)
```

Both Pi runs SAW THE END-OF-RUN MARKER, and MBENCH says so in its own words:

> `(the end-of-run marker was seen — the run completed, so a missing witness here is a GENUINE regression)`

**That sentence is false for pi-2's `U5`.** The fixture ran — its banner is on the wire at line 556 —
and the run completed. The verdict line is simply not in the capture. A completed run is not the same
thing as a complete capture, and the harness currently conflates them.

Both Pi captures also carry live line loss, on three taps:

```
pi-1:327 / pi-2:325   [mirror] fbcon: 8 line(s) dropped, 0 truncated since boot (sink contended or full)
pi-1:2522 / pi-2:2250 [mirror] tste: 1 line(s) dropped, 0 truncated since boot (sink contended or full)
pi-1:2613 / pi-2:2337 [mirror] tste: 2 line(s) dropped, 0 truncated since boot (sink contended or full)
```

while `SERWIT-2` reports `-> PASS` in both (`every line accounted for on all 4 taps, 0 lost on the 3
evidence taps`) — correctly, because `fbcon`'s drops are excluded by design and `tste`'s counters read
`dropped=0 suppressed=322`. **So the tap accounting does not cover whatever lost these lines**: the
primary PL011 capture is not one of the four taps SERWIT-2 conserves over, and the losses above are
invisible to it.

### 3c. `ERET-SCRUB: first-entry` — absent in BOTH runs: a DEFECT, filed as one

`pi4-regression.spec:309` requires
`:: ERET-SCRUB: first-entry GPR/FP/TPIDR residue = 0 .*-> PASS ::`. It is MISSING in **2 of 2** runs,
which by this arc's rule is a defect and not a flake. The measurement that makes it worth a row:

- `LC_ALL=C grep -a -c -F 'ERET-SCRUB'` = **1** in each capture — and the one hit is the *second*
  line, `:: ERET-SCRUB: syscall-return preserved x1-x30 + x8 + SP_EL0 + v0-v31 across SYS_YIELD
  (bitmap=0x0) -> PASS ::` (pi-1:510, pi-2:527).
- `first-entry` = **0** hits, `TPIDR` = **0** hits, `FPSR` = **0** hits in both captures. So the line
  is absent in **either** of its two forms — not the PASS branch and not the FAIL branch — and it is
  not torn: no fragment of it exists anywhere in either capture.
- The producing path RAN: `:: SCHED: task 'el0-eretentry' -> core 3 …` at pi-1:473, and
  `[el0stkhw] task=72:el0-eretentry len=16384 hw=3048 headroom=13336 loguard=0` at pi-1:509 — the
  line immediately before the sibling verdict at 510.
- `arch/aarch64/syscall.rs:6541` (`eret_scrub_verdict`) prints the two lines as **two consecutive
  statements with no early return between them**: the first-entry `if/else` is `:6550-:6559` and the
  syscall-return `if/else` is `:6560-:6569`, and all four arms print. There is no code path on which
  line 1 is skipped and line 2 is printed.

**Conclusion, stated at the confidence the evidence supports:** the emitter cannot skip this line, the
task demonstrably reached the statement after it, and no fragment survives — so the line was emitted
and lost between the kernel and the capture, twice. That is the same class as §3b, not a kernel
regression in the return-path scrub. It is filed as a DEFECT because it is 2-in-2, and the
distinguishing experiment (does it ever appear?) is a quiet-box run, which this arc is not permitted
to make quiet. **What a reader must NOT do is read `124/126` as a scrub regression** — the scrub's
sibling witness passed in both runs.

### 3d. A FAIL that MBENCH's default FORBIDs cannot see

pi-1 line 503, in a run MBENCH scored **`0 forbidden hit(s)`**:

```
:: PWRDRAIN: FAIL — filled=64 lines=12 bytes=816 want_bytes=4352 residue=0 dropped=0 ::
```

`mbench.py:136`'s `DEFAULT_FORBIDS` are `-> FAIL`, `FAIL ::` and `PANIC`. This line carries none of them:
its form is `FAIL — `, with the `::` terminator seven fields later. It is a red that the battery
reports as clean. Found only because this arc scanned for the bare token `FAIL` rather than for the
three shapes the harness knows — and the same widening over the four x86 captures found nothing the
`-> FAIL` scan had not already found (`FAILany` = 3 = `-> FAIL` count, all four runs), so this is a Pi
emitter's spelling, not a tree-wide hole. Reported, not fixed; this arc touches no `.rs`.

---

## 4. The rate table

Population: 4 x86 runs at 1-min load 7.90-15.55 (peak 20.57 in-run), 2 Pi runs at 13.84 and 13.35.
All six on tree `da2a9abc`, all six sequential in one worktree, all six reaching their completion
marker.

| fixture / tag | leg | runs red / total | loads seen at | class | verdict |
|---|---|---|---|---|---|
| `DOCKID` | x86 | **0 / 4** | 7.90, 15.55, 15.18, 15.09 | — | **not reproduced**; 0-in-4 control for the PARTINSTALL 1-in-4 |
| `[dmgovlp] adopt_stretch` | x86 | **0 / 4** | as above | 1c | **not reproduced** at these loads |
| `[ptrdead] backlog` | x86 | **0 / 4** | as above | 3 | **not reproduced**; `fpop12=0 fpop3=0` four times |
| `PWRDRAIN` | x86 | **0 / 4** | as above | — | **not reproduced** |
| `S5DRAIN` / `SINKDRAIN` | x86 | **0 / 4** | as above | — | **not reproduced** |
| `SERWIT-1` / `DOCK` / `SOCK-4` / `DMG-REFUSE` / `APPPIN` | x86 | **0 / 4** | as above | 1a/1b/2 | **not reproduced** |
| `TSTE fatverb.writegate` | x86 | **4 / 4** | as above | — | **DEFECT** — the ruled X86BIND default-leg condition |
| `TSTE vfsroute.refuse` | x86 | **4 / 4** | as above | — | **DEFECT** — same condition |
| `X86BIND` | x86 | **4 / 4** | as above | — | **DEFECT** — same condition, already ruled: re-cut on `UNAOS_AHCI=1` |
| `[wc-g] -> COHER` / `-> RACE-BLIT` | Pi | **1 / 2** | red at 13.35, green at 13.84 | **5** | **FLAKE** — SO7/B26's signature, `own=no`, `maxus` outlier |
| `[wc-d] verify -> FAIL` | Pi | **1 / 2** | red at 13.35, green at 13.84 | **5** | **FLAKE** — `moved=1064`, `bad_cache=783 != bad_ram=829` (2nd asymmetric sighting) |
| `:: U5: capabilities … -> PASS` | Pi | **1 / 2** | absent at 13.35, present at 13.84 | **5** | **FLAKE** — banner printed, verdict line lost |
| `:: PWRDRAIN:` (Pi) | Pi | **1 / 2** absent | absent at 13.35 | **5** | **FLAKE** (absence); and in the run where it DID print it printed `FAIL —`, unseen by the FORBIDs |
| `:: UI1:` | Pi | **1 / 2** absent | absent at 13.84 | **5** | **FLAKE** (absence) |
| `:: ERET-SCRUB: first-entry …` | Pi | **2 / 2** absent | 13.84 and 13.35 | **5** | **DEFECT** (2-in-2) — emitter cannot skip it; the next statement printed both times |

---

## 5. The rule, from these numbers

The brief asked for "the load threshold above which the x86 `test` verdict is unreliable — the highest
1-min load at which all four x86 runs were green vs the lowest at which one red appeared". **The
measurement refuses that question, and the refusal is the finding.**

- **On x86 there is no such threshold in this data.** Four runs at 7.90-15.55 (peak 20.57) produced
  not merely four passes but four IDENTICAL verdict sets — 119 tags, 78 `-> PASS`, 22 `:: PASS ::`,
  3 `-> FAIL`, every time. The highest load at which all four were flake-free is **15.55 at start /
  20.57 observed in-run**. The lowest load at which an x86 flake appeared is **unmeasured** — none
  appeared. That is consistent with rmbp-ledger B9's own tally (2 reds in 5, *both at load ≥ 24*) and
  narrows it from below: **load ~20 is not enough to make the x86 `test` verdict unreliable on this
  20-core box.**
- **On the Pi the ordering is inverted.** The reds landed on the run that started at load1 **13.35**
  and not on the one that started at **13.84**. Two runs cannot establish a threshold, but they are
  enough to falsify the claim that the 1-min load at run start predicts the verdict — here it
  anti-predicted it.

**Proposed rule for LAWS §5's Flakes paragraph (the seat carries it; this arc does not edit LAWS):**

> A host-load number is not a flake predictor and must not be used as one. Measured 2026-09-16
> (FLAKERATE, 20-core box): four `UNAOS_WC=1 UNAOS_QEMU_FULL=1 ./arroyo test 90` runs on one tree at
> 1-min load 7.90-15.55, peak 20.57 in-run, produced **byte-identical** verdict sets — 0 flakes in 4
> across DOCKID, `[dmgovlp]`, `[ptrdead]`, PWRDRAIN, S5DRAIN, SINKDRAIN, SERWIT-1, DOCK, SOCK-4,
> DMG-REFUSE and APPPIN; while two `kernel8-test 300` runs at 13.84 and 13.35 put every red on the
> **lower**-load run. So: **quote the load, never reason from it.** The discriminator is in the line,
> not in `uptime` — `moved=` and `maxus=` for `[wc-*]`, `fpop12=` for `[ptrdead]`, `max_ms=` /
> `drag_evt=` for `[dmgovlp]` — and a fixture that cannot distinguish "the stimulus never ran" from
> "the stimulus ran and the assert failed" costs a re-run every time it fires, whatever the load was.
> **Corollary, measured the same day: a completed run is not a complete capture.** `kernel8-test`
> reported `the end-of-run marker was seen — the run completed, so a missing witness here is a
> GENUINE regression` for a witness whose own banner is on the wire eight lines earlier and whose
> emitter has no path that skips it. A missing REQUIRE on a completed run is a *capture* question
> before it is a regression.

**The quiet-box obligation (SO7 / B26) is NOT discharged by this arc and cannot be discharged by a
load number.** What these six runs establish is narrower and more useful: the baseline Pi failure rate
at ordinary working load is **1 red in 2**, its signature is the one SO7 already attributed, and its
own discriminator (`moved=1064`) was printed and ignored. The fixture fix SO7 named — a distinct
`-> MOVED` verdict when `moved != 0`, NEVER a relaxed FORBID — is the thing that would have made this
run readable without a second one. A quiet box would still be needed to say whether the residual rate
is zero; it is not needed to say that the current verdict is unreadable.

---

## 6. Reading a red from this corpus

1. `LC_ALL=C grep -a -n -F -e 'FAIL' <capture>` — the bare token, not `-> FAIL`. §3d is why.
2. If the red is `[wc-d]` / `[wc-g]`: read `moved=` and `maxus=` FIRST. `moved != 0` means the
   reference moved under the verifier and the sample is invalid — Class 5, SO7's shape, not content.
3. If a REQUIRE is MISSING on a run that saw its end-of-run marker: check whether the fixture's
   BANNER is on the wire (`U5`) or whether a SIBLING line from the same emitter is (`ERET-SCRUB`).
   Either one means the fixture ran and the capture lost the line — a capture question, not a
   regression.
4. Only then re-run, alone, per LAWS §5 — and record the run whether it is green or red.
