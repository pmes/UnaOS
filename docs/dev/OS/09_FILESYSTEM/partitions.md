# Partitions: the MBR layout of record, and how a volume is confined to one

Status: **PARTITION (GR9) landed** — the kernel decodes a classic MBR in one place, exposes each
primary as a bounded addressable range, and the FAT mount now binds a **partition**, not a device.
Reading and bounding only; nothing in this arc writes a partition table.

## 1. The layout of record

```
LBA 0                 MBR — four 16-byte primary entries at offset 446, 0x55AA at 510
partition 1           ESP — FAT16/FAT32, the boot tree (EFI/BOOT/*, kernel.elf, config.txt, …)
partition 2           UnaFS — the native volume (journaled, per-object ACL)
partitions 3, 4       unused
```

This is not a new invention. It is the layout `make-pi-img.sh` already stamps (the UnaFS partition
carries type byte `0x7F`) and the one `ROADMAP.md` §2 K3 names when it says the native volume is
"staged as MBR partition 2 on the Pi image". The UnaFS crate's block adapter
(`unaos/libs/fs/unafs/src/adapter.rs`, BeFS-K2) has read MBR and GPT tables with bound checks since
K2, and `locate_unafs` has found the native volume by its `UNAFS` superblock magic — deliberately
**not** by the type byte, so a card carved by any tool works. What was missing was the other half:
the FAT side, and a bound that survives a hostile or merely wrong table.

GPT remains supported and is unaffected. The installer engine (`install/gpt.rs`) still writes GPT,
and `builder/src/vm_image.rs` still ships a GPT disk image; both are untouched by this arc.

## 2. Everything on LBA 0 is a claim, not a fact

LBA 0 of an arbitrary disk is attacker-shaped input. An unformatted stick, a foreign OS's table, or
a deliberately crafted sector all read back as 512 bytes with no error. The hardware guarantees
exactly one thing — that those bytes came from sector 0 of the device, and only because the block
layer bound-checked the read against the device's own `num_blocks`. Nothing guarantees the type
byte, the start LBA, the length, or that two entries describe disjoint regions.

`drivers::block::decode_mbr` is therefore a pure function that judges each of the four entries and
drops the ones that fail, naming the rule (`PartitionReject`). It never repairs and never clamps:

| Rule                                    | Verdict            | Why                                                                        |
| --------------------------------------- | ------------------ | -------------------------------------------------------------------------- |
| type byte `0x00`                        | `Empty`            | an unused slot — not a defect                                               |
| any entry has type `0xEE`               | `Protective` (all) | the real table is the GPT at LBA 1; a hybrid MBR is an ambiguity, not data  |
| type `0x05` / `0x0F`                    | `Extended`         | we do not walk EBR chains, so the container is not an addressable volume    |
| `sector_count == 0`                     | `ZeroLength`       | addresses nothing; can only mislead                                         |
| `start_lba == 0`                        | `StartsAtZero`     | would contain the table describing it, and aliases the superfloppy case     |
| `start + count` overflows               | `ExtentOverflow`   | a crafted extent that would wrap back into low LBAs                         |
| `start + count > num_blocks`            | `PastDevice`       | would address sectors the medium does not have                              |
| intersects an already-accepted entry    | `Overlap`          | two filesystems sharing sectors is a corruption generator                   |

A sector with no `0x55AA` decodes to `None` — "this is not an MBR", not an error; LBA 0 may
legitimately be a FAT superfloppy BPB or blank media, and the caller falls through to its other
candidates. **A table where every entry fails decodes successfully to a table with zero
partitions**, which every caller reads as "no partition here". That is the safe direction: a caller
can only ever mount fewer volumes, never a volume in a place the table did not honestly describe.

The overlap rule is load-bearing rather than fastidious. Without it, two partitions could each pass
their own bound check while writing the same sectors, which would make §3's confinement a formality.
Entries are judged in table order, so the accepted set is pairwise disjoint and the outcome is
deterministic for a given medium.

## 3. Confinement: two independent bounds on every access

