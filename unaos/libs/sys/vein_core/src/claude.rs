// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The Claude Messages API, as pure functions: the streaming request (body and HTTP head) and the
//! incremental decoder of its server-sent-event stream. No allocation except the `alloc` convenience
//! [`encode_messages_request`]. The shape matches VEINPROV's host `ClaudeProvider` (raw `POST
//! /v1/messages`, `anthropic-version: 2023-06-01`, no `thinking` field, no prefill, `stop_reason` read
//! before the text, server-side fallbacks on by default for the models that take them).

use crate::json;
use crate::{Out, Role};

pub const HOST: &str = "api.anthropic.com";
pub const PATH: &str = "/v1/messages";
pub const API_VERSION: &str = "2023-06-01";
/// The model when `vein.model` is unset (the same default as the host provider).
pub const DEFAULT_MODEL: &str = "claude-opus-5-5";
/// Room for the answer: a chat window, streamed.
pub const DEFAULT_MAX_TOKENS: u32 = 8192;
/// The beta that admits `"fallbacks": "default"` (route a refused turn by category, server-side).
pub const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";

/// Whether `model` takes the `"default"` server-side fallback form.
pub fn wants_fallbacks(model: &str) -> bool {
    model.starts_with("claude-opus-5") || model.starts_with("claude-fable-5") || model == "claude-sonnet-5-5"
}

/// One message of the conversation, borrowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Msg<'a> {
    pub role: Role,
    pub text: &'a str,
}

/// The request's scalar parameters.
#[derive(Debug, Clone, Copy)]
pub struct Params<'a> {
    pub model: &'a str,
    pub max_tokens: u32,
    pub system: &'a str,
    pub fallbacks: bool,
}

impl<'a> Params<'a> {
    pub fn new(model: &'a str, max_tokens: u32, system: &'a str) -> Self {
        Params { model, max_tokens, system, fallbacks: wants_fallbacks(model) }
    }
}

/// Encode the streaming Messages request body into `out`. The API's conversation rules are applied
/// here, once, for every caller: System-role messages fold into the top-level `system`; leading
/// assistant turns are dropped (the first message must be the user's); consecutive same-role messages
/// are merged with a blank line (one turn per role change); empty messages are skipped. `None` = `out`
/// too small, or no user message at all.
pub fn encode_body<'m>(p: &Params<'_>, msgs: impl Iterator<Item = Msg<'m>> + Clone, out: &mut [u8]) -> Option<usize> {
    let mut o = Out::new(out);
    o.put(b"{\"model\":\"");
    json::escape_into(p.model.as_bytes(), &mut o);
    o.put(b"\",\"max_tokens\":");
    o.dec(p.max_tokens as u64);
    o.put(b",\"stream\":true");
    if p.fallbacks {
        o.put(b",\"fallbacks\":\"default\"");
    }
    let mut sys_open = false;
    for t in core::iter::once(p.system).chain(msgs.clone().filter(|m| m.role == Role::System).map(|m| m.text)) {
        if t.is_empty() {
            continue;
        }
        if sys_open {
            o.put(b"\\n\\n");
        } else {
            o.put(b",\"system\":\"");
            sys_open = true;
        }
        json::escape_into(t.as_bytes(), &mut o);
    }
    if sys_open {
        o.put(b"\"");
    }
    o.put(b",\"messages\":[");
    let mut cur: Option<Role> = None;
    for m in msgs.filter(|m| m.role != Role::System && !m.text.is_empty()) {
        if cur.is_none() && m.role != Role::User {
            continue;
        }
        if cur == Some(m.role) {
            o.put(b"\\n\\n");
        } else {
            if cur.is_some() {
                o.put(b"\"},");
            }
            o.put(b"{\"role\":\"");
            o.put(m.role.as_str().as_bytes());
            o.put(b"\",\"content\":\"");
            cur = Some(m.role);
        }
        json::escape_into(m.text.as_bytes(), &mut o);
    }
    cur?;
    o.put(b"\"}]}");
    o.done()
}

/// The `alloc` convenience over [`encode_body`] for a [`crate::model::ChatRequest`] (its `system` and
/// messages; its own `max_tokens` is used when `max_tokens` is 0).
#[cfg(feature = "alloc")]
pub fn encode_messages_request(req: &crate::model::ChatRequest, model: &str, max_tokens: u32) -> alloc::vec::Vec<u8> {
    let mt = if max_tokens == 0 { req.max_tokens } else { max_tokens };
    let p = Params::new(model, mt, &req.system);
    let mut cap = 256 + model.len() + 2 * req.system.len() + req.messages.iter().map(|m| 32 + 6 * m.text.len()).sum::<usize>();
    loop {
        let mut v = alloc::vec![0u8; cap];
        let it = req.messages.iter().map(|m| Msg { role: m.role, text: m.text.as_str() });
        match encode_body(&p, it, &mut v) {
            Some(n) => {
                v.truncate(n);
                return v;
            }
            None if !req.messages.iter().any(|m| m.role == Role::User && !m.text.is_empty()) => return alloc::vec::Vec::new(),
            None => cap *= 2,
        }
    }
}

