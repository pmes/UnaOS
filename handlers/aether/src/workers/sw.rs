//! Service Workers (AETHERSTORAGE, LEDGER SR94; W3C Service Workers §3 model, §4 jobs, §6 fetch handling) —
//! the registrations, the lifecycle, which documents a worker controls, and the fetch interception hook.
//!
//! CHARTER: Aether — engine; `js::sw` is `navigator.serviceWorker`, `workers::realm` the worker's realm.
//!
//! # Registrations
//! Keyed by (storage key, scope URL) and kept in one `kvstore_core` file, `<root>/sw/registrations.kv`: tree
//! `reg` holds `{scope, scriptURL, updateViaCache}` and tree `script` the active worker's script text, so a
//! registration and its worker survive the process. A worker loaded from disk restarts on demand — the first
//! fetch or message it must handle — without running `install` or `activate` again (§3.1: an activated worker
//! stays activated across restarts).
//!
//! # Jobs (§4.2), run synchronously on the calling page's thread
//! - register / update: the script URL and the scope must be same-origin with the page, http(s), the scope
//!   within the script's directory unless the response's `Service-Worker-Allowed` widens it (§4.2.4 step 12);
//!   the script must be served as JavaScript. A byte-identical script at the same URL is no update. Otherwise a
//!   new worker thread evaluates it (an exception fails the job with a TypeError), runs `install` (a rejected
//!   `waitUntil` makes it redundant), and then activates — at once when there is no active worker, when the
//!   new one called `skipWaiting()`, or when no document is controlled by the active one; else it WAITS and
//!   is promoted when the last controlled document goes away ([`doc_gone`]).
//! - unregister: the registration is removed (its worker stops once no document uses it).
//!
//! # Control and interception (§6.4 "handle fetch")
//! A document is controlled by the registration whose scope is the longest prefix of its URL among those with
//! an active worker, decided ONCE, the first time the document asks (its first fetch, or
//! `navigator.serviceWorker.controller`), which approximates "at navigation"; `clients.claim()` takes every
//! in-scope uncontrolled document of the key and tells it (`controllerchange`). [`fetch_handle`] — called by
//! `api::fetch::spawn_http` on the page thread, for `fetch()` and asynchronous `XMLHttpRequest` — answers the
//! controlling worker's handle; on the request's own thread [`SwHandle::fetch`] dispatches a `fetch` event and
//! waits for its answer, `None` meaning "go to the network". [`intercept_navigation`] is the same question for a
//! top-level navigation; the navigation loader's call to it is owed (outside this arc's files).

use super::realm::{self, Cmd, Outcome, RealmCtx, SwResponse};
use kvstore_core::dir::DirBacking;
use kvstore_core::Store;
use serde_json::{json, Value as J};
use std::collections::HashMap;
use std::ops::Bound;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Mutex, OnceLock};

/// A running worker's command channel.
#[derive(Clone)]
pub struct SwHandle {
    tx: Sender<Cmd>,
}

impl SwHandle {
    fn event(&self, kind: &str, payload: String) -> Result<Outcome, String> {
        let (rtx, rrx) = channel();
        self.tx.send(Cmd::Event(kind.to_string(), payload, rtx)).map_err(|_| "the service worker stopped".to_string())?;
        rrx.recv_timeout(realm::EVENT_LIMIT + std::time::Duration::from_secs(1)).map_err(|_| "the service worker did not answer".to_string())?
    }

    /// §6.4 "handle fetch": a `fetch` event for the request; `None` = the network serves it.
    pub fn fetch(&self, method: &str, url: &str, headers: &[(String, String)], body: &[u8], client: u64) -> Option<SwResponse> {
        let mut hex = String::with_capacity(body.len() * 2);
        for b in body {
            hex.push_str(&format!("{b:02x}"));
        }
        let hl: Vec<J> = headers.iter().filter(|(k, _)| !k.starts_with(':')).map(|(k, v)| json!([k, v])).collect();
        let payload = json!({ "method": method, "url": url, "headers": hl, "body": hex, "clientId": client.to_string(), "mode": "cors" });
        match self.event("fetch", payload.to_string()) {
            Ok(o) => o.response,
            Err(e) => {
                crate::ledger::record_js(&format!("sw-fetch-fallback:{}", crate::js::clip(&e, 48)));
                None
            }
        }
    }

