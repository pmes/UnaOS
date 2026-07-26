# BATMON-1 — Apple SMC battery monitor (x86, `UNAOS_SMC=1`)

Status: scout + battery read/render path code-complete + QEMU-gated (provisional key set); the
data-phase fix from the 2026-07-17 sitting — full multi-byte drain (GAP 1) + step-0 idle-guard
(GAP 2) — landed. Metal battery-tracking is the attended 2012 rMBP sitting, not a build gate.

Peter's end goal: run the 2012 MacBook Pro off its battery with an on-screen battery monitor —
"one less set of cords all over the place." This note records the protocol, the write surface, the
key-inventory plan, and the metal-pending list.

## The hardware line

The Apple **System Management Controller (SMC)** speaks a *polled* key/value protocol over two
legacy ISA I/O ports:

| Port  | Role                         |
|-------|------------------------------|
| 0x300 | data (key bytes out, value bytes in) |
| 0x304 | command + status             |

Keys are 4 ASCII characters (e.g. `REV `, `BRSC`, `B0AV`); values are typed, fixed-length byte
strings. There are **no interrupts, no DMA, and no ACPI interpreter** — the driver drives the ports
directly and waits (bounded) on the status byte. This is the `drivers/smc.rs` module; it is x86_64
only and compiled solely under the `smc` cargo feature (`UNAOS_SMC=1`). Knob off ⇒ the module and
every call site are unlinked and the media is byte-identical.

## Read protocol (the only transaction this driver performs)

The classic Apple SMC READ handshake, matched byte-for-byte by QEMU's `isa-applesmc` model:

1. Write `READ_CMD` (0x10) to **0x304**; wait until status low-nibble = `NEW_CMD|ACK` (0x0c).
2. Write the 4 key-name bytes to **0x300**, one at a time; each acks (status low-nibble = `ACK`, 0x04).
3. Write one length byte to **0x300**. The SMC looks the key up:
   * found ⇒ status sets `DATA_READY` (bit 0);
   * missing ⇒ status settles to `CMD_DONE` (0x00) — a *clean* "no such key".
4. Read value bytes from **0x300**, one per handshake. On the real Apple SMC the controller raises
   `BUSY` (0x02) while it shifts the next byte into 0x300 — `DATA_READY` de-asserts under it — so per
   byte the driver **waits for `BUSY` to clear, then inspects `DATA_READY`**: set ⇒ read one value
   byte from 0x300 and repeat; clear ⇒ the SMC has signalled end-of-value → stop and return the count.
   **Termination is the SMC's done-signal, not the caller's buffer length**, so an oversized buffer
   (`REV ` into an 8-byte buffer, the 32-byte scout buffer) still returns the true value length (6 for
   `REV `) with no spurious `Stuck`; the buffer length is only the safety cap that prevents writing
   past the caller. (QEMU's `isa-applesmc` never raises `BUSY` and holds `DATA_READY` continuously
   across all `len` bytes, clearing it once after the last — the same loop drains it byte-identically,
   which is exactly why the metal `len=1` truncation was invisible on QEMU. See the GAP 1 → GAP 2
   section below.)

Status bits (low nibble): `DATA_READY`=0x01, `BUSY`=0x02, `ACK`=0x04, `NEW_CMD`=0x08.

Every status wait is bounded by an `rdtsc` deadline (`SMC_WAIT_CYCLES` ≈ 0.1 s at 2.3 GHz Ivy
Bridge, ≈ 0.25 s under QEMU/TCG). A handshake that does not settle inside the budget returns
`SmcError::Stuck(step)` and the caller emits a traced STOP-NOTE line. **The driver never spins
forever and never forces a transaction through a wedged status bit.** Absent-vs-stuck is
disambiguated from the 0x304 status byte alone.

## Write surface (tripwire-grade)

Port I/O is confined to **{0x300, 0x304}**, and only under `UNAOS_SMC=1`. During a READ the driver
writes the 4 key-name bytes and one length byte to the data port — those are the read protocol's
arguments, **not** a state-changing SMC write. The value-mutating `WRITE_CMD` (0x11) — which would
change machine state (fan speed, LEDs, charge behaviour) — is **never issued**; any future need for
it is a STOP-and-report, a new arc with its own review. The error/interrupt port (0x31e) is
deliberately **not** touched, keeping the surface exactly two ports.

