# PARTINSTALL — installing UnaOS into an existing GPT partition

**What this is for.** rmbp-ledger **B89** records Peter's goal in his own words: *"i do not want to
run catalina, i want unaos on the internal hd — then later boot an experimental version from the sd
card slot"*, and he named it himself as a test of *"whether the OS is booting truly dumb or still
containing hard codings"*. This arc builds the half of that goal the installer owes: laying UnaOS
into **one partition** of a disk that already has a partition table, **beside** a foreign volume that
must come through byte-identical.

**The rule it is built under.** RULINGS **R25** (Peter, 2026-09-08): *"another way to think of it is
like on the macbook if UnaOS saw catalina and immediately formatted the disk as an alien enemy."* A
non-UnaOS disk is a **STRANGER**. rmbp-ledger **B91** states the gap this closes: the pre-existing
guard, INSTALL-SELF, protects the disk we BOOTED from and *"leaves every OTHER disk a legitimate
candidate by construction"*. The refusal table lives in [`docs/SECURITY.md`](../../../SECURITY.md)
§"Installer — what it refuses to touch"; this file is the operator procedure and the firmware facts.

---

## The operator procedure, in five lines

1. **Partition from macOS Disk Utility.** Select the internal SSD → *Partition* → **+** → Format
   *MS-DOS (FAT)* (any format: the installer reformats it, and a FAT is only easier to recognise in
   the census) → at least **64 GiB** if it is to be a real system, **48 MiB** is the absolute floor.
   Apply. Note the new volume's size — that is how you will identify it in step 3.
2. **Boot UnaOS** from the USB stick (⌥ at the chime, pick the stick — R3: there is nobody to hold
   down ⌥ for an unattended reboot, so this step is a human, every time).
3. **Read the census before anything else.** The installer prints one `:: PINSTALL: census part=N …`
   line per partition with its type GUID, LBA range and probed **content**. Find the row whose
   `content=empty` and whose `sectors=` matches the volume you just made. **That number `N` is the
   partition index you name.** Do not count rows: `N` is the GPT slot, and the two differ on any disk
   that has ever had a partition deleted.
4. **Install into that partition, by index.** Every other partition on the disk is refused with a
   named reason, and the run ends with `wrote part=N … -> PASS` followed by
   `neighbours untouched=n/n -> PASS`. If the neighbours line is not PASS, **stop and report it** —
   that is the invariant this whole arc exists to hold.
5. **Make it bootable from macOS Recovery.** ⌘R at the chime → Utilities → Terminal →
   `sudo bless --mount /Volumes/<the new volume> --setBoot`. UnaOS does not do this and will not:
   R3 puts firmware boot selection in the operator's hands on this machine, and `bless` is that act.
   To boot it once without changing the default, hold ⌥ at the chime and pick it there.

---

## What the installer does, in order

| step | what | writes? |
|---|---|---|
| census | parse the existing GPT (CRC-validated), probe each partition's first 8 KiB for a FAT BPB, a UnaFS superblock, an APFS `NXSB`, an HFS+ `H+`/`HX`, or all-zero | no |
| refusals | boot device → transport → slot exists → ESP type → **content** → **declared type** → size, each with a `reason=` token | no |
| zero | zero exactly the FAT metadata region (`fat32::blank_region_sectors`) of the target partition — no more, no less: a stale FAT entry left by whatever was there before would forge an allocation | **yes, inside the partition** |
| format | FAT32 over the partition's own LBA range, then patch `BPB.hidden_sectors` to the partition's absolute LBA | **yes, inside the partition** |
| tree | mirror `EFI/BOOT/BOOTX64.EFI`, the kernel image and the source-along pair `SRC.TGZ`/`SRC.SHA` through `install/clone.rs`'s writer | **yes, inside the partition** |
| verify | re-read every file at the exact extents the writer recorded and SHA-256-check it | no |
| `--as-esp` | *(off by default)* set the partition's type GUID to the EFI System Partition GUID, rewriting both GPT headers with recomputed CRCs and re-validating. It also excuses an ESP-typed target from the content refusal — but only one that is **all-zero**, never a live ESP | **yes — the one table write** |
| re-census | re-parse the table and re-SHA every neighbour's first 4 KiB | no |

**No write can reach a neighbour, and this is structural rather than careful.** Everything after the
refusals goes through `install/partition.rs`'s `PartitionTarget`, an `InstallTarget` whose LBA 0 is
the partition's first sector and whose capacity is its length; an address past the end returns
`BadLba`, never a clamp. The formatter, tree writer and verifier are handed that target and cannot
name a sector on the disk. The neighbours SHA is the measurement of it.

---

## Firmware facts — read the confidence marker on each

- ⚠️ **METAL-UNPROVEN: does this machine's picker list a FAT volume that is not ESP-typed?** The
  honest first cut writes `EFI/BOOT/BOOTX64.EFI` into the **target** partition and does **not** touch
  the disk's existing ESP (`disk0s1`), on the theory that Apple's picker enumerates any FAT volume
  carrying the removable-media boot path. **We have not measured this on the 2012 rMBP.** It is the
  reason `--as-esp` exists: if the picker turns out to require the ESP type GUID, that switch sets it
  (one 16-byte field, both headers rewritten with CRCs, re-validated), and the operator asks for it
  knowingly rather than the installer retyping a partition on its own initiative.
