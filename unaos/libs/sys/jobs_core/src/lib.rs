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
pub const TYPE_KEY: &str = una_abi::attr_keys::TYPE; // SMALLFIX4 item 13
/// ATTRCOLUMNS' folder view attribute (B402): the folder's chosen columns, comma-separated.
pub const VIEW_KEY: &str = una_abi::attr_keys::VIEW; // SMALLFIX4 item 13
/// The type a record's body carries (it is text).
pub const RECORD_TYPE: &str = "text/plain";
/// The saved queries the builder writes: (name, UnaFS query text). SMALLFIX4 item 13 (ATTRKEYS): the text is
/// built from the registered key, never a `job:` literal (GATE-ATTRKEYS).
pub fn saved_queries() -> Vec<(&'static str, String)> {
    alloc::vec![("Open jobs", format!("{} == open", K_STATUS))]
}

/// The `job:*` attribute names (typed: `job:seq` is an Int, every other one a String).
/// SMALLFIX4 item 13 (ATTRKEYS B452): aliases of `una_abi::attr_keys`, the one key registry.
pub const K_KIND: &str = una_abi::attr_keys::JOB_KIND;
pub const K_ID: &str = una_abi::attr_keys::JOB_ID;
pub const K_SEQ: &str = una_abi::attr_keys::JOB_SEQ;
pub const K_STATUS: &str = una_abi::attr_keys::JOB_STATUS;
pub const K_FLIGHT: &str = una_abi::attr_keys::JOB_FLIGHT;
pub const K_LINE: &str = una_abi::attr_keys::JOB_LINE;
pub const K_SET_BY: &str = una_abi::attr_keys::JOB_SET_BY;
pub const K_REFS: &str = una_abi::attr_keys::JOB_REFS;
pub const K_OWNER: &str = una_abi::attr_keys::JOB_OWNER;
pub const K_ARC: &str = una_abi::attr_keys::JOB_ARC;
pub const K_TRACK: &str = una_abi::attr_keys::JOB_TRACK;
pub const K_BRANCH: &str = una_abi::attr_keys::JOB_BRANCH; // JOBSNEXT (B506): the cut's branch
pub const K_TIP: &str = una_abi::attr_keys::JOB_TIP; // JOBSNEXT (B506): the hand-back's tip
/// The `job:` namespace, read off the registry (no literal): the part of a registered key up to its colon.
pub fn job_ns() -> &'static str {
    let i = K_KIND.find(':').map(|i| i + 1).unwrap_or(0);
    &K_KIND[..i]
}
/// Every string key a record may carry, in the order a listing prints them.
pub const STRING_KEYS: &[&str] =
    &[K_KIND, K_ID, K_STATUS, K_FLIGHT, K_LINE, K_SET_BY, K_REFS, K_OWNER, K_ARC, K_TRACK, K_BRANCH, K_TIP];

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
        if k != K_KIND && k != K_ID && k != K_TRACK && k.starts_with(job_ns()) {
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

// ── the Jobs view (JOBSUI, rmbp-ledger B511) ───────────────────────────────────────────────────────
// Peter (2026-10-07): "what happened to the new jobs queue?" — the queue on the volume is VISIBLE on the glass.
// One shape for every surface (R79: one core): the counts at the top, the queue's open items by rank below. Mica
// (`mica jobs view`) and the kernel (the `jobs` verb, Quarry's status line under `/jobs`) both render through these.

/// What every surface says while the jobs scan has not landed (JOBSCAN B497) — never zeros.
pub const PENDING: &str = "scanning…";
/// The open items a view shows below the counts.
pub const VIEW_ITEMS: usize = 10;

/// The glass spelling of a view line: the kernel faces' ASCII page has no ellipsis.
pub fn glass(s: &str) -> String {
    s.replace('…', "...")
}

/// The counts line: `jobs records=<n> claims=<n> ledger=<n> queue=<n>` (records, claims, ledger, queue), or
/// `jobs scanning…` while `None` (the scan is pending) — a pending store is never drawn as zeros.
pub fn counts_line(c: Option<[u64; 4]>) -> String {
    match c {
        Some([r, cl, l, q]) => format!("jobs records={} claims={} ledger={} queue={}", r, cl, l, q),
        None => format!("jobs {}", PENDING),
    }
}

/// Does `line` say the pending state the way [`counts_line`] does — the word, and no number at all?
pub fn says_pending(line: &str) -> bool {
    line.contains(PENDING.trim_end_matches('…')) && !line.bytes().any(|b| b.is_ascii_digit())
}

/// A queue file's rank (`job:seq`, the `<nnn>` its name opens with) and its shown name (the arc after the dash,
/// or `#<nnn>` when the item names none).
pub fn queue_name(file: &str) -> (i64, String) {
    let digits: String = file.chars().take_while(|c| c.is_ascii_digit()).collect();
    let seq = digits.parse::<i64>().unwrap_or(i64::MAX);
    let rest = file[digits.len()..].trim_start_matches('-');
    (seq, if rest.is_empty() { format!("#{}", digits) } else { rest.to_string() })
}

/// The queue folder's files in rank order (the queue's own `job:seq`; JOBSNEXT's ranking, B506, replaces it).
pub fn rank_order(names: &mut Vec<String>) {
    names.sort_by_key(|n| queue_name(n).0);
}

/// Open for the view: `job:status` is `open` and the item's own line does not open with the queue's done mark.
pub fn shown_open(status: &str, body: &str) -> bool {
    status == "open" && !body.starts_with('✓')
}

/// The flight that last touched an item: its `job:flight` when carried, else the last `flight <n>` / `FLIGHT<n>` /
/// `f<n>` its words name (as `f<n>`), else `-`.
pub fn last_flight(flight_attr: &str, body: &str) -> String {
    if !flight_attr.is_empty() {
        return flight_attr.to_string();
    }
    let b = body.as_bytes();
    let mut last: Option<&str> = None;
    let mut i = 0;
    while i < b.len() {
        let word_start = i == 0 || !b[i - 1].is_ascii_alphanumeric();
        let skip = if !word_start {
            0
        } else if b.len() - i > 6 && b[i..i + 6].eq_ignore_ascii_case(b"flight") {
            if b.get(i + 6) == Some(&b' ') { 7 } else { 6 }
        } else if b[i] == b'f' {
            1
        } else {
            0
        };
        if skip > 0 {
            let s = i + skip;
            let mut e = s;
            while e < b.len() && b[e].is_ascii_digit() {
                e += 1;
            }
            if e > s && (e == b.len() || !b[e].is_ascii_alphanumeric()) {
                last = Some(&body[s..e]);
                i = e;
                continue;
            }
        }
        i += 1;
    }
    match last {
        Some(n) => format!("f{}", n),
        None => String::from("-"),
    }
}

/// One item row: `<rank>. <name> <track> <status> <flight>`.
pub fn item_row(rank: usize, name: &str, track: &str, status: &str, flight: &str) -> String {
    let short: String = name.chars().take(22).collect();
    format!("{:>2}. {:<22} {:<5} {:<6} {}", rank, short, track, status, flight)
}

// ── verbs' pure halves ──────────────────────────────────────────────────────────────────────────────

/// Is `status` in the closed set?
pub fn status_ok(status: &str) -> bool {
    STATUSES.contains(&status)
}

/// `f<n>`?
pub fn flight_ok(f: &str) -> bool {
    f.len() > 1 && (f.starts_with('f') || f.starts_with('r')) && f[1..].bytes().all(|b| b.is_ascii_digit()) // `f<n>` rMBP, `r<n>` Orin render n (STATUSORIN B476, status-check.py)
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
        let key = if k.starts_with(job_ns()) { k.to_string() } else { format!("{}{}", job_ns(), k.replace('-', "_")) };
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

// ── JOBSNEXT (rmbp-ledger B506): the next wave, ranked — ONE function both rings call ───────────────────────────
//
// Peter, 2026-10-07: "what happened to the new jobs queue? why do i have to tell you that there's more to do after
// every single boot". Mica's `jobs next` and the kernel shell's `jobs next` both call [`rank_next`]; the §3 order
// they rank by is [`owed_names`] of the latest FLIGHT<n>.md, kept on the volume at [`OWED_FILE`] for the kernel.

/// `/jobs/owed` — the latest flight's §3 item names, one per line, in Peter's order; `job:flight` = `f<n>`.
pub const OWED_FILE: &str = "owed";

/// The item HEADS of a FLIGHT<n>.md `## 3.` section, in the author's order, each once. Items are split at a
/// top-level `,` `;` `+` or sentence `.` (never inside `( )` or backticks); an item's head is its first CAPS word
/// outside them ([`arc_name`]). An item with no CAPS head ("the play verb") is not a name.
pub fn owed_names(md: &str) -> Vec<String> {
    let mut sec = String::new();
    let mut on = false;
    for ln in md.split('\n') {
        if ln.starts_with("## ") {
            let t = ln[3..].trim_start();
            on = t.starts_with("3.") || t.starts_with("3 ");
            continue;
        }
        if on {
            sec.push_str(ln);
            sec.push('\n');
        }
    }
    let mut out: Vec<String> = Vec::new();
    let mut item = String::new();
    let (mut depth, mut tick) = (0i32, false);
    let chars: Vec<char> = sec.chars().collect();
    let flush = |item: &mut String, out: &mut Vec<String>| {
        let n = arc_name(item);
        if !n.is_empty() && !out.contains(&n) {
            out.push(n);
        }
        item.clear();
    };
    for (i, &c) in chars.iter().enumerate() {
        match c {
            '`' => tick = !tick,
            '(' | '[' if !tick => depth += 1,
            ')' | ']' if !tick => depth = (depth - 1).max(0),
            _ => {}
        }
        if depth > 0 || tick || c == '`' || c == ')' || c == ']' {
            // a parenthetical / code span is the item's gloss, never its head: replaced by a space
            item.push(' ');
            continue;
        }
        let end = chars.get(i + 1).is_none_or(|n| n.is_whitespace());
        if c == ',' || c == ';' || c == '+' || c == '\n' && chars.get(i + 1) == Some(&'\n') || (c == '.' && end) {
            flush(&mut item, &mut out);
        } else {
            item.push(c);
        }
    }
    flush(&mut item, &mut out);
    out
}

/// Words that close a queue item or a ledger row for the next wave (its first 96 characters, whole words).
const CLOSED_WORDS: &[&str] = &["DONE", "BUILT", "FIXED", "FIXED-UNFLOWN", "FLOWN", "LANDED", "DROPPED", "RESCUED", "CLOSED", "PARKED"];

fn has_word(hay: &str, w: &str) -> bool {
    let hb = hay.as_bytes();
    let wb = w.as_bytes();
    let wordc = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut i = 0;
    while i + wb.len() <= hb.len() {
        if &hb[i..i + wb.len()] == wb
            && (i == 0 || !wordc(hb[i - 1]))
            && (i + wb.len() == hb.len() || !(wordc(hb[i + wb.len()]) || hb[i + wb.len()] == b'-'))
        {
            return true;
        }
        i += 1;
    }
    false
}

/// Is this record CUT — an executor already holds it (open with a `job:branch`, or open and its own words name its
/// executor branch)?
pub fn running(r: &Record) -> bool {
    // a LANDED item keeps its `job:branch` (the hand-back's record); only an open one is still held — a §3 name
    // whose landed item flew red again is owed again, never skipped for ever
    if r.get(K_STATUS) == "open" && !r.get(K_BRANCH).is_empty() {
        return true;
    }
    let head: String = r.body.chars().take(200).collect::<String>().to_ascii_lowercase();
    r.get(K_STATUS) == "open"
        && match r.kind {
            Kind::Ledger => r.body.to_ascii_lowercase().contains("on branch "),
            // a queue item that names its executor branch in its head was cut (`· SPECPINS2 (branch exec-rmbp-…`)
            _ => head.contains("branch exec-") || head.contains("`exec-"),
        }
}

/// Open for the next wave: `job:status` is `open`, not [`running`], and its own words do not close it (`✓`, or a
/// [`CLOSED_WORDS`] word in its head). A claim is never a wave item.
pub fn next_open(r: &Record) -> bool {
    if r.kind == Kind::Claim || r.get(K_STATUS) != "open" || running(r) || r.body.starts_with("✓") {
        return false;
    }
    let head: String = r.body.chars().take(96).collect::<String>().to_ascii_uppercase();
    !CLOSED_WORDS.iter().any(|w| has_word(&head, w))
}

/// R101's two GPU slots.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gpu {
    /// The GK107 (the copy engine, then the GR/falcon ladder).
    Kepler,
    /// The HD 4000 (gen7: the blitter, then 3D).
    Intel,
}

impl Gpu {
    pub fn word(self) -> &'static str {
        match self {
            Gpu::Kepler => "kepler",
            Gpu::Intel => "intel",
        }
    }
}

