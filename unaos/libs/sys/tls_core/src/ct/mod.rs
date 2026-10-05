//! Certificate Transparency (RFC 6962, RFC 9162) — CTCORE (LEDGER SR60).
//!
//! * [`LogList`] — a CT log list in Google's v3 schema (`log_list.json`, which Apple's `current_log_list.json`
//!   also follows): every log's id, SPKI, operator (and operator history), state and the state's timestamp.
//!   A log whose `log_id` ≠ SHA-256(key) is dropped on load. [`verify_list_signature`] checks the detached
//!   signature over the list bytes (Google publishes `log_list.sig`, RSASSA-PKCS1-v1_5 / SHA-256).
//! * [`verify_sct`] — RFC 6962 §3.2: the `digitally-signed` struct over
//!   `sct_version ‖ signature_type=certificate_timestamp ‖ timestamp ‖ entry_type ‖ signed_entry ‖ extensions`,
//!   where an EMBEDDED SCT signs a `PreCert { issuer_key_hash = SHA-256(issuer SPKI), tbs = the leaf's TBS minus
//!   the SCT-list extension }` and a TLS- or OCSP-delivered SCT signs the leaf certificate itself (`x509_entry`).
//!   ECDSA P-256 / SHA-256 and RSASSA-PKCS1-v1_5 / SHA-256 are the two algorithms RFC 6962 §2.1.4 allows.
//! * [`chrome_policy`] — Chrome's CT policy, transcribed from Chromium's `ChromeCTPolicyEnforcer::
//!   CheckCTPolicyCompliance` (components/certificate_transparency/chrome_ct_policy_enforcer.cc, read 2026-10-05):
//!   Option 1 — a valid TLS/OCSP SCT from a log not disqualified now, SCTs from two distinct operators (operator
//!   as of each SCT's timestamp), one from an RFC 6962 log; Option 2 — an embedded SCT from a log not disqualified
//!   now, two operators, one RFC 6962 log, and embedded SCTs from ≥ 2 distinct logs (lifetime ≤ 180 days) or ≥ 3
//!   (longer), where a retired log counts only if both the issuance date (earliest SCT from a non-retired log) and
//!   the SCT predate its retirement. Static-CT-API (tiled) logs need a `leaf_index` extension. A log list older
//!   than 70 days is not enforced (Chrome's "build not timely"). Pending / rejected logs verify nothing.
//! * [`merkle`] — RFC 9162 §2.1.3.2 inclusion proofs (and the RFC 6962 MerkleTreeLeaf of an SCT's entry), for a
//!   caller that fetched a proof from the log.
//!
//! What this module is NOT: an auditor (it never contacts a log), a gossip client, or Apple's policy (Apple's list
//! contributes keys; the verdict is Chrome's).

pub mod json;
pub mod merkle;

use alloc::string::String;
use alloc::vec::Vec;

use crate::crypto::{CryptoError, CryptoProvider, EcCurve, HashAlg};
use crate::x509::cert::{parse_spki, Certificate, PublicKey};
use crate::x509::der::Der;
use crate::x509::oid;
use crate::x509::sct::{Sct, SctSource};

/// Log list provenance bits (a log can be in both).
pub const LIST_GOOGLE: u8 = 1;
pub const LIST_APPLE: u8 = 2;

/// A log's state (the v3 schema's `state` object; RFC 6962 has no states — the lists define them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogState {
    Pending,
    Qualified,
    Usable,
    ReadOnly,
    Retired,
    Rejected,
}

impl LogState {
    fn from_key(k: &str) -> Option<LogState> {
        Some(match k {
            "pending" => LogState::Pending,
            "qualified" => LogState::Qualified,
            "usable" => LogState::Usable,
            "readonly" => LogState::ReadOnly,
            "retired" => LogState::Retired,
            "rejected" => LogState::Rejected,
            _ => return None,
        })
    }
}

/// One CT log.
#[derive(Debug, Clone)]
pub struct CtLog {
    pub operator: String,
    /// (operator, end_time): the log belonged to `operator` before `end_time` (seconds), oldest first.
    pub previous_operators: Vec<(String, i64)>,
    pub description: String,
    pub log_id: [u8; 32],
    pub key: PublicKey,
    pub state: LogState,
    /// When the log entered `state` (seconds).
    pub state_since: i64,
    /// A static-ct-api ("tiled") log rather than an RFC 6962 log.
    pub tiled: bool,
    /// The list(s) that named it ([`LIST_GOOGLE`], [`LIST_APPLE`]).
    pub lists: u8,
    /// temporal_interval [start, end) on certificate notAfter (seconds), when the list gives one.
    pub temporal: Option<(i64, i64)>,
}