`drivers::block::PartitionRange` is the bounded window the layers above address a partition through.
Every entry point takes a **partition-relative** LBA and refuses any span not entirely inside
`[0, sector_count)` *before* translating it to an absolute LBA — the whole span, not merely its first
sector, because a span whose head is in range and whose tail is not is exactly the shape of a
cross-partition write.

A FAT volume mounted from a partition is confined by **two independent claims**, and neither is
redundant:

1. **The volume's own extent** — `part_lba .. part_lba + vol_sectors`, where `vol_sectors` is the
   BPB's `tot_sec`. Checked by `FatFs::in_extent`.
2. **The partition table's extent** — the retained `PartitionRange`, checked by
   `contains_absolute`, which re-derives nothing from the volume's fields.

These are two different structures written by (possibly) two different tools at two different times.
Agreeing at mount time does not make one redundant at access time: a wrong `vol_sectors` cannot by
itself let an access leave the partition, and a wrong partition length cannot by itself let an
access leave the volume. Below both, the block layer re-checks every LBA against the device's live
`num_blocks`, re-read from the registry on each call — which is the only layer that can notice a
disk that was unplugged.

`fs/fat.rs` routes **all** sector traffic through `rd_sector` / `wr_sector` / `rd_sectors` /
`wr_sectors`, so no call site can bypass the gate. The FAT geometry checks in `parse_bpb` already
make every derived address land inside `tot_sec` by construction; the gate is the second layer that
still holds if a future derivation is wrong, if an on-disk field is corrupt, or if a caller supplies
an LBA it computed itself. Two compares per sector operation, against a path that costs a USB round
trip.

An access that escapes is `FatError::OutOfVolume`, surfaced by the shell as `-EFAULT` — deliberately
not folded into `-EIO`, because it is categorically not a medium fault: the disk was never asked.
On a healthy volume it never occurs.

### The mount-time gate

`parse_bpb` refuses any BPB whose `tot_sec` exceeds its containing partition. The pre-existing check
only proved the volume fits the *disk*, which is also satisfied by a FAT volume that overruns its
partition and runs on into the next one — on the layout of record, straight into UnaFS. The mount is
**refused, not clamped**: a volume whose own header disagrees with its container is not a volume we
understand, and silently shrinking `tot_sec` would leave a filesystem whose FAT and cluster count
describe sectors we then refuse to serve.

### Mount order, and why the superfloppy still goes first

`mount_source` tries, in order: superfloppy (BPB at LBA 0) → GPT → MBR primaries in slot order. The
superfloppy attempt stays first and that ordering is load-bearing: a FAT superfloppy *also* carries
`0x55AA` at offset 510, and its bootstrap code occupying bytes 446..510 can decode as
plausible-looking partition entries. Only the BPB gates (jump instruction, sector size, geometry
consistency) tell the two apart, so they run before any partition table is trusted. The MBR branch
then walks accepted entries in slot order, so **partition 1 is what a FAT mount binds** when it is a
FAT volume — under its own partition's length.

### One deliberate divergence: `volume_serials`

The INSTALL-SELF boot-device guard (`fat::volume_serials`) keeps a **broader** walk than
`decode_mbr`: it does not drop entries for overrunning the device or overlapping a neighbour, and it
does not apply the partition-length gate. That is not parser drift. The guard's only dangerous
failure mode is *missing* a serial, because that would offer the boot disk as an erase target;
enumerating a superset of the mount's candidates keeps the error in the safe direction (more
refusals, never fewer). The mount path's extra rules exist to decide which volume to *trust*, which
is a different question.

### SDSEAM: a bound only holds if it names the same disk the bytes come from

Both bounds above are *per-device* numbers, so each is only as good as the answer to "which device
is this". `fs/unafs.rs`'s `SdSectorDevice` used to answer that question twice, independently: its
reads went to the ambient backend dispatcher (`block::read_block`) while its `sector_count` came
from the ambient global registry row (`block::info`). PI-FS-2 is the boot where the two answers
diverged — a USB mass-storage enumeration overwrote the global row with the stick's geometry while
`read_block` still routed to the microSD, so the SD's own partition (LBA 63, extent 109439) was
refused as out of bounds against the stick's 29120 blocks. That fix pinned the size to the card;
this one removes the second answer.

