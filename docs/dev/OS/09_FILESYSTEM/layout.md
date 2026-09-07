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

### 1.2 Jetson Orin Nano (`hw-jetson`, `tegra` + `sdmmcroot`)

The shared builder above ran, and then `sdmmc_tegra::sdmmc_root_bind` (§ROOTFS,
`arch/aarch64/sdmmc_tegra.rs`) **re-pointed both `/` and `/fat`** at the card's FAT through
`BlockSource::TegraSd`, read-only. It had to: this machine has no UnaFS volume and no `Default`
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

## 2. The namespace this arc establishes

```
/          → the native root      (UnaFS on the Pi; the card's FAT on the Orin and on x86)
/boot      → the volume this machine booted from
/apps      → the programs on that volume   (= /boot's APPS/ directory)
/usb       → the hot-plugged FAT stick, when it enumerates
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

---

## 3. Nothing empty was created

Peter's ruling: lay out only what exists. There is **no** `/etc`, `/home`, `/tmp`, `/var`,
`/bin`, `/lib`, `/dev` or `/proc` — not as directories, not as mount points, not as reserved
prefixes. Every one of those would be a promise about a subsystem that does not exist yet, and an
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

  **It is EXPECTED RED on the Orin (`sdmmcroot`) until VOLID lands**, and green everywhere else:
  on the Pi `/` is native UnaFS and `/boot` is FAT, so the oracle says "different" and the
  implication holds vacuously (the witness text prints which); on x86 both prefixes carry one name
  and both sides say "same".

### 5.2 EL0 has no directory namespace

`sys_open` takes an 8.3 leaf, not a path (§2.1 group 3). Until that changes, any file EL0 opens by
name is pinned to the volume root, and the loader's `find_app`-then-`find_in_root` order is what
lets the layout be honest about it rather than pretend. Not this arc's lane.

---

## 6. The gate

`shell::layout_witness` runs on the same two call sites as `vfsroute_witness` — aarch64 after
`emmc2::probe()`, x86 after the storage-ready pass — i.e. the first moment each board has volumes.
Both legs emit `:: TSTE: layout.<leg> -> PASS ::` / `-> FAIL (got …) ::`, and `arroyo`'s standing
fault patterns treat `-> FAIL` as a gate failure, so neither leg can go quietly.

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

A board with no `/apps` mount, or one whose medium has no `APPS/` directory (a card staged before
this layout), **skips with a stated line** rather than failing — the honest answer, and the reason
`./arroyo test` on the default pattern image does not red.

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
