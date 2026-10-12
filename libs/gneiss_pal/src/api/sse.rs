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

//! A minimal Server-Sent Events decoder (the `text/event-stream` framing both
//! the Claude Messages API and Gemini's `?alt=sse` use). Bytes go in as they
//! arrive — split anywhere, including inside a UTF-8 sequence — and complete
//! events come out.

/// One dispatched event.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SseEvent {
    /// The `event:` field, if the event named itself.
    pub event: Option<String>,
    /// The `data:` lines, joined with `\n`.
    pub data: String,
}

/// Incremental decoder.
#[derive(Debug, Default)]
pub struct SseDecoder {
    buf: Vec<u8>,
    cur: SseEvent,
    has_data: bool,
}

impl SseDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed bytes; answer every event they completed.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<SseEvent> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        while let Some(nl) = self.buf.iter().position(|&b| b == b'\n') {
            let mut line: Vec<u8> = self.buf.drain(..=nl).collect();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let line = String::from_utf8_lossy(&line);
            self.line(&line, &mut out);
        }
        out
    }

    /// End of stream: dispatch a final event that lacked its blank line.
    pub fn finish(&mut self) -> Option<SseEvent> {
        if !self.buf.is_empty() {
            let rest = std::mem::take(&mut self.buf);
            let mut out = Vec::new();
            self.line(String::from_utf8_lossy(&rest).trim_end_matches('\r'), &mut out);
            if let Some(e) = out.pop() {
                return Some(e);
            }
        }
        if self.has_data {
            self.has_data = false;
            return Some(std::mem::take(&mut self.cur));
        }
        None
    }

    fn line(&mut self, line: &str, out: &mut Vec<SseEvent>) {
        if line.is_empty() {
            if self.has_data || self.cur.event.is_some() {
                out.push(std::mem::take(&mut self.cur));
            }
            self.has_data = false;
            return;
        }
        if line.starts_with(':') {
            return; // comment / keep-alive
        }
        let (field, value) = split_field(line);
        match field {
            "event" => self.cur.event = Some(value.to_string()),
            "data" => {
                if self.has_data {
                    self.cur.data.push('\n');
                }
                self.cur.data.push_str(value);
                self.has_data = true;
            }
            _ => {} // id, retry: unused
        }
    }
}

/// One field line (HTML §9.2.6): the name up to the first colon, the value after it with one leading space
/// removed; a line with no colon is a field with an empty value. Shared by [`SseDecoder`] and
/// [`EventStreamDecoder`].
fn split_field(line: &str) -> (&str, &str) {
    match line.find(':') {
        Some(i) => {
            let v = &line[i + 1..];
            (&line[..i], v.strip_prefix(' ').unwrap_or(v))
        }
        None => (line, ""),
    }
}

// ---------------------------------------------------------------------------
// HTML §9.2.6 "interpreting an event stream" — the parser `EventSource` runs (AETHERNET, SR70)
// ---------------------------------------------------------------------------

/// One event as `EventSource` dispatches it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamEvent {
    /// The event type (`"message"` when the stream named none, or named the empty string).
    pub ty: String,
    /// The data buffer with its final LF removed.
    pub data: String,
    /// The last event ID buffer at dispatch (it persists across events until an `id:` changes it).
    pub last_event_id: String,
}

/// The full HTML event-stream interpreter: a UTF-8 decode that strips one leading BOM, lines ending in CRLF,
/// a lone LF or a lone CR (a CR at the end of a chunk waits to see whether an LF follows), the `event`,
/// `data`, `id` (refused when it carries U+0000) and `retry` (ASCII digits only) fields, and dispatch only
/// when the data buffer is non-empty. The [`SseDecoder`] above keeps Vein's looser provider semantics.
#[derive(Debug, Default)]
pub struct EventStreamDecoder {
    buf: Vec<u8>,
    started: bool,
    /// A CR ended the previous chunk: an LF opening the next one belongs to it.
    pending_cr: bool,
    ty: String,
    data: String,
    /// The last event ID buffer (an `id:` line sets it).
    last_id: String,
    /// The event source's last event ID string: the buffer as of the last dispatch — what an event's
    /// `lastEventId` carries and a reconnect sends as `Last-Event-ID`. A new stream's buffer starts from it,
    /// so an `id:` whose event never dispatched does not survive the reconnect.
    id_string: String,
    retry: Option<u64>,
}

