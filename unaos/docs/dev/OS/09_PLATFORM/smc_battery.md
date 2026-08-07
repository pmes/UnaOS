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

#### Presence must be held to count (s41)

The s41 metal boot printed the witness every few seconds — better than 1 Hz but not quiet. The wire
shows why: `present=true` / `present=false` **alternate line to line** (`soc=62%` … `present=false
soc=-%` … `present=true`, `retries=2/178, 2/180, 2/301 …`). With the raw `present` in the state key,
every flap was a "state change" and earned a print. It is not one. The key's presence component is
now the **held/debounced** presence, exactly as `ac_key` carries the settled AC shape: while
BATMON-HOLD is active and the hold is younger than `HOLD_NOTE_MS` (5 s — the same blip/outage
threshold the hold notes use), the key keeps the **whole** shape it had. Presence alone would not be
enough: `soc_pct` reads `None` on precisely those failed sweeps, so a raw-`soc` key would re-print
regardless.

A **real** removal still prints, once: either the hold outlives `HOLD_NOTE_MS` (an outage, not a
blip) or there was never a good reading to hold (QEMU / a battery-less machine), and in both cases
the key resolves to the honest `present=false`. The printed line always reports the **instantaneous**
sweep — only the fire decision is debounced.

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
  **The fire condition here is the original one and was wrong** — it counted a clean `Absent` as a
  failure, which spent the one-shot on `AC-W` every boot. Corrected in *SMC-DIAG was crying wolf*
  below; the line's format and read-only character are unchanged.

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

> Since GR17 the key is **probed once and then skipped** (re-probed every 60 s so the learning stays
> falsifiable), and its absence is stated once instead of being re-alarmed at every boot — see
> *SMC-DIAG was crying wolf* at the end of this note. The inference below is unchanged.

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

## SMC-DIAG was crying wolf — `AC-W` absence is not a failure (GR17, 2026-08-06)

Every SMC boot of the 2012 rMBP printed this, and had done since `AC-W` entered `PROBE_KEYS`:

```
:: SMC-DIAG: FIRST FAILURE key AC-W kind absent step 255 t=3202ms — raw status timeline
   [40 40 40 40 40 40 40 40 40 40 40 40 40 40 40 40] (16 reads, ~15us apart) == evidence ::
```

### What the line actually said

| Field | Reads as | Actually means |
|---|---|---|
| `FIRST FAILURE` | something broke | `read_key` returned `Err` — but `Err` includes `Absent` |
| `kind absent` | a fault mode | `SmcError::Absent`: the SMC **looked the key up and answered "no such key"**. A completed transaction with a negative result. The driver's own definition calls it *"Clean."* |
| `step 255` | handshake step 255 | there is no step 255 — the real steps are 0..=3. `dump_first_failure` substituted `0xFF` for "no step applies" and printed the sentinel straight into the numeric field |
| `[40 ×16]` | raw fault evidence | `0x40 & ST_MASK(0x0f)` = `0x00` = **`ST_CMD_DONE`**: idle, command complete. Bit 6 is this machine's static idle-high bit |

The `[40 ×16]` timeline is the decisive part, and it decodes to the *opposite* of a fault. The same
boot prints `:: SMC-DIAG: pre-touch t=… raw status=0x40 ::` two lines earlier — the cold status byte
read **before any transaction is issued**. The "evidence" of the failure is byte-identical to the
idle reading of a healthy controller. The dump's own rubric only names dead-flat `00`/`ff` (device
absent), busy-wedged, and oscillating; flat-at-`0x40` is none of those, because the dump was never
designed to fire on a clean `Absent`.

### Verdict: false alarm — and a disabled instrument underneath

`AC-W` absence on this machine was already an established, metal-confirmed fact (Caveat 1 above,
2026-07-25; and the driver's own `AcDerived` comments). The s73 capture (3 SMC boots) confirms it is
stable and isolated:

* `probed=18 found=17` on all three boots — **`AC-W` is the only absent key**;
* **zero** `STOP-NOTE handshake stuck` lines in the entire capture;
* the `ac=` field is never `stuck` — 168 `derived:idle`, 58 `?`, 0 `stuck`;
* the battery monitor does **not** degrade: `present=true soc=100% volt≈12817mV full=9962mAh` right
  through, i.e. nothing downstream of the missing key is impaired.

So AC wattage/presence is genuinely unreadable on this machine, exactly as documented, and the
existing IVY-AC handling of that (`ac_present = None`, `ac=derived:*` tagged as an inference, `-`
sentinels, the vug meter using only the `B0AC` sign) was already honest. **Nothing downstream
fabricates an AC reading, and this arc changed none of that.**

The real damage was upstream. `dump_first_failure` fires **once per boot** (`FIRED: AtomicBool`), and
`AC-W` is the first key in `PROBE_KEYS` order to return any `Err` — deterministically, ~4 ms into the
scout. A documented non-event therefore **consumed the diagnostic slot before any real failure could
claim it**. The capture shows the burial concretely: boot 1's DIAG fired on `AC-W absent` at 3252 ms,
and 98 ms later the first battery sweep dropped out completely —

```
[   3350ms] :: SMC-BATT: present=false soc=-% volt=-mV … ac=? retries=11/11 == witness ::
```

— a sweep that burned 11 bounded retries, i.e. a cluster of wedged reads. That is precisely the wire
event the DIAG was built for (see the 2026-07-18 section), and it could not fire. The instrument had
not been able to report a genuine first failure on any boot for weeks.

### The fix — absence is learned, not alarmed at (KEY-SHAPE)

`drivers/smc.rs` now keeps a per-boot table of what it has learned about each key's *existence* on
this SMC, and the DIAG's fire condition reads from it:

| Outcome | Before | Now |
|---|---|---|
| `Stuck(step)`, any key | DIAG fires | DIAG fires — **unchanged** |
| `Absent`, key never seen to answer | DIAG fires (the bug) | learned `Absent`; the scout reports it as inventory |
| `Absent`, key that already answered this boot | — | DIAG fires: `kind absent-unexpected` |
| `Absent` of `REV ` | — | DIAG fires (seeded `Present`: the protocol requires it) |

**Nothing is weakened** — but the first draft of this note argued that badly, and the argument is
worth getting right because it is the whole safety case.

The wrong version claimed `Absent` "requires the status low nibble to read exactly `0x00`, which a
wedge cannot produce". That is false on its face, and this very note disproves it four paragraphs
up: the healthy idle byte on this machine is `0x40`, whose low nibble **is** `0x00`. A bus stuck at
`0x40` would satisfy that test.

The real invariant is the *sequence*, and it is stronger. Reaching the length step — the only place
`Absent` can be returned — means the transaction already passed **two different waits**:

1. `wait_status(ST_AFTER_CMD = 0x0c, step 0)` after the command byte, and
2. `wait_status(ST_AFTER_ARG = 0x04, step 1)` after each of the four key-name bytes.

No constant satisfies both `0x0c` and `0x04`. So a bus stuck at **any** value — `0x00`, `0xff`,
`0x40` alike — times out at step 0, yields `Stuck(0)`, and fires the DIAG unconditionally. `Absent`
is reachable only from a controller that actively handshook through six exchanges and then answered
"no such key", which is the definition of one that is working. The bounded waits, the retry budget,
the write surface and the `#KEY` wedge bound (Caveat 3) are all untouched.

#### One `Absent` is not enough to act on

The DIAG's absent arm spends the boot's only latch, and the sweep beside it already retries a
`Stuck` three times before believing it — so believing a *single* `Absent` was the weaker standard
of the two. Both places that draw a conclusion from an absence now corroborate it first:

* `read_key` re-reads once before firing the DIAG on an `Absent`-of-`Present`, and fires only if it
  repeats. If the re-read succeeds, the value is returned rather than the hole.
* the scout re-reads once before printing "this SMC does not carry it" — a confident claim about the
  machine that also seeds KEY-SHAPE for the rest of the boot. The line now reads `absent (x2)`, and
  a first-absent/second-present disagreement prints as the bad sample it was.

#### Mass absence is the silent path

Every absent key now prints a calm, individually reasonable inventory line — so an EC that acks
`REV ` and then refuses everything else would emit a page of them and **no diagnostic at all**, since
no single read failed in a way the DIAG recognises. That cannot be judged per key, only in aggregate,
so the scout carries a floor: below `REV ` + `OSK0` answering (2 keys — the set even QEMU's minimal
model carries), it emits

```
:: SMC-SCOUT: STOP-NOTE mass absence — only N of M keys answered, below the floor of 2 (REV + OSK0).
   This reads like a controller that acks the presence probe and refuses the rest, NOT a machine
   with a different key set; treat the absent lines above as unproven ::
```

#### Transactions are now serialized

An SMC read is a six-exchange conversation with a stateful controller whose data reads *advance its
own cursor*. Two interleaved conversations do not merely race for a value: one drains the other's
data bytes, so the victim reads `ST_CMD_DONE` and reports a clean **`Absent` for a present key** —
indistinguishable at the call site from the real thing, and under KEY-SHAPE enough to spend the DIAG
latch on a phantom. This reintroduces the arc's own disease by a different door.

It is reachable today: the `batmon` shell verb calls `snapshot()` unthrottled, a sweep against a
wedging SMC overruns the 1000 ms throttle so two `refresh_if_due` callers can both find it due, and
the service-task and vug-cadence sites are distinct paths. So `read_key_inner` and
`read_key_by_index` now run under a `TXN` spin lock. **The lock is safe on every caller**: `pci::init`,
the `main.rs` service bodies, the vug cadence, the `batmon` verb and `bench_ride` are all ordinary
task context and **no interrupt handler touches the SMC**, so an ISR cannot spin on a lock its own
interruptee holds; re-entrancy is impossible (`read_key_inner` calls only port I/O and `now_cycles`);
and hold time is one transaction, bounded per step by `SMC_WAIT_CYCLES` exactly as before.

The throttle-then-transact sequence outside the lock can still race — two callers may both sweep.
That is a wasted sweep, not a wrong reading.

Three smaller corrections ride along:

* **`step 255` → `step n/a`** on the absent arm — a step that does not exist no longer prints as one.
* **The scout's absent line names itself**: `key AC-W absent (x2) — this SMC does not carry it (clean
  negative answer, not a fault) (AC adapter wattage / presence)`. That reading is the *inventory
  result*; it used to arrive dressed as `FIRST FAILURE … == evidence`.
* **`B0Pr` joins `PROBE_KEYS`** (`probed` 18 → 19). `battery::snapshot()` reads it every sweep to
  decide `present`, but the scout never probed it, so its shape on this machine was undocumented and
  was being rediscovered 1 Hz at a time inside the sweep.

#### What the DIAG still does not cover, deliberately

`read_key_by_index` does not route to `dump_first_failure`, and the earlier claim that the DIAG fired
"from whatever path (scout, battery sweep, enumeration)" was simply untrue — enumeration never
reached it. The claim is dropped rather than the path wired up, because **Caveat 3 records the `#KEY`
enumeration wedge as a standing condition on this machine**: routing it to the one-shot latch would
re-spend the DIAG on a known fault every boot, which is precisely what was just removed from `AC-W`.

