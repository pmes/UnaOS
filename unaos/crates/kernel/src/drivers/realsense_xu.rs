//! CHARTER: Sonde — driver
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! REALSENSE (LEDGER SR92, R112) — the Intel RealSense D400's vendor extension unit, AS DATA.
//!
//! The D435i is a UVC device, and everything UVC does not define — the depth scale, the laser, the
//! presets, the firmware's command channel — goes through ONE vendor extension unit (UVC 1.1 §3.7.2.6,
//! Table 3-9) whose protocol is public in librealsense (Apache-2.0). Every value below is a HARDWARE FACT
//! taken from librealsense v2.55.1 with its file and line, R110's shape: the facts and their names enter
//! the tree, no code is copied and no function is transcribed. The builders and readers here are written
//! from the layouts those lines state.
//!
//! What this file is NOT: a transport. It owns no hardware and issues nothing; `uvc::multi` issues the
//! three short `GET_CUR` reads this rung makes, through the EHCI EP0 seam, and `tests sonde` scores the
//! builders against fixtures. The HW-monitor command channel (`hwmon_*`) is a 1024-byte `SET_CUR` +
//! `GET_CUR` pair on the XU (`src/platform/uvc-option.cpp:132-151`), and the EHCI EP0 data buffer is
//! 256 bytes (`qh::Buf256`) — so the channel is built and scored here and WITHHELD on the wire, by name,
//! until a transport with a 1 KiB control buffer carries it (the xHCI path, owed).

// ── Identity ────────────────────────────────────────────────────────────────────────────────────
/// Intel's USB-IF vendor id.
pub const VID_INTEL: u16 = 0x8086;
/// `src/ds/d400/d400-private.h:33` — `RS435I_PID = 0x0b3a; // D435i`.
pub const PID_D435I: u16 = 0x0B3A;
/// `src/ds/d400/d400-private.h:31` — `RS435_RGB_PID = 0x0b07; // AWGC` (the D435, same depth XU).
pub const PID_D435: u16 = 0x0B07;

/// The D400 products this rung recognises (the D435i, and the D435 that shares its depth XU).
pub fn is_d400(vid: u16, pid: u16) -> bool {
    vid == VID_INTEL && (pid == PID_D435I || pid == PID_D435)
}

// ── The depth extension unit ────────────────────────────────────────────────────────────────────
/// `src/ds/ds-private.h:66-68` — `depth_xu = { 0, 3, 2, { 0xC9606CCB, 0x594C, 0x4D25, { 0xaf, 0x47,
/// 0xcc, 0xc4, 0x96, 0x43, 0x59, 0x95 } } }`, annotated at :66 "subdevice[h] unit[fw], node[h]
/// guid[fw]": the FIRMWARE's facts are the unit id (3) and the GUID.
pub const DEPTH_XU_UNIT: u8 = 3;
/// The same GUID as a UVC descriptor stores `guidExtensionCode` (UVC 1.1 Table 3-9): the first three
/// fields little-endian, the last eight bytes in order.
pub const DEPTH_XU_GUID: [u8; 16] = [
    0xCB, 0x6C, 0x60, 0xC9, // 0xC9606CCB
    0x4C, 0x59, // 0x594C
    0x25, 0x4D, // 0x4D25
    0xAF, 0x47, 0xCC, 0xC4, 0x96, 0x43, 0x59, 0x95,
];

