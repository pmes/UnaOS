//! The Indexed Database API 3.0 engine (AETHERSTORAGE, LEDGER SR94) — the half under `js::idb`'s IDL objects.
//!
//! CHARTER: Aether — engine over the shared core `kvstore_core` (unaos/libs/fs).
//!
//! # Layout
//! One database = one `kvstore_core` file, `<root>/idb/<key_dir(storage key)>/<hex(name)>.kv` (§2.1: a
//! database belongs to a storage key and is named by a string). Inside it:
//!
//! - meta `version` — the database version (decimal); absent = 0, a database that does not exist yet;
//! - meta `schema` — the object stores and their indexes as JSON ([`Schema`]), each with a stable numeric id,
//!   so a rename (§4.5 `name` setter) is a schema edit, never a data move;
//! - meta `kg<id>` — store `<id>`'s key generator current number (§2.11);
//! - tree `s<id>` — store `<id>`'s records: `enc(primary key)` → [`Record`] (the index entries the record put,
//!   then the structured-serialized value);
//! - tree `i<id>` — index `<id>`'s entries: `enc(index key) ++ enc(primary key)` → `enc(primary key)`.
//!
//! `enc` is `kvstore_core::key` (order-preserving, prefix-free), so the trees' byte order is §2.6's key order
//! and an index tree is sorted by index key, then primary key — the order §2.8 cursors walk.
//!
//! # Connections and transactions
//! The registry ([`REG`]) is process-wide: every document of the process that opens `(storage key, name)`
//! shares one in-memory [`Db`]. It holds the connections (§2.2) and the transactions (§2.7) and answers the
//! scheduler question [`start`] asks (§2.7.2 "transaction scheduling": a read-only transaction waits for every
//! OLDER unfinished read/write transaction whose scope overlaps; a read/write one for every older overlapping
//! transaction), so no transaction ever reads another one's uncommitted writes. A read/write transaction holds
//! a `kvstore_core::Txn`: its writes apply at once (it reads them back) and are appended to the log as one
//! record by [`commit`], or undone by [`abort`]. The upgrade transaction (§2.7, `versionchange`) scopes every
//! store and owns the schema and version edits, which roll back with it.
//!
//! The wire to `js::idb` is JSON: one native, `__idb(op, json)`, answers JSON (`{"e": name, "m": message}` for
//! a DOMException). A key travels as `["n", number]`, `["d", ms]`, `["s", text]` (or `["u", [code units]]` when
//! the text has a lone surrogate), `["b", [bytes]]`, `["a", [keys]]`; ±Infinity as the strings `"Infinity"` /
//! `"-Infinity"`. Values are the binding's structured serialization (a JSON text) and are stored as bytes.

use super::bucket_dir;
use kvstore_core::dir::DirBacking;
use kvstore_core::key::{after_prefix, Key};
use kvstore_core::{Backing, Store, Txn};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as J};
use std::collections::{BTreeMap, HashMap};
use std::ops::Bound;
use std::sync::{Mutex, OnceLock};

/// §2.11: a key generator's current number never passes 2^53.
const KG_MAX: f64 = 9_007_199_254_740_992.0;

// ------------------------------------------------------------------------------------------------ errors

/// A DOMException the binding throws or reports on a request.
#[derive(Debug, Clone)]
pub struct DomErr {
    pub name: &'static str,
    pub message: String,
}

fn err<T>(name: &'static str, message: impl Into<String>) -> Result<T, DomErr> {
    Err(DomErr { name, message: message.into() })
}

impl DomErr {
    pub fn to_json(&self) -> J {
        json!({ "e": self.name, "m": self.message })
    }
}

type R<T> = Result<T, DomErr>;

// ------------------------------------------------------------------------------------------------ keys on the wire

/// A key from its wire form.
pub fn key_from_json(v: &J) -> R<Key> {
    let bad = || DomErr { name: "DataError", message: "The parameter is not a valid key.".into() };
    let a = v.as_array().ok_or_else(bad)?;
    let tag = a.first().and_then(J::as_str).ok_or_else(bad)?;
    let body = a.get(1).ok_or_else(bad)?;
    let number = |b: &J| -> R<f64> {
        match b {
            J::Number(n) => n.as_f64().ok_or_else(bad),
            J::String(s) if s == "Infinity" => Ok(f64::INFINITY),
            J::String(s) if s == "-Infinity" => Ok(f64::NEG_INFINITY),
            _ => Err(bad()),
        }
    };
    Ok(match tag {
        "n" => Key::Number(number(body)?),
        "d" => Key::Date(number(body)?),
        "s" => Key::string(body.as_str().ok_or_else(bad)?),
        "u" => Key::String(body.as_array().ok_or_else(bad)?.iter().map(|u| u.as_u64().unwrap_or(0) as u16).collect()),
        "b" => Key::Binary(body.as_array().ok_or_else(bad)?.iter().map(|u| u.as_u64().unwrap_or(0) as u8).collect()),
        "a" => Key::Array(body.as_array().ok_or_else(bad)?.iter().map(key_from_json).collect::<R<Vec<_>>>()?),
        _ => return Err(bad()),
    })
}

fn num_json(n: f64) -> J {
    if n.is_infinite() {
        J::String(if n > 0.0 { "Infinity".into() } else { "-Infinity".into() })
    } else {
        json!(n)
    }
}

/// A key's wire form.
pub fn key_to_json(k: &Key) -> J {
    match k {
        Key::Number(n) => json!(["n", num_json(*n)]),
        Key::Date(n) => json!(["d", num_json(*n)]),
        Key::String(u) => match String::from_utf16(u) {
            Ok(s) => json!(["s", s]),
            Err(_) => json!(["u", u]),
        },
        Key::Binary(b) => json!(["b", b]),
        Key::Array(a) => json!(["a", a.iter().map(key_to_json).collect::<Vec<_>>()]),
    }
}

/// An `IDBKeyRange` (or a single key, which is the range [key, key]) on the wire:
/// `{"lower": key|null, "upper": key|null, "lowerOpen": bool, "upperOpen": bool}`; `null` = unbounded.
#[derive(Debug, Clone, Default)]
pub struct KeyRange {
    pub lower: Option<Key>,
    pub upper: Option<Key>,
    pub lower_open: bool,
    pub upper_open: bool,
}

impl KeyRange {
    pub fn from_json(v: &J) -> R<KeyRange> {
        if v.is_null() {
            return Ok(KeyRange::default());
        }
        let opt = |k: &str| -> R<Option<Key>> {
            match v.get(k) {
                None | Some(J::Null) => Ok(None),
                Some(x) => key_from_json(x).map(Some),
            }
        };
        Ok(KeyRange {
            lower: opt("lower")?,
            upper: opt("upper")?,
            lower_open: v.get("lowerOpen").and_then(J::as_bool).unwrap_or(false),
            upper_open: v.get("upperOpen").and_then(J::as_bool).unwrap_or(false),
        })
    }