The information is not lost — the scout's enumeration handler no longer collapses the two outcomes:

* `index enumeration ended at idx N — GET_KEY_BY_INDEX answered no-such-index (clean stop)`
* `index enumeration STOP-NOTE at idx N — handshake wedged at step S (bounded, not forced; Caveat 3)`

If that STOP-NOTE ever stops appearing, it is the evidence for routing enumeration to the DIAG.

#### There is no SWEEP-ABORT, and the comment claiming one was stale

`battery::snapshot()` carried a comment describing a first-key-`Stuck` early return that the code no
longer had — `6b34e1f7` removed the block. **The removal was the point, not collateral**, and that
commit says so: *"a stuck key no longer invalidates the keys that answered … volt, full and rem
dropped out while amp still read, one sweep before BRSC stuck aborted everything and latched
present=false. Three unplugged boots produced zero PWR windows because of it."* On this SMC keys drop
out independently, so keying the whole sweep on `BRSC` discards every other key's good reading and
manufactures a `present=false` the pack never had. **It is not restored**; the comment is corrected
to describe the code that exists and to record why the abort must not come back.

Its stated purpose — not burning ~16 bounded stuck-handshake budgets per second on the vug cadence —
is still served by the `FAIL_STREAK` backoff in `refresh_if_due` (1 s → 32 s on consecutive failed
sweeps, reset by any good one). That throttles the *frequency* of expensive sweeps without discarding
the keys that answered, which is the correct axis. The worst-case single sweep is still long; `TXN`
serialization is what stops that from corrupting a concurrent reader.

### Probe-once for any absent key, and saying the unknown out loud

The 1 Hz sweep re-ran a full `AC-W` transaction every second to rediscover a fixed property of the
SMC's firmware key set. Once a key is known absent the read is skipped, and for `AC-W` the fact is
stated once — this line *replaces* the every-boot false-alarm rather than adding to the log:

```
:: SMC-BATT: AC-W is absent on this SMC (clean negative answer, not a fault) — AC presence is
   UNKNOWN; ac=derived:* is inferred from the B0AC sign, and the key is re-probed every 60000 ms
   == witness ::
```

**The skip is not `AC-W`-specific**, because the argument never was: any key learned absent is a
fixed property of the controller's firmware. `B0Pr` is the concrete reason it had to generalize —
the sweep reads it every pass to decide `present`, its shape here is unknown until the next boot,
and an `AC-W`-only skip would have left it re-probing at 1 Hz permanently: a second standing cost of
exactly the kind this arc removed. `probe_once_skip` therefore keys on `shape_of(key) ==
SHAPE_ABSENT` and covers `read_u16k` as well as the `AC-W` read.

One timer serves all of them. The re-probe window is decided **once at the top of each sweep**, so
every absent key re-probes together on the same minute; a per-key timer sharing one clock would have
stretched any one key's re-probe to 60 s × the number of absent keys and made the cadence depend on
`PROBE_KEYS` order.

The skip is **not** a cached claim: `ABSENT_REPROBE_MS` (60 s) re-tests it, so on a machine that does
carry `AC-W` — or if a clean `Absent` were ever produced by something other than a real lookup miss —
the skip self-corrects within a minute and `ac_present` resolves to the direct answer. Falsifiable,
not assumed. `ac_present` stays `None` while skipped and the `ac=derived:` tagging is unchanged:
**the unknown is reported as unknown.**

#### A liveness re-probe must not report as the sweep's AC state

`ac_stuck` exists to separate two facts that both leave `ac_present = None`: *this machine has no
`AC-W`* (stable, covered by the derived state) versus *`AC-W` is there and the handshake wedged*
(a live fault). A re-probe of a key already known absent is neither — it is a liveness poll — and
letting its failure set `ac_stuck` made the flag alternate `true` on the re-probe minute and `false`
on the other 59 sweeps, **forever**.

`ac_stuck` is part of the `LAST_STATE` quiet-witness key, so every one of those flips earned a
witness line: two extra lines per minute, permanently, from an instrument whose entire purpose is to
print once — and it would have falsified this note's own prediction 6 (`ac=stuck` must not appear) on
the first metal boot. A wedged liveness poll now leaves the sweep's AC fields exactly as a skipped
sweep would; only a genuine `AC-W` read (a key not known absent) can set `ac_stuck`.

#### Related honesty gaps outside this driver (not fixed here)

