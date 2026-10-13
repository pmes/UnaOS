//! An HTTP/2 client connection (RFC 9113) over any byte [`Transport`]: the preface and SETTINGS exchange
//! (§3.4, §6.5), streams opened with HEADERS (+CONTINUATION) (§5.1, §6.2, §6.10), request bodies sent under
//! both flow-control windows (§5.2, §6.9), response DATA credited back with WINDOW_UPDATE, PING answered,
//! SETTINGS applied (INITIAL_WINDOW_SIZE deltas, HEADER_TABLE_SIZE, MAX_FRAME_SIZE), GOAWAY / RST_STREAM
//! honoured, server push refused (we send ENABLE_PUSH = 0), and responses checked for the §8.3.2 pseudo-header
//! and §8.2 field rules.
//!
//! Multiplexing (AETHERCACHE, SR88): many streams are open at once, bounded by the peer's
//! SETTINGS_MAX_CONCURRENT_STREAMS ([`H2Conn::can_open`]). The blocking [`H2Conn::read_head`] /
//! [`H2Conn::read_data`] serve one exchange; a multiplexing owner instead reads ONE frame at a time
//! ([`H2Conn::step_frame`], when [`H2Conn::has_buffered_frame`] or its socket says bytes wait) and polls every open
//! stream without blocking ([`H2Conn::try_head`], [`H2Conn::try_data`]). GOAWAY marks every stream above its
//! last-stream-id REFUSED (§6.8: never processed, safe to retry on a new connection — [`H2Conn::refused`]).
//!
//! Receive flow control (NETDEBT, SR97; §5.2, §6.9): each stream's window ([`RECV_WINDOW`]) is credited back
//! only for bytes its CONSUMER has taken ([`H2Conn::try_data`] popping them), so a stream whose reader stalls
//! stops its server at that window while every other stream keeps flowing. The connection window
//! ([`CONN_RECV_WINDOW`]) is credited as DATA is received: what is buffered is already bounded per stream, and
//! one stalled stream must not starve the connection. Frames that arrive for a stream this side already closed
//! or reset (§5.1 "closed") are discarded — their header blocks still decoded, so HPACK state stays in step.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use super::frame::{self, ErrorCode, Frame, FrameHeader, H2Error};
use super::hpack::{Decoder, Encoder};
use crate::conn::{Error, Transport};
use crate::headers::Headers;

/// Our receive window per stream (4 MiB keeps a fast link busy): the most a stream buffers for its reader.
pub const RECV_WINDOW: u32 = 4 << 20;
/// Our connection receive window (credited on receipt; see the module text).
pub const CONN_RECV_WINDOW: u32 = 16 << 20;
const OUR_MAX_FRAME: u32 = frame::DEFAULT_MAX_FRAME_SIZE;

#[derive(Debug, Default)]
struct Stream {
    send_window: i64,
    /// Flow-controlled bytes received and not yet credited back (buffered + taken-but-uncredited): never more
    /// than RECV_WINDOW, or the peer overran the window.
    recv_outstanding: u32,
    /// Bytes the consumer has taken (or padding) not yet credited back with WINDOW_UPDATE.
    recv_taken: u32,
    head: Option<(u16, Headers)>,
    trailers: Option<Headers>,
    /// DATA payloads with the flow-controlled length each counted (padding included).
    data: alloc::collections::VecDeque<(Vec<u8>, u32)>,
    end: bool,
    reset: Option<ErrorCode>,
}

/// What [`H2Conn::try_data`] found on a stream.
#[derive(Debug, PartialEq, Eq)]
pub enum DataPoll {
    Chunk(Vec<u8>),
    /// END_STREAM: the body is complete.
    End,
    /// Nothing buffered yet.
    Pending,
}

/// Before the peer's SETTINGS names a limit (§5.1.2 says unlimited; RFC 9113 advises at least 100).
const DEFAULT_MAX_CONCURRENT: u32 = 100;

