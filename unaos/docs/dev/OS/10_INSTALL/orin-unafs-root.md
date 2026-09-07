# Orin storage: the UnaFS system volume and the self-hosting card

Status: DESIGN (orin 0, 2026-08-18). Charter: ROADMAP §1c rungs SH-2 (UnaFS system
volume) and SH-3 (self-install, "orin first") — §1c is on `hw-pi4`'s ROADMAP as of
this writing (commits `80daf328`, `5d84dc66`, `0b2c1180`) and reaches trunk at the
next integration. Prior art: `installer_engine.md` (this directory),
`fox/brief-install-pi2-selfclone.md` (Pi analogue of the Orin self-clone flow).

## 1. Audit: where the FAT→UnaFS story actually stands (2026-08-18, against `origin/UnaOS-gemini` @ `122ed63e`)

The direction ("UnaOS lives on UnaFS, not FAT" — Peter, 2026-08-17) was given with a
question attached: how cleanly did x86 drop FAT? The audit answer is that the premise
does not hold — **x86 never dropped FAT**:

- `unafs-v5` (`694429bc` / `c84c4029`, 2026-08-12) is a volume-cap lift in the shared
  `libs/fs/unafs` crate — a second refcount-map level raising the format cap from
  2 GiB to 1 TiB. It touches zero kernel files, zero x86 files, zero FAT files.
  Caveat carried in its merge message: the in-kernel practical mount ceiling is
  ~16 GiB on the 256 MiB heap (whole-map RAM residency); 1 TiB is a format cap,
  not an operational one.
- On trunk, `fs/fat.rs` (4305 lines) is a first-class, actively developed subsystem:
  the SDHC internal-card backend (`1d47b97a`), bounded in-place SD writes
  (`1509f1b8`), the FAT verbs (`ffdfe062`…), boot-serial volume election
  (`591b f6e6`) all landed 2026-08-07..11, *after* the v5 branch point.
- The `unafs` crate is aarch64-only by construction: `fs/mod.rs` gates
  `pub mod unafs` on `target_arch = "aarch64"` and the Cargo dependency is
  target-gated the same way (stated reason K3-N1: keeps the x86 usbdebug-esp size
  band tight). x86 has no UnaFS root discovery because the crate is not linked.
- The one staged-but-unwired piece is `fs/vfs.rs` (trunk): a reviewed `MountTable`
  spine with both a FAT backend and a UnaFS adapter — the intended convergence
  point, explicitly labelled unconsumed. Essentially no dead FAT leftovers exist.

Consequence for SH-2 ("FAT demoted to the ESP shim only — one medium, one
filesystem, every chip"): on x86 that is future work in the rmbp/trunk lane (link
the crate, take the size-band hit or negotiate it, route the verbs through the VFS
spine). **Named here, not acted on — out of this track's lane.** The Orin side,
below, is in-lane and does not depend on it.

## 2. Where Orin stands

The Orin boots from a FAT32 `UNAOS` volume on microSD, and its installer flow
(`arch/aarch64/sdmmc_tegra.rs`) *creates* FAT: `install::{gpt, fat32}` clone the
USB stick's ESP file-by-file onto the card. The Pi, by contrast, already runs a
genuine UnaFS system volume (FAT only as firmware boot partition).