    fn stop(&self) {
        let _ = self.tx.send(Cmd::Stop);
    }
}

#[derive(Clone)]
struct Reg {
    key: String,
    scope: String,
    script_url: String,
    update_via_cache: String,
    /// The active worker's script (what a restart evaluates).
    script: String,
    active: Option<SwHandle>,
    /// A worker installed while the active one still controls documents.
    waiting: Option<(SwHandle, String, String)>,
}

#[derive(Default)]
struct World {
    loaded: bool,
    regs: Vec<Reg>,
    /// document serial → (storage key, its URL, the scope controlling it or None); decided once.
    clients: HashMap<u64, (String, String, Option<String>)>,
}

fn world() -> std::sync::MutexGuard<'static, World> {
    static W: OnceLock<Mutex<World>> = OnceLock::new();
    let mut w = W.get_or_init(|| Mutex::new(World::default())).lock().unwrap_or_else(|e| e.into_inner());
    if !w.loaded {
        w.loaded = true;
        w.regs = load_all();
    }
    w
}

fn db_dir() -> std::path::PathBuf {
    crate::storage::root_dir().join("sw")
}

fn with_db<T>(f: impl FnOnce(&mut Store<DirBacking>) -> T) -> Option<T> {
    let b = DirBacking::new(&db_dir()).ok()?;
    let mut s = Store::open(b, "registrations.kv").ok()?;
    Some(f(&mut s))
}

fn reg_key(key: &str, scope: &str) -> Vec<u8> {
    format!("{key}\n{scope}").into_bytes()
}

fn load_all() -> Vec<Reg> {
    with_db(|s| {
        let mut out = Vec::new();
        for (k, v) in s.range("reg", Bound::Unbounded, Bound::Unbounded, false) {
            let Ok(j) = serde_json::from_slice::<J>(v) else { continue };
            let text = String::from_utf8_lossy(k).into_owned();
            let Some((key, scope)) = text.split_once('\n') else { continue };
            let script = s.get("script", k).map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_default();
            out.push(Reg {
                key: key.to_string(),
                scope: scope.to_string(),
                script_url: j.get("scriptURL").and_then(J::as_str).unwrap_or("").to_string(),
                update_via_cache: j.get("updateViaCache").and_then(J::as_str).unwrap_or("imports").to_string(),
                script,
                active: None,
                waiting: None,
            });
        }
        out
    })
    .unwrap_or_default()
}

fn persist(r: &Reg) -> Result<(), String> {
    with_db(|s| {
        let mut t = s.begin();
        let k = reg_key(&r.key, &r.scope);
        let v = json!({ "scope": r.scope, "scriptURL": r.script_url, "updateViaCache": r.update_via_cache }).to_string();
        let res = s
            .create_tree(&mut t, "reg")
            .and_then(|_| s.create_tree(&mut t, "script"))
            .and_then(|_| s.put(&mut t, "reg", &k, v.as_bytes()))
            .and_then(|_| s.put(&mut t, "script", &k, r.script.as_bytes()));
        match res {
            Ok(()) => s.commit(t).map_err(|e| e.to_string()),
            Err(e) => {
                s.rollback(t);
                Err(e.to_string())
            }
        }
    })
    .unwrap_or_else(|| Err("the registration store could not be opened".into()))
}

fn forget(key: &str, scope: &str) {
    let _ = with_db(|s| {
        let k = reg_key(key, scope);
        let mut t = s.begin();
        let mut res = Ok(());
        if s.get("reg", &k).is_some() {
            res = res.and_then(|_| s.del(&mut t, "reg", &k));
        }
        if s.get("script", &k).is_some() {
            res = res.and_then(|_| s.del(&mut t, "script", &k));
        }
        match res {
            Ok(()) => {
                let _ = s.commit(t);
            }
            Err(_) => s.rollback(t),
        }
    });
}