/// One HTTP/2 connection.
pub struct H2Conn<T: Transport> {
    t: T,
    rbuf: Vec<u8>,
    enc: Encoder,
    dec: Decoder,
    peer_max_frame: u32,
    peer_initial_window: u32,
    peer_max_concurrent: u32,
    conn_send_window: i64,
    conn_recv_consumed: u32,
    next_stream: u32,
    streams: BTreeMap<u32, Stream>,
    /// Header block being assembled across CONTINUATION frames: (stream, block, end_stream).
    partial: Option<(u32, Vec<u8>, bool)>,
    pub goaway: Option<(u32, ErrorCode)>,
    pub settings_acked: bool,
}

fn h2(e: H2Error) -> Error {
    Error::Io(alloc::format!("{e}"))
}

impl<T: Transport> H2Conn<T> {
    /// Send the preface + our SETTINGS + a connection WINDOW_UPDATE, then read the server's SETTINGS (§3.4:
    /// it must be the first frame) and acknowledge it.
    pub fn handshake(mut t: T) -> Result<Self, Error> {
        let mut out = Vec::with_capacity(64);
        out.extend_from_slice(frame::PREFACE);
        let settings = frame::settings_payload(&[
            (frame::SETTINGS_ENABLE_PUSH, 0),
            (frame::SETTINGS_INITIAL_WINDOW_SIZE, RECV_WINDOW),
            (frame::SETTINGS_HEADER_TABLE_SIZE, 4096),
        ]);
        out.extend(frame::encode(frame::SETTINGS, 0, 0, &settings));
        out.extend(frame::encode(frame::WINDOW_UPDATE, 0, 0, &(CONN_RECV_WINDOW - 65_535).to_be_bytes()));
        t.write_all(&out)?;
        let mut c = H2Conn {
            t,
            rbuf: Vec::new(),
            enc: Encoder::new(4096),
            dec: Decoder::new(4096),
            peer_max_frame: frame::DEFAULT_MAX_FRAME_SIZE,
            peer_initial_window: 65_535,
            peer_max_concurrent: DEFAULT_MAX_CONCURRENT,
            conn_send_window: 65_535,
            conn_recv_consumed: 0,
            next_stream: 1,
            streams: BTreeMap::new(),
            partial: None,
            goaway: None,
            settings_acked: false,
        };
        let (h, p) = c.read_frame()?;
        if h.ty != frame::SETTINGS || h.flags & frame::FLAG_ACK != 0 {
            return Err(c.fail(ErrorCode::Protocol));
        }
        let f = frame::parse(&h, &p, OUR_MAX_FRAME).map_err(|e| c.fail_with(e))?;
        c.on_frame(f)?;
        Ok(c)
    }

    fn fail(&mut self, code: ErrorCode) -> Error {
        self.fail_with(H2Error::Connection(code))
    }

    /// A connection error: GOAWAY with the code (§5.4.1), then report it.
    fn fail_with(&mut self, e: H2Error) -> Error {
        if let H2Error::Connection(code) = e {
            let last = self.next_stream.saturating_sub(2);
            let mut p = Vec::new();
            p.extend_from_slice(&last.to_be_bytes());
            p.extend_from_slice(&code.to_u32().to_be_bytes());
            let _ = self.t.write_all(&frame::encode(frame::GOAWAY, 0, 0, &p));
        }
        h2(e)
    }

    fn fill(&mut self) -> Result<(), Error> {
        let mut tmp = [0u8; 16384];
        let n = self.t.read(&mut tmp)?;
        if n == 0 {
            return Err(Error::Http(crate::h1::H1Error::Truncated));
        }
        self.rbuf.extend_from_slice(&tmp[..n]);
        Ok(())
    }

    fn read_frame(&mut self) -> Result<(FrameHeader, Vec<u8>), Error> {
        while self.rbuf.len() < 9 {
            self.fill()?;
        }
        let mut hb = [0u8; 9];
        hb.copy_from_slice(&self.rbuf[..9]);
        let h = frame::parse_header(&hb);
        if h.len > OUR_MAX_FRAME {
            return Err(self.fail(ErrorCode::FrameSize));
        }
        while self.rbuf.len() < 9 + h.len as usize {
            self.fill()?;
        }
        let p: Vec<u8> = self.rbuf[9..9 + h.len as usize].to_vec();
        self.rbuf.drain(..9 + h.len as usize);
        Ok((h, p))
    }