Two consumers report the AC picture more coarsely than the driver knows it. Both are outside
`smc.rs` and were left alone:

* `vug.rs`'s meter renders `chg`/`dis` straight off the `amp_ma` sign with **no deadband**, so the
  on-screen flow indicator can flap between them at rest where the witness — which applies the
  ±32 mA `AC_IDLE_DEADBAND_MA` and lands on `idle` — deliberately does not.
* `shell.rs`'s `batmon` verb collapses *absent*, *stuck* and *derived* into a single `-`. That is
  honest (it does claim unknown, never a number) but it cannot say "this machine has no `AC-W`, and
  the derived state is idle" the way the serial witness does.

### Metal prediction (falsifiable, next `UNAOS_SMC=1` boot)

QEMU has no battery keys and cannot exercise this; the gates prove compilation and reachability only
(`strings target/x86_64_esp/kernel.elf` carries all four new strings). On the 2012 rMBP:

1. **`:: SMC-DIAG: FIRST FAILURE …` does not appear at all** — neither for `AC-W` nor any other key,
   provided no handshake wedges. This is the headline: the line stops being a fixture of every boot.
2. `:: SMC-DIAG: pre-touch t=… raw status=0x40 ::` still prints, unchanged (it is not the one-shot).
3. `:: SMC-SCOUT: key AC-W absent (x2) — this SMC does not carry it (clean negative answer, not a
   fault) (AC adapter wattage / presence) ::` replaces the bare absent line. The `(x2)` is the
   corroborating re-read; if it ever reads `first read said absent, second disagreed`, the s73
   inventory was resting on a bad sample and the key set is not what we recorded.
4. `:: SMC-SCOUT: end (present=Y probed=19 found=…) ::` — `probed` rises 18 → 19 (`B0Pr`).
   `found` = 18 if `B0Pr` answers, 17 if it too is absent; either is a *result*, and its `B0Pr`
   scout line now records which.
5. **No `STOP-NOTE mass absence` line** — `found` will be 17 or 18, far above the floor of 2. It
   appears only if the controller has degraded to acking `REV ` and refusing the rest.
6. The `AC-W is absent on this SMC …` line appears **exactly once**, from the first battery sweep,
   and `AC-W` costs one transaction per minute thereafter rather than one per second. Same for
   `B0Pr` if it turns out absent.
7. `SMC-BATT` lines are otherwise unchanged: still `ac=derived:idle` / `ac=?`, still `present=true
   soc=100% volt≈12.8 V full=9962mAh`, `retries=` still non-zero. **`ac=stuck` must not appear** —
   and unlike the previous draft of this prediction, the re-probe can no longer produce it.
8. **The falsifier for the whole verdict:** if a `FIRST FAILURE` line *does* appear, it will name a
   `stuck` step with a non-`0x40` timeline (or an `absent-unexpected` regression that survived a
   corroborating re-read) — a real fault finally able to reach the instrument. The s73 boot-1 sweep
   drop-out (`retries=11/11` at 3350 ms) makes that a live possibility, and it is now the *desired*
   outcome: the DIAG reporting the thing it was built for instead of the thing it was buried under.

## The instrument fired on its first metal boot — the B0AV step-0 stall (s73, 2026-08-06)

The predictions above were checked against the next `UNAOS_SMC=1` boot. **All eight held**, and
prediction 8 — the falsifier, the one that mattered — is the reason this section exists:

```
:: SMC-DIAG: FIRST FAILURE key B0AV kind stuck step 0 t=3497ms —
   raw status timeline [48 48 48 48 48 48 48 48 48 48 48 48 48 48 48 48]
   (16 reads, ~15us apart) == evidence ::
```

Confirmed the same boot: no `AC-W` `FIRST FAILURE`; `pre-touch … raw status=0x40`; `key AC-W absent
(x2)`; `probed=19 found=17` (so **`B0Pr` is absent on this machine** — the sweep's presence key does
not exist here, and `present` falls back to "did any of soc/volt/rem answer"); no `STOP-NOTE mass
absence`; the `AC-W is absent …` line exactly once; and **zero `ac=stuck`** across 54 witness lines,
which is the finding-3 fix holding on silicon.

### Decoding `0x48`

| Bit | Name | Set? |
|---|---|---|
| `0x40` | this machine's idle-high bit (see `pre-touch raw status=0x40`) | yes |
| `0x08` | `ST_NEW_CMD` | **yes** |
| `0x04` | `ST_ACK` | **no** |
| `0x02` | `ST_BUSY` | no |
| `0x01` | `ST_DATA_READY` | no |

`0x48 & ST_MASK` = `0x08`. Step 0 waits for `ST_AFTER_CMD` = `NEW_CMD|ACK` = `0x0c`. So the
controller **latched a command and never acknowledged it** — a half-open command handshake. This is
not a dead bus (`0xff`), not idle (`0x40`), not busy. It is the SMC mid-conversation and not
answering, and it held that state for the full ~0.1 s budget *plus* the 16 sample reads.

Note what this decode does **not** settle: whose command. `NEW_CMD` asserted is equally consistent
with *we started clean and the SMC stalled on the command we just wrote* and with *`NEW_CMD` was
already set from a prior incomplete transaction*. The second would be a real hole in
`settle_before_command`, which tests only `DATA_READY|BUSY` and is therefore **blind to command-phase
residue**. The two want opposite fixes, so this arc adds the discriminator rather than guessing —
see *What changed* below.

### Verdict: a real fault, transient, fully recovered — and NOT made expected

The wedge is genuine and the DIAG was right to spend its shot on it. But it is a blip, not a standing
condition, and nothing about it is specific to `B0AV`:

* **`B0AV` recovered completely.** Across the boot it read **48 good voltages against 6 holes**
  (12796–12805 mV), the first good one 7 s later. The key is fine.
* **The sweep did not degrade.** `present=true` throughout; `soc=100%`, `full=9962mAh`,
  `rem=9962mAh` all continued. The wedged sweep produced `volt=-` and `soc=-` and BATMON-HOLD did
  its job. `retries` ran 1–8 per sweep, the same rate as every prior boot.
* **`B0AV` is incidental.** In the same sweep `BRSC` also failed — but with *short reads*
  (`Ok(1)`, the GAP-1 truncation signature, which `read_u16k` rejects and retries without ever
  producing a `Stuck`), so it never reached the DIAG. `B0AV` simply drew the first `Stuck`.

**`B0AV` is therefore NOT marked expected-anything, and must not be.** A wedge is a fault; that is
the whole distinction this arc drew against `AC-W`, whose absence was a *successful negative answer*.
Suppressing a wedge class to protect the latch would be the AC-W mistake with the sign flipped.

### The correlation worth recording

The stall lands inside the **kepler takeover / compositor-ignition window**. Reconstructing from the
serial clock (a constant 354 ms offset from `arch::ms()` on every other DIAG line this capture), the
sweep spans roughly the same interval as `kdisp: fb-draw`, `fbcon: glyphs-active`, `[wc-x]
desktop-clear`, the first `[wc-g]` window checksum — including a **`readback_us=102476`**, a 102 ms
uncached framebuffer read-back — and `[wc-x] activate`.

A coherent mechanism exists: the SMC sits behind the LPC bridge, and sustained uncached MMIO traffic
can delay an LPC completion past our 0.1 s budget, which would present as exactly this — command
latched, `ACK` late. **It is a correlation, not a demonstrated cause**, and per the standing law it
stays labelled that way until a controlled experiment separates them. The falsifiable form is cheap:
if the mechanism is real, step-0 stalls cluster in the takeover window and are rare after it.

### What changed — instrumentation only, and why no fix

