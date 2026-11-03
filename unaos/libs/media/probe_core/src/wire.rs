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
//!
//! SONDE-RECORD-IMU (LEDGER SR491): format code 4 ([`FORMAT_MOTION`]) is a MOTION record on the same header, never a
//! [`PixelFormat`]: stream 3 (accel) or 4 (gyro), width 1, height 1, stride = data length, offset 20 = the per-LSB scale
//! in SI units as `f32` bits (0 = unscaled), flags bit 1 = [`FLAG_EXTRINSIC`]; see [`MotionRecord`]. [`decode`] refuses
//! it as [`WireError::Format`], so a frame reader is never handed one.

use alloc::vec::Vec;

use crate::depth::DepthScale;
use crate::frame::{Frame, FrameError, PixelFormat, StreamKind};
use crate::imu::{ImuKind, ImuSample};

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

// ── SONDE-RECORD-IMU (LEDGER SR491): the motion record — one IMU sample on the PRB1 header ─────────────────────────────
//
// The D435i's IMU (its HID interface; the report is `imu::parse_raw`'s, librealsense `src/hid/hid-types.h:56-70`) is
// recorded beside the depth frames in arrival order. A sample is not a picture, so it is not a `PixelFormat`: the header
// is the frames' own 80 bytes with format code 4 and this payload, little-endian:
//
// | off | size | field |
// |---|---|---|
// | 0 | 6 | raw x, y, z as `i16`, as the device sent them |
// | 6 | 2 | zero |
// | 8 | 8 | the device's own timestamp (`u64`, the report's) |
// | 16 | 48 | only with [`FLAG_EXTRINSIC`]: depth-to-IMU rotation (9 `f32`, librealsense's `rs2_extrinsics` order) then translation (3 `f32`, metres) |
//
// Header offset 20 (a frame's depth units) carries the per-LSB scale in SI units (m/s² or rad/s) as `f32` bits; 0 means
// the writer did not know it (the gyro's scale needs the firmware version, `src/ds/ds-motion-common.cpp:520`), and the
// raw counts are all a reader gets ([`MotionRecord::sample`] is `None`). `timestamp_us` is the WRITER's clock (the same
// clock as its depth records), `seq` counts each stream on its own.

/// The motion record's format code (wire-only; never a [`PixelFormat`]).
pub const FORMAT_MOTION: u8 = 4;
/// Flags bit 1: the record carries the depth-to-IMU extrinsic after the sample.
pub const FLAG_EXTRINSIC: u32 = 2;
/// The sample's payload bytes.
pub const MOTION_LEN: usize = 16;
/// The extrinsic's payload bytes: 12 `f32`.
pub const EXTRINSIC_LEN: usize = 48;
/// The longest motion record: header + sample + extrinsic.
pub const MOTION_RECORD_MAX: usize = HEADER_LEN + MOTION_LEN + EXTRINSIC_LEN;

/// One IMU sample as recorded.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MotionRecord {
    pub kind: ImuKind,
    /// x, y, z as the device sent them.
    pub raw: [i16; 3],
    /// The report's own timestamp.
    pub device_ts: u64,
    /// SI units per LSB (m/s² for accel, rad/s for gyro); 0 = unscaled.
    pub scale: f32,
    pub seq: u64,
    /// The writer's clock, the same one its depth records carry.
    pub timestamp_us: u64,
    /// Depth-to-IMU: rotation (9, `rs2_extrinsics` order) then translation (3, metres).
    pub extrinsic: Option<[f32; 12]>,
    pub id: [u8; ID_MAX],
}

impl MotionRecord {
    /// The id as text (up to its first NUL).
    pub fn id_str(&self) -> &str {
        let n = self.id.iter().position(|&b| b == 0).unwrap_or(ID_MAX);
        core::str::from_utf8(&self.id[..n]).unwrap_or("")
    }

    /// The sample in SI units, or `None` when the record is unscaled.
    pub fn sample(&self) -> Option<ImuSample> {
        if self.scale == 0.0 {
            return None;
        }
        let k = self.scale;
        Some(ImuSample {
            kind: self.kind,
            timestamp: self.device_ts,
            xyz: [self.raw[0] as f32 * k, self.raw[1] as f32 * k, self.raw[2] as f32 * k],
        })
    }
}

/// Is `b` the start of a motion record (the magic, and format code 4)?
pub fn is_motion(b: &[u8]) -> bool {
    b.len() >= 8 && b[0..4] == MAGIC && b[7] == FORMAT_MOTION
}

/// A probe id as the record's NUL-padded field; longer than [`ID_MAX`] bytes is refused (a cut id names another probe).
pub fn motion_id(id: &str) -> Result<[u8; ID_MAX], WireError> {
    if id.len() > ID_MAX {
        return Err(WireError::Id);
    }
    let mut b = [0u8; ID_MAX];
    b[..id.len()].copy_from_slice(id.as_bytes());
    Ok(b)
}

