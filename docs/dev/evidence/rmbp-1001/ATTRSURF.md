# ATTRSURF — the typed-attribute surface, end to end, through the VFS (B299; audit B291, B294 identity half)

## Final signatures (PREFS M3 mirrors these)

```rust
// crate::fs::vfs — backend-neutral, both arches
pub enum AttrValue { Int(i64), Float(f64), Str(String), Blob(Vec<u8>), Vector(Vec<f32>) }
pub struct Stat { pub kind: NodeKind, pub size: u64, pub id: Option<u64>, pub mtime: Option<u64> /* unix seconds */ }

trait VfsBackend {
    fn set_attr(&self, rel: &str, key: &str, value: AttrValue, principal: &str) -> Result<(), VfsError>;   // default Unsupported
    fn get_attr(&self, rel: &str, key: &str, principal: &str) -> Result<AttrValue, VfsError>;              // default Unsupported; absent key = Backend("no-attr")
    fn list_attrs(&self, rel: &str, principal: &str) -> Result<Vec<(String, AttrValue)>, VfsError>;        // default Unsupported
    fn query(&self, expr: &str, principal: &str) -> Result<Vec<(u64, String)>, VfsError>;                  // default Unsupported; paths volume-relative
}
impl MountTable {
    pub fn set_attr(&self, path: &str, key: &str, value: AttrValue, principal: &str) -> Result<(), VfsError>;
    pub fn get_attr(&self, path: &str, key: &str, principal: &str) -> Result<AttrValue, VfsError>;
    pub fn list_attrs(&self, path: &str, principal: &str) -> Result<Vec<(String, AttrValue)>, VfsError>;
    pub fn query(&self, expr: &str, principal: &str) -> Result<Vec<(u64, String)>, VfsError>;            // every mount, one pass per volume, namespace paths
}
```

## Finding

The unafs crate has typed attributes (`inode.rs` `AttributeValue` Int/Float/String/Blob/Vector),
`set_attribute`/`get_attribute`/`remove_attribute`/`query`, all `no_std`. Nothing above the VFS could
set or read one: the trait had only `remove_attr`, the shell only `setfattr -x`, una-abi no attribute,
stat or query syscall, `Stat` was `{kind,size}` and the VFS hid inode ids by contract. The kernel never
called `query`.

## Seam: fs-core

One backend-neutral surface on `VfsBackend`, implemented once by the native adapter over `with_unafs`;
every consumer (shell verbs, both arches' syscalls, both arches' bus verbs, the fixture) goes through
`MountTable` and the ONE fulfiller module `fs/attrsys.rs`, which owns the wire codec and the errno
mapping. FAT inherits the refusing defaults and answers `-ENOTSUP` honestly, as `remove_attr` did.

## Rules the adapter keeps

* **ACL**: get/list/query use `unafs::read_authz` (the SYS_OPEN evaluator); set/remove use
  `native_write_authz` (the write twin). A query hit the principal may not read is DROPPED, not shown.
* **Reserved keys**: `owner` and `grants:*` are the per-object ACL. A non-kernel principal may not set
  or remove them through this surface (`-EACCES`): with only `remove_attr` that was already a hole (a
  `w` grantee could drop `owner` and make an object public); `set_attr` would have made it an
  escalation. ACL edits stay with the grant machinery.
* **Identity**: `Stat.id` is the unafs inode id (`None` on FAT); `Stat.mtime` is unix seconds from the
  FAT last-write stamp (`None` on native: UnaFS has no time field until UNAFSTIME).
* **Query paths (v1 cost)**: the crate returns inodes only, so `native_query_paths` (vfs.rs, ONE
  function) walks the volume once per query from the root, collecting `inode -> path` for every
  reachable non-System object (bounded: 16384 nodes, depth 32), then maps the hits. Cost is O(objects
  on the volume) directory reads per query. F3F4 makes the crate return `(inode_id, path)`; at that
  fold `native_query_paths` is deleted and `query` maps the crate's pairs directly.

## Wire (una-abi, one layout for syscalls and bus bodies)

* Value: `AttrWireHdr { tag: u8, _rsv: [u8;3], len: u32 }` (8 bytes, LE) then `len` payload bytes.
  Tags 1 Int (8 B i64) · 2 Float (8 B f64) · 3 Str (UTF-8) · 4 Blob · 5 Vector (len % 4 == 0, f32 LE).
  `ATTR_VALUE_MAX = 3072`.
* Request: `[path_len u16][key_len u16][path][key][value?]` (value only for SET). path ≤ 255, key ≤ 255.
* LIST reply: repeated `[key_len u16][key][value wire]`. QUERY reply: repeated `[id u64][path_len u16][path]`.
* STAT reply: `UserStat { kind u32 (0 file, 1 dir), flags u32 (bit0 id, bit1 mtime), size u64, id u64, mtime u64 }`.
* Syscalls 51..55: `SYS_ATTR_SET(req,len)`, `SYS_ATTR_GET(req,len,out,cap)`, `SYS_ATTR_LIST(path,len,out,cap)`,
  `SYS_QUERY(expr,len,out,cap)`, `SYS_STAT(path,len,out)`. Return = bytes written / 0 / -errno; an out
  buffer too small is `-ERANGE`.
* Bus verbs 11..15 `BUS_VERB_ATTR_SET/GET/LIST/QUERY/STAT`: request body = the syscall's input bytes,
  reply body = the syscall's output bytes. KATs in `bus::attr::selftest` (`:: BANDY-ATTR: ... ::`).
* Errnos (verbs print the same names): ENOENT, ENOTDIR, EISDIR, EACCES, ENOTSUP(-95), ENODEV,
  ENODATA(-61, absent key), EINVAL (bad value/expr/request), ERANGE (out too small), EIO.
* Principal: aarch64 = the caller's stamped `PrincipalRecord` projected by `principal_native_string`
  (the string native `owner` rows carry); x86 = `user:<name>#<uid>` for a slot stamped with the open
  session, else `anon`. Anonymous reads public objects only.

## Milestones

* M1 — VFS: `AttrValue`, `Stat.id/mtime`, the four trait methods + MountTable routing, native impls.
* M2 — verbs: `setfattr <path> <key>=<value>` (`-x` kept), `getfattr <path> [key]`, `query <expr>`,
  `stat` prints id + mtime; HOST_VERBS rows; help rows.
* M3 — syscalls 51..55 on both dispatchers; bus verbs 11..15 fulfilled on both arches; codec KATs.
* M4 — `tests attr`.

## Witness

`:: ATTRSURF: set=3 get=3 list=<n> query=2 denied=3 dir=<dir> -> PASS ::` on a UnaFS volume;
`:: ATTRSURF: set=0 get=0 list=0 query=0 denied=0 reason=no-unafs-volume -> SKIP ::` on a FAT-only tree
(today's rMBP until UNAFSX86 lands). Codec: `:: BANDY-ATTR: ... -> PASS ::` every boot beside BANDY-CODEC2.

## Owed

* x86 flies only after UNAFSX86 (native backend on x86); every new native cfg is `target_arch = "aarch64"`.
* Live queries (F5), range ops (F3F4), mtime on native (UNAFSTIME).
* The crate's catalog is rewritten whole per `set_attribute` (O(n)) — unchanged here.
