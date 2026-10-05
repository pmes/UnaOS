//! HTTP fields (RFC 9110 §5): an ordered, case-insensitive multimap with the field grammar enforced on the way
//! in — `field-name = token` (§5.1, §5.6.2), a field value with no CR, LF or NUL (§5.5), and obs-fold
//! (RFC 9112 §5.2) refused rather than unfolded.

use alloc::string::String;
use alloc::vec::Vec;

/// RFC 9110 §5.6.2 `tchar`.
pub fn is_tchar(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)
}

/// `token = 1*tchar`.
pub fn is_token(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(is_tchar)
}

/// A field value we will put on the wire: no CR, LF or NUL (RFC 9110 §5.5).
pub fn is_valid_value(s: &str) -> bool {
    !s.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderError {
    BadName,
    BadValue,
}

/// An ordered list of fields; lookups ignore ASCII case, insertion order is kept for the wire.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Headers {
    list: Vec<(String, String)>,
}

impl Headers {
    pub fn new() -> Self {
        Headers { list: Vec::new() }
    }

    /// Append a field (repeats allowed — `Set-Cookie`).
    pub fn append(&mut self, name: &str, value: &str) -> Result<(), HeaderError> {
        if !is_token(name) {
            return Err(HeaderError::BadName);
        }
        if !is_valid_value(value) {
            return Err(HeaderError::BadValue);
        }
        self.list.push((String::from(name), String::from(value.trim_matches(|c| c == ' ' || c == '\t'))));
        Ok(())
    }

    /// Replace every field of this name with one.
    pub fn set(&mut self, name: &str, value: &str) -> Result<(), HeaderError> {
        if !is_token(name) {
            return Err(HeaderError::BadName);
        }
        if !is_valid_value(value) {
            return Err(HeaderError::BadValue);
        }
        if let Some(i) = self.list.iter().position(|(k, _)| k.eq_ignore_ascii_case(name)) {
            self.list[i].1 = String::from(value.trim_matches(|c| c == ' ' || c == '\t'));
            let mut j = i + 1;
            while j < self.list.len() {
                if self.list[j].0.eq_ignore_ascii_case(name) {
                    self.list.remove(j);
                } else {
                    j += 1;
                }
            }
            Ok(())
        } else {
            self.append(name, value)
        }
    }

    /// The first value of `name`.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.list.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    /// Every value of `name`, in order.
    pub fn get_all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.list.iter().filter(move |(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    /// RFC 9110 §5.3: the combined field value (members joined with ", ").
    pub fn get_combined(&self, name: &str) -> Option<String> {
        let mut it = self.get_all(name);
        let first = it.next()?;
        let mut s = String::from(first);
        for v in it {
            s.push_str(", ");
            s.push_str(v);
        }
        Some(s)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    pub fn remove(&mut self, name: &str) {
        self.list.retain(|(k, _)| !k.eq_ignore_ascii_case(name));
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.list.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// Does the comma-separated list field `name` contain `token` (ASCII case-insensitive)?
    pub fn has_token(&self, name: &str, token: &str) -> bool {
        self.get_all(name).any(|v| v.split(',').any(|t| t.trim().eq_ignore_ascii_case(token)))
    }
}
