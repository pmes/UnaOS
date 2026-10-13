//! The Web Locks lock manager (AETHERSTORAGE, LEDGER SR94; W3C Web Locks API §3 "lock manager").
//!
//! CHARTER: Aether — engine; `js::locks` is the page-facing `navigator.locks`.
//!
//! One manager for the process: every document of a storage key shares its lock names, so two tabs of one
//! site exclude each other the way the spec's per-storage-bucket lock manager makes them. A request is a
//! queue entry `(storage key, name, mode)`; §3.2 "a lock request is grantable" when no OLDER request of the
//! same name is still waiting and the held set allows it (exclusive: nothing held under the name; shared:
//! nothing exclusive held). `ifAvailable` answers `unavailable` instead of queueing; `steal` releases every
//! held lock of the name (their holders are told, their `request()` promises reject with "AbortError") and is
//! granted at once. A request granted after waiting is announced to its document through the
//! `js::broadcast` inbox (`lock-granted`), because the release that granted it may have happened in another
//! document's thread.

use serde_json::{json, Value as J};
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone)]
struct Req {
    id: u64,
    key: String,
    name: String,
    exclusive: bool,
    doc: u64,
    granted: bool,
}

#[derive(Default)]
struct Manager {
    /// Every request, held and waiting, in request order.
    reqs: Vec<Req>,
    next: u64,
}

fn mgr() -> std::sync::MutexGuard<'static, Manager> {
    static M: OnceLock<Mutex<Manager>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(Manager::default())).lock().unwrap_or_else(|e| e.into_inner())
}

impl Manager {
    fn grantable(&self, i: usize) -> bool {
        let r = &self.reqs[i];
        let same = |o: &&Req| o.key == r.key && o.name == r.name;
        if self.reqs[..i].iter().filter(same).any(|o| !o.granted) {
            return false;
        }
        let held = self.reqs.iter().filter(same).filter(|o| o.granted);
        if r.exclusive {
            held.count() == 0
        } else {
            !held.into_iter().any(|o| o.exclusive)
        }
    }

    /// Grants every waiting request that became grantable; answers (doc, id) of each.
    fn process(&mut self) -> Vec<(u64, u64)> {
        let mut out = Vec::new();
        for i in 0..self.reqs.len() {
            if !self.reqs[i].granted && self.grantable(i) {
                self.reqs[i].granted = true;
                out.push((self.reqs[i].doc, self.reqs[i].id));
            }
        }
        out
    }
}

fn announce(granted: Vec<(u64, u64)>) {
    for (doc, id) in granted {
        crate::js::broadcast::post(doc, "lock-granted", json!({ "id": id }).to_string());
    }
}

/// §4.1 `request(name, options, callback)`, the manager half. Answers `{"id", "state"}` with state
/// `granted`, `queued` or `unavailable`.
pub fn request(key: &str, name: &str, exclusive: bool, if_available: bool, steal: bool, doc: u64) -> J {
    let mut stolen = Vec::new();
    let (id, state, granted) = {
        let mut m = mgr();
        m.next += 1;
        let id = m.next;
        let req = Req { id, key: key.to_string(), name: name.to_string(), exclusive, doc, granted: false };
        if steal {
            m.reqs.retain(|o| {
                let hit = o.key == key && o.name == name && o.granted;
                if hit {
                    stolen.push((o.doc, o.id));
                }
                !hit
            });
            let mut r = req;
            r.granted = true;
            m.reqs.insert(0, r);
            (id, "granted", m.process())
        } else {
            m.reqs.push(req);
            let i = m.reqs.len() - 1;
            if m.grantable(i) {
                m.reqs[i].granted = true;
                (id, "granted", Vec::new())
            } else if if_available {
                m.reqs.pop();
                (id, "unavailable", Vec::new())
            } else {
                (id, "queued", Vec::new())
            }
        }
    };
    for (doc, sid) in stolen {
        crate::js::broadcast::post(doc, "lock-stolen", json!({ "id": sid }).to_string());
    }
    announce(granted.into_iter().filter(|(_, g)| *g != id).collect());
    json!({ "id": id, "state": state })
}

/// The lock's callback settled (or its holder went away): release it and grant what waits.
pub fn release(id: u64) {
    let granted = {
        let mut m = mgr();
        m.reqs.retain(|r| r.id != id);
        m.process()
    };
    announce(granted);
}

/// §4.1 step "signal aborted": a WAITING request leaves the queue. Answers whether it was waiting.
pub fn abort(id: u64) -> bool {
    let (was, granted) = {
        let mut m = mgr();
        let was = m.reqs.iter().any(|r| r.id == id && !r.granted);
        if was {
            m.reqs.retain(|r| r.id != id);
        }
        (was, m.process())
    };
    announce(granted);
    was
}

/// §4.2 `query()`: the storage key's held and pending locks (`clientId` is the owning document's serial).
pub fn query(key: &str) -> J {
    let m = mgr();
    let row = |r: &Req| json!({ "name": r.name, "mode": if r.exclusive { "exclusive" } else { "shared" }, "clientId": r.doc.to_string() });
    let held: Vec<J> = m.reqs.iter().filter(|r| r.key == key && r.granted).map(row).collect();
    let pending: Vec<J> = m.reqs.iter().filter(|r| r.key == key && !r.granted).map(row).collect();
    json!({ "held": held, "pending": pending })
}

/// A document went away: its held locks release, its waiting requests leave the queue.
pub fn doc_gone(doc: u64) {
    let granted = {
        let mut m = mgr();
        m.reqs.retain(|r| r.doc != doc);
        m.process()
    };
    announce(granted);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclusive_then_shared_queue_in_order() {
        let k = "https://locks.kat";
        let a = request(k, "n", true, false, false, 9001);
        assert_eq!(a["state"], "granted");
        let b = request(k, "n", false, false, false, 9001);
        assert_eq!(b["state"], "queued");
        let c = request(k, "n", false, true, false, 9001);
        assert_eq!(c["state"], "unavailable", "ifAvailable never queues");
        release(a["id"].as_u64().unwrap());
        let q = query(k);
        assert_eq!(q["held"].as_array().unwrap().len(), 1);
        assert_eq!(q["held"][0]["mode"], "shared");
        let d = request(k, "n", true, false, true, 9001);
        assert_eq!(d["state"], "granted", "steal is granted at once");
        doc_gone(9001);
        assert!(query(k)["held"].as_array().unwrap().is_empty());
    }
}
