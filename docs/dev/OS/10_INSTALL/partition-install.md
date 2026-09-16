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
3. **Type `install`, and read the census before anything else.** The bare verb is **read-only** and
   it **stops**: it prints one row per partition of every disk the block registry holds, with the
   probed **content** and the refusal that partition would give, and writes nothing.

   ```
   > install
   install: read-only census (name a disk and a partition to install)
   global: 5 partitions  foreign=2 friend=0 empty=3
     whole disk: refused (disk-has-foreign-volumes) — and this verb has no whole-disk form in any case
     part0       8 MiB  content=FAT    REFUSED partition-not-empty
     part1      48 MiB  content=APFS   REFUSED partition-not-empty
     part2      48 MiB  content=empty  installable: install global 2
     part3       4 MiB  content=ESP    REFUSED partition-is-esp
     part4       1 MiB  content=empty  REFUSED partition-too-small
   ahci0: present, NOT censused - the installer cannot read this transport yet (nothing is claimed about what is on it)
   usage: install <disk> <slot> [--as-esp]   (one partition; never a whole disk)
   ```

   **That last row is a third answer and not a polite "no".** `install/mod.rs`'s `BlockTarget`
   reads only the `Global`/`Usb` handles — the SDHC, Tegra and AHCI arms all answer `NotReady`
   (B91 put the AHCI one there) — so on the rMBP the internal SSD is LISTED, because it is a disk
   this machine has, and is reported as unread rather than as unpartitioned. Saying "no readable
   GPT" about the disk Catalina lives on would read as "there is nothing on it". AHCIWRITE's read
   half is what turns this row into a census.

   Find the row whose `content=empty` that the verb calls installable and whose size matches the
   volume you just made. **That number is the GPT slot you name.** Do not count rows: the index is
   the slot, and the two differ on any disk that has ever had a partition deleted. The disk NAME is
   the transport (`global`, `usb0`, `sdhc`, `ahci0`), not a position in a list.
4. **Install into that partition, by index: `install <disk> <slot>`.** The verb has **no whole-disk
   form** — not a flag, not a bare argument. R25 is the reason the grammar cannot express one.

   ```
   > install global 2
   install part2: FAT32 + boot tree, 4 files, 61952 bytes, verified 4/4
   neighbours untouched: 4/4 partitions byte-identical
   ```

   and on the wire, `:: INSTALLVERB: wrote part=2 files=4 bytes=61952 verified=4/4 -> PASS ::`
   followed by `:: INSTALLVERB: neighbours untouched=4/4 -> PASS ::`. If the neighbours line is not
   PASS, **stop and report it** — that is the invariant this whole arc exists to hold. Naming a
   partition that is not yours prints the engine's own refusal and moves no byte:

   ```
   > install global 1
   install refused (NotBlank) — nothing was written to global (see the console log for the reason)
   ```
   `:: PINSTALL: refusal target=part1 reason=partition-not-empty content=APFS -> guard OK ::`

   `--as-esp` is accepted **only** as the explicit third word (`install global 2 --as-esp`), never
   inferred and never in place of the slot. `install --gui` reopens the graphical installer below.
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

## The graphical installer — two presses, and the first one only reads

`video/instgui.rs` is a face on the same engine and holds no authority of its own. Its go-button used
to be one attended Enter away from the **whole-disk** engine, which on this machine is the engine
aimed at Catalina's disk (rmbp-ledger **B91**).

| press | screen | what it does | writes? |
|---|---|---|---|
| Enter on the chooser | *Choose a target disk* | commits the disk identity, then takes the **census** (`partition::census`) and paints it: one row per partition with its content and, for each, the refusal `check_partition` gave | no |
| w / s | *What is on this disk* | step between **installable** partitions only — selection cannot rest on a row the engine would refuse, so the next press cannot be aimed at a stranger's volume | no |
| Enter again | *What is on this disk* | `install_into_partition(disk, slot, as_esp: false)` for that ONE partition. There is no key in the dialog that sets `--as-esp` | **yes, inside the partition** |
| `d` | *What is on this disk* | the whole-disk demo — **offered only when the census found no volume that is not ours**. On any other disk the key prints `whole-disk target REFUSED at the affordance` and does nothing | no |
| Enter on the warning | *Erase and install?* | the whole-disk engine, and the guard is **asked again here** — an affordance check alone is a UI filter, not a guard | **yes, the whole disk** |

So there is no path from this dialog to the whole-disk engine on a disk carrying a foreign volume:
the affordance is absent, the reason is on the glass in its place, and the go re-asks.

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