- ✅ **MEASURED (source): the loader reads its own volume.** `bootloader/src` resolves every file
  through its `LoadedImage` → `DeviceHandle` → `SimpleFileSystem` chain — it does not hunt the disk
  for an ESP and does not care which partition it was started from. So a loader started off the
  target partition reads the kernel off the *target partition*. This is a property of our code, read
  from it; it says nothing about which volumes the firmware offers to start.
- ✅ **RULED (R3, rmbp-ledger A4): firmware boot selection here is the picker plus `bless`.** *"if you
  want to do unattended reboots you cannot because there would be nobody to hold down option"*. The
  loader uses **no** UEFI variable services at all (zero hits for `SetVariable`/`BootNext`/`BootOrder`
  across `bootloader/src/` and `builder/src/` — RULINGS R29 records the same measurement), so
  "set next boot to the new partition" is not a thing this tree can do on this machine.
- ⛔ **REFUSED, not unimplemented: the internal SSD.** `drivers/block.rs`'s `Ahci` handle is read-only
  in every cfg and the image compiles no ATA write opcode, so a SATA target is refused
  `reason=transport-read-only` on the wire. **Until AHCIWRITE lands, step 1's partition can be
  censused but not written.** The procedure above is proven end-to-end on a USB-attached GPT disk;
  the internal-SSD flight is Peter's attended, destructive sitting and is a fresh go.

---

## The QEMU fixture

Build the disk, then run the leg:

```
cd unaos
python3 scripts/make-gpt-fixture.py                 # -> builder/part-fixture.img
UNAOS_WC=1 UNAOS_INSTALLDEMO=1 UNAOS_AHCI=1 \
  UNAOS_PART_DISK=builder/part-fixture.img \
  UNAOS_QEMU_FULL=1 ./arroyo test 120
```

`UNAOS_PART_DISK` is a **builder** knob (it changes what QEMU attaches and adds no byte to any
image), so it carries no kernel feature and no four-place wiring; the kernel half rides the existing
`installdemo` feature. The leg is chosen **by content**, not by a knob: `partition::probe_once`
parses the attached disk's GPT, and a disk that has a valid table is a partition-install fixture,
while a blank scratch has none and the original `UNAOS_INSTALLDEMO` engine demo runs exactly as it
always did. That follows R28 — *"the machine will run itself until it runs right"* — find your own
disk by content.

The fixture disk carries one partition per refusal:

| part | name | content | what it proves |
|---|---|---|---|
| 0 | FOREIGN-FAT | a real FAT32 BPB + a marker string | `partition-not-empty content=FAT`, and it is a neighbour whose 4 KiB must not move |
| 1 | FOREIGN-APFS | `NXSB` at +32, Apple APFS type GUID, **48 MiB** | `partition-not-empty content=APFS` — the Catalina shape, and the neighbour the go-red tries to destroy. Sized equal to the target on purpose: at 8 MiB the go-red was caught by the SIZE guard instead of the probe, and a go-red caught by a different guard has not tested the guard it names |
| 2 | UNAOS-TARGET | all zero, 48 MiB | the one partition that passes every guard; this is where the install lands |
| 3 | ESP-SLOT | ESP type GUID, all zero | `partition-is-esp` |
| 4 | TINY | all zero, 1 MiB | `partition-too-small` |

Witness lines to `awk` for (`awk 'index($0,"PINSTALL:")'` on `target/serial*.log`): the census rollup
and its five `census part=` rows, four `refusal … reason=` lines plus the SATA one,
`wrote part=2 fat32 tree=4 bytes=61952 verified=4/4 -> PASS`, the post-write re-census, and
`neighbours untouched=4/4 -> PASS` (all four non-target slots, not only the two foreign ones).

**Go-red, both recorded, and (a) paid for two design changes before it would fire.**

- **(a) blind the installer to APFS — BOTH witnesses.** Make `classify_bytes`'s `NXSB` arm return
  `Content::Empty` *and* drop the `apple-apfs` row from `FOREIGN_TYPE_GUIDS`. Result: part 1 goes
  eligible, is selected ahead of part 2, and is overwritten —
  `selection part=1 but the fixture built part2 as the only writable slot => FAIL` and
  `neighbours untouched=3/4 -> FAIL`, run rc=1.
- **(b) corrupt the read-back compare.** Point `verify_extents` at a zero digest in
  `write_partition`: four `verify … => MISMATCH` lines and
  `wrote part=2 fat32 tree=4 bytes=0 verified=0/4 -> FAIL`, run rc=1.

**What (a) found the first time it was run, and why the design changed.** With only the content
probe, and with the neighbour baseline computed as "every partition except the one the installer
chose", blinding the probe installed straight onto the APFS partition and the check still printed
`neighbours untouched=4/4 -> PASS`. The APFS volume was destroyed and the gate was green. That is
LAWS §5's exact shape — *"a true check can answer a different question than the one asked … if those
are different sentences, the gap is the error"*: it measured "did we write outside our own choice"
when the decision needed "did we write outside the one partition that was ours". Two things came out
of that run and both are in the shipped code: the **type-GUID backstop** (a second witness that does
not depend on the content probe) and a **fixture-pinned baseline** (`FIXTURE_TARGET_INDEX`, with a
selection check — the fixture knows which slot it built empty and does not ask the code under test).
A guard that cannot be made to fail has not been verified; a guard whose *failure detector* cannot be
made to fail has not been verified either.

**`sfdisk` is absent on this box** (measured by AHCIBOOT), so the fixture's GPT is written by hand in
`scripts/make-gpt-fixture.py`, mirroring `install/gpt.rs`'s writer constant for constant — the
kernel's reader is fed the same layout its own writer makes.
