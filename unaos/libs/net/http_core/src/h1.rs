//! HTTP/1.1 message syntax and framing (RFC 9112): the request line and head we send (§3, §5), the status
//! line and field section we parse (§4, §5), the message body length rules (§6.3), chunked transfer coding
//! with chunk extensions and trailers (§7.1), and connection persistence (§9.3). Push parsers: bytes go in
//! as they arrive, split anywhere; the parsers say how much they consumed, so whatever follows a message on
//! a persistent connection stays in the caller's buffer.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

use crate::headers::{is_tchar, is_token, is_valid_value, Headers};

/// Upper bounds on what a peer can make us hold: the whole head, and the number of field lines.
pub const MAX_HEAD: usize = 64 * 1024;
pub const MAX_FIELDS: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum H1Error {
    /// The status line is not `HTTP/x.y SP 3DIGIT SP reason`.
    BadStatusLine,
    /// A field line without a colon, a bad field name, whitespace before the colon (§5.1), or a CR/NUL
    /// inside a value.
    BadField,
    /// The head is larger than [`MAX_HEAD`] or has more than [`MAX_FIELDS`] fields.
    HeadTooLarge,
    /// §6.3 rule 5: Content-Length is not a number, or its list members differ.
    BadContentLength,
    /// A transfer coding other than `chunked` (we decline to guess at the length).
    UnsupportedTransferCoding,
    /// §7.1: a chunk size that is not hex, overflows, or a chunk not followed by CRLF.
    BadChunk,
    /// The method or request-target cannot be put on the wire.
    BadRequest,
    /// The peer closed before the message was complete.
    Truncated,
}

impl fmt::Display for H1Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            H1Error::BadStatusLine => "malformed status line",
            H1Error::BadField => "malformed header field",
            H1Error::HeadTooLarge => "response head too large",
            H1Error::BadContentLength => "invalid Content-Length",
            H1Error::UnsupportedTransferCoding => "unsupported transfer coding",
            H1Error::BadChunk => "malformed chunked body",
            H1Error::BadRequest => "request cannot be encoded",
            H1Error::Truncated => "connection closed mid-message",
        })
    }
}

// ---------------------------------------------------------------- request head (§3)

/// Encode a request head. `target` is the request-target (origin-form, or authority-form for CONNECT);
/// `host` the Host field value (§7.2 of RFC 9110: first, always). `body_len`: `Some(n)` sends
/// `Content-Length: n` unless the caller already set one; `None` sends no length (the caller set
/// `Transfer-Encoding: chunked` or there is no body for this method).
pub fn encode_request_head(
    method: &str,
    target: &str,
    host: &str,
    headers: &Headers,
    body_len: Option<usize>,
) -> Result<Vec<u8>, H1Error> {
    if !is_token(method) || target.is_empty() || target.bytes().any(|b| b <= b' ' || b == 0x7F) {
        return Err(H1Error::BadRequest);
    }
    if !is_valid_value(host) {
        return Err(H1Error::BadRequest);
    }
    let mut out = Vec::with_capacity(256);
    out.extend_from_slice(method.as_bytes());
    out.push(b' ');
    out.extend_from_slice(target.as_bytes());
    out.extend_from_slice(b" HTTP/1.1\r\nHost: ");
    out.extend_from_slice(host.as_bytes());
    out.extend_from_slice(b"\r\n");
    for (k, v) in headers.iter() {
        if k.eq_ignore_ascii_case("host") {
            continue;
        }
        out.extend_from_slice(k.as_bytes());
        out.extend_from_slice(b": ");
        out.extend_from_slice(v.as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    if let Some(n) = body_len {
        if !headers.contains("content-length") && !headers.contains("transfer-encoding") {
            out.extend_from_slice(b"Content-Length: ");
            out.extend_from_slice(n.to_string().as_bytes());
            out.extend_from_slice(b"\r\n");
        }
    }
    out.extend_from_slice(b"\r\n");
    Ok(out)
}

/// One chunk of a chunked request body (§7.1); an empty `data` is the last-chunk plus the empty trailer.
pub fn encode_chunk(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 12);
    let mut hex = [0u8; 16];
    let mut n = data.len();
    let mut i = hex.len();
    loop {
        i -= 1;
        hex[i] = b"0123456789abcdef"[n & 15];
        n >>= 4;
        if n == 0 {
            break;
        }
    }
    out.extend_from_slice(&hex[i..]);
    out.extend_from_slice(b"\r\n");
    out.extend_from_slice(data);
    out.extend_from_slice(b"\r\n");
    if data.is_empty() {
        out.extend_from_slice(b"\r\n");
    }
    out
}

// ---------------------------------------------------------------- response head (§4, §5)

/// A parsed status line + field section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseHead {
    /// (major, minor) — `HTTP/1.1` is (1, 1).
    pub version: (u8, u8),
    pub status: u16,
    pub reason: String,
    pub headers: Headers,
}