impl EventStreamDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// A decoder whose last event ID string starts at `id` (HTML §9.2.3).
    pub fn with_last_event_id(id: &str) -> Self {
        EventStreamDecoder { last_id: id.to_string(), id_string: id.to_string(), ..Self::default() }
    }

    /// The last event ID string (what a reconnect sends as `Last-Event-ID`).
    pub fn last_event_id(&self) -> &str {
        &self.id_string
    }

    /// A reconnection time a `retry:` field set since the last call.
    pub fn take_retry(&mut self) -> Option<u64> {
        self.retry.take()
    }

    /// A new connection's stream: the BOM check and the per-event buffers start over; the last event ID
    /// buffer restarts from the last event ID string.
    pub fn reset_stream(&mut self) {
        self.buf.clear();
        self.started = false;
        self.pending_cr = false;
        self.ty.clear();
        self.data.clear();
        self.last_id = self.id_string.clone();
    }

    /// Feed bytes; answer every event they completed.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<StreamEvent> {
        let mut bytes = bytes;
        if self.pending_cr && !bytes.is_empty() {
            self.pending_cr = false;
            if bytes[0] == b'\n' {
                bytes = &bytes[1..];
            }
        }
        self.buf.extend_from_slice(bytes);
        if !self.started {
            if self.buf.len() < 3 && b"\xEF\xBB\xBF".starts_with(&self.buf) {
                return Vec::new();
            }
            if self.buf.starts_with(b"\xEF\xBB\xBF") {
                self.buf.drain(..3);
            }
            self.started = true;
        }
        let mut out = Vec::new();
        let mut start = 0;
        let mut i = 0;
        while i < self.buf.len() {
            let b = self.buf[i];
            if b == b'\n' || b == b'\r' {
                let line = String::from_utf8_lossy(&self.buf[start..i]).into_owned();
                let mut next = i + 1;
                if b == b'\r' {
                    if next < self.buf.len() {
                        if self.buf[next] == b'\n' {
                            next += 1;
                        }
                    } else {
                        self.pending_cr = true;
                    }
                }
                self.line(&line, &mut out);
                start = next;
                i = next;
                continue;
            }
            i += 1;
        }
        self.buf.drain(..start);
        out
    }

    fn line(&mut self, line: &str, out: &mut Vec<StreamEvent>) {
        if line.is_empty() {
            // Dispatch (§9.2.6): an empty data buffer resets the buffers and dispatches nothing.
            if self.data.is_empty() {
                self.ty.clear();
                return;
            }
            if self.data.ends_with('\n') {
                self.data.pop();
            }
            let ty = if self.ty.is_empty() { "message".to_string() } else { std::mem::take(&mut self.ty) };
            self.ty.clear();
            self.id_string = self.last_id.clone();
            out.push(StreamEvent { ty, data: std::mem::take(&mut self.data), last_event_id: self.id_string.clone() });
            return;
        }
        if line.starts_with(':') {
            return;
        }
        let (field, value) = split_field(line);
        match field {
            "event" => self.ty = value.to_string(),
            "data" => {
                self.data.push_str(value);
                self.data.push('\n');
            }
            "id" => {
                if !value.contains('\0') {
                    self.last_id = value.to_string();
                }
            }
            "retry" => {
                if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
                    self.retry = value.parse().ok().or(Some(u64::MAX));
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_split_across_chunks_and_crlf() {
        let mut d = SseDecoder::new();
        let mut got = d.push(b"event: ping\r\ndata: {\"a\"");
        assert!(got.is_empty());
        got.extend(d.push(b":1}\r\n\r\n: keepalive\n\nevent: x\ndata: l1\ndata: l2\n\n"));
        assert_eq!(
            got,
            vec![
                SseEvent { event: Some("ping".into()), data: "{\"a\":1}".into() },
                SseEvent { event: Some("x".into()), data: "l1\nl2".into() },
            ]
        );
        assert_eq!(d.finish(), None);
    }

    #[test]
    fn utf8_split_mid_sequence() {
        let mut d = SseDecoder::new();
        let s = "data: caf\u{e9}\n\n".as_bytes();
        let (a, b) = s.split_at(10); // inside the two-byte é
        assert!(d.push(a).is_empty());
        assert_eq!(d.push(b)[0].data, "caf\u{e9}");
    }

    #[test]
    fn event_stream_html_semantics() {
        let mut d = EventStreamDecoder::new();
        // BOM split across chunks, a lone CR line end at a chunk boundary followed by LF, CR-only lines.
        let mut got = d.push(b"\xEF\xBB");
        got.extend(d.push(b"\xBFid: 7\rretry: 1500\r"));
        got.extend(d.push(b"\ndata: a\r\ndata\r\n\revent: \ndata: b\n\n"));
        assert_eq!(got.len(), 2);
        assert_eq!(got[0], StreamEvent { ty: "message".into(), data: "a\n".into(), last_event_id: "7".into() });
        assert_eq!(got[1], StreamEvent { ty: "message".into(), data: "b".into(), last_event_id: "7".into() });
        assert_eq!(d.take_retry(), Some(1500));
        assert_eq!(d.take_retry(), None);
        // event-only blocks dispatch nothing; ids with NUL are ignored; bogus retry ignored; named events.
        let got = d.push(b"event: x\n\nid: a\0b\nretry: 12a\nevent: named\ndata: q\n\n");
        assert_eq!(got, vec![StreamEvent { ty: "named".into(), data: "q".into(), last_event_id: "7".into() }]);
        assert_eq!(d.take_retry(), None);
        // A reconnect keeps the last event id string and drops a half-built event; an `id:` whose event
        // never dispatched does not survive the reconnect.
        d.push(b"id: lost\ndata: partial");
        assert_eq!(d.last_event_id(), "7");
        d.reset_stream();
        let got = d.push(b"data: y\n\nid\ndata: z\n\n");
        assert_eq!((got[0].data.as_str(), got[0].last_event_id.as_str()), ("y", "7"));
        assert_eq!((got[1].data.as_str(), got[1].last_event_id.as_str()), ("z", ""));
        assert_eq!(d.last_event_id(), "");
    }

    #[test]
    fn finish_flushes_unterminated_event() {
        let mut d = SseDecoder::new();
        assert!(d.push(b"data: tail").is_empty());
        assert_eq!(d.finish().unwrap().data, "tail");
    }
}

// ---------------------------------------------------------------------------
// SSE body → DeltaStream, shared by every streaming provider
// ---------------------------------------------------------------------------

use std::collections::VecDeque;

use futures_util::StreamExt;

use super::provider::{ChatDelta, DeltaStream, ProviderError};

/// A provider's translation of its SSE events into [`ChatDelta`]s.
pub trait SseHandler: Send {
    /// One event in; zero or more deltas (or errors) out.
    fn on_event(&mut self, ev: &SseEvent) -> Vec<Result<ChatDelta, ProviderError>>;
    /// The body ended. A provider whose stream has an explicit terminator
    /// answers an error here if it never saw it.
    fn on_end(&mut self) -> Vec<Result<ChatDelta, ProviderError>>;
    /// True once the terminal event has been seen (stop reading).
    fn done(&self) -> bool;
}

type ByteStream = std::pin::Pin<Box<dyn futures_core::Stream<Item = Result<Vec<u8>, super::http::Error>> + Send>>;

struct SseState<H> {
    body: ByteStream,
    dec: SseDecoder,
    pending: VecDeque<Result<ChatDelta, ProviderError>>,
    handler: H,
    ended: bool,
}

/// Turn a successful `text/event-stream` response into a [`DeltaStream`].
pub fn sse_stream<'a, H: SseHandler + 'a>(res: super::http::Response, handler: H) -> DeltaStream<'a> {
    let body: ByteStream = Box::pin(res.bytes_stream());
    let st = SseState { body, dec: SseDecoder::new(), pending: VecDeque::new(), handler, ended: false };
    Box::pin(futures_util::stream::unfold(st, |mut st| async move {
        loop {
            if let Some(x) = st.pending.pop_front() {
                return Some((x, st));
            }
            if st.ended {
                return None;
            }
            if st.handler.done() {
                st.ended = true;
                continue;
            }
            match st.body.next().await {
                Some(Ok(bytes)) => {
                    for ev in st.dec.push(&bytes) {
                        st.pending.extend(st.handler.on_event(&ev));
                        if st.handler.done() {
                            break;
                        }
                    }
                }
                Some(Err(e)) => {
                    st.ended = true;
                    st.pending.push_back(Err(ProviderError::Retryable { status: 0, message: format!("stream interrupted: {e}") }));
                }
                None => {
                    st.ended = true;
                    if let Some(ev) = st.dec.finish() {
                        st.pending.extend(st.handler.on_event(&ev));
                    }
                    if !st.handler.done() {
                        st.pending.extend(st.handler.on_end());
                    }
                }
            }
        }
    }))
}

/// Drain a [`DeltaStream`] into a complete response (text joined in order).
pub async fn collect(mut s: DeltaStream<'_>) -> Result<super::provider::ChatResponse, ProviderError> {
    let mut text = String::new();
    while let Some(d) = s.next().await {
        match d? {
            ChatDelta::Text(t) => text.push_str(&t),
            ChatDelta::Stop { stop, usage } => return Ok(super::provider::ChatResponse { text, stop, usage }),
        }
    }
    Err(ProviderError::Malformed("stream ended without a stop".into()))
}