## Key inventory plan (M1 scout)

`smc::scout()` fires once at boot from `arch::x86_64::pci::init` and dumps the SMC key surface over
serial, bracketed `:: SMC-SCOUT: begin ::` … `:: SMC-SCOUT: end (present=… probed=… found=…) ::`.
It has two parts:

1. **Probe-by-name sweep** over a curated key list (`PROBE_KEYS`) — each key reported
   present(len,bytes) / absent / STOP-NOTE. No key is *assumed* present.
2. **Index enumeration** — read `#KEY` (the ui32 key count) and walk it via `GET_KEY_BY_INDEX`
   (0x12). This is **metal-first**: QEMU implements neither `#KEY` nor 0x12, so on QEMU the scout
   reports enumeration unavailable and moves on (bounded, no hang).

The curated list holds the standard Apple SMC names for the 2012-era battery block — `BNum`, `BSIn`,
`BRSC` (state of charge %), `B0AC`/`B0AV` (amperage/voltage), `B0FC`/`B0RM` (full/remaining
capacity mAh), `B0St`, `B0TF`, `CHBI`/`CHBV` (charger), `AC-W`, and `BC1V`/`BC2V`/`BC3V` (the
per-cell fork probes). **These names are candidates, not facts:** the first attended sitting's
`SMC-SCOUT` log is the machine's real inventory, and it decides M2's exact key set and whether the
per-cell path exists.

## Battery monitor (M2)

`smc::battery::snapshot()` reads the charge/voltage/amperage keys into a `BatterySnapshot` whose
every field is `Option` — a key the SMC lacks stays `None` (honest absence, never a placeholder).
`refresh_if_due()` throttles the port I/O to ~1 s and caches the snapshot; it emits a
`:: SMC-BATT: … ::` witness (see the quiet-boot policy below), and `battery::cached()` feeds the
on-screen meter. The meter is rendered by the vug meter surface
(`vug::draw_meters`, the "BATT" bar + readout) and — because the serial-less metal debug view mirrors
serial to the framebuffer — the `SMC-BATT` line is also on-screen in the `UNAOS_USBDEBUG` boot Peter
photographs at the sitting.

### Quiet-boot witness policy

**Invariant: on a quiet attended boot, each periodic witness appears in the log exactly once —
nothing scrolls.** This is a *UI* requirement, not log hygiene: `PANEL_CONSOLE` mirrors every
`serial_println!` to the panel at the GUI takeover seam, so anything that re-prints on a timer
scrolls the glass forever and ruins the photograph the sitting exists to produce.

`SMC-BATT` therefore fires:

* once on the first refresh (the honest `present=false` line on QEMU / a battery-less machine
  included — it proves the M2 read path ran);
* whenever the pack **state** changes — `present`, `soc_pct`, `ac_present`, `ac_stuck`,
  `ac_derived`. Voltage, amperage and remaining-mAh are deliberately **not** part of the state key:
  they jitter on every sweep on real hardware, so keying on them would re-print at 1 Hz forever.

The `ac_derived` component of that key is the **settled** shape, not the instantaneous one. A single
dashed `B0AC` read makes `derive_ac(None)` return `Unknown` for exactly one sweep; counting that as a
state change flipped the key twice (`derived:… → Unknown → derived:…`) and re-printed the witness at
1 Hz with a transient `ac=?`. When the derivation has nothing to say, the key keeps the last shape it
did have. `ac_present` and `ac_stuck` are unaffected — those are answers, not inferences — and the
printed line still reports the honest instantaneous value.

#### Retries are not an event (s39)

`retries > 0` used to be a third fire condition. On the 2012 rMBP's SMC it is not an event but the
norm: the s39 metal boot showed `retries=2/2, 2/4, 6/14, 3/33, 4/44 …`, i.e. virtually every sweep
consumes some, so the disjunct fired the predicate every second and the witness scrolled the glass
exactly as it had before the quiet fix. It is retired from the quiet arm.