    /// Bounds over a record tree (keys are `enc(primary key)`).
    fn store_bounds(&self) -> (Bound<Vec<u8>>, Bound<Vec<u8>>) {
        let lo = match &self.lower {
            None => Bound::Unbounded,
            Some(k) if self.lower_open => Bound::Excluded(k.encode()),
            Some(k) => Bound::Included(k.encode()),
        };
        let hi = match &self.upper {
            None => Bound::Unbounded,
            Some(k) if self.upper_open => Bound::Excluded(k.encode()),
            Some(k) => Bound::Included(k.encode()),
        };
        (lo, hi)
    }

    /// Bounds over an index tree (keys are `enc(index key) ++ enc(primary key)`).
    fn index_bounds(&self) -> (Bound<Vec<u8>>, Bound<Vec<u8>>) {
        let lo = match &self.lower {
            None => Bound::Unbounded,
            Some(k) if self.lower_open => Bound::Included(after_prefix(&k.encode())),
            Some(k) => Bound::Included(k.encode()),
        };
        let hi = match &self.upper {
            None => Bound::Unbounded,
            Some(k) if self.upper_open => Bound::Excluded(k.encode()),
            Some(k) => Bound::Excluded(after_prefix(&k.encode())),
        };
        (lo, hi)
    }
}

fn as_ref(b: &Bound<Vec<u8>>) -> Bound<&[u8]> {
    match b {
        Bound::Included(v) => Bound::Included(v.as_slice()),
        Bound::Excluded(v) => Bound::Excluded(v.as_slice()),
        Bound::Unbounded => Bound::Unbounded,
    }
}

/// The tighter of two lower bounds (for equal bytes, Excluded is tighter).
fn max_lower(a: Bound<Vec<u8>>, b: Bound<Vec<u8>>) -> Bound<Vec<u8>> {
    match (&a, &b) {
        (Bound::Unbounded, _) => b,
        (_, Bound::Unbounded) => a,
        (Bound::Included(x) | Bound::Excluded(x), Bound::Included(y) | Bound::Excluded(y)) => {
            if x > y || (x == y && matches!(a, Bound::Excluded(_))) {
                a
            } else {
                b
            }
        }
    }
}

/// The tighter of two upper bounds.
fn min_upper(a: Bound<Vec<u8>>, b: Bound<Vec<u8>>) -> Bound<Vec<u8>> {
    match (&a, &b) {
        (Bound::Unbounded, _) => b,
        (_, Bound::Unbounded) => a,
        (Bound::Included(x) | Bound::Excluded(x), Bound::Included(y) | Bound::Excluded(y)) => {
            if x < y || (x == y && matches!(a, Bound::Excluded(_))) {
                a
            } else {
                b
            }
        }
    }
}

// ------------------------------------------------------------------------------------------------ schema

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IndexMeta {
    pub id: u64,
    /// A string or an array of strings (§2.5 key path).
    pub key_path: J,
    pub unique: bool,
    pub multi_entry: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StoreMeta {
    pub id: u64,
    /// `null` = out-of-line keys.
    pub key_path: J,
    pub auto_increment: bool,
    pub indexes: BTreeMap<String, IndexMeta>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Schema {
    pub next_id: u64,
    pub stores: BTreeMap<String, StoreMeta>,
}

impl Schema {
    fn to_wire(&self) -> J {
        let mut stores = serde_json::Map::new();
        for (n, s) in &self.stores {
            let mut idx = serde_json::Map::new();
            for (iname, i) in &s.indexes {
                idx.insert(iname.clone(), json!({ "keyPath": i.key_path, "unique": i.unique, "multiEntry": i.multi_entry }));
            }
            stores.insert(n.clone(), json!({ "keyPath": s.key_path, "autoIncrement": s.auto_increment, "indexes": idx }));
        }
        J::Object(stores)
    }
}

/// A record's stored bytes: the index entries it put (so an overwrite or a delete removes exactly those), then
/// the value: `count:u32le (index_id:u64le len:u32le enc(index key))* value`.
struct Record {
    entries: Vec<(u64, Vec<u8>)>,
    value: Vec<u8>,
}

impl Record {
    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.value.len() + 4 + self.entries.len() * 24);
        out.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for (id, k) in &self.entries {
            out.extend_from_slice(&id.to_le_bytes());
            out.extend_from_slice(&(k.len() as u32).to_le_bytes());
            out.extend_from_slice(k);
        }
        out.extend_from_slice(&self.value);
        out
    }

    fn decode(b: &[u8]) -> Record {
        let rd4 = |i: usize| b.get(i..i + 4).map(|x| u32::from_le_bytes([x[0], x[1], x[2], x[3]]) as usize);
        let mut entries = Vec::new();
        let Some(n) = rd4(0) else { return Record { entries, value: Vec::new() } };
        let mut i = 4;
        for _ in 0..n {
            let Some(id) = b.get(i..i + 8).map(|x| u64::from_le_bytes([x[0], x[1], x[2], x[3], x[4], x[5], x[6], x[7]])) else { break };
            let Some(l) = rd4(i + 8) else { break };
            let Some(k) = b.get(i + 12..i + 12 + l) else { break };
            entries.push((id, k.to_vec()));
            i += 12 + l;
        }
        Record { entries, value: b.get(i..).unwrap_or(&[]).to_vec() }
    }
}

// ------------------------------------------------------------------------------------------------ registry

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    ReadOnly,
    ReadWrite,
    VersionChange,
}

impl Mode {
    fn parse(s: &str) -> R<Mode> {
        match s {
            "readonly" => Ok(Mode::ReadOnly),
            "readwrite" => Ok(Mode::ReadWrite),
            _ => err("TypeError", format!("The provided value '{s}' is not a valid enum value of type IDBTransactionMode.")),
        }
    }
}

struct Conn {
    id: u64,
    version: u64,
    /// The document (`js::broadcast` inbox serial) that owns it.
    doc: u64,
    closed: bool,
}

struct TxnRec {
    id: u64,
    conn: u64,
    /// Store names (empty for the upgrade transaction: it scopes everything).
    scope: Vec<String>,
    mode: Mode,
    started: bool,
    kv: Option<Txn>,
    /// The version an upgrade transaction moves from (restored on abort).
    old_version: u64,
}

struct Db {
    store: Store<DirBacking>,
    conns: Vec<Conn>,
    /// Unfinished transactions, oldest first (creation order is the scheduler's order).
    txns: Vec<TxnRec>,
}

impl Db {
    fn version(&self) -> u64 {
        self.store.meta(b"version").and_then(|v| std::str::from_utf8(v).ok()).and_then(|v| v.parse().ok()).unwrap_or(0)
    }
    fn schema(&self) -> Schema {
        self.store.meta(b"schema").and_then(|v| serde_json::from_slice(v).ok()).unwrap_or_default()
    }
    fn open_conns(&self, except: u64) -> Vec<(u64, u64, u64)> {
        self.conns.iter().filter(|c| !c.closed && c.id != except).map(|c| (c.id, c.doc, c.version)).collect()
    }
    fn upgrading(&self) -> bool {
        self.txns.iter().any(|t| t.mode == Mode::VersionChange)
    }
}

