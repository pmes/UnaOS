// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! The probe frame model: one picture from one stream of one probe.
//!
//! The formats are the four a D435i's video interfaces carry. Their UVC format GUIDs lead with a FourCC
//! (the GUID is `<fourcc>-0000-0010-8000-00AA00389B71`, USB Video Class 1.1 Uncompressed payload §2), and
//! librealsense maps those FourCCs to formats in `src/ds/d400/d400-device.cpp:55-68`: `Z16 ` → Z16 (:64),
//! `GREY` → Y8 (:59), `Y16 ` → Y16 (:62), `YUY2` → YUYV (:56). The `Y8  ` spelling is the UVC payload
//! document's own name for 8-bit luminance and is accepted beside `GREY`.

use alloc::vec::Vec;

/// The pixel layout of a frame's bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PixelFormat {
    /// 16-bit little-endian depth units; multiply by the [`crate::DepthScale`] for metres; 0 = no data.
    Z16,
    /// 8-bit luminance (the infrared imagers).
    Y8,
    /// 16-bit little-endian luminance.
    Y16,
    /// 4:2:2 packed Y0 U Y1 V, two pixels per four bytes (the colour imager).
    Yuy2,
}

impl PixelFormat {
    /// Bytes per pixel (YUY2 averages two: four bytes carry two pixels).
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            PixelFormat::Y8 => 1,
            PixelFormat::Z16 | PixelFormat::Y16 | PixelFormat::Yuy2 => 2,
        }
    }

    /// The UVC GUID's leading FourCC → format (see the module note for the citations).
    pub fn from_fourcc(cc: [u8; 4]) -> Option<PixelFormat> {
        match &cc {
            b"Z16 " => Some(PixelFormat::Z16),
            b"GREY" | b"Y8  " => Some(PixelFormat::Y8),
            b"Y16 " => Some(PixelFormat::Y16),
            b"YUY2" | b"YUYV" => Some(PixelFormat::Yuy2),
            _ => None,
        }
    }

    /// The format's wire name (the `format` field of `SMessage::ProbeFrame`).
    pub const fn name(self) -> &'static str {
        match self {
            PixelFormat::Z16 => "Z16",
            PixelFormat::Y8 => "Y8",
            PixelFormat::Y16 => "Y16",
            PixelFormat::Yuy2 => "YUY2",
        }
    }

    pub fn from_name(s: &str) -> Option<PixelFormat> {
        match s {
            "Z16" => Some(PixelFormat::Z16),
            "Y8" => Some(PixelFormat::Y8),
            "Y16" => Some(PixelFormat::Y16),
            "YUY2" => Some(PixelFormat::Yuy2),
            _ => None,
        }
    }
}

/// Which stream of a probe a frame (or sample) came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StreamKind {
    Depth,
    Infrared,
    Color,
    Accel,
    Gyro,
}

impl StreamKind {
    /// The stream's wire name (the `stream` field of the `Probe*` verbs).
    pub const fn name(self) -> &'static str {
        match self {
            StreamKind::Depth => "depth",
            StreamKind::Infrared => "ir",
            StreamKind::Color => "color",
            StreamKind::Accel => "accel",
            StreamKind::Gyro => "gyro",
        }
    }

    pub fn from_name(s: &str) -> Option<StreamKind> {
        match s {
            "depth" => Some(StreamKind::Depth),
            "ir" => Some(StreamKind::Infrared),
            "color" => Some(StreamKind::Color),
            "accel" => Some(StreamKind::Accel),
            "gyro" => Some(StreamKind::Gyro),
            _ => None,
        }
    }

    /// SONDE2 (LEDGER SR105): the stream a UVC VideoStreaming function carries, from its FIRST format's
    /// GUID FourCC — the D400 census's classification, decided here once for both rings (the kernel's
    /// `drivers/uvc.rs` `multi::role_of` and the host face read the same table). A superset of
    /// [`PixelFormat::from_fourcc`]: the D400 also declares two formats that are not single-plane frames
    /// this crate lays out — `Y8I ` (the two infrared imagers interleaved, left/right in one 16-bit word)
    /// and `UYVY` (4:2:2 in the other byte order, the colour imager's alternative) — and a function whose
    /// first format is one of them is still that stream's function (librealsense `d400-device.cpp:55-75`
    /// lists both beside the four above; the kernel census carried the same two since REALSENSE).
    /// `None` = not a probe stream this crate knows (an MJPEG webcam, an unknown GUID).
    pub fn of_uvc_fourcc(cc: [u8; 4]) -> Option<StreamKind> {
        if let Some(f) = PixelFormat::from_fourcc(cc) {
            return Some(StreamKind::for_format(f));
        }
        match &cc {
            b"Y8I " => Some(StreamKind::Infrared),
            b"UYVY" => Some(StreamKind::Color),
            _ => None,
        }
    }

    /// The stream a video format is carried on, for a D400 (librealsense
    /// `d400-device.cpp:71-75`: YUY2 → COLOR, GREY → INFRARED; Z16 is the depth interface's).
    pub const fn for_format(f: PixelFormat) -> StreamKind {
        match f {
            PixelFormat::Z16 => StreamKind::Depth,
            PixelFormat::Y8 | PixelFormat::Y16 => StreamKind::Infrared,
            PixelFormat::Yuy2 => StreamKind::Color,
        }
    }
}