The counts are not lost. They ride on the once-per-boot line and on every state-change line, and a
**rollup** reports whatever the reader has not already been shown, at most once per
`RETRY_ROLLUP_MS` (300 s quiet, 60 s under `bootlog`):

```
:: SMC-BATT: retry rollup — 41 retries in the last 300000 ms (total 44) == rollup ::
```

Whenever the witness line fires it carries `retries=SWEEP/TOTAL`, so the rollup consumes the delta
and stays silent. Under `bootlog` the witness fires on every retrying sweep, so the delta is always
consumed there and the rollup never has anything to report — that mode's log is unchanged.

#### Holds must earn their line (s39)

The BATMON-HOLD pair (`sweep failed (present=false) — holding` / `good reading returned — hold
released`) also printed once per second on s39: on this SMC a **one-sweep** drop-out is normal, and
each blip produced a full hold/release pair. The first hold of a boot still prints immediately —
that this machine drops sweeps at all is the fact a reader needs — and every later hold prints only
once it has **persisted** past `HOLD_NOTE_MS` (5 s), i.e. once it is an outage rather than a blip.
The line carries the hold's own age (`held N ms`) alongside the cached reading's staleness. The
release line prints only if its hold was announced, so the pair can never appear half-printed.

**No information is lost:** every state change and every failing sweep that lasts still prints, and
retry counts survive on the witness lines plus the rollup. Only identical repeats and sub-threshold
blips are dropped. Under `UNAOS_BOOTLOG=1` (the `bootlog` feature — boot-log-held-on-screen sitting mode) the
full ~1 s cadence is restored **exactly** as it was before this policy, so a sitting can still watch
discharge track live; the two predicates are disjoint arms in `refresh_if_due()`, never OR-ed.

The same audit was applied to the other periodic witnesses in that main-loop service group, all of
which were already once-only and needed no change: `bootlog::service_serial_dump` (re-dumps only
when the milestone ring has **grown**), `flight_recorder::service` (`ANNOUNCED` latch — one
`FLIGHTREC:` PASS or error line per boot), and `xhci::log_summary_once` (fires on the exact 2000th
main-loop pass).

The M2 key decode (ui16 big-endian for voltage/capacity, signed for amperage, `BRSC` as %) is the
documented standard for the era and is **provisional** until the M1 metal inventory confirms it; the
per-cell fork carries **no code** yet (it is gated on the inventory showing `BC?V` keys).

## QEMU vs metal (honest by construction)

QEMU's `isa-applesmc` (attached by the builder under `UNAOS_SMC=1`) implements only `READ_CMD` over
a tiny key set: `REV ` (6 bytes, `01 13 0f 00 00 03`), `OSK0`/`OSK1`, and a few status keys. It has
**no** battery keys, **no** `#KEY`, and **no** `GET_KEY_BY_INDEX`. So the QEMU gate is:

* `:: SMC-SCOUT: key REV present len=6 bytes=[01 13 0f 00 00 03] ::` — the known-key read that
  proves the protocol machinery;
* every battery key + `#KEY` reported cleanly **absent** (bounded, no hang);
* `:: SMC-BATT: present=false … ::` — the M2 read path ran and honestly found no battery.

Battery keys and key enumeration are metal-first by construction.

## Metal data-phase defects — the GAP 1 → GAP 2 causal chain (2026-07-17 sitting)

The first attended 2012 rMBP sitting exposed two defects **invisible on QEMU** (QEMU's `isa-applesmc`
returns proper lengths and never wedges, masking both). Both are fixed by the data-length drain +
step-0 idle-guard landed in this note's driver.

- **GAP 1 — every responding key read back `len=1`.** The old value-drain loop read the 0x304 status
  once per byte and broke the instant `DATA_READY` was clear. On the real SMC `DATA_READY` de-asserts
  momentarily between value bytes while the controller raises `BUSY` (0x02) and shifts the next byte
  into 0x300, so after byte 0 the loop saw `DATA_READY` clear and exited with `n=1`. Every multi-byte
  value collapsed to its first byte; `battery::read_u16` (needs `Ok(2)`) returned `None`, so
  `present(battery)=false`, found=10/18. QEMU holds `DATA_READY` set continuously across all `len`
  bytes, so the identical loop drained fully there — which is why the bug was masked.