    /// Read and process one frame.
    fn step(&mut self) -> Result<(), Error> {
        let (h, p) = self.read_frame()?;
        // §6.10: while a header block is open, only CONTINUATION on that stream may arrive.
        if let Some((s, _, _)) = &self.partial {
            if h.ty != frame::CONTINUATION || h.stream != *s {
                return Err(self.fail(ErrorCode::Protocol));
            }
        }
        match frame::parse(&h, &p, OUR_MAX_FRAME) {
            Ok(f) => self.on_frame(f),
            Err(H2Error::Stream(s, code)) => {
                self.rst(s, code)?;
                Ok(())
            }
            Err(e) => Err(self.fail_with(e)),
        }
    }

    fn rst(&mut self, s: u32, code: ErrorCode) -> Result<(), Error> {
        if let Some(st) = self.streams.get_mut(&s) {
            st.reset = Some(code);
        }
        self.t.write_all(&frame::encode(frame::RST_STREAM, 0, s, &code.to_u32().to_be_bytes()))
    }

    fn on_frame(&mut self, f: Frame) -> Result<(), Error> {
        match f {
            Frame::Settings { ack: true, .. } => self.settings_acked = true,
            Frame::Settings { ack: false, params } => {
                for (id, v) in params {
                    match id {
                        frame::SETTINGS_INITIAL_WINDOW_SIZE => {
                            // §6.9.2: adjust every open stream's window by the delta.
                            let delta = v as i64 - self.peer_initial_window as i64;
                            for st in self.streams.values_mut() {
                                st.send_window += delta;
                                if st.send_window > frame::MAX_WINDOW as i64 {
                                    return Err(self.fail(ErrorCode::FlowControl));
                                }
                            }
                            self.peer_initial_window = v;
                        }
                        frame::SETTINGS_MAX_FRAME_SIZE => self.peer_max_frame = v,
                        frame::SETTINGS_MAX_CONCURRENT_STREAMS => self.peer_max_concurrent = v,
                        frame::SETTINGS_HEADER_TABLE_SIZE => self.enc.set_max(v.min(4096) as usize),
                        _ => {}
                    }
                }
                self.t.write_all(&frame::encode(frame::SETTINGS, frame::FLAG_ACK, 0, &[]))?;
            }
            Frame::Ping { ack: false, data } => self.t.write_all(&frame::encode(frame::PING, frame::FLAG_ACK, 0, &data))?,
            Frame::Ping { ack: true, .. } => {}
            Frame::GoAway { last_stream, code, .. } => {
                // §6.8: streams above last_stream were not (and will not be) processed — refused, retryable.
                for (id, st) in self.streams.iter_mut() {
                    if *id > last_stream && st.reset.is_none() {
                        st.reset = Some(ErrorCode::RefusedStream);
                    }
                }
                self.goaway = Some((last_stream, code));
            }
            Frame::WindowUpdate { stream: 0, increment } => {
                self.conn_send_window += increment as i64;
                if self.conn_send_window > frame::MAX_WINDOW as i64 {
                    return Err(self.fail(ErrorCode::FlowControl));
                }
            }
            Frame::WindowUpdate { stream, increment } => {
                let over = match self.streams.get_mut(&stream) {
                    Some(st) => {
                        st.send_window += increment as i64;
                        st.send_window > frame::MAX_WINDOW as i64
                    }
                    None => false,
                };
                if over {
                    self.rst(stream, ErrorCode::FlowControl)?;
                }
            }
            Frame::RstStream { stream, code } => {
                if let Some(st) = self.streams.get_mut(&stream) {
                    st.reset = Some(code);
                }
            }
            Frame::PushPromise { .. } => return Err(self.fail(ErrorCode::Protocol)), // push was disabled
            Frame::Priority { .. } | Frame::Unknown { .. } => {}
            Frame::Headers { stream, block, end_stream, end_headers, .. } => {
                // An idle (never opened) stream is a protocol error; a closed one is discarded after decoding.
                if !self.streams.contains_key(&stream) && !self.was_opened(stream) {
                    return Err(self.fail(ErrorCode::Protocol));
                }
                if end_headers {
                    self.on_header_block(stream, &block, end_stream)?;
                } else {
                    self.partial = Some((stream, block, end_stream));
                }
            }
            Frame::Continuation { stream, block, end_headers } => {
                let Some((s, mut acc, es)) = self.partial.take() else {
                    return Err(self.fail(ErrorCode::Protocol));
                };
                acc.extend_from_slice(&block);
                if acc.len() > 1 << 20 {
                    return Err(self.fail(ErrorCode::EnhanceYourCalm));
                }
                if end_headers {
                    self.on_header_block(s, &acc, es)?;
                } else {
                    self.partial = Some((stream, acc, es));
                }
            }
            Frame::Data { stream, data, end_stream, flow_len } => {
                // Flow control counts the whole payload, padding included (§6.9.1). The connection window is
                // credited on receipt (the per-stream windows bound what is buffered).
                self.conn_recv_consumed += flow_len;
                if self.conn_recv_consumed >= CONN_RECV_WINDOW / 2 {
                    let n = core::mem::take(&mut self.conn_recv_consumed);
                    self.t.write_all(&frame::encode(frame::WINDOW_UPDATE, 0, 0, &n.to_be_bytes()))?;
                }
                let Some(st) = self.streams.get_mut(&stream) else {
                    if self.was_opened(stream) {
                        return Ok(()); // a stream we closed or cancelled: frames in flight are discarded (§5.1)
                    }
                    return Err(self.fail(ErrorCode::Protocol));
                };
                if st.end {
                    return self.rst(stream, ErrorCode::StreamClosed);
                }
                st.recv_outstanding += flow_len;
                if st.recv_outstanding > RECV_WINDOW {
                    return self.rst(stream, ErrorCode::FlowControl);
                }
                if data.is_empty() {
                    st.recv_taken += flow_len; // padding only: nothing for the consumer to take
                } else {
                    st.data.push_back((data, flow_len));
                }
                st.end |= end_stream;
            }
        }
        Ok(())
    }