/// Which GPU ladder a record is a rung of, by the words of its HEAD (its first 160 characters, case-insensitive) —
/// a passing mention deep in a long item ("the Intel font") is not a rung.
pub fn gpu_lane(r: &Record) -> Option<Gpu> {
    let t = r.body.chars().take(160).collect::<String>().to_ascii_lowercase();
    if ["kepler", "gk107", "nvidia"].iter().any(|w| t.contains(w)) {
        Some(Gpu::Kepler)
    } else if ["gen7", "ivb", "intel", "hd 4000"].iter().any(|w| t.contains(w)) {
        Some(Gpu::Intel)
    } else {
        None
    }
}

/// The ledger row a record cites: a ledger row is its own; otherwise the first `A<n>`/`B<n>`/`E<n>`/`SR<n>` word.
pub fn ledger_ref(r: &Record) -> String {
    if r.kind == Kind::Ledger {
        return r.id.clone();
    }
    let b = r.body.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let start = i == 0 || !b[i - 1].is_ascii_alphanumeric();
        let p = if b[i..].starts_with(b"SR") { 2 } else if matches!(b[i], b'A' | b'B' | b'E') { 1 } else { 0 };
        if start && p > 0 {
            let mut j = i + p;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            if j > i + p && (j == b.len() || !b[j].is_ascii_alphanumeric()) {
                return String::from(&r.body[i..j]);
            }
        }
        i += 1;
    }
    String::from("-")
}

