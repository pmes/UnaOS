// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Mica — shared-core
//!
//! UNAOSVOLUME (rmbp-ledger B427) — **the jobs are one aspect of the one UnaOS volume.** Peter, 2026-10-06:
//! "can this be a next level jobs queue done within UnaOS using it's handlers and UnaFS? … we could use the image
//! we write to our boot disk as the working drive so whenever UnaOS is ready to run on its own everything is
//! already in place".
//!
//! This is the ONE core (R79) both rings link: the record model, the `job:*` attribute names, the closed status
//! set, the parsers of the git text (STATUS.tsv, the ledgers, the queues), the EXPORT writers — byte-identical to
//! what `tools/status-check.py` reads — `verify` (a quoted line found byte-for-byte on a capture), the
//! `status=… flight=…` → UnaFS query translation, and the volume layout under [`ROOT`]. Mica (`handlers/mica`) is
//! the only writer; the kernel reads `/jobs` through the VFS. Design: `docs/dev/evidence/rmbp-1005/unaosvolume.md`.

#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// The volume folder every job lives under.
pub const ROOT: &str = "/jobs";
/// `/jobs/status/ST<n>` — one file per STATUS.tsv claim.
pub const STATUS_DIR: &str = "status";
/// `/jobs/ledger/<track>/<id>` — one file per ledger row.
pub const LEDGER_DIR: &str = "ledger";
/// `/jobs/queue/<track>/<nnn>-<NAME>` — one file per queue item.
pub const QUEUE_DIR: &str = "queue";
/// `/jobs/queries/<name>` — QUERYFOLDER's saved-query shape (B420): a file, typed [`QUERY_TYPE`], text = the query.
pub const QUERIES_DIR: &str = "queries";
/// QUERYFOLDER's saved-query type (`fs/query.rs` UNA_QUERY on exec-rmbp-queryfolder).
pub const QUERY_TYPE: &str = "application/x-vnd.una-query";
/// The type attribute key (`fs/filetype.rs` TYPE_KEY).
pub const TYPE_KEY: &str = "una:type";
/// ATTRCOLUMNS' folder view attribute (B402): the folder's chosen columns, comma-separated.
pub const VIEW_KEY: &str = "una:view";
/// The type a record's body carries (it is text).
pub const RECORD_TYPE: &str = "text/plain";
/// The saved queries the builder writes: (name, UnaFS query text).
pub const SAVED_QUERIES: &[(&str, &str)] = &[("Open jobs", "job:status == open")];

/// The `job:*` attribute names (typed: `job:seq` is an Int, every other one a String).
pub const K_KIND: &str = "job:kind";
pub const K_ID: &str = "job:id";
pub const K_SEQ: &str = "job:seq";
pub const K_STATUS: &str = "job:status";
pub const K_FLIGHT: &str = "job:flight";
pub const K_LINE: &str = "job:line";
pub const K_SET_BY: &str = "job:set_by";
pub const K_REFS: &str = "job:refs";
pub const K_OWNER: &str = "job:owner";
pub const K_ARC: &str = "job:arc";
pub const K_TRACK: &str = "job:track";
/// Every string key a record may carry, in the order a listing prints them.
pub const STRING_KEYS: &[&str] =
    &[K_KIND, K_ID, K_STATUS, K_FLIGHT, K_LINE, K_SET_BY, K_REFS, K_OWNER, K_ARC, K_TRACK];

/// The closed status set. The first five are STATUS.tsv's (`open confirmed refuted parked unflown`); the rest are
/// GATE-LEDGER's enum heads; `-` is a ledger cell with no enum head (the cell is kept verbatim in the body).
pub const STATUSES: &[&str] =
    &["open", "confirmed", "refuted", "parked", "unflown", "fixed-unflown", "flown", "landed", "dropped", "-"];
/// STATUS.tsv's own subset (status-check.py STATUSES).
pub const CLAIM_STATUSES: &[&str] = &["open", "confirmed", "refuted", "parked", "unflown"];
/// GATE-LEDGER's enum (status-check.py ENUM_HEAD).
pub const LEDGER_ENUM: &[&str] = &["open", "fixed-unflown", "flown", "landed", "dropped"];

