# CORE3 probe bench card

Instrumented Pi 4 build to resolve the **CORE3-SMP** regression: on real BCM2711, once
`kernel8.img` crosses **1 MiB**, core 3 comes up as a phantom "core 0" (`__secondary_rust(0)`)
and `CORE_READY[3]` never sets. Static analysis (see `docs/dev/OS/01_BOOT_HAL/arch_arm64.md`
§CORE3-SMP) ruled out every kernel-side hypothesis and left three surviving mechanisms. This
probe disambiguates them by dumping each core's raw state to the PL011 **before any Rust runs**
(MMU off, no stack, physical addressing).

## What the probe emits

Added under the `core3probe` cargo feature to the secondary entry stub `_secondary_start`
(`crates/kernel/src/arch/aarch64/smp.rs`). For **each** released core, as its very first
instructions, it writes one record to the PL011 data register (`0xFE201000`), polling the FR
TXFF bit (`base+0x18`, bit 5) with a bounded spin so no core can wedge another:

```
[<mm>E<e>X<x>]
```

- `<mm>` — `MPIDR_EL1` low byte (Aff0), 2 hex nibbles (uppercase A-F).
- `<e>`  — `CurrentEL` field (`CurrentEL[3:2]`), one digit. Expected `2` (secondaries enter EL2).
- `<x>`  — arrival `x0` low nibble. Spin-table protocol delivers `x0..x3 = 0`, so expected `0`.

Records from cores 1/2/3 may **interleave** char-by-char (three cores race the same UART); the
bracket + hex fields let you reassemble them. Cores come up in any order.

## Healthy signature (QEMU `raspi4b`, all 4 cores up)

QEMU never reproduces the metal fault, so it is the byte-good reference. Cores 1, 2, 3 each
emit their own correct id:

```
[01E2X0][02E2X0][03E2X0]      (order/interleave may vary)
```

i.e. one `[..E2X0]` per secondary with `mm` = `01`, `02`, `03`. These print **before** the
normal `:: AARCH64 SMP: core N online ::` lines.

## Build the instrumented image (attended Pi bench only)

```sh
cd unaos
UNAOS_CORE3PROBE=1 UNAOS_PI=1 ./arroyo kernel8
# writes target/kernel8/kernel8.img  (must be > 1 MiB — verify:)
wc -c target/kernel8/kernel8.img
```

The image must exceed **1 MiB** (`0x100000` loaded, i.e. `> 0x80000` bytes on disk since it
loads at `0x80000`) to exercise the failing regime. The probe code itself enlarges the image;
if for any reason it lands under 1 MiB, that build does NOT reproduce the fault — report the
size rather than proceeding.

Flash to the **16 GB `UNAOS`** card (the 31 MB `UNAOSRW` card is EEPROM-refused), then do **one
cold boot** and capture the **full serial** over the Debug Probe (unmount `UNAOS` first; the
`kernel8` build refuses a mounted card). See the Pi resume runbook for the rig.

## Decision table

Read core 3's record (the `[..E2..]` whose `mm` is `03`, OR the anomalous one that appears where
core 3's should):

| Core-3 probe record | Meaning | Cause implicated |
|---|---|---|
| `[03E2X0]` printed, but later `core 0 online` phantom / no `CORE_READY[3]` | Core 3 **arrived correctly** (MPIDR read 3 at EL2, right entry, x0=0); the id-0 divergence happens **after** the stub, inside `__secondary_rust`/the x0 spill | Rust-side / register-spill — re-examine `drop_to_el1` x0 preservation and the `core_raw` path |
| `[00E2X0]` where core 3's record should be (no `03` record at all) | Core 3 entered the stub but `MPIDR_EL1` **genuinely reads 0** | GPU-firmware / armstub delivery (wrong affinity presented) or an L2-boundary uarch effect on the MPIDR read |
| **No** core-3 record at all (only `01`/`02`), yet later a phantom `core 0 online` runs | Core 3 never reached `_secondary_start`'s first instruction, or branched to a **wrong target** and only re-entered Rust later as id 0 | Wrong branch target / firmware released core 3 to a stale/incorrect entry (delivery below the kernel) |
| `[03E<e≠2>X..]` or `[03E2X<x≠0>]` | Core 3 arrived with an unexpected EL or non-zero x0 | Firmware delivery differs for the last core — compare against cores 1/2 |

Cross-check `mm` against the phantom "core 0 online": the static analysis says that phantom line
**is** physical core 3, so the probe's `mm` field is the ground truth for which physical core
produced it — MPIDR read at EL2 cannot lie the way the post-`and #0xff` id can.

## Knob-off guarantee

`core3probe` is default OFF and gates a *separate* copy of the stub; with it off the secondary
stub is byte-identical to trunk. Verified: knob-off `./arroyo kernel8-test` battery unchanged.
