// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! The IMU sample model and the D400 HID motion report it is read from.
//!
//! FACTS (librealsense v2.55.1, Apache-2.0, R110's shape):
//! * The D435i's motion data arrives on its HID interface as 32-byte input reports
//!   (`src/hid/hid-types.h:21`, `SIZE_OF_HID_IMU_FRAME = 32`), packed: `reportId` u8 @0, one u8 @1,
//!   `timeStamp` u64 @2, `x` `y` `z` i16 @10/@12/@14, then vendor fields to byte 32
//!   (`hid-types.h:56-70`, `#pragma pack(push, 1)` at :45). Report id 1 is the accelerometer, 2 the
//!   gyroscope (`hid-types.h:17-18`).
//! * Accelerometer units: 1 LSB = 0.001 g, librealsense's g = 9.80665 m/s²
//!   (`src/proc/motion-transform.cpp:52-57`).
//! * Gyroscope units: 0.1 deg/s per LSB before firmware 5.16, 0.0001 from 5.16 on
//!   (`src/ds/d400/d400-motion.cpp:83-84`); converted to rad/s (`motion-transform.cpp:68`).
//!
//! All little-endian on the wire (the D400's USB byte order; the pack pragma leaves no padding).

/// Which sensor a sample is from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImuKind {
    /// m/s².
    Accel,
    /// rad/s.
    Gyro,
}

/// One IMU sample in SI units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImuSample {
    pub kind: ImuKind,
    /// The report's own timestamp, as the device sent it.
    pub timestamp: u64,
    pub xyz: [f32; 3],
}

/// The HID report's size (`hid-types.h:21`).
pub const HID_IMU_REPORT_LEN: usize = 32;
/// Report ids (`hid-types.h:17-18`).
pub const REPORT_ID_ACCEL: u8 = 1;
pub const REPORT_ID_GYRO: u8 = 2;
/// Standard gravity, m/s² (`motion-transform.cpp:56`).
pub const GRAVITY: f32 = 9.80665;
/// Accelerometer metres-per-second² per LSB: 0.001 g (`motion-transform.cpp:52,57`).
pub const ACCEL_MPS2_PER_LSB: f32 = 0.001 * GRAVITY;
/// Gyroscope deg/s per LSB, firmware < 5.16 and ≥ 5.16 (`d400-motion.cpp:84`).
pub const GYRO_DPS_PER_LSB_OLD: f32 = 0.1;
pub const GYRO_DPS_PER_LSB_516: f32 = 0.0001;

const DEG_TO_RAD: f32 = core::f32::consts::PI / 180.0;

/// The raw fields of a report: (report id, timestamp, x, y, z). `None` when shorter than 32 bytes.
pub fn parse_raw(report: &[u8]) -> Option<(u8, u64, [i16; 3])> {
    if report.len() < HID_IMU_REPORT_LEN {
        return None;
    }
    let mut ts = [0u8; 8];
    ts.copy_from_slice(&report[2..10]);
    let a = |o: usize| i16::from_le_bytes([report[o], report[o + 1]]);
    Some((report[0], u64::from_le_bytes(ts), [a(10), a(12), a(14)]))
}

/// A report → a sample in SI units. `gyro_dps_per_lsb` is the firmware's gyro scale (one of the two
/// constants above). `None` for a short report or a report id that is neither sensor.
pub fn parse_report(report: &[u8], gyro_dps_per_lsb: f32) -> Option<ImuSample> {
    let (id, timestamp, r) = parse_raw(report)?;
    let (kind, k) = match id {
        REPORT_ID_ACCEL => (ImuKind::Accel, ACCEL_MPS2_PER_LSB),
        REPORT_ID_GYRO => (ImuKind::Gyro, gyro_dps_per_lsb * DEG_TO_RAD),
        _ => return None,
    };
    Some(ImuSample { kind, timestamp, xyz: [r[0] as f32 * k, r[1] as f32 * k, r[2] as f32 * k] })
}
