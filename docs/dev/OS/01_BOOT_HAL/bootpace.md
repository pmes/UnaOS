# BPACE — the boot-phase timing ledger

## Status: LANDED (x86 + aarch64, always compiled, no knob)

Source: `unaos/crates/kernel/src/bootpace.rs`.
Stamp sites: `main.rs`, `arch/x86_64/pci.rs`, `drivers/xhci/mod.rs`, `fs/fat.rs`,
`flight_recorder.rs`.

> **Before quoting any armed-build x86 figure from this document, read §10.** A
> framebuffer memory-typing regression ran from 2026-07-21 to 2026-08-04 and
> inflated every `nvidia-kepler`-armed metal reading taken after the Kepler probe.
> §10c classifies each claim; the short version is that the EHCI and xHCI results
> survive, the desktop *delta* survives but its absolute does not, and
> `pci-usb d=4620ms` must be re-measured. Default-build and aarch64 figures are
> unaffected.

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
| `sched` | after the step-4d scheduler block (x86) | `sched::init` + `enable` + the CLOCK-X1 witness **sample** (the verdict is deferred to the first service pass — §8e) (+ the `witness` ring-3 fixtures, when built) |
| `pci-enter` | first statement of `arch::pci::init` (x86) | step 4e — `apic::report_tick_rate`, a 50 ms PM-timer window |
| `ehci-hid` | before `drivers::ehci::init` | the knob-gated VPERF / EHCI-scout / SMC probes (all absent by default ⇒ ~0) |
| `ehci-hid-done` | after `drivers::ehci::init` | the whole EHCI-3 HID bring-up: 256-bus config walk, wake, port reset, EP0 enumeration — subdivided by the EPACE lines (below), not by more ring stamps |
| `pci-scan` | after `PciScanner::scan()` | the xHCI bus scan (config-space reads only) |
| `portsw` | after the PORTSW-1 flip | `probe_irq_caps` + `enable_bus_master` + the XUSB2PR/USB3_PSSEN routing writes |
| `xhci-handoff` | after `bios_handoff` | the BIOS→OS USBLEGSUP handshake (budget-bounded; a no-op on QEMU) |
| `xhci-halt` | after the `USBSTS.HCH=1` wait | stopping a controller the firmware left running |
| `xhci-hcrst` | after the `USBCMD.HCRST=0` wait | the Intel 1 ms quirk pause + the chip hardware reset |
| `xhci-cnr` | after the `USBSTS.CNR=0` wait | the CNR wall (§1a of `usb_xhci.md`) — ~0 on Intel, ~100s of ms on the VL805 |
| `xhci-ptrs` | after `init_pointers`, before `start()` | command/event ring allocation, `init_interrupter` (a second CNR wait), MSI-X, DCBAAP/CRCR |
| `xhci-run` | after the `USBSTS.HCH=0` wait in `start()` | `CONFIG.MaxSlotsEn` + the RS=1 run handshake |
| `xhci-portpwr` | after the port-power loop in `start()` | one PORTSC read and at most one PP write per root port |
| `xhci-settle` | after the pre-CCS-scan settle | **that one constant alone** — `hw_wait_budget()/4`; see §6a |
| `pci-usb` | after `pci::init` returns | the `start_next_port` tail + the BENCH-RIDE probes + the GPU dispatch + the SDHC probe + the NIC block (NOT the xHCI bring-up — see §6a) — subdivided by the GPACE lines (§9) |
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
`calib`, `smp`, `sched` and `gui` still print, and `gui=` still carries a number;
while the **entire `pci-enter … xhci-settle` subdivision**, `pci-usb`, every
`enum:p*`, `stor-bringup`, `stor-ready`, `bot-first`, `fat-mount`, `fr-flush` and
`ftdi-up` are **absent** — `arch::pci::init` is not called at all on that build,
so `sched` is followed directly by `gui`. That asymmetry is what proves the USB
tags report the USB path rather than the recorder's own liveness — a counter that
printed the same thing with USB compiled out could falsify nothing. Since
BOOTPACE M4 the two readings differ in **thirteen** lines rather than one, so the
subdivision is itself falsifiable: a `skip_xhci` build that still printed
`xhci-cnr` would convict the stamps of reporting the recorder, not the
controller.

The three readings differ, so the instrument can falsify.

### Ring capacity — the `dropped=` arithmetic

`dropped=` is only honest if someone did the sum. M4 takes the fixed phases from
15 to 28. The first metal ledger recorded `n=18` with two ports enumerated; the
same boot under M4 records **31**, leaving 33 of `CAP=64` free — room for 16
further port enumerations (two stamps each) before `dropped=` could go non-zero.
That is well past `MaxPorts` on any controller this kernel has met, so `CAP`
stays 64 and no capacity change rides this arc.

## 6. Metal baseline (2012 rMBP, 2026-07-30, post-M3)

The first complete metal ledger. Take the **last**
`:: BPACE: ... result=LEDGER ::` block in the capture (see §3) — it is the
complete one.

| tag | `t=` | `d=` |
|---|---|---|
| `entry` | 0 ms | 0 ms |
| `heap` | 296 ms | 296 ms |
| `acpi` | 296 ms | 0 ms |
| `calib` | 396 ms | 100 ms |
| `smp` | 456 ms | 60 ms |
| `xhci-settle` | 7746 ms | **7289 ms** |
| `enum:p1` | 7746 ms | 0 ms |
| `pci-usb` | 7860 ms | 113 ms |
| `gui` | 7875 ms | 15 ms |
| `enum:p1-done` | 8003 ms | 127 ms |
| `enum:p2` | 8003 ms | 0 ms |
| `ftdi-up` | 8003 ms | 0 ms |
| `enum:p2-done` | 10783 ms | 2780 ms |
| `stor-bringup` | 10951 ms | 167 ms |
| `bot-first` | 10951 ms | 0 ms |
| `stor-ready` | 11577 ms | 626 ms |
| `fat-mount` | 11589 ms | 11 ms |
| `fr-flush` | 11677 ms | 87 ms |

Totals: `gui=7875ms`  `ftdi=8003ms`  `n=18`  `dropped=0`  `hz=2693848854`

Time to serial console 121 s → 8.0 s and desktop 35 s → 7.9 s across M1–M3; the
M2 ordering inversion is confirmed on this boot (`ftdi-up` precedes
`stor-bringup`).

Read the log with `awk '/BPACE/'` — **not** `grep`; control bytes in the capture
break it.

### 6a. Why `xhci-settle d=7289ms` was never a settle measurement (BOOTPACE M4)

That one delta is 92% of the time to console, and reading it as "the settle costs
7.3 s" would have been wrong in kind, not just in degree.

`d=` is always the delta from the **previous stamp**. Before M4 the previous
stamp was `smp` — there was nothing between them — so `xhci-settle`'s `d=`
silently contained:

- `sched::init` / `enable` / the CLOCK-X1 witness,
- `apic::report_tick_rate` (a 50 ms PM-timer window),
- the EHCI-3 HID bring-up (default-**on** since EHCI-4 M1): a 256-bus config-space
  walk plus a wake, port reset and synchronous EP0 enumeration per EHCI function,
- a second config-space walk in `PciScanner::scan()`,
- `probe_irq_caps`, `enable_bus_master`, the PORTSW-1 routing flip,
- the BIOS→OS `USBLEGSUP` handshake — budget-bounded at ~2 s and free to consume
  all of it on firmware that does not let go,
- the halt / `HCRST` / `CNR` chain — three more ~2 s-bounded waits,
- ring + interrupter programming (including a second `wait_for_cnr_clear`), MSI-X,
  `DCBAAP`/`CRCR`, the RS=1 run handshake, the port-power loop,
- and, last, the settle itself.

The settle's own budget is `hw_wait_budget()/4` ≈ **500 ms** — 1/14th of the
bucket. Whatever else the 7.3 s is, it is overwhelmingly *not* this constant, and
a trim aimed at the constant on the strength of that number would have been aimed
at the wrong thing.

The source comment beside the stamp compounded it, claiming `d=` was "measured
from `pci-usb`". `pci-usb` is recorded **later** — after `pci::init` returns —
because `start()` kicks port enumeration before the function ends. The comment was
corrected in the same commit.

M4 therefore adds thirteen stamps across `main.rs`, `arch/x86_64/pci.rs` and
`drivers/xhci/mod.rs` (see the tag table in §4) so every one of the bullets above
is its own line. Placement rule: **every phase containing a
`hw_wait_budget()`-derived wait is stamped on both sides**, so a boot that dies
inside a phase still names the phase it entered — a stamp that only exists after
a wait that can itself hang tells you nothing about the hang.

Also unattributed and left for a later arc: `enum:p2-done d=2780ms`. One root
port took 2.8 s to enumerate, which is far more than the debounce plus reset
recovery accounts for. That is a per-port question (`xHCI:` lines around the
`=== Enumerating Port 2 ===` marker), not a settle question, and M4 does not
chase it.

## 7. What BPACE is not

It is not a profiler and it does not sample. It records some thirty named
transitions the boot already passes through, at a cost of one `rdtsc` and two
array writes each. It cannot attribute time *within* a phase; when a phase is
found to dominate, the instrument for the inside of that phase is the phase's own
witness line (`:: BOT: … result=SUMMARY ::` for the storage chain,
`BOT_WAIT_BUCKETS` for per-stage latency), not this ledger.

## 8. EPACE — the inside of `ehci-hid-done` (GR12)

The first metal ledger with the M4 split read `ehci-hid-done d=6324ms` — 93% of
the boot's remaining block, in one bucket. Per §7, the instrument for the inside
of a dominating phase is the phase's own witness, so the split lives in
`drivers/ehci/mod.rs` as EPACE, not as more ring stamps: the hub walk is
per-port × per-tier, and overflowing the 64-slot ring would trigger drop-NEWEST
and silently destroy every later boot tag.

EPACE keeps cycle accumulators per phase class on each `Controller` and prints
one summary line per controller before `:: EHCI-HID: end ::`:

```
:: EPACE: [i] wake= hcrst= smoke= rootrst= hseprobe= enum= [hubpwr= hubrst= hidcfg= resid=] == witness ::
:: EPACE: selftest= evid= init= hz= == the ehci-hid d= split ::
```

* `wake` PMCSR-D0/legsup/RS/CONFIGFLAG (+ its 150 ms settle) · `hcrst` the
  firmware-stale quiesce + RS restart, including the probe-14 full re-init ·
  `smoke` the 5 periodic DMA passes · `rootrst` connect debounce + paced root
  resets · `hseprobe` the probe-14 transport probe · `enum` the whole top-level
  `enumerate_at_zero` span.
* The bracketed classes accumulate **inside** `enum` across hub recursion:
  `hubpwr` (PORT_POWER + pwr2good settle), `hubrst` (downstream reset + poll +
  acks), `hidcfg` (`configure_hid`). `resid=` is `enum` minus those three —
  control-transfer descriptor time plus recursion overhead. A large `resid` is a
  finding, not noise.
* Spans are wall-clock around the call sites, so each class contains its own
  serial printing. `evid=` (the DMAR/PCI/RCBA dump) is almost pure serial output
  and therefore doubles as the measured print cost of ~70 witness lines — the
  calibration for how much of every other phase is serial time.

Instrument-baseline: the clock is the same `now_cycles()`/`tsc_hz()` pair the
settles themselves use — a miscalibration moves the report and the settles
together, keeping ratios truthful; `hz=0` prints raw `cy`, never a fabricated
ms. The self-check is `init=` against the independent BPACE `ehci-hid-done d=`:
they must agree to within the EPACE lines' own print cost, or one of the two
instruments is lying.

### 8a. The first EPACE metal reading, and the trim it aimed (s58, 2026-08-01)

One boot: `init=6324ms` against BPACE `ehci-hid-done d=6324ms` — the self-check held
exactly. The split: `hseprobe=2000ms` on BOTH controllers (63% of the block — the probe-14
transport probe burning one full `hw_wait_budget()` per controller on silicon whose answer
was already known), plus the doubled HCRESET/root-reset the HSE re-init forces (~1.25 s
across both), against ~1 s of real enumeration.

