//! CHARTER: Kernel — driver (the D435i vendor extension unit as cited data; the probe is Sonde's, R112, CODEX Amendment IV pending)
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
//! three short `GET_CUR` reads this rung makes, through the EP0 seam its caller hands it, and `tests sonde`
//! scores the builders against fixtures. The HW-monitor command channel (`hwmon_*`) is a 1024-byte
//! `SET_CUR` + `GET_CUR` pair on the XU (`src/platform/uvc-option.cpp:132-151`). SONDE3 (LEDGER SR108):
//! `uvc::multi::after_census` sends it when the EP0 buffer it was handed holds [`HWMON_BUF`] bytes
//! ([`hwmon_fits`]) — the xHCI host's 4 KiB page (`uvc::XHCI_EP0_CAP`, XHCIISO B534) does; the EHCI EP0
//! data buffer is 256 bytes (`qh::Buf256`), so on EHCI the channel stays WITHHELD on the wire, by name.

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

/// UVC 1.1 §4.2.1 / Table 4-1: a class-specific request to an interface's entity, host-to-device (`SET_CUR`).
pub const BM_CLASS_INTF_OUT: u8 = 0x21;
/// The same, device-to-host (`GET_CUR` — the three XU reads above use it).
pub const BM_CLASS_INTF_IN: u8 = 0xA1;
/// UVC 1.1 Appendix A.8 (Video Class-Specific Request Codes): `SET_CUR = 0x01`.
pub const RQ_SET_CUR: u8 = 0x01;
/// UVC 1.1 Appendix A.8: `GET_CUR = 0x81`.
pub const RQ_GET_CUR: u8 = 0x81;

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

/// SONDE3 (SR108): can an EP0 data buffer of `cap` bytes carry the HW-monitor exchange? Both halves of the
/// pair are [`HWMON_BUF`] long (`uvc-option.cpp:132,142`), so the buffer must hold the whole transfer.
pub const fn hwmon_fits(cap: usize) -> bool {
    cap >= HWMON_BUF
}

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

// ── SONDE-RECORD (LEDGER SR472): the coefficients table (GETINTCAL) — the depth stream's rectified intrinsics ──────
//
// APPENDED at the file tail and `#[cfg(feature = "sonderec")]`-erased item by item (sonderec implies sonde, this
// file's own gate): no line above moves and a sonde image without sonderec carries this file exactly as before.
// Hardware facts and their readers only (R110), librealsense v2.55.1, file:line:
// - the request: `command cmd(ds::GETINTCAL, static_cast<int>(table_id))` (`src/ds/d400/d400-device.cpp:416`),
//   `coefficients_table_id = 25` (`src/ds/d400/d400-private.h:183`); the reply is `hwmon_reply`'s data bytes.
// - the table: `d400_coefficients_table` (`d400-private.h:219-231`) = `table_header` (`src/ds/ds-private.h:259-266`,
//   16 bytes: version u16 big-endian, table_type u16, table_size u32, param u32, crc32 u32), four `float3x3` (36 bytes
//   each, rsutils `number/float3.h:35-59`), `baseline` f32, `brown_model` u32, `reserved1[88]` — so `rect_params`
//   (`float4` = fx, fy, ppx, ppy as f32, `float3.h:47-49`) starts at byte 256, one per `ds_rect_resolutions`
//   (`ds-private.h:268-286`, 16 entries), then `reserved2[64]`: 576 bytes.
// - the check: `check_calib` (`ds-private.h:302-325`) refuses a table whose header crc32 differs from the CRC-32 of
//   every byte after the 16-byte header (rsutils `src/crc32.cpp` `calc_crc32`: poly 0xEDB88320, init and final
//   inverted — CRC-32/ISO-HDLC, the kernel's `crate::hash::crc32`).
// - the intrinsics: `get_d400_intrinsic_by_resolution_coefficients_table` (`src/ds/d400/d400-private.cpp:148-175`):
//   fx, fy, ppx, ppy = `rect_params[resolution]` as stored; when x == y and z == w (:163-169) the table holds no
//   calibration for it (DS5U) and librealsense substitutes the ideal — `rect_is_ideal` names that case so a caller
//   says "none" instead of writing the ideal as if measured.

