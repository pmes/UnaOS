# AHCI — the SATA host controller, read-only first rung

`unaos/crates/kernel/src/drivers/ahci.rs`, knob `UNAOS_AHCI=1`, Cargo feature `ahci`, default OFF.

rmbp-ledger **B89** states the defect this module answers: *the rMBP tells us at every boot that it
has both disks he wants, and the OS binds to neither.* Every PCI census this project has taken on
that machine prints

```
[PCI-STOR] bdf 0:31.2 8086:1e03 class=01 sub=06 progif=01 (sata) bar0=0x3080
```

— the Intel 7-series AHCI controller carrying the internal SSD — and nothing in the tree could talk
to it, because there was no SATA driver at all. The loader can already boot from an internal FAT
partition; the kernel then cannot find its root, because `fs/bootdisk.rs` walks block sources and
there was never a source on that controller.

This is the **first rung**: enumerate, identify, read, publish. Nothing here writes.

---

## SCOPE

### What it does

1. Finds the controller by **PCI class** — 0x01 mass-storage / 0x06 SATA / progif 0x01 — never by
   bdf. LAWS §3 forbids a bus or slot literal in kernel source, and the same walk therefore finds
   the controller on the bench rMBP, on QEMU's q35, and on anything else.
2. Takes it from firmware through the **BIOS/OS handoff** (`CAP2.BOH` / `BOHC`) when the controller
   advertises one.
3. Maps **ABAR**, which is **BAR5** (config offset 0x24), uncacheable. Not BAR0: on the bench rMBP
   BAR0 is an **I/O** BAR at `0x3080` — the legacy IDE task-file window the same function also
   decodes — and a driver that read BAR0 would map sixteen bytes of I/O space as if it were a
   register block. AHCI 1.3.1 §2.1.11 places the AHCI register block at BAR5 and nowhere else.
4. Reads `CAP` / `CAP2` / `PI` / `VS`, sets `GHC.AE`, and for every implemented port reporting a
   **plain SATA disk** (`PxSSTS.DET == 3` **and** `PxSIG == 0x0000_0101`) brings the port up with the
   ClearBusy discipline, runs `IDENTIFY DEVICE` (0xEC), reads LBA 0 with `READ DMA EXT` (0x25), and
   publishes the disk into the AHCI registry in `drivers/block.rs`.

### What it does NOT do — 1: it never writes to the disk

There is **no WRITE command word anywhere in this image**. The two ATA opcodes the file can issue are
`IDENTIFY DEVICE` (0xEC) and `READ DMA EXT` (0x25); `WRITE DMA EXT` (0x35) and every other writing
opcode are absent, so an armed build carries no code path that could mutate a sector even if
something above it asked. `block::write_block_ahci` is a refusing stub with a one-shot witness — the
same shape, and for the same reason, as the pre-`sdw` `write_block_sdhc`.

Read-only here is a **property of the image**, not a policy someone can configure away. Writes are
the next arc.

### What it does NOT do — 2: the installer never sees this device

`install/` is not touched by this arc and must not learn the handle exists until the write arc lands.
The reason is rmbp-ledger **B91**, which states the ordering constraint in as many words: the
installer's only guard protects HOME and not STRANGERS, and on this machine *the stranger is
Catalina, safe today only because we cannot see her disk*. B91 requires the stranger guard to land
**before** the AHCI driver.

This arc satisfies that constraint by being read-only and by publishing into a registry no installer
path can name: `register_ahci` never touches the global `BLOCK_DEVICE`, there is no `BlockHandle`
variant for `install/mod.rs` to dispatch on, and the only entry points are reads. A driver that
cannot write and that no write path can reach cannot erase anything.

### What it does NOT do — 3: no interrupts, and no service-pass cost

