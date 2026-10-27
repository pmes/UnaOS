// SPDX-License-Identifier: LGPL-3.0-or-later
//! PAGESAVE (LEDGER SR392): MHTML (RFC 2557 `multipart/related`), written here; read here too (PAGESAVECOMPLETE, SR398).
//!
//! CHARTER: Gneiss — the ONE MHTML writer (no third-party crate): a saved web page as one file, the root
//! `text/html` part (quoted-printable, RFC 2045 §6.7) at the page's URL and one part per resource (base64, RFC 2045
//! §6.8, 76-column lines), each named by its `Content-Location` (RFC 2557 §4), so relative references in the root
//! resolve against the root's location and find their parts without rewriting. Aether's Save As (`chrome::host`)
//! writes through it. std-gated with the network face because the base64 encoder it reuses is `api::http::ws::base64`.
//! PAGESAVECOMPLETE (SR398): [`read`] is the ONE reader beside it (base64 through `forge::base64_decode`): Aether's
//! loader opens a `.mhtml` / `.mht` file through it (`chrome::host::open_archive`).

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

// ------------------------------------------------------------------------------------------ PAGESAVECOMPLETE (SR398)

/// The most parts [`read`] takes from one archive.
pub const MAX_PARTS: usize = 4096;

/// An archive back into its parts: the root (the part whose `Content-ID` the top-level `start` names, else the first)
/// and the others in archive order, each body decoded (`Content-Transfer-Encoding` base64, quoted-printable, or 7bit /
/// 8bit / binary as written), its `location` the `Content-Location` and its `content_type` the `Content-Type` as written
/// (empty when absent). CRLF or bare LF line ends; folded fields unfolded; the boundary quoted or not. None for bytes that
/// are not a `multipart/related` archive with at least one part, or with more than [`MAX_PARTS`].
pub fn read(bytes: &[u8]) -> Option<(Part, Vec<Part>)> {
    let (head, body) = split_head(bytes)?;
    let top = fields(head);
    let (essence, params) = media(lookup(&top, "content-type")?);
    if !essence.eq_ignore_ascii_case("multipart/related") {
        return None;
    }
    let boundary = param(&params, "boundary").filter(|b| !b.is_empty())?;
    let start = param(&params, "start").map(|s| s.trim_matches(|c| c == '<' || c == '>').to_string());
    let delim = format!("--{boundary}");
    let mut raw: Vec<&[u8]> = Vec::new();
    let (mut pos, mut open): (usize, Option<usize>) = (0, None);
    let mut closed = false;
    while pos < body.len() {
        let end = body[pos..].iter().position(|&b| b == b'\n').map(|i| pos + i).unwrap_or(body.len());
        let next = (end + 1).min(body.len());
        let line = trim_end(&body[pos..end]);
        if let Some(rest) = line.strip_prefix(delim.as_bytes()) {
            if rest.is_empty() || rest == b"--" {
                if let Some(s) = open {
                    raw.push(&body[s..break_before(body, s, pos)]);
                    if raw.len() > MAX_PARTS {
                        return None;
                    }
                }
                if rest == b"--" {
                    closed = true;
                    break;
                }
                open = Some(next);
            }
        }
        pos = next;
    }
    if let (false, Some(s)) = (closed, open) {
        raw.push(&body[s..break_before(body, s, body.len())]); // an unterminated last part is still a part
    }
    let mut parts: Vec<(String, Part)> = raw.into_iter().map(part).collect();
    if parts.is_empty() || parts.len() > MAX_PARTS {
        return None;
    }
    let root = start.and_then(|cid| parts.iter().position(|(id, _)| *id == cid)).unwrap_or(0);
    let (_, root) = parts.remove(root);
    Some((root, parts.into_iter().map(|(_, p)| p).collect()))
}

/// The header block and the rest: the first empty line (CRLF or LF) ends the fields. An empty first line: no fields.
fn split_head(b: &[u8]) -> Option<(&[u8], &[u8])> {
    let mut pos = 0;
    while pos < b.len() {
        let end = b[pos..].iter().position(|&c| c == b'\n').map(|i| pos + i)?;
        let line = &b[pos..end];
        if line.is_empty() || line == b"\r" {
            return Some((&b[..pos], &b[end + 1..]));
        }
        pos = end + 1;
    }
    None
}

/// Header fields (RFC 5322 §2.2.3: a line opening with white space continues the one before), names lower-cased,
/// values trimmed.
fn fields(head: &[u8]) -> Vec<(String, String)> {
    let text = String::from_utf8_lossy(head);
    let mut out: Vec<(String, String)> = Vec::new();
    for line in text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)) {
        if line.starts_with([' ', '\t']) {
            if let Some((_, v)) = out.last_mut() {
                v.push(' ');
                v.push_str(line.trim());
            }
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            out.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }
    out
}

fn lookup<'a>(f: &'a [(String, String)], name: &str) -> Option<&'a str> {
    f.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

/// A media type's essence and its `name=value` parameters (names lower-cased, quoted values unquoted; a `;` inside
/// quotes does not split).
fn media(v: &str) -> (String, Vec<(String, String)>) {
    let mut pieces: Vec<String> = vec![String::new()];
    let mut quoted = false;
    for c in v.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                pieces.last_mut().unwrap().push(c);
            }
            ';' if !quoted => pieces.push(String::new()),
            _ => pieces.last_mut().unwrap().push(c),
        }
    }
    let essence = pieces[0].trim().to_string();
    let params = pieces[1..]
        .iter()
        .filter_map(|p| {
            let (k, v) = p.split_once('=')?;
            let v = v.trim();
            let v = v.strip_prefix('"').and_then(|x| x.strip_suffix('"')).unwrap_or(v);
            Some((k.trim().to_ascii_lowercase(), v.to_string()))
        })
        .collect();
    (essence, params)
}

