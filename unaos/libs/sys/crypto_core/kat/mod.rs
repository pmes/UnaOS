// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// CRYPTOCORE known-answer harness — shared, by `#[path]`, between `crypto_core/tests/kat.rs` (cargo test)
// and `tools/crypto-check` (the host bin that prints the counts). Host-only (std). Zero dependencies:
// the vector files are fetched with the system `curl`, verified with our own SHA-256 (which is proven
// first, by the embedded FIPS 180-4 / CAVP short-message vectors), and parsed by the small CAVP-`.rsp`
// and JSON readers below.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::PathBuf;

pub mod m1;
pub mod m2;
pub mod m3;
pub mod m4;
pub mod m5;
pub mod m6;

/// One KAT set's result.
#[derive(Debug, Default)]
pub struct Tally {
    pub name: String,
    pub pass: usize,
    pub fail: usize,
    /// Set when the whole set could not run (offline): the reason.
    pub skipped: Option<String>,
    /// The first few failure descriptions.
    pub failures: Vec<String>,
    /// Individual vectors not run because they exercise something this crate does not offer (named in
    /// the set's doc comment) — counted, never silently dropped.
    pub unsupported: usize,
}

impl Tally {
    pub fn new(name: &str) -> Self {
        Tally { name: name.to_string(), ..Default::default() }
    }
    pub fn check(&mut self, ok: bool, what: impl FnOnce() -> String) {
        if ok {
            self.pass += 1;
        } else {
            self.fail += 1;
            if self.failures.len() < 8 {
                self.failures.push(what());
            }
        }
    }
    pub fn skip(name: &str, why: String) -> Self {
        Tally { name: name.to_string(), skipped: Some(why), ..Default::default() }
    }
}

/// Every KAT set, in milestone order. `filter` keeps the sets whose name starts with it.
pub fn run_all(filter: Option<&str>) -> Vec<Tally> {
    let sets: Vec<(&str, fn() -> Tally)> = m1::SETS
        .iter()
        .chain(m2::SETS.iter())
        .chain(m3::SETS.iter())
        .chain(m4::SETS.iter())
        .chain(m5::SETS.iter())
        .chain(m6::SETS.iter())
        .copied()
        .collect();
    let mut out = Vec::new();
    for (name, f) in sets {
        if let Some(flt) = filter {
            if !name.starts_with(flt) {
                continue;
            }
        }
        let mut t = f();
        t.name = name.to_string();
        out.push(t);
    }
    out
}

// ---------------------------------------------------------------------------------------------
// hex
// ---------------------------------------------------------------------------------------------

pub fn unhex(s: &str) -> Vec<u8> {
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(s.len() % 2 == 0, "odd hex: {s}");
    (0..s.len() / 2).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).expect("hex")).collect()
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

// ---------------------------------------------------------------------------------------------
// vector files: embedded or fetched (vectors.txt pins url + sha256)
// ---------------------------------------------------------------------------------------------

const VECTORS_TXT: &str = include_str!("vectors.txt");

fn cache_dir() -> PathBuf {
    match std::env::var_os("CRYPTO_VECTORS_DIR") {
        Some(d) => PathBuf::from(d),
        None => std::env::temp_dir().join("unaos-crypto-vectors"),
    }
}

/// A FETCH file's bytes: from the cache when its sha256 matches, else downloaded (curl) and verified.
/// `Err(reason)` when offline / `CRYPTO_OFFLINE=1` / the download does not match its pin.
pub fn fetch(name: &str) -> Result<Vec<u8>, String> {
    let line = VECTORS_TXT
        .lines()
        .find(|l| {
            let mut it = l.split_whitespace();
            it.next() == Some("FETCH") && it.next() == Some(name)
        })
        .ok_or_else(|| format!("{name}: not in vectors.txt"))?;
    let f: Vec<&str> = line.split_whitespace().collect();
    let (want, url) = (f[2], f[3]);
    let dir = cache_dir();
    let path = dir.join(name);
    if let Ok(b) = std::fs::read(&path) {
        if hex(&crypto_core::sha2::sha256(&b)) == want {
            return Ok(b);
        }
    }
    if std::env::var_os("CRYPTO_OFFLINE").is_some() {
        return Err(format!("{name}: CRYPTO_OFFLINE set"));
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("{name}: {e}"))?;
    let tmp = dir.join(format!("{name}.part{}", std::process::id()));
    let st = std::process::Command::new("curl")
        .args(["-sSfL", "--max-time", "180", "-o"])
        .arg(&tmp)
        .arg(url)
        .status()
        .map_err(|e| format!("{name}: curl: {e}"))?;
    if !st.success() {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("{name}: offline (curl {st})"));
    }
    let b = std::fs::read(&tmp).map_err(|e| format!("{name}: {e}"))?;
    let got = hex(&crypto_core::sha2::sha256(&b));
    if got != want {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("{name}: sha256 {got} != pinned {want}"));
    }
    std::fs::rename(&tmp, &path).map_err(|e| format!("{name}: {e}"))?;
    Ok(b)
}

