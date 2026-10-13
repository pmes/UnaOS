//! AETHERWPT3 (LEDGER SR138): HTML §13.2.3 "the input byte stream" — determining a document's character encoding
//! (§13.2.3.2: the BOM, the transport layer's `charset`, the `<meta>` prescan of the first 1024 bytes, the default)
//! and decoding it (Encoding §6 "decode", the legacy decoders of `api::encoding`). The parser itself (html_core)
//! takes text; this is what turns a response's bytes into that text for a nested document (api::fetch's
//! `fetch_subdocument`) and names the result for `document.characterSet`.

use crate::api::encoding;

/// The encoding used when nothing names one (§13.2.3.2 step 9, "the implementation-defined default": the one for
/// this engine's single locale, en, per the table there).
pub const DEFAULT: &str = "windows-1252";

fn is_space(b: u8) -> bool {
    matches!(b, 0x09 | 0x0A | 0x0C | 0x0D | 0x20)
}

/// The `charset` parameter of a `Content-Type` value (MIME Sniffing §4.4 parameters, quotes removed), as an
/// encoding.
pub fn transport_charset(content_type: &str) -> Option<&'static str> {
    for param in content_type.split(';').skip(1) {
        let (k, v) = param.split_once('=')?;
        if k.trim().eq_ignore_ascii_case("charset") {
            let v = v.trim().trim_matches('"');
            return encoding::get_encoding(v);
        }
    }
    None
}

/// §2.6.? "the algorithm for extracting a character encoding from a meta element" (the `content` attribute).
pub fn charset_from_content(s: &str) -> Option<&'static str> {
    let b = s.as_bytes();
    let mut pos = 0usize;
    loop {
        let rest = &s[pos..];
        let at = rest.to_ascii_lowercase().find("charset")?;
        pos += at + 7;
        while pos < b.len() && is_space(b[pos]) {
            pos += 1;
        }
        if pos >= b.len() || b[pos] != b'=' {
            continue;
        }
        pos += 1;
        while pos < b.len() && is_space(b[pos]) {
            pos += 1;
        }
        if pos >= b.len() {
            return None;
        }
        let q = b[pos];
        if q == b'"' || q == b'\'' {
            let tail = &s[pos + 1..];
            let end = tail.find(q as char)?;
            return encoding::get_encoding(&tail[..end]);
        }
        let tail = &s[pos..];
        let end = tail.find(|c: char| is_space(c as u32 as u8) && c.is_ascii() || c == ';').unwrap_or(tail.len());
        return encoding::get_encoding(&tail[..end]);
    }
}

/// §13.2.3.2 "get an attribute": (name, value), or None at `>` / the end.
fn get_attribute(b: &[u8], pos: &mut usize) -> Option<(Vec<u8>, Vec<u8>)> {
    while *pos < b.len() && (is_space(b[*pos]) || b[*pos] == b'/') {
        *pos += 1;
    }
    if *pos >= b.len() || b[*pos] == b'>' {
        return None;
    }
    let mut name = Vec::new();
    let mut value = Vec::new();
    loop {
        let c = *b.get(*pos)?;
        if c == b'=' && !name.is_empty() {
            *pos += 1;
            break;
        } else if is_space(c) {
            while *pos < b.len() && is_space(b[*pos]) {
                *pos += 1;
            }
            if b.get(*pos) != Some(&b'=') {
                return Some((name, value));
            }
            *pos += 1;
            break;
        } else if c == b'/' || c == b'>' {
            return Some((name, value));
        } else {
            name.push(c.to_ascii_lowercase());
        }
        *pos += 1;
    }
    while *pos < b.len() && is_space(b[*pos]) {
        *pos += 1;
    }
    let c = *b.get(*pos)?;
    if c == b'"' || c == b'\'' {
        loop {
            *pos += 1;
            let x = *b.get(*pos)?;
            if x == c {
                *pos += 1;
                return Some((name, value));
            }
            value.push(x.to_ascii_lowercase());
        }
    }
    if c == b'>' {
        return Some((name, value));
    }
    value.push(c.to_ascii_lowercase());
    *pos += 1;
    loop {
        let x = *b.get(*pos)?;
        if is_space(x) || x == b'>' {
            return Some((name, value));
        }
        value.push(x.to_ascii_lowercase());
        *pos += 1;
    }
}

fn starts_ci(b: &[u8], at: usize, pat: &[u8]) -> bool {
    b.len() >= at + pat.len() && b[at..at + pat.len()].eq_ignore_ascii_case(pat)
}

