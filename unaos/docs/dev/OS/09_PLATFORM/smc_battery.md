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
`:: SMC-BATT: … ::` witness (once on the first read, then on the cadence when a battery is present),
and `battery::cached()` feeds the on-screen meter. The meter is rendered by the vug meter surface
(`vug::draw_meters`, the "BATT" bar + readout) and — because the serial-less metal debug view mirrors
serial to the framebuffer — the `SMC-BATT` line is also on-screen in the `UNAOS_USBDEBUG` boot Peter
photographs at the sitting.

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