/// One row of the next wave. `rec` indexes the records slice (`None`: a §3 name with no record yet).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ranked {
    pub rec: Option<usize>,
    pub name: String,
    /// 0 = Peter's §3 order, 1 = an R101 GPU slot, 2 = the queue's `job:seq`.
    pub tier: u8,
    pub lane: Option<Gpu>,
    /// For a §3 name with no open record: the newest record of the track that carries the name (any status), whose
    /// ledger row the row cites.
    pub cite: Option<usize>,
}

impl Ranked {
    /// `owed` for a §3 name no open record holds; else the record's `job:status`.
    pub fn status<'a>(&self, records: &'a [Record]) -> &'a str {
        match self.rec {
            Some(i) => records[i].get(K_STATUS),
            None => "owed",
        }
    }
}

/// THE RANKING. (a) every §3 name in `owed`'s order — the open queue item of `track` with that `job:arc`, else its
/// newest open ledger row, else (no open record, none running) the name alone; a name an executor already holds
/// is skipped. (b) R101: the first open Kepler rung and the first open Intel rung of the track's queue by
/// `job:seq` — ALWAYS in the `n` (tier (a) yields its tail). (c) the track's other open queue items by `job:seq`.
pub fn rank_next(records: &[Record], owed: &[String], track: &str, n: usize) -> Vec<Ranked> {
    let mine = |r: &Record| r.track == track && r.kind != Kind::Claim;
    let mut queue: Vec<usize> =
        (0..records.len()).filter(|&i| mine(&records[i]) && records[i].kind == Kind::Queue && next_open(&records[i])).collect();
    queue.sort_by_key(|&i| records[i].seq);
    let mut tier_a: Vec<Ranked> = Vec::new();
    for name in owed {
        let has = |i: &usize| records[*i].get(K_ARC) == name.as_str();
        if (0..records.len()).any(|i| mine(&records[i]) && has(&i) && running(&records[i])) {
            continue;
        }
        let ledger = (0..records.len())
            .filter(|i| mine(&records[*i]) && records[*i].kind == Kind::Ledger && has(i) && next_open(&records[*i]))
            .max_by_key(|&i| records[i].seq);
        let rec = queue.iter().copied().find(|i| has(i)).or(ledger);
        if rec.is_some_and(|i| tier_a.iter().any(|t| t.rec == Some(i))) {
            continue;
        }
        let cite = if rec.is_some() { None } else { (0..records.len()).filter(|i| mine(&records[*i]) && has(i)).max_by_key(|&i| (records[i].kind == Kind::Ledger, records[i].seq)) };
        tier_a.push(Ranked { rec, name: name.clone(), tier: 0, lane: rec.and_then(|i| gpu_lane(&records[i])), cite });
    }
    let mut gpu: Vec<Ranked> = Vec::new();
    for lane in [Gpu::Kepler, Gpu::Intel] {
        if tier_a.iter().any(|t| t.lane == Some(lane)) {
            continue;
        }
        if let Some(&i) = queue.iter().find(|&&i| gpu_lane(&records[i]) == Some(lane)) {
            gpu.push(Ranked { rec: Some(i), name: records[i].get(K_ARC).to_string(), tier: 1, lane: Some(lane), cite: None });
        }
    }
    let keep_a = n.saturating_sub(gpu.len().min(n)).min(tier_a.len());
    let mut out: Vec<Ranked> = tier_a.into_iter().take(keep_a).collect();
    out.extend(gpu.into_iter().take(n.saturating_sub(out.len())));
    for i in queue {
        if out.len() >= n {
            break;
        }
        if !out.iter().any(|t| t.rec == Some(i)) {
            out.push(Ranked { rec: Some(i), name: records[i].get(K_ARC).to_string(), tier: 2, lane: gpu_lane(&records[i]), cite: None });
        }
    }
    out
}