/// §13.2.3.2 "prescan a byte stream to determine its encoding" over the first 1024 bytes.
pub fn prescan(bytes: &[u8]) -> Option<&'static str> {
    let b = &bytes[..bytes.len().min(1024)];
    let mut pos = 0usize;
    while pos < b.len() {
        if b[pos..].starts_with(b"<!--") {
            // to the first '>' preceded by "--" (the dashes of "<!--" count)
            let mut p = pos + 2;
            loop {
                p += 1;
                if p >= b.len() {
                    return None;
                }
                if b[p] == b'>' && b[p - 1] == b'-' && b[p - 2] == b'-' {
                    break;
                }
            }
            pos = p + 1;
            continue;
        }
        if starts_ci(b, pos, b"<meta") && b.get(pos + 5).is_some_and(|&c| is_space(c) || c == b'/') {
            let mut p = pos + 6;
            let mut seen: Vec<Vec<u8>> = Vec::new();
            let (mut got_pragma, mut need_pragma, mut charset): (bool, Option<bool>, Option<&'static str>) = (false, None, None);
            while let Some((name, value)) = get_attribute(b, &mut p) {
                if seen.contains(&name) {
                    continue;
                }
                seen.push(name.clone());
                let v = String::from_utf8_lossy(&value).into_owned();
                match name.as_slice() {
                    b"http-equiv" => {
                        if v == "content-type" {
                            got_pragma = true;
                        }
                    }
                    b"content" => {
                        if charset.is_none() {
                            if let Some(e) = charset_from_content(&v) {
                                charset = Some(e);
                                need_pragma = Some(true);
                            }
                        }
                    }
                    b"charset" => {
                        charset = encoding::get_encoding(&v);
                        need_pragma = Some(false);
                    }
                    _ => {}
                }
            }
            if p >= b.len() {
                return None;
            }
            let decided = match (need_pragma, charset) {
                (None, _) => None,
                (Some(true), _) if !got_pragma => None,
                (_, None) => None,
                (_, Some(c)) => Some(c),
            };
            if let Some(c) = decided {
                return Some(match c {
                    "UTF-16BE" | "UTF-16LE" => "UTF-8",
                    "x-user-defined" => "windows-1252",
                    other => other,
                });
            }
            pos = p + 1;
            continue;
        }
        let tagish = b[pos] == b'<'
            && (b.get(pos + 1).is_some_and(|c| c.is_ascii_alphabetic())
                || (b.get(pos + 1) == Some(&b'/') && b.get(pos + 2).is_some_and(|c| c.is_ascii_alphabetic())));
        if tagish {
            let mut p = pos + 1;
            while p < b.len() && !is_space(b[p]) && b[p] != b'>' {
                p += 1;
            }
            while get_attribute(b, &mut p).is_some() {}
            if p >= b.len() {
                return None;
            }
            pos = p + 1;
            continue;
        }
        if b[pos..].starts_with(b"<!") || b[pos..].starts_with(b"</") || b[pos..].starts_with(b"<?") {
            match b[pos + 1..].iter().position(|&c| c == b'>') {
                Some(off) => pos = pos + 1 + off + 1,
                None => return None,
            }
            continue;
        }
        pos += 1;
    }
    None
}

/// §13.2.3.2 for an HTML document: the BOM, then the transport layer, then the prescan, then [`DEFAULT`] — and the
/// decoded text. `content_type` is the whole `Content-Type` value (its `charset` parameter is the transport's).
pub fn decode_html(bytes: &[u8], content_type: &str) -> (String, &'static str) {
    let fallback = transport_charset(content_type).or_else(|| prescan(bytes)).unwrap_or(DEFAULT);
    encoding::decode(bytes, fallback)
}

/// A non-HTML text resource (plain text, XML as this engine shows it): the BOM, the transport's `charset`, else
/// UTF-8.
pub fn decode_text(bytes: &[u8], content_type: &str) -> (String, &'static str) {
    encoding::decode(bytes, transport_charset(content_type).unwrap_or("UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniff_order() {
        assert_eq!(prescan(b"<!-- <meta charset=koi8-r> --><meta charset=\"shift_jis\">"), Some("Shift_JIS"));
        assert_eq!(prescan(b"<meta http-equiv=Content-Type content='text/html; charset=euc-jp'>"), Some("EUC-JP"));
        assert_eq!(prescan(b"<meta content='text/html; charset=euc-jp'>"), None, "content without the pragma");
        assert_eq!(prescan(b"<meta charset=utf-16le>"), Some("UTF-8"));
        assert_eq!(prescan(b"<title>x</title><meta charset=gbk>"), Some("GBK"));
        let (t, e) = decode_html(&[0x82, 0xA0], "text/html; charset=Shift_JIS");
        assert_eq!((t.as_str(), e), ("あ", "Shift_JIS"));
        let (t, e) = decode_html(b"<meta charset=iso-8859-5>\xd0", "text/html");
        assert_eq!((t.ends_with('а'), e), (true, "ISO-8859-5"));
        assert_eq!(decode_html(b"\xe9", "text/html"), ("é".to_string(), "windows-1252"));
    }
}
