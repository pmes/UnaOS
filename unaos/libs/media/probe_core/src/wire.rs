// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! SONDE2 (LEDGER SR105): one frame as BYTES — the record a ring-0 probe hands ring 3.
//!
//! The host's Sonde speaks `SMessage::ProbeFrame` on bandy (REALSENSE, SR92): `id, stream, format, seq,
//! timestamp_us, width, height, stride, depth_scale, intrinsics, data`. The kernel's bus cannot carry that
//! verb (its frame body ceiling is 4 KiB and its verb space has no probe tag), so the kernel publishes the
//! SAME fields as a file, `/dev/probe/depth0`, read with `SYS_PATH_READ` at any offset: this header, then
//! the frame's bytes. A ring-3 reader decodes the header here and builds its `ProbeFrame` (or its
//! [`Frame`]) field for field — one set of fields, two transports, no second meaning.
//!
//! Layout, all little-endian, [`HEADER_LEN`] = 80 bytes:
//!
//! | off | size | field |
//! |---|---|---|
//! | 0 | 4 | magic `PRB1` |
//! | 4 | 2 | header length (80; a reader skips a longer header it does not know) |
//! | 6 | 1 | stream code ([`stream_code`]: depth 0, ir 1, color 2, accel 3, gyro 4) |
//! | 7 | 1 | format code ([`format_code`]: Z16 0, Y8 1, Y16 2, YUY2 3) |
//! | 8 | 4 | width |
//! | 12 | 4 | height |
//! | 16 | 4 | stride (bytes per row) |
//! | 20 | 4 | depth units, micrometres per Z16 unit (0 = not a depth stream) — the firmware's own integer |
//! | 24 | 8 | seq |
//! | 32 | 8 | timestamp_us |
//! | 40 | 4 | data length (the bytes that follow the header) |
//! | 44 | 4 | flags: bit 0 = intrinsics present |
//! | 48 | 16 | intrinsics `fx, fy, ppx, ppy` as `f32` bit patterns (zero when absent) |
//! | 64 | 16 | probe id, UTF-8, NUL-padded (`uvc:8086:0b3a`) |

use alloc::vec::Vec;

use crate::depth::DepthScale;
use crate::frame::{Frame, FrameError, PixelFormat, StreamKind};

pub const MAGIC: [u8; 4] = *b"PRB1";
pub const HEADER_LEN: usize = 80;
pub const ID_MAX: usize = 16;
pub const FLAG_INTRINSICS: u32 = 1;

pub const fn stream_code(s: StreamKind) -> u8 {
    match s {
        StreamKind::Depth => 0,
        StreamKind::Infrared => 1,
        StreamKind::Color => 2,
        StreamKind::Accel => 3,
        StreamKind::Gyro => 4,
    }
}

pub const fn stream_of_code(c: u8) -> Option<StreamKind> {
    match c {
        0 => Some(StreamKind::Depth),
        1 => Some(StreamKind::Infrared),
        2 => Some(StreamKind::Color),
        3 => Some(StreamKind::Accel),
        4 => Some(StreamKind::Gyro),
        _ => None,
    }
}

pub const fn format_code(f: PixelFormat) -> u8 {
    match f {
        PixelFormat::Z16 => 0,
        PixelFormat::Y8 => 1,
        PixelFormat::Y16 => 2,
        PixelFormat::Yuy2 => 3,
    }
}

pub const fn format_of_code(c: u8) -> Option<PixelFormat> {
    match c {
        0 => Some(PixelFormat::Z16),
        1 => Some(PixelFormat::Y8),
        2 => Some(PixelFormat::Y16),
        3 => Some(PixelFormat::Yuy2),
        _ => None,
    }
}

/// Why a header was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WireError {
    Short,
    Magic,
    HeaderLen,
    Stream,
    Format,
    /// The stated geometry needs more bytes than the stated data length.
    Geometry,
    Id,
}

/// A decoded header.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Header {
    pub header_len: usize,
    pub stream: StreamKind,
    pub format: PixelFormat,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub depth_units_um: u32,
    pub seq: u64,
    pub timestamp_us: u64,
    pub data_len: u32,
    pub intrinsics: Option<[f32; 4]>,
    pub id: [u8; ID_MAX],
}

