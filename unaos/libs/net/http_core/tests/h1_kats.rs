//! RFC 9112 / RFC 9110 known-answer tests: the status line and field grammar, §6.3 framing, §7.1 chunked (split
//! at every byte), malformed refusals, an exchange over a scripted transport (1xx skipped, keep-alive leftover,
//! truncation), the redirect rules, the content codings (Python's gzip/zlib output as the oracle) and the
//! multipart encoder.

use http_core::conn::{Conn, Error, Transport};
use http_core::encoding::{self, Coding, DecodeError};
use http_core::h1::{self, BodyDecoder, ChunkedDecoder, Framing, H1Error};
use http_core::headers::Headers;
use http_core::multipart::Form;
use http_core::redirect::{next_hop, rewrite_headers};
use http_core::url::Url;

fn head(s: &str) -> h1::ResponseHead {
    h1::parse_response_head(s.as_bytes()).unwrap().unwrap().0
}

#[test]
fn status_line_and_fields() {
    let raw = "HTTP/1.1 200 OK\r\nDate: Mon, 27 Jul 2009 12:28:53 GMT\r\nContent-Length: 51\r\nVary: Accept-Encoding\r\nContent-Type: text/plain\r\n\r\nHello";
    let (h, n) = h1::parse_response_head(raw.as_bytes()).unwrap().unwrap();
    assert_eq!((h.version, h.status, h.reason.as_str()), ((1, 1), 200, "OK"));
    assert_eq!(&raw[n..], "Hello");
    assert_eq!(h.headers.get("content-length"), Some("51"));
    assert_eq!(h.headers.get("CONTENT-TYPE"), Some("text/plain"));
    // Incomplete → None, at every prefix.
    for i in 0..n {
        assert_eq!(h1::parse_response_head(&raw.as_bytes()[..i]).unwrap(), None, "prefix {i}");
    }
    // Bare LF line endings (§2.2), an empty reason, OWS around values.
    let h = head("HTTP/1.0 204 \nX-A:   v  \n\n");
    assert_eq!((h.version, h.status, h.reason.as_str(), h.headers.get("x-a")), ((1, 0), 204, "", Some("v")));
    let h = head("HTTP/1.1 404\r\n\r\n");
    assert_eq!(h.status, 404);
    // obs-fold (§5.2): a user agent replaces it with SP.
    let h = head("HTTP/1.1 200 OK\r\nX-Folded: a\r\n  b\r\n\r\n");
    assert_eq!(h.headers.get("x-folded"), Some("a b"));
    // Repeated fields keep order; Set-Cookie is never combined.
    let h = head("HTTP/1.1 200 OK\r\nSet-Cookie: a=1\r\nSet-Cookie: b=2\r\n\r\n");
    assert_eq!(h.headers.get_all("set-cookie").collect::<Vec<_>>(), ["a=1", "b=2"]);
}

#[test]
fn malformed_heads_are_refused() {
    for bad in [
        "HTTP/1.1 20 OK\r\n\r\n",
        "HTTP/1.1 200OK\r\n\r\n",
        "HTTP/11 200 OK\r\n\r\n",
        "ICY 200 OK\r\n\r\n",
        "HTTP/1.1 099 Low\r\n\r\n",
    ] {
        assert_eq!(h1::parse_response_head(bad.as_bytes()), Err(H1Error::BadStatusLine), "{bad:?}");
    }
    for bad in [
        "HTTP/1.1 200 OK\r\nX-A : v\r\n\r\n", // whitespace before the colon (§5.1)
        "HTTP/1.1 200 OK\r\nNoColon\r\n\r\n",
        "HTTP/1.1 200 OK\r\n: empty-name\r\n\r\n",
        "HTTP/1.1 200 OK\r\n continuation-first\r\n\r\n",
        "HTTP/1.1 200 OK\r\nX-A: a\rb\r\n\r\n",
        "HTTP/1.1 200 OK\r\nX(A): v\r\n\r\n",
    ] {
        assert_eq!(h1::parse_response_head(bad.as_bytes()), Err(H1Error::BadField), "{bad:?}");
    }
    let huge = format!("HTTP/1.1 200 OK\r\nX: {}\r\n", "a".repeat(70_000));
    assert_eq!(h1::parse_response_head(huge.as_bytes()), Err(H1Error::HeadTooLarge));
}

