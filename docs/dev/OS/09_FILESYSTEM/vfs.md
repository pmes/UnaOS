# VFS: the unifying virtual-filesystem layer

Status: **VFS-1 (adoption) landed** — the shell's path verbs now RESOLVE THROUGH THE MOUNT TABLE.
This closes the gap the spine left open: VFS-1 built the resolver but marked itself "unconsumed —
shell/syscall adoption is follow-up", and §4 named that follow-up outright ("wiring the shell … onto
it is the follow-up adoption arc"). Until now the Pi ran **three disjoint path universes** — `ls`
resolved against native UnaFS with a hand-rolled `/usb` prefix branch, `cat` (and the other
FAT-direct verbs) resolved `/` as the SD FAT root, and only `run`/`bg`/`mount` used the table. The
same typed path meant three different files depending on which verb read it. See §12.

On top of:

**VFS-3** — the USB FAT stick is in the VFS namespace at `/usb`, alongside the SD boot FAT at
`/fat` and the native UnaFS root at `/` (see §11). The `FatBackend` is parametrized by its block
source, so ONE `MountTable` reaches both FAT volumes at once; the USB mount is bound only when the
stick is present. On top of:

**VFS-2** — the write surface is implemented for both backends
(`unaos/crates/kernel/src/fs/vfs.rs`), on top of VFS-1's read/resolve/authorize spine.
> **RELICS (R26, 2026-09-06).** The verb was `vfs` until Peter's ruling replaced the house
> names with the standard ones; it is now `mount`, which also answers the volume table
> (`mount` with no arguments) and the FAT geometry the retired `fatinfo` printed. The MODULE
> is still `fs/vfs.rs` and every `vfs_*` helper keeps its name — a module is not a verb.
> On aarch64 the plain mutating verbs (`touch`/`write`/`append`/`mkdir`/`rm`/`mv`) now resolve
> through this same namespace, which is what let the `u*` family retire.

First consumer landed (**SHELL-WRITE**): the panel shell's `mount` verb
(`unaos/crates/kernel/src/shell.rs`) routes create / write / truncate / unlink
through the `MountTable` over one namespace — the native UnaFS volume at `/`, the
FAT boot partition at `/fat`. `mount write|append|rm|mkdir <path> [text]`, writing as
`KERNEL_PRINCIPAL` (the shell's trusted-operator posture). `SYS_OPEN` and the
`genet`/net paths remain follow-up adoptions. VFS-2's write path is also proven by
two self-verifying on-card witnesses (§9).

## 1. Why a VFS, and why now

Three filesystems now coexist on one machine:

* the **native UnaFS** volume (journaled, per-object ACL, the kernel's attribute home);
* the **on-SD FAT** volume (the boot/data card);
* a **hot-plugged USB FAT** stick (metal-proven on the Jetson install path).

Until now each was reached by an ad-hoc call at the use site — `fat::mount()` here,
`unafs::with_unafs()` there — and the namespace split (`/` versus `/usb`) was hand-rolled
inside the shell. That does not scale to a ring-3 line where EL0 programs open files by name
and must not know or care which volume backs a path. The VFS replaces the ad-hoc dispatch with
**one namespace**: a mount table maps a path prefix to a backend, a resolver splits a path into
`(backend, volume-relative path)`, and every backend answers the same small read-side trait.

This is forced work, not speculative: FAT + UnaFS + USB-FAT already coexist, and the installer
line (which stages from the USB stick onto the native volume) needs a single `open("/…")` that
does not branch on volume at every call site.

## 2. Not POSIX — the right shape for UnaOS

We deliberately do **not** reproduce the POSIX inode/dentry VFS. UnaOS owns the whole stack, so
the trait is exactly the surface an open-for-read needs today, plus the one thing that makes a
capability OS different from Unix: **the ACL check is part of the open contract, not a bolt-on.**

The backend trait (`VfsBackend`):

```
fn volume_name(&self) -> &str;
fn read_dir(&self, rel: &str) -> Result<Vec<DirEnt>, VfsError>;
fn stat(&self, rel: &str) -> Result<Stat, VfsError>;
fn read(&self, rel: &str, offset: u64, len: usize) -> Result<Vec<u8>, VfsError>;
fn authorize_read(&self, rel: &str, principal: &str) -> Result<(), VfsError>;
fn open_read(&self, rel: &str, principal: &str) -> Result<Stat, VfsError> { /* authorize, then stat */ }
```

`rel` is always **volume-relative** — the resolver has already stripped the mount prefix. `""`
or `"/"` is the volume root. The default `open_read` fixes the open ordering for every backend:
**authorize first, then stat**, so a denied principal never learns even a file's size or that a
name is a file. Backends do not override that ordering.

**Write (VFS-2) — the mutating half.** VFS-2 lands the write surface the `&self` receivers left
room for, honoring the op naming this section shaped — `create` / `write` / `unlink`, plus
`truncate`:

```
fn authorize_write(&self, rel: &str, principal: &str) -> Result<(), VfsError>;
fn create(&self, rel: &str, kind: NodeKind, principal: &str) -> Result<Stat, VfsError>;
fn write(&self, rel: &str, offset: u64, data: &[u8], principal: &str) -> Result<usize, VfsError>;
fn truncate(&self, rel: &str, size: u64, principal: &str) -> Result<(), VfsError>;
fn unlink(&self, rel: &str, principal: &str) -> Result<(), VfsError>;
```

Every mutating verb composes the **write** ACL check first (`authorize_write`, the mutating twin
of `authorize_read`), then acts — the same authorize-first discipline as `open_read`. The trait's
default bodies return `VfsError::Unsupported`, so the surface is **opt-in**: a read-only backend
(or the witness mock) inherits "no mutation" for every verb unchanged.

The two backends already carried rich write paths (FAT in-place/grow/create, UnaFS journaled CoW);
VFS-2 is thin plumbing onto them, not a rewrite. See §5.3 (write authorization) and §8.1 (the
per-backend implementation and its bounds).

## 3. Path resolution contract

### 3.1 Mount-prefix resolution (VFS-owned)

The mount table resolves an absolute path by **longest matching prefix**, matching only at a
path-component boundary:

* `/` (the root mount) claims everything; the relative remainder is the whole path.
* A named prefix such as `/usb` claims `/usb` (remainder `""`, the mount point itself) and
  `/usb/a/b` (remainder `/a/b`) — but **never** `/usbfoo` (no boundary; that stays on the root
  volume). This boundary rule is a witnessed assertion (§7).
* Prefixes are canonicalized on mount: a leading `/` is ensured and a trailing `/` (other than
  the bare root) is dropped, so `usb`, `/usb`, and `/usb/` name one mount.

### 3.2 Component resolution (backend-owned)

Case and long-name posture belong to the **backend**, not the VFS. The VFS forwards `rel`
verbatim; each backend applies its own on-disk rule:

| Backend      | Case posture              | Long names                        |
| ------------ | ------------------------- | --------------------------------- |
| FAT          | case-**insensitive**      | VFAT LFN, else 8.3 short name     |
| native UnaFS | case-**sensitive**, exact-byte | native names (no 8.3 folding) |

This matches each medium's truth (FAT stores short names uppercased; UnaFS stores names
verbatim) and keeps the VFS from imposing a normalization neither backend wants. A consumer that
wants case-folding across a heterogeneous namespace layers it above the VFS, not inside it.

## 4. The namespace of record

The forward-looking namespace the VFS enables:

```
/          → native UnaFS   (the native root; per-object ACL)
/usb       → FAT            (hot-plugged stick; volume-level ACL)
```

This is the "do it right" shape: the **native** filesystem is the root of the namespace, and
foreign (FAT) volumes hang off named mount points. It generalizes the metal-proven `/` vs `/usb`
split the shell hand-rolled — the on-SD FAT card and any future volume become additional named
mounts (`/sd`, `/net/…`) without new dispatch code. The VFS-1 spine can host this table today;
wiring the shell and `SYS_OPEN` onto it (and choosing the exact boot-time mount set) is the
follow-up adoption arc.

## 5. How the ACL check composes across native and foreign volumes

This is the load-bearing design ruling, because the two backends have **fundamentally different
ownership models** and `SYS_OPEN` must present one authorization story.

### 5.1 Native UnaFS — per-object ACL (unchanged)

The native backend defers `authorize_read` to `unafs::read_authz` — the **same** owner/grants
evaluator the syscall layer's live-read check already uses (§`04_SECURITY_IMMUNITY/permission_model.md`
U6). Semantics are unchanged: kernel authority permits; a public object (no `owner`) permits; the
owner permits; a `grants:<principal>` row permits **iff** it carries the READ right (`CAP_READ`);
a deleted live object fails closed for everyone. The VFS adds no policy here — it routes to the
one evaluator so a VFS open and a direct `SYS_OPEN` authorize identically.

