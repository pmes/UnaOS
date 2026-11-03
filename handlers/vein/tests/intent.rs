// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! VUG-VEIN-BUS (LEDGER SR509): Vug's propose exchange over the bus, answered by Vein. The live road runs against a
//! local `std::net::TcpListener` serving vein_core's own recorded fixture streams (no network); Echo and the refusals
//! never open a socket.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use bandy::{IntentEvent, IntentImage, Origin, PrefValue, SMessage, Synapse};
use vein::intent::{self, Road};
use vein::provider::ProviderSlot;
use vein::vein_core::claude::Dialect;
use vein::vein_core::{fixture, propose};

/// What the mock saw: the request line, the lower-cased headers, the body.
struct Seen {
    line: String,
    headers: Vec<(String, String)>,
    body: String,
}

/// Serve ONE connection with `HEAD_200` + `sse` and report the request.
fn mock(sse: &'static str) -> (String, mpsc::Receiver<Seen>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else { return };
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let (mut headers, mut len) = (Vec::new(), 0usize);
        loop {
            let mut h = String::new();
            reader.read_line(&mut h).unwrap();
            let h = h.trim_end();
            if h.is_empty() {
                break;
            }
            if let Some((k, v)) = h.split_once(':') {
                let (k, v) = (k.trim().to_ascii_lowercase(), v.trim().to_string());
                if k == "content-length" {
                    len = v.parse().unwrap_or(0);
                }
                headers.push((k, v));
            }
        }
        let mut body = vec![0u8; len];
        reader.read_exact(&mut body).unwrap();
        stream.write_all(fixture::HEAD_200.as_bytes()).unwrap();
        stream.write_all(sse.as_bytes()).unwrap();
        stream.flush().unwrap();
        let _ = tx.send(Seen { line: line.trim_end().to_string(), headers, body: String::from_utf8_lossy(&body).into_owned() });
    });
    (format!("http://{addr}/v1/messages"), rx)
}

fn propose_ev(request: u64, images: Vec<IntentImage>) -> IntentEvent {
    IntentEvent::Propose {
        principal: Origin::LocalUser("una".to_string()),
        context: "vug-doc version=1 lin=1e-6 ang=1e-9\nsketch s1 plane=xy".to_string(),
        measurements: "l1 len=49.7 sigma=0.4\nl3 len=49.6 sigma=0.4 angle_to_l1=0.3deg".to_string(),
        images,
        request,
    }
}

fn png() -> IntentImage {
    IntentImage { media_type: "image/png".to_string(), bytes: b"\x89PNG\r\n\x1a\nfixture".to_vec() }
}

/// A Claude slot with a key in the environment (no Holocron asked).
fn claude_slot() -> ProviderSlot {
    ProviderSlot::from_lookup(
        |k| (k == "provider").then(|| PrefValue::Str("claude".to_string())),
        |k| (k == "ANTHROPIC_API_KEY").then(|| "sk-test".to_string()),
    )
}

fn live_at(slot: &ProviderSlot, url: String) -> Road {
    match Road::resolve(None, slot) {
        Road::Live(r) => Road::Live(r.with_url(url)),
        _ => panic!("a keyed Claude slot resolves to the live road"),
    }
}

/// What the bus carries back for one Propose fired through `intent::serve` (the brain loop's arm body): the streamed
/// `Proposal`s in arrival order (VUG-VEIN-STREAM SR531), then the one terminal answer.
async fn over_the_bus(ev: IntentEvent, road: Road) -> (Vec<IntentEvent>, IntentEvent) {
    let syn = Synapse::new();
    let mut rx = syn.subscribe();
    syn.fire(SMessage::Intent(ev));
    let got = match rx.recv().await.unwrap() {
        SMessage::Intent(e) if e.is_request() => e,
        _ => panic!("the Propose comes off the bus first"),
    };
    intent::serve(syn.clone(), got, road);
    let mut streamed = Vec::new();
    loop {
        match tokio::time::timeout(Duration::from_secs(20), rx.recv()).await.expect("an answer within 20 s").unwrap() {
            SMessage::Intent(e @ IntentEvent::Proposal { .. }) => streamed.push(e),
            SMessage::Intent(e) if !e.is_request() => return (streamed, e),
            _ => continue,
        }
    }
}

const WANT: [(&str, f64); 2] = [("diameter c7 sketch=s1 circle=h1 d=6.6", 0.86), ("parallel c8 sketch=s1 a=l1 b=l3", 0.93)];