/// `d400-private.h:183` — `coefficients_table_id = 25`, GETINTCAL's param1.
#[cfg(feature = "sonderec")]
pub const COEFFICIENTS_TABLE_ID: u32 = 25;
/// `sizeof(table_header)` (`ds-private.h:259-266`): 2 + 2 + 4 + 4 + 4.
#[cfg(feature = "sonderec")]
pub const CALIB_HEADER_LEN: usize = 16;
/// The header's `crc32` field: the last u32 of the 16-byte header.
#[cfg(feature = "sonderec")]
pub const CALIB_CRC_OFFSET: usize = 12;
/// `rect_params`' offset in the table: 16 (header) + 4 x 36 (float3x3) + 4 (baseline) + 4 (brown_model) + 88.
#[cfg(feature = "sonderec")]
pub const RECT_PARAMS_OFFSET: usize = 256;
/// The table's full length: `RECT_PARAMS_OFFSET` + 16 x 16 (`rect_params`) + 64 (`reserved2`).
#[cfg(feature = "sonderec")]
pub const COEFFICIENTS_TABLE_LEN: usize = 576;
/// `enum ds_rect_resolutions` (`ds-private.h:268-286`) in order, width x height; `reserved_1` / `reserved_2` are (0, 0).
#[cfg(feature = "sonderec")]
pub const RECT_RESOLUTIONS: [(u16, u16); 16] = [
    (1920, 1080), (1280, 720), (640, 480), (848, 480), (640, 360), (424, 240), (320, 240), (480, 270),
    (1280, 800), (960, 540), (0, 0), (0, 0), (640, 400), (576, 576), (720, 720), (1152, 1152),
];

/// The command that reads the coefficients table: `GETINTCAL(coefficients_table_id)` (`d400-device.cpp:416`).
#[cfg(feature = "sonderec")]
pub fn hwmon_build_intcal(out: &mut [u8]) -> usize {
    hwmon_build(OP_GETINTCAL, [COEFFICIENTS_TABLE_ID, 0, 0, 0], out)
}

/// The `ds_rect_resolutions` index of a `w` x `h` stream, or `None` when the enum has no such row (librealsense then
/// scales the 1920x1080 row, `d400-private.cpp:186-215` — not done here).
#[cfg(feature = "sonderec")]
pub fn rect_index(w: u16, h: u16) -> Option<usize> {
    if w == 0 || h == 0 {
        return None;
    }
    RECT_RESOLUTIONS.iter().position(|&(rw, rh)| rw == w && rh == h)
}

/// Every `rect_params` row (fx, fy, ppx, ppy) of a coefficients table — `table` is the reply's data bytes (from the
/// header on). Refused by name: `short-table` (fewer bytes than the rows need), `crc` (`check_calib`'s refusal).
#[cfg(feature = "sonderec")]
pub fn coefficients_rects(table: &[u8]) -> Result<[[f32; 4]; 16], &'static str> {
    if table.len() < RECT_PARAMS_OFFSET + 16 * 16 {
        return Err("short-table");
    }
    let le32 = |o: usize| u32::from_le_bytes([table[o], table[o + 1], table[o + 2], table[o + 3]]);
    if le32(CALIB_CRC_OFFSET) != crate::hash::crc32(&table[CALIB_HEADER_LEN..]) {
        return Err("crc");
    }
    let mut rows = [[0f32; 4]; 16];
    for (i, row) in rows.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = f32::from_bits(le32(RECT_PARAMS_OFFSET + i * 16 + j * 4));
        }
    }
    Ok(rows)
}

/// `d400-private.cpp:163-169`: a row with fx == fy and ppx == ppy carries no calibration (librealsense substitutes
/// the ideal 1.5 x width focal length and the centre).
#[cfg(feature = "sonderec")]
pub fn rect_is_ideal(r: &[f32; 4]) -> bool {
    r[0] == r[1] && r[2] == r[3]
}

// ── SONDE-RECORD-IMU (LEDGER SR491): the IMU calibration table (MMER) — the depth-to-IMU extrinsic ─────────────────────
//
// APPENDED at the file tail and `#[cfg(feature = "sonderec")]`-erased item by item, like the GETINTCAL block above: no
// line above moves. Hardware facts and their readers only (R110), librealsense v2.55.1, file:line:
// - the request: `command cmd(MMER, offset, size)` with offset 0 and size `eeprom_imu_table_size`
//   (`src/ds/ds-calib-parsers.cpp:56-62`), `MMER = 0x4F` "MM EEPROM read (from DS5 cache)" (`src/ds/ds-private.h:108`);
//   `eeprom_imu_table_size` = sizeof(union { tm1_eeprom, dm_v2_eeprom }) (`ds-private.h:474-479`) = 520 (tm1_eeprom:
//   16 + 80 + 424; dm_v2_eeprom: 16 + 308 = 324). The reply is `hwmon_reply`'s data bytes.
// - the platform: the table's first u16 is the eeprom id (`ds-calib-parsers.cpp:29`); the D435i's Depth Module V2 is
//   `dm_v2_eeprom_id = 0x0101` (`ds-private.h:483`); any other id is not this layout.
// - the check: `check_calib` (`ds-private.h:302-325`): the header's crc32 against the CRC-32 of every reply byte after
//   the 16-byte header (the same CRC as the coefficients table, `crate::hash::crc32`).
// - the layout: `dm_v2_eeprom` = header (16) + `dm_v2_calib_info` { header (16), `dm_v2_calibration_table` { header (16),
//   extrinsic_valid u8, intrinsic_valid u8, reserved[2], `depth_to_imu` (`extrinsics_table`: float3x3 rotation + float3
//   translation, metres), accel `dm_v2_imu_intrinsic` (float3x3 sensitivity + float3 bias), gyro the same } }
//   (`ds-private.h:436-470`). So, from the reply's data: valid flags @48 / @49, rotation @52, translation @88, accel
//   @100 (sensitivity) / @136 (bias), gyro @148 / @184 (its bias in deg/s, `ds-calib-parsers.cpp:223`).
// - when `extrinsic_valid != 1` librealsense uses CAD values instead (`ds-calib-parsers.cpp:182-192`); this reader
//   reports the flag and never writes the CAD values as measured.