/// STATUS.tsv's header (status-check.py HEADER).
pub const TSV_HEADER: &[&str] = &["id", "claim", "status", "flight", "line", "set-by", "row-refs"];

/// The git sources the volume is built from: (track, repo-relative path).
pub const LEDGERS: &[(&str, &str)] = &[
    ("rmbp", "docs/dev/OS/rmbp-ledger.md"),
    ("orin", "docs/dev/OS/orin-ledger.md"),
    ("pi", "docs/dev/OS/pi-ledger.md"),
    ("trunk", "docs/dev/LEDGER.md"),
];
pub const QUEUES: &[(&str, &str)] = &[
    ("rmbp", "docs/dev/OS/rmbp-queue.md"),
    ("orin", "docs/dev/OS/orin-queue.md"),
    ("pi", "docs/dev/OS/pi-queue.md"),
    ("trunk", "docs/dev/QUEUE.md"),
];
pub const STATUS_TSV: &str = "docs/dev/STATUS.tsv";

/// What a record is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Claim,
    Ledger,
    Queue,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Kind::Claim => "claim",
            Kind::Ledger => "ledger",
            Kind::Queue => "queue",
        }
    }
    pub fn from_word(w: &str) -> Option<Kind> {
        match w {
            "claim" => Some(Kind::Claim),
            "ledger" => Some(Kind::Ledger),
            "queue" => Some(Kind::Queue),
            _ => None,
        }
    }
}

/// One job record: a file on the volume. `attrs` are the `job:*` strings (absent = not carried — an empty cell is
/// NOT an attribute, so `job:flight` never exists for a claim that names none); `body` is the file's data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub kind: Kind,
    /// `ST<n>`, the ledger id, or `<nnn>-<NAME>` for a queue item.
    pub id: String,
    /// rmbp / orin / pi / trunk (claims: empty).
    pub track: String,
    /// The source order (STATUS.tsv row, ledger row, queue item), 1-based — `job:seq`, an Int attribute.
    pub seq: i64,
    pub attrs: Vec<(String, String)>,
    pub body: String,
}

impl Record {
    pub fn new(kind: Kind, id: &str, track: &str, seq: i64, body: String) -> Record {
        Record { kind, id: id.to_string(), track: track.to_string(), seq, attrs: Vec::new(), body }
    }
    pub fn get(&self, k: &str) -> &str {
        self.attrs.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str()).unwrap_or("")
    }
    /// Set (or, with an empty value, remove) one attribute.
    pub fn set(&mut self, k: &str, v: &str) {
        self.attrs.retain(|(a, _)| a != k);
        if !v.is_empty() {
            self.attrs.push((k.to_string(), v.to_string()));
        }
    }
    /// The record's path on the volume.
    pub fn path(&self) -> String {
        match self.kind {
            Kind::Claim => format!("{}/{}/{}", ROOT, STATUS_DIR, self.id),
            Kind::Ledger => format!("{}/{}/{}/{}", ROOT, LEDGER_DIR, self.track, self.id),
            Kind::Queue => format!("{}/{}/{}/{}", ROOT, QUEUE_DIR, self.track, self.id),
        }
    }
    /// The folder the record lives in.
    pub fn dir(&self) -> String {
        let p = self.path();
        match p.rfind('/') {
            Some(i) => p[..i].to_string(),
            None => String::from(ROOT),
        }
    }
    /// Every string attribute the volume carries for this record, with the identity keys first.
    pub fn volume_attrs(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        out.push((K_KIND.to_string(), self.kind.word().to_string()));
        out.push((K_ID.to_string(), self.id.clone()));
        if !self.track.is_empty() {
            out.push((K_TRACK.to_string(), self.track.clone()));
        }
        for (k, v) in &self.attrs {
            if k != K_KIND && k != K_ID && k != K_TRACK && !v.is_empty() {
                out.push((k.clone(), v.clone()));
            }
        }
        out
    }
}

/// Rebuild a record from what the volume holds (the reader's half of [`Record::volume_attrs`]).
pub fn record_from_volume(attrs: &[(String, String)], seq: i64, body: String) -> Option<Record> {
    let get = |k: &str| attrs.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str()).unwrap_or("");
    let kind = Kind::from_word(get(K_KIND))?;
    let mut r = Record::new(kind, get(K_ID), get(K_TRACK), seq, body);
    for (k, v) in attrs {
        if k != K_KIND && k != K_ID && k != K_TRACK && k.starts_with("job:") {
            r.set(k, v);
        }
    }
    Some(r)
}

