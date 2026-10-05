//! CTCORE M1 — Certificate Transparency (RFC 6962 / RFC 9162) on the PRODUCT provider (CRYPTOCORE).
//!
//! 1. The REAL embedded SCTs of the five captured public leaves (tests/data/public) verify against Chromium's own
//!    log list (the v3 `log_list.json` Chromium compiles in, pinned by commit and sha256, fetched at test time) and
//!    every leaf meets Chrome's CT policy; altered SCTs are refused.
//! 2. Chrome's policy branches on a synthetic list (lifetime quorum, operator diversity, retired logs, tiled logs,
//!    a stale list) — transcribed from Chromium's chrome_ct_policy_enforcer.cc.
//! 3. RFC 9162 inclusion proofs: transparency-dev/merkle's 98 vectors (pinned), plus trees built by Python hashlib.
//! 4. The log-list signature check, against `openssl dgst -verify`.
//! 5. End to end: a local CT log pair (tests/oracle/ct_log.py, independent encoder + OpenSSL ECDSA) whose SCTs
//!    reach tls_core through `openssl s_server -serverinfo` (TLS 1.2 ServerHello and the TLS 1.3 leaf
//!    CertificateEntry): `ct=policy` / `insufficient` / `bad_sig` / `no_scts`, and strict mode refuses the last
//!    three with the server seeing the alert.

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;

use support::*;
use tls_core::ct::{self, merkle, CtConfig, CtMode, CtStatus, LogList, LogState, SctStatus, LIST_GOOGLE};
use tls_core::error::{CertError, TlsError};
use tls_core::x509::pem::pem_blocks;
use tls_core::x509::sct::{parse_list, Sct, SctSource};
use tls_core::x509::verify::collect_scts;
use tls_core::x509::{Certificate, WebPkiVerifier};
use tls_core::{Client, ClientConfig, CryptoProvider};

const LOG_LIST_URL: &str = "https://raw.githubusercontent.com/chromium/chromium/750ecf97c4cafe8acf7bb4ec645fe3fbafc776c4/components/certificate_transparency/data/log_list.json";
const LOG_LIST_SHA256: &str = "38d4bcc4692f427f10562a24db13c1a1917c62398b2839323880583be1035788";
const MERKLE_BASE: &str = "https://raw.githubusercontent.com/transparency-dev/merkle/fbbcd741c3d1c69d8498487baa8edc9e5824847c/testdata/";
const MERKLE_SHA256: &str = "4bac25d56bb2d347cea42be95f5b9facdd7f88e009cd152b93ee81dde5e81f6b";
const HOSTS: [&str; 5] = ["anthropic.com", "index.crates.io", "pypi.org", "raw.githubusercontent.com", "registry.npmjs.org"];

