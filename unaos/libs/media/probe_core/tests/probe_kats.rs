// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// REALSENSE (LEDGER SR92): probe_core's known answers over fixture frames — every number below is
// derivable by hand from the fixture's construction, and each test is written so a wrong offset, a
// swapped axis or a dropped hole check changes the answer.

use probe_core::depth::{self, DepthScale, Intrinsics};
use probe_core::frame::{Frame, FrameError, PixelFormat, StreamKind};
use probe_core::imu::{self, ImuKind};
use probe_core::ramp;
use probe_core::replay;

#[test]
fn fourcc_maps_the_d400_formats() {
    assert_eq!(PixelFormat::from_fourcc(*b"Z16 "), Some(PixelFormat::Z16));
    assert_eq!(PixelFormat::from_fourcc(*b"GREY"), Some(PixelFormat::Y8));
    assert_eq!(PixelFormat::from_fourcc(*b"Y8  "), Some(PixelFormat::Y8));
    assert_eq!(PixelFormat::from_fourcc(*b"Y16 "), Some(PixelFormat::Y16));
    assert_eq!(PixelFormat::from_fourcc(*b"YUY2"), Some(PixelFormat::Yuy2));
    assert_eq!(PixelFormat::from_fourcc(*b"MJPG"), None);
    for f in [PixelFormat::Z16, PixelFormat::Y8, PixelFormat::Y16, PixelFormat::Yuy2] {
        assert_eq!(PixelFormat::from_name(f.name()), Some(f));
    }
    assert_eq!(StreamKind::for_format(PixelFormat::Z16), StreamKind::Depth);
    assert_eq!(StreamKind::for_format(PixelFormat::Y8), StreamKind::Infrared);
    assert_eq!(StreamKind::for_format(PixelFormat::Yuy2), StreamKind::Color);
    for s in [StreamKind::Depth, StreamKind::Infrared, StreamKind::Color, StreamKind::Accel, StreamKind::Gyro] {
        assert_eq!(StreamKind::from_name(s.name()), Some(s));
    }
}

#[test]
fn frame_refuses_what_its_bytes_cannot_hold() {
    let z = |w, h, stride, n| Frame::new(StreamKind::Depth, PixelFormat::Z16, w, h, stride, 0, 0, vec![0; n]);
    assert_eq!(z(0, 4, 0, 0), Err(FrameError::Empty));
    assert_eq!(z(4, 2, 7, 64), Err(FrameError::StrideTooShort { stride: 7, row: 8 }));
    // stride 10, two rows: the last row needs only its 8 pixel bytes — 18, not 20.
    assert_eq!(z(4, 2, 10, 17), Err(FrameError::Short { have: 17, need: 18 }));
    assert!(z(4, 2, 10, 18).is_ok());
    let odd = Frame::packed(StreamKind::Color, PixelFormat::Yuy2, 3, 1, 0, 0, vec![0; 8]);
    assert_eq!(odd, Err(FrameError::OddWidth));
}

#[test]
fn z16_is_little_endian_and_stride_aware() {
    // 2x2, stride 6 (2 pad bytes per row): row 0 = 0x0201, 0x0403; row 1 = 0x0605, 0x0807.
    let data = vec![0x01, 0x02, 0x03, 0x04, 0xEE, 0xEE, 0x05, 0x06, 0x07, 0x08];
    let f = Frame::new(StreamKind::Depth, PixelFormat::Z16, 2, 2, 6, 0, 0, data).unwrap();
    assert_eq!(f.u16_at(0, 0), Some(0x0201));
    assert_eq!(f.u16_at(1, 0), Some(0x0403));
    assert_eq!(f.u16_at(0, 1), Some(0x0605));
    assert_eq!(f.u16_at(1, 1), Some(0x0807));
    assert_eq!(f.u16_at(2, 0), None);
    assert_eq!(f.luma_at(0, 0), None);
}

#[test]
fn depth_scale_is_micrometres() {
    let s = DepthScale::from_micrometres(1000);
    assert!((s.metres_per_unit - 0.001).abs() < 1e-9);
    assert_eq!(s.micrometres(), 1000);
    assert_eq!(DepthScale::from_micrometres(100).micrometres(), 100);
    assert_eq!(DepthScale::D400_DEFAULT.micrometres(), 1000);
    assert_eq!(s.metres(0), None, "raw 0 is a hole, not zero metres");
    assert!((s.metres(1500).unwrap() - 1.5).abs() < 1e-6);
}