/// The header both rings print: `[jobs] next flight=f27 track=rmbp owed=<k> open=<m> ranked=<n> gpu=kepler:<id>,intel:<id>`.
pub fn next_header(records: &[Record], ranked: &[Ranked], flight: &str, owed: usize, track: &str) -> String {
    let open = records.iter().filter(|r| r.track == track && next_open(r)).count();
    let mut gpu: Vec<String> = Vec::new();
    for lane in [Gpu::Kepler, Gpu::Intel] {
        let id = ranked.iter().find(|t| t.lane == Some(lane)).and_then(|t| t.rec).map(|i| records[i].id.as_str()).unwrap_or("-");
        gpu.push(format!("{}:{}", lane.word(), id));
    }
    format!(
        "[jobs] next flight={} track={} owed={} open={} ranked={} gpu={}",
        if flight.is_empty() { "-" } else { flight },
        track,
        owed,
        open,
        ranked.len(),
        gpu.join(",")
    )
}

/// One ranked row: `<rank>  <id>  <name>  <status>  <ledger row>  <brief>` (tab-separated; `brief` is the host's).
pub fn next_row(records: &[Record], rank: usize, t: &Ranked, brief: &str) -> String {
    let (id, lref) = match t.rec {
        Some(i) => (records[i].id.as_str(), ledger_ref(&records[i])),
        None => ("-", t.cite.map(|i| ledger_ref(&records[i])).unwrap_or_else(|| String::from("-"))),
    };
    let tag = match (t.tier, t.lane) {
        (1, Some(l)) => format!(" [gpu:{}]", l.word()),
        _ => String::new(),
    };
    format!(
        "{:>2}\t{}\t{}{}\t{}\t{}\t{}",
        rank,
        id,
        if t.name.is_empty() { "-" } else { t.name.as_str() },
        tag,
        t.status(records),
        lref,
        if brief.is_empty() { "-" } else { brief }
    )
}