**No behavioural change was made, deliberately.** The evidence base is *one wedge on one boot*, and
the driver's existing handling is already correct at every step: the wait was bounded, never forced;
the retry budget re-tried it; the failure became an honest hole rather than a fabricated value;
BATMON-HOLD kept the last good reading; and the key recovered on its own. Changing retry policy off
`n=1` is how this codebase has repeatedly acquired instruments that encode one boot's accident.

The timing does suggest the retries after a step-0 stall may be partly wasted (the witness printed
~14 ms after the DIAG, far too soon for two more full 0.1 s budgets — so attempts 2 and 3 evidently
got *past* step 0 and failed as short reads instead). That is an inference from timestamps with a
serial-latency wobble in it, and it is precisely what the new counter is there to settle. Two
additions, both read-only, no new periodic output:

* **`pre=0xNN` on the DIAG line** — the status latched immediately before the command write. On a
  `stuck step 0` this is decisive: `pre=0x40` means the SMC stalled on *our* command (fix the wait
  or accept the blip); `pre=0x48` means we wrote into command-phase residue and
  `settle_before_command` needs to cover `NEW_CMD`, not just `DATA_READY|BUSY`.
* **`step0-stalls` on the existing retry-rollup line** — the DIAG is one-shot by design, so it can
  report the boot's first wedge and nothing about whether it was one of one or one of hundreds. That
  rate is the transient-vs-standing question, and the one-shot structurally cannot answer it. The
  census rides a line that already fires at most once per `RETRY_ROLLUP_MS`.

`retries=` and the witness format are otherwise untouched, and the corroborating re-read and `TXN`
lock are unaffected by this failure mode: the re-read only guards `Absent`-of-`Present` (this was
`Stuck`), and the stall is a protocol-level stall rather than an interleave. `TXN` does mean a
wedged sweep holds the lock for its duration, so a concurrent `batmon` verb waits rather than
corrupting the transaction — bounded, and the intended trade.

### Metal prediction (next `UNAOS_SMC=1` boot)

1. `:: SMC-DIAG: FIRST FAILURE …` carries a `pre=` field. If a step-0 stall recurs, `pre` reads
   **`0x40`** (SMC-side stall on our command) or **`0x48`** (command-phase residue — then
   `settle_before_command` has a real hole and gets extended to wait out `NEW_CMD`).
2. `:: SMC-BATT: retry rollup — … (total N, step0-stalls S) ::` appears at the 300 s mark. **`S`
   small and flat** (1–3, not growing between rollups) confirms transient and closes this; `S`
   climbing with each rollup means standing, and the takeover-window correlation becomes a
   controlled experiment worth running.
3. The wedged key need **not** be `B0AV` again — the claim is that the identity is incidental. A
   different key wedging is a *confirmation*, not a new finding.
4. Everything else unchanged: `probed=19 found=17`, `B0Pr` and `AC-W` absent `(x2)`, no `ac=stuck`,
   no mass-absence STOP-NOTE, `present=true` with volt tracking ~12.8 V.

## Boot S: the discriminator answered, and it indicted the guard (s73, 2026-08-06)

```
:: SMC-DIAG: FIRST FAILURE key B0AV kind stuck step 0 t=3499ms pre=0x45 —
   raw status timeline [48 48 …] (16 reads, ~15us apart) == evidence ::
```

### `pre=0x45` — bit 0 is `ST_DATA_READY`

| Bit | Name | Set? |
|---|---|---|
| `0x40` | idle-high | yes |
| `0x08` | `ST_NEW_CMD` | no |
| `0x04` | `ST_ACK` | **yes** |
| `0x02` | `ST_BUSY` | no |
| `0x01` | **`ST_DATA_READY`** | **yes** |

Low nibble `0x05` = `DATA_READY|ACK`. **Neither predicted value came back**, and the one that did
overturns the diagnosis rather than confirming a branch of it.

The previous section predicted `pre=0x48` would mean "command-phase residue the guard is blind to,
because `settle_before_command` tests only `DATA_READY|BUSY`". But `pre=0x45` is `DATA_READY` — which
was **already in that mask**. And `pre` is sampled *after* `settle_before_command` runs. So:

> The guard was not blind. It saw the residue, drained against it for its entire ~109 ms budget,
> failed to reach idle, exited through a silent `break`, and let the command go out anyway.

Step 0 then timed out 109 ms later and the DIAG reported `stuck step 0` — blaming the step that
*inherited* the problem. The `0x48` in the timeline is the consequence (our command latching
`NEW_CMD` over an unfinished data phase), not the cause. **The cause is a residue clear that could
lose without saying so** — the same quiet-instrument disease this whole arc has been treating, this
time inside the very function written as GAP-2's defence-in-depth.

Note the drain was not lazy: at the ~15 µs pacing quantum, 109 ms is on the order of **6600 data
reads**, and `DATA_READY` still did not clear. More draining is not the answer.

### Two of my own claims are retracted

* **"Transient" is withdrawn.** `B0AV kind stuck step 0` is now **3 for 3** — Boots R, R2 and S.
* **"The key identity is incidental" is falsified.** It is *positional*. `B0AV` is the key
  immediately after `BRSC` in the sweep, and `BRSC` reads **`len=1` in all six boots** in the
  capture while its true value is two bytes (`soc=100%` requires `[00 64]`). A short read leaves the
  remainder of the value undrained with `DATA_READY` still asserted — which is precisely the
  residue the next key's settle inherits. That next key is always `B0AV`.

That is the **GAP 1 → GAP 2 chain, alive**, and now deterministic rather than intermittent: the
truncation is the root cause, the residue is the mechanism, and `B0AV` is just the address it is
delivered to. My prediction that "a different key wedging is a confirmation, not a new finding" had
it backwards — the *same* key every time is the finding.

One correction to the brief that prompted this: the three stalls are **not** all in the takeover
window. Boots R and S fired at `t≈3.5 s`, but **Boot R2 fired at `t=10882 ms`**, long after the
compositor was up. So the key correlation is 3/3 and the takeover-window correlation is only 2/3 —
which further favours the positional mechanism over the LPC-contention one. The takeover hypothesis
is not dead (it may govern *when* a truncation turns into a stall) but it is no longer the leading
explanation, and nothing in this arc depends on it.

### The fix: the guard may still lose, but it can no longer lose quietly

`settle_before_command()` now returns `Result<(), SmcError>`:

* **Idle is the test.** Ready means low nibble `== ST_CMD_DONE`. This generalizes the old
  `DATA_READY|BUSY` bit list and so *also* covers a lingering `NEW_CMD`/`ACK`, which the old test
  could not see. (The observed residue is `DATA_READY|ACK`, inside the old mask; the `NEW_CMD` arm
  is defence-in-depth, not the fix for what was measured.)
