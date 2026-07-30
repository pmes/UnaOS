# BPACE — the boot-phase timing ledger

## Status: LANDED (x86 + aarch64, always compiled, no knob)

Source: `unaos/crates/kernel/src/bootpace.rs`.
Stamp sites: `main.rs`, `drivers/xhci/mod.rs`, `fs/fat.rs`, `flight_recorder.rs`.

## 1. Why it exists

Until this arc the only measurement of the boot's wall clock was a stopwatch and
an impression — "it sits for a long time before the desktop". That number cannot
be decomposed. It cannot say whether the sit is firmware, the heap, ACPI, the
fixed pre-enumeration USB settle, the per-port enumeration, the SCSI bring-up, or
the first flight-recorder flush; and a constant that has not been decomposed must
not be trimmed. BPACE makes **one** metal boot yield both the current baseline
and the per-phase breakdown, so the next arc's timing work is arithmetic.

It is `bootlog`'s sibling, not its replacement. `bootlog` answers *did this
milestone happen* and is readable on-panel after the GUI takeover. BPACE answers
*when, and how long did the phase before it take*, and its reader is the wire.

## 2. Two deliberate differences from `bootlog`

**Timebase.** Entries are stamped with `arch::now_cycles()`, never `arch::ms()`.
`arch::ms()` is the 1 kHz APIC tick (`arch/x86_64/mod.rs`), and the tick does not
exist until `apic::calibrate` runs deep inside boot — so it reads ~0 for every
phase before calibration and cannot timestamp the part of the boot that most
needs timestamping. `now_cycles()` is rdtsc on x86 and `CNTVCT_EL0` on aarch64:
free-running from reset, arch-neutral, independent of `EFLAGS.IF` and of whether
the timer ISR runs.

**Conversion at print time.** The counter's *rate* is unknown until calibration,
so cycles are converted to milliseconds only when the ledger prints. If the rate
is still unknown then, the ledger prints raw counter ticks with a `cy` suffix and
`hz=0`. This is on purpose: the xHCI driver's `cycles_per_ms()` substitutes a
2 GHz guess, which is the right answer for a *settle* (a wrong guess makes a
settle longer or shorter, never unsound) and the wrong answer for a
*measurement*, where it would turn "I do not know" into a fabricated millisecond
that a later arc would trim a real constant against.

**Overflow is drop-NEWEST** (`bootlog` is drop-oldest). This ledger's subject is
the boot; the entries nearest a late hot-plug are the expendable ones, and losing
`entry` would destroy the origin every other `t=` is measured against. Truncation
is reported as `dropped=` rather than being silent. Capacity is 64 entries
(1 KiB of `.bss`).

## 3. Output format

Emitted by `bootpace::service_dump()`, called from both main-loop service ladders
in `main.rs` (the usbdebug loop and the GUI loop), **ungated**. The full block is
re-printed whenever the ledger has grown:

```
:: BPACE: <tag> t=<since-entry> d=<since-previous> ::
   ... one line per recorded phase, oldest first ...
:: BPACE: total gui=<v> ftdi=<v> n=<phases> dropped=<n> hz=<counter-hz> result=LEDGER ::
```

Durations carry their own unit and are never bare:

| rendering | meaning |
|---|---|
| `412ms` | milliseconds; the counter rate was known when the block printed |
| `948123456cy` | raw counter ticks; `hz=0`, the rate was not known |
| `none` | that phase never recorded at all |

### Why the block is re-emitted on growth

Every line the kernel prints before the FTDI console arms lands in the 64 KiB
capture ring (`drivers/xhci/ftdi.rs`, drop-oldest) and is replayed out the wire
once the console is live. A long boot overflows that ring, and the lines it
discards are the **oldest** — precisely the early phase stamps nobody can
re-measure. Because the whole block is reprinted every time the ledger grows, the
**last** block always rides the live wire after `ftdi-up`, at the tail of the log,
where ring overflow cannot reach it. Bounded by construction: at most one block
per recorded phase.

### Why it carries no env knob

Every change in this arc is default-on. A knob wired into `arroyo` but not into
`unaos/builder/` ships the feature DISABLED with every gate green — that has cost
this project two arcs. The build that reaches metal (`./arroyo esp-x86`) carries
neither `witness` nor `usbdebug`, so a ledger gated on either would be absent
from the only build the bench ever boots. Gate proof is a `strings` count on the
artifact, not a `check` result:

```
strings unaos/target/x86_64_esp/kernel.elf | grep -c 'BPACE'      # must be >= 1
```

## 4. Tag table

