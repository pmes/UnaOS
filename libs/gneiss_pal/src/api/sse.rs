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
        let (field, value) = match line.find(':') {
            Some(i) => {
                let v = &line[i + 1..];
                (&line[..i], v.strip_prefix(' ').unwrap_or(v))
            }
            None => (line, ""),
        };
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