impl CtLog {
    /// Chrome: a log is disqualified once it is retired (at the retirement timestamp).
    pub fn disqualified_at(&self) -> Option<i64> {
        (self.state == LogState::Retired).then_some(self.state_since)
    }
    /// The operator responsible for an SCT issued at `ts_ms` (Chromium's GetOperatorForLog).
    pub fn operator_at(&self, ts_ms: u64) -> &str {
        let t = (ts_ms / 1000) as i64;
        for (name, end) in &self.previous_operators {
            if t < *end {
                return name;
            }
        }
        &self.operator
    }
}

/// A parsed, key-checked log list.
#[derive(Debug, Clone, Default)]
pub struct LogList {
    pub version: String,
    /// `log_list_timestamp` (seconds).
    pub timestamp: i64,
    pub logs: Vec<CtLog>,
    /// Entries refused on load (no key, unsupported key type, log_id ≠ SHA-256(key), unknown state).
    pub refused: usize,
}

/// Why a log list did not load.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListError {
    Json,
    Schema(&'static str),
    Signature,
}

fn b64(v: Option<&json::Value>) -> Option<Vec<u8>> {
    crate::x509::pem::base64_decode(v?.as_str()?)
}

impl LogList {
    /// Parses a v3-schema list (`operators[].logs[]` and `operators[].tiled_logs[]`), tagging every log with
    /// `list` ([`LIST_GOOGLE`] / [`LIST_APPLE`]).
    pub fn parse_v3(p: &dyn CryptoProvider, text: &[u8], list: u8) -> Result<LogList, ListError> {
        let root = json::parse(text).ok_or(ListError::Json)?;
        let timestamp = root
            .get("log_list_timestamp")
            .and_then(|v| v.as_str())
            .and_then(json::rfc3339)
            .ok_or(ListError::Schema("log_list_timestamp"))?;
        let version = String::from(root.get("version").and_then(|v| v.as_str()).unwrap_or(""));
        let ops = root.get("operators").ok_or(ListError::Schema("operators"))?;
        let mut out = LogList { version, timestamp, logs: Vec::new(), refused: 0 };
        for op in ops.as_arr() {
            let name = op.get("name").and_then(|v| v.as_str()).ok_or(ListError::Schema("operator name"))?;
            for (field, tiled) in [("logs", false), ("tiled_logs", true)] {
                for l in op.get(field).map(|v| v.as_arr()).unwrap_or(&[]) {
                    match parse_log(p, l, name, tiled, list) {
                        Some(log) => out.logs.push(log),
                        None => out.refused += 1,
                    }
                }
            }
        }
        if out.logs.is_empty() {
            return Err(ListError::Schema("no usable logs"));
        }
        Ok(out)
    }

    /// Adds `other`'s logs; a log already present keeps its state and operator (the first list is
    /// authoritative — Google's for Chrome's policy) and gains `other`'s list bit.
    pub fn merge(&mut self, other: LogList) {
        for l in other.logs {
            match self.logs.iter_mut().find(|x| x.log_id == l.log_id) {
                Some(x) => x.lists |= l.lists,
                None => self.logs.push(l),
            }
        }
        self.refused += other.refused;
    }

    pub fn find(&self, log_id: &[u8; 32]) -> Option<&CtLog> {
        self.logs.iter().find(|l| &l.log_id == log_id)
    }

    /// Chrome enforces CT only with a list younger than 70 days (10 weeks).
    pub fn timely(&self, now: i64) -> bool {
        now - self.timestamp < 70 * 86_400
    }
}