#[test]
fn message_body_length_rules() {
    let f = |m: &str, s: &str| h1::response_framing(m, &head(s));
    assert_eq!(f("HEAD", "HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n"), Ok(Framing::None));
    assert_eq!(f("GET", "HTTP/1.1 204 No Content\r\nContent-Length: 10\r\n\r\n"), Ok(Framing::None));
    assert_eq!(f("GET", "HTTP/1.1 304 Not Modified\r\n\r\n"), Ok(Framing::None));
    assert_eq!(f("CONNECT", "HTTP/1.1 200 Connection established\r\n\r\n"), Ok(Framing::None));
    assert_eq!(f("GET", "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Length: 10\r\n\r\n"), Ok(Framing::Chunked));
    assert_eq!(f("GET", "HTTP/1.1 200 OK\r\nContent-Length: 42\r\n\r\n"), Ok(Framing::Length(42)));
    assert_eq!(f("GET", "HTTP/1.1 200 OK\r\nContent-Length: 42, 42\r\n\r\n"), Ok(Framing::Length(42)));
    assert_eq!(f("GET", "HTTP/1.1 200 OK\r\nContent-Length: 42\r\nContent-Length: 43\r\n\r\n"), Err(H1Error::BadContentLength));
    assert_eq!(f("GET", "HTTP/1.1 200 OK\r\nContent-Length: -1\r\n\r\n"), Err(H1Error::BadContentLength));
    assert_eq!(f("GET", "HTTP/1.1 200 OK\r\nContent-Length: 0x10\r\n\r\n"), Err(H1Error::BadContentLength));
    assert_eq!(f("GET", "HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip\r\n\r\n"), Err(H1Error::UnsupportedTransferCoding));
    assert_eq!(f("GET", "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked, chunked\r\n\r\n"), Err(H1Error::BadChunk));
    assert_eq!(f("GET", "HTTP/1.1 200 OK\r\n\r\n"), Ok(Framing::Close));
    let h = head("HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n");
    assert!(!h1::keep_alive(&h, Framing::Length(0), false));
    let h = head("HTTP/1.0 200 OK\r\nConnection: Keep-Alive\r\n\r\n");
    assert!(h1::keep_alive(&h, Framing::Length(0), false));
    let h = head("HTTP/1.0 200 OK\r\n\r\n");
    assert!(!h1::keep_alive(&h, Framing::Length(0), false));
}

/// RFC 9112 §7.1 — sizes in hex, a chunk extension, trailers; decoded identically however it is split.
#[test]
fn chunked_every_split() {
    let wire = b"4;name=\"va;lue\"\r\nWiki\r\n5\r\npedia\r\nE\r\n in\r\n\r\nchunks.\r\n0\r\nExpires: Wed, 21 Oct 2015 07:28:00 GMT\r\n\r\nNEXT";
    let want = b"Wikipedia in\r\n\r\nchunks.";
    for split in 0..=wire.len() {
        for split2 in [split, (split + 7).min(wire.len())] {
            let mut d = ChunkedDecoder::new();
            let mut out = Vec::new();
            let mut used = d.push(&wire[..split], &mut out).unwrap();
            if !d.is_done() {
                used += d.push(&wire[split..split2], &mut out).unwrap();
            }
            if !d.is_done() {
                used += d.push(&wire[split2..], &mut out).unwrap();
            }
            assert!(d.is_done());
            assert_eq!(out, want, "split {split}/{split2}");
            assert_eq!(&wire[used..], b"NEXT");
            assert_eq!(d.trailers.get("expires"), Some("Wed, 21 Oct 2015 07:28:00 GMT"));
        }
    }
    for bad in [&b"g\r\n"[..], b"\r\n", b"4\r\nWikiXX", b"fffffffffffffffff\r\n", b"4\x01\r\n"] {
        let mut d = ChunkedDecoder::new();
        assert_eq!(d.push(bad, &mut Vec::new()), Err(H1Error::BadChunk), "{bad:?}");
    }
    assert_eq!(String::from_utf8(h1::encode_chunk(b"hello")).unwrap(), "5\r\nhello\r\n");
    assert_eq!(String::from_utf8(h1::encode_chunk(b"")).unwrap(), "0\r\n\r\n\r\n");
}

