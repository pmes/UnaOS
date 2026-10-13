//! CHARTER: Kernel — driver (the probe is Sonde's, R112; its CODEX row is Amendment IV, pending Peter's signature)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! REALSENSE (LEDGER SR92, R112) — `tests sonde`: the first probe, the Intel RealSense D435i.
//!
//! Two verdict lines, both `:: SONDE: … ::`:
//!
//! 1. THE FIXTURE — `drivers::uvc::multi::census` over a D435i-shaped configuration descriptor written
//!    byte by byte from the UVC 1.1 tables (IAD → VideoControl {header, the depth extension unit} → three
//!    VideoStreaming functions {Z16 848x480, GREY 848x480, YUY2 1280x720} → a HID interface), and
//!    `drivers::realsense_xu`'s builders (the HW-monitor `GET_ADV(etDepthTableControl)` command, the XU
//!    reply reader, the request words) against hand-laid bytes. Field by field; the first failure is
//!    named. Runs on every machine: `-> PASS` or `-> FAIL`.
//! 2. THE DEVICE — what this boot's EHCI walk, or its xHCI `probe_iso` (ARMSONDE, SR101), recorded for a D400
//!    (`uvc::multi::record`), and — SONDE2 (LEDGER SR105) — the DEPTH STREAM `probe_iso` armed on it. With no
//!    D400 on the bus (every QEMU run, every bench without the camera) it is
//!    `:: SONDE: dev=none … frames_2s=0 -> SKIP(no-device) ::`. With one, `probe_iso`'s two SONDE2 folds have
//!    committed the depth VideoStreaming function's `Z16 ` format on its isochronous endpoint (XHCIISO, B534),
//!    and this line counts that stream's frames over two seconds of wall clock, takes ONE complete frame,
//!    decodes it with `probe_core` (`Frame` → `depth::z16_stats`) and publishes it at `/dev/probe/depth0`
//!    (`probe_core::wire`'s 80-byte `PRB1` header + the Z16 bytes — the fields of SR92's
//!    `SMessage::ProbeFrame`):
//!    `:: SONDE: dev=8086:0b3a … fmt=Z16 w=<n> h=<n> … frames_2s=<n> fps=<f> … min_mm=<n> max_mm=<n>
//!    median_mm=<n> valid_pct=<f> depth0=<bytes> -> PASS ::` when `frames_2s > 0`; `-> FAIL reason=…` by name
//!    otherwise (no stream armed — the `[uvc] commit=` line says why —, a stream that is not this camera's
//!    depth, or no frames). The old `SKIP(no-iso-pipe)` is gone: the pipe exists.
//!
//! The depth scale is the D400 default (1000 µm per unit = 0.001 m, librealsense
//! `src/ds/d400/d400-device.cpp:912`) marked `(default)`: its true read is the HW-monitor channel, a 1 KiB XU
//! transfer (see `realsense_xu`), which the xHCI host's 4 KiB EP0 buffer can now carry and `after_census`
//! still withholds (owed). Every millimetre on the line is `raw * 1000 µm / 1000`.
//!
//! RING 3: `depth0_read` is the reader `SYS_PATH_READ` of `/dev/probe/depth0` calls; the one-line hook in
//! `selfdiag::path_fulfil` (and `sys_pathread` on a kernel without `selfdiag`) is outside this arc's files
//! and is named in the SR105 row as owed.

use crate::drivers::realsense_xu as rs;
use crate::drivers::uvc::depth_tap;
use crate::drivers::uvc::multi::{self, Role};
use alloc::vec::Vec;
use probe_core::depth::{self, Z16Stats};
use probe_core::{wire, Frame, PixelFormat, StreamKind};

/// SONDE2 (LEDGER SR105): the path ring 3 reads the published depth frame at (`SYS_PATH_READ`, any offset,
/// `una_abi::PATH_IO_MAX` per call): `probe_core::wire`'s header, then the frame's Z16 bytes.
pub const DEPTH0_PATH: &str = "/dev/probe/depth0";

/// The published record (`None` until a `tests sonde` with the camera streaming took a frame).
static DEPTH0: crate::sync::Mutex<Option<Vec<u8>>> = crate::sync::Mutex::new(None);

