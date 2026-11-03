// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! VUGVEINASK (LEDGER SR504): the propose exchange — the request bytes for a one-image propose (both dialects,
//! byte-exact against hand-written expectations), the encode refusals, both recorded fixture streams through the
//! client to two proposals, every named parse refusal on its malformed answer, and Echo's `[]`.

use vein_core::Out;
use vein_core::claude::{Dialect, Event, Stop};
use vein_core::propose::*;
use vein_core::{client, fixture};

fn b64(d: &[u8]) -> String {
    let mut b = [0u8; 64];
    let mut o = Out::new(&mut b);
    base64_into(d, &mut o);
    let n = o.done().unwrap();
    String::from_utf8(b[..n].to_vec()).unwrap()
}

#[test]
fn base64_rfc4648_vectors() {
    // RFC 4648 §10.
    for (i, o) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")] {
        assert_eq!(b64(i.as_bytes()), o, "{i:?}");
    }
    assert_eq!(b64(b"\x89PNG"), "iVBORw==");
}

const PNG: &[u8] = b"\x89PNG";
const CTX: &str = "sketch s1 o=[0 0 0] z=[0 0 1] x=[1 0 0]";
const MEAS: &str = "h1 circle d=6.58 sigma=0.4";

/// SYSTEM as it appears inside a JSON string, written WITHOUT the escaper under test: SYSTEM has no backslash and
/// no control character, so only its quotes change.
fn sys_json() -> String {
    assert!(!SYSTEM.contains('\\') && !SYSTEM.chars().any(|c| c.is_control()));
    SYSTEM.replace('"', "\\\"")
}