`GHC.IE` and every `PxIE` stay 0. Completion is polled on `PxCI`/`PxIS` against a TSC deadline —
`arch::ms()` is unusable here because the APIC tick does not advance with interrupts masked and this
runs inside `pci::init`. Everything happens in **one pass at enumeration time**, so no service loop
gains a call site and no input band gains a lock. The `unaos/crates/kernel/src/main.rs` pump call
site the brief allowed for was therefore **not needed and not added**.

---

## Registers touched

**Generic host control** (ABAR + offset): `CAP` 0x00, `GHC` 0x04 (AE and HR bits), `IS` 0x08,
`PI` 0x0C, `VS` 0x10, `CAP2` 0x24, `BOHC` 0x28.

**Per port**, at `0x100 + port * 0x80`: `PxCLB`/`PxCLBU` 0x00/0x04, `PxFB`/`PxFBU` 0x08/0x0C,
`PxIS` 0x10, `PxIE` 0x14 (written 0 only), `PxCMD` 0x18, `PxTFD` 0x20, `PxSIG` 0x24, `PxSSTS` 0x28,
`PxSERR` 0x30, `PxSACT` 0x34, `PxCI` 0x38.

`GHC.HR` (HBA Reset) is **never issued**: it would throw away whatever state firmware left on ports
this driver is not claiming.

**Sources: the AHCI 1.3.1 specification and the Serial ATA specification.** Cleanroom — no driver
code from any other operating system was consulted.

---

## DMA structures

Per brought-up port, allocated once from the kernel heap and never freed (the HBA keeps DMAing into
the FIS receive area for as long as `PxCMD.FRE` is set, and this arc has no port teardown):

| Structure | Size | Alignment | Spec |
|---|---|---|---|
| Command list (32 headers x 32 bytes) | 1024 | 1024 | AHCI 1.3.1 §3.3.1 |
| FIS receive area | 256 | 256 | AHCI 1.3.1 §3.3.3 |
| Command table (CFIS + ACMD + 1 PRDT entry) | 256 | 128 | AHCI 1.3.1 §4.2.3 |
| Single-sector landing buffer | 512 | 4096 | — |

Heap addresses are used directly as bus addresses. That is this kernel's standing x86 property, not
an assumption this driver introduces — every DMA structure in `drivers/xhci/mod.rs` (DCBAA, the
scratchpad array, every ring, every BOT staging buffer) is programmed into its controller the same
way, because the identity map means VA == PA. `ahci::bus_addr` is the single place that conversion
happens, so a future aarch64 arm has exactly one function to change.

Only **command slot 0** is ever used, and `read_block_at` holds the port lock across the whole
command. With one slot in play that serialisation is a correctness requirement, not an optimisation.

---

## The witness lines

```
:: AHCI: port=<p> model="<m>" sectors=<n> lba48=<0|1> ::
:: AHCI: port=<p> sector0 sig=<hex> kind=MBR|GPT|none ::
:: AHCI: registered port=<p> as registry index <i> — blocks=<n> (<m> MiB) READ-ONLY ... ::
:: AHCI: selfcheck port=<p> identify=<ok|bad> sector0=<GPT|MBR|none> -> PASS|FAIL ::
```

plus a `[ahci]` diagnostic family (the bdf/BAR5/command-register claim line, the handoff line, the
HBA capability line, per-port refusals naming the register that convicted them, and the closing
`done:` census).

**The selfcheck line is printed only for a port that reached IDENTIFY.** It can execute in every
state it reports on, so its silence means "no SATA disk answered", never "the check passed" — LAWS
§5, *an absence is evidence only if the producing path ran*. A `-> FAIL` on it reds any spec replay,
because `mbench.py` installs `-> FAIL` into `DEFAULT_FORBIDS` for every spec; that is what makes the
go-red mutation (corrupt the `PxSIG` gate or the IDENTIFY decode) a **measured** leg rather than a
read of the source.

Every witness token exceeds 8 bytes (`:: AHCI: port=` is 14), so `LC_ALL=C grep -a -o -F` can certify
them in the built artifact rather than in the diff.