The Orin's gap is one layer below the filesystem: `sdmmc_tegra` never registers
itself with `drivers::block` — it only consumes `block::info()` to find the clone
source. Every SD arm in `drivers/block.rs` is gated
`cfg(all(aarch64, feature = "baremetal"))` (the Pi's emmc2). So the Orin microSD is
invisible to the block layer, and `fs::unafs::SdSectorDevice::open()` would return
`MountError::NoStorage`. The `unafs` crate itself is already linked into an Orin
build (the aarch64 target gate covers it) — that part is free.

## 3. Design: the Orin UnaFS system volume

Medium honesty first: the only storage the Orin track has proven on metal is the
microSD (via `sdmmc_tegra`) and USB sticks (via xHCI, with the standing wall
rules). The devkit's M.2 NVMe slot is unbrought-up hardware. So "UnaFS on the
fixed medium, microSD as boot shim" lands in two stages: the UnaFS volume goes ON
THE CARD first (beside the ESP — same medium, correct layout), and moves to NVMe
when that bring-up happens. The card layout is identical either way; only the
block backend under it changes.

Target card layout (GPT):

| # | Partition | FS | Role |
| - | --- | --- | --- |
| 1 | ESP (`UNAOS`) | FAT32 | boot shim only: UEFI spec requires it; carries EFI/, kernel.elf, the boot-serial guard. Never grows a system role again. |
| 2 | system | UnaFS v5 | the root: programs, ACL store (K1/K2 `UNAFS.ATR`), state. Found by `unafs::adapter::locate_unafs` (superblock MAGIC scan of the partition table). |

Work items, dependency-ordered (each commit-sized):

1. **Block-backend registration** — register `sdmmc_tegra`'s sector read/write as a
   `drivers::block` backend, tegra-gated. Precedent: the x86 `Sdhc` pattern from
   `1d47b97a` (a new handle variant, no precedence rule to get wrong) rather than
   widening the emmc2 `baremetal` arms. Read path first; writes stay behind the
   existing three-gate `sdmmc_arm` ladder untouched.
2. **Sector-count arm** — `SdSectorDevice::open()`'s size preference is hardcoded to
   `emmc2::card_num_blocks()` under `all(aarch64, baremetal)`; add the tegra arm so
   the mount is sized from the card, not a possibly-USB-clobbered global (the
   PI-FS-2 bug class, pre-empted).
3. **Installer format path** — beside `fat32::format_esp`, a `unafs` format step in
   the `sdmmc_tegra` install flow: partition 2 formatted UnaFS v5, extent
   sha-verify through the same `install::hash` primitive both flows already trust.
   The three-gate escalation and GPT-refusal ladder are untouched — the payload is
   the only thing that grows (the INSTALL-PI-2 rule).
4. **Mount + witness** — boot-time mount of the system volume behind a knob
   (default off until metal-confirmed), with a witness line naming volume version,
   block count, and ACL-store presence. Everything above the mount is Pi-proven
   shared code (`with_unafs`, K8a CoW commit, K1/K2 ACL persistence).

## 4. Self-hosting: the card never leaves the slot

SH-3's forcing function is the Orin microSD swap pain. With the layout above, the
self-hosting loop is:

- **Update in place**: a new kernel/system payload arrives over the network (or
  USB) onto the UnaFS system volume as staged files; the ESP swap is the last,
  smallest step — write the new `kernel.elf` + EFI/ to partition 1 through the
  installer engine's existing FAT writer, sha-verify, reboot. The staged payload
  stays on UnaFS until the new boot proves itself (witness on the wire), giving a
  one-deep rollback: re-write the previous ESP payload from the retained stage.
- **Self-clone stays**: the existing `sdmmc_tegra` clone flow (USB → card) remains
  the recovery/first-install path; it grows the partition-2 UnaFS format step and
  otherwise keeps its ladder.
- **The network arm** is the installer multi-arc (`UnaOS_Installer` vessel, Vein
  socket — see the companion design, `vein-smart-installer.md`): images pulled
  over the network land on the UnaFS volume through the same staged-update seam.
  This is exactly why the track's network state (first DHCP lease boot-45, NET-4A
  workaround standing) is the critical path.

## 5. First commit-sized step (this arc)

Item 1 above: the tegra block-backend registration, read path, tegra-gated,
knob-off byte-identical for non-tegra builds. Gate: `UNAOS_TEGRA=1 ./arroyo check`
both arches + `./arroyo test-arm` + tegra media strings-validation. Metal proof of
the mount is attended bench work, not this arc's job.

## 5a. Item-4 status: the root binds to the card's FAT until the UnaFS volume exists (ROOTFS, orin 16, 2026-09-06)

Items 1 and 2 landed (TEGRA-SDBLK: `sdmmc_census` publishes the card through
`block::register_tegra_sd`, `BlockSource::TegraSd` routes `fat.rs` at
`read_block_tegra_sd`, and `SdSectorDevice::open_on` carries the tegra
sector-count arm). **Item 3 has not run on any card** — no Orin card carries a
UnaFS partition — and that is what ledger A28 turned out to be: on render7 the
desktop's `ls /` and quarry both answered `/: backend error: unafs-mount`,
because `shell::vfs_mount_table` binds `/` to `NativeBackend` (native UnaFS)
unconditionally, and `/fat` to `BlockSource::Default`, which no Orin boot ever
registers. Two mounts, zero volumes.

Item 4 is therefore taken at the layer the medium can serve today, behind
`UNAOS_SDMMCROOT=1` (cargo `sdmmcroot`, ⇒ `sdmmc`; default OFF ⇒ byte-identical):

