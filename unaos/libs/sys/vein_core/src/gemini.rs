// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Vein — shared-core
//!
//! LUMEN2 (LEDGER SR121) — the Gemini `streamGenerateContent` API as pure functions, beside [`crate::claude`]:
//! the streaming request (body, path and HTTP head) and the stop-reason mapping. The stream itself is decoded
//! by [`crate::claude::StreamDecoder`] in [`Dialect::Gemini`] (one `GenerateContentResponse` JSON object per
//! `data:` line with `alt=sse`), so a Gemini answer reaches the window as the same [`crate::claude::Event`]s a
//! Claude answer does. The shape matches the host `GeminiProvider` (VEINPROV): `POST
//! /v1beta/models/<model>:streamGenerateContent?alt=sse`, the key in `x-goog-api-key` (never in the URL, so it
//! is never in a log line), `systemInstruction` for the system prompt, roles `user` / `model`.
//!
//! THE KEY RULE is Claude's ([`crate::prefs::plan`]): the key crosses the wire only over a verified TLS
//! connection; the Gemini endpoint is always `https://`, so [`plan`] needs a key on UnaFS (or Holocron) and a
//! verifier, or Echo answers with the reason.

use crate::claude::{Msg, Params, Stop};
use crate::json;
use crate::prefs::{Endpoint, KeyState, Plan, Reason, Verify};
use crate::{Out, Role};

pub use crate::claude::Dialect;

pub const HOST: &str = "generativelanguage.googleapis.com";
/// The model when a conversation names the Gemini family without a version.
pub const DEFAULT_MODEL: &str = "gemini-2.5-flash";
/// Where Gemini requests go (the path is per model: [`path`]).
pub const ENDPOINT: Endpoint<'static> = Endpoint { tls: true, host: HOST, port: 443, path: "/v1beta/models" };
/// The longest path [`path`] writes for a model id of [`MODEL_MAX`] bytes.
pub const MODEL_MAX: usize = 64;
pub const PATH_CAP: usize = 15 + MODEL_MAX + 33;

/// Whether `model` is answered by Gemini (the conversation's model picks the dialect).
pub fn is_gemini(model: &str) -> bool {
    model.starts_with("gemini-") || model.starts_with("gemma-")
}

/// `/v1beta/models/<model>:streamGenerateContent?alt=sse` into `out`. `None` for an empty or overlong model, or
/// one with a byte outside `[A-Za-z0-9._-]` (the id goes into the request line).
pub fn path(model: &str, out: &mut [u8]) -> Option<usize> {
    if model.is_empty() || model.len() > MODEL_MAX || !model.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b'_')) {
        return None;
    }
    let mut o = Out::new(out);
    o.put(b"/v1beta/models/");
    o.put(model.as_bytes());
    o.put(b":streamGenerateContent?alt=sse");
    o.done()
}

/// Encode the streaming request body into `out`, under the conversation rules [`crate::claude::encode_body`]
/// applies (System-role messages fold into `systemInstruction`; leading assistant turns are dropped;
/// consecutive same-role messages merge with a blank line; empty messages are skipped). `None` = `out` too
/// small, or no user message.
pub fn encode_body<'m>(p: &Params<'_>, msgs: impl Iterator<Item = Msg<'m>> + Clone, out: &mut [u8]) -> Option<usize> {
    let mut o = Out::new(out);
    o.put(b"{");
    let mut sys_open = false;
    for t in core::iter::once(p.system).chain(msgs.clone().filter(|m| m.role == Role::System).map(|m| m.text)) {
        if t.is_empty() {
            continue;
        }
        if sys_open {
            o.put(b"\\n\\n");
        } else {
            o.put(b"\"systemInstruction\":{\"parts\":[{\"text\":\"");
            sys_open = true;
        }
        json::escape_into(t.as_bytes(), &mut o);
    }
    if sys_open {
        o.put(b"\"}]},");
    }
    o.put(b"\"contents\":[");
    let mut cur: Option<Role> = None;
    for m in msgs.filter(|m| m.role != Role::System && !m.text.is_empty()) {
        if cur.is_none() && m.role != Role::User {
            continue;
        }
        if cur == Some(m.role) {
            o.put(b"\\n\\n");
        } else {
            if cur.is_some() {
                o.put(b"\"}]},");
            }
            o.put(b"{\"role\":\"");
            o.put(if m.role == Role::User { b"user" as &[u8] } else { b"model" });
            o.put(b"\",\"parts\":[{\"text\":\"");
            cur = Some(m.role);
        }
        json::escape_into(m.text.as_bytes(), &mut o);
    }
    cur?;
    o.put(b"\"}]}],\"generationConfig\":{\"maxOutputTokens\":");
    o.dec(p.max_tokens as u64);
    o.put(b"}}");
    o.done()
}