#[cfg(test)]
mod next_tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;

    const F27: &str = "# F\n## 2. x\nNOTME\n## 3. Owed (the cloud's, in Peter's order)\nFINEMOTION + HIDSTALL (R101: the pointer — fine deltas die), JOBSCAN (the jobs read; 17 s), MENUBAR item overlap (wifi/notify), HIDSTALL (5 s stall), the play verb (`tests play` retired), PRTSCR's capture race, PREHEAP's QEMU blind spot. USBNET RX (the ring; USBNETFTDI PASS). Then Cmd-Tab, the lid.\n## 4. Next\nLATER\n";

    #[test]
    fn owed_names_are_the_item_heads_in_order() {
        let v = owed_names(F27);
        assert_eq!(v, vec!["FINEMOTION", "HIDSTALL", "JOBSCAN", "MENUBAR", "PRTSCR", "PREHEAP", "USBNET"]);
    }

    fn q(seq: i64, body: &str) -> Record {
        let mut r = Record::new(Kind::Queue, &format!("{:03}-{}", seq, arc_name(body)), "rmbp", seq, body.to_string());
        r.set(K_STATUS, queue_status(body));
        r.set(K_ARC, &arc_name(body));
        r
    }

    #[test]
    fn rank_obeys_peter_then_the_gpus_then_seq() {
        let mut recs = vec![
            q(1, "✓ DONE  GMUX-1  the Kepler rung"),
            q(2, "· NEW  CEFLY  first Kepler CE-LADDER flight (ledger row **B126**)"),
            q(3, "· NEW  OLDTHING  a job"),
            q(4, "· NEW  GEN7BLIT  the gen7 blitter on the card"),
            q(5, "· NEW  JOBSCAN  the jobs read off the render handler (B505)"),
            q(6, "· NEW  PRTSCR  capture race"),
            q(7, "· DONE  NEWER  a closed one"),
        ];
        let mut cut = q(8, "· NEW  MENUBAR  overlap");
        cut.set(K_BRANCH, "exec-rmbp-menubar");
        recs.push(cut);
        let owed: Vec<String> = owed_names(F27);
        let r = rank_next(&recs, &owed, "rmbp", 13);
        let ids: Vec<&str> = r.iter().map(|t| t.rec.map(|i| recs[i].id.as_str()).unwrap_or(t.name.as_str())).collect();
        assert_eq!(ids, vec!["FINEMOTION", "HIDSTALL", "005-JOBSCAN", "006-PRTSCR", "PREHEAP", "USBNET", "002-CEFLY", "004-GEN7BLIT", "003-OLDTHING"]);
        assert_eq!(r[0].status(&recs), "owed");
        assert_eq!(ledger_ref(&recs[1]), "B126");
        // the GPU slots are ALWAYS in the n: §3 yields its tail
        let r = rank_next(&recs, &owed, "rmbp", 3);
        let ids: Vec<&str> = r.iter().map(|t| t.rec.map(|i| recs[i].id.as_str()).unwrap_or(t.name.as_str())).collect();
        assert_eq!(ids, vec!["FINEMOTION", "002-CEFLY", "004-GEN7BLIT"]);
        let h = next_header(&recs, &r, "f27", owed.len(), "rmbp");
        assert_eq!(h, "[jobs] next flight=f27 track=rmbp owed=7 open=5 ranked=3 gpu=kepler:002-CEFLY,intel:004-GEN7BLIT");
        assert!(next_row(&recs, 2, &r[1], "").contains("CEFLY [gpu:kepler]\topen\tB126\t-"));
    }

    #[test]
    fn a_running_ledger_row_is_not_next() {
        let mut l = Record::new(Kind::Ledger, "B506", "rmbp", 1, "| B506 | **JOBSNEXT** | rmbp | open — JOBSNEXT on branch exec-rmbp-jobsnext |".to_string());
        l.set(K_STATUS, "open");
        l.set(K_ARC, "JOBSNEXT");
        assert!(running(&l) && !next_open(&l));
        let r = rank_next(&[l], &["JOBSNEXT".to_string()], "rmbp", 13);
        assert!(r.is_empty());
    }
}
