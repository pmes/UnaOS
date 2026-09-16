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

## The bootdisk seam — LANDED (AHCIBOOT, B89's second rung)

`BlockHandle::Ahci { port }` and `fs::fat::BlockSource::Ahci(port)` exist, so `fs/bootdisk.rs`'s walk
sees these disks and a root volume on an internal SATA disk is **found by content**. Still read-only:
no write path, no installer awareness.

### One key, carried by both enums

Both carry the **HBA PORT** — the key `drivers::ahci`'s `PI` mask handed us and the key `AhciDisk` is
published under. `fat::handle_of` / `fat::source_of` is therefore a lossless bijection, and a
`PartitionRange` or a `BlockDeviceId` built on a SATA volume names the right DISK, not merely the
right handle. That is the one structural difference from `BlockSource::UsbN(n)`, whose number is a
registry INDEX the handle cannot carry (see `fs/fat.rs`'s `UsbN` doc for what that costs there).

The port becomes a registry index in exactly one function, `block::ahci_ix_of_port`, and the
`_port` entry points (`ahci_info_port`, `read_block_ahci_port`, `read_blocks_ahci_port`,
`write_block_ahci_port`, `write_blocks_ahci_port`) are the whole seam above the `_ix` pair the first
rung landed. A port that no longer answers gives `NotReady`, never a read of a neighbour's disk.

### The walk needed one line

`fs::bootdisk::walk_and_witness` iterates `fat::live_sources()`. That is where USBREG expanded the
USB rung over the block registry, and it is where this arc expands the SATA rung over the AHCI
registry — **appended at the end of the list, never interleaved**. Everything else follows:

* a machine with no SATA disk (every Pi, every Jetson, every knob-off x86 leg) walks a list that is
  byte-for-byte the list it walked before;
* `admit` deduplicates a SATA disk against the USB stick through the same `fat::same_device` proof
  every other pair goes through;
* each volume is probed BY CONTENT — superfloppy BPB, then GPT, then each MBR slot, read off the
  medium (LAWS §3: root is the volume the kernel was found on, by content);
* every non-root SATA volume gets a `/volumes/<LABEL>` point with its own posture, always READ-ONLY;
* `/` still binds whatever the earlier rungs bound, because `plan` picks the FIRST disk carrying this
  kernel and the SATA rung is last. A SATA disk becomes `/` only when nothing earlier carries the
  kernel — which is exactly the "installed on and booting from the internal disk" case B89 wants.

`disk_census()` gains ` ahci<p>=present` terms, appended after the USB rung for the same reason: the
four original fields keep their spelling and order, so every capture that greps `global=` / `usb=` /
`sdhc=` / `tegra-sd=` reads the line it always did.

### The arms that were owed, and what they cost

| File | Arms | Shape |
|---|---|---|
| `drivers/block.rs` | 11 | `lookup`, `alternate_program_source`, `mbr_census` (bit + name), the four `PartitionRange` dispatches, `span_sectors`, `handle_write_veto`, plus the variant |
| `fs/fat.rs` | 15 | the variant, `name()`, `write_veto()`, the four sector dispatches, `source_of`, `handle_of`, `mount_source`, `volume_serials`, `source_present`, `source_unit`, `source_blocks`, `source_device`, and the `live_sources` expansion |
| `install/mod.rs` | **2, both REFUSALS** | `read_sectors` and `write_sectors` answer `Err(BlockError::NotReady)`. rmbp-ledger **B91**: the internal SSD carries a live Catalina, and the installer must never be able to name her disk. Nothing constructs an `Ahci` target; these arms make the refusal a property of the match rather than of who calls it. |
| `wifi/firmware.rs` | 1 | handle → `BlockSource`, mapped for totality; no firmware search reaches it |
| `fs/bootdisk.rs` | 1 | the census append (the walk itself needed no arm — it goes through `live_sources`) |
| `fs/unafs.rs` | **0 — measured, not assumed** | the module is `#[cfg(target_arch = "aarch64")]` (`fs/mod.rs:42`) and `BlockHandle::Ahci` is x86-only, so its four matches never see the variant. The first rung's report predicted four arms here; the compiler says none, and `./arroyo check` is the measurement. |

### Read-only is still a property of the image

`write_block_ahci_port` / `write_blocks_ahci_port` forward to `write_block_ahci`, which refuses
unconditionally with a one-shot witness — and there is no ladder for them to call even if they
wanted one, because the file still compiles exactly `IDENTIFY DEVICE` (0xEC) and `READ DMA EXT`
(0x25). `BlockSource::Ahci`'s `write_veto` and `BlockHandle::Ahci`'s `handle_write_veto` are
FORWARDS to that standing answer, never a second policy.

---

## The AHCIBOOT wire fixture

`fs::bootdisk::ahciboot_selftest`, `witness`-gated, driven from the tail of `ahci::probe` — the last
statement of the one enumeration pass, every HBA and port lock released, the kernel heap long since
up.

**Why there and not from the walk.** Two facts, both measured rather than assumed:

1. On x86 the walk is never driven on a headless boot. `shell::vfs_mount_table`'s
   `fs::bootdisk::bind` arm is `#[cfg(target_arch = "aarch64")]`; the x86 arm binds
   `open_read_volume()` instead. The first rung's capture shows it: `[vfs]` appears **0 times** in
   `target/serial.log`. That is a REPORTED gap this arc does not close — see §Owed below.
2. `survey()` CACHED its first answer for the boot, and this fixture runs at PCI enumeration time,
   long before USB storage finishes its deferred SCSI bring-up. Driving the cached walk from here
   would latch a root of NONE for every later caller — the hazard `fs/users.rs` already records.
   **SO38 has since fixed the caching half** (X86BIND, 2026-09-15: only a survey that BOUND a root is
   cached; a rootless one is redone when the present-source set changes — `vfs.md` §14.8). This
   fixture still does not call `survey()`, and should not: it walks the SATA rung directly through
   the very functions the walk uses, which is what makes it a proof about the chain rather than
   about the cache.

So the fixture walks the SATA rung of `fat::live_sources()` directly, through the very functions the
walk uses, and leaves the cache untouched.

**The witnesses:**

```
[bootdisk] volume source=ahci port=<p> vol=<label> serial=0x<8hex> blocks=<n> volumes_by_content=<k> ::
:: AHCIBOOT: source=ahci port=<p> vol=<label> found=AHCIBOOT.TXT bytes=<n> fnv=<16hex> -> PASS|FAIL ::
[bootdisk] ahci census: sata_sources=<n> with_fat_volume=<n> carrying_AHCIBOOT.TXT=<n> ::
```

The census line prints unconditionally, so "no SATA disk" and "the fixture never ran" are different
lines on the wire rather than the same silence (LAWS §5: an absence is evidence only if the producing
path ran). The PASS/FAIL line prints only for a volume that actually carries the marker file, so a
default `./arroyo test` with no fixture disk emits no verdict at all and cannot red on absence.

**The marker file and why it is 4096 bytes.** `AHCIBOOT.TXT` is eight whole sectors, so the read goes
through the FAT layer's COUNTED run path (`fat::read_sectors` → `block::read_blocks_ahci_port`) and
not only the single-sector arm a short file would touch. Its content is GENERATED from one rule —
`byte[i] = 'A' + (i % 26)` — which `scripts/make-fat-img.sh` stages and
`fs::bootdisk::ahciboot_expect` reproduces in the kernel. Nothing is stored on both sides, so the two
cannot drift, and a read that returns zeros fails on byte 0.

⚠ **`fnv=` and not `sha=`, and the reason is a finding the check could not see.** `crate::hash`
carries this tree's one SHA-256 and is `#[cfg]`-gated on a feature list (`lib.rs:103-109`) that
`ahci` is not on. `./arroyo check`'s `x86-all` leg carries several of those features, so the first
cut of this fixture compiled GREEN under `check` and then failed to build the `test` artifact with
`E0433: cannot find hash in crate`. LAWS §5's *an instrument's presence is proven in the artifact,
never in the check*, paid for in one build. The fix is a one-term same-line append to that cfg list
— the convention the line already documents — but it is a MODULE DECLARATION LINE in `lib.rs`, which
this arc's brief forbids touching (FC2CHECK); reported, not taken. The digest is not the verdict in
any case: the verdict is the byte-by-byte comparison against the generated rule, and the number on
the wire is a readable fingerprint.

The file is read through a `fs::vfs::MountTable` with a `FatBackend` over the AHCI source — the same
backend `bind` mounts a home-soil volume with — so the PASS is a statement about the whole chain:
VFS → FAT → `BlockSource::Ahci` → block registry → AHCI driver → the wire.

```
bash unaos/scripts/make-fat-img.sh gpt
UNAOS_AHCI=1 UNAOS_AHCI_DISK=<abs>/builder/fat-gpt.img UNAOS_QEMU_FULL=1 ./arroyo test 90
```

---

## Byte identity, knob off — AHCIBOOT's half

Every change this arc makes outside a file tail is a **LINE-NEUTRAL fold onto an existing line**, code
first and comments last, because `panic::Location` embeds source line numbers and a cfg'd-OFF line
still occupies one. `drivers/block.rs`, `fs/fat.rs` and `fs/bootdisk.rs` take tail appends for their
new functions; `install/mod.rs`, `wifi/firmware.rs` and `drivers/ahci.rs` take folds only. Measured,
not argued: `git diff --numstat` reports the same added and deleted count for every file whose only
change is folds.

---

## Owed

* **The x86 walk is not wired.** `fs::bootdisk::bind` is aarch64-only in `shell::vfs_mount_table`, so
  on x86 the home-soil mounts, the `/volumes/<LABEL>` points and the `[vfs] root` witness do not
  happen at all — with or without SATA. This arc makes the walk CAPABLE of seeing a SATA disk and
  proves that capability on the wire through its own fixture; wiring the x86 arm is its own job.
  **X86BIND (2026-09-15) DID that job, and this driver is what made it provable.** The wiring is
  ungated now, and the fixture that demonstrates it is the AHCI one — the default `./arroyo test`
  has no kernel-carrying volume the kernel can MOUNT, so root-by-content has nothing to find there. `builder/src/main.rs:1042-1043` puts the ESP holding
  `kernel.elf` on `ide-hd,drive=esp,bootindex=0` — visible to the kernel only under `UNAOS_AHCI=1`,
  i.e. **through this driver** — and the usb-storage stick is `usb.img`, the raw
  `UNA-OS-DISK-001-ALPHA` pattern, not a filesystem. So the fixture that finally hosts the x86 walk
  is an AHCI one, and that is the same configuration as "UnaOS installed on the internal disk".
  Measured, rc=0: `:: X86BIND: root=ahci5:/kernel.elf serial=0xfabe1afd by=content
  bootinfo=0xfabe1afd agrees=yes mounts=4 layout=true -> PASS ::`, with the SD card listed at
  `/volumes/UNAOS SDHC4` rather than bound. The SATA rung of `fat::live_sources` that the first rung
  appended is the rung the root was found on. See rmbp-ledger B89 (third rung) and LEDGER SO38.
* **Writes**, and the partition-mode installer behind B91's stranger guard (rmbp-queue `AHCIWRITE`).

---

## Next arc

1. **Writes.** `WRITE DMA EXT` (0x35) behind its own knob, with the same refuse-by-default posture
   `sdw` established for the SD path.
2. **The x86 `bootdisk::bind` arm**, so the volumes this arc can now find are actually mounted.
3. **A partition-mode installer.** B91's stranger guard first, then an installer that can target a
   free partition without going near the volume Catalina lives on.