fn parse_log(p: &dyn CryptoProvider, l: &json::Value, operator: &str, tiled: bool, list: u8) -> Option<CtLog> {
    let spki = b64(l.get("key"))?;
    let id = b64(l.get("log_id"))?;
    let digest = p.hash(HashAlg::Sha256, &[&spki]);
    if id.len() != 32 || id != digest.as_bytes() {
        return None; // RFC 6962 §3.2: LogID = SHA-256(public key)
    }
    let mut log_id = [0u8; 32];
    log_id.copy_from_slice(&id);
    let key = {
        let mut d = Der::new(&spki);
        let t = d.expect(0x30).ok()?;
        d.expect_end().ok()?;
        parse_spki(t.value).ok()?
    };
    if matches!(key, PublicKey::Unsupported(_)) {
        return None;
    }
    let (state, state_since) = match l.get("state").and_then(|s| s.as_obj()) {
        Some([(k, v)]) => (LogState::from_key(k)?, json::rfc3339(v.get("timestamp")?.as_str()?)?),
        _ => return None,
    };
    let temporal = l.get("temporal_interval").and_then(|t| {
        Some((json::rfc3339(t.get("start_inclusive")?.as_str()?)?, json::rfc3339(t.get("end_exclusive")?.as_str()?)?))
    });
    let mut previous_operators: Vec<(String, i64)> = l
        .get("previous_operators")
        .map(|v| v.as_arr())
        .unwrap_or(&[])
        .iter()
        .filter_map(|po| Some((String::from(po.get("name")?.as_str()?), json::rfc3339(po.get("end_time")?.as_str()?)?)))
        .collect();
    previous_operators.sort_by_key(|(_, t)| *t);
    // log_type "test" marks a log no policy may count.
    if l.get("log_type").and_then(|v| v.as_str()) == Some("test") {
        return None;
    }
    Some(CtLog {
        operator: String::from(operator),
        previous_operators,
        description: String::from(l.get("description").and_then(|v| v.as_str()).unwrap_or("")),
        log_id,
        key,
        state,
        state_since,
        tiled,
        lists: list,
        temporal,
    })
}

/// Verifies a detached signature over a log list (`key_spki` = the publisher's SubjectPublicKeyInfo DER; RSA →
/// RSASSA-PKCS1-v1_5 / SHA-256 as Google signs `log_list.sig`, EC → ECDSA / SHA-256).
pub fn verify_list_signature(p: &dyn CryptoProvider, key_spki: &[u8], list: &[u8], sig: &[u8]) -> Result<(), ListError> {
    let mut d = Der::new(key_spki);
    let t = d.expect(0x30).map_err(|_| ListError::Signature)?;
    let key = parse_spki(t.value).map_err(|_| ListError::Signature)?;
    let r = match &key {
        PublicKey::Rsa { n, e } => p.rsa_pkcs1_verify(HashAlg::Sha256, n, e, list, sig),
        PublicKey::Ec { curve, point } => p.ecdsa_verify(*curve, HashAlg::Sha256, point, list, sig),
        _ => return Err(ListError::Signature),
    };
    r.map_err(|_| ListError::Signature)
}

/// What an SCT signs over (RFC 6962 §3.2 `signed_entry`).
pub enum Entry<'a> {
    /// x509_entry: the leaf certificate (SCTs from the TLS extension or a stapled OCSP response).
    X509(&'a [u8]),
    /// precert_entry: SHA-256 of the issuer's SPKI and the leaf's TBS without the SCT-list extension.
    Precert { issuer_key_hash: [u8; 32], tbs: Vec<u8> },
}

impl Entry<'_> {
    /// `entry_type ‖ signed_entry` as both the signature input and the MerkleTreeLeaf encode it.
    pub fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Entry::X509(c) => {
                crate::codec::put_u16(out, 0);
                crate::codec::put_vec24(out, c);
            }
            Entry::Precert { issuer_key_hash, tbs } => {
                crate::codec::put_u16(out, 1);
                out.extend_from_slice(issuer_key_hash);
                crate::codec::put_vec24(out, tbs);
            }
        }
    }
}

/// The entry an SCT from `source` signs for `leaf`; `issuer_spki` is required for embedded SCTs.
pub fn entry_for<'a>(p: &dyn CryptoProvider, source: SctSource, leaf: &'a Certificate, issuer_spki: Option<&[u8]>) -> Option<Entry<'a>> {
    match source {
        SctSource::Tls | SctSource::Ocsp => Some(Entry::X509(&leaf.der)),
        SctSource::Embedded => {
            let mut h = [0u8; 32];
            h.copy_from_slice(p.hash(HashAlg::Sha256, &[issuer_spki?]).as_bytes());
            Some(Entry::Precert { issuer_key_hash: h, tbs: leaf.tbs_without_extension(oid::CT_PRECERT_SCTS)? })
        }
    }
}

/// The bytes an SCT's signature covers (RFC 6962 §3.2 `digitally-signed struct`).
pub fn signed_data(sct: &Sct, entry: &Entry<'_>) -> Vec<u8> {
    let mut v = Vec::with_capacity(64);
    v.push(sct.version); // v1(0)
    v.push(0); // signature_type = certificate_timestamp
    v.extend_from_slice(&sct.timestamp.to_be_bytes());
    entry.encode(&mut v);
    crate::codec::put_vec16(&mut v, &sct.extensions);
    v
}