/// A script's text (`importScripts`, the registration's fetch): http(s) through the policed client.
pub fn fetch_script(url: &str) -> Result<String, String> {
    fetch_script_full(url).map(|(t, _)| t)
}

/// The script and its response headers; refuses a non-2xx answer and a non-JavaScript MIME type (§4.2.4
/// step 9 "Update" → "fetch a classic worker script": the MIME type must be a JavaScript MIME type).
fn fetch_script_full(url: &str) -> Result<(String, Vec<(String, String)>), String> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(format!("the script URL scheme is not http(s): {url}"));
    }
    let r = crate::api::fetch::http_sync(crate::api::fetch::HttpReq {
        method: "GET".into(),
        url: url.to_string(),
        headers: vec![("Service-Worker".into(), "script".into())],
        body: Vec::new(),
        redirect: "error".into(),
        timeout_ms: 30_000,
    })?;
    if !(200..300).contains(&r.status) {
        return Err(format!("A bad HTTP response code ({}) was received when fetching the script.", r.status));
    }
    let headers: Vec<(String, String)> = r.headers.chunks(2).filter(|p| p.len() == 2).map(|p| (p[0].to_ascii_lowercase(), p[1].clone())).collect();
    let ct = headers.iter().find(|(k, _)| k == "content-type").map(|(_, v)| v.split(';').next().unwrap_or("").trim().to_ascii_lowercase()).unwrap_or_default();
    let js = matches!(ct.as_str(), "text/javascript" | "application/javascript" | "application/x-javascript" | "text/ecmascript" | "application/ecmascript");
    if !js {
        return Err(format!("The script has an unsupported MIME type ('{ct}')."));
    }
    Ok((String::from_utf8_lossy(&r.body).into_owned(), headers))
}

fn origin_of(url: &str) -> String {
    crate::security::origin::Origin::of(url).serialize()
}

/// The directory of a URL's path (`https://a/x/sw.js` → `https://a/x/`).
fn dir_of(url: &str) -> String {
    let bare = url.split(['?', '#']).next().unwrap_or(url);
    match bare.rfind('/') {
        Some(i) if i > bare.find("://").map(|j| j + 2).unwrap_or(0) => bare[..=i].to_string(),
        _ => format!("{bare}/"),
    }
}

fn reg_json(r: &Reg) -> J {
    json!({
        "scope": r.scope,
        "scriptURL": r.script_url,
        "updateViaCache": r.update_via_cache,
        "active": r.active.is_some() || !r.script.is_empty(),
        "waiting": r.waiting.as_ref().map(|w| w.1.clone()),
    })
}

fn controls(w: &World, key: &str, scope: &str) -> bool {
    w.clients.values().any(|(k, _, s)| k == key && s.as_deref() == Some(scope))
}

fn start_active(r: &mut Reg) -> Option<SwHandle> {
    if let Some(h) = &r.active {
        return Some(h.clone());
    }
    if r.script.is_empty() {
        return None;
    }
    let rc = RealmCtx { key: r.key.clone(), scope: r.scope.clone(), script_url: r.script_url.clone() };
    match realm::spawn(rc, r.script.clone()) {
        Ok(tx) => {
            let h = SwHandle { tx };
            r.active = Some(h.clone());
            Some(h)
        }
        Err(e) => {
            crate::ledger::record_js(&format!("sw-restart-failed:{}", crate::js::clip(&e, 48)));
            None
        }
    }
}