/// The record for `m`: its bytes are the first `.1` bytes of `.0` (no allocation). An id that is not UTF-8 is refused.
pub fn encode_motion(m: &MotionRecord) -> Result<([u8; MOTION_RECORD_MAX], usize), WireError> {
    if core::str::from_utf8(&m.id[..m.id.iter().position(|&c| c == 0).unwrap_or(ID_MAX)]).is_err() {
        return Err(WireError::Id);
    }
    let data_len = MOTION_LEN + if m.extrinsic.is_some() { EXTRINSIC_LEN } else { 0 };
    let mut b = [0u8; MOTION_RECORD_MAX];
    b[0..4].copy_from_slice(&MAGIC);
    b[4..6].copy_from_slice(&(HEADER_LEN as u16).to_le_bytes());
    b[6] = match m.kind {
        ImuKind::Accel => stream_code(StreamKind::Accel),
        ImuKind::Gyro => stream_code(StreamKind::Gyro),
    };
    b[7] = FORMAT_MOTION;
    put32(&mut b, 8, 1);
    put32(&mut b, 12, 1);
    put32(&mut b, 16, data_len as u32);
    put32(&mut b, 20, m.scale.to_bits());
    put64(&mut b, 24, m.seq);
    put64(&mut b, 32, m.timestamp_us);
    put32(&mut b, 40, data_len as u32);
    if m.extrinsic.is_some() {
        put32(&mut b, 44, FLAG_EXTRINSIC);
    }
    b[64..80].copy_from_slice(&m.id);
    let p = HEADER_LEN;
    for (i, v) in m.raw.iter().enumerate() {
        b[p + 2 * i..p + 2 * i + 2].copy_from_slice(&v.to_le_bytes());
    }
    put64(&mut b, p + 8, m.device_ts);
    if let Some(e) = m.extrinsic {
        for (i, v) in e.iter().enumerate() {
            put32(&mut b, p + MOTION_LEN + 4 * i, v.to_bits());
        }
    }
    Ok((b, HEADER_LEN + data_len))
}

/// A motion header's (header length, data length), validated: magic, header length, stream (accel or gyro, else
/// [`WireError::Stream`]), format, the geometry and data length against the extrinsic flag ([`WireError::Geometry`]), id.
pub fn decode_motion_lengths(b: &[u8]) -> Result<(usize, usize), WireError> {
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
    match stream_of_code(b[6]) {
        Some(StreamKind::Accel) | Some(StreamKind::Gyro) => {}
        _ => return Err(WireError::Stream),
    }
    if b[7] != FORMAT_MOTION {
        return Err(WireError::Format);
    }
    let data_len = get32(b, 40) as usize;
    let want = MOTION_LEN + if get32(b, 44) & FLAG_EXTRINSIC != 0 { EXTRINSIC_LEN } else { 0 };
    if get32(b, 8) != 1 || get32(b, 12) != 1 || get32(b, 16) as usize != data_len || data_len != want {
        return Err(WireError::Geometry);
    }
    if core::str::from_utf8(&b[64..64 + b[64..80].iter().position(|&c| c == 0).unwrap_or(ID_MAX)]).is_err() {
        return Err(WireError::Id);
    }
    Ok((header_len, data_len))
}

/// Decode one whole motion record (`b` holds at least its header length + data length, else [`WireError::Short`]).
pub fn decode_motion(b: &[u8]) -> Result<MotionRecord, WireError> {
    let (hl, dl) = decode_motion_lengths(b)?;
    if b.len() < hl + dl {
        return Err(WireError::Short);
    }
    let kind = if stream_of_code(b[6]) == Some(StreamKind::Accel) { ImuKind::Accel } else { ImuKind::Gyro };
    let p = hl;
    let r = |o: usize| i16::from_le_bytes([b[p + o], b[p + o + 1]]);
    let extrinsic = (dl == MOTION_LEN + EXTRINSIC_LEN).then(|| {
        let mut e = [0f32; 12];
        for (i, v) in e.iter_mut().enumerate() {
            *v = f32::from_bits(get32(b, p + MOTION_LEN + 4 * i));
        }
        e
    });
    let mut id = [0u8; ID_MAX];
    id.copy_from_slice(&b[64..80]);
    Ok(MotionRecord {
        kind,
        raw: [r(0), r(2), r(4)],
        device_ts: get64(b, p + 8),
        scale: f32::from_bits(get32(b, 20)),
        seq: get64(b, 24),
        timestamp_us: get64(b, 32),
        extrinsic,
        id,
    })
}
