// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Principia — shared-core
//!
//! SETTINGSFILES (rmbp-ledger B407, R98) — a program's OWN settings: `app.<name>.<key>`, stored in
//! `settings/<name>` ([`crate::files::domain_of`]). The program declares its stanza ONCE through the bus
//! (`PrefDeclare`, [`crate::wire::VERB_DECLARE`]): each key's type, range, default and description — the
//! per-program twin of [`crate::schema::SCHEMA`]. A declared key's writes are checked by [`check`] (the
//! schema's clamp rule) and its description is the comment above it in the file.
//!
//! Body: `<name>` NUL then one line per key, tab-separated: `<key>\t<spec>\t<default literal>\t<doc>`,
//! `<spec>` = `int:<min>:<max>` | `float:<min>:<max>` | `bool` | `str:<max_len>` | `enum:<a>,<b>,…`.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::schema::{Applied, Kind, Refusal};
use crate::PrefValue;

/// Keys one program may declare.
pub const MAX_KEYS: usize = 32;

/// A declared kind (the schema's [`Kind`], owning its enum spellings).
#[derive(Clone, Debug, PartialEq)]
pub enum DeclKind {
    Int { min: i64, max: i64 },
    Float { min: f64, max: f64 },
    Bool,
    Str { max_len: usize },
    Enum(Vec<String>),
}

/// One declared key of a program.
#[derive(Clone, Debug, PartialEq)]
pub struct DeclKey {
    pub key: String,
    pub kind: DeclKind,
    pub default: PrefValue,
    pub doc: String,
}

/// Program name -> its stanza.
pub type Registry = BTreeMap<String, Vec<DeclKey>>;

fn spec(s: &str) -> Option<DeclKind> {
    let (t, rest) = s.split_once(':').unwrap_or((s, ""));
    let two = || {
        let (a, b) = rest.split_once(':')?;
        Some((a, b))
    };
    Some(match t {
        "int" => {
            let (a, b) = two()?;
            let (min, max) = (a.parse().ok()?, b.parse().ok()?);
            if min > max {
                return None;
            }
            DeclKind::Int { min, max }
        }
        "float" => {
            let (a, b) = two()?;
            let (min, max): (f64, f64) = (a.parse().ok()?, b.parse().ok()?);
            if !(min <= max) {
                return None;
            }
            DeclKind::Float { min, max }
        }
        "bool" => DeclKind::Bool,
        "str" => DeclKind::Str { max_len: rest.parse().ok().filter(|n| *n > 0 && *n <= 4096)? },
        "enum" => {
            let v: Vec<String> = rest.split(',').filter(|x| !x.is_empty()).map(String::from).collect();
            if v.is_empty() {
                return None;
            }
            DeclKind::Enum(v)
        }
        _ => return None,
    })
}

fn spec_text(k: &DeclKind) -> String {
    match k {
        DeclKind::Int { min, max } => alloc::format!("int:{}:{}", min, max),
        DeclKind::Float { min, max } => alloc::format!("float:{}:{}", min, max),
        DeclKind::Bool => String::from("bool"),
        DeclKind::Str { max_len } => alloc::format!("str:{}", max_len),
        DeclKind::Enum(v) => alloc::format!("enum:{}", v.join(",")),
    }
}

/// The PrefDeclare body for `name`'s stanza.
pub fn body(name: &str, keys: &[DeclKey]) -> Vec<u8> {
    let mut b = Vec::from(name.as_bytes());
    b.push(0);
    for k in keys {
        b.extend_from_slice(alloc::format!("{}\t{}\t{}\t{}\n", k.key, spec_text(&k.kind), k.default.to_literal(), k.doc).as_bytes());
    }
    b
}

/// Check `v` against a declared kind — the schema's own clamp for the shared kinds.
pub fn check_kind(kind: &DeclKind, v: PrefValue) -> Result<Applied, Refusal> {
    match kind {
        DeclKind::Int { min, max } => crate::schema::clamp(&Kind::Int { min: *min, max: *max }, v),
        DeclKind::Float { min, max } => crate::schema::clamp(&Kind::Float { min: *min, max: *max }, v),
        DeclKind::Bool => crate::schema::clamp(&Kind::Bool, v),
        DeclKind::Str { max_len } => crate::schema::clamp(&Kind::Str { max_len: *max_len, printable: false }, v),
        DeclKind::Enum(ok) => match &v {
            PrefValue::Str(s) if ok.iter().any(|o| o == s) => Ok(Applied { value: v, clamped: false }),
            PrefValue::Str(_) => Err(Refusal::NotInEnum),
            _ => Err(Refusal::WrongType { want: "string" }),
        },
    }
}