/// The HTTP/1.1 head for a body of `body_len` bytes; the key goes in `x-goog-api-key` (refused, never sent, when
/// it holds whitespace or controls).
pub fn request_head(host: &str, path: &str, key: &str, body_len: usize, out: &mut [u8]) -> Option<usize> {
    if key.is_empty() || key.bytes().any(|c| !(0x21..=0x7e).contains(&c)) {
        return None;
    }
    let mut o = Out::new(out);
    o.put(b"POST ");
    o.put(path.as_bytes());
    o.put(b" HTTP/1.1\r\nHost: ");
    o.put(host.as_bytes());
    o.put(b"\r\nUser-Agent: UnaOS-Lumen/1\r\nContent-Type: application/json\r\nAccept: text/event-stream\r\nx-goog-api-key: ");
    o.put(key.as_bytes());
    o.put(b"\r\nContent-Length: ");
    o.dec(body_len as u64);
    o.put(b"\r\nConnection: close\r\n\r\n");
    o.done()
}

/// `candidates[].finishReason` as the stop the window reports.
pub fn finish_stop(s: &str) -> Stop {
    match s {
        "STOP" => Stop::EndTurn,
        "MAX_TOKENS" => Stop::MaxTokens,
        "SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" | "IMAGE_SAFETY" => Stop::Refusal,
        _ => Stop::Other,
    }
}

