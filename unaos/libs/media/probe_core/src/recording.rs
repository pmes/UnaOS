// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! VUGSCANTOOL (LEDGER SR471): a RECORDING — [`wire`](crate::wire) records back to back.
//!
//! CHARTER: Sonde — shared-core (R112): a recording is PRB1 records back to back; one format the kernel writes and
//! every reader walks.
//!
//! A `.prb` file is nothing but SONDE2's record (the 80-byte `PRB1` header + the frame's bytes) repeated. Nothing
//! in a record is recording-global: id, depth units, intrinsics, seq and timestamp ride on every record, so a
//! recording needs no header of its own. Every record delimits itself by its `header length` (a longer header than
//! [`HEADER_LEN`] is skipped, as `decode` documents) and its `data length`, and the magic repeats, so a walk that
//! lands off a record boundary is refused as [`WireError::Magic`] instead of decoding garbage.
//!
//! Two facts the walker owns, because `wire` does not name them:
//! - `decode` admits `data length > Frame::needed` (padding after the last row), so the walk steps by
//!   `header length + data length`, never by the geometry, and the frame receives exactly `data length` bytes;
//! - `decode` refuses a short HEADER (`Short`) but cannot see the data: a record whose data runs past the end of
//!   the bytes is [`RecordingError::Truncated`], as is a tail shorter than one header.
//!
//! Every byte of a header goes through [`wire::encode`] / [`wire::decode`]; this module only walks and slices.

use alloc::string::String;
use alloc::vec::Vec;

use crate::depth::{DepthScale, Intrinsics};
use crate::frame::{Frame, FrameError, PixelFormat, StreamKind};
use crate::wire::{self, HEADER_LEN, Header, WireError};

/// One record: its decoded header and its frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    pub header: Header,
    pub frame: Frame,
}

impl Record {
    /// A record for `frame`, its header built by [`wire::encode`] and read back by [`wire::decode`] (one path:
    /// a record built here is exactly what a reader of its bytes gets). An `id` over [`wire::ID_MAX`] bytes is
    /// refused as [`WireError::Id`].
    pub fn new(frame: Frame, id: &str, depth_units_um: u32, intrinsics: Option<[f32; 4]>) -> Result<Record, WireError> {
        let h = wire::encode(&frame, id, depth_units_um, intrinsics)?;
        let header = wire::decode(&h)?;
        Ok(Record { header, frame })
    }

    /// The record's pinhole intrinsics (`None` when its header carries none: the kernel's records today).
    /// The wire's `fx, fy, ppx, ppy` is the coefficients table's `rect_params` order.
    pub fn intrinsics(&self) -> Option<Intrinsics> {
        self.header.intrinsics.map(|k| Intrinsics::from_rect_params(self.frame.width, self.frame.height, k))
    }

    /// The depth scale the record carries (`None` for a non-depth record or units 0).
    pub fn depth_scale(&self) -> Option<DepthScale> {
        self.header.depth_scale()
    }

    /// A depth record: the depth stream in Z16.
    pub fn is_depth(&self) -> bool {
        self.frame.stream == StreamKind::Depth && self.frame.format == PixelFormat::Z16
    }

    /// The probe id.
    pub fn id(&self) -> String {
        String::from(self.header.id_str())
    }
}

/// A recording: its records in file order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Recording {
    pub records: Vec<Record>,
}

impl Recording {
    /// The depth (Z16) records, in order.
    pub fn depth(&self) -> impl Iterator<Item = &Record> {
        self.records.iter().filter(|r| r.is_depth())
    }
}

/// Why a recording was refused. `index` is the record's position (0-based), `offset` its first byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordingError {
    /// No bytes: not a recording.
    Empty,
    /// The record's header was refused by [`wire::decode`].
    Header { index: usize, offset: usize, error: WireError },
    /// The bytes end inside the record: `need` bytes from `offset`, `have` remain (a tail shorter than one header
    /// is this too, with `need` = [`HEADER_LEN`]).
    Truncated { index: usize, offset: usize, need: u64, have: u64 },
    /// The header decoded but its frame was refused ([`Frame::new`]: an odd-width YUY2).
    Frame { index: usize, offset: usize, error: FrameError },
    /// [`write`]: the record's header could not be encoded ([`WireError::Id`]).
    Encode { index: usize, error: WireError },
}

/// Walk `b` record by record.
pub fn read(b: &[u8]) -> Result<Recording, RecordingError> {
    if b.is_empty() {
        return Err(RecordingError::Empty);
    }
    let mut records = Vec::new();
    let mut offset = 0usize;
    while offset < b.len() {
        let index = records.len();
        let rest = &b[offset..];
        if rest.len() < HEADER_LEN {
            return Err(RecordingError::Truncated { index, offset, need: HEADER_LEN as u64, have: rest.len() as u64 });
        }
        let header = wire::decode(rest).map_err(|error| RecordingError::Header { index, offset, error })?;
        let need = header.header_len as u64 + header.data_len as u64;
        if need > rest.len() as u64 {
            return Err(RecordingError::Truncated { index, offset, need, have: rest.len() as u64 });
        }
        let data = rest[header.header_len..need as usize].to_vec();
        let frame = header.frame(data).map_err(|error| RecordingError::Frame { index, offset, error })?;
        records.push(Record { header, frame });
        offset += need as usize;
    }
    Ok(Recording { records })
}

/// The bytes of `records`: per record, [`wire::encode`] of its frame with its header's id, units and intrinsics,
/// then the frame's data (the header length written is [`HEADER_LEN`]; data length is the frame's byte count).
pub fn write(records: &[Record]) -> Result<Vec<u8>, RecordingError> {
    let mut out = Vec::new();
    for (index, r) in records.iter().enumerate() {
        let h = wire::encode(&r.frame, r.header.id_str(), r.header.depth_units_um, r.header.intrinsics)
            .map_err(|error| RecordingError::Encode { index, error })?;
        out.extend_from_slice(&h);
        out.extend_from_slice(&r.frame.data);
    }
    Ok(out)
}
