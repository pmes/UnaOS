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

**SELFGUARD-AHCI — INSTALL-SELF can see a SATA disk now, and until this it could not.** The guard's
candidate set was built from `block::info()` + `block::usb_info()` and nothing else, so once AHCIBOOT
gave the tree `BlockHandle::Ahci { port }` an installer could be handed a SATA identity that matched
no candidate — `classify` fell to its explicit "do not invent a verdict" arm and answered **Eligible**
for it. On this machine that is the whole ballgame: the boot volume and Catalina are on the **same**
internal SATA disk, so the graphical chooser and the whole-disk engine would both have been told the
disk the OS is running from is a legitimate target. (AHCIWRITE asked the question a second time, and
locally, in `install/partition.rs`'s `sata_is_boot_device`; that was a workaround and it said so.)
`install/selfguard.rs`'s `live()` now appends every live SATA disk, read out of **`fat::live_sources()`
— the same census `fs::bootdisk`'s root walk uses**, so a disk that can become `/` can never fail to
be judged, and there is no second enumeration to drift. The DECISION did not change: `decide(boot,
serials)` is untouched and a SATA disk is refused by exactly the rule a USB stick is refused by —
it carries the boot volume's `BS_VolID`, so it is the device we booted from or a byte clone of it.
The witness is `:: SELFGUARD: census disks=<n> usb=<n> sata=<n> boot=<identity> ::` followed by one
`:: SELFGUARD: classify <identity> -> <verdict> reason=<token> ::` per candidate, emitted from the
same scan that fills the cache every `classify` reads. On the QEMU AHCI leg the ESP carrying
`kernel.elf` is an `ide-hd` on q35's ICH9 controller, so the census reads `sata=1` and that disk
classifies `-> INSTALL-SELF` — the first time the exclusion path has had a live fixture at all.
The "do not invent a verdict" arm stays, for identities the census truly does not know.

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
   `:: INSTALL: refusal target=part1 reason=partition-not-empty content=APFS -> guard OK ::`

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

**INSTALLTYPIST (2026-09-16): that sequence is a VERB now, and the leg is gated.** The block above
is what INSTALLVERB ran by hand — a shell script outside the repo poking `scripts/qmp_type.py` at a
port — so its evidence was real and re-runnable by nobody, and SPECROWS had to record the gap as a
STOP: *"INSTALLVERB gets NO pin … its census is not on any gated wire; a typist fixture is owed, not
a looser regex."* `./arroyo test-install [secs]` (default 180) is that fixture. It builds the GPT
disk, arms all five knobs through a re-exec (they are `_feats` entries, so a lane-local `export`
would build a kernel with the installer compiled out while the banner claimed otherwise), boots the
leg, types the same four commands and two Enters through the same QMP typist, and then replays the
capture against `scripts/specs/x86-install.spec` — 26 pinned lines, of which four are
`:: [midden] cmd="…" -> Host verb=install ::`, the parser's own echo of the text the keyboard
delivered. **Each burst is held on a witness line the previous burst printed**, never on a sleep:
the census's `preview … -> INSTALLABLE` gates the install, `neighbours untouched=` gates the go-red,
`err=NotBlank` gates `--gui`, and `instgui census step=1` gates the committing Enter. A slow boot
therefore delays the typist instead of desynchronising it, and a command that never ran stops the
chain where it broke rather than typing into a prompt that is not listening.

Like `test-fat`, this is a **gate command the seat runs on the fold, not a leg of `check`** — it
boots a five-knob machine with a fixture attached and spends its whole wall typing. It appears in
`check` only through GATE-SPECROOTS' accounting: `X86_INSTALL_SPEC` names the spec in code, so the
spec resolves GATED and carries no `# RUN-BY:` header.

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

Witness lines to `awk` for (`awk 'index($0,"INSTALL:")'` on `target/serial*.log`): the census rollup
and its five `census part=` rows, four `refusal … reason=` lines plus the SATA one,
`wrote part=2 fat32 tree=4 bytes=61952 verified=4/4 -> PASS`, the post-write re-census, and
`neighbours untouched=4/4 -> PASS` (all four non-target slots, not only the two foreign ones).

On the OPERATOR leg the same disk is written by the verb, so the verdicts are
`awk 'index($0,"INSTALLVERB:")'` and `awk 'index($0,"instgui")'`:

```
:: INSTALLVERB: census disks=3 ::
:: INSTALLVERB: census disk=global transport=global ::
:: INSTALLVERB: census disk=ahci5 transport=ahci UNREADABLE-HERE err=NotReady — install/mod.rs's BlockTarget reads only global/usb (B91); the disk is PRESENT and its content is UNKNOWN, not empty ::
:: INSTALL: census part=2 type=ebd0a0a2 lba=116736..215039 sectors=98304 content=empty ::
:: INSTALLVERB: preview target=global:part2 content=empty sectors=98304 -> INSTALLABLE ::
:: INSTALL: refusal target=global:part4 reason=partition-too-small have=1048576B need=34662912B -> guard OK ::
:: INSTALLVERB: wrote part=2 files=4 bytes=61952 verified=4/4 -> PASS ::
:: INSTALLVERB: neighbours untouched=4/4 -> PASS ::
:: INSTALL: refusal target=part1 reason=partition-not-empty content=APFS -> guard OK ::
:: INSTALLVERB: install target=global:part1 err=NotBlank — nothing written ::
[wc-x] instgui census step=1 gpt=1 parts=5 installable=0 whole_disk_offered=0 — READ-ONLY, nothing written
[wc-x] instgui install-go step=2 part=4 (attended Enter on the census screen)
:: INSTALL: refusal target=part4 reason=partition-too-small have=1048576B need=34659328B -> guard OK ::
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

---

## The SSD case — AHCIWRITE, and the knob that belongs on ONE flight line only

Everything above was measured over a USB disk, because until AHCIWRITE the SATA transport refused
every write and the installer said so on the wire (`reason=transport-read-only transport=ahci`). The
transport half has now landed, and it is the half with the metal risk: the disk the operator names in
step 3 of the procedure lives on the same controller as Catalina.

**THE SITTING ITSELF IS `SITTING-1.md` BESIDE THIS FILE, AND IT OVERRIDES THIS SECTION ON THREE
POINTS.** This file is the MECHANISM; `docs/dev/OS/10_INSTALL/SITTING-1.md` is what an operator
follows on Peter's metal — the precondition list, the knob line, the step-by-step witness lines and
the STOP rules. Read it before arming anything, because it measured three things this section does
not say: (1) `UNAOS_AHCI_WRITE=1` **also opens the installer's READ** of a SATA disk
(`install/mod.rs:208`), so the "routine flight, `UNAOS_AHCI=1` alone" row below censuses nothing on
the SSD — the `ahci0: present, NOT censused` row of step 3 above is what it prints; (2) with
`installdemo` + `ahci-write` and **no** `instgui`, the unattended fixture writes a real SATA disk **at
boot** (`main.rs:1817`, `main.rs:6048` → `partition.rs:1021`), so `UNAOS_INSTGUI=1` is a safety knob
on that line and not a cosmetic one; and (3) the operator path writes `demo_tree()`'s **synthetic**
four files (`partition.rs:952`, `:805`), so the installed volume's `BOOTX64.EFI` is 12 KiB of filler
and the ⌥-picker question in *Firmware facts* above cannot yet be answered by starting it.

### The knob, and where it may appear

| sitting | flight line |
|---|---|
| **A routine flight** — any leg that is not installing anything | `UNAOS_AHCI=1` and nothing more. The SATA disks enumerate READ-ONLY and the installer refuses them with `reason=transport-write-disabled transport=ahci knob=UNAOS_AHCI_WRITE`. This is the shipped posture. |
| **An install sitting** — Peter, attended, at the machine, deliberately | `UNAOS_AHCI=1 UNAOS_AHCI_WRITE=1` on that boot and no other. Destructive; a fresh go; the knob is armed for that boot only and never written into a saved alias, a script default or a card's boot config. |

`UNAOS_AHCI_WRITE=1` is the only knob in `arroyo` that can destroy data on the internal SSD. It is
default OFF, it is stripped from every aarch64 build, and a build without it compiles no ATA write
opcode at all — so the safe state is the one you get by forgetting.

### What still refuses, with the knob ON

The knob does not make SATA writable; it makes SATA writable **through a capability**. The refusal
ladder is unchanged and is asked first, and on top of it:

- **INSTALL-SELF, over SATA.** `selfguard`'s candidate list is the global and USB handles only, so it
  answers `Eligible` for a SATA identity by its own "do not invent a verdict" arm. On this machine
  the boot volume and Catalina are on the *same* SATA disk, so `install/partition.rs` asks
  `selfguard`'s own question — is the boot volume's FAT serial on this device — through
  `BlockSource::Ahci`, and refuses `reason=boot-device` when it is. The verdict is printed
  **three-valued** (`install-self=boot-device|eligible|disarmed`): a disarmed guard is not a cleared
  one. ⚠ The right long-term home for this is `selfguard`'s candidate list itself; see the STOP in
  rmbp-ledger B89's AHCIWRITE entry.
- **The whole-disk engine can never obtain a grant.** `install/mod.rs`'s `BlockTarget::write_sectors`
  `Ahci` arm stays a refusal in every cfg, `ahci-write` included. A whole-disk target can address
  LBA 0 and both GPTs — on this machine that is Catalina's partition table — so the disk-wide target
  writes no SATA sector at all. Only `PartitionTarget`, carrying a `WriteGrant` bound to one
  partition's LBA range, writes.
- **Every other SATA disk is fingerprinted, not merely unselected.** The fixture SHA-256s the first
  1 MiB of every SATA disk that is not the target — which includes the ESP the machine booted from —
  before and after the install, and prints `other-disks untouched=n/n -> PASS`.

### The QEMU leg

```
cd unaos
python3 scripts/make-gpt-fixture.py                 # -> builder/part-fixture.img
UNAOS_WC=1 UNAOS_INSTALLDEMO=1 UNAOS_AHCI=1 UNAOS_AHCI_WRITE=1 \
  UNAOS_AHCI_DISK=builder/part-fixture.img \
  UNAOS_QEMU_FULL=1 ./arroyo test 120
```

The same fixture disk as the USB leg, attached to q35's ICH9 AHCI controller on `ide.1` instead of to
the usb-storage slot — the ESP keeps `ide.0` and `bootindex=0`, so the boot is unchanged and the
machine has two SATA disks exactly as the rMBP does. **With the write knob armed the builder attaches
a FRESH COPY** in `target/ahcifixture.img` and never opens the checked-in image: a leg that wrote into
its own fixture would pass once and then measure yesterday's disk, and pointing a writable guest at an
operator-supplied path is the mistake this copy makes impossible.

The leg is still chosen **by content** (R28). `partition::run_fixture` tries the SATA arm first; if no
SATA disk carries a parseable GPT it falls through to the USB leg byte-for-byte as before, so a run
without `UNAOS_AHCI_DISK` is unchanged.

### Witness lines to `awk` for

```
awk 'index($0,"INSTALL:")'  target/serial*.log
awk 'index($0,"[ahci] write REFUSED")' target/serial*.log
```

Measured on the 2026-09-15 run (`target/serial.log`), verbatim:

```
:: AHCI: write path ARMED — opcode WRITE-DMA-EXT-0x35 (ATA8-ACS, LBA48) compiled and reachable only through a WriteGrant (first, once) ::
:: INSTALL: sata disk ix=0 port=1 sectors=262144 install-self=eligible ::
:: INSTALL: sata disk ix=1 port=5 sectors=1032192 install-self=boot-device ::
:: INSTALL: refusal target=disk=sata-port5 reason=boot-device -> guard OK ::
:: INSTALL: fixture start — SATA port=1 carries a valid GPT ::
:: INSTALL: refusal target=disk reason=disk-has-foreign-volumes foreign=2 friend=0 -> guard OK ::
:: INSTALL: SATA transport WRITABLE under `ahci-write` — no transport refusal, writes still need a grant ::
:: INSTALL: other-disk port=5 sha1MiB=0xbbfe59f39e8ad2f6 (pre) ::
:: INSTALL: grant minted transport=ahci port=1 part=2 lba=116736..215039 sectors=98304 — every other LBA on this disk is unreachable through it ::
:: INSTALL: go-red(b) plain write_sectors on the SATA handle lba=116736 => NotReady, refused ::
:: INSTALL: wrote part=2 fat32 tree=4 bytes=61952 verified=4/4 -> PASS ::
:: [ahci] write REFUSED lba=215040 outside grant port=1 116736..215039 — nothing written ::
:: INSTALL: go-red(a) write lba=215040 (grant 116736..215039) refused=1 neighbour-intact=1 -> PASS ::
:: INSTALL: neighbours untouched=4/4 -> PASS ::
:: INSTALL: other-disk port=5 sha1MiB=0xbbfe59f39e8ad2f6 (post) UNCHANGED ::
:: INSTALL: sata other-disks untouched=1/1 -> PASS ::
```

**Two things in that capture are worth reading twice.** First, **the boot ESP is AHCI port 5 and the
fixture is port 1** — QEMU's `ide.N` bus names are not the HBA port indices, so never reason about
"port 0"; read the port off `:: AHCI: port=…` and off the census line, which is what every witness
here does. Second, **INSTALL-SELF over SATA is ARMED on this fixture and it FIRED**: the boot volume's
FAT serial was found on port 5, the disk carrying `kernel.elf`, and that disk was refused
`reason=boot-device` before its GPT was even parsed. The three-valued print is what makes that
readable — `install-self=eligible` on port 1 and `install-self=boot-device` on port 5 are two
measurements, not one measurement and one silence.

### Go-red, three, and two of them are PERMANENT legs rather than mutations

- **(a) one LBA past the grant.** Built into the fixture: after the install it asks
  `write_sectors_granted` for `last_lba + 1`, which on this fixture is **part 3's first sector** — not
  a synthetic address but the exact byte a real off-by-one would destroy. Expected: the
  `[ahci] write REFUSED` witness, `refused=1`, and the victim partition's head SHA unchanged
  (`neighbour-intact=1`). Refused and nothing-moved are two different claims and both are asserted.
- **(b) the plain path, with the knob ON.** Also built in: `BlockTarget::write_sectors` on the SATA
  handle, asked with the target partition's own first sector — the most legitimate-looking write in
  the run — must still answer `NotReady`.
- **(c) the knob OFF.** Rebuild without `UNAOS_AHCI_WRITE`: the refusal returns as
  `reason=transport-write-disabled transport=ahci knob=UNAOS_AHCI_WRITE`, and
  `LC_ALL=C grep -a -o -F 'WRITE-DMA-EXT-0x35'` on the ELF is **0 hits**. This is the one that has to
  be a rebuild: it is a statement about what the artifact contains, not about what it does.
