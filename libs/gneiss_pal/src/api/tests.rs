// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Lesser General Public License for more details.
//
// You should have received a copy of the GNU Lesser General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! Provider tests against a local mock HTTP server (a `std::net::TcpListener`
//! fixture). No live network.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;

use futures_util::StreamExt;
use serde_json::{Value, json};

use super::claude::{ClaudeSse, parse_message};
use super::provider::*;
use super::sse::{SseDecoder, SseHandler};
use super::*;

#[derive(Debug)]
struct Captured {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl Captured {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).expect("request body is JSON")
    }
}

/// Serve `responses` (raw HTTP/1.1 responses) to successive connections, one
/// each, and report every request seen.
fn mock(responses: Vec<String>) -> (String, mpsc::Receiver<Captured>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for resp in responses {
            let (stream, _) = match listener.accept() {
                Ok(s) => s,
                Err(_) => return,
            };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let mut it = line.split_whitespace();
            let method = it.next().unwrap_or("").to_string();
            let path = it.next().unwrap_or("").to_string();
            let mut headers = Vec::new();
            let mut len = 0usize;
            loop {
                let mut h = String::new();
                reader.read_line(&mut h).unwrap();
                let h = h.trim_end();
                if h.is_empty() {
                    break;
                }
                if let Some((k, v)) = h.split_once(':') {
                    let k = k.trim().to_ascii_lowercase();
                    let v = v.trim().to_string();
                    if k == "content-length" {
                        len = v.parse().unwrap();
                    }
                    headers.push((k, v));
                }
            }
            let mut body = vec![0u8; len];
            reader.read_exact(&mut body).unwrap();
            let _ = tx.send(Captured { method, path, headers, body: String::from_utf8(body).unwrap() });
            let mut s = stream;
            s.write_all(resp.as_bytes()).unwrap();
            s.flush().unwrap();
        }
    });
    (format!("http://{addr}"), rx)
}

fn http(status: &str, extra: &[(&str, &str)], ctype: &str, body: &str) -> String {
    let mut r = format!("HTTP/1.1 {status}\r\ncontent-type: {ctype}\r\ncontent-length: {}\r\nconnection: close\r\n", body.len());
    for (k, v) in extra {
        r.push_str(&format!("{k}: {v}\r\n"));
    }
    r.push_str("\r\n");
    r.push_str(body);
    r
}

fn ok_json(v: &Value) -> String {
    http("200 OK", &[], "application/json", &v.to_string())
}

fn claude(base: &str, fallbacks: bool) -> ClaudeProvider {
    ClaudeProvider::new("sk-test-key".into(), "claude-opus-5-5".into(), fallbacks)
        .unwrap()
        .with_base_url(base)
        .with_retry(RetryPolicy::fast())
}

fn req() -> ChatRequest {
    ChatRequest::single(Some("You are Una.".into()), vec![Part::text("hello".into())])
}

fn end_turn_body() -> Value {
    json!({
        "id": "msg_1", "type": "message", "role": "assistant", "model": "claude-opus-5-5",
        "content": [
            {"type": "thinking", "thinking": "", "signature": "x"},
            {"type": "text", "text": "Hello, "},
            {"type": "text", "text": "world."}
        ],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 12, "output_tokens": 5}
    })
}

#[tokio::test]
async fn claude_request_shape_golden() {
    let (base, rx) = mock(vec![ok_json(&end_turn_body())]);
    let p = claude(&base, true);
    p.generate(&req()).await.unwrap();
    let c = rx.recv().unwrap();
    assert_eq!(c.method, "POST");
    assert_eq!(c.path, "/v1/messages");
    assert_eq!(c.header("x-api-key"), Some("sk-test-key"));
    assert_eq!(c.header("anthropic-version"), Some("2023-06-01"));
    assert_eq!(c.header("content-type"), Some("application/json"));
    assert_eq!(c.header("anthropic-beta"), Some("server-side-fallback-2026-07-01"));
    assert_eq!(
        c.json(),
        json!({
            "model": "claude-opus-5-5",
            "max_tokens": 16000,
            "system": "You are Una.",
            "messages": [{"role": "user", "content": [{"type": "text", "text": "hello"}]}],
            "fallbacks": "default"
        })
    );
    // Never a thinking field, never a prefill.
    assert!(c.json().get("thinking").is_none());
}

