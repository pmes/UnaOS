# ROOTDISK (rmbp-ledger B390) — R94: `/lib`, and `/` is the boot disk

## Finding (f24-boots.log, card 3, and the sources)
- `[vfs] apps mount /apps = fat boot volume source=sdhc rooted=APPS ::` and `[vfs] volume alias /volumes/UnaOS = native
  unafs root source=sdhc ::` — `/apps` is a MOUNT of the boot partition's `APPS/` over the root, while
  `/volumes/UnaOS` is a SECOND `NativeBackend` mount of the UnaFS root. `/volumes/UnaOS/apps` therefore resolved on
  the UnaFS volume itself, where no `apps` exists: the alias was a second mount, not the root's own namespace.
- `[quarry] open census cwd=/apps … names: LIB/ …` — the ring-3 libraries live inside the program directory
  (`builder` stages `APPS/LIB`; `linuxabi/ldso.rs` `LIB_DIRS = ["/apps/LIB/dyn"]`; selfbuild3/5/6 consts; tcc is
  configured `--tccdir=/apps/LIB/tcc`; link.rsp is rewritten to `/apps/LIB/rust`).

## The seam (R79) — Kernel, fs-core
The mount table is the one place a name is decided. New file `fs/rootdisk.rs` (`//! CHARTER: Kernel — fs-core`):
- **`/volumes/UnaOS` IS `/`**: `MountTable::resolve` asks `rootdisk::redirect` first; a path under the root's Volumes
  entry (when `/` is native and that entry is bound) is resolved AS the same path without the prefix, through the
  WHOLE table — so `/volumes/UnaOS/apps` reaches the `/apps` mount (same backend object, same relative path: one
  inode, two paths), `mkdir /newfolder` is seen at `/volumes/UnaOS/newfolder`, and `mkdir /volumes/UnaOS/x` lands at
  `/x`. The root's links (`volumes`, `boot`) are not on the volume: refused under the alias (no `/volumes/UnaOS/volumes/…`
  loop). `shell::vfs_ls_collect` lists the aliased path as `/` (one listing: ls, Quarry, gates) minus those two links.
- **`/lib`**: the builder stages `target/LIB` at the boot partition's root `LIB/` (not `APPS/LIB`); `bootdisk::bind_root`
  mounts `/lib` = the boot FAT rooted at `LIB` (the `/apps` shape). The loader's search dir, the selfbuild consts, tcc's
  `--tccdir` and link.rsp all name `/lib`. COMPAT for ONE image: `/apps/LIB/…` resolves on `/lib` and says once per
  subtree `[rootdisk] compat /apps/LIB -> /lib path=… task=… (R94: one image, then it goes) ::`.
- **Links marked**: at `/` (and `/volumes/UnaOS`) the root's special entries — `apps`, `lib` (the boot partition's
  directories), `boot`, `volumes` — are drawn by Quarry with `ls -F`'s link mark `@` instead of `/`.

## Milestones
M1 the alias in `resolve` + the listing · M2 `/lib` (mount, builder, arroyo, loader/selfbuild paths, compat) ·
M3 Quarry's link mark · M4 `tests rootdisk`.

## Witness (x86 metal shape, no knob; `tests rootdisk` is registered in every build)
- boot: `[vfs] lib mount /lib = fat boot volume source=sdhc rooted=LIB (R94: was /apps/LIB) ::`
- `tests rootdisk`: `:: ROOTDISK: root=UnaOS lib=/lib apps_alias=same-inode user_dir_at_root=visible-under-volume -> PASS :: made=<name|-> compat_hits=<n> links=apps,lib,volumes ::`
  (makes `/newfolder` when no user directory stands at `/`; RED names the token that failed).

## Owed (named)
- **Physically on UnaFS**: `apps` and `lib` are still the boot partition's `APPS/` and `LIB/`, linked at the root (the
  mount table's links, drawn `@`). Moving them onto the UnaFS volume needs exec and the FAT-only `find_app` path to read
  UnaFS and the card staging (`tools/unafs put`, recursive) to lay them; the 2 GiB image cap vs LIB/rustc (~420 MB).
- **R94 hot-swap**: every root path must be resolved through the boot volume's HANDLE (today `/` is bound per table
  build from `bootdisk::bind`'s content walk, and `/apps`/`/lib`/`/boot` name the boot FAT's `BlockSource`); no
  absolute volume ids may be baked into stores (users store, Holocron, attrs record paths, not volume ids); the mount
  table needs an atomic re-bind of `/` with open descriptors re-homed or refused, and the links re-pointed at the new
  disk's directories. None of that is built here.
- The compat redirect goes after one image (delete `rootdisk::compat_rel` and its fold in `resolve`).
