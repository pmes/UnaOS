# PREDICTION — `storm` on x86 (STORM-X86)

Branch `wt/stormx86`. Written **before** any metal boot, so the bench round scores a prediction
rather than describes an outcome.

Metal fact this answers: Boot AL, rMBP — Peter typed `storm` at the x86 shell and got
`Unknown command`. The verb was `#[cfg(feature = "baremetal")]` (Pi-only) while the `bg`/`jobs`/`kill`
arms on either side of it had already been widened to x86.

## Build the prediction is for

```
UNAOS_WC=1 UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1 UNAOS_KEPLER_FIFO=1 ./arroyo esp-x86
⚡ kernel features: ehcihid,kbdwit,smolnet,nvidia-kepler,nvidia-kepler-takeover,nvidia-kepler-fifo,wc
```

**Do NOT set `UNAOS_BOOTLOG=1` for a boot that is going to type `storm`.** See "Reachability" below:
with `bootlog` armed, `kernel_main` diverges early, the console/shell loop after it is unreachable,
and the linker drops the ENTIRE shell — `storm`, `bg`, `jobs`, `kill`, `help` and all. That is a
pre-existing property of the `bootlog` build, not of this arc, but it will silently make this whole
prediction unscoreable.

## Reachability — witnessed, not assumed

`strings`/byte search on the staged `target/x86_64_esp/kernel.elf` (the ELF the firmware loads):

| literal / symbol | present |
| --- | --- |
| `storm: launched ` | yes |
| `:: STORM: begin n=` | yes |
| `:: STORM: end \| proc rows free=` | yes |
| `:: STORM: REFUSED at launch ` | yes |
| ``storm: `fat` refused `` | yes |
| `:: STORM: fatw REFUSED ` | yes |
| `storm [n]  (launch n vugs; ...)` (help) | yes |
| `..arch::x86_64::sched::storm_probe` | yes (symbol) |
| `..arch::x86_64::sched::storm_census` | yes (symbol) |

## 1. `storm` (no argument) on an otherwise empty boot — expect a fleet of 6

**Console** (panel, and mirrored to serial by the console sink):

```
storm: n=6 — 6/6 process rows free, 8/8 job rows, 8/8 user slots
bg: /fat/VUG.ELF started — pid 1 (see `jobs`)
bg: /fat/VUG.ELF started — pid 2 (see `jobs`)
bg: /fat/VUG.ELF started — pid 3 (see `jobs`)
bg: /fat/VUG.ELF started — pid 4 (see `jobs`)
bg: /fat/VUG.ELF started — pid 5 (see `jobs`)
bg: /fat/VUG.ELF started — pid 6 (see `jobs`)
storm: launched 6/6 vugs
```

pids are the live monotonic counter, so they start at 1 only on a boot where nothing has been
spawned yet; on a boot that already ran `vug.elf` they simply continue.

**Serial**, in this order:

```
:: STORM: begin n=6 | proc rows free=6 running=0 exited=0 porphaned=0 of 6 | job rows free=8/8 | user slots free=8/8 ::
[storm] pre | busy c0=..% c1=..% ... | rq(ready) c0=0 c1=0 ... | ctx=<N>
[schedx86] load-storm-pre c0=..% ... sw=[..] q=[..]
:: BGRUN: ... (one per launch, the existing bg wording)
[storm] k=1/6 | busy ... | rq(ready) ... | ctx=<N>
[storm] k=2/6 | ...
[storm] k=3/6 | ...
[storm] k=4/6 | ...
[storm] k=5/6 | ...
[storm] k=6/6 | ...
:: STORM: launched 6/6 vugs ::
:: STORM: end | proc rows free=0 running=6 exited=0 porphaned=0 of 6 | user slots free=2/8 ::
[storm] post | busy ... | rq(ready) ... | ctx=<N>
[schedx86] load-storm-post c0=..% ... sw=[..] q=[..]
```

Scoreable claims:

1. **`user slots free=2/8` at `end`.** Six vugs claim six of the eight address-space slots; the
   two-slot reserve that `MAX_PROCS <= USER_SLOTS - 2` asserts is exactly what should be left. A
   number below 2 is a leak and the reason this line is printed unconditionally.
2. **`proc rows free=0 running=6` at `end`**, and `porphaned=0`.
3. **`rq(ready)` and `ctx` both rise across `pre` → `k=6/6` → `post`** — the queues take the fleet
   and context switches accumulate. A flat `ctx` across the burst would mean the fleet never
   dispatched.
