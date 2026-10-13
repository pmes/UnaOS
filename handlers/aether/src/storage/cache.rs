//! The Cache API's disk store (AETHERSTORAGE, LEDGER SR94; Service Workers §5.4–§5.5 `Cache`, `CacheStorage`).
//!
//! CHARTER: Aether — engine; `js::cache_api` is the page-facing `caches`, and the service-worker realm
//! (`workers::sw`) reaches the same store through the same functions.
//!
//! Per storage key, `<root>/caches/<key_dir>/`:
//! - `index.kv` (`kvstore_core`): meta `names` = the cache names in creation order (§5.5 "name to cache map",
//!   an ordered map); tree `c:<name>` = the cache's request-response list, keyed by an insertion sequence
//!   number (big-endian, so the tree's order is the list's order) → the entry as JSON (the request's URL, method
//!   and headers; the response's status, status text, headers, type and URL; the body file);
//! - `b<seq>.body` — one response body per entry.
//!
//! §5.4.2 "query cache" / "request matches cached item": the method must be `GET` unless `ignoreMethod`; URLs
//! compare with their fragments removed (and their queries, under `ignoreSearch`); unless `ignoreVary`, every
//! header the cached response's `Vary` names must have the same value in both requests, and `Vary: *` never
//! matches. A `put` replaces the entries its request matches (§5.4.3 "batch cache operations").

use super::bucket_dir;
use kvstore_core::dir::DirBacking;
use kvstore_core::Store;
use serde_json::{json, Value as J};
use std::collections::HashMap;
use std::ops::Bound;
use std::sync::{Mutex, OnceLock};

fn stores() -> std::sync::MutexGuard<'static, HashMap<String, Store<DirBacking>>> {
    static S: OnceLock<Mutex<HashMap<String, Store<DirBacking>>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new())).lock().unwrap_or_else(|e| e.into_inner())
}

fn with_store<T>(key: &str, f: impl FnOnce(&mut Store<DirBacking>) -> Result<T, String>) -> Result<T, String> {
    let mut m = stores();
    if !m.contains_key(key) {
        let b = DirBacking::new(&bucket_dir("caches", key)).map_err(|e| e.to_string())?;
        let s = Store::open(b, "index.kv").map_err(|e| e.to_string())?;
        m.insert(key.to_string(), s);
    }
    f(m.get_mut(key).expect("inserted"))
}

fn names(s: &Store<DirBacking>) -> Vec<String> {
    s.meta(b"names").and_then(|v| serde_json::from_slice(v).ok()).unwrap_or_default()
}

fn tree(name: &str) -> String {
    format!("c:{name}")
}

fn body_path(key: &str, file: &str) -> std::path::PathBuf {
    bucket_dir("caches", key).join(file)
}

/// The match options of a query (§5.4 `CacheQueryOptions`).
#[derive(Debug, Clone, Copy, Default)]
pub struct QueryOptions {
    pub ignore_search: bool,
    pub ignore_method: bool,
    pub ignore_vary: bool,
}

impl QueryOptions {
    pub fn from_json(v: &J) -> QueryOptions {
        let b = |k: &str| v.get(k).and_then(J::as_bool).unwrap_or(false);
        QueryOptions { ignore_search: b("ignoreSearch"), ignore_method: b("ignoreMethod"), ignore_vary: b("ignoreVary") }
    }
}

fn strip(url: &str, search: bool) -> &str {
    let u = url.split('#').next().unwrap_or(url);
    if search { u.split('?').next().unwrap_or(u) } else { u }
}

fn header<'a>(list: &'a J, name: &str) -> Option<&'a str> {
    list.as_array()?.iter().find_map(|kv| {
        let k = kv.get(0)?.as_str()?;
        k.eq_ignore_ascii_case(name).then(|| kv.get(1).and_then(J::as_str).unwrap_or(""))
    })
}

/// §5.4.2 "request matches cached item".
fn matches(req: &J, entry: &J, o: QueryOptions) -> bool {
    let method = req.get("method").and_then(J::as_str).unwrap_or("GET");
    if !o.ignore_method && !method.eq_ignore_ascii_case("GET") {
        return false;
    }
    let ru = req.get("url").and_then(J::as_str).unwrap_or("");
    let eu = entry.get("url").and_then(J::as_str).unwrap_or("");
    if strip(ru, o.ignore_search) != strip(eu, o.ignore_search) {
        return false;
    }
    if o.ignore_vary {
        return true;
    }
    let Some(vary) = header(entry.get("headers").unwrap_or(&J::Null), "vary") else { return true };
    for f in vary.split(',').map(str::trim).filter(|f| !f.is_empty()) {
        if f == "*" {
            return false;
        }
        let a = header(req.get("headers").unwrap_or(&J::Null), f);
        let b = header(entry.get("reqHeaders").unwrap_or(&J::Null), f);
        if a != b {
            return false;
        }
    }
    true
}