#[tokio::test]
async fn propose_round_trips_to_the_two_fixture_proposals() {
    let (url, seen) = mock(fixture::PROPOSE_CLAUDE_SSE);
    let road = live_at(&claude_slot(), url);
    let (streamed, terminal) = over_the_bus(propose_ev(7, vec![png()]), road).await;
    match terminal {
        IntentEvent::Proposals { request, proposals } => {
            assert_eq!(request, 7);
            // Each proposal streamed first, in order, with the terminal list's exact content.
            assert_eq!(streamed.len(), proposals.len());
            for (i, (e, p)) in streamed.iter().zip(&proposals).enumerate() {
                match e {
                    IntentEvent::Proposal { request: 7, index, proposal } => {
                        assert_eq!(*index as usize, i);
                        assert_eq!(proposal, p);
                    }
                    other => panic!("want Proposal {i}, got {other:?}"),
                }
            }
            let got: Vec<(&str, f64)> = proposals.iter().map(|p| (p.stmt.as_str(), p.confidence)).collect();
            assert_eq!(got, WANT);
            assert!(proposals[0].why.contains("M6 clearance"));
        }
        other => panic!("want Proposals, got {other:?}"),
    }
    let s = seen.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(s.line, "POST /v1/messages HTTP/1.1");
    let h = |k: &str| s.headers.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
    assert_eq!(h("x-api-key"), Some("sk-test"));
    assert_eq!(h("anthropic-version"), Some("2023-06-01"));
    assert!(s.body.contains(r#""type":"image""#), "the photo rides as an image block");
    assert!(s.body.contains("design-intent reader of Vug"), "vein_core's fixed system prompt");
    assert!(!s.body.contains("sk-test"), "the key is a header, never the body");
}

#[test]
fn gemini_fixture_decodes_to_the_same_two() {
    let text = intent::decode(Dialect::Gemini, fixture::PROPOSE_GEMINI_SSE.as_bytes()).unwrap();
    assert_eq!(text, fixture::PROPOSE_TEXT);
    let ps = propose::parse(&text).unwrap();
    let got: Vec<(&str, f64)> = ps.iter().map(|p| (p.stmt.as_str(), p.confidence)).collect();
    assert_eq!(got, WANT);
}

#[tokio::test]
async fn echo_answers_no_proposals_with_no_network() {
    let no_key = ProviderSlot::from_lookup(|_| None, |_| None);
    let road = Road::resolve(Some("echo"), &no_key);
    assert!(matches!(road, Road::Echo));
    match over_the_bus(propose_ev(3, vec![png()]), road).await.1 {
        IntentEvent::Proposals { request: 3, proposals } => assert!(proposals.is_empty()),
        other => panic!("want Proposals {{ request: 3, [] }}, got {other:?}"),
    }
}

/// Prose around the array, in Claude's stream shape.
const PROSE_SSE: &str = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"fx\",\"usage\":{\"input_tokens\":9,\"output_tokens\":1}}}\n\n\
event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Here you go: []\"}}\n\n\
event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n\
event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":5}}\n\n\
event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";

#[tokio::test]
async fn a_malformed_answer_is_refused_by_name() {
    let (url, _seen) = mock(PROSE_SSE);
    let road = live_at(&claude_slot(), url);
    match over_the_bus(propose_ev(9, vec![png()]), road).await.1 {
        IntentEvent::Refused { request: 9, why } => assert_eq!(why, "ProseAroundJson"),
        other => panic!("want Refused ProseAroundJson, got {other:?}"),
    }
}

#[tokio::test]
async fn an_oversize_photo_is_refused_before_any_road() {
    let big = IntentImage { media_type: "image/jpeg".to_string(), bytes: vec![0xff; propose::IMAGE_BYTES_MAX + 1] };
    match intent::answer(&propose_ev(11, vec![big]), &Road::Echo).await {
        Some(IntentEvent::Refused { request: 11, why }) => assert_eq!(why, "ImageTooLarge(0)"),
        other => panic!("want Refused ImageTooLarge(0), got {other:?}"),
    }
    let gif = IntentImage { media_type: "image/gif".to_string(), bytes: vec![1] };
    match intent::answer(&propose_ev(12, vec![png(), gif]), &Road::Echo).await {
        Some(IntentEvent::Refused { request: 12, why }) => assert_eq!(why, "MediaType(1)"),
        other => panic!("want Refused MediaType(1), got {other:?}"),
    }
}

#[tokio::test]
async fn no_key_is_refused_naming_the_fix_and_answers_are_inert() {
    let no_key = ProviderSlot::from_lookup(|_| None, |_| None);
    let road = Road::resolve(None, &no_key);
    match intent::answer(&propose_ev(13, vec![]), &road).await {
        Some(IntentEvent::Refused { request: 13, why }) => {
            assert_eq!(why, "NoProvider: set ANTHROPIC_API_KEY, or choose a provider in Settings")
        }
        other => panic!("want Refused NoProvider, got {other:?}"),
    }
    let answer = IntentEvent::Refused { request: 13, why: "x".to_string() };
    assert!(!answer.is_request());
    assert!(intent::answer(&answer, &Road::Echo).await.is_none());
}

// --- VUG-VEIN-STREAM (SR531): the stream parsed as it arrives, KATs over every byte split ---