### 5.2 Foreign volumes (FAT) — the volume-level capability ruling

**FAT has no owners.** Rather than fabricate per-file owners for a format that does not carry
them, a foreign volume is authorized **at mount time**:

* the mount entry carries a single **volume principal** (the principal that mounted it — the
  installer for the boot stick, `kernel` by default) and an optional **world-readable** posture;
* **every object on the volume inherits that one posture** — there are no per-file grants;
* `authorize_read` on a foreign volume permits the volume principal, kernel authority always, and
  everyone iff the mount is world-readable; else `Denied`.

**Consequences, documented deliberately:**

* **Revocation granularity for a foreign volume is the unmount**, not a per-file grant. Ejecting
  (or programmatically unmounting) `/usb` is the revocation of every capability that volume
  conferred — cleanly, because after unmount the prefix no longer resolves to that backend
  (§7 asserts this).
* A foreign volume cannot express "file A private, file B shared" — that expressiveness is a
  property of the **native** ACL, and is a concrete reason the installer copies foreign content
  onto the native volume (where it acquires a real owner) rather than serving it in place.
* The mounting caller chooses the posture from the trust it has in the medium: the boot stick is
  mounted world-readable (its contents are the install image, meant to be read); a user's data
  stick would be mounted with a private volume principal.

This keeps the VFS **policy-honest**: it never invents ownership metadata a medium does not carry,
and it never silently widens the native ACL to accommodate a foreign one.

### 5.3 Write authorization (VFS-2)

The write ACL mirrors the read ACL's split, testing the **write** right rather than the read
right:

* **Native (per-object).** `authorize_write` consults the same per-object `owner`/`grants:<principal>`
  attributes, admitting the owner, a `grants` row that carries the **write** right (`CAP_WRITE`,
  decoded by the ONE `rights_from_native` decoder the syscall grant machinery uses), kernel
  authority, and a public object (no `owner` — a public native object is writable as it is
  readable). A GONE inode fails closed for everyone (deletion is total revocation). `create`
  authorizes against the **parent directory's** write right (the leaf does not exist yet); the
  other verbs authorize against the target.
* **Foreign (FAT, volume-level).** Only the volume principal and kernel authority may write.
  The **world-readable** posture does **not** confer write — a stick mounted for reading is not
  thereby writable; a world-*writable* mount would be a separate future flag.

A newly `create`d **native** object acquires the creating principal as its `owner`, so it is not
left world-writable — the "acquires a real owner" property §5.2 names as the reason the installer
copies foreign content onto the native volume. Kernel-created native objects stay public (no
owner row).

## 6. Hot-mount / unmount lifecycle

The mount table is the lifecycle surface:

* `mount(prefix, backend)` binds a volume at a prefix — at boot for the native root and the SD
  FAT, and **at hot-plug time** for a USB stick (the Jetson install path proves the need: a stick
  appears after boot and must enter the namespace without a reboot). Re-mounting a live prefix
  replaces its backend (a swapped card).
* `unmount(prefix)` removes a mount on eject. For a foreign volume this is also its authorization
  teardown (§5.2). The spine's `unmount` is synchronous set-removal; **quiescing in-flight opens
  before an unmount** (a descriptor still referencing an ejected volume) is an adoption-arc
  concern — the read-only spine holds no descriptors, so there is nothing to quiesce yet.

The backend adapters are **stateless** with respect to the medium: the FAT adapter re-mounts
through `fat::mount()` per call and the native adapter flows through the one coherent
`unafs::with_unafs()` mount, so a swapped or remounted volume is observed on the next access —
the same posture the shell's existing FAT commands already rely on.

## 7. The VFS-1 witness

`vfs::vfs1_resolution_witness()` is an arch-neutral, disk-free unit-shaped proof. Two in-RAM mock
backends stand in for a native and a foreign volume, mounted at `/` and `/usb`, and the witness
asserts:

1. **routing** — `/usb/b.txt` → the usb backend with remainder `/b.txt`; `/a.txt` → the native
   backend with remainder `/a.txt`; `/usb` (the mount point) → usb with an empty remainder;
2. **boundary** — `/usbfoo` stays on the root volume (does not leak into `/usb`);
3. **read** composes across both backends;
4. **ACL** — the native posture permits the owner and kernel, denies a stranger; the foreign
   posture permits the volume principal and kernel, denies a non-volume principal;