/// `fetch` as text, or a skipped Tally.
pub fn fetch_text(set: &str, name: &str) -> Result<String, Tally> {
    match fetch(name) {
        Ok(b) => Ok(String::from_utf8_lossy(&b).into_owned()),
        Err(e) => Err(Tally::skip(set, e)),
    }
}

// ---------------------------------------------------------------------------------------------
// CAVP `.rsp` / pyca `KEY = VALUE` reader
// ---------------------------------------------------------------------------------------------

/// One record: its `KEY = VALUE` lines (keys upper-cased; a bare word line such as `FAIL` is a key with
/// an empty value) plus the `[section]` headers in force (`[L = 32]` → `L`; `[P-256,SHA-256]` → `_`;
/// `[ENCRYPT]` → `_`).
#[derive(Debug, Clone, Default)]
pub struct Rec {
    pub kv: BTreeMap<String, String>,
    pub hdr: BTreeMap<String, String>,
}

impl Rec {
    pub fn get(&self, k: &str) -> Option<&str> {
        self.kv.get(k).map(|s| s.as_str())
    }
    pub fn bytes(&self, k: &str) -> Vec<u8> {
        unhex(self.get(k).unwrap_or(""))
    }
    pub fn has(&self, k: &str) -> bool {
        self.kv.contains_key(k)
    }
    pub fn h(&self, k: &str) -> Option<&str> {
        self.hdr.get(k).map(|s| s.as_str())
    }
}

pub fn parse_rsp(text: &str) -> Vec<Rec> {
    let mut out = Vec::new();
    let mut hdr: BTreeMap<String, String> = BTreeMap::new();
    let mut cur = Rec::default();
    let mut in_hdr_run = false;
    let flush = |cur: &mut Rec, out: &mut Vec<Rec>| {
        if !cur.kv.is_empty() {
            out.push(std::mem::take(cur));
        }
    };
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('#') {
            continue;
        }
        if line.is_empty() {
            flush(&mut cur, &mut out);
            in_hdr_run = false;
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            flush(&mut cur, &mut out);
            if !in_hdr_run {
                // a new header run replaces the section-scoped `_` header only when it carries one
            }
            in_hdr_run = true;
            let inner = &line[1..line.len() - 1];
            if let Some((k, v)) = inner.split_once('=') {
                hdr.insert(k.trim().to_ascii_uppercase(), v.trim().to_string());
            } else {
                hdr.insert("_".to_string(), inner.trim().to_string());
            }
            continue;
        }
        in_hdr_run = false;
        let (k, v) = match line.split_once('=') {
            Some((k, v)) => (k.trim().to_ascii_uppercase(), v.trim().to_string()),
            None => (line.to_ascii_uppercase(), String::new()),
        };
        // A key repeating inside one record starts a new record (some files omit blank lines).
        if cur.kv.contains_key(&k) {
            flush(&mut cur, &mut out);
        }
        if cur.kv.is_empty() {
            cur.hdr = hdr.clone();
        }
        cur.kv.insert(k, v);
    }
    flush(&mut cur, &mut out);
    out
}

// ---------------------------------------------------------------------------------------------
// JSON (Wycheproof) reader
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum J {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<J>),
    Obj(Vec<(String, J)>),
}

impl J {
    pub fn get(&self, k: &str) -> &J {
        static NULL: J = J::Null;
        match self {
            J::Obj(v) => v.iter().find(|(kk, _)| kk == k).map(|(_, v)| v).unwrap_or(&NULL),
            _ => &NULL,
        }
    }
    pub fn s(&self) -> &str {
        match self {
            J::Str(s) => s,
            _ => "",
        }
    }
    pub fn n(&self) -> f64 {
        match self {
            J::Num(n) => *n,
            _ => 0.0,
        }
    }
    pub fn arr(&self) -> &[J] {
        match self {
            J::Arr(v) => v,
            _ => &[],
        }
    }
    pub fn bytes(&self) -> Vec<u8> {
        unhex(self.s())
    }
    pub fn is_null(&self) -> bool {
        matches!(self, J::Null)
    }
    pub fn has_flag(&self, f: &str) -> bool {
        self.get("flags").arr().iter().any(|x| x.s() == f)
    }
}