#[derive(Default)]
struct Registry {
    dbs: HashMap<(String, String), Db>,
    /// conn id → its database.
    conns: HashMap<u64, (String, String)>,
    /// txn id → its database.
    txns: HashMap<u64, (String, String)>,
    next: u64,
}

fn reg() -> std::sync::MutexGuard<'static, Registry> {
    static REG: OnceLock<Mutex<Registry>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(Registry::default())).lock().unwrap_or_else(|e| e.into_inner())
}

fn file_name(name: &str) -> String {
    let mut s = String::with_capacity(name.len() * 2 + 3);
    for b in name.bytes() {
        s.push_str(&format!("{b:02x}"));
    }
    s.push_str(".kv");
    s
}

fn name_of_file(f: &str) -> Option<String> {
    let hex = f.strip_suffix(".kv")?;
    if hex.len() % 2 != 0 {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok()).collect();
    String::from_utf8(bytes?).ok()
}

fn io(e: kvstore_core::Error) -> DomErr {
    match e {
        kvstore_core::Error::Io(m) if m.contains("No space") || m.contains("quota") => DomErr { name: "QuotaExceededError", message: m },
        other => DomErr { name: "UnknownError", message: other.to_string() },
    }
}

fn load<'a>(r: &'a mut Registry, key: &str, name: &str) -> R<&'a mut Db> {
    let k = (key.to_string(), name.to_string());
    if !r.dbs.contains_key(&k) {
        let backing = DirBacking::new(&bucket_dir("idb", key)).map_err(io)?;
        let store = Store::open(backing, &file_name(name)).map_err(io)?;
        r.dbs.insert(k.clone(), Db { store, conns: Vec::new(), txns: Vec::new() });
    }
    Ok(r.dbs.get_mut(&k).expect("inserted"))
}

fn db_of_txn<'a>(r: &'a mut Registry, txn: u64) -> R<(&'a mut Db, usize)> {
    let Some(k) = r.txns.get(&txn).cloned() else { return err("TransactionInactiveError", "The transaction has finished.") };
    let db = r.dbs.get_mut(&k).ok_or_else(|| DomErr { name: "UnknownError", message: "database gone".into() })?;
    let i = db.txns.iter().position(|t| t.id == txn).ok_or_else(|| DomErr { name: "TransactionInactiveError", message: "The transaction has finished.".into() })?;
    Ok((db, i))
}

// ------------------------------------------------------------------------------------------------ factory

/// §4.3 `databases()`: the names and versions of the storage key's databases.
pub fn databases(key: &str) -> J {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    {
        let r = reg();
        for ((k, n), db) in &r.dbs {
            if k == key && db.version() > 0 {
                seen.insert(n.clone());
                out.push(json!({ "name": n, "version": db.version() }));
            }
        }
    }
    if let Ok(mut b) = DirBacking::new(&bucket_dir("idb", key)) {
        for f in b.list().unwrap_or_default() {
            let Some(n) = name_of_file(&f) else { continue };
            if seen.contains(&n) {
                continue;
            }
            if let Ok(s) = Store::open(b.clone(), &f) {
                let v: u64 = s.meta(b"version").and_then(|v| std::str::from_utf8(v).ok()).and_then(|v| v.parse().ok()).unwrap_or(0);
                if v > 0 {
                    out.push(json!({ "name": n, "version": v }));
                }
            }
        }
    }
    J::Array(out)
}

/// §5.1 "open a database connection", one attempt. Answers one of:
/// `{"conn", "old", "new", "schema"}` (opened, no upgrade), the same plus `"upgrade": txn` (an upgrade
/// transaction has begun: fire `upgradeneeded`), `{"blocked": n}` (n other connections were sent
/// `versionchange` and are still open: fire `blocked` once, retry later), `{"wait": true}` (another upgrade
/// is running: retry later).
pub fn open(key: &str, name: &str, version: Option<u64>, doc: u64, notify: bool) -> R<J> {
    let mut r = reg();
    r.next += 1;
    let id = r.next;
    let db = load(&mut r, key, name)?;
    let cur = db.version();
    if let Some(v) = version {
        if v < cur {
            return err("VersionError", format!("The requested version ({v}) is less than the existing version ({cur})."));
        }
    }
    if db.upgrading() {
        return Ok(json!({ "wait": true }));
    }
    let want = version.unwrap_or(cur.max(1));
    if want > cur {
        let others = db.open_conns(0);
        if !others.is_empty() {
            if notify {
                for (cid, d, v) in &others {
                    crate::js::broadcast::post(*d, "idb-versionchange", json!({ "conn": cid, "old": v, "new": want }).to_string());
                }
            }
            return Ok(json!({ "blocked": others.len(), "old": cur, "new": want }));
        }
        let mut kv = db.store.begin();
        db.store.set_meta(&mut kv, b"version", want.to_string().as_bytes()).map_err(io)?;
        if db.store.meta(b"schema").is_none() {
            let s = serde_json::to_vec(&Schema::default()).unwrap_or_default();
            db.store.set_meta(&mut kv, b"schema", &s).map_err(io)?;
        }
        db.conns.push(Conn { id, version: want, doc, closed: false });
        r.next += 1;
        let tid = r.next;
        let db = r.dbs.get_mut(&(key.to_string(), name.to_string())).expect("loaded");
        db.txns.push(TxnRec { id: tid, conn: id, scope: Vec::new(), mode: Mode::VersionChange, started: true, kv: Some(kv), old_version: cur });
        let schema = db.schema().to_wire();
        r.conns.insert(id, (key.to_string(), name.to_string()));
        r.txns.insert(tid, (key.to_string(), name.to_string()));
        return Ok(json!({ "conn": id, "old": cur, "new": want, "upgrade": tid, "schema": schema }));
    }
    db.conns.push(Conn { id, version: cur, doc, closed: false });
    let schema = db.schema().to_wire();
    r.conns.insert(id, (key.to_string(), name.to_string()));
    Ok(json!({ "conn": id, "old": cur, "new": cur, "schema": schema }))
}

/// §5.3 "delete a database", one attempt: `{"deleted": old version}`, `{"blocked": n}` or `{"wait": true}`.
pub fn delete_database(key: &str, name: &str, notify: bool) -> R<J> {
    let mut r = reg();
    let db = load(&mut r, key, name)?;
    if db.upgrading() {
        return Ok(json!({ "wait": true }));
    }
    let others = db.open_conns(0);
    if !others.is_empty() {
        if notify {
            for (cid, d, v) in &others {
                crate::js::broadcast::post(*d, "idb-versionchange", json!({ "conn": cid, "old": v, "new": J::Null }).to_string());
            }
        }
        return Ok(json!({ "blocked": others.len(), "old": db.version() }));
    }
    let old = db.version();
    let k = (key.to_string(), name.to_string());
    if let Some(db) = r.dbs.remove(&k) {
        let mut b = db.store.into_backing();
        Store::<DirBacking>::destroy(&mut b, &file_name(name)).map_err(io)?;
    }
    r.conns.retain(|_, v| *v != k);
    Ok(json!({ "deleted": old }))
}