- **GAP 2 — 8/18 keys wedged at handshake "step 0"** (bounded, never forced): `REV ` `#KEY` `B0AC`
  `B0FC` `B0St` `CHBI` `AC-W` `BC2V`. **This is downstream of GAP 1, not independent.** A truncated
  read left the remaining value bytes undrained and the READ transaction incomplete (`DATA_READY`
  still pending inside the SMC); with no flush before the next command, the following key's
  `write_cmd(READ)` was issued into a still-busy SMC and its step-0 `NEW_CMD|ACK` wait timed out →
  `Stuck(0)`. The metal inventory proves it exactly: each wedge key was immediately preceded in
  `PROBE_KEYS` order by a **multi-byte** key (`REV `←prior REV, `#KEY`←OSK0, `B0AC`←BRSC, `B0FC`←B0AV,
  `B0St`←B0RM, `CHBI`←B0TF, `AC-W`←CHBV, `BC2V`←BC1V), while every key preceded by a genuine 1-byte key
  (`BSIn`←BNum) or by a wedged key (which wrote no key bytes and left no residue) read fine — a perfect
  wedge/fine alternation past the BNum/BSIn pair. The ~0.1–0.25 s `Stuck` wait outlasts the SMC's own
  transaction timeout, so the SMC self-heals during each wedge and the next key starts clean, which is
  why exactly *every other* key wedged instead of the whole sweep cascading.

**The fix.**
- *Data-length drain.* The value-drain loop now runs the per-byte `BUSY`-then-`DATA_READY` handshake
  of read-protocol step 4 above (new `ST_BUSY = 0x02` const + a bounded `wait_busy_clear` helper on the
  same `rdtsc` budget; a genuine per-byte timeout yields `Stuck(3)`, never an unbounded spin). Full
  multi-byte values come back and each transaction completes cleanly. The same handshake is mirrored in
  `read_key_by_index`'s 4-byte name drain (there an early `DATA_READY`-clear stays an error — a name
  must fully drain).
- *Step-0 idle-guard.* `settle_before_command()` runs before every `write_cmd`: if the status still
  shows `DATA_READY`/`BUSY` (a stale partial read) it drains leftover data bytes under the bounded
  budget before issuing the command. It is a **no-op on an idle SMC** (always the case on QEMU between
  transactions), so QEMU behaviour is byte-identical; on metal it is belt-and-suspenders against any
  residue the clean drain does not already remove.

Because a clean drain completes each transaction, the data-length drain is expected to eliminate GAP 2
on its own; the idle-guard is defence-in-depth. Both fixes are **no-ops on the emulated path by
construction** — QEMU returns full lengths and never wedges — so the QEMU gate is unchanged (`REV `
len=6 `bytes=[01 13 0f 00 00 03]`, `SMC-BATT present=false`). The metal correctness (full multi-byte
values; the 8 keys no longer wedging) is provable only at the attended sitting.

## Metal-pending (the attended rMBP sitting — Peter's, not a build gate)

Assertable at the attended 2012 rMBP sitting (`UNAOS_SMC=1` media). Items 1–5 were the original scout
gate; items 6–8 confirm the GAP 1 / GAP 2 data-phase fix (see the section above) on silicon:

1. `:: SMC-SCOUT: key REV present … ::` on real silicon (protocol works on the metal SMC).
   ✅ 2026-07-17 (SMC alive; real drifting telemetry across boots).
2. The `SMC-SCOUT` battery block: which of the curated keys the real SMC carries + their payloads —
   **the machine's true battery inventory** (records the M2 key set + the per-cell fork verdict).
   ✅ 2026-07-17 (found=10/18 present, per-cell fork observed) — but truncated to `len=1` (GAP 1);
   re-run under the fix records the full payloads.
3. `#KEY` present ⇒ the index walk emits the full key list.
4. `:: SMC-BATT: present=true soc=… volt=… ::` tracks reality: **unplug ⇒ discharge (amp < 0, soc
   falls), plug ⇒ charge (amp > 0)**; the on-screen "BATT" bar follows. The battery-tracking sub-leg —
   a later METAL leg, **explicitly not** part of the data-phase-fix build gate.