fn param(params: &[(String, String)], name: &str) -> Option<String> {
    params.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone())
}

/// A line without its CR and trailing white space (RFC 2046 §5.1.1 allows white space after a delimiter).
fn trim_end(line: &[u8]) -> &[u8] {
    let mut n = line.len();
    while n > 0 && matches!(line[n - 1], b'\r' | b' ' | b'\t') {
        n -= 1;
    }
    &line[..n]
}

/// The end of a part's content: the line break before the delimiter at `at` belongs to the delimiter.
fn break_before(b: &[u8], start: usize, at: usize) -> usize {
    let mut end = at;
    if end > start && b[end - 1] == b'\n' {
        end -= 1;
        if end > start && b[end - 1] == b'\r' {
            end -= 1;
        }
    }
    end
}

/// One part's content: its `Content-ID` (angle brackets dropped) and the decoded [`Part`].
fn part(raw: &[u8]) -> (String, Part) {
    let (head, body) = split_head(raw).unwrap_or((raw, &[]));
    let f = fields(head);
    let cte = lookup(&f, "content-transfer-encoding").unwrap_or("").to_ascii_lowercase();
    let body = match cte.as_str() {
        "base64" => crate::forge::base64_decode(&String::from_utf8_lossy(body)).unwrap_or_default(),
        "quoted-printable" => unqp(body),
        _ => body.to_vec(),
    };
    let id = lookup(&f, "content-id").unwrap_or("").trim_matches(|c| c == '<' || c == '>').to_string();
    let p = Part {
        location: lookup(&f, "content-location").unwrap_or("").to_string(),
        content_type: lookup(&f, "content-type").unwrap_or("").to_string(),
        body,
    };
    (id, p)
}

/// RFC 2045 §6.7 decoding: `=XX` a byte, `=` before a line break (CRLF or LF) a soft break; a malformed `=` stays
/// literal; hard line breaks stay as written.
fn unqp(b: &[u8]) -> Vec<u8> {
    let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'=' {
            if b.get(i + 1) == Some(&b'\r') && b.get(i + 2) == Some(&b'\n') {
                i += 3;
                continue;
            }
            if b.get(i + 1) == Some(&b'\n') {
                i += 2;
                continue;
            }
            if let (Some(h), Some(l)) = (b.get(i + 1).and_then(|&c| hex(c)), b.get(i + 2).and_then(|&c| hex(c))) {
                out.push(h << 4 | l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{read, write, Part, BOUNDARY, LINE};

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
        let head = std::str::from_utf8(&bytes).unwrap().split_once("\r\n\r\n").unwrap().0.to_string();
        assert!(head.contains("MIME-Version: 1.0"));
        assert!(head.contains(&format!("multipart/related; type=\"text/html\"; boundary=\"{BOUNDARY}\"")));
        let (root, got) = read(&bytes).expect("the writer's archive reads back");
        assert_eq!(got.len(), 2, "the root and two parts");
        assert_eq!(root.content_type, "text/html; charset=utf-8");
        assert_eq!(root.location, "https://example.com/page");
        assert_eq!(String::from_utf8(root.body).unwrap().replace("\r\n", "\n"), html, "the root decodes to the page");
        assert_eq!((got[0].content_type.as_str(), got[0].location.as_str()), ("image/png", "https://example.com/a.png"));
        assert_eq!(got[0].body, (0..=255u8).collect::<Vec<u8>>(), "binary bytes come back equal");
        assert_eq!(got[1].content_type, "application/octet-stream", "an empty type is octet-stream");
        assert_eq!(got[1].location, "https://example.com/s.cssX-Evil: 1", "CR/LF cannot open a field");
        assert_eq!(got[1].body, b"p{}");
    }

    /// PAGESAVECOMPLETE (SR398): another writer's shape: bare LF, a folded Content-Type, an unquoted boundary, `start`
    /// naming the second part as the root, a 7bit root and a soft-broken quoted-printable part.
    #[test]
    fn mhtml_read_lf_start() {
        let a = "MIME-Version: 1.0\nContent-Type: multipart/related;\n\ttype=\"text/html\"; boundary=b1; start=\"<root@x>\"\n\n\
preamble\n--b1\nContent-Type: image/gif\nContent-Transfer-Encoding: base64\nContent-Location: https://x.test/g.gif\n\
Content-ID: <g@x>\n\nR0lG\nODlh\n--b1 \nContent-Type: text/html\nContent-Transfer-Encoding: 7bit\nContent-ID: <root@x>\n\
Content-Location: https://x.test/\n\n<p>hi</p>\nline2\n--b1\nContent-Type: text/css\nContent-Transfer-Encoding: quoted-printable\n\
Content-Location: https://x.test/s.css\n\np{color:=\nred}=3D\n--b1--\nepilogue\n";
        let (root, parts) = read(a.as_bytes()).expect("an LF archive");
        assert_eq!((root.location.as_str(), root.content_type.as_str()), ("https://x.test/", "text/html"), "`start` picks the root");
        assert_eq!(root.body, b"<p>hi</p>\nline2", "7bit as written; the break before the delimiter is the delimiter's");
        assert_eq!(parts.len(), 2);
        assert_eq!((parts[0].location.as_str(), parts[0].body.as_slice()), ("https://x.test/g.gif", &b"GIF89a"[..]));
        assert_eq!(parts[1].body, b"p{color:red}=", "a soft LF break joins; =3D is '='");
        assert!(read(b"Content-Type: text/html\n\n<p>x</p>").is_none(), "not multipart/related");
        assert!(read(b"no header block").is_none());
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