#[test]
fn deprojection_is_the_pinhole_model() {
    let k = Intrinsics::from_rect_params(848, 480, [420.0, 421.0, 424.5, 239.5]);
    assert_eq!((k.fx, k.fy, k.ppx, k.ppy), (420.0, 421.0, 424.5, 239.5));
    // the principal point is on the axis
    assert_eq!(k.deproject(424.5, 239.5, 2.0), Some([0.0, 0.0, 2.0]));
    // 42 px right of it at 1 m is 0.1 m right; 42.1 px down is 0.1 m down
    let p = k.deproject(466.5, 281.6, 1.0).unwrap();
    assert!((p[0] - 0.1).abs() < 1e-6 && (p[1] - 0.1).abs() < 1e-6 && p[2] == 1.0, "{p:?}");
    // project is its inverse
    let px = k.project(p).unwrap();
    assert!((px[0] - 466.5).abs() < 1e-3 && (px[1] - 281.6).abs() < 1e-3, "{px:?}");
    assert_eq!(k.project([0.0, 0.0, 0.0]), None);
    let zero = Intrinsics::from_rect_params(1, 1, [0.0, 0.0, 0.0, 0.0]);
    assert_eq!(zero.deproject(0.0, 0.0, 1.0), None);
    let ideal = Intrinsics::ideal(640, 480);
    assert_eq!((ideal.fx, ideal.fy, ideal.ppx, ideal.ppy), (960.0, 960.0, 320.0, 240.0));
}

#[test]
fn the_replay_plane_deprojects_planar() {
    let (w, h) = (64, 48);
    let f = replay::depth_plane(w, h, 500, 10, 0);
    assert_eq!((f.width, f.height, f.format, f.stream), (w, h, PixelFormat::Z16, StreamKind::Depth));
    assert_eq!(f.u16_at(0, 0), Some(500));
    assert_eq!(f.u16_at(63, 0), Some(500 + 630));
    assert_eq!(f.u16_at(32, 24), Some(0), "the centre is the hole");
    let (valid, holes, lo, hi) = depth::census(&f);
    assert_eq!(holes, 32 * 24, "the hole is the centre quarter");
    assert_eq!(valid, 64 * 48 - 32 * 24);
    assert_eq!((lo, hi), (500, 1130));

    let k = Intrinsics::ideal(w, h);
    let cloud = depth::point_cloud(&f, DepthScale::D400_DEFAULT, &k, 1);
    assert_eq!(cloud.len(), valid as usize);
    // Every point lies on z = 0.5 + 0.01 * column; recover the column from x and check z against it.
    for p in &cloud {
        let col = p[0] / p[2] * k.fx + k.ppx - 0.5;
        let want = 0.5 + 0.01 * col.round();
        assert!((p[2] - want).abs() < 1e-4, "point {p:?} off the plane (col {col})");
    }
    // A stride of 4 keeps every 4th pixel each way.
    let sparse = depth::point_cloud(&f, DepthScale::D400_DEFAULT, &k, 4);
    assert_eq!(sparse.len(), (16 * 12) - (8 * 6));
    // the hole moves with the frame number
    let g = replay::depth_plane(w, h, 500, 10, 3);
    assert_eq!(g.u16_at(16, 24), Some(500 + 160), "shifted by 3: column 16 is outside the hole now");
    assert_eq!(g.u16_at(48, 24), Some(0));
    assert_eq!(g.timestamp_us, 3 * 33_333);
}

#[test]
fn the_ramp_runs_blue_to_red_and_holes_are_black() {
    assert_eq!(ramp::ramp_rgb(0), [0, 0, 255]);
    assert_eq!(ramp::ramp_rgb(256), [0, 255, 255]);
    assert_eq!(ramp::ramp_rgb(512), [0, 255, 0]);
    assert_eq!(ramp::ramp_rgb(768), [255, 255, 0]);
    assert_eq!(ramp::ramp_rgb(1023), [255, 0, 0]);
    // [0, 1000, 3000] raw at 1 mm: hole, mid-range of 0..2 m, past far.
    let data: Vec<u8> = [0u16, 1000, 3000].iter().flat_map(|v| v.to_le_bytes()).collect();
    let f = Frame::packed(StreamKind::Depth, PixelFormat::Z16, 3, 1, 0, 0, data).unwrap();
    let rgba = ramp::depth_to_rgba(&f, DepthScale::D400_DEFAULT, 0.0, 2.0);
    assert_eq!(&rgba[0..4], &[0, 0, 0, 255]);
    assert_eq!(&rgba[4..8], &[ramp::ramp_rgb(511)[0], ramp::ramp_rgb(511)[1], ramp::ramp_rgb(511)[2], 255]);
    assert_eq!(&rgba[8..12], &[255, 0, 0, 255]);
    // an inverted range is black, not a panic
    assert_eq!(&ramp::depth_to_rgba(&f, DepthScale::D400_DEFAULT, 2.0, 1.0)[4..8], &[0, 0, 0, 255]);
}

