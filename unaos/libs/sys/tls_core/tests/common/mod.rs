#![allow(dead_code)]
//! Shared test helpers: hex, the RFC 8448 vector file, a fake transport.

use std::collections::{HashMap, VecDeque};

use tls_core::error::TlsError;
use tls_core::Transport;

pub fn hex(s: &str) -> Vec<u8> {
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(s.len() % 2 == 0, "odd hex length");
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex")).collect()
}

pub fn to_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{:02x}", x)).collect()
}

/// One `[Section]` of tests/data/rfc8448.vec.
pub struct Trace {
    pub fields: HashMap<String, String>,
}

impl Trace {
    pub fn get(&self, k: &str) -> Vec<u8> {
        hex(self.fields.get(k).unwrap_or_else(|| panic!("missing field {k}")))
    }
    pub fn has(&self, k: &str) -> bool {
        self.fields.contains_key(k)
    }
}

pub fn rfc8448(section: &str) -> Trace {
    let text = include_str!("../data/rfc8448.vec");
    let mut cur: Option<String> = None;
    let mut fields = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            cur = Some(line.trim_matches(|c| c == '[' || c == ']').to_string());
            continue;
        }
        if cur.as_deref() == Some(section) {
            if let Some((k, v)) = line.split_once('=') {
                fields.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
    }
    assert!(!fields.is_empty(), "no section {section}");
    Trace { fields }
}

/// Splits concatenated TLS records into individual records.
pub fn split_records(mut b: &[u8]) -> Vec<Vec<u8>> {
    let mut v = Vec::new();
    while !b.is_empty() {
        let len = u16::from_be_bytes([b[3], b[4]]) as usize;
        v.push(b[..5 + len].to_vec());
        b = &b[5 + len..];
    }
    v
}

/// A scripted peer: hands out `incoming` bytes in chunks of `chunk` (to exercise reassembly) and records every
/// byte the client writes.
pub struct FakeTransport {
    pub incoming: VecDeque<u8>,
    pub written: Vec<u8>,
    pub chunk: usize,
}

impl FakeTransport {
    pub fn new(incoming: Vec<u8>) -> Self {
        FakeTransport { incoming: incoming.into(), written: Vec::new(), chunk: 4096 }
    }
    pub fn push(&mut self, b: &[u8]) {
        self.incoming.extend(b.iter().copied());
    }
}

impl Transport for FakeTransport {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, TlsError> {
        let n = buf.len().min(self.chunk).min(self.incoming.len());
        for b in buf.iter_mut().take(n) {
            *b = self.incoming.pop_front().unwrap();
        }
        Ok(n)
    }
    fn write_all(&mut self, data: &[u8]) -> Result<(), TlsError> {
        self.written.extend_from_slice(data);
        Ok(())
    }
}

/// std::net::TcpStream as a Transport.
pub struct Tcp(pub std::net::TcpStream);

impl Transport for Tcp {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, TlsError> {
        use std::io::Read;
        self.0.read(buf).map_err(|_| TlsError::Transport)
    }
    fn write_all(&mut self, data: &[u8]) -> Result<(), TlsError> {
        use std::io::Write;
        self.0.write_all(data).map_err(|_| TlsError::Transport)
    }
}