/// `caches.open(name)`.
pub fn open(key: &str, name: &str) -> Result<(), String> {
    with_store(key, |s| {
        let mut n = names(s);
        if n.iter().any(|x| x == name) {
            return Ok(());
        }
        n.push(name.to_string());
        let mut t = s.begin();
        let r = s.create_tree(&mut t, &tree(name)).and_then(|_| s.set_meta(&mut t, b"names", &serde_json::to_vec(&n).unwrap_or_default()));
        match r {
            Ok(()) => s.commit(t).map_err(|e| e.to_string()),
            Err(e) => {
                s.rollback(t);
                Err(e.to_string())
            }
        }
    })
}

/// `caches.has(name)`.
pub fn has(key: &str, name: &str) -> bool {
    with_store(key, |s| Ok(names(s).iter().any(|x| x == name))).unwrap_or(false)
}

/// `caches.keys()`: the names in creation order.
pub fn cache_names(key: &str) -> Vec<String> {
    with_store(key, |s| Ok(names(s))).unwrap_or_default()
}

/// `caches.delete(name)`: the cache and its bodies.
pub fn delete_cache(key: &str, name: &str) -> Result<bool, String> {
    let files = with_store(key, |s| {
        let mut n = names(s);
        if !n.iter().any(|x| x == name) {
            return Ok(None);
        }
        let files: Vec<String> = s
            .range(&tree(name), Bound::Unbounded, Bound::Unbounded, false)
            .into_iter()
            .filter_map(|(_, v)| serde_json::from_slice::<J>(v).ok()?.get("body")?.as_str().map(str::to_string))
            .collect();
        n.retain(|x| x != name);
        let mut t = s.begin();
        let r = s.drop_tree(&mut t, &tree(name)).and_then(|_| s.set_meta(&mut t, b"names", &serde_json::to_vec(&n).unwrap_or_default()));
        match r {
            Ok(()) => s.commit(t).map_err(|e| e.to_string())?,
            Err(e) => {
                s.rollback(t);
                return Err(e.to_string());
            }
        }
        Ok(Some(files))
    })?;
    let Some(files) = files else { return Ok(false) };
    for f in files {
        let _ = std::fs::remove_file(body_path(key, &f));
    }
    Ok(true)
}

/// `cache.put(request, response)` (§5.4.3 batch operation "put"): `req` = `{url, method, headers}`, `resp` =
/// `{status, statusText, headers, type, url}`; the body goes to its own file.
pub fn put(key: &str, name: &str, req: &J, resp: &J, body: &[u8]) -> Result<(), String> {
    let (old_files, file) = with_store(key, |s| {
        if !names(s).iter().any(|x| x == name) {
            return Err("NotFoundError: the cache was deleted".to_string());
        }
        let t_name = tree(name);
        let o = QueryOptions::default();
        let mut stale = Vec::new();
        let mut old_files = Vec::new();
        let mut last = 0u64;
        for (k, v) in s.range(&t_name, Bound::Unbounded, Bound::Unbounded, false) {
            if k.len() == 8 {
                last = last.max(u64::from_be_bytes([k[0], k[1], k[2], k[3], k[4], k[5], k[6], k[7]]));
            }
            let Ok(e) = serde_json::from_slice::<J>(v) else { continue };
            // The request side of the match uses the NEW request against each cached entry.
            let probe = json!({ "url": req.get("url"), "method": "GET", "headers": req.get("headers") });
            if matches(&probe, &e, o) {
                stale.push(k.to_vec());
                if let Some(f) = e.get("body").and_then(J::as_str) {
                    old_files.push(f.to_string());
                }
            }
        }
        let seq = last + 1;
        let file = format!("b{seq}.body");
        let entry = json!({
            "url": strip(req.get("url").and_then(J::as_str).unwrap_or(""), false),
            "method": "GET",
            "reqHeaders": req.get("headers").cloned().unwrap_or(json!([])),
            "status": resp.get("status").cloned().unwrap_or(json!(200)),
            "statusText": resp.get("statusText").cloned().unwrap_or(json!("")),
            "headers": resp.get("headers").cloned().unwrap_or(json!([])),
            "type": resp.get("type").cloned().unwrap_or(json!("default")),
            "respUrl": resp.get("url").cloned().unwrap_or(json!("")),
            "body": file,
            "size": body.len(),
        });
        // The body is durable before the index names it.
        std::fs::write(body_path(key, &file), body).map_err(|e| e.to_string())?;
        let mut t = s.begin();
        let mut r = Ok(());
        for k in &stale {
            r = r.and_then(|_| s.del(&mut t, &t_name, k));
        }
        r = r.and_then(|_| s.put(&mut t, &t_name, &seq.to_be_bytes(), entry.to_string().as_bytes()));
        match r {
            Ok(()) => s.commit(t).map_err(|e| e.to_string())?,
            Err(e) => {
                s.rollback(t);
                let _ = std::fs::remove_file(body_path(key, &file));
                return Err(e.to_string());
            }
        }
        Ok((old_files, file))
    })?;
    let _ = file;
    for f in old_files {
        let _ = std::fs::remove_file(body_path(key, &f));
    }
    Ok(())
}

