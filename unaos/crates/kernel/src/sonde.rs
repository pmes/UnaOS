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
//! 2. THE DEVICE — what this boot's EHCI walk, or its xHCI `probe_iso` (ARMSONDE, SR101), recorded for a D400 (`uvc::multi::record`):
//!    `:: SONDE: dev=8086:0b3a streams=<depth,ir,color> fmt=Z16 w=<n> h=<n> depth_scale=<f> frames_2s=<n>
//!    -> … ::`. With no D400 on the bus (every QEMU run, every bench without the camera) it is
//!    `-> SKIP(no-device)`. With one, the census, the XU reads and the depth scale are printed and the
//!    verdict is `-> SKIP(no-iso-pipe)` with `frames_2s=0`, by name: the tree has no isochronous pipe
//!    (CAMERA1's `[uvc] commit=withheld reason=no-iso-pipe`; EHCI has no iTD path and the D435i's native
//!    bus is xHCI), so a frame count is the NEXT rung's to measure and this line does not pretend one.
//!
//! The depth scale on the device line is the D400 default (1000 µm per unit = 0.001 m,
//! librealsense `src/ds/d400/d400-device.cpp:912`) marked `(default)`: its true read is the HW-monitor
//! channel, a 1 KiB XU transfer the 256-byte EP0 buffer cannot carry (see `realsense_xu`).

use crate::drivers::realsense_xu as rs;
use crate::drivers::uvc::multi::{self, Role};

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

    let (ip, fp) = um_parts(rs::DEPTH_UNITS_DEFAULT_UM);
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

    // ── 3. the device this boot saw ──
    match multi::record() {
        None => serial_println!(
            ":: SONDE: dev=none streams=- fmt=- w=0 h=0 depth_scale=- frames_2s=0 -> SKIP(no-device) ::"
        ),
        Some(r) => {
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
            let fmt = r.depth_cc;
            let fmt = if fmt == [0; 4] { [b'-', b' ', b' ', b' '] } else { fmt };
            let v = |x: u32| -> i64 {
                match x {
                    multi::NOT_READ => -1,
                    multi::STALLED => -2,
                    n => n as i64,
                }
            };
            serial_println!(
                ":: SONDE: dev={:04x}:{:04x} addr={} streams={} fmt={} w={} h={} depth_scale={}.{:06}(default) laser_mw={} emitter={} preset={} xu_unit={} window_short={} frames_2s=0 -> SKIP(no-iso-pipe) ::",
                r.vid, r.pid, r.addr, core::str::from_utf8(&names[..k]).unwrap_or("?"),
                core::str::from_utf8(&fmt).unwrap_or("?").trim_end(), r.depth_w, r.depth_h, ip, fp,
                v(r.xu[0]), v(r.xu[1]), v(r.xu[2]), r.xu_unit, r.ran_short
            );
        }
    }
}
