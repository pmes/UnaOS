// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The wire protocols, bytes in and bytes out (`gitprotocol-common(5)`, `gitprotocol-v2(5)`,
//! `gitprotocol-pack(5)`, `gitprotocol-http(5)`, `gitprotocol-capabilities(5)`):
//!
//! * pkt-line framing (`0000` flush, `0001` delim, `0002` response-end);
//! * protocol v2 over smart HTTP: the capability advertisement, `ls-refs` (ref-prefix, symrefs,
//!   peel), `fetch` (want / have / done, `deepen`, `thin-pack`, `ofs-delta`, `no-progress`) and its
//!   sectioned response (acknowledgments, shallow-info, wanted-refs, packfile) with side-band
//!   demultiplexing (band 1 data, 2 progress, 3 fatal error);
//! * push over `git-receive-pack` (protocol v0 — v2 does not define push): the ref advertisement
//!   with capabilities, update commands, the pack, and `report-status` (optionally side-banded);
//! * the dumb HTTP protocol's `info/refs` and `objects/info/packs`.
//!
//! `git://` (the daemon transport) and SSH are out of scope: UnaOS speaks git over HTTP(S), which
//! http_core (SR51) already carries with UnaOS's own TLS.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::hash::{HashKind, ObjectId};
use crate::{Error, Result};

/// One pkt-line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pkt<'a> {
    /// Payload.
    Data(&'a [u8]),
    /// `0000`.
    Flush,
    /// `0001`.
    Delim,
    /// `0002`.
    ResponseEnd,
}

/// Append one data pkt-line.
pub fn pkt(out: &mut Vec<u8>, data: &[u8]) {
    let n = data.len() + 4;
    out.extend_from_slice(alloc::format!("{n:04x}").as_bytes());
    out.extend_from_slice(data);
}

/// Append a text pkt-line (a `\n` is added).
pub fn pkt_line(out: &mut Vec<u8>, s: &str) {
    let mut v = s.as_bytes().to_vec();
    v.push(b'\n');
    pkt(out, &v);
}

/// Append a flush.
pub fn flush(out: &mut Vec<u8>) {
    out.extend_from_slice(b"0000");
}

/// Append a delimiter.
pub fn delim(out: &mut Vec<u8>) {
    out.extend_from_slice(b"0001");
}

/// Iterate pkt-lines.
pub struct PktReader<'a> {
    d: &'a [u8],
    /// Position.
    pub pos: usize,
}

impl<'a> PktReader<'a> {
    /// Read from `d`.
    pub fn new(d: &'a [u8]) -> Self {
        PktReader { d, pos: 0 }
    }
    /// The next pkt, `Ok(None)` at the end of input.
    pub fn next_pkt(&mut self) -> Result<Option<Pkt<'a>>> {
        if self.pos >= self.d.len() {
            return Ok(None);
        }
        let h = self.d.get(self.pos..self.pos + 4).ok_or(Error::Corrupt("pkt-line: truncated length"))?;
        let mut n = 0usize;
        for &c in h {
            let v = (c as char).to_digit(16).ok_or(Error::Corrupt("pkt-line: bad length"))?;
            n = n * 16 + v as usize;
        }
        self.pos += 4;
        Ok(Some(match n {
            0 => Pkt::Flush,
            1 => Pkt::Delim,
            2 => Pkt::ResponseEnd,
            3 => return Err(Error::Corrupt("pkt-line: length 3")),
            _ => {
                let end = self.pos + n - 4;
                let data = self.d.get(self.pos..end).ok_or(Error::Corrupt("pkt-line: truncated payload"))?;
                self.pos = end;
                Pkt::Data(data)
            }
        }))
    }
}

fn chomp(d: &[u8]) -> &[u8] {
    d.strip_suffix(b"\n").unwrap_or(d)
}

