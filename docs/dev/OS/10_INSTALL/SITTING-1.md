# SITTING-1 — the first install of UnaOS into a Disk-Utility partition on Peter's internal SSD

The operator brief for rmbp-queue's `PARTINSTALL METAL` row: **Peter's sitting, ATTENDED,
DESTRUCTIVE, and a FRESH GO every time.** Peter's goal, verbatim in rmbp-ledger **B89**: *"i do not
want to run catalina, i want unaos on the internal hd — then later boot an experimental version from
the sd card slot"*.

The mechanism is `docs/dev/OS/10_INSTALL/partition-install.md`; this file is the SITTING — what the
seat verifies before the write is armed, the knob line, the sequence at the shell, and the two places
the code does not yet do what the sitting would need it to. Precedent for the shape:
`~/unaos-bench/flash/rmbp/SITTING-5-BRIEF.md` (read-only; the 2026-07-22 Kepler pull-6 sitting).

Every claim below is a `file:line` in this tree at `11ca67f1` or a line copied verbatim out of
`~/unaos-bench/capture/rmbp12-flight8/ttyUSB0.log`, read with `awk 'index($0,"<tag>")'`.

---

## 0. The verdict up front — two STOPs and one correction

**STOP 1 — the knob pair the row names WRITES BY ITSELF, with no operator in the loop.**
`main.rs:1817` and `main.rs:6048` both call `unaos_kernel::install::install_probe_once()` under
`#[cfg(all(feature = "installdemo", not(feature = "instgui")))]` — and `main.rs:6048` is in the loop
the bench media runs (the FBCON-PACE comment beside it says so). That one-shot reaches
`install/mod.rs:413` → `partition::probe_once()` (`partition.rs:1222`) → `run_fixture()`
(`partition.rs:1015`), whose FIRST act under `ahci-write` is `if sata_fixture() { return true; }`
(`partition.rs:1021`). `sata_fixture` (`partition.rs:1297`) picks a SATA disk **by content** — any
disk whose GPT parses (`partition.rs:1346`) — names the first slot that passes the ladder
(`partition.rs:1376`), **mints a grant** (`partition.rs:1438`) and **writes** (`partition.rs:1470`).
Nothing in that path is attended.

The only guard that could refuse Peter's SSD there is `sata_is_boot_device` (`partition.rs:621`),
which asks whether the **boot volume's** FAT serial is on that disk. On the bench rMBP the boot
volume is the **SD card**, not the SSD — flights 8/9, `docs/dev/evidence/rmbp-0916/flight8-9/FLIGHT8-9.md`
X86BIND row: `root=sdhc:/kernel.elf … by=content … -> PASS`. So port 0 will read
`install-self=eligible`, the guard clears the SSD, and the fixture writes into the first slot that
passes — which is exactly the empty slot Peter made in Disk Utility.

**`UNAOS_INSTGUI=1` is what disarms it**, by `cfg`-erasing both call sites. It is therefore NOT an
optional cosmetic knob on this sitting's line: without it, `UNAOS_INSTALLDEMO=1 UNAOS_AHCI_WRITE=1`
on Peter's metal is an unattended destructive write at boot, before he can type anything.

