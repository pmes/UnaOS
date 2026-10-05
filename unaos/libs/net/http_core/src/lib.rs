//! HTTPCORE (LEDGER SR51) — UnaOS's own HTTP client core, `no_std` + `alloc`, written from the specifications:
//!
//! * [`url`] — the WHATWG URL Standard parser, serializer, hosts (IPv4/IPv6/IDNA-Punycode), origins.
//! * [`headers`] — RFC 9110 §5 fields.
//! * [`h1`] — RFC 9112 HTTP/1.1: request head, status line, fields, §6.3 body length, §7.1 chunked + trailers,
//!   §9.3 persistence.
//! * [`conn`] — one exchange over any byte [`conn::Transport`], with a streaming body source.
//! * [`redirect`] — RFC 9110 §15.4 + Fetch's method/header rules.
//! * [`cookie`] — RFC 6265 §5 parser, dates, jar, Cookie header.
//! * [`encoding`] — gzip/deflate content codings through pixel_core's inflater (br owed).
//! * [`multipart`] — RFC 7578 `multipart/form-data`.
//! * [`h2`] (feature `h2`) — RFC 9113 framing and RFC 7541 HPACK.
//! * `host` (feature `host`, std) — the host transport: std `TcpStream`, `tls_core` verified against a PEM
//!   trust store, HTTP CONNECT proxies, a keep-alive pool of connection threads.
//!
//! No third-party crate: the only dependencies are UnaOS's own `pixel_core` (inflate) and, for `host`,
//! `tls_core` + `crypto_core`.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;
#[cfg(feature = "host")]
extern crate std;

pub mod conn;
pub mod cookie;
pub mod encoding;
pub mod h1;
pub mod headers;
pub mod multipart;
pub mod redirect;
pub mod url;

#[cfg(feature = "h2")]
pub mod h2;

#[cfg(feature = "host")]
pub mod host;

pub use conn::{Conn, Error, Transport};
pub use headers::Headers;
pub use url::Url;