EPACE-TRIM M1 (same arc): the chain-HSE verdict is a property of the PCH DMA path, not of
one EHCI function — `CHAIN_HSE_SEEN` carries it, so later controllers are born
overlay-direct: no probe, no re-init, no doubled resets (~2.6 s). The first controller
still measures it (the probe IS the platform check; QEMU requires chain mode). A carried
verdict is witnessed as `chain-HSE verdict CARRIED … (inference, not a measurement)`, and
a wrong inheritance cannot fail silently — the enumeration error witnesses are
unconditional.

EPACE-TRIM M2 (same arc): the remaining 2000 ms on the first controller is one budget burn
across three bounded waits (PSS enable, completion, PSS disable) — undecomposable from
outside, and a constant that has not been decomposed must not be trimmed. `chain HSE
sub-split: sched-en= done-wait= sched-dis=` prints on the failure exits only; the next
metal boot names the wait, and the trim that follows is arithmetic.

### 8b. s59 metal verdicts (2026-08-01, same sitting)

`ehci-hid-done d=4010ms` (from 6324): M1 verified — the CARRIED witness printed, `[1]
hseprobe=0ms(n=0)`, controller 1's hcrst/rootrst halved (no re-init). Desktop 12.4 → 10.0 s.
The M2 sub-split named the guilty wait exactly: `sched-en=0ms done-wait=0ms sched-dis=2000ms`
— the HSE latches instantly, the completion wait exits on it, and the whole budget burned
waiting for PSS to clear on the wedged engine, ahead of a caller that HCRESETs regardless.
EPACE-TRIM M3 (landed after this boot) skips the PSS wait on the HSE path only; the healthy
path keeps the full EHCI 4.8 handshake. Expected next ledger: `ehci-hid-done d=` ≈ 2.0 s,
`sched-dis=0ms` on the sub-split, boot to desktop ≈ 8.0 s with all four GPU/SMC lanes in.

**Prediction closed — both halves achieved, same capture.**
`/home/pmes/unaos-bench/capture/rmbp-gr12/ttyUSB0.log` holds four boots and its
fourth is the post-M3 one:

| line | reading |
|---|---|
| 3787 | `:: EPACE: selftest=0ms evid=0ms init=2010ms hz=2693847134 ::` |
| 4334 | `:: BPACE: ehci-hid-done t=2999ms d=2010ms ::` |
| 4348 | `:: BPACE: total gui=8048ms … result=LEDGER ::` |

`init=2010ms` against `ehci-hid-done d=2010ms` — the §8 self-check held exactly, a
third time. The `init=` progression across that one capture's four boots is
6324 → 6324 → 4010 → 2010 ms, and `total gui=` moves 12429 → 12429 → 10037 → 8048 ms.
§9 cites the 12429 → 8048 move as context for GPACE without saying it is this
prediction landing; it is. See §10 for what the 8048 ms absolute is worth and what
the 4.4 s delta is worth — they are not the same answer.

### 8c. The 2021 ms floor decomposed, and the two waits that were not settles (GR18, 2026-08-06)

`init=2010..2022ms` has been the same number on every metal boot since s59 — eight
boots of `rmbp-gr16-s73/ttyUSB0.log` (2020, 2020, 2021, 2021, 2021, 2021, 2022, 2022)
across witness-on, witness-off, kepler-armed and default builds. That constancy is
itself the finding: a block that does not move when everything around it moves is not
measuring anything. Per-class, boot 7 (`ttyUSB0.log` L8302-8304, `gui=6242ms`):

