# ROOTACL (rmbp-ledger B456) — the system trees get a `system` owner

**Finding (SECREVIEW F2's owed sibling, SEC-2026-10-06).** The kernel and `arroyo`'s ROOTDISK2 put loop create
`/system`, `/system/types/<mime>`, `/apps`, `/lib` on the UnaFS root with NO `owner` row; `native_write_authz` reads
no-owner as public, so any ring-3 principal (even `anon`) may create, write, truncate, unlink, rename, rmdir and retag
there. F2's patch (`attrsys::system_tree`, on exec-rmbp-secreview) refuses only the attribute SET.

**The seam.** No second store and no path list in the ACL: one OWNER VALUE, `system`, in the native ACL the VFS
already evaluates (`fs/vfs.rs` `native_write_authz` for every native write verb; `fs/unafs.rs` `read_authz` for reads).
`fs/rootacl.rs` (CHARTER: Kernel — fs-core) holds the rule:
* an object is SYSTEM-OWNED when its nearest `owner` row, walking the v6 parent chain, is `system` (a file put under
  `/lib/rustc/...` carries no row of its own and inherits the tree's);
* a system-owned object is WRITTEN (create under it, write, truncate, unlink, rename, rmdir, attribute set/remove)
  only by `kernel` and by a principal whose user row is the administrator (R100's admin_authority, asked of the
  PRINCIPAL, not the console session: `user:<name>#<uid>` -> `users::role_of`) — the installer and the updater.
  Everyone else: `Denied` (-EACCES) and one `[rootacl] refused` line;
* `owner=system` is never a name match (a program or user called `system` is not the owner);
* a system-owned object READS for everyone (`read_authz`: `system` permits, as no-owner did);
* an admin create under a system tree stamps no personal owner — the new object inherits `system`.

**Stamps.** (a) `arroyo esp_x86_unafs_card`: after the put loop, `tools/unafs attr-set <tree> owner '"system"'` for
each tree it put (`/apps`, `/lib`, `/system`) — the card is system-owned from its first boot; (b) the kernel, at
FILETYPES' registry build (`assoc::seed_once`, after the root answers): `rootacl::stamp` stamps `/system`,
`/system/types`, `/apps`, `/lib` that exist on the root and lack the row (the installed SSD, a pre-ROOTACL card, the
Pi/Orin `/system`). One line: `[rootacl] owner=system trees=<n> stamped=<m> root=<volume>`.

**The fold.** F2's `attrsys::system_tree` special case is subsumed (do_set -> `set_attr` -> `native_write_authz`
refuses a non-admin on a system-owned object); at the merge with exec-rmbp-merge18 the seat drops `system_tree` and
its call (the general rule ADMITS the administrator, which F2's refused).

**Milestones.** M1 this design. M2 `fs/rootacl.rs` + the two ACL hooks + the create stamp skip. M3 the stamps (kernel
at seed, arroyo after the put loop). M4 `tests rootacl` + SECURITY.md row.

**Witness.** Boot (every boot with a UnaFS root, at the registry build):
`[rootacl] owner=system trees=4 stamped=<0 on a ROOTACL card, 1 for /system/types on its first boot> root=UnaOS`.
`tests rootacl`: `:: ROOTACL: owner=system trees=4 anon_write=refused admin_write=ok -> PASS :: anon_read=ok
anon_create=refused anon_unlink=refused anon_attr=refused admin=<name> ::` (`admin_write=no-admin` before the first
user exists: FAIL-free SKIP spelled out).

**Owed.** The aarch64 syscall layer's direct `with_unafs` writers (33 sites, the K-series FAT-ACL rows) do not pass
`native_write_authz`; the Pi/Orin roots carry only `/system` — audited in a later arc. A volume older than v6 has no
parent pointers: there only the stamped tree roots are system-owned (their children stay public) — the x86 card and the
installed SSD are formatted v6 by this tree. Objects a ring-3 principal planted in `/system` before ROOTACL keep their
owner row (nearest owner wins).