// ── STATUS.tsv ──────────────────────────────────────────────────────────────────────────────────────

/// Parse STATUS.tsv into claim records. Refuses a header that is not [`TSV_HEADER`] or a row with the wrong cell
/// count (status-check.py's T1 shape) — the volume never holds what the gate would not read.
pub fn parse_status_tsv(text: &str) -> Result<Vec<Record>, String> {
    let mut lines: Vec<&str> = text.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    let head: Vec<&str> = lines.first().map(|l| l.split('\t').collect()).unwrap_or_default();
    if head != TSV_HEADER {
        return Err(String::from("STATUS.tsv:1: header is not id claim status flight line set-by row-refs"));
    }
    let mut out = Vec::new();
    for (i, ln) in lines.iter().enumerate().skip(1) {
        let c: Vec<&str> = ln.split('\t').collect();
        if c.len() != TSV_HEADER.len() {
            return Err(format!("STATUS.tsv:{}: {} cells, want {}", i + 1, c.len(), TSV_HEADER.len()));
        }
        let mut r = Record::new(Kind::Claim, c[0], "", i as i64, c[1].to_string());
        r.set(K_STATUS, c[2]);
        r.set(K_FLIGHT, c[3]);
        r.set(K_LINE, c[4]);
        r.set(K_SET_BY, c[5]);
        r.set(K_REFS, c[6]);
        out.push(r);
    }
    Ok(out)
}

/// The claims as STATUS.tsv — the volume's EXPORT, in `job:seq` order, header first, `\n` after every row.
pub fn export_status_tsv(records: &[Record]) -> String {
    let mut claims: Vec<&Record> = records.iter().filter(|r| r.kind == Kind::Claim).collect();
    claims.sort_by_key(|r| r.seq);
    let mut s = TSV_HEADER.join("\t");
    s.push('\n');
    for r in claims {
        let cells = [r.id.as_str(), r.body.as_str(), r.get(K_STATUS), r.get(K_FLIGHT), r.get(K_LINE), r.get(K_SET_BY), r.get(K_REFS)];
        s.push_str(&cells.join("\t"));
        s.push('\n');
    }
    s
}

/// The next free `ST<n>`.
pub fn next_claim_id(records: &[Record]) -> String {
    let n = records
        .iter()
        .filter(|r| r.kind == Kind::Claim)
        .filter_map(|r| r.id.strip_prefix("ST").and_then(|d| d.parse::<u64>().ok()))
        .max()
        .unwrap_or(0);
    format!("ST{}", n + 1)
}

// ── the ledgers ─────────────────────────────────────────────────────────────────────────────────────

/// A markdown table row's cells (status-check.py `split_row`: `\|` inside a cell is not a separator).
pub fn split_row(line: &str) -> Vec<String> {
    let mut body = line.trim();
    if let Some(b) = body.strip_prefix('|') {
        body = b;
    }
    if let Some(b) = body.strip_suffix('|') {
        body = b;
    }
    let mut cells = Vec::new();
    let mut cur = String::new();
    let mut prev = '\0';
    for ch in body.chars() {
        if ch == '|' && prev != '\\' {
            cells.push(cur.trim().to_string());
            cur.clear();
        } else {
            cur.push(ch);
        }
        prev = ch;
    }
    cells.push(cur.trim().to_string());
    cells
}

/// The enum head of a ledger status cell (status-check.py ENUM_HEAD: `^\W*(open|fixed-unflown|…)\b`), or `-`.
pub fn ledger_head(cell: &str) -> &'static str {
    let t = cell.trim_start_matches(|c: char| !(c.is_alphanumeric() || c == '_'));
    for w in ["fixed-unflown", "open", "flown", "landed", "dropped"] {
        let tb = t.as_bytes();
        if tb.len() >= w.len() && tb[..w.len()].eq_ignore_ascii_case(w.as_bytes()) {
            let next = t.get(w.len()..).and_then(|r| r.chars().next());
            if !next.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                return w;
            }
        }
    }
    "-"
}

