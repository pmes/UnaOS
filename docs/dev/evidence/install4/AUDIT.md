# INSTALL4 — the installer re-walked against today's tree (rmbp-ledger B619)

Peter, 2026-10-14: *"is installing unaos onto the rmbp hard drive coming soon"*. The installer's rungs
(SELFINSTALL B271, SELFINSTALL2 B310, AHCIROOT B332, INSTALL3 B342) were folded at merge12 (boot 22,
`cfc307f2`); boot 19 never ran the verb and boot 20 answered it `unknown command` (B271's status,
ST130; B310's status). Twelve images of folds later, this is the path re-read module by module at
`513ca5f8`, before anyone points it at the rMBP's SSD. Design written before the code (R106); each
seam is one commit on `exec-rmbp-install4`.

Rulings that bound every row: **R25** (a non-UnaOS disk is a STRANGER; it changes only by an
operator's explicit act), **R3** (the ⌥ picker is the human step), **R29** (no UEFI variable
services in the loader), **R99** (the boot FAT holds only what firmware needs; `/apps`, `/lib`,
`/home` live on UnaFS), **R100** (the administrator's authority is what `install` checks), **R86**
(nothing runs at boot that the boot does not need).

## The audit table

| # | seam | what it does today (513ca5f8) | drift since boot 22 | fix, or refused |
|---|---|---|---|---|
| 1 | the verb on the image | `shell.rs:5606` `install` arm is `#[cfg(feature = "installdemo")]`; `install ssd …` dispatches to `selfinstall::verb` under `x86_64 + ahci`. `scripts/image-line.env` carries no `UNAOS_INSTALLDEMO` and names `UNAOS_AHCIROOT` "install sitting only" | none in the verb; the line never carried it (B310's flight-20 `unknown command`) | **fix (C4):** `UNAOS_INSTALLDEMO=1`, `UNAOS_AHCI_WRITE=1`, `UNAOS_AHCIROOT=1` on the line, in arroyo's knob order. All three already live in the five places (arroyo map, Cargo.toml, builder, banner-cert rows `installdemo`/`ahci-write`/`ahciroot`, k8-reach rows) |
| 2 | the SATA census read | `install/mod.rs:207`: `BlockTarget`'s `Ahci` READ arm is `NotReady` unless `ahci-write`; `probe` → `gpt::read_table` → `partition::census` all read through it | none | **knob, not code:** without `ahci-write`, `install ssd --dry-run` prints `probe err=NotReady … REFUSE`. Row 1's `UNAOS_AHCI_WRITE` opens the read; every write still needs a grant (row 6) |
| 3 | the unattended boot fixture | `main.rs:1817`/`:6048` call `install::install_probe_once` on every `installdemo` image without `instgui`; it runs `partition::run_fixture` → (under `ahci-write`) `sata_fixture`, which censuses every SATA GPT disk, picks the first slot that passes the ladder (empty, not foreign-typed, not ESP) and WRITES `demo_tree` into it with a grant it mints itself; else `run_demo` lays a GPT on a blank global disk. No operator anywhere (SITTING-1 STOP 1) | unchanged since PARTINSTALL; harmless only because the knob was never on media | **fix (C1):** the writers run only where `tests-at-boot` is compiled (every QEMU lane, never media — R77 M3); an image prints `[install] armed, verbs only: …` once and writes nothing. Folded on the existing line in `install_probe_once` (`install/mod.rs`, which `main.rs` calls — `main.rs` is PISPEC's) |
| 4 | which SATA disk the verb names | `selfinstall.rs:75` `first_ahci()` = registry index 0. On q35 the builder's boot ESP is `ide.0` = AHCI port 0, so every QEMU `install ssd` probed the boot ESP (a stranger, MBR) and never the virtual SSD on `ide.1` | none (latent since SELFINSTALL) | **fix (C2):** the first AHCI disk that is not the boot device (`selfguard::refuses` / `partition::sata_is_boot_device`), else index 0 so the refusal still names it. On the rMBP (SSD = port 0, boot = the SD card) the answer is unchanged |
| 5 | the clone source | `selfinstall.rs:200` `source_is_ahci` refuses any SATA source ("already running from the SSD"), so a machine that booted from a SATA ESP (every QEMU lane) can never install onto a SECOND SATA disk | none (latent) | **fix (C2):** refuse only a source on the TARGET port (`source=ahci:<p>` with p = the target). The rMBP card boot is unchanged; the QEMU lane can now clone the ESP it booted |
| 6 | the write grant | `WriteGrant::new` has one call (`partition::grant_range`); minted by `mint_grant` (one partition), `mint_disk_grant[_quiet]` (whole disk, Blank/Ours/Confirmed only, never the boot disk), `mint_root_grant` / `mint_scratch_grant` (AHCIROOT). `block::write_sectors_granted` is the one caller of `ahci::write_block_at`; the AHCIROOT slot (`hold_ahci_grant`) admits the plain port path only inside a held range; `GrantHold`'s drop FLUSHes | IMAGE 15 fix (`29a8982f`) gated the quiet minter `x86_64 + ahci-write`; nothing else | **kept as is.** C3 adds two minters behind the same `grant_range`, each refusing unless its extent is free space (no in-use entry overlaps) or a GPT structure outside `[first_usable, last_usable]`, and each taking a `BesideConsent` only the typed token constructs |
| 7 | the stranger (Catalina) | `probe` → `Verdict::Stranger` on any foreign content/type; the plan line is `REFUSE(stranger …)`; the `--write` refusal then PRINTS `install ssd --write --erase-stranger ERASE-<port>-<sectors>`, which (AHCIROOT) clears the verdict and lays a fresh GPT over the WHOLE disk — macOS included. INSTALL3's confirm screen reaches the same erase when the operator types the disk name | none | **fix (C3): the stranger's plan is BESIDE, never a wipe.** The dry run reads the table (primary + backup headers, both arrays, CRCs, geometry), finds the largest free extent and prints `plan=beside(...)` — ESP 512 MiB + UnaFS (the rest) in free space, every existing partition untouched — or the named refusal `no-free-space have= need=` (make room in macOS: `diskutil apfs resizeContainer`). The refusal text leads with the beside path; the erase token stays (it is an explicit operator act, R25) but is no longer the first sentence an operator reads and is NOT in the flight-35 procedure |
| 8 | the GPT write | whole disk only: `gpt::write_plan` lays an `amber_core::Plan` image (MBR, both headers, both arrays) and re-reads it. The one in-place editor is `set_entry_type_guid` (retype one slot). No editor ADDS an entry | none | **finish (C3):** `gpt::read_raw_table` (primary + backup header, both arrays byte-equal, entry size 128, arrays outside the usable range) and `gpt::add_entries` (encode new entries into EMPTY slots only, non-overlapping and inside free space; arrays first, then headers with recomputed CRC-32s; re-read: every old entry byte-identical, the new ones present, backup consistent). Writes go through a `TableTarget` that holds two grants: `[1, primary array end]` and `[backup array, backup header]` — LBA 0 and every partition sector are outside both |
| 9 | the ESP | `fat32::format_esp` + `clone::write_snapshot` + `verify_extents` through a `PartitionTarget` (or the whole-disk `AhciDisk`); the tree is `clone::snapshot_capped` of the RUNNING FAT volume (`bootdisk::locate`), ≤128 MiB/file, ≤160 MiB total, refused unless it carries `EFI/BOOT/BOOTX64.EFI` + `kernel.elf`. Under R99 that volume now holds only firmware's files (ROOTDISK2 `e125b707`) — the shape the SSD's ESP should have | R99 moved `/apps`, `/lib` off the boot FAT: the clone now copies only the firmware set, as it should | **kept.** The beside install (C3) reuses it unchanged. The INSTALLVERB partition verb (`install <disk> <slot>`) still writes the SYNTHETIC `demo_tree` (`partition.rs:954`, SITTING-1 STOP 2): **refused for flight 35** — the rMBP path is `install ssd` |
| 10 | the UnaFS root | `unafsmirror::clone_span` sector-clones the running UnaFS span (fenced on its root block: `SourceMoved` if it commits mid-copy), `fsck_target` mounts the copy, `grow_unafs` grows it to p2 minus the 64-sector scratch tail; with no running UnaFS, AHCIROOT formats a fresh 4096 MiB volume (`UnaFS::format`) through the held slot | UNAFSMAP `fb5a6435` lifted the grow cap to the format wall (2^31 blocks) | **kept.** The beside install reuses mirror/fresh/fsck/grow through an `AhciDisk` whose grant is the two new partitions only, and the slot holds the UnaFS partition only. A FRESH volume has no `/apps` or `/lib` (R99 moved them onto UnaFS): on the image line the card's UnaFS is mirrored, so this is a QEMU-lane fact only |
| 11 | the EFI boot entry | the loader uses no UEFI variable services (R29: zero `SetVariable`/`BootOrder`/`RuntimeServices`). The installed ESP carries the card's `EFI/BOOT/BOOTX64.EFI` — the removable-media fallback path the Mac's ⌥ picker lists as "EFI Boot" without NVRAM | none | **refused by construction (R29, R3):** no `BootOrder`/`BootNext` write is possible without runtime services; the startup disk is the operator's ⌥ choice (or `bless` from macOS, the operator's act). Whether this machine's picker lists a SECOND ESP on the internal disk is a firmware fact not on any wire yet — flight 35 measures it |
| 12 | the first boot off the SSD | `bootdisk::locate` finds THIS kernel by content on every block source; `fat::mount_source` mounts the FIRST FAT volume of a GPT disk (`fat.rs:2018` `scan_gpt`, entry order). AHCIROOT then roots `/` on an OURS SSD's UnaFS "whichever disk the kernel was found on" (`bootdisk.rs:1292`) | none | **whole disk:** works as B332 designed (the installed ESP is the SSD's first FAT). **Beside Catalina — STOP (fs/fat.rs is not this arc's):** the Mac's own EFI partition is slot 0, so the kernel cannot find itself on the SSD's second FAT; with the card inserted the card's identical kernel matches (`/boot` = card) and the SSD's UnaFS is `/` (C3 widens `ahciroot::ours_unafs_part` to a stranger's disk carrying exactly one UNAFS-typed UnaFS partition — the root grant is that partition only). The card-less boot needs `fat.rs` to offer every FAT volume of a GPT disk to bootdisk (exact change in the report) |
| 13 | the glass | `instgui` auto-OPENS at every desktop activation (`desktop_uefi.rs:657`); INSTALL3's five SSD screens are whole-disk only, and a foreign disk's confirm is the typed-name ERASE | KERNELFONT2/UIMETRICS/APPEARANCE re-skinned it; the screens' logic is unchanged | **refused for the image line:** `UNAOS_INSTGUI` stays off (an installer window over every boot breaks R86), so flight 35 drives the shell verb. Owed: the window opened only by `install --gui` (`desktop_uefi.rs`, not this arc's) and a BESIDE screen in `install3.rs` |
| 14 | the QEMU lane | `./arroyo test-install` boots one machine with the five-slot USB GPT fixture and types `install`, `install global 2`, `install global 1`, `install --gui` (x86-install.spec). It never types `install ssd`, never writes a SATA disk through the verb, never boots off one | none | **fix (C5):** after the spec'd partition leg, three SSD legs on their own builds (`UNAOS_AHCIROOT`): (a) a BLANK virtual SSD — `install ssd --dry-run`, `install ssd --write`; (b) a reboot with that disk as the ONLY disk; (c) a STRANGER fixture (APFS-typed, `NXSB`, free space after it) — dry run, the refused write, `--write --beside <token>`, the APFS head and LBA 0 compared byte-for-byte off the image, then (d) a reboot off it. Verdicts by `awk` on each capture |
| 15 | the identity of an installed ESP | `amber_core::fat32` stamps every volume it formats with the serial `UNAS`; `una-card` lays the card with the same core | none | **named, not changed:** once the SSD carries an ESP formatted by the installer, a card whose ESP also reads `UNAS` makes `sata_is_boot_device` call the SSD the boot device — the SAFE direction (a re-install refuses `boot-device`; boot the card with `UNAOS_ROOT_PREFER=sdhc` and re-run the dry run to see it). A per-install serial is owed in `amber_core` |
| 16 | the administrator | FIRSTUSER (`7c7f2479`, R100): under `login`, `install ssd --write` refuses without the administrator's session | new since boot 22 | **kept;** the line carries `UNAOS_LOGIN=1`, so flight 35 types `--write` from the administrator's session. `--dry-run` needs none |

## What C3 adds, stated as mechanism

- `install ssd --dry-run` on a STRANGER: `selfinstall::beside_plan` → `gpt::read_raw_table` (refuses
  `gpt-not-standard` on any inconsistency: header CRCs, `current_lba`/`backup_lba` cross-links,
  array CRCs equal and the two arrays byte-equal, `entry_size == 128`, the primary array ending
  before `first_usable` and the backup array starting after `last_usable`), the free extents of
  `[first_usable, last_usable]` not covered by any in-use entry, the largest one, 1 MiB aligned:
  ESP = 512 MiB at its aligned start, UnaFS = aligned after it through the extent's end, refused
  `no-free-space` when the UnaFS partition would hold less than `p2_min_sectors` (the volume to
  mirror, or the fresh 4096 MiB, plus the scratch tail). Two EMPTY slots are required
  (`no-free-slot` otherwise). Printed: `[install] beside free=<a>..<b> (<MiB> MiB) slots=<i>,<j>`,
  the plan lines, and `[install] beside token=BESIDE-<port>-<sectors>`.
- `install ssd --write --beside BESIDE-<port>-<sectors>` (AHCIROOT builds; the administrator under
  `login`): the same probe; refused unless the verdict is `stranger` (a blank or ours disk takes the
  whole-disk path), the token matches THIS disk, the disk is not the boot device, the slot is free
  (not the live root), and the beside plan recomputed off the live table is the one the token was
  printed for. Then, in order and each step refusing the rest: snapshot the running ESP; mint the
  part grant (free space only) and the two table grants; `gpt::add_entries`; re-census (every
  pre-existing partition's first 4 KiB SHA-256 equal before and after, same rule as
  `neighbours untouched=`); format + clone + verify the ESP; mirror or format + fsck + grow the
  UnaFS; FLUSH (the slot's drop). Witness:
  `:: INSTALL4: beside port=<p> table=ok old=<n>/<n> untouched=<m>/<m> esp=<MiB> unafs=<MiB> cloned=<MiB> files=<n> verified=<n> fsck=<ok|fail> ms=<ms> -> PASS|FAIL ::` (`untouched` counts LBA 0 plus every pre-existing partition). The token compare is case-blind (the QEMU typist types letters lower-case); it still binds the port and the exact sector count.
- Root: `ahciroot::ours_unafs_part` also answers for a `stranger` disk that carries EXACTLY ONE
  partition that is UNAFS-typed AND whose content is a UnaFS superblock (no other OS writes UnaFS);
  the root grant is that partition's extent only, so every macOS sector stays outside it.

## What stays refused

- Any write to LBA 0 (the protective MBR) — no grant in this tree covers it.
- Any write to a partition the census did not create in this run: the beside grants are minted from
  the PRE-edit table and refuse any extent an in-use entry overlaps.
- A `BootOrder`/`BootNext` write (R29: no runtime services; R3: the ⌥ picker is the human step).
- The unattended boot fixture on any image without `tests-at-boot` (row 3).
- The installer window on the image line (row 13).
- `install <disk> <slot>` for flight 35 (it writes the synthetic tree, row 9).
- `--erase-stranger` in the flight-35 procedure: it erases macOS; it needs Peter's own fresh word.
- The card-less boot off a beside install until `fs/fat.rs` offers every FAT volume (row 12).

## The commits (exec-rmbp-install4, parent 513ca5f8)

| commit | seam(s) | what |
|---|---|---|
| `59f801c1` | 3 | the unattended boot fixture writes only under `tests-at-boot` (install/mod.rs, folded) |
| `f6926776` | 4, 5 | `install ssd` names the first non-boot SATA disk; only a source on the target port is refused |
| `9fa6fa3f` | 6, 7, 8, 12 | the beside plan and write: `gpt::read_raw_table`/`add_entries`, `partition::mint_beside_*`, `selfinstall` dry run + `--write --beside`, `ahciroot::ours_unafs_part` |
| `f8077bab` | 1, 2, 13 | `image-line.env`: INSTALLDEMO, AHCI_WRITE, AHCIROOT; INSTGUI named off |
| `51e638c0` | 14 | `./arroyo test-install`: ssd-blank, boot-blank, ssd-beside, beside-image, boot-beside |
| the commit carrying this table | — | the flight-35 procedure (`docs/dev/OS/10_INSTALL/FLIGHT35-INSTALL.md`), pointers in SITTING-1.md and partition-install.md, B619's status |

## Owed outside this arc's files (reported, not taken)

- `unaos/crates/kernel/src/fs/fat.rs` — offer bootdisk EVERY FAT volume of a GPT disk (today `scan_gpt`,
  fat.rs:2018, returns the first): e.g. `pub fn mount_source_all(source) -> Vec<FatFs>` walking
  `gpt_volume_spans` through `parse_bpb`, and `fs/bootdisk.rs`'s per-source probe (bootdisk.rs:970)
  iterating it. Without it the kernel cannot find itself on the SSD's ESP when the Mac's EFI partition is
  the disk's first FAT (the card-less boot off a beside install).
- `unaos/crates/kernel/src/video/desktop_uefi.rs:655-657` — drop the `instgui::open()` at activation so
  the window opens only on `install --gui`; then `UNAOS_INSTGUI` can ride the line. A beside screen in
  `video/install3.rs` follows.
- `docs/SECURITY.md` §"Installer — what it refuses to touch" and §"The SATA write capability" — they
  still say "the whole-disk engine can never obtain [a grant]" and "nothing writes … the partition
  tables in partition mode"; SELFINSTALL's whole-disk grant and INSTALL4's two table grants need rows
  (the beside grants: free space only; the table grants: `1..primary array end` and `backup array..
  backup header`, LBA 0 in neither; all behind the typed token).
- `docs/dev/evidence/foldbuild/IMAGE-LINE.md` — the line's derivation gains the three INSTALL4 knobs.
- `unaos/libs/sys/amber_core/src/fat32.rs` — a per-install FAT volume serial instead of the constant
  `UNAS` (row 15), so the boot serial tells the SSD's loader from the card's.