#[tokio::test]
async fn claude_fallbacks_off_sends_neither_header_nor_field() {
    let (base, rx) = mock(vec![ok_json(&end_turn_body())]);
    claude(&base, false).generate(&req()).await.unwrap();
    let c = rx.recv().unwrap();
    assert_eq!(c.header("anthropic-beta"), None);
    assert!(c.json().get("fallbacks").is_none());
}

#[tokio::test]
async fn claude_non_streaming_parse() {
    let (base, _rx) = mock(vec![ok_json(&end_turn_body())]);
    let r = claude(&base, true).generate(&req()).await.unwrap();
    assert_eq!(r.stop, StopReason::EndTurn);
    assert_eq!(r.text, "Hello, world.");
    assert_eq!(r.usage, Usage { input_tokens: 12, output_tokens: 5 });
}

#[tokio::test]
async fn claude_refusal() {
    let body = json!({
        "type": "message", "role": "assistant", "content": [],
        "stop_reason": "refusal", "stop_details": {"category": "cyber"},
        "usage": {"input_tokens": 3, "output_tokens": 0}
    });
    let (base, _rx) = mock(vec![ok_json(&body)]);
    let r = claude(&base, true).generate(&req()).await.unwrap();
    assert_eq!(r.stop, StopReason::Refusal { category: Some("cyber".into()) });
    assert_eq!(r.text, "");
    let r = parse_message(&json!({"content": [], "stop_reason": "max_tokens"})).unwrap();
    assert_eq!(r.stop, StopReason::MaxTokens);
}

#[tokio::test]
async fn claude_429_honours_retry_after_then_succeeds() {
    let limited = http(
        "429 Too Many Requests",
        &[("retry-after", "0")],
        "application/json",
        r#"{"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#,
    );
    let (base, rx) = mock(vec![limited, ok_json(&end_turn_body())]);
    let r = claude(&base, true).generate(&req()).await.unwrap();
    assert_eq!(r.text, "Hello, world.");
    assert_eq!(rx.try_iter().count(), 2, "one 429, one retry");
}