impl Header {
    /// The id as text (up to its first NUL).
    pub fn id_str(&self) -> &str {
        let n = self.id.iter().position(|&b| b == 0).unwrap_or(ID_MAX);
        core::str::from_utf8(&self.id[..n]).unwrap_or("")
    }

    /// The depth scale the header carries (`None` for a non-depth stream).
    pub fn depth_scale(&self) -> Option<DepthScale> {
        (self.format == PixelFormat::Z16 && self.depth_units_um != 0).then(|| DepthScale::from_micrometres(self.depth_units_um))
    }

    /// The frame, from the bytes that followed the header.
    pub fn frame(&self, data: Vec<u8>) -> Result<Frame, FrameError> {
        Frame::new(self.stream, self.format, self.width, self.height, self.stride as usize, self.timestamp_us, self.seq, data)
    }
}

fn put32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

fn put64(b: &mut [u8], o: usize, v: u64) {
    b[o..o + 8].copy_from_slice(&v.to_le_bytes());
}

fn get32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn get64(b: &[u8], o: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[o..o + 8]);
    u64::from_le_bytes(a)
}

/// The header for `frame` (its data length is `frame.data.len()`). An `id` longer than [`ID_MAX`] bytes is
/// refused rather than cut (a cut id names another probe).
pub fn encode(
    frame: &Frame,
    id: &str,
    depth_units_um: u32,
    intrinsics: Option<[f32; 4]>,
) -> Result<[u8; HEADER_LEN], WireError> {
    if id.len() > ID_MAX {
        return Err(WireError::Id);
    }
    let mut b = [0u8; HEADER_LEN];
    b[0..4].copy_from_slice(&MAGIC);
    b[4..6].copy_from_slice(&(HEADER_LEN as u16).to_le_bytes());
    b[6] = stream_code(frame.stream);
    b[7] = format_code(frame.format);
    put32(&mut b, 8, frame.width);
    put32(&mut b, 12, frame.height);
    put32(&mut b, 16, frame.stride as u32);
    put32(&mut b, 20, if frame.format == PixelFormat::Z16 { depth_units_um } else { 0 });
    put64(&mut b, 24, frame.seq);
    put64(&mut b, 32, frame.timestamp_us);
    put32(&mut b, 40, frame.data.len() as u32);
    if let Some(k) = intrinsics {
        put32(&mut b, 44, FLAG_INTRINSICS);
        for (i, v) in k.iter().enumerate() {
            put32(&mut b, 48 + 4 * i, v.to_bits());
        }
    }
    b[64..64 + id.len()].copy_from_slice(id.as_bytes());
    Ok(b)
}

/// Decode and validate a header: magic, length, codes, and that the geometry fits the data length.
pub fn decode(b: &[u8]) -> Result<Header, WireError> {
    if b.len() < HEADER_LEN {
        return Err(WireError::Short);
    }
    if b[0..4] != MAGIC {
        return Err(WireError::Magic);
    }
    let header_len = u16::from_le_bytes([b[4], b[5]]) as usize;
    if header_len < HEADER_LEN {
        return Err(WireError::HeaderLen);
    }
    let stream = stream_of_code(b[6]).ok_or(WireError::Stream)?;
    let format = format_of_code(b[7]).ok_or(WireError::Format)?;
    let (width, height, stride) = (get32(b, 8), get32(b, 12), get32(b, 16));
    let data_len = get32(b, 40);
    if width == 0
        || height == 0
        || (stride as usize) < width as usize * format.bytes_per_pixel()
        || Frame::needed(format, width, height, stride as usize) > data_len as usize
    {
        return Err(WireError::Geometry);
    }
    let intrinsics = (get32(b, 44) & FLAG_INTRINSICS != 0).then(|| {
        [f32::from_bits(get32(b, 48)), f32::from_bits(get32(b, 52)), f32::from_bits(get32(b, 56)), f32::from_bits(get32(b, 60))]
    });
    let mut id = [0u8; ID_MAX];
    id.copy_from_slice(&b[64..80]);
    if core::str::from_utf8(&id[..id.iter().position(|&c| c == 0).unwrap_or(ID_MAX)]).is_err() {
        return Err(WireError::Id);
    }
    Ok(Header {
        header_len,
        stream,
        format,
        width,
        height,
        stride,
        depth_units_um: get32(b, 20),
        seq: get64(b, 24),
        timestamp_us: get64(b, 32),
        data_len,
        intrinsics,
        id,
    })
}
