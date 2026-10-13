//! URL Standard §5 application/x-www-form-urlencoded — the parser (§5.1), the serializer (§5.2) — and the
//! §6.2 `URLSearchParams` list with every IDL operation (append, delete, get, getAll, has, set, sort, size,
//! stringification).

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use super::percent::{in_set, percent_decode, push_byte, EncodeSet};

/// §5.1 the application/x-www-form-urlencoded parser over bytes.
pub fn parse(input: &[u8]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for seq in input.split(|&b| b == b'&') {
        if seq.is_empty() {
            continue;
        }
        let (name, value) = match seq.iter().position(|&b| b == b'=') {
            Some(i) => (&seq[..i], &seq[i + 1..]),
            None => (seq, &[][..]),
        };
        let plus = |s: &[u8]| -> Vec<u8> { s.iter().map(|&b| if b == b'+' { b' ' } else { b }).collect() };
        let n = String::from_utf8_lossy(&percent_decode(&plus(name))).into_owned();
        let v = String::from_utf8_lossy(&percent_decode(&plus(value))).into_owned();
        out.push((n, v));
    }
    out
}

/// [`parse`] over a string's UTF-8 bytes.
pub fn parse_str(input: &str) -> Vec<(String, String)> {
    parse(input.as_bytes())
}

/// The urlencoded byte serializer: percent-encode with the application/x-www-form-urlencoded percent-encode
/// set, space as `+` (§1.3 "percent-encode after encoding", spaceAsPlus = true, UTF-8).
pub fn byte_serialize(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len());
    byte_serialize_into(&mut out, input);
    out
}

/// [`byte_serialize`], appending to `out`.
pub fn byte_serialize_into(out: &mut String, input: &[u8]) {
    for &b in input {
        if b == b' ' {
            out.push('+');
        } else if in_set(EncodeSet::FormUrlencoded, b) {
            push_byte(out, b);
        } else {
            out.push(b as char);
        }
    }
}

/// §5.2 the application/x-www-form-urlencoded serializer: `name=value` pairs joined by `&`.
pub fn serialize<I, K, V>(pairs: I) -> String
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<str>,
    V: AsRef<str>,
{
    let mut out = String::new();
    for (i, (k, v)) in pairs.into_iter().enumerate() {
        if i > 0 {
            out.push('&');
        }
        byte_serialize_into(&mut out, k.as_ref().as_bytes());
        out.push('=');
        byte_serialize_into(&mut out, v.as_ref().as_bytes());
    }
    out
}

/// §6.2 a `URLSearchParams` object's list of name-value pairs (the URL object link is the caller's: after a
/// mutation it runs the update steps, [`super::Url::set_search_params`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UrlSearchParams {
    list: Vec<(String, String)>,
}

impl UrlSearchParams {
    pub fn new() -> Self {
        Self::default()
    }

    /// The constructor's string form: a single leading `?` is removed, the rest parsed.
    pub fn parse(init: &str) -> Self {
        let s = init.strip_prefix('?').unwrap_or(init);
        UrlSearchParams { list: parse_str(s) }
    }

    /// The constructor's sequence / record forms.
    pub fn from_pairs<I, K, V>(pairs: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        UrlSearchParams { list: pairs.into_iter().map(|(k, v)| (k.into(), v.into())).collect() }
    }

    /// `size`.
    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn append(&mut self, name: &str, value: &str) {
        self.list.push((name.into(), value.into()));
    }

    /// `delete(name, value)`: every pair with that name (and, when given, that value) is removed.
    pub fn delete(&mut self, name: &str, value: Option<&str>) {
        self.list.retain(|(n, v)| !(n == name && value.is_none_or(|w| v == w)));
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.list.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }

    pub fn get_all(&self, name: &str) -> Vec<&str> {
        self.list.iter().filter(|(n, _)| n == name).map(|(_, v)| v.as_str()).collect()
    }

    /// `has(name, value)`.
    pub fn has(&self, name: &str, value: Option<&str>) -> bool {
        self.list.iter().any(|(n, v)| n == name && value.is_none_or(|w| v == w))
    }

    /// `set(name, value)`: the first pair with that name takes the value, the others are removed; appended
    /// when there is none.
    pub fn set(&mut self, name: &str, value: &str) {
        let mut seen = false;
        self.list.retain_mut(|(n, v)| {
            if n != name {
                return true;
            }
            if seen {
                return false;
            }
            seen = true;
            *v = value.into();
            true
        });
        if !seen {
            self.append(name, value);
        }
    }

    /// `sort()`: stable, by name, comparing UTF-16 code units.
    pub fn sort(&mut self) {
        self.list.sort_by(|a, b| a.0.encode_utf16().cmp(b.0.encode_utf16()));
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.list.iter().map(|(n, v)| (n.as_str(), v.as_str()))
    }

    pub fn pairs(&self) -> &[(String, String)] {
        &self.list
    }
}

impl fmt::Display for UrlSearchParams {
    /// The stringification behavior: the §5.2 serializer.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&serialize(self.list.iter().map(|(n, v)| (n.as_str(), v.as_str()))))
    }
}
