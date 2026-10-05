//! HTTP/2 framing (RFC 9113 §4, §6): the 9-octet frame header, every frame type's payload with the size and
//! stream-identifier rules of §6 enforced, SETTINGS parameter validation (§6.5.2), and the error codes (§7).

use alloc::vec::Vec;
use core::fmt;

/// §3.4 the client connection preface.
pub const PREFACE: &[u8; 24] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

pub const DATA: u8 = 0x0;
pub const HEADERS: u8 = 0x1;
pub const PRIORITY: u8 = 0x2;
pub const RST_STREAM: u8 = 0x3;
pub const SETTINGS: u8 = 0x4;
pub const PUSH_PROMISE: u8 = 0x5;
pub const PING: u8 = 0x6;
pub const GOAWAY: u8 = 0x7;
pub const WINDOW_UPDATE: u8 = 0x8;
pub const CONTINUATION: u8 = 0x9;

pub const FLAG_END_STREAM: u8 = 0x1;
pub const FLAG_ACK: u8 = 0x1;
pub const FLAG_END_HEADERS: u8 = 0x4;
pub const FLAG_PADDED: u8 = 0x8;
pub const FLAG_PRIORITY: u8 = 0x20;

/// §6.5.2 setting identifiers.
pub const SETTINGS_HEADER_TABLE_SIZE: u16 = 0x1;
pub const SETTINGS_ENABLE_PUSH: u16 = 0x2;
pub const SETTINGS_MAX_CONCURRENT_STREAMS: u16 = 0x3;
pub const SETTINGS_INITIAL_WINDOW_SIZE: u16 = 0x4;
pub const SETTINGS_MAX_FRAME_SIZE: u16 = 0x5;
pub const SETTINGS_MAX_HEADER_LIST_SIZE: u16 = 0x6;

pub const DEFAULT_MAX_FRAME_SIZE: u32 = 16_384;
pub const MAX_WINDOW: u32 = (1 << 31) - 1;

/// §7 error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    NoError,
    Protocol,
    Internal,
    FlowControl,
    SettingsTimeout,
    StreamClosed,
    FrameSize,
    RefusedStream,
    Cancel,
    Compression,
    Connect,
    EnhanceYourCalm,
    InadequateSecurity,
    Http11Required,
    Unknown(u32),
}

impl ErrorCode {
    pub fn from_u32(v: u32) -> Self {
        use ErrorCode::*;
        match v {
            0 => NoError,
            1 => Protocol,
            2 => Internal,
            3 => FlowControl,
            4 => SettingsTimeout,
            5 => StreamClosed,
            6 => FrameSize,
            7 => RefusedStream,
            8 => Cancel,
            9 => Compression,
            0xa => Connect,
            0xb => EnhanceYourCalm,
            0xc => InadequateSecurity,
            0xd => Http11Required,
            o => Unknown(o),
        }
    }
    pub fn to_u32(self) -> u32 {
        use ErrorCode::*;
        match self {
            NoError => 0,
            Protocol => 1,
            Internal => 2,
            FlowControl => 3,
            SettingsTimeout => 4,
            StreamClosed => 5,
            FrameSize => 6,
            RefusedStream => 7,
            Cancel => 8,
            Compression => 9,
            Connect => 0xa,
            EnhanceYourCalm => 0xb,
            InadequateSecurity => 0xc,
            Http11Required => 0xd,
            Unknown(o) => o,
        }
    }
}

/// A framing error: of the whole connection, or of one stream (§5.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum H2Error {
    Connection(ErrorCode),
    Stream(u32, ErrorCode),
}

impl fmt::Display for H2Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            H2Error::Connection(c) => write!(f, "h2 connection error {c:?}"),
            H2Error::Stream(s, c) => write!(f, "h2 stream {s} error {c:?}"),
        }
    }
}

/// §4.1 the frame header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHeader {
    pub len: u32,
    pub ty: u8,
    pub flags: u8,
    pub stream: u32,
}

