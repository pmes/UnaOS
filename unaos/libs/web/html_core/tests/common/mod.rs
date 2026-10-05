//! Shared test harness: vectors.txt reader, fetch-at-test-time cache (curl + sha256sum, skip offline), and a
//! small JSON reader for the html5lib tokenizer files (the crate has zero dependencies, dev included).

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

pub fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

pub fn cache_dir() -> PathBuf {
    let base = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate_dir().join("../../../../target"));
    let d = base.join("vectors-cache/html_core");
    std::fs::create_dir_all(&d).ok();
    d
}

/// `(url, sha256)` for every vectors.txt line of `suite`.
pub fn vectors(suite: &str) -> Vec<(String, String)> {
    let txt = std::fs::read_to_string(crate_dir().join("vectors.txt")).expect("vectors.txt");
    txt.lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let (s, u, h) = (it.next()?, it.next()?, it.next()?);
            (s == suite).then(|| (u.to_string(), h.to_string()))
        })
        .collect()
}

fn sha256_file(p: &std::path::Path) -> Option<String> {
    let out = Command::new("sha256sum").arg(p).output().ok()?;
    let s = String::from_utf8(out.stdout).ok()?;
    s.split_whitespace().next().map(|x| x.to_string())
}

/// Fetch `url` into the cache (once), verify sha256; `None` (with a note on stderr) when offline or mismatched.
pub fn fetch(url: &str, sha: &str) -> Option<String> {
    let name: String = url.rsplit('/').take(2).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("__");
    let commit = url.split('/').find(|s| s.len() == 40).unwrap_or("x");
    let path = cache_dir().join(format!("{}-{}", &commit[..commit.len().min(8)], name));
    if !(path.exists() && sha256_file(&path).as_deref() == Some(sha)) {
        let st = Command::new("curl").args(["-sSfL", "--max-time", "60", "-o"]).arg(&path).arg(url).status();
        if !matches!(st, Ok(s) if s.success()) {
            eprintln!("SKIP (offline?): could not fetch {url}");
            return None;
        }
        let got = sha256_file(&path);
        if got.as_deref() != Some(sha) {
            eprintln!("SKIP: sha256 mismatch for {url}: got {got:?}, want {sha}");
            return None;
        }
    }
    std::fs::read_to_string(&path).ok()
}

pub fn file_name(url: &str) -> String {
    url.rsplit('/').next().unwrap_or(url).to_string()
}

// ---- a small JSON reader -------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    /// A string as UTF-16 code units (so lone surrogates in `\uD800` escapes survive).
    Str(Vec<u16>),
    Arr(Vec<Json>),
    Obj(BTreeMap<String, Json>, Vec<String>),
}

impl Json {
    pub fn get(&self, k: &str) -> Option<&Json> {
        match self {
            Json::Obj(m, _) => m.get(k),
            _ => None,
        }
    }
    pub fn arr(&self) -> &[Json] {
        match self {
            Json::Arr(a) => a,
            _ => &[],
        }
    }
    /// The string, or `None` if it holds a lone surrogate.
    pub fn str(&self) -> Option<String> {
        match self {
            Json::Str(u) => String::from_utf16(u).ok(),
            _ => None,
        }
    }
    pub fn units(&self) -> Option<&[u16]> {
        match self {
            Json::Str(u) => Some(u),
            _ => None,
        }
    }
    /// Object keys in document order.
    pub fn keys(&self) -> &[String] {
        match self {
            Json::Obj(_, k) => k,
            _ => &[],
        }
    }
}

pub fn parse_json(s: &str) -> Json {
    let b = s.as_bytes();
    let mut i = 0;
    let v = value(b, &mut i);
    v
}

fn ws(b: &[u8], i: &mut usize) {
    while *i < b.len() && matches!(b[*i], b' ' | b'\t' | b'\n' | b'\r') {
        *i += 1;
    }
}

