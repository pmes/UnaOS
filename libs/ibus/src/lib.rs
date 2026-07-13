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

//! FlySky i-BUS servo frame codec — the rover's receiver-side wire format.
//!
//! ENDURO (ROADMAP §4) puts UnaOS between the FGr8B receiver and the actuators.
//! The receiver speaks **i-BUS** over a UART at 115200 baud: 32-byte servo
//! frames every ~7 ms. This crate decodes that byte stream into
//! [`Frame`]s of 14 channel values in microseconds, with checksum validation
//! and stream resync — the properties a safety loop needs before it is ever
//! near a 3S LiPo.
//!
//! # Design constraints
//!
//! - `#![no_std]`, **zero dependencies**, `#![forbid(unsafe_code)]`. The future
//!   kernel UART driver feeds bytes to the very same [`Parser`]; nothing here
//!   may pull in an allocator or the host.
//! - The [`Parser`] holds **at most one 32-byte window** — no unbounded
//!   buffering, no allocation. A corrupt, truncated, or misaligned stream
//!   **resyncs** (it scans forward for the `0x20 0x40` header) and never panics.
//! - Frames are decoded but **flagged** when any channel falls outside the
//!   sane servo band ([`CHAN_MIN_US`]..=[`CHAN_MAX_US`]); the drive core refuses
//!   an out-of-range frame. See [`Frame::in_range`].
//!
//! # The wire format (frozen by the KATs in `tests/kat_vectors.rs`)
//!
//! | bytes  | meaning                                                        |
//! |--------|----------------------------------------------------------------|
//! | 0      | frame length, always `0x20` (32)                               |
//! | 1      | command: `0x40` = servo channels (other commands are skipped)  |
//! | 2..30  | 14 channels, little-endian `u16`, µs (1000..2000 nominal)       |
//! | 30..32 | little-endian `u16` checksum = `0xFFFF − Σ bytes[0..30]`        |

#![no_std]
#![forbid(unsafe_code)]

/// Total i-BUS frame length in bytes.
pub const FRAME_LEN: usize = 32;
/// Number of servo channels carried in a servo frame.
pub const NUM_CHANNELS: usize = 14;
/// Frame-length header byte (byte 0). Also the resync anchor.
pub const HEADER_LEN: u8 = 0x20;
/// Command byte (byte 1) identifying a servo-channel frame.
pub const CMD_SERVO: u8 = 0x40;

/// Lower bound of the sane servo band, µs. Below this a frame is out-of-range.
pub const CHAN_MIN_US: u16 = 900;
/// Upper bound of the sane servo band, µs. Above this a frame is out-of-range.
pub const CHAN_MAX_US: u16 = 2100;

/// A decoded servo frame: 14 channel values in microseconds plus a validity
/// view.
///
/// A `Frame` is only ever produced from a **checksum-valid** servo frame. Its
/// [`in_range`](Frame::in_range) flag additionally reports whether every
/// channel lies within [`CHAN_MIN_US`]..=[`CHAN_MAX_US`]; the drive core refuses
/// a frame whose flag is `false` (it counts toward the deadman, never toward
/// actuation).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    channels: [u16; NUM_CHANNELS],
    in_range: bool,
}

impl Frame {
    /// The channel values in microseconds, index 0..[`NUM_CHANNELS`].
    #[inline]
    pub fn channels(&self) -> &[u16; NUM_CHANNELS] {
        &self.channels
    }

    /// Channel `i` in microseconds, or `None` if `i >= `[`NUM_CHANNELS`].
    #[inline]
    pub fn channel(&self, i: usize) -> Option<u16> {
        self.channels.get(i).copied()
    }

    /// `true` when every channel lies within the sane servo band. A `false`
    /// frame is still fully decoded, but the drive core must refuse it.
    #[inline]
    pub fn in_range(&self) -> bool {
        self.in_range
    }
}

/// Cheap running tallies a witness (or the future kernel service) can read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    /// Checksum-valid servo (`0x40`) frames decoded and returned.
    pub frames_ok: u32,
    /// Frames rejected on a checksum mismatch (each triggers a resync).
    pub frames_bad: u32,
    /// Checksum-valid **non-servo** frames (telemetry/sensor); skipped, counted.
    pub frames_skipped: u32,
}

/// Streaming i-BUS parser. Feed it bytes as they arrive from the UART; it emits
/// a [`Frame`] each time a complete, checksum-valid servo frame lands.
///
/// The parser holds a single fixed 32-byte window (`buf`); `len` is how much of
/// the current candidate frame has accumulated. Byte 0 of the window is always
/// the [`HEADER_LEN`] anchor, so a desynchronised stream cannot grow the buffer
/// past one frame — on any failure the window is compacted to the next `0x20`.
#[derive(Clone, Debug)]
pub struct Parser {
    buf: [u8; FRAME_LEN],
    len: usize,
    counters: Counters,
}

impl Default for Parser {
    fn default() -> Self {
        Self::new()
    }
}

impl Parser {
    /// A fresh parser awaiting the first header byte.
    #[inline]
    pub const fn new() -> Self {
        Self {
            buf: [0u8; FRAME_LEN],
            len: 0,
            counters: Counters {
                frames_ok: 0,
                frames_bad: 0,
                frames_skipped: 0,
            },
        }
    }