/// `ds-private.h:108` — `MMER = 0x4F`.
#[cfg(feature = "sonderec")]
pub const OP_MMER: u8 = 0x4F;
/// `eeprom_imu_table_size` (`ds-private.h:479`): the bytes MMER is asked for.
#[cfg(feature = "sonderec")]
pub const IMU_EEPROM_LEN: u32 = 520;
/// `ds-private.h:483` — `dm_v2_eeprom_id`.
#[cfg(feature = "sonderec")]
pub const DM_V2_EEPROM_ID: u16 = 0x0101;
/// The dm_v2 field offsets in the reply's data (see the block comment).
#[cfg(feature = "sonderec")]
pub const DM_V2_EXTRINSIC_VALID: usize = 48;
#[cfg(feature = "sonderec")]
pub const DM_V2_INTRINSIC_VALID: usize = 49;
#[cfg(feature = "sonderec")]
pub const DM_V2_DEPTH_TO_IMU: usize = 52;
#[cfg(feature = "sonderec")]
pub const DM_V2_ACCEL: usize = 100;
#[cfg(feature = "sonderec")]
pub const DM_V2_GYRO: usize = 148;
/// The bytes the reader needs: through the gyro bias.
#[cfg(feature = "sonderec")]
pub const DM_V2_NEED: usize = DM_V2_GYRO + 48;

/// The IMU calibration a dm_v2 table carries: the two valid flags, depth-to-IMU (rotation 9 then translation 3, metres)
/// and each sensor's sensitivity (9) then bias (3), as stored.
#[cfg(feature = "sonderec")]
#[derive(Clone, Copy, PartialEq)]
pub struct ImuCal {
    pub extrinsic_valid: bool,
    pub intrinsic_valid: bool,
    pub depth_to_imu: [f32; 12],
    pub accel: [f32; 12],
    pub gyro: [f32; 12],
}

/// The command that reads the IMU table: `MMER(0, eeprom_imu_table_size)` (`ds-calib-parsers.cpp:58-61`).
#[cfg(feature = "sonderec")]
pub fn hwmon_build_mmer(out: &mut [u8]) -> usize {
    hwmon_build(OP_MMER, [0, IMU_EEPROM_LEN, 0, 0], out)
}

/// The dm_v2 IMU table out of MMER's reply data (`table` from its first byte). Refused by name: `short-table`,
/// `not-dm-v2` (another eeprom id), `crc` (`check_calib`'s refusal).
#[cfg(feature = "sonderec")]
pub fn imu_calibration(table: &[u8]) -> Result<ImuCal, &'static str> {
    if table.len() < DM_V2_NEED {
        return Err("short-table");
    }
    if u16::from_le_bytes([table[0], table[1]]) != DM_V2_EEPROM_ID {
        return Err("not-dm-v2");
    }
    let le32 = |o: usize| u32::from_le_bytes([table[o], table[o + 1], table[o + 2], table[o + 3]]);
    if le32(CALIB_CRC_OFFSET) != crate::hash::crc32(&table[CALIB_HEADER_LEN..]) {
        return Err("crc");
    }
    let twelve = |o: usize| {
        let mut v = [0f32; 12];
        for (i, x) in v.iter_mut().enumerate() {
            *x = f32::from_bits(le32(o + 4 * i));
        }
        v
    };
    Ok(ImuCal {
        extrinsic_valid: table[DM_V2_EXTRINSIC_VALID] == 1,
        intrinsic_valid: table[DM_V2_INTRINSIC_VALID] == 1,
        depth_to_imu: twelve(DM_V2_DEPTH_TO_IMU),
        accel: twelve(DM_V2_ACCEL),
        gyro: twelve(DM_V2_GYRO),
    })
}