`SdSectorDevice` carries a `block::BlockHandle` and dispatches reads, writes **and** sizing on that
one value. `locate_on` / `mount_on` are the handle-named constructors; `locate` / `mount` are
`BlockHandle::Global` wrappers, so every Pi path opens the same handle through the same calls it
always made and its behaviour is unchanged. `partition_witness` follows the mount's handle too, so
the census and the bounded magic re-read describe the volume that was actually mounted rather than
whatever occupies the global slot.

The three dispatches are **exhaustive — no wildcard arm**. `BlockHandle` is total by construction in
the block layer, so a handle added there is an `E0004` at this seam rather than a silent mis-route
into the ambient backend. That is why the tegra card's sizing arm, deliberately skipped during
TEGRASD (on tegra the ambient path reaches the USB stick, so `tegra_sd_info()` would have guarded
stick reads with the card's capacity — PI-FS-2 inverted), is correct the moment it is written: the
arm that supplies the capacity and the arm that supplies the bytes are picked by the same value.

### UNAFSBIND: the mount cache names its disk too — the handle-aware binding contract

SDSEAM made the *device* carry its handle; the process-wide `with_unafs` mount cache still assumed
one. Its lazy bind called `mount()` — a `BlockHandle::Global` wrapper — so on a machine whose unafs
volume arrives via any **other** handle (the orin's TegraSd card is the motivating case: the global
handle there is the USB boot stick), the shell's `with_unafs` could never see its own volume: the
cache was hardwired to the global block device. UNAFSBIND extends the SDSEAM philosophy one level
up. The contract:

* **The cache entry stores its handle.** The entry is a `BoundMount { handle, fs }` — the
  `BlockHandle` the volume was mounted from is recorded beside the live mount, not re-derived, and
  `mount_bound_handle()` exposes it so callers can label their witnesses with the disk the shared
  mount actually rides. Every sector the mount moves already routes through that same handle, because
  the adapter inside `fs` was built by `mount_on(handle)` (SDSEAM's carried-handle device).
* **`Global` first, and it wins whenever it holds a volume.** The lazy bind (`bind_mount`) attempts
  `Global` before anything else, call-for-call the pre-UNAFSBIND `mount()`. On the pi — where
  `Global` is the SD/USB path and holds the volume of record — the probe walk never runs and
  behaviour is byte-for-byte unchanged.
* **Probing happens only on proof of absence, never on ambiguity.** Other handles are probed only
  when the global attempt failed with a no-volume-reachable verdict (`NoStorage`,
  `BadSectorSize`, `Part`, `NoVolume`) **and** no transient `Busy` was latched during the attempt. A
  `Busy`-tainted or `Fs(_)`-refused global attempt returns its error unprobed — the global volume may
  merely be mid-loan or mount-refused, and binding another handle's volume behind it would shadow the
  volume of record with a plausible impostor. If nothing binds anywhere, the **global** error is
  returned, so a volume-less machine reports today's exact verdict.
* **The probe walk is E0004-guarded.** `bind_probe_admitted` is an exhaustive match — a handle added
  in the block layer is a compile error at the bind seam, forcing the decision "should this handle be
  discoverable?" in the same commit that adds the handle (for `TegraSd` the answer is `true`, one
  line, and the shell needs no tegra-side wiring at all). `handle_kind_name` and
  `bind_probe_candidates` sit beside it under the same MERGE NOTE.
* **The binding is witnessed.** A successful bind prints one line, so any machine's wire shows which
  handle its mount rode (`native` is the volume's VFS name of record; re-binds after
  `force_remount`/discard print again — each line describes one live binding):

```
:: [unafsbind] mount=native handle=global ::
```

## 4. Witnesses a metal boot prints

Raw first, conclusion second — the same discipline the block layer's `io-cause` witness follows.
Latched once per handle per boot.

```
:: PART: mbr-raw handle=global dev_blocks=<n> sig=55aa ::
:: PART: mbr-raw handle=global e1 = 00 20 21 00 0c ... ::      (four lines, 16 bytes each, verbatim)
:: PART: mbr handle=global slot=1 type=0x0c boot=0x00 start=… count=… end=… ACCEPT ::
:: PART: mbr handle=global slot=2 type=0x7f boot=0x00 start=… count=… end=… ACCEPT ::
:: PART: mbr handle=global slot=3 REJECT Empty ::
:: PART: mbr handle=global slot=4 REJECT Empty ::
:: PART: mbr census handle=global protective=0 accepted=2 rejected=0 ::
:: PART: fat mounted from MBR slot 1 — extent LBA …..… (… sectors), FAT32 vol@LBA… volsec=… … ::
:: PART: unafs span check — slot=2 type=0x7f part=[…..…) span_base=… span_blocks=… fits=yes magic=ok ::
```

**Instrument honesty.** `accepted` and `rejected` are properties of the *medium*, read at the moment
the table was decoded — there is no window in which they are undefined, and nothing on an idle
system can make them drift. On the layout of record they read `accepted=2 rejected=0` from the first
read of every boot. `rejected` deliberately does **not** count `Empty` slots, so a non-zero value
always means the table carried an entry we refused, and the per-slot line above names which rule it
broke. The latches gate *printing* only: the decode they guard is pure and is re-run on every mount,
so a latched census can never make a later mount see a table different from the one it printed.

The `unafs span check` line is a cross-check between the tree's **two** partition-table readers —
the unafs crate's `parse_partitions` (which `locate_unafs` uses) and the kernel block layer's
`decode_mbr` (which the FAT mount uses). They were written at different times against the same spec,
so "they agree on this medium" is a real, falsifiable claim, and the one that matters: if they
disagreed, the ESP and the native volume could be bound to overlapping extents. The magic re-read
goes through a `PartitionRange`, so a `magic=ok` also proves the bounded partition-relative path maps
sector 0 to the same bytes the crate's adapter reached by its own arithmetic. It is strictly
advisory — every outcome is a printed line, never an error — so a wrong witness can mislead a reader
but can never change what gets mounted. aarch64 only, because `fs::unafs` is.

## 5. Bounds of this arc

* **Nothing writes a partition table.** Laying an MBR down is a medium-destroying operation and
  belongs to its own arc behind its own approval. `install/gpt.rs` (which does write a GPT, with its
  own parse-back verifier and blank-check arming) is untouched.
* **Extended partitions are not walked.** A `0x05`/`0x0F` container is rejected, not followed.
* **The UnaFS side is unchanged below the witness.** `locate_unafs` + `BlockAdapter::for_partition`
  already applied a partition base and a `block_count` bound, and that path is metal-proven
  (K3/K4/K8a); this arc adds a cross-check to it, not a rewrite.
* **Not yet verified on metal.** `./arroyo check` is green on both arches and the witness text is
  present in both artifacts; the lines above are what an attended boot should print, not what one
  has printed.

## 6. File map

* `unaos/crates/kernel/src/drivers/block.rs` — `decode_mbr`, `MbrTable`, `PartitionEntry`,
  `PartitionReject`, `mbr_census`, `PartitionRange`.
* `unaos/crates/kernel/src/fs/fat.rs` — partition-bounded mount (`parse_bpb`'s container gate,
  `mount_source`'s slot walk, `gpt_volume_spans`), the extent gate (`in_extent` and the four
  `rd_*`/`wr_*` wrappers), `FatError::OutOfVolume`.
* `unaos/crates/kernel/src/fs/unafs.rs` — `partition_witness`, the two-reader cross-check; and
  (SDSEAM) `SdSectorDevice`'s carried `BlockHandle` with the `handle_info` / `handle_read` /
  `handle_write` dispatches, plus `locate_on` / `mount_on`; and (UNAFSBIND) the handle-aware mount
  cache — `BoundMount`, `bind_mount`, `bind_probe_admitted`, `mount_bound_handle`.
* `unaos/libs/fs/unafs/src/adapter.rs` — the pre-existing (BeFS-K2) GPT/MBR parser and
  `BlockAdapter`, unchanged.