// XU control selectors — `src/ds/ds-private.h:49-62`.
/// :49 `DS5_HWMONITOR = 1` — the firmware command channel.
pub const CTRL_HWMONITOR: u8 = 1;
/// :50 `DS5_DEPTH_EMITTER_ENABLED = 2` — 1 byte: 0 off, 1 laser, 2 laser auto, 3 LED
/// (`src/ds/ds-options.cpp:36-38` names the values).
pub const CTRL_EMITTER: u8 = 2;
/// :51 `DS5_EXPOSURE = 3`.
pub const CTRL_EXPOSURE: u8 = 3;
/// :52 `DS5_LASER_POWER = 4` — 2 bytes, milliwatts (`src/ds/d400/d400-device.cpp:1295-1298`:
/// `uvc_xu_option<uint16_t>` "Manual laser power in mw").
pub const CTRL_LASER_POWER: u8 = 4;
/// :53 `DS5_HARDWARE_PRESET = 6` — 1 byte (`d400-device.cpp:650-653`, `uvc_xu_option<uint8_t>`
/// "Hardware pipe configuration"; librealsense registers it for the D416 only, so on a D435i a stall
/// is a legal answer and is printed as one).
pub const CTRL_HW_PRESET: u8 = 6;

/// One XU read this rung makes: the wire name, the selector, the length in bytes.
pub struct XuRead {
    pub name: &'static str,
    pub ctrl: u8,
    pub len: u16,
}

/// The three reads, in order. Every one is a `GET_CUR` — a probe that changes the device is not a probe.
pub const XU_READS: [XuRead; 3] = [
    XuRead { name: "laser_mw", ctrl: CTRL_LASER_POWER, len: 2 },
    XuRead { name: "emitter", ctrl: CTRL_EMITTER, len: 1 },
    XuRead { name: "preset", ctrl: CTRL_HW_PRESET, len: 1 },
];

/// UVC 1.1 §4.2.1 (unit control requests): `wValue` = control selector << 8.
pub const fn xu_wvalue(ctrl: u8) -> u16 {
    (ctrl as u16) << 8
}

/// UVC 1.1 §4.2.1: `wIndex` = entity id << 8 | VideoControl interface number.
pub const fn xu_windex(unit: u8, vc_intf: u8) -> u16 {
    ((unit as u16) << 8) | vc_intf as u16
}

// ── The firmware command channel (HW monitor) ───────────────────────────────────────────────────
// Opcodes — `src/ds/ds-private.h:83-133` (`enum fw_cmd : uint8_t`).
/// :93 `GVD = 0x10` — camera details (firmware version, serial).
pub const OP_GVD: u8 = 0x10;
/// :94 `GETINTCAL = 0x15` — read a calibration table (the coefficients table, id 25,
/// `src/ds/d400/d400-private.h:183`, carries the per-resolution `rect_params` → intrinsics).
pub const OP_GETINTCAL: u8 = 0x15;
/// :98 `HWRST = 0x20` — hardware reset. Named so it is never sent by accident: this rung sends nothing.
pub const OP_HWRST: u8 = 0x20;
/// :101 `SET_ADV = 0x2B`.
pub const OP_SET_ADV: u8 = 0x2B;
/// :102 `GET_ADV = 0x2C` — read an advanced-mode table.
pub const OP_GET_ADV: u8 = 0x2C;
/// :110 `GET_EXTRINSICS = 0x53`.
pub const OP_GET_EXTRINSICS: u8 = 0x53;

/// `ds-private.h:171` — `etDepthTableControl = 9`, the table whose first `uint32_t` is `depth_units`
/// (`ds-private.h:488-495`), micrometres per Z16 unit (`src/ds/ds-options.cpp:378-382`).
pub const ET_DEPTH_TABLE_CONTROL: u32 = 9;
/// `ds-private.h:173-178` — `advanced_query_mode`: GET_VAL = 0, GET_MIN = 1, GET_MAX = 2.
pub const ADV_GET_VAL: u32 = 0;
/// `sizeof(depth_table_control)`: five 4-byte fields (`ds-private.h:488-495`).
pub const DEPTH_TABLE_LEN: usize = 20;