/// §4.2 Register (and Update when the scope exists): see the module docs. Answers the registration as JSON, or
/// `(DOMException name, message)`.
pub fn register(key: &str, page_url: &str, script_url: &str, scope: Option<&str>, update_via_cache: &str) -> Result<J, (&'static str, String)> {
    let page_origin = origin_of(page_url);
    let resolve = |u: &str| html_core::url::Url::parse(page_url).and_then(|b| b.join(u)).map(|u| u.to_string()).map_err(|_| ("TypeError", format!("The URL '{u}' is invalid.")));
    let script = resolve(script_url)?;
    if !(script.starts_with("http://") || script.starts_with("https://")) {
        return Err(("TypeError", format!("The URL protocol of the script ('{script}') is not supported.")));
    }
    if origin_of(&script) != page_origin {
        return Err(("SecurityError", format!("The origin of the provided scriptURL ('{}') does not match the current origin ('{page_origin}').", origin_of(&script))));
    }
    let scope = match scope {
        Some(s) => resolve(s)?,
        None => dir_of(&script),
    };
    let scope = scope.split('#').next().unwrap_or(&scope).to_string();
    if origin_of(&scope) != page_origin {
        return Err(("SecurityError", format!("The origin of the provided scope ('{}') does not match the current origin ('{page_origin}').", origin_of(&scope))));
    }
    let (text, headers) = fetch_script_full(&script).map_err(|e| ("TypeError", format!("Failed to register a ServiceWorker for scope ('{scope}') with script ('{script}'): {e}")))?;
    let max_scope = match headers.iter().find(|(k, _)| k == "service-worker-allowed") {
        Some((_, v)) => html_core::url::Url::parse(&script).and_then(|b| b.join(v)).map(|u| u.to_string()).unwrap_or_else(|_| dir_of(&script)),
        None => dir_of(&script),
    };
    let path_of = |u: &str| html_core::url::Url::parse(u).map(|u| u.pathname().to_string()).unwrap_or_default();
    if !path_of(&scope).starts_with(&path_of(&max_scope)) {
        return Err((
            "SecurityError",
            format!("The path of the provided scope ('{}') is not under the max scope allowed ('{}'). Adjust the scope, move the Service Worker script, or use the Service-Worker-Allowed HTTP header to allow the scope.", path_of(&scope), path_of(&max_scope)),
        ));
    }

    // Byte-identical script at the same URL: no new worker (§4.2.4 step 13).
    {
        let mut w = world();
        if let Some(r) = w.regs.iter_mut().find(|r| r.key == key && r.scope == scope) {
            r.update_via_cache = update_via_cache.to_string();
            if r.script_url == script && r.script == text {
                let out = reg_json(r);
                let snapshot = r.clone();
                drop(w);
                let _ = persist(&snapshot);
                return Ok(json!({ "registration": out, "installed": false }));
            }
        }
    }

    // Install a new worker (outside the registry lock: install may fetch).
    let rc = RealmCtx { key: key.to_string(), scope: scope.clone(), script_url: script.clone() };
    let tx = realm::spawn(rc, text.clone()).map_err(|e| ("TypeError", format!("ServiceWorker script evaluation failed: {e}")))?;
    let h = SwHandle { tx };
    let installed = h.event("install", "{}".into());
    let skip = match installed {
        Ok(o) => o.skip_waiting,
        Err(e) => {
            h.stop();
            crate::ledger::record_js("sw-install-rejected");
            return Err(("TypeError", format!("ServiceWorker installation failed: {e}")));
        }
    };

    let mut w = world();
    let busy = w.regs.iter().any(|r| r.key == key && r.scope == scope && (r.active.is_some() || !r.script.is_empty())) && controls(&w, key, &scope);
    let idx = match w.regs.iter().position(|r| r.key == key && r.scope == scope) {
        Some(i) => i,
        None => {
            w.regs.push(Reg {
                key: key.to_string(),
                scope: scope.clone(),
                script_url: script.clone(),
                update_via_cache: update_via_cache.to_string(),
                script: String::new(),
                active: None,
                waiting: None,
            });
            w.regs.len() - 1
        }
    };
    let activate_now = !busy || skip;
    let r = &mut w.regs[idx];
    if activate_now {
        if let Some(old) = r.active.take() {
            old.stop();
        }
        if let Some((old, _, _)) = r.waiting.take() {
            old.stop();
        }
        r.script_url = script.clone();
        r.script = text;
        r.active = Some(h.clone());
    } else {
        if let Some((old, _, _)) = r.waiting.take() {
            old.stop();
        }
        r.waiting = Some((h.clone(), script.clone(), text));
    }
    let snapshot = r.clone();
    let out = reg_json(r);
    // A controlled document keeps its worker until it goes; with skipWaiting the new one takes them over.
    let taken: Vec<u64> = if activate_now && busy {
        w.clients.iter().filter(|(_, (k, _, s))| k == key && s.as_deref() == Some(scope.as_str())).map(|(d, _)| *d).collect()
    } else {
        Vec::new()
    };
    drop(w);
    if activate_now {
        if let Err(e) = h.event("activate", "{}".into()) {
            crate::ledger::record_js(&format!("sw-activate-rejected:{}", crate::js::clip(&e, 40)));
        }
        let _ = persist(&snapshot);
        for d in taken {
            crate::js::broadcast::post(d, "sw-controllerchange", json!({ "scope": scope }).to_string());
        }
    }
    // A waiting worker is not the registration's script of record until it activates (nothing persisted).
    Ok(json!({ "registration": out, "installed": true, "activated": activate_now }))
}