* **Bounded by bytes as well as time.** `MAX_RESIDUE_DRAIN = 64` is 2× the largest buffer any caller
  passes (the scout's 32), so it **cannot truncate a legitimate drain** — the protocol cannot produce
  more residue than one maximum-length value. What it cuts short is a non-terminating data phase:
  ~1 ms to fail instead of ~109 ms. Nothing that used to succeed can stop succeeding.
* **Loud.** A one-shot `:: SMC-DIAG: STOP-NOTE residue drain lost — status 0xNN … after N drained
  bytes; command NOT issued into it ==` names the condition, with the byte and the drain count.
* **Attributed.** It returns `Stuck(4)` (`STEP_RESIDUE`, a new step code documented on `SmcError`)
  and stores the offending status so the DIAG's `pre=` reports *the residue that defeated the drain*.
  A `Stuck(0)` will now mean what it says.
* **The command is not issued into residue.** Per-attempt cost drops from ~218 ms (109 drain +
  109 step-0) to ~1 ms, so a wedged key's three attempts cost ~3 ms instead of ~0.65 s. That also
  shortens the worst-case sweep that review finding 1 identified as the interleave enabler.

Not weakened: no protection is skipped, no wait is forced, the retry budget is unchanged, and the
outcome for a genuinely wedged key is the same honest hole — reached faster and correctly labelled.

### The census was itself mute — fixed

`step0-stalls` was added last commit to the **retry-rollup** line. That line has fired **zero times
in the entire s73 capture** — six boots, ~24 minutes of uptime. The rollup only prints when the
witness did *not*, and it resets its own clock whenever the witness fires; on this SMC the pack state
flaps often enough that `fire` lands well inside the 300 s period, starving the rollup permanently.

A counter on a line that never prints is the exact disease this arc exists to treat, and I put one
there. The counts now ride the `SMC-BATT` witness itself:

```
… ac=derived:idle retries=6/8 stall0=1 resid=1 == witness ::
```

Both are cumulative since boot. **Flat across a boot ⇒ blip; climbing ⇒ standing.** That is the
transient-vs-standing question the one-shot DIAG structurally cannot answer, now on a line that
actually prints.

### Metal prediction (next `UNAOS_SMC=1` boot)

1. `:: SMC-DIAG: STOP-NOTE residue drain lost — status 0x45 …` appears, **once**, around
   `t≈3.5 s` — the same event Boot S recorded, now named at its true cause and ~109 ms cheaper.
2. The boot's `FIRST FAILURE` becomes `key B0AV kind stuck step 4 pre=0x45` (residue), **not**
   `step 0`. If it still reads `step 0` with `pre` idle (`0x40`), the residue theory is wrong and the
   SMC really is stalling on our command — a clean falsifier either way.
3. `stall0=` and `resid=` appear on every `SMC-BATT` witness line. Expect `resid` to climb by ~1 per
   affected sweep and `stall0` to stay **low or flat**, since the command is no longer issued into
   residue. `stall0` climbing alongside `resid` would mean step-0 stalls have a second, independent
   cause.
4. `B0AV` still holes on the affected sweep and still recovers (≈12.8 V within a sweep or two); the
   fix is to the driver's honesty and cost, **not** a claim that the truncation is repaired.
5. Everything else unchanged: `probed=19 found=17`, `AC-W`/`B0Pr` absent `(x2)`, no `ac=stuck`, no
   mass-absence STOP-NOTE.

**The open root cause remains GAP 1** — `BRSC` truncating to `len=1`, six boots for six. Repairing
the value drain is a separate arc with its own evidence; this one stops the driver lying about which
step failed, and stops it paying 218 ms to do so.

## GAP 1, root-caused: `DATA_READY` clear was never a done-signal (s73, 2026-08-06)

The `BRSC` truncation that feeds the `B0AV` wedge turns out not to be about `BRSC` at all.

### The controls answer the "is it really one byte?" question first

The brief offered a disposition: if the SMC genuinely serves one byte for `BRSC`, learn it per
KEY-SHAPE like `AC-W` absence. **That is refuted by evidence already in the capture:**

* `soc=100%` has printed **200 times**. `read_u16k` only returns a value on `Ok(2)` and computes
  `(b[0] << 8) | b[1]`; `100` is `0x0064`. So the SMC serves `BRSC` as **two bytes — `[00 64]` —
  whenever it is asked for two.**

So `BRSC` is a two-byte key, the `len=1` is ours, and KEY-SHAPE must not learn it. Learning a
truncation as a fact would be the `AC-W` mistake committed against a *real* defect.

### It is not `BRSC`-specific — it is universal

| Scout observation (32-byte request, six boots) | Value |
|---|---|
| Present-reads recorded | 102 |
| Returned `len=1` | **69** |
| Returned `len=2` | 33 |
| Returned more than 2 | **0** |

`REV `, whose value is 6 bytes on the QEMU model, reads `len=1` or `len=2` on metal — never 6. So
every key on this machine is being cut short; `BRSC` is merely the one whose truncation has a
visible downstream victim, because it is the key immediately before `B0AV` in the sweep.

### Mechanism: delivered and dropped

Of the brief's three candidates — never requested / requested but not waited for / delivered and
dropped — it is the third, and the driver's own vocabulary convicts it.

The drain treated `DATA_READY == 0` as *"end-of-value signalled by the SMC"*. **That is not a signal
at all.** Everywhere else in this driver, a finished transaction is identified by the status
returning to `ST_CMD_DONE` — at the length step (`s == ST_CMD_DONE ⇒ Absent`) and in
`settle_before_command`. `DATA_READY` clear while the transaction is still **open** means the
opposite of done: the next byte has not been shifted in yet.

`pre=0x45` from Boot S is the proof, and it was already in hand: `DATA_READY|ACK`, with **`ACK` still
asserted** — the preceding read had walked away from a transaction the SMC still considered live.
The abandoned remainder is exactly the residue the next key inherits.

The second half of the 2026-07-17 GAP-1 fix is suspect for a related reason. `wait_busy_clear` exists
on the premise that the SMC raises `BUSY` during the byte shift. If it does not, that helper returns
on its **first** status read, the gap is never covered, and the fix has been **vacuous since the day
it landed** — which would explain a truncation that was declared fixed and then persisted for six
boots. The `busy=` census settles that directly.

### The fix

* **`gap_wait()` replaces the break.** A clear `DATA_READY` mid-value now asks which of three things
  happened: `More` (it came back — an inter-byte gap, read the byte), `Done` (status reached
  `CMD_DONE` — genuine end-of-value), or `StillOpen` (neither, inside budget — the truncation).
  Bounded by `SMC_GAP_CYCLES` = 16 pacing quanta ≈ 240 µs, the same window the DIAG samples over and
  ~450× shorter than the transaction budget: even if *every* read paid it in full, the 19-key scout
  would cost ~4.5 ms.
* **`close_transaction()` runs before every value read returns.** Whatever `n` came out — buffer
  full, or a gap given up on — the SMC may still be mid-value, and leaving it open is what hands the
  next key its residue. Residue is now cleared by the transaction that *created* it instead of being
  charged to the innocent key that follows. Bounded by the short gap budget; if the SMC will not
  close promptly, the next `settle_before_command` still owns it and reports `Stuck(4)`.

Neither can change what a caller receives: `close_transaction` runs after the value is in hand and
cannot alter `n`, and `gap_wait` can only *add* bytes the old loop dropped. No protection is skipped,
no wait is forced, no new command or port is touched, and the retry budget is unchanged.

### The instruments that will prove or refute it

Three counters join `stall0`/`resid` on the `SMC-BATT` witness line — the only SMC line that reliably
prints:

```
… retries=6/8 stall0=0 resid=0 unclosed=0 gap=14 busy=0 == witness ::
```

| Field | Meaning | What it decides |
|---|---|---|
| `gap` | bytes `gap_wait` recovered that the old drain dropped | **non-zero ⇒ mechanism confirmed**; zero ⇒ the bytes were never there and the cause is upstream (the length byte, or the SMC serving short) |
| `busy` | times `ST_BUSY` was ever observed set | **zero over a boot ⇒ the GAP-1 BUSY premise is false** and `wait_busy_clear` has always been a no-op here |
| `rok` | settles that started dirty and reached `CMD_DONE` — residue **seen and cleared** | separates "no residue" from "residue rescued", which `rfail` alone cannot (see Boot T) |
| `rfail` | drains that gave up (`Stuck(4)`) | renamed from `resid` |
| `unclosed` | value reads that returned with the transaction still open | should fall to ~0; high means closing is not working either |

### Metal prediction (next `UNAOS_SMC=1` boot)

1. **`SMC-SCOUT` lines get longer.** `BRSC` reads `len=2 bytes=[00 64]`; `REV ` reads more than 2
   (its true length, likely 6). The 69-of-102 `len=1` rate collapses. **This is the headline** — if
   the lengths do not change, `gap_wait` is recovering nothing and the mechanism is elsewhere.
2. **`gap` climbs and `busy` stays 0.** `gap` non-zero with `busy=0` is the complete story: the SMC
   never raises BUSY, so the old loop broke in every inter-byte gap it met.
3. **`B0AV` stops wedging.** No `FIRST FAILURE key B0AV kind stuck`, and `stall0`/`resid` stay at 0 —
   because `BRSC` no longer leaves residue for it to inherit. This is the causal claim: fix the
   truncation and the wedge disappears without ever being addressed directly.
4. `unclosed` ≈ 0. A non-zero `unclosed` with `resid=0` would mean closing is working but not
   promptly; `unclosed` high **and** `resid` high means the SMC will not close on demand at all.
5. `soc=100%` keeps printing and the `-` holes get rarer (206 holes vs 200 good in the s73 capture).
   `present=true`, volt ≈ 12.8 V, `probed=19 found=17`, `AC-W`/`B0Pr` absent `(x2)`, no `ac=stuck`.

**Falsifier for the whole arc:** `gap=0` with the `len=1` rate unchanged. That would mean the bytes
were never delivered, and the next suspect is the length byte — `read_key_inner` sends `out.len()`,
the *buffer* size, where the Apple SMC protocol wants the key's real size (which is what
`GET_KEY_INFO`, command `0x13`, exists to provide). That command is **not** issued today and adding
it would be a deliberate expansion of this driver's command set, so it waits on this boot's evidence.