5. **unmount is revocation** — after `unmount("/usb")`, `/usb/b.txt` no longer resolves to the
   usb backend (it falls through to the root volume).

The function returns `Ok(())` when every assertion holds and `Err(reason)` naming the first that
fails. It is a pure function compiled on both arches (proving the spine is arch-neutral); a
follow-up arc may wire it behind the `witness` feature alongside the other kernel selftests.

## 8. Backend write implementation (VFS-2) and its bounds

Both adapters plumb the trait's write verbs onto the backends' existing, already-crash-safe
mutation primitives — no new on-disk mutation logic is introduced.

**FAT (`FatBackend`).** `create` → `create_in_dir` (file) / `create_dir` (directory);
`write` → `write_grow` (which subsumes overwrite-in-place, append-at-EOF, and free-cluster
allocation from the FAT, publishing the grown `size`/`first_cluster` to the directory entry
LAST — data and both FATs durable first); `unlink` → `delete_located` (marks the directory slot
`0xE5` FIRST, then frees the chain). `truncate` supports grow (zero-extend via `write_grow`),
no-op, and truncate-**to-zero** (`delete_located` + a fresh 0-length entry — the only shrink the
public FAT surface expresses).

**Native (`NativeBackend`).** `create` → `create_file` / `mkdir` then a per-object `owner`
attribute (§5.3); `write` → `write_data` (journaled CoW); `unlink` → `unlink`. `truncate`
supports grow and no-op.

**Documented bounds (this arc):**