---

## The QEMU fixture

q35's ICH9 has an AHCI controller built in, and the ESP already rides it: `ide-hd,drive=esp,
bootindex=0` on `ide.0`. `UNAOS_AHCI_DISK=<path>` attaches a **second** disk on `ide.1` — an explicit
bus, not the first-free default, so the ESP's port assignment cannot move under it — with **no
`bootindex`**, so OVMF keeps booting the ESP exactly as it does today. Unset, not one argument is
added and a default run's QEMU command line is byte-identical to what it was before this arc.

The fixture the DONE gate uses is `builder/fat-gpt.img`, built by `scripts/make-fat-img.sh gpt`: a
GPT-partitioned FAT32 disk, so the sector-0 witness reads `kind=GPT` off a real protective MBR rather
than off a synthetic one.

```
bash unaos/scripts/make-fat-img.sh gpt
UNAOS_AHCI=1 UNAOS_AHCI_DISK=builder/fat-gpt.img UNAOS_QEMU_FULL=1 ./arroyo test 90
```

---

## Byte identity, knob off

* `drivers/ahci.rs` is **not lexed at all** — the `pub mod` declaration is `#[cfg]`-erased, which
  LAWS §5 names as the one byte-safe case, and it is declared **last** in `drivers/mod.rs` so the
  line numbering of every module above it is untouched.
* The AHCI registry in `drivers/block.rs` is a **file-tail append**: nothing above it moves, so no
  `core::panic::Location` in that file changes.
* The call site in `arch/x86_64/pci.rs` is a **LINE-NEUTRAL append** onto the closing brace of the
  existing SDHC block, before that line's first `//` (LEDGER P7).
* `arroyo`'s `arm_features` strips `ahci` from aarch64 media, so the feature cannot shift an aarch64
  `-Cmetadata` fingerprint either way.

---

## ⚠ What this rung does NOT yet reach: `fs::bootdisk`

There is **no `BlockHandle::Ahci` and no `fs::fat::BlockSource::Ahci`**, so `fs/bootdisk.rs`'s walk
cannot see these disks yet. The driver identifies the disk and reads its sectors; the block layer
publishes it; the filesystem layer has no name for it.

That is a scope boundary, not an oversight. Adding the `BlockHandle` variant makes four **exhaustive
matches outside `drivers/block.rs`** non-exhaustive, and one of the files is the one this arc is
forbidden to touch:

| File | Arms needed |
|---|---|
| `install/mod.rs` | 2 (`read_at`, `write_at`) — `Err(BlockError::NotReady)`, the `Sdhc`/`TegraSd` shape. **Forbidden by this brief (B91).** |
| `wifi/firmware.rs` | 1 — handle → `BlockSource` |
| `fs/unafs.rs` | 4 — `handle_info`, `handle_read`, `handle_write`, `SdSectorDevice::open_on` |
| `fs/fat.rs` | `BlockSource::Ahci` + `name()` + `write_veto()` + 4 dispatch arms |
| `fs/bootdisk.rs` | the `presence()` row and the walk's `admit` |

The block-layer entry points that landed in this arc (`ahci_info_ix`, `read_block_ahci_ix`,
`read_blocks_ahci_ix`, `write_block_ahci`) **are** the seam those arms would dispatch to, so the
follow-on is mechanical once the ordering B91 fixes allows it.

---

## Next arc

1. **Writes.** `WRITE DMA EXT` (0x35) behind its own knob, with the same refuse-by-default posture
   `sdw` established for the SD path.
2. **The `BlockHandle`/`BlockSource` plumbing** above, so `fs::bootdisk` can bind a root by content
   on a SATA volume — which is what makes "UnaOS on the internal SSD, booting from it" reachable.
3. **A partition-mode installer.** B91's stranger guard first, then an installer that can target a
   free partition without going near the volume Catalina lives on.