### Boot T: the wedge did not form — and the counter could not say why

Boot T ran `dd2c2649` (the residue-guard commit; the GAP-1 drain fix above was not yet in it) and
chose neither predicted door. **No `FIRST FAILURE` line at all**, and `stall0=0 resid=0` throughout.

**Hypothesis (b) — "`BRSC` read clean for once" — is refuted, twice:**

* the scout still reads `key BRSC present len=1 bytes=[00]`, making the truncation **7 boots for 7**;
* the sweep's `BRSC` behaviour is unchanged. (**I first wrote a percentage here and it was
  meaningless** — see *The 50% was an artifact* below. The valid statement is the one above plus the
  retry rate.)

That is expected: `dd2c2649` changed nothing in the value drain. GAP 1 is untouched.

**`resid=0` did not mean what it looks like.** `resid` counts drains that *fail*. A drain that
succeeds was never counted, so `resid=0` is equally consistent with "there was no residue" and
"there was residue on every affected sweep and the drain cleared it every time" — opposite facts
about the machine, indistinguishable in the log. That is my own instrument repeating the exact defect
this arc exists to find. Fixed here: `rok` counts settles that started dirty and reached `CMD_DONE`,
and `resid` is renamed `rfail` so the pair reads as what it is.

**A third hypothesis, better than either offered, and now testable.** The old settle loop's exit test
was `DATA_READY|BUSY`. A status of **`0x44`** — idle-high | `ACK`, `DATA_READY` clear — passes that
test while the transaction is still **open**, so the old loop could exit *claiming success* and then
write a command into an open transaction: the step-0 wedge. (`pre=0x45` was the other old exit, the
deadline break.) `dd2c2649` replaced that test with `low nibble == CMD_DONE`, so at `0x44` the driver
now **waits** — microseconds, when the SMC closes promptly — instead of charging ahead. That would
remove the wedge *and* produce `resid=0`, because the drain now succeeds where it used to exit early
on a false settle. `rok > 0` on the next boot confirms it; `rok = 0` kills it.

**Confounds, stated plainly: this is n=1 and three variables moved at once.**

| | Boots R / R2 / S | Boot T |
|---|---|---|
| build | pre-`dd2c2649` | `dd2c2649` |
| power | **adapter, idle** (`amp=0mA`, `ac=derived:idle`, soc 100%, rem 9962) | **battery, discharging** (`amp=-2497mA`, `ac=derived:discharging`, soc 100→99, rem falling) |
| first SMC touch | `t≈1746 ms` | `t=1626 ms` |

The machine was unplugged for Boot T. The SMC is doing materially different work — real current
measurement with changing values instead of a resting pack — and the boot reached the SMC ~120 ms
earlier. **No causal claim about the wedge can be made from this boot**, in either direction. It is a
data point that the wedge is not unconditional, nothing more.

### Revised prediction (next `UNAOS_SMC=1` boot, with the GAP-1 drain fix)

Robust to the power-state confound — none of these depend on adapter vs battery:

1. **`BRSC` reads `len=2 bytes=[00 64]` in the scout**, and `REV ` reads more than 2. Unchanged
   `len=1` means `gap_wait` recovered nothing and the mechanism is upstream (the length byte — see
   the falsifier above).
2. **`gap > 0`.** This is the direct measurement of "delivered and dropped".
3. **`busy = 0`** across the boot ⇒ `wait_busy_clear` has always been a no-op here and the
   2026-07-17 GAP-1 fix was vacuous from the day it landed.
4. **`rok` disambiguates Boot T.** `rok > 0` with `rfail = 0` means residue is real and being
   rescued — which retroactively explains Boot T's `resid=0`. `rok = 0` **and** `gap > 0` would mean
   `close_transaction` is now clearing residue at the source, before settle ever sees it (the
   intended outcome).
5. `unc ≈ 0`; **`short` falls sharply** — that counter, not any `soc` percentage, is the cleanest
   single number for whether GAP 1 is actually fixed.
6. A `B0AV` wedge may or may not reappear. **It is no longer the arc's success criterion**: Boot T
   showed the wedge can vanish without the truncation being touched, so only the `len=` and `gap=`
   evidence can settle GAP 1.

### The Boot T sit: abundant short reads, and two instruments that lied about them

The sit kept running and the per-key holes are frequent — `soc=-%`, `rem=-mAh`, `full=-mAh`,
`volt=-mV`, `amp=-mA` — with `retries` climbing 3 → 433 over 150 s while `stall0=0 resid=0`
throughout. Two conclusions were drawn from that shape, and **both were instrument artifacts.**

#### The "sweep-by-sweep alternation" is the fire condition, not the SMC

`soc` appears to alternate `-` / `99` line after line. It does not. `soc_pct` is part of the
`LAST_STATE` quiet-witness key, so the witness fires on **both edges** of a drop — once when `soc`
goes missing and once when it returns — and stays silent through every sweep in between. The gaps
between consecutive witness lines say it plainly:

```
13s 1s 18s 1s 5s 1s 1s 1s 1s 1s 11s 1s 6s 1s 2s 1s 21s 1s 19s 2s 11s 1s 18s 1s 6s 1s 2s
```

Long gap, then 1 s. Those are **drop + recovery pairs** separated by 5–21 s stretches in which the
key read cleanly every second and nothing printed. There is no square wave, and no periodicity to
chase — the apparent regularity is edge-triggering.

#### The 50% was an artifact — mine

The previous section reported "`soc` good on 50% / 49% / 51% / 54% of witness lines" as if it were a
truncation rate. **It is not a rate at all.** Because the witness fires on both edges of every drop,
good and missing lines are ~50/50 *by construction*, whatever the SMC is doing. Counting an
edge-triggered log and reporting it as a frequency is precisely the error this arc keeps finding in
other people's instruments; I published it, and it is withdrawn.

The cumulative `retries` counter is *not* edge-triggered and does give a real figure: **433 retries
over 150 s ≈ 2.9/s**, against ~1 sweep/s and ~5 read keys per sweep — roughly 0.58 retries per key
read. So the coordinator's "abundant" is right; the evidence for it is `retries`, not `soc` edges.

Neither of those is a direct measurement, so this arc adds one. **`short`** counts sweep-path reads
that returned `Ok(n)` with `n != 2` from a two-byte request — the truncation itself, counted where it
happens, independent of any print policy.

#### `resid=0` still cannot mean "no residue"