    /// Was `id` a stream this side opened (client-initiated, below the next id)?
    fn was_opened(&self, id: u32) -> bool {
        id % 2 == 1 && id < self.next_stream
    }

    fn on_header_block(&mut self, stream: u32, block: &[u8], end_stream: bool) -> Result<(), Error> {
        let fields = match self.dec.decode(block) {
            Ok(f) => f,
            Err(_) => return Err(self.fail(ErrorCode::Compression)),
        };
        if !self.streams.contains_key(&stream) {
            return Ok(()); // a closed stream's block: decoded for the HPACK state, then dropped
        }
        let mut status: Option<u16> = None;
        let mut headers = Headers::new();
        let mut malformed = false;
        let mut regular_seen = false;
        for (n, v) in fields {
            let name = String::from_utf8_lossy(&n).into_owned();
            let value = String::from_utf8_lossy(&v).into_owned();
            if let Some(pseudo) = name.strip_prefix(':') {
                // §8.3: pseudo-headers precede regular fields; a response has exactly one :status.
                if regular_seen || pseudo != "status" || status.is_some() {
                    malformed = true;
                }
                status = value.parse().ok().filter(|s: &u16| (100..1000).contains(s));
                if status.is_none() {
                    malformed = true;
                }
                continue;
            }
            regular_seen = true;
            // §8.2.1/§8.2.2: lowercase names, no connection-specific fields.
            if name.bytes().any(|b| b.is_ascii_uppercase())
                || matches!(name.as_str(), "connection" | "keep-alive" | "proxy-connection" | "transfer-encoding" | "upgrade")
            {
                malformed = true;
            }
            if headers.append(&name, &value).is_err() {
                malformed = true;
            }
        }
        let st = self.streams.get_mut(&stream).expect("checked");
        let is_trailers = st.head.is_some();
        if (!is_trailers && status.is_none()) || (is_trailers && status.is_some()) {
            malformed = true;
        }
        if malformed {
            return self.rst(stream, ErrorCode::Protocol);
        }
        if is_trailers {
            st.trailers = Some(headers);
        } else if let Some(s) = status {
            if (100..200).contains(&s) {
                // Interim responses are skipped (§8.1).
            } else {
                st.head = Some((s, headers));
            }
        }
        st.end |= end_stream;
        Ok(())
    }