#[tokio::test]
async fn claude_429_exhausted_is_retryable_error() {
    let limited = || http("429 Too Many Requests", &[("retry-after", "0")], "application/json",
        r#"{"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#);
    // fast policy: 1 attempt + 3 retries.
    let (base, rx) = mock(vec![limited(), limited(), limited(), limited()]);
    let e = claude(&base, true).generate(&req()).await.unwrap_err();
    assert_eq!(e, ProviderError::Retryable { status: 429, message: "slow down".into() });
    assert_eq!(rx.try_iter().count(), 4);
}

#[tokio::test]
async fn claude_400_and_401_are_final() {
    for (status, code) in [("400 Bad Request", 400u16), ("401 Unauthorized", 401)] {
        let resp = http(status, &[], "application/json",
            r#"{"type":"error","error":{"type":"invalid_request_error","message":"nope"}}"#);
        let (base, rx) = mock(vec![resp]);
        let e = claude(&base, true).generate(&req()).await.unwrap_err();
        assert_eq!(e, ProviderError::Request { status: code, message: "nope".into() });
        assert_eq!(rx.try_iter().count(), 1, "no retry on {code}");
    }
}

#[tokio::test]
async fn claude_5xx_is_retried() {
    let (base, rx) = mock(vec![
        http("529 Overloaded", &[], "application/json", r#"{"type":"error","error":{"type":"overloaded_error","message":"busy"}}"#),
        ok_json(&end_turn_body()),
    ]);
    assert!(claude(&base, true).generate(&req()).await.is_ok());
    assert_eq!(rx.try_iter().count(), 2);
}

/// A captured-shape Messages stream (thinking block, two text deltas, ping).
const CLAUDE_SSE: &str = "event: message_start\n\
data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"claude-opus-5-5\",\"stop_reason\":null,\"usage\":{\"input_tokens\":25,\"output_tokens\":1}}}\n\n\
event: content_block_start\n\
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"hmm\"}}\n\n\
event: content_block_stop\n\
data: {\"type\":\"content_block_stop\",\"index\":0}\n\n\
event: ping\n\
data: {\"type\":\"ping\"}\n\n\
event: content_block_start\n\
data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\" there.\"}}\n\n\
event: content_block_stop\n\
data: {\"type\":\"content_block_stop\",\"index\":1}\n\n\
event: message_delta\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":15}}\n\n\
event: message_stop\n\
data: {\"type\":\"message_stop\"}\n\n";

#[test]
fn claude_sse_fixture_parse_pure() {
    let mut d = SseDecoder::new();
    let mut h = ClaudeSse::default();
    let mut out = Vec::new();
    // Feed in awkward 7-byte slices.
    for chunk in CLAUDE_SSE.as_bytes().chunks(7) {
        for ev in d.push(chunk) {
            out.extend(h.on_event(&ev).into_iter().map(Result::unwrap));
        }
    }
    assert!(h.done());
    assert_eq!(
        out,
        vec![
            ChatDelta::Text("Hello".into()),
            ChatDelta::Text(" there.".into()),
            ChatDelta::Stop { stop: StopReason::EndTurn, usage: Usage { input_tokens: 25, output_tokens: 15 } },
        ]
    );
}

#[tokio::test]
async fn claude_stream_over_http() {
    let (base, rx) = mock(vec![http("200 OK", &[], "text/event-stream", CLAUDE_SSE)]);
    let p = claude(&base, true);
    let r = req();
    let s = p.stream(&r).await.unwrap();
    let deltas: Vec<_> = s.map(Result::unwrap).collect().await;
    assert_eq!(deltas.len(), 3);
    assert_eq!(rx.recv().unwrap().json()["stream"], json!(true));
    let s = {
        let (base, _rx) = mock(vec![http("200 OK", &[], "text/event-stream", CLAUDE_SSE)]);
        let p = claude(&base, true);
        let r = sse::collect(p.stream(&r).await.unwrap()).await.unwrap();
        r
    };
    assert_eq!(s.text, "Hello there.");
    assert_eq!(s.stop, StopReason::EndTurn);
}

#[test]
fn claude_sse_refusal_and_truncated_stream() {
    let mut h = ClaudeSse::default();
    let ev = |d: &str| super::sse::SseEvent { event: None, data: d.into() };
    assert!(h.on_event(&ev(r#"{"type":"message_delta","delta":{"stop_reason":"refusal","stop_details":{"category":"bio"}},"usage":{"output_tokens":0}}"#)).is_empty());
    let out = h.on_event(&ev(r#"{"type":"message_stop"}"#));
    assert_eq!(
        out,
        vec![Ok(ChatDelta::Stop { stop: StopReason::Refusal { category: Some("bio".into()) }, usage: Usage::default() })]
    );
    let mut h = ClaudeSse::default();
    assert!(matches!(h.on_end()[0], Err(ProviderError::Retryable { .. })));
    let out = h.on_event(&ev(r#"{"type":"error","error":{"type":"overloaded_error","message":"busy"}}"#));
    assert_eq!(out, vec![Err(ProviderError::Retryable { status: 529, message: "busy".into() })]);
}

#[test]
fn claude_attachments_never_silently_dropped() {
    let p = ClaudeProvider::new("k".into(), "claude-opus-5-5".into(), true).unwrap();
    let mk = |mime: &str, uri: &str| {
        ChatRequest::single(None, vec![Part::text("see".into()), Part::file_data(mime.into(), uri.into())])
    };
    let b = p.build_body(&mk("application/pdf", "https://example.org/a.pdf"), false).unwrap();
    assert_eq!(b["messages"][0]["content"][1], json!({"type":"document","source":{"type":"url","url":"https://example.org/a.pdf"}}));
    let b = p.build_body(&mk("image/png", "https://example.org/a.png"), false).unwrap();
    assert_eq!(b["messages"][0]["content"][1]["type"], json!("image"));
    let e = p.build_body(&mk("application/pdf", "gs://bucket/a.pdf"), false).unwrap_err();
    assert!(matches!(e, ProviderError::Unsupported(ref m) if m.contains("gs://bucket/a.pdf")), "{e:?}");
    // Whitespace-only text parts are skipped (the API rejects them); an all-blank message is an error.
    let blank = ChatRequest::single(None, vec![Part::text(" ".into())]);
    assert!(matches!(p.build_body(&blank, false), Err(ProviderError::Unsupported(_))));
}

// --- Gemini ---

fn gemini_cfg(auth: GeminiAuth) -> GeminiConfig {
    GeminiConfig {
        model: "gemini-test-model".into(),
        auth,
        settings: GeminiSettings { project: Some("proj-x".into()), region: "europe-west4".into(), ..Default::default() },
        temperature: None,
    }
}

#[test]
fn gemini_urls_come_from_configuration() {
    let p = GeminiProvider::with_token(gemini_cfg(GeminiAuth::GcloudAdc), "t".into()).unwrap();
    assert_eq!(
        p.url("gemini-test-model", "generateContent", false),
        "https://europe-west4-aiplatform.googleapis.com/v1beta1/projects/proj-x/locations/europe-west4/publishers/google/models/gemini-test-model:generateContent"
    );
    assert_eq!(
        p.url("text-embedding-004", "predict", true),
        "https://us-central1-aiplatform.googleapis.com/v1beta1/projects/proj-x/locations/us-central1/publishers/google/models/text-embedding-004:predict"
    );
    let mut cfg = gemini_cfg(GeminiAuth::GcloudAdc);
    cfg.settings.region = "global".into();
    let p = GeminiProvider::with_token(cfg, "t".into()).unwrap();
    assert!(p.url("m", "generateContent", false).starts_with("https://aiplatform.googleapis.com/v1beta1/projects/proj-x/locations/global/"));
    let p = GeminiProvider::with_token(gemini_cfg(GeminiAuth::ApiKey("k".into())), String::new()).unwrap();
    assert_eq!(
        p.url("gemini-test-model", "generateContent", false),
        "https://generativelanguage.googleapis.com/v1beta/models/gemini-test-model:generateContent"
    );
}

#[tokio::test]
async fn gemini_request_shape_and_parse() {
    let body = json!({
        "candidates": [{"content": {"role": "model", "parts": [
            {"text": "thinking...", "thought": true}, {"text": "Hi "}, {"text": "there"}
        ]}, "finishReason": "STOP"}],
        "usageMetadata": {"promptTokenCount": 7, "candidatesTokenCount": 2, "totalTokenCount": 9}
    });
    let (base, rx) = mock(vec![ok_json(&body)]);
    let p = GeminiProvider::with_token(gemini_cfg(GeminiAuth::GcloudAdc), "tok-1".into())
        .unwrap()
        .with_base_url(&base)
        .with_retry(RetryPolicy::fast());
    let mut r = ChatRequest::single(Some("sys".into()), vec![Part::text("q".into()), Part::file_data("application/pdf".into(), "gs://b/f.pdf".into())]);
    r.max_tokens = 0;
    let out = p.generate(&r).await.unwrap();
    assert_eq!(out, ChatResponse { text: "Hi there".into(), stop: StopReason::EndTurn, usage: Usage { input_tokens: 7, output_tokens: 2 } });
    let c = rx.recv().unwrap();
    assert_eq!(c.path, "/v1beta1/projects/proj-x/locations/europe-west4/publishers/google/models/gemini-test-model:generateContent");
    assert_eq!(c.header("authorization"), Some("Bearer tok-1"));
    assert_eq!(
        c.json(),
        json!({
            "contents": [{"role": "user", "parts": [{"text": "q"}, {"fileData": {"mimeType": "application/pdf", "fileUri": "gs://b/f.pdf"}}]}],
            "systemInstruction": {"parts": [{"text": "sys"}]},
            "generationConfig": {"temperature": 0.4}
        })
    );
}

#[tokio::test]
async fn gemini_api_key_mode_and_block_is_refusal() {
    let body = json!({"promptFeedback": {"blockReason": "SAFETY"}});
    let (base, rx) = mock(vec![ok_json(&body)]);
    let p = GeminiProvider::with_token(gemini_cfg(GeminiAuth::ApiKey("gk".into())), String::new())
        .unwrap()
        .with_base_url(&base);
    let out = p.generate(&req()).await.unwrap();
    assert_eq!(out.stop, StopReason::Refusal { category: Some("SAFETY".into()) });
    let c = rx.recv().unwrap();
    assert_eq!(c.path, "/v1beta/models/gemini-test-model:generateContent");
    assert_eq!(c.header("x-goog-api-key"), Some("gk"));
    assert_eq!(c.header("authorization"), None);
    assert_eq!(c.json()["generationConfig"]["maxOutputTokens"], json!(16000));
}

#[tokio::test]
async fn gemini_stream_over_http() {
    let sse = "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Hel\"}]}}]}\r\n\r\n\
data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"lo\"}]},\"finishReason\":\"MAX_TOKENS\"}],\"usageMetadata\":{\"promptTokenCount\":4,\"candidatesTokenCount\":2}}\r\n\r\n";
    let (base, rx) = mock(vec![http("200 OK", &[], "text/event-stream", sse)]);
    let p = GeminiProvider::with_token(gemini_cfg(GeminiAuth::ApiKey("gk".into())), String::new()).unwrap().with_base_url(&base);
    let r = req();
    let out = sse::collect(p.stream(&r).await.unwrap()).await.unwrap();
    assert_eq!(out, ChatResponse { text: "Hello".into(), stop: StopReason::MaxTokens, usage: Usage { input_tokens: 4, output_tokens: 2 } });
    assert_eq!(rx.recv().unwrap().path, "/v1beta/models/gemini-test-model:streamGenerateContent?alt=sse");
}

#[tokio::test]
async fn gemini_embed_both_modes() {
    let (base, rx) = mock(vec![ok_json(&json!({"predictions": [{"embeddings": {"values": [0.5, 0.25]}}]}))]);
    let p = GeminiProvider::with_token(gemini_cfg(GeminiAuth::GcloudAdc), "t".into()).unwrap().with_base_url(&base);
    assert_eq!(p.embed("x").await.unwrap(), vec![0.5, 0.25]);
    let c = rx.recv().unwrap();
    assert!(c.path.ends_with("/locations/us-central1/publishers/google/models/text-embedding-004:predict"), "{}", c.path);
    assert_eq!(c.json(), json!({"instances": [{"content": "x"}]}));

    let (base, rx) = mock(vec![ok_json(&json!({"embedding": {"values": [1.0]}}))]);
    let p = GeminiProvider::with_token(gemini_cfg(GeminiAuth::ApiKey("gk".into())), String::new()).unwrap().with_base_url(&base);
    assert_eq!(p.embed("y").await.unwrap(), vec![1.0]);
    assert_eq!(rx.recv().unwrap().path, "/v1beta/models/text-embedding-004:embedContent");
}

#[tokio::test]
async fn claude_has_no_embeddings() {
    let p = ClaudeProvider::new("k".into(), "claude-opus-5-5".into(), true).unwrap();
    assert!(matches!(p.embed("x").await, Err(ProviderError::Unsupported(_))));
}