/// One SCT's verification result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SctStatus {
    Valid,
    /// No log in the list has this id.
    UnknownLog,
    /// The log is pending or rejected: nothing it signs counts.
    LogNotAccepted,
    BadSignature,
    /// Timestamp after the check time.
    FutureTimestamp,
    /// hash/signature algorithm not SHA-256 with ECDSA or RSA, or not the log key's type.
    UnsupportedAlgorithm,
    /// An embedded SCT without a known issuer, or a TBS without the SCT extension.
    NoEntry,
}

/// An SCT with its verdict.
#[derive(Debug, Clone)]
pub struct SctResult {
    pub source: SctSource,
    pub log_id: [u8; 32],
    pub timestamp: u64,
    pub status: SctStatus,
    /// Index into [`LogList::logs`] when the log is known.
    pub log: Option<usize>,
}

/// Verifies one SCT for `leaf` (RFC 6962 §3.2, §2.1.4).
pub fn verify_sct(
    p: &dyn CryptoProvider,
    list: &LogList,
    sct: &Sct,
    leaf: &Certificate,
    issuer_spki: Option<&[u8]>,
    now_ms: u64,
) -> SctResult {
    let mut r = SctResult { source: sct.source, log_id: sct.log_id, timestamp: sct.timestamp, status: SctStatus::UnknownLog, log: None };
    let Some(idx) = list.logs.iter().position(|l| l.log_id == sct.log_id) else {
        return r;
    };
    r.log = Some(idx);
    let log = &list.logs[idx];
    if matches!(log.state, LogState::Pending | LogState::Rejected) {
        r.status = SctStatus::LogNotAccepted;
        return r;
    }
    if sct.timestamp > now_ms {
        r.status = SctStatus::FutureTimestamp;
        return r;
    }
    let Some(entry) = entry_for(p, sct.source, leaf, issuer_spki) else {
        r.status = SctStatus::NoEntry;
        return r;
    };
    let msg = signed_data(sct, &entry);
    // RFC 5246 §7.4.1.4.1 codes: hash 4 = sha256; signature 1 = rsa, 3 = ecdsa.
    let res = match (sct.hash_alg, sct.sig_alg, &log.key) {
        (4, 3, PublicKey::Ec { curve: EcCurve::P256, point }) => p.ecdsa_verify(EcCurve::P256, HashAlg::Sha256, point, &msg, &sct.signature),
        (4, 1, PublicKey::Rsa { n, e }) => p.rsa_pkcs1_verify(HashAlg::Sha256, n, e, &msg, &sct.signature),
        _ => Err(CryptoError::Unsupported("SCT algorithm")),
    };
    r.status = match res {
        Ok(()) => SctStatus::Valid,
        Err(CryptoError::Unsupported(_)) => SctStatus::UnsupportedAlgorithm,
        Err(_) => SctStatus::BadSignature,
    };
    r
}

/// The `ct=` verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CtStatus {
    /// Chrome's CT policy is met (`ct=policy`).
    Compliant,
    /// No SCT anywhere (`ct=no_scts`).
    NoScts,
    /// SCTs, valid ones among them, but not enough / not diverse (`ct=insufficient`).
    Insufficient,
    /// The policy is not met and at least one SCT from a known log failed its signature (`ct=bad_sig`).
    BadSig,
    /// The log list is older than 70 days; Chrome does not enforce (`ct=stale_list`).
    StaleList,
    /// No log list configured (`ct=off`).
    Off,
}

impl CtStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            CtStatus::Compliant => "policy",
            CtStatus::NoScts => "no_scts",
            CtStatus::Insufficient => "insufficient",
            CtStatus::BadSig => "bad_sig",
            CtStatus::StaleList => "stale_list",
            CtStatus::Off => "off",
        }
    }
}

/// The policy verdict with its evidence.
#[derive(Debug, Clone)]
pub struct CtVerdict {
    pub status: CtStatus,
    pub results: Vec<SctResult>,
    /// Distinct logs whose embedded SCTs counted toward the quorum, and the quorum (2 or 3).
    pub embedded_logs: usize,
    pub required: usize,
    pub diverse_operators: bool,
}

impl CtVerdict {
    pub fn off() -> CtVerdict {
        CtVerdict { status: CtStatus::Off, results: Vec::new(), embedded_logs: 0, required: 0, diverse_operators: false }
    }
    pub fn valid(&self) -> usize {
        self.results.iter().filter(|r| r.status == SctStatus::Valid).count()
    }
}