/// Every `ST<n>` a text cites, comma-joined (status-check.py CITE).
pub fn cited(text: &str) -> String {
    let b = text.as_bytes();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i + 2 < b.len() {
        let word_start = i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_');
        if word_start && b[i] == b'S' && b[i + 1] == b'T' && b[i + 2].is_ascii_digit() {
            let mut j = i + 2;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            if j == b.len() || !(b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                let id = String::from(&text[i..j]);
                if !out.contains(&id) {
                    out.push(id);
                }
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out.join(",")
}

/// The first arc-like name in a text: an ASCII run of capitals/digits, three or more long, starting with a
/// capital, not one of the status words a queue line opens with.
pub fn arc_name(text: &str) -> String {
    const SKIP: &[&str] = &[
        "NEW", "FLOWN", "FIXED", "UNFLOWN", "DONE", "BUILT", "PASS", "FAIL", "THE", "STATE", "RESCUED", "DROPPED",
        "ALREADY", "IN", "TREE", "BLOCKED", "GRANT", "NEEDED", "BY", "DESIGN", "LANDED", "OPEN", "AND", "FOR", "NOT",
        "METAL", "FLIGHT", "CLOSED", "HELD", "OWED", "TODO", "WIP", "UTC",
    ];
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_uppercase() && (i == 0 || !b[i - 1].is_ascii_alphanumeric()) {
            let mut j = i;
            while j < b.len() && (b[j].is_ascii_uppercase() || b[j].is_ascii_digit()) {
                j += 1;
            }
            let word = &text[i..j];
            let ends = j == b.len() || !b[j].is_ascii_alphanumeric();
            if ends && word.len() >= 3 && !SKIP.contains(&word) {
                return word.to_string();
            }
            i = j.max(i + 1);
        } else {
            i += 1;
        }
    }
    String::new()
}

/// Is this line a table separator (`|---|---|`)?
fn is_separator(line: &str) -> bool {
    line.trim().chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

/// Parse one ledger file into records: every row of a table whose header names `id` and `status` (status-check.py
/// `ledger_cells`). The body is the row VERBATIM (so the export is the file itself); the status cell's enum head is
/// `job:status`, the `ST<n>` it cites `job:refs`. A repeated id (two tables in one file) gets `~<n>`.
pub fn parse_ledger(track: &str, md: &str) -> Vec<Record> {
    let mut out: Vec<Record> = Vec::new();
    let mut col: Option<(usize, Option<usize>)> = None;
    let mut seq = 0i64;
    for ln in md.split('\n') {
        if !ln.trim_start().starts_with('|') {
            col = None;
            continue;
        }
        let cells = split_row(ln);
        let low: Vec<String> = cells.iter().map(|c| c.to_ascii_lowercase()).collect();
        if low.iter().any(|c| c == "id") && low.iter().any(|c| c == "status") {
            let st = low.iter().position(|c| c == "status").unwrap_or(0);
            col = Some((st, low.iter().position(|c| c == "owner")));
            continue;
        }
        let Some((st, owner)) = col else { continue };
        if is_separator(ln) || cells.len() <= st {
            continue;
        }
        seq += 1;
        let base = cells[0].trim_matches(|c| c == '*' || c == ' ').to_string();
        let mut id = sanitize(&base);
        if id.is_empty() {
            id = format!("row{}", seq);
        }
        if out.iter().any(|r| r.id == id) {
            let mut n = 2;
            while out.iter().any(|r| r.id == format!("{}~{}", id, n)) {
                n += 1;
            }
            id = format!("{}~{}", id, n);
        }
        let mut r = Record::new(Kind::Ledger, &id, track, seq, ln.to_string());
        r.set(K_STATUS, ledger_head(&cells[st]));
        r.set(K_REFS, &cited(&cells[st]));
        if let Some(o) = owner.and_then(|o| cells.get(o)) {
            r.set(K_OWNER, o);
        }
        r.set(K_ARC, &arc_name(cells.get(1).map(String::as_str).unwrap_or("")));
        out.push(r);
    }
    out
}

/// The ledger file rebuilt from its records — the volume's EXPORT of the status cells. `template` is the file in
/// git (the prose between the tables is not a job); every ledger row is replaced by its record's body, in order.
/// A record that was not changed yields the row it came from, so an untouched volume exports the file byte for byte.
pub fn export_ledger(track: &str, template: &str, records: &[Record]) -> String {
    let mut rows: Vec<&Record> = records.iter().filter(|r| r.kind == Kind::Ledger && r.track == track).collect();
    rows.sort_by_key(|r| r.seq);
    let mut col: Option<usize> = None;
    let mut seq = 0usize;
    let mut out = String::with_capacity(template.len());
    let mut first = true;
    for ln in template.split('\n') {
        if !first {
            out.push('\n');
        }
        first = false;
        if !ln.trim_start().starts_with('|') {
            col = None;
            out.push_str(ln);
            continue;
        }
        let cells = split_row(ln);
        let low: Vec<String> = cells.iter().map(|c| c.to_ascii_lowercase()).collect();
        if low.iter().any(|c| c == "id") && low.iter().any(|c| c == "status") {
            col = low.iter().position(|c| c == "status");
            out.push_str(ln);
            continue;
        }
        let Some(st) = col else {
            out.push_str(ln);
            continue;
        };
        if is_separator(ln) || cells.len() <= st {
            out.push_str(ln);
            continue;
        }
        match rows.get(seq) {
            Some(r) => out.push_str(&r.body),
            None => out.push_str(ln),
        }
        seq += 1;
    }
    out
}

/// Replace a ledger row's status cell (the body's `status` column) — `cite` on a ledger record.
pub fn set_ledger_cell(header_cells: &[String], row: &str, cell: &str) -> Option<String> {
    let st = header_cells.iter().position(|c| c.eq_ignore_ascii_case("status"))?;
    let mut cells = split_row(row);
    if cells.len() <= st || cell.contains('|') {
        return None;
    }
    cells[st] = cell.to_string();
    Some(format!("| {} |", cells.join(" | ")))
}

// ── the queues ──────────────────────────────────────────────────────────────────────────────────────

/// Is this queue line an ITEM: an unindented line opening with the queue's bullet `·`, a `✓`, or a `⚠`?
fn queue_item(line: &str) -> bool {
    line.starts_with("· ") || line.starts_with("✓ ") || line.starts_with("⚠ ")
}

/// The status a queue item's words carry (its first status word), else `open`.
pub fn queue_status(line: &str) -> &'static str {
    let head: String = line.chars().take(96).collect::<String>().to_ascii_uppercase();
    for (w, s) in [
        ("FIXED-UNFLOWN", "fixed-unflown"),
        ("FIXED IN TREE (UNFLOWN)", "fixed-unflown"),
        ("DROPPED", "dropped"),
        ("FLOWN", "flown"),
        ("LANDED", "landed"),
        ("PARKED", "parked"),
    ] {
        if head.contains(w) {
            return s;
        }
    }
    "open"
}

/// Parse one queue file: every item line under a `## ` section is a job (`<nnn>-<NAME>`, body = the line).
pub fn parse_queue(track: &str, md: &str) -> Vec<Record> {
    let mut out: Vec<Record> = Vec::new();
    let mut in_section = false;
    for ln in md.split('\n') {
        if ln.starts_with("## ") {
            in_section = true;
            continue;
        }
        if ln.starts_with("# ") {
            in_section = false;
            continue;
        }
        if !in_section || !queue_item(ln) {
            continue;
        }
        let seq = out.len() as i64 + 1;
        let arc = arc_name(ln);
        let id = if arc.is_empty() { format!("{:03}", seq) } else { format!("{:03}-{}", seq, arc) };
        let mut r = Record::new(Kind::Queue, &id, track, seq, ln.to_string());
        r.set(K_STATUS, queue_status(ln));
        r.set(K_ARC, &arc);
        r.set(K_REFS, &cited(ln));
        out.push(r);
    }
    out
}

/// A file name the volume accepts: no `/`, no NUL, not `.`/`..`.
pub fn sanitize(s: &str) -> String {
    let t: String = s.chars().map(|c| if c == '/' || c == '\0' || c.is_control() { '_' } else { c }).collect();
    if t == "." || t == ".." { String::new() } else { t }
}

// ── verbs' pure halves ──────────────────────────────────────────────────────────────────────────────

/// Is `status` in the closed set?
pub fn status_ok(status: &str) -> bool {
    STATUSES.contains(&status)
}

/// `f<n>`?
pub fn flight_ok(f: &str) -> bool {
    f.len() > 1 && f.starts_with('f') && f[1..].bytes().all(|b| b.is_ascii_digit())
}

/// VERIFY: the quoted line found byte-for-byte in one of the flight's captures (status-check.py T3).
pub fn verify_line(line: &str, captures: &[&[u8]]) -> bool {
    let n = line.as_bytes();
    !n.is_empty() && captures.iter().any(|c| c.windows(n.len()).any(|w| w == n))
}

/// CITE a claim: status + flight + line, refused by the gate's rules (T2/T4) before a byte is written.
/// `verified` is whether [`verify_line`] found the line on the flight's capture (the caller reads the files).
pub fn cite(r: &mut Record, status: &str, flight: &str, line: &str, verified: bool, set_by: &str) -> Result<(), String> {
    if r.kind != Kind::Claim {
        return Err(format!("{}: cite takes a claim (ST<n>)", r.id));
    }
    if !CLAIM_STATUSES.contains(&status) {
        return Err(format!("status '{}' not in {}", status, CLAIM_STATUSES.join("/")));
    }
    if !flight.is_empty() && !flight_ok(flight) {
        return Err(format!("flight '{}' is not f<n> (the bench's number; never invented)", flight));
    }
    if (status == "confirmed" || status == "refuted") && (flight.is_empty() || line.is_empty()) {
        return Err(format!("{} requires a flight AND a quoted wire line", status));
    }
    if status == "unflown" && !flight.is_empty() {
        return Err(String::from("unflown carries no flight"));
    }
    if !line.is_empty() && !verified {
        return Err(format!("line NOT on the wire of {}", flight));
    }
    if line.contains('\t') || line.contains('\n') || set_by.contains('\t') {
        return Err(String::from("a cell carries no tab or newline"));
    }
    r.set(K_STATUS, status);
    r.set(K_FLIGHT, flight);
    r.set(K_LINE, line);
    if !set_by.is_empty() {
        r.set(K_SET_BY, set_by);
    }
    Ok(())
}

/// QUERY: `status=confirmed flight=f24 kind=claim` → the UnaFS query text (`job:status == "confirmed" AND …`).
/// A word that already reads as UnaFS syntax (it holds a space-separated operator) passes through untouched.
pub fn query_text(q: &str) -> Result<String, String> {
    let q = q.trim();
    if q.contains("==") || q.contains("!=") || q.contains(" ~ ") {
        return Ok(q.to_string());
    }
    let mut terms: Vec<String> = Vec::new();
    for w in q.split_whitespace() {
        let (k, v) = w.split_once('=').ok_or_else(|| format!("'{}' is not key=value", w))?;
        if k.is_empty() || v.is_empty() || v.contains('"') {
            return Err(format!("'{}' is not key=value", w));
        }
        let key = if k.starts_with("job:") { k.to_string() } else { format!("job:{}", k.replace('-', "_")) };
        terms.push(format!("{} == \"{}\"", key, v));
    }
    if terms.is_empty() {
        return Err(String::from("an empty query"));
    }
    Ok(terms.join(" AND "))
}

/// Counts for the witness line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Census {
    pub records: usize,
    pub claims: usize,
    pub ledger: usize,
    pub queue: usize,
}

pub fn census(records: &[Record]) -> Census {
    let mut c = Census { records: records.len(), ..Census::default() };
    for r in records {
        match r.kind {
            Kind::Claim => c.claims += 1,
            Kind::Ledger => c.ledger += 1,
            Kind::Queue => c.queue += 1,
        }
    }
    c
}

/// The host witness: `:: UNAOSVOLUME: records=<n> claims=<n> queue=<n> export=identical verify=<n>/<n> ::`.
pub fn witness(c: &Census, identical: bool, verified: usize, quoted: usize) -> String {
    format!(
        ":: UNAOSVOLUME: records={} claims={} queue={} export={} verify={}/{} ::",
        c.records,
        c.claims,
        c.queue,
        if identical { "identical" } else { "DIFFERS" },
        verified,
        quoted
    )
}
