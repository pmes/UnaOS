# BATMON-1 — Apple SMC battery monitor (x86, `UNAOS_SMC=1`)

Status: M1 code-complete + QEMU-gated; M2 read/render path landed (provisional key set);
M3 (metal tracking) is the attended 2012 rMBP sitting, not a build gate.

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
4. Read value bytes from **0x300** while `DATA_READY` holds; the SMC clears it after the last byte.

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

## Metal-pending (the M3 sitting — Peter's, not a build gate)

Assertable at the attended 2012 rMBP sitting (`UNAOS_SMC=1` media):

1. `:: SMC-SCOUT: key REV present … ::` on real silicon (protocol works on the metal SMC).
2. The `SMC-SCOUT` battery block: which of the curated keys the real SMC carries + their payloads —
   **the machine's true battery inventory** (records the M2 key set + the per-cell fork verdict).
3. `#KEY` present ⇒ the index walk emits the full key list.
4. `:: SMC-BATT: present=true soc=… volt=… ::` tracks reality: **unplug ⇒ discharge (amp < 0, soc
   falls), plug ⇒ charge (amp > 0)**; the on-screen "BATT" bar follows.
5. No handshake STOP-NOTE on the metal SMC (bounded waits sized correctly for real timing).
