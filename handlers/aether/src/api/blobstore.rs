//! AETHERWPT3 (LEDGER SR138): the blob URL store (File API §8.3, "A URL for Blob and MediaSource reference"): one per
//! user agent, keyed by the serialized URL, each entry the Blob's bytes and type and the creating environment's
//! origin. `URL.createObjectURL` / `revokeObjectURL` (api/platform.rs, page and worker realms) add and remove
//! entries; `fetch` and `XMLHttpRequest` (Fetch §4.3, the `blob` scheme), a nested navigation
//! (`api::fetch::fetch_subdocument`) and script loading (`js::host`) resolve them.
//!
//! The store is process-wide (a dedicated worker's thread creates URLs its page fetches); fetches are partitioned
//! by origin — an entry resolves for a fetch only from the origin that created it (File API §8.3.4 / Fetch §4.3:
//! the environment's storage key must match), while a navigation resolves it from anywhere (the navigation
//! exception).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// A blob URL entry: the Blob's bytes (shared — a revoked URL's Request keeps its reference), its type, and the
/// origin of the environment that created it.
#[derive(Clone, Debug)]
pub struct Entry {
    pub bytes: Arc<Vec<u8>>,
    pub mime: String,
    pub origin: String,
}

fn store() -> &'static Mutex<HashMap<String, Entry>> {
    static STORE: OnceLock<Mutex<HashMap<String, Entry>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// File API §8.3.1 "generate a new blob URL": `blob:` + the serialized origin + `/` + a UUID (RFC 4122 §4.4,
/// version 4), and the entry added under it.
pub fn register(origin: &str, bytes: Vec<u8>, mime: &str) -> String {
    let mut r = crate::api::platform::entropy(16);
    r[6] = (r[6] & 0x0F) | 0x40;
    r[8] = (r[8] & 0x3F) | 0x80;
    let h: String = r.iter().map(|b| format!("{b:02x}")).collect();
    let uuid = format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32]);
    let o = if origin.is_empty() { "null" } else { origin };
    let url = format!("blob:{o}/{uuid}");
    if let Ok(mut s) = store().lock() {
        s.insert(url.clone(), Entry { bytes: Arc::new(bytes), mime: mime.to_string(), origin: o.to_string() });
    }
    url
}

/// File API §8.3.3 "remove an entry from the blob URL store" for `url` exactly as given (a fragment makes it a
/// different URL), when the caller's origin created it.
pub fn revoke(url: &str, origin: &str) {
    if let Ok(mut s) = store().lock() {
        if s.get(url).is_some_and(|e| e.origin == origin || origin.is_empty()) {
            s.remove(url);
        }
    }
}

/// File API §8.3.4 "resolve a blob URL" (the URL serialized without its fragment); `origin` = the requesting
/// environment's origin for a fetch (the partition), None for a navigation.
pub fn resolve(url: &str, origin: Option<&str>) -> Option<Entry> {
    let key = url.split('#').next().unwrap_or(url);
    let s = store().lock().ok()?;
    let e = s.get(key)?;
    if origin.is_some_and(|o| o != e.origin) {
        return None;
    }
    Some(e.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_resolve_revoke() {
        let u = register("http://a.test", b"hi".to_vec(), "text/plain");
        assert!(u.starts_with("blob:http://a.test/"));
        assert_eq!(u.len(), "blob:http://a.test/".len() + 36);
        assert_eq!(resolve(&format!("{u}#frag"), Some("http://a.test")).map(|e| e.bytes.to_vec()), Some(b"hi".to_vec()));
        assert!(resolve(&u, Some("http://b.test")).is_none(), "another origin's fetch");
        assert!(resolve(&u, None).is_some(), "a navigation");
        revoke(&format!("{u}#frag"), "http://a.test");
        assert!(resolve(&u, None).is_some(), "only exact matches revoke");
        revoke(&u, "http://a.test");
        assert!(resolve(&u, None).is_none());
    }
}