/// Find the end of a head: the index just past the empty line. Lines end in CRLF, or a bare LF (§2.2 lets
/// a recipient accept one).
fn head_end(buf: &[u8]) -> Option<usize> {
    let mut i = 0;
    while let Some(off) = buf[i..].iter().position(|&b| b == b'\n') {
        let nl = i + off;
        // An empty line: "\n" right after the previous line's "\n", or "\r\n" right after it.
        if nl == 0 || buf[nl - 1] == b'\n' || (nl >= 2 && buf[nl - 1] == b'\r' && buf[nl - 2] == b'\n') {
            return Some(nl + 1);
        }
        i = nl + 1;
    }
    None
}

fn lines(head: &[u8]) -> impl Iterator<Item = &[u8]> {
    head.split(|&b| b == b'\n').map(|l| l.strip_suffix(b"\r").unwrap_or(l))
}

/// Parse a response head from the front of `buf`. `Ok(None)`: not complete yet. `Ok(Some((head, n)))`: the
/// head occupied `buf[..n]`.
pub fn parse_response_head(buf: &[u8]) -> Result<Option<(ResponseHead, usize)>, H1Error> {
    let end = match head_end(buf) {
        Some(e) => e,
        None => {
            return if buf.len() > MAX_HEAD { Err(H1Error::HeadTooLarge) } else { Ok(None) };
        }
    };
    if end > MAX_HEAD {
        return Err(H1Error::HeadTooLarge);
    }
    let mut it = lines(&buf[..end]);
    let sl = it.next().ok_or(H1Error::BadStatusLine)?;
    // status-line = HTTP-version SP status-code SP [ reason-phrase ]
    if sl.len() < 12 || &sl[..5] != b"HTTP/" || !sl[5].is_ascii_digit() || sl[6] != b'.' || !sl[7].is_ascii_digit() || sl[8] != b' ' {
        return Err(H1Error::BadStatusLine);
    }
    let version = (sl[5] - b'0', sl[7] - b'0');
    let code = &sl[9..12];
    if !code.iter().all(u8::is_ascii_digit) || (sl.len() > 12 && sl[12] != b' ') {
        return Err(H1Error::BadStatusLine);
    }
    let status = (code[0] - b'0') as u16 * 100 + (code[1] - b'0') as u16 * 10 + (code[2] - b'0') as u16;
    if status < 100 {
        return Err(H1Error::BadStatusLine);
    }
    let reason = if sl.len() > 13 { String::from_utf8_lossy(&sl[13..]).into_owned() } else { String::new() };
    let headers = parse_fields(it)?;
    Ok(Some((ResponseHead { version, status, reason, headers }, end)))
}

/// Parse field lines up to (and ignoring) the empty line. obs-fold (§5.2): a user agent MUST replace it with
/// SP before interpreting the value — done here; a fold before the first field is malformed.
fn parse_fields<'a>(it: impl Iterator<Item = &'a [u8]>) -> Result<Headers, H1Error> {
    let mut raw: Vec<(String, String)> = Vec::new();
    for line in it {
        if line.is_empty() {
            break;
        }
        if line[0] == b' ' || line[0] == b'\t' {
            let last = raw.last_mut().ok_or(H1Error::BadField)?;
            let cont = trim_ows(line);
            if cont.iter().any(|&b| b == b'\r' || b == 0) {
                return Err(H1Error::BadField);
            }
            last.1.push(' ');
            last.1.push_str(&String::from_utf8_lossy(cont));
            continue;
        }
        let colon = line.iter().position(|&b| b == b':').ok_or(H1Error::BadField)?;
        let name = &line[..colon];
        if name.is_empty() || !name.iter().all(|&b| is_tchar(b)) {
            return Err(H1Error::BadField);
        }
        let value = trim_ows(&line[colon + 1..]);
        if value.iter().any(|&b| b == b'\r' || b == 0) {
            return Err(H1Error::BadField);
        }
        if raw.len() >= MAX_FIELDS {
            return Err(H1Error::HeadTooLarge);
        }
        // Field values are octets; non-ASCII is kept as Latin-1-ish lossless text via lossy UTF-8.
        raw.push((String::from_utf8_lossy(name).into_owned(), String::from_utf8_lossy(value).into_owned()));
    }
    let mut h = Headers::new();
    for (k, v) in raw {
        h.append(&k, &v).map_err(|_| H1Error::BadField)?;
    }
    Ok(h)
}

fn trim_ows(mut s: &[u8]) -> &[u8] {
    while let [b' ' | b'\t', rest @ ..] = s {
        s = rest;
    }
    while let [rest @ .., b' ' | b'\t'] = s {
        s = rest;
    }
    s
}

