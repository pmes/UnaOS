// SPDX-License-Identifier: LGPL-3.0-or-later
//! PAGESAVE (LEDGER SR392): MHTML (RFC 2557 `multipart/related`), written here.
//!
//! CHARTER: Gneiss — the ONE MHTML writer (no third-party crate): a saved web page as one file, the root
//! `text/html` part (quoted-printable, RFC 2045 §6.7) at the page's URL and one part per resource (base64, RFC 2045
//! §6.8, 76-column lines), each named by its `Content-Location` (RFC 2557 §4), so relative references in the root
//! resolve against the root's location and find their parts without rewriting. Aether's Save As (`chrome::host`)
//! writes through it. std-gated with the network face because the base64 encoder it reuses is `api::http::ws::base64`.
//! Owed: an MHTML reader (opening a `.mhtml`).

/// One resource of the page: its absolute URL, its media type and its bytes as fetched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    pub location: String,
    pub content_type: String,
    pub body: Vec<u8>,
}

/// The boundary. It carries `=_`, which neither quoted-printable (a literal `=` is always `=XX` or a soft break) nor
/// base64 output can contain, so it never occurs inside a part.
pub const BOUNDARY: &str = "----=_NextPart_UnaOS_PageSave";

/// The longest encoded line (RFC 2045: 76 characters, the CRLF not counted).
const LINE: usize = 76;

/// The archive: `root_url`'s page `html` as the root part, then each of `parts`, CRLF line ends throughout.
pub fn write(root_url: &str, html: &str, parts: &[Part]) -> Vec<u8> {
    let mut out = String::new();
    out.push_str("MIME-Version: 1.0\r\n");
    out.push_str(&format!("Content-Type: multipart/related; type=\"text/html\"; boundary=\"{BOUNDARY}\"\r\n\r\n"));
    out.push_str(&format!("--{BOUNDARY}\r\n"));
    out.push_str("Content-Type: text/html; charset=utf-8\r\n");
    out.push_str("Content-Transfer-Encoding: quoted-printable\r\n");
    out.push_str(&format!("Content-Location: {}\r\n\r\n", field(root_url)));
    out.push_str(&qp(html.as_bytes()));
    out.push_str("\r\n");
    for p in parts {
        out.push_str(&format!("--{BOUNDARY}\r\n"));
        let ty = field(&p.content_type);
        out.push_str(&format!("Content-Type: {}\r\n", if ty.is_empty() { "application/octet-stream" } else { &ty }));
        out.push_str("Content-Transfer-Encoding: base64\r\n");
        out.push_str(&format!("Content-Location: {}\r\n\r\n", field(&p.location)));
        let b64 = crate::api::http::ws::base64(&p.body);
        for chunk in b64.as_bytes().chunks(LINE) {
            out.push_str(core::str::from_utf8(chunk).unwrap_or_default());
            out.push_str("\r\n");
        }
    }
    out.push_str(&format!("--{BOUNDARY}--\r\n"));
    out.into_bytes()
}

/// A header field value: control bytes (CR / LF among them) dropped, so a value never ends the field early.
fn field(v: &str) -> String {
    v.chars().filter(|c| !c.is_control()).collect::<String>().trim().to_string()
}