/// §4.2 Update for an existing registration: re-runs register with its script URL and scope.
pub fn update(key: &str, page_url: &str, scope: &str) -> Result<J, (&'static str, String)> {
    let found = world().regs.iter().find(|r| r.key == key && r.scope == scope).map(|r| (r.script_url.clone(), r.update_via_cache.clone()));
    let Some((script, uvc)) = found else { return Err(("InvalidStateError", "The registration has been unregistered.".into())) };
    register(key, page_url, &script, Some(scope), &uvc)
}

/// §4.2 Unregister.
pub fn unregister(key: &str, scope: &str) -> bool {
    let removed = {
        let mut w = world();
        let i = w.regs.iter().position(|r| r.key == key && r.scope == scope);
        let busy = controls(&w, key, scope);
        i.map(|i| {
            let r = w.regs.remove(i);
            if !busy {
                if let Some(h) = &r.active {
                    h.stop();
                }
            }
            if let Some((h, _, _)) = &r.waiting {
                h.stop();
            }
        })
        .is_some()
    };
    if removed {
        forget(key, scope);
    }
    removed
}

/// §3.4 "match service worker registration": the longest scope that prefixes `url`.
fn matching(w: &World, key: &str, url: &str) -> Option<usize> {
    let mut best: Option<usize> = None;
    for (i, r) in w.regs.iter().enumerate() {
        if r.key == key && url.starts_with(&r.scope) && best.is_none_or(|b| w.regs[b].scope.len() < r.scope.len()) {
            best = Some(i);
        }
    }
    best
}

/// `getRegistration(clientURL)` / `getRegistrations()`.
pub fn get_registration(key: &str, url: &str) -> J {
    let w = world();
    matching(&w, key, url).map(|i| reg_json(&w.regs[i])).unwrap_or(J::Null)
}

pub fn registrations(key: &str) -> J {
    J::Array(world().regs.iter().filter(|r| r.key == key).map(reg_json).collect())
}

/// The scope controlling document `doc` (at `url`), deciding it on the first ask.
pub fn controller(doc: u64, key: &str, url: &str) -> Option<String> {
    let mut w = world();
    if let Some((_, u, s)) = w.clients.get_mut(&doc) {
        if *u != url {
            *u = url.to_string();
        }
        return s.clone();
    }
    let scope = matching(&w, key, url).filter(|&i| !w.regs[i].script.is_empty() || w.regs[i].active.is_some()).map(|i| w.regs[i].scope.clone());
    w.clients.insert(doc, (key.to_string(), url.to_string(), scope.clone()));
    scope
}

/// `navigator.serviceWorker.controller` for the running page: the controlling registration's JSON or null.
pub fn controller_json(doc: u64, key: &str, url: &str) -> J {
    match controller(doc, key, url) {
        Some(scope) => {
            let w = world();
            w.regs.iter().find(|r| r.key == key && r.scope == scope).map(reg_json).unwrap_or(J::Null)
        }
        None => J::Null,
    }
}

/// The hook `api::fetch::spawn_http` calls on the PAGE thread before a request leaves: the controlling
/// worker's handle (started on demand), or `None` when the page is not controlled.
pub fn fetch_handle(url: &str) -> Option<(SwHandle, u64)> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return None;
    }
    let key = crate::storage::page_storage_key()?;
    let doc = crate::js::broadcast::doc();
    let page = crate::js::page_url();
    let scope = controller(doc, &key, &page)?;
    let mut w = world();
    let r = w.regs.iter_mut().find(|r| r.key == key && r.scope == scope)?;
    start_active(r).map(|h| (h, doc))
}