/// `SYS_PATH_READ` of [`DEPTH0_PATH`]: up to `cap` bytes from `off` (an empty read at or past the end), or
/// `-ENOENT` while nothing is published. The shape of `selfdiag::path_fulfil`'s return, so its hook is one
/// line: `if path == crate::sonde::DEPTH0_PATH { return crate::sonde::depth0_read(off, cap); }`.
pub fn depth0_read(off: u64, cap: usize) -> Result<(Vec<u8>, i64), i64> {
    let g = DEPTH0.lock();
    let Some(rec) = g.as_ref() else { return Err(una_abi::ENOENT) };
    if off >= rec.len() as u64 || cap == 0 {
        return Ok((Vec::new(), 0));
    }
    let o = off as usize;
    let n = (rec.len() - o).min(cap);
    let mut out = Vec::new();
    if out.try_reserve_exact(n).is_err() {
        return Err(una_abi::EAGAIN);
    }
    out.extend_from_slice(&rec[o..o + n]);
    Ok((out, n as i64))
}

/// The published record's length (0 = none).
pub fn depth0_len() -> usize {
    DEPTH0.lock().as_ref().map(|r| r.len()).unwrap_or(0)
}

pub fn ensure() {
    use core::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, Ordering::AcqRel) {
        crate::tests::register("sonde", selftest);
    }
}

/// A D435i-shaped configuration descriptor, 318 bytes, every value distinct where a mis-read offset
/// could alias it (the three functions' widths, heights, bpp and FourCCs all differ pairwise in at least
/// one field the census reads).
#[rustfmt::skip]
const FIXTURE: [u8; 318] = [
    // Configuration (USB 2.0 Table 9-10): wTotalLength=318 (0x013E) bNumInterfaces=5 bConfigurationValue=1
    9, 0x02, 0x3E, 0x01, 5, 1, 0, 0x80, 250,
    // IAD (USB 2.0 IAD ECN Table 9-Z): bFirstInterface=0 bInterfaceCount=4 class=0x0E sub=0x03
    8, 0x0B, 0, 4, 0x0E, 0x03, 0, 0,
    // VideoControl interface 0 (Table 9-12)
    9, 0x04, 0, 0, 0, 0x0E, 0x01, 0, 0,
    // VC_HEADER (UVC 1.1 Table 3-3), bInCollection=3 -> bLength 15: bcdUVC=0x0110 wTotalLength=43
    // dwClockFrequency=48 MHz (0x02DC6C00) baInterfaceNr=1,2,3
    15, 0x24, 0x01, 0x10, 0x01, 43, 0, 0x00, 0x6C, 0xDC, 0x02, 3, 1, 2, 3,
    // VC_EXTENSION_UNIT (UVC 1.1 Table 3-9), p=1 n=3 -> bLength 28: bUnitID=3 guidExtensionCode = the
    // D400 depth XU (realsense_xu::DEPTH_XU_GUID) bNumControls=18 bNrInPins=1 baSourceID=2 bControlSize=3
    // bmControls=0x03FFFF iExtension=0
    28, 0x24, 0x06, 3,
    0xCB, 0x6C, 0x60, 0xC9, 0x4C, 0x59, 0x25, 0x4D, 0xAF, 0x47, 0xCC, 0xC4, 0x96, 0x43, 0x59, 0x95,
    18, 1, 2, 3, 0xFF, 0xFF, 0x03, 0,
    // ── function 1: DEPTH ──
    9, 0x04, 1, 0, 0, 0x0E, 0x02, 0, 0,
    // VS_INPUT_HEADER (Table 3-13), bNumFormats=1 wTotalLength=71 bEndpointAddress=0x81 bTerminalLink=2
    14, 0x24, 0x01, 1, 71, 0, 0x81, 0, 2, 0, 0, 0, 1, 0,
    // VS_FORMAT_UNCOMPRESSED (Uncompressed payload Table 3-1): 'Z16 ' + the UVC GUID tail, bpp=16
    27, 0x24, 0x04, 1, 1,
    b'Z', b'1', b'6', b' ', 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71,
    16, 1, 0, 0, 0, 0,
    // VS_FRAME_UNCOMPRESSED (Table 3-2), 1 discrete interval: 848x480 (0x0350 x 0x01E0) 333333 (30 fps)
    30, 0x24, 0x05, 1, 0, 0x50, 0x03, 0xE0, 0x01,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0x15, 0x16, 0x05, 0x00, 1, 0x15, 0x16, 0x05, 0x00,
    // ── function 2: INFRARED ──
    9, 0x04, 2, 0, 0, 0x0E, 0x02, 0, 0,
    14, 0x24, 0x01, 1, 71, 0, 0x82, 0, 2, 0, 0, 0, 1, 0,
    // 'GREY', bpp=8 (librealsense d400-device.cpp:59: GREY -> Y8)
    27, 0x24, 0x04, 1, 1,
    b'G', b'R', b'E', b'Y', 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71,
    8, 1, 0, 0, 0, 0,
    30, 0x24, 0x05, 1, 0, 0x50, 0x03, 0xE0, 0x01,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0x15, 0x16, 0x05, 0x00, 1, 0x15, 0x16, 0x05, 0x00,
    // ── function 3: COLOUR ──
    9, 0x04, 3, 0, 0, 0x0E, 0x02, 0, 0,
    14, 0x24, 0x01, 1, 71, 0, 0x83, 0, 2, 0, 0, 0, 1, 0,
    27, 0x24, 0x04, 1, 1,
    b'Y', b'U', b'Y', b'2', 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71,
    16, 1, 0, 0, 0, 0,
    // 1280x720 (0x0500 x 0x02D0)
    30, 0x24, 0x05, 1, 0, 0x00, 0x05, 0xD0, 0x02,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0x15, 0x16, 0x05, 0x00, 1, 0x15, 0x16, 0x05, 0x00,
    // ── the IMU: HID interface 4 (class 0x03), outside the video IAD ──
    9, 0x04, 4, 0, 1, 0x03, 0, 0, 0,
];

