//! Downloads (std-free): when a response is a download, what the file is called, and the list of
//! downloads the chrome shows as progress rows.
//!
//! A top-level navigation's response is DOWNLOADED rather than rendered when (HTML §7.4.2.3 "navigate"
//! → "the response is to be downloaded"; Fetch's MIME handling) its `Content-Disposition` is
//! `attachment` (RFC 6266 §4.2), or its `Content-Type` is not a type Aether renders as a document
//! ([`renderable`]). The file name is RFC 6266's: `filename*` (RFC 8187 ext-value, UTF-8) before
//! `filename`, else the URL path's last segment, percent-decoded — then [`sanitize`]d so a hostile
//! server can never name a path outside the Downloads folder, and made [`unique_name`] against what is
//! already there.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// RFC 6266 §4.2: the disposition type is `attachment` (case-insensitive).
pub fn is_attachment(content_disposition: Option<&str>) -> bool {
    content_disposition
        .and_then(|v| v.split(';').next())
        .is_some_and(|t| t.trim().eq_ignore_ascii_case("attachment"))
}

/// Whether a response of `content_type` is a document Aether renders. Absent (or empty) is rendered —
/// the fetcher's text decode is the sniff.
pub fn renderable(content_type: Option<&str>) -> bool {
    let Some(ct) = content_type else { return true };
    let essence = ct.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    essence.is_empty()
        || matches!(
            essence.as_str(),
            "text/html" | "application/xhtml+xml" | "text/plain" | "text/xml" | "application/xml" | "image/svg+xml"
        )
}

/// The suggested file name for a download of `url` with this `Content-Disposition`.
pub fn file_name(content_disposition: Option<&str>, url: &str) -> String {
    let mut star: Option<String> = None;
    let mut plain: Option<String> = None;
    if let Some(cd) = content_disposition {
        for param in split_params(cd).into_iter().skip(1) {
            let Some((k, v)) = param.split_once('=') else { continue };
            let k = k.trim().to_ascii_lowercase();
            let v = v.trim();
            if k == "filename*" {
                // ext-value = charset "'" [ language ] "'" value-chars
                let mut parts = v.splitn(3, '\'');
                let (cs, _lang, val) = (parts.next().unwrap_or(""), parts.next(), parts.next());
                if let Some(val) = val {
                    if cs.eq_ignore_ascii_case("utf-8") || cs.eq_ignore_ascii_case("iso-8859-1") {
                        star = Some(percent_decode(val));
                    }
                }
            } else if k == "filename" {
                plain = Some(unquote(v));
            }
        }
    }
    let from_url = || {
        let path = url.split(['?', '#']).next().unwrap_or("");
        let path = path.split_once("://").map(|(_, rest)| rest.split_once('/').map(|(_, p)| p).unwrap_or("")).unwrap_or(path);
        percent_decode(path.rsplit('/').next().unwrap_or(""))
    };
    let name = sanitize(&star.or(plain).unwrap_or_else(from_url));
    if name.is_empty() { "download".to_string() } else { name }
}

/// Split a header value on `;` outside quoted strings.
fn split_params(v: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut quoted, mut escaped) = (false, false);
    for c in v.chars() {
        if escaped {
            cur.push(c);
            escaped = false;
            continue;
        }
        match c {
            '\\' if quoted => {
                cur.push(c);
                escaped = true;
            }
            '"' => {
                quoted = !quoted;
                cur.push(c);
            }
            ';' if !quoted => out.push(core::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

fn unquote(v: &str) -> String {
    let v = v.trim();
    if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
        let mut out = String::new();
        let mut esc = false;
        for c in v[1..v.len() - 1].chars() {
            if esc {
                out.push(c);
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else {
                out.push(c);
            }
        }
        out
    } else {
        v.to_string()
    }
}

/// `%XX` → byte; the bytes are read as UTF-8 (lossily).
pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A name that is one file in one folder: no path separators, no control characters, no leading dots
/// (no `..`, no hidden files), trimmed, at most 200 bytes (on a character boundary).
pub fn sanitize(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| if c == '/' || c == '\\' || c == ':' || c.is_control() { '_' } else { c })
        .collect();
    s = s.trim().trim_start_matches('.').trim().to_string();
    if s.len() > 200 {
        let mut cut = 200;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
    }
    s
}

/// `name`, or `stem (n).ext` for the first `n` whose name `exists` says is free.
pub fn unique_name(name: &str, exists: impl Fn(&str) -> bool) -> String {
    if !exists(name) {
        return name.to_string();
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    let mut n = 1u32;
    loop {
        let candidate = alloc::format!("{stem} ({n}){ext}");
        if !exists(&candidate) || n == u32::MAX {
            return candidate;
        }
        n += 1;
    }
}

/// One download, as its progress row shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Download {
    pub id: u64,
    pub name: String,
    pub url: String,
    pub received: u64,
    pub total: Option<u64>,
    pub done: bool,
    pub error: Option<String>,
}

impl Download {
    pub fn new(id: u64, name: &str, url: &str, total: Option<u64>) -> Self {
        Self { id, name: name.to_string(), url: url.to_string(), received: 0, total, done: false, error: None }
    }

    /// Progress in 0..=1, when the length is known.
    pub fn fraction(&self) -> Option<f64> {
        let t = self.total.filter(|&t| t > 0)?;
        Some((self.received as f64 / t as f64).clamp(0.0, 1.0))
    }
}

/// The downloads of this session, oldest first.
#[derive(Clone, Debug, Default)]
pub struct DownloadList {
    items: Vec<Download>,
}

impl DownloadList {
    pub fn items(&self) -> &[Download] {
        &self.items
    }

    /// Insert or replace the row with `d.id`.
    pub fn upsert(&mut self, d: Download) {
        match self.items.iter_mut().find(|x| x.id == d.id) {
            Some(x) => *x = d,
            None => self.items.push(d),
        }
    }

    pub fn get(&self, id: u64) -> Option<&Download> {
        self.items.iter().find(|d| d.id == id)
    }

    /// How many are still running.
    pub fn active(&self) -> usize {
        self.items.iter().filter(|d| !d.done).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disposition_and_type_decide_a_download() {
        assert!(is_attachment(Some("Attachment; filename=a.txt")));
        assert!(!is_attachment(Some("inline")));
        assert!(!is_attachment(None));
        assert!(renderable(Some("text/html; charset=utf-8")));
        assert!(renderable(None));
        assert!(!renderable(Some("application/zip")));
    }

    #[test]
    fn the_file_name_is_rfc6266s_and_cannot_escape_the_folder() {
        assert_eq!(file_name(Some("attachment; filename=\"a b.txt\""), "https://x/y"), "a b.txt");
        assert_eq!(file_name(Some("attachment; filename=\"x.txt\"; filename*=UTF-8''%C3%A9t%C3%A9.txt"), "https://x/y"), "été.txt");
        assert_eq!(file_name(None, "https://x.test/dir/una%20os.iso?x=1"), "una os.iso");
        assert_eq!(file_name(Some("attachment; filename=\"../../etc/passwd\""), "https://x/"), "_.._etc_passwd");
        assert_eq!(file_name(None, "https://x.test/"), "download");
        assert_eq!(unique_name("a.txt", |n| n == "a.txt" || n == "a (1).txt"), "a (2).txt");
        assert_eq!(percent_decode("100%"), "100%");
    }
}