5. No handshake STOP-NOTE on the metal SMC (bounded waits sized correctly for real timing).
6. **Full multi-byte values (GAP 1 fixed).** Multi-byte keys report `len>1` — voltages `B0AV`/`BC1V`/
   `CHBV`, capacities `B0RM`/`BSIn`, the 6-byte `REV `, the 32-byte `OSK0` — not the pre-fix `len=1`
   truncation; and `:: SMC-BATT: present=true … ::` (with `B0AV`/`B0RM`/`BRSC` decoding to plausible
   values) replaces the pre-fix `present=false`.
7. **No step-0 wedge (GAP 2 fixed).** All 18 `PROBE_KEYS` respond (present or clean-absent); the
   pre-fix 8/18 `STOP-NOTE handshake stuck at step 0` list (`REV ` `#KEY` `B0AC` `B0FC` `B0St` `CHBI`
   `AC-W` `BC2V`) is gone once the clean drain leaves the SMC idle between transactions.
8. The idle-guard (`settle_before_command`) never has to drain residue on a healthy SMC — an honest
   check that GAP 2 was truly downstream of GAP 1 and not a second independent defect.

## GUI-build dead-SMC investigation (2026-07-18, arc 4 of the sitting-1 verdicts)

Fourth consecutive data point: GUI/sched_demo builds read `SMC-BATT present=false` on every metal
boot while a usbdebug build on the same machine, same day, read `present=true soc=51%`. A source
diff of everything that runs before the first SMC read found **no feature-gated difference in the
SMC path itself** — scout and first battery sweep fire from the same `pci::init` site in both
builds, after the same ACPI/calibrate/SMP/EHCI bring-up. The two real differences are *timing*
(the GUI's quiet-panel boot reaches `pci::init` much earlier than the fbcon-heavy usbdebug boot —
the "scout too early relative to EC readiness" hypothesis stays open) and *scheduling* (sched_demo
APs + 1 kHz heartbeats live during the transactions). Neither is proven; per the directive the
driver now carries **unconditional first-failure instrumentation** so the next sitting captures
wire truth:

* `:: SMC-DIAG: pre-touch t=<ms> raw status=<byte> ::` — timestamp + cold status byte before the
  first transaction of the boot (compares first-touch time across builds).
* `:: SMC-DIAG: FIRST FAILURE key <k> kind <absent|stuck> step <n> t=<ms> — raw status timeline
  [16 bytes, ~15 µs apart] == evidence ::` — one-shot, fires on the boot's first failed key read
  from any path, usbdebug-independent, read-only (status reads only). Dead-flat `00`/`ff` vs
  busy-wedged vs oscillating status distinguishes absent-device / wedged-handshake / EC-not-ready.

Perf coupling fixed the same arc (the "cursor slows vug" metal verdict was actually this): on an
unresponsive SMC each battery sweep burned up to ~16 bounded stuck-handshake budgets (~0.1 s each)
on the vug meter cadence, every second. Now a sweep whose FIRST key comes back Stuck aborts the
rest (one-shot noted line), and consecutive failed sweeps back the refresh interval off 1 s → 32 s
(reset by any good sweep). Clean-`Absent` sweeps (QEMU) still probe every key — the QEMU witness
lines are unchanged apart from the additive SMC-DIAG lines.

## IVY — read robustness against the three standing metal caveats (2026-07-25)

Three caveats survived every earlier arc, all metal-observed, none reproducible on QEMU. This arc
addresses two of them and deliberately leaves the third alone.

### Caveat 1 — `AC-W` is ABSENT on this machine ⇒ derived AC state

The 2012 rMBP's SMC carries **no `AC-W` key**: the read comes back a clean `Absent` (the SMC looked
it up and said no), not a wedge. So `ac_present` can never resolve there, and the witness printed a
bare `ac=?` on every line forever — a hole where the most user-visible fact belongs.

AC presence is nonetheless *inferable*, because the `B0AC` amperage is signed and its sign is
metal-confirmed to flip correctly with the adapter (BATMON M2 sitting):