/// A top-level navigation to `url` from a page of storage key `key`: the matching registration's worker
/// answers its `fetch` event (`None` = the network). The navigation loader's call is owed.
pub fn intercept_navigation(key: &str, url: &str) -> Option<SwResponse> {
    let h = {
        let mut w = world();
        let i = matching(&w, key, url)?;
        start_active(&mut w.regs[i])?
    };
    h.fetch("GET", url, &[("Accept".into(), "text/html".into())], &[], 0)
}

/// `clients.matchAll()` for a worker: the documents it controls (all documents of the key with
/// `includeUncontrolled`).
pub fn clients_of(key: &str, scope: &str, include_uncontrolled: bool) -> J {
    let w = world();
    J::Array(
        w.clients
            .iter()
            .filter(|(_, (k, _, s))| k == key && (include_uncontrolled || s.as_deref() == Some(scope)))
            .map(|(d, (_, u, _))| json!({ "id": d.to_string(), "url": u, "type": "window" }))
            .collect(),
    )
}

/// `clients.claim()`: every in-scope document of the key not controlled by this registration becomes
/// controlled by it and hears `controllerchange`.
pub fn claim(key: &str, scope: &str) {
    let changed: Vec<u64> = {
        let mut w = world();
        let mut out = Vec::new();
        for (d, (k, u, s)) in w.clients.iter_mut() {
            if k == key && u.starts_with(scope) && s.as_deref() != Some(scope) {
                *s = Some(scope.to_string());
                out.push(*d);
            }
        }
        out
    };
    for d in changed {
        crate::js::broadcast::post(d, "sw-controllerchange", json!({ "scope": scope }).to_string());
    }
}

/// `ServiceWorker.postMessage` from a page: a `message` event in the registration's active worker.
pub fn post_message(key: &str, scope: &str, data: &str, doc: u64, page_url: &str) -> bool {
    let h = {
        let mut w = world();
        let Some(r) = w.regs.iter_mut().find(|r| r.key == key && r.scope == scope) else { return false };
        start_active(r)
    };
    let Some(h) = h else { return false };
    let payload = json!({ "data": data, "origin": origin_of(page_url), "source": { "id": doc.to_string(), "url": page_url } }).to_string();
    // The event runs on the worker's thread; the page does not wait for it.
    std::thread::spawn(move || {
        let _ = h.event("message", payload);
    });
    true
}

/// A document went away: it is no longer a client; a waiting worker whose registration no longer controls
/// anything is promoted (activated), and an unregistered registration's worker stops.
pub fn doc_gone(doc: u64) {
    let promote: Vec<(SwHandle, Reg)> = {
        let mut w = world();
        if w.clients.remove(&doc).is_none() {
            return;
        }
        let mut out = Vec::new();
        let idle: Vec<usize> = (0..w.regs.len()).filter(|&i| w.regs[i].waiting.is_some() && !controls(&w, &w.regs[i].key, &w.regs[i].scope)).collect();
        for i in idle {
            let r = &mut w.regs[i];
            if let Some((h, url, text)) = r.waiting.take() {
                if let Some(old) = r.active.take() {
                    old.stop();
                }
                r.script_url = url;
                r.script = text;
                r.active = Some(h.clone());
                out.push((h, r.clone()));
            }
        }
        out
    };
    for (h, r) in promote {
        std::thread::spawn(move || {
            let _ = h.event("activate", "{}".into());
            let _ = persist(&r);
        });
    }
}

/// Bytes the storage key's registrations hold (their scripts).
pub fn usage(key: &str) -> u64 {
    world().regs.iter().filter(|r| r.key == key).map(|r| (r.script.len() + r.scope.len() + r.script_url.len()) as u64).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_directory() {
        assert_eq!(dir_of("https://a.example/x/sw.js?v=1"), "https://a.example/x/");
        assert_eq!(dir_of("https://a.example/sw.js"), "https://a.example/");
    }
}