/// §4.4 `close()`: the connection is closed once its transactions finish (the binding stops creating them).
pub fn close(conn: u64) {
    let mut r = reg();
    let Some(k) = r.conns.remove(&conn) else { return };
    if let Some(db) = r.dbs.get_mut(&k) {
        if let Some(c) = db.conns.iter_mut().find(|c| c.id == conn) {
            c.closed = true;
        }
        db.conns.retain(|c| !c.closed || db.txns.iter().any(|t| t.conn == c.id));
    }
}

/// A document went away: its connections close and its unfinished transactions abort.
pub fn doc_gone(doc: u64) {
    let (conns, txns): (Vec<u64>, Vec<u64>) = {
        let r = reg();
        let mut conns = Vec::new();
        let mut txns = Vec::new();
        for db in r.dbs.values() {
            for c in db.conns.iter().filter(|c| c.doc == doc) {
                conns.push(c.id);
                txns.extend(db.txns.iter().filter(|t| t.conn == c.id).map(|t| t.id));
            }
        }
        (conns, txns)
    };
    for t in txns {
        let _ = abort(t);
    }
    for c in conns {
        close(c);
    }
}

/// The schema as the connection sees it now (refreshed after an upgrade or an abort).
pub fn schema(conn: u64) -> R<J> {
    let r = reg();
    let Some(k) = r.conns.get(&conn) else { return err("InvalidStateError", "The database connection is closing.") };
    let db = r.dbs.get(k).ok_or_else(|| DomErr { name: "InvalidStateError", message: "database gone".into() })?;
    let v = db.conns.iter().find(|c| c.id == conn).map(|c| c.version).unwrap_or(0);
    Ok(json!({ "schema": db.schema().to_wire(), "version": v }))
}

// ------------------------------------------------------------------------------------------------ transactions

/// §4.4 `transaction()`: a new transaction (not yet started) over `stores`.
pub fn begin(conn: u64, stores: &[String], mode: &str) -> R<u64> {
    let mode = Mode::parse(mode)?;
    let mut r = reg();
    let Some(k) = r.conns.get(&conn).cloned() else { return err("InvalidStateError", "The database connection is closing.") };
    r.next += 1;
    let id = r.next;
    let db = r.dbs.get_mut(&k).ok_or_else(|| DomErr { name: "InvalidStateError", message: "database gone".into() })?;
    if db.txns.iter().any(|t| t.conn == conn && t.mode == Mode::VersionChange) {
        return err("InvalidStateError", "A version change transaction is running.");
    }
    if stores.is_empty() {
        return err("InvalidAccessError", "The transaction scope is empty.");
    }
    let schema = db.schema();
    for s in stores {
        if !schema.stores.contains_key(s) {
            return err("NotFoundError", format!("One of the specified object stores ('{s}') was not found."));
        }
    }
    let mut scope = stores.to_vec();
    scope.sort();
    scope.dedup();
    db.txns.push(TxnRec { id, conn, scope, mode, started: false, kv: None, old_version: 0 });
    r.txns.insert(id, k);
    Ok(id)
}

fn overlaps(a: &TxnRec, b: &TxnRec) -> bool {
    a.scope.is_empty() || b.scope.is_empty() || a.scope.iter().any(|s| b.scope.contains(s))
}

/// §2.7.2: starts the transaction when nothing older and overlapping forbids it. Answers whether it runs.
pub fn start(txn: u64) -> R<bool> {
    let mut r = reg();
    let (db, i) = db_of_txn(&mut r, txn)?;
    if db.txns[i].started {
        return Ok(true);
    }
    let me = &db.txns[i];
    let blocked = db.txns[..i].iter().any(|older| {
        overlaps(older, me) && (me.mode != Mode::ReadOnly || older.mode != Mode::ReadOnly)
    });
    if blocked {
        return Ok(false);
    }
    let kv = (me.mode != Mode::ReadOnly).then(|| db.store.begin());
    let t = &mut db.txns[i];
    t.started = true;
    t.kv = kv;
    Ok(true)
}

/// §5.4 "commit a transaction": the writes become one durable log record.
pub fn commit(txn: u64) -> R<J> {
    let mut r = reg();
    let (db, i) = db_of_txn(&mut r, txn)?;
    let t = db.txns.remove(i);
    let res = match t.kv {
        Some(kv) => db.store.commit(kv).map_err(io),
        None => Ok(()),
    };
    let conn = t.conn;
    let _ = conn;
    db.conns.retain(|c| !c.closed || db.txns.iter().any(|x| x.conn == c.id));
    r.txns.remove(&txn);
    res.map(|_| json!({ "ok": true }))
}

/// §5.5 "abort a transaction": every write undone; an upgrade's version and schema revert with it.
pub fn abort(txn: u64) -> R<J> {
    let mut r = reg();
    let (db, i) = db_of_txn(&mut r, txn)?;
    let t = db.txns.remove(i);
    if let Some(kv) = t.kv {
        db.store.rollback(kv);
    }
    if t.mode == Mode::VersionChange {
        // §5.8 "abort an upgrade transaction": the connection's version is the previous one; the connection
        // itself is closed by the binding (the open request fails with AbortError).
        if let Some(c) = db.conns.iter_mut().find(|c| c.id == t.conn) {
            c.version = t.old_version;
        }
    }
    r.txns.remove(&txn);
    Ok(json!({ "ok": true, "old": t.old_version }))
}

// ------------------------------------------------------------------------------------------------ data

/// The running transaction `txn` (started, read/write when `write`), its database, and the store's meta.
fn with_store<T>(txn: u64, store: &str, write: bool, f: impl FnOnce(&mut Db, usize, &StoreMeta) -> R<T>) -> R<T> {
    let mut r = reg();
    let (db, i) = db_of_txn(&mut r, txn)?;
    let t = &db.txns[i];
    if !t.started {
        return err("TransactionInactiveError", "The transaction has not started.");
    }
    if write && t.mode == Mode::ReadOnly {
        return err("ReadOnlyError", "The transaction is read-only.");
    }
    if !t.scope.is_empty() && !t.scope.iter().any(|s| s == store) {
        return err("NotFoundError", format!("The object store '{store}' is not in this transaction's scope."));
    }
    let schema = db.schema();
    let Some(meta) = schema.stores.get(store).cloned() else {
        return err("InvalidStateError", format!("The object store '{store}' has been deleted."));
    };
    f(db, i, &meta)
}

fn rec_tree(meta: &StoreMeta) -> String {
    format!("s{}", meta.id)
}

fn idx_tree(id: u64) -> String {
    format!("i{id}")
}

fn kg_key(meta: &StoreMeta) -> Vec<u8> {
    format!("kg{}", meta.id).into_bytes()
}

/// Runs `f` with the read/write transaction's kvstore txn taken out (and puts it back).
fn mutate<T>(db: &mut Db, i: usize, f: impl FnOnce(&mut Store<DirBacking>, &mut Txn) -> R<T>) -> R<T> {
    let mut kv = db.txns[i].kv.take().ok_or_else(|| DomErr { name: "ReadOnlyError", message: "The transaction is read-only.".into() })?;
    let res = f(&mut db.store, &mut kv);
    db.txns[i].kv = Some(kv);
    res
}