pub fn parse_header(b: &[u8; 9]) -> FrameHeader {
    FrameHeader {
        len: (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32,
        ty: b[3],
        flags: b[4],
        // The reserved bit is ignored on receipt (§4.1).
        stream: u32::from_be_bytes([b[5], b[6], b[7], b[8]]) & 0x7FFF_FFFF,
    }
}

/// Encode a whole frame.
pub fn encode(ty: u8, flags: u8, stream: u32, payload: &[u8]) -> Vec<u8> {
    let len = payload.len() as u32;
    let mut o = Vec::with_capacity(9 + payload.len());
    o.extend_from_slice(&[(len >> 16) as u8, (len >> 8) as u8, len as u8, ty, flags]);
    o.extend_from_slice(&(stream & 0x7FFF_FFFF).to_be_bytes());
    o.extend_from_slice(payload);
    o
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Priority {
    pub exclusive: bool,
    pub depends_on: u32,
    pub weight: u8,
}

/// A parsed frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    Data { stream: u32, data: Vec<u8>, end_stream: bool, flow_len: u32 },
    Headers { stream: u32, block: Vec<u8>, end_stream: bool, end_headers: bool, priority: Option<Priority> },
    Priority { stream: u32, priority: Priority },
    RstStream { stream: u32, code: ErrorCode },
    Settings { ack: bool, params: Vec<(u16, u32)> },
    PushPromise { stream: u32, promised: u32, block: Vec<u8>, end_headers: bool },
    Ping { ack: bool, data: [u8; 8] },
    GoAway { last_stream: u32, code: ErrorCode, debug: Vec<u8> },
    WindowUpdate { stream: u32, increment: u32 },
    Continuation { stream: u32, block: Vec<u8>, end_headers: bool },
    /// §4.1: unknown types are ignored.
    Unknown { ty: u8, stream: u32 },
}

fn conn(c: ErrorCode) -> H2Error {
    H2Error::Connection(c)
}

/// Strip §6.1 padding when PADDED is set: the pad length octet plus that many trailing octets.
fn unpad(flags: u8, p: &[u8]) -> Result<&[u8], H2Error> {
    if flags & FLAG_PADDED == 0 {
        return Ok(p);
    }
    let pad = *p.first().ok_or(conn(ErrorCode::FrameSize))? as usize;
    if pad >= p.len() {
        return Err(conn(ErrorCode::Protocol));
    }
    Ok(&p[1..p.len() - pad])
}

fn priority(p: &[u8]) -> Priority {
    let d = u32::from_be_bytes([p[0], p[1], p[2], p[3]]);
    Priority { exclusive: d & 0x8000_0000 != 0, depends_on: d & 0x7FFF_FFFF, weight: p[4] }
}

/// Validate one SETTINGS parameter (§6.5.2).
pub fn check_setting(id: u16, v: u32) -> Result<(), H2Error> {
    match id {
        SETTINGS_ENABLE_PUSH if v > 1 => Err(conn(ErrorCode::Protocol)),
        SETTINGS_INITIAL_WINDOW_SIZE if v > MAX_WINDOW => Err(conn(ErrorCode::FlowControl)),
        SETTINGS_MAX_FRAME_SIZE if !(DEFAULT_MAX_FRAME_SIZE..=(1 << 24) - 1).contains(&v) => Err(conn(ErrorCode::Protocol)),
        _ => Ok(()),
    }
}

/// Parse a frame's payload under the §6 rules. `max_frame_size` is OUR advertised SETTINGS_MAX_FRAME_SIZE.
pub fn parse(h: &FrameHeader, p: &[u8], max_frame_size: u32) -> Result<Frame, H2Error> {
    if h.len > max_frame_size {
        return Err(conn(ErrorCode::FrameSize));
    }
    debug_assert_eq!(p.len() as u32, h.len);
    let s = h.stream;
    Ok(match h.ty {
        DATA => {
            if s == 0 {
                return Err(conn(ErrorCode::Protocol));
            }
            let d = unpad(h.flags, p)?;
            Frame::Data { stream: s, data: d.to_vec(), end_stream: h.flags & FLAG_END_STREAM != 0, flow_len: h.len }
        }
        HEADERS => {
            if s == 0 {
                return Err(conn(ErrorCode::Protocol));
            }
            let mut d = unpad(h.flags, p)?;
            let mut pr = None;
            if h.flags & FLAG_PRIORITY != 0 {
                if d.len() < 5 {
                    return Err(conn(ErrorCode::FrameSize));
                }
                pr = Some(priority(d));
                d = &d[5..];
            }
            Frame::Headers {
                stream: s,
                block: d.to_vec(),
                end_stream: h.flags & FLAG_END_STREAM != 0,
                end_headers: h.flags & FLAG_END_HEADERS != 0,
                priority: pr,
            }
        }
        PRIORITY => {
            if s == 0 {
                return Err(conn(ErrorCode::Protocol));
            }
            if p.len() != 5 {
                return Err(H2Error::Stream(s, ErrorCode::FrameSize));
            }
            Frame::Priority { stream: s, priority: priority(p) }
        }
        RST_STREAM => {
            if s == 0 {
                return Err(conn(ErrorCode::Protocol));
            }
            if p.len() != 4 {
                return Err(conn(ErrorCode::FrameSize));
            }
            Frame::RstStream { stream: s, code: ErrorCode::from_u32(u32::from_be_bytes([p[0], p[1], p[2], p[3]])) }
        }
        SETTINGS => {
            if s != 0 {
                return Err(conn(ErrorCode::Protocol));
            }
            let ack = h.flags & FLAG_ACK != 0;
            if (ack && !p.is_empty()) || p.len() % 6 != 0 {
                return Err(conn(ErrorCode::FrameSize));
            }
            let mut params = Vec::with_capacity(p.len() / 6);
            for c in p.chunks(6) {
                let id = u16::from_be_bytes([c[0], c[1]]);
                let v = u32::from_be_bytes([c[2], c[3], c[4], c[5]]);
                check_setting(id, v)?;
                params.push((id, v));
            }
            Frame::Settings { ack, params }
        }
        PUSH_PROMISE => {
            if s == 0 {
                return Err(conn(ErrorCode::Protocol));
            }
            let d = unpad(h.flags, p)?;
            if d.len() < 4 {
                return Err(conn(ErrorCode::FrameSize));
            }
            Frame::PushPromise {
                stream: s,
                promised: u32::from_be_bytes([d[0], d[1], d[2], d[3]]) & 0x7FFF_FFFF,
                block: d[4..].to_vec(),
                end_headers: h.flags & FLAG_END_HEADERS != 0,
            }
        }
        PING => {
            if s != 0 {
                return Err(conn(ErrorCode::Protocol));
            }
            if p.len() != 8 {
                return Err(conn(ErrorCode::FrameSize));
            }
            let mut data = [0u8; 8];
            data.copy_from_slice(p);
            Frame::Ping { ack: h.flags & FLAG_ACK != 0, data }
        }
        GOAWAY => {
            if s != 0 {
                return Err(conn(ErrorCode::Protocol));
            }
            if p.len() < 8 {
                return Err(conn(ErrorCode::FrameSize));
            }
            Frame::GoAway {
                last_stream: u32::from_be_bytes([p[0], p[1], p[2], p[3]]) & 0x7FFF_FFFF,
                code: ErrorCode::from_u32(u32::from_be_bytes([p[4], p[5], p[6], p[7]])),
                debug: p[8..].to_vec(),
            }
        }
        WINDOW_UPDATE => {
            if p.len() != 4 {
                return Err(conn(ErrorCode::FrameSize));
            }
            let inc = u32::from_be_bytes([p[0], p[1], p[2], p[3]]) & 0x7FFF_FFFF;
            if inc == 0 {
                return Err(if s == 0 { conn(ErrorCode::Protocol) } else { H2Error::Stream(s, ErrorCode::Protocol) });
            }
            Frame::WindowUpdate { stream: s, increment: inc }
        }
        CONTINUATION => {
            if s == 0 {
                return Err(conn(ErrorCode::Protocol));
            }
            Frame::Continuation { stream: s, block: p.to_vec(), end_headers: h.flags & FLAG_END_HEADERS != 0 }
        }
        ty => Frame::Unknown { ty, stream: s },
    })
}

/// SETTINGS payload from parameters.
pub fn settings_payload(params: &[(u16, u32)]) -> Vec<u8> {
    let mut o = Vec::with_capacity(params.len() * 6);
    for &(id, v) in params {
        o.extend_from_slice(&id.to_be_bytes());
        o.extend_from_slice(&v.to_be_bytes());
    }
    o
}
