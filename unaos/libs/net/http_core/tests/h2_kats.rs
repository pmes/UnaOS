//! M4 — HTTP/2: the http2jp/hpack-test-case vectors (five independent encoders' wire, decoded to the exact
//! header lists, dynamic table and size updates included), our encoder round-tripped over the same header
//! sets, and RFC 9113 framing KATs (exact octets and the §6 refusals).

mod common;

use std::process::Command;

use common::{cache_dir, crate_dir, parse_json, vectors, Json};
use http_core::h2::frame::{self, ErrorCode, Frame, H2Error};
use http_core::h2::hpack::{Decoder, Encoder};

fn fetch_hpack() -> Option<std::path::PathBuf> {
    let (base, sha) = vectors("hpack").into_iter().next()?;
    let names: Vec<String> = std::fs::read_to_string(crate_dir().join("tests/hpack.list")).ok()?.lines().map(String::from).collect();
    let dir = cache_dir().join("hpack-8a1406e7");
    let missing: Vec<&String> = names.iter().filter(|n| !dir.join(n).exists()).collect();
    if !missing.is_empty() {
        let mut cfg = String::new();
        for n in &missing {
            let out = dir.join(n);
            std::fs::create_dir_all(out.parent().unwrap()).ok()?;
            cfg.push_str(&format!("url = \"{base}{n}\"\noutput = \"{}\"\n", out.display()));
        }
        let cfgp = dir.join("curl.cfg");
        std::fs::write(&cfgp, cfg).ok()?;
        let st = Command::new("curl").args(["-sSfL", "--parallel", "--parallel-max", "16", "--max-time", "300", "-K"]).arg(&cfgp).status();
        if !matches!(st, Ok(s) if s.success()) {
            eprintln!("SKIP (offline?): could not fetch hpack-test-case");
            return None;
        }
    }
    let mut cat = Vec::new();
    for n in &names {
        cat.extend_from_slice(n.as_bytes());
        cat.push(b'\n');
        cat.extend_from_slice(&std::fs::read(dir.join(n)).ok()?);
    }
    let catp = dir.join("all.cat");
    std::fs::write(&catp, &cat).ok()?;
    let out = Command::new("sha256sum").arg(&catp).output().ok()?;
    let got = String::from_utf8_lossy(&out.stdout).split_whitespace().next().unwrap_or("").to_string();
    let _ = std::fs::remove_file(&catp);
    if got != sha {
        eprintln!("SKIP: hpack aggregate sha256 {got} != {sha}");
        return None;
    }
    Some(dir)
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

fn headers_of(case: &Json) -> Vec<(Vec<u8>, Vec<u8>)> {
    case.get("headers")
        .map(|h| h.arr())
        .unwrap_or(&[])
        .iter()
        .map(|o| {
            let k = &o.keys()[0];
            (k.as_bytes().to_vec(), o.get(k).and_then(|v| v.str()).unwrap_or_default().into_bytes())
        })
        .collect()
}

#[test]
fn hpack_test_case_vectors() {
    let Some(dir) = fetch_hpack() else { return };
    let names: Vec<String> = std::fs::read_to_string(crate_dir().join("tests/hpack.list")).unwrap().lines().map(String::from).collect();
    let (mut blocks, mut pass, mut stories) = (0usize, 0usize, 0usize);
    let (mut rt_pass, mut raw_bytes, mut our_bytes) = (0usize, 0usize, 0usize);
    for n in &names {
        let story = parse_json(&std::fs::read_to_string(dir.join(n)).unwrap());
        stories += 1;
        let mut dec = Decoder::new(4096);
        let mut enc = Encoder::new(4096);
        let mut our_dec = Decoder::new(4096);
        for case in story.get("cases").unwrap().arr() {
            blocks += 1;
            let want = headers_of(case);
            if let Some(Json::Num(sz)) = case.get("header_table_size") {
                // The vector's encoder announced a new table size: our SETTINGS would allow it.
                dec.set_limit(*sz as usize);
            }
            let wire = unhex(&case.get("wire").unwrap().str().unwrap());
            match dec.decode(&wire) {
                Ok(got) if got == want => pass += 1,
                Ok(got) => println!("MISMATCH {n} seq {:?}: got {} fields, want {}", case.get("seqno"), got.len(), want.len()),
                Err(e) => println!("ERROR {n} seq {:?}: {e}", case.get("seqno")),
            }
            let ours = enc.encode(want.iter().map(|(a, b)| (a.as_slice(), b.as_slice())));
            raw_bytes += want.iter().map(|(a, b)| a.len() + b.len() + 4).sum::<usize>();
            our_bytes += ours.len();
            if our_dec.decode(&ours).ok().as_ref() == Some(&want) {
                rt_pass += 1;
            }
        }
    }
    println!("hpack-test-case: {pass}/{blocks} header blocks decoded exactly across {stories} stories; encoder round trip {rt_pass}/{blocks}, {our_bytes} B for {raw_bytes} B of raw fields ({:.1}%)", 100.0 * our_bytes as f64 / raw_bytes as f64);
    assert!(stories >= 150);
    assert_eq!(pass, blocks, "every vector must decode exactly");
    assert_eq!(rt_pass, blocks);
}

fn hdr(b: &[u8]) -> frame::FrameHeader {
    frame::parse_header(b[..9].try_into().unwrap())
}

fn parse(b: &[u8]) -> Result<Frame, H2Error> {
    let h = hdr(b);
    frame::parse(&h, &b[9..], 16_384)
}

#[test]
fn framing_kats() {
    // §4.1 layout: length 24 | type 8 | flags 8 | R | stream 31.
    let f = frame::encode(frame::HEADERS, frame::FLAG_END_HEADERS | frame::FLAG_END_STREAM, 1, b"\x82\x86\x84");
    assert_eq!(f, [0, 0, 3, 1, 5, 0, 0, 0, 1, 0x82, 0x86, 0x84]);
    assert_eq!(frame::PREFACE, b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n");
    // The reserved bit is ignored on receipt.
    assert_eq!(hdr(&[0, 0, 0, 4, 0, 0x80, 0, 0, 0]).stream, 0);
    // SETTINGS with two parameters, and its ACK.
    let s = frame::encode(frame::SETTINGS, 0, 0, &frame::settings_payload(&[(frame::SETTINGS_ENABLE_PUSH, 0), (frame::SETTINGS_INITIAL_WINDOW_SIZE, 65_535)]));
    assert_eq!(s, [0, 0, 12, 4, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 4, 0, 0, 0xff, 0xff]);
    assert_eq!(parse(&s), Ok(Frame::Settings { ack: false, params: vec![(2, 0), (4, 65_535)] }));
    // DATA with padding: 1 pad-length octet + 3 data + 2 padding.
    let d = [0, 0, 6, 0, 0x9, 0, 0, 0, 3, 2, b'a', b'b', b'c', 0, 0];
    assert_eq!(parse(&d), Ok(Frame::Data { stream: 3, data: b"abc".to_vec(), end_stream: true, flow_len: 6 }));
    // HEADERS with PRIORITY.
    let h = [0, 0, 6, 1, 0x24, 0, 0, 0, 5, 0x80, 0, 0, 3, 15, 0x82];
    match parse(&h).unwrap() {
        Frame::Headers { stream: 5, block, end_headers: true, priority: Some(p), .. } => {
            assert_eq!((block, p.exclusive, p.depends_on, p.weight), (vec![0x82], true, 3, 15));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(parse(&[0, 0, 8, 6, 1, 0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8]), Ok(Frame::Ping { ack: true, data: [1, 2, 3, 4, 5, 6, 7, 8] }));
    assert_eq!(
        parse(&[0, 0, 10, 7, 0, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0xb, b'h', b'i']),
        Ok(Frame::GoAway { last_stream: 7, code: ErrorCode::EnhanceYourCalm, debug: b"hi".to_vec() })
    );
    assert_eq!(parse(&[0, 0, 4, 8, 0, 0, 0, 0, 1, 0, 1, 0, 0]), Ok(Frame::WindowUpdate { stream: 1, increment: 65_536 }));
    assert_eq!(parse(&[0, 0, 1, 0xfa, 0, 0, 0, 0, 1, 9]), Ok(Frame::Unknown { ty: 0xfa, stream: 1 }));
}

#[test]
fn framing_refusals() {
    use ErrorCode::*;
    let c = |e| Err(H2Error::Connection(e));
    assert_eq!(parse(&[0, 0, 1, 0, 0, 0, 0, 0, 0, 9]), c(Protocol), "DATA on stream 0");
    assert_eq!(parse(&[0, 0, 2, 0, 8, 0, 0, 0, 1, 2, 9]), c(Protocol), "padding >= payload");
    assert_eq!(parse(&[0, 0, 5, 4, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0]), c(FrameSize), "SETTINGS not a multiple of 6");
    assert_eq!(parse(&[0, 0, 6, 4, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0]), c(FrameSize), "SETTINGS ACK with payload");
    assert_eq!(parse(&[0, 0, 6, 4, 0, 0, 0, 0, 1, 0, 1, 0, 0, 0, 0]), c(Protocol), "SETTINGS on a stream");
    assert_eq!(parse(&[0, 0, 6, 4, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 2]), c(Protocol), "ENABLE_PUSH = 2");
    assert_eq!(parse(&[0, 0, 6, 4, 0, 0, 0, 0, 0, 0, 4, 0x80, 0, 0, 0]), c(FlowControl), "INITIAL_WINDOW_SIZE 2^31");
    assert_eq!(parse(&[0, 0, 6, 4, 0, 0, 0, 0, 0, 0, 5, 0, 0, 0x3f, 0xff]), c(Protocol), "MAX_FRAME_SIZE < 2^14");
    assert_eq!(parse(&[0, 0, 7, 6, 0, 0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7]), c(FrameSize), "PING of 7");
    assert_eq!(parse(&[0, 0, 4, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0]), c(Protocol), "WINDOW_UPDATE 0 on the connection");
    assert_eq!(parse(&[0, 0, 4, 8, 0, 0, 0, 0, 3, 0, 0, 0, 0]), Err(H2Error::Stream(3, Protocol)), "WINDOW_UPDATE 0 on a stream");
    assert_eq!(parse(&[0, 0, 4, 2, 0, 0, 0, 0, 3, 0, 0, 0, 0]), Err(H2Error::Stream(3, FrameSize)), "PRIORITY of 4");
    assert_eq!(parse(&[0, 0, 3, 3, 0, 0, 0, 0, 3, 0, 0, 0]), c(FrameSize), "RST_STREAM of 3");
    assert_eq!(parse(&[0, 0, 4, 7, 0, 0, 0, 0, 0, 0, 0, 0, 0]), c(FrameSize), "GOAWAY of 4");
    assert_eq!(parse(&[0, 0, 1, 9, 0, 0, 0, 0, 0, 0x82]), c(Protocol), "CONTINUATION on stream 0");
    let big = frame::encode(frame::DATA, 0, 1, &vec![0; 16_385]);
    assert_eq!(parse(&big), c(FrameSize), "larger than SETTINGS_MAX_FRAME_SIZE");
}