- `/` rebinds to the card's **FAT** volume through `BlockSource::TegraSd`,
  READ-ONLY (`write_veto` on the source, and `write_block_tegra_sd` refuses in
  every cfg — the card's only writer is still the armed ladder).
- `/fat` re-points at the **same** volume. It named the unregistered `Default`
  device, and it is not decoration: `/fat` is `shell::EXEC_ROOT` (the second
  probe of `exec_resolve`, which is why a bare `vug` works from anywhere) and
  the literal prefix of `/fat/VUG.ELF`, `/fat/STAT.ELF`, `/fat/ELFHELLO.ELF`
  and quarry's double-click route. Unmounting it would trade one dead namespace
  for another; re-pointing it is what lets the desktop launch anything. Two
  adapters over one source is safe — `FatBackend` re-mounts per call and holds
  no volume state — and quarry's `root_prefixes` already treats a mount point
  claimed by another mount point as a child, not a second root.
- Witness: `[sdmmc] root mount source=tegra-sd … -> OK label="…" …` then
  `[sdmmc] root bound / and /fat = tegra-sd FAT read-only … entries=N dirs=… files=… list_us=…`,
  or one named `[sdmmc] root -> REFUSED reason=…`.

Cost: every sector is one polled CMD17 at 1-bit default speed (≤25 MHz) —
`tegra_sd_read_blocks_512` loops that primitive, because the only multi-block
path in the file is `install_target`-gated and unproven. The deciding term is
the sector count of a listing (a volume re-probe of ~4–6 sectors plus the root
directory's chain), not the clock. The witness carries the two times itself
(`probe_us`, `list_us`); the SECTOR term would come from `UNAOS_FATPERF=1`, which
`arroyo` wires only into the Pi `kernel8` leg today — extending that mapping to
the jetson image is a named follow-up.

**This does not retire item 3, and the layout in §3 is unchanged.** When the
UnaFS system partition is formatted, `/` goes back to `NativeBackend` and the
FAT returns to the boot-shim role §3 gives it; `sdmmc_root_bind` is the one
place that changes. Code: `arch/aarch64/sdmmc_tegra.rs` §ROOTFS,
`fs/vfs.rs`'s tail `FatBackend::new_tegra_sd`, and one cfg-gated statement in
`shell::vfs_mount_table`.

### 5.1 UNAFSROOT (orin 20) — item 3 HAPPENED, and item 4 did not notice

The paragraph above is the prediction; this subsection is it coming true, plus
the reason it took a round to notice.

**Item 3 has run.** A card in the bench Orin carries a UnaFS volume at MBR
slot 2. The render9 capture
(`~/unaos-bench/scratch/orin19/pointerlag/boot-render9.log`) proves it twice
over, from two independent readers, at lines 745 and 746:

```
:: PART: unafs span check — slot=2 type=0x7f part=[114688..131072) span_base=114688 span_blocks=2048 fits=yes magic=ok ::
:: TEGRA-UNAFS: native unafs volume MOUNTED read-only on TegraSd — card 62333952 sectors, 0 committed root flip(s) on the volume ::
```

**And the binder contradicted them, in the same boot, 117 lines later:**

```
[sdmmc] root bound /, /boot and /apps = tegra-sd FAT read-only (all were dead: unafs has no volume here, Default has no device) …
```

The parenthesis was a **string literal** inside the census `serial_println!`. It
printed on every boot regardless of the medium, and `sdmmc_root_bind` aliased
`/` onto the FAT ESP on that stated premise. There was no wrong check — there
was **no check**, and a claim that cannot be wrong on any boot is never
falsified by any boot. No amount of flying would have surfaced it.

**The fix is that the claim became a measurement.** `root_probe` now derives the
fact from `fs::unafs::locate_on(BlockHandle::TegraSd)`, which walks the card's
own partition table through the card's own routed reads (the SDSEAM "one disk
end to end" discipline). This is `drivers/emmc2.rs::onecard_witness`'s shape
ported from the Pi, where `absent` has always been a returned error value rather
than a word. Three outcomes, and the wire says which:

| verdict | `/` | when |
|---|---|---|
| `ROOT_NATIVE` | **left on `NativeBackend`** — the shared builder already mounted it | `locate_on(TegraSd)` returned `Ok` |
| `ROOT_BOUND` | the card's FAT, as before | the partition walk returned an error, **which is printed** |
| `ROOT_REFUSED` | untouched | no published card (unchanged) |

Note the shape of the first row: *binding the native volume is spelled as **not
clobbering** the mount `shell::vfs_mount_table` already made.* `/boot` and
`/apps` still re-point at the FAT ESP, which is why program launching is
unaffected — `EXEC_ROOT` and the literal `/apps/VUG.ELF` paths reach the same
files they always did.

No new mount mechanism was needed. `fs/unafs.rs`'s `bind_probe_admitted`
already carries its `BlockHandle::TegraSd => true` arm, so `with_unafs` — and
therefore `NativeBackend` — could already discover the card's volume whenever
the global path has none. The only thing standing between the Orin and a native
root was this file overwriting it.

**Witness, new form.** The medium census, then the bind:

```
[sdmmc] root medium native=present base_lba=114688 blocks=2048 (8 MiB) writable=no (block::write_block_tegra_sd refuses in every cfg) | Default=absent-so-far -> / = NATIVE unafs ::
[sdmmc] root bound / = native-unafs-ro rode=TegraSd, /boot and /apps = tegra-sd FAT read-only entries=N dirs=N files=N list_us=N ::
```

`rode=` is `fs::unafs::mount_bound_handle()` — the handle the shared unafs mount
**actually** bound, read back after the first listing forced the lazy bind. It
is on the line for one reason: `root=native-unafs-ro` with `rode=Global` or
`rode=Usb` would mean `/` is some *other* disk's volume (UNAFSBIND rule 1 is
"Global wins whenever it holds a volume", and on an Orin `Global` is the USB
stick). That is the "assembled from two disks" defect, and it now says so on the
wire instead of reading as a success.

The fallback prints its reason rather than asserting one:

```
[sdmmc] root medium native=absent (NoVolume) | Default=… -> / falls back to the card FAT ::
```

`jetson-sync1.spec` carries `FORBID unafs has no volume here` so the literal
cannot come back silently. It was proved RED on render9 (2 hits, `MBENCH FAIL`)
and GREEN on the same boot with only the census line replaced, before it landed.

#### The two constraints that ARE real — and the size correction

Two things could still make an operator want `/` elsewhere. **Neither of them is
"there is no volume", and that distinction is the whole arc.**

- **Read-only.** `block::write_block_tegra_sd` refuses in every cfg with
  `BlockError::NotReady`. A write verb aimed at `/` therefore **REFUSES**, below
  anything this file could weaken. That refusal is asserted, not hidden: a
  correctly-reported refusal is the fixed behaviour, the same posture `XVOL`
  takes on the cross-volume `mv`.
- **Size — and it is 8 MiB, not 1 MiB.** `PartitionSpan::block_count`
  (`libs/fs/unafs/src/adapter.rs`) counts **whole 4096 B blocks**, so render9's
  `span_blocks=2048` is 8 MiB. The wire corroborates independently:
  `part=[114688..131072)` is 16384 sectors x 512 B = 8 MiB, and `fits=yes` with
  2048 x 8 = 16384 means the volume fills its partition exactly. Reading
  `block_count` as 512 B sectors gives the 1 MiB figure, an 8x undercount.

Making the volume writable, and growing it, are separate arcs. Section §3's
layout is still the layout of record; what changed here is only that `/` now
believes the card.

#### Consequence to expect on the first flight

`entries=` on the census reports the **native volume's** root from now on, not
the FAT ESP's, so the count will change and may be zero. That is a statement
about what the installer wrote into the 8 MiB partition, not about the binding.
`shell::layout_volid`'s probe degrades honestly on an empty root (it prints
`lists no file — ls/cat/stat legs skipped`), and its `vfsroute.samevol` leg
keeps passing because its oracle is observed rather than reported: with `/` on
UnaFS and `/boot` on FAT the Orin now takes the **Pi's** shape, where the two
prefixes genuinely are two filesystems.

## 6. Open calls (named, not acted)

- **NVMe bring-up** — new hardware lane work; sequenced after the card-resident
  volume proves the layout.
- **x86 FAT demotion** (SH-2's other half) — rmbp/trunk lane; the VFS spine
  (`fs/vfs.rs`) is the staged convergence point.
- **r8169 firmware load** (the NET-4A real-fix candidate) — LICENSING, Peter's
  call, unchanged.
