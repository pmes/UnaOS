# VOLUMES2 (rmbp-ledger B376) — R89: Volumes = UnaOS + boot; EFI is boot/efi

**Finding (flight 23 wire).** `:: VOLUMES: … shown=EFI,UnaOS,boot -> PASS` — the third entry, `EFI`, is not the
card's ESP at all: it is the rMBP's INTERNAL SSD's own EFI system partition (`[vfs] volume mounted /volumes/EFI
source=ahci0 rw=no ::`, f23-boots.log line 129 / 3337), mounted by the HOMESOIL rule (`bootdisk::bind`, every
non-root disk at `/volumes/<label>`). And under `/volumes/boot` the presentation showed `EFI` (Quarry only — the
shell's `ls /volumes/boot` listed the whole FAT: APPS, SYSTEM, KERNEL.ELF …). The B366 gate checked only that
`boot` and `UnaOS` were AMONG the entries, so it passed a list Peter rejected.

**The seam.** `fs/volumes.rs` (CHARTER: Kernel — fs-core) stays the one place the Volumes layout is decided:
* `bind_aliases` (already called at the tail of `bootdisk::bind` with the root known): on a native UnaFS root, a
  non-root disk's FAT volume named `EFI` (a firmware system partition of another OS) is WITHDRAWN from `/volumes`
  and said once on the wire. HOMESOIL's rule for other disks (sticks, Pi/Orin cards) is not changed.
* `publish(path, rows)` — the ONE presentation of `/volumes/boot`: only its EFI tree, under the name `efi`.
  Called from `shell::vfs_ls_collect` (so the shell's `ls`, Quarry, Quarry2 and the gate read one listing);
  Quarry's `view` keeps its `/` rule (no `boot` row at the root) and delegates the boot rule to it. FAT lookup is
  case-insensitive, so `/volumes/boot/efi/BOOT/BOOTX64.EFI` resolves on the medium's `EFI/BOOT/BOOTX64.EFI`.
  `/boot` itself is untouched (every verb, spec and loader path names it).

**Milestones.** M1 the publish rule (`efi` under `boot`, shell + Quarry) · M2 the foreign ESP withdrawn from
`/volumes` on a native root · M3 the gate rewritten to R89.

**Witness lines (x86 metal shape, no extra knob; `tests volumes` is registered in every build).**
* boot: `[vfs] volume /volumes/EFI withdrawn — another disk's EFI system partition is not a volume (R89: Volumes = UnaOS + boot) ::`
* `tests volumes`: `:: VOLUMES: shown=UnaOS,boot efi=boot/efi -> PASS :: root_view=apps,home,system,var,volumes pure=true home_on_fat=0 withdrawn=EFI ::`
  RED (`-> FAIL`) on any other `/volumes` list, on `/volumes/boot` showing anything but `efi`, or on
  `/volumes/boot/efi/BOOT` not resolving.

**Owed.** A removable stick on a native root still mounts at `/volumes/<label>` (HOMESOIL) and the gate reads RED
with it in, naming it — whether a stick belongs in Volumes is a question for Peter, not guessed here (R89 §3).