| tag | recorded at | what `d=` measures |
|---|---|---|
| `entry` | first statement of `kernel_main` | see note below — approximately firmware + bootloader |
| `heap` | after `arch::memory::init` | early paging/framebuffer/heap setup |
| `acpi` | after `acpi::init` + `dmar_report` + `pm_timer_report` (x86) | the whole ACPI discovery phase |
| `calib` | after `apic::calibrate` (x86) | the TSC/APIC calibration alone |
| `smp` | after `smp::start_aps` (x86) | AP bring-up + the post-bring-up smoke test |
| `pci-usb` | after `pci::init` returns | PCI scan + synchronous xHCI controller bring-up |
| `xhci-settle` | after the pre-CCS-scan settle | that one constant — `hw_wait_budget()/4`, ~0.5 s |
| `enum:p<N>` | `start_next_port`, at the `=== Enumerating Port N ===` line | — |
| `enum:p<N>-done` | top of `start_next_port`, for the port being left | with the pair above: this port's enumeration cost |
| `stor-bringup` | entry to the SCSI bring-up in `service_storage` | — |
| `stor-ready` | after `bring_up_storage` returns **Ok** | TUR + INQUIRY + READ CAPACITY |
| `bot-first` | first completed BOT stage (one-shot, in `run_bot_stage`) | when mass storage first moved |
| `fat-mount` | `mount()` success inside `fs::fat::probe_once` | BPB + FAT read — the first filesystem I/O |
| `fr-flush` | first successful flight-recorder flush | the boot's first sustained WRITE workload |
| `gui` | beside `bootlog::record("gui:handoff")` | **the desktop-up number** |
| `ftdi-up` | beside `bootlog::record("ftdi:console-up")` | when a second host can see anything |

**`entry` is approximate.** On x86 the rdtsc value read there is the counter
since the last processor **reset**, which on a warm boot need not be the moment
power was applied. Read it as an upper bound on pre-kernel time, not as a
measurement of firmware.

**`enum:p<N>-done` means "the FSM left this port", success or surrender.**
`start_next_port` is the single funnel through which the root enumeration FSM
releases a port — reached from every configure-complete branch (storage, FTDI,
HID) and from `recover_enumeration`'s give-up path. One stamp there covers all of
them; stamping the individual Configure-Endpoint branches instead would have
missed HID (whose enumeration continues through SET_CONFIGURATION) and every
failure path. The *outcome* is on the neighbouring `xHCI:` lines; the *time* is
here.

## 5. What this instrument reads — the baseline law

*A witness is not finished until someone has written down what it reads in the
healthy case, what it reads when the mechanism did not run, and shown that those
two differ.* For BPACE:

**Healthy.** The full ledger prints. Every tag from `entry` through `ftdi-up` is
present; `t=` is strictly nondecreasing; `gui=` is in the tens of seconds; and
`ftdi=` is strictly **greater** than `gui=`. That last inequality is structural,
not incidental: on a GUI build the handoff happens *before* the service loop that
runs enumeration, storage and the FTDI hooks starts at all, so every main-loop tag
necessarily lands after `gui`. On a `usbdebug` build the GUI is never reached and
the same ledger reads `gui=none` with `ftdi=` present. `dropped=0`, and `hz=` a
plausible TSC rate (~2.3e9 on the Ivy Bridge rMBP).

**Did not run (a) — no FTDI cable.** `ftdi-up` never records, `total` reads
`ftdi=none`, **and no BPACE block reaches a second host at all**. The absence of
the block *is* the reading. A ledger that "looks fine" on a wire nobody is
watching proves nothing, which is why (a) is listed as a distinct case rather
than folded into (b).

**Did not run (b) — a `UNAOS_SKIP_XHCI=1` build.** `entry`, `heap`, `acpi`,
`calib`, `smp` and `gui` still print, and `gui=` still carries a number; while
`pci-usb`, `xhci-settle`, every `enum:p*`, `stor-bringup`, `stor-ready`,
`bot-first`, `fat-mount`, `fr-flush` and `ftdi-up` are **absent**. That asymmetry
is what proves the USB tags report the USB path rather than the recorder's own
liveness — a counter that printed the same thing with USB compiled out could
falsify nothing.

The three readings differ, so the instrument can falsify.

## 6. Metal baseline

To be filled from the first metal boot after this arc. Take the **last**
`:: BPACE: ... result=LEDGER ::` block in the capture (see §3) — it is the
complete one.

| tag | `t=` | `d=` |
|---|---|---|
| `entry` | | |
| `heap` | | |
| `acpi` | | |
| `calib` | | |
| `smp` | | |
| `pci-usb` | | |
| `xhci-settle` | | |
| `enum:p…` | | |
| `enum:p…-done` | | |
| `stor-bringup` | | |
| `stor-ready` | | |
| `bot-first` | | |
| `fat-mount` | | |
| `fr-flush` | | |
| `gui` | | |
| `ftdi-up` | | |

Totals: `gui=` ______  `ftdi=` ______  `n=` ____  `dropped=` ____  `hz=` ________

Read the log with `awk '/BPACE/'` — **not** `grep`; control bytes in the capture
break it.

## 7. What BPACE is not

It is not a profiler and it does not sample. It records fifteen-odd named
transitions the boot already passes through, at a cost of one `rdtsc` and two
array writes each. It cannot attribute time *within* a phase; when a phase is
found to dominate, the instrument for the inside of that phase is the phase's own
witness line (`:: BOT: … result=SUMMARY ::` for the storage chain,
`BOT_WAIT_BUCKETS` for per-stage latency), not this ledger.
