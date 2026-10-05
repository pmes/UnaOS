# UNAFSGROW — the UnaFS volume grows to its partition (B347)

Branch `exec-rmbp-unafsgrow`, cut from e78cf228; INSTALL3 (B342, `exec-rmbp-install3`, which carries
AHCIROOT B332) merged first, clean. No new knob: the library verb is every build's; the installer leg
rides `ahciroot`/`installdemo` + `unafs`; `tests unafsgrow` rides `unafs` (x86) and the aarch64 native
module.

## Design (written before the code)

**Finding.** AHCIROOT and INSTALL3 lay a p2 of the operator's size, but the volume inside keeps the
mirrored card size or a fixed 4 GiB (`FRESH_UNAFS_MIB`); the unafs crate has no way to make a volume
larger. The census cannot read the SD card (`BlockTarget`'s `Sdhc` arm is `NotReady`, so the card is
missing or unreadable), and `partition::mint_disk_grant` prints `grant minted … WHOLE-DISK` on the
glass's DRY RUN too.

**The on-disk truth the grow must respect.** UnaFS v3+ has no journal, no block bitmap and no inode
table: allocation is the REFCOUNT MAP (`refmap.rs`, u32 per block, persisted CoW each commit as leaves
+ one or two index levels, leaf count = f(block_count)), inodes are reached through the CoW inode map
(id-indexed, grows with `next_inode`, not with the volume). So "extend the bitmap and the inode table"
is: extend the refcount map; the inode map needs nothing. Block 0 (the superblock) was "written once";
the grow is the one rewriter.

**Seam.** `fs-core`: `UnaFS::grow` / `unafs::grow(device, new_blocks)` in the shared no_std crate — the
same function `tools/unafs grow`, the installer and `tests unafsgrow` call (one implementation). The
installer is the fulfiller (`Kernel — driver`, the held install grant) and only adds the call.

**Ordering (new structures first, superblock last).**
1. refuse: a shrink, a size `Superblock::validate` refuses (past the two-level map = 1 TiB, or past one
   level on a pre-v5 volume — the map cannot address it), a device that does not reach the last block;
2. extend the in-RAM map to the new size with the allocation ceiling held at the OLD size;
3. commit with root flag `GROW` (bit 0 of the reserved `flags` word): the fresh map (sized for the new
   volume) is written into blocks below the old end, then the root flips;
4. rewrite block 0 with the new `block_count`, flush;
5. lift the ceiling, commit again (flag clear).
A cut before 3's flip: the old volume, untouched. Between 3 and 4: the old superblock under a root
whose map is larger — mount accepts that ONLY with the `GROW` flag and reads the volume at the old size
(every block the root reaches is below the old end by construction); re-running the grow finishes it.
After 4: consistent at the new size. No format version change (the flag word was reserved, 0 today).

**Milestones.**
- M1 `UnaFS::grow` + `unafs::grow`, mount tolerance, host KATs on image files (format at N, files, grow
  to 4N, fsck clean, files intact, free count = old free + added − the map's own growth; refusals:
  shrink, past the map, pre-v5 past one level, short device; the interrupted grow mounts at the old
  size and finishes on re-run), `tools/unafs grow <img> <blocks>`.
- M2 the installer: after the mirror/fresh volume is fsck'd, grow it to p2 minus the scratch tail,
  capped at the heap-safe `GROW_CAP_BLOCKS`; `[install] stage=grow from=<b> to=<b> <ok|fail|skip>`;
  fsck again. The SSD root then mounts at the new size (mount reads `block_count` off block 0).
- M3 `BlockTarget` reads `Sdhc` (write stays refused), the census shows the card as a read-only
  `not SATA` row; the dry-run mint is silent, the real mint prints.
- M4 `tests unafsgrow`: format a scratch image (RAM), write a witness file, grow by 1 MiB (256 blocks),
  fsck, persist it to `/var/tmp/unafsgrow.img` on the UnaFS root, read it back, mount + fsck the
  readback → `:: UNAFSGROW: from=<blocks> to=<blocks> fsck=ok -> PASS ::`.

**Witness.** `:: UNAFSGROW: from=<blocks> to=<blocks> fsck=ok -> PASS ::` (after one `[unafsgrow]`
detail line); on an install, `[install] stage=grow from=<b> to=<b> ok`.

**Owed.** Growing past `GROW_CAP_BLOCKS` (the in-RAM map is 8 B/block across two views and every
commit rewrites the whole map — the 256 MiB x86 heap and the one-sector AHCI write path make the whole
SSD unmountable/unusable until the map is incremental); online grow of the mounted root; shrink; unflown
(R78).