/// `sse` in `dialect` fed as `pieces`: every proposal streamed, the count after each piece, and the terminal verdict.
fn stream(dialect: Dialect, pieces: &[&[u8]]) -> (Vec<propose::Proposal>, Vec<usize>, Result<Vec<propose::Proposal>, String>) {
    let mut line = vec![0u8; 256 * 1024];
    let mut st = intent::ProposeStream::new(&mut line, dialect);
    let (mut got, mut counts) = (Vec::new(), Vec::new());
    for p in pieces {
        got.extend(st.feed(p));
        counts.push(st.yielded());
    }
    (got, counts, st.finish())
}

/// Two chunks at EVERY byte boundary, and one byte at a time: the same proposals in order, the same verdict.
fn every_split(dialect: Dialect, sse: &str, want_items: &[propose::Proposal], want_fin: &Result<Vec<propose::Proposal>, String>) {
    let b = sse.as_bytes();
    for i in 0..=b.len() {
        let (got, _, fin) = stream(dialect, &[&b[..i], &b[i..]]);
        assert_eq!(got, want_items, "split at byte {i}");
        assert_eq!(&fin, want_fin, "split at byte {i}");
    }
    let bytes: Vec<&[u8]> = b.chunks(1).collect();
    let (got, _, fin) = stream(dialect, &bytes);
    assert_eq!(got, want_items, "one byte at a time");
    assert_eq!(&fin, want_fin, "one byte at a time");
}

/// The first proposal is out once the stream's second text delta (the one carrying its closing brace) has arrived.
fn first_before_last(dialect: Dialect, sse: &str, delta2_marker: &str) {
    let b = sse.as_bytes();
    let at = sse.find(delta2_marker).unwrap();
    let end2 = at + sse[at..].find('\n').unwrap() + 1;
    let (_, counts, fin) = stream(dialect, &[&b[..end2], &b[end2..]]);
    assert_eq!(counts, vec![1, 2], "one proposal after the second delta, both at the end");
    assert_eq!(fin.unwrap().len(), 2);
}

#[test]
fn claude_fixture_streams_its_proposals_at_every_byte_split() {
    let whole = propose::parse(fixture::PROPOSE_TEXT).unwrap();
    every_split(Dialect::Claude, fixture::PROPOSE_CLAUDE_SSE, &whole, &Ok(whole.clone()));
    let got: Vec<(&str, f64)> = whole.iter().map(|p| (p.stmt.as_str(), p.confidence)).collect();
    assert_eq!(got, WANT);
    first_before_last(Dialect::Claude, fixture::PROPOSE_CLAUDE_SSE, "M6 clearance diameter.");
}

#[test]
fn gemini_fixture_streams_its_proposals_at_every_byte_split() {
    let whole = propose::parse(fixture::PROPOSE_TEXT).unwrap();
    every_split(Dialect::Gemini, fixture::PROPOSE_GEMINI_SSE, &whole, &Ok(whole.clone()));
    first_before_last(Dialect::Gemini, fixture::PROPOSE_GEMINI_SSE, "M6 clearance diameter.");
}

/// The fixture's two deltas carrying the whole array, then a third delta of prose, in Claude's stream shape.
fn array_then_prose_sse() -> String {
    let delta = |t: &str| {
        let mut esc = String::new();
        for c in t.chars() {
            match c {
                '"' => esc.push_str("\\\""),
                '\\' => esc.push_str("\\\\"),
                c => esc.push(c),
            }
        }
        format!("event: content_block_delta\ndata: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":\"{esc}\"}}}}\n\n")
    };
    let cut = fixture::PROPOSE_TEXT.find("},{").unwrap() + 1;
    let start = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"fx\",\"usage\":{\"input_tokens\":9,\"output_tokens\":1}}}\n\n\
event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n";
    let end = "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n\
event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":5}}\n\n\
event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
    format!(
        "{start}{}{}{}{end}",
        delta(&fixture::PROPOSE_TEXT[..cut]),
        delta(&fixture::PROPOSE_TEXT[cut..]),
        delta(" Hope this helps.")
    )
}

#[test]
fn prose_after_the_array_is_refused_after_the_streamed_two() {
    let sse = array_then_prose_sse();
    let whole = propose::parse(fixture::PROPOSE_TEXT).unwrap();
    assert_eq!(intent::decode(Dialect::Claude, sse.as_bytes()).unwrap(), format!("{} Hope this helps.", fixture::PROPOSE_TEXT));
    every_split(Dialect::Claude, &sse, &whole, &Err("ProseAroundJson".to_string()));
}

#[tokio::test]
async fn prose_after_the_array_over_the_bus_names_what_arrived() {
    let sse: &'static str = Box::leak(array_then_prose_sse().into_boxed_str());
    let (url, _seen) = mock(sse);
    let road = live_at(&claude_slot(), url);
    let (streamed, terminal) = over_the_bus(propose_ev(21, vec![png()]), road).await;
    assert_eq!(streamed.len(), 2);
    match terminal {
        IntentEvent::Refused { request: 21, why } => assert_eq!(why, "ProseAroundJson after 2 proposals"),
        other => panic!("want Refused after 2, got {other:?}"),
    }
}