#[test]
fn yuy2_is_bt601_studio_range() {
    assert_eq!(ramp::yuv_to_rgb(16, 128, 128), [0, 0, 0]);
    assert_eq!(ramp::yuv_to_rgb(235, 128, 128), [255, 255, 255]);
    // BT.601 red (Y 81, Cb 90, Cr 240) lands at (255, 0, 0) within a code value.
    let r = ramp::yuv_to_rgb(81, 90, 240);
    assert!(r[0] >= 254 && r[1] <= 1 && r[2] <= 1, "{r:?}");
    let f = replay::color_bars(16, 2, 0);
    let rgba = ramp::yuy2_to_rgba(&f);
    assert_eq!(rgba.len(), 16 * 2 * 4);
    // bar 7 (black) is the last two pixels of a 16-wide frame at seq 0
    assert_eq!(&rgba[14 * 4..15 * 4], &[0, 0, 0, 255]);
    // and rotates to the first bar one frame later
    let g = replay::color_bars(16, 2, 1);
    assert_eq!(&ramp::yuy2_to_rgba(&g)[14 * 4..15 * 4], &rgba[0..4]);
}

#[test]
fn blit_scales_nearest_and_clips() {
    let src = vec![1, 1, 1, 255, 2, 2, 2, 255]; // 2x1
    let mut dst = vec![0u8; 5 * 2 * 4];
    ramp::blit_scaled(&mut dst, 5, 2, 1, 0, 4, 2, &src, 2, 1);
    let px = |d: &[u8], x: usize, y: usize| d[(y * 5 + x) * 4];
    assert_eq!([px(&dst, 0, 0), px(&dst, 1, 0), px(&dst, 2, 0), px(&dst, 3, 0), px(&dst, 4, 0)], [0, 1, 1, 2, 2]);
    assert_eq!([px(&dst, 1, 1), px(&dst, 4, 1)], [1, 2]);
    // past the right edge is clipped, never written out of bounds
    ramp::blit_scaled(&mut dst, 5, 2, 4, 0, 4, 2, &src, 2, 1);
    assert_eq!(px(&dst, 4, 0), 1);
}

#[test]
fn imu_reports_read_in_si_units() {
    let mut r = [0u8; 32];
    r[0] = imu::REPORT_ID_ACCEL;
    r[2..10].copy_from_slice(&0x0102_0304_0506_0708u64.to_le_bytes());
    r[10..12].copy_from_slice(&1000i16.to_le_bytes()); // 1 g
    r[12..14].copy_from_slice(&(-500i16).to_le_bytes());
    r[14..16].copy_from_slice(&0i16.to_le_bytes());
    let s = imu::parse_report(&r, imu::GYRO_DPS_PER_LSB_OLD).unwrap();
    assert_eq!(s.kind, ImuKind::Accel);
    assert_eq!(s.timestamp, 0x0102_0304_0506_0708);
    assert!((s.xyz[0] - 9.80665).abs() < 1e-4 && (s.xyz[1] + 4.903325).abs() < 1e-4 && s.xyz[2] == 0.0);

    r[0] = imu::REPORT_ID_GYRO;
    r[10..12].copy_from_slice(&900i16.to_le_bytes()); // 90 deg/s at 0.1 deg/s/LSB
    let g = imu::parse_report(&r, imu::GYRO_DPS_PER_LSB_OLD).unwrap();
    assert_eq!(g.kind, ImuKind::Gyro);
    assert!((g.xyz[0] - core::f32::consts::FRAC_PI_2).abs() < 1e-5, "{:?}", g.xyz);

    r[0] = 3;
    assert_eq!(imu::parse_report(&r, imu::GYRO_DPS_PER_LSB_OLD), None, "report id 3 is not a motion sensor");
    assert_eq!(imu::parse_report(&r[..31], imu::GYRO_DPS_PER_LSB_OLD), None);
}
