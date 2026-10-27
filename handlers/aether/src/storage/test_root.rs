//! AETHER-TESTROOT (LEDGER SR355): the ONE storage fixture of Aether's lib tests (cfg(test) only; no production
//! path names it). [`TestRoot`] serialises its holders on one lock, points `storage::set_root_dir` at a fresh
//! directory under the test process's own [`process_dir`], and empties every process-wide store
//! (`storage::local_areas`, `cache`'s stores, `quota`'s store, `locks`' manager, `idb`'s registry) on acquire
//! and again on drop (a panic included), so no test sees another's state and no file outlives the run. A
//! lib-test build's default storage root is [`process_dir`] too (`storage::data_dir`), so nothing in
//! `cargo test -p aether --lib` resolves to the person's platform data directory. Every lib test that touches
//! a store holds one: the three IndexedDB KATs (`storage::idb`, `js::idb`) and the Web Locks KAT.
//!
//! CHARTER: Aether — the engine half's test fixture (`storage`); `workers::sw::world` is not reset here (every
//! page load reaches it, held or not: owed TESTROOT-SW).

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

/// The test process's own storage directory: `<target>/tmp/aether-libtest-<pid>` (beside the integration
/// binaries' `CARGO_TARGET_TMPDIR` roots; `<target>` is the test executable's nearest ancestor holding cargo's
/// `CACHEDIR.TAG`, whichever layout cargo builds the test binary in), else `<temp>/aether-libtest-<pid>`.
pub(crate) fn process_dir() -> PathBuf {
    static D: OnceLock<PathBuf> = OnceLock::new();
    D.get_or_init(|| {
        let target_tmp = std::env::current_exe().ok().and_then(|exe| {
            let target = exe.ancestors().skip(1).find(|d| d.join("CACHEDIR.TAG").is_file())?;
            let tmp = target.join("tmp");
            std::fs::create_dir_all(&tmp).ok()?;
            Some(tmp)
        });
        target_tmp.unwrap_or_else(std::env::temp_dir).join(format!("aether-libtest-{}", std::process::id()))
    })
    .clone()
}

/// Empties every process-wide storage store under its own lock, one at a time (each reopens lazily under the
/// current `storage::root_dir`).
fn reset_stores() {
    super::reset_local_areas_for_test();
    super::cache::reset_for_test();
    super::quota::reset_for_test();
    super::locks::reset_for_test();
    super::idb::reset_for_test();
}

/// The storage root a lib test holds for its whole run (see the module doc).
pub(crate) struct TestRoot {
    dir: PathBuf,
    _lock: MutexGuard<'static, ()>,
}

impl TestRoot {
    pub(crate) fn new(name: &str) -> TestRoot {
        static LOCK: Mutex<()> = Mutex::new(());
        let lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = process_dir().join(name);
        let _ = std::fs::remove_dir_all(&dir);
        super::set_root_dir(Some(dir.clone()));
        reset_stores();
        TestRoot { dir, _lock: lock }
    }

    /// The directory the storage buckets point at while this guard is held.
    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        reset_stores();
        super::set_root_dir(None);
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Every file under `dir` (and `dir` itself) with its modification time; empty when `dir` is absent.
    fn snapshot(dir: &Path) -> Vec<(PathBuf, Option<std::time::SystemTime>)> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            let Ok(meta) = std::fs::symlink_metadata(&d) else { continue };
            out.push((d.clone(), meta.modified().ok()));
            if meta.is_dir() {
                if let Ok(rd) = std::fs::read_dir(&d) {
                    stack.extend(rd.flatten().map(|e| e.path()));
                }
            }
        }
        out.sort();
        out
    }

    fn files_under(dir: &Path) -> usize {
        snapshot(dir).iter().filter(|(p, _)| p.is_file()).count()
    }

    /// AETHER-TESTROOT: one op per process-wide store under a TestRoot writes under the guard's directory, the
    /// drop removes it, a fresh guard finds every store empty, and the platform data directory is untouched.
    #[test]
    fn testroot_leaves_no_residue() {
        let platform = crate::storage::platform_data_dir();
        let before = snapshot(&platform);
        let key = "https://testroot.kat";

        let root = TestRoot::new("aether_testroot_kat");
        let dir = root.dir().to_path_buf();
        assert!(dir.starts_with(process_dir()), "the guard's root is the test process's own");
        assert!(!dir.starts_with(&platform), "never the platform data directory");
        assert_eq!(crate::storage::root_dir(), dir);
        crate::storage::with_area(true, key, &mut |a| {
            a.set("k", "v").unwrap();
        });
        crate::storage::cache::open(key, "c").unwrap();
        let req = json!({ "url": "https://testroot.kat/x", "method": "GET", "headers": [] });
        let resp = json!({ "status": 200, "statusText": "OK", "headers": [], "type": "basic", "url": "https://testroot.kat/x" });
        crate::storage::cache::put(key, "c", &req, &resp, b"body").unwrap();
        assert!(crate::storage::quota::persist(key));
        assert_eq!(crate::storage::locks::request(key, "n", true, false, false, 9101)["state"], "granted");
        let o = crate::storage::idb::open(key, "db", Some(1), 0, false).unwrap();
        crate::storage::idb::commit(o["upgrade"].as_u64().unwrap()).unwrap();
        crate::storage::idb::close(o["conn"].as_u64().unwrap());
        assert!(files_under(&dir) > 0, "the ops wrote under the guard's root");
        drop(root);
        assert!(!dir.exists(), "the drop removes the guard's root");

        let again = TestRoot::new("aether_testroot_kat");
        crate::storage::with_area(true, key, &mut |a| assert!(a.is_empty(), "localStorage areas reset"));
        assert!(crate::storage::cache::cache_names(key).is_empty(), "Cache API stores reset");
        assert!(!crate::storage::quota::persisted(key), "the bucket-mode store reset");
        assert!(crate::storage::locks::query(key)["held"].as_array().unwrap().is_empty(), "the lock manager reset");
        assert_eq!(crate::storage::idb::databases(key), json!([]), "the IndexedDB registry reset");
        drop(again);

        assert_eq!(snapshot(&platform), before, "the platform data directory is untouched");
    }
}