/// `micrometres` as metres with six decimals, without floating point: 1000 -> "0.001000".
fn um_parts(um: u32) -> (u32, u32) {
    (um / 1_000_000, um % 1_000_000)
}

fn selftest() {
    let mut fails = 0u32;
    let mut first: (&'static str, u64, u64) = ("", 0, 0);
    let mut check = |name: &'static str, got: u64, want: u64| {
        if got != want {
            if fails == 0 {
                first = (name, got, want);
            }
            fails += 1;
        }
    };
    let cc = |c: [u8; 4]| u32::from_le_bytes(c) as u64;

    // ── 1. the census ──
    let s = multi::census(&FIXTURE);
    check("census.ran_short", s.ran_short as u64, 0);
    check("census.overflowed", s.overflowed as u64, 0);
    check("vc.present", s.have_vc as u64, 1);
    check("vc.intf", s.vc_intf as u64, 0);
    check("fns", s.nvs as u64, 3);
    let want: [(u8, u8, [u8; 4], u8, u16, u16, Role); 3] = [
        (1, 0x81, *b"Z16 ", 16, 848, 480, Role::Depth),
        (2, 0x82, *b"GREY", 8, 848, 480, Role::Ir),
        (3, 0x83, *b"YUY2", 16, 1280, 720, Role::Color),
    ];
    for (i, w) in want.iter().enumerate() {
        if i >= s.nvs {
            break;
        }
        let f = &s.vs[i];
        let n: [&'static str; 8] = match i {
            0 => ["fn0.intf", "fn0.ep", "fn0.fourcc", "fn0.bpp", "fn0.w", "fn0.h", "fn0.role", "fn0.interval"],
            1 => ["fn1.intf", "fn1.ep", "fn1.fourcc", "fn1.bpp", "fn1.w", "fn1.h", "fn1.role", "fn1.interval"],
            _ => ["fn2.intf", "fn2.ep", "fn2.fourcc", "fn2.bpp", "fn2.w", "fn2.h", "fn2.role", "fn2.interval"],
        };
        check(n[0], f.intf as u64, w.0 as u64);
        check(n[1], f.ep as u64, w.1 as u64);
        check(n[2], cc(f.fourcc), cc(w.2));
        check(n[3], f.bpp as u64, w.3 as u64);
        check(n[4], f.width as u64, w.4 as u64);
        check(n[5], f.height as u64, w.5 as u64);
        check(n[6], (f.role() == w.6) as u64, 1);
        check(n[7], f.default_interval as u64, 333_333);
    }
    check("xus", s.nxu as u64, 1);
    check("xu.id", s.xus[0].id as u64, rs::DEPTH_XU_UNIT as u64);
    check("xu.controls", s.xus[0].ncontrols as u64, 18);
    check("xu.d400_depth", s.depth_xu().is_some() as u64, 1);
    check("hid.intf", s.hid_intf.map(|i| i as u64).unwrap_or(99), 4);
    let mut lbl = [0u8; 32];
    let ln = s.label(&mut lbl);
    check("streams.label", (&lbl[..ln] == b"depth,ir,color") as u64, 1);
    // A window cut mid-descriptor must say so (the 256-byte EP0 buffer meets a real D435i this way).
    check("census.short_window", multi::census(&FIXTURE[..256]).ran_short as u64, 1);

    // ── 2. the XU and HW-monitor builders ──
    check("xu.wvalue.laser", rs::xu_wvalue(rs::CTRL_LASER_POWER) as u64, 0x0400);
    check("xu.windex", rs::xu_windex(rs::DEPTH_XU_UNIT, 0) as u64, 0x0300);
    check("d400.d435i", rs::is_d400(0x8086, 0x0B3A) as u64, 1);
    check("d400.facetime", rs::is_d400(0x05AC, 0x8510) as u64, 0);
    let mut cmd = [0xEEu8; 32];
    let n = rs::hwmon_build_depth_units(&mut cmd);
    // hw-monitor.cpp:28-59: len 20, magic 0xCDAB, opcode 0x2C, p1 = 9 (etDepthTableControl), p2 = 0.
    #[rustfmt::skip]
    const CMD: [u8; 24] = [20, 0, 0xAB, 0xCD, 0x2C, 0, 0, 0, 9, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    check("hwmon.len", n as u64, 24);
    check("hwmon.bytes", (cmd[..24] == CMD) as u64, 1);
    check("hwmon.tail_zeroed", (cmd[24..] == [0u8; 8]) as u64, 1);
    // A 1024-byte reply: opcode echo, depth_units = 100 um (0.0001 m — distinct from the default), then
    // the four other table fields, size 20 at byte 1020.
    let mut reply = [0u8; rs::HWMON_BUF];
    reply[0] = rs::OP_GET_ADV;
    reply[4..8].copy_from_slice(&100u32.to_le_bytes());
    reply[8..12].copy_from_slice(&7u32.to_le_bytes());
    reply[rs::HWMON_DATA_SIZE_OFFSET..].copy_from_slice(&20u32.to_le_bytes());
    check("hwmon.depth_units", rs::hwmon_reply_depth_units(&reply).unwrap_or(0) as u64, 100);
    let mut bad = reply;
    bad[0] = 0xF8; // an error code in place of the echo
    check("hwmon.error_echo_refused", rs::hwmon_reply_depth_units(&bad).is_none() as u64, 1);
    let mut short = reply;
    short[rs::HWMON_DATA_SIZE_OFFSET] = 16; // fewer bytes than a depth table
    check("hwmon.short_table_refused", rs::hwmon_reply_depth_units(&short).is_none() as u64, 1);

    // ── 3. SONDE2 (SR105): the shared core the device path decodes with ──
    check("core.role.z16", (multi::role_of(*b"Z16 ") == Role::Depth) as u64, 1);
    check("core.role.y8i", (multi::role_of(*b"Y8I ") == Role::Ir) as u64, 1);
    check("core.role.uyvy", (multi::role_of(*b"UYVY") == Role::Color) as u64, 1);
    check("core.role.mjpg", (multi::role_of(*b"MJPG") == Role::Unknown) as u64, 1);
    // 4x2 Z16: hole, 300, 1000, 260 / hole, 261, 2000, 255 -> valid 6, min 255, max 2000, lower median 261
    // (rank (6-1)/2 = 2 of 255 260 261 300 1000 2000), valid 75.0 %.
    let px: [u16; 8] = [0, 300, 1000, 260, 0, 261, 2000, 255];
    let mut zb = Vec::with_capacity(16);
    for v in px {
        zb.extend_from_slice(&v.to_le_bytes());
    }
    match Frame::packed(StreamKind::Depth, PixelFormat::Z16, 4, 2, 0, 7, zb) {
        Ok(zf) => {
            let st = depth::z16_stats(&zf).unwrap_or_default();
            check("core.z16.valid", st.valid as u64, 6);
            check("core.z16.min", st.min_raw as u64, 255);
            check("core.z16.max", st.max_raw as u64, 2000);
            check("core.z16.median", st.median_raw as u64, 261);
            check("core.z16.permille", st.valid_permille() as u64, 750);
            match wire::encode(&zf, "uvc:8086:0b3a", rs::DEPTH_UNITS_DEFAULT_UM, None) {
                Ok(h) => match wire::decode(&h) {
                    Ok(d) => {
                        check("core.wire.wh", ((d.width as u64) << 16) | d.height as u64, (4 << 16) | 2);
                        check("core.wire.stride", d.stride as u64, 8);
                        check("core.wire.seq", d.seq, 7);
                        check("core.wire.data_len", d.data_len as u64, 16);
                        check("core.wire.um", d.depth_units_um as u64, rs::DEPTH_UNITS_DEFAULT_UM as u64);
                        check("core.wire.id", (d.id_str() == "uvc:8086:0b3a") as u64, 1);
                    }
                    Err(_) => check("core.wire.decode", 0, 1),
                },
                Err(_) => check("core.wire.encode", 0, 1),
            }
        }
        Err(_) => check("core.z16.frame", 0, 1),
    }

    if fails == 0 {
        serial_println!(
            ":: SONDE: fixture={}B fns={} streams={} fmt=Z16 w={} h={} xu={} hid={} hwmon_cmd=ok depth_units_reply=100 -> PASS ::",
            FIXTURE.len(), s.nvs, core::str::from_utf8(&lbl[..ln]).unwrap_or("?"), s.vs[0].width, s.vs[0].height,
            s.xus[0].id, s.hid_intf.unwrap_or(0)
        );
    } else {
        serial_println!(
            ":: SONDE: fixture={}B failures={} first={} got={} want={} -> FAIL ::",
            FIXTURE.len(), fails, first.0, first.1, first.2
        );
    }

    // ── 4. the device this boot saw, and its depth stream ──
    let Some(r) = multi::record() else {
        serial_println!(":: SONDE: dev=none streams=- fmt=- w=0 h=0 depth_scale=- frames_2s=0 -> SKIP(no-device) ::");
        return;
    };
    let mut names = [0u8; 32];
    let mut k = 0usize;
    for (bit, name) in [(1u32, &b"depth"[..]), (2, &b"ir"[..]), (4, &b"color"[..])] {
        if r.roles & bit != 0 {
            if k != 0 {
                names[k] = b',';
                k += 1;
            }
            names[k..k + name.len()].copy_from_slice(name);
            k += name.len();
        }
    }
    if k == 0 {
        names[0] = b'-';
        k = 1;
    }
    let v = |x: u32| -> i64 {
        match x {
            multi::NOT_READ => -1,
            multi::STALLED => -2,
            n => n as i64,
        }
    };
    let streams = core::str::from_utf8(&names[..k]).unwrap_or("?");
    let um = rs::DEPTH_UNITS_DEFAULT_UM;
    let (ip, fp) = um_parts(um);

    // The stream `probe_iso` armed must be THIS camera's depth (`Z16 `) — anything else is named, not counted.
    let (w, h) = match depth_tap::armed() {
        Some((addr, w, h, cc)) if addr == r.addr && cc == *b"Z16 " => (w, h),
        armed => {
            let (why, a, cc) = match armed {
                None => ("no-armed-stream(see-[uvc]-commit=)", 0u8, *b"-   "),
                Some((a, _, _, cc)) if a != r.addr => ("armed-stream-is-another-device", a, cc),
                Some((a, _, _, cc)) => ("armed-stream-is-not-z16", a, cc),
            };
            serial_println!(
                ":: SONDE: dev={:04x}:{:04x} addr={} streams={} fmt=Z16 w={} h={} depth_scale={}.{:06}(default) laser_mw={} emitter={} preset={} xu_unit={} window_short={} stream_addr={} stream_fourcc={} frames_2s=0 -> FAIL reason={} ::",
                r.vid, r.pid, r.addr, streams, r.depth_w, r.depth_h, ip, fp, v(r.xu[0]), v(r.xu[1]), v(r.xu[2]), r.xu_unit,
                r.ran_short, a, core::str::from_utf8(&cc).unwrap_or("?").trim_end(), why
            );
            return;
        }
    };

    // Two seconds of wall clock — `tests uvc`'s window, verbatim in shape (uvc.rs `stream_fixture`).
    let c0 = depth_tap::counts();
    let t0 = crate::arch::now_cycles();
    let span = crate::arch::hw_wait_budget();
    while crate::arch::now_cycles().wrapping_sub(t0) < span {
        #[cfg(target_arch = "x86_64")]
        crate::arch::sched::sleep_ticks(10);
        // aarch64: the window drives the event ring itself (uvc.rs `stream_fixture`'s ARMSONDE note).
        #[cfg(not(target_arch = "x86_64"))]
        {
            if let Ok(mut x) = crate::drivers::xhci::claim() {
                x.poll_events();
            }
            crate::arch::sched::yield_now();
        }
        core::hint::spin_loop();
    }
    let dt = crate::arch::now_cycles().wrapping_sub(t0).max(1);
    let c1 = depth_tap::counts();
    let frames = c1.0.wrapping_sub(c0.0);
    #[cfg(target_arch = "x86_64")]
    let hz = crate::arch::apic::tsc_hz();
    #[cfg(target_arch = "aarch64")]
    let hz = crate::arch::timer::cntfrq();
    let fps10 = if hz != 0 { (frames as u64).saturating_mul(10).saturating_mul(hz) / dt } else { frames as u64 * 5 };

    // ONE frame: copied out of the front buffer, decoded by probe_core, published for ring 3.
    let row = w as usize * 2;
    let need = row * h as usize;
    let mut buf = Vec::new();
    let got = if need != 0 && buf.try_reserve_exact(need).is_ok() { depth_tap::copy_latest(&mut buf) } else { None };
    let ts_us = if hz != 0 { (crate::arch::now_cycles() as u128 * 1_000_000 / hz as u128) as u64 } else { 0 };
    let (stats, seq, depth0): (Option<Z16Stats>, u32, Result<usize, &'static str>) = match got {
        None => (None, 0, Err("no-complete-frame")),
        Some((seq, len)) if buf.len() != len => (None, seq, if len > need { Err("frame-larger-than-geometry") } else { Err("copy") }),
        Some((seq, _)) => match Frame::new(StreamKind::Depth, PixelFormat::Z16, w as u32, h as u32, row, ts_us, seq as u64, core::mem::take(&mut buf)) {
            Err(_) => (None, seq, Err("frame-short-of-geometry")),
            Ok(mut f) => {
                let st = depth::z16_stats(&f);
                let id = alloc::format!("uvc:{:04x}:{:04x}", r.vid, r.pid);
                let pub_ = match wire::encode(&f, &id, um, None) {
                    Err(_) => Err("wire-encode"),
                    Ok(hd) => {
                        let mut rec = Vec::new();
                        if rec.try_reserve_exact(hd.len() + f.data.len()).is_err() {
                            Err("heap")
                        } else {
                            rec.extend_from_slice(&hd);
                            rec.append(&mut f.data);
                            let n = rec.len();
                            *DEPTH0.lock() = Some(rec);
                            Ok(n)
                        }
                    }
                };
                (st, seq, pub_)
            }
        },
    };
    let mm = |raw: Option<u16>| -> i64 { raw.map(|x| Z16Stats::mm(x, um) as i64).unwrap_or(-1) };
    let pm = stats.map(|s| s.valid_permille()).unwrap_or(0);
    let pass = frames > 0;
    serial_println!(
        ":: SONDE: dev={:04x}:{:04x} addr={} streams={} fmt=Z16 w={} h={} depth_scale={}.{:06}(default) laser_mw={} emitter={} preset={} xu_unit={} window_short={} frames_2s={} fps={}.{} good={} short={} err={} over={} bad_hdr={} seq={} min_mm={} max_mm={} median_mm={} valid_pct={}.{} depth0={} -> {}{} ::",
        r.vid, r.pid, r.addr, streams, w, h, ip, fp, v(r.xu[0]), v(r.xu[1]), v(r.xu[2]), r.xu_unit, r.ran_short,
        frames, fps10 / 10, fps10 % 10, c1.1.wrapping_sub(c0.1), c1.2.wrapping_sub(c0.2), c1.3.wrapping_sub(c0.3),
        c1.4.wrapping_sub(c0.4), c1.6.wrapping_sub(c0.6), seq,
        mm(stats.map(|s| s.min_raw)), mm(stats.map(|s| s.max_raw)), mm(stats.map(|s| s.median_raw)), pm / 10, pm % 10,
        match depth0 { Ok(n) => n as i64, Err(_) => 0 },
        if pass { "PASS" } else { "FAIL" },
        if pass { "" } else { " reason=no-frames" }
    );
    if let Err(why) = depth0 {
        serial_println!("[sonde] depth0=withheld path={} reason={}", DEPTH0_PATH, why);
    } else {
        crate::census_println!("[sonde] depth0=published path={} bytes={} header={} seq={}", DEPTH0_PATH, depth0_len(), wire::HEADER_LEN, seq);
    }
}