/// `src/hw-monitor.h:41` — `IVCAM_MONITOR_MAGIC_NUMBER = 0xcdab`.
pub const HWMON_MAGIC: u16 = 0xCDAB;
/// `hw-monitor.h:47` — `SIZE_OF_HW_MONITOR_HEADER = 4` (the length field + the magic).
pub const HWMON_HEADER: usize = 4;
/// `hw-monitor.h:292` — `size_of_command_without_data = 24` (header + opcode + four params).
pub const HWMON_CMD_LEN: usize = 24;
/// `hw-monitor.h:45` — `HW_MONITOR_BUFFER_SIZE = 1024`: the XU transfer is always this long
/// (`uvc-option.cpp:132,142`).
pub const HWMON_BUF: usize = 1024;
/// `hw-monitor.h:46` — `HW_MONITOR_DATA_SIZE_OFFSET = 1020`: where the XU reply states its data size.
pub const HWMON_DATA_SIZE_OFFSET: usize = 1020;

/// Lay out a data-less HW-monitor command at the front of `out` (`src/hw-monitor.cpp:28-59`): u16 length
/// (of everything after the 4-byte header), u16 magic, u32 opcode, u32 × 4 params, little-endian. The
/// rest of the 1024-byte XU buffer is zero (`uvc-option.cpp:132-133`). Returns the command's length, or
/// 0 if `out` is shorter than one command.
pub fn hwmon_build(op: u8, params: [u32; 4], out: &mut [u8]) -> usize {
    if out.len() < HWMON_CMD_LEN {
        return 0;
    }
    for b in out.iter_mut() {
        *b = 0;
    }
    let len = (HWMON_CMD_LEN - HWMON_HEADER) as u16;
    out[0..2].copy_from_slice(&len.to_le_bytes());
    out[2..4].copy_from_slice(&HWMON_MAGIC.to_le_bytes());
    out[4..8].copy_from_slice(&(op as u32).to_le_bytes());
    for (i, p) in params.iter().enumerate() {
        let o = 8 + i * 4;
        out[o..o + 4].copy_from_slice(&p.to_le_bytes());
    }
    HWMON_CMD_LEN
}

/// The command that reads the depth scale: `GET_ADV(etDepthTableControl, GET_VAL)`
/// (`ds-options.cpp:336-341`).
pub fn hwmon_build_depth_units(out: &mut [u8]) -> usize {
    hwmon_build(OP_GET_ADV, [ET_DEPTH_TABLE_CONTROL, ADV_GET_VAL, 0, 0], out)
}

/// Read the XU reply to a HW-monitor command: the data size is the `u32` at byte 1020, counting the
/// bytes after the 4-byte opcode echo (`uvc-option.cpp:148-151`); the first four bytes echo the opcode
/// sent, anything else is the firmware's error code (`hw-monitor.cpp:112-126`, the check at :176-191).
/// Returns the data bytes' range within `reply`, or `None` for an error echo or a size that does not fit.
pub fn hwmon_reply(reply: &[u8], op: u8) -> Option<core::ops::Range<usize>> {
    if reply.len() < HWMON_BUF {
        return None;
    }
    let le32 = |o: usize| u32::from_le_bytes([reply[o], reply[o + 1], reply[o + 2], reply[o + 3]]);
    if le32(0) != op as u32 {
        return None;
    }
    let n = le32(HWMON_DATA_SIZE_OFFSET) as usize;
    if 4 + n > HWMON_DATA_SIZE_OFFSET {
        return None;
    }
    Some(4..4 + n)
}

/// The depth scale, micrometres per unit, out of a `GET_ADV(etDepthTableControl)` reply.
pub fn hwmon_reply_depth_units(reply: &[u8]) -> Option<u32> {
    let r = hwmon_reply(reply, OP_GET_ADV)?;
    if r.len() < DEPTH_TABLE_LEN {
        return None;
    }
    let o = r.start;
    Some(u32::from_le_bytes([reply[o], reply[o + 1], reply[o + 2], reply[o + 3]]))
}

/// The D400 depth scale when the depth table is not read: 1000 µm = 0.001 m
/// (`src/ds/d400/d400-device.cpp:910-917`).
pub const DEPTH_UNITS_DEFAULT_UM: u32 = 1000;
