//! The Storage panel: the page's cookies, localStorage and sessionStorage, IndexedDB databases and Cache
//! API caches.
//!
//! Seam (AETHERSTORAGE SR94, AETHERORIGIN SR86): cookies are `net::cookie_jar()` — the one jar every
//! client shares — filtered to the ones whose domain and path match the page URL (HttpOnly ones included:
//! the panel is not script); the two Web Storage areas are `storage::with_area` under the page's storage
//! key (`storage::page_storage_key`, the key `js::net` uses for `localStorage`); IndexedDB is
//! `storage::idb::databases` (names and versions, open or on disk) and `storage::idb::usage`; the Cache
//! API is `storage::cache::cache_names`, `request_keys` and `usage`. An opaque-origin page has no storage
//! key, and the panel says so instead of reading another key's buckets.

use crate::AetherEngine;

/// One cookie row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CookieRow {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    /// Unix seconds; None for a session cookie.
    pub expires: Option<i64>,
    pub http_only: bool,
    pub secure: bool,
    /// `Strict`, `Lax`, `None`, or "" when unset.
    pub same_site: String,
    /// The CHIPS partition (the top-level site), when partitioned.
    pub partition: Option<String>,
}

/// One Cache API cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheRow {
    pub name: String,
    /// The stored requests as `METHOD url`.
    pub requests: Vec<String>,
}

/// Everything the panel shows for the page.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StorageView {
    /// The page's storage key; None for an opaque origin (no buckets).
    pub storage_key: Option<String>,
    pub cookies: Vec<CookieRow>,
    pub local: Vec<(String, String)>,
    pub session: Vec<(String, String)>,
    /// (database name, version).
    pub indexed_db: Vec<(String, u64)>,
    pub indexed_db_bytes: u64,
    pub caches: Vec<CacheRow>,
    pub cache_bytes: u64,
}

fn host_and_path(url: &str) -> Option<(String, String)> {
    let u = html_core::url::Url::parse(url).ok()?;
    Some((u.hostname().to_ascii_lowercase(), u.pathname()))
}

/// RFC 6265 §5.1.3 domain-match (host-only cookies match only their own host).
fn domain_matches(host: &str, domain: &str, host_only: bool) -> bool {
    let domain = domain.trim_start_matches('.').to_ascii_lowercase();
    host == domain || (!host_only && host.ends_with(&format!(".{domain}")))
}

/// RFC 6265 §5.1.4 path-match.
fn path_matches(path: &str, cookie_path: &str) -> bool {
    path == cookie_path
        || (path.starts_with(cookie_path) && (cookie_path.ends_with('/') || path[cookie_path.len()..].starts_with('/')))
}

/// The cookies the jar holds for `url` (expired ones left out).
pub fn cookies(url: &str) -> Vec<CookieRow> {
    let Some((host, path)) = host_and_path(url) else { return Vec::new() };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let jar = crate::net::cookie_jar();
    let Ok(j) = jar.lock() else { return Vec::new() };
    j.cookies()
        .iter()
        .filter(|c| domain_matches(&host, &c.domain, c.host_only) && path_matches(&path, &c.path))
        .filter(|c| !c.persistent || c.expiry > now)
        .map(|c| CookieRow {
            name: c.name.clone(),
            value: c.value.clone(),
            domain: c.domain.clone(),
            path: c.path.clone(),
            expires: c.persistent.then_some(c.expiry),
            http_only: c.http_only,
            secure: c.secure_only,
            same_site: c.same_site.as_ref().map(|s| format!("{s:?}")).unwrap_or_default(),
            partition: c.partition.clone(),
        })
        .collect()
}

fn area(local: bool, key: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    crate::storage::with_area(local, key, &mut |a| {
        out = a.keys().into_iter().filter_map(|k| a.get(&k).map(|v| (k.clone(), v.to_string()))).collect();
    });
    out
}

/// The panel's view of the running page.
pub fn view(engine: &AetherEngine) -> StorageView {
    let url = engine.current_url();
    let mut v = StorageView { cookies: cookies(&url), ..StorageView::default() };
    let Some(key) = crate::storage::page_storage_key() else { return v };
    v.local = area(true, &key);
    v.session = area(false, &key);
    if let serde_json::Value::Array(dbs) = crate::storage::idb::databases(&key) {
        v.indexed_db = dbs
            .iter()
            .filter_map(|d| Some((d.get("name")?.as_str()?.to_string(), d.get("version")?.as_u64()?)))
            .collect();
    }
    v.indexed_db_bytes = crate::storage::idb::usage(&key);
    v.caches = crate::storage::cache::cache_names(&key)
        .into_iter()
        .map(|name| {
            let requests = crate::storage::cache::request_keys(&key, &name, None, crate::storage::cache::QueryOptions::default())
                .iter()
                .map(|r| {
                    let method = r.get("method").and_then(|m| m.as_str()).unwrap_or("GET");
                    let url = r.get("url").and_then(|u| u.as_str()).unwrap_or("");
                    format!("{method} {url}")
                })
                .collect();
            CacheRow { name, requests }
        })
        .collect();
    v.cache_bytes = crate::storage::cache::usage(&key);
    v.storage_key = Some(key);
    v
}

impl StorageView {
    /// The view as the text face's lines.
    pub fn lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        out.push(format!("Cookies ({})", self.cookies.len()));
        for c in &self.cookies {
            let mut flags = Vec::new();
            if c.http_only {
                flags.push("HttpOnly".to_string());
            }
            if c.secure {
                flags.push("Secure".to_string());
            }
            if !c.same_site.is_empty() {
                flags.push(format!("SameSite={}", c.same_site));
            }
            if let Some(p) = &c.partition {
                flags.push(format!("Partitioned({p})"));
            }
            let exp = c.expires.map(|e| format!("expires {e}")).unwrap_or_else(|| "session".to_string());
            out.push(format!("  {}={}  {}{}  {}  {}", c.name, c.value, c.domain, c.path, exp, flags.join(" ")));
        }
        let Some(key) = &self.storage_key else {
            out.push("No storage: the page's origin is opaque.".to_string());
            return out;
        };
        out.push(format!("Storage key {key}"));
        out.push(format!("Local storage ({})", self.local.len()));
        out.extend(self.local.iter().map(|(k, v)| format!("  {k} = {v}")));
        out.push(format!("Session storage ({})", self.session.len()));
        out.extend(self.session.iter().map(|(k, v)| format!("  {k} = {v}")));
        out.push(format!("IndexedDB ({}, {} bytes)", self.indexed_db.len(), self.indexed_db_bytes));
        out.extend(self.indexed_db.iter().map(|(n, ver)| format!("  {n} v{ver}")));
        out.push(format!("Cache storage ({}, {} bytes)", self.caches.len(), self.cache_bytes));
        for c in &self.caches {
            out.push(format!("  {} ({})", c.name, c.requests.len()));
            out.extend(c.requests.iter().map(|r| format!("    {r}")));
        }
        out
    }
}
