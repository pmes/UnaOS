# Filesystem layout — the namespace an operator types

**Status:** established by LAYOUT (orin 18). Companion to
[`vfs.md`](vfs.md), which owns the mount *mechanism*; this file owns the *names*.

`vfs.md` §4 called the namespace "forward-looking" and left the exact boot-time mount set to a
follow-up. This is that follow-up. It records what the namespace WAS, measured; what it is now;
what was deliberately not created; and the questions that are still open, with the calls that were
made on them.

---

## 1. The namespace TODAY (measured, per platform)

Everything in this section is the state at `cd91a3df`, the commit before this arc, read out of the
code rather than remembered.

### 1.1 Pi 4 bare-metal (`hw-pi4`, `baremetal`)

`shell::vfs_mount_table` builds the table fresh on every verb (there is no mount state; a
`FatBackend` re-mounts through `fat::mount_source` per call). The aarch64 arm bound:

| prefix | backend | volume |
|---|---|---|
| `/` | `NativeBackend("native")` | native UnaFS (partition 2, type `0x7f`) |
| `/fat` | `FatBackend("fat", Default)` | the SD boot FAT32 |
| `/usb` | `FatBackend::new_usb("usb")` | the stick, **only when it enumerates** (honest hot-plug, `vfs.md` §11) |

Programs were staged into the FAT **volume root**, beside `KERNEL8.IMG`, `config.txt`, the Pi
firmware, `overlays/` and every fixture's scratch file. `shell::EXEC_ROOT` was `"/fat"` — the
second probe of `exec_resolve`, i.e. the reason a bare `vug` worked from anywhere.

### 1.2 Jetson Orin Nano (`hw-jetson`, `tegra`) — HISTORY; see §1.4 for what the aarch64 boards do now
> **SUPERSEDED (BOOTROOT, orin 22, branch `exec-orin22-bootroot`).** The per-board root knob this
> section describes — `UNAOS_SDMMCROOT=1` / cargo `sdmmcroot`, its file-tail section in
> `arch/aarch64/sdmmc_tegra.rs`, its hard-coded `BlockSource::SdMmc` constructor in `fs/vfs.rs`
> and its statement in `shell::vfs_mount_table` — is DELETED. Nothing below is edited away: the rows
> record what was true and why, and remain the history of how the fault was found. What replaces it
> is `fs/bootdisk.rs`: the kernel is told nothing about where it came from, brings up every disk
> driver the board has, enumerates every FAT volume on every source, and finds the ONE file whose
> bytes are this running kernel's own `.text` window. That disk is the hard drive; `/`, `/boot` and
> `/apps` bind to it. Zero disks or no match prints one witness naming what was looked for and what
> was found, and the mount table is EMPTY (the verbs answer `-ENODEV`); two or more matches REFUSE
> rather than guess. No board, slot, bus, serial, card geometry, boot method or knob is in the
> decision.


The shared builder above ran, and then `sdmmc_tegra::sdmmc_root_bind` (§ROOTFS,
`arch/aarch64/sdmmc_tegra.rs`) **re-pointed both `/` and `/fat`** at the card's FAT through
`BlockSource::SdMmc`, read-only. It had to: this machine has no UnaFS volume and no `Default`
block device, so before ROOTFS (orin 16, A28) `ls /` answered `backend error: unafs-mount` and
`/fat` answered `-ENODEV`. Two mounts, zero volumes.

The two re-binds are constructed with **different volume-name strings** — `"card"` at `/` and
`"fat"` at `/fat`. See §5.1: that is a live defect, not a description of intent.

### 1.3 x86 (`hw-rmbp`)

Before VFSROUTE (orin 17) x86 **registered no mounts at all** — `vfs_mount_table` was
`#[cfg(target_arch = "aarch64")]`, so every file verb carried two bodies and the x86 body walked
`fat.rs` directly (`vfs.md` §8.1, §13.3). VFSROUTE bound the program source
(`block::program_source`, through `open_read_volume` so the FATVERB `READ_BIND` stamp is
preserved) at **both `/` and `/fat`**, one volume under two prefixes, because `/fat` was the
spelling the packaging text, the staging scripts and the exec probe all used.

x86 had **no `EXEC_ROOT`**: its cwd already sat on the volume the executables lived in, so
"resolve from the cwd" and "resolve where programs live" were the same sentence, and one probe
covered both.

---

### 1.4 BOTH aarch64 boards, today (BOOTROOT, orin 22) — the root is the disk the kernel was FOUND on

§1.1 and §1.2 describe two different answers to one question, each written down in advance: the Pi
got `NativeBackend` at `/` unconditionally because the Pi has a UnaFS volume, and the Orin got a knob
that named the Tegra card because it does not. `shell::vfs_mount_table`'s aarch64 arm now carries
ONE body with no board `cfg` in it, and it asks instead of assuming.

Peter, 2026-09-08: *"It is an OS booting off an SD card. The card is the hard drive. Every boot is
stone cold — no prefs, no special checks. Boot cold, boot dumb, presume nothing about the machine,
even though we keep booting the same machine."* And, on being handed the boot medium's identity by a
loader: *"WTF does it matter what method I choose to boot? You are assuming too much."*

**The mechanism** (`crates/kernel/src/fs/bootdisk.rs`; the module docs are the design of record):