4. **Exactly 6 `[storm] k=` lines**, numbered 1..6.

Per-core `busy` tokens follow the `[schedx86]` three-form rule: `NN%` measured, `NN%*(name)`
inferred, `--` absent. `post` is taken immediately after the burst, so its percents still carry the
PRE-burst window — that is the zero mark, not a reading of the settled fleet. The settled fleet is
described by the `[schedx86] load` heartbeat lines that follow.

If the fleet starves the shell, `[storm]` stops and the last `k=` names the launch it stopped after.
That silence is not a refutation; read it against the `[schedx86] load` heartbeat, which does not
depend on the shell being dispatched.

## 2. `storm 8` — expect a REFUSAL that names the process table

The clamp admits 1..8, but `proc_table_rows()` is 6 on x86 (`MAX_PROCS` in
`arch/x86_64/syscall.rs`, same reserve assertion as aarch64). So `storm 7` and `storm 8` cannot
succeed as asked on ANY boot; the seventh launch is refused and the loop stops honestly.

**Console:**

```
storm: n=8 — 6/6 process rows free, 8/8 job rows, 8/8 user slots
bg: /fat/VUG.ELF started — pid N (see `jobs`)      × 6
bg: /fat/VUG.ELF: <the spawn refusal reason>
storm: launched 6/8 vugs
```

**Serial** (the added line):

```
:: STORM: REFUSED at launch 7 of 8 — fleet stands at 6 | proc rows free=0 running=6 exited=0 porphaned=0 | user slots free=2 ::
:: STORM: launched 6/8 vugs ::
```

Scoreable claim: the fleet `storm 8` leaves is **the same fleet `storm 6` builds** — 6 vugs, 2 slots
free — and the shortfall is reported as `6/8`, never rounded up.

## 3. `storm fat` — expect a REFUSAL BY NAME, and a normal fleet of 6

`fat` is not numeric, so it falls through the `n` parse and the default 6 stands. The arg is parsed
and then refused out loud; the fleet still launches.

**Console** — the ordinary `storm 6` transcript, then, after the `post` census:

```
storm: `fat` refused — the USB-writer provocation is aarch64/baremetal-only (fleet launched without it)
```

**Serial:**

```
:: STORM: fatw REFUSED — aarch64/baremetal-only provocation, not ported to x86 ::
```

Scoreable claim: **no `:: STORM: fatw begin`, no `fatw r=` lines, and no `stormfatw` task** on x86.
`storm_fat_writer` drives `BlockSource::Usb` through the Pi's masked FAT/dir RMW path against the
xHCI loan (WEDGE-8/F3) and stays `#[cfg(feature = "baremetal")]`; porting it is its own arc.

## What would falsify the arc

* `Unknown command` still — the gate did not take.
* `storm` prints the console census but **no `[storm]` serial lines** — the x86 `storm_probe` is not
  being reached (or the run carries `UNAOS_BOOTLOG=1`; see above).
* `user slots free` at `end` below 2 with a fleet of 6 — a slot leak, and the reserve assertion in
  `arch::syscall` is no longer describing reality.
* `storm 8` reporting `launched 8/8` — the process-table cap stopped biting, which would make the
  clamp's honesty argument false.
* Any `fatw` line on x86.

## REVIEW CORRECTIONS (seat, pre-flight — these supersede §1/§2 where they conflict)

1. **No x86 instrument survives a starved shell.** The `[schedx86] load` heartbeat is emitted
   from `x86_render_service` — the same task that dispatches shell commands — so a truncated
   `[storm]` tail and a stopped `load` train are ONE silence. §1's closing claim is withdrawn;
   a silent tail is settled by the next boot, not by a surviving witness.
2. **On the `UNAOS_WC=1` build §1 names, STAT.ELF (the desktop app) already holds one proc
   row, one job row, and one user slot.** Expected transcript: `storm: n=6 — 5/6 process rows
   free, 7/8 job rows, 7/8 user slots`, FIVE launches (`k=1..5`), then
   `:: STORM: REFUSED at launch 6 of 6 ... ::` and `storm: launched 5/6 vugs`; `storm 8` is
   refused at launch 6 of 8. Scoreable claims `user slots free=2/8` and
   `proc rows free=0 running=6` stand (the desktop app substitutes for the sixth vug).
   The all-six transcript applies only to a knob-off (no `wc`) build or a card with no
   STAT.ELF staged. The `free < 2` refute stays discriminating in both shapes.