// ---------------------------------------------------------------- §6.3 message body length

/// How the body of a response is delimited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Framing {
    /// No body at all (HEAD, 1xx, 204, 304, a 2xx to CONNECT).
    None,
    /// Exactly this many octets.
    Length(u64),
    /// Chunked transfer coding.
    Chunked,
    /// Everything until the server closes the connection.
    Close,
}

/// RFC 9112 §6.3, for a response to a request with method `method`.
pub fn response_framing(method: &str, head: &ResponseHead) -> Result<Framing, H1Error> {
    let s = head.status;
    if method.eq_ignore_ascii_case("HEAD") || (100..200).contains(&s) || s == 204 || s == 304 {
        return Ok(Framing::None);
    }
    if method.eq_ignore_ascii_case("CONNECT") && (200..300).contains(&s) {
        return Ok(Framing::None);
    }
    if head.headers.contains("transfer-encoding") {
        let codings: Vec<String> = head
            .headers
            .get_all("transfer-encoding")
            .flat_map(|v| v.split(','))
            .map(|t| t.trim().to_ascii_lowercase())
            .filter(|t| !t.is_empty())
            .collect();
        return match codings.last().map(String::as_str) {
            Some("chunked") if codings.iter().filter(|c| c.as_str() == "chunked").count() == 1 => {
                if codings.iter().all(|c| c == "chunked" || c == "identity") {
                    Ok(Framing::Chunked)
                } else {
                    Err(H1Error::UnsupportedTransferCoding)
                }
            }
            Some("chunked") => Err(H1Error::BadChunk),
            // §6.3 rule 3: a response whose final coding is not chunked is close-delimited — but its body is
            // still transfer-coded and we do not decode those codings.
            _ => Err(H1Error::UnsupportedTransferCoding),
        };
    }
    if head.headers.contains("content-length") {
        let mut len: Option<u64> = None;
        for v in head.headers.get_all("content-length") {
            for member in v.split(',') {
                let m = member.trim();
                if m.is_empty() || !m.bytes().all(|b| b.is_ascii_digit()) || m.len() > 19 {
                    return Err(H1Error::BadContentLength);
                }
                let n: u64 = m.parse().map_err(|_| H1Error::BadContentLength)?;
                if len.is_some_and(|l| l != n) {
                    return Err(H1Error::BadContentLength);
                }
                len = Some(n);
            }
        }
        return Ok(Framing::Length(len.unwrap_or(0)));
    }
    Ok(Framing::Close)
}

/// §9.3: may this connection carry another request after `head` (framed as `framing`)?
pub fn keep_alive(head: &ResponseHead, framing: Framing, request_said_close: bool) -> bool {
    if request_said_close || framing == Framing::Close || head.headers.has_token("connection", "close") {
        return false;
    }
    if head.version == (1, 0) {
        return head.headers.has_token("connection", "keep-alive");
    }
    head.version >= (1, 1)
}

// ---------------------------------------------------------------- §7.1 chunked

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChunkState {
    Size,
    Ext,
    SizeLf,
    Data,
    DataCr,
    DataLf,
    Trailer,
    Done,
}

/// Incremental chunked decoder (§7.1.3's algorithm), trailers collected (§7.1.2).
#[derive(Debug, Clone)]
pub struct ChunkedDecoder {
    st: ChunkState,
    size: u64,
    size_digits: u32,
    remaining: u64,
    line: Vec<u8>,
    trailer_lines: Vec<Vec<u8>>,
    pub trailers: Headers,
}

impl Default for ChunkedDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl ChunkedDecoder {
    pub fn new() -> Self {
        ChunkedDecoder {
            st: ChunkState::Size,
            size: 0,
            size_digits: 0,
            remaining: 0,
            line: Vec::new(),
            trailer_lines: Vec::new(),
            trailers: Headers::new(),
        }
    }

    pub fn is_done(&self) -> bool {
        self.st == ChunkState::Done
    }