/// Parse a PrefDeclare body: the program name and its keys. `None` = malformed (a bad name, a bad spec, a
/// default its own kind refuses, more than [`MAX_KEYS`]).
pub fn parse(body: &[u8]) -> Option<(String, Vec<DeclKey>)> {
    let z = body.iter().position(|&b| b == 0)?;
    let name = core::str::from_utf8(&body[..z]).ok()?;
    if !crate::valid_segment(name) {
        return None;
    }
    let text = core::str::from_utf8(&body[z + 1..]).ok()?;
    let mut keys = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let mut f = line.splitn(4, '\t');
        let (key, sp, def, doc) = (f.next()?, f.next()?, f.next()?, f.next().unwrap_or(""));
        crate::validate_key(key).ok()?;
        let kind = spec(sp)?;
        let default = PrefValue::from_literal(def).ok()?;
        let a = check_kind(&kind, default.clone()).ok()?;
        if a.clamped {
            return None;
        }
        keys.push(DeclKey { key: String::from(key), kind, default, doc: String::from(doc) });
    }
    (!keys.is_empty() && keys.len() <= MAX_KEYS).then(|| (String::from(name), keys))
}

/// The declared row of `app.<name>.<rest>` (`key` = `<name>.<rest>`), if any.
pub fn lookup<'a>(reg: &'a Registry, key: &str) -> Option<&'a DeclKey> {
    let (name, rest) = key.split_once('.')?;
    reg.get(name)?.iter().find(|k| k.key == rest)
}

/// The value a write of `app.<key>` stores: a declared key is checked by its kind; an undeclared key
/// passes unchanged (the schema's rule for undeclared keys).
pub fn check(reg: &Registry, key: &str, v: PrefValue) -> Result<Applied, Refusal> {
    match lookup(reg, key) {
        Some(d) => check_kind(&d.kind, v),
        None => Ok(Applied { value: v, clamped: false }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stanza_round_trip_and_clamp() {
        let keys = alloc::vec![
            DeclKey { key: String::from("window.frame"), kind: DeclKind::Str { max_len: 32 }, default: PrefValue::Str(String::new()), doc: String::from("Lumen's window frame x,y,w,h") },
            DeclKey { key: String::from("zoom"), kind: DeclKind::Int { min: 1, max: 4 }, default: PrefValue::Int(1), doc: String::from("zoom") },
            DeclKey { key: String::from("theme"), kind: DeclKind::Enum(alloc::vec![String::from("dark"), String::from("light")]), default: PrefValue::Str(String::from("dark")), doc: String::new() },
        ];
        let b = body("lumen", &keys);
        let (n, back) = parse(&b).unwrap();
        assert_eq!((n.as_str(), &back), ("lumen", &keys));
        let mut reg = Registry::new();
        reg.insert(n, back);
        assert_eq!(check(&reg, "lumen.zoom", PrefValue::Int(9)), Ok(Applied { value: PrefValue::Int(4), clamped: true }));
        assert!(check(&reg, "lumen.theme", PrefValue::Str(String::from("blue"))).is_err());
        assert!(check(&reg, "lumen.zoom", PrefValue::Bool(true)).is_err());
        assert_eq!(check(&reg, "lumen.other", PrefValue::Int(9)), Ok(Applied { value: PrefValue::Int(9), clamped: false }));
        // Refusals: no NUL, a bad spec, a default outside its own range.
        assert!(parse(b"lumen").is_none());
        assert!(parse(b"lumen\0k\tint:5:1\t1\td\n").is_none());
        assert!(parse(b"lumen\0k\tint:0:3\t9\td\n").is_none());
        assert!(parse(b"lu men\0k\tbool\ttrue\td\n").is_none());
    }
}