    /// Running tallies (ok / bad / skipped).
    #[inline]
    pub fn counters(&self) -> Counters {
        self.counters
    }

    /// Feed one byte. Returns `Some(frame)` when this byte completes a
    /// checksum-valid **servo** frame; `None` otherwise (mid-frame, resync,
    /// a rejected frame, or a skipped non-servo frame).
    ///
    /// Never panics and never allocates; a bad checksum discards the window and
    /// resyncs to the next `0x20`.
    pub fn push(&mut self, byte: u8) -> Option<Frame> {
        // Byte 0 of a window is the header anchor: until we see `0x20`, every
        // byte is stream noise and is dropped. This is what bounds the buffer.
        if self.len == 0 {
            if byte != HEADER_LEN {
                return None;
            }
            self.buf[0] = byte;
            self.len = 1;
            return None;
        }

        // Accumulate into the current window (byte 1 = command, 2..30 = data,
        // 30..32 = checksum). We do not reject on the command byte here: a
        // checksum-valid non-servo command is legal and merely skipped later.
        self.buf[self.len] = byte;
        self.len += 1;

        if self.len == FRAME_LEN {
            return self.complete();
        }
        None
    }

    /// Feed a slice, invoking `on_frame` for each servo frame that lands.
    /// Convenience over [`push`](Parser::push) for buffered input and tests.
    pub fn feed<F: FnMut(Frame)>(&mut self, bytes: &[u8], mut on_frame: F) {
        for &b in bytes {
            if let Some(f) = self.push(b) {
                on_frame(f);
            }
        }
    }

    /// The window is full (32 bytes, `buf[0] == HEADER_LEN`). Validate, classify,
    /// and consume — or reject and resync.
    fn complete(&mut self) -> Option<Frame> {
        debug_assert_eq!(self.len, FRAME_LEN);
        if !self.checksum_ok() {
            self.counters.frames_bad = self.counters.frames_bad.wrapping_add(1);
            self.resync();
            return None;
        }

        let cmd = self.buf[1];
        if cmd != CMD_SERVO {
            // Valid frame, not a servo command (telemetry/sensor): skip + count.
            self.counters.frames_skipped = self.counters.frames_skipped.wrapping_add(1);
            self.len = 0;
            return None;
        }

        let frame = self.decode();
        self.counters.frames_ok = self.counters.frames_ok.wrapping_add(1);
        self.len = 0;
        Some(frame)
    }

    /// i-BUS checksum: `0xFFFF − Σ bytes[0..30]` (each byte summed as a `u16`),
    /// compared against the little-endian `u16` in `bytes[30..32]`.
    fn checksum_ok(&self) -> bool {
        let mut sum: u16 = 0;
        for &b in &self.buf[..FRAME_LEN - 2] {
            sum = sum.wrapping_add(b as u16);
        }
        let expected = 0xFFFFu16.wrapping_sub(sum);
        let actual = u16::from_le_bytes([self.buf[FRAME_LEN - 2], self.buf[FRAME_LEN - 1]]);
        expected == actual
    }

    /// Decode the 14 little-endian `u16` channels and compute the range flag.
    fn decode(&self) -> Frame {
        let mut channels = [0u16; NUM_CHANNELS];
        let mut in_range = true;
        for (i, ch) in channels.iter_mut().enumerate() {
            let off = 2 + i * 2;
            let v = u16::from_le_bytes([self.buf[off], self.buf[off + 1]]);
            *ch = v;
            if !(CHAN_MIN_US..=CHAN_MAX_US).contains(&v) {
                in_range = false;
            }
        }
        Frame { channels, in_range }
    }

    /// Resync after a rejected window: drop the stale leading `0x20` and compact
    /// the buffer to the next `0x20` candidate (if any). No byte after that
    /// candidate is lost — the remainder of the real frame arrives on later
    /// pushes, so the parser recovers within one frame of the true header.
    fn resync(&mut self) {
        let mut i = 1;
        while i < self.len && self.buf[i] != HEADER_LEN {
            i += 1;
        }
        if i >= self.len {
            self.len = 0;
        } else {
            let n = self.len - i;
            self.buf.copy_within(i..self.len, 0);
            self.len = n;
        }
    }
}

/// Build a checksum-valid servo frame from 14 channel values (µs). Shared by the
/// KATs and available to the future transmit/loopback path so the wire format
/// lives in exactly one place.
///
/// This does **not** clamp: pass out-of-band values to exercise the range flag.
pub fn encode_servo_frame(channels: &[u16; NUM_CHANNELS]) -> [u8; FRAME_LEN] {
    let mut buf = [0u8; FRAME_LEN];
    buf[0] = HEADER_LEN;
    buf[1] = CMD_SERVO;
    for (i, &c) in channels.iter().enumerate() {
        let off = 2 + i * 2;
        let le = c.to_le_bytes();
        buf[off] = le[0];
        buf[off + 1] = le[1];
    }
    let mut sum: u16 = 0;
    for &b in &buf[..FRAME_LEN - 2] {
        sum = sum.wrapping_add(b as u16);
    }
    let checksum = 0xFFFFu16.wrapping_sub(sum);
    let le = checksum.to_le_bytes();
    buf[FRAME_LEN - 2] = le[0];
    buf[FRAME_LEN - 1] = le[1];
    buf
}