fn one_image<'a>(imgs: &'a [ImageRef<'a>]) -> Propose<'a> {
    Propose { context: CTX, measurements: MEAS, images: imgs }
}

fn enc(p: &Propose<'_>, d: Dialect, model: &str) -> String {
    let mut b = vec![0u8; 8192];
    let n = encode(p, d, model, &mut b).unwrap();
    String::from_utf8(b[..n].to_vec()).unwrap()
}

#[test]
fn claude_one_image_request_is_byte_exact() {
    let imgs = [ImageRef { media_type: "image/png", bytes: PNG }];
    let p = one_image(&imgs);
    let want = String::from(r#"{"model":"claude-opus-5-5","max_tokens":4096,"stream":true,"fallbacks":"default","system":""#)
        + &sys_json()
        + r#"","messages":[{"role":"user","content":[{"type":"image","source":{"type":"base64","media_type":"image/png","data":"iVBORw=="}},{"type":"text","text":"Design document (vug-doc):\nsketch s1 o=[0 0 0] z=[0 0 1] x=[1 0 0]\n\nMeasurements:\nh1 circle d=6.58 sigma=0.4\n"}]}]}"#;
    assert_eq!(enc(&p, Dialect::Claude, "claude-opus-5-5"), want);
    assert_eq!(request(&p, Dialect::Claude, "claude-opus-5-5").unwrap(), want.as_bytes());
}

#[test]
fn gemini_one_image_request_is_byte_exact() {
    let imgs = [ImageRef { media_type: "image/png", bytes: PNG }];
    let p = one_image(&imgs);
    let want = String::from(r#"{"systemInstruction":{"parts":[{"text":""#)
        + &sys_json()
        + r#""}]},"contents":[{"role":"user","parts":[{"inlineData":{"mimeType":"image/png","data":"iVBORw=="}},{"text":"Design document (vug-doc):\nsketch s1 o=[0 0 0] z=[0 0 1] x=[1 0 0]\n\nMeasurements:\nh1 circle d=6.58 sigma=0.4\n"}]}],"generationConfig":{"maxOutputTokens":4096,"responseMimeType":"application/json"}}"#;
    assert_eq!(enc(&p, Dialect::Gemini, "gemini-2.5-flash"), want);
    assert_eq!(request(&p, Dialect::Gemini, "gemini-2.5-flash").unwrap(), want.as_bytes());
}

fn refuse(imgs: &[ImageRef<'_>], b: &mut [u8]) -> AskRefusal {
    encode(&one_image(imgs), Dialect::Claude, "claude-opus-5-5", b).unwrap_err()
}

#[test]
fn encode_refusals_are_named() {
    let mut b = vec![0u8; 8192];
    assert_eq!(refuse(&[ImageRef { media_type: "image/tiff", bytes: PNG }], &mut b), AskRefusal::MediaType(0));
    let ok = ImageRef { media_type: "image/jpeg", bytes: PNG };
    assert_eq!(refuse(&[ok, ImageRef { media_type: "image/gif", bytes: PNG }], &mut b), AskRefusal::MediaType(1));
    assert_eq!(refuse(&[ImageRef { media_type: "image/png", bytes: b"" }], &mut b), AskRefusal::EmptyImage(0));
    let big = vec![0u8; IMAGE_BYTES_MAX + 1];
    assert_eq!(refuse(&[ImageRef { media_type: "image/png", bytes: &big }], &mut b), AskRefusal::ImageTooLarge(0));
    let full = ImageRef { media_type: "image/webp", bytes: &big[..IMAGE_BYTES_MAX] };
    assert_eq!(refuse(&[full; 4], &mut b), AskRefusal::ImagesTooLarge);
    assert_eq!(refuse(&[ok; IMAGES_MAX + 1], &mut b), AskRefusal::TooManyImages);
    let mut tiny = [0u8; 16];
    assert_eq!(refuse(&[ok], &mut tiny), AskRefusal::OutTooSmall);
    assert!(encode(&one_image(&[ok; IMAGES_MAX]), Dialect::Gemini, "gemini-2.5-flash", &mut b).is_ok());
    assert_eq!(AskRefusal::MediaType(0).name(), "MediaType");
}

#[test]
fn both_fixture_streams_parse_to_two_proposals() {
    for (d, sse) in [(Dialect::Claude, fixture::PROPOSE_CLAUDE_SSE), (Dialect::Gemini, fixture::PROPOSE_GEMINI_SSE)] {
        let mut t = fixture::Replay::new([fixture::HEAD_200.as_bytes(), sse.as_bytes(), b"", b""], 7);
        let (mut rx, mut line) = ([0u8; 512], [0u8; 1024]);
        let (mut text, mut pieces) = (String::new(), 0);
        let o = client::exchange_dialect(&mut t, d, b"HEAD", b"BODY", &mut rx, &mut line, &mut |e| {
            if let Event::Text(s) = e {
                text.push_str(s);
                pieces += 1;
            }
        });
        assert_eq!((o.status, o.stop, o.fail), (200, Some(Stop::EndTurn), None), "{d:?}");
        assert!(pieces >= 3, "{d:?}");
        assert_eq!(text, fixture::PROPOSE_TEXT, "{d:?}");
        let ps = parse(&text).unwrap();
        assert_eq!(
            ps,
            vec![
                Proposal { stmt: "diameter c7 sketch=s1 circle=h1 d=6.6".into(), confidence: 0.86, why: "The hole measures 6.58 +/- 0.4 mm, the M6 clearance diameter.".into() },
                Proposal {
                    stmt: "parallel c8 sketch=s1 a=l1 b=l3".into(),
                    confidence: 0.93,
                    why: "The two long edges measure 0.3 deg apart, inside the outline's 0.5 deg uncertainty.".into()
                },
            ]
        );
    }
}

/// One answer object from raw JSON value texts.
fn el(stmt: &str, conf: &str, why: &str) -> String {
    format!(r#"{{"stmt":{stmt},"confidence":{conf},"why":{why}}}"#)
}

const S: &str = r#""parallel c8 sketch=s1 a=l1 b=l3""#;
const W: &str = r#""Edges 0.3 deg apart.""#;

fn good() -> String {
    el(S, "0.93", W)
}

fn refused(answer: &str) -> Refusal {
    parse(answer).expect_err(answer)
}

#[test]
fn echo_and_empty_answers_are_no_proposals() {
    assert_eq!(parse(ECHO_ANSWER), Ok(vec![]));
    assert_eq!(parse(" \n[ ]\n"), Ok(vec![]));
}

#[test]
fn a_pretty_printed_array_is_taken() {
    let a = "\n[\n  {\n    \"stmt\": \"parallel c8 sketch=s1 a=l1 b=l3\",\n    \"confidence\": 1,\n    \"why\": \"Edges 0.3 deg apart.\"\n  },\n  {\"why\":\"Round.\",\"confidence\":5e-1,\"stmt\":\"diameter c7 sketch=s1 circle=h1 d=6.6\"}\n]\n";
    let ps = parse(a).unwrap();
    assert_eq!(ps.len(), 2);
    assert_eq!((ps[0].confidence, ps[1].confidence), (1.0, 0.5));
    assert_eq!(ps[1].stmt, "diameter c7 sketch=s1 circle=h1 d=6.6");
}

#[test]
fn not_an_array_and_prose_around_json() {
    assert_eq!(refused(""), Refusal::NotAnArray);
    assert_eq!(refused("I see a bracket."), Refusal::NotAnArray);
    assert_eq!(refused(&good()), Refusal::NotAnArray);
    assert_eq!(refused(&format!("Here are my proposals: [{}]", good())), Refusal::ProseAroundJson);
    assert_eq!(refused(&format!("```json\n[{}]\n```", good())), Refusal::ProseAroundJson);
    assert_eq!(refused(&format!("[{}] Hope this helps.", good())), Refusal::ProseAroundJson);
    assert_eq!(Refusal::ProseAroundJson.name(), "ProseAroundJson");
}

#[test]
fn bad_json_shapes() {
    let g = good();
    assert_eq!(refused("[{"), Refusal::BadJson);
    assert_eq!(refused("[1}"), Refusal::BadJson);
    assert_eq!(refused(&format!("[{g} {g}]")), Refusal::BadJson);
    assert_eq!(refused(&format!("[{g},,{g}]")), Refusal::BadJson);
    assert_eq!(refused(&format!("[{g},]")), Refusal::BadJson);
    assert_eq!(refused(&format!("[,{g}]")), Refusal::BadJson);
    assert_eq!(refused(&format!(r#"[{{"stmt":{S} "confidence":0.5,"why":{W}}}]"#)), Refusal::BadJson);
    assert_eq!(refused(&format!(r#"[{{"stmt":{S},"confidence":0.5,"why":{W},}}]"#)), Refusal::BadJson);
    assert_eq!(refused(&format!(r#"[{{"stmt":{S},"confidence":0.5,"why":{W} x}}]"#)), Refusal::BadJson);
}

#[test]
fn objects_keys_and_values_are_refused_by_name() {
    let g = good();
    assert_eq!(refused("[1]"), Refusal::NotAnObject(0));
    assert_eq!(refused(&format!("[{g},\"x\"]")), Refusal::NotAnObject(1));
    assert_eq!(refused(&format!(r#"[{{"stmt":{S},"confidence":0.5,"why":{W},"x":1}}]"#)), Refusal::ExtraKey(0));
    assert_eq!(refused(&format!(r#"[{{"stmt":{S},"stmt":{S},"confidence":0.5,"why":{W}}}]"#)), Refusal::ExtraKey(0));
    assert_eq!(refused(&format!(r#"[{g},{{"stmt":{S},"confidence":0.5}}]"#)), Refusal::MissingKey(1));
    for c in ["1.5", "-0.1", "\"0.9\"", "01", ".5", "1e400", "null"] {
        assert_eq!(refused(&format!("[{}]", el(S, c, W))), Refusal::BadConfidence(0), "{c}");
    }
    assert_eq!(refused(&format!("[{}]", el("5", "0.5", W))), Refusal::NotAString(0));
    assert_eq!(refused(&format!("[{}]", el(S, "0.5", "[]"))), Refusal::NotAString(0));
    assert_eq!(refused(&format!("[{}]", el(r#""parallel c8\nsketch=s1 a=l1 b=l3""#, "0.5", W))), Refusal::MultiLineStmt(0));
    for s in [r#""Parallel c8 sketch=s1 a=l1 b=l3""#, r#""""#, r#"" parallel c8""#, r#""9x c1""#, r#""x=1""#] {
        assert_eq!(refused(&format!("[{}]", el(s, "0.5", W))), Refusal::BadStmt(0), "{s}");
    }
    for w in [r#""""#, r#""   ""#, r#""one\ntwo""#] {
        assert_eq!(refused(&format!("[{}]", el(S, "0.5", w))), Refusal::BadWhy(0), "{w}");
    }
    let many = |n: usize| format!("[{}]", vec![g.clone(); n].join(","));
    assert_eq!(parse(&many(PROPOSALS_MAX)).unwrap().len(), PROPOSALS_MAX);
    assert_eq!(refused(&many(PROPOSALS_MAX + 1)), Refusal::TooMany);
}