/// Skip the smart-HTTP `# service=<name>` preamble (pkt + flush) if present.
pub fn skip_service_header(body: &[u8]) -> Result<&[u8]> {
    let mut r = PktReader::new(body);
    if let Some(Pkt::Data(d)) = r.next_pkt()? {
        if d.starts_with(b"# service=") {
            match r.next_pkt()? {
                Some(Pkt::Flush) => return Ok(&body[r.pos..]),
                _ => return Err(Error::Corrupt("smart http: service header not followed by flush")),
            }
        }
    }
    Ok(body)
}

/// A protocol-v2 capability advertisement.
#[derive(Debug, Clone, Default)]
pub struct Capabilities {
    /// `key[=value]` lines after `version 2`.
    pub caps: Vec<(String, Option<String>)>,
}

impl Capabilities {
    /// Parse (`version 2` first).
    pub fn parse(body: &[u8]) -> Result<Self> {
        let body = skip_service_header(body)?;
        let mut r = PktReader::new(body);
        match r.next_pkt()? {
            Some(Pkt::Data(d)) if chomp(d) == b"version 2" => {}
            _ => return Err(Error::Unsupported("server does not speak protocol v2")),
        }
        let mut caps = Vec::new();
        while let Some(p) = r.next_pkt()? {
            match p {
                Pkt::Data(d) => {
                    let s = String::from_utf8_lossy(chomp(d)).into_owned();
                    match s.split_once('=') {
                        Some((k, v)) => caps.push((k.into(), Some(v.into()))),
                        None => caps.push((s, None)),
                    }
                }
                Pkt::Flush => break,
                _ => return Err(Error::Corrupt("v2 advertisement: unexpected delimiter")),
            }
        }
        Ok(Capabilities { caps })
    }
    /// The value of capability `k` (`Some("")` for a bare one).
    pub fn get(&self, k: &str) -> Option<&str> {
        self.caps.iter().find(|(x, _)| x == k).map(|(_, v)| v.as_deref().unwrap_or(""))
    }
    /// Does the `fetch` capability list `feature` (e.g. `shallow`)?
    pub fn fetch_has(&self, feature: &str) -> bool {
        self.get("fetch").is_some_and(|v| v.split(' ').any(|f| f == feature))
    }
    /// The server's object format (`sha1` when unstated).
    pub fn object_format(&self) -> HashKind {
        self.get("object-format").and_then(HashKind::from_name).unwrap_or(HashKind::Sha1)
    }
}

/// The client agent string.
pub const AGENT: &str = "git/unaos-git_core-0.1";

fn command_header(out: &mut Vec<u8>, command: &str, hk: HashKind, caps: &Capabilities) {
    pkt_line(out, &alloc::format!("command={command}"));
    if caps.get("agent").is_some() {
        pkt_line(out, &alloc::format!("agent={AGENT}"));
    }
    if caps.get("object-format").is_some() {
        pkt_line(out, &alloc::format!("object-format={}", hk.name()));
    }
    delim(out);
}

/// An `ls-refs` request.
pub fn ls_refs_request(hk: HashKind, caps: &Capabilities, prefixes: &[&str]) -> Vec<u8> {
    let mut o = Vec::new();
    command_header(&mut o, "ls-refs", hk, caps);
    pkt_line(&mut o, "peel");
    pkt_line(&mut o, "symrefs");
    if caps.get("ls-refs").is_some_and(|v| v.split(' ').any(|f| f == "unborn")) {
        pkt_line(&mut o, "unborn");
    }
    for p in prefixes {
        pkt_line(&mut o, &alloc::format!("ref-prefix {p}"));
    }
    flush(&mut o);
    o
}

/// One advertised ref.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteRef {
    /// Full name.
    pub name: String,
    /// Value (`None` for an unborn HEAD).
    pub id: Option<ObjectId>,
    /// `symref-target:`.
    pub symref_target: Option<String>,
    /// `peeled:`.
    pub peeled: Option<ObjectId>,
}