The proposed discriminator — *the keys drop out without producing residue, or the residue guard would
be counting it* — does not hold yet, for the reason the previous section already flagged: `resid`
(now `rfail`) counts drains that **fail**. A drain that succeeds returns `Ok` and is counted nowhere,
so `rfail=0` is exactly as consistent with "residue on every affected sweep, cleared every time" as
with "no residue". The two are opposite facts and the log cannot yet separate them.

`rok` — added here — is what makes that discriminator real. Until a boot reports it, "short reads and
residue are different mechanisms" is a hypothesis, not a finding. It remains a *good* hypothesis: a
short read that stops one byte early leaves one byte, and a single leftover byte is exactly what a
settle drain would clear silently and instantly.

## Boot U: GAP 1 root-caused and fixed on metal (s73, 2026-08-06, kernel `3477640c` from `7814d258`)

> This section was written as "GAP 1 CLOSED" and an adversarial review (GR18) refuted the *closure*
> while confirming the *mechanism*. The claims below are corrected in place; the review round and
> what it changed in the driver are at the end of the section. Short version: **GAP 1 is root-caused
> and fixed in `read_key_inner`. It was not closed — the sibling path `read_key_by_index` still ran
> the old drain, and this boot's own log carries the new failure line that proves it.**

One boot, every prediction from the table above answered, all in the fix's favor:

```
:: SMC-SCOUT: key REV  present len=6 bytes=[02 03 0f 00 00 36] ::
:: SMC-SCOUT: key OSK0 present len=32 bytes=[6f 75 …] ::
:: SMC-SCOUT: key BRSC present len=2 bytes=[00 60] ::
:: SMC-BATT: present=true soc=96% volt=12353mV amp=-2525mA full=9962mAh rem=9571mAh
   ac=derived:discharging retries=0/0 st0=0 rfail=0 rok=1 short=0 unc=0 gap=39 busy=0 == witness ::
```