| class | [0] | [1] | total | what it is | verdict |
|---|---|---|---|---|---|
| `hubpwr` | 200 (n=1) | 400 (n=2) | **600 ms** | `settle_ms(pwr2good_ms + 100)`; all three hubs declare `pwr-on 2 good 100 ms` | **spec floor — do not trim.** The `+100` is not margin: it is the USB 2.0 §7.1.7.3 T_ATTDB attach debounce, which for an already-attached downstream device starts at power-good and must complete before the port reset two lines below. 100 + 100 is exactly right; it was merely unlabelled. |
| `rootrst` | 320 (n=2) | 160 (n=1) | **480 ms** | 100 T_ATTDB + 50 PR hold + 10 T_RSTRCY per attempt | spec floor — three USB 2.0 minima, no slack |
| `hcrst` | 307 (n=2) | 154 (n=1) | **461 ms** | HCRESET + `wake_route`'s `settle_ms(150)` | **450 of the 461 is the settle** — see EPACE-TRIM M4 |
| `hubrst` | 50 (n=1) | 250 (n=5) | **300 ms** | `settle_ms(50)` in front of a bounded poll | **50.0 ms per port, to the millisecond** — see EPACE-TRIM M5 |
| `smoke` | 29 | 29 | 58 ms | 5 periodic DMA discriminator passes | real work |
| `resid` | 29 | 68 | 97 ms | control transfers + hub recursion | real work |
| `hidcfg` | 5 | 7 | 12 ms | | real work |
| `wake` / `hseprobe` | 0 | 0 | 0 ms | | already zero since M1/M3 |
| sum | 941 | 1069 | **2010 ms** | (+11 ms of EPACE's own print cost → `init=2021ms`) | |

**EPACE-TRIM M4 — the port-power settle that guarded a power edge that never happened.**
`ehci_scout::wake_route` closed with an unconditional `settle_ms(150)` described as a
connect-debounce. It runs three times per boot (controller 0 routes twice — the probe-14
re-init — and controller 1 once), which is the whole `hcrst=` column: 3 × ~154 ms against
an HCRESET that measures ~4 ms. The settle bundled two debts and neither survives contact
with the capture:

* **Power-good** is owed only if this function applied port power. It does not, on this
  silicon. Three lines say so in as many words, every boot:
  `:: EHCI-CONFIG: [0] PPC=0 — ports always-powered, no PP write ::` (L8192, L8205) and
  the same for `[1]` (L8224). The gap that follows each is the pure sleep:
  L8192 `t=1419ms` → L8193 `t=1575ms` = **156 ms**, and L8224 `t=2364ms` →
  L8225 `t=2520ms` = **156 ms**. No PP write, no edge, nothing to settle.
* **Attach debounce** is owed either way — and is already paid in full by the only caller
  that looks at a port. `reset_root_port` opens with `settle_ms(100)` labelled T_ATTDB
  before its first PORTSC read. Paying it twice, serially, was the redundancy.

M4 prices the settle by what happened: `PPC=1` keeps the full 150 ms untouched (this arc
has no measurement of a real power-on edge, and an undecomposed constant is not trimmed);
`PPC=0` takes 20 ms, which covers only the CONFIGFLAG 0→1 port-mux re-route, with the
caller's 100 ms T_ATTDB still landing on top.

**M4's real hazard was the ordering, and that is what the follow-up fixes.** The caller that
pays T_ATTDB is not the first thing to look at a port. The first look is the CCS gate at the
top of the port walk — `if portsc & PORT_CCS == 0 || portsc & PORT_OWNER != 0 { continue }` —
and it decides whether `reset_root_port` is called at all. CF 0→1 is a real edge on this path
(the firmware-stale HCRESET drops CONFIGFLAG, and the first PORTSC read comes back
`0x00001803` with CSC latched), so as first written M4 sampled CCS ~49 ms after that edge
where the old code sampled at ~180 — 51 ms inside the debounce the section above says is
owed. A port whose CCS had not re-asserted would take the bare `continue`: no line, no EPACE
class, no annotation, and a boot that reads *faster* than predicted for the worst possible
reason, the internal keyboard missing. So the debounce is paid **ahead of the gate**
(`settle_ms(100)` before the port loop, charged to `rootrst`) and dropped from
`reset_root_port` for that path — the same 100 ms, at the point that needs it, `rootrst=`
totals unchanged, `n=` one higher per controller. The probe-14 re-init path keeps its own
debounce: it re-routes CONFIGFLAG and returns to a known-connected port without passing the
gate. And the skip is now loud: `port N not walked: PORTSC=… CCS=… owner=…
(post-T_ATTDB sample)`. That line, not a timing number, is what catches this class.

**`PWR_SETTLE_TRIMMED` is an annotation, not a tripwire, and the code says so.** Every
root-port failure line carries the settle that was in force, which narrows a diagnosis — but
PPC is a property of the silicon, not of the boot, so on the bench the string is present on
every failure and absent on none. A latch that cannot vary cannot falsify. It is excluded
from the falsifier list below for that reason.

**Coverage, stated plainly: M4 and M5 have no QEMU coverage at all.** The x86 QEMU targets
attach `qemu-xhci` and nothing else — there is no EHCI function in the emulated machine, so
`drivers/ehci` never runs there. Both trims are exercised only on metal, and the `ppc == 1`
branch M4 deliberately leaves at 150 ms has no coverage anywhere: it is the untested-but-
unchanged path, kept precisely because this arc has no measurement that would justify moving
it. The gates for this work are `./arroyo check` (both arches) plus a metal boot; QEMU-green
carries zero information about either trim.

**EPACE-TRIM M5 — a blind sleep standing in front of a poll that already existed.**
The downstream-port reset ran `settle_ms(50)`, then a bounded 60 × 10 ms poll of
`PORT_RESET` with a ~600 ms budget. `hubrst=50ms(n=1)` and `hubrst=250ms(n=5)` — 50.0 ms
per port on all eight boots — means the poll's **first** probe always found the bit
already clear: the loop has never once iterated, and the real reset time is somewhere
under 50 ms and has never been measured. M5 starts at the USB 2.0 §11.5.1.5 T_DRST floor
(10 ms, the minimum a hub may drive reset for) and lets the existing poll measure the rest.
The cost becomes 10 + 10·`poll_steps` ms against the old flat 50: cheaper for any port that
clears inside 40 ms, equal in the 40-50 band, dearer only past 50 — and the report threshold
is `>= 4` precisely so that the band where the trim stops paying can never be silent. The
budget and its loud exit are unchanged.

T_RSTRCY (USB 2.0 §7.1.7.5, 10 ms of reset recovery before the device is addressed) is **not**
paid by M5 and never needed to be: the `settle_ms(10)` immediately before `enumerate_at_zero`
already is it, correctly placed — only hub-addressed ClearPortFeature traffic sits between the
reset completing and that point — and it predates this arc. The first cut of M5 added a second
one under the belief that the old 50 ms had been supplying the recovery by accident; it was
not, and the duplicate is gone. That interval is now labelled at its site so the next reader
does not repeat the mistake.

The number is now printed, in four cases that are deliberately not interchangeable:

* `poll_steps == 0` — silent. **Expected on some ports, not framed as the norm**: an HS hub
  typically holds PORT_RESET for ~20 ms through the handshake, so `poll_steps` of 1-2 on most
  ports every boot is the healthy reading, not a regression. Silence means only that this
  particular port cleared inside the 10 ms T_DRST floor.
* `0 < poll_steps < 4` — one quiet line naming the observed milliseconds.
* `poll_steps >= 4` **with the bit actually cleared** — `EPACE-TRIM M5 TRIPWIRE`: this
  hardware wanted the old constant.
* the bit never cleared — a *separate* `EPACE-TRIM M5 TRIPWIRE` that names the timeout and
  says in the line that it is not a reset-time measurement. `poll_steps` counts sleeps, so on
  budget exhaustion it reads 60 whether PORT_RESET was stuck or `GET_PORT_STATUS` itself was
  failing; the first cut printed "needed ~610 ms to clear PORT_RESET" in both cases, which the
  very next line contradicted.

**Prediction for the next metal boot** (before → after, and the lines that decide it):

| reading | before (boots 7/8) | predicted after |
|---|---|---|
| `EPACE: [0] hcrst=` | 307 ms | **47 ms** (2 × ~4 ms HCRESET + 2 × 20 ms settle) |
| `EPACE: [1] hcrst=` | 154 ms | **24 ms** |
| `EPACE: [0] hubrst=` | 50 ms | **10-22 ms** |
| `EPACE: [1] hubrst=` | 250 ms | **50-110 ms** (10-22 ms × 5 ports) |
| `EPACE: [0] rootrst=` | 320 ms | **320 ms, unchanged** — the debounce moved earlier inside the same class |
| `EPACE: [1] rootrst=` | 160 ms | **160 ms, unchanged** |
| `EPACE: … init=` | 2021 ms | **1400-1470 ms** |
| `BPACE: ehci-hid-done d=` | 2021 ms | **1400-1470 ms** (self-check: must equal `init=` per §8) |
| `BPACE: total gui=` | 6242 / 6266 ms | **≈ 5640-5700 ms** |

M4's share is deterministic (−130 ms × 3 = −390 ms of pure sleep); M5's is the measured
unknown (−170 to −240 ms), which is the point of it. This table is the **re-issued**
prediction: the first cut said 1460-1525 ms because it double-paid T_RSTRCY, ~10 ms on each
of six ports.

**Structural falsifiers — the `n=` counts, which move independently of any timing.** A trim
that quietly loses a device makes every `ms` reading in the table look *better*, so the
counts are the real gate and any change to one falsifies the arc regardless of `init=`:

| reading | required | what a change would mean |
|---|---|---|
| `EPACE: [0] rootrst n=` | **3** (was 2) | +1 is the pre-scan T_ATTDB becoming its own span. Anything else: a root port stopped being walked. |
| `EPACE: [1] rootrst n=` | **2** (was 1) | same |
| `EPACE: [0] hubpwr n=` / `[1] hubpwr n=` | **1** / **2** | a hub tier vanished from the walk |
| `EPACE: [1] hubrst n=` | **5** | a downstream port stopped being reset — the exact silent-skip class finding 1 was about |
| `:: EHCI-HID: [1] M2 armed keyboard addr=6 ep=IN3 mps=10 interval=8 (boot protocol) ==` witness `::` | **present, identical** | the internal keyboard is the whole point of this driver; if this line moves or goes, nothing else in the table matters |
| `:: EHCI-HID: [N] port P not walked: …` | **absent** on a healthy boot | a root port failed the post-T_ATTDB CCS sample |

Timing falsifiers, after those: an `EPACE-TRIM M5 TRIPWIRE` (the hubs wanted the old 50 ms,
or the poll timed out); or `init=` and `ehci-hid-done d=` disagreeing by more than the EPACE
print cost, which would mean one of the two instruments is lying rather than that the trim
worked. The M4 annotation is deliberately **not** on this list — see above; it cannot vary on
this silicon.

**Not trimmed, and why.** `hubpwr` (600 ms) and `rootrst` (480 ms) are 1080 ms of USB 2.0
minima. Cutting either buys a second and violates the spec; they are named here so the
next reader does not have to re-derive that they are floors.

### 8d. Where the armed-minus-off second lives outside the kepler window (GR18)

`gui=6242ms` (witness + kepler) against `gui=4094ms` (witness-OFF, kepler, boot 2) is a
2148 ms delta. §10h closed the in-window half; the out-of-window half is one line.

| phase | boot 7 armed | boot 2 witness-OFF | Δ |
|---|---|---|---|
| `heap` | 253 | 296 | −43 (jitter) |
| `calib` / `smp` / `pci-enter` | 210 | 210 | 0 |
| **`sched`** | **951** | **17** | **+934** |
| `ehci-hid-done` | 2021 | 2022 | −1 |
| `pci-scan` | 100 | 0 | +100 (boot 8 reads 9 ms — jitter, not a witness cost) |
| `xhci-*` (incl. `xhci-settle` 100) | 101 | 101 | 0 |
| `pci-usb` (the kepler window) | 2587 | 1430 | +1157 |
| `gui` | 15 | 15 | 0 |
| | | | **+2147** |

The entire `sched` delta is one witness that waits on a 1 Hz edge:

```
[    414ms] :: U2-0c: canonical-rcx guard refuses 0x8000_0000_0000_0000 -> PASS ::
[   1210ms] :: CLOCK-X1: TSC invariant, ~2693 MHz; monotone (rdtsc +2143545592);
             uptime 15->16 s (JD17 x86-frozen clock now advances) == witness ::
```

boot 7 L7990 → L7991, **796 ms**; boot 8 L10106 → L10107, **910 ms**. CLOCK-X1 asserts
that the uptime *seconds* counter advances, so it blocks for whatever is left of the
current second — mean 500 ms, worst 1000 ms, and the boot pays it at the one moment
nothing else is in flight. It is the same shape §10h solved for the video battery: the
assertion is sound, the *placement* is the cost. Named, not taken, in this arc: it is one
line in the clock witness, not in the EHCI lane. **§8e takes it, and corrects the
attribution above** — the delta is not a witness-build cost at all.

### 8e. `sched` was never a witness cost — it was a coin flip against the 1 Hz edge (GR18)

§8d read the `sched` delta off boots 7 and 8 and filed it under "witness-armed". That
attribution is wrong, and the capture says so on one line: **boot 3 is a witness-OFF
build and it paid 979 ms.** `clock_x1_witness()` has no `#[cfg]` on it — it is called
unconditionally from the step-4d block in `main.rs`, on every x86 build, which is why all
eight boots of `rmbp-gr16-s73/ttyUSB0.log` print a `CLOCK-X1` line. What varies is not the
feature set. It is the *phase*.

`clock::uptime_secs()` is `rdtsc / tsc_hz`, and on x86 the TSC counts from CPU reset, not
from our entry — hence `uptime 14..24 s` on a boot whose own `t=` is under half a second:
the reading is dominated by the firmware's POST. The old witness spun until it saw that
whole-second value change, so its cost was `1000 − (uptime_ms mod 1000)` at the moment
step 4d reached it: a uniform draw over the second, with a phase nobody can predict or
control. Eight boots, the gap from the line before `CLOCK-X1` to `CLOCK-X1` itself:

| boot | build | preceding line | `CLOCK-X1` | **wait** | uptime | `sched d=` |
|---|---|---|---|---|---|---|
| 2 | default | L1809 `t=456ms` | L1810 `t=474ms` | **18 ms** | 14→15 | 17 |
| 4 | default | L3616 `t=456ms` | L3617 `t=483ms` | **27 ms** | 14→15 | 26 |
| 5 | witness | L4737 `t=414ms` | L4738 `t=519ms` | **105 ms** | 14→15 | 260 |
| 1 | witness | L66 `t=414ms` | L67 `t=904ms` | **490 ms** | 16→17 | 646 |
| 6 | witness | L6353 `t=414ms` | L6354 `t=1021ms` | **607 ms** | 16→17 | 763 |
| 7 | witness | L7990 `t=414ms` | L7991 `t=1210ms` | **796 ms** | 15→16 | 951 |
| 8 | witness | L10106 `t=414ms` | L10107 `t=1324ms` | **910 ms** | 23→24 | 1066 |
| 3 | **default** | L2900 `t=457ms` | L2901 `t=1436ms` | **979 ms** | 15→16 | **979** |

Sorted by wait, the builds interleave completely — the witness-off boots hold both the
minimum (18 ms) and the maximum (979 ms). `sched d=` decomposes exactly, on every row, as
**wait + post**, where post is 155–156 ms on all five witness boots (the ring-3 fixture
ladder that follows: LOGWIT-1, SNTP-X86-GATE, DNS-X86-GATE, U1a, U2-0a) and **0 ms** on all
three default boots. Boots 2 and 4 did not "skip" anything; they arrived 18 and 27 ms
before an edge and got the whole assertion for nearly free. That is luck, not a build
difference, and it is exactly what makes the cost intolerable: it is unbudgetable.

**The trim.** `clock_x1_witness()` now SAMPLES (`uptime`, `rdtsc`, and the 1 kHz APIC tick)
and returns; `clock_x1_poll()` delivers the verdict from `bootpace::service_dump()` — the
one call all three x86 service loops make ungated (BSP GUI, `usbdebug`, SCHED-X86
`x86_usb_pump`), so it reaches the `./arroyo esp-x86` media build that actually boots on
the bench. The first service pass is seconds past the edge on every build, so the advance
is observed with zero waiting. No knob: the witness is not optional, it is paid later.

**What the deferred form proves that the blocking form could not.** The sample captures
`arch::ticks()` alongside `rdtsc`, so the verdict cross-checks the deferral as the TSC
measured it against the deferral as the APIC tick measured it, in MILLISECONDS, and prints
both figures:
`[paygo: deferred 3620 ms TSC / 3608 ms APIC, uptime +4 s, core=7 — CONSISTENT]`.

Both numbers are on the wire deliberately. `apic::ticks()` counts INTERRUPTS, so it
undercounts by the total time IF was masked, and the EHCI bring-up busy-spins masked by
design — over a ~19.6 s compositor-boot deferral that loss is on the order of 2.5 s. Print
only the difference and that artefact is indistinguishable from a calibration fault; print
both and it is a readable number a reader separates on sight. The tolerance scales for the
same reason: `SKEW` fires past `200 ms + 5 % of the deferral`, which catches a 10 %
`tsc_hz` error at any deferral over ~2 s while absorbing ~5 % of masked time. (The
first draft compared whole SECONDS with a flat ±2 s window — at a 3 s deferral that needed
~65 % miscalibration to fire, and at 19.6 s it false-fired on masked-tick loss alone.)

**What the cross-check cannot see**, and it is a real limit: `tsc_hz` and the APIC timer's
`initcnt` are both derived from the same `pm_hz`/`elapsed_pm` denominator in
`apic::calibrate`. A bad PM-timer reference scales both identically, the two views agree,
and the error is provably undetectable here. What `SKEW` convicts is a **differentially
mis-armed heartbeat** — one arm mis-derived while the other is right, the shape of the
historical ~8× `FIXED_INITCNT` bug — not a calibration oracle.

A second derivation that is genuinely stuck now prints
`:: CLOCK-X1: FROZEN — uptime still N s after M ms APIC / K ms TSC …`, where the old
iteration cap expired into a benign `monotone (… over 50000000 spins, <1 s)` line that
*claimed the witness passed*. That fallback line is gone; it could not be told from a fast
boot, which is the definition of an instrument that cannot falsify. The frozen deadline is
armed on BOTH counters (3000 ms of APIC ticks **or** `tsc_hz × 3` cycles), because a
deadline measured only by the tick cannot survive the case where the tick is dead too:
`elapsed_ms` would stay 0 and the poll would go silent for the whole boot.

**What it can no longer see, stated plainly.** The blocking form always caught the FIRST
transition and so always reported `u1 -> u1+1`. Delivered at a service pass the advance is
several seconds, so "the uptime jumped further than wall time" is no longer visible as a
surprising pair of adjacent integers. The millisecond cross-check replaces that reading and
sharpens it — but a jump inside the scaled window is now tolerated where before it would
have been eyeballed. Recorded deliberately.

**The two halves run on different cores.** The sample is the BSP inside step 4d; on a
SCHED-X86 build the verdict runs in `x86_usb_pump` on the service core (core 7 on the
bench), so every subtraction is cross-core. That is sound only because this kernel never
writes the TSC — no `wrmsr` to `IA32_TIME_STAMP_COUNTER` (0x10) or `IA32_TSC_ADJUST`
(0xC000_0103) exists in the tree, and the APs are offered no sync step, so they keep the
firmware's power-on synchronisation. The invariant was true when this landed and is stated
here because nothing else states it; the verdict carries `core=N` so a boot that breaks it
shows a skew attributable to a named core. The APIC half needs no such argument —
`apic::ticks()` is one global counter driven by the BSP.

**Absence is loud, and it is read by WHICH line is present, not by counting.** The FTDI
capture ring is drop-oldest, so an overflowed capture can hold the verdict without the
armed line — counting `CLOCK-X1` lines would misread that as the failure case. The rule:
an armed line (`… SAMPLED — second-advance DEFERRED …`) with **no verdict line anywhere
below it** means the boot never reached a service pass, the same reading and the same cause
as a missing `:: BPACE:` ledger block (§5). A verdict line with no armed line above it is
ring overflow and proves the witness fired. Zero `CLOCK-X1` lines still means what it
always meant: no invariant TSC or no calibration, the honest silent gate.

**Prediction for the next metal boot:**

| reading | before | predicted after |
|---|---|---|
| `BPACE: sched d=`, default build | 17 / 26 / **979** ms | **0–3 ms, on every boot** |
| `BPACE: sched d=`, witness build | 260 / 646 / 763 / 951 / 1066 ms | **155–160 ms, on every boot** |
| `BPACE: total gui=` | boot-dependent | lower by that boot's old wait (0–1000 ms) |
| `CLOCK-X1` lines per boot | 1 | **2** — SAMPLED at `t≈460ms`, verdict after the first `BPACE:` block |

**Watched side effect, not a risk claim.** Deleting a mean-500 ms block that sat directly
after `sched::enable()` means everything downstream of it — the `witness` ring-3 fixture
ladder, then the EHCI bring-up — now starts up to ~1 s earlier against unchanged hardware
timers and unchanged firmware state. Nothing in either path is known to depend on that
delay, and none of it was ever a documented settle. It is named here so the first boot is
read with it in mind rather than discovering it: an EHCI or ring-3 anomaly that appears on
the first post-trim boot and on no earlier capture should be tested against this before
anything else.

The headline is not the mean saving (~500 ms) but the **variance**: `sched d=` becomes a
constant. Falsifiers, in order: a default-build `sched d=` above 10 ms (the sample still
blocks somewhere); a `CLOCK-X1: FROZEN` line (either the clock really is stuck or the
3000 ms / `tsc_hz × 3` deadline is too tight against a slow first service pass); a
`NON-MONOTONE` line (either counter went backwards, with `a`/`b`/`u1`/`u2` naming which);
a `SKEW` clause — read the two printed millisecond figures before blaming anything, since
the two causes are **a differentially mis-armed heartbeat** and **APIC tick loss across a
long IF-masked phase**, and only the first is a defect; or an armed line with no verdict
below it.

**Not trimmed, and why.** The 155 ms of post-sample work on a witness build is the ring-3
fixture ladder doing real transfers, not a wait — it is named here so the next reader does
not mistake the residual for more edge-blocking. The ~0 ms default floor is
`sched::init` + `sched::enable` + one serial line, and there is nothing left in it.

### 8f. `enum` decomposed to the millisecond — 79% of it is USB 2.0 minima (GR19, 2026-08-06)

M4/M5 took `init=` 2021 → 1482 ms and left `enum` as the largest class: `enum=285ms(n=1)` on
controller [0] and `enum=576ms(n=1)` on [1], 861 ms of the remaining 1470. This section
decomposes those 861 ms **without a new boot**: the M1/M5 witness lines already carry
timestamps at every phase boundary, so the post-M4/M5 boots of
`~/unaos-bench/capture/rmbp-gr16-s73/ttyUSB0.log` (final boot, L15607-L15746) decompose the
class arithmetically. Reading the timeline against the code:

| item | site | [0] | [1] | total | verdict |
|---|---|---|---|---|---|
| `hubpwr` — bPwrOn2PwrGood + T_ATTDB | `bring_up_hub`, `settle_ms(pwr2good+100)` | 200 | 400 | **600 ms** | **floor.** USB 2.0 §11.23.2.1 (all three hubs declare `pwr-on 2 good 100 ms`) + §7.1.7.3. Confirms §8c; now cited at the site. |
| `hubrst` — T_DRST + poll | port reset loop | 20 | 100 | **120 ms** | 10 ms is the §11.5.1.5 floor; the other 10 is **poll quantization** — see M6 |
| T_RSTRCY — 10 ms/port | `settle_ms(10)` before `enumerate_at_zero` | 10 | 50 | **60 ms** | **floor**, USB 2.0 §7.1.7.5 |
| SET_ADDRESS recovery — 2 ms/device | `settle_ms(2)` in `enumerate_at_zero` | 4 | 12 | **16 ms** | **floor**, USB 2.0 §9.2.6.3 |
| `hidcfg` | `configure_hid` | 5 | 7 | **12 ms** | real work |
| control transfers (the rest of `resid`) | `control()` | ~47 | ~7 | **~54 ms** | **undecomposed — instrumented, not trimmed.** See M7 |
| sum | | 286 | 576 | **862 ms** | (matches `enum=286` / `576`, L15743-15744) |

The per-port arithmetic is visible in the log rather than inferred. Controller [1]: root RMH
addressed at `t=1521ms` (L15666) → `hub addr 1: 8 downstream ports` → +200 ms `hubpwr` → port 1
reset clears at `1742ms` (L15668, `1521+200+20`) → child addressed at `1757ms` (L15669, i.e.
10 T_RSTRCY + 2 SET_ADDRESS recovery + ~3 ms of wire) → and so on through five ports and two
hub tiers to `M2 armed keyboard` at `2095ms` (L15682). Every gap is a named floor plus 1-3 ms.
**676 of the 861 ms (79%) are USB 2.0 minima this arc will not touch.**

**The one anomaly the timeline exposes, and why it is not trimmed.** Controller [0] spends
`1241ms` → `1299ms` — **58 ms** (L15641 → L15642) — on a single device, `05ac:8510` at
address 2. Subtract the 10 ms T_RSTRCY and the 2 ms SET_ADDRESS recovery and **46 ms** is three
control transfers: `GET_DESCRIPTOR(8)`, `SET_ADDRESS`, `GET_DESCRIPTOR(18)`. The *same* three
transfers against the RMH one tier up cost **2 ms** (L15638 → L15639), and against every device
on controller [1] cost 1-3 ms. 46 ms is more than half of all `resid` in the boot and it is a
single device. Two hypotheses fit it and they want opposite fixes:

* **ours** — `overlay_txn` toggles `USBCMD.ASE` **per stage** and bounded-waits `USBSTS.ASS`
  both ways. EHCI 1.0 §4.8.2 lets the controller defer that transition to a frame boundary, so
  a control transfer can pay up to six ~1 ms handshakes it does not need. Hoisting ASE across
  the three stages would be ours to take.
* **the device's** — it NAKs its way through address assignment, in which case there is nothing
  to trim and hoisting ASE would be a behavioural change to the transport path bought with
  nothing.

Per this ledger's own law an undecomposed constant is not trimmed. **EPACE-TRIM M7** measures
it instead, and the measurement is what decides the next arc.

**EPACE-TRIM M7 — the transport meter, and the two cuts that must not be added.** The EPACE
line grows a second group, in braces:
`… [hubpwr=… hubrst=… hidcfg=… resid=…] {xfer=…(n=…) ass=… act=…}`. `[]` is a **partition** —
disjoint phase classes summing to `enum`. `{}` is an **overlapping** view: a control transfer
runs inside whichever class is open, so `xfer` is a second cut of the same milliseconds and
adding it to the bracket double-counts. The braces exist so that cannot happen by accident.
`xfer` is wall time inside `control()` (every EP0 transfer, `n=` its count, both transports);
`ass` is the two ASS handshakes per overlay stage; `act` is the wait for the overlay token's
Active bit — the device and the wire. On QEMU's chain path `ass`/`act` stay 0 while `xfer`
counts, which is the honest reading of a path that never runs `overlay_txn`, not a dead counter.
`xfer − ass − act` is driver-side setup/teardown.

**EPACE-TRIM M6 — the poll granularity, which M5's own capture convicted on its first boot.**
M5 replaced a blind `settle_ms(50)` with the §11.5.1.5 T_DRST floor plus a 10 ms poll. Every
port then reported the same thing: `PORT_RESET cleared after ~20 ms (T_DRST floor 10 ms + 1 poll
step(s))` — six ports (L15641, L15668, L15671, L15674, L15677, L15680), three boots, **one poll
step every time, never zero and never two**. That is the M5 signature one level down: the true
clear point lies inside (10, 20] ms and the 10 ms grain rounds it up to 20. M6 takes the grain
to 2 ms. It is nearly free because each probe is a hub-addressed `GET_PORT_STATUS` costing
~0.15 ms here — the six no-connection probes for ports 3-7 between L15672 (`t=1794ms`) and
L15674 (`t=1815ms`) fit inside the 1 ms that gap leaves over the 20 ms reset — so five buckets
cost at most ~0.6 ms per port. The wall-clock budget is deliberately identical: 10 + 300 × 2 =
610 ms against M5's 10 + 60 × 10 = 610 ms, same loud exit.

Both M5 thresholds are now stated in **milliseconds** rather than step counts, so the next
change of grain cannot silently move them, and M6 carries its own tripwire:

* the bit never cleared — `EPACE-TRIM M5 TRIPWIRE`, and the line says it is a timeout, not a
  measurement (unchanged).
* `rst_ms >= 50` — `EPACE-TRIM M5 TRIPWIRE`: this hardware wanted the constant M5 replaced.
* `rst_ms >= 20` — **`EPACE-TRIM M6 TRIPWIRE`**: the finer grain still landed at or past the
  20 ms the 10 ms grain reported, so the grain was **not** quantizing and M6 bought nothing on
  this port. A legitimate outcome on healthy hardware, which is exactly why it is a named
  tripwire and not a failure — it can fire, and its absence is the trim paying.
* `10 < rst_ms < 20` — the quiet line, now naming the grain it measured with.

**Coverage, stated plainly: none.** As §8c recorded for M4/M5, the x86 QEMU targets attach
`qemu-xhci` and no EHCI function, so `drivers/ehci` never executes there. M6 and M7 are
metal-only; `./arroyo check` (both arches, plain and `UNAOS_WITNESS=1 UNAOS_LOGTS=1`) is the
whole automated gate and QEMU-green carries zero information about either.

**Prediction for the next metal boot** (before → after, and the lines that decide it):

| reading | before (s73 boots 6-8) | predicted after |
|---|---|---|
| `EPACE: [0] hubrst=` | 20 ms (n=1) | **12-20 ms**, n=1 |
| `EPACE: [1] hubrst=` | 100 ms (n=5) | **60-100 ms**, n=5 |
| `EPACE: [0] enum=` | 285/286 ms | **277-286 ms** |
| `EPACE: [1] enum=` | 576 ms | **536-576 ms** |
| `EPACE: … init=` | 1482 ms | **1435-1484 ms** |
| `BPACE: ehci-hid-done d=` | 1482 ms | same as `init=` ± the EPACE print cost (§8 self-check) |
| `{xfer=…(n=…)}` on [0] | absent | present; `xfer` ≈ **50-60 ms** (the 46 ms anomaly plus a few), `n` ≈ **25-40** (one hub tier, two devices, six root-port probes, one chain-mode HSE attempt) |
| `{xfer=…(n=…)}` on [1] | absent | present; `xfer` ≈ **10-25 ms**, `n` ≈ **60-95** (two hub tiers, six devices, ten port-status walks) |
| `ass=` vs `act=` on [0] | absent | **the verdict.** `ass` ≳ 35 ms indicts our per-stage ASE toggle (next arc hoists it); `act` ≳ 35 ms acquits it and closes the 46 ms as the device's own |

M6's saving is bounded at 0-48 ms and is *deliberately* an unknown inside that band — recovering
the number the 10 ms grain was hiding is the point, and 0 ms (every port reporting the M6
tripwire) is a valid, informative result, not a failure. The upper bound on `init=` is 2 ms
above the 1482 baseline because the two EPACE lines each grew ~40 characters of `{}` group and
those prints are inside the `init` span (§8c measured EPACE's own print cost at ~11 ms).

**Structural falsifiers — the `n=` counts and the arming line, unchanged from §8c.** A trim that
loses a device makes every `ms` reading above look *better*, so these gate the arc regardless of
`init=`:

| reading | required | what a change would mean |
|---|---|---|
| `EPACE: [1] hubrst n=` | **5** | a downstream port stopped being reset — M6 changed the loop |
| `EPACE: [0] hubrst n=` | **1** | same |
| `EPACE: [0] hubpwr n=` / `[1] hubpwr n=` | **1** / **2** | a hub tier vanished from the walk |
| `EPACE: [0] rootrst n=` / `[1] rootrst n=` | **3** / **2** | unchanged by this arc; a move means M4's debounce placement shifted |
| `EPACE: [0] enum n=` / `[1] enum n=` | **1** / **1** | the top-level span itself changed shape |
| `:: EHCI-HID: [1] M2 armed keyboard addr=6 ep=IN3 mps=10 interval=8 (boot protocol) ==` witness `::` | **present, identical** | the internal keyboard is the whole point of this driver; if this line moves or goes, nothing else in the table matters |
| six `PORT_RESET cleared after ~N ms` lines (or their M6-tripwire form) | **six, one per reset port** | a port stopped reporting — the silent-skip class |
| `{xfer=…}` `n=` on [0] vs [1] | **[1] > [0]** | [1] walks two hub tiers and six devices to [0]'s one and two; an inversion means the meter is counting the wrong thing |

**Not trimmed, and why.** `hubpwr` (600 ms), `rootrst` (480 ms), T_RSTRCY (60 ms) and
SET_ADDRESS recovery (16 ms) are **1156 ms of USB 2.0 minima** across §8c and this section. The
46 ms transfer anomaly is undecomposed and therefore untouched by law; M7 is what earns the
right to trim it.

## 9. GPACE — the inside of `pci-usb` (GR13)

Once EPACE took `ehci-hid-done` down, the s60 capture's largest remaining block
was `pci-usb d=4620ms`. Same shape as §8, same remedy: per §7 the instrument for
the inside of a dominating phase is the phase's own witness, so the split lives
in `arch/x86_64/pci.rs` as GPACE, not as more ring stamps.

> **`pci-usb d=4620ms` and its `kepler=4393ms` split are SUSPENDED as magnitudes**
> — see §10. Every armed reading taken between 2026-07-21 and 2026-08-04 spans the
> framebuffer memory-typing regression, and the Kepler probe is where the regression
> is *caused*, so this block contains the fault by construction. The
> instrument's design, its self-check and its baseline law are unaffected; only the
> numbers need re-taking on a `72a4adf1`-or-later build.

Two things about that delta are easy to misread, and GPACE exists to make both
mistakes unavailable.

**`pci-usb` is not a USB block.** The tag is stamped after `pci::init`
*returns*, and `d=` is the delta from the previous stamp — which, because
`xhci.start()` kicks port-1 enumeration before returning, is `enum:p1`. The
window is therefore the `start_next_port` tail, the BENCH-RIDE probes,
`gpu::detect::detect_gpus`, `igpu::init`, `kepler::init`, `sdhc::probe` and the
NIC block. The xHCI bring-up is *upstream* of it and already subdivided by the M4
tags (§6a).

**Most of the 4620 ms is not in a default build.** The s60 stick was armed with
`UNAOS_KEPLER` / `UNAOS_KEPLER_TAKEOVER` / `UNAOS_IVB` / `UNAOS_WC`. The
2026-07-30 metal baseline in §6, with none of them, reads `pci-usb … 113 ms` for
the same tag. A split that did not say **which build it measured** would let a
reader charge a default boot for 4.5 s it never pays, so the second GPACE line
carries a `build=` field naming the compiled knob set, and a knobless build
prints `build=default(no-gpu-knobs)`.

It is also a *deterministic* burn, not a device- or contention-dependent one:
across the four boots in the s60 capture `pci-usb` reads 4576 / 4576 / 4575 /
4620 ms while `gui=` moves 12429 → 8048. A block that stable across a 4.4 s swing
is fixed iteration counts and a fixed volume of MMIO.

**That stability does not exonerate the magnitude.** All four of those boots ran
the same pre-`72a4adf1` build, so all four were handicapped identically; a
constant handicap reproduces as a constant. Stability across repetitions
falsifies *contention* and *device variance* as explanations — which is all this
paragraph ever claimed — and says nothing about a systematic offset shared by
every repetition. §10 is the general form of that distinction.

### The two lines

```
:: GPACE: xtail=<v>ms(n=1) bench=<v>ms(n=0) detect=<v>ms(n=1) igpu=<v>ms(n=1) kepler=<v>ms(n=1) sdhc=<v>ms(n=1) nic=<v>ms(n=1) resid=<v>ms == witness ::
:: GPACE: span=<v>ms anchor=enum:p1 since-entry=<v>ms hz=<v> build=kepler+takeover+fifo+ivb+wc+smc+ == the pci-usb d= split ::
```

(The `build=` string above is the observed s60 knob set, `UNAOS_WC` included.)

A third line appears **only** when the tiling is broken — see "the tripwire"
below:

```
:: GPACE: OVERLAP sum>span by <v>cy — classes are not disjoint ::
```

* `xtail` the `start_next_port` tail — from the `enum:p1` stamp to the end of the
  xHCI block, plus publishing the controller into `XHCI_CONTROLLER` · `bench` the
  knob-gated BENCH-RIDE probes, `n=` counting how many of the three ran · `detect`
  the class-0x03 census · `igpu` / `kepler` one span per device found · `sdhc` the
  read-only SDHC census · `nic` the class-0x02 lookup and whatever followed it.
* `bench` is the only class with more than one call site, so the three probes are
  named **individually** in `build=` (`therm+` / `pcilink+` / `vrom+`) rather than
  rolled into one fragment. Without that, `bench=..ms(n=2)` said two of three ran
  and no reading said which two — the single place the none-vs-zero rule broke.
  (It was also double-counted: `thermprobe` pulls in `smc`, so it already appeared
  in `build=` under a second name.)
* Spans are wall-clock around the call sites, so each class contains everything
  its callee did — MMIO, settles **and** the serial printing of its own witness
  lines.
* **The GPU calls are measured from the outside.** `igpu::init` and `kepler::init`
  are bracketed where `pci.rs` calls them; `gpu/detect.rs`, `gpu/igpu.rs` and
  `gpu/kepler.rs` are not touched at all. Those files belong to other lanes, and
  the split does not need them.

### How the self-check is enforced

`span` is measured from `bootpace::last_stamp()` — the stamp BPACE will itself
subtract when it computes `pci-usb d=`, read out of the ring at the instant the
xHCI block ends — to the report. Anchoring on the *last* stamp rather than on the
literal tag `enum:p1` is what makes `span == pci-usb d=` a property of the
construction instead of a coincidence of this machine's topology; the tag that
actually anchored is printed as `anchor=`, so a reader sees immediately when the
topology changes. Conversion goes through `bootpace::origin_hz()`, the same rate
the ledger divides its own `d=` by — and `gpace_fmt` uses `Dur`'s own expression,
`cy / (hz / 1000)`, rather than EPACE's `cy * 1000 / hz`, so the two instruments
share a renderer and not merely a counter. The practical gap between those two
expressions is tiny — at this machine's `hz=2693848854` the relative difference
is ≈3.2e-7, about 0.0006 ms over a 1845 ms reading, and it can only change a
printed digit when it happens to straddle a floor boundary (~1e-6 per reading).
The change was still right, because identical-by-construction beats
almost-always-equal and it cost nothing; but it removed a remote possibility, not
a standing error.

The remaining slack is these two lines' own print cost, which lands inside
`pci-usb` and outside `span`. **Measured: 1 ms** — `span=26ms` against
`pci-usb d=27ms`, and `since-entry=1845ms` against `pci-usb t=1846ms`. Those are
**the same interval seen from two origins, one observation and not two**:
`d − span` and `t − since-entry` both reduce to *(the `pci-usb` stamp instant) −
(the GPACE report instant)*, so if the print cost is 1 ms both gaps are 1 ms by
construction. They corroborate the arithmetic, not each other.

Provenance: that capture predates the `gpace_fmt` change above, so it was taken
with the `cy * 1000 / hz` renderer. The 1 ms cannot be an artefact of that — the
two expressions differ by ~0.0006 ms at this `hz`, four orders of magnitude too
small — so the number stands for the current build.

`resid = span − Σ(named classes)` **over the printed millisecond values**, and
the classes are disjoint sequential spans inside `span`. The printed domain is
deliberate: each class floors to ms independently, so a residual computed in
cycles and then floored a ninth time leaves the printed row short of the printed
`span` by up to `N_GPACE + 1 = 8` ms. Against the 4620 ms armed block that is
noise; against the ~113 ms default-build baseline of §9a — the row this
document calls load-bearing — it is up to 7%, sitting in exactly the field whose
job is to prove nothing went unattributed. A reader adds up the line, so the line
must add up. `Σfloor(x_k/d) ≤ floor(Σx_k/d) ≤ floor(span/d)` for any integer
`d ≥ 1` makes the subtraction non-negative by construction whenever
`Σx_k ≤ span`; on the `cy` path there is no flooring at all and `resid` is
`span − Σ` outright. A class that under-measures therefore **inflates `resid`** —
the arithmetic still closes, so the lie surfaces as unattributed time instead of
as a tidy-looking total.

That closure holds **in a sound build** — equivalently, whenever the tripwire
below stays silent. The overlap case is the deliberate exception: there the row
does *not* close and `resid=0ms` still prints, which is exactly why that
condition gets a line of its own rather than being left to the subtraction.

**The tripwire.** The cycle-domain comparison survives, but as a conviction
rather than as the printed residual. `Σ > span` in cycles is what overlapping
spans, a wrong anchor, or a non-monotonic `now_cycles()` across cores would
produce — and a `saturating_sub` renders all three as `resid=0ms`, which is also
the *healthy* reading. Clamping a broken instrument into the shape of a working
one is the defect class this whole ledger exists to avoid, so the overlap case
gets its own line instead. One branch, never taken in a sound build.

The clamp on `resid` is acceptable precisely because it can never be silent:
`named > sv` implies `Σx_k > span`, which implies `sum_cy > span`, which is the
tripwire's own condition. Any reading the clamp would have hidden is announced by
the line above it.

Every class prints `<value><unit>(n=<count>)`. `0ms(n=0)` means *this code was
not compiled in or never reached*; `0ms(n=1)` means *it ran and cost nothing*.
The two are structurally distinguishable, which this project has twice paid for
learning.

New read-only accessors on the ledger make this possible: `cycles_of(tag)`,
`last_stamp()` and `origin_hz()`. None records, grows the ring or perturbs
`dropped=` — the ring sits at `n=31` of `CAP=64` under drop-NEWEST, so any growth
would be spent on the late boot tags.

**What GPACE cannot do: attribute a wedge.** Both lines print at the single exit
of `pci::init`, so a hang anywhere inside the window kills the entire report — no
partial split, no "it got as far as `kepler`". GPACE's *silence* is therefore not
evidence about where the boot stopped; the callees' own witness lines are the
only fallback for that. The limitation is inherent to a single-exit accumulator
and is shared with EPACE (§8). BPACE's own both-sides stamping rule (§6a) exists
precisely because ring stamps do not have this property.

### The prerequisite trap this arc removed

The network block ended in a bare `return` on the non-Intel-NIC branch, and this
machine's Broadcom `0x14e4` takes it on **every** boot
(`:: x86_64 PCI: non-Intel NIC (0x14e4) — no e1000 driver, skipping ::`). Any
report placed after it would never have executed on the machine it exists to
measure — an instrument that cannot run in the state it reports on. That block is
now `init_network()`; the early exit returns from the helper, `pci::init` has
exactly one exit, and the report sits on it.

### 9a. Baseline law — the three readings

| build | reading |
|---|---|
| bench media, GPU knobs armed | both lines print; `span` ≈ 4600 ms with `kepler=` dominant; `build=kepler+takeover+…` |
| default `./arroyo esp-x86` | both lines still print, with `detect=0ms(n=0) igpu=0ms(n=0) kepler=0ms(n=0)`, `build=default(no-gpu-knobs)`, `span=6ms` (MEASURED, s73 boot B 2026-08-06 — the ≈113 ms this table used to predict was a stale code-comment figure; the first metal default boot came in 19× under it) |
| `UNAOS_SKIP_XHCI=1` | the lines are **absent entirely** — `pci::init` is never called |

The middle reading is the load-bearing one: it is what proves the numbers report
the GPU path rather than the reporter's own liveness. A build with the GPU code
compiled out that still printed a large `kepler=` would convict the instrument.
The third matches BPACE's own "did not run (b)" asymmetry (§5).

**Provenance of the ≈113 ms, and what it is not.** This document previously wrote
that row as "≈ 100 ms" while §6 recorded `pci-usb … 113 ms` for the same tag on
the same machine. There is one number, not two: 113 ms, from the 2026-07-30
default-build metal ledger in §6. The row now carries it.

But it is a **BPACE** number. No metal capture has ever printed
`build=default(no-gpu-knobs)` — `awk '/default.no-gpu-knobs/'` over
`/home/pmes/unaos-bench/capture/` returns nothing, because §6 predates GPACE and
every armed boot since has carried the knobs. So the middle row is a *derived
expectation*: GPACE's `span` must equal BPACE's `pci-usb d=` by construction
(see "How the self-check is enforced"), and the one measured `pci-usb d=` on a
knobless build is 113 ms. Calling it a "reading" overstated it. The first metal
default-build GPACE line is still owed, and when it arrives it is the check on
this row — not the other way round.

This row is unaffected by §10: a default build never maps Kepler BAR1 and never
lost WC.

Read the lines with `awk '/GPACE/'` — **not** `grep`.

## 10. The framebuffer write-combining regression (2026-07-21 → 2026-08-04)

Every armed metal figure this document records between those dates was taken on a
machine whose panel was running strong-uncacheable instead of write-combining.
This section says which of them survive, which are inflated, and which must be
re-taken — because "the instrument was honest" and "the number means what we
thought" are different claims, and the regression separates them.

`set_framebuffer_wc` retypes the framebuffer's huge-page leaves to PAT index 4
(WC). The Kepler probe later maps BAR1 — `0x90000000` + 256 MiB, which *contains*
the framebuffer — and `map_mmio_window` ORed `PCD|PWT` onto each leaf without
clearing the PAT bit, turning index 4 into index 7: strong UC, which
`ensure_pat_wc` never reprograms. `FB_WC_DONE` latches the retype, so nothing put
it back. Fixed in `72a4adf1` (`arch/x86_64/memory.rs`), confirmed on metal.

### 10a. Scope — three independent narrowings

The blast radius is much smaller than "every figure since 2026-07-21", in three
ways that compound:

1. **Only `nvidia-kepler`-armed builds.** The clobber is reached only through
   `crate::drivers::gpu::kepler::init(gpu)` at `arch/x86_64/pci.rs:620`, inside a
   `#[cfg(feature = "nvidia-kepler")]` match arm. A default `./arroyo esp-x86`
   build never maps BAR1 and never lost WC — so §6, §9a's middle row, and every
   default-build figure are untouched.
2. **Only x86.** The faulty function is `arch::x86_64::memory::map_mmio_window`.
   aarch64 has no counterpart in that role: its same-named function lives in
   `arch/aarch64/mmu_tegra.rs`, is called only from the Tegra RTL8168 path
   (`rtl8168_tegra.rs`), and never sees a framebuffer. Every Pi, Jetson and QEMU
   figure is untouched — not because an arm returned early, but because the code
   is not shared at all.
3. **Only the part of a boot after the Kepler probe.** The retype happens at
   fbcon init; the clobber happens inside `kepler::init`, three lines after
   `[NVIDIA] Initializing Kepler GPU`. In the one capture that recorded both
   (`rmbp-gr15-s70/ttyUSB0.log`, third boot) they are 541 log lines apart —
   `x86 fb-wc: retyped 15 leaf(s) WC (PAT PA4)` at line 3122, the BAR1
   `x86 mmio-map: 0x90000000..0xa0000000` at line 3663. In wall-clock, on the
   pre-fix boot below: BPACE `enum:p1 t=3378ms`, plus GPACE `detect=5ms igpu=1ms`,
   puts the clobber at **≈3.38 s**. Everything a ledger stamps before that ran at
   full WC speed and is unaffected.

### 10b. The controlled pre/post pair

`/home/pmes/unaos-bench/capture/rmbp-gr15-s70/ttyUSB0.log` holds **three** boots,
not two. Boundaries — each landmark appears three times, in step:

| landmark | boot 1 | boot 2 | boot 3 |
|---|---|---|---|
| `SERWIT-1: 6 cores` | 56 | 1608 | 3366 |
| `Enumerating Port 1` | 280 | 1832 | 3590 |
| GPACE `since-entry` | 788 | 2337 | 4097 |
| `FB Init` | 789 | 2338 | 4098 |

Boot 1 ends at line 1543 and boot 2 begins at 1544. Boot 1 is the pre-fix boot:
it prints no `x86 mmio-map` line at all, because `72a4adf1` added that line.
Boots 2 and 3 print it with `wc-kept=15` over BAR1 — the fix taking effect.

Two traps in this file:

- **Line 1905 is not the boundary.** It is a `mmio-map` line 361 lines *inside*
  boot 2. Treating it as the split misattributes an entire bring-up.
- **Only boot 3 captured its own head.** `FTDI-CAP … early-boot capture INTACT`,
  the `fb-wc: retyped` line and `X86_64 Memory Init` each appear exactly once, at
  3121/3122/3128. Boots 1 and 2 begin mid-stream. This does not weaken the
  comparison — every mark compared below comes from the `BOOTLOG` replay block,
  which is printed late — but it is why the retype→clobber line distance in §10a
  is measurable in boot 3 only.

The `BOOTLOG` replay, boot 1 → boot 2:

| mark | pre-fix | post-fix | Δ |
|---|---|---|---|
| `ehci:kbd-armed` | 2813 ms | 3001 ms | +188 |
| `portsw:flip` | 2922 ms | 3110 ms | +188 |
| `gui:handoff` | 28701 ms | 21593 ms | **−7108** |
| `ftdi:console-up` | 28869 ms | 21761 ms | −7108 |
| `block:up` | 35190 ms | 28078 ms | −7112 |

Everything upstream of `gui:handoff` is unchanged or slightly *slower* (the +188 ms
is common to both early marks and is unrelated boot-to-boot variance, not a
regression effect — it is upstream of the clobber). The two intervals that bracket
the saving are flat: `gui:handoff`→`console-up` is 168 ms in **both** boots, and
`console-up`→`block:up` is 6321 vs 6317 ms. The entire 7108 ms therefore lands
inside the GUI/Kepler phase and nowhere else.

GPACE agrees from an independent origin: `kepler=25315ms` → `18169ms`
(boot 3: `17079ms`), a 7146 ms move that matches the BOOTLOG 7108 ms to 38 ms.
The work is identical — 221 Kepler log lines in each boot, spanning ~420 lines of
output in each.

**Mechanism.** The console is mirrored into a 1314×750 panel window, so each
mirrored line costs a present. Same window, same byte count, from `[wc-h]`:

| | pre-fix | post-fix |
|---|---|---|
| `present_us`, win=1 full frame (3 942 000 B) | 24269 | 2660 / 2789 |
| `present_us`, win=1, all sizes | 23815 – 24269 | 233 – 2789 |
| `compose_us`, same 3.9 MB | 2279 / 2265 | 2247 / 2266 / 2316 |

`compose_us` is the control: the identical 3.9 MB composited into **cached RAM**
does not move. That is what confines the handicap to framebuffer writes and rules
out a general slowdown. Console geometry is byte-identical across both boots
(`box=1314x750` throughout), so `PANEL_SCALE` is not a confound.

The arithmetic is consistent but not exact, and should not be stated as exact:
7108 ms at ~21.6 ms saved per full-window present is ~329 presents, against 221
instrumented Kepler lines inside a ~420-line span. Per-line present counts are not
established by this capture — some presents are banded (`span=736`) and cheaper.
What the capture does establish is the *class* of cost and its magnitude.

### 10c. Claim classification

| claim | verdict |
|---|---|
| EHCI-HID `init=` 6324 → 2010 ms (§8a/§8b) | **survives.** Both readings are stamped at ~3.0 s, before the clobber. The mechanism — deleting a hard-coded 2000 ms PSS wait — is a constant, independent of write rate. |
| `xhci-settle` 500 → 150 → 100 ms, and `xhci-settle d=7289ms` (§6/§6a) | **survives.** Both are stamped at ~3.3 s and earlier; entirely pre-clobber. |
| desktop 12.4 → 8.0 s | **delta survives, absolute inflated.** The 4.4 s move is 98.5% the EHCI saving, which is pre-clobber. But `total gui=8048ms` is a post-clobber endpoint: ~4890 ms of it is the handicap. Quote the delta, not the absolute. |
| `pci-usb d=4620ms` / `kepler=4393ms` — "the 4.6 s block" (§9) | **must be re-measured.** The window spans the clobber and contains the probe that causes it. Four-boot stability does not help (§9). |
| §6 metal baseline, §9a default row | **unaffected** — default build, §10a(1). |
| any aarch64 / Pi / Jetson / QEMU figure | **unaffected** — §10a(2). |

### 10d. How fast is it, honestly

The speedup is not one number, and "9×" unqualified is wrong in every context
except one:

- **8.7 – 9.1×** for large framebuffer writes — the present of a 3.9 MB frame,
  24269 → 2660 / 2789 µs. This is the only place 9× belongs.
- **5.2×** end-to-end for the console composite, which is compose + present:
  2279 + 24269 = 26548 µs → 2316 + 2789 = 5105 µs. Compose is now **45%** of the
  cost, so further work on the present buys progressively less.
- **1.33×** to `gui:handoff` (28701 → 21593 ms) and to `ftdi:console-up`;
  **1.25×** to `block:up` (35190 → 28078 ms), because the post-handoff storage
  chain never paid the handicap and so gains nothing.

Below ~20 KB the ratio is unreliable: the smallest presents land at 116–130 µs
against 13–22 µs of timer granularity, and the ratio there is dominated by
quantisation, not by memory type.

### 10e. Cycles → milliseconds: use 2.6938 GHz, not 2.3 GHz

Commit `d5d4684f` describes this board as "2.3 GHz Ivy Bridge base"; `03df9398`,
`4d4919be` and `72a4adf1` all work in the measured TSC rate, ~2.6938 GHz. They
disagree by 17%, which is enough to move a converted figure out of agreement with
the ledger it is being checked against.

**For any BPACE / EPACE / GPACE conversion, the correct rate is the measured TSC
rate**, and the capture prints it: `hz=2693847134` … `2693851785` across every
boot in `rmbp-gr12` and `rmbp-gr15-s70`. This is not a preference — it is the
same `origin_hz()` the three instruments divide their own cycle counts by (§8,
"Instrument-baseline"; §9, "How the self-check is enforced"), so converting with
anything else breaks the property that makes their cross-checks meaningful. The
2.3 GHz figure is the part's nominal base clock and is not what the TSC ticks at
on this board; do not use it for instrument arithmetic.

Read this capture with `awk '/pattern/'` — **not** `grep`.

### 10f. Re-derivation: the SCHED-X86 render-placement argument under the true write rate

Queue item from §10c. The regression did not only inflate boot figures; it fed a
*design* argument. Several placement and presentation decisions were justified by
"a present costs most of a frame on this panel", and that cost was a measurement
of the UC clobber, not of the panel. This subsection redoes the arithmetic and
says what the placement decision would look like if it were being made today. It
is analysis for a decision, not the decision — nothing here changes code, and the
one measurement that would settle it has not been taken.

#### The premise, and where it actually lives

There is no single document that states "render is placed here because a
full-screen present costs ~50 ms". The premise is distributed across four code
comments, each carrying a different number, and no two of them are reconcilable
with each other:

| site | figure as written | classification |
|---|---|---|
| `arch/x86_64/memory.rs:1047` | "flat ~162 MB/s, size-invariant … 7.6 -> 53.8 fps with WC live" | **measured** (metal, `rmbp-gr15-s70`) |
| `arch/x86_64/sched.rs:2383` (`Channel::try_recv`, the DRAIN doc) | "A 2880x1800 present is ~50 ms" | **inferred**, origin not recorded |
| `pal.rs:195-198` (CURSOR-X86) | `flush` = 69–86% of the frame budget at 10.9 MB/frame; "measured write bandwidth … ~150 MB/s" from `[wc-h] win=2 bytes=1620080 present_us=9858` | **measured** rate, quoted 9% low: 1 620 080 / 9858 µs = **164.3 MB/s** |
| `video/fbcon.rs:967` (FBCON-DMG) | "the 24 ms measured in `[wc-h] win=1`" | **measured** — this is the 1314×750 console window, 3 942 000 B |

All four are the same reading of the same defect. 3 942 000 B / 24 269 µs =
**162.4 MB/s**; the `win=2` sample gives 164.3 MB/s at a byte count 2.4× smaller,
which is exactly the size-invariance `memory.rs` names. The rate is real and the
instruments were honest. What none of them knew is that it was the memory type
talking.

#### The corrected arithmetic

Post-fix, same window, same byte count (§10b): 3 942 000 B / 2660 µs =
**1482 MB/s**; / 2789 µs = **1413 MB/s**. Take **1.41 – 1.48 GB/s** as the WC
write rate and re-cost every present the premise depended on:

| payload | bytes | at 162.4 MB/s (UC) | at 1.41 – 1.48 GB/s (WC) |
|---|---|---|---|
| console window, `box=1314x750` | 3 942 000 | 24.3 ms *(measured)* | 2.66 / 2.79 ms *(measured)* |
| `[wc-h] win=2` sample | 1 620 080 | 9.9 ms *(measured)* | ~1.1 ms *(inferred)* |
| `[vugfps]` damage-limited frame | 10 900 000 | 67 ms *(inferred)* | **7.4 – 7.7 ms** *(inferred)* |
| **full panel**, 2880×1800×4 | 20 736 000 | 128 ms *(inferred)* | **14.0 – 14.7 ms** *(inferred)* |

Two things fall out immediately.

**The `sched.rs` "~50 ms" was never a full-screen present.** At 162.4 MB/s, 50 ms
buys 8.1 MB — 39% of a 2880×1800 panel. A genuine full-panel present cost **128
ms** under UC, not 50. The 50 ms figure is consistent with a damage-limited
present of roughly `[vugfps]` size, and the comment's own corroborating figures
agree with that reading (`memory.rs`'s 7.6 fps = 132 ms/frame is the full-panel
number; `rast_demo.rs`'s "~22 fps" = 45 ms is the damage-limited one). Whichever
payload the 50 ms described, dividing by the measured ratio gives **~5.6 ms**.

**A full-panel present is now approximately one frame, not eight to ten.** 14.0 –
14.7 ms sits inside a 16.7 ms (60 Hz) budget and marginally *outside* a 13.3 ms
(75 Hz) one. The panel's actual refresh rate is not established anywhere in this
tree — do not assume 75 Hz here; the honest statement is "one frame, ±10%",
which is a materially different regime from "most of a frame" and a completely
different regime from "eight frames".

#### What the premise was actually holding up, item by item

Read from code. Four decisions cite present cost; only one of them depends on it.

1. **Rule 1 — the pump and the render task must be on different cores**
   (`main.rs:1216-1222`, `scheduler.md` §SCHED-X86). Rests on `XHCI_CONTROLLER`
   being a raw `spin::Mutex` with two preemptible takers, which hard-deadlocks on
   one core. **Bandwidth-independent. Unchanged. Do not relax it on the strength
   of this section** — the deadlock is a liveness property and 9× more headroom
   does not touch it.
2. **Rule 2 — never place a COOPERATIVE (IF=0) ring-3 task on core 0.** Rests on
   core 0 being the sole advancer of `arch::ms()`. **Unchanged.** Its neighbour —
   never place a cooperative ring-3 task on the *render* core — is also unchanged:
   a task that owns its core until it syscalls stalls the panel for an unbounded
   time, and unbounded does not shrink by 9×.
3. **The PAT/WC paragraph in `scheduler.md`** ("pinning render to an AP would have
   made that AP's fb PTE select PA4=WB … effective-UC"). Already resolved by
   `smp::ap_entry` calling `ensure_pat_wc()` on **every** AP, and the doc already
   says the placement decision is therefore not load-bearing. §10's clobber was a
   *different* mechanism (a leaf retype, not a per-core MSR) and does not revive
   this argument. **Unchanged.**
4. **`smp::worker_cpu`'s blanket exclusion of the render core** (`smp.rs:55-88`).
   This is the one. It has two justifications welded together: cooperative ring-3
   tasks stall the panel (item 2, still sound and sufficient on its own), *and*
   the render core has no slack to share (present cost — **now 9× weaker**).

And the open question `scheduler.md` records but does not answer:
`syscall::bg_place_cpu` deliberately places `bg`/`run` programs on the caller's
core, which since SCHED-X86 is the render core, "so a foreground `run` degrades
the panel for its duration".

#### What the decision looks like under the true rate

Inferred throughout — this is the argument, not a result.

The render core's duty cycle is what the premise was really about. HID reports
arrive at 125 Hz (8 ms apart; `video/cursor.rs:1745`). Under UC, an isolated
damage-limited present at ~50 ms against an 8 ms report interval means the render
task cannot keep up by roughly 6×: the DRAIN in `x86_render_service` was not an
optimisation but the only thing standing between the desktop and an unbounded
`GUI_CHANNEL_X86` backlog. Under WC the same present is ~5.6 ms — *inside* the
report interval. Even the full-panel worst case, 14 ms, is under two report
intervals. The render task stops being a permanently saturated core and becomes a
bursty one.

That reverses the sign of the argument in item 4, and only item 4:

- **Keep the DRAIN.** Its worst case is a 64-slot channel drained to one present
  instead of 64; the amplification is unchanged by the write rate, only its
  urgency drops. A present-per-event build would now cost 64 × 5.6 ms rather than
  64 × 50 ms, which is still catastrophic. This is cheap insurance and there is
  no argument for removing it.
- **Split item 4's two justifications.** Cooperative ring-3 must still never land
  on the render core (liveness). *Preemptible* work has only the slack argument
  behind it, and the slack argument is now weak. The concrete consequence is the
  `PARTIAL` verdict: `PLACE-CHECK` reports `PARTIAL` when `pool < 3`, and on the
  QEMU default `-smp 6` the pool meets the requirement with **zero slack**, so one
  AP failing INIT-SIPI-SIPI silently skips U7x/SOCK-4/U6gx — U6gx being the only
  automated exercise of the STOR-1 S5 mitigation. Admitting preemptible non-xHCI
  work onto the render core as a *fallback when `pool < 3`* converts a skipped
  fixture into a slightly jittery panel. On the corrected arithmetic that is the
  better trade; on the old arithmetic it was not.
- **`bg_place_cpu` can probably close as "correct as written".** `run`/`bg` use
  `spawn_user_preemptible` (IF=1), so the co-located program is preempted normally
  and the interference is bounded by the timeslice, not by the program. The panel
  degradation it was flagged for is now ~5.6 ms of stolen present per contended
  slice rather than ~50. Closing it means accepting that a foreground `run` makes
  the desktop *slightly* choppy instead of unusable — which is ordinary
  single-core-desktop behaviour, not a defect. The condition on that closure is
  that nothing routes a *cooperative* spawn through it.
- **The `exclusive` tier stops being the only acceptable tier.** `tier=exclusive
  pool=5` on the 8-core metal box is unaffected either way; what changes is that
  `tier`s below it are no longer evidence of a broken desktop.

None of this touches rules 1 and 2, and none of it is a reason to un-pin render or
to fold it back onto the BSP: the reason render is a scheduled task on its own
core is that the BSP was never popping a run queue (2 BGRUN spawns, zero
`SYS_WIN_CREATE`, `c0:0/0` beside `0/263`), which has nothing to do with
bandwidth.

#### The measurement that would confirm or refute it

**Everything above rests on one unmeasured extrapolation**: that the 1.41 –
1.48 GB/s measured at 3 942 000 B still holds at 20 736 000 B. UC was
size-invariant, so extrapolating from it was safe; WC is combining-buffer
behaviour and there is no evidence in the record that it is. Every post-fix
`present_us` in `rmbp-gr15-s70` is the 1314×750 console window on the FBCON-DMG
banded path during boot. **There is not one WC-live full-panel present in the
capture record.** Take that first.

Prerequisites for any of these to mean anything: `UNAOS_WC=1` plus the Kepler
knobs (the compositor's only ignition on x86 is the Kepler takeover), `wc` in the
`⚡ kernel features:` banner, and — the one that makes the run post-fix rather
than a repeat of the premise — the boot must print `x86 mmio-map:
0x90000000..0xa0000000` with `wc-kept=15`. Without that line the panel is UC
again and every number below reproduces the old argument. Read with
`awk '/pattern/'`, not `grep`.

| # | witness line | source | confirms | refutes |
|---|---|---|---|---|
| 1 | `[wc-h] win=… box=…x… span=… band=… bytes=… compose_us=… present_us=… rectscan_us=… torn=… -> BUFFERED` | `video/wcg.rs:788` | a row with `bytes` at/near 20 736 000 and `present_us` ≈ 14 000 – 14 700 (i.e. `bytes`/`present_us` still 1.4 – 1.5 GB/s) | `present_us` ≫ 25 000 at that byte count — WC does not hold at full-panel size, the extrapolation fails, and item 4's slack argument survives |
| 2 | `[wc-h] rollup … maxpresent_us=… frame_us=… ` | `video/wcg.rs:922` | `maxpresent_us` is the single number the placement argument needs — the **worst** present in the window, not the mean. Confirms if it stays under ~15 000 | a `maxpresent_us` in the tens of thousands means the render core still has a stall long enough to matter, whatever the average says |
| 3 | `[schedx86] depth sent=… recv=… inflight=… (render core N)` | `main.rs:4166` | `inflight` at 0–1 through a sustained trackpad sweep plus a console scroll: the render task is keeping up, and "the render core is saturated" is dead | `inflight` climbing toward the 64-slot cap — the backlog the DRAIN exists to prevent is still forming, and no placement should be relaxed |
| 4 | `:: [vugfps4] drain=…us draw=…us flush=…us tail=…us sum=…us (=1e6/fps)` and `:: [vugfps] …fps …bytes/frame flushed…` | `vug.rs:966` / `vug.rs:943` | `flush` falling to well under 30% of `sum`. `pal.rs`'s "69–86% of the frame budget" is a UC-era figure and inverts if this holds — the frame stops being flush-bound | `flush` still the majority of `sum` — the present is still the frame and CURSOR-X86's premise stands unmodified |
| 5 | `:: SCHED-X86 PLACE: aps=… render=c… svc=c… worker=[…] xhci=[…] tier=… pool=… ::` and `:: SCHED-X86 PLACE-CHECK: actual=c… arg=c… published=c… pool=… collide=… tier=… verdict=… ::` | `smp.rs:250` / `smp.rs:320` | not evidence for or against the arithmetic — this is the **state any change would move**, recorded so a before/after exists. Today's metal reading is `tier=exclusive pool=5 verdict=PASS` | `verdict=FAIL` at any point stops the whole enquiry: the renderer is not where it was published, and no timing read from that boot describes the core it names |

Witness 1 is the load-bearing one and it is cheap: it needs a single full-panel
present on a WC-live boot. Until it exists, item 4 and `bg_place_cpu` should be
recorded as *arguments whose premise has been withdrawn*, not as decisions
reversed — a withdrawn premise leaves a decision unjustified, which is not the
same as leaving it wrong.

### 10g. The 17-second kepler block, solved: it was the witness battery (s73, 2026-08-06)

GR15 closed with `kepler=17138ms` of a 27.9 s boot as the one number left with
nothing inside it. The s73 sitting opened it with four boots on metal, all at
kernel `d86eadf8`:

| boot | build | `kepler=` | `gui=` | resolution |
|---|---|---|---|---|
| A | kepler+wc+**witness**+logts | (civil-degraded) | ~50 s wall | 1 s — see below |
| C | kepler+wc+logts (no witness) | **1402ms** | **4094ms** | 1 ms |
| A2 | identical repeat of C | **1401ms** | 4102ms | 1 ms |
| B | default(no-gpu-knobs)+logts | 0ms(n=0) | 3628ms | 1 ms |

**The real Kepler bring-up is 1.40 s, deterministic to 1 ms across two boots.**
The other ~15.7 s of the "kepler block" was the witness battery running inside
the measured span — dominated by the `[wc-g]` glass-verify passes (four in boot
A, ~3 s each, timestamped 15:30:51/54/57/31:00 in the capture) plus the rest of
the boot-fixture ladder. Every witness-armed `kepler=` figure ever recorded
carries that overhead inside it; the boots differ ONLY in the `witness` feature.

Corollaries, each earned on s73:

* **Baseline law closed (§9a).** Boot B printed `build=default(no-gpu-knobs)`
  with `kepler=0ms(n=0)` for the first time in any capture — beside boot C's
  `kepler=1402ms(n=1)` the zeros are proven to mean "path absent," not
  "reporter dead." Measured `span=6ms` (the table's old ≈113 ms prediction was
  a stale comment figure).
* **Boot-to-boot variance is ~1 ms at the kepler class and ~10 ms at `gui=`**
  (C vs A2) — the §10 queue's "n=1 per arm is uncharacterised" concern is
  retired for witness-off arms. Witness-ON variance remains uncharacterised.
* **Why boot A read in whole seconds:** the witness `sntp-x86` fixture's
  set-clock leg planted its canned `2026-07-22T15:30:45Z` anchor ~1 s into
  boot, flipping every later `logts` prefix to the 1-second civil form (and
  dating every FAT write July 22). Fixed the same day: the fixture now clears
  the anchor it planted (`clock::witness_clear_anchor`, fixture-only) and says
  so on the wire — `:: [sntp-x86] canned anchor cleared ::` is the expected
  line in every future witness capture.
* **The next optimization target is the instrument, not the boot.** A 4.1 s
  boot pays ~46 s of witness wall-clock when armed. The witness battery needs a
  pay-as-you-go shape (budgeted passes, or off the boot path) before any
  witness-armed timing figure is quoted as a boot cost again.

**§10g addendum (boot A-ms, same day):** the witness-armed boot re-flown with the fixed fixture,
full ms resolution end to end. `kepler=17129ms` (matches 17138/boot A — witness-on cost is itself
reproducible), and the block decomposes exactly: four `[wc-g]` glass-verify passes at
2873/2876/2861/2878 ms (11.49 s), `[wc-d]` verifies 2321+497 ms (2.82 s), the `kdisp: fb-draw hold`
loop 1.12 s (whose per-"second" tick measures 225 ms — its delay constant is ~4.4× fast; kepler
lane, reported), and ~1.4 s of real bring-up — 16.8 of the 17.3 s span accounted, the remainder
sub-100 ms lines. *Corrected (GR17, `serial-analyzer --wcg`, same capture):* the "remainder" is not
noise — the same 229 kepler/kdisp lines that cost 159 ms total (0.69 ms/line) on the witness-off
boots cost 1438 ms (6.28 ms/line) on the armed boot, a distributed per-print tax that begins at
`[wc-x] console-route first-paint` and is invisible to any top-gaps view because no single gap is
large. Armed-boot bring-up is therefore ~1.35 s real + ~1.29 s print tax, and the tax mechanism
(witness-build serial synchrony vs console-window present cost) is unattributed — it is the third
instrument-cost term beside wc-g (11.5 s) and wc-d (2.8 s), and it caps what any wc-g-only reshape
can recover. Fixture fix verified on the same wire: the canned anchor lives for exactly one
line (`[15:30:45Z] … sets clock => PASS` → `[519ms] canned anchor cleared`). The wc-g pass is the
single redesign target: ~2.87 s per pass, four passes, every witness-armed boot.

## §10h — GR17: the pass decomposed on the wire, and every term attacked (2026-08-06)

**Boot P (witness+logts, `370fa1e0`, kernel `e717e4c4…`) profiled the pass per phase and
confirmed the GR17 cost model to a fraction of a percent.** Per win=1 pass, measured vs
predicted: `readback_us=2832894–2837864` vs 2 834 000; `cks_blit_us/civac_us/cks_after_us`
each 5 738–5 744 µs vs 5 700; `surf_bytes=3862528` and `probes=965632` exact; per-probe
2.939 µs vs 2.935. **The glass read-back is 98.7 % of the pass**, and its cost is the latency
of single-byte volatile reads from the WC-mapped Kepler BAR over PCIe: ~976 ns/byte, three
byte reads per probed pixel. `kepler=17077ms(n=1)` — consistent with 17129/17138 (the prof
lines' own serial is ~13 ms/pass, stated cost). Analyzer: `serial-analyzer --wcg`, boot 6.

**The §10g per-print tax is convicted and fixed.** It was `wcg::on_present`'s budget gate: a
global `any()` over eight window slots, of which this bench occupies three — five never-spendable
slots held the gate permanently open, so every routed console line paid a full-surface 3.86 MB
byte-at-a-time FNV. Arithmetic closes to 2.6 %: 4 cycles/byte × 3 862 528 B ÷ 2.693 GHz =
5.74 ms/line predicted vs 5.59 ms/line measured (the residual 0.69 ms is the banded compose both
builds pay). Onset at `console-route first-paint` because that is the first present a routed line
reaches. The UART-drain hypothesis was falsified from the capture (23 220 B emitted within one
civil second — a synchronous 115200 link caps at 11 520 B/s). Fix: the budget closes per window
(`f7421a20`); no witness weakened — `begin` is the checksum's only reader and already refuses a
spent id.

**Every term of the 17.1 s witness-armed kepler block now has a landed fix** (all metal-pending
until Boot Q):

| term | was | fix | predicted |
|---|---|---|---|
| wc-g read-back ×4 | 11.5 s | bulk u64 glass reads (1 transaction/probe, not 3) + paygo battery: lattice pass 1 (`coverage=lattice16` on the wire), full passes 2–4 deferred past 15 s since entry | ~0.06 s in-window |
| wc-d verify | 2.84 s | `read_pixel` widened to one aligned volatile u32 (`3c05856d`), same probes, same decisions | ~0.95 s |
| per-print tax | ~1.29 s | per-window `on_present` budget (`f7421a20`) | ~0 |
| real bring-up | 1.35 s | untouched | 1.35 s |

**Boot Q flew the same day (witness+logts+`UNAOS_WCG_PAYGO`, `f7421a20`, kernel `aee612a7…`)
and the predictions held: `kepler=2564ms` (predicted 2.3–2.6 s), from 17 077 ms — 6.7×.**
Whole boot `gui=6242ms` witness-armed, from 20 727. Per term (analyzer boot 7): wc-g in-window
128 ms (the 60 ms prediction was 2× optimistic — the lattice pass costs 126 ms with its
checksums and serial, stated rather than absorbed); wc-d 1010 ms (predicted ~950 — the
`read_pixel` widening measured 2.8×); bring-up 1360 ms vs the witness-off 1352 ms — **the
per-print tax is gone to within 8 ms**. The wire behaved exactly as designed: pass 1 per
window `coverage=lattice16` (`fbbad=0/60352` for the console window — 1/16 of 965 632, said
out loud), deferral opened at `since_entry_ms=15453` `clock=entry`, `deferred=` ran as a
census (`emit=2 deferred=264` on the console window mid-wait), win=3 completed its battery
`-> PAID`, and every verdict stayed CLEAN — no COHER/RACE/BLIT anywhere in the boot.

The remaining terms above a ~2 s `kepler=`: wc-d's full verify (1.01 s; its own lattice/defer
treatment in wm.rs is the next lever) and 1.36 s of real bring-up. Witness in-window cost is
now ~1.15 s against 15.7 s before GR17 — 13.7× — with full coverage still arriving via the
deferred passes. Deferral honesty is spec-shaped: a lattice line carries `coverage=`,
`deferred=` is a running census (`emit=` ordinal, `since_entry_ms=` / `clock=unarmed` guard),
and no x86 spec yet reads any `[wc-g]` line — the pi4 gate is the only automated reader, a
coverage gap recorded here deliberately.

**§10h addendum — Boot R (kernel `7f488c31…`, `1b08332d`, same day): the whole sweep measured.**
`gui=3767ms` (from 20 727 this morning — 5.5×; prediction band was 4.2–4.7 s, beaten).
Per prediction: `sched d=155ms` (predicted 155–160, and now a constant — the CLOCK-X1 verdict
fired from core 7 at `deferred 3451 ms TSC / 3438 ms APIC — CONSISTENT`, 13 ms of honest skew);
`ehci-hid-done d=1482ms` (predicted 1400–1470 — 12 ms over the band top; the two new loud-skip
"not walked" lines' serial cost was not in the model, noted); `hcrst=47/24 ms` exact,
`rootrst n=3/n=2` exactly the moved debounce's +1, `hubrst=100ms(n=5)` in-band, `M2 armed
keyboard addr=6 ep=IN3` present and identical; `kepler=1521ms` (wc-d's in-window verify now
lattice — both batteries complete post-boot: wc-g and wc-d `-> PAID` for every presenting
window); console-pace census `ran=7 held=262 busy=0 idle=1` — 262 line-merges retired by 7
presents. Zero FAIL, zero TRIPWIRE, zero exceptions. Two live findings the boot handed back:
the resurrected SMC FIRST FAILURE fired on a REAL wedge on its first outing (`B0AV kind stuck
step 0`, status timeline 0x48 — a fault the AC-W false alarm used to bury), and KBDWIT reports
the known-intermittent s58 keyboard silence (`NO-COMPLETIONS quiet=9017ms` — armed, never
completing; standing issue, predates this sweep).

**Correction (2026-08-06) — that second finding is not a finding.** The KBDWIT line above was read
as an s58 recurrence. It is not one, and the same capture refutes it. In that same Boot R, on that
same arming, with no re-arm and no `STOP-NOTE` between them, the keyboard delivered its first key at
`[215418ms] EHCI-HID: KEY: 'l'` and 96 reports by `[276587ms]`. The dump had fired at `[11125ms]`.

The corpus generalizes it. Across every capture carrying this witness — `rmbp-gr13`,
`rmbp-s62-probe`, `rmbp-s66-cand444`, `rmbp-gr15-s70`, `rmbp-gr16-s73`, 23 boots — **every** kbd dump
reads `reports=0`, and ten of those boots typed fine afterwards. The deadline is `KBDWIT_QUIET_MS`
(4 s) after arming, i.e. 4.7–26 s into a boot; nobody types that fast. A boot-protocol keyboard NAKs
indefinitely until a key is pressed, so `reports=0` at that deadline was measuring the absence of a
typist and printing it in the grammar of a fault. The instrument's own section comment predicted
this exactly ("IT CANNOT SEPARATE, at this deadline, an idle keyboard from the s58 recurrence") and
the prediction came true against its own author's boot report.

Boot R2 (the next boot in the same capture, dump at `[11156ms]`) is the control: it ran to
`159290 ms` with **zero** `EHCI-HID: KEY` lines and the same `reports=0`. Nobody typed; nothing was
reported.

The registers were never in doubt in either boot, and none of them convict anything:
`usbsts=0x00006000 hch=0 hse=0 pss=1 rs=1 pse=1`, `tok active=1 halted=0 xact=0 babble=0 dbuf=0
missed=0 cerr=3 rem=10`, `adv=0x1a33`/`0x1b3b`. `cerr=3` un-burned means no transaction ever failed.
Two readings that did look like findings are answered in the driver's KBDWIT section comment:
`qtd_tok=0x00000000 qtd_driven=0` is the instrument declaring the standalone qTD **out of the
transfer** on the overlay-direct path, not a missing write-back; and `horiz=0x00000001` on IN3 is the
end of an intact chain (`fl0` → IN1 → IN3 → terminate), not a broken link.

**Disposition: the instrument was fixed, the driver was not touched.** KBDWIT-2 adds `sched=` /
`polls=` / `walks=` / `split_or=` to line 1 and a latched `SILENCE-BROKE` line at the first
completion after a dump. `walks` counts service passes on which the controller's own split-progress
words (`overlay[4]`/`overlay[5]` — C-prog-mask and FrameTag/S-bytes, EHCI 1.0 §3.5.4, words this
driver never writes) moved, which answers "is the host side transacting against this QH?" **without
a keypress**. Boot R and R2 already carry that answer in a single sample — `ovl5=0x00000017` vs
`0x00000018`, a different FrameTag at the same QH position — so both boots were `WALKED` all along;
`sched=` only makes the driver say so instead of leaving it to a reader with the spec open. Bounded
at one extra line per endpoint per boot; read-only; no transfer-path write.

s58 itself remains **open and unconvicted** — no capture in this corpus is an s58 boot, so nothing
here refutes the original observation. What is closed is the false alarm: from now on
`sched=WALKED reports=0` with no keypress is the baseline, and the recurrence signature is
`sched=WALKED` + keys pressed + **no** `SILENCE-BROKE` (device/TT/toggle, host side excluded) or
`sched=NOT-WALKED` (host side, convicted with no keypress needed).
