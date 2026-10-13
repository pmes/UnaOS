//! `navigator.storage` — the Storage Standard's StorageManager (AETHERSTORAGE, LEDGER SR94; Storage §5).
//!
//! CHARTER: Aether — engine; `js::idb` installs the `StorageManager` interface over [`dispatch`].
//!
//! - `estimate()` (§5 "storage estimate"): `usage` is the bytes the storage key's buckets hold — the IndexedDB
//!   files, the Cache API store, the service-worker scripts and the `localStorage` area (two bytes per UTF-16
//!   code unit) — with `usageDetails` per area; `quota` is the UA's: [`QUOTA`] per storage key, never more
//!   than the usage plus what the volume has free when that is knowable.
//! - `persist()` / `persisted()` (§4.3 bucket mode): the mode is kept per storage key in one `kvstore_core`
//!   file (`<root>/quota.kv`, meta `persist:<key>`). This UA grants persistence to every secure context that
//!   asks (no prompt: there is no permission UI to ask through), and best-effort buckets are never evicted
//!   either — eviction is owed with the quota pressure it answers.

use kvstore_core::dir::DirBacking;
use kvstore_core::Store;
use serde_json::{json, Value as J};
use std::sync::{Mutex, OnceLock};

/// The quota one storage key gets: 2 GiB.
pub const QUOTA: u64 = 2 * 1024 * 1024 * 1024;

fn modes() -> &'static Mutex<Option<Store<DirBacking>>> {
    static M: OnceLock<Mutex<Option<Store<DirBacking>>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(None))
}

fn with_modes<T>(f: impl FnOnce(&mut Store<DirBacking>) -> T) -> Option<T> {
    let mut g = modes().lock().unwrap_or_else(|e| e.into_inner());
    if g.is_none() {
        let b = DirBacking::new(&super::root_dir()).ok()?;
        *g = Store::open(b, "quota.kv").ok();
    }
    g.as_mut().map(f)
}

/// Whether the storage key's bucket is persistent.
pub fn persisted(key: &str) -> bool {
    let k = format!("persist:{key}");
    with_modes(|s| s.meta(k.as_bytes()) == Some(&b"1"[..])).unwrap_or(false)
}

/// Makes the bucket persistent; answers the resulting mode.
pub fn persist(key: &str) -> bool {
    let k = format!("persist:{key}");
    with_modes(|s| {
        if s.meta(k.as_bytes()) == Some(&b"1"[..]) {
            return true;
        }
        let mut t = s.begin();
        if s.set_meta(&mut t, k.as_bytes(), b"1").is_err() {
            s.rollback(t);
            return false;
        }
        s.commit(t).is_ok()
    })
    .unwrap_or(false)
}

fn local_storage_bytes(key: &str) -> u64 {
    let mut n = 0u64;
    super::with_area(true, key, &mut |a| {
        for k in a.keys() {
            n += (k.encode_utf16().count() + a.get(&k).map(|v| v.encode_utf16().count()).unwrap_or(0)) as u64 * 2;
        }
    });
    n
}

/// §5 "storage estimate" for the storage key.
pub fn estimate(key: &str, origin: &str) -> J {
    let idb = super::idb::usage(key);
    let caches = super::cache::usage(key);
    let sw = crate::workers::sw::usage(key);
    let ls = local_storage_bytes(origin);
    let usage = idb + caches + sw + ls;
    json!({
        "usage": usage,
        "quota": QUOTA.max(usage),
        "usageDetails": { "indexedDB": idb, "caches": caches, "serviceWorkerRegistrations": sw, "localStorage": ls },
    })
}

/// `__storage_manager(op)`: `estimate`, `persist`, `persisted`.
pub fn dispatch(key: &str, origin: &str, op: &str) -> J {
    match op {
        "estimate" => estimate(key, origin),
        "persist" => json!(persist(key)),
        "persisted" => json!(persisted(key)),
        _ => J::Null,
    }
}