* **FAT LFN write is out of scope.** `create` accepts only representable 8.3 short names
  (`format_83`); a non-representable name is `VfsError::Unsupported`. Reading VFAT long names is
  unchanged (that is the backend's read posture, §3.2).
* **No non-zero in-place shrink.** `0 < size < current` is `VfsError::Unsupported` on both
  backends — neither carries an in-place shrink primitive. For **native**, even shrink-to-zero is
  `Unsupported`: doing it by unlink+recreate would DROP the per-object ACL, the one thing the
  native volume must never silently lose. (FAT truncate-to-zero is fine — FAT has no per-object
  ACL to preserve.) A caller wanting smaller content creates a fresh object and writes it.
* **`offset > size` (a sparse hole) is rejected**, matching `write_grow`'s no-hole contract.

**Failure safety.** Every write is write-through and each backend step is atomic under its own
lock, so a mid-sequence failure leaves the volume consistent, never partial-committed: a failed
FAT grow keeps the OLD (smaller) size (size is published last); a failed native mutation unwinds
to the last committed root (CoW). The witnesses (§9) abort with a `FAIL` line rather than commit a
half-write.

### 8.1 VFSX86 — `FatBackend` is arch-neutral (2026-08-21)

From VFS-1/VFS-2 until this change, `impl VfsBackend for FatBackend` — and its helpers
`read_only`, `resolve_entry`, `resolve_parent`, `fat_err`, `fat_create_err` — were
`#[cfg(target_arch = "aarch64")]`. On x86_64 `FatBackend` was therefore a struct with no backend
impl: it could not be coerced to `Box<dyn VfsBackend>`, so it could not be mounted into a
`MountTable` at all, and every x86 consumer that needed to write a file routed around the VFS and
called `fs::fat`'s directory primitives directly.

**The gate had no hardware reason.** It recorded where the work was done — the Pi came first — not
a property of the code. The impl body contains no arch-specific line, and every primitive it calls
is compiled unconditionally on x86_64 and always was: `fat::mount_source` (whose own `match` even
carries an x86-only `Sdhc` arm), `read_root` / `read_dir` / `read_at`, and the entire write half
(`locate_in_dir`, `create_in_dir`, `create_dir`, `write_grow`, `delete_located`). The single
genuinely gated dependency was `fat::fat_reason`, itself gated only by its first caller (the Pi's
USB mount witness) despite being a total `match` over a plain enum; it is now arch-neutral too.

The gate is therefore **removed**, and the VFS is a real seam on both chips. `NativeBackend` keeps
its gate, which does have a stated reason: the kernel `unafs` module is itself aarch64-only.

**What this does NOT do.** Widening the seam does not enroll anyone in the x86 write discipline.
x86 has no in-`fat.rs` FAT/directory mutation lock — `with_fat_lock` / `with_dir_lock` are
`#[inline(always)]` passthroughs there, and volume consistency is held caller-side by the **X86
FAT-MUTATOR ROSTER** documented on `fat::with_fat_lock`. This impl is a seam, not a caller: as
landed it has **zero x86 callers**, so it mutates nothing and joins no roster row. The roster rule
binds whoever calls it — an x86 consumer migrating onto these verbs must either submit through the
storage-service task or run in program order on the BSP main loop ahead of the launchers, and must
add itself to that roster.

**Consumer migration is deliberately not part of this change.** The three existing direct callers
(`shell.rs`, `flight_recorder.rs`, and the Holocron bond store) still call `fs::fat` directly. What
they gain by migrating is the half of the check they do not reproduce today: each write verb calls
`authorize_write` FIRST, which refuses a read-only volume (`Unsupported`, via `FatBackend::read_only`
→ `BlockSource::write_veto`) and *then* enforces the volume-principal ACL (`principal ==
self.principal || principal == KERNEL_PRINCIPAL`; the `world_readable` posture is deliberately not
consulted for writes). The direct callers reproduce the read-only refusal and **not** the ACL.

Known blockers a migration arc must clear first (none of which this change is blocked on):

* **No x86 mount registration exists.** Every `MountTable` in the tree is built per-call, and every
  registration site is aarch64-gated. x86 has no mount table to present a principal to yet.
* **`FatBackend`'s source is fixed by its constructor** (`new` → `Default`, `new_usb` → `Usb`).
  There is no constructor for `mount_program_source()` / `BlockSource::Sdhc`, which is what
  `shell.rs` actually resolves to on x86.
* **Missing verbs.** The trait has no `remove_dir` / `rename_entry` / `move_entry`, so the shell's
  `rmdir`, `mv`, and recursive-copy tree half cannot migrate without extending it.
* **`flight_recorder::write_log` is roster row 2 precisely because it is NOT a mutator** (bounded
  `write_at` over an already-reserved chain, writing no FAT entry and no directory sector). Routing
  it through `VfsBackend::write` (→ `write_grow`) would convert it into a directory-sector RMW on
  the unsynchronized x86 path — a regression against the roster, not a refactor. It also needs a
  `first_cluster` back from reservation, which `create`/`truncate` do not surface.
* **Holocron's write must stay deferred past the `EHCI_HID` lock.** The Bluetooth bonding chain
  that produces the record runs under `drivers::ehci::EHCI_HID`, inside which a store call must be
  a memcpy and nothing else; a FAT write there would contend the xHCI storage loan from inside the
  EHCI service pass and hold keyboard and trackpad hostage for its duration. The deferral is a
  `flush_if_dirty` guard *above* `write_store_file`, so swapping in `MountTable` calls does not by
  itself break it — but the check sits per-call-site rather than on the seam. A migration arc should
  decide whether a "no driver lock held" assertion belongs on the trait's write verbs.

**Verification status — the runtime proof is owed at metal.** This change is gated on
`./arroyo check` (green, both arches, all 12 cfg-gated legs including `x86-all` and the seven x86
feature mixes) plus the static call-path trace below. It carries **no QEMU or hardware run**: the
witnesses that would drive it (`vfs2_fat_write_witness`) are invoked only from
`arch/aarch64/syscall.rs`, so an x86 runtime proof needs a call site in `arch/x86_64/syscall.rs` —
outside the VFS lane. Landing the seam unconsumed is what makes that acceptable: nothing executes
this code on x86 until a consumer or a witness is wired to it, and wiring either is a separate arc
that must bring its own runtime evidence.

The static path an x86 `VfsBackend::write` takes to the medium, and where the two checks sit:

| Step | Location |
|---|---|
| `FatBackend::write` — calls `authorize_write` FIRST | `fs/vfs.rs:712` |
| `authorize_write` — **read-only refusal** (`VfsError::Unsupported`) | `fs/vfs.rs:664` → `read_only` `fs/vfs.rs:479` → `BlockSource::write_veto` `fs/fat.rs:648` |
| `authorize_write` — **principal ACL** (`VfsError::Denied`) | `fs/vfs.rs:664` |
| mount the volume from the backend's source | `fat::mount_source` `fs/fat.rs:1509` |
| resolve parent dir, locate the leaf | `resolve_parent` `fs/vfs.rs:524` → `locate_in_dir` `fs/fat.rs:3029` |
| grow/overwrite, publishing size LAST | `write_grow` `fs/fat.rs:2910` |
| data write → sector run | `write_span` `fs/fat.rs:2623` → `wr_sector` / `wr_sectors` `fs/fat.rs:1887` / `1902` |
| extent-checked dispatch to the block layer | `write_sector` `fs/fat.rs:736` |
| the medium | `block::write_block` (Default) / **`block::write_block_usb`** (Usb) / `block::write_block_sdhc` (x86+`sdhcblk`) — `fs/fat.rs:746` / `747` / `751` |

**Metal watch item — the lines a future boot must show.** When a consumer or an x86 witness is
wired to this seam, that arc must produce, on an x86 boot with a writable FAT volume:

1. a create → write → read-back → checksum round-trip through a `MountTable`, i.e. the
   `:: VFS2: write test — created <path>, wrote <n>, readback OK :: PASS ::` line §9 defines, with
   the scratch file unlinked afterwards; and
2. the **refusal** arm on the same boot: a write attempted against a volume whose `write_veto()` is
   `Some` must return `VfsError::Unsupported` **before** any sector is touched, and a write
   presenting a principal that is neither the volume principal nor `KERNEL_PRINCIPAL` must return
   `VfsError::Denied`. A green round-trip without both refusals is not the proof.

## 9. The VFS-2 write witnesses

Two on-card, self-cleaning witnesses prove a `create` → `write` → read-back → checksum round-trip
through the mount table to each real backend (aarch64-only; honest skip when the volume is absent).
Since VFSX86 (§8.1) the *FAT backend* is arch-neutral, so `vfs2_fat_write_witness`'s aarch64-only
reach is now a property of its **call site** (`arch/aarch64/syscall.rs`), not of `FatBackend`; an
x86 equivalent needs a call site in `arch/x86_64/syscall.rs`, which is outside the VFS lane.
`vfs2_native_write_witness` stays aarch64-only for the backend reason (`unafs` is aarch64-gated):

* `vfs::vfs2_fat_write_witness()` — FAT volume mounted at `/`;
* `vfs::vfs2_native_write_witness()` — native UnaFS volume mounted at `/`.

Each emits `:: VFS2: write test — created <path>, wrote <n>, readback OK :: PASS ::` and unlinks
its scratch file. They are invoked from the `syscall.rs` storage battery (after the K-series, so
their scratch never perturbs those fixtures); because `syscall.rs` is outside the VFS lane, that
one-line-per-witness wiring lands as a deferred diff alongside this arc.

## 11. VFS-3 — the USB volume in the namespace

VFS-2's SHELL-WRITE consumer flagged a gap: the `FatBackend` always mounted through
`fat::mount()` (the globally-registered block device = the SD boot partition on the Pi), so the
hot-plugged USB stick — the volume `ls /usb` and the `/fs/usb` HTTP route already read through
`fat::mount_source(BlockSource::Usb)` — could not be reached through the `MountTable` at all. The
namespace of record (§4) names `/usb → FAT`, but nothing could bind it.

**The fix (the VFS-1 design intent, made real).** `FatBackend` now carries the
`fat::BlockSource` it mounts through, and `MountTable` can therefore hold BOTH FAT volumes at
once, each reaching its own device:

```
/          → native UnaFS   (BlockSource::Default via unafs; per-object ACL)
/fat       → FAT boot part   (BlockSource::Default; the SD card; writable)
/usb       → FAT USB stick   (BlockSource::Usb; the xHCI stick; READ-ONLY)
```

* `FatBackend::new(volume, principal, world_readable)` mounts the `Default` source (the boot
  FAT), unchanged — every VFS-2 call site keeps working.
* `FatBackend::new_usb(volume, principal)` mounts the `Usb` source (world-readable), the same
  read-only xHCI mount `ls /usb` uses.

**Read-only is honored honestly, not fabricated as writable.** The `Usb` block source is
**read-only by construction** (PIUSB-27: `write_sector` refuses any non-`Default` source, so no
FAT/dir/data write can ever reach the stick). The adapter mirrors that guard at the VFS layer:
`authorize_write` refuses every write verb on a non-`Default` source with `VfsError::Unsupported`
(`-ENOTSUP`, a clean "read-only volume" answer) BEFORE it touches the block path — never a raw
I/O error, and never by weakening the block-layer guard. A future world-*writable* USB flag (or a
USB WRITE(10) path) is the only thing that would relax this; VFS-3 does not, and must not, invent
one. Writing a USB stick's contents onto a writable volume is the installer's copy-onto-native
job (§5.2), not an in-place `/usb` write.

**Honest hot-plug (doc §6).** The `/usb` mount is bound only when the stick is actually
enumerated *and its FAT is mountable* — `vfs_mount_table()` (rebuilt per shell `mount` invocation)
and the witness both do a `mount_source(Usb)` presence check at build time. Absent (or the medium
unreadable) → `/usb` is simply not in the table. A stick plugged (or ejected) between commands is
picked up on the next `mount`. See §11.2 for how a mutating verb reports an unbound `/usb`.

**The shell `mount` verb picks it up automatically.** `vfs_mount_table()` gained the conditional
`/usb` mount, so `mount write|append|rm|mkdir /usb/...` route through the same four verbs — read
paths work, writes return `-ENOTSUP` (the read-only posture). No new dispatch code.

### 11.1 The VFS-3 USB-mount witness

`vfs::vfs3_usb_mount_witness()` (aarch64) builds a `MountTable` with `/fat` (Default) and `/usb`
(Usb) and proves through the table that the USB volume is reachable (its root lists, a file reads
back), that the read-only guard is enforced (a `create` at `/usb/...` returns `Unsupported`, no
panic), and that `/fat` coexists (its root still lists) — emitting
`:: VFS3: usb-mount test — /usb root <n> entries, read <m> bytes, read-only enforced, /fat coexists=<b> :: PASS ::`.

This is a **metal** proof, honest-skip under QEMU: the stick is reached through the xHCI `Usb`
source, which needs the BCM2711 PCIe RC + VL805 xHCI. QEMU raspi4b models no PCIe RC and attaches
no usb-storage, so `mount_source(Usb)` finds no device and the witness prints
`:: VFS3: usb-mount — no USB volume — skipped ::` there (verified under
`UNAOS_V3D=1 UNAOS_GENET=1 UNAOS_PIUSB=1 ./arroyo kernel8-test`). The positive PASS line is the
attended-metal evidence — the same posture the whole piusb line carries ("attended-metal for
positive verify"). Its permanent call site is the aarch64 storage/USB path (co-located with
`piusb27_service`, where the stick is live), which lives outside the VFS lane; that one-line
wiring lands as a deferred diff, and a temporary proof captured the QEMU skip line for this arc
(reverted after capture), exactly as VFS-2 did.

### 11.2 VFS-4 — unbound `/usb` reports "volume not mounted", not `-ENOENT`

The P44 metal sitting exposed a **misdirecting error message**. With the USB stick fully
enumerated (`storage enumerated … 30436 MiB`), `mount write /usb/test123.txt hi` returned
`mount write: /usb/test123.txt: no such file or directory (-ENOENT)`. Root cause was **upstream of
the VFS** (PIUSB-34): the stick's `READ(10)` of LBA 0 returned all-zeros with a *passing* CSW, so
`fat::mount_source(Usb)` honestly found no FAT volume, and `vfs_mount_table()`'s presence check
therefore never bound `/usb`. The path then fell through the longest-prefix resolver to the native
root (`/` claims everything), where `NativeBackend::create` tried to resolve the parent `/usb` as a
*native* path, failed, and surfaced `-ENOENT` — as if the file were missing, when in truth the
whole **volume** was not mounted. That misdirection cost bench time.

VFS-4 fixes the reporting half (the block-layer read fault is PIUSB-34's lane). The shell reserves
the volume prefixes that may be absent (`RESERVED_VOLUME_PREFIXES = ["/usb", "/fat"]`), and
`vfs_cmd` checks — before dispatch, for *every* verb — whether the target path names a reserved
volume that is not in the live mount table (`unmounted_reserved_volume`, boundary-matched exactly
as the resolver: `/usb` and `/usb/…` match, `/usbfoo` does not). If so it reports
`mount <op>: <path>: volume /usb not mounted (-ENODEV)` and stops, instead of letting the path fall
through to the native root. The honest-absent posture of §6 is preserved — an unbound `/usb` is
still simply not in the table — but the *message* now names the real condition. `/` is deliberately
excluded from the reserved set: it is always mounted and is the legitimate fall-through for
un-prefixed paths. No new dispatch code beyond the one guard; when `/usb` *is* bound (the FAT
mounts), every verb routes to `FatBackend` exactly as before.

The FAT root-level create walk itself was exonerated: `vfs2_fat_write_witness` already proves a
`create → write → read-back` round-trip on a FAT volume root through the same `MountTable` /
`FatBackend` path (it runs green in the aarch64 storage battery), so no `(b)`-type create-walk bug
exists — the only defect was the presence/reporting path above.

## 10. File map

* `unaos/crates/kernel/src/fs/vfs.rs` — the spine + adapters: `VfsError`, `NodeKind`, `Stat`,
  `DirEnt`, the `VfsBackend` trait (read + write surface), `MountTable` (mount/unmount/resolve +
  read/write resolve-then-dispatch conveniences), the `FatBackend` and `NativeBackend` adapters
  (read + write), the VFS-1 resolution witness, the two VFS-2 write witnesses, and the VFS-1
  adoption routing witness (`vfs1_routing_witness`).
* `unaos/crates/kernel/src/shell.rs` — the routing seam (§12): `vfs_path` (the one
  argument -> absolute-path entry), `vfs_mount_table`, `vfs_ls_collect` + `pi_ls` (the single
  listing collector and renderer), `vfs_cat`, `vfs_mtime_field`, `vfs_err`,
  `unmounted_reserved_volume`, and `read_el0_image`/`vfs_cmd` as seam consumers.
* `unaos/crates/kernel/src/fs/fat.rs`, `…/unafs.rs` — the backends the adapters wrap (unchanged
  by VFS-2; their existing write primitives are consumed as-is).

## 12. VFS-1 (adoption) — the routing seam

VFS-1 through VFS-4 built a correct mount table and then used it in three places. The verbs an
operator actually types mostly did not go through it, so the namespace the docs described was not
the namespace the shell implemented.

### 12.1 What was actually wrong

Three path universes coexisted on aarch64:

1. **FAT-direct + cwd** — `cat`, `stat`, `cp`, `mv`, `rm`, `head`, `tail`, `find`, `du`, `hexdump`, `cd`
   and friends bound `fat::mount_program_source()` and treated `/` as the **SD FAT root**.
2. **unafs + an inline `/usb` branch** — `ls` alone, resolving against **native UnaFS**, with a
   hand-rolled `if path == "/usb" || path.starts_with("/usb/")` test choosing a second, duplicate
   listing renderer.
3. **the `MountTable`** — `run`, `bg`, `mount`, where `/`, `/fat` and `/usb` meant what §4 says.

Consequences: `ls /fat` failed (no `/fat` branch existed in `ls`); `cat /usb/FILE` failed (the name
resolved as a literal FAT directory called `usb`); `cat /X` and `ls /X` named **different files**;
and `run`/`bg`/`mount` ignored the cwd entirely, so a relative name after a `cd` resolved against the
root. The `/usb` prefix test was also the exact `starts_with` bug §3.1's boundary rule exists to
prevent — it was correct only because it was hand-checked, not because it used the resolver.

### 12.2 The seam

One function, `shell::vfs_path(arg)`, turns an operator-typed argument into an absolute VFS path
(`normalize_path` against the cwd — purely lexical, so `.` collapses and `..` pops before any
backend is consulted). Every routed verb calls it and nothing else. **Which volume the path lands
on is then decided solely by `MountTable::resolve`'s longest-prefix rule — never by the verb.**

Adopted this arc: `ls`/`dir` (via the new `vfs_ls_collect`), `cat`/`type`, `run`, `bg`, `mount`. All
five now agree on `/` = native UnaFS, `/fat` = SD boot FAT, `/usb` = the stick.

Two per-volume listing collectors (`pi_ls_collect` against unafs, `pi_usb_ls_collect` against the
USB FAT) and their two renderers collapsed into one collector and one renderer — the duplication
existed *only* to serve the prefix branch. `ls /` also stopped probing `usb_info()` for its `usb/`
pseudo-row: mount points immediately below the listed path are now synthesized **from the mount
table**, so `/fat` appears the same way `/usb` does, and an absent stick contributes no row (the
honest hot-plug posture of §6, unchanged).

Two guards that existed on one verb became shared rather than re-derived:

* the VFS-4 `-ENODEV` guard (a path naming a reserved-but-unbound volume reports the *volume* as
  missing, not a bare `-ENOENT` off the native root) now covers `ls`, `cat`, `run` and `bg` as well
  as `mount`. `run /usb/X` with the stick absent previously reproduced the exact P44 misdirection
  VFS-4 fixed for `mount`;
* the cwd, which the mount-table verbs did not honour at all.

### 12.3 `DirEnt` gained `size` and `mtime`

`ls` could not be routed through the table while `DirEnt` carried only a name and a kind — it would
have had to keep its per-volume collectors purely to recover a size column, which is the dispatch
this layer exists to delete. `DirEnt` now carries `size: u64` and `mtime: Option<VfsTime>`, where
`VfsTime` is a small **VFS-owned** stamp: the FAT backend decodes its on-disk `DirEntry::mtime()`
into it (so no FAT type crosses the trait boundary) and the native backend answers `None`, which the
renderer shows as a dash rather than a fabricated date — §3.2's "the backend answers for its own
medium" applied to time. An all-zero FAT stamp is also `None`: that is FAT's "unset", not 1980.

`read_dir` also stopped presenting `.`/`..`. The resolver is lexical, so a dot entry has no meaning
in this namespace; both deleted shell collectors filtered them, and the filter moved to the one
place that owns it rather than disappearing.

### 12.4 Deliberately NOT in this arc

This was plumbing unification, not new filesystem features. No new on-disk format, no new mount
grammar (the `/`, `/fat`, `/usb` convention already in the tree is followed, not replaced), and **no
write-enablement anywhere**: the read-only postures stay exactly as ruled, and §12.5's `ro` leg
asserts that a backend which implements no write methods refuses every mutating verb through the
table.

Still FAT-direct, and named here so the remaining gap is visible rather than implied: the JD12
**glob** expansion (`cat *.MD`), and the mutating/inspecting FAT verbs `stat`, `cp`, `mv`, `rm`,
`mkdir`, `head`, `tail`, `find`, `du`, `hexdump`, `touch`, `append`, `write`, plus `cd` (whose cwd is
still a FAT 8.3 path). `SYS_OPEN` and the `genet` `/fs/usb` route remain follow-up adoptions as
before. x86 is unchanged by design: `fs/vfs.rs` gates both backends to aarch64, so that arch has no
mount table to route through and exactly one FAT volume to confuse.

### 12.5 The VFS-1 routing witness

`vfs1_routing_witness()` asserts the four claims the seam makes, against the **live** table the
shell builds, in the uncounted `:: VFS-1: … ::` idiom of the VFS-2/VFS-3 witnesses:

| leg | claim |
| --- | --- |
| `route /fat/VUG.ELF` | resolves to the **FAT** backend, mount prefix stripped (`rel=/VUG.ELF`) |
| `route /K3HELLO.TXT` | resolves to the **native UnaFS** backend, path intact |
| `boundary` | `/fatty.bin` and `/usbfoo` are **native** names, verbatim — a prefix claims a path only at a component boundary (§3.1). The negative a naive `starts_with` gets wrong |
| `ro-seam` | a backend implementing no write methods refuses `create`/`write`/`truncate`/`unlink` through the table with `Unsupported`, **and still reads** — read-only is the trait's *default* posture, not something a backend must remember to assert |

Measured on `./arroyo kernel8-test 210` (QEMU raspi4b, no stick attached, so the boundary leg
reports `usb bound=false` honestly):

```
:: VFS-1: route /fat/VUG.ELF -> vol=fat rel=/VUG.ELF :: PASS ::
:: VFS-1: route /K3HELLO.TXT -> vol=native rel=/K3HELLO.TXT :: PASS ::
:: VFS-1: boundary /fatty.bin,/usbfoo -> vol=native verbatim (usb bound=false) :: PASS ::
:: VFS-1: ro-seam create/write/truncate/unlink -> Unsupported, read still ok=true :: PASS ::
```

The routed `ls` is witnessed by the existing `:: ls1: ::` line, which now shows the mount-point row
the table synthesizes rather than a `usb_info()` probe:

```
:: ls1: /: K3HELLO.TXT K3PAT.BIN fat/ (2 file, 1 dir) ::
```

## 13. VFSROUTE (orin 17) — every file verb asks the mount table

**Authority.** Peter, 2026-09-06, on being shown that `ls` had been *made* to list the native
volume: *"should UnaFS not support ls? I'm confused that ls has to be made to list a dir. Should a
mounted file system not be listable? Sounds like we'll be adding each filesystem to ls so it
lists."*

That is the defect §12.4 had left standing, named precisely. §12 routed five verbs and listed the
rest as follow-ups; this arc routes **all** of them, on **both** arches, and deletes the per-verb
filesystem dispatch entirely.

### 13.1 The rule

A verb resolves its argument to an absolute path in the ONE namespace, hands that path to the mount
table, and calls the trait. Which filesystem answers is `MountTable::resolve`'s longest-prefix
decision and nothing else.

* **No verb names a filesystem.** `with_unafs` and `mount_read_volume` no longer appear in any verb
  or verb helper (census in §13.6).
* **No verb carries a `target_arch` gate.** `vfs_mount_table()` is arch-neutral; the two `cat`
  bodies, the two `ls` bodies and the two `read_el0_image` bodies collapsed to one each.
* **A backend that cannot perform an operation returns a typed error the verb prints** — never a
  silent fall-through to another volume. `rmdir` on the native volume is the worked example: the
  UnaFS crate carries no directory removal, so `NativeBackend` inherits the trait's default and the
  operator sees `-ENOTSUP`. Before this arc that keystroke walked `fat.rs` looking for the name on
  the **boot partition**.

### 13.2 What the trait gained

VFS-2 shaped a write surface for a *syscall*; a *shell* needs four more questions answered, and each
is a question about a VOLUME, so each belongs to the backend. All four default to a refusal, so a
backend opts in to a capability rather than remembering to refuse.

| method | default | FAT | native UnaFS |
| --- | --- | --- | --- |
| `write_veto() -> Option<&'static str>` | refuse | forwards to `BlockSource::write_veto` (the ONE definition FATVERB established) | `None` (journaled, read-write since K4) |
| `rename(from_rel, to_rel, principal)` | `Unsupported` | `rename_entry` in-parent, `move_entry` across | one CoW transaction (`unafs::rename`) |
| `remove_dir(rel, principal)` | `Unsupported` | `fat::remove_dir`; non-empty → `Backend("not-empty")` | **not implemented** — the crate has none |
| `remove_attr(rel, key, principal)` | `Unsupported` | **not implemented** — FAT has no typed attributes | `remove_attribute` |
| `volume_bytes() -> Option<u64>` | `None` | the BPB's volume size | `None` (no published total) |
| `describe() -> Option<String>` | `None` | the geometry line the retired `fatinfo` printed | `None` |
| `volume_id() -> Option<u64>` | **no default — required** (VOLID) | `Some(fnv("vfs:fat:" ‖ source ‖ BS_VolID ‖ cluster count))`; `None` if the volume will not mount | `Some(fnv("vfs:unafs:"))` — one global mount per machine |

`MountTable` gained the matching resolve-then-dispatch conveniences plus `volume_name(path)`,
`same_volume(a, b)`, `volume_id(path)` and `rows()` (one tuple per mount: prefix, volume, veto,
capacity, description — the five facts `df`/`mount` render and the only five they know).

**A cross-volume `rename` is refused by the TABLE, not by a verb.** A backend receives
volume-relative paths and cannot tell that the other end lives elsewhere, so `MountTable::rename`
compares the two resolved backends and returns `Unsupported` when they differ. `mv` prints
*"cross-volume move is not supported (copy with `cp`, then `rm`)"* — the same answer the old verb
gave for the native/FAT pair, now true for every pair of volumes the machine will ever mount.

#### 13.2.1 What "the same volume" means (VOLID, orin 18 — rmbp 15 condition C1)

That comparison has been three different things, and only the third is an identity:

1. **the two backend POINTERS** — too fine. One medium bound at two prefixes is two objects, so a
   plain in-volume relink read as a cross-volume move.
2. **the two VOLUME NAMES** — wrong in *both* directions, and the reason this section exists.
   `sdmmc_root_bind` binds the Orin's card as `"card"` at `/` and as `"fat"` at `/fat`, so one
   physical card read as two volumes and `mv /A.TXT /fat/B.TXT` was refused on exactly the
   configuration the bench flies. The converse is worse: two different media typed one name would
   read as ONE volume, and a rename between them relinks a directory entry to a name that means
   nothing on its volume. A name is an argument the mount site typed, never a fact about the medium.
3. **`volume_id()` — the backend's own STORAGE IDENTITY**, which is what the question always meant.

**Identity is over `(DEVICE, FILESYSTEM)`, never the device alone.** The device alone fails on the
Pi, which mounts UnaFS at `/` and the FAT program volume at `/fat` off *the one physical card*
(`drivers/emmc2.rs` registers it as `BlockSource::Default` and sizes the UnaFS volume from the same
geometry). So every implementor mixes a **domain tag** naming its filesystem before it mixes
anything about the medium, and two filesystems on one card are unequal by construction. The FAT
adapter then adds the block source *and* the mounted volume's own fingerprint —
`FatFs::volume_fingerprint()`, `(BS_VolID, count_of_clusters)`, the same primitive the aarch64
`UNAFS.ATR` ACL store binds to. It deliberately does **not** use `fat::volume_serials()`: that is a
device-wide census, a deliberate superset for the boot-device guard, so two partitions on one device
would share its id set — the same defect one layer down.

**`Option`, and no default.** A constant default would make every non-overriding backend the same
volume as every other, which is the corrupting direction; a name-derived default is defect 2 again.
So the method is required, and `None` means *"I cannot establish this volume's identity"* — **never
equal to anything, including another `None`.** `fs::vfs::same_storage(a, b)` is the one
interpretation of the contract (`same_volume` reports it, `rename` enforces it, so the two cannot
drift): object identity first — the same backend object is the same storage, which keeps the
relation reflexive and keeps a `None`-answering backend usable inside its own mount — then
`Some(a) == Some(b)`.

**Where it is measured.** The `vfsroute` transcript prints `:: volid: … ::` census lines
unconditionally — each mount's `id=`, and `same=` for every pair of mounted prefixes — and carries
two legs. `vfsroute.samevol` convicts `same_volume` against an oracle taken from the probe file's
`stat` under both prefixes, never from `same_volume` itself (which is what the transcript did
before, so a wrong router moved the expectation and the answer together). `vfsroute.alias` *builds*
the C1 shape in a scratch mount table — one block source under two names, which must be the same
volume, and a different source under the identical name, which must not be — because no board the
QEMU gates can boot mounts that shape.

**`mv` asks the write veto BEFORE the namespace question** (rmbp 15 N1). The order is the diagnosis:
on a read-only card both refusals are true and the operator sees only the first, and *"cross-volume
move is not supported (copy with `cp`, then `rm`)"* recommends a workaround that cannot help on a
volume admitting no writes. The final order is **argument validity → write veto (on the source) →
namespace**. The destination gate the old order ran is subsumed, not dropped: once `same_volume` has
passed, the destination is the same medium, and `write_veto` is a pure function of the `BlockSource`,
which is one of the terms `volume_id` mixes.

### 13.3 x86 has a namespace now

`vfs_mount_table()` was `#[cfg(target_arch = "aarch64")]` from VFS-1 — because the Pi came first, not
because a mount table is an aarch64 idea. That gate is what forced every verb to carry two bodies.

On x86 the table binds **the program source** (`block::program_source`, resolved through
`open_read_volume` so the FATVERB `READ_BIND` instrument is stamped exactly as it was when each verb
mounted for itself) at BOTH `/` and `/fat` — `/fat` being the spelling the packaging text, the
staged-image script and `exec_resolve`'s second probe all use for that one volume. This is the
two-prefix shape `sdmmc_root_bind` already uses on the Orin, honest for the same reason: `/fat` IS a
mount point, so `ls /` showing it is a fact, not decoration.

An arch with no volume returns an EMPTY table and the verbs say `no filesystem mounted (-ENODEV)` —
better than the old `no FAT filesystem (NoDisk)`, which named a filesystem the operator never asked
about.

### 13.4 FATVERB is preserved, and moved to the layer that owns the question

FATVERB's law is that a file verb binds the PROGRAM SOURCE and that a write verb consults a gate
before it starts. Both still hold; the gate simply asks the right thing now.

* The write gate is `MountTable::write_veto(path)` — the veto of the volume **the path resolves
  to**, asked before any multi-step verb begins. On a machine with two writable volumes in one
  namespace that is the difference between gating the volume you are about to write and gating a
  different one. It still stamps `WRITE_GATE` and still writes the two-sink refusal
  (`REFUSED READ-ONLY` on the panel, the census on serial), so `fatverb_storage_witness` reads the
  same instrument.
* The **exec probe** (`FatVolume::is_file`) deliberately keeps binding `mount_program_source()`
  itself and walking FAT directly (`fat_path_is_file`, the one FAT-direct walk left in `shell.rs`).
  That is not an oversight: the witness compares the probe's stamp with a read verb's, and its value
  is that the two are INDEPENDENT producers. Route the probe through the table as well and the leg
  becomes an expression compared with itself — the exact defect FATVERB's own note records the first
  cut of that witness making.

### 13.5 What stopped being printable, and why that is the arc working

Three FAT-specific facts left the output, because a trait that could carry them would be a FAT
trait: `rm`'s freed-cluster count, `stat`'s attr byte / first cluster / directory-slot LBA, and
`ls`'s canonical 8.3 re-spelling of a typed name. `stat` prints `volume:` instead — which volume
answered, the fact the old output could not express. The forensic pair is unchanged and
volume-honest: `hexdump` for a file's bytes, `dd if=<lba>` for a raw sector, `fdisk -l` for device
geometry.

Gains, for the same reason: **glob works on every volume** (the expander walks
`MountTable::read_dir`, so `cat *.TXT` reaches the native volume, which it silently could not
before), and **`cp` works across volumes** (the read side asks one backend, the write side another —
the old verb held one `FatFs` and both ends had to be on it).

### 13.6 The census

`unaos/crates/kernel/src/shell.rs`, base `dc683c40` → this arc:

| predicate | before | after | where the survivors live |
| --- | --- | --- | --- |
| `mount_read_volume` | 19 | **0** | — (the helper is deleted) |
| `with_unafs` | 17 | **9** | the `snap` verb family (snapshots are a UnaFS FEATURE, not a file verb) and the RELICS transcript |
| `fs::fat::mount*` | 10 | **8** | the x86 table builder's program-source mount, the exec probe, the FATVERB witness's independent veto read, the `/usb` presence probe in the table builder, `fdisk -l` (device geometry), the `storm` stress writer, 2 comments |

Inside `dispatch_command`'s own match body: `with_unafs` 0, `mount_read_volume` 0,
`fs::fat::mount*` 1 — the `fdisk -l` arm, which prints DEVICE geometry and is entitled to name a
block source.

### 13.7 The VFSROUTE transcript

Arch-neutral legs, in the `:: TSTE: <name> -> PASS/FAIL ::` shape. Each drives the REAL verb through
`witness_capture` (a heap-only console, dropped on return) and asserts against a fact read off the
mount table — never against the verb's own return value.

**What makes them two-sided.** The failure this arc prevents is a verb answering off whichever
volume it happens to hold. So the legs take a PROBE NAME off the root volume's own listing and then
require it to appear under a different prefix **iff** the two prefixes resolve to the same volume.
On aarch64 `/` is native and `/fat` is the boot FAT, so a verb that kept its FAT-direct body shows a
native file under `/fat` and the leg reds; on x86 the two prefixes ARE one volume and the same
expression demands the opposite answer. Neither arch's leg can pass by being stuck.

| leg | claim |
| --- | --- |
| `vfsroute.route` | every mounted prefix resolves to its own volume, and `/fatty.bin` / `/usbfoo` resolve to the ROOT volume — the boundary rule a naive `starts_with` gets wrong |
| `vfsroute.ls` | the probe appears under `/`, and under `/fat` iff same volume |
| `vfsroute.cat` | the probe reads under `/`, and under `/fat` iff same volume |
| `vfsroute.stat` | `stat` NAMES the volume that answered, and its size equals the size `ls` reported for the same object — two verbs, one fact |
| `vfsroute.refuse` | `remove_attr` on a FAT path is exactly `Unsupported` through the table (typed attributes are a UnaFS feature the FAT backend does not implement), and `remove_dir` on the root volume is a typed error from the volume that owns the path |
| `vfsroute.touch` (aarch64) | `touch` lands on the native volume and `ls` — a different verb — sees it |
| `vfsroute.mkdir` (aarch64) | `mkdir` at that name is `-EEXIST` and the name is still a FILE: the ROUTING proof (a `mkdir` riding `fat.rs` would not find the name on the boot partition and would create a directory and report success) |
| `vfsroute.rmdir` (aarch64) | `-ENOTSUP`, and nothing anywhere is removed: the CAPABILITY proof |
| `vfsroute.rm` (aarch64) | both gone AND the root carries no `VFSR*` entry — the self-clean, asserted rather than assumed (`k3_mount_selftest` bit5 fails on a leaked fixture) |

The aarch64 legs work only in root-level FILES and never create a directory, for the reason RELICS
recorded: the UnaFS crate cannot remove one, so a leaked `/VFSRD` would be a permanent K3-mount red
on any metal Pi from its second boot onward.

Call sites: aarch64 after `emmc2::probe()` (the `midden_witness` tail), x86 from
`fatverb_storage_witness` — the two moments each board actually has volumes to route to.
`midden_witness`'s own x86 site is at boot step 5, before `pci::init` and the storage publish, where
a routing leg would assert against an empty namespace and be dead rather than quiet.