fn hexs(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn offline() -> bool {
    std::env::var("CRYPTO_OFFLINE").is_ok() || std::env::var("TLSCORE_OFFLINE").is_ok()
}

fn cache_dir() -> PathBuf {
    let d = std::env::temp_dir().join("ctcore-vectors");
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// GETs `url` with the system curl into a cache; None when offline / unreachable.
fn fetch(url: &str) -> Option<Vec<u8>> {
    if offline() {
        return None;
    }
    let name: String = url.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' { c } else { '_' }).collect();
    let path = cache_dir().join(&name[name.len().saturating_sub(120)..]);
    if let Ok(b) = std::fs::read(&path) {
        return Some(b);
    }
    let out = Command::new("curl").args(["-gsSfL", "--max-time", "30", url]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    std::fs::write(&path, &out.stdout).ok()?;
    Some(out.stdout)
}

fn sha256_hex(p: &dyn CryptoProvider, parts: &[&[u8]]) -> String {
    hexs(p.hash(tls_core::crypto::HashAlg::Sha256, parts).as_bytes())
}

/// Chromium's log list at the pinned commit (or the staged trust bundle copy when it has the same pin).
pub fn chromium_log_list(p: &dyn CryptoProvider) -> Option<LogList> {
    let staged = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../system/trust/ct/log_list.json");
    let bytes = std::fs::read(&staged).ok().filter(|b| sha256_hex(p, &[b]) == LOG_LIST_SHA256).or_else(|| fetch(LOG_LIST_URL))?;
    assert_eq!(sha256_hex(p, &[&bytes]), LOG_LIST_SHA256, "pinned log list sha256");
    Some(LogList::parse_v3(p, &bytes, LIST_GOOGLE).expect("log list parses"))
}

fn public_chain(host: &str) -> Vec<Vec<u8>> {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/public");
    pem_blocks(&std::fs::read_to_string(base.join(format!("{host}.pem"))).unwrap(), "CERTIFICATE").0
}

fn captured_at() -> i64 {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/public");
    std::fs::read_to_string(base.join("CAPTURED_AT")).unwrap().trim().parse().unwrap()
}

#[test]
fn real_embedded_scts_verify_and_meet_chrome_policy() {
    let p = provider();
    let Some(list) = chromium_log_list(&p) else {
        println!("CTCORE M1 SKIPPED: log list not fetchable (offline)");
        return;
    };
    let states = |s: LogState| list.logs.iter().filter(|l| l.state == s).count();
    println!(
        "LOG LIST v{} @{} logs={} (qualified {} usable {} readonly {} retired {}; tiled {}) refused={}",
        list.version,
        list.timestamp,
        list.logs.len(),
        states(LogState::Qualified),
        states(LogState::Usable),
        states(LogState::ReadOnly),
        states(LogState::Retired),
        list.logs.iter().filter(|l| l.tiled).count(),
        list.refused
    );
    assert_eq!(list.refused, 0, "every log's id is SHA-256 of its key");
    let now = captured_at();
    let (mut total, mut valid, mut tampered_refused) = (0, 0, 0);
    for host in HOSTS {
        let chain = public_chain(host);
        let leaf = Certificate::parse(&chain[0]).unwrap();
        let issuer = chain[1..].iter().map(|d| Certificate::parse(d).unwrap()).find(|c| c.subject == leaf.issuer).expect("issuer presented");
        let scts = collect_scts(Some(&chain[0]), None);
        let v = ct::evaluate(&p, &list, &leaf, Some(&issuer.spki), &scts, now);
        let logs: Vec<String> = v.results.iter().map(|r| format!("{}:{:?}", r.log.map(|i| list.logs[i].description.as_str()).unwrap_or("?"), r.status)).collect();
        println!("CT {host:<26} ct={} scts={} valid={} logs={}/{} diverse={} {logs:?}", v.status.as_str(), scts.len(), v.valid(), v.embedded_logs, v.required, v.diverse_operators);
        assert_eq!(v.valid(), scts.len(), "{host}: every embedded SCT verifies");
        assert_eq!(v.status, CtStatus::Compliant, "{host}: Chrome CT policy");
        total += scts.len();
        valid += v.valid();

        // Tamper each SCT: one signature byte, then the timestamp — that SCT must fail, the rest still verify.
        for i in 0..scts.len() {
            for how in ["sig", "ts"] {
                let mut t = scts.clone();
                match how {
                    "sig" => {
                        let n = t[i].signature.len();
                        t[i].signature[n / 2] ^= 0x01;
                    }
                    _ => t[i].timestamp += 1,
                }
                let tv = ct::evaluate(&p, &list, &leaf, Some(&issuer.spki), &t, now);
                assert_eq!(tv.results[i].status, SctStatus::BadSignature, "{host} sct {i} {how}");
                assert_eq!(tv.valid(), scts.len() - 1);
                tampered_refused += 1;
            }
        }
        // All SCTs altered → the policy fails with bad_sig; a different issuer key → bad_sig too.
        let mut all = scts.clone();
        for s in &mut all {
            let n = s.signature.len();
            s.signature[n / 2] ^= 0x01;
        }
        assert_eq!(ct::evaluate(&p, &list, &leaf, Some(&issuer.spki), &all, now).status, CtStatus::BadSig);
        let wrong_issuer = ct::evaluate(&p, &list, &leaf, Some(&leaf.spki), &scts, now);
        assert_eq!(wrong_issuer.status, CtStatus::BadSig, "{host}: issuer_key_hash is bound");
        // Delivered over TLS instead of embedded, the same SCTs sign the precert, not the final cert → bad.
        let as_tls: Vec<Sct> = scts.iter().cloned().map(|mut s| { s.source = SctSource::Tls; s }).collect();
        assert_eq!(ct::evaluate(&p, &list, &leaf, Some(&issuer.spki), &as_tls, now).status, CtStatus::BadSig);
        // No SCTs at all.
        assert_eq!(ct::evaluate(&p, &list, &leaf, Some(&issuer.spki), &[], now).status, CtStatus::NoScts);
        // A list 71 days old at check time is not enforced.
        assert_eq!(ct::evaluate(&p, &list, &leaf, Some(&issuer.spki), &scts, list.timestamp + 71 * 86_400).status, CtStatus::StaleList);
    }
    println!("CT REAL: {valid}/{total} embedded SCTs verified across 5 leaves; {tampered_refused}/{tampered_refused} alterations refused");
    assert_eq!(total, 12);
}

fn log(id: u8, op: &str, state: LogState, since: i64, tiled: bool) -> ct::CtLog {
    ct::CtLog {
        operator: op.into(),
        previous_operators: vec![],
        description: format!("log{id}"),
        log_id: [id; 32],
        key: tls_core::x509::PublicKey::Unsupported("synthetic"),
        state,
        state_since: since,
        tiled,
        lists: LIST_GOOGLE,
        temporal: None,
    }
}

fn synth_sct(id: u8, ts_s: i64, src: SctSource, leaf_index: bool) -> Sct {
    let extensions = if leaf_index { vec![0, 0, 5, 0, 0, 0, 0, 7] } else { vec![] };
    Sct { source: src, version: 0, log_id: [id; 32], timestamp: ts_s as u64 * 1000, extensions_len: extensions.len(), hash_alg: 4, sig_alg: 3, signature_len: 0, extensions, signature: vec![] }
}

/// Chrome's policy branches (results forced Valid: this exercises the policy alone).
#[test]
fn chrome_policy_branches() {
    let p = provider();
    let base = &public_chain("pypi.org")[0];
    let mut leaf = Certificate::parse(base).unwrap();
    let now = 1_791_000_000;
    let mut list = LogList { version: "t".into(), timestamp: now - 86_400, logs: vec![], refused: 0 };
    list.logs.push(log(1, "A", LogState::Usable, 0, false));
    list.logs.push(log(2, "B", LogState::Qualified, 0, false));
    list.logs.push(log(3, "A", LogState::ReadOnly, 0, false));
    list.logs.push(log(4, "C", LogState::Retired, now - 1000, false));
    list.logs.push(log(5, "D", LogState::Usable, 0, true));
    list.logs.push(log(6, "E", LogState::Usable, 0, true));
    let run = |list: &LogList, leaf: &Certificate, scts: &[Sct], now: i64| {
        let results = scts
            .iter()
            .map(|s| ct::SctResult { source: s.source, log_id: s.log_id, timestamp: s.timestamp, status: SctStatus::Valid, log: list.logs.iter().position(|l| l.log_id == s.log_id) })
            .collect();
        ct::chrome_policy(list, leaf, scts, results, now)
    };
    let e = SctSource::Embedded;
    let t0 = now - 50_000;
    let mut cases: Vec<(&str, i64, Vec<Sct>, CtStatus)> = vec![
        ("2 embedded, 2 operators, 90-day cert", 90, vec![synth_sct(1, t0, e, false), synth_sct(2, t0, e, false)], CtStatus::Compliant),
        ("2 embedded, ONE operator", 90, vec![synth_sct(1, t0, e, false), synth_sct(3, t0, e, false)], CtStatus::Insufficient),
        ("2 embedded, 2 operators, 397-day cert needs 3", 397, vec![synth_sct(1, t0, e, false), synth_sct(2, t0, e, false)], CtStatus::Insufficient),
        ("3 embedded, 397-day cert", 397, vec![synth_sct(1, t0, e, false), synth_sct(2, t0, e, false), synth_sct(3, t0, e, false)], CtStatus::Compliant),
        ("retired log, SCT + issuance before retirement counts", 397, vec![synth_sct(1, t0, e, false), synth_sct(2, t0, e, false), synth_sct(4, t0, e, false)], CtStatus::Compliant),
        ("retired log, SCT after retirement does not", 397, vec![synth_sct(1, now - 500, e, false), synth_sct(2, now - 500, e, false), synth_sct(4, now - 500, e, false)], CtStatus::Insufficient),
        ("only a retired log is not 'valid at check time'", 90, vec![synth_sct(4, t0, e, false), synth_sct(4, t0, e, false)], CtStatus::Insufficient),
        ("same log twice counts once", 90, vec![synth_sct(1, t0, e, false), synth_sct(1, t0, e, false)], CtStatus::Insufficient),
        ("tiled logs only: no RFC 6962 log", 90, vec![synth_sct(5, t0, e, true), synth_sct(6, t0, e, true)], CtStatus::Insufficient),
        ("tiled + RFC 6962", 90, vec![synth_sct(5, t0, e, true), synth_sct(1, t0, e, false)], CtStatus::Compliant),
        ("tiled without leaf_index is ignored", 90, vec![synth_sct(5, t0, e, false), synth_sct(1, t0, e, false)], CtStatus::Insufficient),
        ("Option 1: TLS SCTs from 2 operators", 397, vec![synth_sct(1, t0, SctSource::Tls, false), synth_sct(2, t0, SctSource::Ocsp, false)], CtStatus::Compliant),
        ("Option 1: TLS SCTs, one operator", 397, vec![synth_sct(1, t0, SctSource::Tls, false), synth_sct(3, t0, SctSource::Tls, false)], CtStatus::Insufficient),
        ("Option 1: a retired log's TLS SCT never counts", 397, vec![synth_sct(1, t0, SctSource::Tls, false), synth_sct(4, t0, SctSource::Tls, false)], CtStatus::Insufficient),
        ("unknown log", 90, vec![synth_sct(9, t0, e, false), synth_sct(1, t0, e, false)], CtStatus::Insufficient),
        ("no SCTs", 90, vec![], CtStatus::NoScts),
    ];
    // Operator history: log 3 belonged to "Z" until t0 + 1 → an SCT at t0 is Z's, diverse from A.
    let mut n = 0;
    for (name, days, scts, want) in cases.drain(..) {
        leaf.not_after = leaf.not_before + days * 86_400;
        let v = run(&list, &leaf, &scts, now);
        println!("POLICY {name:<56} → ct={}", v.status.as_str());
        assert_eq!(v.status, want, "{name}");
        n += 1;
    }
    list.logs[2].previous_operators = vec![("Z".into(), t0 + 1)];
    leaf.not_after = leaf.not_before + 90 * 86_400;
    let v = run(&list, &leaf, &[synth_sct(1, t0, e, false), synth_sct(3, t0, e, false)], now);
    println!("POLICY {:<56} → ct={}", "operator as of the SCT timestamp (previous_operators)", v.status.as_str());
    assert_eq!(v.status, CtStatus::Compliant);
    assert_eq!(run(&list, &leaf, &[synth_sct(1, t0, e, false), synth_sct(2, t0, e, false)], now + 70 * 86_400).status, CtStatus::StaleList);
    println!("POLICY: {} branches agree with chrome_ct_policy_enforcer.cc", n + 2);
    let _ = p;
}

#[test]
fn merkle_inclusion_vectors_and_python_trees() {
    let p = provider();
    let manifest = include_str!("data/ct/merkle_inclusion.manifest");
    let mut files = Vec::new();
    for path in manifest.split_whitespace() {
        match fetch(&format!("{MERKLE_BASE}{path}")) {
            Some(b) => files.push((path, b)),
            None => {
                println!("MERKLE VECTORS SKIPPED: not fetchable (offline)");
                files.clear();
                break;
            }
        }
    }
    if !files.is_empty() {
        let parts: Vec<&[u8]> = files.iter().map(|(_, b)| b.as_slice()).collect();
        assert_eq!(sha256_hex(&p, &parts), MERKLE_SHA256, "pinned vectors");
        let (mut ok, mut neg) = (0, 0);
        for (path, b) in &files {
            let v = ct::json::parse(b).unwrap();
            let d = |k: &str| v.get(k).and_then(|x| x.as_str()).and_then(tls_core::x509::pem::base64_decode).unwrap_or_default();
            let to32 = |b: Vec<u8>| -> Option<[u8; 32]> { b.try_into().ok() };
            let proof: Option<Vec<[u8; 32]>> = v.get("proof").map(|a| a.as_arr()).unwrap_or(&[]).iter().map(|x| to32(tls_core::x509::pem::base64_decode(x.as_str()?)?)).collect();
            let want_err = v.get("wantErr") == Some(&ct::json::Value::Bool(true));
            let got = match (to32(d("leafHash")), to32(d("root")), proof) {
                (Some(l), Some(r), Some(pr)) => merkle::verify_inclusion(&p, &l, v.get("leafIdx").unwrap().as_u64().unwrap(), v.get("treeSize").unwrap().as_u64().unwrap(), &pr, &r),
                _ => false,
            };
            assert_eq!(got, !want_err, "{path}");
            if want_err { neg += 1 } else { ok += 1 }
        }
        println!("MERKLE transparency-dev vectors: {ok} proofs accepted, {neg} refused — {}/{} agree", ok + neg, files.len());
    }
    // Python hashlib builds trees of 1..=33 leaves and every inclusion proof (RFC 9162 §2.1.3.1 PATH).
    let script = r#"
import hashlib,sys
H=lambda b:hashlib.sha256(b).digest()
def mth(d):
    if len(d)==1: return H(b'\0'+d[0])
    k=1
    while k*2<len(d): k*=2
    return H(b'\1'+mth(d[:k])+mth(d[k:]))
def path(m,d):
    if len(d)==1: return []
    k=1
    while k*2<len(d): k*=2
    return path(m,d[:k])+[mth(d[k:])] if m<k else path(m-k,d[k:])+[mth(d[:k])]
for n in range(1,34):
    d=[bytes([i])*i for i in range(n)]
    for m in range(n):
        print(n,m,mth(d).hex(),H(b'\0'+d[m]).hex(),','.join(x.hex() for x in path(m,d)))
"#;
    let Ok(out) = Command::new("python3").args(["-c", script]).output() else {
        println!("PYTHON MERKLE ORACLE SKIPPED");
        return;
    };
    let (mut n_ok, mut n_bad) = (0, 0);
    for line in String::from_utf8(out.stdout).unwrap().lines() {
        let f: Vec<&str> = line.split(' ').collect();
        let (n, m): (u64, u64) = (f[0].parse().unwrap(), f[1].parse().unwrap());
        let h32 = |s: &str| -> [u8; 32] { common_hex(s).try_into().unwrap() };
        let root = h32(f[2]);
        let lh = h32(f[3]);
        let proof: Vec<[u8; 32]> = f.get(4).map(|s| s.split(',').filter(|x| !x.is_empty()).map(h32).collect()).unwrap_or_default();
        let leaves: Vec<[u8; 32]> = (0..n).map(|i| merkle::leaf_hash(&p, &vec![i as u8; i as usize])).collect();
        assert_eq!(merkle::tree_root(&p, &leaves), root, "MTH n={n}");
        assert!(merkle::verify_inclusion(&p, &lh, m, n, &proof, &root), "n={n} m={m}");
        n_ok += 1;
        if !proof.is_empty() {
            let mut bad = proof.clone();
            bad[0][0] ^= 1;
            assert!(!merkle::verify_inclusion(&p, &lh, m, n, &bad, &root));
            assert!(!merkle::verify_inclusion(&p, &lh, m ^ 1, n, &proof, &root) || n == 1);
            n_bad += 1;
        }
    }
    println!("MERKLE Python trees 1..=33: {n_ok} proofs verified, {n_bad} altered proofs refused");
    assert_eq!(n_ok, (1..=33).sum::<u64>());
}

fn common_hex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

#[test]
fn log_list_signature_vs_openssl() {
    let p = provider();
    if !have_tools() {
        println!("SKIPPED: openssl");
        return;
    }
    let list = fetch(LOG_LIST_URL).unwrap_or_else(|| br#"{"log_list_timestamp":"2026-10-01T00:00:00Z","operators":[]}"#.to_vec());
    let dir = std::env::temp_dir().join(format!("ctcore-sig-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = |n: &str| dir.join(n).display().to_string();
    std::fs::write(f("list.json"), &list).unwrap();
    let mut tampered = list.clone();
    tampered[10] ^= 1;
    std::fs::write(f("tampered.json"), &tampered).unwrap();
    let mut n = 0;
    for (kind, genargs) in [("rsa", vec!["genpkey", "-algorithm", "RSA", "-pkeyopt", "rsa_keygen_bits:2048"]), ("p256", vec!["genpkey", "-algorithm", "EC", "-pkeyopt", "ec_paramgen_curve:P-256"])] {
        let key = f(&format!("{kind}.key"));
        assert!(Command::new("openssl").args(&genargs).args(["-out", &key]).status().unwrap().success());
        let spki = Command::new("openssl").args(["pkey", "-in", &key, "-pubout", "-outform", "DER"]).output().unwrap().stdout;
        assert!(Command::new("openssl").args(["pkey", "-in", &key, "-pubout", "-out", &f("pub.pem")]).status().unwrap().success());
        assert!(Command::new("openssl").args(["dgst", "-sha256", "-sign", &key, "-out", &f("sig"), &f("list.json")]).status().unwrap().success());
        let sig = std::fs::read(f("sig")).unwrap();
        for (file, body) in [("list.json", &list), ("tampered.json", &tampered)] {
            let ours = ct::verify_list_signature(&p, &spki, body, &sig).is_ok();
            let theirs = Command::new("openssl").args(["dgst", "-sha256", "-verify", &f("pub.pem"), "-signature", &f("sig"), &f(file)]).output().unwrap().status.success();
            println!("LIST SIG {kind:<4} {file:<13} tls_core={ours} openssl={theirs}");
            assert_eq!(ours, theirs);
            assert_eq!(ours, file == "list.json");
            n += 1;
        }
    }
    println!("LIST SIG: {n}/{n} agree with openssl dgst -verify");
    let _ = std::fs::remove_dir_all(&dir);
}

struct LocalLog {
    dir: PathBuf,
    list: LogList,
}

impl LocalLog {
    fn new(p: &dyn CryptoProvider, tag: &str) -> Option<LocalLog> {
        let dir = pki(tag)?;
        let ok = Command::new("python3").arg(oracle_dir().join("ct_log.py")).arg("keys").arg(&dir).status().map(|s| s.success()).unwrap_or(false);
        assert!(ok, "ct_log.py keys");
        let list = LogList::parse_v3(p, &std::fs::read(dir.join("log_list.json")).unwrap(), LIST_GOOGLE).unwrap();
        assert_eq!(list.logs.len(), 2);
        Some(LocalLog { dir, list })
    }
    fn sct(&self, log: &str, ts_ms: u64) -> String {
        let out = Command::new("python3")
            .arg(oracle_dir().join("ct_log.py"))
            .args(["sct", &self.dir.display().to_string(), log, &self.dir.join("p256.pem").display().to_string(), &ts_ms.to_string()])
            .output()
            .unwrap();
        assert!(out.status.success());
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }
    fn serverinfo(&self, v: &str, scts: &[String]) -> String {
        let f = self.dir.join(format!("si{v}-{}.pem", scts.len()));
        let mut c = Command::new("python3");
        c.arg(oracle_dir().join("ct_log.py")).args(["serverinfo", &f.display().to_string(), v]).args(scts);
        assert!(c.status().unwrap().success());
        f.display().to_string()
    }
}

#[test]
fn tls_delivered_scts_end_to_end_with_strict_mode() {
    let p = provider();
    let Some(ll) = LocalLog::new(&p, "m9ct") else { return };
    let now_ms = SystemClock.now_ms();
    let a = ll.sct("a", now_ms - 60_000);
    let b = ll.sct("b", now_ms - 60_000);
    let mut bad = common_hex(&b);
    let n = bad.len();
    bad[n - 10] ^= 1;
    let bad = hexs(&bad);
    // Cross-check the oracle's wire SCT parses the way we read it.
    let parsed = parse_list(&{
        let s = common_hex(&a);
        let mut l = ((s.len() + 2) as u16).to_be_bytes().to_vec();
        l.extend_from_slice(&(s.len() as u16).to_be_bytes());
        l.extend_from_slice(&s);
        l
    }, SctSource::Tls).unwrap();
    assert_eq!(parsed[0].log_id, ll.list.logs[0].log_id);

    let mut roots = store_of(&ll.dir.join("root.pem"));
    let cases: Vec<(&str, Vec<String>, CtStatus)> = vec![
        ("two operators", vec![a.clone(), b.clone()], CtStatus::Compliant),
        ("one SCT", vec![a.clone()], CtStatus::Insufficient),
        ("one forged", vec![a.clone(), bad.clone()], CtStatus::BadSig),
        ("none", vec![], CtStatus::NoScts),
    ];
    let mut n = 0;
    for v in ["1", "2"] {
        for (name, scts, want) in &cases {
            for mode in [CtMode::Report, CtMode::Strict] {
                roots.ct = Some(CtConfig { list: ll.list.clone(), mode });
                let mut extra: Vec<String> = vec![if v == "1" { "-tls1_2" } else { "-tls1_3" }.into()];
                if !scts.is_empty() {
                    extra.push("-serverinfo".into());
                    extra.push(ll.serverinfo(v, scts));
                }
                let extra_ref: Vec<&str> = extra.iter().map(|s| s.as_str()).collect();
                let srv = SServer::start(&ll.dir, "p256", &extra_ref);
                let clock = SystemClock;
                let verifier = WebPkiVerifier { store: &roots, clock: &clock };
                let mut cfg = ClientConfig::new(Some("tlscore.test"), &verifier);
                cfg.enable_tls12();
                let r = Client::connect(&p, &cfg, dial(srv.port));
                let tls = if v == "1" { "1.2" } else { "1.3" };
                match (&r, mode, want) {
                    (Ok(c), _, _) => {
                        let ng = c.negotiated();
                        println!("CT E2E TLS {tls} {name:<14} {mode:?}: ct={} scts={}", ng.ct.status.as_str(), ng.scts.len());
                        assert_eq!(ng.ct.status, *want, "{name} {tls}");
                        assert!(mode == CtMode::Report || *want == CtStatus::Compliant);
                        assert_eq!(ng.version, if v == "1" { 0x0303 } else { 0x0304 });
                    }
                    (Err(e), CtMode::Strict, w) if *w != CtStatus::Compliant => {
                        assert_eq!(*e, TlsError::Certificate(CertError::CtPolicy(w.as_str())), "{name}");
                        let seen = srv.wait_for("alert bad certificate", 2000);
                        println!("CT E2E TLS {tls} {name:<14} {mode:?}: refused {e:?}; s_server saw alert bad certificate: {seen}");
                        assert!(seen, "{}", srv.log());
                    }
                    (Err(e), _, _) => panic!("{name} {tls} {mode:?}: {e:?}\n{}", srv.log()),
                }
                n += 1;
            }
        }
    }
    println!("CT E2E: {n}/{n} cases as expected");
    let _ = std::fs::remove_dir_all(&ll.dir);
}

use tls_core::x509::Clock;