/// The HTTP/1.1 head for a body of `body_len` bytes. `key` is sent as `x-api-key` when present (a relay
/// endpoint that holds the key itself gets none).
pub fn request_head(host: &str, path: &str, key: Option<&str>, body_len: usize, fallbacks: bool, out: &mut [u8]) -> Option<usize> {
    let mut o = Out::new(out);
    o.put(b"POST ");
    o.put(path.as_bytes());
    o.put(b" HTTP/1.1\r\nHost: ");
    o.put(host.as_bytes());
    o.put(b"\r\nUser-Agent: UnaOS-Lumen/1\r\nContent-Type: application/json\r\nAccept: text/event-stream\r\nanthropic-version: ");
    o.put(API_VERSION.as_bytes());
    o.put(b"\r\n");
    if fallbacks {
        o.put(b"anthropic-beta: ");
        o.put(FALLBACK_BETA.as_bytes());
        o.put(b"\r\n");
    }
    if let Some(k) = key {
        if k.bytes().any(|c| c < 0x21 || c > 0x7e) {
            return None; // a key with whitespace or controls would split the header: refuse, never send
        }
        o.put(b"x-api-key: ");
        o.put(k.as_bytes());
        o.put(b"\r\n");
    }
    o.put(b"Content-Length: ");
    o.dec(body_len as u64);
    o.put(b"\r\nConnection: close\r\n\r\n");
    o.done()
}

/// Why the answer stopped (`message_delta.delta.stop_reason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    EndTurn,
    MaxTokens,
    StopSequence,
    ToolUse,
    PauseTurn,
    Refusal,
    Other,
}

impl Stop {
    fn parse(s: &str) -> Stop {
        match s {
            "end_turn" => Stop::EndTurn,
            "max_tokens" => Stop::MaxTokens,
            "stop_sequence" => Stop::StopSequence,
            "tool_use" => Stop::ToolUse,
            "pause_turn" => Stop::PauseTurn,
            "refusal" => Stop::Refusal,
            _ => Stop::Other,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Stop::EndTurn => "end_turn",
            Stop::MaxTokens => "max_tokens",
            Stop::StopSequence => "stop_sequence",
            Stop::ToolUse => "tool_use",
            Stop::PauseTurn => "pause_turn",
            Stop::Refusal => "refusal",
            Stop::Other => "other",
        }
    }
}

/// One decoded stream event the caller acts on. Everything else (`ping`, `message_start`, thinking and
/// signature deltas, block start/stop) is consumed silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event<'a> {
    /// A piece of the answer's visible text.
    Text(&'a str),
    /// `message_delta` carried the stop reason.
    Stop(Stop),
    /// `message_stop`: the answer is complete.
    Done,
    /// An `error` event (or an error body): the message text.
    Error(&'a str),
    /// A stream line longer than the decoder's buffer was skipped.
    Dropped,
}

/// The incremental SSE decoder: feed it the response body in any split; it yields [`Event`]s. The line
/// buffer is the caller's (ring 3 gives it a static; a `data:` line longer than it is skipped, reported
/// once as [`Event::Dropped`]).
pub struct StreamDecoder<'b> {
    line: &'b mut [u8],
    n: usize,
    overflow: bool,
    pub stop: Option<Stop>,
    pub done: bool,
    pub text_bytes: usize,
}

impl<'b> StreamDecoder<'b> {
    pub fn new(line: &'b mut [u8]) -> Self {
        StreamDecoder { line, n: 0, overflow: false, stop: None, done: false, text_bytes: 0 }
    }

