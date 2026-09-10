# INTEGRATE AMENDMENT 01 (pi 9, 00:30Z) — apply in commit 1 if read; else the seat's follow-on before the gate
`shell.rs` `RESERVED_VOLUME_PREFIXES` (`&["/usb", "/boot"]` at 98213b7f; consumed by `unmounted_reserved_volume`)
is a STATIC list of mount-point spellings whose purpose (VFS-4) is: a verb aimed at a reserved volume that is not
mounted answers "volume not mounted", never falls through to the native root's bare -ENOENT (the P44 incident in
its comment). With label mounts at `/volumes/<LABEL>` that list matches nothing and the regression returns.
FIX (structural, no list of labels): the "volume not mounted" answer is derived from the LIVE mount set — any
path whose first component is the `/volumes` namespace root and whose second component is not a current mount
point answers "volume not mounted: /volumes/<name>" (the namespace root `/volumes` itself lists the mounted
names). `/volumes` is the ONE static spelling (the OS's own convention, like `/boot`); everything below it is
runtime. Keep `/boot` in the reserved list. Delete `/usb` from it (no such mount point exists any more) and update
the comment to say what replaced it. RED-first: `vfs write /volumes/NOPE/x` with no such volume → "volume not
mounted", never -ENOENT; a fixture in the existing VFS-4 leg's shape.

## (rmbp 16, 00:40Z) on label mounts — all in the same function, no battery
1. DEVICE DEDUPE is IN the frozen set: eb9996cc (FOLLOWUP's committed home-soil commit, which INTEGRATE builds on)
   dedupes by (num_blocks, BS_VolID) — verify it is applied to the non-root mounts too, not only to the root walk:
   one card reached as both Default and Usb (block.rs:705-709) must yield ONE `/volumes/<label>` mount, never
   `<label>` + `<label> 1`. RED-first: publish one device into both statics → one mount, `aliased=` on the witness.
2. COLLISION SUFFIX IS DETERMINISTIC across boots: colliding labels are ordered by a fact on the medium (BS_VolID
   ascending), never by enumeration order — the same two cards get the same paths on every boot regardless of what
   else is plugged in (B90's third class).
3. "Sanitizes to empty" is its own arm: a label the whitelist strips to nothing (e.g. `+++++`) → `Untitled`, never
   `/volumes/` or an empty component. Distinct from "unnamed" (all-spaces / `NO NAME`) in the witness reason.
4. `.` and `..` are rejected EXPLICITLY after sanitization (a label is 11 attacker-controlled bytes used to build a
   path); RED-first fixture: label bytes `..         ` → `Untitled` with `label_raw=`.
5. (rmbp 16, 00:50Z — Peter's scenario) `DiskId = (num_blocks, BS_VolID)` MERGES CLONES: a card and the card it was
   imaged from share size and BS_VolID (byte copies inherit it) → deduped → one card silently HIDDEN from /volumes
   with `aliased=` claiming one device. FIX: dedupe ONLY when the two sources PROVABLY resolve to the same underlying
   device — on the tegra build the block registry answers this by construction (`publish_usb_geometry` stores ONE
   `dev` into both BLOCK_DEVICE and USB_BLOCK_DEVICE: compare the registry entries' identity — the same
   `BlockDeviceInfo` value/slot id published by the same call — not the FAT contents). When DiskId matches but device
   identity is NOT provably the same → MOUNT BOTH, `aliased=ambiguous` on both witnesses (two friends who look alike
   are two friends). This does NOT reopen the root refusal Peter overruled: mounting is not exclusive; root stays
   first-found. Apply the same rule to the root walk's per-disk counting (two clones = two disks: `matches=2`).
   RED-first: (a) one device in both statics → one mount, `aliased=usb->global`; (b) two devices with identical
   (num_blocks, BS_VolID) → two mounts, `aliased=ambiguous`, suffix by BS_VolID order then by source name.
6. (rmbp 16, 01:05Z) `matches=N` must be actionable: the root line's `source=` and `serial=` ARE the winner — keep
   them, and for clones (identical serial) the SOURCE NAME is the only physical discriminator, so `home=` lists every
   other match as `<source>:<path>` and the root line keeps `source=` first. An operator must be able to read which
   physical card took the writes from the wire alone.
7. (rmbp 16, 01:10Z) SAY AT THE SITE (bootdisk.rs witness + vfs.md §14): the mount PATH is label-derived BECAUSE bus
   names are meaningless to an operator (Peter's ruling), and the WITNESS carries the source name BECAUSE it is the
   only thing that distinguishes identical media — same token, two jobs, one ruled out and one required. A future
   cleanup that strips `source=` "for consistency" deletes the one field that answers Peter's own question.
