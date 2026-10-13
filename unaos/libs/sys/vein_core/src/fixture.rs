// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Vein — shared-core
//!
//! LUMEN2 (LEDGER SR121) — the FIXTURE PROVIDER: one recorded answer in each dialect's event stream and a
//! [`Replay`] transport that hands a whole HTTP response back a few bytes per read, so a streamed reply is
//! exercised end to end (head parse, chunked framing, the incremental SSE decode, the text pieces in order)
//! with no network. The host tests and the kernel's `tests lumen` drive (`kernel/src/lumen.rs`) replay the
//! SAME bytes through the SAME [`crate::client::exchange_dialect`] LUMEN.ELF streams through, so a PASS there
//! is a statement about this code, not a copy of it.

use crate::client::Transport;

/// Claude's stream for the answer [`TEXT`] (thinking and ping events included: the decoder must skip them).
pub const CLAUDE_SSE: &str = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"fx\",\"usage\":{\"input_tokens\":9,\"output_tokens\":1}}}\n\n\
event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\n\
event: ping\ndata: {\"type\": \"ping\"}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"Here is \"}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"code:\\n```rust\\nfn main() {}\\n\"}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"```\\n\"}}\n\n\
event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":12}}\n\n\
event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";

/// The text [`CLAUDE_SSE`] decodes to.
pub const TEXT: &str = "Here is code:\n```rust\nfn main() {}\n```\n";

/// Gemini's stream (`alt=sse`): a thought part to skip, three visible pieces, `finishReason` and usage last.
pub const GEMINI_SSE: &str = "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"thinking\",\"thought\":true},{\"text\":\"Hello \"}],\"role\":\"model\"}}]}\r\n\r\n\
data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"from \\\"Gemini\\\"\\n\"}],\"role\":\"model\"}}]}\r\n\r\n\
data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"!\"}],\"role\":\"model\"},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":11,\"candidatesTokenCount\":6,\"thoughtsTokenCount\":2,\"totalTokenCount\":19}}\r\n\r\n";

/// The text [`GEMINI_SSE`] decodes to.
pub const GEMINI_TEXT: &str = "Hello from \"Gemini\"\n!";

/// A whole HTTP response replayed `step` bytes per read, in up to four parts (head, then body pieces — so a
/// caller can frame a body without an allocator). What was sent is counted, not kept.
pub struct Replay<'a> {
    parts: [&'a [u8]; 4],
    part: usize,
    pos: usize,
    step: usize,
    /// Bytes the exchange wrote (head + body).
    pub sent: usize,
    /// Reads served.
    pub reads: usize,
}

impl<'a> Replay<'a> {
    pub fn new(parts: [&'a [u8]; 4], step: usize) -> Self {
        Replay { parts, part: 0, pos: 0, step: step.max(1), sent: 0, reads: 0 }
    }
}

impl Transport for Replay<'_> {
    fn send_all(&mut self, b: &[u8]) -> Result<(), i64> {
        self.sent += b.len();
        Ok(())
    }
    fn recv(&mut self, b: &mut [u8]) -> Result<usize, i64> {
        while self.part < self.parts.len() && self.pos >= self.parts[self.part].len() {
            self.part += 1;
            self.pos = 0;
        }
        if self.part >= self.parts.len() {
            return Ok(0);
        }
        let p = self.parts[self.part];
        let n = self.step.min(b.len()).min(p.len() - self.pos);
        b[..n].copy_from_slice(&p[self.pos..self.pos + n]);
        self.pos += n;
        self.reads += 1;
        Ok(n)
    }
}

/// The plain (Content-Length-free, `Connection: close`) response head both fixtures are served under.
pub const HEAD_200: &str = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::{Dialect, Event, Stop};
    use crate::client::exchange_dialect;
    extern crate std;
    use std::string::String;

    #[test]
    fn both_fixtures_stream_piece_by_piece() {
        for (d, sse, want, pieces_min) in [(Dialect::Claude, CLAUDE_SSE, TEXT, 3), (Dialect::Gemini, GEMINI_SSE, GEMINI_TEXT, 3)] {
            let mut t = Replay::new([HEAD_200.as_bytes(), sse.as_bytes(), b"", b""], 7);
            let (mut rx, mut line) = ([0u8; 512], [0u8; 1024]);
            let (mut text, mut pieces) = (String::new(), 0);
            let o = exchange_dialect(&mut t, d, b"HEAD", b"BODY", &mut rx, &mut line, &mut |e| {
                if let Event::Text(s) = e {
                    text.push_str(s);
                    pieces += 1;
                }
            });
            assert_eq!(text, want);
            assert!(pieces >= pieces_min);
            assert_eq!((o.status, o.stop, o.fail), (200, Some(Stop::EndTurn), None));
            assert_eq!(t.sent, 8);
            assert!(t.reads > 10);
        }
    }
}
