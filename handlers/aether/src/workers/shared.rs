//! Shared workers (AETHERWORKERS3, LEDGER SR129; HTML §10.2.6.4 "The SharedWorker interface", §10.2.1.3
//! SharedWorkerGlobalScope, §10.2.3 "the worker's lifetime").
//!
//! CHARTER: Aether — engine (the page-facing `SharedWorker` object is `js::worker`'s second binding; the thread
//! and realm are `workers::dedicated`'s, run with `Spawn::shared`).
//!
//! # The registry
//! One process-wide list of running shared workers, each keyed by (storage key of the constructing document,
//! script URL, name) — HTML §10.2.6.4 step 11.4: "if there exists a SharedWorkerGlobalScope whose … constructor
//! origin / storage key, constructor URL equals urlRecord, and name equals the value of options's name member".
//! A document with an opaque origin keys its workers by its own serial (`opaque:<doc>`), so they match nothing
//! of any other document. A match whose script type differs is an error at the new SharedWorker object (step
//! 11.5); a match whose thread is already ending is replaced by a fresh worker.
//!
//! # Connecting
//! `new SharedWorker(url, options)` (page thread, `js::worker`) mints a MessageChannel: the outside port is the
//! object's `port`, the inside port is shipped (in transit, `js::ports`) and handed to [`connect`], which either
//! queues a [`Cmd::Connect`](super::dedicated::Cmd::Connect) to the matching worker or starts one — its script
//! fetched on its own thread under the document's request context (destination `sharedworker`), evaluated, and
//! then the queued `connect` runs: the realm adopts the inside port and fires `connect` (data "", the port as
//! `source` and `ports[0]`). A script fetch failure is a simple `error` event at EVERY SharedWorker object that
//! connected while the fetch ran ([`fetch_failed`], inbox kind `sharedworker-error`), and the entry goes, so a
//! later constructor retries.
//!
//! # Lifetime
//! A shared worker has no owning document (`Spawn::doc` 0): it outlives the document that started it. Its
//! clients are (document serial, SharedWorker object id, outside port id) triples; a client leaves when its
//! document goes ([`doc_gone`], from `dedicated::doc_gone`, which `js::broadcast::retire` calls) or when its
//! outside port closes ([`port_closed`], from `js::ports` — `port.close()`, the port's owner going, or the port
//! collected). The worker is terminated when its last client leaves — HTML §10.2.3's "owner set" emptied — and
//! leaves the registry when its thread ends ([`worker_gone`], also after its own `close()`).
//!
//! Lock order: this registry's lock is never held while calling into `dedicated` or `js::ports`.

use super::dedicated::{self, Kind, Spawn};
use std::sync::{Mutex, MutexGuard, OnceLock};

/// One client of a shared worker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Client {
    /// The document's inbox serial.
    doc: u64,
    /// The SharedWorker object's id in that document (its `sharedworker-error` address).
    object: u64,
    /// The outside port (the object's `port`).
    port: u64,
}

struct Entry {
    id: u64,
    key: String,
    url: String,
    name: String,
    kind: Kind,
    clients: Vec<Client>,
}

fn reg() -> MutexGuard<'static, Vec<Entry>> {
    static R: OnceLock<Mutex<Vec<Entry>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(Vec::new())).lock().unwrap_or_else(|e| e.into_inner())
}

/// What `new SharedWorker()` hands the engine (the page thread's facts, `js::worker`).
pub struct Connect {
    /// The registry key's storage part (the document's storage key, or `opaque:<doc>`).
    pub match_key: String,
    /// The storage key the worker's realm gets ("" for an opaque origin).
    pub storage_key: String,
    pub url: String,
    pub name: String,
    pub kind: Kind,
    pub doc: u64,
    pub object: u64,
    pub inside_port: u64,
    pub outside_port: u64,
    pub navigator: String,
    pub secure: bool,
    /// The document's request context, destination `sharedworker`.
    pub ctx: crate::security::RequestContext,
}