/// RFC 2045 §6.7 quoted-printable: `\n` (with a `\r` before it folded in) is a hard line break (CRLF); printable
/// ASCII other than `=` stays literal, a space or tab too except before a line break or at the end; every other byte
/// is `=XX`; soft breaks (`=` CRLF) keep each line at most 76 characters.
fn qp(data: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(data.len() + data.len() / 8);
    let mut col = 0usize;
    let mut i = 0usize;
    while i < data.len() {
        let b = data[i];
        if b == b'\n' || (b == b'\r' && data.get(i + 1) == Some(&b'\n')) {
            out.push_str("\r\n");
            col = 0;
            i += if b == b'\r' { 2 } else { 1 };
            continue;
        }
        let at_end = match data.get(i + 1) {
            None => true,
            Some(b'\n') => true,
            Some(b'\r') => data.get(i + 2) == Some(&b'\n'),
            _ => false,
        };
        let literal = (b == b' ' || b == b'\t') && !at_end || (33..=126).contains(&b) && b != b'=';
        let enc_len = if literal { 1 } else { 3 };
        // Room for the token plus a trailing soft-break `=` unless the token ends the line.
        let need = if at_end { enc_len } else { enc_len + 1 };
        if col + need > LINE {
            out.push_str("=\r\n");
            col = 0;
        }
        if literal {
            out.push(b as char);
        } else {
            out.push('=');
            out.push(HEX[(b >> 4) as usize] as char);
            out.push(HEX[(b & 15) as usize] as char);
        }
        col += enc_len;
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{write, Part, BOUNDARY, LINE};

    /// A minimal reader (the test's own; the reader is owed): the top-level fields, then per part its fields and its
    /// decoded body.
    fn read(bytes: &[u8]) -> (String, Vec<(Vec<(String, String)>, Vec<u8>)>) {
        let s = std::str::from_utf8(bytes).expect("an archive is ASCII");
        let (head, body) = s.split_once("\r\n\r\n").expect("a header block");
        let delim = format!("--{BOUNDARY}");
        let body = body.split(&format!("{delim}--")).next().unwrap();
        let mut parts = Vec::new();
        for chunk in body.split(&delim).skip(1) {
            let chunk = chunk.strip_prefix("\r\n").unwrap();
            let (fh, fb) = chunk.split_once("\r\n\r\n").unwrap();
            let fields: Vec<(String, String)> = fh
                .split("\r\n")
                .map(|l| {
                    let (k, v) = l.split_once(':').unwrap();
                    (k.trim().to_ascii_lowercase(), v.trim().to_string())
                })
                .collect();
            let cte = fields.iter().find(|(k, _)| k == "content-transfer-encoding").map(|(_, v)| v.clone()).unwrap();
            let fb = fb.strip_suffix("\r\n").unwrap();
            let data = match cte.as_str() {
                "base64" => crate::forge::base64_decode(fb).expect("valid base64"),
                "quoted-printable" => unqp(fb),
                other => panic!("unknown encoding {other}"),
            };
            parts.push((fields, data));
        }
        (head.to_string(), parts)
    }

    fn unqp(s: &str) -> Vec<u8> {
        let b = s.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'=' {
                if b.get(i + 1) == Some(&b'\r') && b.get(i + 2) == Some(&b'\n') {
                    i += 3;
                    continue;
                }
                let h = std::str::from_utf8(&b[i + 1..i + 3]).unwrap();
                out.push(u8::from_str_radix(h, 16).unwrap());
                i += 3;
            } else {
                out.push(b[i]);
                i += 1;
            }
        }
        out
    }

    fn get<'a>(f: &'a [(String, String)], k: &str) -> &'a str {
        f.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str()).unwrap_or("")
    }

    fn sample() -> (String, Vec<Part>) {
        let html = format!(
            "<!DOCTYPE html><html><head><title>Ünïcode = test</title></head>\n<body>\n<p>trailing spaces   \n{}</p>\t\n<img src=\"a.png\"></body></html>",
            "x".repeat(200)
        );
        let parts = vec![
            Part { location: "https://example.com/a.png".into(), content_type: "image/png".into(), body: (0..=255u8).collect() },
            Part { location: "https://example.com/s.css\r\nX-Evil: 1".into(), content_type: String::new(), body: b"p{}".to_vec() },
        ];
        (html, parts)
    }

    #[test]
    fn mhtml_round_trip() {
        let (html, parts) = sample();
        let bytes = write("https://example.com/page", &html, &parts);
        let (head, got) = read(&bytes);
        assert!(head.contains("MIME-Version: 1.0"));
        assert!(head.contains(&format!("multipart/related; type=\"text/html\"; boundary=\"{BOUNDARY}\"")));
        assert_eq!(got.len(), 3, "the root and two parts");
        let (f, d) = &got[0];
        assert_eq!(get(f, "content-type"), "text/html; charset=utf-8");
        assert_eq!(get(f, "content-location"), "https://example.com/page");
        assert_eq!(String::from_utf8(d.clone()).unwrap().replace("\r\n", "\n"), html, "the root decodes to the page");
        let (f, d) = &got[1];
        assert_eq!((get(f, "content-type"), get(f, "content-location")), ("image/png", "https://example.com/a.png"));
        assert_eq!(*d, (0..=255u8).collect::<Vec<u8>>(), "binary bytes come back equal");
        let (f, d) = &got[2];
        assert_eq!(get(f, "content-type"), "application/octet-stream", "an empty type is octet-stream");
        assert_eq!(get(f, "content-location"), "https://example.com/s.cssX-Evil: 1", "CR/LF cannot open a field");
        assert!(f.iter().all(|(k, _)| k != "x-evil"));
        assert_eq!(d, b"p{}");
    }

    #[test]
    fn mhtml_lines_short() {
        let (html, parts) = sample();
        let bytes = write("https://example.com/page", &html, &parts);
        let s = String::from_utf8(bytes).unwrap();
        assert!(s.ends_with(&format!("--{BOUNDARY}--\r\n")));
        let body = s.split_once("\r\n\r\n").unwrap().1;
        for l in body.split("\r\n") {
            assert!(l.len() <= LINE, "line of {}: {l:?}", l.len());
            assert!(!l.contains('\n') && !l.contains('\r'), "CRLF only");
            assert!(!l.ends_with(' ') && !l.ends_with('\t'), "no trailing white space: {l:?}");
        }
        assert!(s.is_ascii(), "the archive is 7-bit");
    }
}