/// The key rule for a Gemini conversation: a key (Holocron or a UnaFS key file) and a verifier, else Echo with
/// the reason. Gemini has no plain-HTTP relay form, so `send_key` is always true when it runs.
pub fn plan(key: KeyState, verify: Verify) -> Plan {
    match key {
        KeyState::None => Plan::Echo(Reason::NoKey),
        KeyState::OnFat => Plan::Echo(Reason::KeyOnFat),
        KeyState::UnaFs if verify != Verify::Ready => Plan::Echo(Reason::TlsUnverified(verify)),
        KeyState::UnaFs => Plan::Claude { send_key: true },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::{Event, StreamDecoder};
    use crate::client::{self, Transport};
    extern crate std;
    use std::string::String;
    use std::vec::Vec;

    fn enc(p: &Params<'_>, m: &[Msg<'_>]) -> Option<String> {
        let mut b = [0u8; 2048];
        let n = encode_body(p, m.iter().copied(), &mut b)?;
        Some(String::from_utf8(b[..n].to_vec()).unwrap())
    }

    #[test]
    fn body_path_and_head() {
        let p = Params::new("gemini-2.5-flash", 512, "Be brief.");
        let m = [
            Msg { role: Role::Assistant, text: "stray" },
            Msg { role: Role::User, text: "hi \"you\"" },
            Msg { role: Role::User, text: "again" },
            Msg { role: Role::Assistant, text: "hello" },
            Msg { role: Role::User, text: "bye" },
        ];
        assert_eq!(
            enc(&p, &m).unwrap(),
            r#"{"systemInstruction":{"parts":[{"text":"Be brief."}]},"contents":[{"role":"user","parts":[{"text":"hi \"you\"\n\nagain"}]},{"role":"model","parts":[{"text":"hello"}]},{"role":"user","parts":[{"text":"bye"}]}],"generationConfig":{"maxOutputTokens":512}}"#
        );
        let p = Params::new("gemini-2.5-pro", 9, "");
        assert_eq!(enc(&p, &[Msg { role: Role::User, text: "x" }]).unwrap(), r#"{"contents":[{"role":"user","parts":[{"text":"x"}]}],"generationConfig":{"maxOutputTokens":9}}"#);
        assert!(enc(&p, &[Msg { role: Role::Assistant, text: "only" }]).is_none());
        let mut b = [0u8; PATH_CAP];
        let n = path("gemini-2.5-flash", &mut b).unwrap();
        assert_eq!(&b[..n], b"/v1beta/models/gemini-2.5-flash:streamGenerateContent?alt=sse");
        assert!(path("bad model", &mut b).is_none());
        assert!(path(&"m".repeat(MODEL_MAX), &mut b).is_some());
        let mut h = [0u8; 512];
        let n = request_head(HOST, "/p", "AIza-test", 7, &mut h).unwrap();
        let hs = core::str::from_utf8(&h[..n]).unwrap();
        assert!(hs.starts_with("POST /p HTTP/1.1\r\nHost: generativelanguage.googleapis.com\r\n"));
        assert!(hs.contains("\r\nx-goog-api-key: AIza-test\r\n"));
        assert!(hs.ends_with("Content-Length: 7\r\nConnection: close\r\n\r\n"));
        assert!(request_head(HOST, "/p", "a b", 1, &mut h).is_none());
        assert!(is_gemini("gemini-2.5-pro") && !is_gemini("claude-opus-5-5"));
    }

    #[test]
    fn rules() {
        assert_eq!(plan(KeyState::UnaFs, Verify::Ready), Plan::Claude { send_key: true });
        assert_eq!(plan(KeyState::UnaFs, Verify::NoClock), Plan::Echo(Reason::TlsUnverified(Verify::NoClock)));
        assert_eq!(plan(KeyState::None, Verify::Ready), Plan::Echo(Reason::NoKey));
        assert_eq!(finish_stop("SAFETY"), Stop::Refusal);
    }

    #[test]
    fn stream_decodes_in_any_split() {
        let b = crate::fixture::GEMINI_SSE.as_bytes();
        for split in 0..b.len() {
            let mut buf = std::vec![0u8; 1024];
            let mut d = StreamDecoder::with_dialect(&mut buf, Dialect::Gemini);
            let (mut text, mut stops, mut done) = (String::new(), Vec::new(), 0);
            let mut on = |e: Event<'_>| match e {
                Event::Text(t) => text.push_str(t),
                Event::Stop(s) => stops.push(s),
                Event::Done => done += 1,
                _ => {}
            };
            d.feed(&b[..split], &mut on);
            d.feed(&b[split..], &mut on);
            d.finish(&mut on);
            assert_eq!(text, "Hello from \"Gemini\"\n!", "split {split}");
            assert_eq!(stops, [Stop::EndTurn]);
            assert_eq!(done, 1);
            assert_eq!((d.input_tokens, d.output_tokens), (Some(11), Some(8)));
        }
        let mut buf = std::vec![0u8; 256];
        let mut d = StreamDecoder::with_dialect(&mut buf, Dialect::Gemini);
        let mut errs = Vec::new();
        d.feed(b"data: {\"error\":{\"code\":429,\"message\":\"quota\",\"status\":\"RESOURCE_EXHAUSTED\"}}\n", &mut |e| {
            if let Event::Error(m) = e {
                errs.push(String::from(m))
            }
        });
        d.finish(&mut |_| {});
        assert_eq!(errs, ["quota"]);
        assert!(!d.done, "no finishReason: truncated, not done");
    }

    struct Fake {
        wire: Vec<u8>,
        pos: usize,
    }
    impl Transport for Fake {
        fn send_all(&mut self, _: &[u8]) -> Result<(), i64> {
            Ok(())
        }
        fn recv(&mut self, b: &mut [u8]) -> Result<usize, i64> {
            let n = 5.min(b.len()).min(self.wire.len() - self.pos);
            b[..n].copy_from_slice(&self.wire[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    #[test]
    fn exchange_streams_gemini_pieces_as_they_arrive() {
        let wire = std::format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n{}", crate::fixture::GEMINI_SSE);
        let mut f = Fake { wire: wire.into_bytes(), pos: 0 };
        let (mut rx, mut line) = ([0u8; 256], [0u8; 1024]);
        let mut pieces = 0;
        let o = client::exchange_dialect(&mut f, Dialect::Gemini, b"H", b"B", &mut rx, &mut line, &mut |e| {
            if let Event::Text(_) = e {
                pieces += 1;
            }
        });
        assert_eq!((o.status, o.fail, o.stop, pieces), (200, None, Some(Stop::EndTurn), 3));
    }
}
