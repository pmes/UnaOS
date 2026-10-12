//! Web Storage (HTML §12.2).
//!
//! - [`Area`] / [`with_area`] — the storage areas the page-facing `localStorage` and `sessionStorage` read
//!   and write (AETHERNET, SR70): `localStorage` is one area per origin shared by every document of the
//!   process (so a write in one document is seen — and announced by a `storage` event — in the others);
//!   `sessionStorage` is per origin per page thread (one top-level browsing context). Both live in memory
//!   for the life of the process; the quota is [`QUOTA`] UTF-16 code units of keys plus values per area.
//! - [`PRELUDE`] — the `Storage` interface over the `__storage` native (`js::net`), with named-property
//!   access through a Proxy, and the `storage` event's receiving half.
//! - [`LocalStorage`] — the persistent per-origin JSON file store (under the platform data directory), the
//!   persistence layer a later arc wires behind the areas.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// Most UTF-16 code units (keys plus values) one storage area holds (HTML §12.2.1: "5 MiB per origin" is
/// what engines give).
pub const QUOTA: usize = 5 * 1024 * 1024;

/// One storage area: its key/value pairs in insertion order (what `key(n)` enumerates).
#[derive(Debug, Default)]
pub struct Area {
    keys: Vec<String>,
    map: HashMap<String, String>,
    units: usize,
}

fn units(s: &str) -> usize {
    s.encode_utf16().count()
}

impl Area {
    pub fn len(&self) -> usize {
        self.keys.len()
    }
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
    pub fn key(&self, i: usize) -> Option<&str> {
        self.keys.get(i).map(String::as_str)
    }
    pub fn keys(&self) -> Vec<String> {
        self.keys.clone()
    }
    pub fn get(&self, k: &str) -> Option<&str> {
        self.map.get(k).map(String::as_str)
    }
    /// Sets `k`; answers the previous value, or `Err` when the area would pass [`QUOTA`] (nothing changes).
    pub fn set(&mut self, k: &str, v: &str) -> Result<Option<String>, ()> {
        let old = self.map.get(k);
        let freed = old.map(|o| units(o) + units(k)).unwrap_or(0);
        let need = units(k) + units(v);
        if self.units - freed + need > QUOTA {
            return Err(());
        }
        self.units = self.units - freed + need;
        let prev = self.map.insert(k.to_string(), v.to_string());
        if prev.is_none() {
            self.keys.push(k.to_string());
        }
        Ok(prev)
    }
    pub fn remove(&mut self, k: &str) -> Option<String> {
        let old = self.map.remove(k)?;
        self.units -= units(k) + units(&old);
        self.keys.retain(|x| x != k);
        Some(old)
    }
    /// Empties the area; answers whether it held anything.
    pub fn clear(&mut self) -> bool {
        let had = !self.keys.is_empty();
        self.keys.clear();
        self.map.clear();
        self.units = 0;
        had
    }
}

fn local_areas() -> &'static Mutex<HashMap<String, Area>> {
    static LOCAL: OnceLock<Mutex<HashMap<String, Area>>> = OnceLock::new();
    LOCAL.get_or_init(|| Mutex::new(HashMap::new()))
}

thread_local! {
    static SESSION: RefCell<HashMap<String, Area>> = RefCell::new(HashMap::new());
}

/// Runs `f` on `origin`'s local area (`local`, process-wide) or session area (this page thread's).
pub fn with_area(local: bool, origin: &str, f: &mut dyn FnMut(&mut Area)) {
    if local {
        let mut m = local_areas().lock().unwrap_or_else(|e| e.into_inner());
        f(m.entry(origin.to_string()).or_default());
    } else {
        SESSION.with(|m| f(m.borrow_mut().entry(origin.to_string()).or_default()));
    }
}