fn remove_record(store: &mut Store<DirBacking>, kv: &mut Txn, tree: &str, pk: &[u8]) -> R<bool> {
    let Some(old) = store.get(tree, pk).map(Record::decode) else { return Ok(false) };
    for (iid, ik) in &old.entries {
        let mut ek = ik.clone();
        ek.extend_from_slice(pk);
        let it = idx_tree(*iid);
        if store.has_tree(&it) {
            store.del(kv, &it, &ek).map_err(io)?;
        }
    }
    store.del(kv, tree, pk).map_err(io)?;
    Ok(true)
}

/// The next key the store's generator gives (§2.11 "generate a key"), consuming it. `ConstraintError` once
/// it passes 2^53.
pub fn generate_key(txn: u64, store: &str) -> R<J> {
    with_store(txn, store, true, |db, i, meta| {
        if !meta.auto_increment {
            return err("DataError", "The object store uses out-of-line keys and has no key generator, and the key parameter was not provided.");
        }
        let cur: f64 = db.store.meta(&kg_key(meta)).and_then(|v| std::str::from_utf8(v).ok()).and_then(|v| v.parse().ok()).unwrap_or(1.0);
        if cur > KG_MAX {
            return err("ConstraintError", "The key generator has reached its maximum value.");
        }
        let kgk = kg_key(meta);
        mutate(db, i, |s, kv| s.set_meta(kv, &kgk, (cur + 1.0).to_string().as_bytes()).map_err(io))?;
        Ok(key_to_json(&Key::Number(cur)))
    })
}

/// §5.6 "store a record into an object store". `index_keys` maps index name → the index keys the binding
/// extracted from the value (§2.5 key path evaluation happens in the binding, over the live JS value; an
/// index whose key path yields no valid key is absent). Answers the record's key.
pub fn put(txn: u64, store: &str, key: &Key, value: &[u8], index_keys: &BTreeMap<String, Vec<Key>>, no_overwrite: bool) -> R<J> {
    with_store(txn, store, true, |db, i, meta| {
        let tree = rec_tree(meta);
        let pk = key.encode();
        if no_overwrite && db.store.get(&tree, &pk).is_some() {
            return err("ConstraintError", "Key already exists in the object store.");
        }
        // §2.11 "possibly update the key generator".
        let mut kg_update = None;
        if meta.auto_increment {
            if let Key::Number(n) = key {
                let cur: f64 = db.store.meta(&kg_key(meta)).and_then(|v| std::str::from_utf8(v).ok()).and_then(|v| v.parse().ok()).unwrap_or(1.0);
                let next = (n.floor() + 1.0).min(KG_MAX + 1.0);
                if next > cur {
                    kg_update = Some(next);
                }
            }
        }
        let mut entries = Vec::new();
        for (iname, im) in &meta.indexes {
            let Some(keys) = index_keys.get(iname) else { continue };
            let mut seen = std::collections::HashSet::new();
            for ik in keys {
                let e = ik.encode();
                if !seen.insert(e.clone()) {
                    continue;
                }
                if im.unique {
                    let hi = after_prefix(&e);
                    let clash = db.store.range(&idx_tree(im.id), Bound::Included(&e), Bound::Excluded(&hi), false).into_iter().any(|(_, v)| v != pk.as_slice());
                    if clash {
                        return err("ConstraintError", format!("Unable to add key to index '{iname}': at least one key does not satisfy the uniqueness requirements."));
                    }
                }
                entries.push((im.id, e));
            }
        }
        let kgk = kg_key(meta);
        mutate(db, i, |s, kv| {
            remove_record(s, kv, &tree, &pk)?;
            for (iid, e) in &entries {
                let mut ek = e.clone();
                ek.extend_from_slice(&pk);
                s.put(kv, &idx_tree(*iid), &ek, &pk).map_err(io)?;
            }
            let rec = Record { entries, value: value.to_vec() }.encode();
            s.put(kv, &tree, &pk, &rec).map_err(io)?;
            if let Some(n) = kg_update {
                s.set_meta(kv, &kgk, n.to_string().as_bytes()).map_err(io)?;
            }
            Ok(())
        })?;
        Ok(key_to_json(key))
    })
}

fn record_json(pk: &[u8], rec: &[u8], with_value: bool) -> J {
    let k = Key::decode_all(pk).map(|k| key_to_json(&k)).unwrap_or(J::Null);
    if with_value {
        let v = Record::decode(rec).value;
        json!({ "key": k, "pk": k, "value": String::from_utf8_lossy(&v) })
    } else {
        json!({ "key": k, "pk": k })
    }
}

/// `get`/`getKey`/`getAll`/`getAllKeys`/`count` on an object store (§4.5), over `range`.
pub fn store_read(txn: u64, store: &str, op: &str, range: &KeyRange, count: Option<usize>) -> R<J> {
    with_store(txn, store, false, |db, _, meta| {
        let tree = rec_tree(meta);
        let (lo, hi) = range.store_bounds();
        match op {
            "get" | "getKey" => Ok(match db.store.first(&tree, as_ref(&lo), as_ref(&hi), false) {
                Some((k, v)) => record_json(k, v, op == "get"),
                None => J::Null,
            }),
            "getAll" | "getAllKeys" => {
                let n = count.filter(|c| *c > 0).unwrap_or(usize::MAX);
                let all = db.store.range(&tree, as_ref(&lo), as_ref(&hi), false);
                Ok(J::Array(all.into_iter().take(n).map(|(k, v)| record_json(k, v, op == "getAll")).collect()))
            }
            "count" => Ok(json!(db.store.range(&tree, as_ref(&lo), as_ref(&hi), false).len())),
            _ => err("TypeError", "unknown read"),
        }
    })
}

/// `delete(range)` and `clear()` on an object store (§5.7 "delete records from an object store", §5.9).
pub fn store_delete(txn: u64, store: &str, range: Option<&KeyRange>) -> R<J> {
    with_store(txn, store, true, |db, i, meta| {
        let tree = rec_tree(meta);
        let keys: Vec<Vec<u8>> = match range {
            Some(r) => {
                let (lo, hi) = r.store_bounds();
                db.store.range(&tree, as_ref(&lo), as_ref(&hi), false).into_iter().map(|(k, _)| k.to_vec()).collect()
            }
            None => Vec::new(),
        };
        let index_ids: Vec<u64> = meta.indexes.values().map(|m| m.id).collect();
        mutate(db, i, |s, kv| {
            match range {
                Some(_) => {
                    for k in &keys {
                        remove_record(s, kv, &tree, k)?;
                    }
                }
                None => {
                    s.clear(kv, &tree).map_err(io)?;
                    for id in &index_ids {
                        if s.has_tree(&idx_tree(*id)) {
                            s.clear(kv, &idx_tree(*id)).map_err(io)?;
                        }
                    }
                }
            }
            Ok(())
        })?;
        Ok(J::Null)
    })
}