pub fn parse_json(text: &str) -> J {
    let b = text.as_bytes();
    let mut i = 0usize;
    let v = jval(b, &mut i);
    v
}

fn ws(b: &[u8], i: &mut usize) {
    while *i < b.len() && (b[*i] as char).is_ascii_whitespace() {
        *i += 1;
    }
}

fn jval(b: &[u8], i: &mut usize) -> J {
    ws(b, i);
    match b[*i] {
        b'{' => {
            *i += 1;
            let mut v = Vec::new();
            loop {
                ws(b, i);
                if b[*i] == b'}' {
                    *i += 1;
                    break;
                }
                let k = match jval(b, i) {
                    J::Str(s) => s,
                    _ => panic!("json key"),
                };
                ws(b, i);
                assert_eq!(b[*i], b':');
                *i += 1;
                let x = jval(b, i);
                v.push((k, x));
                ws(b, i);
                if b[*i] == b',' {
                    *i += 1;
                }
            }
            J::Obj(v)
        }
        b'[' => {
            *i += 1;
            let mut v = Vec::new();
            loop {
                ws(b, i);
                if b[*i] == b']' {
                    *i += 1;
                    break;
                }
                v.push(jval(b, i));
                ws(b, i);
                if b[*i] == b',' {
                    *i += 1;
                }
            }
            J::Arr(v)
        }
        b'"' => {
            *i += 1;
            let mut s = String::new();
            loop {
                let c = b[*i];
                *i += 1;
                match c {
                    b'"' => break,
                    b'\\' => {
                        let e = b[*i];
                        *i += 1;
                        match e {
                            b'n' => s.push('\n'),
                            b't' => s.push('\t'),
                            b'r' => s.push('\r'),
                            b'b' => s.push('\u{8}'),
                            b'f' => s.push('\u{c}'),
                            b'u' => {
                                let h = std::str::from_utf8(&b[*i..*i + 4]).unwrap();
                                *i += 4;
                                let cp = u32::from_str_radix(h, 16).unwrap();
                                s.push(char::from_u32(cp).unwrap_or('\u{fffd}'));
                            }
                            other => s.push(other as char),
                        }
                    }
                    _ => {
                        // copy one UTF-8 sequence verbatim
                        let start = *i - 1;
                        let mut end = *i;
                        while end < b.len() && (b[end] & 0xC0) == 0x80 {
                            end += 1;
                        }
                        s.push_str(std::str::from_utf8(&b[start..end]).unwrap_or("\u{fffd}"));
                        *i = end;
                    }
                }
            }
            J::Str(s)
        }
        b't' => {
            *i += 4;
            J::Bool(true)
        }
        b'f' => {
            *i += 5;
            J::Bool(false)
        }
        b'n' => {
            *i += 4;
            J::Null
        }
        _ => {
            let start = *i;
            while *i < b.len() && matches!(b[*i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9') {
                *i += 1;
            }
            J::Num(std::str::from_utf8(&b[start..*i]).unwrap().parse().unwrap())
        }
    }
}

/// Every Wycheproof test with its group: `(group, test)`.
pub fn wycheproof_tests(doc: &J) -> Vec<(&J, &J)> {
    let mut v = Vec::new();
    for g in doc.get("testGroups").arr() {
        for t in g.get("tests").arr() {
            v.push((g, t));
        }
    }
    v
}

/// Wycheproof `result`: valid → Some(true), invalid → Some(false), acceptable → None (either answer
/// is conformant; the harness records which way we went but never fails it).
pub fn wy_expect(t: &J) -> Option<bool> {
    match t.get("result").s() {
        "valid" => Some(true),
        "invalid" => Some(false),
        _ => None,
    }
}

/// Like [`parse_rsp`] but a record starts only at `start_key` (blank lines inside a record are allowed —
/// the pyca RFC 5869 transcription puts one before `PRK`).
pub fn parse_rsp_by(text: &str, start_key: &str) -> Vec<Rec> {
    let mut out: Vec<Rec> = Vec::new();
    for r in parse_rsp(text) {
        if r.has(start_key) || out.is_empty() {
            out.push(r);
        } else {
            let last = out.last_mut().unwrap();
            for (k, v) in r.kv {
                last.kv.insert(k, v);
            }
        }
    }
    out
}