    pub fn feed(&mut self, mut b: &[u8], on: &mut dyn FnMut(Event<'_>)) {
        while !b.is_empty() {
            match b.iter().position(|&c| c == b'\n') {
                Some(p) => {
                    self.push(&b[..p]);
                    b = &b[p + 1..];
                    self.line_done(on);
                }
                None => {
                    self.push(b);
                    b = &[];
                }
            }
        }
    }

    /// End of body: a final line without `\n` is still a line.
    pub fn finish(&mut self, on: &mut dyn FnMut(Event<'_>)) {
        if self.n > 0 || self.overflow {
            self.line_done(on);
        }
    }

    fn push(&mut self, b: &[u8]) {
        if self.overflow || self.n + b.len() > self.line.len() {
            self.overflow = true;
            return;
        }
        self.line[self.n..self.n + b.len()].copy_from_slice(b);
        self.n += b.len();
    }

    fn line_done(&mut self, on: &mut dyn FnMut(Event<'_>)) {
        let (n, over) = (self.n, self.overflow);
        self.n = 0;
        self.overflow = false;
        if over {
            on(Event::Dropped);
            return;
        }
        let mut end = n;
        if end > 0 && self.line[end - 1] == b'\r' {
            end -= 1;
        }
        let l = &self.line[..end];
        let Some(rest) = l.strip_prefix(b"data:") else { return };
        let off = 5 + usize::from(rest.first() == Some(&b' '));
        self.event(off, end, on);
    }

    /// Decode one `data:` JSON payload at `line[s..e]`.
    fn event(&mut self, s: usize, e: usize, on: &mut dyn FnMut(Event<'_>)) {
        let base = self.line.as_ptr() as usize;
        let d = &self.line[s..e];
        let Some(ty) = json::get(d, "type").and_then(json::plain_str) else { return };
        // Locate the string to decode as an absolute range in `line` (so it can be unescaped in place).
        let pick = |v: &[u8]| -> (usize, usize) {
            let a = v.as_ptr() as usize - base;
            (a, a + v.len())
        };
        let target = match ty {
            "content_block_delta" => {
                let Some(delta) = json::get(d, "delta") else { return };
                if json::get(delta, "type").and_then(json::plain_str) != Some("text_delta") {
                    return; // thinking / signature / tool-input deltas are not the visible answer
                }
                let Some(t) = json::get(delta, "text") else { return };
                Some((pick(t), true))
            }
            "message_delta" => {
                if let Some(sr) = json::get(d, "delta").and_then(|x| json::get(x, "stop_reason")).and_then(json::plain_str) {
                    let st = Stop::parse(sr);
                    self.stop = Some(st);
                    on(Event::Stop(st));
                }
                None
            }
            "message_stop" => {
                self.done = true;
                on(Event::Done);
                None
            }
            "error" => {
                let m = json::get(d, "error").and_then(|x| json::get(x, "message"));
                match m {
                    Some(m) => Some((pick(m), false)),
                    None => {
                        on(Event::Error("stream error"));
                        None
                    }
                }
            }
            _ => None,
        };
        let Some(((a, b), is_text)) = target else { return };
        let Some(r) = json::unescape_in_place(self.line, a, b) else { return };
        let Ok(t) = core::str::from_utf8(&self.line[r]) else { return };
        if is_text {
            self.text_bytes += t.len();
            on(Event::Text(t));
        } else {
            on(Event::Error(t));
        }
    }
}

/// The message of an API error body (`{"type":"error","error":{"type":..,"message":..}}`), decoded in
/// place inside `body`.
pub fn error_message(body: &mut [u8]) -> Option<&str> {
    let base = body.as_ptr() as usize;
    let m = json::get(body, "error").and_then(|x| json::get(x, "message"))?;
    let (a, b) = (m.as_ptr() as usize - base, m.as_ptr() as usize - base + m.len());
    let r = json::unescape_in_place(body, a, b)?;
    core::str::from_utf8(&body[r]).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    use std::string::String;
    use std::vec::Vec;

    fn enc(p: &Params<'_>, m: &[Msg<'_>]) -> Option<String> {
        let mut b = [0u8; 2048];
        let n = encode_body(p, m.iter().copied(), &mut b)?;
        Some(String::from_utf8(b[..n].to_vec()).unwrap())
    }

    #[test]
    fn body_kat_and_conversation_rules() {
        let p = Params::new("claude-opus-5-5", 8192, "You are Lumen.");
        let m = [
            Msg { role: Role::Assistant, text: "stray greeting" },
            Msg { role: Role::User, text: "hi \"there\"" },
            Msg { role: Role::User, text: "again" },
            Msg { role: Role::System, text: "be brief" },
            Msg { role: Role::Assistant, text: "hello\nworld" },
            Msg { role: Role::User, text: "" },
            Msg { role: Role::User, text: "bye" },
        ];
        assert_eq!(
            enc(&p, &m).unwrap(),
            r#"{"model":"claude-opus-5-5","max_tokens":8192,"stream":true,"fallbacks":"default","system":"You are Lumen.\n\nbe brief","messages":[{"role":"user","content":"hi \"there\"\n\nagain"},{"role":"assistant","content":"hello\nworld"},{"role":"user","content":"bye"}]}"#
        );
        let p = Params::new("claude-haiku-4-5", 100, "");
        assert_eq!(enc(&p, &[Msg { role: Role::User, text: "x" }]).unwrap(), r#"{"model":"claude-haiku-4-5","max_tokens":100,"stream":true,"messages":[{"role":"user","content":"x"}]}"#);
        assert!(enc(&p, &[Msg { role: Role::Assistant, text: "only me" }]).is_none());
        let mut tiny = [0u8; 20];
        assert!(encode_body(&p, [Msg { role: Role::User, text: "x" }].into_iter(), &mut tiny).is_none());
    }

    #[test]
    fn alloc_request_matches_the_borrowed_encoder() {
        let mut c = crate::model::Conversation::new(1);
        c.push(Role::User, "hi");
        c.push(Role::Assistant, "hello");
        let req = crate::context::assemble("sys", &c, "more", 64);
        let v = encode_messages_request(&req, "claude-opus-5-5", 0);
        let s = core::str::from_utf8(&v).unwrap();
        assert!(s.starts_with(r#"{"model":"claude-opus-5-5","max_tokens":64,"#));
        assert!(s.ends_with(r#""messages":[{"role":"user","content":"hi"},{"role":"assistant","content":"hello"},{"role":"user","content":"more"}]}"#));
    }

    #[test]
    fn head_carries_version_key_and_beta() {
        let mut b = [0u8; 512];
        let n = request_head(HOST, PATH, Some("sk-test"), 42, true, &mut b).unwrap();
        let h = core::str::from_utf8(&b[..n]).unwrap();
        assert!(h.starts_with("POST /v1/messages HTTP/1.1\r\nHost: api.anthropic.com\r\n"));
        assert!(h.contains("\r\nanthropic-version: 2023-06-01\r\n"));
        assert!(h.contains("\r\nanthropic-beta: server-side-fallback-2026-07-01\r\n"));
        assert!(h.contains("\r\nx-api-key: sk-test\r\n"));
        assert!(h.ends_with("Content-Length: 42\r\nConnection: close\r\n\r\n"));
        let n = request_head("relay", PATH, None, 1, false, &mut b).unwrap();
        assert!(!core::str::from_utf8(&b[..n]).unwrap().contains("x-api-key"));
        assert!(request_head(HOST, PATH, Some("sk bad\r\nX: y"), 1, false, &mut b).is_none());
    }

    const STREAM: &str = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"content\":[]}}\n\n\
event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"abc\"}}\n\n\
event: ping\ndata: {\"type\": \"ping\"}\n\n\
event: content_block_delta\r\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hel\"}}\r\n\r\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"lo \\u00e9\\n\"}}\n\n\
event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":5}}\n\n\
event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";

    #[derive(Debug, PartialEq)]
    enum E {
        T(String),
        S(Stop),
        D,
        Err(String),
        Drop,
    }

    fn run(chunks: &[&[u8]], cap: usize) -> Vec<E> {
        let mut buf = std::vec![0u8; cap];
        let mut d = StreamDecoder::new(&mut buf);
        let mut ev = Vec::new();
        let mut on = |e: Event<'_>| {
            ev.push(match e {
                Event::Text(t) => E::T(t.into()),
                Event::Stop(s) => E::S(s),
                Event::Done => E::D,
                Event::Error(m) => E::Err(m.into()),
                Event::Dropped => E::Drop,
            })
        };
        for c in chunks {
            d.feed(c, &mut on);
        }
        d.finish(&mut on);
        // merge adjacent text for split-independence
        let mut out: Vec<E> = Vec::new();
        for e in ev {
            match (out.last_mut(), e) {
                (Some(E::T(a)), E::T(b)) => a.push_str(&b),
                (_, e) => out.push(e),
            }
        }
        out
    }

    #[test]
    fn stream_decodes_text_stop_done_in_any_split() {
        let want = std::vec![E::T("Hello \u{e9}\n".into()), E::S(Stop::EndTurn), E::D];
        let b = STREAM.as_bytes();
        assert_eq!(run(&[b], 1024), want);
        for split in 0..b.len() {
            assert_eq!(run(&[&b[..split], &b[split..]], 1024), want, "split {split}");
        }
        let bytes: Vec<&[u8]> = b.chunks(1).collect();
        assert_eq!(run(&bytes, 1024), want);
    }

    #[test]
    fn stream_errors_refusals_and_overlong_lines() {
        let s = b"event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Over \\\"loaded\\\"\"}}\n\n";
        assert_eq!(run(&[s], 512), std::vec![E::Err("Over \"loaded\"".into())]);
        let r = b"data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"refusal\"}}\n";
        assert_eq!(run(&[r], 512), std::vec![E::S(Stop::Refusal)]);
        let long = std::format!("data: {{\"type\":\"content_block_delta\",\"delta\":{{\"type\":\"text_delta\",\"text\":\"{}\"}}}}\ndata: {{\"type\":\"message_stop\"}}\n", "x".repeat(200));
        assert_eq!(run(&[long.as_bytes()], 64), std::vec![E::Drop, E::D]);
    }

    #[test]
    fn error_body_message() {
        let mut b = *br#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#;
        assert_eq!(error_message(&mut b), Some("invalid x-api-key"));
    }
}