**STOP 2 — the volume this installs is NOT bootable, and the reason is upstream of the ESP question.**
`install_into_partition` — the operator entry point (`partition.rs:944`) — builds its payload with
`let tree = demo_tree();` at `partition.rs:952`, **unconditionally; there is no metal variant**.
`demo_tree` (`partition.rs:805`) is four SYNTHETIC files: `EFI/BOOT/BOOTX64.EFI` is
`body("UNAOS-PARTINSTALL-LOADER\n", 12_288)` (`partition.rs:824`) — a 12,288-byte deterministic byte
pattern, not a PE executable and not this kernel — plus `KERNEL.ELF` 40,960 B, `SRC.TGZ` 8,192 B and
`SRC.SHA` 512 B (`partition.rs:829-833`), total 61,952 B. The real-tree reader
`install/clone.rs:80 pub fn snapshot(src: &FatFs)` **has no caller anywhere in the kernel**
(`grep -rn "snapshot(" unaos/crates/kernel/src` returns only `bootlog.rs`, `bootpace.rs`,
`flight_recorder.rs`, `serial_ring.rs` and `shell.rs`'s SMC battery — none of them this one).

So the sitting as the row describes it produces a correct FAT32 volume containing a filler file
named `BOOTX64.EFI`. **Whether the ⌥ picker lists it is not the interesting question any more**: if
the picker lists it and Peter starts it, the firmware loads 12 KiB of deterministic filler. Item (4)
of this brief — "what makes the new partition bootable" — therefore has no answer in this tree, and
**this is the STOP the seat cuts**. The sitting's boot-2 verdict has to be *"did the write land
inside the partition and leave every neighbour byte-identical"*, never *"does it boot"*.
Retiring this STOP is one change in one place — `install_into_partition` sourcing its tree from
`clone::snapshot(&FatFs)` on metal instead of `demo_tree()` — and it is reported, not taken.

**CORRECTION — the row's "Boot 1, read-only, needs `UNAOS_AHCI=1`" cannot produce a census.**
`install/mod.rs:208` is the `BlockTarget` read arm: under
`all(target_arch = "x86_64", feature = "ahci", not(feature = "ahci-write"))` the `Ahci` handle
answers `Err(NotReady)`, and the READ opens **only** under `ahci-write`. With `UNAOS_AHCI=1` alone
the `install` verb prints `ahci0: present, NOT censused …` (`shell.rs:8134`) and
`:: INSTALL: census part=…` never appears for the SSD at all. **Boot 1 needs `UNAOS_AHCI_WRITE=1`
for the READ** — and `UNAOS_INSTGUI=1` to stop that same knob writing (STOP 1). One image serves
both boots; what separates them is what Peter types, not what is compiled.

Two smaller facts the row and the reader need:

- **There is no `:: PART: gpt …` line in this tree.** `:: PART:` is emitted only by
  `drivers/block.rs:2292-2324` (MBR), `fs/unafs.rs` (the unafs span check) and `fs/fat.rs:1768`.
  The GPT view of the SSD is `:: INSTALL: census …` — the header at `partition.rs:305` and one row
  per slot at `partition.rs:314`.
- **The flown images carry no `install` verb.** The shell arm is `#[cfg(feature = "installdemo")]`
  (`shell.rs:5553`), and `UNAOS_INSTALLDEMO` is absent from both flight-8 and flight-9 knob lines
  (`~/unaos-bench/flash/rmbp/MANIFEST:639`). Image 3 is a new build, not a re-flash.

---

## 1. PRECONDITIONS the seat verifies on the wire BEFORE any write is armed

### 1.1 What the kernel has actually read off the SSD — and what it has not

Verbatim from `~/unaos-bench/capture/rmbp12-flight8/ttyUSB0.log`
(`awk 'index($0,":: AHCI:")'` and `awk 'index($0,"[bootdisk] volume")'`):

```
[  23256ms] :: AHCI: port=0 model="APPLE SSD SM768E" sectors=1467339812 lba48=1 ::
[  23256ms] :: AHCI: port=0 sector0 sig=0xaa55 kind=GPT ::
[  23256ms] :: AHCI: registered port=0 as registry index 0 — blocks=1467339812 (716474 MiB) READ-ONLY (global BLOCK_DEVICE untouched, installer not told) ::
[  23256ms] :: AHCI: selfcheck port=0 identify=ok sector0=GPT -> PASS ::
[  23262ms] [bootdisk] volume source=ahci port=0 vol=EFI serial=0x67e317ed blocks=1467339812 volumes_by_content=2 ::
```

**The slot list is UNMEASURED.** On the same capture,
`awk 'index($0,"INSTALL")' ~/unaos-bench/capture/rmbp12-flight8/ttyUSB0.log` returns **0 lines** and
`awk 'index($0,"INSTALLVERB")'` returns **0 lines** — because neither `installdemo` nor `ahci-write`
was on that flight line. We know the SSD is 1,467,339,812 sectors (716,474 MiB), carries a GPT, and
that `bootdisk` found two volumes by content on it, one of them the `EFI` volume with serial
`0x67e317ed`. **We do not know what UnaOS thinks the slots are.** Producing that list is the first
act of the sitting, and it is what boot 1 exists for.

### 1.2 The three things to check the census against Disk Utility

Read `:: INSTALL: census part=… type=… lba=…..… sectors=… content=… ::` (`partition.rs:314`) for
every slot of `ahci0`, against what macOS Disk Utility says about the same disk:

1. **The partition COUNT matches** the `parts=` figure on the header line (`partition.rs:305`).
2. **`content=APFS` lands on the Catalina container.** If that slot reads `unknown` or `empty`,
   **STOP — the probe is wrong about the one volume it exists to protect.** No write boot happens
   until that is understood. (`classify_bytes`, `partition.rs:148`: APFS is `NXSB` at +32.)
3. **`type=7c3457ef` on that same slot** — the Apple APFS type GUID, first entry of
   `FOREIGN_TYPE_GUIDS` (`partition.rs:202`). If it does not read that, the declaration backstop is
   not covering Catalina either and BOTH witnesses are blind.

### 1.3 The slot Peter must create in Disk Utility — and it is NOT what the old procedure says

**Measured, and it contradicts `partition-install.md`'s step 1.** `Content::is_installable`
(`partition.rs:121`) matches **only** `Content::Empty`. `classify_bytes` (`partition.rs:148`)
returns `Content::Fat` for anything with a FAT BPB (`is_fat_bpb`, `partition.rs:233`) and
`Content::Empty` only when every byte of the probe window is zero. **The window is the first 8
SECTORS = 4,096 bytes**: `const PROBE_BYTES: usize = 8 * SECTOR` with `SECTOR = 512`
(`partition.rs:62`, `:57`), read by `probe_content` at `partition.rs:137`. A Disk-Utility
*MS-DOS (FAT)* volume has a BPB in sector 0, so it is refused
`reason=partition-not-empty content=FAT`, exactly as INSTALLVERB's run showed.

> ⚠ Reported, not fixed: `partition-install.md` says the probe reads *"the first 8 KiB"* in its
> census row and in its refusal narrative. The constant is **4 KiB**. Nothing turns on it — the
> signatures all live below +1,024 and the blank test is over whatever was read — but an operator
> zeroing "the first 8 KiB" by that sentence is doing twice the necessary work, and an operator
> checking a 5 KiB-deep signature by it would be reading a guarantee that is not there.

**There is no `--reformat` grant.** `install_into_partition` takes exactly one switch, `as_esp`
(`partition.rs:944-948`), and the verb accepts only `--as-esp` as the third word
(`shell.rs:8374`). So Disk Utility must produce an **EMPTY** slot, and the way to get one is two
steps, not one:

1. Disk Utility → the internal SSD → *Partition* → **+** → Format **MS-DOS (FAT)**, size per §1.4.
   The format is chosen for its **TYPE GUID**, not its bytes: Disk Utility stamps Microsoft Basic
   Data `EBD0A0A2-…`, which is **deliberately absent** from `FOREIGN_TYPE_GUIDS`
   (`partition.rs:198`: *"That is what Disk Utility stamps on the FAT partition Peter is told to
   make … so listing it would refuse the one target this arc exists to accept"*). Apply.
2. **Then blank its head from macOS Terminal**, so the content probe reads `empty`:
   `sudo dd if=/dev/zero of=/dev/rdiskNsM bs=512 count=16` — 16 sectors covers the 8 the probe reads,
   with margin. Verify in Disk Utility that the partition still EXISTS (it must: a "Free space" slot
   has no GPT entry at all, and a slot that is not in the table earns `reason=no-such-partition`,
   `partition.rs:537`).

Boot 1's census is what confirms this worked: the new slot must read **`content=empty`** with
`sectors=` matching its size. If it reads `FAT`, step 2 did not take — **STOP**, redo it in macOS,
do not proceed.

### 1.4 The size floor, measured

`size_requirement(tree_bytes)` = `FAT32_MIN_CLUSTERS * 512 + tree_bytes + SLACK_BYTES`
(`partition.rs:494`, with `FAT32_MIN_CLUSTERS = 65525` at `:71` and `SLACK_BYTES = 1 MiB` at `:66`).
For the real tree (61,952 B) that is 33,548,800 + 61,952 + 1,048,576 = **34,659,328 B ≈ 33.1 MiB**.
The `install` census PREVIEW asks about a deliberately larger 64 KiB tree
(`INSTALL_PREVIEW_TREE_BYTES`, `shell.rs:8036`) = 34,662,912 B, so the preview can only ever be
pessimistic about size, never optimistic. `partition-install.md:42`'s "48 MiB absolute floor" is a
safe round-up of this number; for a real system Peter wants **≥ 64 GiB** as that doc says.

### 1.5 The refusal ladder, in the order it is evaluated

`check_partition` (`partition.rs:508`), and **the order is the design** (`partition.rs:503`: *"Boot
device first … then existence, then type, then content, then size"*). Each stops the ladder; each
prints `:: INSTALL: refusal target=… reason=<token> … -> guard OK ::` (`Refusal::say`,
`partition.rs:386`), and the `reason=` token is the API (`partition.rs:371`).

| # | line | `reason=` | what it means on THIS disk |
|---|---|---|---|
| 1 | `partition.rs:516` | `boot-device` | `selfguard::refuses` — the global/USB half. Cannot see a SATA disk. |
| 2 | `partition.rs:532` | `boot-device` | the SATA half (`ahci-write` only), `sata_is_boot_device`. **Will read `eligible` here** — see STOP 1. |
| 3 | `partition.rs:535` | `transport-read-only` / `transport-write-disabled` | the second names its own knob: `transport=ahci knob=UNAOS_AHCI_WRITE`. |
| 4 | `partition.rs:537` | `no-such-partition` | the slot is not in the table. |
| 5 | `partition.rs:539` | `partition-is-esp` | the ESP type GUID, refused unless `--as-esp`. |
| 6 | `partition.rs:554` | `partition-not-empty` | carries `content=<tag>`. **FAT lands here.** |
| 7 | `partition.rs:557` | `partition-foreign-type` | the declaration backstop, fires even on blank bytes. |
| 8 | `partition.rs:562` | `partition-too-small` | prints both `have=`B and `need=`B. |

And above all of them, asked of the whole disk and **never actable by any spelling of the verb**:
`check_whole_disk` (`partition.rs:650`) → `disk-has-foreign-volumes foreign=n friend=n` (R25).

---

## 2. THE KNOB LINE for image 3

Built on flight 8's line verbatim (`~/unaos-bench/flash/rmbp/MANIFEST:639`), plus four knobs:

```
UNAOS_WC=1 UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1 UNAOS_KEPLER_FIFO=1 UNAOS_KEPLER_CE=1 \
UNAOS_IVB=1 UNAOS_IVB3D=1 UNAOS_GMUX_IGD=1 UNAOS_WITNESS=1 UNAOS_WCG_PAYGO=1 UNAOS_LOGTS=1 \
UNAOS_WIFI=1 UNAOS_WIFI2=1 UNAOS_BT=1 UNAOS_BTC=1 UNAOS_SMC=1 UNAOS_SMCWALK=1 UNAOS_RTWIT=1 \
UNAOS_USBDEBUG=1 UNAOS_NOASPM=1 UNAOS_DEADMAN=1 UNAOS_WCDVALVE=1 UNAOS_FTDIRX=1 UNAOS_BEAM=1 \
UNAOS_AHCI=1 \
UNAOS_AHCI_WRITE=1 UNAOS_INSTALLDEMO=1 UNAOS_INSTGUI=1 UNAOS_QUARRY=1 \
./arroyo esp-x86
```

`UNAOS_BAR1WEDGE`, `UNAOS_IVB3D_R8`, `UNAOS_KEPLER_KFBIND` and `UNAOS_KEPLER_KDHEAD` are dropped:
they are flight-8 probe knobs and this sitting is not about them. `UNAOS_FTDIRX=1` stays — it is why
typing over the wire works at all (FLIGHT8-9 A9: `first byte rx=1 byte=0x68 'h'`, then the shell
answered `date`).

**The four-place check, measured on each of the four new knobs** (map / builder read / strip):

| knob | arroyo map | builder reads | `arm_features` |
|---|---|---|---|
| `UNAOS_AHCI` | `arroyo:2325` | `builder/src/main.rs:283` | stripped, `arroyo:2539` |
| `UNAOS_AHCI_WRITE` | `arroyo:2356` | `builder/src/main.rs:294` | **stripped, `arroyo:2551`** |
| `UNAOS_INSTALLDEMO` | `arroyo:616` | `builder/src/main.rs:647` | not stripped — arch-neutral, and named on `arm-pi`/`arm-tegra` (`arroyo:4443`, `:4448`) |
| `UNAOS_INSTGUI` | `arroyo:993` | `builder/src/main.rs:610` | **NOT stripped — see the note below** |
| `UNAOS_QUARRY` | `arroyo:1524` | `builder/src/main.rs:122` | not stripped, deliberately (`arroyo:1913`) |
| `UNAOS_WC` | `arroyo:785` | `builder/src/main.rs:498` | not stripped |

`ahci-write` is present on the `x86-all` type-check leg (`arroyo:4427`, last name on the line) and
absent from every `arm-*` leg, and `arm_features` rewrites `,ahci-write,` → `,` at `arroyo:2551` with
its own order-independence argument against the `,ahci,` strip one line family above. **So arming
this sitting moves no Pi and no Jetson media hash.**

`UNAOS_WC=1` is REQUIRED alongside `UNAOS_INSTGUI=1`: the module is gated on **both**
(`video/mod.rs:159`: `#[cfg(all(target_arch = "x86_64", feature = "wc", feature = "instgui"))]`).
It is already on the flight line.

> ⚠ **Reported, not taken: `instgui` is missing from `arm_features`.** Every one of its siblings
> (`ahci`, `ahci-write`, `sdw`, `sdhcblk`, `pcicensus`, `wcdvalve`, …) is stripped on the stated
> grounds that the feature emits no aarch64 code, and `video/mod.rs:159` gates `instgui` inside
> `target_arch = "x86_64"` exactly as they are. It costs nothing on this sitting (this is an x86
> media build) and it is not this brief's file to fix, but it is the same four-place gap
> BANNERCERT2 found in `ehcihid`.

**Verify the artifact, never the banner.** Before the card is cut, on the built `kernel.elf`:

```
LC_ALL=C grep -a -o -F 'WRITE-DMA-EXT-0x35' kernel.elf   # must be ≥ 1 — go-red (c) inverted
LC_ALL=C grep -a -o -F 'INSTALL: census part=' kernel.elf
LC_ALL=C grep -a -o -F 'INSTALLVERB: preview target=' kernel.elf
```

Zero hits on any of those and the knob did not reach the image — **STOP, do not fly it.**

---

## 3. THE OPERATOR SEQUENCE at the shell, over the FTDI wire

One image, two boots. **Boot 1 is read-only because nobody types the second command**, not because
anything is compiled differently — that is the honest statement of this sitting's safety and it is
why the STOP rules below are absolute.

**The standing STOP rule, for every step:** *any refusal you did not expect, any `-> FAIL`, any line
you did not predict — stop, capture, power-cycle, no retry.* (The SITTING-5-BRIEF phrasing:
*"ANY corruption/blank/tear = STOP, capture, power-cycle, no retry."*)

### Step 0 — before the machine is touched

- The FTDI capture is running and writing to a file; the `⌥` picker step is a human, every time (R3).
- Peter has a current backup of Catalina. This sitting is DESTRUCTIVE and the disk is unrecoverable
  if a bound fails.
- The empty slot exists per §1.3 and Peter has written down **its size**.

### Step 1 — boot, and read the SATA census

Hold ⌥ at the chime, pick the UnaOS stick.

**Witness to read:**
```
:: AHCI: port=0 model="APPLE SSD SM768E" sectors=1467339812 lba48=1 ::
:: AHCI: port=0 sector0 sig=0xaa55 kind=GPT ::
:: AHCI: selfcheck port=0 identify=ok sector0=GPT -> PASS ::
```
plus, new on this image (`drivers/ahci.rs`, first-once):
```
:: AHCI: write path ARMED — opcode WRITE-DMA-EXT-0x35 (ATA8-ACS, LBA48) compiled and reachable only through a WriteGrant (first, once) ::
```

**STOP if** any `:: INSTALL:` line appears on its own. With `UNAOS_INSTGUI=1` the unattended probe
is `cfg`-erased (`main.rs:1817`, `main.rs:6048`); an `INSTALL` line before Peter types anything means
the gate did not hold — **power-cycle immediately**, the disk is being written to.

### Step 2 — `install` — the read-only census

Type `install` (nothing else). It is read-only **by construction**: the bare form runs
`install_census_disk` per disk and returns (`shell.rs:8337`), and `census` +
`check_partition` write no byte.

**Witness to read**, one per slot (`partition.rs:314`):
```
:: INSTALL: census part=<i> type=<8 hex> lba=<a>..<b> sectors=<n> content=<tag> ::
```
and on the console, per slot, `REFUSED <reason>` or `installable: install ahci0 <i>`.

**Check §1.2's three things.** Then find the row whose `content=empty` is called **installable** and
whose size matches the volume Peter made. **That index is the slot you name — do not count rows**;
the index is the GPT slot and the two differ on any disk that has ever had a partition deleted
(`partition-install.md`, step 3).

**STOP if:** `ahci0: present, NOT censused` (the knob did not reach the image) · Catalina's slot
reads anything but `content=APFS type=7c3457ef` · no slot is `content=empty` · two slots are
`empty` and their sizes do not disambiguate · the census names a slot Disk Utility does not show.

### Step 3 — the preview, and Peter's FRESH GO

There is **no dry-run flag**: `install ahci0 <slot>` is the act. The census line from step 2 IS the
preview (`:: INSTALLVERB: preview target=ahci0:part<i> content=empty sectors=<n> -> INSTALLABLE ::`).
Read it aloud with the slot number and the size, and **Peter says go, at the machine, for this boot**.
A go carried over from an earlier boot is not a go.

### Step 4 — `install ahci0 <slot>` — the write

Type it exactly; no third word. (`--as-esp` is the only third word the verb takes, `shell.rs:8374`,
and this sitting does not use it — see §4.)

**Witnesses, in order** (`partition.rs` / `shell.rs`):
```
:: INSTALLVERB: install target=ahci0:part<i> as_esp=0 neighbours=<n> ::
:: INSTALL: grant minted transport=ahci port=0 part=<i> lba=<a>..<b> sectors=<n> — every other LBA on this disk is unreachable through it ::
:: INSTALLVERB: wrote part=<i> files=4 bytes=61952 verified=4/4 -> PASS ::
:: INSTALLVERB: neighbours untouched=<n>/<n> -> PASS ::
```

**The `neighbours untouched` line is the verdict of the whole sitting.** Anything but `n/n -> PASS`
— or `:: INSTALLVERB: neighbour part=<i> CHANGED across the install ::`, or
`post-write census — GPT no longer parses -> FAIL` — is a **STOP: capture, power off, do not reboot
into macOS, report.**

**STOP also if:** `verified=` is less than `files=` ("the volume is NOT trustworthy") ·
`:: INSTALL: verify part=… file=… => MISMATCH ::` appears · any `[ahci] write REFUSED lba=… outside
grant …` appears (that is go-red (a)'s witness firing on a REAL write, i.e. the installer tried to
leave its partition and bound 2 caught it) · the grant line names a `lba=…..…` range that is not the
slot's own `lba=` from step 2's census.

### Step 5 — verify OFF the machine

Power down. Boot macOS. **Disk Utility must still mount Catalina.** That, not our own
`neighbours untouched` line, is what clears this sitting — our line is our code marking its own
homework on the one disk where being wrong is unrecoverable.

### Step 6 — the ⌥ picker

Reboot, hold ⌥. **Look at the picker and write down what it lists.** This is the one genuinely new
firmware measurement the sitting makes and it has never been taken on this machine
(`partition-install.md`'s METAL-UNPROVEN marker).

**Do NOT start the new volume.** Per STOP 2, `BOOTX64.EFI` on it is 12 KiB of filler
(`partition.rs:824`); starting it hands the firmware a non-executable. The measurement is *"does the
picker list a non-ESP-typed FAT volume carrying `EFI/BOOT/BOOTX64.EFI` at all"*, and the picker's
own list answers it. `bless` (`partition-install.md` step 5) is **not** part of this sitting.

---

## 4. WHAT MAKES THE NEW PARTITION BOOTABLE — measured, and it does not

**The installer never touches the disk's existing ESP.** Everything after the refusals goes through
`PartitionTarget` (`partition.rs:713`), whose LBA 0 is the target partition's first sector; its
`map()` (`partition.rs:748`) returns `BadLba` — *"never a clamp, never a wrap"* — for any address
past the partition's length. `write_partition` (`partition.rs:868`) zeroes only
`fat32::blank_region_sectors`, formats, patches `BPB.hidden_sectors` at volume sectors 0 and 6, and
mirrors the tree. **No step addresses a sector outside the named slot**, so `disk0s1` — the ESP
Catalina and the firmware use — is not read, not written, and not referenced.

The file list the installer writes, which is the whole of what the new volume will contain
(`partition.rs:824-833`):

| path | size | contents |
|---|---|---|
| `EFI/BOOT/BOOTX64.EFI` | 12,288 B | `body("UNAOS-PARTINSTALL-LOADER\n", …)` — **filler, not a loader** |
| `KERNEL.ELF` | 40,960 B | `body("UNAOS-PARTINSTALL-KERNEL\n", …)` — **filler, not a kernel** |
| `SRC.TGZ` | 8,192 B | filler |
| `SRC.SHA` | 512 B | filler |

So there are two separate gaps between this sitting and a bootable UnaOS on the SSD, and only the
first one is a *firmware* question:

1. **Does the picker list it?** METAL-UNPROVEN; `--as-esp` (`partition.rs:965`, the one partition-
   table edit in the arc) exists precisely because it might not. Step 6 measures it for free.
2. **Is there anything to boot?** **No.** `demo_tree()` is the payload on metal as in QEMU
   (`partition.rs:952`), and `clone::snapshot` — the reader that would produce a real tree off the
   running boot volume (`install/clone.rs:80`) — is **dead code, called from nowhere**.

**This is the STOP the seat cuts.** A sitting that installs a volume the firmware cannot start is
still worth flying — it measures the write path, the bounds and the neighbours on the real disk, and
it answers (1) — but it must be *flown as that*, and the row must not promise a boot. The fix is one
call site and it is reported, not taken.

---

## 5. KNOWN / EXPECTED, and the recovery path

### 5.1 Expected on the wire, and not a surprise

- `:: INSTALL: refusal target=ahci0:disk reason=disk-has-foreign-volumes foreign=… friend=… -> guard OK ::`
  — the whole-disk question is asked and answered on every census, and **no spelling of the verb can
  act on it** (`shell.rs`, the INSTALLVERB header block; R25).
- `:: INSTALL: refusal target=ahci0:part<i> reason=partition-not-empty content=APFS -> guard OK ::`
  on Catalina's slot, and `content=ESP` → `reason=partition-is-esp` on `disk0s1`. Both are the guards
  working.
- `install-self=eligible` on port 0. **Expected and NOT reassuring** — see STOP 1. It means "the boot
  volume's FAT serial is not on this disk", which is true (we booted the stick) and says nothing
  about whether writing this disk is safe.
- `:: INSTALL: SATA transport WRITABLE under \`ahci-write\` — no transport refusal, writes still need a grant ::`
  from `transport_leg` (`partition.rs:1181`) — but only on the fixture path, which `instgui` disarms.

### 5.2 If the write is interrupted — the bounds, and what is recoverable

**Three bounds in three files for one write** (the comment naming all three is at
`partition.rs:769-782`):

1. **`install/partition.rs:748`** — `PartitionTarget::map()`. Translate and bound; `BadLba` for
   anything that would leave the partition, never a clamp.
2. **`drivers/block.rs:3214`** — `write_sectors_granted()`. Re-checks the ABSOLUTE lba against the
   granted extent at the wire and refuses **with a witness**:
   `:: [ahci] write REFUSED lba=… outside grant port=… …..… — nothing written ::`.
3. **`drivers/ahci.rs:975`** — `write_block_at()`. Re-checks against the device's own capacity.

And one structural bound above all three: **`install/mod.rs:232`** — `BlockTarget`'s `Ahci` write arm
is `Err(NotReady)` in **every** cfg, `ahci-write` included. The whole-disk target can address LBA 0
and both GPT headers, so it writes no SATA sector at all; the only path to a SATA write is
`PartitionTarget` carrying a `WriteGrant`, and the **only** minter of one is `mint_grant`
(`partition.rs:585`, with its "DO NOT ADD A SECOND MINTER" invariant:
`grep -rn 'WriteGrant::new' unaos/crates/kernel/src` must print exactly two lines forever).

**So what a power-loss mid-write can damage is bounded to the target partition's own LBA range** —
by construction, not by care. Concretely:

- **Catalina, the ESP and every other slot are untouched**, because no write could name their
  sectors. Recovery for them: none needed; boot macOS and confirm.
- **The target slot is left half-formatted** (zeroed FAT metadata region, or a FAT32 volume with a
  partial tree). There is **no journal and no resume** — `write_partition` (`partition.rs:868`) is
  linear and stateless. Recovery: **re-run the sitting from step 1.** The interrupted slot's head is
  zero or FAT; if it reads `content=FAT` the installer will refuse it (§1.3) and Peter re-blanks its
  first 16 sectors from macOS exactly as in §1.3 step 2.
- **The GPT is only ever written by `--as-esp`** (`partition.rs:965` → `gpt::set_entry_type_guid`,
  which rewrites both headers with fresh CRCs and re-validates). **This sitting does not pass
  `--as-esp`**, so the partition table is never written at all and a power loss cannot corrupt it.

**If `neighbours untouched` is NOT `n/n -> PASS`:** power off, do not reboot into macOS from the
internal disk, capture the whole log, and report. That line failing means a bound failed, which has
never happened in QEMU and would be the most important measurement this arc has produced.