/// Parse an `ls-refs` response.
pub fn parse_ls_refs(hk: HashKind, body: &[u8]) -> Result<Vec<RemoteRef>> {
    let mut r = PktReader::new(body);
    let mut v = Vec::new();
    while let Some(p) = r.next_pkt()? {
        let Pkt::Data(d) = p else { break };
        let s = String::from_utf8_lossy(chomp(d)).into_owned();
        let mut it = s.split(' ');
        let first = it.next().ok_or(Error::Corrupt("ls-refs: empty line"))?;
        let name = it.next().ok_or(Error::Corrupt("ls-refs: no name"))?.to_string();
        let id = if first == "unborn" { None } else { Some(ObjectId::from_hex_kind(hk, first.as_bytes()).ok_or(Error::Corrupt("ls-refs: bad id"))?) };
        let mut rr = RemoteRef { name, id, symref_target: None, peeled: None };
        for a in it {
            if let Some(t) = a.strip_prefix("symref-target:") {
                rr.symref_target = Some(t.into());
            } else if let Some(p) = a.strip_prefix("peeled:") {
                rr.peeled = ObjectId::from_hex_kind(hk, p.as_bytes());
            }
        }
        v.push(rr);
    }
    Ok(v)
}

/// Arguments of one `fetch` request.
#[derive(Debug, Clone, Default)]
pub struct FetchArgs {
    /// `want <oid>`.
    pub wants: Vec<ObjectId>,
    /// `have <oid>`.
    pub haves: Vec<ObjectId>,
    /// Send `done` (end negotiation).
    pub done: bool,
    /// `deepen <n>`.
    pub depth: Option<u32>,
    /// `shallow <oid>` lines (our current shallow boundary).
    pub shallow: Vec<ObjectId>,
    /// Ask for a thin pack.
    pub thin_pack: bool,
    /// Suppress progress.
    pub no_progress: bool,
    /// `include-tag`.
    pub include_tag: bool,
}

/// A `fetch` request.
pub fn fetch_request(hk: HashKind, caps: &Capabilities, a: &FetchArgs) -> Vec<u8> {
    let mut o = Vec::new();
    command_header(&mut o, "fetch", hk, caps);
    if a.thin_pack {
        pkt_line(&mut o, "thin-pack");
    }
    if a.no_progress {
        pkt_line(&mut o, "no-progress");
    }
    pkt_line(&mut o, "ofs-delta");
    if a.include_tag {
        pkt_line(&mut o, "include-tag");
    }
    for s in &a.shallow {
        pkt_line(&mut o, &alloc::format!("shallow {s}"));
    }
    if let Some(d) = a.depth {
        pkt_line(&mut o, &alloc::format!("deepen {d}"));
    }
    for w in &a.wants {
        pkt_line(&mut o, &alloc::format!("want {w}"));
    }
    for h in &a.haves {
        pkt_line(&mut o, &alloc::format!("have {h}"));
    }
    if a.done {
        pkt_line(&mut o, "done");
    }
    flush(&mut o);
    o
}

/// A parsed `fetch` response.
#[derive(Debug, Clone, Default)]
pub struct FetchResponse {
    /// `ACK <oid>` lines.
    pub acks: Vec<ObjectId>,
    /// `NAK` seen.
    pub nak: bool,
    /// `ready` seen.
    pub ready: bool,
    /// `shallow <oid>`.
    pub shallow: Vec<ObjectId>,
    /// `unshallow <oid>`.
    pub unshallow: Vec<ObjectId>,
    /// `wanted-refs` entries.
    pub wanted_refs: Vec<(ObjectId, String)>,
    /// The demultiplexed pack (band 1).
    pub pack: Vec<u8>,
    /// Progress text (band 2).
    pub progress: Vec<u8>,
}