#[test]
fn request_head_encoding() {
    let mut h = Headers::new();
    h.append("User-Agent", "UnaOS").unwrap();
    h.append("host", "ignored").unwrap();
    let head = h1::encode_request_head("POST", "/v1/messages?x=1", "api.example:8443", &h, Some(5)).unwrap();
    assert_eq!(
        String::from_utf8(head).unwrap(),
        "POST /v1/messages?x=1 HTTP/1.1\r\nHost: api.example:8443\r\nUser-Agent: UnaOS\r\nContent-Length: 5\r\n\r\n"
    );
    assert_eq!(h1::encode_request_head("GE T", "/", "h", &h, None), Err(H1Error::BadRequest));
    assert_eq!(h1::encode_request_head("GET", "/a b", "h", &h, None), Err(H1Error::BadRequest));
    assert!(h.clone().append("X-Bad", "a\r\nInjected: 1").is_err());
    assert!(h.clone().append("Bad Name", "v").is_err());
}

/// A transport that replays a script, `max` bytes per read, and records what was written.
struct Script {
    data: Vec<u8>,
    pos: usize,
    max: usize,
    written: Vec<u8>,
}
impl Transport for Script {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Error> {
        let n = buf.len().min(self.max).min(self.data.len() - self.pos);
        buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
    fn write_all(&mut self, d: &[u8]) -> Result<(), Error> {
        self.written.extend_from_slice(d);
        Ok(())
    }
}

#[test]
fn exchange_over_a_scripted_transport() {
    for max in [1usize, 3, 7, 64, 100_000] {
        let wire = b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\nHTTP/1.1 404 Not Found\r\nContent-Length: 4\r\n\r\nnope";
        let mut c = Conn::new(Script { data: wire.to_vec(), pos: 0, max, written: Vec::new() });
        c.send(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n", b"").unwrap();
        let (h, f) = c.read_head("GET").unwrap();
        assert_eq!((h.status, f), (200, Framing::Chunked));
        let mut d = BodyDecoder::new(f);
        assert_eq!(c.read_body_to_end(&mut d).unwrap(), b"abc");
        assert!(h1::keep_alive(&h, f, false));
        // The second response on the same (persistent) connection.
        let (h, f) = c.read_head("GET").unwrap();
        assert_eq!((h.status, f), (404, Framing::Length(4)));
        let mut d = BodyDecoder::new(f);
        assert_eq!(c.read_body_to_end(&mut d).unwrap(), b"nope");
        assert_eq!(c.buffered(), 0);
    }
    // Content-Length longer than what arrives: truncation is an error, not a short body.
    let mut c = Conn::new(Script { data: b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nshort".to_vec(), pos: 0, max: 4, written: vec![] });
    let (_, f) = c.read_head("GET").unwrap();
    assert_eq!(c.read_body_to_end(&mut BodyDecoder::new(f)), Err(Error::Http(H1Error::Truncated)));
    // Close-delimited: EOF ends it.
    let mut c = Conn::new(Script { data: b"HTTP/1.0 200 OK\r\n\r\nall of it".to_vec(), pos: 0, max: 4, written: vec![] });
    let (_, f) = c.read_head("GET").unwrap();
    assert_eq!(c.read_body_to_end(&mut BodyDecoder::new(f)).unwrap(), b"all of it");
    // A chunked body cut off mid-chunk.
    let mut c = Conn::new(Script { data: b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nab".to_vec(), pos: 0, max: 4, written: vec![] });
    let (_, f) = c.read_head("GET").unwrap();
    assert_eq!(c.read_body_to_end(&mut BodyDecoder::new(f)), Err(Error::Http(H1Error::Truncated)));
}

#[test]
fn redirect_rules() {
    let cur = Url::parse("https://api.example/v1/a?x#frag").unwrap();
    let hop = next_hop(301, "POST", &cur, Some("/v2/b")).unwrap().unwrap();
    assert_eq!((hop.url.href().as_str(), hop.method.as_str(), hop.drop_body, hop.cross_origin), ("https://api.example/v2/b#frag", "GET", true, false));
    let hop = next_hop(307, "POST", &cur, Some("b")).unwrap().unwrap();
    assert_eq!((hop.url.href().as_str(), hop.method.as_str(), hop.drop_body), ("https://api.example/v1/b#frag", "POST", false));
    let hop = next_hop(308, "PUT", &cur, Some("https://other.example/c#own")).unwrap().unwrap();
    assert_eq!((hop.url.href().as_str(), hop.method.as_str(), hop.cross_origin), ("https://other.example/c#own", "PUT", true));
    let hop = next_hop(303, "HEAD", &cur, Some("/x")).unwrap().unwrap();
    assert_eq!(hop.method, "HEAD");
    let hop = next_hop(303, "DELETE", &cur, Some("/x")).unwrap().unwrap();
    assert_eq!((hop.method.as_str(), hop.drop_body), ("GET", true));
    let hop = next_hop(302, "GET", &cur, Some("http://api.example/")).unwrap().unwrap();
    assert!(hop.cross_origin, "a scheme change is another origin");
    assert!(next_hop(200, "GET", &cur, Some("/x")).is_none());
    assert!(next_hop(302, "GET", &cur, None).is_none());
    assert!(next_hop(302, "GET", &cur, Some("ftp://files.example/")).unwrap().is_err());
    let mut h = Headers::new();
    for (k, v) in [("X-Api-Key", "sk"), ("Authorization", "Bearer t"), ("Content-Type", "application/json"), ("Accept", "*/*")] {
        h.append(k, v).unwrap();
    }
    let hop = next_hop(302, "POST", &cur, Some("https://evil.example/")).unwrap().unwrap();
    rewrite_headers(&mut h, &hop);
    assert_eq!(h.iter().map(|(k, _)| k).collect::<Vec<_>>(), ["Accept"]);
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

/// Python's `gzip.compress(d, mtime=0)`, `zlib.compress(d)` and a raw `wbits=-15` stream are the oracle.
#[test]
fn content_codings() {
    let d = b"UnaOS HTTPCORE says hello. ".repeat(8);
    let gz = unhex("1f8b08000000000002030bcd4bf40f56f008090970f60f7255284eac2c56c848cdc9c9d753081dca5200bcc3c8c3d8000000");
    let zl = unhex("789c0bcd4bf40f56f008090970f60f7255284eac2c56c848cdc9c9d753081dca520077574589");
    let raw = unhex("0bcd4bf40f56f008090970f60f7255284eac2c56c848cdc9c9d753081dca5200");
    assert_eq!(encoding::decode(&[Coding::Gzip], &gz).unwrap(), d);
    assert_eq!(encoding::decode(&[Coding::Deflate], &zl).unwrap(), d);
    assert_eq!(encoding::decode(&[Coding::Deflate], &raw).unwrap(), d);
    // Stacked codings are removed in reverse order.
    assert_eq!(encoding::decode(&[Coding::Deflate, Coding::Identity], &zl).unwrap(), d);
    let mut bad = gz.clone();
    let n = bad.len();
    bad[n - 6] ^= 1; // the CRC-32
    assert!(matches!(encoding::decode(&[Coding::Gzip], &bad), Err(DecodeError::Inflate(_))));
    assert_eq!(encoding::decode(&[Coding::Brotli], b"x"), Err(DecodeError::Unsupported("br".into())));
    let h = head("HTTP/1.1 200 OK\r\nContent-Encoding: identity, x-gzip\r\n\r\n");
    assert_eq!(encoding::content_codings(&h.headers), [Coding::Gzip]);
}

#[test]
fn multipart_golden() {
    let f = Form::new("XyZ").part_text_and_file();
    assert_eq!(f.content_type(), "multipart/form-data; boundary=XyZ");
    assert_eq!(
        String::from_utf8(f.encode()).unwrap(),
        "--XyZ\r\nContent-Disposition: form-data; name=\"description\"\r\n\r\nUploaded via Vein\r\n--XyZ\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a%22b.txt\"\r\nContent-Type: application/octet-stream\r\n\r\nDATA\r\n--XyZ--\r\n"
    );
}

trait Fixture {
    fn part_text_and_file(self) -> Self;
}
impl Fixture for Form {
    fn part_text_and_file(self) -> Self {
        self.text("description", "Uploaded via Vein").file("file", "a\"b.txt", "application/octet-stream", b"DATA".to_vec())
    }
}