/// `match`/`matchAll` (one cache, or every cache in creation order when `name` is `None`): the matching
/// entries (JSON) and their bodies, in list order. `req` `None` = every entry (`matchAll()` with no request).
pub fn match_all(key: &str, name: Option<&str>, req: Option<&J>, o: QueryOptions, first_only: bool) -> Vec<(J, Vec<u8>)> {
    let found: Vec<J> = with_store(key, |s| {
        let list: Vec<String> = match name {
            Some(n) => names(s).into_iter().filter(|x| x == n).collect(),
            None => names(s),
        };
        let mut out = Vec::new();
        for n in list {
            for (_, v) in s.range(&tree(&n), Bound::Unbounded, Bound::Unbounded, false) {
                let Ok(e) = serde_json::from_slice::<J>(v) else { continue };
                if req.is_none_or(|r| matches(r, &e, o)) {
                    out.push(e);
                    if first_only {
                        return Ok(out);
                    }
                }
            }
        }
        Ok(out)
    })
    .unwrap_or_default();
    found
        .into_iter()
        .map(|e| {
            let body = e.get("body").and_then(J::as_str).and_then(|f| std::fs::read(body_path(key, f)).ok()).unwrap_or_default();
            (e, body)
        })
        .collect()
}

/// `cache.delete(request, options)` (§5.4.3 batch operation "delete"): answers whether anything was removed.
pub fn delete_entries(key: &str, name: &str, req: &J, o: QueryOptions) -> Result<bool, String> {
    let files = with_store(key, |s| {
        let t_name = tree(name);
        let mut hit = Vec::new();
        let mut files = Vec::new();
        for (k, v) in s.range(&t_name, Bound::Unbounded, Bound::Unbounded, false) {
            let Ok(e) = serde_json::from_slice::<J>(v) else { continue };
            if matches(req, &e, o) {
                hit.push(k.to_vec());
                if let Some(f) = e.get("body").and_then(J::as_str) {
                    files.push(f.to_string());
                }
            }
        }
        if hit.is_empty() {
            return Ok(Vec::new());
        }
        let mut t = s.begin();
        let mut r = Ok(());
        for k in &hit {
            r = r.and_then(|_| s.del(&mut t, &t_name, k));
        }
        match r {
            Ok(()) => s.commit(t).map_err(|e| e.to_string())?,
            Err(e) => {
                s.rollback(t);
                return Err(e.to_string());
            }
        }
        Ok(if files.is_empty() { vec![String::new()] } else { files })
    })?;
    let any = !files.is_empty();
    for f in files.into_iter().filter(|f| !f.is_empty()) {
        let _ = std::fs::remove_file(body_path(key, &f));
    }
    Ok(any)
}

/// `cache.keys(request?, options)`: the requests (`{url, method, headers}`) of the matching entries.
pub fn request_keys(key: &str, name: &str, req: Option<&J>, o: QueryOptions) -> Vec<J> {
    with_store(key, |s| {
        Ok(s.range(&tree(name), Bound::Unbounded, Bound::Unbounded, false)
            .into_iter()
            .filter_map(|(_, v)| serde_json::from_slice::<J>(v).ok())
            .filter(|e| req.is_none_or(|r| matches(r, e, o)))
            .map(|e| json!({ "url": e.get("url"), "method": e.get("method"), "headers": e.get("reqHeaders") }))
            .collect())
    })
    .unwrap_or_default()
}

/// Bytes the storage key's caches occupy on disk.
pub fn usage(key: &str) -> u64 {
    DirBacking::new(&bucket_dir("caches", key)).map(|b| b.usage()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vary_and_search_matching() {
        let entry = json!({ "url": "https://a.example/x?q=1", "headers": [["Vary", "Accept"]], "reqHeaders": [["accept", "text/html"]] });
        let req = |url: &str, accept: &str| json!({ "url": url, "method": "GET", "headers": [["accept", accept]] });
        let o = QueryOptions::default();
        assert!(matches(&req("https://a.example/x?q=1#frag", "text/html"), &entry, o));
        assert!(!matches(&req("https://a.example/x?q=1", "image/png"), &entry, o));
        assert!(matches(&req("https://a.example/x?q=1", "image/png"), &entry, QueryOptions { ignore_vary: true, ..o }));
        assert!(!matches(&req("https://a.example/x?q=2", "text/html"), &entry, o));
        assert!(matches(&req("https://a.example/x?q=2", "text/html"), &entry, QueryOptions { ignore_search: true, ..o }));
        let post = json!({ "url": "https://a.example/x?q=1", "method": "POST", "headers": [] });
        assert!(!matches(&post, &entry, QueryOptions { ignore_vary: true, ..o }));
    }
}