/// Why a [`Frame`] was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameError {
    /// Width or height is zero.
    Empty,
    /// The stride is shorter than one row of pixels.
    StrideTooShort { stride: usize, row: usize },
    /// YUY2 carries pixels in pairs; an odd width cannot be laid out.
    OddWidth,
    /// The data is shorter than `stride * (height - 1) + row`.
    Short { have: usize, need: usize },
}

/// One frame. Constructed only through [`Frame::new`], so every frame in the system has a size its
/// bytes can hold — the accessors below index without re-checking the geometry.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub stream: StreamKind,
    pub format: PixelFormat,
    pub width: u32,
    pub height: u32,
    /// Bytes from the start of one row to the start of the next (≥ `width * bytes_per_pixel`).
    pub stride: usize,
    /// Capture time in microseconds on the probe's clock.
    pub timestamp_us: u64,
    /// Frame counter on its stream.
    pub seq: u64,
    pub data: Vec<u8>,
}

impl Frame {
    /// The minimum byte length a `width x height` frame of `format` at `stride` needs.
    pub fn needed(format: PixelFormat, width: u32, height: u32, stride: usize) -> usize {
        let row = width as usize * format.bytes_per_pixel();
        if height == 0 { 0 } else { stride * (height as usize - 1) + row }
    }

    pub fn new(
        stream: StreamKind,
        format: PixelFormat,
        width: u32,
        height: u32,
        stride: usize,
        timestamp_us: u64,
        seq: u64,
        data: Vec<u8>,
    ) -> Result<Frame, FrameError> {
        if width == 0 || height == 0 {
            return Err(FrameError::Empty);
        }
        if format == PixelFormat::Yuy2 && width % 2 != 0 {
            return Err(FrameError::OddWidth);
        }
        let row = width as usize * format.bytes_per_pixel();
        if stride < row {
            return Err(FrameError::StrideTooShort { stride, row });
        }
        let need = Frame::needed(format, width, height, stride);
        if data.len() < need {
            return Err(FrameError::Short { have: data.len(), need });
        }
        Ok(Frame { stream, format, width, height, stride, timestamp_us, seq, data })
    }

    /// A tightly packed frame (`stride = width * bytes_per_pixel`).
    pub fn packed(
        stream: StreamKind,
        format: PixelFormat,
        width: u32,
        height: u32,
        timestamp_us: u64,
        seq: u64,
        data: Vec<u8>,
    ) -> Result<Frame, FrameError> {
        let stride = width as usize * format.bytes_per_pixel();
        Frame::new(stream, format, width, height, stride, timestamp_us, seq, data)
    }

    #[inline]
    fn offset(&self, x: u32, y: u32) -> Option<usize> {
        if x >= self.width || y >= self.height {
            return None;
        }
        Some(y as usize * self.stride + x as usize * self.format.bytes_per_pixel())
    }

    /// The raw 16-bit sample at `(x, y)` of a Z16 or Y16 frame (little-endian on the wire).
    pub fn u16_at(&self, x: u32, y: u32) -> Option<u16> {
        if !matches!(self.format, PixelFormat::Z16 | PixelFormat::Y16) {
            return None;
        }
        let o = self.offset(x, y)?;
        Some(u16::from_le_bytes([self.data[o], self.data[o + 1]]))
    }

    /// The luminance at `(x, y)`: Y8 as is, Y16 by its high byte, YUY2 by its Y sample.
    pub fn luma_at(&self, x: u32, y: u32) -> Option<u8> {
        let o = self.offset(x, y)?;
        match self.format {
            PixelFormat::Y8 => Some(self.data[o]),
            PixelFormat::Y16 => Some(self.data[o + 1]),
            // YUY2: pixel 2k is byte 4k, pixel 2k+1 is byte 4k+2 — `offset` already lands on 2x.
            PixelFormat::Yuy2 => Some(self.data[o]),
            PixelFormat::Z16 => None,
        }
    }
}