/// The `Storage` interface (HTML §12.2.1) over `__storage`, and the `storage` event's receiving half.
pub const PRELUDE: &str = r#"
(function () {
    var AREA = typeof WeakMap === 'function' ? new WeakMap() : null;
    var areaOf = function (st) {
        var a = AREA.get(st);
        if (a === undefined) { throw new TypeError('Illegal invocation'); }
        return a;
    };
    function Storage() { throw new TypeError('Illegal constructor'); }
    var P = Storage.prototype;
    var def = function (name, len, f) {
        Object.defineProperty(f, 'length', { value: len });
        Object.defineProperty(P, name, { value: f, writable: true, enumerable: true, configurable: true });
    };
    var need = function (n, have, m) {
        if (have < n) {
            throw new TypeError("Failed to execute '" + m + "' on 'Storage': " + n + " argument" + (n > 1 ? 's' : '') +
                ' required, but only ' + have + ' present.');
        }
    };
    def('key', 1, function key(i) { need(1, arguments.length, 'key'); return __storage(areaOf(this), 'key', (Number(i) >>> 0)); });
    def('getItem', 1, function getItem(k) { need(1, arguments.length, 'getItem'); return __storage(areaOf(this), 'get', String(k)); });
    def('setItem', 2, function setItem(k, v) { need(2, arguments.length, 'setItem'); __storage(areaOf(this), 'set', String(k), String(v)); });
    def('removeItem', 1, function removeItem(k) { need(1, arguments.length, 'removeItem'); __storage(areaOf(this), 'remove', String(k)); });
    def('clear', 0, function clear() { __storage(areaOf(this), 'clear'); });
    Object.defineProperty(P, 'length', {
        get: function () { return __storage(areaOf(this), 'length'); }, enumerable: true, configurable: true,
    });
    Object.defineProperty(P, Symbol.toStringTag, { value: 'Storage', configurable: true });
    Object.defineProperty(globalThis, 'Storage', { value: Storage, writable: true, configurable: true, enumerable: false });

    // Named properties (WebIDL §3.9, no [LegacyOverrideBuiltIns]: a name on the prototype chain wins).
    var mk = function (area) {
        var target = Object.create(P);
        var proxy = new Proxy(target, {
            get: function (t, p, r) {
                if (typeof p === 'symbol' || p in t) { return Reflect.get(t, p, proxy); }
                var v = __storage(area, 'get', p);
                return v === null ? undefined : v;
            },
            set: function (t, p, v, r) {
                if (typeof p === 'symbol' || p in t) { return Reflect.set(t, p, v, proxy); }
                __storage(area, 'set', p, String(v));
                return true;
            },
            has: function (t, p) {
                if (typeof p === 'symbol' || p in t) { return true; }
                return __storage(area, 'get', p) !== null;
            },
            deleteProperty: function (t, p) {
                if (typeof p === 'symbol' || Object.prototype.hasOwnProperty.call(t, p)) { return Reflect.deleteProperty(t, p); }
                __storage(area, 'remove', p);
                return true;
            },
            ownKeys: function (t) { return __storage(area, 'keys').concat(Reflect.ownKeys(t)); },
            getOwnPropertyDescriptor: function (t, p) {
                if (typeof p !== 'symbol') {
                    var v = __storage(area, 'get', p);
                    if (v !== null && !(p in P)) { return { value: v, writable: true, enumerable: true, configurable: true }; }
                }
                return Reflect.getOwnPropertyDescriptor(t, p);
            },
            defineProperty: function (t, p, d) {
                if (typeof p === 'symbol' || p in P) { return Reflect.defineProperty(t, p, d); }
                __storage(area, 'set', p, String(d.value));
                return true;
            },
        });
        AREA.set(proxy, area);
        AREA.set(target, area);
        return proxy;
    };
    var ls = mk('local'), ss = mk('session');
    Object.defineProperty(globalThis, 'localStorage', { get: function () { return ls; }, configurable: true, enumerable: true });
    Object.defineProperty(globalThis, 'sessionStorage', { get: function () { return ss; }, configurable: true, enumerable: true });

    // HTML §12.2.1 "broadcast": a write in another document of this origin arrives as a task here.
    Object.defineProperty(globalThis, '__storage_event', {
        value: function (origin, key, oldValue, newValue, url) {
            __net_fire(globalThis, 'StorageEvent', 'storage',
                { key: key, oldValue: oldValue, newValue: newValue, url: url, storageArea: ls });
        },
        writable: true, configurable: true, enumerable: false,
    });
})();
"#;

#[derive(Debug)]
pub struct LocalStorage {
    data: HashMap<String, String>,
    path: PathBuf,
}

impl LocalStorage {
    /// Opens (or creates empty) the store at `path`.
    pub fn new(path: PathBuf) -> Self {
        let data = fs::read_to_string(&path)
            .ok()
            .and_then(|c| serde_json::from_str(&c).ok())
            .unwrap_or_default();
        Self { data, path }
    }

    /// The store file for `origin` (sanitized to a file name) under the platform data directory.
    pub fn for_origin(origin: &str) -> Self {
        let sanitized = origin.replace(|c: char| !c.is_alphanumeric(), "_");
        let dir = directories::ProjectDirs::from("", "UnaOS", "Aether")
            .map(|p| p.data_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("/tmp/aether_storage"));
        let _ = fs::create_dir_all(&dir);
        Self::new(dir.join(format!("{sanitized}.json")))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn get_item(&self, key: &str) -> Option<&str> {
        self.data.get(key).map(String::as_str)
    }

    pub fn set_item(&mut self, key: &str, value: &str) {
        self.data.insert(key.to_string(), value.to_string());
        self.save();
    }

    pub fn remove_item(&mut self, key: &str) {
        self.data.remove(key);
        self.save();
    }

    pub fn clear(&mut self) {
        self.data.clear();
        self.save();
    }

    fn save(&self) {
        if let Ok(content) = serde_json::to_string(&self.data) {
            let _ = fs::write(&self.path, content);
        }
    }
}