/// Does a static-ct-api SCT carry a well-formed leaf_index extension (type 0, uint40)?
fn has_leaf_index(sct: &Sct) -> bool {
    let mut r = crate::codec::Reader::new(&sct.extensions);
    while !r.is_empty() {
        let (Ok(t), Ok(d)) = (r.u8(), r.vec16()) else { return false };
        if t == 0 {
            return d.len() == 5;
        }
    }
    false
}

/// Chrome's CT policy over verified SCTs (see the module doc for the transcription).
pub fn chrome_policy(list: &LogList, leaf: &Certificate, scts: &[Sct], results: Vec<SctResult>, now: i64) -> CtVerdict {
    let lifetime = leaf.not_after - leaf.not_before;
    let required = if lifetime > 180 * 86_400 { 3 } else { 2 };
    let mut v = CtVerdict { status: CtStatus::Insufficient, results, embedded_logs: 0, required, diverse_operators: false };
    if scts.is_empty() {
        v.status = CtStatus::NoScts;
        return v;
    }
    if !list.timely(now) {
        v.status = CtStatus::StaleList;
        return v;
    }
    let disq = |l: &CtLog| l.disqualified_at().filter(|&d| now >= d);
    // The SCTs that count: valid, from a log in Google's list (Chrome's), static-ct-api ones with a leaf index.
    let counted: Vec<(&Sct, &CtLog)> = scts
        .iter()
        .zip(v.results.iter())
        .filter(|(_, r)| r.status == SctStatus::Valid)
        .filter_map(|(s, r)| Some((s, &list.logs[r.log?])))
        .filter(|(_, l)| l.lists & LIST_GOOGLE != 0)
        .filter(|(s, l)| !l.tiled || has_leaf_index(s))
        .collect();
    let issuance_ms = counted.iter().filter(|(_, l)| disq(l).is_none()).map(|(s, _)| s.timestamp).min().unwrap_or(u64::MAX);
    let (mut valid_embedded, mut valid_nonembedded, mut rfc6962) = (false, false, false);
    let mut first_op: Option<&str> = None;
    let mut embedded_ids: Vec<[u8; 32]> = Vec::new();
    for (s, l) in &counted {
        let d = disq(l);
        if d.is_some() && s.source != SctSource::Embedded {
            continue;
        }
        if s.source != SctSource::Embedded {
            valid_nonembedded = true;
        } else {
            valid_embedded |= d.is_none();
            let d_ms = d.map(|d| (d.max(0) as u64) * 1000);
            if d_ms.is_none_or(|dm| issuance_ms < dm && s.timestamp < dm) && !embedded_ids.contains(&l.log_id) {
                embedded_ids.push(l.log_id);
            }
        }
        let op = l.operator_at(s.timestamp);
        match first_op {
            None => first_op = Some(op),
            Some(f) => v.diverse_operators |= f != op,
        }
        rfc6962 |= !l.tiled;
    }
    v.embedded_logs = embedded_ids.len();
    let bad_sig = v.results.iter().any(|r| r.status == SctStatus::BadSignature);
    let fail = if bad_sig { CtStatus::BadSig } else { CtStatus::Insufficient };
    // Option 1: TLS / OCSP SCTs.
    if valid_nonembedded && v.diverse_operators && rfc6962 {
        v.status = CtStatus::Compliant;
        return v;
    }
    // Option 2: embedded SCTs.
    v.status = if valid_embedded && v.diverse_operators && rfc6962 && v.embedded_logs >= required { CtStatus::Compliant } else { fail };
    v
}

/// Verifies every SCT for `leaf` and applies Chrome's policy.
pub fn evaluate(p: &dyn CryptoProvider, list: &LogList, leaf: &Certificate, issuer_spki: Option<&[u8]>, scts: &[Sct], now: i64) -> CtVerdict {
    let now_ms = (now.max(0) as u64) * 1000;
    let results = scts.iter().map(|s| verify_sct(p, list, s, leaf, issuer_spki, now_ms)).collect();
    chrome_policy(list, leaf, scts, results, now)
}

/// Report the verdict, or refuse a chain that does not meet the policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CtMode {
    #[default]
    Report,
    /// `no_scts`, `insufficient` and `bad_sig` refuse the handshake (`stale_list` does not: Chrome would not
    /// enforce either).
    Strict,
}

/// The CT configuration a trust store carries.
#[derive(Debug, Clone, Default)]
pub struct CtConfig {
    pub list: LogList,
    pub mode: CtMode,
}