    /// Decode from `input`, appending body octets to `out`; returns how many input octets were consumed
    /// (everything, unless the message ended inside `input`).
    pub fn push(&mut self, input: &[u8], out: &mut Vec<u8>) -> Result<usize, H1Error> {
        let mut i = 0;
        while i < input.len() {
            let b = input[i];
            match self.st {
                ChunkState::Done => return Ok(i),
                ChunkState::Size => match b {
                    b'0'..=b'9' | b'a'..=b'f' | b'A'..=b'F' => {
                        if self.size_digits >= 15 {
                            return Err(H1Error::BadChunk);
                        }
                        let d = (b as char).to_digit(16).unwrap() as u64;
                        self.size = self.size << 4 | d;
                        self.size_digits += 1;
                    }
                    b';' | b' ' | b'\t' if self.size_digits > 0 => self.st = ChunkState::Ext,
                    b'\r' if self.size_digits > 0 => self.st = ChunkState::SizeLf,
                    b'\n' if self.size_digits > 0 => self.size_done(),
                    _ => return Err(H1Error::BadChunk),
                },
                ChunkState::Ext => match b {
                    // chunk-ext = *( BWS ";" BWS ext-name [ BWS "=" BWS ext-val ] ) — ignored, but no CTLs.
                    b'\r' => self.st = ChunkState::SizeLf,
                    b'\n' => self.size_done(),
                    0..=8 | 10..=31 | 127 => return Err(H1Error::BadChunk),
                    _ => {}
                },
                ChunkState::SizeLf => {
                    if b != b'\n' {
                        return Err(H1Error::BadChunk);
                    }
                    self.size_done();
                }
                ChunkState::Data => {
                    let take = core::cmp::min(self.remaining, (input.len() - i) as u64) as usize;
                    out.extend_from_slice(&input[i..i + take]);
                    self.remaining -= take as u64;
                    i += take;
                    if self.remaining == 0 {
                        self.st = ChunkState::DataCr;
                    }
                    continue;
                }
                ChunkState::DataCr => match b {
                    b'\r' => self.st = ChunkState::DataLf,
                    b'\n' => self.next_chunk(),
                    _ => return Err(H1Error::BadChunk),
                },
                ChunkState::DataLf => {
                    if b != b'\n' {
                        return Err(H1Error::BadChunk);
                    }
                    self.next_chunk();
                }
                ChunkState::Trailer => {
                    if b == b'\n' {
                        let mut line = core::mem::take(&mut self.line);
                        if line.last() == Some(&b'\r') {
                            line.pop();
                        }
                        if line.is_empty() {
                            let lines = core::mem::take(&mut self.trailer_lines);
                            self.trailers = parse_fields(lines.iter().map(Vec::as_slice))?;
                            self.st = ChunkState::Done;
                            return Ok(i + 1);
                        }
                        if self.trailer_lines.len() >= MAX_FIELDS {
                            return Err(H1Error::HeadTooLarge);
                        }
                        self.trailer_lines.push(line);
                    } else {
                        if self.line.len() >= MAX_HEAD {
                            return Err(H1Error::HeadTooLarge);
                        }
                        self.line.push(b);
                    }
                }
            }
            i += 1;
        }
        Ok(i)
    }

    fn size_done(&mut self) {
        if self.size == 0 {
            self.st = ChunkState::Trailer;
        } else {
            self.remaining = self.size;
            self.st = ChunkState::Data;
        }
    }

    fn next_chunk(&mut self) {
        self.size = 0;
        self.size_digits = 0;
        self.st = ChunkState::Size;
    }
}

// ---------------------------------------------------------------- body decoder over any framing

/// Turns raw connection bytes into body bytes for one response, whatever its framing.
#[derive(Debug, Clone)]
pub struct BodyDecoder {
    framing: Framing,
    remaining: u64,
    chunked: ChunkedDecoder,
    done: bool,
}

impl BodyDecoder {
    pub fn new(framing: Framing) -> Self {
        let (remaining, done) = match framing {
            Framing::None => (0, true),
            Framing::Length(n) => (n, n == 0),
            _ => (0, false),
        };
        BodyDecoder { framing, remaining, chunked: ChunkedDecoder::new(), done }
    }

    pub fn framing(&self) -> Framing {
        self.framing
    }

    pub fn is_done(&self) -> bool {
        self.done
    }

    pub fn trailers(&self) -> &Headers {
        &self.chunked.trailers
    }

    /// Feed connection bytes; body bytes go to `out`; returns the input consumed.
    pub fn push(&mut self, input: &[u8], out: &mut Vec<u8>) -> Result<usize, H1Error> {
        if self.done {
            return Ok(0);
        }
        match self.framing {
            Framing::None => Ok(0),
            Framing::Length(_) => {
                let take = core::cmp::min(self.remaining, input.len() as u64) as usize;
                out.extend_from_slice(&input[..take]);
                self.remaining -= take as u64;
                self.done = self.remaining == 0;
                Ok(take)
            }
            Framing::Chunked => {
                let n = self.chunked.push(input, out)?;
                self.done = self.chunked.is_done();
                Ok(n)
            }
            Framing::Close => {
                out.extend_from_slice(input);
                Ok(input.len())
            }
        }
    }

    /// The connection reached EOF: fine for a close-delimited body, truncation for anything else.
    pub fn eof(&mut self) -> Result<(), H1Error> {
        if self.done {
            return Ok(());
        }
        if self.framing == Framing::Close {
            self.done = true;
            return Ok(());
        }
        Err(H1Error::Truncated)
    }
}
