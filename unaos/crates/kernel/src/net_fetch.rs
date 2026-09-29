// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// NETFETCH: the pure, arch-neutral half of the shell `fetch` verb — URL parsing, the HTTP/1.0 request-line
// builder, and the status/header splitter. No I/O, no NIC; the driver (connect/send/recv over the smoltcp
// TCP client, VFS write) lives in `shell.rs` (`shell_fetch`, file tail). See docs/dev/evidence/rmbp-0929/NETFETCH.md.
use alloc::string::String;

/// Body cap: the driver stops (and reports `status=... bytes=cap`) at this many body bytes.
pub const FETCH_CAP: usize = 4 * 1024 * 1024;
/// Streaming chunk: one `stack_recv` buffer and one VFS write.
pub const FETCH_CHUNK: usize = 1400;
/// Overall wall budget for connect + request + body.
pub const FETCH_TIMEOUT_MS: u64 = 10_000;
/// Progress line cadence.
pub const FETCH_PROGRESS: usize = 64 * 1024;
/// Response headers larger than this are refused (hostile-input bound).
pub const FETCH_HDR_MAX: usize = 8192;

/// A parsed `http://host[:port][/path]` URL (borrows from the input).
#[derive(Debug, PartialEq, Eq)]
pub struct Url<'a> {
    pub host: &'a str,
    pub port: u16,
    pub path: &'a str,
}

/// Parse an `http://` URL (scheme optional). `None` on an empty host, a bad/zero port, or `https://`.
pub fn parse_url(u: &str) -> Option<Url<'_>> {
    if u.starts_with("https://") {
        return None;
    }
    let rest = u.strip_prefix("http://").unwrap_or(u);
    let (hostport, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match hostport.rfind(':') {
        Some(i) => (&hostport[..i], hostport[i + 1..].parse::<u16>().ok()?),
        None => (hostport, 80),
    };
    if host.is_empty() || port == 0 {
        return None;
    }
    Some(Url { host, port, path })
}

/// The last path component (`/a/b/c.txt?x` -> `c.txt`); `index.html` when the path ends in `/` or is empty.
pub fn basename(path: &str) -> &str {
    let p = path.split(|c| c == '?' || c == '#').next().unwrap_or("");
    match p.rsplit('/').next() {
        Some(b) if !b.is_empty() => b,
        _ => "index.html",
    }
}

/// Build the HTTP/1.0 GET request for `u` (the `Host:` header carries the URL's host text).
pub fn build_request(u: &Url) -> String {
    alloc::format!(
        "GET {} HTTP/1.0\r\nHost: {}\r\nUser-Agent: UnaOS-fetch\r\nConnection: close\r\n\r\n",
        u.path, u.host
    )
}

/// Split `buf` at the blank line ending the headers: `Some((status_code, body_offset))` once
/// `\r\n\r\n` has arrived and the status line is `HTTP/1.x NNN`; `None` while incomplete or malformed.
pub fn parse_response_head(buf: &[u8]) -> Option<(u16, usize)> {
    let end = buf.windows(4).position(|w| w == b"\r\n\r\n")? + 4;
    let line = buf[..end].split(|&b| b == b'\r').next()?;
    if line.len() < 12 || !line.starts_with(b"HTTP/1.") || line[8] != b' ' {
        return None;
    }
    let d = &line[9..12];
    if !d.iter().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let code = (d[0] - b'0') as u16 * 100 + (d[1] - b'0') as u16 * 10 + (d[2] - b'0') as u16;
    Some((code, end))
}

/// NETFETCH-PARSE: deterministic battery over the pure half (URL, basename, request line, head split).
/// Prints `:: NETFETCH-PARSE: ... -> PASS|FAIL ::` on serial. Returns the verdict.
pub fn parse_gate() -> bool {
    let a = parse_url("http://10.0.2.2:8080/a/b.txt");
    let a_ok = a == Some(Url { host: "10.0.2.2", port: 8080, path: "/a/b.txt" });
    let b = parse_url("example.com");
    let b_ok = b == Some(Url { host: "example.com", port: 80, path: "/" });
    let c_ok = parse_url("https://x/").is_none() && parse_url("http://:80/").is_none()
        && parse_url("http://h:0/").is_none() && parse_url("http://h:99999/").is_none();
    let d_ok = basename("/a/b.txt?q=1") == "b.txt" && basename("/") == "index.html";
    let req_ok = match &a {
        Some(u) => build_request(u)
            == "GET /a/b.txt HTTP/1.0\r\nHost: 10.0.2.2\r\nUser-Agent: UnaOS-fetch\r\nConnection: close\r\n\r\n",
        None => false,
    };
    let h = b"HTTP/1.0 200 OK\r\nContent-Length: 2\r\n\r\nhi";
    let h_ok = parse_response_head(h) == Some((200, h.len() - 2))
        && parse_response_head(b"HTTP/1.0 200 OK\r\nX: y\r\n").is_none()
        && parse_response_head(b"garbage\r\n\r\n").is_none();
    let ok = a_ok && b_ok && c_ok && d_ok && req_ok && h_ok;
    serial_println!(
        ":: NETFETCH-PARSE: url={} basename={} request={} head={} -> {} ::",
        a_ok && b_ok && c_ok, d_ok, req_ok, h_ok, if ok { "PASS" } else { "FAIL" }
    );
    ok
}