fn value(b: &[u8], i: &mut usize) -> Json {
    ws(b, i);
    match b[*i] {
        b'{' => {
            *i += 1;
            let mut m = BTreeMap::new();
            let mut order = Vec::new();
            loop {
                ws(b, i);
                if b[*i] == b'}' {
                    *i += 1;
                    break;
                }
                let k = match value(b, i) {
                    Json::Str(u) => String::from_utf16_lossy(&u),
                    _ => panic!("object key"),
                };
                ws(b, i);
                assert_eq!(b[*i], b':');
                *i += 1;
                let v = value(b, i);
                order.push(k.clone());
                m.insert(k, v);
                ws(b, i);
                if b[*i] == b',' {
                    *i += 1;
                }
            }
            Json::Obj(m, order)
        }
        b'[' => {
            *i += 1;
            let mut a = Vec::new();
            loop {
                ws(b, i);
                if b[*i] == b']' {
                    *i += 1;
                    break;
                }
                a.push(value(b, i));
                ws(b, i);
                if b[*i] == b',' {
                    *i += 1;
                }
            }
            Json::Arr(a)
        }
        b'"' => {
            *i += 1;
            let mut u: Vec<u16> = Vec::new();
            loop {
                let c = b[*i];
                if c == b'"' {
                    *i += 1;
                    break;
                }
                if c == b'\\' {
                    let e = b[*i + 1];
                    *i += 2;
                    match e {
                        b'n' => u.push(10),
                        b't' => u.push(9),
                        b'r' => u.push(13),
                        b'b' => u.push(8),
                        b'f' => u.push(12),
                        b'/' => u.push(b'/' as u16),
                        b'\\' => u.push(b'\\' as u16),
                        b'"' => u.push(b'"' as u16),
                        b'u' => {
                            let h = std::str::from_utf8(&b[*i..*i + 4]).unwrap();
                            u.push(u16::from_str_radix(h, 16).unwrap());
                            *i += 4;
                        }
                        _ => panic!("bad escape"),
                    }
                } else {
                    // copy one UTF-8 scalar
                    let len = match c {
                        0x00..=0x7F => 1,
                        0xC0..=0xDF => 2,
                        0xE0..=0xEF => 3,
                        _ => 4,
                    };
                    let s = std::str::from_utf8(&b[*i..*i + len]).unwrap();
                    let mut buf = [0u16; 2];
                    u.extend_from_slice(s.chars().next().unwrap().encode_utf16(&mut buf));
                    *i += len;
                }
            }
            Json::Str(u)
        }
        b't' => {
            *i += 4;
            Json::Bool(true)
        }
        b'f' => {
            *i += 5;
            Json::Bool(false)
        }
        b'n' => {
            *i += 4;
            Json::Null
        }
        _ => {
            let st = *i;
            while *i < b.len() && matches!(b[*i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9') {
                *i += 1;
            }
            Json::Num(std::str::from_utf8(&b[st..*i]).unwrap().parse().unwrap())
        }
    }
}

/// html5lib `doubleEscaped`: decode `\uHHHH` sequences inside an already-JSON-decoded string (UTF-16 units).
pub fn double_unescape(u: &[u16]) -> Vec<u16> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < u.len() {
        if u[i] == b'\\' as u16 && i + 5 < u.len() && u[i + 1] == b'u' as u16 {
            let h: String = u[i + 2..i + 6].iter().map(|&c| c as u8 as char).collect();
            if let Ok(v) = u16::from_str_radix(&h, 16) {
                out.push(v);
                i += 6;
                continue;
            }
        }
        out.push(u[i]);
        i += 1;
    }
    out
}

// ---- html5lib tree-construction .dat reader -----------------------------------------------------------------

pub struct DatTest {
    pub data: String,
    pub fragment: Option<String>,
    pub script: Option<bool>,
    pub document: String,
}

/// Split a .dat file into tests.
pub fn parse_dat(text: &str) -> Vec<DatTest> {
    let mut out = Vec::new();
    let body = text.strip_prefix("#data\n").unwrap_or(text);
    for block in body.split("\n#data\n") {
        let mut data = String::new();
        let mut fragment = None;
        let mut script = None;
        let mut document = String::new();
        let mut section = "data";
        for line in block.split_inclusive('\n') {
            let header = line.trim_end_matches('\n');
            let is_header = section != "document"
                && matches!(header, "#errors" | "#new-errors" | "#document-fragment" | "#script-off" | "#script-on" | "#document");
            if is_header {
                section = header.trim_start_matches('#');
                match section {
                    "script-off" => script = Some(false),
                    "script-on" => script = Some(true),
                    _ => {}
                }
                continue;
            }
            match section {
                "data" => data.push_str(line),
                "document-fragment" => fragment = Some(header.to_string()),
                "document" => document.push_str(line),
                _ => {}
            }
        }
        if data.ends_with('\n') {
            data.pop();
        }
        out.push(DatTest { data, fragment, script, document });
    }
    out
}

