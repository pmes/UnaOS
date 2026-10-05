//! One HTTP/1.1 exchange over any byte [`Transport`] — the seam the host client (std `TcpStream`, plain or
//! under `tls_core`), the metal (`vein_ring3`'s socket syscalls) and the tests (a byte script) all implement.
//! Blocking and pull-shaped: send the head and body, read the head (interim 1xx answers skipped, §15.2), then
//! pull body chunks until the framing says the message is complete. Bytes past the end of the message stay
//! buffered, so a persistent connection carries the next exchange.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use pixel_core::inflate::ByteSource;

use crate::encoding::DecodeError;
use crate::h1::{self, BodyDecoder, Framing, H1Error, ResponseHead};
use crate::redirect::RedirectError;
use crate::url::UrlError;

/// A reliable, ordered byte stream. `read` returning `Ok(0)` is the peer's orderly close.
pub trait Transport {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Error>;
    fn write_all(&mut self, data: &[u8]) -> Result<(), Error>;
}

impl<T: Transport + ?Sized> Transport for &mut T {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Error> {
        (**self).read(buf)
    }
    fn write_all(&mut self, data: &[u8]) -> Result<(), Error> {
        (**self).write_all(data)
    }
}

/// Everything an exchange can fail with, by layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The transport (socket, timeout) — the text is the transport's.
    Io(String),
    /// TLS (handshake or record layer) — `tls_core`'s reason.
    Tls(String),
    /// The proxy refused the tunnel.
    Proxy(String),
    Http(H1Error),
    Url(UrlError),
    Redirect(RedirectError),
    TooManyRedirects,
    Decode(DecodeError),
    /// The read timed out.
    Timeout,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(s) => write!(f, "connection error: {s}"),
            Error::Tls(s) => write!(f, "tls: {s}"),
            Error::Proxy(s) => write!(f, "proxy: {s}"),
            Error::Http(e) => write!(f, "http: {e}"),
            Error::Url(e) => write!(f, "url: {e}"),
            Error::Redirect(RedirectError::BadLocation(e)) => write!(f, "redirect: bad Location ({e})"),
            Error::Redirect(RedirectError::UnsupportedScheme(s)) => write!(f, "redirect: unsupported scheme {s}"),
            Error::TooManyRedirects => f.write_str("too many redirects"),
            Error::Decode(e) => write!(f, "{e}"),
            Error::Timeout => f.write_str("timed out"),
        }
    }
}

impl From<H1Error> for Error {
    fn from(e: H1Error) -> Self {
        Error::Http(e)
    }
}
impl From<UrlError> for Error {
    fn from(e: UrlError) -> Self {
        Error::Url(e)
    }
}
impl From<DecodeError> for Error {
    fn from(e: DecodeError) -> Self {
        Error::Decode(e)
    }
}

const READ_CHUNK: usize = 16 * 1024;

/// A connection: the transport plus whatever was read past the last message.
pub struct Conn<T: Transport> {
    pub transport: T,
    buf: Vec<u8>,
}

impl<T: Transport> Conn<T> {
    pub fn new(transport: T) -> Self {
        Conn { transport, buf: Vec::new() }
    }

    pub fn into_inner(self) -> T {
        self.transport
    }

    /// Octets received past the end of the last message (a persistent connection should have none).
    pub fn buffered(&self) -> usize {
        self.buf.len()
    }

    fn fill(&mut self) -> Result<usize, Error> {
        let mut tmp = [0u8; READ_CHUNK];
        let n = self.transport.read(&mut tmp)?;
        self.buf.extend_from_slice(&tmp[..n]);
        Ok(n)
    }

    /// Send a request head and body.
    pub fn send(&mut self, head: &[u8], body: &[u8]) -> Result<(), Error> {
        if body.is_empty() {
            return self.transport.write_all(head);
        }
        // One write for small requests (fewer TLS records, fewer segments).
        if body.len() <= 16 * 1024 {
            let mut all = Vec::with_capacity(head.len() + body.len());
            all.extend_from_slice(head);
            all.extend_from_slice(body);
            return self.transport.write_all(&all);
        }
        self.transport.write_all(head)?;
        self.transport.write_all(body)
    }

    /// Read the final response head for a request with `method`; interim 1xx responses (other than
    /// 101 Switching Protocols) are consumed and skipped.
    pub fn read_head(&mut self, method: &str) -> Result<(ResponseHead, Framing), Error> {
        loop {
            match h1::parse_response_head(&self.buf)? {
                Some((head, n)) => {
                    self.buf.drain(..n);
                    if (100..200).contains(&head.status) && head.status != 101 {
                        continue;
                    }
                    let framing = h1::response_framing(method, &head)?;
                    return Ok((head, framing));
                }
                None => {
                    if self.fill()? == 0 {
                        return Err(Error::Http(H1Error::Truncated));
                    }
                }
            }
        }
    }

    /// The next piece of body (`None` once the message is complete).
    pub fn read_body(&mut self, dec: &mut BodyDecoder) -> Result<Option<Vec<u8>>, Error> {
        loop {
            if dec.is_done() {
                return Ok(None);
            }
            if !self.buf.is_empty() {
                let mut out = Vec::new();
                let n = dec.push(&self.buf, &mut out)?;
                self.buf.drain(..n);
                if !out.is_empty() {
                    return Ok(Some(out));
                }
                if dec.is_done() {
                    return Ok(None);
                }
            }
            if self.fill()? == 0 {
                dec.eof()?;
                return Ok(None);
            }
        }
    }

    /// Read the whole (still content-coded) body.
    pub fn read_body_to_end(&mut self, dec: &mut BodyDecoder) -> Result<Vec<u8>, Error> {
        let mut all = Vec::new();
        while let Some(c) = self.read_body(dec)? {
            all.extend_from_slice(&c);
        }
        Ok(all)
    }
}

/// The body of one response as a pull [`ByteSource`] — what the inflater reads from, so a gzip body is
/// decoded as it arrives. A transport/framing error ends the source; it is kept in `error`.
pub struct BodySource<'a, T: Transport> {
    pub conn: &'a mut Conn<T>,
    pub dec: &'a mut BodyDecoder,
    chunk: Vec<u8>,
    pos: usize,
    pub error: Option<Error>,
}

impl<'a, T: Transport> BodySource<'a, T> {
    pub fn new(conn: &'a mut Conn<T>, dec: &'a mut BodyDecoder) -> Self {
        BodySource { conn, dec, chunk: Vec::new(), pos: 0, error: None }
    }
}

impl<T: Transport> ByteSource for BodySource<'_, T> {
    fn next(&mut self) -> Option<u8> {
        while self.pos >= self.chunk.len() {
            if self.error.is_some() {
                return None;
            }
            match self.conn.read_body(self.dec) {
                Ok(Some(c)) => {
                    self.chunk = c;
                    self.pos = 0;
                }
                Ok(None) => return None,
                Err(e) => {
                    self.error = Some(e);
                    return None;
                }
            }
        }
        let b = self.chunk[self.pos];
        self.pos += 1;
        Some(b)
    }
}