/// `get`/`getKey`/`getAll`/`getAllKeys`/`count` on an index (§4.6): records by index key.
pub fn index_read(txn: u64, store: &str, index: &str, op: &str, range: &KeyRange, count: Option<usize>) -> R<J> {
    with_store(txn, store, false, |db, _, meta| {
        let Some(im) = meta.indexes.get(index) else { return err("InvalidStateError", format!("The index '{index}' has been deleted.")) };
        let tree = idx_tree(im.id);
        let rtree = rec_tree(meta);
        let (lo, hi) = range.index_bounds();
        let entry = |ek: &[u8], pk: &[u8], with_value: bool| -> J {
            let ik = Key::decode(ek).map(|(k, _)| key_to_json(&k)).unwrap_or(J::Null);
            let pkj = Key::decode_all(pk).map(|k| key_to_json(&k)).unwrap_or(J::Null);
            if with_value {
                let v = db.store.get(&rtree, pk).map(|r| Record::decode(r).value).unwrap_or_default();
                json!({ "key": ik, "pk": pkj, "value": String::from_utf8_lossy(&v) })
            } else {
                json!({ "key": ik, "pk": pkj })
            }
        };
        match op {
            "get" | "getKey" => Ok(match db.store.first(&tree, as_ref(&lo), as_ref(&hi), false) {
                Some((ek, pk)) => entry(ek, pk, op == "get"),
                None => J::Null,
            }),
            "getAll" | "getAllKeys" => {
                let n = count.filter(|c| *c > 0).unwrap_or(usize::MAX);
                let all = db.store.range(&tree, as_ref(&lo), as_ref(&hi), false);
                Ok(J::Array(all.into_iter().take(n).map(|(ek, pk)| entry(ek, pk, op == "getAll")).collect()))
            }
            "count" => Ok(json!(db.store.range(&tree, as_ref(&lo), as_ref(&hi), false).len())),
            _ => err("TypeError", "unknown read"),
        }
    })
}

/// §5.10 "iterate a cursor", one step, stateless: the entry after (`next*`) or before (`prev*`) the cursor's
/// position `pos` = (key, primary key), honouring `range`, the direction's uniqueness, `continue(key)`'s
/// `target` and `continuePrimaryKey`'s `target_pk`. Answers `{"key", "pk", "value"?}` or null.
#[allow(clippy::too_many_arguments)]
pub fn cursor_step(
    txn: u64,
    store: &str,
    index: Option<&str>,
    range: &KeyRange,
    dir: &str,
    pos: Option<(Key, Key)>,
    target: Option<Key>,
    target_pk: Option<Key>,
    with_value: bool,
) -> R<J> {
    with_store(txn, store, false, |db, _, meta| {
        let rev = dir.starts_with("prev");
        let unique = dir.ends_with("unique");
        let rtree = rec_tree(meta);
        match index {
            None => {
                let (mut lo, mut hi) = range.store_bounds();
                if let Some((k, _)) = &pos {
                    if rev {
                        hi = min_upper(hi, Bound::Excluded(k.encode()));
                    } else {
                        lo = max_lower(lo, Bound::Excluded(k.encode()));
                    }
                }
                if let Some(t) = &target {
                    if rev {
                        hi = min_upper(hi, Bound::Included(t.encode()));
                    } else {
                        lo = max_lower(lo, Bound::Included(t.encode()));
                    }
                }
                Ok(match db.store.first(&rtree, as_ref(&lo), as_ref(&hi), rev) {
                    Some((k, v)) => record_json(k, v, with_value),
                    None => J::Null,
                })
            }
            Some(iname) => {
                let Some(im) = meta.indexes.get(iname) else { return err("InvalidStateError", format!("The index '{iname}' has been deleted.")) };
                let tree = idx_tree(im.id);
                let (mut lo, mut hi) = range.index_bounds();
                if let Some((k, pk)) = &pos {
                    let ke = k.encode();
                    if rev {
                        hi = min_upper(hi, if unique { Bound::Excluded(ke) } else {
                            let mut e = ke;
                            pk.encode_into(&mut e);
                            Bound::Excluded(e)
                        });
                    } else {
                        lo = max_lower(lo, if unique { Bound::Included(after_prefix(&ke)) } else {
                            let mut e = ke;
                            pk.encode_into(&mut e);
                            Bound::Excluded(e)
                        });
                    }
                }
                if let Some(t) = &target {
                    let te = t.encode();
                    let b = match &target_pk {
                        Some(p) => {
                            let mut e = te;
                            p.encode_into(&mut e);
                            Bound::Included(e)
                        }
                        None if rev => Bound::Excluded(after_prefix(&te)),
                        None => Bound::Included(te),
                    };
                    if rev {
                        hi = min_upper(hi, b);
                    } else {
                        lo = max_lower(lo, b);
                    }
                }
                let Some((ek, pk)) = db.store.first(&tree, as_ref(&lo), as_ref(&hi), rev) else { return Ok(J::Null) };
                let (mut ek, mut pk) = (ek.to_vec(), pk.to_vec());
                if rev && unique {
                    // prevunique visits each index key at its FIRST record (lowest primary key).
                    if let Some((ik, _)) = Key::decode(&ek) {
                        let p = ik.encode();
                        let a = after_prefix(&p);
                        let lo2 = max_lower(lo.clone(), Bound::Included(p));
                        if let Some((e2, p2)) = db.store.first(&tree, as_ref(&lo2), Bound::Excluded(&a), false) {
                            ek = e2.to_vec();
                            pk = p2.to_vec();
                        }
                    }
                }
                let ik = Key::decode(&ek).map(|(k, _)| key_to_json(&k)).unwrap_or(J::Null);
                let pkj = Key::decode_all(&pk).map(|k| key_to_json(&k)).unwrap_or(J::Null);
                if with_value {
                    let v = db.store.get(&rtree, &pk).map(|r| Record::decode(r).value).unwrap_or_default();
                    Ok(json!({ "key": ik, "pk": pkj, "value": String::from_utf8_lossy(&v) }))
                } else {
                    Ok(json!({ "key": ik, "pk": pkj }))
                }
            }
        }
    })
}

// ------------------------------------------------------------------------------------------------ schema edits

/// Runs a schema edit inside the upgrade transaction `txn`.
fn with_upgrade<T>(txn: u64, f: impl FnOnce(&mut Db, usize, &mut Schema) -> R<T>) -> R<T> {
    let mut r = reg();
    let (db, i) = db_of_txn(&mut r, txn)?;
    if db.txns[i].mode != Mode::VersionChange {
        return err("InvalidStateError", "The database is not running a version change transaction.");
    }
    let mut schema = db.schema();
    let out = f(db, i, &mut schema)?;
    let bytes = serde_json::to_vec(&schema).unwrap_or_default();
    mutate(db, i, |s, kv| s.set_meta(kv, b"schema", &bytes).map_err(io))?;
    Ok(out)
}

/// §4.4 `createObjectStore(name, {keyPath, autoIncrement})` (the binding validated the key path).
pub fn create_store(txn: u64, name: &str, key_path: J, auto_increment: bool) -> R<J> {
    with_upgrade(txn, |db, i, schema| {
        if schema.stores.contains_key(name) {
            return err("ConstraintError", format!("An object store with the specified name ('{name}') already exists."));
        }
        schema.next_id += 1;
        let id = schema.next_id;
        let meta = StoreMeta { id, key_path, auto_increment, indexes: BTreeMap::new() };
        let tree = rec_tree(&meta);
        mutate(db, i, |s, kv| s.create_tree(kv, &tree).map_err(io))?;
        schema.stores.insert(name.to_string(), meta);
        Ok(J::Null)
    })
}