| `B0AC` (mA) | `AcDerived` | what it means |
|---|---|---|
| `> +32` | `charging` | charge flowing INTO the pack ⇒ adapter present and sourcing |
| `< −32` | `discharging` | pack sourcing the machine ⇒ adapter not carrying the load |
| within ±32 | `idle` | **ambiguous** — see below |
| no reading | `unknown` | `B0AC` was a hole this sweep; nothing to infer from |

**The ambiguity is real and is not papered over.** A full pack on the adapter settles at ~0 mA,
which is indistinguishable *from amperage alone* from a machine resting on battery below the noise
floor. Both land in `idle`, which asserts nothing about AC presence — `idle` is a refusal to guess,
not a claim. The ±32 mA deadband exists because the reading dithers by a few mA at rest; without it
the state would flap between `charging` and `discharging` on sensor noise alone.

The inference **never overrides a direct answer**: `ac_present` (from `AC-W`) remains the truth on
any machine that carries the key, and the witness only falls back to the derived state when `AC-W`
did not answer. The serial field is tagged so the two can never be confused:

* `ac=yes` / `ac=no` — direct, from `AC-W`;
* `ac=derived:charging|discharging|idle` — inferred from the `B0AC` sign (the rMBP's normal case);
* `ac=?` — neither an `AC-W` answer nor a `B0AC` reading. This should now be rare.

### Caveat 2 — per-read field drop-out ⇒ counted bounded retry

Individual keys intermittently fail a given sweep on the real SMC while their neighbours succeed,
so a telemetry line can carry holes (`-`) that mean "this read missed", not "this machine lacks the
key". The numeric keys already re-read on failure (`READ_ATTEMPTS`, each attempt itself bounded by
the `rdtsc` budget, a clean `Absent` never retried). This arc:

* extends the same bounded retry to the `AC-W` read, which previously got exactly one shot, so a
  wedged handshake there no longer masquerades as an absent key;
* **counts** every re-read and reports it, because the caveat was previously invisible in the
  telemetry — holes were observable, but their *frequency* and how often a retry rescued a read
  were not. The witness now ends `retries=SWEEP/TOTAL`: re-reads consumed by the sweep that
  produced this line, and re-reads since boot. A metal sitting can now read drop-out rate directly:
  `retries=0/0` means the SMC answered first-try throughout; a climbing total with `present=true`
  means retries are doing their job; a climbing total *with holes* means the budget is undersized.

The retry budget itself is deliberately **unchanged** — this is instrumentation plus one gap-fill,
not a loosening. Total work per sweep stays bounded and no re-read can become a spin.

### Caveat 3 — the `#KEY` bounded wedge is LEFT EXACTLY AS IS

`#KEY` enumeration wedges at a handshake step on this machine; the driver already bounds it
(`SMC_WAIT_CYCLES` + `MAX_ENUM_KEYS`) so the wedge is finite and reported, never forced. That bound
is a protection and this arc does not touch it, relax it, or route around it.

### Witness format (the only line that changed; nothing new is periodic)

```
:: SMC-BATT: present=true soc=51% volt=11540mV amp=-1820mA full=9962mAh rem=5081mAh ac=derived:discharging retries=0/3 == witness ::
```

Two additive fields on the existing ~1 s SMC-BATT line: `ac=` gained the `derived:` form, and
`retries=` is new. No new periodic output was introduced.

### What only metal can verify

QEMU's `isa-applesmc` has **no battery keys at all**, so on QEMU the sweep aborts on the first key
and the witness line never exercises either change with real data. Specifically, only the 2012 rMBP
can confirm:

1. `ac=derived:` actually appears (i.e. `AC-W` is still absent and `B0AC` still reads);
2. the derived state matches physical reality — `discharging` on battery, flipping to `charging`
   within a sweep or two of plugging the adapter in;
3. the deadband is correctly sized — a resting machine reads `idle` rather than flapping;
4. `retries=` is non-trivial, quantifying the drop-out rate for the first time;
5. holes (`-`) become rarer than the pre-IVY baseline, if the retry is in fact rescuing reads.

The build gates (`arroyo check` both arches, `test`, `test-arm`) prove only that the code compiles
and that the non-SMC kernel is unregressed.