/// Parse a v2 `fetch` response.
pub fn parse_fetch_response(hk: HashKind, body: &[u8]) -> Result<FetchResponse> {
    let mut r = PktReader::new(body);
    let mut out = FetchResponse::default();
    let mut section: Option<String> = None;
    while let Some(p) = r.next_pkt()? {
        match p {
            Pkt::Flush | Pkt::ResponseEnd => {
                if section.as_deref() == Some("packfile") || section.as_deref() == Some("acknowledgments") || p == Pkt::ResponseEnd {
                    break;
                }
                section = None;
            }
            Pkt::Delim => section = None,
            Pkt::Data(d) => {
                let Some(sec) = section.as_deref() else {
                    section = Some(String::from_utf8_lossy(chomp(d)).into_owned());
                    continue;
                };
                match sec {
                    "packfile" => match d.first() {
                        Some(1) => out.pack.extend_from_slice(&d[1..]),
                        Some(2) => out.progress.extend_from_slice(&d[1..]),
                        Some(3) => return Err(Error::Io(alloc::format!("remote error: {}", String::from_utf8_lossy(&d[1..])))),
                        _ => return Err(Error::Corrupt("packfile: bad side-band")),
                    },
                    "acknowledgments" => {
                        let l = chomp(d);
                        if l == b"NAK" {
                            out.nak = true;
                        } else if l == b"ready" {
                            out.ready = true;
                        } else if let Some(h) = l.strip_prefix(b"ACK ") {
                            out.acks.push(ObjectId::from_hex_kind(hk, h).ok_or(Error::Corrupt("ACK: bad id"))?);
                        }
                    }
                    "shallow-info" => {
                        let l = chomp(d);
                        if let Some(h) = l.strip_prefix(b"shallow ") {
                            out.shallow.push(ObjectId::from_hex_kind(hk, h).ok_or(Error::Corrupt("shallow: bad id"))?);
                        } else if let Some(h) = l.strip_prefix(b"unshallow ") {
                            out.unshallow.push(ObjectId::from_hex_kind(hk, h).ok_or(Error::Corrupt("unshallow: bad id"))?);
                        }
                    }
                    "wanted-refs" => {
                        let l = String::from_utf8_lossy(chomp(d)).into_owned();
                        if let Some((h, n)) = l.split_once(' ') {
                            out.wanted_refs.push((ObjectId::from_hex_kind(hk, h.as_bytes()).ok_or(Error::Corrupt("wanted-refs: bad id"))?, n.into()));
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// receive-pack (push), protocol v0
// ---------------------------------------------------------------------------------------------

/// A v0 ref advertisement (`receive-pack` / `upload-pack` without v2).
#[derive(Debug, Clone, Default)]
pub struct V0Advert {
    /// (name, id); empty for an empty repository.
    pub refs: Vec<(String, ObjectId)>,
    /// Capabilities from the first line.
    pub caps: Vec<String>,
}

/// Parse a v0 advertisement.
pub fn parse_v0_advert(hk: HashKind, body: &[u8]) -> Result<V0Advert> {
    let body = skip_service_header(body)?;
    let mut r = PktReader::new(body);
    let mut a = V0Advert::default();
    let mut first = true;
    while let Some(p) = r.next_pkt()? {
        let Pkt::Data(d) = p else { break };
        let mut l = chomp(d);
        if first {
            first = false;
            if let Some(nul) = l.iter().position(|&c| c == 0) {
                a.caps = String::from_utf8_lossy(&l[nul + 1..]).split(' ').filter(|s| !s.is_empty()).map(String::from).collect();
                l = &l[..nul];
            }
        }
        let s = String::from_utf8_lossy(l).into_owned();
        let (h, n) = s.split_once(' ').ok_or(Error::Corrupt("advertisement: bad line"))?;
        let id = ObjectId::from_hex_kind(hk, h.as_bytes()).ok_or(Error::Corrupt("advertisement: bad id"))?;
        if n == "capabilities^{}" {
            continue;
        }
        a.refs.push((n.into(), id));
    }
    Ok(a)
}

/// One ref update for push.
#[derive(Debug, Clone)]
pub struct Command {
    /// Old value (null to create).
    pub old: ObjectId,
    /// New value (null to delete).
    pub new: ObjectId,
    /// Ref name.
    pub name: String,
}

/// A `git-receive-pack` request body: commands (capabilities on the first), flush, pack.
pub fn push_request(cmds: &[Command], caps: &[&str], pack: &[u8]) -> Vec<u8> {
    let mut o = Vec::new();
    for (i, c) in cmds.iter().enumerate() {
        let mut l = alloc::format!("{} {} {}", c.old, c.new, c.name).into_bytes();
        if i == 0 && !caps.is_empty() {
            l.push(0);
            l.extend_from_slice(caps.join(" ").as_bytes());
        }
        l.push(b'\n');
        pkt(&mut o, &l);
    }
    flush(&mut o);
    o.extend_from_slice(pack);
    o
}

/// The `report-status` result.
#[derive(Debug, Clone, Default)]
pub struct Report {
    /// `unpack ok`?
    pub unpack_ok: bool,
    /// The unpack status text.
    pub unpack: String,
    /// Per ref: Ok or the `ng` reason.
    pub refs: Vec<(String, core::result::Result<(), String>)>,
}

/// Parse `report-status`, demultiplexing side-band when `sideband`.
pub fn parse_report_status(body: &[u8], sideband: bool) -> Result<Report> {
    let inner: Vec<u8>;
    let data: &[u8] = if sideband {
        let mut r = PktReader::new(body);
        let mut v = Vec::new();
        while let Some(p) = r.next_pkt()? {
            match p {
                Pkt::Data(d) => match d.first() {
                    Some(1) => v.extend_from_slice(&d[1..]),
                    Some(2) => {}
                    Some(3) => return Err(Error::Io(alloc::format!("remote error: {}", String::from_utf8_lossy(&d[1..])))),
                    _ => return Err(Error::Corrupt("report-status: bad side-band")),
                },
                _ => break,
            }
        }
        inner = v;
        &inner
    } else {
        body
    };
    let mut r = PktReader::new(data);
    let mut rep = Report::default();
    while let Some(p) = r.next_pkt()? {
        let Pkt::Data(d) = p else { break };
        let s = String::from_utf8_lossy(chomp(d)).into_owned();
        if let Some(u) = s.strip_prefix("unpack ") {
            rep.unpack_ok = u == "ok";
            rep.unpack = u.into();
        } else if let Some(n) = s.strip_prefix("ok ") {
            rep.refs.push((n.into(), Ok(())));
        } else if let Some(rest) = s.strip_prefix("ng ") {
            let (n, why) = rest.split_once(' ').unwrap_or((rest, ""));
            rep.refs.push((n.into(), Err(why.into())));
        }
    }
    Ok(rep)
}

// ---------------------------------------------------------------------------------------------
// dumb HTTP
// ---------------------------------------------------------------------------------------------

/// Parse a dumb `info/refs` (`<hex>\t<name>` lines; `^{}` peeled lines dropped).
pub fn parse_dumb_info_refs(hk: HashKind, body: &[u8]) -> Result<Vec<(String, ObjectId)>> {
    let mut v = Vec::new();
    for l in body.split(|&c| c == b'\n').filter(|l| !l.is_empty()) {
        let t = l.iter().position(|&c| c == b'\t').ok_or(Error::Corrupt("info/refs: no tab"))?;
        let name = String::from_utf8_lossy(&l[t + 1..]).into_owned();
        if name.ends_with("^{}") {
            continue;
        }
        v.push((name, ObjectId::from_hex_kind(hk, &l[..t]).ok_or(Error::Corrupt("info/refs: bad id"))?));
    }
    Ok(v)
}

/// Parse `objects/info/packs` (`P pack-<hex>.pack` lines).
pub fn parse_info_packs(body: &[u8]) -> Vec<String> {
    body.split(|&c| c == b'\n').filter_map(|l| l.strip_prefix(b"P ")).map(|p| String::from_utf8_lossy(p).trim().to_string()).collect()
}