1. Every disk driver the board has is in the image — none behind a knob. On the jetson image that
   meant making `sdmmc` default-on (`arroyo`'s `esp_jetson()`, opt out `UNAOS_NOSDMMC=1`), because
   `BlockSource::SdMmc` exists only under that feature and a walk cannot enumerate a slot whose
   type is not compiled.
2. Every FAT volume on every compiled-in `BlockSource` is walked (depth ≤ 4, ≤ 4096 entries; a cap
   that is HIT is a named reason, never a silent stop).
3. Every file with `size ≥ 4096` is tested by CONTENT: 4096 bytes of the running kernel's own
   `.text`, at `_start`, against the same bytes at the corresponding offset in the file — computed
   through the file's `PT_LOAD`s for an ELF, and at `_start`'s distance from the image base for a
   flat image. No name, extension, size or directory heuristic is involved.
4. Matches are COUNTED across all sources. Exactly one ⇒ bind. Zero ⇒ one witness and an EMPTY
   table (the verbs answer `-ENODEV`); never a guess at another disk. Two or more ⇒ REFUSE, listing
   `source:path` for each.

**The layout over that disk**, which is the part this document is about:

| prefix  | volume                                                                       |
|---------|------------------------------------------------------------------------------|
| `/boot` | the FAT volume the kernel's own image was found in                            |
| `/apps` | the SAME volume, rooted at `APPS/`, under the SAME volume NAME (see §5.1)     |
| `/`     | that DISK's native UnaFS volume when it has one and the shared mount is riding that disk; otherwise `/boot`'s volume |
| `/volumes/<NAME>` | every OTHER enumerated disk with a FAT volume, under its own label (HOMESOIL) |
| `/volumes/data` | CARDROOT, §1.6: `/boot`'s volume rooted at `DATA/`, and only when that directory exists |
| `/usb`  | unchanged — the stick, and only when it is actually enumerated (honest hot-plug) |

So the Pi's namespace in §1.1 is reproduced exactly, and now for a reason rather than by
coincidence: its card carries `KERNEL8.IMG` on FAT p1 and a UnaFS volume on p2, so `/` is native and
`/boot` is FAT. Measured on `./arroyo kernel8-test 300`:

```
[vfs] root = boot volume serial=0x894e44b4 source=global match=/KERNEL8.IMG unafs=present
  matches=1 window_off=0x80000 window_len=4096 file_off=0x0 candidates=12
  disks=global=present usb=absent sdhc=unbuilt tegra-sd=unbuilt ::
```

`window_off=0x80000` is the Pi's load address — the window IS `_start` — and `file_off=0x0` is the
flat-image arm: the first 4096 bytes of `KERNEL8.IMG` are the first 4096 bytes of the running
kernel. An Orin whose card carries only the FAT ESP gets `/`, `/boot` and `/apps` over that one
volume, which is the outcome §1.2's knob produced, reached without naming the board.

### 1.5 The WRITE POSTURE on each of those mounts (`rw=`) — SDWRITE, A60, 2026-09-12

Every mount the binder makes announces `rw=`, and the rule is one sentence: **`rw=` is sampled from
the thing being mounted, and it says `yes` only when the BLOCK LAYER admits the write.** Not from a
second derivation, not from the `BlockSource` when a `NativeBackend` is what gets bound.

| mount | posture read from |
|---|---|
| `/volumes/<NAME>` | `!FatBackend::read_only()` on the very backend handed to `mt.mount` |
| `/boot`, and `/` when it is the FAT volume | the same, on that mount's own backend |
| `/` when it is the NATIVE volume | `NativeBackend::write_veto()`, which forwards `block::native_mount_write_veto()` — the block layer's answer for the handle the shared unafs mount is riding |

The native row is the one that changed. It used to read `BlockSource::write_veto()` — a question
about a different object — while `NativeBackend::write_veto` itself returned a flat `None`, i.e. "this
volume is always writable", a claim no layer had checked. On the Orin that produced the only outcome
that actually mattered: the block layer refused **every** write to the microSD in every cfg
(`write_block_tegra_sd`), so a native root on the card could not be written and `rw=` was reporting a
refusal decided two layers below it.

`sdwrite` (DEFAULT ON, `UNAOS_NOSDWRITE=1` to opt out) lifts that refusal — see
`docs/dev/OS/01_BOOT_HAL/arch_arm64.md` §ORIN-SDMMC-5 / SDWRITE for the mechanism and the card-safety
statement — and the posture plumbing above is what keeps the wire honest in BOTH polarities:

```
[vfs] root mount / = native unafs volume source=tegra-sd rw=yes ::     # sdwrite ON  (shipped)
[vfs] root mount / = native unafs volume source=tegra-sd rw=no  ::     # UNAOS_NOSDWRITE=1
```

Leg 8 (`fs::bootdisk::sdwrite_posture_selftest`, `witness`) drives the mapping both ways and asserts
that the FAT-layer veto and the block-layer posture agree for every source in `fat::ALL_SOURCES`, so
the two views of one answer cannot drift apart again. It runs on QEMU `virt` and on x86; no QEMU
machine models the Tegra SDHCI, so it tests the REPORT and never the medium.

## 1.6 CARDROOT — what `/` lists on a ONE-MEDIUM boot (rMBP flight 10, 2026-09-17)

Peter, at the bench, after `=== SQUAWK MARK flight10`: *"root appears to be listing files that should
not be there."* This section is the measurement that answers him, and the options, so the layout call
is his to make on numbers rather than on an impression. **Nothing here is a defect report: every one
of the 21 entries is at `/` because a rule in this document put it there.** What flight 10 exposed is
that three of those rules compose badly on a machine with exactly one medium.

### 1.6.1 The wire

```
[quarry] open census cwd=/ entries=21 dirs=6 files=15 truncated=false names: APPS/ B43/ EFI/ apps/
  boot/ volumes/ BLOCK.TXT(197) GROW.BIN*(512) HELLO.BIN*(72) PULSE.ELF*(12568) S8W.BIN*(64)
  SCRATCH.BIN*(1024) …
[quarry] open volumes mounts=["/", "/apps", "/boot", "/volumes/EFI"] roots=["/"]
:: X86BIND: root=sdhc:/kernel.elf … by=content … mounts=4 layout=true -> PASS ::
```

`layout=true` and `by=content` say the binder did exactly what §1.4 specifies. `/volumes/EFI` is the
laptop's internal disk, mounted as home soil under its own label — also correct.

### 1.6.2 The census, classified

The card is written FLAT: `~/unaos-bench/scratch/rmbp-0915/logs/foldgate/card-write.sh` (seat-local)
copies the staged `data/` tree AND the ESP tree to the one FAT root, because the rMBP boots from one
SD card with one partition. `CARD-LAYOUT.txt` in each staged directory records the two lists. The 21
entries are the union (`HELLO.BIN` and `hello.txt` exist in both trees; the ESP copy wins), plus the
three names the mount table contributes synthetically.

| # | entry | class | put there by | can it move? |
|---|---|---|---|---|
| 1 | `apps/` | **(a) kernel root** | `shell::vfs_ls_collect`'s synthetic child of `/` for the `/apps` mount | n/a — not on the medium |
| 2 | `boot/` | **(a) kernel root** | same, for `/boot` | n/a |
| 3 | `volumes/` | **(a) kernel root** | same, for `/volumes/EFI` | n/a |
| 4 | `EFI/` | **(b) ESP, the loader's** | `esp-x86` — `EFI/BOOT/BOOTX64.EFI` is what UEFI loads | no — firmware fixed path (§2.1 group 1) |
| 5 | `kernel.elf` | **(b) ESP, the loader's** | `esp-x86`; also the file `fs::bootdisk` matches itself against | no — §2.1 group 1 |
| 6 | `APPS/` | **(b) ESP, the kernel's** | `esp-x86`; the directory `/apps` is rooted at | no — it IS the layout |
| 7 | `HELLO.BIN` | **(b) ESP** (data copy shadowed) | `esp-x86` + the data tree | **no** — EL0 `sys_open` (§2.1 group 3) |
| 8 | `hello.txt` | **(b) ESP** (data copy shadowed) | `esp-x86` + the data tree | yes, but see §1.6.4 |
| 9 | `SRC.TGZ` | **(b) ESP** | `pack_source_along` (SOURCE-ALONG) | **no** — SELFHOST-2 verifies it off the program-source volume ROOT (`main.rs:1232`) |
| 10 | `SRC.SHA` | **(b) ESP** | same | **no** — same reader |
| 11 | `SCRATCH.BIN` | **(c) data set** | `x86_64_data/` — U9x write fixture | **no** — EL0 `sys_open` |
| 12 | `GROW.BIN` | **(c) data set** | `x86_64_data/` — U10 growth fixture | **no** — EL0 `sys_open` |
| 13 | `S8W.BIN` | **(c) data set** | `x86_64_data/` — STOR-1 S8 write witness | **no** — EL0 `sys_open` |
| 14 | `BLOCK.TXT` | **(c) data set** | `x86_64_data/` — zeolite blocklist, read through S7 `SYS_OPEN` | **no** — EL0 `sys_open` |
| 15 | `readme.txt` | **(c) data set** | `x86_64_data/` | **no** — STOR-1 S7 opens `README.TXT` dynamically off the volume root, and the DIRNS `abs` leg resolves `/README.TXT` (`arch/x86_64/syscall.rs:23029`, `:24071`) |
| 16 | `STAT.ELF` | **(c) data set, STALE** | the carried-forward `data/` (see below) | **yes, for free** — byte-identical duplicate of `APPS/STAT.ELF` |
| 17 | `VUG.ELF` | **(c) data set, STALE** | same | yes, for free — duplicate of `APPS/VUG.ELF` |
| 18 | `VUGC.ELF` | **(c) data set, STALE** | same | yes, for free — duplicate of `APPS/VUGC.ELF` |
| 19 | `VUGX.ELF` | **(c) data set, STALE** | same | yes, for free — duplicate of `APPS/VUGX.ELF` |
| 20 | `PULSE.ELF` | **(c) data set, STALE** | same | yes, for free — duplicate of `APPS/PULSE.ELF` |
| 21 | `B43/` | **(d) wifi firmware** | carried forward by hand into the staged `data/` | **no** — `wifi/firmware.rs:152` `SEARCH_DIRS` = `/`, `/B43/`, `/FIRMWARE/`, read off the FAT volume root |

**Rows 16-20 are the one genuine accident on the card, and it costs no kernel byte to fix.** Measured
on the flight-10 staged tree
(`~/unaos-bench/flash/rmbp/UnaOS-rmbp-esp-rmbp12flight10-20260916T2132Z-fa4dcf0`): all five are
`sha256`-identical to their `APPS/` siblings, and `APPS/VUGK.ELF` has no root twin. The reason is that
`~/unaos-bench/tools/stage-x86.sh` **carries `data/` forward from the previous flight's staged
directory** ("a build tree has no data/"), refreshing the `*.ELF` files in place, so the card's data
tree still has the SHAPE a 2026-08 data volume had. The CURRENT builder puts every one of those under
`APPS/` on the data volume — `builder/src/main.rs`, "all but HELLO.BIN under APPS/" — so five root
entries survive only because the directory that holds them is inherited rather than rebuilt. Only
`B43/` genuinely has to be carried forward; it is not build-produced.

⚠ **A hazard that the stale shape currently HIDES:** `card-write.sh`'s flat write does
`cp -a "$f" "$MNT"/` per top-level ESP entry. Rebuild `data/` from `target/x86_64_data/` and it gains
an `APPS/` of its own, which is copied to the card root FIRST — and the ESP's `APPS` then lands as
`APPS/APPS`. Whoever refreshes the staging fixes the copy (`cp -a "$S/$f/." "$MNT/$f"` for
directories) in the same edit, or carries `data/` as a DIRECTORY (option ii), where the collision
cannot arise at all.

**So the 21 split (a) 3 · (b) 7 · (c) 10 · (d) 1 — and exactly FIVE of the 21 can move.** Rows
16-20, the stale duplicates. Every other entry on the medium is read from the volume ROOT by a
contract outside this document: the firmware (rows 4, 5), the EL0 `sys_open` ABI (rows 7, 11-14, 15),
SELFHOST-2 (rows 9, 10), the wifi firmware loader (row 21), and the layout itself (row 6). That is
the measurement Peter's question needed, and it is the reason a `DATA/` directory cannot empty `/`
today however the card is written.

### 1.6.3 What the other boards list at `/`, and why the rMBP is the odd one

The rule in §1.4 is `/` = the disk's native UnaFS volume when it has one, else the FAT boot volume.

* **Pi 4** — `make-pi-img.sh` lays a UnaFS volume down as MBR partition 2, so `/` is NATIVE and lists
  the K3 fixture set. The card's FAT root — `kernel8.img`, `config.txt`, `start4.elf`, `fixup4.dat`,
  `bcm2711-rpi-4-b.dtb`, `overlays/`, `APPS/`, `HELLO.BIN`, `K2OWN.BIN`, `K2IMP.BIN`, `MIDDEN.BIN`,
  `SCRATCH.BIN`, `GROW.BIN` — is at `/boot`, where a card root belongs. That list is LONGER than the
  rMBP's and nobody has ever complained about it, because it is not `/`.
* **Jetson Orin** — `unafs=absent` until UNAFSGROW, so `/` WAS the FAT card root, exactly the rMBP's
  shape (orin-ledger A53, the row that says so in Peter's hearing). Since A53/A60 the card is imaged
  with a native volume and the Orin's `/` is `native unafs volume source=tegra-sd rw=yes`.
* **rMBP** — no native volume on the card, and `fs::vfs::NativeBackend` + `fs::unafs` are
  `#[cfg(target_arch = "aarch64")]`, so `bootdisk::unafs_state` returns `"unbuilt"` on x86 and `/`
  can never be anything but the FAT boot volume. **The rMBP is the only board left whose `/` IS a
  card root, and it is the only board where flattening two trees onto one card is visible at `/`.**

### 1.6.4 The options, with costs

**(0) REFRESH THE STAGED `data/` FROM THE BUILD TREE instead of carrying its shape forward.** No
kernel byte, no card layout change, no decision: it removes rows 16-20 because the current builder
never put them at a data-volume root in the first place. `/` goes **21 → 16**. Cost: the `APPS/APPS`
hazard above must be fixed in the same edit, and `B43/` must keep being carried forward. This is a
bench-tooling change (`~/unaos-bench/tools/stage-x86.sh`, `card-write.sh`), both seat-local, and it is
worth doing whichever of the four below Peter picks.

**(i) A SECOND PARTITION on the card carrying the data set as its own volume.** The kernel already
mounts it: HOMESOIL walks every enumerated disk — but a second PARTITION of one disk is not a second
DISK, and `fs::bootdisk`'s walk is per `BlockSource`, so this needs the partition reader
(`fs/fat.rs`'s MBR/GPT arm, `docs/dev/OS/09_FILESYSTEM/partitions.md`) to publish slot 2 as a mountable
volume and the walk to enumerate volumes rather than sources. Cost: kernel work in `fs/fat.rs` and
`fs/bootdisk.rs`, a card-image builder (`arroyo esp-x86-img`, the shape `esp-jetson-img` already has),
and `media-writer.sh --image` instead of a file-level write — so the card stops being editable by
copying files onto it, which is how every rMBP flight has been staged. **It does not fix `/`**: `/`
would still be the ESP's FAT root, with rows 4-14 and 21 on it.

**(ii) A DATA DIRECTORY on the card (`DATA/`), mounted where the two-device boot puts the data
VOLUME.** The kernel half is what was BUILT, and it is one rule in `fs/bootdisk.rs` — see §1.6.5.
Card-side cost: one line in the seat-local `card-write.sh` (`cp -a "$S/data" "$MNT"/data` in place of
`cp -a "$S"/data/. "$MNT"/`), plus HOISTING out of `data/` the entries that may not move —
`SCRATCH.BIN`, `GROW.BIN`, `S8W.BIN`, `BLOCK.TXT`, `HELLO.BIN` and `B43/`. **That hoist is the honest
limit of this option and it is not negotiable from `fs/bootdisk.rs`:** rows 7 and 11-14 are pinned to
the volume root by EL0's `sys_open` ABI, whose namespace is a flat 8.3 volume root with no directory
component (§2.1 group 3, §5.2), and row 21 by the firmware loader reading `SEARCH_DIRS` off the FAT
root. Moving THOSE is a syscall-ABI change plus a `wifi/firmware.rs` change — not a layout change,
which is the sentence §2.1 group 3 has carried since orin 18 and which this arc has now paid for a
second time on a second board.

**Measured, not argued: over (0) this option moves ZERO further entries off `/` today.** Once rows
16-20 are gone by (0), every remaining data entry is one a root reader owns. The kernel rule is
therefore built and INERT — it is the namespace this card layout will need the day the EL0 ABI grows
a directory component (§5.2), and it costs nothing standing ready. `/` lands at **16** under (0),
under (ii), and under (0)+(ii) alike.

⚠ **The pins are read off the code, not executed, and that is stated rather than glossed.** Moving
`README.TXT` into `DATA/` on the QEMU `sf` medium was run (§1.6.7) and reddened NOTHING: `S7` and
`DIRNS` emit zero lines on the x86 `test-fat` lane, so this suite cannot convict the row-15 pin.
The citation is the code — `arch/x86_64/syscall.rs:23029` (`const NAME: &str = "README.TXT"`) and
`:24071` (`const ABS: &str = "/README.TXT"`) — and the owed fixture is a lane that drives the STOR-1
witnesses on x86. A pin nothing executes is a pin that will be moved by someone who read only the
listing.

**(iii) A VFS VIEW RULE hiding non-root entries at `/`.** Cheapest to write and the worst of the
four, for three reasons that are each independently sufficient. It makes `ls /` DISAGREE with the
medium, so an operator who copies a file onto the card in a desktop card reader cannot see it on the
machine that boots from it — a filesystem that lies about its own contents. It needs a LIST of what
is allowed at `/`, which is a board fact and a build fact in kernel source, and LAWS §3 forbids
exactly that ("no board, bus, slot, serial or card geometry in kernel source"). And it hides the
symptom of the composition problem while leaving every file where it was, so the next thing staged
flat is invisible instead of merely untidy.

**(iv) GIVE THE rMBP CARD A NATIVE VOLUME — i.e. make x86 do what both aarch64 boards already do.**
This is the option the measurement argues for and it is not in the brief, so it is recorded rather
than built. `/` becomes the system volume, the card's FAT root becomes `/boot` and stops being the
thing anybody looks at, and the rMBP joins the rule §1.4 already states instead of being its
exception. Cost, named: `fs::vfs::NativeBackend`, `fs::unafs` and `bootdisk::unafs_state` are all
`#[cfg(target_arch = "aarch64")]` — a cfg-widen with real work behind it (the block-handle seam
`fs/unafs.rs` `handle_write` has no x86 arm) — plus option (i)'s card image, since a native volume is
a second partition. It subsumes (i) and makes (ii) cosmetic.

**The decision is Peter's**, and it is one sentence: **does `/` on the rMBP stay the card's FAT root
(and the card gets tidier, option ii), or does the rMBP get a native root like the Pi and the Orin
(option iv, and `/` stops being a card root at all)?**

### 1.6.5 What was built: the `DATA/` rule

`fs::bootdisk::bind_data`, called from `bind` after `bind_root`. If the boot volume has a `DATA/`
directory at its root, it is mounted at `/volumes/data` — the same volume, rooted at a directory,
under the same volume NAME `boot`, which is `/apps`'s shape exactly, so
`same_volume("/boot", "/volumes/data")` stays true about one card (§5.1's rule).

**It hangs off `bind`, not `bind_root`, and that placement is load-bearing.** `bind_root` is driven
FOUR times at heap-up by `unafsroot_selftest` (leg 6) with a scratch table, on its own stated premise
that it is table shape only and that a fixture running ahead of the walk must not be the first thing
to touch the card. `bind_data` asks the MEDIUM, so from `bind_root` it would touch the card before
the walk AND latch `absent` from a `Default` source at a moment when no disk has enumerated — the
mount would then never appear for the rest of the boot. That is SO38's shape (a rootless observation
cached as an answer) reappearing in a new rule, and it is avoided by putting the rule where the
question belongs: the root LAYOUT is `bind_root`'s, a fact about the DISK is `bind`'s.

`/volumes/data` and not `/data`: on a TWO-device boot the data set is a separate medium and HOMESOIL
already mounts it at `/volumes/<its label>`. Putting the one-card data set anywhere else would make
the same files answer to two different paths depending on how many devices are plugged in.

```
[vfs] data mount /volumes/data = fat boot volume source=<src> rooted=DATA rw=<yes|no> ::
```

Three properties worth stating because each is a place this could have gone wrong:

* **It costs one probe per BOOT, not one per verb.** The mount table is rebuilt per verb; probing the
  medium for a directory every time is the cost §5.1 measured shifting the x86 window-manager battery
  into two different fixture flakes. The answer is latched in `DATA_PRESENT`.
* **A real disk labelled `data` outranks the directory.** `MountTable::mount` REPLACES a prefix rather
  than refusing it, so without the guard the card's directory would silently evict a card the operator
  is holding. The directory is still reachable at `/boot/data` in that case.
* **On every medium that exists today it does nothing at all** — no `DATA/` directory, no mount, no
  witness line. It is inert until a card is written with one.

### 1.6.6 What flight 11's `/` must list

Written down in advance so the next capture is SCORED and not merely read. Against a card staged with
option (0) alone — the tooling change, no kernel change needed, the shape flight 11 gets if nothing
else is done:

```
[quarry] open census cwd=/ entries=16 dirs=6 files=10 …
  names: APPS/ B43/ EFI/ apps/ boot/ volumes/
         BLOCK.TXT GROW.BIN HELLO.BIN S8W.BIN SCRATCH.BIN SRC.SHA SRC.TGZ hello.txt kernel.elf readme.txt
```

`STAT.ELF`, `VUG.ELF`, `VUGC.ELF`, `VUGX.ELF` and `PULSE.ELF` must be **ABSENT from `/` and present
under `/apps`** — that pair is the verdict, and the absence half is the load-bearing one for the same
reason `layout.mv`'s is (§6): a file that exists somewhere passes a presence-only test.

Against a card staged with option (ii) as well, `data/` joins the dirs and **no file leaves** (§1.6.4
(ii): every remaining data entry has a root reader), but one more line must appear:

```
[vfs] data mount /volumes/data = fat boot volume source=<src> rooted=DATA rw=<yes|no> ::
[quarry] open volumes mounts=["/", "/apps", "/boot", "/volumes/data", …] roots=["/"]
```

If that `[vfs] data mount` line is absent while `data/` is in the census, the card carries the
directory and the rule did not fire — read `DATA_PRESENT`'s probe and `/boot/data` before anything
else. If the line is present and `/volumes/data` is NOT in the `mounts=[…]` list, a real disk labelled
`data` took the point first (§1.6.5) and `[vfs] volume mounted /volumes/data source=…` will be on the
wire above it.

### 1.6.7 What was measured, and the one witness that did not fire

Two boots of the same kernel on two media, which is the pair that makes the rule falsifiable — one
variable, opposite outcomes:

| medium | `X86BIND` | `ls /` | `/volumes/data` |
|---|---|---|---|
| `builder/fat-sf.img` as `test-fat sf` builds it | `mounts=4 layout=true -> PASS` | `AHCIBOOT.TXT APPS B43 BLOCK.TXT EFI GROW.BIN HELLO.BIN HELLO.TXT KERNEL.ELF Long Filename Example.txt MixedCaseName.md README.TXT S8W.BIN SCRATCH.BIN SUBDIR apps boot volumes (11 file, 7 dir)` | absent |
| the same image with `DATA/` added and `README.TXT` moved into it (`mmd`/`mcopy`/`mdel`) | `mounts=5 layout=true -> PASS` | `… BLOCK.TXT DATA EFI … S8W.BIN SCRATCH.BIN SUBDIR … apps boot volumes` — **no `README.TXT`** | `:: volid: mount /volumes/data name=boot id=Some(8354126049188793961) ::` |

The volume id on that line is the SAME value `/`, `/boot` and `/apps` report on the same boot, which
is the §5.1 claim the mount owes: one card, one volume identity, four prefixes.

**The `[vfs] data mount …` witness did NOT print, and the reason is not this rule.** `bind` takes
`announce` from a one-shot latch, and on x86 the FIRST mount table is built at serial line ~156 —
before any disk enumerates, so `bind` returns early with the latch already consumed (SO38's
asynchronous-storage story, §1.4). Measured: `[vfs] root mount` appears **0 times** in both captures
above, and `[vfs] volume mounted` 0 times, on boots whose `/`, `/boot`, `/apps` and
`/volumes/UNAOS SDHC4` all bound correctly. **Every per-mount `[vfs]` witness on this board is dead,
has been since X86BIND, and the new one inherits that rather than adding to it.** The fix is to move
the latch from "the first table built" to "the first table that BOUND A ROOT"; it is not made here
because it changes `bind`'s announce for every board. What carries the wire meanwhile is
`:: volid: mount …`, which rides a fixture and therefore runs.

Go-red, on the artifact rather than on a third boot, because the behavioural pair above already has
two outcomes: deleting the ONE call `bind_data(mt, found.source, announce);` and rebuilding with the
same verb drops `/volumes/data` from the kernel image `2 → 0` and the witness sentence `1 → 0`, with
the control string `/volumes/dataX` reading 0 in both (`LC_ALL=C grep -a -o -F` on
`target/x86_64_esp/kernel.elf`, `./arroyo esp-x86` both sides). Reverted; the source is byte-identical
(`git diff --numstat` unchanged).

### 1.6.8 VFSWIT — the latch moved to the first table that BOUND A ROOT (flight 11, 2026-09-22)

The fix §1.6.7 named is made, and it is one clause: `bind` takes `announce` from
`s.root.is_some() && !MOUNTS_ANNOUNCED.swap(true, …)` instead of the bare `swap`, so a mount table
built before any disk enumerated no longer spends the latch on a table that prints nothing. The
condition is the ROOT and deliberately not "this table has anything to say"
(`|| !s.others.is_empty()`): `survey()` caches only a walk that FOUND a root and re-walks on every
fingerprint change (SO38), so a table can carry home-soil disks while the root is still unresolved,
and latching there would lose the root/boot/apps/data lines — the same defect one table later. The
gate run's own wire shows that window: `[vfs] resurvey n=2 … bound_on_pass=3`, i.e. two walks found
no root before the disk enumerated. What the choice gives up, stated rather than hidden: a board that
enumerates disks and never binds a root announces no `[vfs] volume mounted` line at all; its witness
is the `[vfs] root -> NONE` line the walk prints, and the mounts are still MADE either way.

MEASURED, `UNAOS_WC=1 UNAOS_QUARRY=1 UNAOS_QEMU_FULL=1 ./arroyo test-fat sf 200` (rc=0,
`completion=complete`, `complete_line=2319`), each count taken with
`awk -v p='<the line>' 'index($0,p)' target/serial.log | wc -l`:

| witness | §1.6.7 (before) | this run |
|---|---|---|
| `[vfs] root mount` | 0 | **1** |
| `[vfs] boot mount` | 0 | **1** |
| `[vfs] apps mount` | 0 | **1** |
| `[vfs] volume mounted` | 0 | **1** (`/volumes/UNAOS SDHC4 source=sdhc rw=no`) |
| `[vfs] data mount` | 0 | 0 — this image carries no `DATA/`, per §1.6.7's first row |

Four witness lines for the four prefixes the same boot's `:: X86BIND: … mounts=4 layout=true -> PASS ::`
and its four `:: volid: mount …` lines report: **one line per bound prefix, exactly once**. The pins
live in `unaos/scripts/specs/x86-fat.spec`, which `test-fat sf` replays as a STEP of the verb (its rc
IS the verb's rc), so these witnesses are scored on every gate run rather than reasoned about —
`40/40 required witnesses` on that run, where the same spec asked for 36 before. `/volumes/data` is
pinned OPTIONAL there because the sf image has no data set to mount; a REQUIRE would pin a fact about
a different image.

THE AARCH64 LANE DOES NOT CHANGE, and the reason is worth keeping: on `./arroyo test-arm` (rc=0) the
virt machine never reaches a filesystem verb, so its `[vfs]` wire is 2 lines before and after (the
`planwalk` fixture's own INVARIANT-BROKEN pair) and `diff` over them is empty; with numeric fields
normalised the WHOLE 658-line capture diffs to zero. The Pi and the Orin bind a root on the first
table they build, where the old latch and the new one answer the same, so their counts are unchanged
by construction. Image cost of the clause: aarch64 `kernel.elf` 2,142,776 → 2,142,808 B (+32); x86_64
`kernel.elf` 3,027,608 → 3,025,136 B (−2472, the early-return path re-inlining — the clause is not a
size argument in either direction and the numbers are quoted because they were asked for).

GO-RED, behavioural and on this gate: put the latch back on the first table
(`let announce = !MOUNTS_ANNOUNCED.swap(…)`, nothing else touched) and rebuild. All five witnesses
read 0 lines again, and `mbench.py --replay` — the single verdict authority, the same step the verb
runs — answers **`MBENCH FAIL — 36/40 required witnesses`, rc=1**, naming
`FIRST-SHORTFALL x86-fat.spec:339 REQUIRE \[vfs\] root mount …`; the green build of the same spec on
the same command reads `40/40`, rc=0. The go-red verb run also reds one line EARLIER than that, on
an unrelated flake — `[dmgovlp] verdict … drag_evt=0 … adopt_stretch=1/4 -> FAIL`, the compositor's
drag fixture receiving no drag events on that boot (`drag_evt=5 … adopt_stretch=4/4 -> PASS` on the
green run) — and `scan_serial_faults` runs before the replay, so the spec verdict above was taken by
running that authority over the very capture the red run produced. Reverted; the source is back to
the one clause (`git diff` on `bootdisk.rs` shows only it and its comment).

## 2. The namespace this arc establishes

```
/          → the native root      (UnaFS on the Pi; the card's FAT on the Orin and on x86)
/boot      → the volume this machine booted from
/apps      → the programs on that volume   (= /boot's APPS/ directory)
/usb       → the hot-plugged FAT stick, when it enumerates
/home      → the users' home directories on the EL0 FAT volume (LOGIN M2, 2026-09-12): `HOME/<NAME>`
             is created at that user's FIRST LOGIN, never laid out empty
```

**`/boot`, not `/fat`.** Peter's ruling: `/fat` names a *filesystem*, which is an implementation
detail the operator did not ask about. `/boot` names a *place*. (Seat's call: `/boot`, not
`/boot/efi` — there is one boot volume here and no second EFI-vs-boot distinction to draw.)

**`/apps`, and it is a real mount.** `FatBackend::rooted(fat::APPS_DIR)`
(`fs/vfs.rs:686`) binds a mount whose root is a volume-relative DIRECTORY: `on_volume` prefixes it
onto every `rel` the trait hands the backend, so a consumer of `/apps/VUG.ELF` reaches
`APPS/VUG.ELF` on the medium and **can never reach above the directory**. Every pre-LAYOUT mount
keeps `root = ""`, which prefixes nothing, so `/`, `/boot` and `/usb` walk byte-for-byte as before.

The on-medium spelling is `APPS` (8.3), presented as `/apps` (seat's call). It has to be an 8.3
short name: this FAT driver's create path writes short names only (VFAT LFN write is out of scope,
`vfs.md` §8), and every FAT lookup here is case-insensitive, so `apps`/`Apps`/`APPS` on the wire
all reach it.

**`/usb` is unchanged this round** (seat's call). It already names a place.

### 2.1 What is in `/apps`, and what is not

`/apps` holds the launchable programs, and only those:

| medium | in `APPS/` |
|---|---|
| Pi 4 (`arroyo kernel8`) | `ELFHELLO.ELF`, `VUG.ELF`, `VUGC.ELF`, `VUGX.ELF`, `VUGK.ELF`, `STAT.ELF`, `PULSE.ELF` |
| Jetson (`arroyo esp-jetson` / `esp-arm`) | `ELFHELLO.ELF`, `VUG.ELF`, `VUGK.ELF`, `STAT.ELF`, `PULSE.ELF` |
| x86 ESP + DATA tree (`builder`, `make-fat-img.sh`) | `STAT.ELF`, `VUG.ELF`, `VUGC.ELF`, `VUGX.ELF`, `VUGK.ELF`, `PULSE.ELF` |

Stays in the **volume root**, in three groups:

1. **Files the firmware reads by fixed path** — `kernel8.img`, `config.txt`, the Pi firmware +
   `overlays/`, `EFI/BOOT/BOOTX64.EFI`, `kernel.elf`. These are not ours to move.
2. **Data the fixtures read or write** — `SCRATCH.BIN` (U9), `GROW.BIN` (U10), `S8W.BIN`,
   `BLOCK.TXT`, `hello.txt`, `readme.txt`, `SRC.TGZ`/`SRC.SHA`, and everything EL0 creates at run
   time (`K2PRIV.BIN`, `SLTF/SLTG.BIN`, `MIDCPY.TXT`, `A11.BIN`, …).
3. **THE FLAT EL0 FIXTURE BLOBS** — `HELLO.BIN`, `K2OWN.BIN`, `K2IMP.BIN`, `MIDDEN.BIN`. This
   group is the interesting one and it is measured, not assumed. These are spawned by the kernel
   *and* opened **by EL0, by name**, through `sys_open` — whose namespace is a **flat 8.3 volume
   root with no directory component at all** (`MAX_NAME`, `find_located`;
   `arch/aarch64/syscall.rs:9016`). Staged under `APPS/`, the kernel loaded them fine and every
   EL0 `SYS_OPEN("HELLO.BIN")` answered `-ENOENT`: `kernel8-test` reported
   `:: U6b: real File handles FAIL — witness=0x18 … (want 0x1f) ::` and the whole U7 chain behind
   it (K1/K2/K3/K4/K8/K9/IMG-SIG/FATDIRS/FATMOVE/BANDY, 84 required witnesses) fell over.
   Moving them is a **syscall-ABI change, not a layout change**. See §5.2.

### 2.2 How a program is reached

Three paths, and they now agree:

* **`run` / `bg` / quarry's double-click** → `shell::read_el0_image` → the mount table →
  `/apps/VUG.ELF` → `APPS/VUG.ELF`. One body, both arches (VFSROUTE).
* **A bare name** (`vug`) → `exec_resolve`: the cwd first, then `EXEC_ROOT` = `/apps`
  (`shell.rs:6005`). **`EXEC_ROOT` is no longer aarch64-only.** x86 had no second probe because
  its cwd already sat on the program volume; putting the programs in a DIRECTORY breaks that
  identity on x86 exactly as it was already broken on the Pi, so `FatVolume::is_file` and
  `bare_exec_reresolve` both gained the cwd-then-`/apps` order. The x86 probe stays FAT-direct
  (volume-relative `/APPS`) so it keeps binding `mount_program_source()` and stamping `EXEC_BIND`,
  which the FATVERB witness reads.
* **The FAT-direct loaders** (`load_program_into_slot`, `image_principal_of_file`, WINX-2, WINX-8,
  PULSE-W, the desktop app launcher) → `FatFs::find_app` (`fs/fat.rs:2541`), the FAT-direct twin of
  the `/apps` mount. The two aarch64 loaders probe `find_app` **then** `find_in_root`, for the
  §2.1-group-3 reason and no other.

`IMAGE_SHA256` principals are content hashes, so no principal moved.

### 2.3 One volume is no longer one address space — what `mv` had to learn (rmbp 15 B66)

A rooted mount splits a distinction that used to be free. Before this arc every mount was rooted
at its volume root, so the remainder the resolver hands a backend was **volume**-relative and one
mount's remainder was a valid address on any other mount of the same volume. `MountTable::rename`
was built on exactly that sentence, written out in its own comment, and it handed the DESTINATION's
remainder to the SOURCE's backend. `/apps` falsified the sentence: `/boot` (root `""`) and `/apps`
(root `/APPS`) are **one volume with two address spaces**, and the remainder is now MOUNT-relative.

Both outcomes were silent, wrong-location, and reported success:

| the operator typed | the file actually went | the operator was told |
|---|---|---|
| `mv /boot/A.TXT /apps/B.TXT` | `B.TXT` at the **volume root** — never inside `APPS/` | `moved … -> /apps/B.TXT` |
| `mv /apps/X.ELF /boot/Y.ELF` | `APPS/X.ELF` → `APPS/Y.ELF` — **never left `/apps`** | `moved … -> /boot/Y.ELF` |

Both leave a file that exists *somewhere*, so a presence-only test passes on the bug. That is why
`layout.mv` (§6) scores **absence from the source** as well.

**The fix translates; it does not refuse.** `VfsBackend` gained two DEFAULTED methods —
`mount_root()` (`""` for every mount that is not `.rooted(…)`) and `on_volume()` (the one definition
of mount-space → volume-space, used by the FAT walk *and* by the translation, so they cannot drift).
`MountTable::rename` lifts each remainder to the volume through its own mount and then expresses the
pair inside whichever of the two mounts can address both — the source's when it reaches the
destination, otherwise the destination's when it reaches the source. When the destination's mount
executes, the source mount's `authorize_write` is asked too, so a move out of a rooted mount cannot
borrow the other mount's write posture. Two sibling rooted mounts that can each reach only their own
subtree are genuinely cross-space and are refused **by name**,
`VfsError::Backend("cross-mount-root")` — no board mounts that shape today.

Two other shapes were considered and rejected. *Refusing whenever the two roots differ* is a real
capability regression: `mv /boot/X /apps/Y` is a legitimate same-volume move that worked before this
arc. *Making the two-argument backend ops take volume-absolute paths* is the structural answer — no
caller could mix spaces again — but it changes the `VfsBackend` contract for every implementor, and
that is not a change to make under a boot deadline. The defaulted accessors above are deliberately
**not** that change: no implementor gains an obligation.

---

### 2.4 `/home` — from the day a user exists (LOGIN M2)

RULINGS R51 ("login, get a home folder and all"). `HOME/` at the root of the EL0 FAT volume — the
volume `SYS_OPEN` names files on, so a user's programs can reach their files — and `HOME/<NAME>` inside
it, created by `fs::users::ensure_home` at that user's first login (`[users] home=/home/<name> created
volume=el0-fat`) and idempotent after (`exists`). Nothing is created for a user who has never logged
in, and `HOME/` itself appears with the first one. User names are 8.3 leaves (1-8 bytes, `[a-z0-9_-]`,
first a letter) because the directory is. FAT carries no owner attribute: the DIRECTORY has no ACL row
(LEDGER SO35); the FILES a program creates inside it are owned by `user:<name>` through the SYS_OPEN
owner/grants rows like any private create, and the knob-on aarch64 `sys_open` walks a `/`-separated
path (`HOME/UNA/NOTES.TXT`) to reach them — the first step on SO20 (x86 still opens its static root
table). On a Pi whose `/` is native UnaFS the home still lives on the FAT boot volume for the same
reason (EL0 opens FAT); the native, owner-attributed home moves with the native-EL0 namespace.

## 3. Nothing empty was created

Peter's ruling: lay out only what exists. There is **no** `/etc`, `/tmp`, `/var`,
`/bin`, `/lib`, `/dev` or `/proc` — not as directories, not as mount points, not as reserved
prefixes. (`/home` joined the namespace on 2026-09-12 under the same rule: it exists only once a user
does, §2.4.) Every one of those would be a promise about a subsystem that does not exist yet, and an
empty directory in a listing is a question the operator cannot answer.

`/apps` is created because programs exist and were already being staged somewhere; `/boot` is a
rename of a prefix that was already bound.

---

## 4. Where the other written artifacts land — unchanged, and the question is recorded

* **Screenshots.** `video/prtscr.rs` writes `SCREEN<n>.PNG` at the **root of the first writable
  FAT** (the program source, else the USB stick). Unchanged this round (seat's call).
* **`UNAFS.ATR`.** The on-disk ACL store, `arch/aarch64/syscall.rs` `ATR_NAME`, on the **native
  UnaFS volume** (K1 persistence). The kernel owns it: `sys_open` denies it to EL0 outright.
  Unchanged this round (seat's call).

**The open question, recorded rather than answered:** neither belongs in a program directory, and
neither obviously belongs at a volume root either. A screenshot is user data; `UNAFS.ATR` is
kernel state that happens to live in the user-visible namespace. The shapes that would answer it
(`/home`, `/var`, or a hidden-attribute convention) are all §3 promises about subsystems that do
not exist. **Deferred to Peter, with the current placement stated so the deferral is visible.**

---

## 5. Open defects this arc names but does not fix

### 5.1 One card, two volume names (rmbp 15 C1) — `layout.volid`

`MountTable::same_volume` compares the backends' constructor **strings**
(`fs/vfs.rs`, `same_volume`). `sdmmc_root_bind` constructs the Orin's two re-binds as `"card"` at
`/` and `"fat"` at `/boot`, so **one card reads as two volumes**. This arc:

* **does not rename them.** A rename that handed the two mounts two NEW different names would
  reproduce the defect under a new spelling, and volume identity belongs to the VOLID arc.
* **binds `/apps` under `/boot`'s volume name**, on all three binds (the shared builder's two arms
  and `sdmmc_root_bind`), so the new mount does not add a third alias.
* **convicts it.** `shell::layout_witness`'s `layout.volid` leg (`shell.rs:3544`) asserts the
  implication *"if the oracle says `/` and `/boot` are one medium, `same_volume` must say so
  too"*, with an oracle that is **not** `same_volume`: the backends' own `describe()` geometry
  lines (`part_lba`, `vol_sectors`, `bytes_per_sec`, `sec_per_clus`, `fat_start`, `data_start`,
  `count_of_clusters`) plus an entry-for-entry comparison of the two root listings.

  The implication, not the equality — the oracle can only ever prove SAMENESS, since two different
  media could in principle carry identical geometry and identical listings.

  **The oracle is consulted only when `same_volume` says NO**, which follows from the claim being
  an implication: a `same_volume` that already says "one volume" satisfies it whatever the medium
  turns out to be. That is not only tidiness — `describe()` re-mounts the volume per call and the
  listings are two full root walks, and asking anyway was measured shifting the x86 window-manager
  battery's timing into two different fixture flakes (`WINMENU` on one run, `[wm-act] lead=false`
  on the next, with the same commit green twice at the baseline). Same reason `layout.apps` reads
  `prefixes()` rather than `rows()`: a listing question must not cost a FAT mount per mount point.

  It is green everywhere but the Orin: on the Pi `/` is native UnaFS and `/boot` is FAT, so the
  oracle says "different" and the implication holds vacuously (the witness text prints which); on
  x86 both prefixes carry one name and both sides say "same".

  **The Orin's expected red now EXPIRES BY ITSELF, and the leg detects the condition.** This leg
  used to carry the sentence *"EXPECTED RED on the Orin until VOLID lands"*, and rmbp 15 was right
  that such a sentence is a **mask**: while a leg is expected to fail it cannot report anything else
  failing, and nobody re-reads a comment to find out when the excuse expired. The excuse is now
  *measured*, by `shell::volume_identity_is_medium_derived` — a probe of the MECHANISM, built in a
  scratch `MountTable` that no board mounts:

  * **A** — one source mounted twice under **different** names. Name-derived identity says
    DIFFERENT; medium-derived identity says SAME.
  * **B** — one source mounted twice under the **same** name. Name-derived identity says SAME;
    medium-derived identity says SAME when that source carries a volume and DIFFERENT when it does
    not (an identity that cannot be established equals nothing, not even another such identity).

  `A || !B` is true under medium-derived identity **whether or not `BlockSource::Default` has a
  volume on this board**, and false under name-derived identity in both cases — which is what makes
  it a probe of the mechanism rather than of the medium, and why it answers correctly on the Orin,
  where `Default` has no device at all.

  What the leg then does:

  | probe | claim violated? | emitted |
  |---|---|---|
  | either | no | `layout.volid -> PASS` |
  | medium-derived | yes | `layout.volid -> FAIL` — the excuse has expired; a real defect |
  | name-derived | yes | `layout.volid.pre -> PASS/FAIL` on the weaker claim, under a **different leg name** |

  The weaker claim is the only shape excusable without VOLID: *one medium reported as two volumes
  **because the two constructor names differ***. Any other shape reds `layout.volid.pre` too, so
  nothing is masked. The day VOLID lands beneath this tree the probe flips, `layout.volid.pre`
  disappears from the transcript, and `layout.volid` must be GREEN — so a
  `layout.volid -> PASS` line always means the full claim, and never a graded one.

  **The probe is consulted only on the branch that is already failing**, so the green path costs
  nothing: a name-derived `same_volume` compares two `&str` and touches no block device. That
  matters for the same measured reason the oracle is lazy (above).

### 5.2 EL0 has no directory namespace

`sys_open` takes an 8.3 leaf, not a path (§2.1 group 3). Until that changes, any file EL0 opens by
name is pinned to the volume root, and the loader's `find_app`-then-`find_in_root` order is what
lets the layout be honest about it rather than pretend. Not this arc's lane.

---

## 6. The gate

`shell::layout_witness` runs on the same two call sites as `vfsroute_witness` — aarch64 after
`emmc2::probe()`, x86 after the storage-ready pass — i.e. the first moment each board has volumes.
Every leg emits `:: TSTE: layout.<leg> -> PASS ::` / `-> FAIL (got …) ::`, and `arroyo`'s standing
fault patterns treat `-> FAIL` as a gate failure, so no leg can go quietly.

`shell::layout_mv_witness` (`layout.mv`, §2.3) rides the **aarch64 bare-metal site only**. It is a
write transcript — create, two renames, four listings, unlinks — and the x86 site is the
storage-ready pass, whose timing this same arc measured breaking under added block I/O
(commit `38b56dba`). The code it convicts is arch-neutral (`fs/vfs.rs`), so the Pi bare-metal gate
convicts it for both arches.

`layout.apps` asserts three things about the LIVE table, and the prefix `/apps` is **spelled out**
rather than read from `EXEC_ROOT` — reading the constant under test would make the leg say only
"programs live wherever `EXEC_ROOT` points", which is true of every value and convicts nothing:

1. a program in `/apps` resolves (`VUG.ELF` when staged, else the first file listed);
2. `/fat` is gone — not a mount prefix, does not `stat`, and does not answer for the leaf;
3. a bare name resolves through `EXEC_ROOT` to the `/apps` path. **This is the failable leg.**

Proved failable on `kernel8-test`, on this arc's final code, by pointing `EXEC_ROOT` back at
`/boot` and running the gate unmodified:

```
RED    :: TSTE: layout.apps -> FAIL (got probe=VUG.ELF apps_resolves=true fat_prefix_gone=true
                                     fat_gone=true bare=None exec_root=/boot) ::
       MBENCH FAIL — 119/119 required witnesses, 1 forbidden hit(s)      exit 1

GREEN  :: TSTE: layout.apps -> PASS ::
       :: TSTE: layout.volid -> PASS ::
       MBENCH PASS — 119/119 required witnesses, 0 forbidden hit(s)      exit 0
```

`layout.mv` asserts, per direction: the destination lists the new leaf, and the source lists
**neither** the old leaf (it really moved) **nor** the new one (it did not land in the source's own
space under the destination's name). The absence half is the load-bearing one — both wrong outcomes
leave a file that exists somewhere, so a presence-only assertion passes on the bug. It also asserts
`roots_differ`, without which the leg would be a passing no-op on a build that quietly un-rooted
`/apps`. The two directions stage their own probes rather than chaining, so neither can hide behind
the other's failure.

Proved failable on `kernel8-test`, on this arc's final code, by stubbing the translation in
`MountTable::rename` back to `(bf, relf, relt)` — the pre-fix body, nothing else touched:

```
RED    :: TSTE: layout.mv -> FAIL (got roots_differ=true ("" vs "/APPS") a_at_dst=false
              a_src_clean=false b_at_dst=false b_src_clean=false
              said_a=[moved /boot/LAYMV1.TMP -> /apps/LAYMV2.TMP]
              said_b=[moved /apps/LAYMV2.TMP -> /boot/LAYMV3.TMP]) ::
       MBENCH FAIL — 119/119 required witnesses, 1 forbidden hit(s)      exit 1

GREEN  :: TSTE: layout.mv -> PASS ::
       MBENCH PASS — 119/119 required witnesses, 0 forbidden hit(s)      exit 0
```

Read the two `said_` fields in the RED verdict: **the verb reported success in both directions while
all four location assertions were false.** That is the defect's whole character, on the wire beside
the verdict.

A board with no `/apps` mount, or one whose medium has no `APPS/` directory (a card staged before
this layout), **skips with a stated line** rather than failing — the honest answer, and the reason
`./arroyo test` on the default pattern image does not red. `layout.mv` skips the same way when a
board binds only one of the two prefixes, reports them as different volumes, or vetoes writes (before
SDWRITE, §1.5, that was the Orin's read-only card; with `sdwrite` on, the card admits the write and
the skip is reached only by a genuine veto).

---

## 7. What was renamed, and what deliberately was not

The `/fat` → `/boot` sweep is **path-scoped, never token-scoped**. The pattern is `/fat` NOT
followed by `[A-Za-z0-9._-]`, which matches the path component and cannot reach `fs/fat.rs`,
`fatperf`, `/fatty.bin`, `builder/fat.img`, `fat-gpt.img`, `fat16.img` or the `fatverb` witness
tags. **The letters "fat" are not renamed anywhere.** In particular
`unaos/scripts/specs/pi4-regression.spec`'s four scored directives — `REQUIRE FATDIRS:`,
`FORBID FATDIRS:`, `REQUIRE FATMOVE:`, `FORBID FATMOVE:` — and their emitters in
`arch/aarch64/syscall.rs` are byte-identical. A blanket rename would have reddened the two
REQUIREs loudly and left the two FORBIDs printing clean while guarding nothing.

**Historical records were not rewritten.** `docs/dev/evidence/`, `docs/MILESTONES.md`, `review/`
and the ledger rows quote wire captures of what actually flew; re-spelling one would falsify it.
Where those files say `/fat`, they are correct about the boot they describe.