/// §4.4 `deleteObjectStore(name)`.
pub fn delete_store(txn: u64, name: &str) -> R<J> {
    with_upgrade(txn, |db, i, schema| {
        let Some(meta) = schema.stores.remove(name) else {
            return err("NotFoundError", format!("The object store '{name}' was not found."));
        };
        let tree = rec_tree(&meta);
        let kgk = kg_key(&meta);
        mutate(db, i, |s, kv| {
            s.drop_tree(kv, &tree).map_err(io)?;
            for im in meta.indexes.values() {
                if s.has_tree(&idx_tree(im.id)) {
                    s.drop_tree(kv, &idx_tree(im.id)).map_err(io)?;
                }
            }
            if s.meta(&kgk).is_some() {
                s.del_meta(kv, &kgk).map_err(io)?;
            }
            Ok(())
        })?;
        Ok(J::Null)
    })
}

/// §4.5 the `name` setter of an object store.
pub fn rename_store(txn: u64, from: &str, to: &str) -> R<J> {
    with_upgrade(txn, |_, _, schema| {
        if from == to {
            return Ok(J::Null);
        }
        if schema.stores.contains_key(to) {
            return err("ConstraintError", format!("An object store with the specified name ('{to}') already exists."));
        }
        let m = schema.stores.remove(from).ok_or_else(|| DomErr { name: "InvalidStateError", message: "The object store has been deleted.".into() })?;
        schema.stores.insert(to.to_string(), m);
        Ok(J::Null)
    })
}

/// §4.5 `createIndex(name, keyPath, {unique, multiEntry})`. `entries` are the index keys of every existing
/// record (primary key → keys), extracted by the binding; a uniqueness clash is a `ConstraintError`, which
/// aborts the upgrade transaction (§4.5 step 13).
pub fn create_index(txn: u64, store: &str, name: &str, key_path: J, unique: bool, multi_entry: bool, entries: &[(Key, Vec<Key>)]) -> R<J> {
    with_upgrade(txn, |db, i, schema| {
        let Some(sm) = schema.stores.get_mut(store) else { return err("InvalidStateError", "The object store has been deleted.") };
        if sm.indexes.contains_key(name) {
            return err("ConstraintError", format!("An index with the specified name ('{name}') already exists."));
        }
        let rtree = rec_tree(sm);
        schema.next_id += 1;
        let id = schema.next_id;
        let sm = schema.stores.get_mut(store).expect("checked");
        sm.indexes.insert(name.to_string(), IndexMeta { id, key_path, unique, multi_entry });
        let mut taken: HashMap<Vec<u8>, Vec<u8>> = HashMap::new();
        let mut plan: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        for (pk, keys) in entries {
            let pke = pk.encode();
            let mut seen = std::collections::HashSet::new();
            for ik in keys {
                let e = ik.encode();
                if !seen.insert(e.clone()) {
                    continue;
                }
                if unique {
                    if let Some(other) = taken.get(&e) {
                        if *other != pke {
                            return err("ConstraintError", format!("Unable to create index '{name}': at least one key does not satisfy the uniqueness requirements."));
                        }
                    }
                    taken.insert(e.clone(), pke.clone());
                }
                plan.push((e, pke.clone()));
            }
        }
        let tree = idx_tree(id);
        mutate(db, i, |s, kv| {
            s.create_tree(kv, &tree).map_err(io)?;
            for (e, pke) in &plan {
                let mut ek = e.clone();
                ek.extend_from_slice(pke);
                s.put(kv, &tree, &ek, pke).map_err(io)?;
                // The record remembers its entry, so an overwrite or a delete removes it.
                if let Some(old) = s.get(&rtree, pke).map(Record::decode) {
                    let mut rec = old;
                    rec.entries.push((id, e.clone()));
                    s.put(kv, &rtree, pke, &rec.encode()).map_err(io)?;
                }
            }
            Ok(())
        })?;
        Ok(J::Null)
    })
}

/// §4.5 `deleteIndex(name)`.
pub fn delete_index(txn: u64, store: &str, name: &str) -> R<J> {
    with_upgrade(txn, |db, i, schema| {
        let Some(sm) = schema.stores.get_mut(store) else { return err("InvalidStateError", "The object store has been deleted.") };
        let Some(im) = sm.indexes.remove(name) else { return err("NotFoundError", format!("The index '{name}' was not found.")) };
        let tree = idx_tree(im.id);
        mutate(db, i, |s, kv| if s.has_tree(&tree) { s.drop_tree(kv, &tree).map_err(io) } else { Ok(()) })?;
        Ok(J::Null)
    })
}

/// §4.6 the `name` setter of an index.
pub fn rename_index(txn: u64, store: &str, from: &str, to: &str) -> R<J> {
    with_upgrade(txn, |_, _, schema| {
        let Some(sm) = schema.stores.get_mut(store) else { return err("InvalidStateError", "The object store has been deleted.") };
        if from == to {
            return Ok(J::Null);
        }
        if sm.indexes.contains_key(to) {
            return err("ConstraintError", format!("An index with the specified name ('{to}') already exists."));
        }
        let m = sm.indexes.remove(from).ok_or_else(|| DomErr { name: "InvalidStateError", message: "The index has been deleted.".into() })?;
        sm.indexes.insert(to.to_string(), m);
        Ok(J::Null)
    })
}

/// Bytes the storage key's databases occupy on disk (the quota estimate's `indexedDB` usage).
pub fn usage(key: &str) -> u64 {
    DirBacking::new(&bucket_dir("idb", key)).map(|b| b.usage()).unwrap_or(0)
}

// ------------------------------------------------------------------------------------------------ the wire

fn s<'a>(v: &'a J, k: &str) -> &'a str {
    v.get(k).and_then(J::as_str).unwrap_or("")
}

fn u(v: &J, k: &str) -> u64 {
    v.get(k).and_then(J::as_u64).unwrap_or(0)
}

fn opt_key(v: &J, k: &str) -> R<Option<Key>> {
    match v.get(k) {
        None | Some(J::Null) => Ok(None),
        Some(x) => key_from_json(x).map(Some),
    }
}

fn keys_list(v: &J) -> R<Vec<Key>> {
    v.as_array().map(|a| a.iter().map(key_from_json).collect()).unwrap_or_else(|| Ok(Vec::new()))
}