**INSTALLVERB: the OPERATOR leg adds `UNAOS_INSTGUI=1` and a typist, and the first knob is not
cosmetic.** `main.rs`'s two `install_probe_once` call sites are gated
`all(feature = "installdemo", not(feature = "instgui"))` — *"INSTGUI supersedes the auto-probe: when
the graphical installer is armed, the attended Enter on its warning screen is the ONLY trigger"*. So
with `UNAOS_INSTGUI=1` the unattended fixture does **not** run, part 2 is still empty when the
prompt arrives, and what installs into it is the operator. That is the whole point of the leg: the
same disk, written by a human's two words instead of by a boot probe.

```
UNAOS_QEMU_EXTRA="-qmp tcp:127.0.0.1:4478,server,nowait" \
UNAOS_WC=1 UNAOS_INSTGUI=1 UNAOS_INSTALLDEMO=1 UNAOS_AHCI=1 \
  UNAOS_PART_DISK=builder/part-fixture.img UNAOS_QEMU_FULL=1 ./arroyo test 180
# and, against that QMP port, in order (scripts/qmp_type.py --text '<line>' --enter):
#   install                 -> the census, read-only
#   install global 2        -> the install
#   install global 1        -> GO-RED: the APFS slot, refused
#   install --gui           -> the window; then Enter (census), Enter (second press)
```

Keys are real: `send-key` → the emulated keyboard → HID → the same console the operator types into.
`install --gui` sets a flag that `instgui::service()` honours on the next main-loop pass rather than
creating the window on the shell's stack — the first cut called `open()` inline and faulted
(`site=fbcon::panic_screen`, `#DB`), which is why the dialog has exactly one spawn place.

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

On the OPERATOR leg the same disk is written by the verb, so the verdicts are
`awk 'index($0,"INSTALLVERB:")'` and `awk 'index($0,"instgui")'`:

```
:: INSTALLVERB: census disks=3 ::
:: INSTALLVERB: census disk=global transport=global ::
:: INSTALLVERB: census disk=ahci5 transport=ahci UNREADABLE-HERE err=NotReady — install/mod.rs's BlockTarget reads only global/usb (B91); the disk is PRESENT and its content is UNKNOWN, not empty ::
:: PINSTALL: census part=2 type=ebd0a0a2 lba=116736..215039 sectors=98304 content=empty ::
:: INSTALLVERB: preview target=global:part2 content=empty sectors=98304 -> INSTALLABLE ::
:: PINSTALL: refusal target=global:part4 reason=partition-too-small have=1048576B need=34662912B -> guard OK ::
:: INSTALLVERB: wrote part=2 files=4 bytes=61952 verified=4/4 -> PASS ::
:: INSTALLVERB: neighbours untouched=4/4 -> PASS ::
:: PINSTALL: refusal target=part1 reason=partition-not-empty content=APFS -> guard OK ::
:: INSTALLVERB: install target=global:part1 err=NotBlank — nothing written ::
[wc-x] instgui census step=1 gpt=1 parts=5 installable=0 whole_disk_offered=0 — READ-ONLY, nothing written
[wc-x] instgui install-go step=2 part=4 (attended Enter on the census screen)
:: PINSTALL: refusal target=part4 reason=partition-too-small have=1048576B need=34659328B -> guard OK ::
```

**The two `need=` numbers differ by 3,584 B on purpose, and the difference is the one approximation
in the verb.** The preview asks `check_partition` about `shell.rs`'s `INSTALL_PREVIEW_TREE_BYTES`
(64 KiB) because `demo_tree()`'s real 61,952 B is private to `install/partition.rs`; the constant is
deliberately the LARGER of the two, so the preview can only ever be pessimistic — it can call a slot
too small that the engine would take, and can never call one installable that the engine then
refuses for size. The engine's number is the one that decides.

**The neighbour invariant, measured off the image by the HOST after the run** (`dd` + `sha256sum` over
each partition's extent of `builder/part-fixture.img` versus `target/partfixture.img`, the copy QEMU
actually wrote). This is independent of anything the kernel claims:

| extent | pristine | after the operator install |
|---|---|---|
| LBA 0..33 (protective MBR + header + entry array) | `2d088e15…` | `2d088e15…` **identical** |
| part 0 FOREIGN-FAT | `bc06d5b0…` | `bc06d5b0…` **identical** |
| part 1 FOREIGN-APFS | `bd17276b…` | `bd17276b…` **identical** (and it is the slot the go-red named) |
| part 2 UNAOS-TARGET | `152ba99d…` | `5b609830…` **changed — the only one** |
| parts 3+4 ESP + TINY | `c036cbb7…` | `c036cbb7…` **identical** |

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