    /// Open a stream: send the request header block and body (under flow control). Returns the stream id.
    pub fn send_request(
        &mut self,
        method: &str,
        scheme: &str,
        authority: &str,
        path: &str,
        headers: &Headers,
        body: &[u8],
    ) -> Result<u32, Error> {
        if self.goaway.is_some() {
            return Err(Error::Io("h2: connection is going away".into()));
        }
        let id = self.next_stream;
        self.next_stream += 2;
        self.streams.insert(id, Stream { send_window: self.peer_initial_window as i64, ..Default::default() });
        let mut fields: Vec<(Vec<u8>, Vec<u8>)> = alloc::vec![
            (b":method".to_vec(), method.as_bytes().to_vec()),
            (b":scheme".to_vec(), scheme.as_bytes().to_vec()),
            (b":authority".to_vec(), authority.as_bytes().to_vec()),
            (b":path".to_vec(), path.as_bytes().to_vec()),
        ];
        for (k, v) in headers.iter() {
            let lk = k.to_ascii_lowercase();
            if matches!(lk.as_str(), "host" | "connection" | "keep-alive" | "proxy-connection" | "transfer-encoding" | "upgrade") {
                continue;
            }
            fields.push((lk.into_bytes(), v.as_bytes().to_vec()));
        }
        if !body.is_empty() && !headers.contains("content-length") {
            fields.push((b"content-length".to_vec(), alloc::format!("{}", body.len()).into_bytes()));
        }
        let block = self.enc.encode(fields.iter().map(|(n, v)| (n.as_slice(), v.as_slice())));
        let max = self.peer_max_frame as usize;
        let end_stream = if body.is_empty() { frame::FLAG_END_STREAM } else { 0 };
        let mut chunks = block.chunks(max).peekable();
        let first = chunks.next().unwrap_or(&[]);
        let mut out = frame::encode(frame::HEADERS, end_stream | if chunks.peek().is_none() { frame::FLAG_END_HEADERS } else { 0 }, id, first);
        while let Some(c) = chunks.next() {
            out.extend(frame::encode(frame::CONTINUATION, if chunks.peek().is_none() { frame::FLAG_END_HEADERS } else { 0 }, id, c));
        }
        self.t.write_all(&out)?;
        let mut sent = 0;
        while sent < body.len() {
            let window = self.conn_send_window.min(self.streams[&id].send_window);
            if window <= 0 {
                self.step()?;
                if let Some(code) = self.streams[&id].reset {
                    return Err(Error::Io(alloc::format!("h2: stream {id} reset ({code:?})")));
                }
                continue;
            }
            let n = (body.len() - sent).min(window as usize).min(max);
            let last = sent + n == body.len();
            self.t.write_all(&frame::encode(frame::DATA, if last { frame::FLAG_END_STREAM } else { 0 }, id, &body[sent..sent + n]))?;
            self.conn_send_window -= n as i64;
            self.streams.get_mut(&id).unwrap().send_window -= n as i64;
            sent += n;
        }
        Ok(id)
    }

    fn check_reset(&self, id: u32) -> Result<(), Error> {
        match self.streams.get(&id).and_then(|s| s.reset) {
            Some(code) => Err(Error::Io(alloc::format!("h2: stream {id} reset ({code:?})"))),
            None => Ok(()),
        }
    }

    /// Wait for the final response head of stream `id`.
    pub fn read_head(&mut self, id: u32) -> Result<(u16, Headers), Error> {
        loop {
            if let Some(h) = self.try_head(id)? {
                return Ok(h);
            }
            self.step()?;
        }
    }

    /// The final response head of stream `id` if it has arrived (never blocks). A stream that ended without
    /// one is truncated.
    pub fn try_head(&mut self, id: u32) -> Result<Option<(u16, Headers)>, Error> {
        self.check_reset(id)?;
        let st = self.streams.get_mut(&id).ok_or(Error::Io("h2: unknown stream".into()))?;
        if let Some(h) = st.head.take() {
            if h.1.is_empty() && h.0 == 0 {
                st.head = Some(h);
                return Err(Error::Io("h2: head already taken".into()));
            }
            // Keep a marker that the head arrived (trailers are told apart from it).
            st.head = Some((0, Headers::new()));
            return Ok(Some(h));
        }
        if st.end {
            return Err(Error::Http(crate::h1::H1Error::Truncated));
        }
        Ok(None)
    }