1. **The scout lines got longer — the headline held.** `BRSC len=2 [00 60]` (0x60 = 96, equal to the
   live `soc=96%`; the table's `[00 64]` was written when the pack read 100%). `REV` reads its full
   **6 bytes** — the QEMU model's length, now confirmed on silicon. `OSK0` reads **32 bytes whole**.
   The `len=1` census: **2 of 17 present reads** (only a present read carries a `len=`; the earlier
   "2 of 24" counted the probe list, not the answers). Both are `BNum` and `BSIn`. Note what that
   last step is and is not: `PROBE_KEYS` is name-plus-description, **this repo has no declared
   key-size table**, and `GET_KEY_INFO` (0x13) — the one instrument that would answer it — is
   deliberately unissued. "Both are genuinely one-byte keys" is therefore Apple-SMC background
   knowledge, not a measurement made here. The measured statement is the census: from
   69-of-102-truncated (six boots, ~11.5 of 17 per boot) to 2-of-17 in one boot.
2. **`gap=39` with `busy=0`.** `gap_wait` recovered 39 bytes the old drain would have dropped, and
   `ST_BUSY` was never once observed set. **The 2026-07-17 `wait_busy_clear` fix is proven vacuous on
   this hardware** — and that conviction is *qualified*, not a fallthrough zero: `gap=39` proves the
   drain-loop body ran ≥39 times and `wait_busy_clear(3)` is its first statement, so the helper was
   called ≥39 times and returned on its first status read every time.
   The **stronger** sentence this section used to carry — "the SMC never raises BUSY on this
   machine" — was more than the instrument could see when it was written. `wait_busy_clear` samples
   BUSY once, at the *start* of a gap; the premise under test is that BUSY appears *during* the
   shift, and `gap_wait` used to spin that whole window with no census at all. GR18 fixes the
   instrument rather than the sentence: `gap_wait`'s poll now ORs its BUSY sightings into the same
   counter, so from the next boot on `busy=0` covers the entire gap window and the strong sentence
   becomes sayable. On Boot U's build it is not yet — read Boot U's `busy=0` as *"BUSY was never set
   at the start of an inter-byte gap"*.
3. **The B0AV wedge did not reappear.** No `FIRST FAILURE` of any kind, `st0=0 rfail=0`. That is
   **consistent with** the causal claim (fix the truncation and the wedge starves); it does not
   confirm it. `7814d258` disqualified the wedge's absence as a success criterion in its own
   prediction — "a B0AV wedge may or may not reappear and is no longer the success criterion",
   because Boot T showed the wedge vanishing with the truncation untouched — and a criterion
   disqualified one boot cannot be re-qualified as confirmation the next. What settles GAP 1 is
   `len=` and `gap=`, and those are what settled it.
4. **`unc=0`** — closing works, promptly. `rok=1`: exactly one settle started dirty and was rescued,
   so residue was seen, cleared, and *counted* this time. But see the review note below on what
   created that residue, and on why `unc=0` was **not** evidence that no read was truncated.
5. **`retries=0/0`** against the pre-fix sit's cumulative ≈0.58 retries per key read — a fair
   like-for-like: Boot T read `retries=3/3` at the same phase (1749 ms), 53 by 33.7 s, 433 by 150 s,
   in the same unplugged/discharging power state. Boot U held 0/0 through 40.5 s. Coverage caveat:
   the witness is edge-triggered and fired twice, so the closure rests on ~40 s of counter coverage,
   not the hours a standing sit delivers.

**Cost of the gap budget, priced correctly.** `SMC_GAP_CYCLES` is 16 × 35 000 cycles = **~208 µs**
at this machine's measured 2 693 855 654 Hz (BPACE) — and it is a **per-gap** budget, not per-read:
`gap_wait` can be entered once per byte, and `close_transaction` adds one more full window per read.
Worst case per read is `(out.len() + 1) × 208 µs` → **~6.9 ms** for a 32-byte scout read and
**≈130 ms** for the 19-key scout. `7814d258`'s "~4.5 ms for the scout" priced one gap per read and
understates it by ~29x; do not quote it forward. The bound is for a hostile SMC that makes every
byte boundary pay in full — **observed cost on Boot U was nil**: the scout ran 1743→1748 ms and
`gui=3408ms` sits inside the 3407/3410 T/T2 band.

Boot health around it: `gui=3408ms` (T/T2 band: 3407/3410 — no regression), `kepler=1522ms`,
`sched d=67ms`, `ehci-hid-done d=1450ms`. **One new line, and it matters:**

```
:: SMC-SCOUT: index enumeration STOP-NOTE at idx 0 — handshake wedged at step 3 (bounded, not forced; Caveat 3) ::
:: SMC-SCOUT: index walk done (0 of 493 names) ::
```

This section originally claimed "zero new FAIL/TRIPWIRE lines". **That was false**, and the line it
missed is the fix's own sibling defect — see below.

Status: **n=1** on the fix build. `GET_KEY_INFO 0x13` stays unissued — the falsifier door it guarded
never opened.

### Adversarial review round (GR18) — what the closure actually covers

An adversarial pass re-derived Boot U's evidence independently. **The mechanism survives intact**:
`REV` 2→6, `OSK0` 2→32, `#KEY` 1→4, `BRSC` 1→2 on the same build minus the drain fix, at the same
boot phase and power state; values cross-validate across three independent keys to 0.1 %
(`9571/9962 = 96.07 %` against `soc=96%`; `9538/9962 = 95.74 %` against `soc=95%` at 40.5 s — a
one-byte shift in any of them moves the value by ≥256x). "A clear `DATA_READY` was never a
done-signal" is correct and `gap_wait` fixes it.

What the review narrowed, and what this change does about it:

* **The closure covered `read_key_inner` only.** `read_key_by_index` still ran the *pre-fix* drain —
  four name bytes, each aborting `Stuck(3)` the instant `DATA_READY` read clear — i.e. GAP 1
  verbatim, live in the sibling path. Worse, the fix *opened the door onto it*: `#KEY` used to
  truncate to `len=1 [00]`, so `count=0` and the walk never ran (Boot T: `index walk done (0 of 0
  names)`); un-truncated it reads `[00 00 01 ed]` = 493 and the walk runs straight into the unfixed
  code. That is the STOP-NOTE above. **Fixed here**: the name drain waits through `gap_wait()` like
  the value drain, and `Gap::Done` (a name shorter than 4 bytes — a protocol error) and an expired
  gap budget stay `Stuck(3)`.
* **The old path also leaked an open transaction**, which is the residue-charged-to-the-next-key
  defect `close_transaction` was written to end. The log fingerprints it: `rok=1` — exactly one
  dirty settle in the whole boot — sitting between the abandoned walk at 1748 ms and the next
  transaction (`AC-W`) at 1749 ms. **Fixed here**: every exit of `read_key_by_index` — `Ok`,
  `Absent`, every `Err` — now returns through `close_transaction()`. (The pre-command
  `settle_before_command()` stays outside that wrapper: if it fails, no command of ours was written
  and there is no transaction of ours to close.) The equivalent gap on `read_key_inner`'s own error
  paths remains open and is not in this change's scope.
* **`Gap::StillOpen` was unmeasured.** `close_transaction` reported only when it *failed* to close.
  If the delayed byte arrived a moment after `gap_wait` gave up, the drain read it, discarded it,
  reached `CMD_DONE` and incremented nothing — so `unc=0` did **not** mean "no truncation", and
  "every key whole" rested entirely on the `len=` census against sizes this repo does not declare.
  **Fixed here**: `late=` counts every byte the drain discards. The two arms now partition
  `StillOpen` — the late byte arrives ⇒ `late`, it never arrives and the SMC will not close ⇒ `unc`
  — so `late=0 unc=0` over a boot makes "every read ended where the SMC said it ended" a
  measurement. (`late` also counts the benign case of a key longer than the caller's buffer; the two
  are told apart by which key was read.)
* **Caveat 3 is stale as a statement about the SMC.** "`#KEY` enumeration is a standing bounded
  wedge on this machine" described our own drain bug in un-fixed code, both times it was observed.
  The bound itself (`SMC_WAIT_CYCLES` + `MAX_ENUM_KEYS`) is a protection and stays exactly as is;
  what changes is the attribution.
* **The load-bearing arm has no automated coverage.** QEMU's `isa-applesmc` holds `DATA_READY`
  across all `len` bytes and never inserts an inter-byte gap, so on QEMU the drain reaches
  `gap_wait` exactly once — after the last byte, with the status already `CMD_DONE` → `Gap::Done` →
  the same `break` the old code took. `Gap::More` (**the entire fix**), `Gap::StillOpen`, and
  `close_transaction`'s drain loop are **never taken on QEMU**. `#KEY` is absent there too, so the
  enumeration walk fixed above does not run either. `./arroyo check` plus the cfg legs give
  compile-and-link coverage; **the GAP-1 fix has no regression test anywhere except a metal boot**,
  and a revert would show up only as `gap=` falling to 0 with `len=1` returning.

### Prediction for the next metal boot (GR18 build)

1. The `#KEY` walk **proceeds past idx 0** — names enumerate, `:: SMC-SCOUT: idx 0 = … ::` lines
   appear, and the walk's own cap (`MAX_ENUM_KEYS = 512` against `count=493`) governs where it
   stops, not a handshake.
2. `index enumeration STOP-NOTE at idx 0` **disappears, or moves** to a later index.
3. `late=` appears on the SMC-BATT witness (appended after `busy=`), expected **~0**.
4. `gap` may rise, possibly sharply and early, as the walk's ~2000 name bytes pay the same census as
   value bytes — a single boot-time step, then the slow sweep-driven climb as before.
5. `short=0` and `busy=0` hold. `busy=0` now covers the whole gap window, not just its first poll.

**Falsifier:** the walk still wedges at idx 0 *with the gap budget expiring* (`Stuck(3)` after a
full ~208 µs window). That would mean the name phase genuinely stalls longer than the budget rather
than tripping over a short poll, and the suspicion returns to `GET_KEY_INFO`/pacing — the door
`7814d258` named and Boot U left shut.

## Boot W: the steady state, measured over a long sit (s73, 2026-08-06, kernel `7748d22c` @ `68370d6f`)

The first capture that carries a *sit* behind the GR18 fix rather than a boot alone: five sweep
witnesses across ~12 minutes of uptime. The predictions above are answered, and the answers are
dull in the way a fixed driver should be.

**The walk ran, and it ran quiet.** `#KEY count=493` enumerates in full, at a cost of ~99 ms:

```
[   1743ms] :: SMC-SCOUT: #KEY count=493 — walking index list ::
[   1842ms] :: SMC-SCOUT: index walk done (493 of 493 names) ::
[   1842ms] :: SMC-SCOUT: end (present=Y probed=19 found=17) == witness ::
```

`UNAOS_SMCWALK` is back to its default-off (WALK-QUIET), so the 493 per-name lines are not
emitted. That is not cosmetic: those lines are serial bytes on the same link the boot's tail is
measured through, and Boot V paid `SPACE … wait=1553ms … ftdi=1519ms` for them. Boot W reads
`wait=209ms … tur=994ms … ftdi=177ms` — Boot U's shape restored. **The walk is not the cost;
printing it was.** The knob is there for when the names are wanted.

**The sit: `gap` climbs, and nothing else moves.**

```
[   1843ms] :: SMC-BATT: … retries=0/0 st0=0 rfail=0 rok=0 short=0 unc=0 gap=976  busy=30 late=0 == witness ::
[ 198939ms] :: SMC-BATT: … retries=0/0 st0=0 rfail=0 rok=0 short=0 unc=0 gap=2312 busy=49 late=0 == witness ::
[ 578899ms] :: SMC-BATT: … retries=0/0 st0=0 rfail=0 rok=0 short=0 unc=0 gap=4163 busy=49 late=0 == witness ::
[ 702886ms] :: SMC-BATT: … retries=0/0 st0=0 rfail=0 rok=0 short=0 unc=0 gap=4773 busy=49 late=0 == witness ::
```

`gap` rises monotonically — 2312 → 4163 over ~10 minutes, 4773 by the last witness line — which
is `gap_wait` doing steady work: every sweep recovers bytes the pre-fix drain would have dropped
on the floor. Prediction 4 called exactly that shape, a boot-time step followed by a slow
sweep-driven climb, and this is the first capture long enough to show the climb rather than the
step.

**`busy=49` is STABLE across the whole sit — and that is worth stating as a finding.** The
boot-time line reads `busy=30` at 1843 ms; the counter reaches 49 during the rest of bring-up and
then does not increment once in the following ~11.5 minutes. **All 49 `ST_BUSY` sightings are
boot-time. A steady-state sweep never sees BUSY at all** — BUSY on this SMC appears only under
boot-time load, which is consistent with Boot U's `busy=0` (a boot short enough not to reach it)
and refines prediction 5 rather than contradicting it.

**`late=0 short=0 st0=0 rfail=0 unc=0 retries=0/0` on every line, boot and sit.** `late` and
`unc` are the two arms the GR18 round built to partition `Gap::StillOpen`, so `late=0 unc=0`
sustained across the full capture is the measured statement that **every read ended where the SMC
said it ended** — not a fallthrough zero, since `gap` is simultaneously non-zero and climbing,
which proves the drain-loop body those counters live in ran thousands of times. `rok=0`
throughout is the other half: the open-transaction leak the review found in `read_key_by_index`
(Boot U's single `rok=1`) does not recur, so `close_transaction` is covering every exit.

**The sibling fix's steady state is clean.** Nothing in this capture is a falsifier, and the
walk-wedge falsifier above did not fire: the walk completed 493 of 493.