/// HTML §10.2.6.4 steps 11-12: connects a new SharedWorker object to the matching worker, or starts one. `Err`:
/// the message of the simple `error` event the object gets (a type mismatch, a thread that cannot start).
pub fn connect(c: Connect) -> Result<(), String> {
    let client = Client { doc: c.doc, object: c.object, port: c.outside_port };
    for _ in 0..2 {
        let found = {
            let mut r = reg();
            match r.iter_mut().find(|e| e.key == c.match_key && e.url == c.url && e.name == c.name) {
                Some(e) if e.kind != c.kind => {
                    return Err(format!(
                        "Failed to connect to the SharedWorker '{}' at '{}': it already runs as a {} worker.",
                        c.name,
                        c.url,
                        if e.kind == Kind::Module { "module" } else { "classic" }
                    ));
                }
                Some(e) => {
                    e.clients.push(client);
                    Some(e.id)
                }
                None => None,
            }
        };
        let Some(id) = found else { break };
        if dedicated::connect_port(id, c.inside_port) {
            return Ok(());
        }
        // The thread is ending (its own close(), a terminate in flight): that worker is no match any more.
        reg().retain(|e| e.id != id);
    }
    let id = dedicated::next_id();
    reg().push(Entry { id, key: c.match_key.clone(), url: c.url.clone(), name: c.name.clone(), kind: c.kind, clients: vec![client] });
    let sp = Spawn {
        id,
        doc: 0,
        owner: 0,
        parent: None,
        url: c.url.clone(),
        kind: c.kind,
        name: c.name.clone(),
        source: None,
        storage_key: c.storage_key,
        navigator: c.navigator,
        secure: c.secure,
        ctx: c.ctx,
        shared: true,
    };
    if let Err(e) = dedicated::spawn(sp) {
        reg().retain(|e| e.id != id);
        return Err(format!("Failed to start the shared worker: {e}"));
    }
    dedicated::connect_port(id, c.inside_port);
    Ok(())
}

/// Removes the clients `gone` matches; answers the workers left with none (to terminate, outside the lock).
fn drop_clients(gone: impl Fn(&Client) -> bool) -> Vec<u64> {
    let mut r = reg();
    let mut ended = Vec::new();
    for e in r.iter_mut() {
        let before = e.clients.len();
        e.clients.retain(|c| !gone(c));
        if before > 0 && e.clients.is_empty() {
            ended.push(e.id);
        }
    }
    r.retain(|e| !ended.contains(&e.id));
    ended
}

/// A document went away: it leaves every client list; a worker whose last client it was is terminated.
pub fn doc_gone(doc: u64) {
    for id in drop_clients(|c| c.doc == doc) {
        dedicated::terminate(id);
    }
}

/// A port closed (or was collected, or its owner went): if it was a client's outside port, that client
/// leaves; a worker whose last client it was is terminated.
pub fn port_closed(port: u64) {
    for id in drop_clients(|c| c.port == port) {
        dedicated::terminate(id);
    }
}

/// The worker's thread ended: it leaves the registry (a later constructor starts a new one).
pub fn worker_gone(id: u64) {
    reg().retain(|e| e.id != id);
}

/// The worker's script could not be fetched: every SharedWorker object connected to it gets a simple `error`
/// event (the console naming `message`), and the entry goes.
pub fn fetch_failed(id: u64, message: &str) {
    let clients: Vec<Client> = {
        let mut r = reg();
        let Some(i) = r.iter().position(|e| e.id == id) else { return };
        r.remove(i).clients
    };
    for c in clients {
        crate::js::broadcast::post(c.doc, "sharedworker-error", serde_json::json!({ "id": c.object, "message": message }).to_string());
    }
}

/// The shared workers `doc` is a client of (their ids): `dedicated::busy` counts them as the document's own.
pub fn serves(doc: u64) -> Vec<u64> {
    reg().iter().filter(|e| e.clients.iter().any(|c| c.doc == doc)).map(|e| e.id).collect()
}

/// Whether a shared worker for (`url`, `name`) runs (any storage key) — the lifetime the host tests observe.
pub fn is_running(url: &str, name: &str) -> bool {
    reg().iter().any(|e| e.url == url && e.name == name)
}

/// How many shared workers run.
pub fn live() -> usize {
    reg().len()
}