    /// The next DATA payload of stream `id` (`None` at END_STREAM). Credits the windows back.
    pub fn read_data(&mut self, id: u32) -> Result<Option<Vec<u8>>, Error> {
        loop {
            match self.try_data(id)? {
                DataPoll::Chunk(d) => return Ok(Some(d)),
                DataPoll::End => return Ok(None),
                DataPoll::Pending => self.step()?,
            }
        }
    }

    /// The next buffered DATA payload of stream `id` without blocking. Credits the windows back.
    pub fn try_data(&mut self, id: u32) -> Result<DataPoll, Error> {
        {
            self.check_reset(id)?;
            let st = self.streams.get_mut(&id).ok_or(Error::Io("h2: unknown stream".into()))?;
            if let Some((d, flow_len)) = st.data.pop_front() {
                // The consumer took these bytes: re-open the stream's window once half of it is taken.
                st.recv_taken += flow_len;
                if st.recv_taken >= RECV_WINDOW / 2 {
                    let n = core::mem::take(&mut st.recv_taken);
                    st.recv_outstanding = st.recv_outstanding.saturating_sub(n);
                    if !st.end {
                        self.t.write_all(&frame::encode(frame::WINDOW_UPDATE, 0, id, &n.to_be_bytes()))?;
                    }
                }
                return Ok(DataPoll::Chunk(d));
            }
            if st.end {
                return Ok(DataPoll::End);
            }
            Ok(DataPoll::Pending)
        }
    }

    /// Read and process exactly one frame (blocking until it is whole).
    pub fn step_frame(&mut self) -> Result<(), Error> {
        self.step()
    }

    /// Is a whole frame already in the read buffer (so [`H2Conn::step_frame`] will not wait on the socket)?
    pub fn has_buffered_frame(&self) -> bool {
        if self.rbuf.len() < 9 {
            return false;
        }
        let len = ((self.rbuf[0] as usize) << 16) | ((self.rbuf[1] as usize) << 8) | self.rbuf[2] as usize;
        self.rbuf.len() >= 9 + len
    }

    /// May another stream be opened now (no GOAWAY, under the peer's concurrency limit, ids left)?
    pub fn can_open(&self) -> bool {
        self.reusable() && (self.streams.len() as u64) < self.peer_max_concurrent as u64
    }

    /// Open streams (opened and not yet closed with [`H2Conn::close_stream`]).
    pub fn open_streams(&self) -> usize {
        self.streams.len()
    }

    /// Was stream `id` refused — RST_STREAM REFUSED_STREAM, or above a GOAWAY's last-stream-id — so the
    /// server never processed it (§8.7) and the request may be retried on another connection?
    pub fn refused(&self, id: u32) -> bool {
        self.streams.get(&id).and_then(|s| s.reset) == Some(ErrorCode::RefusedStream)
    }

    /// Abandon stream `id`: RST_STREAM CANCEL (§8.1) and forget it; the connection stays usable.
    pub fn cancel(&mut self, id: u32) -> Result<(), Error> {
        let open = self.streams.get(&id).is_some_and(|s| !s.end && s.reset.is_none());
        self.streams.remove(&id);
        if open {
            self.t.write_all(&frame::encode(frame::RST_STREAM, 0, id, &ErrorCode::Cancel.to_u32().to_be_bytes()))?;
        }
        Ok(())
    }

    pub fn trailers(&self, id: u32) -> Option<&Headers> {
        self.streams.get(&id).and_then(|s| s.trailers.as_ref())
    }

    /// Forget a finished stream.
    pub fn close_stream(&mut self, id: u32) {
        self.streams.remove(&id);
    }

    /// May another request go on this connection?
    pub fn reusable(&self) -> bool {
        self.goaway.is_none() && self.next_stream < (1 << 31) - 2
    }

    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.t
    }
}