/// `__idb(op, json)`: dispatches one binding call; answers JSON (`{"e","m"}` on a DOMException).
pub fn dispatch(key: &str, doc: u64, op: &str, a: &J) -> J {
    let res: R<J> = (|| -> R<J> { match op {
        "databases" => Ok(databases(key)),
        "open" => open(key, s(a, "name"), a.get("version").and_then(J::as_u64), doc, a.get("notify").and_then(J::as_bool).unwrap_or(true)),
        "deleteDatabase" => delete_database(key, s(a, "name"), a.get("notify").and_then(J::as_bool).unwrap_or(true)),
        "close" => {
            close(u(a, "conn"));
            Ok(J::Null)
        }
        "schema" => schema(u(a, "conn")),
        "begin" => {
            let stores: Vec<String> = a.get("stores").and_then(J::as_array).map(|v| v.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
            begin(u(a, "conn"), &stores, s(a, "mode")).map(|t| json!(t))
        }
        "start" => start(u(a, "txn")).map(|b| json!(b)),
        "commit" => commit(u(a, "txn")),
        "abort" => abort(u(a, "txn")),
        "genkey" => generate_key(u(a, "txn"), s(a, "store")),
        "put" => {
            let k = key_from_json(a.get("key").unwrap_or(&J::Null))?;
            let mut idx = BTreeMap::new();
            if let Some(m) = a.get("index").and_then(J::as_object) {
                for (n, ks) in m {
                    idx.insert(n.clone(), keys_list(ks)?);
                }
            }
            put(u(a, "txn"), s(a, "store"), &k, s(a, "value").as_bytes(), &idx, a.get("noOverwrite").and_then(J::as_bool).unwrap_or(false))
        }
        "read" => {
            let r = KeyRange::from_json(a.get("range").unwrap_or(&J::Null))?;
            let count = a.get("count").and_then(J::as_u64).map(|c| c as usize);
            match a.get("index").and_then(J::as_str) {
                Some(ix) => index_read(u(a, "txn"), s(a, "store"), ix, s(a, "what"), &r, count),
                None => store_read(u(a, "txn"), s(a, "store"), s(a, "what"), &r, count),
            }
        }
        "delete" => {
            let r = KeyRange::from_json(a.get("range").unwrap_or(&J::Null))?;
            store_delete(u(a, "txn"), s(a, "store"), Some(&r))
        }
        "clear" => store_delete(u(a, "txn"), s(a, "store"), None),
        "cursor" => {
            let r = KeyRange::from_json(a.get("range").unwrap_or(&J::Null))?;
            let pos = match (opt_key(a, "posKey")?, opt_key(a, "posPk")?) {
                (Some(k), Some(p)) => Some((k, p)),
                (Some(k), None) => Some((k.clone(), k)),
                _ => None,
            };
            cursor_step(
                u(a, "txn"),
                s(a, "store"),
                a.get("index").and_then(J::as_str),
                &r,
                s(a, "dir"),
                pos,
                opt_key(a, "target")?,
                opt_key(a, "targetPk")?,
                a.get("value").and_then(J::as_bool).unwrap_or(true),
            )
        }
        "createStore" => create_store(u(a, "txn"), s(a, "name"), a.get("keyPath").cloned().unwrap_or(J::Null), a.get("autoIncrement").and_then(J::as_bool).unwrap_or(false)),
        "deleteStore" => delete_store(u(a, "txn"), s(a, "name")),
        "renameStore" => rename_store(u(a, "txn"), s(a, "from"), s(a, "to")),
        "createIndex" => {
            let mut entries = Vec::new();
            if let Some(list) = a.get("entries").and_then(J::as_array) {
                for e in list {
                    let pk = key_from_json(e.get(0).unwrap_or(&J::Null))?;
                    entries.push((pk, keys_list(e.get(1).unwrap_or(&J::Null))?));
                }
            }
            create_index(
                u(a, "txn"),
                s(a, "store"),
                s(a, "name"),
                a.get("keyPath").cloned().unwrap_or(J::Null),
                a.get("unique").and_then(J::as_bool).unwrap_or(false),
                a.get("multiEntry").and_then(J::as_bool).unwrap_or(false),
                &entries,
            )
        }
        "deleteIndex" => delete_index(u(a, "txn"), s(a, "store"), s(a, "name")),
        "renameIndex" => rename_index(u(a, "txn"), s(a, "store"), s(a, "from"), s(a, "to")),
        "cmp" => {
            let x = key_from_json(a.get("a").unwrap_or(&J::Null))?;
            let y = key_from_json(a.get("b").unwrap_or(&J::Null))?;
            Ok(json!(x.compare(&y) as i8))
        }
        _ => err("TypeError", format!("__idb: unknown operation {op}")),
    } })();
    match res {
        Ok(v) => json!({ "ok": v }),
        Err(e) => e.to_json(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(n: f64) -> Key {
        Key::Number(n)
    }

    /// The engine half of the IndexedDB ordering KAT (`js::idb`'s test drives the page half): an upgrade
    /// creates a store and a unique index; a read/write transaction and a read-only one created after it over
    /// the same store start in creation order (the read-only one waits); an abort undoes its writes.
    #[test]
    fn scheduler_orders_overlapping_transactions() {
        let dir = std::env::temp_dir().join(format!("aether_idb_kat_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        crate::storage::set_root_dir(Some(dir.clone()));
        let key = "https://kat.example";
        let o = open(key, "db", Some(1), 0, false).unwrap();
        let conn = o["conn"].as_u64().unwrap();
        let up = o["upgrade"].as_u64().unwrap();
        create_store(up, "s", json!("id"), false).unwrap();
        create_index(up, "s", "by_name", json!("name"), true, false, &[]).unwrap();
        commit(up).unwrap();

        let rw = begin(conn, &["s".into()], "readwrite").unwrap();
        let ro = begin(conn, &["s".into()], "readonly").unwrap();
        assert!(start(rw).unwrap());
        assert!(!start(ro).unwrap(), "readonly waits for the older overlapping readwrite");
        let mut idx = BTreeMap::new();
        idx.insert("by_name".to_string(), vec![Key::string("a")]);
        put(rw, "s", &k(1.0), b"{\"v\":1}", &idx, true).unwrap();
        assert_eq!(put(rw, "s", &k(2.0), b"{}", &idx, true).unwrap_err().name, "ConstraintError");
        commit(rw).unwrap();
        assert!(start(ro).unwrap());
        let got = store_read(ro, "s", "get", &KeyRange { lower: Some(k(1.0)), upper: Some(k(1.0)), ..Default::default() }, None).unwrap();
        assert_eq!(got["value"], json!("{\"v\":1}"));
        commit(ro).unwrap();

        let rw2 = begin(conn, &["s".into()], "readwrite").unwrap();
        assert!(start(rw2).unwrap());
        store_delete(rw2, "s", None).unwrap();
        abort(rw2).unwrap();
        let ro2 = begin(conn, &["s".into()], "readonly").unwrap();
        assert!(start(ro2).unwrap());
        assert_eq!(store_read(ro2, "s", "count", &KeyRange::default(), None).unwrap(), json!(1));
        let c = cursor_step(ro2, "s", Some("by_name"), &KeyRange::default(), "next", None, None, None, false).unwrap();
        assert_eq!(c["pk"], key_to_json(&k(1.0)));
        commit(ro2).unwrap();
        close(conn);
        assert_eq!(databases(key), json!([{ "name": "db", "version": 1 }]));
        assert_eq!(delete_database(key, "db", false).unwrap()["deleted"], json!(1));
        crate::storage::set_root_dir(None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
