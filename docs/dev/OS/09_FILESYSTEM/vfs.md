# VFS: the unifying virtual-filesystem layer

Status: **VFS-2 landed** — the write surface is implemented for both backends
(`unaos/crates/kernel/src/fs/vfs.rs`), on top of VFS-1's read/resolve/authorize spine.
The spine is still **unconsumed by production callers**: no shell command, syscall, or
EL0 path routes through it yet (adoption — shell, `SYS_OPEN`, `genet`/net paths — is a
follow-up). VFS-2's write path is proven by two self-verifying on-card witnesses
(§9); wiring those into the boot battery is a one-line-per-witness `syscall.rs` change
carried as a deferred diff (that file is outside the VFS lane).

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

## 9. The VFS-2 write witnesses

Two on-card, self-cleaning witnesses prove a `create` → `write` → read-back → checksum round-trip
through the mount table to each real backend (aarch64-only; honest skip when the volume is absent):

* `vfs::vfs2_fat_write_witness()` — FAT volume mounted at `/`;
* `vfs::vfs2_native_write_witness()` — native UnaFS volume mounted at `/`.

Each emits `:: VFS2: write test — created <path>, wrote <n>, readback OK :: PASS ::` and unlinks
its scratch file. They are invoked from the `syscall.rs` storage battery (after the K-series, so
their scratch never perturbs those fixtures); because `syscall.rs` is outside the VFS lane, that
one-line-per-witness wiring lands as a deferred diff alongside this arc.

## 10. File map

* `unaos/crates/kernel/src/fs/vfs.rs` — the spine + adapters: `VfsError`, `NodeKind`, `Stat`,
  `DirEnt`, the `VfsBackend` trait (read + write surface), `MountTable` (mount/unmount/resolve +
  read/write resolve-then-dispatch conveniences), the `FatBackend` and `NativeBackend` adapters
  (read + write), the VFS-1 resolution witness, and the two VFS-2 write witnesses.
* `unaos/crates/kernel/src/fs/fat.rs`, `…/unafs.rs` — the backends the adapters wrap (unchanged
  by VFS-2; their existing write primitives are consumed as-is).
